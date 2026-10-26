import { api, isAppError, type Closure, type ColourMatch, type PaintResult, type RegionAction, type RegionPreview, type Selection, type Summary, type Vec3 } from "./api";
import { num, t, volume, type Key } from "./i18n";
import "./styles/region.css";

const $ = <T extends HTMLElement>(selector: string) => document.querySelector<T>(selector)!;
/** The selection's highlight, mixed over the filament so the painting still shows under it. */
const PICKED: Vec3 = [214, 51, 132];
const PICKED_MIX = 0.55;
/** While the pieces are pulled apart, what stays where it was fades out. */
const GHOST: Vec3 = [236, 236, 232];
/** The brush's footprint: the selection's colour, or grey while it erases. */
const CURSOR = { select: "#d63384", erase: "#7b807b" };

type Op = "grow" | "connected" | "smooth" | "invert" | "clear";
/** A one-click way out of a refused cut: what the button says and what it changes. */
interface Fix { label: string; run: () => void }

/**
 * The region tool, in three steps. Select part of the surface without
 * painting it, with a brush or by clicking a colour, and adjust the selection.
 * Choose what to do with it: extract it as a piece or remove it. Choose how the
 * cut is closed: a shell of even thickness, or a cap down to a point inside.
 * The result is previewed before it can be applied (the closing faces in
 * orange, the pieces apart if asked); a refused cut says why and offers the
 * changes that would fix it.
 */
export class RegionPanel {
  private active = false;
  private add = true;
  private alt = false;
  private through = false;
  private action: "remove" | "extract" = "extract";
  private kind: "shell" | "cap" = "shell";
  /** Each closing keeps its own size: a shell's thickness, a cap's depth. */
  private sizes = { shell: 2, cap: 2 };
  private layFlat = false;
  private apart = false;
  private byColour = false;
  /** The last colour pick, to try it again when its criteria change: the face, how it added, and the selection before it. */
  private lastPick: { seed: number; add: boolean; before: number[] } | null = null;
  private picks = 0;
  private picked = new Set<number>();
  private info: Selection = { tris: [], count: 0, regions: 0 };
  private preview: RegionPreview | null = null;
  /** The preview matches the selection and the settings as they are. */
  private fresh = false;
  private failure: unknown = null;
  private generation = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private mm = $<HTMLInputElement>("#reg-mm");
  private mmRange = $<HTMLInputElement>("#reg-mm-range");
  private clearance = $<HTMLInputElement>("#reg-clearance");
  private group = $<HTMLInputElement>("#reg-group");
  private match = $<HTMLSelectElement>("#reg-match");
  private tolerance = $<HTMLInputElement>("#reg-tolerance");
  private contiguous = $<HTMLInputElement>("#reg-contiguous");

