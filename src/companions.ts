// Before converting an OBJ, glTF or FBX, finds the files it references (its
// .mtl, .bin and textures). When some are missing, a dialog walks through one
// optional step per kind so the user can add them; nothing is required to go on.
// An OBJ whose materials lost their textures on export (grey, no map, but UVs)
// gets a step to give each material an image.
import { open } from "@tauri-apps/plugin-dialog";
import { api, type Companion } from "./api";
import { t } from "./i18n";
import "./styles/companions.css";

type Kind = Companion["kind"];
const KINDS: Kind[] = ["materials", "buffers", "textures", "unmapped"];
const IMAGES = ["png", "jpg", "jpeg", "tga", "bmp", "tif", "tiff", "exr", "webp", "hdr"];
const PICK: Record<Kind, string[]> = { materials: ["mtl"], buffers: ["bin"], textures: IMAGES, unmapped: IMAGES };

export interface Added {
  files: string[];
  /** [material, image] for materials whose texture the model does not name. */
  textures: [string, string][];
}

/** A name stripped of the prefixes and suffixes exporters add, so that
 *  `proj_Leg_L1.png` and `pmat_Leg_L1` meet as `leg_l1`. */
function stem(name: string): string {
  const words = fileName(name).toLowerCase().replace(/\.[a-z0-9]+$/, "").split(/[\s._-]+/).filter(Boolean);
  const lead = new Set(["pmat", "mat", "material", "proj", "tex", "texture", "t", "m"]);
  const tail = new Set(["basecolor", "base", "color", "colour", "col", "diffuse", "diff", "albedo", "d"]);
  while (words.length > 1 && lead.has(words[0])) words.shift();
  while (words.length > 1 && tail.has(words[words.length - 1])) words.pop();
  return words.join("_");
}

/** Each material the images can be told apart for: same stem, else the only image containing it. */
function match(materials: string[], images: string[]): Map<string, string> {
  const out = new Map<string, string>();
  for (const m of materials) {
    const key = stem(m);
    const exact = images.filter((i) => stem(i) === key);
    const loose = images.filter((i) => stem(i).includes(key) || key.includes(stem(i)));
    const pick = exact.length === 1 ? exact[0] : exact.length === 0 && loose.length === 1 ? loose[0] : undefined;
    if (pick) out.set(m, pick);
  }
  return out;
}
/** Formats whose colours can live in other files. */
const WITH_COMPANIONS = /\.(obj|gltf|fbx)$/i;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className = "", text = ""): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

const fileName = (p: string) => p.split(/[\\/]/).pop() || p;

/** What the user added to convert with, or null when they cancel. Resolves at
 *  once, without a dialog, when the model needs nothing it cannot find. */
