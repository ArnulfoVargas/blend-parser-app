import { api, type MeshFix, type PrintabilityCheck, type PrintabilityParams, type PrintabilityReport, type Summary, type Vec3 } from "./api";
import { num, onLangChange, t, tKey } from "./i18n";
import "./styles/printability.css";

type Severity = PrintabilityCheck["severity"];
type Tone = "ok" | "warn" | "error";

/** Field, min, max. Bounds mirror `Params::valid` in bp-core. */
const FIELDS: [keyof PrintabilityParams, number, number][] = [
  ["extrusionMm", 0.05, 3], ["perimeters", 1, 20], ["wallMm", 0.05, 20],
  ["detailMm", 0.05, 20], ["componentMm", 0, 100], ["overhangDeg", 0, 90],
  ["contactMm2", 0, 10000], ["toleranceMm", 0.00001, 0.1], ["samples", 16, 4096],
];
/** Pass/fail checks: their count is 0 or 1, so it says nothing the severity does not. */
const UNCOUNTED = new Set(["contact", "buildVolume"]);
const INTEGER = new Set<keyof PrintabilityParams>(["perimeters", "samples"]);
const RANK: Record<Severity, number> = { error: 0, warning: 1, pass: 2, incomplete: 3 };
/** Checks whose measurement and threshold carry a unit worth showing; the rest are plain counts. */
const UNITS: Record<string, string> = {
  contact: "mm²", thinWalls: "mm", smallComponents: "mm", smallDetails: "mm", overhang: "°",
};

/** How an applied fix is taken back, and whether the document still has it
 *  (Ctrl+Z or the settings panel can undo it behind the panel's back). */
interface Undo {
  revert(): Promise<unknown>;
  holds(s: Summary): boolean;
}
/** A correction the core can make. Each change is one undo step. */
interface Fix {
  /** Whether it would change anything for this document. */
  available(s: Summary, r: PrintabilityReport): boolean;
  /** Rebuilding the mesh drops hand painting; turning or resizing does not. */
  repaints: boolean;
  /** Value of `fixHelp.<id>`'s `{mm}`, when it has one. */
  mm?(s: Summary, r: PrintabilityReport): number;
  /** Makes the change; null when it found nothing to change. */
  apply(s: Summary, r: PrintabilityReport): Promise<Undo | null>;
}
/** What each fix asks of the app; each call applies and re-renders. */
interface FixOps {
  summary(): Summary | null;
  fix(fix: MeshFix): Promise<unknown>;
  orient(overhangDeg: number): Promise<unknown>;
  rotate(rotation: number[][]): Promise<unknown>;
  fitPlate(): Promise<unknown>;
  resize(mm: Vec3): Promise<unknown>;
}
/** Which fix answers each check. Checks without one can only be located. */
const FIX_OF: Record<string, string> = {
  degenerate: "deepClean", duplicate: "deepClean", nonManifoldEdges: "deepClean", winding: "orient", volume: "orient",
  openEdges: "fillHoles", smallComponents: "removeSmall", contact: "flattenBase",
  buildVolume: "fitPlate", floating: "dropFloating", nonManifoldVertices: "splitVertices", overhang: "rotate",
};
/** Where a fix's mesh operations show in the summary, to tell whether it still holds. */
const SETTING: Record<keyof MeshFix, keyof Summary["settings"]> = {
  repair: "repair", orient: "orient", fillHoles: "fillHoles", splitVertices: "splitVertices",
  dropFloating: "dropFloating", minPieceMm: "minPieceMm", flattenMm: "flattenMm", deepClean: "deepClean",
};
const same = (a: number[] | number[][], b: number[] | number[][]) =>
  a.flat().every((v, i) => Math.abs(v - b.flat()[i]) < 1e-5);
const base = (s: Summary) => Math.max(0.4, Math.round(s.settings.layerMm * 2 * 100) / 100);

