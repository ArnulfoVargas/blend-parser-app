// E2E scenarios over the real app: Rust backend, Blender import, WebKitGTK webview.
//
//   cargo build -p blend-parser-app && (cd e2e && npm install && npm run fixtures && npm test)
//
// Writes screenshots and a JSON report (checks, command timings, layout metrics)
// to e2e/out. A scenario whose control is missing on this branch is skipped, not failed,
// so the same run compares branches.
import path from "node:path";
import { readFileSync } from "node:fs";
import { inflateRawSync } from "node:zlib";
import {
  HERE, OUT, services, stop, session, waitState, settle, shot, report, times, probe,
  click, exists, visible, text, setValue, key, sleep,
} from "./lib.mjs";

const MODEL = process.env.BP_MODEL ?? path.join(HERE, "e2e/fixtures/out/figure.blend");
/** Suzanne's head alone, open at the eyes and the neck (`fixtures/simple.py`). */
const HEAD = path.join(HERE, "e2e/fixtures/out/head.blend");
/** A closed cylinder, 44 × 45 mm (`fixtures/simple.py`). */
const CYLINDER = path.join(HERE, "e2e/fixtures/out/cylinder.blend");
/** A 20 mm block with a 0.5 mm fin and a 0.6 mm pin on it (`fixtures/simple.py`). */
const THIN = path.join(HERE, "e2e/fixtures/out/thin.blend");
/** The `3D/Objects/*.model` files of a 3MF, as text. */
function objectModels(file) {
  const buf = readFileSync(file);
  const end = buf.lastIndexOf(Buffer.from([0x50, 0x4b, 0x05, 0x06]));
  let at = buf.readUInt32LE(end + 16);
  const out = [];
  for (let n = buf.readUInt16LE(end + 10); n > 0; n--) {
    const [method, size, nameLen, extra, comment, local] = [buf.readUInt16LE(at + 10), buf.readUInt32LE(at + 20), buf.readUInt16LE(at + 28), buf.readUInt16LE(at + 30), buf.readUInt16LE(at + 32), buf.readUInt32LE(at + 42)];
    const name = buf.toString("utf8", at + 46, at + 46 + nameLen);
    if (/^3D\/Objects\/.*\.model$/.test(name)) {
      const start = local + 30 + buf.readUInt16LE(local + 26) + buf.readUInt16LE(local + 28);
      const data = buf.subarray(start, start + size);
      out.push((method === 8 ? inflateRawSync(data) : data).toString("utf8"));
    }
    at += 46 + nameLen + extra + comment;
  }
  return out;
}
/** Every triangle of a 3MF by its corners (to the micrometre), with its paint. */
function paintedTriangles(file) {
  const tris = new Map();
  for (const xml of objectModels(file)) {
    const v = [...xml.matchAll(/<vertex x="([^"]+)" y="([^"]+)" z="([^"]+)"/g)].map((m) => [m[1], m[2], m[3]].map((x) => Number(x).toFixed(3)).join(","));
    for (const m of xml.matchAll(/<triangle v1="(\d+)" v2="(\d+)" v3="(\d+)"(?: paint_color="([^"]*)")?/g)) {
      tris.set([v[m[1]], v[m[2]], v[m[3]]].sort().join("|"), m[4] ?? "");
    }
  }
  return tris;
}
const invoke = (b, cmd, args = {}) => b.executeAsync((c, a, done) => window.__TAURI_INTERNALS__.invoke(c, a).then((v) => done({ v }), (e) => done({ e: JSON.stringify(e) })), cmd, args);
const only = process.argv.slice(2);
const results = [];

function check(scenario, name, ok, detail = "") {
  results.push({ scenario, name, ok: ok === true ? "pass" : ok === null ? "skip" : "fail", detail });
  const mark = ok === true ? "✔" : ok === null ? "–" : "✘";
  console.log(`  ${mark} ${name}${detail ? ` — ${detail}` : ""}`);
}

/** How crowded the window is: what a user has to scan and scroll through. */
async function layout(b) {
  return b.execute(() => {
    const vis = (el) => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0 && !el.closest("[hidden], dialog:not([open])"); };
    const inter = "button, input, select, textarea, summary, [role=tab]";
    const side = document.querySelector(".side");
    const sideControls = side ? [...side.querySelectorAll(inter)].filter(vis).length : 0;
    const allControls = [...document.querySelectorAll(inter)].filter(vis).length;
    const groups = side ? side.querySelectorAll("details.group").length : 0;
    const openGroups = side ? side.querySelectorAll("details.group[open]").length : 0;
    const foot = document.querySelector(".stage__foot");
    const small = [...document.querySelectorAll(inter)].filter(vis).filter((el) => { const r = el.getBoundingClientRect(); return r.height < 24 || r.width < 24; }).length;
    const unlabeled = [...document.querySelectorAll("button, input, select")].filter(vis).filter((el) => {
      const t = (el.getAttribute("aria-label") || el.textContent || el.getAttribute("title") || "").trim();
      const lab = el.id && document.querySelector(`label[for="${el.id}"]`);
      return !t && !lab && !el.closest("label");
    }).map((el) => el.id || el.outerHTML.slice(0, 80));
    return {
      sideControls, allControls, groups, openGroups,
      sideScreens: side ? +(side.scrollHeight / side.clientHeight).toFixed(2) : 0,
      footHeight: foot ? Math.round(foot.getBoundingClientRect().height) : 0,
      tabs: document.querySelectorAll("[role=tab]").length,
      smallTargets: small, unlabeled,
    };
  });
}

