// Wires the window: file in (drop or dialog), settings out to Rust, the
// summary back into the panels, and the preview in sync with the painting.
// Every word the user reads comes from the catalogues in ./i18n.
import "@fontsource/schibsted-grotesk/700.css";
import "@fontsource/instrument-sans/400.css";
import "@fontsource/instrument-sans/500.css";
import "@fontsource/dm-mono/400.css";
import "@fontsource/dm-mono/500.css";
import "./styles/app.css";

import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { ask, open, save } from "@tauri-apps/plugin-dialog";
import {
  api,
  isAppError,
  type PlatePrefs,
  type PrinterProfile,
  type Printers,
  type Prefs,
  type Progress,
  type Summary,
  type Vec3,
  type WastePrefs,
} from "./api";
import { applyStatic, currentLang, inputValue, LANGS, length, list, mass, num, onLangChange, setLang, t, tKey, volume, type Key, type Lang } from "./i18n";
import { menubar } from "./menubar";
import { Picker, type PickerItem, type PickerTexts } from "./picker";
import { companionsDrop, gatherCompanions } from "./companions";
import { currentVersion, findUpdate, installUpdate, type Update } from "./updater";
import { Viewer, type Tool, type View } from "./viewer";

type State = "empty" | "converting" | "error" | "ready";

const app = document.querySelector<HTMLElement>("[data-app]")!;
const $ = <T extends HTMLElement>(sel: string) => document.querySelector<T>(sel)!;
const live = $<HTMLParagraphElement>("#live");
const nf = (n: number, d = 0) => num(n, d);
/** Millimetres as a printer spec reads them: 0,4, 0,25, 256. */
const mm = (n: number) => num(n, 2, false);
const dims = (v: number[], d = 1) => `${v.map((x) => nf(x, d)).join(" × ")} mm`;
/** What Blender can import for us; `bp_blend::INPUT_EXTENSIONS` is the same list. */
const MODEL_EXTENSIONS = ["blend", "obj", "fbx", "gltf", "glb", "ply"];
const MODEL_EXT = new RegExp(`\\.(${MODEL_EXTENSIONS.join("|")})$`, "i");

let summary: Summary | null = null;
let printers: Printers | null = null;
let prefs: Prefs | null = null;
/** Swaps before the last change, to show what a setting did. */
let previousSwaps: number | null = null;
let brushSlot = 0;
let state: State = "empty";
/** The error shown in the tray, kept to word it again in another language. */
let shownError: unknown = null;
let step: Progress = { key: "blender", name: "" };

function bind(key: string, text: string) {
  document.querySelectorAll(`[data-bind="${key}"]`).forEach((el) => (el.textContent = text));
}

/** A backend error in the user's language. */
function errText(e: unknown): string {
  if (isAppError(e)) {
    const params = { ...e.params };
    for (const [k, v] of Object.entries(params)) if (typeof v === "number") params[k] = mm(v);
    return tKey(e.key, params, t("error.unknown", { detail: String(e.params?.detail ?? e.key) }));
  }
  return String(e);
}

function setState(s: State, error?: unknown) {
  state = s;
  app.dataset.state = s;
  if (s === "error") shownError = error ?? shownError;
  bind("error", s === "error" ? errText(shownError) : "");
  bind("step", tKey(`progress.${step.key}`, { name: step.name }, step.name));
  $<HTMLButtonElement>("#choose").disabled = s === "converting";
  for (const id of ["#export", "#open-zip"]) $<HTMLButtonElement>(id).setAttribute("aria-disabled", String(s !== "ready"));
  live.textContent = t(`state.${s}` as Key);
}

// ---- toast ------------------------------------------------------------------------

const toastEl = $<HTMLDivElement>("#toast");
const toastAction = $<HTMLButtonElement>("#toast-action");
let toastTimer = 0;
let onToastAction: (() => void) | null = null;

/** Hides the notice. */
const hideToast = () => toastEl.matches(":popover-open") && toastEl.hidePopover();

/** Where the notice lives when no dialog is open. */
const toastHome = toastEl.parentElement!;

/** Shows the notice in the window's corner. It is a popover, so it lands above
 *  any open dialog; it moves into that dialog, since a modal dialog makes
 *  everything outside it inert and a click on the notice would reach the
 *  dialog's backdrop instead. */
function raiseToast() {
  hideToast();
  const host = document.querySelector<HTMLDialogElement>("dialog[open]") ?? toastHome;
  if (toastEl.parentElement !== host) host.append(toastEl);
  toastEl.showPopover();
}

/** A visible notice on the stage. Errors stay until closed; the rest fade after 8 s. */
function toast(message: string, kind: "ok" | "error" = "ok", action?: { label: string; run: () => void }, sticky = false) {
  window.clearTimeout(toastTimer);
  bind("toast", message);
  toastEl.dataset.kind = kind;
  toastEl.setAttribute("role", kind === "error" ? "alert" : "status");
  raiseToast();
  onToastAction = action?.run ?? null;
  toastAction.hidden = !action;
  if (action) toastAction.textContent = action.label;
  if (kind !== "error" && !sticky) toastTimer = window.setTimeout(hideToast, 8000);
}
$("#toast-close").addEventListener("click", hideToast);
toastAction.addEventListener("click", () => {
  hideToast();
  onToastAction?.();
});

/** Runs a backend change with the stage marked as working; errors become a toast. */
async function work<T>(fn: () => Promise<T>): Promise<T | undefined> {
  app.classList.add("is-working");
  try {
    return await fn();
  } catch (e) {
    toast(errText(e), "error");
    return undefined;
  } finally {
    app.classList.remove("is-working");
  }
}

/** A change that repaints: apply, render, refresh the mesh. */
const apply = (call: () => Promise<Summary | null>, done?: () => void) =>
  work(async () => {
    previousSwaps = summary?.estimate.swaps ?? null;
    const s = await call();
    if (s) {
      render(s);
      await reloadMesh();
    }
    done?.();
  });

// ---- viewer ------------------------------------------------------------------------

/** Measured points without the per-axis stretch, so they stay on the model when it is resized. */
let measured: Vec3[] = [];

const viewer = new Viewer($<HTMLCanvasElement>("#viewport"), {
  pick: (o, d) => api.pick(o, d),
  paint: async (o, d) => {
    if (!summary) return null;
    const r = await api.paint(o, d, radiusMm(), brushSlot);
    if (r) viewer.applyPaint(r.tris, brushSlot);
    return r;
  },
  endStroke: () => {
    previousSwaps = summary?.estimate.swaps ?? null;
    void api.endStroke().then(render);
  },
  measure: (p) => {
    const st = summary?.size.stretch ?? [1, 1, 1];
    if (measured.length >= 2) measured = [];
    measured.push(p.map((v, i) => v / st[i]) as Vec3);
    syncMeasure();
  },
  distance: (a, b, kind) => {
    const d = nf(Math.hypot(...a.map((v, i) => v - b[i])) * (summary?.size.previewScale ?? 1), 2);
    return kind === "free" ? t("measure.short", { d }) : kind === "height" ? t("measure.height", { d }) : t("measure.axis", { axis: "XYZ"[kind], d });
  },
  resize: async (f) => {
    if (!summary) return false;
    const next = summary.size.targetMm.map((v, i) => Math.round(v * f[i] * 10) / 10);
    if (!next.every((v) => v >= SIZE_MIN && v <= SIZE_MAX)) {
      toast(t("error.sizeRange", { min: nf(SIZE_MIN), max: nf(SIZE_MAX) }), "error");
      return false;
    }
    return (await apply(() => api.setSize(next as Vec3))) !== undefined;
  },
});

/** Points in preview units for the current size. */
const measuredInPreview = () => {
  const st = summary?.size.stretch ?? [1, 1, 1];
  return measured.map((p) => p.map((v, i) => v * st[i]) as Vec3);
};

function syncMeasure() {
  const pts = measuredInPreview();
  viewer.setMeasure(pts);
  $<HTMLButtonElement>("#measure-clear").hidden = pts.length === 0;
  if (pts.length === 2 && summary) {
    const k = summary.size.previewScale;
    const d = pts[1].map((v, i) => Math.abs(v - pts[0][i]) * k);
    bind("measure", t("measure.distance", { d: nf(Math.hypot(...d), 2), x: nf(d[0], 2), y: nf(d[1], 2), z: nf(d[2], 2) }));
  } else {
    bind("measure", pts.length === 1 ? t("measure.second") : app.dataset.tool === "measure" ? t("measure.first") : "");
  }
}
$("#measure-clear").addEventListener("click", () => {
  measured = [];
  syncMeasure();
});

const radiusInput = $<HTMLInputElement>("#radius");
const radiusMm = () => Number(radiusInput.value);
function syncBrush() {
  bind("radiusOut", `${nf(radiusMm(), 1)} mm`);
  const scale = summary && viewer.modelHeight() > 0 ? summary.sizeMm[2] / viewer.modelHeight() : 1;
  viewer.brushRadius = radiusMm() / scale;
  const hex = summary?.slots[brushSlot]?.hex;
  if (hex) {
    app.style.setProperty("--brush", hex);
    viewer.setBrushColor(hex);
  }
  bind("brushSlot", t("slot.name", { n: brushSlot + 1 }));
}
radiusInput.addEventListener("input", syncBrush);

