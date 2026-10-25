// Wires the window: file in (drop or dialog), settings out to Rust, the
// summary back into the panels, and the preview in sync with the painting.
import "@fontsource/schibsted-grotesk/700.css";
import "@fontsource/instrument-sans/400.css";
import "@fontsource/instrument-sans/500.css";
import "@fontsource/dm-mono/400.css";
import "@fontsource/dm-mono/500.css";
import "./styles/app.css";

import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { ask, open, save } from "@tauri-apps/plugin-dialog";
import { api, type PrinterProfile, type Printers, type Summary } from "./api";
import { currentVersion, findUpdate, installUpdate, type Update } from "./updater";
import { Viewer, type Tool, type View } from "./viewer";

type State = "empty" | "converting" | "error" | "ready";

const app = document.querySelector<HTMLElement>("[data-app]")!;
const $ = <T extends HTMLElement>(sel: string) => document.querySelector<T>(sel)!;
const live = $<HTMLParagraphElement>("#live");
const nf = (n: number, d = 0) => n.toLocaleString("es", { maximumFractionDigits: d, minimumFractionDigits: d });
/** Millimetres as a printer spec reads them: 0,4, 0,25, 256. */
const mm = (n: number) => n.toLocaleString("es", { maximumFractionDigits: 2 });

let summary: Summary | null = null;
let printers: Printers | null = null;
/** Swaps before the last change, to show what a setting did. */
let previousSwaps: number | null = null;
let brushSlot = 0;

function bind(key: string, text: string) {
  document.querySelectorAll(`[data-bind="${key}"]`).forEach((el) => (el.textContent = text));
}

function setState(s: State, detail?: string) {
  app.dataset.state = s;
  if (detail) bind(s === "error" ? "error" : "step", detail);
  $<HTMLButtonElement>("#choose").disabled = s === "converting";
  for (const id of ["#export", "#open-zip"]) $<HTMLButtonElement>(id).setAttribute("aria-disabled", String(s !== "ready"));
  live.textContent = { empty: "Sin archivo", converting: "Convirtiendo", error: "Error", ready: "Listo para exportar" }[s];
}

// ---- toast ------------------------------------------------------------------------

const toastEl = $<HTMLDivElement>("#toast");
const toastAction = $<HTMLButtonElement>("#toast-action");
let toastTimer = 0;
let onToastAction: (() => void) | null = null;

/** A visible notice on the stage. Errors stay until closed; the rest fade after 8 s. */
function toast(message: string, kind: "ok" | "error" = "ok", action?: { label: string; run: () => void }, sticky = false) {
  window.clearTimeout(toastTimer);
  bind("toast", message);
  toastEl.dataset.kind = kind;
  toastEl.setAttribute("role", kind === "error" ? "alert" : "status");
  toastEl.hidden = false;
  onToastAction = action?.run ?? null;
  toastAction.hidden = !action;
  if (action) toastAction.textContent = action.label;
  if (kind !== "error" && !sticky) toastTimer = window.setTimeout(() => (toastEl.hidden = true), 8000);
}
$("#toast-close").addEventListener("click", () => (toastEl.hidden = true));
toastAction.addEventListener("click", () => {
  toastEl.hidden = true;
  onToastAction?.();
});

/** Runs a backend change with the stage marked as working; errors become a toast. */
async function work<T>(fn: () => Promise<T>): Promise<T | undefined> {
  app.classList.add("is-working");
  try {
    return await fn();
  } catch (e) {
    toast(String(e), "error");
    return undefined;
  } finally {
    app.classList.remove("is-working");
  }
}

/** A change that repaints: apply, render, refresh the mesh. */
const apply = (call: () => Promise<Summary>, done?: () => void) =>
  work(async () => {
    previousSwaps = summary?.estimate.swaps ?? null;
    render(await call());
    await reloadMesh();
    done?.();
  });

// ---- viewer ------------------------------------------------------------------------

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
});