const scenarios = {
  async open(b) {
    const t0 = Date.now();
    await waitState(b, "ready");
    const ms = Date.now() - t0;
    await settle(b);
    check("open", "model converts and reaches ready", true, `${ms} ms to ready`);
    const tris = await text(b, "[data-bind=triangles]");
    check("open", "triangle count shown", !!tris, tris ?? "");
    await shot(b, "01-open");
    return { msToReady: ms, layout: await layout(b) };
  },

  async size(b) {
    await waitState(b, "ready");
    const before = await text(b, "[data-bind=dims]");
    await setValue(b, "#size-z", 120);
    await settle(b);
    const after = await text(b, "[data-bind=dims]");
    check("size", "typing a height resizes", before !== after && /120/.test(after ?? ""), `${before} → ${after}`);
    await click(b, "#size-undo");
    await settle(b);
    check("size", "undo size restores", (await text(b, "[data-bind=dims]")) === before);
    await shot(b, "02-size");
  },

  async cut(b) {
    await waitState(b, "ready");
    // Reach the cut: the tab if the branch has one, else the viewport tool.
    const viaTab = await click(b, "[role=tab][data-tab=cut]");
    if (!viaTab) await click(b, "button[data-tool=section]");
    await settle(b);
    const panel = await visible(b, "#cut-apply");
    check("cut", "cut controls visible", panel, viaTab ? "via Cortes tab" : "via viewport tool (C)");
    if (!panel) return;
    await click(b, "[data-cut-axis='2']");
    await setValue(b, "#cut-pos", 40, "input");
    await settle(b);
    await shot(b, "03-cut-preview");
    await click(b, "#cut-keep-other");
    await click(b, "#cut-apply");
    await settle(b);
    const undoOn = await b.execute(() => !document.querySelector("#cut-undo")?.disabled);
    check("cut", "cut applies and can be undone", undoOn);
    await shot(b, "04-cut-done");
    await click(b, "#cut-undo");
    await settle(b);
  },

  async mesh(b) {
    await waitState(b, "ready");
    await click(b, "[role=tab][data-tab=mesh]");
    const before = await text(b, "[data-bind=triangles]");
    await b.execute(() => document.querySelectorAll("details.group").forEach((d) => d.querySelector("#keep") && (d.open = true)));
    await setValue(b, "#keep", 50);
    await settle(b);
    const after = await text(b, "[data-bind=triangles]");
    check("mesh", "simplify to 50 % lowers the triangles", before !== after, `${before} → ${after}`);
    await shot(b, "05-mesh");
    await setValue(b, "#keep", 100);
    await settle(b);
  },

  async printability(b) {
    await waitState(b, "ready");
    await click(b, "[role=tab][data-tab=mesh]");
    await b.execute(() => { const d = document.querySelector("#printability"); if (d) d.open = true; });
    const ok = await click(b, "#printability-run");
    if (!ok) return check("printability", "analysis button", null);
    await b.waitUntil(async () => (await b.execute(() => document.querySelectorAll(".pa-results > *").length)) > 0, { timeout: 120000 }).catch(() => {});
    const n = await b.execute(() => document.querySelectorAll(".pa-results > *").length);
    check("printability", "analysis lists results", n > 0, `${n} cards`);
    await shot(b, "06-printability");
  },

  async brush(b) {
    await waitState(b, "ready");
    await click(b, "#slots li:nth-child(2) button");
    await key(b, "b");
    const canvas = await b.$("#viewport");
    const { width, height } = await canvas.getSize();
    // A short stroke across the model's middle.
    await b.action("pointer").move({ origin: canvas, x: -20, y: 0 }).down().move({ origin: canvas, x: 20, y: 10, duration: 300 }).up().perform().catch(() => {});
    await settle(b);
    const canUndo = await b.execute(() => !document.querySelector("#undo")?.disabled);
    check("brush", "a stroke paints and can be undone", canUndo, `${width}×${height} canvas`);
    await shot(b, "07-brush");
    await key(b, "o");
  },

  async exportFile(b) {
    await waitState(b, "ready");
    const file = path.join(OUT, "e2e.3mf");
    const err = await b.executeAsync((p, done) => window.__TAURI_INTERNALS__.invoke("export_3mf", { path: p, lang: "es", objects: null, thumbnail: null }).then(() => done(null), (e) => done(JSON.stringify(e))), file);
    check("export", "3MF is written", err === null, err ?? file);
  },

  async a11y(b) {
    await waitState(b, "ready");
    // Keyboard: how many Tab presses until the primary action takes focus.
    const presses = await b.execute(() => {
      const f = [...document.querySelectorAll("a[href], button:not([disabled]), input:not([disabled]), select, summary, [tabindex]:not([tabindex='-1'])")]
        .filter((el) => { const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0; });
      return f.indexOf(document.querySelector("#export")) + 1;
    });
    check("a11y", "Tab stops before Export", presses > 0, String(presses));
    const l = await layout(b);
    check("a11y", "every visible control has a name", l.unlabeled.length === 0, l.unlabeled.slice(0, 5).join(", "));
    return { tabStopsToExport: presses, layout: l };
  },

  // The panels: tab keys, the cut tool opening its tab, a typed cut height.
  async tabs(b) {
    await waitState(b, "ready");
    if (!(await exists(b, "[role=tab][data-tab=mesh]"))) return check("tabs", "settings tabs", null);
    const sel = () => b.execute(() => document.querySelector("[role=tab][aria-selected=true]")?.dataset.tab);
    const l0 = { ...(await layout(b)), canvas: await b.execute(() => Math.round(document.querySelector("#viewport").getBoundingClientRect().width)) };
    await b.execute(() => document.querySelector("[role=tab][data-tab=model]").focus());
    await b.keys("ArrowRight");
    check("tabs", "→ moves to Malla", (await sel()) === "mesh");
    const view = () => b.execute(() => ({ canvas: Math.round(document.querySelector("#viewport").getBoundingClientRect().width), ams: !!document.querySelector(".ams")?.offsetParent, inBar: !!document.querySelector("[role=tab]").closest(".bar") }));
    await sleep(400);
    const vm = await view();
    check("tabs", "modes live in the top bar, as Blender's workspaces", vm.inBar);
    check("tabs", "Malla hides the AMS and widens the view", !vm.ams && vm.canvas > (l0.canvas ?? 0), `${l0.canvas} → ${vm.canvas} px`);
    check("tabs", "Malla holds repair, simplify, base and printability", await b.execute(() => ["#repair", "#keep", "#flatten", "#printability-run"].every((s) => document.querySelector(s)?.closest("#panel-mesh"))));
    const tools = () => b.execute(() => [...document.querySelectorAll(".toolbar__btn[data-tool]")].filter((el) => el.offsetParent).map((el) => el.dataset.tool).join(","));
    const meshTools = (await exists(b, '.toolbar__btn[data-tool="clip"]')) ? "orbit,measure,move,rotate,clip,thickness" : "orbit,measure,move,rotate";
    check("tabs", "Malla's view offers orbit, measure, move and turn", (await tools()) === meshTools, await tools());
    await shot(b, "10-tab-mesh");
    await key(b, "c");
    check("tabs", "Cortes' view offers orbit, measure and the cut", (await tools()) === "orbit,measure,section", await tools());
    check("tabs", "C opens Cortes with the cut on", (await sel()) === "cut" && (await b.execute(() => document.querySelector("[data-app]").dataset.tool)) === "section");
    await setValue(b, "#cut-mm", 30);
    await settle(b);
    const t = await b.execute(() => document.querySelector("#cut-pos").value);
    check("tabs", "typing 30 mm moves the slider", Math.abs(Number(t) - (30 / 80.78) * 100) < 1, `${t} %`);
    await setValue(b, "#cut-mm", "abc");
    check("tabs", "junk height leaves the cut where it was", (await b.execute(() => document.querySelector("#cut-pos").value)) === t);
    const help = await text(b, "[data-bind=cutPosHelp]");
    check("tabs", "says which side stays", /abajo|bottom/.test(help ?? ""), help ?? "");
    await shot(b, "11-tab-cut");
    await click(b, "#cut-apply");
    await settle(b);
    check("tabs", "after cutting the tab stays on Cortes, undo is on", (await sel()) === "cut" && (await b.execute(() => !document.querySelector("#cut-undo").disabled)));
    await click(b, "[role=tab][data-tab=mesh]");
    await key(b, "g");
    check("tabs", "move options sit in the side panel in an editing mode", await b.execute(() => !!document.querySelector('#tool-options .place[data-for="move"]')?.offsetParent));
    await b.execute(() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "PageUp", ctrlKey: true, bubbles: true })));
    check("tabs", "Ctrl+PageUp goes back a mode", (await sel()) === "model");
    await click(b, "[role=tab][data-tab=model]");
    check("tabs", "the model mode shows the AMS again and moves the options back under the view", await b.execute(() => !!document.querySelector(".ams").offsetParent && !!document.querySelector('.stage__foot .place[data-for="move"]')));
    await key(b, "o");
    await click(b, "[role=tab][data-tab=cut]");
    await key(b, "b");
    check("tabs", "the brush key from Cortes opens Modelo with the brush", (await sel()) === "model" && (await b.execute(() => document.querySelector("[data-app]").dataset.tool)) === "brush");
    await key(b, "o");
    check("tabs", "leaving Cortes puts the view back to orbit", (await b.execute(() => document.querySelector("[data-app]").dataset.tool)) === "orbit");
    const l1 = await layout(b);
    return { before: l0, after: l1 };
  },

  // Malla → Reparar: one switch per repair, its count before and after, one undo step each.
  async repairs(b) {
    await waitState(b, "ready");
    if (!(await exists(b, '#repair-ops input[data-op="fillHoles"]'))) return check("repairs", "repair switches", null);
    await click(b, "[role=tab][data-tab=mesh]");
    const line = (op) => text(b, `#repair-ops [data-count="${op}"]`);
    await b.waitUntil(async () => !/Contando|Counting/.test((await line("fillHoles")) ?? "Contando"), { timeout: 60000, interval: 200 });
    const found = await line("fillHoles");
    check("repairs", "the open head shows its holes", /\d+ agujeros?, \d+ aristas abiertas/.test(found ?? ""), found ?? "");
    const lines = await b.execute(() => [...document.querySelectorAll("#repair-ops [data-count]")].map((el) => `${el.dataset.count}: ${el.textContent}`));
    check("repairs", "every repair says what it finds", lines.every((l) => l.split(": ")[1]?.length > 0), lines.join(" | "));
    await shot(b, "12-repairs-before");
    await click(b, '#repair-ops input[data-op="fillHoles"]');
    await settle(b);
    await b.waitUntil(async () => /Ahora|Now/.test((await line("fillHoles")) ?? ""), { timeout: 60000, interval: 200 }).catch(() => {});
    const after = await line("fillHoles");
    check("repairs", "Rellenar agujeros closes the head", /Ahora: sin agujeros/.test(after ?? ""), after ?? "");
    const issues = await invoke(b, "mesh_issues");
    check("repairs", "no open edge is left", issues.v?.after.openEdges === 0 && issues.v?.before.holes > 0, JSON.stringify(issues.v ?? issues.e));
    const sum = await b.execute(() => document.querySelector("#undo").title);
    check("repairs", "the change is the next undo step", /malla|mesh|corrección|fix/i.test(sum), sum);
    await shot(b, "13-repairs-after");
    await b.execute(() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "z", ctrlKey: true, bubbles: true })));
    await settle(b);
    const back = await b.execute(() => document.querySelector('#repair-ops input[data-op="fillHoles"]').checked);
    const reopened = await invoke(b, "mesh_issues");
    check("repairs", "Ctrl+Z opens the holes again and unticks the box", !back && reopened.v?.after.holes > 0, `${reopened.v?.after.holes} holes`);
    await click(b, '#repair-ops input[data-op="orient"]');
    await settle(b);
    const oriented = await line("orient");
    check("repairs", "each other switch applies on its own", /Antes|Before/.test(oriented ?? "") && !(await b.execute(() => document.querySelector('#repair-ops input[data-op="fillHoles"]').checked)), oriented ?? "");
  },

  // Malla → Orientación: the best turn for the figure upside down, what it gains, the face it rests on.
  async orient(b) {
    await waitState(b, "ready");
    if (!(await exists(b, "#orient-best"))) return check("orient", "best orientation button", null);
    // Upside down: two quarter turns about X.
    for (let i = 0; i < 2; i++) { await click(b, 'button[data-turn="0"]'); await settle(b); }
    await click(b, "[role=tab][data-tab=mesh]");
    const gain = () => text(b, "[data-bind=orientGain]");
    await b.waitUntil(async () => /→|ya está/i.test((await gain()) ?? ""), { timeout: 120000, interval: 200 });
    const g = (await gain()) ?? "";
    const nums = (g.match(/[\d.,]+(?= cm²)/g) ?? []).map((x) => Number(x.replace(/\./g, "").replace(",", ".")));
    check("orient", "says what the turn gains", /Voladizo: .* cm² → .* cm²; base: .* → .* mm²/.test(g) && nums[1] < nums[0], g);
    check("orient", "tints the face that will rest on the plate", await visible(b, "#orient-legend"));
    await shot(b, "14-orient-preview");
    await click(b, "#orient-best");
    await settle(b);
    await b.waitUntil(async () => /ya está/i.test((await gain()) ?? ""), { timeout: 120000, interval: 200 }).catch(() => {});
    check("orient", "after the turn there is nothing better", /ya está/i.test((await gain()) ?? ""), (await gain()) ?? "");
    const low = await b.executeAsync((done) => window.__TAURI_INTERNALS__.invoke("preview_mesh", {}).then((buf) => {
      const leaves = new DataView(buf, 0, 8).getUint32(0, true);
      const p = new Float32Array(buf, 8, leaves * 9);
      let lo = Infinity;
      for (let i = 2; i < p.length; i += 3) lo = Math.min(lo, p[i]);
      done(lo);
    }));
    check("orient", "the turned model rests on the plate", Math.abs(low) < 1e-3, `lowest z ${low}`);
    check("orient", "the tint goes once applied", !(await visible(b, "#orient-legend")));
    await shot(b, "15-orient-done");
    const undo = await b.execute(() => document.querySelector("#undo").title);
    await b.execute(() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "z", ctrlKey: true, bubbles: true })));
    await settle(b);
    await b.waitUntil(async () => /→/.test((await gain()) ?? ""), { timeout: 120000, interval: 200 }).catch(() => {});
    check("orient", "Ctrl+Z turns it back", /→/.test((await gain()) ?? ""), undo);
    await click(b, "#orient-reset");
    await settle(b);
    check("orient", "Orientación original is at hand", await b.execute(() => document.querySelector("#orient-reset").disabled), "disabled once back");
  },

  // Malla → Mostrar: overhangs and edges painted on the model from the analysis, without measuring again.
  async analysis(b) {
    await waitState(b, "ready");
    if (!(await exists(b, "button[data-analysis]"))) return check("analysis", "analysis views", null);
    await click(b, "[role=tab][data-tab=mesh]");
    const pressed = () => b.execute(() => document.querySelector("button[data-analysis][aria-pressed=true]")?.dataset.analysis);
    const legend = () => b.execute(() => { const el = document.querySelector("#analysis-legend"); return el && !el.hidden ? el.textContent.trim() : ""; });
    const cards = () => b.execute(() => document.querySelectorAll(".pa-results .pa-check").length);
    await click(b, 'button[data-analysis="overhang"]');
    await settle(b);
    check("analysis", "Voladizos paints at once, from the faces alone, with the angle in the legend", (await pressed()) === "overhang" && /45°/.test(await legend()) && (await cards()) === 0, await legend());
    await shot(b, "16-analysis-overhang");
    // Edges need a report: picking the view runs the analysis.
    await click(b, 'button[data-analysis="edges"]');
    await b.waitUntil(async () => (await pressed()) === "edges", { timeout: 120000, interval: 200 }).catch(() => {});
    check("analysis", "Bordes analyses and draws the open edges", (await pressed()) === "edges" && (await cards()) > 0 && /\d+ aristas abiertas/.test(await legend()), await legend());
    await shot(b, "17-analysis-edges");
    // A new analysis would replace the cards: a mark on one tells whether it ran again.
    await b.execute(() => (document.querySelector(".pa-results .pa-check").dataset.mark = "1"));
    await click(b, 'button[data-analysis="overhang"]');
    const picked = await b.execute(() => {
      const card = [...document.querySelectorAll(".pa-check.is-viewable")].find((c) => /abiert/i.test(c.textContent));
      card?.querySelector(".pa-check__title").click();
      return !!card;
    });
    await settle(b);
    check("analysis", "a click on the open-edges card switches to Bordes", picked && (await pressed()) === "edges");
    check("analysis", "switching views measures nothing again", await b.execute(() => !!document.querySelector('.pa-results .pa-check[data-mark="1"]')));
    await click(b, '#repair-ops input[data-op="fillHoles"]');
    await settle(b);
    check("analysis", "a change to the mesh drops the edges view (its report is old)", (await pressed()) === "none");
    await click(b, 'button[data-analysis="overhang"]');
    await click(b, "[role=tab][data-tab=model]");
    check("analysis", "leaving Malla shows the colours again", (await pressed()) === "none" && !(await legend()));
    await click(b, "#slots li:nth-child(2) button");
    await key(b, "b");
    const canvas = await b.$("#viewport");
    await b.action("pointer").move({ origin: canvas, x: -20, y: 0 }).down().move({ origin: canvas, x: 20, y: 10, duration: 300 }).up().perform().catch(() => {});
    await settle(b);
    const undo = await b.execute(() => document.querySelector("#undo").title);
    check("analysis", "the brush paints after coming back to Modelo", /trazo|stroke/.test(undo), undo);
    await shot(b, "19-analysis-brush");
    await key(b, "o");
  },

  // Grosor on a closed model: the closed cylinder of `simple.py`.
  async thickness(b) {
    await waitState(b, "ready");
    if (!(await exists(b, 'button[data-analysis="thickness"]'))) return check("thickness", "thickness view", null);
    await click(b, "[role=tab][data-tab=mesh]");
    await click(b, 'button[data-analysis="thickness"]');
    const pressed = () => b.execute(() => document.querySelector("button[data-analysis][aria-pressed=true]")?.dataset.analysis);
    await b.waitUntil(async () => (await pressed()) === "thickness", { timeout: 120000, interval: 200 }).catch(() => {});
    const leg = await b.execute(() => document.querySelector("#analysis-legend").textContent.trim());
    check("thickness", "Grosor colours the walls against two nozzle lines", (await pressed()) === "thickness" && /Más fina que [\d,]+ mm \(2 líneas de la boquilla de 0,4 mm\)/.test(leg), leg);
    await shot(b, "18-analysis-thickness");
  },

  // Simplify at 50 % with the colour borders kept: no more filament changes than before, and the deviation shown.
  async simplify(b) {
    await waitState(b, "ready");
    if (!(await exists(b, "#protect-colors"))) return check("simplify", "protect switches", null);
    await click(b, "[role=tab][data-tab=mesh]");
    const swaps = () => b.execute(() => Number(document.querySelector("[data-bind=swaps]").textContent.replace(/\D/g, "")));
    const full = await swaps();
    await setValue(b, "#keep", 50);
    await settle(b);
    const plain = await swaps();
    const plainLine = await text(b, "[data-bind=triangles]");
    await click(b, "#protect-colors");
    await settle(b);
    const kept = await swaps();
    const line = await text(b, "[data-bind=triangles]");
    check("simplify", "with Proteger bordes de color the filament changes do not go up", kept <= full, `100 %: ${full}, 50 %: ${plain}, 50 % protegido: ${kept}`);
    check("simplify", "the maximum deviation is shown beside the triangles", /desviación máx\. [\d,]+ mm/.test(line ?? ""), `${plainLine} | ${line}`);
    await click(b, "#protect-detail");
    await settle(b);
    check("simplify", "Proteger detalle applies too", await b.execute(() => document.querySelector("#protect-detail").checked), await text(b, "[data-bind=triangles]"));
    await shot(b, "20-simplify-protected");
  },

  // Malla → Suavizar on the 48-sided cylinder: the count said first, 2 levels, the paint kept, Ctrl+Z.
  async smooth(b) {
    await waitState(b, "ready");
    if (!(await exists(b, "#smooth-apply"))) return check("smooth", "smooth section", null);
    await click(b, "[role=tab][data-tab=mesh]");
    await b.execute(() => (document.querySelector("#smooth-group").open = true));
    const summary = async () => (await invoke(b, "end_stroke")).v;
    const before = await summary();
    await click(b, "#subdiv-2");
    const estimate = await text(b, "[data-bind=smoothEstimate]");
    check("smooth", "the resulting triangles are said before applying", estimate?.includes(new Intl.NumberFormat("es").format(before.triangles * 16)) ?? false, estimate ?? "");
    await setValue(b, "#tri-cap", 1000);
    const over = await b.execute(() => ({ off: document.querySelector("#smooth-apply").disabled, text: document.querySelector("[data-bind=smoothEstimate]").textContent }));
    check("smooth", "over the cap it cannot be applied", over.off && /tope/.test(over.text), over.text);
    const refused = await invoke(b, "smooth_mesh", { subdivide: 2, smooth: 0, creaseDeg: 30, maxTriangles: 1000 });
    check("smooth", "the backend refuses it too", /subdivideCap/.test(refused.e ?? ""), refused.e ?? "accepted");
    await setValue(b, "#tri-cap", 2000000);
    await setValue(b, "#smooth-passes", 2);
    await click(b, "#smooth-apply");
    await settle(b);
    const after = await summary();
    check("smooth", "2 levels make every triangle sixteen", after.triangles === before.triangles * 16, `${before.triangles} → ${after.triangles}`);
    await shot(b, "21-smooth-cylinder");
    await b.execute(() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "z", ctrlKey: true, bubbles: true })));
    await settle(b);
    const back = await summary();
    check("smooth", "Ctrl+Z gives the original mesh back", back.triangles === before.triangles && back.settings.subdivide === 0, `${back.triangles}`);
  },

  // Suavizar on the painted figure: one level, each colour keeps its share of the surface.
  async smoothPaint(b) {
    await waitState(b, "ready");
    if (!(await exists(b, "#smooth-apply"))) return check("smoothPaint", "smooth section", null);
    const summary = async () => (await invoke(b, "end_stroke")).v;
    const before = await summary();
    const r = await invoke(b, "smooth_mesh", { subdivide: 1, smooth: 2, creaseDeg: 30, maxTriangles: 2000000 });
    const after = r.v;
    const share = (s) => s.slots.map((x) => x.triangles / s.triangles);
    const drift = Math.max(...share(before).map((v, i) => Math.abs(v - share(after)[i])));
    check("smoothPaint", "each colour keeps its share after subdividing", !!after && drift < 0.01, `${share(before).map((v) => v.toFixed(3))} → ${after && share(after).map((v) => v.toFixed(3))}`);
    check("smoothPaint", "filament changes do not go up", !!after && after.estimate.swaps <= before.estimate.swaps, `${before.estimate.swaps} → ${after?.estimate.swaps}`);
  },

  // Malla → Vaciar on the closed two-colour cylinder: the saving, the drains, the paint outside kept, thin walls flagged.
  async hollow(b) {
    await waitState(b, "ready");
    if (!(await exists(b, "#hollow-apply"))) return check("hollow", "hollow section", null);
    await click(b, "[role=tab][data-tab=mesh]");
    await b.execute(() => (document.querySelector("#hollow-group").open = true));
    const solidFile = path.join(OUT, "hollow-solid.3mf");
    await invoke(b, "export_3mf", { path: solidFile });
    await setValue(b, "#hollow-wall", 0.5);
    check("hollow", "a wall under two nozzle lines cannot be asked for", await b.execute(() => document.querySelector("#hollow-apply").disabled && document.querySelector("#hollow-wall").getAttribute("aria-invalid") === "true"), await text(b, "[data-bind=hollowWallHelp]"));
    await setValue(b, "#hollow-wall", 2);
    await click(b, "#hollow-apply");
    await settle(b);
    const result = await text(b, "[data-bind=hollowResult]");
    check("hollow", "says what it saves", /−\d+ % de volumen, −[\d,]+ g/.test(result ?? ""), result ?? "");
    const issues = (await invoke(b, "mesh_issues")).v;
    check("hollow", "the hollow model is still closed", issues?.after.openEdges === 0 && issues?.after.flippedTriangles === 0, JSON.stringify(issues?.after));
    const hollowFile = path.join(OUT, "hollow-2mm.3mf");
    await invoke(b, "export_3mf", { path: hollowFile });
    const [a, h] = [paintedTriangles(solidFile), paintedTriangles(hollowFile)];
    const kept = [...a].filter(([k, paint]) => h.get(k) === paint).length;
    check("hollow", "the 3MF keeps the outside and its paint as it was", kept === a.size && h.size > a.size, `${kept} of ${a.size} outer triangles unchanged; ${h.size} in all`);
    await shot(b, "22-hollow");
    // Two drains through the base, then a wall the printability check flags as thin.
    await click(b, "#drains-2");
    await click(b, "#hollow-apply");
    await settle(b);
    const drained = await text(b, "[data-bind=hollowResult]");
    const after = (await invoke(b, "mesh_issues")).v;
    check("hollow", "two drains through the base, still closed", /2 de 2 agujeros/.test(drained ?? "") && after?.after.openEdges === 0, drained ?? "");
    await setValue(b, "#hollow-wall", 0.8);
    await click(b, "#hollow-apply");
    await settle(b);
    const params = { extrusionMm: 0.44, perimeters: 2, wallMm: 0.88, detailMm: 0.44, componentMm: 2, overhangDeg: 45, contactMm2: 4, toleranceMm: 0.001, samples: 512 };
    const report = (await invoke(b, "analyze_printability", { params })).v;
    const thin = report?.checks.find((c) => c.code === "thinWalls");
    check("hollow", "printability flags the 0,8 mm wall as too thin", thin?.evidence === "estimated" && thin.count > 0, `${thin?.count} thin of ${report?.thickness.length}, min ${thin?.measurement?.toFixed(2)} mm`);
    await click(b, "#hollow-off");
    await settle(b);
    check("hollow", "Rellenar gives the solid back", await b.execute(() => document.querySelector("[data-bind=hollowResult]").textContent === ""));
  },

  // Engrosar from the thin-walls card: the fin and the pin reach the minimum, the block does not move.
  async thicken(b) {
    await waitState(b, "ready");
    if (!(await exists(b, "#thicken-apply"))) return check("thicken", "thicken section", null);
    await click(b, "[role=tab][data-tab=mesh]");
    // The block's points (under the fin and pin), to see that they stay where they are.
    const block = () => b.executeAsync((done) => window.__TAURI_INTERNALS__.invoke("preview_mesh", {}).then((buf) => {
      const leaves = new DataView(buf, 0, 8).getUint32(0, true);
      const p = new Float32Array(buf, 8, leaves * 9);
      const out = new Set();
      for (let i = 0; i < p.length; i += 3) if (p[i + 2] < 19) out.add(`${p[i].toFixed(4)},${p[i + 1].toFixed(4)},${p[i + 2].toFixed(4)}`);
      done([...out].sort().join(";"));
    }));
    const before = await block();
    await click(b, "#thicken-show");
    await b.waitUntil(async () => (await b.execute(() => document.querySelector("button[data-analysis][aria-pressed=true]")?.dataset.analysis)) === "thickness", { timeout: 120000, interval: 200 }).catch(() => {});
    check("thicken", "Ver zonas finas shows them in the thickness view", await b.execute(() => document.querySelector("button[data-analysis][aria-pressed=true]")?.dataset.analysis === "thickness"));
    await shot(b, "23-thicken-before");
    const button = await b.execute(() => {
      const card = [...document.querySelectorAll(".pa-check")].find((c) => /Espesor de pared/.test(c.textContent));
      const btn = card?.querySelector('[data-fix="thicken"]');
      return btn ? btn.textContent : null;
    });
    check("thicken", "the thin-walls card offers Engrosar", button === "Engrosar", button ?? "no button");
    await b.execute(() => document.querySelector('.pa-check [data-fix="thicken"]')?.click());
    await b.waitUntil(async () => (await b.execute(() => [...document.querySelectorAll(".pa-check")].some((c) => /Espesor de pared/.test(c.textContent) && c.dataset.severity === "fixed"))), { timeout: 120000, interval: 300 }).catch(() => {});
    const card = await b.execute(() => [...document.querySelectorAll(".pa-check")].find((c) => /Espesor de pared/.test(c.textContent))?.dataset.severity ?? "gone");
    check("thicken", "printability no longer flags thin walls", card === "fixed" || card === "gone", card);
    const result = await text(b, "[data-bind=thickenResult]");
    check("thicken", "says how much it moved and where", /Engrosó [\d,]+ cm² .*hasta [\d,]+ mm hacia fuera/.test(result ?? ""), result ?? "");
    check("thicken", "the block does not move at all", (await block()) === before);
    const issues = (await invoke(b, "mesh_issues")).v;
    check("thicken", "the mesh stays closed", issues?.after.openEdges === 0, JSON.stringify(issues?.after));
    await shot(b, "24-thicken-after");
    await click(b, "#thicken-off");
    await settle(b);
    check("thicken", "Quitar puts the details back", (await b.execute(() => document.querySelector("[data-bind=thickenResult]").textContent)) === "");
  },

  // Malla's view helpers on the hollowed cylinder: wireframe, a section that cuts nothing, the wall measured.
  async meshView(b) {
    await waitState(b, "ready");
    if (!(await exists(b, "#toggle-wire"))) return check("meshView", "mesh view helpers", null);
    await click(b, "[role=tab][data-tab=mesh]");
    await b.execute(() => (document.querySelector("#hollow-group").open = true));
    await setValue(b, "#hollow-wall", 2);
    await click(b, "#hollow-apply");
    await settle(b);
    await key(b, "z");
    await settle(b);
    const wire = await b.executeAsync((done) => window.__TAURI_INTERNALS__.invoke("wire_mesh", {}).then((buf) => done(new DataView(buf, 0, 8).getUint32(4, true))));
    check("meshView", "Z shows the wireframe", (await b.execute(() => document.querySelector("#toggle-wire").getAttribute("aria-pressed"))) === "true" && wire > 0, `${wire} edges`);
    await shot(b, "25-mesh-wire");
    await key(b, "z");
    const plain = path.join(OUT, "view-plain.3mf");
    await invoke(b, "export_3mf", { path: plain });
    await key(b, "k");
    await settle(b);
    check("meshView", "K turns the section on in Malla", await b.execute(() => document.querySelector("[data-app]").dataset.tool === "clip" && document.querySelector('button[data-tool="clip"]').dataset.on === "true"));
    await shot(b, "26-mesh-section");
    const cut = path.join(OUT, "view-section.3mf");
    await invoke(b, "export_3mf", { path: cut });
    check("meshView", "the section does not change the exported 3MF", objectModels(plain).join() === objectModels(cut).join());
    // The wall through the section: the hollow's 2 mm, give or take a voxel.
    await key(b, "t");
    const canvas = await b.$("#viewport");
    await b.action("pointer").move({ origin: canvas, x: 0, y: 40 }).down().up().perform().catch(() => {});
    await b.waitUntil(async () => /mm/.test((await text(b, "[data-bind=thicknessOut]")) ?? ""), { timeout: 30000, interval: 200 }).catch(() => {});
    const reading = (await text(b, "[data-bind=thicknessOut]")) ?? "";
    const mm = Number((reading.match(/([\d,]+) mm/)?.[1] ?? "").replace(",", "."));
    check("meshView", "the thickness tool reads the hollow's wall through the section", Math.abs(mm - 2) < 0.7, reading);
    await shot(b, "27-mesh-thickness");
    check("meshView", "the section stays on while measuring", await b.execute(() => document.querySelector('button[data-tool="clip"]').dataset.on === "true"));
    await click(b, "[role=tab][data-tab=model]");
    const off = await b.execute(() => ({ clip: document.querySelector('button[data-tool="clip"]').dataset.on, wire: document.querySelector("#toggle-wire").getAttribute("aria-pressed"), reading: document.querySelector("[data-bind=thicknessOut]").textContent }));
    check("meshView", "leaving Malla turns all three off", off.clip === "false" && off.wire === "false" && off.reading === "", JSON.stringify(off));
  },

  async english(b) {
    await waitState(b, "ready");
    const missing = await b.execute(() => [...document.querySelectorAll("[data-i18n]")].filter((el) => el.offsetParent && !el.textContent.trim()).map((el) => el.dataset.i18n));
    check("english", "no empty catalogue texts", missing.length === 0, missing.slice(0, 5).join(", "));
    await shot(b, "08-english");
  },
};