async function reloadMesh(frame = false) {
  viewer.load(await api.previewMesh(), frame);
  syncBrush();
  syncMeasure();
}

// ---- rendering the summary ------------------------------------------------------------

/** Why the model does not fit, the first problem first; null when it fits. */
function fitProblem(s: Summary): string | null {
  const [x, y, z] = s.sizeMm;
  const p = s.plate;
  const plate = dims([p.area.x1 - p.area.x0, p.area.y1 - p.area.y0], 0);
  if (!s.fitsHeight) return t("fit.tooTall", { z: nf(z), max: nf(s.maxHeightMm) });
  if (p.offPlate) return t("fit.offPlate", { size: dims([x, y], 0), plate });
  if (p.onExcluded) return t("fit.excluded");
  if (p.onTower) return t("fit.tower", { gap: mm(p.prefs.towerGapMm) });
  if (p.inMargin) return t("fit.margin", { margin: mm(p.prefs.marginMm) });
  return null;
}

/** Notices that inform rather than warn: shown as a plain line under the warnings. */
const IMPORT_INFO = new Set(["units", "blendTextures"]);

/** Import notices grouped by kind, a few names each; units are information, not a warning. */
function renderImportWarnings(s: Summary) {
  const box = $("#import-warnings");
  const info = s.importWarnings.filter((w) => IMPORT_INFO.has(w.code));
  const groups = new Map<string, string[]>();
  for (const w of s.importWarnings) if (!IMPORT_INFO.has(w.code)) groups.set(w.code, [...(groups.get(w.code) ?? []), w.detail]);
  const nodes: HTMLElement[] = [];
  const total = [...groups.values()].reduce((n, d) => n + d.length, 0);
  if (total) {
    const head = document.createElement("p");
    head.className = "import-warnings__head";
    head.textContent = t("import.warnings", { count: total });
    nodes.push(head);
    for (const [code, details] of groups) {
      const group = document.createElement("div");
      group.className = "import-warnings__group";
      const title = document.createElement("strong");
      title.textContent = tKey(`import.${code}`, {}, code);
      const names = document.createElement("ul");
      const SHOWN = 3;
      for (const detail of details.slice(0, SHOWN)) {
        const li = document.createElement("li");
        // Paths read best by file name; the whole path stays one hover away.
        li.textContent = detail.split(/[\\/]/).pop() || detail;
        li.title = detail;
        names.append(li);
      }
      if (details.length > SHOWN) {
        const li = document.createElement("li");
        li.className = "import-warnings__more";
        li.textContent = t("import.more", { count: details.length - SHOWN });
        li.title = details.slice(SHOWN).join("\n");
        names.append(li);
      }
      const help = document.createElement("p");
      help.className = "help";
      help.textContent = tKey(`import.${code}Help`);
      group.append(title, names, help);
      nodes.push(group);
    }
  }
  box.hidden = !total;
  box.replaceChildren(...nodes);
  const unitNote = $("#import-units");
  unitNote.hidden = !info.length;
  unitNote.textContent = info.map((w) => tKey(`import.${w.code}`, { detail: w.detail }, w.detail)).join(" ");
}

function render(s: Summary) {
  summary = s;
  bind("fileName", s.fileName);
  bind("fileMeta", t("file.meta", { mb: nf(s.fileSize / 1048576, 1), count: s.materials.length, n: s.materials.length }));

  renderSize(s);
  renderPlate(s);

  // Settings.
  const st = s.settings;
  setIfIdle("#keep", String(Math.round(st.keepRatio * 100)));
  setIfIdle("#flatten", String(st.flattenMm));
  setIfIdle("#piece", String(st.minPieceMm));
  setIfIdle("#detail", String(st.borderDetail));
  setIfIdle("#island", String(st.minIslandMm2));
  setIfIdle("#reduce-to", String(Math.max(1, s.slots.length - 1)));
  $<HTMLInputElement>("#repair").checked = st.repair;
  renderPrinterPick(st.printer, st.nozzleMm);
  syncOutputs();

  // Colour patches the nozzle cannot draw.
  const fixTo = Math.ceil(s.minDetailMm2);
  bind("detailNote", t("colour.smallIslands", { count: s.smallIslands, n: nf(s.smallIslands), area: mm(s.minDetailMm2), nozzle: mm(st.nozzleMm) }));
  bind("detailFix", t("colour.fixIslands", { area: nf(fixTo) }));
  document.querySelectorAll<HTMLInputElement>("#count input").forEach((i) => (i.checked = Number(i.value) === s.slots.length));

  // Mesh.
  bind("triangles", s.triangles === s.sourceTriangles
    ? t("mesh.triangles", { n: nf(s.triangles), split: nf(s.splitTriangles) })
    : t("mesh.trianglesOf", { n: nf(s.triangles), of: nf(s.sourceTriangles) }));
  bind("contact", st.flattenMm > 0
    ? t("mesh.contactFlat", { mm: nf(st.flattenMm, 1), area: nf(s.contactMm2) })
    : t("mesh.contact", { area: nf(s.contactMm2) }));
  const r = s.repair;
  const fixes = [
    r.weldedVertices && t("mesh.welded", { count: r.weldedVertices, n: nf(r.weldedVertices) }),
    r.degenerateTriangles && t("mesh.degenerate", { count: r.degenerateTriangles, n: nf(r.degenerateTriangles) }),
    r.duplicateTriangles && t("mesh.duplicate", { count: r.duplicateTriangles, n: nf(r.duplicateTriangles) }),
    r.loosePieces && t("mesh.loosePieces", { count: r.loosePieces, n: nf(r.loosePieces), tris: nf(r.looseTriangles) }),
  ].filter((x): x is string => Boolean(x));
  bind("repairOut", !st.repair && !r.loosePieces ? t("mesh.notRepaired") : fixes.length ? t("mesh.fixed", { list: fixes.join(", ") }) : t("mesh.clean"));
  bind("meshSum", t("mesh.sum", { n: nf(s.triangles) }));
  bind("colorSum", t("colour.sum", { count: s.slots.length, n: s.slots.length }));

  // Filament changes, with what the last change did.
  bind("swaps", nf(s.estimate.swaps));
  bind("swapsWord", t("swaps.word", { count: s.estimate.swaps }));
  const delta = previousSwaps === null ? 0 : s.estimate.swaps - previousSwaps;
  const deltaEl = document.querySelector<HTMLElement>('[data-bind="swapsDelta"]')!;
  deltaEl.textContent = delta === 0 ? "" : t("swaps.delta", { sign: delta < 0 ? "−" : "+", n: nf(Math.abs(delta)) });
  deltaEl.classList.toggle("is-better", delta < 0);
  bind("swapsDetail", t("swaps.detail", { multi: nf(s.estimate.multicolorLayers), layers: nf(s.estimate.layers), layer: mm(st.layerMm) }));

  renderWaste(s);
  renderSlots(s);
  renderImportWarnings(s);
  const mats = $<HTMLDListElement>("#materials");
  mats.replaceChildren(...s.materials.flatMap((m) => {
    const dt = document.createElement("dt");
    dt.textContent = m.name;
    const dd = document.createElement("dd");
    dd.textContent = `${tKey(`material.${m.kind}`, { detail: m.detail }, m.detail)}, ${t("material.faces", { count: m.faces, n: nf(m.faces) })}`;
    return [dt, dd];
  }));

  const undoBtn = $<HTMLButtonElement>("#undo");
  undoBtn.disabled = !s.canUndo;
  undoBtn.title = s.undoLabel ? t("undo.title", { what: t(`undo.${s.undoLabel}`) }) : t("undo.nothing");
  bind("exportLabel", t("export.label"));
  viewer.setPalette(s.slots.map((x) => x.hex));
  syncBrush();
  if (wasteDialog.open) fillFlushTable(s);
}

/** Only overwrite a field the user is not editing. */
function setIfIdle(sel: string, value: string) {
  const el = $<HTMLInputElement>(sel);
  if (document.activeElement !== el) el.value = value;
}

function syncOutputs() {
  bind("keepOut", `${$<HTMLInputElement>("#keep").value} %`);
  const d = Number($<HTMLInputElement>("#detail").value);
  bind("detailOut", d === 0 ? t("colour.detailOff") : t("colour.detailLevel", { n: d }));
  const i = Number($<HTMLInputElement>("#island").value);
  bind("islandOut", i === 0 ? t("colour.islandsOff") : `${i} mm²`);
  // The warning stays until islands are cleaned at the recommended size or more.
  $<HTMLDivElement>("#detail-note").classList.toggle("is-gone", !summary || summary.smallIslands === 0 || i >= Math.ceil(summary.minDetailMm2));
}

// ---- size ------------------------------------------------------------------------------

