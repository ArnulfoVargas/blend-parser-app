import { api, type CutProposal, type Piece, type Pieces, type Summary, type Vec3 } from "./api";
import { num, t } from "./i18n";
import { compact, objectColour } from "./separate-plan";
import "./styles/separate.css";

const $ = <T extends HTMLElement>(selector: string) => document.querySelector<T>(selector)!;
const NONE = 2 ** 32 - 1;
/** Pieces left in the model show in the filament's place as a quiet grey. */
const STAYS: Vec3 = [172, 176, 172];

/**
 * The separation tool: lists the disconnected pieces found in the model,
 * lets the user group them into objects or leave them in the model, and
 * colours each piece in the view by the object it will become. Continuous
 * meshes only get cut proposals, reviewed and confirmed in the Section tool.
 */
export class SeparatePanel {
  private active = false;
  private pieces: Piece[] = [];
  private triPiece: number[] = [];
  private groups: (number | null)[] = [];
  private picked = new Set<number>();
  private flat = false;
  private generation = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private proposals: CutProposal[] = [];
  private searched = false;
  private weld = $<HTMLInputElement>("#sep-weld");
  private rows = $<HTMLTableSectionElement>("#sep-rows");

  constructor(private deps: {
    summary(): Summary | null;
    tint(colors: (Vec3 | null)[] | null): void;
    separate(weldMm: number, groups: (number | null)[], layFlat: boolean): Promise<boolean>;
    review(proposal: CutProposal): void;
    close(): void;
    error(e: unknown): string;
  }) {
    this.weld.addEventListener("input", () => {
      clearTimeout(this.timer);
      this.timer = setTimeout(() => this.detect(), 350);
    });
    $("#sep-detect").addEventListener("click", () => this.detect());
    $<HTMLInputElement>("#sep-all").addEventListener("change", (e) => {
      this.picked = (e.target as HTMLInputElement).checked ? new Set(this.pieces.keys()) : new Set();
      this.render();
    });
    $("#sep-group").addEventListener("click", () => this.assign("group"));
    $("#sep-split").addEventListener("click", () => this.assign("each"));
    $("#sep-keep").addEventListener("click", () => this.assign("keep"));
    for (const [id, flat] of [["#sep-assembled", false], ["#sep-flat", true]] as const) {
      $(id).addEventListener("click", () => { this.flat = flat; this.render(); });
    }
    $("#sep-off").addEventListener("click", () => deps.close());
    $("#sep-apply").addEventListener("click", () => void this.apply());
    $("#sep-propose").addEventListener("click", () => void this.propose());
    document.addEventListener("click", (e) => {
      const menu = $<HTMLDetailsElement>("#sep-proposals");
      if (menu.open && !menu.contains(e.target as Node)) menu.open = false;
    });
    // Opening the list looks for proposals the first time.
    $("#sep-proposals").addEventListener("toggle", () => {
      if ($<HTMLDetailsElement>("#sep-proposals").open && !this.searched) void this.propose();
    });
  }

  open() {
    this.active = true;
    this.detect();
  }

  close() {
    this.active = false;
    $<HTMLDetailsElement>("#sep-proposals").open = false;
    this.generation++;
    clearTimeout(this.timer);
    this.deps.tint(null);
  }

  /** A new preview mesh: the pieces and their triangles are read again. */
  meshChanged() {
    this.searched = false;
    this.proposals = [];
    this.renderProposals();
    $("#sep-propose-status").textContent = "";
    if (this.active) this.detect();
  }

  /** Words and colours again, for a new language or palette. */
  update() {
    if (this.active) this.render();
    this.renderProposals();
  }

  /** A click on the view: picks the piece under it; with Ctrl or Shift, adds it. */
  click(triangle: number | null, additive: boolean) {
    const k = triangle === null ? NONE : this.triPiece[triangle] ?? NONE;
    if (k === NONE) {
      if (!additive) this.picked.clear();
    } else if (additive) {
      if (!this.picked.delete(k)) this.picked.add(k);
    } else {
      this.picked = this.picked.size === 1 && this.picked.has(k) ? new Set() : new Set([k]);
    }
    this.render();
    if (k !== NONE) this.rows.children[k]?.scrollIntoView({ block: "nearest" });
  }

  private weldMm(): number | null {
    const v = this.weld.valueAsNumber;
    const ok = Number.isFinite(v) && v >= 0 && v <= 5;
    this.weld.setAttribute("aria-invalid", String(!ok));
    return ok ? v : null;
  }