const plan = [
  ["open", {}], ["size", {}], ["cut", {}], ["mesh", {}], ["printability", {}], ["brush", {}], ["exportFile", {}], ["a11y", {}], ["tabs", {}], ["repairs", { model: HEAD }], ["orient", {}], ["analysis", {}], ["thickness", { model: CYLINDER }], ["simplify", {}], ["smooth", { model: CYLINDER }], ["smoothPaint", {}], ["hollow", { model: CYLINDER }], ["thicken", { model: THIN }], ["meshView", { model: CYLINDER }], ["english", { lang: "en" }],
];

const metrics = {};
await services();
try {
  for (const [name, opts] of plan) {
    if (only.length && !only.includes(name)) continue;
    console.log(`▸ ${name}`);
    let b;
    try {
      b = await session(opts.model ?? MODEL, opts);
    } catch (e) {
      check(name, "app session starts", false, String(e.message ?? e).split("\n")[0]);
      continue;
    }
    try {
      metrics[name] = (await scenarios[name](b)) ?? {};
      metrics[name].commands = await times(b);
    } catch (e) {
      check(name, "scenario ran", false, String(e.message ?? e).split("\n")[0]);
      await shot(b, `fail-${name}`).catch(() => {});
    } finally {
      await b.deleteSession().catch(() => {});
    }
  }
} finally {
  report("report", { model: MODEL, results, metrics });
  stop();
}
const failed = results.filter((r) => r.ok === "fail").length;
console.log(`\n${results.filter((r) => r.ok === "pass").length} pass, ${failed} fail, ${results.filter((r) => r.ok === "skip").length} skip → ${OUT}`);
process.exit(failed ? 1 : 0);
