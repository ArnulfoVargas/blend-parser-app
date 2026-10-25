// A select with a search box: a button that opens a grouped list filtered as you
// type. Follows the ARIA combobox pattern (the search field owns the list and
// points at the active option), so it works from the keyboard and with a reader.
// The panel is a popover, so a dialog's scrolling never clips it.

export interface PickerItem {
  value: string;
  label: string;
  /** Heading the item sits under; items keep the order they come in. */
  group: string;
  /** Second line, quieter: size, brand, nozzles. */
  detail?: string;
  /** Extra words the search matches. */
  keywords?: string;
  /** What the closed button shows, when the label alone is too short out of its group. */
  full?: string;
}

interface PickerOptions {
  /** Accessible name of the button and the list ("Impresora"). */
  label: string;
  placeholder: string;
  /** What the empty list says for a search with no match. */
  empty: (query: string) => string;
  /** Optional action under an empty list ("Crear una propia"). */
  emptyAction?: { label: string; run: (query: string) => void };
  /** Accessible name of the search field ("Buscar: Impresora"). */
  searchLabel: string;
  /** What the button reads when nothing is picked. */
  none: string;
  onPick: (value: string) => void;
}

/** The words a picker shows, so they can change with the interface language. */
export type PickerTexts = Pick<PickerOptions, "label" | "placeholder" | "empty" | "emptyAction" | "searchLabel" | "none">;

/** Lower case, no accents, hyphens as spaces: "Ender-3" matches "ender 3". */
const fold = (s: string) => s.normalize("NFD").replace(/[̀-ͯ]/g, "").replace(/[-_]/g, " ").toLowerCase();

let uid = 0;

export class Picker {
  readonly button: HTMLButtonElement;
  private readonly panel: HTMLDivElement;
  private readonly search: HTMLInputElement;
  private readonly list: HTMLDivElement;
  private readonly emptyEl: HTMLDivElement;
  private items: PickerItem[] = [];
  private shown: PickerItem[] = [];
  private active = -1;
  private current = "";
  private readonly id = `picker-${++uid}`;