  constructor(private deps: {
    summary(): Summary | null;
    radiusMm(): number;
    tint(colors: (Vec3 | null)[] | null, mix: number): void;
    overlay(preview: RegionPreview | null, pull: number): void;
    apply(closure: Closure, action: RegionAction): Promise<boolean>;
    /** Switches the view between brushing and picking faces with a click. */
    picking(on: boolean): void;
    /** The brush's footprint colour changed: selecting or erasing. */
    cursor(): void;
    /** Makes the colour borders that cross triangles edges of the mesh. */
    refine(): Promise<void>;
    close(): void;
    error(e: unknown): string;
  }) {
    const seg = (on: string, off: string, set: (v: boolean) => void) => {
      $(on).addEventListener("click", () => { set(true); this.changed(); });
      $(off).addEventListener("click", () => { set(false); this.changed(); });
    };
    seg("#reg-add", "#reg-erase", (v) => {
      this.add = v;
      deps.cursor();
    });
    seg("#reg-through", "#reg-visible", (v) => (this.through = v));
    seg("#reg-extract", "#reg-remove", (v) => (this.action = v ? "extract" : "remove"));
    seg("#reg-flat", "#reg-assembled", (v) => (this.layFlat = v));
    seg("#reg-shell", "#reg-cap", (v) => this.setKind(v ? "shell" : "cap"));
    seg("#reg-by-colour", "#reg-by-brush", (v) => {
      this.byColour = v;
      deps.picking(v);
    });
    for (const [id, apart] of [["#reg-together", false], ["#reg-apart", true]] as const) {
      $(id).addEventListener("click", () => { this.apart = apart; this.render(); });
    }
    this.mm.addEventListener("input", () => {
      this.sizes[this.kind] = this.mm.valueAsNumber;
      this.mmRange.value = this.mm.value;
      this.changed();
    });
    this.mmRange.addEventListener("input", () => {
      this.mm.value = this.mmRange.value;
      this.sizes[this.kind] = this.mmRange.valueAsNumber;
      this.changed();
    });
    this.clearance.addEventListener("input", () => this.changed());
    this.group.addEventListener("change", () => this.changed());
    // A new criterion, tolerance or reach tries the last pick again on the selection it started from.
    for (const el of [this.match, this.contiguous]) el.addEventListener("change", () => void this.repick());
    this.tolerance.addEventListener("input", () => {
      $("#reg-tolerance-out").textContent = this.tolerance.value;
      void this.repick();
    });
    $("#reg-refine").addEventListener("click", () => void deps.refine());
    // The panel's radius is the brush's: moving either moves both.
    const radius = $<HTMLInputElement>("#radius");
    const regRadius = $<HTMLInputElement>("#reg-radius");
    regRadius.addEventListener("input", () => {
      radius.value = regRadius.value;
      radius.dispatchEvent(new Event("input"));
    });
    radius.addEventListener("input", () => {
      regRadius.value = radius.value;
      $("#reg-radius-out").textContent = `${num(radius.valueAsNumber, 1)} mm`;
    });
    // More options close when the pointer goes elsewhere, like a menu.
    document.addEventListener("click", (e) => {
      const more = $<HTMLDetailsElement>("#reg-more");
      if (more.open && !more.contains(e.target as Node)) more.open = false;
    });
    document.querySelectorAll<HTMLButtonElement>("[data-reg-op]").forEach((b) =>
      b.addEventListener("click", () => void this.op(b.dataset.regOp as Op)));
    $("#reg-off").addEventListener("click", () => deps.close());
    $("#reg-apply").addEventListener("click", () => void this.apply());
    // Alt held turns the brush into an eraser for as long as it is held.
    const onAlt = (e: KeyboardEvent) => {
      if (e.key !== "Alt" || this.alt === e.altKey) return;
      this.alt = e.altKey;
      deps.cursor();
      this.render();
    };
    window.addEventListener("keydown", onAlt);
    window.addEventListener("keyup", onAlt);
    window.addEventListener("blur", () => { this.alt = false; deps.cursor(); this.render(); });
  }

  open() {
    this.active = true;
    const radius = $<HTMLInputElement>("#radius");
    $<HTMLInputElement>("#reg-radius").value = radius.value;
    $("#reg-radius-out").textContent = `${num(radius.valueAsNumber, 1)} mm`;
    void this.refresh();
  }

  close() {
    this.active = false;
    $<HTMLDetailsElement>("#reg-more").open = false;
    this.lastPick = null;
    this.generation++;
    clearTimeout(this.timer);
    this.deps.tint(null, 1);
    this.deps.overlay(null, 0);
  }

  /** A new preview mesh: the backend keeps the selection only if the triangles are the same. */
  meshChanged() {
    this.lastPick = null;
    this.preview = null;
    this.fresh = false;
    if (this.active) void this.refresh();
  }

  /** The shortcuts the tool has now: brushing, or picking by colour. */
  hint(): "hint.region" | "hint.regionColour" {
    return this.byColour ? "hint.regionColour" : "hint.region";
  }

  /** The brush footprint's colour: erasing shows grey. */
  cursorColour(): string {
    return this.add === this.alt ? CURSOR.erase : CURSOR.select;
  }

  /** Words again, for a new language. */
  update() {
    if (this.active) this.render();
  }