const radiusInput = $<HTMLInputElement>("#radius");
const radiusMm = () => Number(radiusInput.value);
function syncBrush() {
  bind("radiusOut", `${nf(radiusMm(), 1)} mm`);
  const scale = summary && viewer.modelHeight() > 0 ? summary.sizeMm[2] / viewer.modelHeight() : 1;
  viewer.brushRadius = radiusMm() / scale;
  const hex = summary?.slots[brushSlot]?.hex;
  if (hex) {
    app.style.setProperty("--brush", hex);
    viewer.setBrushColor(hex);
  }
  bind("brushSlot", `Ranura ${brushSlot + 1}`);
}
radiusInput.addEventListener("input", syncBrush);

async function reloadMesh(frame = false) {
  viewer.load(await api.previewMesh(), frame);
  syncBrush();
}

// ---- rendering the summary ------------------------------------------------------------

function render(s: Summary) {
  summary = s;
  bind("fileName", s.fileName);
  bind("fileMeta", `${nf(s.fileSize / 1048576, 1)} MB, ${s.materials.length} mat.`);

  // Size and plate.
  const [x, y, z] = s.sizeMm;
  bind("dims", `${nf(x, 1)} × ${nf(y, 1)} × ${nf(z, 1)} mm`);
  const plate = `${nf(s.bedMm[0])} × ${nf(s.bedMm[1])} mm`;
  const fits = s.fitsBed && s.fitsHeight;
  bind("bedFit", !s.fitsBed
    ? `No cabe en la placa de ${plate}.`
    : !s.fitsHeight
      ? `Mide ${nf(z)} mm de alto y la impresora llega a ${nf(s.maxHeightMm)} mm.`
      : `Cabe en la placa de ${plate} y en sus ${nf(s.maxHeightMm)} mm de alto.`);
  bind("fitBanner", !s.fitsBed ? `No cabe en la placa: mide ${nf(x)} × ${nf(y)} mm.` : `Es demasiado alto: mide ${nf(z)} mm y la impresora llega a ${nf(s.maxHeightMm)} mm.`);
  $<HTMLDivElement>("#fit-banner").hidden = fits;
  app.classList.toggle("is-oversize", !fits);
  bind("exportLabel", "Exportar .3mf");

  // Settings.
  const st = s.settings;
  setIfIdle("#height", nf(st.heightMm, 1).replace(",", ".").replace(/\s/g, ""));
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
  $<HTMLDivElement>("#detail-note").hidden = s.smallIslands === 0;
  bind("detailNote", `${nf(s.smallIslands)} ${s.smallIslands === 1 ? "mancha de color mide" : "manchas de color miden"} menos de ${mm(s.minDetailMm2)} mm². Con boquilla de ${mm(st.nozzleMm)} mm salen borrosas o no salen.`);
  bind("detailFix", `Limpiar islas menores de ${fixTo} mm²`);
  document.querySelectorAll<HTMLInputElement>("#count input").forEach((i) => (i.checked = Number(i.value) === s.slots.length));

  // Mesh.
  bind("triangles", s.triangles === s.sourceTriangles
    ? `${nf(s.triangles)} triángulos, ${nf(s.splitTriangles)} subdivididos en bordes`
    : `${nf(s.triangles)} de ${nf(s.sourceTriangles)} triángulos`);
  bind("contact", st.flattenMm > 0
    ? `Base aplanada ${nf(st.flattenMm, 1)} mm. Contacto con la placa: ${nf(s.contactMm2)} mm².`
    : `Contacto con la placa: ${nf(s.contactMm2)} mm². Aplana 0,5 a 1 mm si la base no es plana.`);
  const r = s.repair;
  const fixes = [
    r.weldedVertices && `${nf(r.weldedVertices)} vértices unidos`,
    r.degenerateTriangles && `${nf(r.degenerateTriangles)} triángulos degenerados`,
    r.duplicateTriangles && `${nf(r.duplicateTriangles)} caras repetidas`,
    r.loosePieces && `${nf(r.loosePieces)} piezas sueltas (${nf(r.looseTriangles)} triángulos)`,
  ].filter(Boolean);
  bind("repairOut", !st.repair && !r.loosePieces ? "Sin reparar." : fixes.length ? `Corregido: ${fixes.join(", ")}.` : "La malla ya estaba limpia.");
  bind("meshSum", `${nf(s.triangles)} triángulos`);
  bind("colorSum", `${s.slots.length} colores`);

  // Filament changes, with what the last change did.
  bind("swaps", nf(s.estimate.swaps));
  const delta = previousSwaps === null ? 0 : s.estimate.swaps - previousSwaps;
  const deltaEl = document.querySelector<HTMLElement>('[data-bind="swapsDelta"]')!;
  deltaEl.textContent = delta === 0 ? "" : `(${delta < 0 ? "−" : "+"}${nf(Math.abs(delta))} con el último ajuste)`;
  deltaEl.classList.toggle("is-better", delta < 0);
  bind("swapsDetail", `${nf(s.estimate.purgeGrams)} g de purga; ${nf(s.estimate.multicolorLayers)} de ${nf(s.estimate.layers)} capas llevan más de un color. Estimado a ${mm(st.layerMm)} mm por capa.`);

  renderSlots(s);
  const mats = $<HTMLDListElement>("#materials");
  mats.replaceChildren(...s.materials.flatMap((m) => {
    const dt = document.createElement("dt");
    dt.textContent = m.name;
    const dd = document.createElement("dd");
    dd.textContent = `${m.detail}, ${nf(m.faces)} caras`;
    return [dt, dd];
  }));

  const undoBtn = $<HTMLButtonElement>("#undo");
  undoBtn.disabled = !s.canUndo;
  undoBtn.title = s.undoLabel ? `Deshacer ${s.undoLabel} (Ctrl+Z)` : "Nada que deshacer";
  viewer.setPalette(s.slots.map((x) => x.hex));
  syncBrush();
}

