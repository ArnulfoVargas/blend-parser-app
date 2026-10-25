// The 3D preview: orbit / pan / zoom with OrbitControls, the painted mesh with
// one colour per leaf triangle, the brush, the measuring tape, the size handles
// and the printer's plate. Picking and painting run in Rust; this file only
// turns pointer positions into rays, recolours what changed and keeps the HTML
// labels over the canvas in step with the camera.
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import type { Hit, PaintResult, Plate, PreviewMesh, Vec3 } from "./api";

// Colours are shown exactly as the filament swatches, without tone mapping.
THREE.ColorManagement.enabled = false;

export type View = "filament" | "texture";
export type Tool = "orbit" | "brush" | "measure" | "size";
export type Axis = 0 | 1 | 2;

export interface BrushHandlers {
  pick(origin: Vec3, dir: Vec3): Promise<Hit | null>;
  paint(origin: Vec3, dir: Vec3): Promise<PaintResult | null>;
  endStroke(): void;
  /** A click on the model with the measuring tool. */
  measure(point: Vec3): void;
  /** How the tape reads two preview points: along any line, the model's height, or one axis. */
  distance(a: Vec3, b: Vec3, kind: "free" | "height" | Axis): string;
  /** Scale factors dragged with the size handles; false when they were refused. */
  resize(factors: Vec3): Promise<boolean>;
}

/** A size handle: one axis, or all three from the corner. */
type Grip = Axis | "all";

/** Blender's axis colours: X red, Y green, Z blue. */
const AXIS_COLOURS = ["--axis-x", "--axis-y", "--axis-z"];

/** The one axis two points differ on, if they differ on exactly one. */
function axisOf(a: Vec3, b: Vec3): Axis | null {
  const moved = [0, 1, 2].filter((i) => Math.abs(a[i] - b[i]) > 1e-6);
  return moved.length === 1 ? (moved[0] as Axis) : null;
}

/** Resolves a CSS colour token (oklch included) through a 1×1 canvas. */
function cssColor(name: string): THREE.Color {
  const ctx = document.createElement("canvas").getContext("2d", { willReadFrequently: true })!;
  ctx.fillStyle = getComputedStyle(document.documentElement).getPropertyValue(name).trim() || "gray";
  ctx.fillRect(0, 0, 1, 1);
  const [r, g, b] = ctx.getImageData(0, 0, 1, 1).data;
  return new THREE.Color(r / 255, g / 255, b / 255);
}

export class Viewer {
  private renderer: THREE.WebGLRenderer;
  private scene = new THREE.Scene();
  private camera = new THREE.PerspectiveCamera(35, 1, 0.1, 10000);
  private controls: OrbitControls;
  private mesh: THREE.Mesh | null = null;
  private colors: Uint8Array | null = null;
  private data: PreviewMesh | null = null;
  private palette: [number, number, number][] = [];
  private view: View = "filament";
  private tool: Tool = "orbit";
  private cursor = new THREE.Group();
  private cursorFill: THREE.MeshBasicMaterial;
  private plate: THREE.Mesh;
  private tape = new THREE.Group();
  private findings = new THREE.Group();
  private tapePoints: Vec3[] = [];
  /** Where the pointer is on the model while measuring. */
  private hover: Vec3 | null = null;
  private hoverDot = new THREE.Group();
  private rubber: THREE.Line;
  private sizeBox: THREE.LineSegments;
  private sizeValues: number[] | null = null;
  /** Shift held: the second measuring point stays on one axis from the first. */
  private shift = false;
  private heightLine: THREE.Line;
  private heightLabel: HTMLDivElement;
  private grips = new THREE.Group();
  private hoverGrip: Grip | null = null;
  private drag: { grip: Grip; pivot: THREE.Vector3; dir: THREE.Vector3; reach: number; factors: THREE.Vector3 } | null = null;
  /** Dragging one axis scales all three, as the size panel's lock does. */
  proportional = true;
  private bed = new THREE.Group();
  private bedPlate: { plate: Plate; scale: number } | null = null;
  private showBed = false;
  private overlay: HTMLDivElement;
  private tapeLabel: HTMLDivElement;
  private sizeChips: HTMLLabelElement[];
  private down: { x: number; y: number } | null = null;
  private ray = new THREE.Raycaster();
  private pending = false;
  private painting = false;
  private frame = 0;
  /** Brush radius in preview units (millimetres before the user's scale). */
  brushRadius = 3;

