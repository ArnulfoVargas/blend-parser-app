import * as THREE from "three";
import { api, type ConnectReview, type Summary, type Vec3 } from "./api";
import { ConnectorPreview, reach, type ConnectorSpec } from "./connect-preview";
import { inputValue, num, t } from "./i18n";
import { sectionNormal, type Section } from "./viewer";
import "./styles/connect.css";

const $ = <T extends HTMLElement>(selector: string) => document.querySelector<T>(selector)!;

/** The plane's own axes, as its sheet in the view shows them; connectors are kept in these. */
function sheetAxes(s: Section): [THREE.Vector3, THREE.Vector3] {
  const q = new THREE.Quaternion().setFromEuler(new THREE.Euler(...s.rotation.map(THREE.MathUtils.degToRad) as Vec3, "XYZ"));
  return [new THREE.Vector3(1, 0, 0).applyQuaternion(q), new THREE.Vector3(0, 1, 0).applyQuaternion(q)];
}

/** Field limits, the same the backend checks. */
const LIMITS = { size: [1, 50], depth: [1, 100], clearance: [0, 2], angle: [-360, 360] } as const;

/**
 * The connectors tool: the cut plane with pins and sockets or loose dowels
 * placed on it by clicking, every one checked by the backend (inside the
 * section, apart from the others, with walls round its socket) before the
 * cut is offered.
 */
export class ConnectPanel {
  private active = false;
  private spec: ConnectorSpec = { kind: "pin", pinsOnA: true, shape: "round", sizeMm: 5, depthMm: 6, clearanceMm: 0.2, angleDeg: 0 };
  /** Connector centres in the plane's own axes, millimetres from its point: they follow the plane. */
  private points: [number, number][] = [];
  private issues: (string | null)[] = [];
  private objects: number | null = null;
  private lay = false;
  private generation = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private running = false;
  private again = false;
  readonly preview: ConnectorPreview;
  private fields: { key: "sizeMm" | "depthMm" | "clearanceMm" | "angleDeg"; input: HTMLInputElement; limits: readonly [number, number] }[];

  constructor(private deps: {
    summary(): Summary | null;
    section(): Section | null;
    cut(points: Vec3[], spec: ConnectorSpec, lay: boolean, selected: number[]): Promise<boolean>;
    close(): void;
    redraw(): void;
    error(e: unknown): string;
    colours: ConstructorParameters<typeof ConnectorPreview>[0];
  }) {
    this.preview = new ConnectorPreview(deps.colours);
    const seg = (name: string, read: () => string, write: (v: string) => void) => {
      document.querySelectorAll<HTMLButtonElement>(`[data-con-${name}]`).forEach((b) => b.addEventListener("click", () => {
        write(b.dataset[`con${name[0].toUpperCase()}${name.slice(1)}`]!);
        this.changed();
      }));
      return read;
    };
    seg("kind", () => this.spec.kind, (v) => (this.spec.kind = v as ConnectorSpec["kind"]));
    seg("shape", () => this.spec.shape, (v) => (this.spec.shape = v as ConnectorSpec["shape"]));
    seg("pins", () => (this.spec.pinsOnA ? "a" : "b"), (v) => (this.spec.pinsOnA = v === "a"));
    this.fields = ([["sizeMm", "#con-size", LIMITS.size], ["depthMm", "#con-depth", LIMITS.depth], ["clearanceMm", "#con-clearance", LIMITS.clearance], ["angleDeg", "#con-angle", LIMITS.angle]] as const)
      .map(([key, id, limits]) => {
        const input = $<HTMLInputElement>(id);
        input.addEventListener("input", () => {
          const value = input.valueAsNumber;
          const ok = Number.isFinite(value) && value >= limits[0] && value <= limits[1];
          input.setAttribute("aria-invalid", String(!ok));
          if (!ok) return;
          this.spec[key] = value;
          this.changed();
        });
        input.addEventListener("blur", () => { input.value = inputValue(this.spec[key], 2); input.setAttribute("aria-invalid", "false"); });
        return { key, input, limits };
      });
    $<HTMLInputElement>("#con-lay").addEventListener("change", (e) => { this.lay = (e.target as HTMLInputElement).checked; this.changed(); });
    $("#con-auto").addEventListener("click", () => void this.auto());
    $("#con-clear").addEventListener("click", () => { this.points = []; this.changed(); });
    $("#con-off").addEventListener("click", () => deps.close());
    $("#con-apply").addEventListener("click", () => void this.apply());
    this.render();
  }