export async function gatherCompanions(model: string, extras: string[] = []): Promise<Added | null> {
  const plain: Added = { files: extras, textures: [] };
  if (!WITH_COMPANIONS.test(model)) return plain;
  let found: Companion[];
  try {
    found = await api.inspectModel(model, extras);
  } catch {
    return plain; // Inspection is a convenience; the import reports what it misses.
  }
  if (found.every((c) => c.found)) return plain;

  const dialog = document.querySelector<HTMLDialogElement>("#files-dialog")!;
  const stepper = dialog.querySelector<HTMLOListElement>(".files__steps")!;
  const panel = dialog.querySelector<HTMLElement>(".files__panel")!;
  const back = dialog.querySelector<HTMLButtonElement>("#files-back")!;
  const next = dialog.querySelector<HTMLButtonElement>("#files-next")!;
  const now = dialog.querySelector<HTMLButtonElement>("#files-now")!;
  dialog.querySelector(".files__model")!.textContent = fileName(model);

  const added = [...extras];
  const assigned = new Map<string, string>();
  let unmatched = 0;
  const has = (c: Companion) => c.kind === "unmapped" ? assigned.has(c.name) : c.found;
  const result = (): Added => ({ files: added, textures: [...assigned] });
  // Steps stay once shown; adding an .mtl can reveal a textures step after it.
  let kinds: Kind[] = [];
  let at = 0;
  const refresh = () => {
    for (const kind of KINDS) if (!kinds.includes(kind) && found.some((c) => c.kind === kind && !has(c))) kinds.push(kind);
    kinds.sort((a, b) => KINDS.indexOf(a) - KINDS.indexOf(b));
  };
  refresh();

  async function add(kind: Kind, directory: boolean) {
    const picked = await open(directory
      ? { directory: true, multiple: false, title: t("files.pickFolder") }
      : { multiple: true, directory: false, filters: [{ name: t(`files.${kind}.filter`), extensions: PICK[kind] }] });
    const paths = picked === null ? [] : Array.isArray(picked) ? picked : [picked];
    if (!paths.length) return;
    for (const p of paths) if (!added.includes(p)) added.push(p);
    try {
      found = await api.inspectModel(model, added);
    } catch {
      // Keep what we had; the import will still use the files.
    }
    refresh();
    render();
  }

  /** Images for the materials: several at once are matched by name, one goes to `only`. */
  async function assign(only?: string) {
    const picked = await open({ multiple: !only, directory: false, filters: [{ name: t("files.textures.filter"), extensions: IMAGES }] });
    const paths = picked === null ? [] : Array.isArray(picked) ? picked : [picked];
    if (!paths.length) return;
    if (only) {
      assigned.set(only, paths[0]);
      unmatched = 0;
    } else {
      const materials = found.filter((c) => c.kind === "unmapped").map((c) => c.name);
      const pairs = match(materials, paths);
      for (const [m, p] of pairs) assigned.set(m, p);
      unmatched = paths.filter((p) => ![...pairs.values()].includes(p)).length;
    }
    render();
  }

  function unmappedRows(items: Companion[]): HTMLElement {
    const list = el("ul", "files__list files__list--assign");
    for (const c of items) {
      const li = el("li");
      const image = assigned.get(c.name);
      li.dataset.found = String(Boolean(image));
      const target = el("span", "files__name", image ? fileName(image) : t("files.unmapped.none"));
      if (image) target.title = image;
      const choose = el("button", "link", t(image ? "files.unmapped.change" : "files.unmapped.choose"));
      choose.type = "button";
      choose.addEventListener("click", () => void assign(c.name));
      li.append(el("span", "files__mark", image ? "✓" : "–"), el("span", "files__material mono", c.name), target, choose);
      if (image) {
        const clear = el("button", "files__clear", "×");
        clear.type = "button";
        clear.title = t("files.unmapped.clear");
        clear.setAttribute("aria-label", t("files.unmapped.clear"));
        clear.addEventListener("click", () => { assigned.delete(c.name); render(); });
        li.append(clear);
      }
      list.append(li);
    }
    return list;
  }

  function render() {
    const kind = kinds[at];
    const steps = [t("files.step.model"), ...kinds.map((k) => t(`files.${k}.step`))];
    stepper.replaceChildren(...steps.map((label, i) => {
      const li = el("li", "files__step");
      const items = i === 0 ? [] : found.filter((c) => c.kind === kinds[i - 1]);
      const complete = i === 0 || items.every(has);
      li.dataset.state = i - 1 === at ? "current" : complete ? "done" : i - 1 < at ? "skipped" : "todo";
      if (i - 1 === at) li.setAttribute("aria-current", "step");
      li.append(el("span", "files__dot", complete && i - 1 !== at ? "✓" : String(i + 1)), el("span", "files__label", label));
      return li;
    }));

    const items = found.filter((c) => c.kind === kind);
    const ready = items.filter(has).length;
    const head = el("div", "files__head");
    head.append(el("h3", "", t(`files.${kind}.title`)));
    const required = items.some((c) => c.required && !c.found);
    const tag = el("span", "files__tag", t(required ? "files.required" : "files.optional"));
    tag.dataset.required = String(required);
    head.append(tag);
    const list = el("ul", "files__list");
    for (const c of items) {
      const li = el("li");
      li.dataset.found = String(c.found);
      const name = el("span", "files__name mono", fileName(c.name));
      name.title = c.name;
      li.append(el("span", "files__mark", c.found ? "✓" : "–"), name, el("span", "files__state", t(c.found ? "files.found" : "files.missing")));
      list.append(li);
    }
    const actions = el("div", "files__add");
    const pick = el("button", "btn btn--sm", t(`files.${kind}.add`));
    pick.type = "button";
    pick.addEventListener("click", () => void (kind === "unmapped" ? assign() : add(kind, false)));
    actions.append(pick);
    if (kind === "textures") {
      const folder = el("button", "link", t("files.pickFolder"));
      folder.type = "button";
      folder.addEventListener("click", () => void add(kind, true));
      actions.append(folder);
    }
    panel.replaceChildren(
      head,
      el("p", "help", t(`files.${kind}.help`)),
      el("p", "files__count", t(kind === "unmapped" ? "files.unmapped.count" : "files.count", { found: ready, total: items.length })),
      kind === "unmapped" ? unmappedRows(items) : list,
      actions,
    );
    if (kind === "unmapped" && unmatched) panel.append(el("p", "help files__added", t("files.unmapped.unmatched", { count: unmatched })));
    if (kind === "unmapped" && /\.obj$/i.test(model)) panel.append(el("p", "note files__tip", t("files.unmapped.blend")));
    if (added.length) panel.append(el("p", "help files__added", t("files.added", { count: added.length })));

    const last = at === kinds.length - 1;
    back.hidden = at === 0;
    now.hidden = last;
    next.textContent = t(last ? "files.convert" : ready === items.length ? "files.next" : "files.skip");
  }

  return new Promise((resolve) => {
    let chosen: Added | null = null;
    const onBack = () => { at = Math.max(0, at - 1); render(); };
    const onNext = () => {
      if (at < kinds.length - 1) { at++; render(); return; }
      chosen = result();
      dialog.close();
    };
    const onNow = () => { chosen = result(); dialog.close(); };
    back.addEventListener("click", onBack);
    next.addEventListener("click", onNext);
    now.addEventListener("click", onNow);
    dialog.addEventListener("close", () => {
      back.removeEventListener("click", onBack);
      next.removeEventListener("click", onNext);
      now.removeEventListener("click", onNow);
      resolve(chosen);
    }, { once: true });
    render();
    dialog.showModal();
    next.focus();
  });
}
