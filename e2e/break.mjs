// Tries to break the app: commands with hostile arguments, bursts of concurrent
// calls, undo spam, cutting everything away, and junk typed into the forms.
// After every attack it checks the document still answers (a panic while the
// document is locked poisons it and every later command fails) and that the
// window logged no uncaught error.
//
//   BP_APP=<binary> BP_ROOT=<checkout> node break.mjs [attack…]
import path from "node:path";
import { HERE, OUT, services, stop, session, waitState, settle, shot, report, setValue, click, sleep } from "./lib.mjs";

const MODEL = process.env.BP_MODEL ?? path.join(HERE, "e2e/fixtures/out/figure.blend");
const results = [];
const log = (attack, name, verdict, detail = "") => {
  results.push({ attack, name, verdict, detail });
  const mark = { ok: "✔", rejected: "·", BROKEN: "✘", HANG: "⌛", missing: "–" }[verdict] ?? "?";
  console.log(`  ${mark} ${name} — ${verdict}${detail ? `: ${detail}` : ""}`);
};

/** One command; the error comes back as a value. Times out as a hang. */
const call = (b, cmd, args = {}, ms = 60000) => // eslint-disable-line

  b.executeAsync((c, a, t, done) => {
    const timer = setTimeout(() => done({ hang: true }), t);
    window.__TAURI_INTERNALS__.invoke(c, a).then(
      (v) => { clearTimeout(timer); done({ ok: true, size: v == null ? 0 : (v.byteLength ?? JSON.stringify(v).length) }); },
      (e) => { clearTimeout(timer); done({ err: typeof e === "string" ? e : JSON.stringify(e) }); },
    );
  }, cmd, args, ms);

/** Does the document still answer, and did the window log errors? */
async function health(b) {
  const r = await call(b, "pick", { origin: [0, 0, 500], dir: [0, 0, -1] }, 20000);
  const errors = await b.execute(() => (window.__bpErrors ?? []).splice(0));
  return { alive: !!r.ok, why: r.err ?? (r.hang ? "hang" : ""), errors };
}

async function attack(b, name, cmd, args, ms = 60000) {
  const t0 = Date.now();
  const r = await call(b, cmd, args, ms);
  const h = await health(b);
  const took = Date.now() - t0;
  const unknown = r.err && /not found|unknown command|did not find|invalid args.*missing/i.test(r.err) && /command/i.test(r.err);
  if (unknown) return log(name, `${cmd}`, "missing", r.err.slice(0, 120));
  if (r.hang) return log(name, `${cmd} ${JSON.stringify(args).slice(0, 90)}`, "HANG", `> ${ms} ms`);
  if (!h.alive) return log(name, `${cmd} ${JSON.stringify(args).slice(0, 90)}`, "BROKEN", `document dead after it: ${h.why.slice(0, 160)} (call said ${(r.err ?? "ok").slice(0, 120)})`);
  if (h.errors.length) return log(name, `${cmd} ${JSON.stringify(args).slice(0, 90)}`, "BROKEN", `window error: ${h.errors[0].slice(0, 160)}`);
  if (r.err && /panic|internal|poison/i.test(r.err)) return log(name, `${cmd} ${JSON.stringify(args).slice(0, 90)}`, "BROKEN", `panic: ${r.err.slice(0, 200)}`);
  log(name, `${cmd} ${JSON.stringify(args).slice(0, 90)}`, r.err ? "rejected" : "ok", `${took} ms ${r.err ? r.err.slice(0, 100) : ""}`);
}

const BIG = 1e30;

