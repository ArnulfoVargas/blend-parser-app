import type { Summary, Vec3 } from "./api";
import { inputValue, num, t } from "./i18n";
import { sectionNormal, type Section } from "./viewer";
import "./styles/cut-controls.css";

const $ = <T extends HTMLElement>(selector: string) => document.querySelector<T>(selector)!;

/** Exact values below the viewport; all spatial manipulation stays in the view. */
export class CutPanel {
  private scale = 1;
  private position = $<HTMLInputElement>("#cut-pos");
  private target = $<HTMLSelectElement>("#cut-target");
  private fields: { kind: "point" | "rotation"; axis: number; input: HTMLInputElement }[];

  constructor(private actions: {
    current(): Section;
    set(section: Section): void;
    reset(): Section;
    range(section: Section): [number, number];
    snap(degrees: number): void;
  }) {
    this.fields = (["point", "rotation"] as const).flatMap((kind) => [0, 1, 2].map((axis) => {
      const input = $<HTMLInputElement>(`#cut-${kind === "rotation" ? "r" : ""}${"xyz"[axis]}`);
      input.addEventListener("input", () => {
        if (!input.value || !Number.isFinite(input.valueAsNumber)) return;
        const section = actions.current();
        section[kind][axis] = kind === "point" ? input.valueAsNumber / this.scale : input.valueAsNumber % 360;
        actions.set(section);
      });
      input.addEventListener("blur", () => this.sync(actions.current()));
      input.addEventListener("keydown", (e) => { if (e.key === "Enter") input.blur(); });
      return { kind, axis, input };
    }));
    this.position.addEventListener("input", () => {
      const s = actions.current();
      const normal = sectionNormal(s);
      const [lo, hi] = actions.range(s);
      const delta = lo + (hi - lo) * Number(this.position.value) / 100 - s.point.reduce((sum, v, i) => sum + v * normal[i], 0);
      actions.set({ ...s, point: s.point.map((v, i) => v + normal[i] * delta) as Vec3 });
    });
    document.querySelectorAll<HTMLButtonElement>("[data-cut-axis]").forEach((button) => button.addEventListener("click", () => {
      const axis = Number(button.dataset.cutAxis);
      const rotation: Vec3 = axis === 3 ? [0, 45, 0] : axis === 0 ? [0, 90, 0] : axis === 1 ? [-90, 0, 0] : [0, 0, 0];
      actions.set({ ...actions.current(), rotation });
    }));
    $("#cut-reset").addEventListener("click", () => actions.set({ ...actions.reset(), selected: actions.current().selected }));
    const snap = $<HTMLSelectElement>("#cut-snap");
    snap.addEventListener("change", () => actions.snap(Number(snap.value)));
    actions.snap(Number(snap.value));
    this.target.addEventListener("change", () => {
      const selected = Array.from(this.target.selectedOptions).filter((o) => o.value !== "all").map((o) => Number(o.value));
      actions.set({ ...actions.current(), selected });
    });
    $("#cut-flip").addEventListener("click", () => actions.set({ ...actions.current(), flip: !actions.current().flip }));
    document.addEventListener("click", (e) => {
      const parts = $<HTMLDetailsElement>("#cut-parts");
      if (parts.open && !parts.contains(e.target as Node)) parts.open = false;
    });
  }

  update(summary: Summary) {
    this.scale = summary.size.previewScale;
    const selected = this.actions.current().selected;
    const signature = JSON.stringify(summary.parts.map((p) => p.name));
    if (this.target.dataset.parts !== signature) {
      this.target.dataset.parts = signature;
      this.target.replaceChildren(new Option(t("cut.all"), "all", false, selected.length === 0),
        ...summary.parts.map((part, i) => new Option(part.name, String(i), false, selected.includes(i))));
    } else this.target.options[0].textContent = t("cut.all");
    this.sync(this.actions.current());
  }

  sync(section: Section | null) {
    if (!section) return;
    const normal = sectionNormal(section);
    const [lo, hi] = this.actions.range(section);
    const distance = section.point.reduce((sum, v, i) => sum + v * normal[i], 0);
    this.position.value = String(100 * (distance - lo) / Math.max(1e-6, hi - lo));
    $("#cut-depth-value").textContent = `${num((distance - lo) * this.scale, 1)} mm`;
    this.fields.forEach(({ kind, axis, input }) => {
      const value = section[kind][axis] * (kind === "point" ? this.scale : 1);
      if (document.activeElement !== input) input.value = inputValue(value, 4);
    });
    document.querySelectorAll<HTMLButtonElement>("[data-cut-axis]").forEach((button) => {
      const axis = Number(button.dataset.cutAxis);
      const diagonal = Math.abs(normal[0] - Math.SQRT1_2) < 1e-5 && Math.abs(normal[2] - Math.SQRT1_2) < 1e-5;
      button.setAttribute("aria-pressed", String(axis === 3 ? diagonal : normal[axis] > 1 - 1e-6));
    });
    $("#cut-flip").setAttribute("aria-pressed", String(section.flip));
    $("#cut-side").textContent = t(section.flip ? "cut.positive" : "cut.negative");
    $("#cut-target-count").textContent = section.selected.length ? String(section.selected.length) : t("cut.allShort");
    for (const option of this.target.options) option.selected = section.selected.length
      ? option.value !== "all" && section.selected.includes(Number(option.value)) : option.value === "all";
  }
}
