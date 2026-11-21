import * as THREE from "three";
import type { Vec3 } from "./api";

export type ScreenPoint = { x: number; y: number };
export type ViewportRect = { left: number; top: number; width: number; height: number };
export type LinePlane = { point: Vec3; rotation: Vec3 };
export type LineError = "tooShort" | "misses";

/** Both rays lie in the cut plane, including off-centre perspective strokes. */
export function planeFromStroke(
  a: ScreenPoint, b: ScreenPoint, rect: ViewportRect, camera: THREE.Camera, centre: THREE.Vector3,
): LinePlane | LineError {
  if (rect.width <= 0 || rect.height <= 0 || Math.hypot(b.x - a.x, b.y - a.y) < 12) return "tooShort";
  camera.updateMatrixWorld();
  const ray = (p: ScreenPoint) => {
    const caster = new THREE.Raycaster();
    caster.setFromCamera(new THREE.Vector2(2 * (p.x - rect.left) / rect.width - 1, 1 - 2 * (p.y - rect.top) / rect.height), camera);
    return caster.ray;
  };
  const first = ray(a), second = ray(b);
  const perspective = (camera as THREE.PerspectiveCamera).isPerspectiveCamera;
  const normal = perspective ? first.direction.clone().cross(second.direction)
    : second.origin.clone().sub(first.origin).cross(first.direction);
  if (normal.lengthSq() < 1e-16) return "tooShort";
  normal.normalize();
  const plane = new THREE.Plane().setFromNormalAndCoplanarPoint(normal, first.origin);
  const point = plane.projectPoint(centre, new THREE.Vector3());
  const rotation = new THREE.Euler().setFromQuaternion(new THREE.Quaternion().setFromUnitVectors(new THREE.Vector3(0, 0, 1), normal), "XYZ");
  return { point: point.toArray() as Vec3, rotation: [rotation.x, rotation.y, rotation.z].map(THREE.MathUtils.radToDeg) as Vec3 };
}

/** Tests actual triangles, rather than their bounding box or the infinite stroke. */
export function strokeCrossesMesh(
  a: ScreenPoint, b: ScreenPoint, rect: ViewportRect, camera: THREE.Camera, plane: LinePlane,
  positions: ArrayLike<number>, ranges: { previewStart: number; previewEnd: number }[],
): boolean {
  const normal = new THREE.Vector3(0, 0, 1).applyEuler(new THREE.Euler(...plane.rotation.map(THREE.MathUtils.degToRad) as Vec3, "XYZ"));
  const cut = new THREE.Plane().setFromNormalAndCoplanarPoint(normal, new THREE.Vector3(...plane.point));
  const delta = new THREE.Vector2(b.x - a.x, b.y - a.y), lengthSq = delta.lengthSq();
  const vertex = (i: number) => new THREE.Vector3(positions[i], positions[i + 1], positions[i + 2]);
  const bounds = new THREE.Box3();
  for (const range of ranges) for (let i = range.previewStart * 9; i < range.previewEnd * 9; i += 3) bounds.expandByPoint(vertex(i));
  if (bounds.isEmpty()) return false;
  const eps = Math.max(bounds.getSize(new THREE.Vector3()).length() * 1e-7, 1e-7);
  for (const range of ranges) {
    let low = false, high = false, overlaps = false;
    for (let i = range.previewStart * 9; i < range.previewEnd * 9; i += 9) {
      const vertices = [vertex(i), vertex(i + 3), vertex(i + 6)];
      const distances = vertices.map((v) => cut.distanceToPoint(v));
      low ||= distances.some((d) => d < -eps);
      high ||= distances.some((d) => d > eps);
      const hits: THREE.Vector3[] = [];
      for (let k = 0; k < 3; k++) {
        const next = (k + 1) % 3, d = distances[k], other = distances[next];
        if (Math.abs(d) <= eps) hits.push(vertices[k]);
        if (d * other < 0) hits.push(vertices[k].clone().lerp(vertices[next], d / (d - other)));
      }
      const parameters = hits.filter((v) => v.clone().sub(camera.position).dot(camera.getWorldDirection(new THREE.Vector3())) > 0).map((v) => {
        const p = v.clone().project(camera);
        return ((rect.left + (p.x + 1) * rect.width / 2 - a.x) * delta.x
          + (rect.top + (1 - p.y) * rect.height / 2 - a.y) * delta.y) / lengthSq;
      });
      if (parameters.length && Math.max(...parameters) >= 0 && Math.min(...parameters) <= 1) overlaps = true;
      if (low && high && overlaps) return true;
    }
  }
  return false;
}

export function snappedEnd(a: ScreenPoint, b: ScreenPoint, snap: boolean): ScreenPoint {
  if (!snap) return b;
  const length = Math.hypot(b.x - a.x, b.y - a.y);
  const angle = Math.round(Math.atan2(b.y - a.y, b.x - a.x) / (Math.PI / 12)) * Math.PI / 12;
  return { x: a.x + Math.cos(angle) * length, y: a.y + Math.sin(angle) * length };
}

type Stroke = { start: ScreenPoint; end: ScreenPoint };
/** A stroke remains editable until the camera or the plane changes. */
export class SectionLine {
  private armed = false;
  private stroke: Stroke | null = null;
  private saved: Stroke | null = null;
  private drag: { id: number; kind: "new" | "start" | "end" | "move"; from: ScreenPoint; base: Stroke } | null = null;
  private svg: SVGSVGElement;
  private line: SVGLineElement;
  private handles: SVGCircleElement[];

