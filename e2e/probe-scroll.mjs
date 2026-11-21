// Changing a setting must leave the side panel where it was: scroll each
// control into the middle, change it, and record how far the panel jumped.
import path from "node:path";
import { HERE, services, stop, session, waitState, settle, report, sleep } from "./lib.mjs";
const MODEL = process.argv[2] ?? path.join(HERE, "e2e/fixtures/out/figure.blend");
const out = [];
try {
  await services();
  const b = await session(MODEL);
  await waitState(b, "ready");
  await settle(b);
  await b.setWindowSize(1100, 700);
  for (const tab of ["model", "mesh"]) {
    await b.execute((t) => document.querySelector(`[role=tab][data-tab="${t}"]`)?.click(), tab);
    await settle(b);
    // Open every group so every control can be reached.
    await b.execute(() => document.querySelectorAll(".side details").forEach((d) => (d.open = true)));
    await sleep(300);
    const n = await b.execute(() => document.querySelectorAll(".side [role=tabpanel]:not([hidden]) input:not([type=hidden]), .side [role=tabpanel]:not([hidden]) select").length);
    for (let i = 0; i < n; i++) {
      const r = await b.execute(async (i) => {
        const side = document.querySelector(".side");
        const el = [...document.querySelectorAll(".side [role=tabpanel]:not([hidden]) input:not([type=hidden]), .side [role=tabpanel]:not([hidden]) select")][i];
        if (!el || el.disabled || !el.getClientRects().length) return null;
        el.scrollIntoView({ block: "center" });
        await new Promise((r) => setTimeout(r, 50));
        const before = side.scrollTop;
        const top0 = el.getBoundingClientRect().top;
        el.focus();
        if (el.type === "checkbox") el.click();
        else if (el.tagName === "SELECT") { if (el.options.length > 1) { el.selectedIndex = (el.selectedIndex + 1) % el.options.length; el.dispatchEvent(new Event("change", { bubbles: true })); } }
        else if (el.type === "range" || el.type === "number") { el.value = String(Number(el.value || 0) + Number(el.step || 1)); el.dispatchEvent(new Event("input", { bubbles: true })); el.dispatchEvent(new Event("change", { bubbles: true })); }
        else return null;
        return { id: el.id || el.dataset.op || el.name, before, top0 };
      }, i);
      if (!r) continue;
      await settle(b);
      // A confirmation dialog: answer no.
      await b.execute(() => document.querySelector("dialog[open] .btn--quiet, dialog[open] [value=cancel]")?.click());
      await settle(b);
      const after = await b.execute((id) => {
        const side = document.querySelector(".side");
        const el = document.getElementById(id) ?? document.querySelector(`[data-op="${id}"]`);
        return { scroll: side.scrollTop, top: el?.getBoundingClientRect().top ?? null, page: [document.documentElement.scrollTop, document.querySelector(".app").scrollTop, document.querySelector(".work").scrollTop] };
      }, r.id);
      const jump = after.top === null ? null : Math.round(after.top - r.top0);
      out.push({ tab, ...r, ...after, jump });
      if (jump === null || Math.abs(jump) > 40 || after.page.some(Boolean)) console.log(tab, r.id, "jump", jump, "scroll", r.before, "->", after.scroll, "page", after.page.join(","));
    }
    // Undo everything the run changed.
    for (let k = 0; k < 40; k++) {
      const can = await b.execute(() => !document.querySelector("#undo").disabled);
      if (!can) break;
      await b.execute(() => document.querySelector("#undo").click());
      await settle(b);
    }
  }
  console.log("checked", out.length);
  await b.deleteSession();
} finally { report("scroll", out); stop(); }
