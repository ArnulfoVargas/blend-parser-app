import * as THREE from "three";
import { api, type SplitPlan, type SplitProposal, type SplitReport, type Summary, type Vec3 } from "./api";
import { inputValue, num, t } from "./i18n";
import { CutPlanes, evenCuts, type Cuts } from "./split-plan";
import "./styles/split.css";

const $ = <T extends HTMLElement>(selector: string) => document.querySelector<T>(selector)!;

/**
 * The split tool: proposes cuts so every piece fits the printer's room on a
 * plate, lets the user change how many cuts each axis gets and where, how the
 * pieces lie and whether dowels join them, and shows before applying how many
 * pieces and plates come out and anything that does not fit. The model's
 * scale is only changed by the explicit "shrink instead" link.
 */
export class SplitPanel {
  private active = false;
  private proposal: SplitProposal | null = null;
  private cuts: Cuts = [[], [], []];
  private lay = false;
  private report: SplitReport | null = null;
  private generation = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  readonly planes: CutPlanes;

  constructor(private deps: {
    summary(): Summary | null;
    box(): { min: Vec3; max: Vec3 } | null;
    split(plan: SplitPlan): Promise<boolean>;
    shrink(): void;
    close(): void;
    redraw(): void;
    error(e: unknown): string;
    colours: [THREE.Color, THREE.Color, THREE.Color];
  }) {
    this.planes = new CutPlanes(deps.colours);
    for (const axis of [0, 1, 2]) {
      $(`[data-spl-less="${axis}"]`).addEventListener("click", () => this.count(axis, -1));
      $(`[data-spl-more="${axis}"]`).addEventListener("click", () => this.count(axis, 1));
    }
    $("#spl-keep").addEventListener("click", () => { this.lay = false; this.changed(); });
    $("#spl-lay").addEventListener("click", () => { this.lay = true; this.changed(); });
    $("#spl-dowels").addEventListener("change", () => this.changed());
    for (const id of ["#spl-gap", "#spl-dsize", "#spl-ddepth", "#spl-dclear"]) {
      const input = $<HTMLInputElement>(id);
      input.addEventListener("input", () => {
        input.setAttribute("aria-invalid", String(!this.valid(input)));
        if (this.valid(input)) this.changed();
      });
    }
    $("#spl-reset").addEventListener("click", () => void this.propose());
    $("#spl-off").addEventListener("click", () => deps.close());
    $("#spl-scale").addEventListener("click", () => deps.shrink());
    $("#spl-apply").addEventListener("click", () => void this.apply());
  }

  open() {
    this.active = true;
    void this.propose();
  }

  close() {
    this.active = false;
    this.generation++;
    clearTimeout(this.timer);
    this.planes.show(null, null);
    this.deps.redraw();
  }

  /** A new model or size: the proposal is worked out again. */
  meshChanged() {
    if (this.active) void this.propose();
  }

  /** New words. */
  update() {
    if (this.active) this.render();
  }

  private valid(input: HTMLInputElement) {
    const v = input.valueAsNumber;
    return Number.isFinite(v) && v >= Number(input.min) && v <= Number(input.max);
  }

  private plan(): SplitPlan | null {
    const fields = ["#spl-gap", "#spl-dsize", "#spl-ddepth", "#spl-dclear"].map((id) => $<HTMLInputElement>(id));
    if (!fields.every((f) => this.valid(f))) return null;
    const [gap, size, depth, clearance] = fields.map((f) => f.valueAsNumber);
    const dowels = $<HTMLInputElement>("#spl-dowels").checked ? { shape: "round" as const, sizeMm: size, depthMm: depth, clearanceMm: clearance } : null;
    return { cuts: this.cuts.map((a) => [...a]) as Cuts, layOnCut: this.lay, gapMm: gap, dowels };
  }

  private async propose() {
    const gap = $<HTMLInputElement>("#spl-gap");
    if (!this.deps.summary() || !this.valid(gap)) return;
    const generation = ++this.generation;
    try {
      const proposal = await api.proposeSplit(gap.valueAsNumber);
      if (generation !== this.generation) return;
      this.proposal = proposal;
      this.cuts = proposal.cuts.map((a) => [...a]) as Cuts;
      this.changed();
    } catch (e) {
      if (generation === this.generation) this.summary(this.deps.error(e), "error");
    }
  }

  /** One more or one fewer cut along `axis`, spread evenly again. */
  private count(axis: number, step: number) {
    const box = this.deps.box();
    if (!box) return;
    const n = Math.max(0, Math.min(20, this.cuts[axis].length + step));
    this.cuts[axis] = evenCuts(box.min[axis], box.max[axis], n);
    this.changed();
  }

  private changed() {
    this.report = null;
    this.render();
    this.generation++;
    clearTimeout(this.timer);
    const plan = this.plan();
    if (!this.active || !plan) return;
    if (!this.cuts.some((a) => a.length)) return;
    this.summary(t("spl.checking"), "pending");
    const generation = this.generation;
    this.timer = setTimeout(async () => {
      try {
        const report = await api.previewSplit(plan);
        if (generation !== this.generation) return;
        this.report = report;
        this.render();
      } catch (e) {
        if (generation === this.generation) this.summary(this.deps.error(e), "error");
      }
    }, 450);
  }

