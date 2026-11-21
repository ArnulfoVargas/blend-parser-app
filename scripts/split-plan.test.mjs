import assert from "node:assert/strict";
import { test } from "node:test";
import { build } from "esbuild";
import * as THREE from "three";

const out = await build({ entryPoints: ["src/split-plan.ts"], bundle: true, platform: "node", format: "esm", write: false });
const { evenCuts, CutPlanes } = await import(`data:text/javascript;base64,${Buffer.from(out.outputFiles[0].text).toString("base64")}`);

test("cuts spread a span into equal pieces, as the backend proposes them", () => {
  assert.deepEqual(evenCuts(0, 300, 2), [100, 200]);
  assert.deepEqual(evenCuts(-50, 50, 0), []);
});

test("each cut is drawn across the model, square to its axis", () => {
  const planes = new CutPlanes([new THREE.Color(1, 0, 0), new THREE.Color(0, 1, 0), new THREE.Color(0, 0, 1)]);
  planes.show([[10], [], [5, 15]], { min: [-20, -10, 0], max: [20, 10, 20] });
  assert.equal(planes.group.children.length, 3);
  planes.group.updateMatrixWorld(true);
  const box = (i) => new THREE.Box3().setFromObject(planes.group.children[i]);
  const x = box(0);
  assert.ok(Math.abs(x.min.x - 10) < 1e-6 && Math.abs(x.max.x - 10) < 1e-6, "an X cut is flat in X");
  assert.ok(x.max.y - x.min.y > 20 && x.max.z - x.min.z > 20, "and spans the model in Y and Z");
  const z = box(2);
  assert.ok(Math.abs(z.min.z - 15) < 1e-6 && z.max.x - z.min.x > 40);
  planes.show(null, null);
  assert.equal(planes.group.visible, false);
});
