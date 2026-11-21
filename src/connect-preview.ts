import * as THREE from "three";
import type { Vec3 } from "./api";

/** Sides of a round connector, as the backend makes it (`connect::ROUND_SIDES`). */
export const ROUND_SIDES = 32;

export interface ConnectorSpec {
  kind: "pin" | "dowel";
  pinsOnA: boolean;
  shape: "round" | "square";
  sizeMm: number;
  depthMm: number;
  clearanceMm: number;
  angleDeg: number;
}

/** The plane's in-plane axes as the backend picks them (`cut::Plane::basis`), so squares turn the same way. */
export function planeBasis(normal: Vec3): [Vec3, Vec3] {
  const n = new THREE.Vector3(...normal).normalize();
  const seed = Math.abs(n.x) < 0.8 ? new THREE.Vector3(1, 0, 0) : new THREE.Vector3(0, 1, 0);
  const u = seed.clone().cross(n).normalize();
  const v = n.clone().cross(u);
  return [u.toArray() as Vec3, v.toArray() as Vec3];
}

/** Radius of the circle round a connector's outline grown by `grow` mm. */
export function reach(spec: ConnectorSpec, grow: number): number {
  const half = spec.sizeMm / 2 + grow;
  return spec.shape === "square" ? half * Math.SQRT2 : half;
}

/** The outline's corners (mm, in plane axes) grown by `grow`, as the backend draws it. */
export function outline(spec: ConnectorSpec, grow: number): [number, number][] {
  const square = spec.shape === "square";
  const n = square ? 4 : ROUND_SIDES;
  const r = reach(spec, grow);
  const turn = square ? THREE.MathUtils.degToRad(spec.angleDeg) + Math.PI / 4 : 0;
  return Array.from({ length: n }, (_, k) => {
    const a = turn + (k / n) * Math.PI * 2;
    return [r * Math.cos(a), r * Math.sin(a)];
  });
}

/** A closed prism over `ring` from 0 to `length` along +Z. */
function prism(ring: [number, number][], length: number): THREE.BufferGeometry {
  const shape = new THREE.Shape(ring.map(([x, y]) => new THREE.Vector2(x, y)));
  return new THREE.ExtrudeGeometry(shape, { depth: length, bevelEnabled: false });
}

/**
 * What the connectors will make, drawn on the cut plane before anything is
 * cut: pins and dowels solid, sockets as glass with the clearance round them,
 * and each connector's outline on the plane, red when something is wrong.
 */
export class ConnectorPreview {
  readonly group = new THREE.Group();
  private solid: THREE.MeshStandardMaterial;
  private bad: THREE.MeshStandardMaterial;
  private glass: THREE.MeshBasicMaterial;
  private rim: THREE.LineBasicMaterial;
  private rimBad: THREE.LineBasicMaterial;

  constructor(colours: { ok: THREE.Color; bad: THREE.Color; glass: THREE.Color }) {
    // Drawn over the model, like the handles: the half they go into would hide them.
    this.solid = new THREE.MeshStandardMaterial({ color: colours.ok, roughness: 0.6, depthTest: false, transparent: true, opacity: 0.95 });
    this.bad = new THREE.MeshStandardMaterial({ color: colours.bad, roughness: 0.6, depthTest: false, transparent: true, opacity: 0.95 });
    this.glass = new THREE.MeshBasicMaterial({ color: colours.glass, transparent: true, opacity: 0.22, depthWrite: false, depthTest: false, side: THREE.DoubleSide });
    this.rim = new THREE.LineBasicMaterial({ color: colours.ok, depthTest: false, transparent: true });
    this.rimBad = new THREE.LineBasicMaterial({ color: colours.bad, depthTest: false, transparent: true });
    this.group.visible = false;
  }

  clear() {
    for (const child of [...this.group.children]) {
      child.traverse((o) => (o as THREE.Mesh).geometry?.dispose());
      this.group.remove(child);
    }
  }

  /** `centres` on the plane in preview units; `unit` is preview units per mm. */
  show(spec: ConnectorSpec, normal: Vec3, centres: Vec3[], issues: (string | null)[], unit: number) {
    this.clear();
    this.group.visible = centres.length > 0;
    const n = new THREE.Vector3(...normal).normalize();
    const [u, v] = planeBasis(normal).map((a) => new THREE.Vector3(...a));
    // Prism space (x, y in the plane, z along the normal) to preview space.
    const frame = new THREE.Matrix4().makeBasis(u, v, n).multiply(new THREE.Matrix4().makeScale(unit, unit, unit));
    const deep = spec.depthMm + spec.clearanceMm;
    // A is below the plane (−normal), B above it.
    const socketSides = spec.kind === "dowel" ? [-1, 1] : [spec.pinsOnA ? 1 : -1];
    centres.forEach((c, i) => {
      const at = new THREE.Matrix4().makeTranslation(c[0], c[1], c[2]).multiply(frame);
      const wrong = issues[i] != null;
      const add = (geometry: THREE.BufferGeometry, material: THREE.Material, from: number, sign: number) => {
        const mesh = new THREE.Mesh(geometry, material);
        // The prism runs 0..length on +Z; flipped for the side below the plane.
        mesh.applyMatrix4(new THREE.Matrix4().makeScale(1, 1, sign).premultiply(new THREE.Matrix4().makeTranslation(0, 0, from)).premultiply(at));
        mesh.renderOrder = material === this.glass ? 18 : 17;
        this.group.add(mesh);
      };
      for (const side of socketSides) add(prism(outline(spec, spec.clearanceMm), deep), this.glass, 0, side);
      const body = wrong ? this.bad : this.solid;
      if (spec.kind === "dowel") add(prism(outline(spec, 0), 2 * spec.depthMm), body, -spec.depthMm, 1);
      else add(prism(outline(spec, 0), spec.depthMm), body, 0, spec.pinsOnA ? 1 : -1);
      const ring = outline(spec, spec.clearanceMm).map(([x, y]) => new THREE.Vector3(x, y, 0));
      const line = new THREE.LineLoop(new THREE.BufferGeometry().setFromPoints(ring), wrong ? this.rimBad : this.rim);
      line.applyMatrix4(at);
      line.renderOrder = 19;
      this.group.add(line);
    });
  }
}