  constructor(private canvas: HTMLCanvasElement, private actions: {
    state(active: boolean): void;
    finish(a: ScreenPoint, b: ScreenPoint): boolean;
    cancel(): void;
    begin?(): void;
    preview?(a: ScreenPoint, b: ScreenPoint): void;
  }) {
    this.svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    this.svg.classList.add("section-line");
    this.svg.setAttribute("aria-hidden", "true");
    this.line = document.createElementNS("http://www.w3.org/2000/svg", "line");
    this.handles = [0, 1, 2].map(() => document.createElementNS("http://www.w3.org/2000/svg", "circle"));
    this.handles.forEach((handle, i) => handle.setAttribute("r", String(i === 2 ? 5 : 7)));
    this.svg.append(this.line, ...this.handles);
    this.svg.style.display = "none";
    canvas.after(this.svg);
    canvas.addEventListener("pointerdown", (e) => {
      if (!this.armed && e.button !== 0) return;
      const p = { x: e.clientX, y: e.clientY };
      const kind = !this.armed ? this.handleAt(p) : "new";
      if (!kind) return;
      e.stopImmediatePropagation();
      e.preventDefault();
      if (e.button !== 0 || this.drag) return;
      if (!this.armed) this.arm();
      if (kind === "new") this.stroke = { start: p, end: p };
      this.drag = { id: e.pointerId, kind, from: p, base: structuredClone(this.stroke!) };
      canvas.setPointerCapture(e.pointerId);
      this.draw();
    }, { capture: true });
    canvas.addEventListener("pointermove", (e) => {
      if (!this.armed) {
        if (this.handleAt({ x: e.clientX, y: e.clientY })) { e.stopImmediatePropagation(); canvas.style.cursor = "grab"; }
        return;
      }
      e.stopImmediatePropagation();
      if (!this.drag || this.drag.id !== e.pointerId) return;
      this.update(e);
    }, { capture: true });
    window.addEventListener("pointerup", (e) => {
      if (!this.armed || this.drag?.id !== e.pointerId) return;
      e.stopImmediatePropagation();
      this.update(e);
      const drag = this.drag;
      this.drag = null;
      if (this.actions.finish(this.stroke!.start, this.stroke!.end)) this.stop(true);
      else if (drag.kind !== "new") this.cancel();
      else { this.stroke = null; this.hide(); }
      if (canvas.hasPointerCapture(e.pointerId)) canvas.releasePointerCapture(e.pointerId);
    }, { capture: true });
    canvas.addEventListener("pointercancel", () => this.cancel());
    canvas.addEventListener("lostpointercapture", () => { if (this.drag) this.cancel(); });
    window.addEventListener("blur", () => this.cancel());
    window.addEventListener("keydown", (e) => {
      if (e.key !== "Escape" || !this.armed) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      this.cancel();
    }, { capture: true });
  }

  get active() { return this.armed; }

  start() {
    if (this.armed) return;
    this.arm();
    this.stroke = null;
    this.hide();
  }

  private arm() {
    this.saved = this.stroke && structuredClone(this.stroke);
    this.armed = true;
    this.actions.begin?.();
    this.actions.state(true);
  }

  cancel() {
    if (!this.armed) return;
    this.stroke = this.saved;
    this.stop(true);
    this.actions.cancel();
  }

  clear() {
    if (this.armed) return;
    this.stroke = null;
    this.hide();
  }

  private stop(preserve = false) {
    const id = this.drag?.id;
    this.drag = null;
    this.armed = false;
    if (preserve && this.stroke) this.draw();
    else this.hide();
    if (id !== undefined && this.canvas.hasPointerCapture(id)) this.canvas.releasePointerCapture(id);
    this.actions.state(false);
  }

  private hide() { this.svg.style.display = "none"; }

  private handleAt(p: ScreenPoint): "start" | "end" | "move" | null {
    const s = this.stroke;
    if (!s) return null;
    const distance = (q: ScreenPoint) => Math.hypot(p.x - q.x, p.y - q.y);
    return distance(s.start) < 12 ? "start" : distance(s.end) < 12 ? "end"
      : distance({ x: (s.start.x + s.end.x) / 2, y: (s.start.y + s.end.y) / 2 }) < 12 ? "move" : null;
  }

  private update(e: PointerEvent) {
    const drag = this.drag!, p = { x: e.clientX, y: e.clientY };
    if (drag.kind === "move") {
      const move = (q: ScreenPoint) => ({ x: q.x + p.x - drag.from.x, y: q.y + p.y - drag.from.y });
      this.stroke = { start: move(drag.base.start), end: move(drag.base.end) };
    } else if (drag.kind === "start") this.stroke = { start: snappedEnd(drag.base.end, p, e.shiftKey), end: drag.base.end };
    else this.stroke = { start: drag.base.start, end: snappedEnd(drag.base.start, p, e.shiftKey) };
    this.draw();
    this.actions.preview?.(this.stroke.start, this.stroke.end);
  }

  private draw() {
    if (!this.stroke) return this.hide();
    const rect = this.canvas.getBoundingClientRect(), { start, end } = this.stroke;
    this.svg.style.display = "";
    this.svg.setAttribute("viewBox", `0 0 ${rect.width} ${rect.height}`);
    for (const [name, value] of Object.entries({ x1: start.x - rect.left, y1: start.y - rect.top, x2: end.x - rect.left, y2: end.y - rect.top })) this.line.setAttribute(name, String(value));
    [start, end, { x: (start.x + end.x) / 2, y: (start.y + end.y) / 2 }].forEach((p, i) => {
      this.handles[i].setAttribute("cx", String(p.x - rect.left)); this.handles[i].setAttribute("cy", String(p.y - rect.top));
    });
  }
}