  private detect() {
    const weld = this.weldMm();
    if (!this.active || weld === null || !this.deps.summary()) {
      if (weld === null) this.status(t("sep.weldRange"), "error");
      return;
    }
    const generation = ++this.generation;
    this.status(t("sep.detecting"), "pending");
    api.detectPieces(weld).then((found) => {
      if (generation !== this.generation) return;
      this.found(found);
    }, (e) => generation === this.generation && this.status(this.deps.error(e), "error"));
  }

  private found(found: Pieces) {
    const same = found.pieces.length === this.pieces.length && found.pieces.every((p, i) => p.part === this.pieces[i].part && p.triangles === this.pieces[i].triangles);
    this.pieces = found.pieces;
    this.triPiece = found.triPiece;
    // Each piece its own object, unless the user already chose for these same pieces.
    if (!same) {
      this.groups = this.pieces.map((_, i) => i);
      this.picked.clear();
    }
    this.render();
  }

  private assign(how: "group" | "each" | "keep") {
    if (!this.picked.size) return;
    const picked = [...this.picked].sort((a, b) => a - b);
    const free = () => Math.max(-1, ...this.groups.map((g) => g ?? -1)) + 1;
    if (how === "keep") for (const k of picked) this.groups[k] = null;
    else if (how === "group") {
      const into = picked.map((k) => this.groups[k]).find((g) => g !== null) ?? free();
      for (const k of picked) this.groups[k] = into;
    } else for (const k of picked) this.groups[k] = free();
    this.groups = compact(this.groups);
    this.render();
  }

  /** New objects and pieces that stay, as the apply button will make them. */
  private plan() {
    const objects = new Set(this.groups.filter((g) => g !== null)).size;
    const stay = this.groups.filter((g) => g === null).length;
    return { objects, stay };
  }

  private render() {
    if (!this.active) return;
    const many = new Set(this.pieces.map((p) => p.part)).size > 1;
    const options = (k: number) => [new Option(t("sep.stays"), ""), ...this.pieces.map((_, g) => new Option(t("sep.object", { n: g + 1 }), String(g)))]
      .map((o) => { o.selected = o.value === (this.groups[k] === null ? "" : String(this.groups[k])); return o; });
    this.rows.replaceChildren(...this.pieces.map((p, k) => {
      const row = document.createElement("tr");
      row.dataset.picked = String(this.picked.has(k));
      const pick = Object.assign(document.createElement("input"), { type: "checkbox", checked: this.picked.has(k) });
      pick.setAttribute("aria-label", t("sep.pick", { n: k + 1 }));
      pick.addEventListener("change", () => { if (pick.checked) this.picked.add(k); else this.picked.delete(k); this.render(); });
      const name = document.createElement("th");
      name.scope = "row";
      const swatch = document.createElement("i");
      swatch.className = "sep-swatch";
      swatch.style.background = `rgb(${this.colour(k).join(" ")})`;
      const label = Object.assign(document.createElement("span"), { textContent: t("sep.pieceName", { n: k + 1 }) });
      name.append(swatch, label);
      if (many) name.append(Object.assign(document.createElement("small"), { textContent: p.partName }));
      const size = Object.assign(document.createElement("td"), { className: "num", textContent: `${p.sizeMm.map((v) => num(v, 1)).join(" × ")} mm` });
      size.title = t("sep.triangles", { count: p.triangles, n: num(p.triangles) });
      const surface = document.createElement("td");
      surface.append(Object.assign(document.createElement("span"), {
        className: `sep-chip${p.closed ? "" : " is-open"}`,
        textContent: t(p.closed ? "sep.closed" : "sep.open"),
        title: t(p.closed ? "sep.closedHelp" : "sep.openHelp"),
      }));
      const goes = document.createElement("td");
      const select = Object.assign(document.createElement("select"), { className: "sep-goes" });
      select.setAttribute("aria-label", t("sep.goesFor", { n: k + 1 }));
      select.append(...options(k));
      select.addEventListener("change", () => {
        this.groups[k] = select.value === "" ? null : Number(select.value);
        this.groups = compact(this.groups);
        this.render();
      });
      goes.append(select);
      row.append(Object.assign(document.createElement("td"), { className: "sep-pick" }), name, size, surface, goes);
      row.firstElementChild!.append(pick);
      row.addEventListener("click", (e) => {
        if ((e.target as HTMLElement).closest("input, select")) return;
        if (!this.picked.delete(k)) this.picked.add(k);
        this.render();
      });
      return row;
    }));
    const all = $<HTMLInputElement>("#sep-all");
    all.checked = this.pieces.length > 0 && this.picked.size === this.pieces.length;
    all.indeterminate = this.picked.size > 0 && !all.checked;
    $("#sep-picked").textContent = this.picked.size ? t("sep.picked", { count: this.picked.size, n: this.picked.size }) : t("sep.pickHelp");
    for (const id of ["#sep-group", "#sep-split", "#sep-keep"]) $<HTMLButtonElement>(id).disabled = this.picked.size === 0;
    $<HTMLButtonElement>("#sep-group").disabled = this.picked.size < 2;
    $("#sep-assembled").setAttribute("aria-pressed", String(!this.flat));
    $("#sep-flat").setAttribute("aria-pressed", String(this.flat));
    const { objects, stay } = this.plan();
    $("#sep-count").textContent = t("sep.count", { count: this.pieces.length, n: this.pieces.length });
    const open = this.pieces.filter((p) => !p.closed).length;
    const ready = this.pieces.length > 1 && objects > 0 && !(objects === 1 && stay === 0 && new Set(this.pieces.map((p) => p.object)).size === 1);
    $<HTMLButtonElement>("#sep-apply").disabled = !ready;
    $<HTMLButtonElement>("#sep-apply").textContent = objects ? t("sep.applyN", { count: objects, n: objects }) : t("sep.apply");
    if (this.pieces.length < 2) this.status(t("sep.single"), "info");
    else if (!ready) this.status(t("sep.nothing"), "info");
    else this.status([t("sep.plan", { objects, stay }), open ? t("sep.openWarn", { count: open, n: open }) : ""].filter(Boolean).join(" "), open ? "warn" : "valid");
    this.deps.tint(this.triPiece.map((k) => (k === NONE ? null : this.colour(k))));
  }

