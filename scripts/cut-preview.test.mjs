import assert from "node:assert/strict";
import { test } from "node:test";
import { build } from "esbuild";
import * as THREE from "three";

async function load(entry) {
  const compiled = await build({ entryPoints: [entry], bundle: true, platform: "node", format: "esm", write: false });
  return import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`);
}
const { cutExtent, CutPreview } = await load("src/cut-preview.ts");
const { CutReview } = await load("src/cut-review.ts");
const geo = new THREE.BoxGeometry(20, 20, 20).toNonIndexed();
const positions = geo.getAttribute("position").array;
const range = { previewStart: 0, previewEnd: positions.length / 9 };

test("contour and half dimensions follow the exact plane on all axes and oblique cuts", () => {
  for (const normal of [[1, 0, 0], [0, 1, 0], [0, 0, 1], [.3, -.4, .8]]) {
    const n = new THREE.Vector3(...normal).normalize().toArray();
    const extent = cutExtent(positions, [range], [0, 0, 0], n);
    assert.ok(extent.segments.length > 0);
    for (let i = 0; i < extent.segments.length; i += 3) assert.ok(Math.abs(new THREE.Vector3().fromArray(extent.segments, i).dot(new THREE.Vector3(...n))) < 1e-5);
    assert.ok(extent.low.getSize(new THREE.Vector3()).equals(extent.high.getSize(new THREE.Vector3())));
    const axis = normal.findIndex((v) => v === 1);
    if (axis >= 0) assert.equal(extent.low.getSize(new THREE.Vector3()).getComponent(axis), 10);
  }
});

test("dimensions and outlines include only selected pieces, and misses have an empty half", () => {
  const second = Array.from(positions, (v, i) => i % 3 === 0 ? v + 50 : v);
  const both = [...positions, ...second];
  const extent = cutExtent(both, [range], [0, 0, 0], [0, 0, 1]);
  assert.equal(extent.boxes.length, 1);
  assert.equal(extent.low.max.x, 10);
  const miss = cutExtent(positions, [range], [0, 0, 30], [0, 0, 1]);
  assert.equal(miss.high.isEmpty(), true);
  assert.equal(miss.segments.length, 0);
});

test("ghost shares original paint buffers, hides unselected groups and flips its clipping side", () => {
  const preview = new CutPreview(new THREE.Color("green"));
  preview.attach(geo);
  const plane = new THREE.Plane(new THREE.Vector3(0, 0, 1), 0);
  const extent = cutExtent(positions, [range], [0, 0, 0], [0, 0, 1]);
  preview.show(plane, false, extent, true);
  assert.equal(preview.ghost.geometry, geo);
  assert.equal(preview.ghost.material[1].visible, false);
  assert.equal(preview.removed.clippingPlanes[0].normal.z, -1);
  assert.equal(preview.removed.opacity, .22);
  preview.show(plane, true, null, false);
  assert.equal(preview.removed.opacity, .85);
  preview.show(null, false, null, false);
  assert.equal(preview.group.visible, false);
});

const tick = () => new Promise((resolve) => setTimeout(resolve, 5));
test("outdated validation cannot approve a changed or cancelled cut; only one request runs", async () => {
  const running = [], results = [];
  const review = new CutReview({
    pending() {},
    run: (value) => new Promise((resolve, reject) => running.push({ value, resolve, reject })),
    result: (value, error) => results.push([value, error]),
  }, 0);
  review.schedule("first"); await tick();
  review.schedule("second"); await tick();
  review.schedule("latest"); await tick();
  assert.equal(running.length, 1);
  running[0].resolve(1); await tick();
  assert.equal(results.length, 0);
  assert.equal(running.length, 2); assert.equal(running[1].value, "latest");
  review.schedule(null);
  running[1].resolve(2); await tick();
  assert.equal(results.length, 0);
  review.schedule("invalid"); await tick();
  running[2].reject("open mesh"); await tick();
  assert.deepEqual(results, [[null, "open mesh"]]);
  review.schedule("valid"); await tick(); running[3].resolve(2); await tick();
  assert.deepEqual(results.at(-1), [2, undefined]);
});
