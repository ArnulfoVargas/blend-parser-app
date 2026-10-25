// Before converting an OBJ, glTF or FBX, finds the files it references (its
// .mtl, .bin and textures). When some are missing, a dialog walks through one
// optional step per kind so the user can add them; nothing is required to go on.
import { open } from "@tauri-apps/plugin-dialog";
import { api, type Companion } from "./api";
import { t } from "./i18n";
import "./styles/companions.css";

type Kind = Companion["kind"];
const KINDS: Kind[] = ["materials", "buffers", "textures"];
const PICK: Record<Kind, string[]> = {
  materials: ["mtl"],
  buffers: ["bin"],
  textures: ["png", "jpg", "jpeg", "tga", "bmp", "tif", "tiff", "exr", "webp", "hdr"],
};
/** Formats whose colours can live in other files. */
const WITH_COMPANIONS = /\.(obj|gltf|fbx)$/i;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className = "", text = ""): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

const fileName = (p: string) => p.split(/[\\/]/).pop() || p;

/** The added files to convert with, or null when the user cancels. Resolves at
 *  once, without a dialog, when the model needs nothing it cannot find. */
export async function gatherCompanions(model: string, extras: string[] = []): Promise<string[] | null> {
  if (!WITH_COMPANIONS.test(model)) return extras;
  let found: Companion[];
  try {
    found = await api.inspectModel(model, extras);
  } catch {
    return extras; // Inspection is a convenience; the import reports what it misses.
  }
  if (found.every((c) => c.found)) return extras;

  const dialog = document.querySelector<HTMLDialogElement>("#files-dialog")!;
  const stepper = dialog.querySelector<HTMLOListElement>(".files__steps")!;
  const panel = dialog.querySelector<HTMLElement>(".files__panel")!;
  const back = dialog.querySelector<HTMLButtonElement>("#files-back")!;
  const next = dialog.querySelector<HTMLButtonElement>("#files-next")!;
  const now = dialog.querySelector<HTMLButtonElement>("#files-now")!;
  dialog.querySelector(".files__model")!.textContent = fileName(model);

  const added = [...extras];
  // Steps stay once shown; adding an .mtl can reveal a textures step after it.
  let kinds: Kind[] = [];
  let at = 0;
  const refresh = () => {
    for (const kind of KINDS) if (!kinds.includes(kind) && found.some((c) => c.kind === kind && !c.found)) kinds.push(kind);
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

  function render() {
    const kind = kinds[at];
    const steps = [t("files.step.model"), ...kinds.map((k) => t(`files.${k}.step`))];
    stepper.replaceChildren(...steps.map((label, i) => {
      const li = el("li", "files__step");
      const items = i === 0 ? [] : found.filter((c) => c.kind === kinds[i - 1]);
      const complete = i === 0 || items.every((c) => c.found);
      li.dataset.state = i - 1 === at ? "current" : complete ? "done" : i - 1 < at ? "skipped" : "todo";
      if (i - 1 === at) li.setAttribute("aria-current", "step");
      li.append(el("span", "files__dot", complete && i - 1 !== at ? "✓" : String(i + 1)), el("span", "files__label", label));
      return li;
    }));

    const items = found.filter((c) => c.kind === kind);
    const ready = items.filter((c) => c.found).length;
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
    pick.addEventListener("click", () => void add(kind, false));
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
      el("p", "files__count", t("files.count", { found: ready, total: items.length })),
      list,
      actions,
    );
    if (added.length) panel.append(el("p", "help files__added", t("files.added", { count: added.length })));

    const last = at === kinds.length - 1;
    back.hidden = at === 0;
    now.hidden = last;
    next.textContent = t(last ? "files.convert" : ready === items.length ? "files.next" : "files.skip");
  }

  return new Promise((resolve) => {
    let result: string[] | null = null;
    const onBack = () => { at = Math.max(0, at - 1); render(); };
    const onNext = () => {
      if (at < kinds.length - 1) { at++; render(); return; }
      result = added;
      dialog.close();
    };
    const onNow = () => { result = added; dialog.close(); };
    back.addEventListener("click", onBack);
    next.addEventListener("click", onNext);
    now.addEventListener("click", onNow);
    dialog.addEventListener("close", () => {
      back.removeEventListener("click", onBack);
      next.removeEventListener("click", onNext);
      now.removeEventListener("click", onNow);
      resolve(result);
    }, { once: true });
    render();
    dialog.showModal();
    next.focus();
  });
}