  constructor(private canvas: HTMLCanvasElement, private handlers: BrushHandlers) {
    this.renderer = new THREE.WebGLRenderer({ canvas, antialias: true, alpha: true });
    this.renderer.outputColorSpace = THREE.LinearSRGBColorSpace;
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    this.camera.up.set(0, 0, 1);
    this.controls = new OrbitControls(this.camera, canvas);
    this.controls.enableDamping = true;
    this.controls.dampingFactor = 0.12;
    this.controls.addEventListener("change", () => this.requestRender());

    this.scene.add(this.findings);
    this.scene.add(new THREE.HemisphereLight(0xffffff, 0x8a8f86, 1.6));
    const key = new THREE.DirectionalLight(0xffffff, 1.6);
    key.position.set(0.6, -1, 1.4);
    this.camera.add(key);
    this.scene.add(this.camera);

    this.plate = new THREE.Mesh(
      new THREE.CircleGeometry(1, 96),
      new THREE.MeshBasicMaterial({ color: cssColor("--color-paper-3"), transparent: true, opacity: 0.85 }),
    );
    this.plate.visible = false;
    this.scene.add(this.plate);

    // Brush footprint: the slot colour at 30%, a light ring and a dark ring so it
    // reads on any surface.
    const flat = (color: THREE.Color, opacity: number) =>
      new THREE.MeshBasicMaterial({ color, depthTest: false, transparent: true, opacity, side: THREE.DoubleSide });
    this.cursorFill = flat(cssColor("--color-accent"), 0.3);
    const parts: [THREE.BufferGeometry, THREE.Material][] = [
      [new THREE.CircleGeometry(1, 64), this.cursorFill],
      [new THREE.RingGeometry(0.9, 1.04, 96), flat(cssColor("--glint"), 1)],
      [new THREE.RingGeometry(0.94, 1, 96), flat(cssColor("--color-ink"), 1)],
    ];
    parts.forEach(([g, m], i) => {
      const mesh = new THREE.Mesh(g, m);
      mesh.renderOrder = 10 + i;
      this.cursor.add(mesh);
    });
    this.cursor.visible = false;
    this.scene.add(this.cursor);
    this.tape.renderOrder = 20;
    this.scene.add(this.tape);

    // Measuring: a dot under the pointer, and a dashed line from the first point to it.
    const solid = (name: string) => new THREE.MeshBasicMaterial({ color: cssColor(name), depthTest: false, transparent: true });
    const ring = new THREE.Mesh(new THREE.SphereGeometry(1.45, 16, 12), solid("--glint"));
    const dot = new THREE.Mesh(new THREE.SphereGeometry(1, 16, 12), solid("--color-accent-deep"));
    ring.renderOrder = 23;
    dot.renderOrder = 24;
    this.hoverDot.add(ring, dot);
    this.hoverDot.visible = false;
    this.scene.add(this.hoverDot);
    this.rubber = new THREE.Line(
      new THREE.BufferGeometry().setFromPoints([new THREE.Vector3(), new THREE.Vector3()]),
      new THREE.LineDashedMaterial({ color: cssColor("--color-accent-deep"), dashSize: 1, gapSize: 0.6, depthTest: false, transparent: true }),
    );
    this.rubber.renderOrder = 22;
    this.rubber.visible = false;
    this.scene.add(this.rubber);
    // With nothing measured yet, the tape shows the model's height.
    this.heightLine = new THREE.Line(
      new THREE.BufferGeometry().setFromPoints([new THREE.Vector3(), new THREE.Vector3()]),
      new THREE.LineBasicMaterial({ color: cssColor("--axis-z"), depthTest: false, transparent: true }),
    );
    this.heightLine.renderOrder = 22;
    this.heightLine.visible = false;
    this.scene.add(this.heightLine);

    // Size grips: a cube past each face, coloured by axis, and one on a top corner for all three.
    [0, 1, 2, 3].forEach((i) => {
      const grip = new THREE.Mesh(
        new THREE.BoxGeometry(1, 1, 1),
        new THREE.MeshBasicMaterial({ color: cssColor(i < 3 ? AXIS_COLOURS[i] : "--color-ink"), depthTest: false, transparent: true }),
      );
      grip.userData.grip = i < 3 ? i : "all";
      grip.renderOrder = 25;
      this.grips.add(grip);
    });
    this.grips.visible = false;
    this.scene.add(this.grips);

    // The model's box while sizing.
    this.sizeBox = new THREE.LineSegments(
      new THREE.EdgesGeometry(new THREE.BoxGeometry(1, 1, 1)),
      new THREE.LineBasicMaterial({ color: cssColor("--color-accent-deep"), depthTest: false, transparent: true, opacity: 0.7 }),
    );
    this.sizeBox.renderOrder = 19;
    this.sizeBox.visible = false;
    this.scene.add(this.sizeBox);
    this.bed.visible = false;
    this.scene.add(this.bed);

    this.overlay = Object.assign(document.createElement("div"), { className: "overlay" });
    canvas.after(this.overlay);
    this.overlay.setAttribute("aria-hidden", "true");
    this.tapeLabel = Object.assign(document.createElement("div"), { className: "tape-label", hidden: true });
    this.heightLabel = Object.assign(document.createElement("div"), { className: "tape-label is-z", hidden: true });
    this.sizeChips = ([0, 1, 2] as Axis[]).map((axis) => {
      const chip = Object.assign(document.createElement("label"), { className: `size-chip is-${"xyz"[axis]}`, hidden: true });
      chip.innerHTML = `<span class="size-chip__axis">${"XYZ"[axis]}</span><span class="size-chip__value"></span><span class="size-chip__unit">mm</span>`;
      return chip;
    });
    this.overlay.append(this.tapeLabel, this.heightLabel, ...this.sizeChips);

    // Shift changes the tape while the pointer rests, so listen to the key itself too.
    const onShift = (e: KeyboardEvent) => {
      if (e.key !== "Shift" || this.shift === e.shiftKey) return;
      this.shift = e.shiftKey;
      if (this.tool === "measure") this.setHover(this.hover);
    };
    window.addEventListener("keydown", onShift);
    window.addEventListener("keyup", onShift);

    new ResizeObserver(() => this.resize()).observe(canvas);
    canvas.addEventListener("pointermove", (e) => this.onMove(e));
    canvas.addEventListener("pointerdown", (e) => this.onDown(e));
    canvas.addEventListener("pointerdown", (e) => this.grab(e), { capture: true });
    window.addEventListener("pointerup", (e) => this.onUp(e));
    canvas.addEventListener("pointerleave", () => {
      this.cursor.visible = false;
      this.setHover(null);
    });
    this.setTool("orbit");
    this.loop();
  }

