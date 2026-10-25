// The window's menu bar (Archivo, Editar, Ayuda), drawn in the page so it
// follows the interface language and theme. Follows the ARIA menubar
// pattern: arrows move between menus and items, Enter runs, Esc closes.

export interface MenuHandlers {
  /** Runs the command named by an item's `data-cmd`. */
  run(cmd: string): void;
  /** Whether a command can run now; the rest show disabled. */
  enabled(cmd: string): boolean;
}

export function menubar(root: HTMLElement, handlers: MenuHandlers) {
  const tops = [...root.querySelectorAll<HTMLButtonElement>(":scope > li > [aria-haspopup]")];
  const menuOf = (top: HTMLButtonElement) => top.nextElementSibling as HTMLElement;
  const itemsOf = (top: HTMLButtonElement) =>
    [...menuOf(top).querySelectorAll<HTMLButtonElement>("[role=menuitem]")].filter((i) => i.getAttribute("aria-disabled") !== "true");
  let open: HTMLButtonElement | null = null;

  function close(refocus = false) {
    if (!open) return;
    open.setAttribute("aria-expanded", "false");
    menuOf(open).hidden = true;
    if (refocus) open.focus();
    open = null;
  }

  function show(top: HTMLButtonElement, focus: "first" | "last" | null) {
    if (open !== top) close();
    open = top;
    menuOf(top).querySelectorAll<HTMLButtonElement>("[role=menuitem]").forEach((i) =>
      i.setAttribute("aria-disabled", String(!handlers.enabled(i.dataset.cmd!))));
    top.setAttribute("aria-expanded", "true");
    menuOf(top).hidden = false;
    const items = itemsOf(top);
    if (focus) (focus === "first" ? items[0] : items[items.length - 1])?.focus();
  }

  const step = (from: HTMLButtonElement, by: number) => tops[(tops.indexOf(from) + by + tops.length) % tops.length];

  tops.forEach((top) => {
    top.addEventListener("click", () => (open === top ? close() : show(top, null)));
    // With one menu open, pointing at another opens it, as desktop menu bars do.
    top.addEventListener("pointerenter", () => open && open !== top && show(top, null));
    top.addEventListener("keydown", (e) => {
      if (e.key === "ArrowDown" || e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        show(top, "first");
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        show(top, "last");
      } else if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
        e.preventDefault();
        const next = step(top, e.key === "ArrowRight" ? 1 : -1);
        if (open) show(next, null);
        next.focus();
      } else if (e.key === "Escape") close();
    });
    menuOf(top).addEventListener("keydown", (e) => {
      const items = itemsOf(top);
      const i = items.indexOf(document.activeElement as HTMLButtonElement);
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        items[(i + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
      } else if (e.key === "Home" || e.key === "End") {
        e.preventDefault();
        items[e.key === "Home" ? 0 : items.length - 1]?.focus();
      } else if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
        e.preventDefault();
        const next = step(top, e.key === "ArrowRight" ? 1 : -1);
        show(next, "first");
      } else if (e.key === "Escape") {
        e.preventDefault();
        close(true);
      } else if (e.key === "Tab") close();
    });
    menuOf(top).addEventListener("click", (e) => {
      const item = (e.target as HTMLElement).closest<HTMLButtonElement>("[role=menuitem]");
      if (!item || item.getAttribute("aria-disabled") === "true") return;
      close();
      handlers.run(item.dataset.cmd!);
    });
  });

  document.addEventListener("pointerdown", (e) => {
    if (open && !root.contains(e.target as Node)) close();
  });
  window.addEventListener("blur", () => close());
}