function makeFixes(ops: FixOps): Record<string, Fix> {
  /** Turns mesh operations on; reverting puts back the values they had. */
  const mesh = (available: Fix["available"], patch: (s: Summary, r: PrintabilityReport) => MeshFix, mm?: Fix["mm"]): Fix => ({
    available, mm, repaints: true,
    async apply(s, r) {
      const set = patch(s, r);
      const keys = Object.keys(set) as (keyof MeshFix)[];
      const previous = Object.fromEntries(keys.map((k) => [k, s.settings[SETTING[k]]])) as MeshFix;
      const holds = (x: Summary) => keys.every((k) => x.settings[SETTING[k]] === set[k]);
      await ops.fix(set);
      const now = ops.summary();
      return now && holds(now) ? { revert: () => ops.fix(previous), holds } : null;
    },
  });
  /** A change read back from the summary: `value` before and after tells whether it happened. */
  const measured = <T extends number[] | number[][]>(value: (s: Summary) => T, run: (r: PrintabilityReport) => Promise<unknown>, back: (v: T, s: Summary) => Promise<unknown>): Fix => ({
    available: () => true, repaints: false,
    async apply(s, r) {
      const before = value(s);
      await run(r);
      const now = ops.summary();
      if (!now || same(value(now), before)) return null;
      const after = value(now);
      return { revert: () => back(before, s), holds: (x) => same(value(x), after) };
    },
  });
  return {
    // Stray faces and slivers are what usually leave holes and shared vertices,
    // so closing holes and splitting vertices clean them first.
    deepClean: mesh((s) => !s.settings.deepClean, () => ({ repair: true, deepClean: true })),
    orient: mesh((s) => !s.settings.orient, () => ({ repair: true, orient: true })),
    fillHoles: mesh((s) => !s.settings.fillHoles, () => ({ repair: true, deepClean: true, fillHoles: true })),
    splitVertices: mesh((s) => !s.settings.splitVertices, () => ({ repair: true, deepClean: true, splitVertices: true })),
    dropFloating: mesh((s) => !s.settings.dropFloating, () => ({ repair: true, dropFloating: true })),
    removeSmall: mesh((s, r) => s.settings.minPieceMm < r.params.componentMm, (_, r) => ({ repair: true, minPieceMm: r.params.componentMm }), (_, r) => r.params.componentMm),
    flattenBase: mesh((s) => s.settings.flattenMm === 0, (s) => ({ flattenMm: base(s) }), base),
    // Turning drops a per-axis stretch, so turning back also puts the size back.
    rotate: measured((s) => s.settings.rotation, (r) => ops.orient(r.params.overhangDeg), async (v, s) => {
      await ops.rotate(v);
      const now = ops.summary();
      if (now && !same(now.size.targetMm, s.size.targetMm)) await ops.resize(s.size.targetMm);
    }),
    fitPlate: measured((s) => s.size.targetMm, () => ops.fitPlate(), (v) => ops.resize(v as Vec3)),
  };
}

/** Same starting points as `Params::for_nozzle`. */
function defaults(nozzle: number): PrintabilityParams {
  return { extrusionMm: nozzle * 1.1, perimeters: 2, wallMm: nozzle * 2.2, detailMm: nozzle * 1.1,
    componentMm: 2, overhangDeg: 45, contactMm2: 4, toleranceMm: 0.001, samples: 512 };
}

/** What a report depends on. Painting and colour merges leave it untouched; size, geometry and profile do not. */
function revisionOf(s: Summary | null): string {
  return s ? JSON.stringify([s.fileName, s.sourceTriangles, s.triangles, s.sizeMm, s.settings, s.maxHeightMm, s.plate.fits, s.plate.prefs]) : "";
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className = "", text = ""): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

const size = (v: Vec3, digits: number) => v.map((x) => num(x, digits)).join(" × ");