  private loop = () => {
    requestAnimationFrame(this.loop);
    // Damping needs a few frames after the user lets go.
    if (this.controls.update() || this.frame > 0) {
      this.renderer.render(this.scene, this.camera);
      this.placeOverlay();
      this.frame = Math.max(0, this.frame - 1);
    }
  };

  requestRender() {
    this.frame = 2;
  }

  private resize() {
    const { clientWidth: w, clientHeight: h } = this.canvas;
    if (!w || !h) return;
    this.renderer.setSize(w, h, false);
    this.camera.aspect = w / h;
    this.camera.updateProjectionMatrix();
    this.requestRender();
  }

  /** Replaces the mesh. Keeps the camera unless `frame` is set. */
  load(data: PreviewMesh, frame: boolean) {
    this.data = data;
    if (this.mesh) {
      this.scene.remove(this.mesh);
      this.mesh.geometry.dispose();
    }
    const geo = new THREE.BufferGeometry();
    geo.setAttribute("position", new THREE.BufferAttribute(data.positions, 3));
    this.colors = new Uint8Array(data.labels.length * 9);
    geo.setAttribute("color", new THREE.BufferAttribute(this.colors, 3, true));
    geo.computeVertexNormals();
    geo.computeBoundingSphere();
    geo.computeBoundingBox();
    this.mesh = new THREE.Mesh(geo, new THREE.MeshStandardMaterial({ vertexColors: true, roughness: 0.78, metalness: 0, flatShading: true }));
    this.scene.add(this.mesh);
    this.recolorAll();

    const box = geo.boundingBox!;
    const sphere = geo.boundingSphere!;
    this.plate.position.set(sphere.center.x, sphere.center.y, box.min.z - 0.01);
    this.plate.scale.setScalar(Math.max(box.max.x - box.min.x, box.max.y - box.min.y) * 0.75);
    this.drag = null;
    this.placeGrips();
    this.sizeBox.visible = this.tool === "size";
    this.grips.visible = this.tool === "size";
    this.buildBed();
    if (frame) this.frameAll();
    this.requestRender();
  }

  /** Height of the shown mesh in preview units. */
  modelHeight(): number {
    const b = this.mesh?.geometry.boundingBox;
    return b ? b.max.z - b.min.z : 0;
  }

