import assert from "node:assert/strict";
import { test } from "node:test";
import { build } from "esbuild";
import * as THREE from "three";

const compiled = await build({ entryPoints: ["src/viewer.ts"], bundle: true, platform: "node", format: "esm", write: false });
const { Viewer, sectionNormal } = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`);
const original = () => ({ point: [4, -3, 11], rotation: [23, -38, 15], selected: [1, 3], flip: true });

function harness() {
  const viewer = Object.create(Viewer.prototype);
  viewer.section = original();
  viewer.mesh = new THREE.Mesh(new THREE.BoxGeometry(20, 20, 20));
  viewer.mesh.geometry.computeBoundingBox();
  viewer.canvas = { style: { cursor: "grabbing" } };
  viewer.controls = { enabled: false };
  viewer.moveDrag = viewer.turnDrag = viewer.planeDrag = null;
  viewer.cutSnap = 15;
  viewer.frame = 0;
  viewer.settling = false;
  viewer.camera = new THREE.PerspectiveCamera();
  viewer.camera.position.set(50, 60, 70);
  viewer.applySection = viewer.showHandles = viewer.placeHandles = viewer.rayFrom = () => {};
  viewer.mmPerUnit = () => 2;
  viewer.screenAngle = (e) => e.angle;
  const events = [];
  viewer.handlers = {
    section: (section) => events.push(structuredClone(section)),
    rotate: () => { throw Error("A plane preview must not rotate the model"); },
    move: () => { throw Error("A plane preview must not move the model"); },
  };
  return { viewer, events };
}

for (const axis of [0, 1, 2]) {
  test(`plane ring ${"XYZ"[axis]} rotates an already oblique plane without changing the model`, async () => {
    const { viewer, events } = harness();
    const before = original();
    viewer.turnDrag = { axis, centre: new THREE.Vector3(...before.point), sign: 1, last: 0, total: 0, degrees: 0, section: before };
    viewer.turnTo({ angle: Math.PI / 2, shiftKey: false });
    const expected = new THREE.Vector3(...sectionNormal(before)).applyAxisAngle(new THREE.Vector3().setComponent(axis, 1), Math.PI / 2);
    assert.ok(expected.distanceTo(new THREE.Vector3(...sectionNormal(viewer.section))) < 1e-6);
    assert.deepEqual(viewer.section.point, before.point);
    assert.deepEqual(viewer.section.selected, before.selected);
    assert.equal(viewer.section.flip, true);
    assert.ok(viewer.mesh.quaternion.equals(new THREE.Quaternion()));
    assert.ok(viewer.mesh.position.equals(new THREE.Vector3()));
    assert.equal(events.length, 1);
    await viewer.releaseTurn();
    assert.equal(viewer.turnDrag, null);
    assert.equal(viewer.controls.enabled, true);
    assert.ok(viewer.frame > 0);
  });
}

for (const slide of [0, 1, 2, "plane"]) {
  test(`plane translation ${slide} preserves angles and restricts motion to its handle`, () => {
    const { viewer } = harness();
    viewer.slideAt = () => slide;
    let at = new THREE.Vector3(1, 2, 3);
    viewer.slidePoint = () => at.clone();
    assert.ok(viewer.grabPlaneSlide());
    at.add(new THREE.Vector3(2, 4, 6));
    viewer.slidePlaneTo({ shiftKey: false });
    const expected = slide === "plane" ? [6, 1, 11] : [4, -3, 11].map((v, i) => v + (i === slide ? [2, 4, 6][i] : 0));
    assert.deepEqual(viewer.section.point, expected);
    assert.deepEqual(viewer.section.rotation, original().rotation);
    assert.ok(viewer.mesh.position.equals(new THREE.Vector3()));
    viewer.releasePlaneSlide();
    assert.equal(viewer.planeDrag, null);
    assert.equal(viewer.controls.enabled, true);
  });
}

test("cancel restores the complete plane and orbit after translation or rotation", () => {
  for (const kind of ["planeDrag", "turnDrag"]) {
    const { viewer, events } = harness();
    viewer[kind] = { section: original() };
    viewer.section = { ...original(), point: [100, 200, 300], rotation: [45, 90, 135] };
    viewer.cancelPlace();
    assert.deepEqual(viewer.section, original());
    assert.deepEqual(events.at(-1), original());
    assert.equal(viewer.controls.enabled, true);
    assert.equal(viewer.planeDrag, null);
    assert.equal(viewer.turnDrag, null);
    assert.equal(viewer.canvas.style.cursor, "");
  }
});

test("fine ring rotation uses tenths and the selected rotation step otherwise", () => {
  const { viewer } = harness();
  for (const [fine, expected] of [[false, 15], [true, 17.2]]) {
    viewer.turnDrag = { axis: 0, centre: new THREE.Vector3(), sign: 1, last: 0, total: 0, degrees: 0, section: original() };
    viewer.turnTo({ angle: THREE.MathUtils.degToRad(17.23), shiftKey: fine });
    assert.ok(Math.abs(viewer.turnDrag.degrees - expected) < 1e-6);
  }
});

test("releasing a model ring ends its drag before the backend responds", async () => {
  const { viewer } = harness();
  let resolve;
  viewer.handlers.rotate = () => new Promise((done) => { resolve = done; });
  viewer.turnDrag = { axis: 2, centre: new THREE.Vector3(), degrees: 45, section: null };
  const pending = viewer.releaseTurn();
  assert.equal(viewer.turnDrag, null);
  assert.equal(viewer.settling, true);
  assert.equal(viewer.controls.enabled, true);
  resolve(false);
  await pending;
  assert.equal(viewer.settling, false);
  assert.ok(viewer.mesh.quaternion.equals(new THREE.Quaternion()));
});
