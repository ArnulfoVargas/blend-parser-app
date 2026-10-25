import { api, type PrintabilityCheck, type PrintabilityParams, type PrintabilityReport, type Summary, type Vec3 } from "./api";
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
  apply(): Promise<unknown>;
  error(error: unknown): void;
}) {
  const panel = document.querySelector<HTMLElement>("#printability")!;
  const grid = panel.querySelector<HTMLElement>(".pa-params__grid")!;
  const output = panel.querySelector<HTMLElement>(".pa-results")!;
  const status = panel.querySelector<HTMLElement>(".pa-status")!;
  const sum = panel.querySelector<HTMLElement>(".pa-sum")!;
  const analyze = panel.querySelector<HTMLButtonElement>("#printability-run")!;
  const cleanup = panel.querySelector<HTMLButtonElement>("#printability-cleanup")!;
  const hide = panel.querySelector<HTMLButtonElement>("#printability-hide")!;
  const dialog = document.querySelector<HTMLDialogElement>("#printability-dialog")!;
  const inputs = new Map<keyof PrintabilityParams, HTMLInputElement>();
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
    const s = options.summary();
    analyze.disabled = busy || !s;
    analyze.setAttribute("aria-busy", String(busy));
    analyze.textContent = t(report ? "printability.rerun" : "printability.run");
    cleanup.disabled = busy || !s || s.settings.repair;
  }

  function invalidate() {
    token++;
    outdated = outdated || report !== null;
    report = null;
    output.replaceChildren();
    markers(null, []);
    status.textContent = t(outdated ? "printability.outdated" : "printability.stale");
    status.dataset.tone = outdated ? "warn" : "";
    sum.textContent = "";
    controls();
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
    }
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

  function card(c: PrintabilityCheck): HTMLElement {
    const row = el("article", "pa-check");
    row.dataset.severity = c.severity;
    const head = el("div", "pa-check__head");
    head.append(el("span", "pa-dot"), el("strong", "pa-check__title", tKey(`printability.check.${c.code}`)));
    if (!UNCOUNTED.has(c.code)) head.append(el("span", "pa-check__count mono", t("printability.count", { count: c.count })));
    row.append(head, el("p", "pa-check__meta", checkMeta(c)), el("p", "help", tKey(`printability.help.${c.code}`)));
    if (c.locations.length) {
      const locate = el("button", "link", t("printability.locate"));
      locate.type = "button";
      locate.setAttribute("aria-pressed", "false");
      locate.addEventListener("click", () =>
        shown === locate ? markers(null, []) : markers(locate, c.locations));
      row.append(locate);
    }
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

  function show(r: PrintabilityReport) {
    report = r;
    outdated = false;
    markers(null, []);
    const checks = [...r.checks].sort((a, b) => RANK[a.severity] - RANK[b.severity]);
    const count = (s: Severity) => checks.filter((c) => c.severity === s).length;
    const errors = count("error");
    const warnings = count("warning");
    const passed = checks.filter((c) => c.severity === "pass");
    const skipped = checks.filter((c) => c.severity === "incomplete");

    const chips = el("div", "pa-chips");
    const chip = (severity: Severity, n: number) => {
      const node = el("span", "pa-chip", t(`printability.chip.${severity as "error" | "warning" | "pass"}`, { count: n }));
      node.dataset.severity = severity;
      chips.append(node);
    };
    if (errors) chip("error", errors);
    if (warnings) chip("warning", warnings);
    chip("pass", passed.length);
    status.textContent = errors || warnings ? "" : t("printability.allClear");
    status.dataset.tone = "";
    sum.textContent = errors || warnings
      ? [errors && t("printability.chip.error", { count: errors }), warnings && t("printability.chip.warning", { count: warnings })].filter(Boolean).join(" · ")
      : t("printability.chip.pass", { count: passed.length });

    const s = options.summary();
    const context = el("p", "pa-context mono", t("printability.context", {
      printer: r.printer ?? s?.settings.printer ?? "",
      nozzle: num(r.nozzleMm ?? s?.settings.nozzleMm ?? 0, 2),
      layer: num(r.layerMm ?? s?.settings.layerMm ?? 0, 2),
      size: size(r.sizeMm, 1),
      triangles: num(r.triangles, 0),
    }));
    output.replaceChildren(chips, context, ...checks.filter((c) => c.severity === "error" || c.severity === "warning").map(card));

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
      for (const [key, tone] of [["thin", "thin"], ["ok", "ok"], ["none", "none"]] as const) {
        const item = el("li", "", t(`printability.legend.${key}`));
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
    status.textContent = t("printability.failed");
    status.dataset.tone = "error";
    options.error(e);
  }

  async function compare(before: PrintabilityReport, after: PrintabilityReport): Promise<boolean> {
    const body = dialog.querySelector<HTMLTableSectionElement>("tbody")!;
    const row = (label: string, a: string, b: string) => {
      const tr = el("tr");
      if (a !== b) tr.className = "is-changed";
      const th = el("th", "", label);
      th.scope = "row";
      tr.append(th, el("td", "mono", a), el("td", "mono", b));
      return tr;
    };
    body.replaceChildren(
      row(t("printability.compare.triangles"), num(before.triangles, 0), num(after.triangles, 0)),
      row(t("printability.compare.size"), size(before.sizeMm, 2), size(after.sizeMm, 2)),
      ...after.checks.flatMap((a) => {
        const b = before.checks.find((c) => c.code === a.code);
        if (!b || (a.count === b.count && a.evidence === b.evidence)) return [];
        const value = (c: PrintabilityCheck) => c.evidence === "notEvaluated" ? tKey("printability.evidence.notEvaluated") : num(c.count, 0);
        return [row(tKey(`printability.check.${a.code}`), value(b), value(a))];
      }),
    );
    dialog.returnValue = "cancel";
    dialog.showModal();
    return new Promise<boolean>((resolve) => dialog.addEventListener("close", () => resolve(dialog.returnValue === "apply"), { once: true }));
  }

  async function run(preview = false) {
    const params = read();
    if (!params || !options.summary() || busy) return;
    const mine = ++token;
    busy = true;
    controls();
    status.textContent = t("printability.running");
    status.dataset.tone = "";
    try {
      const before = await api.analyzePrintability(params);
      if (mine !== token) return;
      if (!preview) return show(before);
      const after = await api.analyzePrintability(params, true);
      if (mine !== token) return;
      busy = false;
      controls();
      status.textContent = "";
      if (!(await compare(before, after)) || mine !== token) return show(before);
      busy = true;
      controls();
      await options.apply();
      // Applying changes the revision; analyse what is now the document.
      const next = await api.analyzePrintability(params);
      show(next);
    } catch (e) {
      if (mine === token) fail(e);
    } finally {
      busy = false;
      controls();
    }
  }

  analyze.addEventListener("click", () => void run());
  cleanup.addEventListener("click", () => void run(true));
  hide.addEventListener("click", () => markers(null, []));
  panel.querySelector("#printability-reset")!.addEventListener("click", reset);
  const labels = () => grid.querySelectorAll<HTMLElement>("[data-param]").forEach((label) => (label.textContent = tKey(`printability.param.${label.dataset.param}`)));
  onLangChange(() => {
    labels();
    if (report) show(report);
    else status.textContent = t(outdated ? "printability.outdated" : "printability.stale");
    controls();
  });
  labels();
  reset();
  return { update };
}
