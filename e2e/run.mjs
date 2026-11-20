// E2E scenarios over the real app: Rust backend, Blender import, WebKitGTK webview.
//
//   cargo build -p blend-parser-app && (cd e2e && npm install && npm run fixtures && npm test)
//
// Writes screenshots and a JSON report (checks, command timings, layout metrics)
// to e2e/out. A scenario whose control is missing on this branch is skipped, not failed,
// so the same run compares branches.
import path from "node:path";
import {
  HERE, OUT, services, stop, session, waitState, settle, shot, report, times, probe,
  click, exists, visible, text, setValue, key, sleep,
} from "./lib.mjs";

const MODEL = process.env.BP_MODEL ?? path.join(HERE, "e2e/fixtures/out/figure.blend");
/** Suzanne's head alone, open at the eyes and the neck (`fixtures/simple.py`). */
const HEAD = path.join(HERE, "e2e/fixtures/out/head.blend");
/** A closed cylinder, 44 × 45 mm (`fixtures/simple.py`). */
const CYLINDER = path.join(HERE, "e2e/fixtures/out/cylinder.blend");
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
    check("tabs", "Malla's view offers orbit, measure, move and turn", (await tools()) === "orbit,measure,move,rotate", await tools());
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

  async english(b) {
    await waitState(b, "ready");
    const missing = await b.execute(() => [...document.querySelectorAll("[data-i18n]")].filter((el) => el.offsetParent && !el.textContent.trim()).map((el) => el.dataset.i18n));
    check("english", "no empty catalogue texts", missing.length === 0, missing.slice(0, 5).join(", "));
    await shot(b, "08-english");
  },
};

const plan = [
  ["open", {}], ["size", {}], ["cut", {}], ["mesh", {}], ["printability", {}], ["brush", {}], ["exportFile", {}], ["a11y", {}], ["tabs", {}], ["repairs", { model: HEAD }], ["orient", {}], ["analysis", {}], ["thickness", { model: CYLINDER }], ["simplify", {}], ["english", { lang: "en" }],
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
