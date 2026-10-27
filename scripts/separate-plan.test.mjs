import assert from "node:assert/strict";
import { test } from "node:test";
import { build } from "esbuild";

const load = async (entry) => {
  const out = await build({ entryPoints: [entry], bundle: true, platform: "node", format: "esm", write: false, loader: { ".css": "empty" } });
  return import(`data:text/javascript;base64,${Buffer.from(out.outputFiles[0].text).toString("base64")}`);
};
const { compact, objectColour } = await load("src/separate-plan.ts");
const { Viewer } = await load("src/viewer.ts");

test("new objects are numbered in the order their first piece appears", () => {
  assert.deepEqual(compact([4, null, 4, 1, 9, null, 1]), [0, null, 0, 1, 2, null, 1]);
  assert.deepEqual(compact([null, null]), [null, null]);
});

test("each new object gets its own clear colour", () => {
  const colours = Array.from({ length: 8 }, (_, g) => objectColour(g).join());
  assert.equal(new Set(colours).size, 8);
  for (const c of colours) for (const v of c.split(",").map(Number)) assert.ok(v >= 0 && v <= 255);
});

test("a triangle tint covers all of its leaves and goes away with null", () => {
  const viewer = Object.create(Viewer.prototype);
  // Three preview triangles: one leaf, three leaves, two leaves.
  viewer.data = { offsets: new Uint32Array([0, 1, 4, 6]), labels: new Uint8Array(6), sourceRgb: new Uint8Array(18) };
  viewer.colors = new Uint8Array(6 * 9);
  viewer.palette = [[10, 20, 30]];
  viewer.view = "filament";
  viewer.mesh = { geometry: { getAttribute: () => ({}) } };
  viewer.requestRender = () => {};
  const leaf = (l) => Array.from(viewer.colors.slice(l * 9, l * 9 + 3));
  viewer.setTriangleTint([null, [200, 0, 0], [0, 0, 200]]);
  assert.deepEqual([0, 1, 2, 3, 4, 5].map(leaf), [[10, 20, 30], [200, 0, 0], [200, 0, 0], [200, 0, 0], [0, 0, 200], [0, 0, 200]]);
  // A tint for another mesh is ignored rather than misapplied.
  viewer.setTriangleTint([[1, 1, 1]]);
  assert.deepEqual(leaf(1), [10, 20, 30]);
  viewer.setTriangleTint(null);
  assert.deepEqual(leaf(4), [10, 20, 30]);
});