  private async apply() {
    const plan = this.plan();
    if (!plan || !this.report?.fits) return;
    if (await this.deps.split(plan)) this.report = null;
  }

  private summary(text: string, state: "pending" | "valid" | "error" | "warn" | "info") {
    const el = $("#spl-summary");
    el.textContent = text;
    el.dataset.state = state;
  }

  private render() {
    const s = this.deps.summary();
    const scale = s?.size.previewScale ?? 1;
    const box = this.deps.box();
    const p = this.proposal;
    const dims = (v: number[]) => `${v.map((x) => num(x, 0)).join(" × ")} mm`;
    $("#spl-room").textContent = p ? t("spl.room", { model: dims(p.sizeMm), room: dims([...p.roomMm, p.heightMm]) }) : "";
    $("#spl-lay").setAttribute("aria-pressed", String(this.lay));
    $("#spl-keep").setAttribute("aria-pressed", String(!this.lay));
    $("#spl-dowel-fields").hidden = !$<HTMLInputElement>("#spl-dowels").checked;
    // Each cut as millimetres from the model's low end along its axis, editable.
    for (const axis of [0, 1, 2]) {
      $(`[data-spl-count="${axis}"]`).textContent = String(this.cuts[axis].length);
      $<HTMLButtonElement>(`[data-spl-less="${axis}"]`).disabled = this.cuts[axis].length === 0;
      const list = $(`[data-spl-at="${axis}"]`);
      const lo = box?.min[axis] ?? 0;
      list.replaceChildren(...this.cuts[axis].map((value, i) => {
        const input = Object.assign(document.createElement("input"), { type: "number", step: "1", className: "spl-at" });
        input.value = inputValue((value - lo) * scale, 1);
        input.setAttribute("aria-label", t("spl.cutAt", { axis: "XYZ"[axis], n: i + 1 }));
        input.addEventListener("change", () => {
          const mm = input.valueAsNumber;
          const span = box ? (box.max[axis] - lo) * scale : Infinity;
          const ok = Number.isFinite(mm) && mm > 0 && mm < span;
          input.setAttribute("aria-invalid", String(!ok));
          if (!ok) return;
          this.cuts[axis][i] = lo + mm / scale;
          this.cuts[axis].sort((a, b) => a - b);
          this.changed();
        });
        return input;
      }));
    }
    this.planes.show(this.active ? this.cuts : null, box);
    this.deps.redraw();
    const r = this.report;
    const rows = $("#spl-rows");
    $("#spl-table").hidden = !r;
    const apply = $<HTMLButtonElement>("#spl-apply");
    apply.disabled = !r?.fits;
    apply.textContent = r ? t("spl.applyN", { count: r.pieces.length, n: r.pieces.length }) : t("spl.apply");
    if (!this.cuts.some((a) => a.length)) {
      rows.replaceChildren();
      return this.summary(p && p.cuts.every((a) => !a.length) ? t("spl.fitsAlready") : t("spl.noCuts"), "info");
    }
    if (!r) return;
    rows.replaceChildren(...r.pieces.map((piece) => {
      const row = document.createElement("tr");
      row.dataset.bad = String(piece.plate === null);
      const name = Object.assign(document.createElement("th"), { scope: "row", textContent: piece.name });
      const size = Object.assign(document.createElement("td"), { className: "num", textContent: `${piece.sizeMm.map((v) => num(v, 1)).join(" × ")} mm` });
      const plate = Object.assign(document.createElement("td"), { className: "num", textContent: piece.plate === null ? "—" : String(piece.plate + 1) });
      const notes = [piece.plate === null && t("spl.noFit"), piece.dowel && t("spl.dowel"), piece.laid && t("spl.laid"), piece.small && t("spl.small")].filter(Boolean) as string[];
      const note = Object.assign(document.createElement("td"), { textContent: notes.join(" · ") });
      row.append(name, size, plate, note);
      return row;
    }));
    const parts = [
      t("spl.pieces", { count: r.pieces.length, n: r.pieces.length }),
      t("spl.plates", { count: r.plates, n: r.plates }),
      t("spl.cutsDone", { count: r.cuts, n: r.cuts }),
      r.joints ? t("spl.joints", { count: r.joints, n: r.joints }) : "",
    ].filter(Boolean).join(" · ");
    const problems = [
      !r.fits && t("spl.someDoNotFit", { count: r.pieces.filter((x) => x.plate === null).length, n: r.pieces.filter((x) => x.plate === null).length }),
      r.jointsMissing > 0 && t("spl.jointsMissing", { count: r.jointsMissing, n: r.jointsMissing }),
      r.pieces.some((x) => x.small) && t("spl.smallWarn"),
    ].filter(Boolean) as string[];
    this.summary([parts + ".", ...problems].join(" "), !r.fits ? "error" : problems.length ? "warn" : "valid");
  }
}
