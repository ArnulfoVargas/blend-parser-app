import * as THREE from "three";
import type { Vec3 } from "./api";
import { cutExtent, type CutExtent } from "./cut-extent";
export { cutExtent, type CutExtent } from "./cut-extent";

/** Shares the painted geometry; discarded surfaces retain their original colours. */
export class CutPreview {
  readonly group = new THREE.Group();
  private ghost: THREE.Mesh | null = null;
  private removed = new THREE.MeshBasicMaterial({ vertexColors: true, side: THREE.DoubleSide, transparent: true, opacity: .22, depthWrite: false });
  private invisible = new THREE.MeshBasicMaterial({ visible: false });
  private contour: THREE.LineSegments;
  private selection = new THREE.Group();
  private worker: Worker | null = null;
  private source: ArrayLike<number> | null = null;
  private generation = 0;
  private running = false;
  private next: (() => void) | null = null;
  private receive: ((extent: CutExtent) => void) | null = null;

  constructor(colour: THREE.Color) {
    this.contour = new THREE.LineSegments(new THREE.BufferGeometry(), new THREE.LineBasicMaterial({ color: colour, depthTest: false, transparent: true }));
    this.contour.renderOrder = 15;
    this.group.add(this.contour, this.selection);
    this.group.visible = false;
    if (typeof Worker !== "undefined") {
      this.worker = new Worker(new URL("./cut-preview-worker.ts", import.meta.url), { type: "module" });
      this.worker.onmessage = (event) => {
        const result = event.data;
        const box = (bounds: [Vec3, Vec3] | null) => bounds ? new THREE.Box3(new THREE.Vector3(...bounds[0]), new THREE.Vector3(...bounds[1])) : new THREE.Box3();
        this.running = false;
        if (result.id === this.generation) this.receive?.({ low: box(result.low), high: box(result.high), boxes: result.boxes.map(box), segments: result.segments });
        const next = this.next; this.next = null; next?.();
      };
    }
  }

  attach(geometry: THREE.BufferGeometry) {
    this.invalidate();
    if (this.ghost) this.group.remove(this.ghost);
    this.ghost = new THREE.Mesh(geometry, [this.removed, this.invisible]);
    this.ghost.renderOrder = 3;
    this.group.add(this.ghost);
  }

  invalidate() { this.source = null; this.generation++; this.next = null; }

  calculate(positions: ArrayLike<number>, ranges: { previewStart: number; previewEnd: number }[], point: Vec3, normal: Vec3, receive: (extent: CutExtent) => void) {
    if (!this.worker) { receive(cutExtent(positions, ranges, point, normal)); return; }
    if (this.source !== positions) {
      const copy = new Float32Array(positions);
      this.worker.postMessage({ positions: copy }, [copy.buffer]);
      this.source = positions;
    }
    const id = ++this.generation;
    this.receive = receive;
    const send = () => { this.running = true; this.worker!.postMessage({ id, ranges, point, normal }); };
    if (this.running) this.next = send; else send();
  }

  show(plane: THREE.Plane | null, both: boolean, extent: CutExtent | null, selected: boolean) {
    this.group.visible = plane !== null;
    if (!plane) { this.generation++; this.next = null; return; }
    this.removed.clippingPlanes = [plane.clone().negate()];
    this.removed.needsUpdate = true;
    this.removed.opacity = both ? .85 : .22;
    this.contour.visible = extent !== null;
    this.selection.visible = extent !== null;
    if (!extent) return;
    this.contour.geometry.dispose();
    this.contour.geometry = new THREE.BufferGeometry().setAttribute("position", new THREE.BufferAttribute(extent.segments, 3));
    for (const child of this.selection.children) {
      const helper = child as THREE.Box3Helper;
      helper.geometry.dispose(); (helper.material as THREE.Material).dispose();
    }
    this.selection.clear();
    if (selected) for (const box of extent.boxes) {
      const helper = new THREE.Box3Helper(box, (this.contour.material as THREE.LineBasicMaterial).color);
      (helper.material as THREE.Material).depthTest = false;
      helper.renderOrder = 14;
      this.selection.add(helper);
    }
  }
}