  /** The piece's colour in the view: its new object's, or grey when it stays; dimmed outside the selection. */
  private colour(k: number): Vec3 {
    const g = this.groups[k];
    const base = g === null || g === undefined ? STAYS : objectColour(g);
    if (!this.picked.size || this.picked.has(k)) return base;
    return base.map((v) => Math.round(v * 0.45 + 214 * 0.55)) as Vec3;
  }

  private status(text: string, state: "pending" | "valid" | "error" | "warn" | "info") {
    const el = $("#sep-status");
    el.textContent = text;
    el.dataset.state = state;
  }

  private async apply() {
    const weld = this.weldMm();
    if (weld === null) return;
    if (await this.deps.separate(weld, this.groups, this.flat)) this.picked.clear();
  }

  private async propose() {
    const status = $("#sep-propose-status");
    status.textContent = t("sep.proposing");
    this.searched = true;
    try {
      this.proposals = await api.proposeCuts([]);
      status.textContent = this.proposals.length ? t("sep.proposed", { count: this.proposals.length, n: this.proposals.length }) : t("sep.noProposals");
    } catch (e) {
      this.proposals = [];
      status.textContent = this.deps.error(e);
    }
    this.renderProposals();
  }

  private renderProposals() {
    const s = this.deps.summary();
    const k = s?.size.previewScale ?? 1;
    const list = $("#sep-proposal-list");
    list.replaceChildren(...this.proposals.map((p) => {
      const li = document.createElement("li");
      const where = t("sep.at", { axis: "XYZ"[p.axis], mm: num(p.point[p.axis] * k, 1) });
      const text = document.createElement("p");
      const kind = Object.assign(document.createElement("strong"), { textContent: t(p.signal.kind === "neck" ? "sep.neck" : "sep.colour") });
      text.append(kind, ` · ${where}. `);
      if (p.signal.kind === "neck") {
        text.append(t("sep.neckWhy", { area: num(p.signal.areaMm2, 0), before: num(p.signal.beforeMm2, 0), after: num(p.signal.afterMm2, 0) }));
      } else {
        const sw = (slot: number) => {
          const i = Object.assign(document.createElement("i"), { className: "sep-swatch" });
          i.style.background = s?.slots[slot]?.hex ?? "gray";
          return i;
        };
        text.append(sw(p.signal.from), t("sep.colourWhy", { from: p.signal.from + 1, to: p.signal.to + 1 }), sw(p.signal.to));
      }
      if ((s?.parts.length ?? 0) > 1) text.append(Object.assign(document.createElement("small"), { textContent: ` ${s!.parts[p.part]?.name ?? ""}` }));
      const review = Object.assign(document.createElement("button"), { type: "button", className: "btn btn--quiet btn--sm", textContent: t("sep.review") });
      review.addEventListener("click", () => this.deps.review(p));
      li.append(text, review);
      return li;
    }));
  }
}