/** Only overwrite a field the user is not editing. */
function setIfIdle(sel: string, value: string) {
  const el = $<HTMLInputElement>(sel);
  if (document.activeElement !== el) el.value = value;
}

function syncOutputs() {
  bind("keepOut", `${$<HTMLInputElement>("#keep").value} %`);
  const d = Number($<HTMLInputElement>("#detail").value);
  bind("detailOut", d === 0 ? "sin subdividir" : `nivel ${d}`);
  const i = Number($<HTMLInputElement>("#island").value);
  bind("islandOut", i === 0 ? "apagado" : `${i} mm²`);
}

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
    li.innerHTML = `
      <label class="slot__edit" title="Cambiar el color de la ranura ${i + 1}"><span class="visually-hidden">Color de la ranura ${i + 1}</span><input type="color" value="${slot.hex.toLowerCase()}"></label>
      <button class="slot__pick" type="button" aria-pressed="${i === brushSlot}" title="Pintar con la ranura ${i + 1}">
        <span class="slot__no"><span>Ranura ${i + 1}</span><span class="slot__brush">${i === brushSlot ? "Pincel" : "Pintar con esta"}</span></span>
        <strong class="mono">${slot.hex}</strong>
        <span class="slot__count">${nf(slot.triangles)} triángulos</span>
        <span class="slot__near${close ? " is-close" : ""}">${near ? `Parecida a R${near[0] + 1} (ΔE ${nf(near[1])})` : "Única ranura"}</span>
      </button>
      <div class="slot__actions">${s.slots.length < 2 ? "" : `
        ${close ? `<button class="btn btn--quiet btn--sm slot__join" type="button">Unir con R${near![0] + 1}</button>` : ""}
        <label class="visually-hidden" for="merge-${i}">Unir la ranura ${i + 1} a otra</label>
        <select class="slot__merge" id="merge-${i}">
          <option value="">Unir a…</option>
          ${s.slots.map((_, j) => (j === i ? "" : `<option value="${j}">Ranura ${j + 1}</option>`)).join("")}
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
        b.querySelector(".slot__brush")!.textContent = j === i ? "Pincel" : "Pintar con esta";
      });
      setTool("brush");
      syncBrush();
    });
    const merge = (into: number) =>
      void apply(() => api.mergeSlots(i, into), () =>
        toast(`Ranura ${i + 1} unida a la ranura ${into + 1}.`, "ok", { label: "Deshacer", run: () => void undo() }));
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