  frameAll() {
    const s = this.mesh?.geometry.boundingSphere;
    if (!s) return;
    const dist = s.radius / Math.sin(THREE.MathUtils.degToRad(this.camera.fov / 2)) * 1.1;
    const dir = new THREE.Vector3(0.55, -1, 0.62).normalize();
    this.controls.target.copy(s.center);
    this.camera.position.copy(s.center).addScaledVector(dir, dist);
    this.camera.near = dist / 100;
    this.camera.far = dist * 10;
    this.camera.updateProjectionMatrix();
    this.controls.update();
    this.requestRender();
  }

  setPalette(hex: string[]) {
    this.palette = hex.map((h) => {
      const n = parseInt(h.slice(1), 16);
      return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
    });
    this.recolorAll();
  }

  setView(v: View) {
    this.view = v;
    this.recolorAll();
  }

  setTool(t: Tool) {
    this.tool = t;
    // In brush mode the left button paints; the right button orbits and the middle pans.
    this.controls.mouseButtons = t === "brush"
      ? { LEFT: null as unknown as THREE.MOUSE, MIDDLE: THREE.MOUSE.PAN, RIGHT: THREE.MOUSE.ROTATE }
      : { LEFT: THREE.MOUSE.ROTATE, MIDDLE: THREE.MOUSE.DOLLY, RIGHT: THREE.MOUSE.PAN };
    if (t !== "brush") {
      this.cursor.visible = false;
      this.canvas.style.cursor = "";
    }
    if (t !== "measure") this.setHover(null);
    this.sizeBox.visible = t === "size" && this.mesh !== null;
    this.grips.visible = this.sizeBox.visible;
    this.heightLine.visible = false;
    this.requestRender();
  }

  setBrushColor(hex: string) {
    this.cursorFill.color.set(hex);
    this.requestRender();
  }

  /** Recolours the leaves of the given triangles with `slot`. */
  applyPaint(tris: number[], slot: number) {
    if (!this.data || !this.colors) return;
    const { offsets, labels } = this.data;
    for (const t of tris) {
      for (let l = offsets[t]; l < offsets[t + 1]; l++) labels[l] = slot;
    }
    if (this.view === "filament") {
      for (const t of tris) this.colorLeaves(offsets[t], offsets[t + 1]);
      this.mesh!.geometry.getAttribute("color").needsUpdate = true;
    }
    this.requestRender();
  }

  private recolorAll() {
    if (!this.data || !this.colors) return;
    this.colorLeaves(0, this.data.labels.length);
    this.mesh!.geometry.getAttribute("color").needsUpdate = true;
    this.requestRender();
  }

  private colorLeaves(from: number, to: number) {
    const { labels, sourceRgb } = this.data!;
    const c = this.colors!;
    for (let l = from; l < to; l++) {
      let r: number, g: number, b: number;
      if (this.view === "texture") {
        r = sourceRgb[l * 3]; g = sourceRgb[l * 3 + 1]; b = sourceRgb[l * 3 + 2];
      } else {
        [r, g, b] = this.palette[labels[l]] ?? [200, 200, 200];
      }
      for (let k = 0; k < 3; k++) {
        const i = l * 9 + k * 3;
        c[i] = r; c[i + 1] = g; c[i + 2] = b;
      }
    }
  }

  // ---- brush ----------------------------------------------------------------

  private rayFrom(e: PointerEvent): [Vec3, Vec3] {
    const rect = this.canvas.getBoundingClientRect();
    const ndc = new THREE.Vector2(((e.clientX - rect.left) / rect.width) * 2 - 1, -((e.clientY - rect.top) / rect.height) * 2 + 1);
    this.ray.setFromCamera(ndc, this.camera);
    const { origin: o, direction: d } = this.ray.ray;
    return [[o.x, o.y, o.z], [d.x, d.y, d.z]];
  }

  private showCursor(hit: Hit | null) {
    // Over the model the footprint replaces the system cursor.
    this.canvas.style.cursor = this.tool === "brush" ? (hit ? "none" : "crosshair") : "";
    if (!hit) {
      this.cursor.visible = false;
    } else {
      const n = new THREE.Vector3(...hit.normal);
      this.cursor.position.set(...hit.point).addScaledVector(n, this.brushRadius * 0.02);
      this.cursor.quaternion.setFromUnitVectors(new THREE.Vector3(0, 0, 1), n);
      this.cursor.scale.setScalar(this.brushRadius);
      this.cursor.visible = true;
    }
    this.requestRender();
  }