const axisInputs = [0, 1, 2].map((i) => $<HTMLInputElement>(`#size-${"xyz"[i]}`));
const lockBtn = $<HTMLButtonElement>("#lock");
const scaleInput = $<HTMLInputElement>("#scale-pct");
const SIZE_MIN = 1;
const SIZE_MAX = 1000;
let locked = true;

// Sizes are kept to hundredths of a millimetre, so equal factors can differ by that much.
const uniform = (k: number[]) => k.every((v) => Math.abs(v - k[0]) < 5e-4 * k[0]);

/** The tool's shortcuts under the view; the whole line also as a tooltip, for when it is cut. */
function setHint(text: string) {
  bind("hint", text);
  $(".stage__hint").title = text;
}

function syncLock() {
  lockBtn.setAttribute("aria-pressed", String(locked));
  viewer.proportional = locked;
  bind("lockLabel", locked ? t("size.locked") : t("size.unlocked"));
}

function renderSize(s: Summary) {
  const z = s.size;
  axisInputs.forEach((el, i) => setIfIdle(`#${el.id}`, inputValue(z.targetMm[i])));
  const even = uniform(z.axisScale);
  setIfIdle("#scale-pct", even ? inputValue(z.axisScale[0] * 100, 2) : "");
  scaleInput.placeholder = even ? "" : "—";
  bind("scaleHelp", even
    ? t("size.scaleHelp")
    : t("size.stretched", { x: nf(z.axisScale[0] * 100, 1), y: nf(z.axisScale[1] * 100, 1), z: nf(z.axisScale[2] * 100, 1) }));
  viewer.setSizeValues(z.targetMm, (n) => nf(n, 1));
  viewer.setPlate(s.plate, z.previewScale);
  bind("sizeOriginal", dims(z.originalMm, 2));
  bind("sizeResult", dims(s.sizeMm, 2));
  bind("dims", dims(s.sizeMm, 1));
  $<HTMLButtonElement>("#size-undo").disabled = !z.canUndo;
  const reset = z.targetMm.every((v, i) => Math.abs(v - z.originalMm[i]) < 1e-4);
  $<HTMLButtonElement>("#size-reset").disabled = reset;

  const problem = fitProblem(s);
  bind("bedFit", problem ?? t("fit.ok", { plate: dims([s.bedMm[0], s.bedMm[1]], 0), max: nf(s.maxHeightMm) }));
  bind("fitBanner", problem ?? "");
  $<HTMLDivElement>("#fit-banner").hidden = problem === null;
  app.classList.toggle("is-oversize", problem !== null);
}

/** Sends a new size after checking every axis is in range. */
function requestSize(next: number[], field?: HTMLInputElement) {
  const ok = next.every((v) => Number.isFinite(v) && v >= SIZE_MIN && v <= SIZE_MAX);
  field?.setAttribute("aria-invalid", String(!ok));
  if (!ok) return toast(t("error.sizeRange", { min: nf(SIZE_MIN), max: nf(SIZE_MAX) }), "error");
  void apply(() => api.setSize(next.map((v) => Math.round(v * 100) / 100) as Vec3));
}

/** One axis typed in the panel or on the view; locked, the other two follow. */
function resizeAxis(axis: number, v: number, field?: HTMLInputElement) {
  if (!summary) return;
  const now = summary.size.targetMm;
  if (!(v > 0)) return requestSize([NaN, NaN, NaN], field);
  requestSize(locked ? now.map((x) => (x * v) / now[axis]) : now.map((x, i) => (i === axis ? v : x)), field);
}
axisInputs.forEach((el, axis) => el.addEventListener("change", () => resizeAxis(axis, Number(el.value), el)));

function toggleLock() {
  locked = !locked;
  syncLock();
  if (app.dataset.tool === "size") setHint(t("hint.size", { lock: locked ? t("size.locked") : t("size.unlocked") }));
}
lockBtn.addEventListener("click", toggleLock);

/** A uniform scale against the original: it brings back the original proportions. */
function scaleTo(pct: number) {
  if (!summary) return;
  if (!(pct > 0)) return requestSize([NaN, NaN, NaN], scaleInput);
  requestSize(summary.size.originalMm.map((v) => (v * pct) / 100), scaleInput);
}
scaleInput.addEventListener("change", () => scaleTo(Number(scaleInput.value)));
const nudge = (by: number) => {
  if (!summary) return;
  const k = summary.size.axisScale;
  scaleTo(Math.round((uniform(k) ? k[0] : k[2]) * 100 + by));
};
$("#scale-down").addEventListener("click", () => nudge(-1));
$("#scale-up").addEventListener("click", () => nudge(1));

const fit = () => {
  if (!summary) return;
  void apply(() => api.fitToPlate(), () => toast(t("size.fitted", { size: dims(summary!.size.targetMm, 1) })));
};
$("#fit").addEventListener("click", fit);
$("#fit-side").addEventListener("click", fit);
$("#size-reset").addEventListener("click", () => void apply(() => api.resetSize()));
$("#size-undo").addEventListener("click", () => void apply(() => api.undoSize()));

// ---- plate and tower --------------------------------------------------------------------

const plateSvg = $<SVGSVGElement & HTMLElement>("#plate-view");

function renderPlate(s: Summary) {
  const p = s.plate;
  const a = p.area;
  const pad = Math.max(a.x1 - a.x0, a.y1 - a.y0) * 0.03;
  const [w, h] = [a.x1 - a.x0, a.y1 - a.y0];
  plateSvg.setAttribute("viewBox", `${a.x0 - pad} ${-a.y1 - pad} ${w + 2 * pad} ${h + 2 * pad}`);
  // Plate Y grows to the back; the SVG draws it upwards.
  const rect = (r: { x0: number; y0: number; x1: number; y1: number }, cls: string) =>
    `<rect class="${cls}" x="${r.x0}" y="${-r.y1}" width="${Math.max(0, r.x1 - r.x0)}" height="${Math.max(0, r.y1 - r.y0)}"/>`;
  const gap = p.prefs.towerGapMm;
  const tower = p.tower;
  plateSvg.innerHTML = [
    rect(a, "plate__area"),
    rect(p.usable, "plate__usable"),
    ...p.excluded.map((e) => rect(e, "plate__excl")),
    tower ? rect({ x0: tower.x0 - gap, y0: tower.y0 - gap, x1: tower.x1 + gap, y1: tower.y1 + gap }, "plate__keep") : "",
    tower ? rect(tower, "plate__tower") : "",
    rect(p.model, `plate__model${p.fits ? "" : " is-bad"}`),
  ].join("");

  const issues: string[] = [];
  if (p.offPlate) issues.push(t("plate.issueOff"));
  else if (p.inMargin) issues.push(t("plate.issueMargin", { margin: mm(p.prefs.marginMm) }));
  if (p.onExcluded) issues.push(t("plate.issueExcluded"));
  if (p.onTower) issues.push(t("plate.issueTower"));
  if (p.towerOffPlate) issues.push(t("plate.issueTowerOff"));
  if (p.prefs.tower && p.needsTower && p.fitFactor < 1 && p.fits === false) {
    issues.push(t("plate.issueFit", { pct: nf(p.fitFactor * 100) }));
  }
  $("#plate-issues").replaceChildren(...issues.map((text) => Object.assign(document.createElement("li"), { textContent: text })));
  bind("plateSum", p.fits ? t("plate.sumOk") : t("plate.sumBad", { count: issues.length, n: issues.length }));
  bind("plateDesc", [
    t("plate.desc", { plate: dims([w, h], 0), model: dims([p.model.x1 - p.model.x0, p.model.y1 - p.model.y0], 0) }),
    tower ? t("plate.descTower", { size: dims(p.towerMm, 0) }) : "",
    ...issues,
  ].join(" "));

  $<HTMLInputElement>("#tower-on").checked = p.prefs.tower;
  setIfIdle("#tower-depth", p.prefs.towerDepthMm === null ? "" : inputValue(p.prefs.towerDepthMm, 1));
  $<HTMLInputElement>("#tower-depth").placeholder = nf(p.towerMm[1] - 6, 0);
  setIfIdle("#tower-gap", inputValue(p.prefs.towerGapMm, 1));
  setIfIdle("#margin", inputValue(p.prefs.marginMm, 1));
  bind("towerDepthHelp", p.towerEstimated
    ? t("plate.depthEstimated", { depth: nf(p.towerMm[1] - 6) })
    : t("plate.depthSet"));
  bind("towerNote", !p.needsTower
    ? t("plate.noTower")
    : tower
      ? t("plate.towerNote", { x: nf(tower.x0), y: nf(tower.y0), size: dims(p.towerMm, 0) })
      : t("plate.towerOff"));
}

