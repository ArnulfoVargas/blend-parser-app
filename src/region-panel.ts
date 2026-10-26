import { api, type Closure, type ColourMatch, type PaintResult, type RegionAction, type RegionPreview, type Selection, type Summary, type Vec3 } from "./api";
import { num, t, volume } from "./i18n";
import "./styles/region.css";

const $ = <T extends HTMLElement>(selector: string) => document.querySelector<T>(selector)!;
/** The selection's highlight, mixed over the filament so the painting still shows under it. */
const PICKED: Vec3 = [214, 51, 132];
const PICKED_MIX = 0.55;
/** While the pieces are pulled apart, what stays where it was fades out. */
const GHOST: Vec3 = [236, 236, 232];

/**
 * The region tool: a brush, or a click that picks every face of a colour,
 * selects part of the surface without painting it; tools reshape that
 * selection, and the choice of what to do with it
 * (remove it or extract it) and how to close what is left (a shell of a given
 * thickness or a cap at a given depth). The result is previewed before it can
 * be applied: the closing faces in the warning colour and, if asked, the
 * pieces pulled apart.
 */
export class RegionPanel {
  private active = false;
  private add = true;
  private alt = false;
  private through = false;
  private action: "remove" | "extract" = "extract";
  private layFlat = false;
  private byColour = false;
  /** The last colour pick, to try it again when its criteria change: the face, how it added, and the selection before it. */
  private lastPick: { seed: number; add: boolean; before: number[] } | null = null;
  private picks = 0;
  private picked = new Set<number>();
  private info: Selection = { tris: [], count: 0, regions: 0 };
  private preview: RegionPreview | null = null;
  /** The preview matches the selection and the settings as they are. */
  private fresh = false;
  private generation = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private mm = $<HTMLInputElement>("#reg-mm");
  private closure = $<HTMLSelectElement>("#reg-closure");
  private pull = $<HTMLInputElement>("#reg-pull");
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
    /** Makes the colour borders that cross triangles edges of the mesh. */
    refine(): Promise<void>;
    close(): void;
    error(e: unknown): string;
  }) {
    const seg = (on: string, off: string, set: (v: boolean) => void) => {
      $(on).addEventListener("click", () => { set(true); this.changed(); });
      $(off).addEventListener("click", () => { set(false); this.changed(); });
    };
    seg("#reg-add", "#reg-erase", (v) => (this.add = v));
    seg("#reg-through", "#reg-visible", (v) => (this.through = v));
    seg("#reg-extract", "#reg-remove", (v) => (this.action = v ? "extract" : "remove"));
    seg("#reg-flat", "#reg-assembled", (v) => (this.layFlat = v));
    this.closure.addEventListener("change", () => this.changed());
    this.mm.addEventListener("input", () => this.changed());
    this.clearance.addEventListener("input", () => this.changed());
    this.group.addEventListener("change", () => this.changed());
    seg("#reg-by-colour", "#reg-by-brush", (v) => {
      this.byColour = v;
      deps.picking(v);
    });
    // A new criterion, tolerance or reach tries the last pick again on the selection it started from.
    for (const el of [this.match, this.tolerance, this.contiguous]) el.addEventListener("change", () => void this.repick());
    this.tolerance.addEventListener("input", () => void this.repick());
    $("#reg-refine-apply").addEventListener("click", () => void deps.refine());
    this.pull.addEventListener("change", () => this.show());
    document.querySelectorAll<HTMLButtonElement>("[data-reg-op]").forEach((b) =>
      b.addEventListener("click", () => void this.op(b.dataset.regOp as "grow" | "connected" | "smooth" | "invert" | "clear")));
    $("#reg-off").addEventListener("click", () => deps.close());
    $("#reg-apply").addEventListener("click", () => void this.apply());
    // Alt held turns the brush into an eraser for as long as it is held.
    const onAlt = (e: KeyboardEvent) => {
      if (e.key !== "Alt" || this.alt === e.altKey) return;
      this.alt = e.altKey;
      this.render();
    };
    window.addEventListener("keydown", onAlt);
    window.addEventListener("keyup", onAlt);
    window.addEventListener("blur", () => { this.alt = false; this.render(); });
  }

  open() {
    this.active = true;
    void this.refresh();
  }

  close() {
    this.active = false;
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

  /** A click on the view while picking by colour: the faces like the one under it, added (Ctrl or Shift) or instead of the selection; Alt takes them out. */
  async click(triangle: number | null, additive: boolean) {
    if (!this.byColour || triangle === null) return;
    const add = !this.alt;
    const before = additive || !add ? [...this.picked] : [];
    this.lastPick = { seed: triangle, add, before };
    await this.pick();
  }

  private how(): ColourMatch | null {
    if (this.match.value !== "similar") return { kind: this.match.value as "material" | "filament" };
    const deltaE = this.tolerance.valueAsNumber;
    const ok = Number.isFinite(deltaE) && deltaE >= 0 && deltaE <= 100;
    this.tolerance.setAttribute("aria-invalid", String(!ok));
    return ok ? { kind: "similar", deltaE } : null;
  }

  private async repick() {
    this.render();
    if (this.lastPick) await this.pick();
  }

  private async pick() {
    const p = this.lastPick;
    const how = this.how();
    if (!p || !how) return;
    // Only the last of several quick tries counts.
    const token = ++this.picks;
    try {
      const got = await api.selectByColour(p.seed, how, this.contiguous.checked, p.add, p.before);
      if (token === this.picks) this.got(got);
    } catch (e) {
      if (token === this.picks) this.status(this.deps.error(e), "error");
    }
  }

  /** The brush was lifted: count the regions again and preview the cut. */
  endStroke() {
    void this.refresh();
  }

  private async op(op: "grow" | "connected" | "smooth" | "invert" | "clear") {
    this.lastPick = null;
    try {
      this.got(await api.selectionOp(op));
    } catch (e) {
      this.status(this.deps.error(e), "error");
    }
  }

  private async refresh() {
    if (!this.active || !this.deps.summary()) return;
    try {
      this.got(await api.selectionOp("get"));
    } catch (e) {
      this.status(this.deps.error(e), "error");
    }
  }

  private got(info: Selection) {
    this.info = info;
    this.picked = new Set(info.tris);
    this.changed();
  }

  private settings(): { closure: Closure; action: RegionAction } | null {
    const mm = this.mm.valueAsNumber;
    const shell = this.closure.value === "shell";
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
    this.deps.overlay(null, 0);
    this.render();
    clearTimeout(this.timer);
    if (this.active && this.picked.size) this.timer = setTimeout(() => void this.previewNow(), 250);
  }

  private async previewNow() {
    const s = this.settings();
    if (!s || !this.picked.size) return this.render();
    const generation = ++this.generation;
    this.status(t("reg.previewing"), "pending");
    try {
      const preview = await api.previewRegion(s.closure, s.action);
      if (generation !== this.generation) return;
      this.preview = preview;
      this.fresh = true;
    } catch (e) {
      if (generation !== this.generation) return;
      this.preview = null;
      this.fresh = false;
      this.render();
      this.status(this.deps.error(e), "error");
      return;
    }
    this.render();
  }

  private tint() {
    const n = this.deps.summary()?.triangles ?? 0;
    if (!this.picked.size || !n) return this.deps.tint(null, 1);
    const pulled = this.fresh && this.pull.checked && this.action === "extract";
    const colors: (Vec3 | null)[] = new Array(n).fill(null);
    for (const id of this.picked) if (id < n) colors[id] = pulled ? GHOST : PICKED;
    this.deps.tint(colors, pulled ? 0.85 : PICKED_MIX);
  }

  /** The overlay and the highlight for the preview as it stands. */
  private show() {
    const s = this.deps.summary();
    const span = s ? Math.max(...s.sizeMm) / (s.size.previewScale || 1) : 0;
    const pull = this.pull.checked && this.action === "extract" ? span * 0.3 : 0;
    this.deps.overlay(this.fresh ? this.preview : null, pull);
    this.tint();
  }

  private render() {
    if (!this.active) return;
    const erase = this.add === this.alt;
    $("#reg-add").setAttribute("aria-pressed", String(!erase));
    $("#reg-erase").setAttribute("aria-pressed", String(erase));
    $("#reg-visible").setAttribute("aria-pressed", String(!this.through));
    $("#reg-through").setAttribute("aria-pressed", String(this.through));
    $("#reg-remove").setAttribute("aria-pressed", String(this.action === "remove"));
    $("#reg-extract").setAttribute("aria-pressed", String(this.action === "extract"));
    $("#reg-assembled").setAttribute("aria-pressed", String(!this.layFlat));
    $("#reg-flat").setAttribute("aria-pressed", String(this.layFlat));
    $("#reg-place").hidden = this.action !== "extract";
    $("#reg-pull-row").hidden = this.action !== "extract";
    $("#reg-group-row").hidden = this.action !== "extract";
    const shell = this.closure.value === "shell";
    $("#reg-clearance-row").hidden = !shell || this.action !== "extract";
    $("#reg-by-brush").setAttribute("aria-pressed", String(!this.byColour));
    $("#reg-by-colour").setAttribute("aria-pressed", String(this.byColour));
    document.querySelectorAll<HTMLElement>("[data-reg-brush]").forEach((el) => (el.hidden = this.byColour));
    document.querySelectorAll<HTMLElement>("[data-reg-colour]").forEach((el) => (el.hidden = !this.byColour));
    $("#reg-tolerance-row").hidden = !this.byColour || this.match.value !== "similar";
    // Borders inside triangles only matter when picking by filament.
    const split = this.deps.summary()?.splitTriangles ?? 0;
    $("#reg-refine").hidden = !this.byColour || this.match.value !== "filament" || split === 0;
    $("#reg-refine-note").textContent = t("reg.refineNote", { count: split, n: num(split) });
    $("#reg-mm-label").textContent = t(shell ? "reg.thickness" : "reg.depth");
    this.mm.min = shell ? "0.1" : "-50";
    this.mm.title = t(shell ? "reg.thicknessHelp" : "reg.depthHelp");
    const count = this.picked.size;
    $("#reg-count").textContent = count
      ? `${t("reg.count", { count: this.info.count, n: num(this.info.count) })} · ${t("reg.regions", { count: this.info.regions, n: num(this.info.regions) })}`
      : "";
    document.querySelectorAll<HTMLButtonElement>("[data-reg-op]").forEach((b) => (b.disabled = !count && b.dataset.regOp !== "invert"));
    const apply = $<HTMLButtonElement>("#reg-apply");
    apply.disabled = !this.fresh || !this.preview;
    apply.textContent = t(this.action === "remove" ? "reg.applyRemove" : "reg.applyExtract");
    this.show();
    if (!count) return this.status(t("reg.empty"), "info");
    const p = this.preview;
    if (!this.fresh || !p) return this.status(t("reg.previewing"), "pending");
    const pieces = p.pieceMm3.reduce((a, b) => a + b, 0);
    const parts = [
      this.action === "remove"
        ? t("reg.removes", { volume: volume(pieces), rest: volume(p.restMm3) })
        : t("reg.extracts", { count: p.pieceMm3.length, n: num(p.pieceMm3.length), volume: volume(pieces), rest: volume(p.restMm3) }),
      p.closure.length ? t("reg.closureShown") : "",
      p.closed ? "" : t("reg.open"),
      p.small ? t("reg.small", { count: p.small, n: num(p.small) }) : "",
    ];
    this.status(parts.filter(Boolean).join(" "), p.closed && !p.small ? "valid" : "warn");
  }

  private status(text: string, state: "pending" | "valid" | "error" | "warn" | "info") {
    const el = $("#reg-status");
    el.textContent = text;
    el.dataset.state = state;
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