  private async onMove(e: PointerEvent) {
    if (this.drag) return this.dragTo(e);
    if (this.tool === "size" && e.buttons === 0) {
      const grip = this.gripAt(e);
      if (grip !== this.hoverGrip) {
        // Grows under the pointer.
        this.hoverGrip = grip;
        this.placeGrips();
        this.requestRender();
      }
      this.canvas.style.cursor = grip === null ? "" : "grab";
      return;
    }
    // Measuring: the point under the pointer, and the tape stretched to it.
    this.shift = e.shiftKey;
    if (this.tool === "measure" && this.mesh && !this.pending && e.buttons === 0) {
      this.pending = true;
      const [o, d] = this.rayFrom(e);
      try {
        const hit = await this.handlers.pick(o, d);
        this.setHover(hit ? hit.point : null);
      } finally {
        this.pending = false;
      }
      return;
    }
    if (this.tool !== "brush" || !this.mesh || this.pending) return;
    this.pending = true;
    const [o, d] = this.rayFrom(e);
    try {
      if (this.painting) {
        const r = await this.handlers.paint(o, d);
        this.showCursor(r);
      } else if (e.buttons === 0) {
        this.showCursor(await this.handlers.pick(o, d));
      }
    } finally {
      this.pending = false;
    }
  }

  /** Final sizes the labels show, in millimetres; `fmt` writes them in the interface's numbers. */
  setSizeValues(mm: number[] | null, fmt: (n: number) => string = String) {
    this.sizeValues = mm;
    this.sizeFmt = fmt;
    this.showSizes(new THREE.Vector3(1, 1, 1));
  }
  private sizeFmt: (n: number) => string = String;

  private showSizes(f: THREE.Vector3) {
    this.sizeChips.forEach((chip, i) => {
      chip.querySelector(".size-chip__value")!.textContent = this.sizeValues ? this.sizeFmt(this.sizeValues[i] * f.getComponent(i)) : "";
    });
    this.requestRender();
  }

  // ---- size grips -------------------------------------------------------------

  /** The model's box as the preview shows it now, drag included. */
  private liveBox(): THREE.Box3 | null {
    const b = this.mesh?.geometry.boundingBox;
    return b ? b.clone().applyMatrix4(this.mesh!.matrixWorld) : null;
  }

  private placeGrips() {
    this.mesh?.updateMatrixWorld();
    const box = this.liveBox();
    if (!box) return;
    const c = box.getCenter(new THREE.Vector3());
    this.sizeBox.position.copy(c);
    this.sizeBox.scale.copy(box.getSize(new THREE.Vector3()));
    const r = this.markerSize();
    const gap = r * 6;
    const cam = this.camera.position;
    const spots = [
      new THREE.Vector3(box.max.x + gap, c.y, c.z),
      new THREE.Vector3(c.x, box.max.y + gap, c.z),
      new THREE.Vector3(c.x, c.y, box.max.z + gap),
      // The top corner nearest the camera.
      new THREE.Vector3(cam.x < c.x ? box.min.x : box.max.x, cam.y < c.y ? box.min.y : box.max.y, box.max.z),
    ];
    this.grips.children.forEach((g, i) => {
      g.position.copy(spots[i]);
      // The same size on screen at any zoom: 18 px, 24 px under the pointer.
      const px = g.userData.grip === this.hoverGrip || g.userData.grip === this.drag?.grip ? 24 : 18;
      g.scale.setScalar(px * this.unitsPerPixel(g.position));
    });
  }

  /** Preview units one screen pixel spans at `at`. */
  private unitsPerPixel(at: THREE.Vector3) {
    const dist = this.camera.position.distanceTo(at);
    return (2 * dist * Math.tan(THREE.MathUtils.degToRad(this.camera.fov / 2))) / Math.max(1, this.canvas.clientHeight);
  }

  private gripAt(e: PointerEvent): Grip | null {
    if (!this.grips.visible) return null;
    this.rayFrom(e);
    const hit = this.ray.intersectObjects(this.grips.children, false)[0];
    return hit ? (hit.object.userData.grip as Grip) : null;
  }

  /** Starts dragging a grip; the model scales about its base centre, as the size does. */
  private grab(e: PointerEvent) {
    if (this.tool !== "size" || e.button !== 0 || !this.mesh) return;
    const grip = this.gripAt(e);
    if (grip === null) return;
    // Ours, not an orbit.
    e.stopImmediatePropagation();
    e.preventDefault();
    const box = this.mesh.geometry.boundingBox!;
    const c = box.getCenter(new THREE.Vector3());
    this.rayFrom(e);
    const pivot = new THREE.Vector3(c.x, c.y, box.min.z);
    const dir = grip === "all" ? this.grips.children[3].position.clone().sub(pivot).normalize() : new THREE.Vector3().setComponent(grip, 1);
    // Where the pointer grabbed is a factor of 1: the model follows it from there.
    const reach = this.along(pivot, dir);
    if (reach === null || reach <= 1e-6) return;
    this.drag = { grip, pivot, dir, reach, factors: new THREE.Vector3(1, 1, 1) };
    this.controls.enabled = false;
    this.canvas.setPointerCapture(e.pointerId);
    this.canvas.style.cursor = "grabbing";
  }