/** The model's box in preview coordinates (the ones planes and rays use). */
async function box(b) {
  return b.executeAsync((done) => {
    window.__TAURI_INTERNALS__.invoke("preview_mesh", {}).then((buf) => {
      const leaves = new DataView(buf, 0, 8).getUint32(0, true);
      const p = new Float32Array(buf, 8, leaves * 9);
      const lo = [Infinity, Infinity, Infinity], hi = [-Infinity, -Infinity, -Infinity];
      for (let i = 0; i < p.length; i += 3) for (let k = 0; k < 3; k++) { lo[k] = Math.min(lo[k], p[i + k]); hi[k] = Math.max(hi[k], p[i + k]); }
      done({ lo, hi, mid: lo.map((v, k) => (v + hi[k]) / 2), size: lo.map((v, k) => hi[k] - v) });
    }, (e) => done({ err: JSON.stringify(e) }));
  });
}
const at = (g, fz) => [g.mid[0], g.mid[1], g.lo[2] + g.size[2] * fz];
const above = (g) => [g.mid[0], g.mid[1], g.hi[2] + g.size[2]];
const joint = (o = {}) => ({ kind: "plug", shape: "circle", sizeMm: 5, depthMm: 5, toleranceMm: 0.2, count: 0, ...o });
const connectors = (o = {}) => ({ kind: "dowel", pinsOnA: true, shape: "round", sizeMm: 5, depthMm: 5, clearanceMm: 0.2, angleDeg: 0, at: [], ...o });
const plan = (o = {}) => ({ cuts: [[], [], [20]], layOnCut: true, gapMm: 5, dowels: null, ...o });
const kit = (o = {}) => ({ voxelMm: 1, depthMm: 2, coreSlot: null, minPieceMm3: 5, mix: false, contents: { threeMf: true, stl: "piece", guide: true, text: true, quality: "low" }, pins: null, ...o });

