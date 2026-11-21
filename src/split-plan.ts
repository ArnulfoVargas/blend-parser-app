import * as THREE from "three";
import type { Vec3 } from "./api";

export type Cuts = [number[], number[], number[]];

/** `count` cuts spreading `lo..hi` into equal pieces (as the backend proposes them). */
export function evenCuts(lo: number, hi: number, count: number): number[] {
  return Array.from({ length: count }, (_, i) => lo + ((hi - lo) * (i + 1)) / (count + 1));
}

/** The planes a split will cut along, drawn across the model in their axis' colour. */
export class CutPlanes {
  readonly group = new THREE.Group();
  constructor(private colours: [THREE.Color, THREE.Color, THREE.Color]) {
    this.group.visible = false;
  }

  show(cuts: Cuts | null, box: { min: Vec3; max: Vec3 } | null) {
    for (const child of [...this.group.children]) {
      (child as THREE.Mesh).geometry?.dispose();
      this.group.remove(child);
    }
    this.group.visible = cuts !== null && box !== null;
    if (!cuts || !box) return;
    const size = box.max.map((v, i) => v - box.min[i]);
    const centre = box.max.map((v, i) => (v + box.min[i]) / 2);
    cuts.forEach((at, axis) => at.forEach((value) => {
      // A sheet a little larger than the model's box across the other two axes.
      const [u, v] = [(axis + 1) % 3, (axis + 2) % 3];
      const geometry = new THREE.PlaneGeometry(size[u] * 1.1 + 2, size[v] * 1.1 + 2);
      const sheet = new THREE.Mesh(geometry, new THREE.MeshBasicMaterial({ color: this.colours[axis], transparent: true, opacity: 0.16, depthWrite: false, side: THREE.DoubleSide }));
      const rim = new THREE.LineSegments(new THREE.EdgesGeometry(geometry), new THREE.LineBasicMaterial({ color: this.colours[axis], depthTest: false, transparent: true, opacity: 0.9 }));
      const place = new THREE.Group();
      place.add(sheet, rim);
      // The plane geometry lies on XY; its local X goes along axis u, Y along v, Z along the cut axis.
      const basis = new THREE.Matrix4().makeBasis(
        new THREE.Vector3().setComponent(u, 1), new THREE.Vector3().setComponent(v, 1), new THREE.Vector3().setComponent(axis, 1));
      place.applyMatrix4(basis);
      const position = [...centre] as Vec3;
      position[axis] = value;
      place.position.set(...position);
      sheet.renderOrder = 7;
      rim.renderOrder = 17;
      this.group.add(place);
    }));
  }
}