  /** How far along the line from `pivot` in `dir` the last ray passes closest; null when they run parallel. */
  private along(pivot: THREE.Vector3, dir: THREE.Vector3): number | null {
    const { origin: o, direction: u } = this.ray.ray;
    const w0 = pivot.clone().sub(o);
    const b = dir.dot(u);
    const denom = 1 - b * b;
    return denom < 1e-4 ? null : (b * u.dot(w0) - dir.dot(w0)) / denom;
  }

  private dragTo(e: PointerEvent) {
    const d = this.drag!;
    this.rayFrom(e);
    const t = this.along(d.pivot, d.dir);
    if (t === null) return;
    const k = THREE.MathUtils.clamp(t / d.reach, 0.05, 20);
    const all = d.grip === "all" || this.proportional;
    d.factors.set(1, 1, 1);
    if (all) d.factors.setScalar(k);
    else d.factors.setComponent(d.grip as Axis, k);
    // Scale about the pivot: x' = p + (x - p) f.
    const m = this.mesh!;
    m.scale.copy(d.factors);
    m.position.copy(d.pivot).multiply(new THREE.Vector3(1, 1, 1).sub(d.factors));
    this.placeGrips();
    this.showSizes(d.factors);
  }

  private async release() {
    const d = this.drag;
    if (!d) return;
    this.controls.enabled = true;
    this.canvas.style.cursor = "";
    const f = d.factors.toArray() as Vec3;
    if (f.every((v) => Math.abs(v - 1) < 1e-4)) {
      this.drag = null;
      return this.placeGrips();
    }
    if (!(await this.handlers.resize(f))) {
      // Refused: back to the size the model has.
      this.drag = null;
      this.mesh?.scale.set(1, 1, 1);
      this.mesh?.position.set(0, 0, 0);
      this.placeGrips();
      this.showSizes(new THREE.Vector3(1, 1, 1));
    }
  }

  /** The printer's plate under the model; `scale` turns preview units into millimetres. */
  setPlate(plate: Plate, scale: number) {
    this.bedPlate = { plate, scale };
    this.buildBed();
  }

  /** Shows the printer's plate, or just a soft shadow under the model. */
  setBedVisible(on: boolean) {
    this.showBed = on;
    this.buildBed();
  }

  private buildBed() {
    this.bed.clear();
    const box = this.mesh?.geometry.boundingBox;
    const on = this.showBed && this.bedPlate !== null && box !== undefined && box !== null;
    this.bed.visible = on;
    this.plate.visible = !on && this.mesh !== null;
    if (!on) return this.requestRender();
    const { plate, scale } = this.bedPlate!;
    // The model's centre sits on the plate's centre, as the 3MF places it.
    const c = box.getCenter(new THREE.Vector3());
    const mx = (plate.model.x0 + plate.model.x1) / 2;
    const my = (plate.model.y0 + plate.model.y1) / 2;
    const z = box.min.z - 0.02 / scale;
    const flat = (r: { x0: number; y0: number; x1: number; y1: number }, color: string, opacity: number, lift: number) => {
      const w = (r.x1 - r.x0) / scale;
      const d = (r.y1 - r.y0) / scale;
      const mesh = new THREE.Mesh(
        new THREE.PlaneGeometry(Math.max(w, 1e-3), Math.max(d, 1e-3)),
        new THREE.MeshBasicMaterial({ color: cssColor(color), transparent: true, opacity, depthWrite: false, side: THREE.DoubleSide }),
      );
      mesh.position.set(c.x + ((r.x0 + r.x1) / 2 - mx) / scale, c.y + ((r.y0 + r.y1) / 2 - my) / scale, z + lift / scale);
      mesh.renderOrder = 1;
      this.bed.add(mesh);
      const edge = new THREE.LineSegments(
        new THREE.EdgesGeometry(mesh.geometry),
        new THREE.LineBasicMaterial({ color: cssColor(color === "--color-paper-3" ? "--color-muted" : color), transparent: true, opacity: 0.9 }),
      );
      edge.position.copy(mesh.position);
      this.bed.add(edge);
    };
    flat(plate.area, "--color-paper-3", 0.9, 0);
    plate.excluded.forEach((e) => flat(e, "--color-ink-2", 0.35, 0.01));
    if (plate.tower) {
      const g = plate.prefs.towerGapMm;
      const t = plate.tower;
      flat({ x0: t.x0 - g, y0: t.y0 - g, x1: t.x1 + g, y1: t.y1 + g }, "--color-warn", 0.25, 0.02);
      flat(t, "--color-warn", 0.85, 0.03);
    }
    this.requestRender();
  }