  /** A brush stroke on the view: selects, or erases with the eraser or Alt. */
  async stroke(o: Vec3, d: Vec3): Promise<PaintResult | null> {
    const add = this.add !== this.alt;
    const r = await api.selectRegion(o, d, this.deps.radiusMm(), add, this.through);
    if (r && r.tris.length) {
      for (const id of r.tris) {
        if (add) this.picked.add(id); else this.picked.delete(id);
      }
      this.fresh = false;
      this.preview = null;
      this.deps.overlay(null, 0);
      this.tint();
    }
    return r;
  }

  /** The brush was lifted: count the regions again and preview the cut. */
  endStroke() {
    void this.refresh();
  }

  /** A click on the view while picking by colour: the faces like the one under it, added (Ctrl or Shift) or instead of the selection; Alt takes them out. */
  async click(triangle: number | null, additive: boolean) {
    if (!this.byColour || triangle === null) return;
    const add = !this.alt;
    const before = additive || !add ? [...this.picked] : [];
    this.lastPick = { seed: triangle, add, before };
    await this.pick();
  }

  private setKind(kind: "shell" | "cap") {
    this.kind = kind;
    this.mm.value = String(this.sizes[kind]);
    this.mmRange.value = this.mm.value;
  }

  private how(): ColourMatch {
    if (this.match.value !== "similar") return { kind: this.match.value as "material" | "filament" };
    return { kind: "similar", deltaE: this.tolerance.valueAsNumber };
  }

  private async repick() {
    this.render();
    if (this.lastPick) await this.pick();
  }

  private async pick() {
    const p = this.lastPick;
    if (!p) return;
    // Only the last of several quick tries counts.
    const token = ++this.picks;
    try {
      const got = await api.selectByColour(p.seed, this.how(), this.contiguous.checked, p.add, p.before);
      if (token === this.picks) this.got(got);
    } catch (e) {
      if (token === this.picks) this.fail(e);
    }
  }

  private async op(op: Op) {
    this.lastPick = null;
    try {
      this.got(await api.selectionOp(op));
    } catch (e) {
      this.fail(e);
    }
  }

  private async refresh() {
    if (!this.active || !this.deps.summary()) return;
    try {
      this.got(await api.selectionOp("get"));
    } catch (e) {
      this.fail(e);
    }
  }

  private got(info: Selection) {
    this.info = info;
    this.picked = new Set(info.tris);
    this.changed();
  }

  private settings(): { closure: Closure; action: RegionAction } | null {
    const mm = this.mm.valueAsNumber;
    const shell = this.kind === "shell";
    const ok = Number.isFinite(mm) && (shell ? mm > 0 && mm <= 50 : mm >= -50 && mm <= 50);
    this.mm.setAttribute("aria-invalid", String(!ok));
    // Clearance only matters for a piece that comes out to fit back.
    const fits = shell && this.action === "extract";
    const clearanceMm = fits ? this.clearance.valueAsNumber : 0;
    const clearanceOk = !fits || (Number.isFinite(clearanceMm) && clearanceMm >= 0 && clearanceMm <= 5 && clearanceMm < mm);
    this.clearance.setAttribute("aria-invalid", String(!clearanceOk));
    if (!ok || !clearanceOk) return null;
    return {
      closure: shell ? { kind: "shell", mm, clearanceMm } : { kind: "cap", mm },
      action: this.action === "remove" ? { kind: "remove" } : { kind: "extract", layFlat: this.layFlat, group: this.group.checked },
    };
  }

  /** Something the result depends on changed: preview it again shortly. */
  private changed() {
    this.fresh = false;
    this.preview = null;
    this.failure = null;
    this.deps.overlay(null, 0);
    this.render();
    clearTimeout(this.timer);
    if (this.active && this.picked.size) this.timer = setTimeout(() => void this.previewNow(), 250);
  }

  private async previewNow() {
    const s = this.settings();
    if (!s || !this.picked.size) return this.render();
    const generation = ++this.generation;
    try {
      const preview = await api.previewRegion(s.closure, s.action);
      if (generation !== this.generation) return;
      this.preview = preview;
      this.fresh = true;
    } catch (e) {
      if (generation !== this.generation) return;
      this.preview = null;
      this.fresh = false;
      this.failure = e;
    }
    this.render();
  }