  constructor(root: HTMLElement, private opts: PickerOptions) {
    root.classList.add("picker");
    this.button = Object.assign(document.createElement("button"), { type: "button", className: "picker__button", id: `${root.id || this.id}-button` });
    this.button.setAttribute("aria-haspopup", "listbox");
    this.button.setAttribute("aria-expanded", "false");
    this.button.innerHTML = `<span class="picker__value"></span><span class="picker__caret" aria-hidden="true"></span>`;

    this.panel = Object.assign(document.createElement("div"), { className: "picker__panel" });
    this.panel.setAttribute("popover", "manual");
    this.search = Object.assign(document.createElement("input"), {
      type: "search",
      className: "input picker__search",
      placeholder: opts.placeholder,
      autocomplete: "off",
      spellcheck: false,
    });
    this.search.setAttribute("role", "combobox");
    this.search.setAttribute("aria-label", opts.searchLabel);
    this.search.setAttribute("aria-expanded", "true");
    this.search.setAttribute("aria-controls", `${this.id}-list`);
    this.search.setAttribute("aria-autocomplete", "list");
    this.list = Object.assign(document.createElement("div"), { className: "picker__list", id: `${this.id}-list` });
    this.list.setAttribute("role", "listbox");
    this.list.setAttribute("aria-label", opts.label);
    this.emptyEl = Object.assign(document.createElement("div"), { className: "picker__empty", hidden: true });
    this.panel.append(this.search, this.list, this.emptyEl);
    root.append(this.button, this.panel);

    this.button.addEventListener("click", () => (this.isOpen ? this.close() : this.open()));
    this.button.addEventListener("keydown", (e) => {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        this.open();
      }
    });
    this.search.addEventListener("input", () => this.filter());
    this.search.addEventListener("keydown", (e) => this.onKey(e));
    this.list.addEventListener("pointerdown", (e) => e.preventDefault()); // keep focus in the search field
    this.list.addEventListener("click", (e) => {
      const opt = (e.target as HTMLElement).closest<HTMLElement>("[role=option]");
      if (opt) this.pick(opt.dataset.value!);
    });
    this.list.addEventListener("pointermove", (e) => {
      const opt = (e.target as HTMLElement).closest<HTMLElement>("[role=option]");
      if (opt) this.setActive(Number(opt.dataset.index), false);
    });
    document.addEventListener("pointerdown", (e) => {
      if (this.isOpen && !root.contains(e.target as Node)) this.close(false);
    });
    window.addEventListener("resize", () => this.isOpen && this.place());
  }

  get value() {
    return this.current;
  }

  get isOpen() {
    return this.button.getAttribute("aria-expanded") === "true";
  }

  set disabled(on: boolean) {
    this.button.disabled = on;
    if (on) this.close(false);
  }

  setItems(items: PickerItem[]) {
    this.items = items;
    this.setValue(this.current);
    if (this.isOpen) this.filter();
  }

  setValue(value: string) {
    this.current = value;
    const item = this.items.find((i) => i.value === value);
    this.button.querySelector(".picker__value")!.textContent = item?.full ?? item?.label ?? "";
    this.button.title = item ? [item.group, item.label, item.detail].filter(Boolean).join(", ") : "";
    this.button.setAttribute("aria-label", `${this.opts.label}: ${item?.label ?? this.opts.none}`);
  }

  /** New words for the same picker, after a language change. */
  relabel(texts: PickerTexts) {
    this.opts = { ...this.opts, ...texts };
    this.search.placeholder = texts.placeholder;
    this.search.setAttribute("aria-label", texts.searchLabel);
    this.list.setAttribute("aria-label", texts.label);
    this.setValue(this.current);
    if (this.isOpen) this.filter();
  }

  open() {
    if (this.isOpen || this.button.disabled) return;
    this.button.setAttribute("aria-expanded", "true");
    this.search.value = "";
    this.filter();
    this.panel.showPopover();
    this.place();
    this.search.focus();
    this.active = Math.max(0, this.shown.findIndex((i) => i.value === this.current));
    this.setActive(this.active, true);
  }

  close(refocus = true) {
    if (!this.isOpen) return;
    this.button.setAttribute("aria-expanded", "false");
    this.panel.hidePopover();
    if (refocus) this.button.focus();
  }

  /** Under the button, or above it when the window has no room below. */
  private place() {
    const r = this.button.getBoundingClientRect();
    const gap = 6;
    const width = Math.min(Math.max(r.width, 400), window.innerWidth - 16);
    const left = Math.min(Math.max(8, r.left), window.innerWidth - width - 8);
    const below = window.innerHeight - r.bottom - gap - 8;
    const above = r.top - gap - 8;
    const up = below < 280 && above > below;
    Object.assign(this.panel.style, {
      left: `${left}px`,
      width: `${width}px`,
      top: up ? "auto" : `${r.bottom + gap}px`,
      bottom: up ? `${window.innerHeight - r.top + gap}px` : "auto",
      maxHeight: `${Math.min(460, up ? above : below)}px`,
    });
  }

  private filter() {
    const words = fold(this.search.value).split(/\s+/).filter(Boolean);
    this.shown = this.items.filter((i) => {
      // Names only: the detail's sizes would make "3" match every 300 mm printer.
      const hay = fold(`${i.group} ${i.label} ${i.keywords ?? ""}`);
      return words.every((w) => hay.includes(w));
    });
    const frag = document.createDocumentFragment();
    let group: HTMLDivElement | null = null;
    this.shown.forEach((item, index) => {
      if (!group || group.dataset.name !== item.group) {
        group = Object.assign(document.createElement("div"), { className: "picker__group" });
        group.dataset.name = item.group;
        group.setAttribute("role", "group");
        if (item.group) {
          const head = Object.assign(document.createElement("div"), { className: "picker__head", id: `${this.id}-g${index}`, textContent: item.group });
          head.setAttribute("aria-hidden", "true");
          group.setAttribute("aria-labelledby", head.id);
          group.append(head);
        }
        frag.append(group);
      }
      const opt = Object.assign(document.createElement("div"), { className: "picker__option", id: `${this.id}-o${index}` });
      opt.setAttribute("role", "option");
      opt.setAttribute("aria-selected", String(item.value === this.current));
      opt.dataset.value = item.value;
      opt.dataset.index = String(index);
      opt.append(Object.assign(document.createElement("span"), { className: "picker__label", textContent: item.label }));
      if (item.detail) opt.append(Object.assign(document.createElement("span"), { className: "picker__detail", textContent: item.detail }));
      group.append(opt);
    });
    this.list.replaceChildren(frag);
    const query = this.search.value.trim();
    this.emptyEl.hidden = this.shown.length > 0;
    this.list.hidden = this.shown.length === 0;
    if (!this.shown.length) {
      this.emptyEl.replaceChildren(Object.assign(document.createElement("p"), { textContent: this.opts.empty(query) }));
      const action = this.opts.emptyAction;
      if (action) {
        const btn = Object.assign(document.createElement("button"), { type: "button", className: "link", textContent: action.label });
        btn.addEventListener("click", () => {
          this.close(false);
          action.run(query);
        });
        this.emptyEl.append(btn);
      }
    }
    this.setActive(this.shown.length ? 0 : -1, false);
  }

  private setActive(index: number, scroll: boolean) {
    this.list.querySelector(".is-active")?.classList.remove("is-active");
    this.active = index;
    const opt = index >= 0 ? this.list.querySelector<HTMLElement>(`#${this.id}-o${index}`) : null;
    if (opt) {
      opt.classList.add("is-active");
      this.search.setAttribute("aria-activedescendant", opt.id);
      if (scroll) opt.scrollIntoView({ block: "nearest" });
    } else {
      this.search.removeAttribute("aria-activedescendant");
    }
  }

  private onKey(e: KeyboardEvent) {
    const last = this.shown.length - 1;
    const moves: Record<string, number> = { ArrowDown: this.active + 1, ArrowUp: this.active - 1, PageDown: this.active + 8, PageUp: this.active - 8 };
    if (e.key in moves) {
      e.preventDefault();
      if (last >= 0) this.setActive(Math.min(last, Math.max(0, moves[e.key])), true);
    } else if (e.key === "Enter") {
      e.preventDefault();
      const item = this.shown[this.active];
      if (item) this.pick(item.value);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation(); // a dialog around it stays open
      this.close();
    } else if (e.key === "Tab") {
      this.close(false);
    }
  }

  private pick(value: string) {
    const changed = value !== this.current;
    this.setValue(value);
    this.close();
    if (changed) this.opts.onPick(value);
  }
}