/** The plate form as settings; null with the offending field marked. */
function readPlate(): PlatePrefs | null {
  const depthEl = $<HTMLInputElement>("#tower-depth");
  const fields: [HTMLInputElement, number, number][] = [
    [$<HTMLInputElement>("#tower-gap"), 0, 50],
    [$<HTMLInputElement>("#margin"), 0, 50],
  ];
  const depth = depthEl.value.trim() === "" ? null : Number(depthEl.value);
  for (const [el, lo, hi] of [...fields, ...(depth === null ? [] : [[depthEl, 5, 250] as [HTMLInputElement, number, number]])]) {
    const v = Number(el.value);
    const ok = el.value.trim() !== "" && v >= lo && v <= hi;
    el.setAttribute("aria-invalid", String(!ok));
    if (!ok) {
      toast(t("error.rangeField", { field: document.querySelector(`label[for="${el.id}"]`)?.textContent ?? "", min: nf(lo), max: nf(hi) }), "error");
      return null;
    }
  }
  return { tower: $<HTMLInputElement>("#tower-on").checked, towerDepthMm: depth, towerGapMm: Number(fields[0][0].value), marginMm: Number(fields[1][0].value) };
}

async function savePlate(next: PlatePrefs | null) {
  if (!next) return;
  await apply(async () => {
    const s = await api.setPlate(next);
    if (prefs) prefs.plate = next;
    return s;
  });
}
["#tower-on", "#tower-depth", "#tower-gap", "#margin"].forEach((sel) => $(sel).addEventListener("change", () => void savePlate(readPlate())));
$("#plate-reset").addEventListener("click", () => {
  if (prefs) void savePlate(prefs.plateDefaults);
});

// ---- waste ----------------------------------------------------------------------------

function renderWaste(s: Summary) {
  const w = s.waste;
  const g = (mm3: number) => mass((mm3 * w.prefs.densityGCm3) / 1000);
  bind("wasteTotal", mass(w.totalG));
  bind("wasteTotalDetail", t("waste.totalDetail", { length: length(w.totalM), volume: volume(w.totalMm3) }));
  const rows: [string, string, string?][] = [
    [t("waste.flush"), g(w.flushMm3), t("waste.flushNote", { count: w.swaps, n: nf(w.swaps) })],
    [t("waste.start"), g(w.startMm3)],
    [t("waste.tower"), g(w.towerMm3), w.towerMm3 > 0 ? t("waste.towerNote", { h: w.towerHeightMm < 1000 ? `${nf(w.towerHeightMm, 1)} mm` : length(w.towerHeightMm / 1000) }) : t("waste.noTower")],
  ];
  if (w.reusedMm3 > 0) rows.push([t("waste.reused"), g(w.reusedMm3), t("waste.reusedNote")]);
  $("#waste-breakdown").replaceChildren(...rows.flatMap(([label, value, note]) => {
    const dt = Object.assign(document.createElement("dt"), { textContent: label });
    const dd = document.createElement("dd");
    dd.append(Object.assign(document.createElement("span"), { className: "num", textContent: value }));
    if (note) dd.append(Object.assign(document.createElement("small"), { textContent: note }));
    return [dt, dd];
  }));
  bind("wasteAssumes", [
    fitProblem(s) ? t("waste.oversize") : "",
    t("waste.assumes", {
      layer: mm(w.layerMm),
      nozzle: mm(w.nozzleMm),
      diameter: mm(w.prefs.filamentDiameterMm),
      density: mm(w.prefs.densityGCm3),
      k: mm(w.prefs.flushMultiplier),
    }),
    w.purgeChute ? t("waste.chute") : t("waste.intoTower"),
    t("waste.limits"),
  ].filter(Boolean).join(" "));
}

const wasteDialog = $<HTMLDialogElement>("#waste-dialog");
const wasteForm = $<HTMLFormElement>("#waste-form");
const wasteError = $<HTMLParagraphElement>("#waste-error");
const wf = {
  layer: $<HTMLInputElement>("#w-layer"),
  material: $<HTMLSelectElement>("#w-material"),
  density: $<HTMLInputElement>("#w-density"),
  diameter: $<HTMLInputElement>("#w-diameter"),
  multiplier: $<HTMLInputElement>("#w-multiplier"),
  start: $<HTMLInputElement>("#w-start"),
  prime: $<HTMLInputElement>("#w-prime"),
  reuse: $<HTMLInputElement>("#w-reuse"),
};
/** Typical densities, g/cm³. */
const MATERIALS: [string, number][] = [["PLA", 1.24], ["PETG", 1.27], ["ABS", 1.04], ["ASA", 1.07], ["TPU", 1.21], ["PLA-CF", 1.3]];

function fillMaterials() {
  wf.material.replaceChildren(
    ...MATERIALS.map(([name, d]) => new Option(`${name} (${mm(d)} g/cm³)`, String(d))),
    new Option(t("wasteForm.other"), ""),
  );
  syncMaterial();
}
function syncMaterial() {
  const d = Number(wf.density.value);
  wf.material.value = MATERIALS.find(([, v]) => Math.abs(v - d) < 1e-6)?.[1].toString() ?? "";
}
wf.material.addEventListener("change", () => {
  if (wf.material.value) wf.density.value = wf.material.value;
});
wf.density.addEventListener("input", syncMaterial);

function fillWasteForm(p: WastePrefs) {
  wf.layer.value = p.layerMm === null ? "" : inputValue(p.layerMm, 3);
  wf.density.value = inputValue(p.densityGCm3, 3);
  wf.diameter.value = inputValue(p.filamentDiameterMm, 2);
  wf.multiplier.value = inputValue(p.flushMultiplier, 2);
  wf.start.value = inputValue(p.startPurgeMm3, 0);
  wf.prime.value = inputValue(p.primeMm3, 0);
  wf.reuse.value = inputValue(p.flushReuse * 100, 0);
  for (const el of Object.values(wf)) el.removeAttribute("aria-invalid");
  wasteError.hidden = true;
  syncMaterial();
  const nozzle = summary?.waste.nozzleMm ?? printers?.nozzleMm ?? 0.4;
  wf.layer.max = String(Math.round(nozzle * 80) / 100);
  bind("wasteNozzle", `${mm(nozzle)} mm`);
  bind("layerHelp", t("wasteForm.layerHelp", { max: mm(Number(wf.layer.max)) }));
}

function readWasteForm(): WastePrefs {
  return {
    layerMm: wf.layer.value.trim() === "" ? null : Number(wf.layer.value),
    densityGCm3: Number(wf.density.value),
    filamentDiameterMm: Number(wf.diameter.value),
    flushMultiplier: Number(wf.multiplier.value),
    startPurgeMm3: Number(wf.start.value),
    primeMm3: Number(wf.prime.value),
    flushReuse: Number(wf.reuse.value) / 100,
  };
}

/** Which field each range error points at. */
const WASTE_FIELDS: Record<string, HTMLInputElement> = {
  "error.layerRange": wf.layer,
  "error.densityRange": wf.density,
  "error.filamentRange": wf.diameter,
  "error.multiplierRange": wf.multiplier,
  "error.startPurgeRange": wf.start,
  "error.primeRange": wf.prime,
  "error.reuseRange": wf.reuse,
};

async function saveWaste(next: WastePrefs, done: string) {
  for (const el of Object.values(wf)) el.removeAttribute("aria-invalid");
  wasteError.hidden = true;
  try {
    previousSwaps = summary?.estimate.swaps ?? null;
    const s = await api.setWaste(next);
    if (prefs) prefs.waste = next;
    if (s) {
      render(s);
      await reloadMesh();
    }
    toast(done);
  } catch (e) {
    wasteError.textContent = isAppError(e) && e.key === "error.reuseRange" ? t("error.reusePct") : errText(e);
    wasteError.hidden = false;
    const field = isAppError(e) ? WASTE_FIELDS[e.key] : undefined;
    field?.setAttribute("aria-invalid", "true");
    field?.focus();
  }
}

wasteForm.addEventListener("submit", (e) => {
  e.preventDefault();
  void saveWaste(readWasteForm(), t("wasteForm.saved"));
});
$("#waste-defaults").addEventListener("click", () => {
  if (!prefs) return;
  fillWasteForm(prefs.wasteDefaults);
  void saveWaste(prefs.wasteDefaults, t("wasteForm.reset"));
});
$("#waste-close").addEventListener("click", () => wasteDialog.close());
$("#open-waste").addEventListener("click", async () => {
  if (app.dataset.state !== "ready" || !summary) return;
  prefs = await api.prefs();
  fillMaterials();
  fillWasteForm(prefs.waste);
  fillFlushTable(summary);
  wasteDialog.showModal();
});

