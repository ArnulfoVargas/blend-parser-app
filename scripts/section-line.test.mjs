import assert from "node:assert/strict";
import { test } from "node:test";
import { build } from "esbuild";
import * as THREE from "three";

const compiled = await build({ entryPoints: ["src/section-line.ts"], bundle: true, platform: "node", format: "esm", write: false });
const { planeFromStroke, strokeCrossesMesh, SectionLine, snappedEnd } = await import(`data:text/javascript;base64,${Buffer.from(compiled.outputFiles[0].text).toString("base64")}`);
const box = new THREE.BoxGeometry(20, 20, 20).toNonIndexed();
const positions = box.getAttribute("position").array;
const ranges = [{ previewStart: 0, previewEnd: positions.length / 9 }];
const centre = new THREE.Vector3();
const normalOf = (plane) => new THREE.Vector3(0, 0, 1).applyEuler(new THREE.Euler(...plane.rotation.map(THREE.MathUtils.degToRad), "XYZ"));
const screen = (v, camera, rect) => {
  const p = v.clone().project(camera);
  return { x: rect.left + (p.x + 1) * rect.width / 2, y: rect.top + (1 - p.y) * rect.height / 2 };
};
const directions = { front: [0, -1, 0], side: [1, 0, 0], top: [0, 0, 1], oblique: [.55, -1, .62] };

for (const [view, direction] of Object.entries(directions)) {
  for (const distance of [55, 140]) for (const rect of [
    { left: 0, top: 0, width: 900, height: 600 },
    { left: 281, top: 157, width: 420, height: 770 },
  ]) for (const pan of [new THREE.Vector3(), new THREE.Vector3(5, 2, 3)]) {
    test(`stroke matches ${view}, distance ${distance}, viewport ${rect.width}, pan ${pan.length()}`, () => {
      const camera = new THREE.PerspectiveCamera(35, rect.width / rect.height, .1, 10000);
      camera.up.set(0, 0, 1);
      camera.position.copy(new THREE.Vector3(...direction).normalize().multiplyScalar(distance).add(pan));
      camera.lookAt(pan);
      camera.updateMatrixWorld();
      const middle = screen(centre, camera, rect);
      const a = { x: middle.x - 65, y: middle.y + 27 }, b = { x: middle.x + 65, y: middle.y - 27 };
      const result = planeFromStroke(a, b, rect, camera, centre);
      assert.equal(typeof result, "object");
      const plane = new THREE.Plane().setFromNormalAndCoplanarPoint(normalOf(result), new THREE.Vector3(...result.point));
      assert.ok(strokeCrossesMesh(a, b, rect, camera, result, positions, ranges));
      // Independent world-space verification at multiple depths: every ray of
      // the drawn segment stays in the plane, not just at the model's centre.
      for (const fraction of [0, .3, 1]) {
        const p = { x: a.x + fraction * (b.x - a.x), y: a.y + fraction * (b.y - a.y) };
        const caster = new THREE.Raycaster();
        caster.setFromCamera(new THREE.Vector2(2 * (p.x - rect.left) / rect.width - 1, 1 - 2 * (p.y - rect.top) / rect.height), camera);
        for (const depth of [5, 50, 500]) assert.ok(Math.abs(plane.distanceToPoint(caster.ray.at(depth, new THREE.Vector3()))) < 1e-8);
      }
    });
  }
}

test("rejects short strokes, silhouette misses, tangent cuts and segments beyond the model", () => {
  const rect = { left: 200, top: 80, width: 800, height: 600 };
  const camera = new THREE.PerspectiveCamera(35, 800 / 600, .1, 1000);
  camera.position.set(0, 0, 100);
  camera.lookAt(centre);
  camera.updateMatrixWorld();
  const p = screen(centre, camera, rect);
  assert.equal(planeFromStroke(p, { x: p.x + 11, y: p.y }, rect, camera, centre), "tooShort");
  for (const [a, b] of [
    [{ x: p.x - 60, y: p.y - 220 }, { x: p.x + 60, y: p.y - 220 }],
    [{ x: p.x + 210, y: p.y }, { x: p.x + 300, y: p.y }],
  ]) {
    const plane = planeFromStroke(a, b, rect, camera, centre);
    assert.equal(strokeCrossesMesh(a, b, rect, camera, plane, positions, ranges), false);
  }
  // Orthographic tangent is exactly the box's right face.
  const ortho = new THREE.OrthographicCamera(-40, 40, 30, -30, .1, 1000);
  ortho.position.copy(camera.position); ortho.lookAt(centre); ortho.updateMatrixWorld();
  const edge = screen(new THREE.Vector3(10, 0, 0), ortho, rect);
  const a = { x: edge.x, y: edge.y - 100 }, b = { x: edge.x, y: edge.y + 100 };
  assert.equal(strokeCrossesMesh(a, b, rect, ortho, planeFromStroke(a, b, rect, ortho, centre), positions, ranges), false);
});