async function openFile(path: string) {
  const lower = path.toLowerCase();
  const name = path.split(/[\\/]/).pop() ?? path;
  if (/\.blend\d+$/.test(lower)) return setState("error", `${name} es una copia de seguridad de Blender. Ábrela y guárdala como .blend para convertirla.`);
  if (!lower.endsWith(".blend")) return setState("error", `${name} no es un archivo .blend. BlendParser necesita el .blend para leer sus materiales y texturas.`);
  setState("converting", "Abriendo Blender");
  toastEl.hidden = true;
  unlistenProgress?.();
  unlistenProgress = await listen<string>("progress", (e) => bind("step", e.payload));
  try {
    previousSwaps = null;
    render(await api.openBlend(path));
    await reloadMesh(true);
    setState("ready");
    // Start both panels at the top once layout has settled.
    requestAnimationFrame(() => setTimeout(() => document.querySelectorAll<HTMLElement>(".side, .ams").forEach((el) => el.scrollTo({ top: 0 })), 60));
  } catch (e) {
    const msg = String(e);
    $<HTMLButtonElement>("#find-blender").hidden = !msg.includes("Blender");
    setState("error", msg);
  } finally {
    unlistenProgress?.();
    unlistenProgress = null;
  }
}

$("#choose").addEventListener("click", async () => {
  const path = await open({ multiple: false, directory: false, filters: [{ name: "Blender", extensions: ["blend"] }] });
  if (typeof path === "string") await openFile(path);
});

$("#find-blender").addEventListener("click", async () => {
  const path = await open({ multiple: false, directory: false, title: "Busca blender.exe" });
  if (typeof path !== "string") return;
  try {
    await api.setBlenderPath(path);
    $<HTMLButtonElement>("#find-blender").hidden = true;
    setState("empty");
    toast(`Usaré Blender desde ${path}.`);
  } catch (e) {
    setState("error", String(e));
  }
});

void getCurrentWebview().onDragDropEvent((e) => {
  const p = e.payload;
  if (p.type === "enter" || p.type === "over") app.classList.add("is-dragging");
  else app.classList.remove("is-dragging");
  if (p.type === "drop" && p.paths.length && app.dataset.state !== "converting") void openFile(p.paths[0]);
});

// ---- export ------------------------------------------------------------------------------

async function confirmOversize(): Promise<boolean> {
  if (!summary || (summary.fitsBed && summary.fitsHeight)) return true;
  const [x, y, z] = summary.sizeMm;
  const why = !summary.fitsBed
    ? `El modelo mide ${nf(x)} × ${nf(y)} mm y no cabe en la placa de ${nf(summary.bedMm[0])} × ${nf(summary.bedMm[1])} mm.`
    : `El modelo mide ${nf(z)} mm de alto y la impresora llega a ${nf(summary.maxHeightMm)} mm.`;
  return ask(`${why} ¿Exportar de todos modos?`, {
    title: "No cabe en la impresora",
    kind: "warning",
    okLabel: "Exportar igual",
    cancelLabel: "Cancelar",
  });
}

$("#export").addEventListener("click", async () => {
  const btn = $<HTMLButtonElement>("#export");
  if (app.dataset.state !== "ready" || !summary || btn.getAttribute("aria-busy") === "true") return;
  if (!(await confirmOversize())) return;
  const path = await save({ defaultPath: summary.fileName.replace(/\.blend$/i, ".3mf"), filters: [{ name: "3MF", extensions: ["3mf"] }] });
  if (!path) return;
  btn.setAttribute("aria-busy", "true");
  btn.setAttribute("aria-disabled", "true");
  bind("exportLabel", "Exportando…");
  try {
    await api.export3mf(path);
    toast(`3MF guardado en ${path}`);
  } catch (e) {
    toast(String(e), "error");
  } finally {
    btn.removeAttribute("aria-busy");
    btn.setAttribute("aria-disabled", "false");
    bind("exportLabel", "Exportar .3mf");
  }
});