/** The flush from each slot (rows) to each other slot (columns). */
function fillFlushTable(s: Summary) {
  const n = s.slots.length;
  const w = s.waste;
  bind("flushHelp", n < 2 ? t("wasteForm.flushOne") : t("wasteForm.flushHelp", { k: mm(w.prefs.flushMultiplier) }));
  $<HTMLButtonElement>("#flush-reset").hidden = !w.flushSet.some(Boolean);
  const table = document.createElement("table");
  if (n >= 2) {
    const swatch = (i: number) => `<span class="flush__slot" style="--t:${s.slots[i].hex}">${i + 1}</span>`;
    table.innerHTML = `<caption class="visually-hidden">${t("wasteForm.flushCaption")}</caption>
      <thead><tr><th scope="col"><span class="visually-hidden">${t("wasteForm.flushFromTo")}</span></th>${s.slots.map((_, j) => `<th scope="col" title="${t("slot.name", { n: j + 1 })}">${swatch(j)}</th>`).join("")}</tr></thead>
      <tbody>${s.slots.map((_, i) => `<tr><th scope="row" title="${t("slot.name", { n: i + 1 })}">${swatch(i)}</th>${s.slots.map((_, j) => (i === j
        ? `<td class="flush__self" aria-hidden="true">—</td>`
        : `<td><input class="input flush__cell${w.flushSet[i * n + j] ? " is-set" : ""}" type="number" inputmode="numeric" min="0" max="2000" step="10"
            style="--heat:${Math.min(1, Math.max(0, (w.flushMm3Matrix[i * n + j] - 107) / 693)).toFixed(2)}"
            data-from="${i}" data-to="${j}" value="${Math.round(w.flushMm3Matrix[i * n + j])}"
            aria-label="${t("wasteForm.flushCell", { from: i + 1, to: j + 1 })}"></td>`)).join("")}</tr>`).join("")}</tbody>`;
  }
  $("#flush-table").replaceChildren(table);
  table.querySelectorAll<HTMLInputElement>("input").forEach((el) => el.addEventListener("change", () => {
    const v = el.value.trim() === "" ? null : Number(el.value);
    const ok = v === null || (v >= 0 && v <= 2000);
    el.setAttribute("aria-invalid", String(!ok));
    if (!ok) return toast(t("error.flushRange", { min: "0", max: nf(2000) }), "error");
    void apply(() => api.setFlush(Number(el.dataset.from), Number(el.dataset.to), v));
  }));
}
$("#flush-reset").addEventListener("click", () => void apply(() => api.setFlush(null, null, null)));

const CLOSE_DELTA_E = 15;

function renderSlots(s: Summary) {
  if (brushSlot >= s.slots.length) brushSlot = 0;
  const list = $<HTMLOListElement>("#slots");
  list.replaceChildren(...s.slots.map((slot, i) => {
    const li = document.createElement("li");
    li.className = "slot";
    li.style.setProperty("--t", slot.hex);
    const near = slot.nearest;
    const close = near !== null && near[1] < CLOSE_DELTA_E;
    const n = i + 1;
    li.innerHTML = `
      <label class="slot__edit" title="${t("slot.changeColour", { n })}"><span class="visually-hidden">${t("slot.colourOf", { n })}</span><input type="color" value="${slot.hex.toLowerCase()}"></label>
      <button class="slot__pick" type="button" aria-pressed="${i === brushSlot}" title="${t("slot.paintWith", { n })}">
        <span class="slot__no"><span>${t("slot.name", { n })}</span><span class="slot__brush">${i === brushSlot ? t("slot.brush") : t("slot.useBrush")}</span></span>
        <strong class="mono">${slot.hex}</strong>
        <span class="slot__count">${t("slot.triangles", { count: slot.triangles, n: nf(slot.triangles) })}</span>
        <span class="slot__near${close ? " is-close" : ""}">${near ? t("slot.near", { n: near[0] + 1, de: nf(near[1]) }) : t("slot.only")}</span>
      </button>
      <div class="slot__actions">${s.slots.length < 2 ? "" : `
        ${close ? `<button class="btn btn--quiet btn--sm slot__join" type="button">${t("slot.joinWith", { n: near![0] + 1 })}</button>` : ""}
        <label class="visually-hidden" for="merge-${i}">${t("slot.mergeLabel", { n })}</label>
        <select class="slot__merge" id="merge-${i}">
          <option value="">${t("slot.mergeInto")}</option>
          ${s.slots.map((_, j) => (j === i ? "" : `<option value="${j}">${t("slot.name", { n: j + 1 })}</option>`)).join("")}
        </select>`}
      </div>`;

    const input = li.querySelector<HTMLInputElement>('input[type="color"]')!;
    input.addEventListener("input", () => li.style.setProperty("--t", input.value));
    input.addEventListener("change", () => {
      const colors = s.slots.map((x, j) => (j === i ? input.value : x.hex));
      void apply(() => api.setPalette(colors));
    });
    li.querySelector(".slot__pick")!.addEventListener("click", () => {
      brushSlot = i;
      list.querySelectorAll(".slot__pick").forEach((b, j) => {
        b.setAttribute("aria-pressed", String(j === i));
        b.querySelector(".slot__brush")!.textContent = j === i ? t("slot.brush") : t("slot.useBrush");
      });
      setTool("brush");
      syncBrush();
    });
    const merge = (into: number) =>
      void apply(() => api.mergeSlots(i, into), () =>
        toast(t("slot.merged", { from: i + 1, into: into + 1 }), "ok", { label: t("undo.label"), run: () => void undo() }));
    li.querySelector(".slot__join")?.addEventListener("click", () => merge(near![0]));
    li.querySelector<HTMLSelectElement>(".slot__merge")?.addEventListener("change", (e) => {
      const v = (e.target as HTMLSelectElement).value;
      if (v !== "") merge(Number(v));
    });
    return li;
  }));
}

// ---- opening files ---------------------------------------------------------------------

let unlistenProgress: (() => void) | null = null;

/** `extras`: files dropped with the model, offered for what it references. */
async function openFile(path: string, extras: string[] = []) {
  const lower = path.toLowerCase();
  const name = path.split(/[\\/]/).pop() ?? path;
  if (/\.blend\d+$/.test(lower)) return setState("error", { key: "error.backupFile", params: { name } });
  if (!MODEL_EXT.test(lower)) return setState("error", { key: "error.notBlend", params: { name } });
  const added = await gatherCompanions(path, extras);
  if (added === null) return;
  step = { key: "blender", name: "" };
  setState("converting");
  hideToast();
  unlistenProgress?.();
  unlistenProgress = await listen<Progress>("progress", (e) => {
    step = e.payload;
    bind("step", tKey(`progress.${step.key}`, { name: step.name }, step.name));
  });
  try {
    previousSwaps = null;
    measured = [];
    render(await api.openBlend(path, added));
    await reloadMesh(true);
    setState("ready");
    // Start both panels at the top once layout has settled.
    requestAnimationFrame(() => setTimeout(() => document.querySelectorAll<HTMLElement>(".side, .ams").forEach((el) => el.scrollTo({ top: 0 })), 60));
  } catch (e) {
    $<HTMLButtonElement>("#find-blender").hidden = !(isAppError(e) && e.key.startsWith("error.blender"));
    setState("error", e);
  } finally {
    unlistenProgress?.();
    unlistenProgress = null;
  }
}

$("#choose").addEventListener("click", async () => {
  const path = await open({ multiple: false, directory: false, filters: [{ name: t("import.formats"), extensions: MODEL_EXTENSIONS }] });
  if (typeof path === "string") await openFile(path);
});

$("#find-blender").addEventListener("click", async () => {
  const path = await open({ multiple: false, directory: false, title: t("tray.findBlenderTitle") });
  if (typeof path !== "string") return;
  try {
    await api.setBlenderPath(path);
    $<HTMLButtonElement>("#find-blender").hidden = true;
    setState("empty");
    toast(t("tray.blenderSet", { path }));
  } catch (e) {
    setState("error", e);
  }
});

void getCurrentWebview().onDragDropEvent((e) => {
  const p = e.payload;
  const over = p.type === "enter" || p.type === "over";
  // The model files dialog takes what is dropped while it is open.
  const into = companionsDrop.active;
  if (into) {
    $("#files-dialog").classList.toggle("is-dropping", over);
    if (p.type === "drop" && p.paths.length) into(p.paths);
    return;
  }
  if (over) app.classList.add("is-dragging");
  else app.classList.remove("is-dragging");
  if (p.type === "drop" && p.paths.length && app.dataset.state !== "converting") {
    // A model dropped with its .mtl, .bin or textures: the rest go along with it.
    const model = p.paths.find((f) => MODEL_EXT.test(f) || /\.blend\d+$/i.test(f)) ?? p.paths[0];
    void openFile(model, p.paths.filter((f) => f !== model));
  }
});

// ---- export ------------------------------------------------------------------------------

async function confirmOversize(): Promise<boolean> {
  const why = summary && fitProblem(summary);
  if (!why) return true;
  return ask(t("export.confirmOversize", { why }), {
    title: t("export.oversizeTitle"),
    kind: "warning",
    okLabel: t("export.anyway"),
    cancelLabel: t("common.cancel"),
  });
}

$("#export").addEventListener("click", async () => {
  const btn = $<HTMLButtonElement>("#export");
  if (app.dataset.state !== "ready" || !summary || btn.getAttribute("aria-busy") === "true") return;
  if (!(await confirmOversize())) return;
  const path = await save({ defaultPath: summary.fileName.replace(MODEL_EXT, ".3mf"), filters: [{ name: "3MF", extensions: ["3mf"] }] });
  if (!path) return;
  btn.setAttribute("aria-busy", "true");
  btn.setAttribute("aria-disabled", "true");
  bind("exportLabel", t("export.busy"));
  try {
    await api.export3mf(path);
    toast(t("export.saved", { path }));
  } catch (e) {
    toast(errText(e), "error");
  } finally {
    btn.removeAttribute("aria-busy");
    btn.setAttribute("aria-disabled", "false");
    bind("exportLabel", t("export.label"));
  }
});