const attacks = {
  // Every branch: the old commands with values the UI never sends.
  async sizes(b) {
    for (const mm of [[0, 0, 0], [-5, 10, 10], [BIG, BIG, BIG], [1, 1, 1], [1000, 1000, 1000], [1, 1000, 1]]) await attack(b, "sizes", "set_size", { mm });
    for (let i = 0; i < 30; i++) await call(b, "undo_size");
    await attack(b, "sizes", "undo_size", {});
    await attack(b, "sizes", "set_offset", { mm: [BIG, -BIG] });
    await attack(b, "sizes", "fit_to_plate", {});
  },
  async turns(b) {
    for (const [axis, degrees] of [[0, 0], [3, 90], [-1, 90], [2, BIG], [1, 360], [0, 0.0001], [2, -720]]) await attack(b, "turns", "rotate", { axis, degrees });
    await attack(b, "turns", "set_rotation", { rotation: [[0, 0, 0], [0, 0, 0], [0, 0, 0]] });
    await attack(b, "turns", "set_rotation", { rotation: [[2, 0, 0], [0, 2, 0], [0, 0, 2]] });
    await attack(b, "turns", "reset_rotation", {});
  },
  async colours(b) {
    await attack(b, "colours", "suggest_palette", { count: 0 });
    await attack(b, "colours", "suggest_palette", { count: 300 });
    await attack(b, "colours", "set_palette", { colors: Array(40).fill("#ff0000") });
    await attack(b, "colours", "reduce_palette", { target: 0 });
    await attack(b, "colours", "merge_slots", { from: 99, into: 0 });
    await attack(b, "colours", "merge_slots", { from: 0, into: 0 });
    await attack(b, "colours", "set_color_ops", { borderDetail: 255, minIslandMm2: BIG });
    const g = await box(b);
    await attack(b, "colours", "paint", { origin: above(g), dir: [0, 0, -1], radiusMm: BIG, slot: 200 });
    await attack(b, "colours", "paint", { origin: above(g), dir: [0, 0, -1], radiusMm: 1000, slot: 1 });
    await attack(b, "colours", "paint", { origin: above(g), dir: [0, 0, 0], radiusMm: 5, slot: 1 });
    await attack(b, "colours", "end_stroke", {});
    await attack(b, "colours", "set_flush", { from: 50, to: 0, mm3: -1 });
    // Last: on develop it panics with the document locked, and nothing answers after.
    await attack(b, "colours", "set_palette", { colors: [] });
  },
  async mesh(b) {
    for (const [keepRatio, flattenMm, minPieceMm] of [[0, 0, 0], [0.0001, 0, 0], [1, BIG, 0], [1, 0, BIG], [1, 79, 0], [-1, -1, -1]])
      await attack(b, "mesh", "set_mesh_ops", { keepRatio, flattenMm, repair: true, minPieceMm });
    for (const [protectColors, protectDetail] of [[true, true], [true, false], ["yes", 1], [null, null]])
      await attack(b, "mesh", "set_mesh_ops", { keepRatio: 0.05, flattenMm: 0, repair: true, minPieceMm: 0, protectColors, protectDetail });
    await attack(b, "mesh", "set_mesh_ops", { keepRatio: NaN, flattenMm: 0, repair: true, minPieceMm: 0, protectColors: true });
    await attack(b, "mesh", "set_mesh_ops", { keepRatio: 1, flattenMm: 0, repair: true, minPieceMm: 0 });
    await attack(b, "mesh", "fix_mesh", { fix: { repair: true, orient: true, fillHoles: true, splitVertices: true, dropFloating: true, deepClean: true, minPieceMm: 1000, flattenMm: 1000 } });
    await attack(b, "mesh", "undo", {});
    await attack(b, "mesh", "orient_for_print", { overhangDeg: -10 });
    for (const overhangDeg of [-10, 0, 90, 1e30, NaN, "45"]) await attack(b, "mesh", "plan_orientation", { overhangDeg });
    await attack(b, "mesh", "set_rotation", { rotation: [[0, 0, 0], [0, 0, 0], [0, 0, 0]] });
    await attack(b, "mesh", "set_rotation", { rotation: [[1e30, 0, 0], [0, 1, 0], [0, 0, 1]] });
    await attack(b, "mesh", "undo", {});
  },
  // The repair switches: counted while other commands run, toggled in a burst, undone in a loop.
  async repairs(b) {
    await attack(b, "repairs", "mesh_issues", {});
    for (const fix of [{ fillHoles: true }, { orient: true }, { splitVertices: true }, { dropFloating: true }, { deepClean: true }, { fillHoles: false, orient: false }, { fillHoles: "yes" }, { holes: true }])
      await attack(b, "repairs", "fix_mesh", { fix });
    const r = await b.executeAsync((done) => {
      const I = window.__TAURI_INTERNALS__.invoke;
      const calls = [];
      for (let i = 0; i < 12; i++) {
        calls.push(I("fix_mesh", { fix: { [["fillHoles", "orient", "splitVertices", "dropFloating", "deepClean"][i % 5]]: i % 2 === 0 } }));
        calls.push(I("mesh_issues", {}));
        calls.push(I("undo", {}));
      }
      Promise.allSettled(calls).then((rs) => done(rs.filter((x) => x.status === "rejected").map((x) => JSON.stringify(x.reason)).slice(0, 3)));
    });
    const h = await health(b);
    log("repairs", "36 concurrent repairs, counts and undos", h.alive && !h.errors.length ? "ok" : "BROKEN", `${r.length} rejected ${r.join(" ").slice(0, 160)} ${h.why}`);
    await click(b, "[role=tab][data-tab=mesh]");
    for (let i = 0; i < 4; i++) for (const op of ["fillHoles", "orient", "splitVertices", "dropFloating", "deepClean"]) await click(b, `#repair-ops input[data-op="${op}"]`);
    await settle(b);
    await attack(b, "repairs", "mesh_issues", {});
  },
  // The analysis views: picked in quick succession while the analysis runs, and across modes.
  async views(b) {
    await click(b, "[role=tab][data-tab=mesh]");
    for (let i = 0; i < 5; i++) for (const v of ["thickness", "edges", "overhang", "none"]) await click(b, `button[data-analysis="${v}"]`);
    await click(b, "[role=tab][data-tab=model]");
    await click(b, "[role=tab][data-tab=mesh]");
    await click(b, 'button[data-analysis="edges"]');
    await click(b, '#repair-ops input[data-op="fillHoles"]');
    await settle(b);
    const h = await health(b);
    log("views", "analysis views clicked 20 times and across modes", h.alive && !h.errors.length ? "ok" : "BROKEN", `${h.why} ${h.errors[0] ?? ""}`.slice(0, 200));
  },
  // Subdivide and smooth: out of range, past the cap, and in a burst with undos.
  async smoothing(b) {
    for (const args of [{ subdivide: 3, smooth: 0, creaseDeg: 30, maxTriangles: 1e9 }, { subdivide: 2, smooth: 99, creaseDeg: 30, maxTriangles: 1e6 },
      { subdivide: 2, smooth: 2, creaseDeg: -1, maxTriangles: 1e6 }, { subdivide: 2, smooth: 2, creaseDeg: NaN, maxTriangles: 1e6 },
      { subdivide: 2, smooth: 2, creaseDeg: 30, maxTriangles: 0 }, { subdivide: 2, smooth: 2, creaseDeg: 30, maxTriangles: -5 },
      { subdivide: 1, smooth: 20, creaseDeg: 180, maxTriangles: 8000000 }, { subdivide: 2, smooth: 0, creaseDeg: 0, maxTriangles: 8000000 }])
      await attack(b, "smoothing", "smooth_mesh", args, 180000);
    for (let i = 0; i < 4; i++) await call(b, "undo");
    const r = await b.executeAsync((done) => {
      const I = window.__TAURI_INTERNALS__.invoke;
      const calls = [];
      for (let i = 0; i < 8; i++) {
        calls.push(I("smooth_mesh", { subdivide: i % 3, smooth: i, creaseDeg: 30, maxTriangles: 2000000 }));
        calls.push(I("undo", {}));
      }
      Promise.allSettled(calls).then((rs) => done(rs.filter((x) => x.status === "rejected").map((x) => JSON.stringify(x.reason)).slice(0, 3)));
    });
    const h = await health(b);
    log("smoothing", "16 concurrent smooths and undos", h.alive && !h.errors.length ? "ok" : "BROKEN", `${r.length} rejected ${r.join(" ").slice(0, 160)} ${h.why}`);
  },
  // Hollowing: hostile walls and drains, an open model, and hollowing in a burst with undos.
  async hollowing(b) {
    for (const args of [{ wallMm: -1, drains: 0, drainMm: 3 }, { wallMm: 1e30, drains: 0, drainMm: 3 }, { wallMm: NaN, drains: 0, drainMm: 3 },
      { wallMm: 0.1, drains: 0, drainMm: 3 }, { wallMm: 2, drains: 9, drainMm: 3 }, { wallMm: 2, drains: 4, drainMm: 0 }, { wallMm: 2, drains: 4, drainMm: 1e9 },
      { wallMm: 50, drains: 0, drainMm: 3 }, { wallMm: 2, drains: 4, drainMm: 20 }, { wallMm: 0.8, drains: 4, drainMm: 1 }, { wallMm: 0, drains: 0, drainMm: 3 }])
      await attack(b, "hollowing", "hollow_mesh", args, 180000);
    const r = await b.executeAsync((done) => {
      const I = window.__TAURI_INTERNALS__.invoke;
      const calls = [];
      for (let i = 0; i < 6; i++) {
        calls.push(I("hollow_mesh", { wallMm: 1 + i, drains: i % 3, drainMm: 3 }));
        calls.push(I("fix_mesh", { fix: { fillHoles: i % 2 === 0 } }));
        calls.push(I("undo", {}));
      }
      Promise.allSettled(calls).then((rs) => done(rs.filter((x) => x.status === "rejected").map((x) => JSON.stringify(x.reason)).slice(0, 3)));
    });
    const h = await health(b);
    log("hollowing", "18 concurrent hollows, repairs and undos", h.alive && !h.errors.length ? "ok" : "BROKEN", `${r.length} rejected ${r.join(" ").slice(0, 160)} ${h.why}`);
  },
  // Thickening: hostile minimums through fix_mesh, and in a burst with undos.
  async thickening(b) {
    for (const thickenMm of [-1, 0, 1e30, NaN, 10, 0.05, "1"]) await attack(b, "thickening", "fix_mesh", { fix: { thickenMm } }, 180000);
    const r = await b.executeAsync((done) => {
      const I = window.__TAURI_INTERNALS__.invoke;
      const calls = [];
      for (let i = 0; i < 8; i++) {
        calls.push(I("fix_mesh", { fix: { thickenMm: (i % 4) * 0.5 } }));
        calls.push(I("undo", {}));
      }
      Promise.allSettled(calls).then((rs) => done(rs.filter((x) => x.status === "rejected").map((x) => JSON.stringify(x.reason)).slice(0, 3)));
    });
    const h = await health(b);
    log("thickening", "16 concurrent thickenings and undos", h.alive && !h.errors.length ? "ok" : "BROKEN", `${r.length} rejected ${r.join(" ").slice(0, 160)} ${h.why}`);
  },
  // Malla's view helpers: hostile rays and tolerances, wireframe bursts, keys pressed across modes.
  async meshView(b) {
    const g = await box(b);
    for (const [origin, dir, toleranceMm] of [[above(g), [0, 0, -1], 0.001], [above(g), [0, 0, 0], 0.001], [above(g), [0, 0, -1], 0], [above(g), [0, 0, -1], BIG],
      [[BIG, BIG, BIG], [-1, -1, -1], 0.001], [[NaN, 0, 0], [0, 0, -1], 0.001], [at(g, 0.5), [1, 0, 0], 0.001]])
      await attack(b, "meshView", "measure_thickness", { origin, dir, toleranceMm });
    const r = await b.executeAsync((done) => {
      const I = window.__TAURI_INTERNALS__.invoke;
      const calls = [];
      for (let i = 0; i < 10; i++) {
        calls.push(I("wire_mesh", {}));
        calls.push(I("set_mesh_ops", { keepRatio: i % 2 ? 0.5 : 1, flattenMm: 0, repair: true, minPieceMm: 0 }));
      }
      Promise.allSettled(calls).then((rs) => done(rs.filter((x) => x.status === "rejected").map((x) => JSON.stringify(x.reason)).slice(0, 3)));
    });
    const h = await health(b);
    log("meshView", "20 concurrent wireframes and simplifications", h.alive && !h.errors.length ? "ok" : "BROKEN", `${r.length} rejected ${r.join(" ").slice(0, 160)} ${h.why}`);
    for (const k of ["z", "k", "t", "c", "k", "z", "b", "z", "k", "t"]) await b.execute((kk) => window.dispatchEvent(new KeyboardEvent("keydown", { key: kk, bubbles: true })), k);
    for (let i = 0; i < 5; i++) for (const sel of ["#toggle-wire", 'button[data-tool="clip"]', "#clip-flip", "#clip-off", '[role=tab][data-tab=cut]', '[role=tab][data-tab=mesh]']) await click(b, sel);
    await settle(b);
    const h2 = await health(b);
    log("meshView", "helper keys and buttons across modes", h2.alive && !h2.errors.length ? "ok" : "BROKEN", `${h2.why} ${h2.errors[0] ?? ""}`.slice(0, 200));
  },
  // Cuts, in each branch's signature (the missing ones report "missing").
  async cuts(b) {
    for (const t of [0, 1, -1, 2, 0.5]) await attack(b, "cuts", "cut_model", { axis: 2, t, keepLow: true, keepOther: true, joint: null });
    for (let i = 0; i < 10; i++) await call(b, "undo_cut");
    await attack(b, "cuts", "cut_model", { axis: 9, t: 0.5, keepLow: true, keepOther: false, joint: null });
    // Plane cuts (#41 stack), on the real model: flat, tilted, grazing, degenerate.
    const g = await box(b);
    for (const [fz, normal] of [[0.5, [0, 0, 0]], [0.5, [0, 0, 1]], [0.5, [1e-20, 0, 0]], [0.5, [BIG, BIG, BIG]], [0.5, [1, 1, 1]], [0.9999, [0, 0, 1]], [0.0001, [0, 0, -1]], [0.55, [0.01, 0, 1]], [0.5, [0, 1, 0]]]) {
      await attack(b, "cuts", "cut_model", { point: at(g, fz), normal, keepLow: true, keepOther: true, layOnCut: true, selected: [] });
    }
    await attack(b, "cuts", "cut_model", { point: at(g, 0.5), normal: [0, 0, 1], keepLow: true, keepOther: true, layOnCut: true, selected: [999] });
    await attack(b, "cuts", "preview_cut", { point: [0, 0, BIG], normal: [0, 0, 1], keepLow: true, keepOther: true, layOnCut: false, selected: [] });
    // Twenty cuts in a row, keeping both sides each time: many objects.
    for (let i = 1; i <= 20; i++) await call(b, "cut_model", { point: at(g, i / 21), normal: [0, 0, 1], keepLow: true, keepOther: true, layOnCut: false, selected: [] });
    await attack(b, "cuts", "export_3mf", { path: path.join(OUT, "many.3mf"), lang: "es", objects: null });
    await attack(b, "cuts", "fit_to_plate", {});
    for (let i = 0; i < 10; i++) await call(b, "undo_cut");
    // Everything cut away, then everything else.
    await attack(b, "cuts", "cut_model", { axis: 2, t: 0.001, keepLow: true, keepOther: false, joint: null });
    for (let i = 0; i < 30; i++) await call(b, "undo_cut");
    await attack(b, "cuts", "cut_model", { point: at(g, 0.0005), normal: [0, 0, 1], keepLow: true, keepOther: false, layOnCut: false, selected: [] });
    for (const [cmd, args] of [["set_mesh_ops", { keepRatio: 0.5, flattenMm: 0, repair: true, minPieceMm: 0 }], ["analyze_printability", { params: { extrusionMm: 0.42, perimeters: 2, wallMm: 0.8, detailMm: 0.4, componentMm: 1, overhangDeg: 50, contactMm2: 10, toleranceMm: 0.1, samples: 64 } }], ["fit_to_plate", {}], ["preview_mesh", {}], ["export_3mf", { path: path.join(OUT, "empty.3mf"), lang: "es", objects: null }], ["rotate", { axis: 0, degrees: 90 }]])
      await attack(b, "cuts", `${cmd} (after cutting all away)`.split(" ")[0], args);
    for (let i = 0; i < 10; i++) await call(b, "undo_cut");
  },
  // #34/#35 (#43, #44): connectors, split for the printer, separate pieces.
  async connectors(b) {
    const g = await box(b);
    const P = at(g, 0.4);
    for (const c of [connectors({ sizeMm: 0 }), connectors({ sizeMm: BIG }), connectors({ depthMm: -5 }), connectors({ clearanceMm: 50 }), connectors({ at: Array(500).fill(P) }), connectors({ at: [[BIG, BIG, BIG]] })]) {
      await attack(b, "connectors", "suggest_connectors", { point: P, normal: [0, 0, 1], selected: [], connectors: c });
      await attack(b, "connectors", "preview_connected_cut", { point: P, normal: [0, 0, 1], layOnCut: true, selected: [], connectors: c });
    }
    // Two connectors on the same spot, one off the face, then the auto suggestion.
    await attack(b, "connectors", "cut_connected", { point: P, normal: [0, 0, 1], layOnCut: true, selected: [], connectors: connectors({ at: [P, P] }), dowelLabel: "x" });
    await attack(b, "connectors", "cut_connected", { point: P, normal: [0, 0, 1], layOnCut: true, selected: [], connectors: connectors({ at: [[g.hi[0] * 5, g.mid[1], P[2]]] }), dowelLabel: "x" });
    const sug = await b.executeAsync((p, done) => window.__TAURI_INTERNALS__.invoke("suggest_connectors", { point: p, normal: [0, 0, 1], selected: [], connectors: { kind: "pin", pinsOnA: true, shape: "square", sizeMm: 4, depthMm: 4, clearanceMm: 0.15, angleDeg: 0, at: [] } }).then(done, () => done(null)), P);
    if (sug) await attack(b, "connectors", "cut_connected", { point: P, normal: [0, 0, 1], layOnCut: true, selected: [], connectors: connectors({ kind: "pin", shape: "square", sizeMm: 4, depthMm: 4, at: sug }), dowelLabel: "pin" });
    // A cut with connectors on the thin neck, where there is no room for them.
    await attack(b, "connectors", "cut_connected", { point: at(g, 0.6), normal: [0, 0, 1], layOnCut: true, selected: [], connectors: connectors({ sizeMm: 30, depthMm: 30, at: [at(g, 0.6)] }), dowelLabel: "x" });
    for (let i = 0; i < 5; i++) await call(b, "undo_cut");
    await attack(b, "split", "propose_split", { gapMm: -1 });
    await attack(b, "split", "propose_split", { gapMm: BIG });
    for (const p of [plan({ cuts: [[], [], []] }), plan({ cuts: [Array(200).fill(10), [], []] }), plan({ cuts: [[BIG], [-BIG], [0]] }), plan({ gapMm: -5 }), plan({ dowels: { shape: "round", sizeMm: 0, depthMm: 0, clearanceMm: 0 } })]) {
      await attack(b, "split", "preview_split", { plan: p });
    }
    await attack(b, "split", "split_for_printer", { plan: plan({ cuts: [[g.mid[0], g.mid[0], g.mid[0]], [], [at(g, 0.3)[2], at(g, 0.3)[2]]] }), dowelLabel: "" }, 120000);
    for (let i = 0; i < 10; i++) await call(b, "undo_cut");
    await attack(b, "split", "set_size", { mm: [900, 900, 900] });
    const prop = await b.executeAsync((done) => window.__TAURI_INTERNALS__.invoke("propose_split", { gapMm: 5 }).then(done, (e) => done({ err: JSON.stringify(e) })));
    if (prop && prop.cuts) {
      await attack(b, "split", "preview_split", { plan: plan({ cuts: prop.cuts, dowels: { shape: "round", sizeMm: 6, depthMm: 6, clearanceMm: 0.2 } }) }, 300000);
      await attack(b, "split", "split_for_printer", { plan: plan({ cuts: prop.cuts, dowels: { shape: "round", sizeMm: 6, depthMm: 6, clearanceMm: 0.2 } }), dowelLabel: "d" }, 300000);
    } else log("split", "propose_split at 900 mm", "rejected", JSON.stringify(prop).slice(0, 120));
    for (let i = 0; i < 10; i++) await call(b, "undo_cut");
    await attack(b, "separate", "detect_pieces", { weldMm: -1 });
    await attack(b, "separate", "detect_pieces", { weldMm: BIG });
    await attack(b, "separate", "separate_pieces", { weldMm: 0, groups: [], layFlat: true });
    await attack(b, "separate", "separate_pieces", { weldMm: 0, groups: Array(100).fill(7), layFlat: true });
    await attack(b, "separate", "propose_cuts", { selected: [99] });
  },
  // #37/#38 (#45, #46): region selection and extraction.
  async region(b) {
    const g = await box(b);
    await attack(b, "region", "select_region", { origin: [0, 0, 500], dir: [0, 0, -1], radiusMm: BIG, add: true, through: true });
    await attack(b, "region", "select_region", { origin: above(g), dir: [0, 0, -1], radiusMm: 100, add: true, through: true });
    for (const op of ["grow", "smooth", "connected", "invert", "invert", "get"]) await attack(b, "region", "selection_op", { op });
    for (const closure of [{ kind: "shell", mm: 0 }, { kind: "shell", mm: BIG }, { kind: "cap", mm: -BIG }, { kind: "shell", mm: 1, clearanceMm: -3 }])
      await attack(b, "region", "preview_region", { closure, action: { kind: "extract", layFlat: true } });
    await attack(b, "region", "apply_region", { closure: { kind: "cap", mm: 0 }, action: { kind: "remove" } });
    // Select everything, then extract it all as one shell: nothing is left of the model.
    await call(b, "selection_op", { op: "invert" });
    await attack(b, "region", "preview_region", { closure: { kind: "shell", mm: 50 }, action: { kind: "extract", layFlat: true, group: true } }, 120000);
    await attack(b, "region", "apply_region", { closure: { kind: "shell", mm: 0.01 }, action: { kind: "extract", layFlat: true, group: false } }, 120000);
    await attack(b, "region", "apply_region", { closure: { kind: "cap", mm: -50 }, action: { kind: "remove" } }, 120000);
    await attack(b, "region", "select_by_colour", { seed: 4e9, how: { kind: "similar", deltaE: -1 }, contiguous: true, add: true, base: null });
    await attack(b, "region", "select_by_colour", { seed: 0, how: { kind: "material" }, contiguous: false, add: true, base: [4e9, 1, 1] });
    await attack(b, "region", "refine_to_paint", {});
    await attack(b, "region", "refine_to_paint", {});
    await attack(b, "region", "undo", {});
  },
  // #67: cut with joints, split to fit.
  async joints(b) {
    for (const j of [joint({ sizeMm: 0 }), joint({ sizeMm: BIG }), joint({ depthMm: 0 }), joint({ toleranceMm: -1 }), joint({ count: 10000 }), joint({ kind: "dowel", shape: "hexagon", toleranceMm: 20 })])
      await attack(b, "joints", "cut_model", { axis: 2, t: 0.5, keepLow: true, keepOther: true, joint: j });
    for (let i = 0; i < 10; i++) await call(b, "undo_cut");
    await attack(b, "joints", "set_size", { mm: [900, 900, 900] });
    await attack(b, "joints", "pieces_to_fit", {});
    await attack(b, "joints", "split_to_fit", { joint: joint() }, 180000);
    await attack(b, "joints", "split_to_fit", { joint: joint() }, 180000);
    for (let i = 0; i < 40; i++) await call(b, "undo_cut");
  },
  // Exports (#68 and the kit stack).
  async exports(b) {
    for (const [t, s] of [[0, true], [-1, false], [BIG, true], [0.01, false]])
      await attack(b, "exports", "export_color_zip", { path: path.join(OUT, `c${t}.zip`), thicknessMm: t, mainSolid: s, lang: "xx" }, 120000);
    for (const o of [kit({ voxelMm: 0 }), kit({ voxelMm: 0.05 }), kit({ depthMm: BIG }), kit({ coreSlot: 99 }), kit({ pins: { diameterMm: 0, depthMm: BIG, clearanceMm: -1 } })])
      await attack(b, "exports", "export_color_zip", { path: path.join(OUT, "kit.zip"), options: o, lang: "es" }, 180000);
    await attack(b, "exports", "cancel_color_zip", {});
    await attack(b, "exports", "export_3mf", { path: "/nonexistent/dir/x.3mf", lang: "es", objects: [99999] });
    const g = await box(b);
    await attack(b, "exports", "bucket", { origin: above(g), dir: [0, 0, -1], slot: 250, region: true, add: true });
    await attack(b, "exports", "bucket", { origin: above(g), dir: [0, 0, -1], slot: 1, region: false, add: true });
    await attack(b, "exports", "bucket_pick", { origin: above(g), dir: [0, 0, 0], region: true, add: false });
  },
  // Many commands at once, as fast clicking does.
  async burst(b) {
    const r = await b.executeAsync((done) => {
      const I = window.__TAURI_INTERNALS__.invoke;
      const calls = [];
      for (let i = 0; i < 25; i++) {
        calls.push(I("set_size", { mm: [50 + i, 50 + i, 50 + i] }));
        calls.push(I("rotate", { axis: i % 3, degrees: 90 }));
        calls.push(I("set_mesh_ops", { keepRatio: i % 2 ? 0.5 : 1, flattenMm: 0, repair: true, minPieceMm: 0 }));
        calls.push(I("preview_mesh", {}));
      }
      Promise.allSettled(calls).then((rs) => done(rs.filter((x) => x.status === "rejected").map((x) => JSON.stringify(x.reason)).slice(0, 3)));
    });
    const h = await health(b);
    log("burst", "100 concurrent commands", h.alive && !h.errors.length ? "ok" : "BROKEN", `${r.length} rejected ${r.join(" ").slice(0, 160)} ${h.why}`);
  },
  // Junk typed into every number field of the window, as a user might.
  async forms(b) {
    const ids = await b.execute(() => [...document.querySelectorAll("input[type=number], input[type=range]")].map((el) => el.id).filter(Boolean));
    let broke = 0;
    for (const id of ids) {
      for (const v of ["-5", "0", "99999999", "abc", "", "1e400"]) {
        await setValue(b, `#${id}`, v);
      }
      await sleep(150);
      const h = await health(b);
      if (!h.alive || h.errors.length) { broke++; log("forms", `#${id}`, "BROKEN", `${h.why} ${h.errors[0] ?? ""}`.slice(0, 200)); }
    }
    await settle(b);
    log("forms", `${ids.length} number fields × 6 junk values`, broke ? "BROKEN" : "ok");
    await shot(b, "break-forms");
  },
  // Every primary button in the window, clicked five times quickly.
  async buttons(b) {
    const ids = await b.execute(() => [...document.querySelectorAll("button[id]")].filter((el) => !el.closest("dialog, .menubar, header")).map((el) => el.id)
      .filter((id) => !/^(choose|find-blender|version|toast|export)/.test(id)));
    let broke = 0;
    for (const id of ids) {
      for (let i = 0; i < 5; i++) await click(b, `#${id}`);
      await b.execute(() => document.querySelectorAll("dialog[open]").forEach((d) => d.close()));
      const h = await health(b);
      if (!h.alive || h.errors.length) { broke++; log("buttons", `#${id}`, "BROKEN", `${h.why} ${h.errors[0] ?? ""}`.slice(0, 200)); }
    }
    await settle(b);
    log("buttons", `${ids.length} buttons × 5 quick clicks`, broke ? "BROKEN" : "ok");
    await shot(b, "break-buttons");
  },
};

const only = process.argv.slice(2);
await services();
try {
  for (const name of Object.keys(attacks)) {
    if (only.length && !only.includes(name)) continue;
    console.log(`▸ ${name}`);
    let b;
    try {
      b = await session(MODEL);
    } catch (e) {
      log(name, "app session starts", "BROKEN", String(e.message ?? e).split("\n")[0]);
      continue;
    }
    try {
      await b.execute(() => {
        window.__bpErrors = [];
        window.addEventListener("error", (e) => window.__bpErrors.push(String(e.message)));
        window.addEventListener("unhandledrejection", (e) => window.__bpErrors.push(`unhandled: ${JSON.stringify(e.reason)}`));
      });
      await waitState(b, "ready");
      await attacks[name](b);
    } catch (e) {
      log(name, "attack ran", "BROKEN", String(e.message ?? e).split("\n")[0]);
      await shot(b, `break-fail-${name}`).catch(() => {});
    } finally {
      await b.deleteSession().catch(() => {});
    }
  }
} finally {
  report("break", { model: MODEL, results });
  stop();
}
const broken = results.filter((r) => r.verdict === "BROKEN" || r.verdict === "HANG");
console.log(`\n${broken.length} broken/hung of ${results.length} → ${OUT}`);
