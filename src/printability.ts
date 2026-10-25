import { api, type MeshFix, type PrintabilityCheck, type PrintabilityParams, type PrintabilityReport, type Summary, type Vec3 } from "./api";
import { num, onLangChange, t, tKey } from "./i18n";
import "./styles/printability.css";

type Severity = PrintabilityCheck["severity"];

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

/** A correction the core can make, as the mesh operations it turns on. Each one
 *  is a single undo step and is reverted by putting the previous values back. */
interface Fix {
  /** Whether it would change anything for this document. */
  available(s: Summary, r: PrintabilityReport): boolean;
  patch(s: Summary, r: PrintabilityReport): MeshFix;
  /** Value of `fixHelp.<id>`'s `{mm}`, when it has one. */
  mm?(s: Summary, r: PrintabilityReport): number;
}
const base = (s: Summary) => Math.max(0.4, Math.round(s.settings.layerMm * 2 * 100) / 100);
const FIXES: Record<string, Fix> = {
  clean: { available: (s) => !s.settings.repair, patch: () => ({ repair: true }) },
  orient: { available: (s) => !s.settings.orient, patch: () => ({ repair: true, orient: true }) },
  fillHoles: { available: (s) => !s.settings.fillHoles, patch: () => ({ repair: true, fillHoles: true }) },
  removeSmall: {
    available: (s, r) => s.settings.minPieceMm < r.params.componentMm,
    patch: (_, r) => ({ minPieceMm: r.params.componentMm }),
    mm: (_, r) => r.params.componentMm,
  },
  flattenBase: { available: (s) => s.settings.flattenMm === 0, patch: (s) => ({ flattenMm: base(s) }), mm: base },
};
/** Which fix answers each check. Checks without one can only be located. */
const FIX_OF: Record<string, string> = {
  degenerate: "clean", duplicate: "clean", winding: "orient", volume: "orient",
  openEdges: "fillHoles", smallComponents: "removeSmall", contact: "flattenBase",
};
/** Where a fix's mesh operations show in the summary, to tell whether it still holds. */
const SETTING: Record<keyof MeshFix, keyof Summary["settings"]> = {
  repair: "repair", orient: "orient", fillHoles: "fillHoles", minPieceMm: "minPieceMm", flattenMm: "flattenMm",
};

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
  confirm(message: string): Promise<boolean>;
  error(error: unknown): void;
}) {
  const panel = document.querySelector<HTMLElement>("#printability")!;
  const grid = panel.querySelector<HTMLElement>(".pa-params__grid")!;
  const output = panel.querySelector<HTMLElement>(".pa-results")!;
  const status = panel.querySelector<HTMLElement>(".pa-status")!;
  const sum = panel.querySelector<HTMLElement>(".pa-sum")!;
  const analyze = panel.querySelector<HTMLButtonElement>("#printability-run")!;
  const hide = panel.querySelector<HTMLButtonElement>("#printability-hide")!;
  const inputs = new Map<keyof PrintabilityParams, HTMLInputElement>();
  /** Fixes applied from this panel: what they set, what to put back, and the counts before. */
  const applied = new Map<string, { patch: MeshFix; previous: MeshFix; before: Map<string, number> }>();
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

  function say(text: string, tone: "" | "ok" | "warn" | "error" = "") {
    status.textContent = text;
    status.dataset.tone = tone;
  }

  function invalidate() {
    token++;
    outdated = outdated || report !== null;
    report = null;
    output.replaceChildren();
    markers(null, []);
    say(t(outdated ? "printability.outdated" : "printability.stale"), outdated ? "warn" : "");
    sum.replaceChildren();
    controls();
  }

  /** Forgets fixes the document no longer has: undone, or changed in the settings panel. */
  function prune(s: Summary | null) {
    for (const [id, a] of applied) {
      const holds = s && (Object.keys(a.patch) as (keyof MeshFix)[]).every((k) => s.settings[SETTING[k]] === a.patch[k]);
      if (!holds) applied.delete(id);
    }
  }

  /** Called with every summary; only a change that affects the geometry clears the report. */
  function update(s: Summary | null) {
    const next = revisionOf(s);
    if (next === revision) return controls();
    // Another file starts fresh; the same file edited says the old report is outdated.
    const sameFile = revision !== "" && s !== null && JSON.parse(revision)[0] === s.fileName;
    revision = next;
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
    const fixable = id && s && !done && c.severity !== "pass" && FIXES[id].available(s, r);
    if (fixable) row.append(el("p", "pa-check__fix", fixText(id, s, r)));

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

  function show(r: PrintabilityReport, message = "") {
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
    say(message || (errors || warnings ? "" : t("printability.allClear")), message ? "ok" : "");

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
  async function step(text: string, change: () => Promise<unknown>, done: () => string) {
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
      show(next, done());
    } catch (e) {
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
    if (s.paintedTriangles > 0 && !(await options.confirm(t("printability.fixPaint")))) return;
    const patch = FIXES[id].patch(s, report);
    const previous = Object.fromEntries((Object.keys(patch) as (keyof MeshFix)[]).map((k) => [k, s.settings[SETTING[k]]])) as MeshFix;
    applied.set(id, { patch, previous, before: new Map(report.checks.map((c) => [c.code, c.count])) });
    // A failed change leaves the settings as they were, and `prune` drops the entry.
    await step(t("printability.fixing"), () => options.fix(patch), () => applied.has(id) ? t("printability.fixDone") : "");
  }

  async function revert(id: string) {
    const a = applied.get(id);
    if (!a || busy) return;
    applied.delete(id);
    await step(t("printability.reverting"), () => options.fix(a.previous), () => t("printability.revertDone"));
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
