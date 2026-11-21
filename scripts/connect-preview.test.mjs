import assert from "node:assert/strict";
import { test } from "node:test";
import { build } from "esbuild";

const out = await build({ entryPoints: ["src/connect-preview.ts"], bundle: true, platform: "node", format: "esm", write: false });
const { planeBasis, outline, reach, ConnectorPreview, ROUND_SIDES } = await import(`data:text/javascript;base64,${Buffer.from(out.outputFiles[0].text).toString("base64")}`);
import * as THREE from "three";

const spec = { kind: "pin", pinsOnA: true, shape: "round", sizeMm: 5, depthMm: 6, clearanceMm: 0.2, angleDeg: 0 };

test("the plane axes match the backend's and are orthonormal", () => {
  for (const n of [[0, 0, 1], [1, 0, 0], [0.3, -0.2, 0.9]]) {
    const [u, v] = planeBasis(n).map((a) => new THREE.Vector3(...a));
    const normal = new THREE.Vector3(...n).normalize();
    assert.ok(Math.abs(u.length() - 1) < 1e-6 && Math.abs(v.length() - 1) < 1e-6);
    assert.ok(Math.abs(u.dot(normal)) < 1e-6 && Math.abs(v.dot(normal)) < 1e-6 && Math.abs(u.dot(v)) < 1e-6);
    // u = normalize(seed × n), seed X unless the normal is close to X (cut::Plane::basis).
    const seed = Math.abs(normal.x) < 0.8 ? new THREE.Vector3(1, 0, 0) : new THREE.Vector3(0, 1, 0);
    assert.ok(seed.cross(normal).normalize().distanceTo(u) < 1e-6);
  }
});

test("outlines carry the clearance on every side", () => {
  const round = outline(spec, 0.2);
  assert.equal(round.length, ROUND_SIDES);
  assert.ok(round.every(([x, y]) => Math.abs(Math.hypot(x, y) - 2.7) < 1e-9));
  const square = { ...spec, shape: "square", angleDeg: 30 };
  const pts = outline(square, 0.2);
  const side = Math.hypot(pts[1][0] - pts[0][0], pts[1][1] - pts[0][1]);
  assert.ok(Math.abs(side - 5.4) < 1e-9);
  assert.ok(Math.abs(reach(square, 0) - 2.5 * Math.SQRT2) < 1e-9);
});

test("the preview draws a body, its sockets and a rim per connector", () => {
  const colours = { ok: new THREE.Color(0, 1, 0), bad: new THREE.Color(1, 0, 0), glass: new THREE.Color(0.2, 0.2, 0.2) };
  const preview = new ConnectorPreview(colours);
  preview.show(spec, [0, 0, 1], [[0, 0, 10], [5, 0, 10]], [null, "overlap"], 1);
  // Pin: one socket, one body, one rim each.
  assert.equal(preview.group.children.length, 6);
  preview.show({ ...spec, kind: "dowel" }, [0, 0, 1], [[0, 0, 10]], [null], 1);
  assert.equal(preview.group.children.length, 4);
  const box = new THREE.Box3().setFromObject(preview.group);
  // A dowel spans its depth both ways from the plane; its sockets add the clearance.
  assert.ok(Math.abs(box.min.z - (10 - 6.2)) < 1e-6 && Math.abs(box.max.z - (10 + 6.2)) < 1e-6, `${box.min.z} ${box.max.z}`);
  preview.show(spec, [0, 0, 1], [], [], 1);
  assert.equal(preview.group.visible, false);
});