const zipDialog = $<HTMLDialogElement>("#zip-dialog");
$("#open-zip").addEventListener("click", () => {
  if (app.dataset.state === "ready") zipDialog.showModal();
});
$("#export-zip").addEventListener("click", async () => {
  if (!summary) return;
  const shell = $<HTMLInputElement>("#shell");
  const thickness = Number(shell.value);
  const ok = thickness >= 0.2 && thickness <= 20;
  shell.setAttribute("aria-invalid", String(!ok));
  if (!ok) {
    shell.focus();
    return;
  }
  zipDialog.close();
  if (!(await confirmOversize())) return;
  const suffix = currentLang() === "en" ? "_colours.zip" : "_colores.zip";
  const path = await save({ defaultPath: summary.fileName.replace(MODEL_EXT, suffix), filters: [{ name: "ZIP", extensions: ["zip"] }] });
  if (!path) return;
  await work(async () => {
    const n = await api.exportColorZip(path, thickness, $<HTMLInputElement>("#main-solid").checked, currentLang());
    toast(t("zip.saved", { count: n, n, path }));
  });
});

// ---- settings ----------------------------------------------------------------------------

const form = $<HTMLFormElement>("#settings");
form.addEventListener("submit", (e) => e.preventDefault());
form.addEventListener("input", syncOutputs);

const meshOps = () => void apply(() => api.setMeshOps(
  Number($<HTMLInputElement>("#keep").value) / 100,
  Math.max(0, Number($<HTMLInputElement>("#flatten").value) || 0),
  $<HTMLInputElement>("#repair").checked,
  Math.max(0, Number($<HTMLInputElement>("#piece").value) || 0),
));
["#keep", "#repair", "#piece"].forEach((sel) => $(sel).addEventListener("change", meshOps));

// Flattening applies with its button or Enter, so the effect is never a surprise.
const flatten = () => {
  const el = $<HTMLInputElement>("#flatten");
  const v = Number(el.value);
  const ok = v >= 0 && v <= 50;
  el.setAttribute("aria-invalid", String(!ok));
  if (!ok) return toast(t("error.flattenRange"), "error");
  meshOps();
};
$("#flatten-apply").addEventListener("click", flatten);
$("#flatten").addEventListener("keydown", (e) => {
  if ((e as KeyboardEvent).key === "Enter") {
    e.preventDefault();
    flatten();
  }
});

const colorOps = () => void apply(() => api.setColorOps(Number($<HTMLInputElement>("#detail").value), Number($<HTMLInputElement>("#island").value)));
$("#detail").addEventListener("change", colorOps);
$("#island").addEventListener("change", colorOps);

/** Re-detecting colours throws away hand painting: ask first. */
async function confirmRepaint(): Promise<boolean> {
  if (!summary?.paintedTriangles) return true;
  return ask(t("colour.confirmRepaint", { count: summary.paintedTriangles, n: nf(summary.paintedTriangles) }), {
    title: t("colour.repaintTitle"),
    kind: "warning",
    okLabel: t("colour.repaintOk"),
    cancelLabel: t("common.cancel"),
  });
}

$("#count").addEventListener("change", async (e) => {
  const n = Number((e.target as HTMLInputElement).value);
  if (!(await confirmRepaint())) {
    if (summary) render(summary);
    return;
  }
  void apply(() => api.suggestPalette(n));
});
$("#suggest").addEventListener("click", async () => {
  if (!summary || !(await confirmRepaint())) return;
  const n = summary.slots.length;
  void apply(() => api.suggestPalette(n));
});

$("#reduce").addEventListener("click", () => {
  const input = $<HTMLInputElement>("#reduce-to");
  const n = Math.round(Number(input.value));
  const max = (summary?.slots.length ?? 1) - 1;
  const ok = summary !== null && n >= 1 && n <= max;
  input.setAttribute("aria-invalid", String(!ok));
  if (!ok) {
    toast(max < 1 ? t("colour.onlyOne") : t("colour.reduceRange", { max, now: max + 1 }), "error");
    input.focus();
    return;
  }
  void apply(() => api.reducePalette(n), () =>
    toast(t("colour.reduced", { count: n, n }), "ok", { label: t("undo.label"), run: () => void undo() }));
});

$("#detail-fix").addEventListener("click", () => {
  if (!summary) return;
  const island = $<HTMLInputElement>("#island");
  island.value = String(Math.ceil(summary.minDetailMm2));
  syncOutputs();
  colorOps();
});

// ---- printers ------------------------------------------------------------------------------

const nozzleSelect = $<HTMLSelectElement>("#nozzle");
const profileOf = (id: string) => printers?.profiles.find((p) => p.id === id);
const closest = (list: number[], n: number) => list.reduce((a, b) => (Math.abs(b - n) < Math.abs(a - n) ? b : a), list[0]);
const volumeText = (p: PrinterProfile) => `${mm(p.volumeMm[0])} × ${mm(p.volumeMm[1])} × ${mm(p.volumeMm[2])} mm`;
/** "Bambu Lab A1" reads as "A1" under its Bambu Lab heading. */
const shortName = (p: PrinterProfile) => (p.vendor && p.name.startsWith(`${p.vendor} `) ? p.name.slice(p.vendor.length + 1) : p.name);
const nozzleList = (ns: number[]) => `${list(ns.map(mm))} mm`;

/** Every printer as a picker item: the user's first, then by brand as the catalogue orders them. */
function printerItems(): PickerItem[] {
  const all = printers?.profiles ?? [];
  return [...all.filter((p) => p.custom), ...all.filter((p) => !p.custom)].map((p) => ({
    value: p.id,
    label: shortName(p),
    group: p.custom ? t("printers.mine") : p.vendor,
    detail: t("printers.detail", { volume: volumeText(p), nozzles: nozzleList(p.nozzles) }),
    keywords: p.name,
    full: p.name,
  }));
}

const printerTexts = (): PickerTexts => ({
  label: t("printers.picker"),
  placeholder: t("printers.search"),
  searchLabel: t("printers.searchLabel"),
  none: t("printers.none"),
  empty: (q) => t("printers.noMatch", { q }),
  emptyAction: { label: t("printers.createOwn"), run: (q) => openPrinterDialog(q) },
});

const printerPicker = new Picker($("#printer-picker"), {
  ...printerTexts(),
  onPick: (id) => void choosePrinter(id, Number(nozzleSelect.value)),
});

/** The bar's printer and nozzle pickers. */
function renderPrinterPick(id: string, nozzle: number) {
  if (!printers) return;
  printers.selected = id;
  printers.nozzleMm = nozzle;
  printerPicker.setItems(printerItems());
  printerPicker.setValue(id);
  const profile = profileOf(id);
  nozzleSelect.replaceChildren(...(profile?.nozzles ?? [nozzle]).map((n) => new Option(`${mm(n)} mm`, String(n))));
  nozzleSelect.value = String(nozzle);
  nozzleSelect.disabled = (profile?.nozzles.length ?? 1) < 2;
}

/** Picks a printer; the open model is prepared again for its nozzle. */
async function choosePrinter(id: string, nozzle: number) {
  const profile = profileOf(id);
  if (!profile) return;
  const n = profile.nozzles.includes(nozzle) ? nozzle : closest(profile.nozzles, nozzle);
  await work(async () => {
    previousSwaps = summary?.estimate.swaps ?? null;
    const s = await api.setPrinter(id, n);
    renderPrinterPick(id, n);
    if (s) {
      render(s);
      await reloadMesh();
    }
  });
}

nozzleSelect.addEventListener("change", () => void choosePrinter(printerPicker.value, Number(nozzleSelect.value)));

// The custom printer dialog: a list of the user's printers and one form to add or edit.
const printerDialog = $<HTMLDialogElement>("#printer-dialog");
const printerForm = $<HTMLFormElement>("#printer-form");
const printerError = $<HTMLParagraphElement>("#printer-error");
const pf = {
  name: $<HTMLInputElement>("#pf-name"),
  w: $<HTMLInputElement>("#pf-w"),
  d: $<HTMLInputElement>("#pf-d"),
  h: $<HTMLInputElement>("#pf-h"),
};
let editing = "";

const baseTexts = (): PickerTexts => ({
  label: t("printers.base"),
  placeholder: t("printers.search"),
  searchLabel: t("printers.baseSearchLabel"),
  none: t("printers.none"),
  empty: (q) => t("printers.baseNoMatch", { q }),
});

// The Bambu Studio profile a custom printer borrows; picking one fills in its volume when still empty.
const basePicker = new Picker($("#pf-base"), {
  ...baseTexts(),
  onPick: (id) => {
    const base = profileOf(id);
    if (!base) return;
    [pf.w, pf.d, pf.h].forEach((el, i) => {
      if (!el.value) el.value = String(base.volumeMm[i]);
    });
  },
});