const zipDialog = $<HTMLDialogElement>("#zip-dialog");
$("#open-zip").addEventListener("click", () => {
  if (app.dataset.state === "ready") zipDialog.showModal();
});
$("#export-zip").addEventListener("click", async () => {
  if (!summary) return;
  const shell = $<HTMLInputElement>("#shell");
  const mm = Number(shell.value);
  const ok = mm >= 0.2 && mm <= 20;
  shell.setAttribute("aria-invalid", String(!ok));
  if (!ok) {
    shell.focus();
    return;
  }
  zipDialog.close();
  if (!(await confirmOversize())) return;
  const path = await save({ defaultPath: summary.fileName.replace(/\.blend$/i, "_colores.zip"), filters: [{ name: "ZIP", extensions: ["zip"] }] });
  if (!path) return;
  await work(async () => {
    const n = await api.exportColorZip(path, mm, $<HTMLInputElement>("#main-solid").checked);
    toast(`${n} colores guardados en ${path}`);
  });
});

// ---- settings ----------------------------------------------------------------------------

const form = $<HTMLFormElement>("#settings");
form.addEventListener("submit", (e) => e.preventDefault());
form.addEventListener("input", syncOutputs);

$("#height").addEventListener("change", (e) => {
  const el = e.target as HTMLInputElement;
  const mm = Number(el.value);
  const ok = mm >= 1 && mm <= 1000;
  el.setAttribute("aria-invalid", String(!ok));
  if (ok) void apply(() => api.setHeight(mm));
  else toast("La altura debe estar entre 1 y 1000 mm.", "error");
});

// Largest height whose footprint fits 90% of the plate and that the printer reaches.
$("#fit").addEventListener("click", () => {
  if (!summary) return;
  const [x, y] = summary.sizeMm;
  const k = Math.min((summary.bedMm[0] * 0.9) / x, (summary.bedMm[1] * 0.9) / y);
  const h = Math.floor(Math.min(summary.settings.heightMm * k, summary.maxHeightMm) * 10) / 10;
  void apply(() => api.setHeight(h), () => toast(`Altura ajustada a ${nf(h, 1)} mm para que quepa en la impresora.`));
});

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
  const mm = Number(el.value);
  const ok = mm >= 0 && mm <= 50;
  el.setAttribute("aria-invalid", String(!ok));
  if (!ok) return toast("Aplanar base: escribe entre 0 y 50 mm (0 lo desactiva).", "error");
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
  return ask(`Volver a detectar los colores borra lo que pintaste a mano (${nf(summary.paintedTriangles)} triángulos). Para conservarlo, usa «Unir los más parecidos».`, {
    title: "Se perderá lo pintado",
    kind: "warning",
    okLabel: "Detectar de nuevo",
    cancelLabel: "Cancelar",
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
    toast(max < 1 ? "Solo queda un color: no hay nada que unir." : `Escribe un número entre 1 y ${max}: ahora hay ${max + 1} colores.`, "error");
    input.focus();
    return;
  }
  void apply(() => api.reducePalette(n), () =>
    toast(`Colores unidos: ahora hay ${n}.`, "ok", { label: "Deshacer", run: () => void undo() }));
});

$("#detail-fix").addEventListener("click", () => {
  if (!summary) return;
  const island = $<HTMLInputElement>("#island");
  island.value = String(Math.ceil(summary.minDetailMm2));
  syncOutputs();
  colorOps();
});

// ---- printers ------------------------------------------------------------------------------

