import * as THREE from "three";
import type { Vec3 } from "./api";

export interface CutExtent { low: THREE.Box3; high: THREE.Box3; segments: Float32Array; boxes: THREE.Box3[] }

/** Clip triangle edges for the contour and dimensions without changing topology or paint. */
export function cutExtent(positions: ArrayLike<number>, ranges: { previewStart: number; previewEnd: number }[], point: Vec3, normal: Vec3): CutExtent {
  const plane = new THREE.Plane().setFromNormalAndCoplanarPoint(new THREE.Vector3(...normal), new THREE.Vector3(...point));
  const low = new THREE.Box3(), high = new THREE.Box3(), segments: number[] = [], boxes: THREE.Box3[] = [];
  const vertices = [new THREE.Vector3(), new THREE.Vector3(), new THREE.Vector3()];
  for (const range of ranges) {
    const box = new THREE.Box3();
    for (let i = range.previewStart * 9; i < range.previewEnd * 9; i += 9) {
      vertices.forEach((v, k) => { v.set(positions[i + k * 3], positions[i + k * 3 + 1], positions[i + k * 3 + 2]); box.expandByPoint(v); });
      const distances = vertices.map((v) => plane.distanceToPoint(v));
      const hits: THREE.Vector3[] = [];
      for (let k = 0; k < 3; k++) {
        const next = (k + 1) % 3, d = distances[k], other = distances[next];
        if (d <= 0) low.expandByPoint(vertices[k]);
        if (d >= 0) high.expandByPoint(vertices[k]);
        if (Math.abs(d) < 1e-7) hits.push(vertices[k].clone());
        if (d * other < 0) {
          const hit = vertices[k].clone().lerp(vertices[next], d / (d - other));
          hits.push(hit); low.expandByPoint(hit); high.expandByPoint(hit);
        }
      }
      const unique = hits.filter((v, k) => hits.slice(0, k).every((p) => p.distanceToSquared(v) > 1e-14));
      if (unique.length === 2) segments.push(...unique[0].toArray(), ...unique[1].toArray());
    }
    if (!box.isEmpty()) boxes.push(box);
  }
  return { low, high, segments: new Float32Array(segments), boxes };
}