basePicker.button.setAttribute("aria-describedby", "pf-base-help");

function baseItems(): PickerItem[] {
  const generic: PickerItem = { value: "", label: t("printers.generic"), group: "", detail: t("printers.genericDetail") };
  return [generic, ...printerItems().filter((i) => i.group !== t("printers.mine"))];
}

function renderPrinterList() {
  if (!printers) return;
  const mine = printers.profiles.filter((p) => p.custom);
  $<HTMLParagraphElement>("#printer-empty").hidden = mine.length > 0;
  const list = $<HTMLUListElement>("#printer-list");
  list.hidden = mine.length === 0;
  list.replaceChildren(...mine.map((p) => {
    const li = document.createElement("li");
    li.className = "printers__row";
    li.toggleAttribute("aria-current", p.id === editing);
    const base = p.base ? profileOf(p.base)?.name : undefined;
    li.innerHTML = `
      <div class="printers__text">
        <strong></strong>
        <span class="help"></span>
      </div>
      <button class="btn btn--quiet btn--sm" type="button" data-act="edit">${t("printers.edit")}</button>
      <button class="btn btn--quiet btn--sm" type="button" data-act="delete">${t("printers.delete")}</button>`;
    li.querySelector("strong")!.textContent = p.name;
    li.querySelector(".help")!.textContent = `${t("printers.detail", { volume: volumeText(p), nozzles: nozzleList(p.nozzles) })}. ${base ? t("printers.profileOf", { base }) : t("printers.genericShort")}`;
    li.querySelector('[data-act="edit"]')!.addEventListener("click", () => fillPrinterForm(p));
    li.querySelector('[data-act="delete"]')!.addEventListener("click", () => void deletePrinter(p));
    return li;
  }));
}

function printerFormTitle() {
  const p = printers?.profiles.find((x) => x.id === editing);
  $("#h-printer-form").textContent = p ? t("printers.editTitle", { name: p.name }) : t("printers.newTitle");
}

function fillPrinterForm(p: PrinterProfile | null) {
  editing = p?.id ?? "";
  printerFormTitle();
  $<HTMLButtonElement>("#printer-new").hidden = !p;
  pf.name.value = p?.name ?? "";
  [pf.w.value, pf.d.value, pf.h.value] = (p?.volumeMm ?? ["", "", ""]).map(String);
  basePicker.setValue(p?.base ?? "");
  const nozzles = p?.nozzles ?? [0.4];
  document.querySelectorAll<HTMLInputElement>("#pf-nozzles input").forEach((i) => (i.checked = nozzles.includes(Number(i.value))));
  for (const el of [pf.name, pf.w, pf.d, pf.h]) el.removeAttribute("aria-invalid");
  printerError.hidden = true;
  renderPrinterList();
}

/** Opens the custom printer dialog, the name filled in when it comes from a search. */
function openPrinterDialog(name = "") {
  if (!printers) return;
  basePicker.setItems(baseItems());
  $("#pf-nozzles").replaceChildren(
    Object.assign(document.createElement("legend"), { textContent: t("printers.nozzles") }),
    ...printers.nozzles.flatMap((n) => {
      const input = Object.assign(document.createElement("input"), { type: "checkbox", id: `pf-n${n}`, value: String(n), name: "pf-nozzle" });
      const label = Object.assign(document.createElement("label"), { htmlFor: input.id, textContent: mm(n) });
      return [input, label];
    }),
  );
  fillPrinterForm(null);
  pf.name.value = name;
  printerDialog.showModal();
  pf.name.focus();
}

function printerFormError(message: string, field?: HTMLElement) {
  printerError.textContent = message;
  printerError.hidden = false;
  field?.setAttribute("aria-invalid", "true");
  field?.focus();
}

printerForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const saveBtn = $<HTMLButtonElement>("#printer-save");
  if (saveBtn.getAttribute("aria-busy") === "true") return;
  for (const el of [pf.name, pf.w, pf.d, pf.h]) el.removeAttribute("aria-invalid");
  printerError.hidden = true;
  const name = pf.name.value.trim();
  if (!name) return printerFormError(t("printers.needName"), pf.name);
  const volume = [pf.w, pf.d, pf.h].map((el) => Number(el.value));
  const bad = [pf.w, pf.d, pf.h].find((el, i) => !el.value || !(volume[i] >= 20 && volume[i] <= 2000));
  if (bad) return printerFormError(t("error.printerVolume"), bad);
  const nozzles = [...document.querySelectorAll<HTMLInputElement>("#pf-nozzles input:checked")].map((i) => Number(i.value));
  if (!nozzles.length) return printerFormError(t("printers.needNozzle"), document.querySelector<HTMLInputElement>("#pf-nozzles input") ?? undefined);
  const profile: PrinterProfile = { id: editing, name, volumeMm: volume as [number, number, number], nozzles, base: basePicker.value || null, vendor: "", custom: true };
  saveBtn.setAttribute("aria-busy", "true");
  saveBtn.textContent = t("printers.saving");
  try {
    const nozzle = closest(nozzles, printers?.nozzleMm ?? 0.4);
    printers = await api.savePrinter(profile, nozzle);
    previousSwaps = summary?.estimate.swaps ?? null;
    if (summary) {
      // The open model now follows the saved printer.
      const s = await api.setPrinter(printers.selected, printers.nozzleMm);
      if (s) {
        render(s);
        await reloadMesh();
      }
    }
    renderPrinterPick(printers.selected, printers.nozzleMm);
    const saved = printers.profiles.find((p) => p.id === printers!.selected) ?? null;
    fillPrinterForm(saved);
    toast(t("printers.saved", { name }));
  } catch (err) {
    printerFormError(errText(err));
  } finally {
    saveBtn.removeAttribute("aria-busy");
    saveBtn.textContent = t("printers.save");
  }
});

async function deletePrinter(p: PrinterProfile) {
  const ok = await ask(t("printers.confirmDelete", { name: p.name }), {
    title: t("printers.deleteTitle"),
    kind: "warning",
    okLabel: t("printers.delete"),
    cancelLabel: t("common.cancel"),
  });
  if (!ok) return;
  try {
    const wasSelected = printers?.selected === p.id;
    printers = await api.deletePrinter(p.id);
    if (wasSelected && summary) {
      const s = await api.setPrinter(printers.selected, printers.nozzleMm);
      if (s) {
        render(s);
        await reloadMesh();
      }
    }
    renderPrinterPick(printers.selected, printers.nozzleMm);
    if (editing === p.id) fillPrinterForm(null);
    else renderPrinterList();
    toast(t("printers.deleted", { name: p.name }));
  } catch (err) {
    printerFormError(errText(err));
  }
}

$("#printer-new").addEventListener("click", () => {
  fillPrinterForm(null);
  pf.name.focus();
});
$("#printer-close").addEventListener("click", () => printerDialog.close());

// Shortcuts from the filament-change summary to the controls that lower it.
function reveal(groupSel: string, focusSel: string) {
  const group = $<HTMLDetailsElement>(groupSel);
  group.open = true;
  const el = $<HTMLElement>(focusSel);
  el.scrollIntoView({ block: "center", behavior: "smooth" });
  el.focus({ preventScroll: true });
}
$("#go-reduce").addEventListener("click", () => reveal("#color-group", "#reduce-to"));
$("#go-islands").addEventListener("click", () => reveal("#color-group", "#island"));

// ---- view, tool, undo ------------------------------------------------------------------

function setView(v: View) {
  app.dataset.view = v;
  document.querySelectorAll<HTMLButtonElement>("button[data-view]").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.view === v)));
  viewer.setView(v);
}

function setTool(tool: Tool) {
  app.dataset.tool = tool;
  document.querySelectorAll<HTMLButtonElement>("button[data-tool]").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.tool === tool)));
  radiusInput.disabled = tool !== "brush";
  viewer.setTool(tool);
  setHint(tool === "size" ? t("hint.size", { lock: locked ? t("size.locked") : t("size.unlocked") }) : t(`hint.${tool}`));
  syncMeasure();
}

// The plate in the view, remembered between sessions.
const bedBtn = $<HTMLButtonElement>("#toggle-bed");
const BED_KEY = "blendparser.bed";
let bedOn = true;
try {
  bedOn = localStorage.getItem(BED_KEY) !== "0";
} catch {
  // Storage blocked: the plate shows.
}
function setBed(on: boolean) {
  bedOn = on;
  bedBtn.setAttribute("aria-pressed", String(on));
  viewer.setBedVisible(on);
  try {
    localStorage.setItem(BED_KEY, on ? "1" : "0");
  } catch {
    // Not remembered; it still applies now.
  }
}
bedBtn.addEventListener("click", () => setBed(!bedOn));
setBed(bedOn);