  private markerSize() {
    return (this.mesh?.geometry.boundingSphere?.radius ?? 50) * 0.012;
  }

  /** The second point as it would be placed: with Shift, on the first point's axis it moves most along. */
  private target(): Vec3 | null {
    const p = this.hover;
    const from = this.tapePoints.length === 1 ? this.tapePoints[0] : null;
    if (!p || !from || !this.shift) return p;
    const delta = p.map((v, i) => v - from[i]);
    const axis = [0, 1, 2].reduce((a, i) => (Math.abs(delta[i]) > Math.abs(delta[a]) ? i : a), 0);
    return from.map((v, i) => (i === axis ? p[i] : v)) as Vec3;
  }

  private setHover(p: Vec3 | null) {
    this.hover = p;
    const at = this.target();
    this.hoverDot.visible = at !== null;
    const r = this.markerSize();
    if (at) {
      this.hoverDot.position.set(...at);
      this.hoverDot.scale.setScalar(r);
    }
    // From the first point to the pointer while the second is still to come.
    const from = this.tapePoints.length === 1 ? this.tapePoints[0] : null;
    this.rubber.visible = at !== null && from !== null;
    if (at && from) {
      this.rubber.geometry.setFromPoints([new THREE.Vector3(...from), new THREE.Vector3(...at)]);
      const mat = this.rubber.material as THREE.LineDashedMaterial;
      const axis = axisOf(from, at);
      mat.color.copy(cssColor(axis === null ? "--color-accent-deep" : AXIS_COLOURS[axis]));
      mat.dashSize = r * 2;
      mat.gapSize = r * 1.2;
      this.rubber.computeLineDistances();
    }
    this.requestRender();
  }

  /** Puts the HTML labels where their 3D anchors land on screen. */
  private placeOverlay() {
    const w = this.canvas.clientWidth;
    const h = this.canvas.clientHeight;
    const screen = (v: THREE.Vector3) => {
      const p = v.clone().project(this.camera);
      return p.z > 1 ? null : { x: ((p.x + 1) / 2) * w, y: ((1 - p.y) / 2) * h };
    };
    const put = (el: HTMLElement, at: { x: number; y: number } | null) => {
      el.hidden = at === null;
      if (at) el.style.transform = `translate(${at.x}px, ${at.y}px) translate(-50%, -50%)`;
    };

    const pts = this.tapePoints;
    const target = this.target();
    const pair = pts.length === 2 ? [pts[0], pts[1]] : pts.length === 1 && target ? [pts[0], target] : null;
    if (pair) {
      const [a, b] = pair.map((p) => new THREE.Vector3(...p));
      const axis = axisOf(pair[0], pair[1]);
      this.tapeLabel.textContent = this.handlers.distance(pair[0], pair[1], axis ?? "free");
      this.tapeLabel.className = `tape-label${pts.length === 1 ? " is-live" : ""}${axis === null ? "" : ` is-${"xyz"[axis]}`}`;
      put(this.tapeLabel, screen(a.add(b).multiplyScalar(0.5)));
    } else put(this.tapeLabel, null);

    // Nothing measured yet: the height, on the vertical edge nearest the camera.
    const mbox = this.mesh?.geometry.boundingBox;
    const showHeight = this.tool === "measure" && pts.length === 0 && mbox !== null && mbox !== undefined;
    this.heightLine.visible = showHeight;
    if (showHeight) {
      const cam = this.camera.position;
      const c = mbox.getCenter(new THREE.Vector3());
      const x = cam.x < c.x ? mbox.min.x : mbox.max.x;
      const y = cam.y < c.y ? mbox.min.y : mbox.max.y;
      const lo: Vec3 = [x, y, mbox.min.z];
      const hi: Vec3 = [x, y, mbox.max.z];
      this.heightLine.geometry.setFromPoints([new THREE.Vector3(...lo), new THREE.Vector3(...hi)]);
      this.heightLabel.textContent = this.handlers.distance(lo, hi, "height");
      put(this.heightLabel, screen(new THREE.Vector3(x, y, c.z)));
    } else put(this.heightLabel, null);

    const box = this.liveBox();
    if (this.tool !== "size" || !box || !this.sizeValues) {
      this.sizeChips.forEach((chip) => put(chip, null));
      return;
    }
    if (!this.drag) this.placeGrips();
    // Each handle sits on the edge of its axis nearest the camera.
    const cam = this.camera.position;
    const near = (axis: number) =>
      (Math.abs(cam.getComponent(axis) - box.min.getComponent(axis)) < Math.abs(cam.getComponent(axis) - box.max.getComponent(axis)) ? box.min : box.max).getComponent(axis);
    const c = box.getCenter(new THREE.Vector3());
    const edges = [
      new THREE.Vector3(c.x, near(1), box.min.z),
      new THREE.Vector3(near(0), c.y, box.min.z),
      new THREE.Vector3(near(0), near(1), c.z),
    ];
    this.sizeChips.forEach((chip, i) => put(chip, screen(edges[i])));
  }