/** The report belongs to one document revision; `update` clears it when that revision changes. */
export function setupPrintability(options: {
  summary(): Summary | null;
  locate(points: Vec3[], values?: (number | null)[], threshold?: number): void;
  fix(fix: MeshFix): Promise<unknown>;
  orient(overhangDeg: number): Promise<unknown>;
  rotate(rotation: number[][]): Promise<unknown>;
  fitPlate(): Promise<unknown>;
  resize(mm: Vec3): Promise<unknown>;
  confirm(message: string): Promise<boolean>;
  error(error: unknown): void;
}) {
  const FIXES = makeFixes(options);
  const panel = document.querySelector<HTMLElement>("#printability")!;
  const grid = panel.querySelector<HTMLElement>(".pa-params__grid")!;
  const output = panel.querySelector<HTMLElement>(".pa-results")!;
  const status = panel.querySelector<HTMLElement>(".pa-status")!;
  const sum = panel.querySelector<HTMLElement>(".pa-sum")!;
  const analyze = panel.querySelector<HTMLButtonElement>("#printability-run")!;
  const hide = panel.querySelector<HTMLButtonElement>("#printability-hide")!;
  const inputs = new Map<keyof PrintabilityParams, HTMLInputElement>();
  /** Fixes applied from this panel: how to take each back, and the counts before it. */
  const applied = new Map<string, Undo & { before: Map<string, number> }>();
  /** Fixes that found nothing to change in this revision; their card says so instead of offering them again. */
  const fruitless = new Set<string>();
  let report: PrintabilityReport | null = null;
  let outdated = false;
  let token = 0;
  let busy = false;
  let revision = "";
  let defaultsNozzle = 0;
  /** The button whose markers are on the model, so it can show as pressed. */
  let shown: HTMLButtonElement | null = null;

  for (const [key, min, max] of FIELDS) {
    const row = el("div", "field");
    const label = el("label");
    label.htmlFor = `pa-${key}`;
    label.dataset.param = key;
    const input = el("input", "input mono");
    Object.assign(input, { type: "number", id: label.htmlFor, min: String(min), max: String(max), required: true, step: INTEGER.has(key) ? "1" : "any" });
    input.addEventListener("input", () => {
      input.removeAttribute("aria-invalid");
      invalidate();
    });
    inputs.set(key, input);
    row.append(label, input);
    grid.append(row);
  }

  function reset() {
    defaultsNozzle = options.summary()?.settings.nozzleMm ?? 0.4;
    const values = defaults(defaultsNozzle);
    for (const [key, input] of inputs) {
      input.value = String(Number(values[key].toFixed(5)));
      input.removeAttribute("aria-invalid");
    }
    invalidate();
  }

  function read(): PrintabilityParams | null {
    const result = {} as PrintabilityParams;
    for (const [key, input] of inputs) {
      const n = Number(input.value);
      const ok = input.checkValidity() && Number.isFinite(n) && (!INTEGER.has(key) || Number.isInteger(n));
      input.toggleAttribute("aria-invalid", !ok);
      if (!ok) {
        panel.querySelector<HTMLDetailsElement>(".pa-params")!.open = true;
        input.focus();
        input.reportValidity();
        return null;
      }
      result[key] = n;
    }
    return result;
  }

  function markers(button: HTMLButtonElement | null, points: Vec3[], values?: (number | null)[], threshold?: number) {
    shown?.setAttribute("aria-pressed", "false");
    shown = button;
    shown?.setAttribute("aria-pressed", "true");
    hide.hidden = !button;
    options.locate(points, values, threshold);
  }

  function controls() {
    analyze.disabled = busy || !options.summary();
    analyze.setAttribute("aria-busy", String(busy));
    analyze.textContent = t(report ? "printability.rerun" : "printability.run");
    output.querySelectorAll<HTMLButtonElement>("[data-fix]").forEach((b) => (b.disabled = busy));
  }

  function say(text: string, tone: Tone | "" = "") {
    status.textContent = text;
    status.dataset.tone = tone;
  }

  function invalidate() {
    token++;
    outdated = outdated || report !== null;
    report = null;
    markers(null, []);
    // A fix in progress redraws the results once it has analysed again; emptying
    // them meanwhile collapses the panel and the sidebar jumps back to the top.
    if (!busy) {
      output.replaceChildren();
      sum.replaceChildren();
      say(t(outdated ? "printability.outdated" : "printability.stale"), outdated ? "warn" : "");
    }
    controls();
  }

  /** Forgets fixes the document no longer has: undone, or changed in the settings panel. */
  function prune(s: Summary | null) {
    for (const [id, a] of applied) if (!s || !a.holds(s)) applied.delete(id);
  }

  /** Called with every summary; only a change that affects the geometry clears the report. */
  function update(s: Summary | null) {
    const next = revisionOf(s);
    if (next === revision) return controls();
    // Another file starts fresh; the same file edited says the old report is outdated.
    const sameFile = revision !== "" && s !== null && JSON.parse(revision)[0] === s.fileName;
    revision = next;
    fruitless.clear();
    if (!sameFile) {
      report = null;
      outdated = false;
      applied.clear();
    }
    prune(s);
    if (s && defaultsNozzle !== s.settings.nozzleMm) reset();
    else invalidate();
  }

  function checkMeta(c: PrintabilityCheck): string {
    const parts = [tKey(`printability.evidence.${c.evidence}`)];
    const unit = UNITS[c.code];
    // Degrees read as whole numbers and sit against the figure; millimetres keep two decimals.
    const value = (n: number) => unit === "°" ? `${num(n, 0)}°` : `${num(n, 2)} ${unit}`;
    if (unit && c.measurement !== null) parts.push(t("printability.measured", { n: value(c.measurement) }));
    if (unit && c.threshold !== null) parts.push(t("printability.threshold", { n: value(c.threshold) }));
    return parts.join(" · ");
  }

  const noneText = (id: string) => t(`printability.fixNone.${id === "rotate" ? "rotate" : "any"}`);

  function fixText(id: string, s: Summary, r: PrintabilityReport) {
    const mm = FIXES[id].mm?.(s, r);
    return tKey(`printability.fixHelp.${id}`, { mm: mm === undefined ? "" : num(mm, 2) });
  }

  /** The applied fix that resolved `c`: its own, or one that fixed it on the way (closing holes also welds). */
  function resolver(c: PrintabilityCheck): string | undefined {
    const own = FIX_OF[c.code];
    if (own && applied.has(own)) return own;
    if (c.severity !== "pass") return undefined;
    for (const [id, a] of applied) if ((a.before.get(c.code) ?? 0) > 0) return id;
    return undefined;
  }

  function card(c: PrintabilityCheck, r: PrintabilityReport, s: Summary | null): HTMLElement {
    const by = resolver(c);
    const id = by ?? FIX_OF[c.code];
    const done = by ? applied.get(by) : undefined;
    const row = el("article", "pa-check");
    row.dataset.severity = done && c.severity === "pass" ? "fixed" : c.severity;
    const head = el("div", "pa-check__head");
    head.append(el("span", "pa-dot"), el("strong", "pa-check__title", tKey(`printability.check.${c.code}`)));
    if (row.dataset.severity === "fixed") head.append(el("span", "pa-check__count", t("printability.fixed")));
    else if (!UNCOUNTED.has(c.code)) head.append(el("span", "pa-check__count mono", t("printability.count", { count: c.count })));
    row.append(head, el("p", "pa-check__meta", checkMeta(c)));
    if (done) {
      const before = done.before.get(c.code);
      if (before !== undefined && !UNCOUNTED.has(c.code)) row.append(el("p", "pa-check__delta mono", t("printability.fixedDelta", { before, after: c.count })));
    } else {
      row.append(el("p", "help", tKey(`printability.help.${c.code}`)));
    }
    const open = id && s && !done && c.severity !== "pass";
    const fixable = open && !fruitless.has(id) && FIXES[id].available(s, r);
    if (fixable) row.append(el("p", "pa-check__fix", fixText(id, s, r)));
    // The outcome shows where it was asked for: the status line is above the cards, often out of view.
    else if (open && fruitless.has(id)) row.append(el("p", "pa-check__note", noneText(id)));
    else if (open) row.append(el("p", "pa-check__note", t("printability.fixSpent")));

    const actions = el("div", "pa-check__actions");
    if (done || fixable) {
      const button = el("button", "btn btn--sm", t(done ? "printability.revert" : "printability.fix"));
      button.type = "button";
      button.dataset.fix = id;
      button.disabled = busy;
      button.addEventListener("click", () => void (done ? revert(id) : fix(id)));
      actions.append(button);
    }
    if (c.locations.length && c.severity !== "pass") {
      const locate = el("button", "link", t("printability.locate"));
      locate.type = "button";
      locate.setAttribute("aria-pressed", "false");
      locate.addEventListener("click", () => shown === locate ? markers(null, []) : markers(locate, c.locations));
      actions.append(locate);
    }
    if (actions.childElementCount) row.append(actions);
    return row;
  }

  function fold(summaryText: string, checks: PrintabilityCheck[], note = ""): HTMLDetailsElement {
    const box = el("details", "pa-more");
    box.append(el("summary", "", summaryText));
    if (note) box.append(el("p", "help", note));
    const list = el("ul", "pa-more__list");
    for (const c of checks) {
      const item = el("li");
      item.dataset.severity = c.severity;
      item.append(el("span", "pa-dot"), el("span", "pa-more__title", tKey(`printability.check.${c.code}`)));
      const meta = UNITS[c.code] && c.evidence !== "notEvaluated" ? checkMeta(c) : "";
      if (meta) item.append(el("span", "pa-more__meta mono", meta.split(" · ").slice(1).join(" · ")));
      item.title = tKey(`printability.help.${c.code}`);
      list.append(item);
    }
    box.append(list);
    return box;
  }

  function chips(target: HTMLElement, counts: [Severity | "fixed", number][]) {
    target.replaceChildren(...counts.filter(([, n]) => n > 0).map(([severity, n]) => {
      const node = el("span", "pa-chip", t(`printability.chip.${severity as "error" | "warning" | "pass" | "fixed"}`, { count: n }));
      node.dataset.severity = severity;
      return node;
    }));
  }

  function show(r: PrintabilityReport, message = "", tone: Tone = "ok") {
    report = r;
    outdated = false;
    markers(null, []);
    const s = options.summary();
    const checks = [...r.checks].sort((a, b) => RANK[a.severity] - RANK[b.severity]);
    const problem = (c: PrintabilityCheck) => c.severity === "error" || c.severity === "warning";
    const fixedNow = (c: PrintabilityCheck) => c.severity === "pass" && resolver(c) !== undefined;
    const cards = checks.filter((c) => problem(c) || fixedNow(c));
    const passed = checks.filter((c) => c.severity === "pass" && !fixedNow(c));
    const skipped = checks.filter((c) => c.severity === "incomplete");
    const errors = checks.filter((c) => c.severity === "error").length;
    const warnings = checks.filter((c) => c.severity === "warning").length;
    const fixed = checks.filter(fixedNow).length;
    const counts: [Severity | "fixed", number][] = [["error", errors], ["warning", warnings], ["fixed", fixed], ["pass", passed.length]];

    const top = el("div", "pa-chips");
    chips(top, counts);
    chips(sum, counts);
    say(message || (errors || warnings ? "" : t("printability.allClear")), message ? tone : "");

    const context = el("p", "pa-context mono", t("printability.context", {
      printer: r.printer ?? s?.settings.printer ?? "",
      nozzle: num(r.nozzleMm ?? s?.settings.nozzleMm ?? 0, 2),
      layer: num(r.layerMm ?? s?.settings.layerMm ?? 0, 2),
      size: size(r.sizeMm, 1),
      triangles: num(r.triangles, 0),
    }));
    output.replaceChildren(top, context, ...cards.map((c) => card(c, r, s)));

    if (r.thickness.length) {
      const threshold = Math.max(r.params.wallMm, r.params.extrusionMm * r.params.perimeters);
      const map = el("div", "pa-map");
      const button = el("button", "btn btn--sm", t("printability.map"));
      button.type = "button";
      button.setAttribute("aria-pressed", "false");
      button.addEventListener("click", () => shown === button
        ? markers(null, [])
        : markers(button, r.thickness.map((x) => x.point), r.thickness.map((x) => x.mm), threshold));
      const legend = el("ul", "pa-legend");
      for (const tone of ["thin", "ok", "none"] as const) {
        const item = el("li", "", t(`printability.legend.${tone}`));
        item.dataset.tone = tone;
        legend.append(item);
      }
      map.append(button, legend);
      output.append(map);
    }
    if (passed.length) output.append(fold(t("printability.passed", { count: passed.length }), passed));
    if (skipped.length) output.append(fold(t("printability.skipped", { count: skipped.length }), skipped, t("printability.skippedHelp")));
    controls();
  }

  function fail(e: unknown) {
    say(t("printability.failed"), "error");
    options.error(e);
  }

  /** Runs `change` with the panel busy, then analyses what the document is now. */
  async function step(id: string, text: string, change: () => Promise<unknown>, done: () => [string, Tone]) {
    const params = read();
    if (!params || busy) return;
    busy = true;
    controls();
    say(text);
    try {
      await change();
      prune(options.summary());
      const next = await api.analyzePrintability(params);
      token++;
      show(next, ...done());
      // The clicked button was replaced; keep the keyboard where it was without scrolling.
      output.querySelector<HTMLButtonElement>(`[data-fix="${id}"]`)?.focus({ preventScroll: true });
    } catch (e) {
      // The results kept on screen during the change no longer describe the document.
      if (!report) {
        output.replaceChildren();
        sum.replaceChildren();
      }
      fail(e);
    } finally {
      busy = false;
      controls();
    }
  }

  async function fix(id: string) {
    const s = options.summary();
    if (!s || !report || busy) return;
    // Rebuilding the mesh drops hand painting; Revertir and Ctrl+Z bring it back.
    if (FIXES[id].repaints && s.paintedTriangles > 0 && !(await options.confirm(t("printability.fixPaint")))) return;
    const r = report;
    const before = new Map(r.checks.map((c) => [c.code, c.count]));
    // A failed or empty change leaves no entry, and the message says so.
    await step(id, t("printability.fixing"), async () => {
      const undo = await FIXES[id].apply(s, r);
      if (undo) applied.set(id, { ...undo, before });
      else fruitless.add(id);
    }, () => applied.has(id) ? [t("printability.fixDone"), "ok"] : [noneText(id), "warn"]);
  }

  async function revert(id: string) {
    const a = applied.get(id);
    if (!a || busy) return;
    applied.delete(id);
    await step(id, t("printability.reverting"), () => a.revert(), () => [t("printability.revertDone"), "ok"]);
  }

  async function run() {
    const params = read();
    if (!params || !options.summary() || busy) return;
    const mine = ++token;
    busy = true;
    controls();
    say(t("printability.running"));
    try {
      const next = await api.analyzePrintability(params);
      if (mine === token) show(next);
    } catch (e) {
      if (mine === token) fail(e);
    } finally {
      busy = false;
      controls();
    }
  }

  analyze.addEventListener("click", () => void run());
  hide.addEventListener("click", () => markers(null, []));
  panel.querySelector("#printability-reset")!.addEventListener("click", reset);
  const labels = () => grid.querySelectorAll<HTMLElement>("[data-param]").forEach((label) => (label.textContent = tKey(`printability.param.${label.dataset.param}`)));
  onLangChange(() => {
    labels();
    if (report) show(report);
    else say(t(outdated ? "printability.outdated" : "printability.stale"), outdated ? "warn" : "");
    controls();
  });
  labels();
  reset();
  return { update };
}