  private fail(e: unknown) {
    this.failure = e;
    this.render();
  }

  private tint() {
    const n = this.deps.summary()?.triangles ?? 0;
    if (!this.picked.size || !n) return this.deps.tint(null, 1);
    const pulled = this.pulled();
    const colors: (Vec3 | null)[] = new Array(n).fill(null);
    for (const id of this.picked) if (id < n) colors[id] = pulled ? GHOST : PICKED;
    this.deps.tint(colors, pulled ? 0.85 : PICKED_MIX);
  }

  private pulled() {
    return this.fresh && this.apart && this.action === "extract";
  }

  /** The overlay and the highlight for the preview as it stands. */
  private show() {
    const s = this.deps.summary();
    const span = s ? Math.max(...s.sizeMm) / (s.size.previewScale || 1) : 0;
    this.deps.overlay(this.fresh ? this.preview : null, this.pulled() ? span * 0.3 : 0);
    this.tint();
  }

  /** What would fix a refused cut, as buttons; none when nothing obvious would. */
  private fixes(e: unknown): Fix[] {
    if (!isAppError(e)) return [];
    const thin = Math.round(this.sizes.shell * 5) / 10;
    const useCap = { label: t("reg.fixCap"), run: () => this.setKind("cap") };
    const useShell = { label: t("reg.fixShell"), run: () => this.setKind("shell") };
    switch (e.key) {
      case "error.regionThin":
      case "error.regionFolded":
        return [{ label: t("reg.fixThinner", { mm: num(thin, 1) }), run: () => { this.sizes.shell = thin; this.setKind("shell"); } }, useCap];
      case "error.regionCap":
        return [{ label: t("reg.fixShallower"), run: () => { this.sizes.cap = Math.round(this.sizes.cap * 5) / 10; this.setKind("cap"); } }, useShell];
      case "error.regionFlat":
        return [{ label: t("reg.fixDeeper", { mm: num(2) }), run: () => { this.sizes.cap = 2; this.setKind("cap"); } }, useShell];
      case "error.regionPinched":
      case "error.regionBorder":
        return [{ label: t("reg.smooth"), run: () => void this.op("smooth") }, { label: t("reg.grow"), run: () => void this.op("grow") }];
      case "error.regionClearance":
        return [{ label: t("reg.fixNoClearance"), run: () => { this.clearance.value = "0"; } }];
      case "error.regionEverything":
        return [{ label: t("reg.fixExtract"), run: () => { this.action = "extract"; } }];
      default:
        return [];
    }
  }

