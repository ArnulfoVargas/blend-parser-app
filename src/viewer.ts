// The 3D preview: orbit / pan / zoom with OrbitControls, the painted mesh with
// one colour per leaf triangle, the brush and the measuring tape. Picking and
// painting run in Rust; this file only turns pointer positions into rays and
// recolours what changed.
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import type { Hit, PaintResult, PreviewMesh, Vec3 } from "./api";

// Colours are shown exactly as the filament swatches, without tone mapping.
THREE.ColorManagement.enabled = false;

export type View = "filament" | "texture";
export type Tool = "orbit" | "brush" | "measure";

export interface BrushHandlers {
  pick(origin: Vec3, dir: Vec3): Promise<Hit | null>;
  paint(origin: Vec3, dir: Vec3): Promise<PaintResult | null>;
  endStroke(): void;
  /** A click on the model with the measuring tool. */
  measure(point: Vec3): void;
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

    new ResizeObserver(() => this.resize()).observe(canvas);
    canvas.addEventListener("pointermove", (e) => this.onMove(e));
    canvas.addEventListener("pointerdown", (e) => this.onDown(e));
    window.addEventListener("pointerup", (e) => this.onUp(e));
    canvas.addEventListener("pointerleave", () => {
      this.cursor.visible = false;
      this.requestRender();
    });
    this.setTool("orbit");
    this.loop();
  }

  private loop = () => {
    requestAnimationFrame(this.loop);
    // Damping needs a few frames after the user lets go.
    if (this.controls.update() || this.frame > 0) {
      this.renderer.render(this.scene, this.camera);
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
    this.plate.visible = true;
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

  /** Shows the measured points (preview units) and the line between them. */
  setMeasure(points: Vec3[]) {
    this.tape.clear();
    const r = (this.mesh?.geometry.boundingSphere?.radius ?? 50) * 0.012;
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
    this.requestRender();
  }

  private onDown(e: PointerEvent) {
    if (this.tool === "measure" && e.button === 0) this.down = { x: e.clientX, y: e.clientY };
    if (this.tool !== "brush" || e.button !== 0 || !this.mesh) return;
    this.painting = true;
    this.canvas.setPointerCapture(e.pointerId);
    this.pending = false;
    void this.onMove(e);
  }

  private onUp(e: PointerEvent) {
    // A click, not a drag to orbit, places a measuring point.
    const d = this.down;
    this.down = null;
    if (this.tool === "measure" && d && this.mesh && e.target === this.canvas && Math.hypot(e.clientX - d.x, e.clientY - d.y) < 5) {
      const [o, dir] = this.rayFrom(e);
      void this.handlers.pick(o, dir).then((hit) => hit && this.handlers.measure(hit.point));
    }
    if (!this.painting) return;
    this.painting = false;
    this.handlers.endStroke();
  }
}