  open() {
    this.active = true;
    this.changed();
  }

  close() {
    this.active = false;
    this.generation++;
    clearTimeout(this.timer);
    this.preview.group.visible = false;
    this.deps.redraw();
  }

  /** The plane moved or turned: the connectors go with it and are checked again. */
  planeChanged() {
    if (this.active) this.changed(false);
  }

  /** A new summary or language. */
  update() {
    this.render();
  }

  /** A click on the plane: on a connector it goes, elsewhere a new one comes. */
  click(point: Vec3 | null) {
    const s = this.deps.section();
    const scale = this.deps.summary()?.size.previewScale ?? 1;
    if (!point || !s) return;
    const [u, v] = sheetAxes(s);
    const d = new THREE.Vector3(...point).sub(new THREE.Vector3(...s.point));
    const q: [number, number] = [d.dot(u) * scale, d.dot(v) * scale];
    const hit = this.points.findIndex((p) => Math.hypot(p[0] - q[0], p[1] - q[1]) <= reach(this.spec, this.spec.clearanceMm));
    if (hit >= 0) this.points.splice(hit, 1); else this.points.push(q);
    this.changed();
  }

  /** Half A or B picked on the view: the pins stand out of it. */
  half(b: boolean) {
    if (this.spec.kind !== "pin") return;
    this.spec.pinsOnA = !b;
    this.changed();
  }

  /** Centres in preview coordinates on the plane as it is now. */
  private centres(): Vec3[] {
    const s = this.deps.section();
    const scale = this.deps.summary()?.size.previewScale ?? 1;
    if (!s) return [];
    const [u, v] = sheetAxes(s);
    return this.points.map(([a, b]) => new THREE.Vector3(...s.point).addScaledVector(u, a / scale).addScaledVector(v, b / scale).toArray() as Vec3);
  }

  private changed(cleared = true) {
    if (cleared) this.issues = this.points.map(() => null);
    this.objects = null;
    this.render();
    this.schedule();
  }

  private schedule() {
    this.generation++;
    clearTimeout(this.timer);
    if (!this.active || !this.deps.summary()) return;
    if (!this.points.length) return this.status(t("con.empty"), "info");
    this.status(t("con.checking"), "pending");
    this.timer = setTimeout(() => void this.review(), 350);
  }

  private async review() {
    if (this.running) { this.again = true; return; }
    const s = this.deps.section();
    if (!s) return;
    const generation = this.generation;
    this.running = true;
    try {
      const result: ConnectReview = await api.previewConnectedCut(s.point, sectionNormal(s), this.lay && this.spec.kind === "dowel", s.selected, { ...this.spec, at: this.centres() });
      if (generation !== this.generation) return;
      this.issues = result.issues;
      this.objects = result.objects;
      this.render();
    } catch (e) {
      if (generation === this.generation) this.status(this.deps.error(e), "error");
    } finally {
      this.running = false;
      if (this.again) { this.again = false; void this.review(); }
    }
  }

  private async auto() {
    const s = this.deps.section();
    if (!s || !this.deps.summary()) return;
    const scale = this.deps.summary()!.size.previewScale;
    try {
      const found = await api.suggestConnectors(s.point, sectionNormal(s), s.selected, { ...this.spec, at: [] });
      if (!found.length) return this.status(t("con.noRoom"), "warn");
      const [u, v] = sheetAxes(s);
      this.points = found.map((p) => {
        const d = new THREE.Vector3(...p).sub(new THREE.Vector3(...s.point));
        return [d.dot(u) * scale, d.dot(v) * scale] as [number, number];
      });
      this.changed();
    } catch (e) {
      this.status(this.deps.error(e), "error");
    }
  }