const printerSelect = $<HTMLSelectElement>("#printer");
const nozzleSelect = $<HTMLSelectElement>("#nozzle");
const profileOf = (id: string) => printers?.profiles.find((p) => p.id === id);
const closest = (list: number[], n: number) => list.reduce((a, b) => (Math.abs(b - n) < Math.abs(a - n) ? b : a), list[0]);

/** The bar's printer and nozzle pickers, grouped as Bambu Studio and "mine". */
function renderPrinterPick(id: string, nozzle: number) {
  if (!printers) return;
  printers.selected = id;
  printers.nozzleMm = nozzle;
  const option = (p: PrinterProfile) => new Option(p.custom ? p.name : p.name.replace(/^Bambu Lab /, ""), p.id);
  const bambu = document.createElement("optgroup");
  bambu.label = "Bambu Lab";
  bambu.append(...printers.profiles.filter((p) => !p.custom).map(option));
  const mine = printers.profiles.filter((p) => p.custom);
  const own = document.createElement("optgroup");
  own.label = "Mis impresoras";
  own.append(...mine.map(option));
  printerSelect.replaceChildren(bambu, ...(mine.length ? [own] : []));
  printerSelect.value = id;
  const profile = profileOf(id);
  nozzleSelect.replaceChildren(...(profile?.nozzles ?? [nozzle]).map((n) => new Option(`${mm(n)} mm`, String(n))));
  nozzleSelect.value = String(nozzle);
  nozzleSelect.disabled = (profile?.nozzles.length ?? 1) < 2;
  printerSelect.title = profile ? `${profile.name}: ${mm(profile.volumeMm[0])} × ${mm(profile.volumeMm[1])} × ${mm(profile.volumeMm[2])} mm` : "";
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

printerSelect.addEventListener("change", () => void choosePrinter(printerSelect.value, Number(nozzleSelect.value)));
nozzleSelect.addEventListener("change", () => void choosePrinter(printerSelect.value, Number(nozzleSelect.value)));

// The custom printer dialog: a list of the user's printers and one form to add or edit.
const printerDialog = $<HTMLDialogElement>("#printer-dialog");
const printerForm = $<HTMLFormElement>("#printer-form");
const printerError = $<HTMLParagraphElement>("#printer-error");
const pf = {
  name: $<HTMLInputElement>("#pf-name"),
  w: $<HTMLInputElement>("#pf-w"),
  d: $<HTMLInputElement>("#pf-d"),
  h: $<HTMLInputElement>("#pf-h"),
  base: $<HTMLSelectElement>("#pf-base"),
};
let editing = "";

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
    const nozzles = p.nozzles.map(mm);
    const base = printers!.bases.find((b) => b.id === p.base)?.name;
    li.innerHTML = `
      <div class="printers__text">
        <strong></strong>
        <span class="help"></span>
      </div>
      <button class="btn btn--quiet btn--sm" type="button" data-act="edit">Editar</button>
      <button class="btn btn--quiet btn--sm" type="button" data-act="delete">Borrar</button>`;
    li.querySelector("strong")!.textContent = p.name;
    li.querySelector(".help")!.textContent =
      `${mm(p.volumeMm[0])} × ${mm(p.volumeMm[1])} × ${mm(p.volumeMm[2])} mm, boquilla de ${nozzles.length > 1 ? `${nozzles.slice(0, -1).join(", ")} o ${nozzles[nozzles.length - 1]}` : nozzles[0]} mm. ${base ? `Perfil de ${base}.` : "Genérica."}`;
    li.querySelector('[data-act="edit"]')!.addEventListener("click", () => fillPrinterForm(p));
    li.querySelector('[data-act="delete"]')!.addEventListener("click", () => void deletePrinter(p));
    return li;
  }));
}