test("validates only selected parts and does not mistake empty space inside bounds for a hit", () => {
  const rect = { left: 0, top: 0, width: 800, height: 600 };
  const camera = new THREE.PerspectiveCamera(35, 800 / 600, .1, 1000);
  camera.position.set(0, 0, 150); camera.lookAt(centre); camera.updateMatrixWorld();
  const left = Array.from(positions, (value, i) => i % 3 === 0 ? value - 25 : value);
  const right = Array.from(positions, (value, i) => i % 3 === 0 ? value + 25 : value);
  const both = [...left, ...right], parts = [...ranges, { previewStart: ranges[0].previewEnd, previewEnd: ranges[0].previewEnd * 2 }];
  const a = screen(new THREE.Vector3(-25, -20, 0), camera, rect), b = screen(new THREE.Vector3(-25, 20, 0), camera, rect);
  const plane = planeFromStroke(a, b, rect, camera, centre);
  assert.equal(strokeCrossesMesh(a, b, rect, camera, plane, both, [parts[0]]), true);
  assert.equal(strokeCrossesMesh(a, b, rect, camera, plane, both, [parts[1]]), false);
  const gapA = screen(new THREE.Vector3(0, -20, 0), camera, rect), gapB = screen(new THREE.Vector3(0, 20, 0), camera, rect);
  assert.equal(strokeCrossesMesh(gapA, gapB, rect, camera, planeFromStroke(gapA, gapB, rect, camera, centre), both, parts), false);
});

class Element extends EventTarget {
  style = {};
  attributes = {};
  classList = { add() {} };
  pointers = new Set();
  append() {}
  after() {}
  setAttribute(key, value) { this.attributes[key] = value; }
  setPointerCapture(id) { this.pointers.add(id); }
  hasPointerCapture(id) { return this.pointers.has(id); }
  releasePointerCapture(id) { this.pointers.delete(id); this.dispatchEvent(new Event("lostpointercapture")); }
  getBoundingClientRect() { return { left: 200, top: 80, width: 800, height: 600 }; }
}
const dispatch = (target, type, properties = {}) => {
  const event = new Event(type, { cancelable: true });
  for (const [key, value] of Object.entries(properties)) Object.defineProperty(event, key, { value });
  target.dispatchEvent(event);
};

test("drawing lifecycle restores input after finish, Esc, blur, pointer cancellation and lost capture", () => {
  globalThis.document = { createElementNS: () => new Element() };
  globalThis.window = new EventTarget();
  const canvas = new Element(), states = [], strokes = [];
  let cancels = 0, accepted = true;
  const line = new SectionLine(canvas, {
    state: (active) => states.push(active),
    finish: (a, b) => { strokes.push([a, b]); return accepted; },
    cancel: () => cancels++,
  });
  const down = () => dispatch(canvas, "pointerdown", { button: 0, pointerId: 1, clientX: 220, clientY: 100 });
  const up = () => dispatch(window, "pointerup", { pointerId: 1, clientX: 350, clientY: 200 });
  line.start(); down(); up();
  assert.equal(line.active, false); assert.equal(canvas.pointers.size, 0); assert.equal(strokes.length, 1);
  accepted = false;
  line.start(); down(); up();
  assert.equal(line.active, true); assert.equal(canvas.pointers.size, 0);
  accepted = true; down(); up(); assert.equal(line.active, false);
  for (const cancel of [
    () => dispatch(window, "keydown", { key: "Escape" }),
    () => dispatch(window, "blur"),
    () => dispatch(canvas, "pointercancel"),
    () => canvas.releasePointerCapture(1),
    () => line.cancel(),
  ]) {
    line.start(); down(); cancel();
    assert.equal(line.active, false); assert.equal(canvas.pointers.size, 0);
    const before = strokes.length; up(); assert.equal(strokes.length, before);
  }
  assert.equal(cancels, 5);
  assert.equal(states.at(-1), false);
  // After cancellation, pointer input propagates to orbit/paint/measure again.
  let delivered = 0;
  canvas.addEventListener("pointerdown", () => delivered++);
  dispatch(canvas, "pointerdown", { button: 0, pointerId: 1, clientX: 600, clientY: 500 }); assert.equal(delivered, 1);
  line.start(); down(); assert.equal(delivered, 1); line.cancel();
});

test("endpoints and centre edit the completed stroke, preview live and restore on Esc", () => {
  globalThis.document = { createElementNS: () => new Element() };
  globalThis.window = new EventTarget();
  const canvas = new Element(), previews = [], finished = [];
  const line = new SectionLine(canvas, {
    state() {}, begin() {}, cancel() {},
    preview: (a, b) => previews.push(structuredClone([a, b])),
    finish: (a, b) => { finished.push(structuredClone([a, b])); return true; },
  });
  const pointer = (target, type, x, y, shiftKey = false) => dispatch(target, type, { button: 0, pointerId: 1, clientX: x, clientY: y, shiftKey });
  line.start(); pointer(canvas, "pointerdown", 250, 150); pointer(window, "pointerup", 450, 150);
  pointer(canvas, "pointerdown", 450, 150); pointer(canvas, "pointermove", 450, 200, true);
  assert.equal(line.active, true);
  const end = previews.at(-1)[1];
  assert.ok(Math.abs(Math.atan2(end.y - 150, end.x - 250) * 180 / Math.PI - 15) < 1e-8);
  dispatch(window, "keydown", { key: "Escape" });
  assert.equal(line.active, false);
  // The original centre still moves both endpoints by the same displacement.
  pointer(canvas, "pointerdown", 350, 150); pointer(window, "pointerup", 360, 175);
  assert.deepEqual(finished.at(-1), [{ x: 260, y: 175 }, { x: 460, y: 175 }]);
  line.clear();
  pointer(canvas, "pointerdown", 460, 175); assert.equal(line.active, false);
  assert.deepEqual(snappedEnd({ x: 0, y: 0 }, { x: 20, y: 1 }, false), { x: 20, y: 1 });
});