// The tool shelf shows icons; expanded, also their names. Remembered too.
const toolbar = $<HTMLDivElement>("#toolbar");
const expandBtn = $<HTMLButtonElement>("#toolbar-expand");
const SHELF_KEY = "blendparser.toolNames";
function setExpanded(on: boolean) {
  toolbar.dataset.expanded = String(on);
  expandBtn.setAttribute("aria-expanded", String(on));
  const label = on ? t("stage.collapse") : t("stage.expand");
  bind("toolbarExpand", label);
  expandBtn.title = label;
  expandBtn.setAttribute("aria-label", label);
  try {
    localStorage.setItem(SHELF_KEY, on ? "1" : "0");
  } catch {
    // Not remembered.
  }
}
expandBtn.addEventListener("click", () => setExpanded(toolbar.dataset.expanded !== "true"));
let shelfOpen = false;
try {
  shelfOpen = localStorage.getItem(SHELF_KEY) === "1";
} catch {
  // Collapsed by default.
}
setExpanded(shelfOpen);

document.querySelectorAll<HTMLButtonElement>("button[data-view]").forEach((b) => b.addEventListener("click", () => setView(b.dataset.view as View)));
document.querySelectorAll<HTMLButtonElement>("button[data-tool]").forEach((b) => b.addEventListener("click", () => setTool(b.dataset.tool as Tool)));

async function undo() {
  if (!summary?.canUndo) return;
  const what = summary.undoLabel ? t(`undo.${summary.undoLabel}`) : t("undo.last");
  await apply(() => api.undo(), () => toast(t("undo.done", { what })));
}
$("#undo").addEventListener("click", () => void undo());

window.addEventListener("keydown", (e) => {
  const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement || e.target instanceof HTMLTextAreaElement;
  if (typing || app.dataset.state !== "ready" || document.querySelector("dialog[open]")) return;
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z") {
    e.preventDefault();
    void undo();
  } else if (e.key === "b") setTool("brush");
  else if (e.key === "o") setTool("orbit");
  else if (e.key === "m") setTool("measure");
  else if (e.key === "s") setTool("size");
  else if (e.key === "p") setBed(!bedOn);
  else if (e.key === "l") toggleLock();
  else if (e.key === "f") viewer.frameAll();
  else if (e.key === "Escape" && measured.length) {
    measured = [];
    syncMeasure();
  } else if (e.key === "[" || e.key === "]") {
    radiusInput.stepUp(e.key === "]" ? 1 : -1);
    syncBrush();
  }
});

// ---- updates -----------------------------------------------------------------------------

const versionBtn = $<HTMLButtonElement>("#version");
let pendingUpdate: Update | null = null;

/** Looks for a newer release. Quiet mode only speaks up when there is one. */
async function checkForUpdates(quiet: boolean) {
  versionBtn.disabled = true;
  if (!quiet) bind("version", t("update.searching"));
  try {
    pendingUpdate = await findUpdate();
    versionBtn.classList.toggle("has-update", pendingUpdate !== null);
    if (pendingUpdate) {
      const u = pendingUpdate;
      toast(t("update.available", { v: u.version }), "ok", { label: t("update.install", { v: u.version }), run: () => void applyUpdate(u) }, true);
    } else if (!quiet) {
      toast(t("update.latest", { v: await currentVersion() }));
    }
  } catch (e) {
    if (!quiet) toast(t("update.failed", { detail: String(e) }), "error");
  } finally {
    versionBtn.disabled = false;
    bind("version", `v${await currentVersion()}`);
  }
}

async function applyUpdate(u: Update) {
  if (app.dataset.state === "ready") {
    const ok = await ask(t("update.confirm", { v: u.version }), {
      title: t("update.title"),
      kind: "info",
      okLabel: t("update.restart"),
      cancelLabel: t("update.later"),
    });
    if (!ok) return;
  }
  versionBtn.disabled = true;
  toast(t("update.downloading", { v: u.version }), "ok", undefined, true);
  try {
    await installUpdate(u, (f) =>
      bind("toast", f === null ? t("update.downloading", { v: u.version }) : t("update.progress", { v: u.version, pct: Math.round(f * 100) })));
  } catch (e) {
    versionBtn.disabled = false;
    toast(t("update.installFailed", { detail: String(e) }), "error");
  }
}

versionBtn.addEventListener("click", () => {
  if (pendingUpdate) void applyUpdate(pendingUpdate);
  else void checkForUpdates(false);
});

// ---- language ----------------------------------------------------------------------------

const langSelect = $<HTMLSelectElement>("#lang");
langSelect.replaceChildren(...LANGS.map((l) => Object.assign(new Option(l.name, l.code), { lang: l.code })));
langSelect.value = currentLang();
langSelect.addEventListener("change", () => setLang(langSelect.value as Lang));
const prefsDialog = $<HTMLDialogElement>("#prefs-dialog");
function openPrefs() {
  if (document.querySelector("dialog[open]")) return;
  langSelect.value = currentLang();
  prefsDialog.showModal();
}

// ---- menu bar ----------------------------------------------------------------------------

const ready = () => app.dataset.state === "ready";
const COMMANDS: Record<string, { run: () => void; enabled?: () => boolean }> = {
  open: { run: () => $("#choose").click(), enabled: () => state !== "converting" },
  export: { run: () => $("#export").click(), enabled: ready },
  printers: { run: () => openPrinterDialog() },
  undo: { run: () => void undo(), enabled: () => ready() && Boolean(summary?.canUndo) },
  undoSize: { run: () => $("#size-undo").click(), enabled: () => ready() && Boolean(summary?.size.canUndo) },
  waste: { run: () => $("#open-waste").click(), enabled: ready },
  prefs: { run: openPrefs },
  updates: { run: () => versionBtn.click() },
  shortcuts: { run: () => toast(t("menu.shortcutList"), "ok", undefined, true) },
};
menubar($("#menubar"), {
  run: (cmd) => COMMANDS[cmd]?.run(),
  enabled: (cmd) => COMMANDS[cmd]?.enabled?.() ?? true,
});

// Menu shortcuts work in any state and outside text fields.
window.addEventListener("keydown", (e) => {
  if (!(e.ctrlKey || e.metaKey) || e.altKey || document.querySelector("dialog[open]")) return;
  const cmd = { o: "open", s: "export", ",": "prefs" }[e.key.toLowerCase()];
  if (!cmd || !(COMMANDS[cmd].enabled?.() ?? true)) return;
  e.preventDefault();
  COMMANDS[cmd].run();
});

/** Everything drawn from code, worded again. */
onLangChange(() => {
  langSelect.value = currentLang();
  printerPicker.relabel(printerTexts());
  basePicker.relabel(baseTexts());
  if (printers) renderPrinterPick(printers.selected, printers.nozzleMm);
  if (printerDialog.open) {
    basePicker.setItems(baseItems());
    printerFormTitle();
    renderPrinterList();
    $("#pf-nozzles legend").textContent = t("printers.nozzles");
  }
  if (wasteDialog.open) {
    const keep = readWasteForm();
    fillMaterials();
    fillWasteForm(keep);
  }
  setState(state);
  setTool((app.dataset.tool as Tool) ?? "orbit");
  setExpanded(toolbar.dataset.expanded === "true");
  syncLock();
  syncOutputs();
  bind("shellHelp", t("zip.thicknessHelp", { min: mm(0.2), max: mm(20) }));
  if (summary) render(summary);
  syncBrush();
  if (toastEl.dataset.kind !== "error") hideToast();
});

// A notice shown inside a dialog goes back home when the dialog closes, still showing.
document.querySelectorAll("dialog").forEach((d) => d.addEventListener("close", () => {
  if (toastEl.parentElement !== d) return;
  const open = toastEl.matches(":popover-open");
  toastHome.append(toastEl);
  if (open) toastEl.showPopover();
}));

// A click outside a dialog's box closes it, like Esc. The press has to start
// outside too, so dragging a selection out of a field does not close it.
document.querySelectorAll("dialog").forEach((d) => {
  let fromOutside = false;
  const outside = (e: MouseEvent) => {
    const r = d.getBoundingClientRect();
    return e.target === d && (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom);
  };
  d.addEventListener("pointerdown", (e) => (fromOutside = outside(e)));
  d.addEventListener("click", (e) => {
    if (fromOutside && outside(e)) d.close();
    fromOutside = false;
  });
});

// ---- start -------------------------------------------------------------------------------

applyStatic();
bind("shellHelp", t("zip.thicknessHelp", { min: mm(0.2), max: mm(20) }));
syncLock();
setState("empty");
setTool("orbit");
void api.printers().then((p) => {
  printers = p;
  renderPrinterPick(p.selected, p.nozzleMm);
});
void api.prefs().then((p) => (prefs = p));
void currentVersion().then((v) => bind("version", `v${v}`));
// Check quietly once the window has settled; only a newer version is announced.
window.setTimeout(() => void checkForUpdates(true), 4000);
syncOutputs();
syncBrush();
void api.blenderStatus().then(async (path) => {
  if (!path) {
    $<HTMLButtonElement>("#find-blender").hidden = false;
    setState("error", { key: "error.blenderMissing", params: {} });
    return;
  }
  const file = await api.initialFile();
  if (file) await openFile(file);
});