function fillPrinterForm(p: PrinterProfile | null) {
  editing = p?.id ?? "";
  $("#h-printer-form").textContent = p ? `Editar ${p.name}` : "Nueva impresora";
  $<HTMLButtonElement>("#printer-new").hidden = !p;
  pf.name.value = p?.name ?? "";
  [pf.w.value, pf.d.value, pf.h.value] = (p?.volumeMm ?? ["", "", ""]).map(String);
  pf.base.value = p?.base ?? "";
  const nozzles = p?.nozzles ?? [0.4];
  document.querySelectorAll<HTMLInputElement>("#pf-nozzles input").forEach((i) => (i.checked = nozzles.includes(Number(i.value))));
  for (const el of [pf.name, pf.w, pf.d, pf.h]) el.removeAttribute("aria-invalid");
  printerError.hidden = true;
  renderPrinterList();
}

function openPrinterDialog() {
  if (!printers) return;
  pf.base.replaceChildren(new Option("Ninguno: impresora genérica", ""), ...printers.bases.map((b) => new Option(b.name, b.id)));
  $("#pf-nozzles").replaceChildren(
    Object.assign(document.createElement("legend"), { textContent: "Boquillas que admite, en mm" }),
    ...printers.nozzles.flatMap((n) => {
      const input = Object.assign(document.createElement("input"), { type: "checkbox", id: `pf-n${n}`, value: String(n), name: "pf-nozzle" });
      const label = Object.assign(document.createElement("label"), { htmlFor: input.id, textContent: mm(n) });
      return [input, label];
    }),
  );
  fillPrinterForm(null);
  printerDialog.showModal();
  pf.name.focus();
}

// A Bambu Lab base fills in its volume when the fields are still empty.
pf.base.addEventListener("change", () => {
  const base = profileOf(pf.base.value);
  if (!base) return;
  [pf.w, pf.d, pf.h].forEach((el, i) => {
    if (!el.value) el.value = String(base.volumeMm[i]);
  });
});

function printerFormError(message: string, field?: HTMLElement) {
  printerError.textContent = message;
  printerError.hidden = false;
  field?.setAttribute("aria-invalid", "true");
  field?.focus();
}

printerForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const save = $<HTMLButtonElement>("#printer-save");
  if (save.getAttribute("aria-busy") === "true") return;
  for (const el of [pf.name, pf.w, pf.d, pf.h]) el.removeAttribute("aria-invalid");
  printerError.hidden = true;
  const name = pf.name.value.trim();
  if (!name) return printerFormError("Ponle un nombre a la impresora.", pf.name);
  const volume = [pf.w, pf.d, pf.h].map((el) => Number(el.value));
  const bad = [pf.w, pf.d, pf.h].find((el, i) => !el.value || !(volume[i] >= 20 && volume[i] <= 2000));
  if (bad) return printerFormError("Ancho, fondo y alto van entre 20 y 2000 mm.", bad);
  const nozzles = [...document.querySelectorAll<HTMLInputElement>("#pf-nozzles input:checked")].map((i) => Number(i.value));
  if (!nozzles.length) return printerFormError("Marca al menos una boquilla.", document.querySelector<HTMLInputElement>("#pf-nozzles input") ?? undefined);
  const profile: PrinterProfile = { id: editing, name, volumeMm: volume as [number, number, number], nozzles, base: pf.base.value || null, custom: true };
  save.setAttribute("aria-busy", "true");
  save.textContent = "Guardando…";
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
    toast(`${name} guardada y elegida. Seguirá aquí la próxima vez que abras BlendParser.`);
  } catch (err) {
    printerFormError(String(err));
  } finally {
    save.removeAttribute("aria-busy");
    save.textContent = "Guardar impresora";
  }
});

async function deletePrinter(p: PrinterProfile) {
  const ok = await ask(`Se borrará ${p.name} de tus impresoras. Los 3MF que ya exportaste no cambian.`, {
    title: "Borrar impresora",
    kind: "warning",
    okLabel: "Borrar",
    cancelLabel: "Cancelar",
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
    toast(`${p.name} borrada.`);
  } catch (err) {
    printerFormError(String(err));
  }
}

$("#open-printers").addEventListener("click", openPrinterDialog);
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

function setTool(t: Tool) {
  app.dataset.tool = t;
  document.querySelectorAll<HTMLButtonElement>("button[data-tool]").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.tool === t)));
  radiusInput.disabled = t !== "brush";
  viewer.setTool(t);
  bind("hint", t === "brush"
    ? "Arrastra para pintar, botón derecho para orbitar, [ y ] cambian el radio."
    : "Arrastra para orbitar, botón derecho para mover, rueda para acercar, F para encuadrar.");
}