  /** Bounded diagnostic markers; final-to-preview scaling is supplied by the caller. */
  setFindings(points: Vec3[], values?: (number | null)[], threshold = 0) {
    for (const child of this.findings.children) {
      if (child instanceof THREE.Points) { child.geometry.dispose(); (child.material as THREE.Material).dispose(); }
    }
    this.findings.clear();
    if (points.length) {
      const geometry = new THREE.BufferGeometry().setFromPoints(points.map(p => new THREE.Vector3(...p)));
      const colors = points.flatMap((_, i) => values ? values[i] === null ? [0.6, 0.6, 0.6] : values[i]! < threshold ? [1, 0.15, 0.05] : [0.05, 0.75, 0.9] : [1, 0.3, 0.05]);
      geometry.setAttribute("color", new THREE.Float32BufferAttribute(colors, 3));
      const markers = new THREE.Points(geometry, new THREE.PointsMaterial({ size: 8, sizeAttenuation: false, vertexColors: true, depthTest: false }));
      markers.renderOrder = 30; this.findings.add(markers);
      if (!values) this.controls.target.set(...points[0]);
    }
    this.requestRender();
  }

  /** Shows the measured points (preview units) and the line between them. */
  setMeasure(points: Vec3[]) {
    this.tapePoints = points;
    this.tape.clear();
    const r = this.markerSize();
    const mat = (name: string) => new THREE.MeshBasicMaterial({ color: cssColor(name), depthTest: false, transparent: true });
    const dot = new THREE.SphereGeometry(r, 16, 12);
    const ring = new THREE.SphereGeometry(r * 1.45, 16, 12);
    for (const p of points) {
      const outer = new THREE.Mesh(ring, mat("--glint"));
      const inner = new THREE.Mesh(dot, mat("--color-ink"));
      outer.position.set(...p);
      inner.position.set(...p);
      outer.renderOrder = 20;
      inner.renderOrder = 21;
      this.tape.add(outer, inner);
    }
    if (points.length === 2) {
      const geo = new THREE.BufferGeometry().setFromPoints(points.map((p) => new THREE.Vector3(...p)));
      const line = new THREE.Line(geo, new THREE.LineBasicMaterial({ color: cssColor("--color-ink"), depthTest: false, transparent: true }));
      line.renderOrder = 22;
      this.tape.add(line);
    }
    this.setHover(this.tool === "measure" ? this.hover : null);
  }

  private onDown(e: PointerEvent) {
    if (this.tool === "measure" && e.button === 0) this.down = { x: e.clientX, y: e.clientY };
    if (this.drag) return;
    if (this.tool !== "brush" || e.button !== 0 || !this.mesh) return;
    this.painting = true;
    this.canvas.setPointerCapture(e.pointerId);
    this.pending = false;
    void this.onMove(e);
  }

  private onUp(e: PointerEvent) {
    if (this.drag) return void this.release();
    // A click, not a drag to orbit, places a measuring point.
    const d = this.down;
    this.down = null;
    if (this.tool === "measure" && d && this.mesh && e.target === this.canvas && Math.hypot(e.clientX - d.x, e.clientY - d.y) < 5) {
      this.shift = e.shiftKey;
      // With Shift the point is where the tape shows it, on the axis; otherwise on the model.
      const placed = this.shift && this.tapePoints.length === 1 ? this.target() : null;
      if (placed) this.handlers.measure(placed);
      else {
        const [o, dir] = this.rayFrom(e);
        void this.handlers.pick(o, dir).then((hit) => hit && this.handlers.measure(hit.point));
      }
    }
    if (!this.painting) return;
    this.painting = false;
    this.handlers.endStroke();
  }
}
