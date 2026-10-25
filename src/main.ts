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
import { api, type Summary } from "./api";
import { currentVersion, findUpdate, installUpdate, type Update } from "./updater";
import { Viewer, type Tool, type View } from "./viewer";

type State = "empty" | "converting" | "error" | "ready";

const app = document.querySelector<HTMLElement>("[data-app]")!;
const $ = <T extends HTMLElement>(sel: string) => document.querySelector<T>(sel)!;
const live = $<HTMLParagraphElement>("#live");
const nf = (n: number, d = 0) => n.toLocaleString("es", { maximumFractionDigits: d, minimumFractionDigits: d });

let summary: Summary | null = null;
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
  bind("bedFit", s.fitsBed ? `Cabe en la placa de ${plate}.` : `No cabe en la placa de ${plate}.`);
  bind("fitBanner", `No cabe en la placa: mide ${nf(x)} × ${nf(y)} mm.`);
  $<HTMLDivElement>("#fit-banner").hidden = s.fitsBed;
  app.classList.toggle("is-oversize", !s.fitsBed);
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
  $<HTMLSelectElement>("#printer").value = st.printer;
  syncOutputs();
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
  bind("swapsDetail", `${nf(s.estimate.purgeGrams)} g de purga; ${nf(s.estimate.multicolorLayers)} de ${nf(s.estimate.layers)} capas llevan más de un color. Estimado a 0,2 mm por capa.`);

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
  if (!summary || summary.fitsBed) return true;
  return ask(`El modelo mide ${nf(summary.sizeMm[0])} × ${nf(summary.sizeMm[1])} mm y no cabe en la placa de ${nf(summary.bedMm[0])} × ${nf(summary.bedMm[1])} mm. ¿Exportar de todos modos?`, {
    title: "No cabe en la placa",
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
const syncCore = () => {
  $<HTMLElement>("#core-field").hidden = !(Number($<HTMLInputElement>("#depth").value) > 0);
};
$("#depth").addEventListener("input", syncCore);
$("#open-zip").addEventListener("click", () => {
  if (app.dataset.state !== "ready" || !summary) return;
  const core = $<HTMLSelectElement>("#core");
  const keep = core.value;
  core.replaceChildren(
    new Option("El más usado", ""),
    ...summary.slots.map((s, i) => new Option(`Ranura ${i + 1} · ${s.hex}`, String(i))),
  );
  core.value = [...core.options].some((o) => o.value === keep) ? keep : "";
  syncCore();
  zipDialog.showModal();
});
$("#export-zip").addEventListener("click", async () => {
  if (!summary) return;
  const depth = $<HTMLInputElement>("#depth");
  const minPiece = $<HTMLInputElement>("#min-piece");
  const depthMm = Number(depth.value);
  const minPieceMm3 = Number(minPiece.value);
  const depthOk = depth.value !== "" && depthMm >= 0 && depthMm <= 50;
  const minOk = minPiece.value !== "" && minPieceMm3 >= 0 && minPieceMm3 <= 10000;
  depth.setAttribute("aria-invalid", String(!depthOk));
  minPiece.setAttribute("aria-invalid", String(!minOk));
  if (!depthOk || !minOk) {
    (depthOk ? minPiece : depth).focus();
    return;
  }
  const core = $<HTMLSelectElement>("#core").value;
  const options = {
    voxelMm: Number($<HTMLSelectElement>("#voxel").value),
    depthMm,
    coreSlot: depthMm > 0 && core !== "" ? Number(core) : null,
    minPieceMm3,
  };
  zipDialog.close();
  if (!(await confirmOversize())) return;
  const path = await save({ defaultPath: summary.fileName.replace(/\.blend$/i, "_piezas.zip"), filters: [{ name: "ZIP", extensions: ["zip"] }] });
  if (!path) return;
  await work(async () => {
    const r = await api.exportColorZip(path, options);
    const extra = r.oversize ? ` ${r.oversize} no caben en la placa; revisa la guía.` : "";
    toast(`${r.pieces} piezas en ${r.plates} placas, con guía de armado, guardadas en ${path}.${extra}`, r.oversize ? "error" : undefined);
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

// Largest height whose footprint fits 90% of the plate.
$("#fit").addEventListener("click", () => {
  if (!summary) return;
  const [x, y] = summary.sizeMm;
  const k = Math.min((summary.bedMm[0] * 0.9) / x, (summary.bedMm[1] * 0.9) / y);
  const mm = Math.floor(summary.settings.heightMm * k * 10) / 10;
  void apply(() => api.setHeight(mm), () => toast(`Altura ajustada a ${nf(mm, 1)} mm para que quepa en la placa.`));
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

$("#printer").addEventListener("change", (e) => {
  void work(async () => render(await api.setPrinter((e.target as HTMLSelectElement).value)));
});

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
  if (typing || app.dataset.state !== "ready" || zipDialog.open) return;
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