  private async apply() {
    const s = this.deps.section();
    if (!s || this.objects === null) return;
    if (await this.deps.cut(this.centres(), this.spec, this.lay && this.spec.kind === "dowel", s.selected)) {
      this.points = [];
      this.issues = [];
      this.objects = null;
      this.render();
    }
  }

  private status(text: string, state: "pending" | "valid" | "error" | "warn" | "info") {
    const el = $("#con-status");
    el.textContent = text;
    el.dataset.state = state;
  }

  private render() {
    const sp = this.spec;
    const pressed = (attr: string, value: string) => document.querySelectorAll<HTMLButtonElement>(`[${attr}]`).forEach((b) => b.setAttribute("aria-pressed", String(b.getAttribute(attr) === value)));
    pressed("data-con-kind", sp.kind);
    pressed("data-con-shape", sp.shape);
    pressed("data-con-pins", sp.pinsOnA ? "a" : "b");
    $("#con-pins").hidden = sp.kind !== "pin";
    $("#con-angle-field").hidden = sp.shape !== "square";
    $("#con-lay-field").hidden = sp.kind !== "dowel";
    $("#con-size-label").textContent = t(sp.shape === "square" ? "con.side" : "con.diameter");
    for (const f of this.fields) if (document.activeElement !== f.input) f.input.value = inputValue(sp[f.key], 2);
    // What the clearance makes of the socket, so the gap is never implicit.
    const socket = sp.sizeMm + 2 * sp.clearanceMm;
    $("#con-socket").textContent = t(sp.kind === "dowel" ? "con.specDowel" : "con.specPin", {
      mark: sp.shape === "square" ? "□" : "Ø", size: num(sp.sizeMm, 2, false), socket: num(socket, 2, false), deep: num(sp.depthMm + sp.clearanceMm, 2, false),
      length: num(sp.kind === "dowel" ? 2 * sp.depthMm : sp.depthMm, 2, false), half: sp.pinsOnA ? "A" : "B",
    });
    $("#con-socket").dataset.kind = sp.kind;
    const chips = $("#con-list");
    chips.replaceChildren(...this.points.map((_, i) => {
      const issue = this.issues[i];
      const chip = Object.assign(document.createElement("button"), { type: "button", className: `con-chip${issue ? " is-bad" : ""}` });
      chip.textContent = String(i + 1);
      const why = issue ? t(`con.issue.${issue}` as Parameters<typeof t>[0]) : t("con.ok");
      chip.title = t("con.remove", { n: i + 1, why });
      chip.setAttribute("aria-label", chip.title);
      chip.addEventListener("click", () => { this.points.splice(i, 1); this.changed(); });
      return chip;
    }));
    $("#con-count").textContent = t("con.count", { count: this.points.length, n: this.points.length });
    $<HTMLButtonElement>("#con-clear").disabled = !this.points.length;
    $<HTMLButtonElement>("#con-apply").disabled = this.objects === null;
    if (this.points.length && this.issues.some((x) => x)) {
      const kinds = [...new Set(this.issues.filter(Boolean))] as string[];
      this.status(t("con.problems", { list: kinds.map((k) => t(`con.issue.${k}` as Parameters<typeof t>[0])).join(" · ") }), "error");
    } else if (this.objects !== null) {
      this.status(t("con.valid", { count: this.points.length, n: this.points.length, objects: this.objects }), "valid");
    } else if (!this.points.length) this.status(t("con.empty"), "info");
    // The view: what the cut will make, on the plane as it stands.
    const s = this.deps.section();
    const scale = this.deps.summary()?.size.previewScale ?? 1;
    if (this.active && s) this.preview.show(sp, sectionNormal(s), this.centres(), this.issues, 1 / scale);
    else this.preview.group.visible = false;
    this.deps.redraw();
  }
}
