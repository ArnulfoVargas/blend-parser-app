import { api, type Summary, type PrintabilityParams, type PrintabilityReport, type Vec3 } from "./api";
import { num, onLangChange, t, tKey } from "./i18n";
import "./styles/printability.css";

/** The report belongs to one document revision. Geometry/profile changes clear it. */
export function setupPrintability(options: {
  summary(): Summary | null;
  locate(points: Vec3[], values?: (number | null)[], threshold?: number): void;
  apply(): Promise<unknown>;
  error(error: unknown): void;
}) {
  const panel = document.querySelector<HTMLElement>("#printability")!;
  const form = panel.querySelector<HTMLElement>(".printability__params")!;
  const output = panel.querySelector<HTMLElement>(".printability__results")!;
  const status = panel.querySelector<HTMLElement>(".printability__status")!;
  const analyze = panel.querySelector<HTMLButtonElement>("#printability-run")!;
  const cleanup = panel.querySelector<HTMLButtonElement>("#printability-cleanup")!;
  const fields: [keyof PrintabilityParams, number, number, number][] = [
    ["extrusionMm", 0.05, 3, 0.01], ["perimeters", 1, 20, 1], ["wallMm", 0.05, 20, 0.05],
    ["detailMm", 0.05, 20, 0.05], ["componentMm", 0, 100, 0.5], ["overhangDeg", 0, 90, 1],
    ["contactMm2", 0, 10000, 1], ["toleranceMm", 0.00001, 0.1, 0.00001], ["samples", 16, 4096, 16],
  ];
  let report: PrintabilityReport | null = null;
  let revision = 0;
  let busy = false;
  let defaultsNozzle = 0;
  let lastProfile = "";
  for (const [key, min, max] of fields) {
    const row = document.createElement("div"); row.className = "field";
    const label = document.createElement("label"); label.htmlFor = `pa-${key}`;
    label.dataset.param = key;
    const input = document.createElement("input"); input.className = "input";
    input.type = "number"; input.id = label.htmlFor; input.min = String(min); input.max = String(max); input.step = key === "samples" || key === "perimeters" ? "1" : "any"; input.required = true;
    input.addEventListener("input", invalidate);
    row.append(label, input); form.append(row);
  }
  function reset() {
    const nozzle = options.summary()?.settings.nozzleMm ?? 0.4;
    defaultsNozzle = nozzle;
    const values: PrintabilityParams = { extrusionMm: nozzle * 1.1, perimeters: 2, wallMm: nozzle * 2.2,
      detailMm: nozzle * 1.1, componentMm: 2, overhangDeg: 45, contactMm2: 4, toleranceMm: 0.001, samples: 512 };
    for (const [key] of fields) (document.getElementById(`pa-${key}`) as HTMLInputElement).value = String(Number(values[key].toFixed(5)));
    invalidate();
  }
  function read(): PrintabilityParams | null {
    const result = {} as PrintabilityParams;
    for (const [key] of fields) {
      const el = document.getElementById(`pa-${key}`) as HTMLInputElement;
      if (!el.reportValidity()) return null;
      const n = Number(el.value);
      if (!Number.isFinite(n) || (key === "samples" || key === "perimeters") && !Number.isInteger(n)) { el.reportValidity(); return null; }
      result[key] = n;
    }
    return result;
  }
  function invalidate() {
    revision++; report = null; output.replaceChildren(); options.locate([]);
    status.textContent = t("printability.stale");
    const s = options.summary();
    analyze.disabled = busy || !s; cleanup.disabled = busy || !s || s.settings.repair;
    const profile = s ? `${s.settings.printer}/${s.settings.nozzleMm}` : "";
    if (profile !== lastProfile) {
      lastProfile = profile;
      if (s && defaultsNozzle !== s.settings.nozzleMm) reset();
    }
  }
  function show(r: PrintabilityReport) {
    output.replaceChildren();
    const errors = r.checks.filter(c => c.severity === "error").length;
    const warnings = r.checks.filter(c => c.severity === "warning").length;
    status.textContent = t("printability.summary", { errors, warnings }) + " " + t("printability.incomplete");
    const context = document.createElement("p"); context.className = "help";
    const s = options.summary()!;
    context.textContent = t("printability.context", { printer: r.printer ?? s.settings.printer, nozzle: num(r.nozzleMm ?? s.settings.nozzleMm, 2), layer: num(r.layerMm ?? s.settings.layerMm, 2), size: r.sizeMm.map(v => num(v, 2)).join(" × "), triangles: r.triangles });
    output.append(context);
    for (const c of r.checks) {
      const row = document.createElement("div"); row.className = "printability__check"; row.dataset.severity = c.severity;
      const title = document.createElement("strong"); title.textContent = tKey(`printability.check.${c.code}`);
      const text = document.createElement("p"); text.className = "help";
      text.textContent = `${tKey(`printability.evidence.${c.evidence}`)} · ${tKey(`printability.severity.${c.severity}`)}`;
      if (c.evidence !== "notEvaluated") text.textContent += ` · ${t("printability.count", { n: c.count })}`;
      if (c.measurement !== null) text.textContent += ` · ${t("printability.measured", { n: num(c.measurement, 3) })}`;
      if (c.threshold !== null) text.textContent += ` · ${t("printability.threshold", { n: num(c.threshold, 3) })}`;
      const help = document.createElement("p"); help.className = "help"; help.textContent = tKey(`printability.help.${c.code}`);
      row.append(title, text, help);
      if (c.locations.length && c.count > 0) {
        const locate = document.createElement("button"); locate.type = "button"; locate.className = "link";
        locate.textContent = t("printability.locate"); locate.addEventListener("click", () => options.locate(c.locations)); row.append(locate);
      }
      output.append(row);
    }
    if (r.thickness.length) {
      const map = document.createElement("button"); map.type = "button"; map.className = "btn btn--quiet";
      map.textContent = t("printability.map"); map.addEventListener("click", () => options.locate(r.thickness.map(s => s.point), r.thickness.map(s => s.mm), Math.max(r.params.wallMm, r.params.extrusionMm * r.params.perimeters)));
      output.append(map);
    }
  }
  async function run(preview = false) {
    const params = read(); if (!params || !options.summary() || busy) return;
    const token = revision; busy = true; analyze.disabled = cleanup.disabled = true;
    status.textContent = t("printability.running");
    try {
      const before = await api.analyzePrintability(params);
      if (token !== revision) return;
      if (preview) {
        const after = await api.analyzePrintability(params, true);
        if (token !== revision) return;
        const dialog = document.querySelector<HTMLDialogElement>("#printability-dialog")!;
        const body = dialog.querySelector<HTMLElement>(".printability__comparison")!;
        body.replaceChildren();
        for (const a of after.checks) {
          const b = before.checks.find(c => c.code === a.code);
          if (b && (a.count !== b.count || a.evidence !== b.evidence)) {
            const p = document.createElement("p"); p.textContent = `${tKey(`printability.check.${a.code}`)}: ${b.count} → ${a.count} (${tKey(`printability.evidence.${a.evidence}`)})`; body.append(p);
          }
        }
        const info = document.createElement("p"); info.textContent = t("printability.compare", { before: before.triangles, after: after.triangles, sizeBefore: before.sizeMm.map(v => num(v, 3)).join(" × "), sizeAfter: after.sizeMm.map(v => num(v, 3)).join(" × ") }); body.prepend(info);
        dialog.returnValue = "cancel";
        dialog.showModal();
        const result = await new Promise<boolean>(resolve => dialog.addEventListener("close", () => resolve(dialog.returnValue === "apply"), { once: true }));
        if (token !== revision) return;
        if (result) { await options.apply(); await refresh(); }
        else { report = before; show(before); }
      } else { report = before; show(before); }
    } catch (e) { if (token === revision) { status.textContent = t("printability.failed"); options.error(e); } }
    finally { busy = false; analyze.disabled = !options.summary(); cleanup.disabled = !options.summary() || Boolean(options.summary()?.settings.repair); }
  }
  async function refresh() {
    const params = read(); if (!params) return;
    const token = revision;
    try {
      const next = await api.analyzePrintability(params);
      if (token !== revision) return;
      report = next; show(next);
    } catch (e) {
      if (token === revision) { status.textContent = t("printability.failed"); options.error(e); }
    }
  }
  analyze.addEventListener("click", () => void run()); cleanup.addEventListener("click", () => void run(true));
  document.querySelector("#printability-reset")!.addEventListener("click", reset);
  onLangChange(() => {
    for (const el of form.querySelectorAll<HTMLElement>("[data-param]")) el.textContent = tKey(`printability.param.${el.dataset.param}`);
    if (report) show(report); else status.textContent = t("printability.stale");
  });
  for (const el of form.querySelectorAll<HTMLElement>("[data-param]")) el.textContent = tKey(`printability.param.${el.dataset.param}`);
  reset();
  return { invalidate };
}