document.querySelectorAll<HTMLButtonElement>("button[data-view]").forEach((b) => b.addEventListener("click", () => setView(b.dataset.view as View)));
document.querySelectorAll<HTMLButtonElement>("button[data-tool]").forEach((b) => b.addEventListener("click", () => setTool(b.dataset.tool as Tool)));

async function undo() {
  if (!summary?.canUndo) return;
  const what = summary.undoLabel;
  await apply(() => api.undo(), () => toast(`Deshecho: ${what ?? "último cambio"}.`));
}
$("#undo").addEventListener("click", () => void undo());

window.addEventListener("keydown", (e) => {
  const typing = e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement || e.target instanceof HTMLTextAreaElement;
  if (typing || app.dataset.state !== "ready" || zipDialog.open || printerDialog.open) return;
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z") {
    e.preventDefault();
    void undo();
  } else if (e.key === "b") setTool("brush");
  else if (e.key === "o") setTool("orbit");
  else if (e.key === "f") viewer.frameAll();
  else if (e.key === "[" || e.key === "]") {
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
  if (!quiet) bind("version", "Buscando…");
  try {
    pendingUpdate = await findUpdate();
    versionBtn.classList.toggle("has-update", pendingUpdate !== null);
    if (pendingUpdate) {
      const u = pendingUpdate;
      toast(`Hay una versión nueva: ${u.version}.`, "ok", { label: `Actualizar a ${u.version}`, run: () => void applyUpdate(u) }, true);
    } else if (!quiet) {
      toast(`Tienes la versión más reciente (${await currentVersion()}).`);
    }
  } catch (e) {
    if (!quiet) toast(`No pude buscar actualizaciones: ${e}`, "error");
  } finally {
    versionBtn.disabled = false;
    bind("version", `v${await currentVersion()}`);
  }
}

async function applyUpdate(u: Update) {
  if (app.dataset.state === "ready") {
    const ok = await ask(`BlendParser se cerrará para instalar la versión ${u.version}. Exporta antes lo que quieras conservar.`, {
      title: "Actualizar BlendParser",
      kind: "info",
      okLabel: "Actualizar y reiniciar",
      cancelLabel: "Más tarde",
    });
    if (!ok) return;
  }
  versionBtn.disabled = true;
  toast(`Descargando la versión ${u.version}…`, "ok", undefined, true);
  try {
    await installUpdate(u, (f) =>
      bind("toast", f === null ? `Descargando la versión ${u.version}…` : `Descargando la versión ${u.version}: ${Math.round(f * 100)} %`));
  } catch (e) {
    versionBtn.disabled = false;
    toast(`No se pudo instalar la actualización: ${e}`, "error");
  }
}

versionBtn.addEventListener("click", () => {
  if (pendingUpdate) void applyUpdate(pendingUpdate);
  else void checkForUpdates(false);
});

// ---- start -------------------------------------------------------------------------------

setState("empty");
setTool("orbit");
void api.printers().then((p) => {
  printers = p;
  renderPrinterPick(p.selected, p.nozzleMm);
});
void currentVersion().then((v) => bind("version", `v${v}`));
// Check quietly once the window has settled; only a newer version is announced.
window.setTimeout(() => void checkForUpdates(true), 4000);
syncOutputs();
syncBrush();
void api.blenderStatus().then(async (path) => {
  if (!path) {
    $<HTMLButtonElement>("#find-blender").hidden = false;
    setState("error", "No encontré Blender en este equipo. BlendParser lo usa para leer el .blend; indícame dónde está blender.exe.");
    return;
  }
  const file = await api.initialFile();
  if (file) await openFile(file);
});