  private render() {
    if (!this.active) return;
    const pressed = (id: string, on: boolean) => $(id).setAttribute("aria-pressed", String(on));
    const erase = this.add === this.alt;
    pressed("#reg-add", !erase);
    pressed("#reg-erase", erase);
    pressed("#reg-visible", !this.through);
    pressed("#reg-through", this.through);
    pressed("#reg-by-brush", !this.byColour);
    pressed("#reg-by-colour", this.byColour);
    pressed("#reg-extract", this.action === "extract");
    pressed("#reg-remove", this.action === "remove");
    pressed("#reg-shell", this.kind === "shell");
    pressed("#reg-cap", this.kind === "cap");
    pressed("#reg-assembled", !this.layFlat);
    pressed("#reg-flat", this.layFlat);
    pressed("#reg-together", !this.apart);
    pressed("#reg-apart", this.apart);
    document.querySelectorAll<HTMLElement>("[data-reg-brush]").forEach((el) => (el.hidden = this.byColour));
    document.querySelectorAll<HTMLElement>("[data-reg-colour]").forEach((el) => (el.hidden = !this.byColour));
    $("#reg-tolerance-row").hidden = this.match.value !== "similar";
    // Borders inside triangles only matter when picking by filament.
    const split = this.deps.summary()?.splitTriangles ?? 0;
    $("#reg-refine").hidden = !this.byColour || this.match.value !== "filament" || split === 0;
    $("#reg-refine-note").textContent = t("reg.refineNote", { count: split, n: num(split) });

    const extract = this.action === "extract";
    const shell = this.kind === "shell";
    $("#reg-mm-label").textContent = t(shell ? "reg.thickness" : "reg.depth");
    this.mm.min = shell ? "0.1" : "-50";
    this.mm.title = t(shell ? "reg.thicknessHelp" : "reg.depthHelp");
    $("#reg-clearance-row").hidden = !shell || !extract;
    $("#reg-place").hidden = !extract;
    $("#reg-group-row").hidden = !extract;
    $("#reg-view").hidden = !extract || !(this.fresh && this.preview);

    // Steps 2 and 3 wait for something to be selected.
    const count = this.picked.size;
    for (const id of ["#reg-step2", "#reg-step3"]) $<HTMLFieldSetElement>(id).disabled = !count;
    $("#reg-count").textContent = count
      ? `${t("reg.count", { count: this.info.count, n: num(this.info.count) })} · ${t("reg.regions", { count: this.info.regions, n: num(this.info.regions) })}`
      : "";
    document.querySelectorAll<HTMLButtonElement>("[data-reg-op]").forEach((b) => (b.disabled = !count && b.dataset.regOp !== "invert"));

    const p = this.fresh ? this.preview : null;
    const apply = $<HTMLButtonElement>("#reg-apply");
    apply.disabled = !p;
    apply.textContent = !extract
      ? t("reg.applyRemove")
      : p ? t("reg.applyExtractN", { count: p.pieceMm3.length, n: num(p.pieceMm3.length) }) : t("reg.applyExtract");
    this.show();
    this.renderResult(p, count);
  }

  /** What to do first, while nothing is selected. */
  private tip(): Key {
    return this.byColour ? "reg.tipColour" : "reg.tipBrush";
  }

  private renderResult(p: RegionPreview | null, count: number) {
    const chips: [string, string, string?][] = [];
    const fixes = this.failure ? this.fixes(this.failure) : [];
    let status = "";
    let state: "pending" | "valid" | "error" | "warn" | "info" = "info";
    if (this.failure) {
      status = this.deps.error(this.failure);
      state = "error";
    } else if (!count) {
      status = t(this.tip());
    } else if (!p) {
      status = t("reg.previewing");
      state = "pending";
    } else if (p) {
      const pieces = p.pieceMm3.reduce((a, b) => a + b, 0);
      chips.push(this.action === "extract"
        ? [t("reg.chipPieces", { count: p.pieceMm3.length, n: num(p.pieceMm3.length), volume: volume(pieces) }), "is-piece"]
        : [t("reg.chipRemoved", { volume: volume(pieces) }), "is-piece"]);
      chips.push([t("reg.chipRest", { volume: volume(p.restMm3) }), ""]);
      if (p.closure.length) chips.push([t("reg.chipClosure"), "is-closure"]);
      // Warnings stay short; the whole explanation is one hover away.
      if (!p.closed) chips.push([t("reg.chipOpen"), "is-warn", t("reg.open")]);
      if (p.small) chips.push([t("reg.chipSmall", { count: p.small, n: num(p.small) }), "is-warn", t("reg.small", { count: p.small, n: num(p.small) })]);
      state = p.closed && !p.small ? "valid" : "warn";
    }
    $("#reg-summary").replaceChildren(...chips.map(([text, className, title]) => Object.assign(document.createElement("span"), { textContent: text, className, title: title ?? "" })));
    const el = $("#reg-status");
    el.textContent = status;
    el.dataset.state = state;
    $("#reg-fixes").replaceChildren(...fixes.map((f) => {
      const b = Object.assign(document.createElement("button"), { type: "button", className: "btn btn--quiet btn--sm", textContent: f.label });
      b.addEventListener("click", () => { f.run(); this.changed(); });
      return b;
    }));
  }

  private async apply() {
    const s = this.settings();
    if (!s || !this.fresh) return;
    if (await this.deps.apply(s.closure, s.action)) {
      this.picked.clear();
      this.preview = null;
      this.fresh = false;
      this.info = { tris: [], count: 0, regions: 0 };
      this.render();
    }
  }
}
