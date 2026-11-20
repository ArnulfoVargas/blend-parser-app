// Changing a tool's settings under the view must not move the view or the column.
import path from "node:path";
import { HERE, services, stop, session, waitState, settle, sleep } from "./lib.mjs";
const MODEL = process.argv[2] ?? path.join(HERE, "e2e/fixtures/out/figure.blend");
try {
  await services();
  const b = await session(MODEL);
  await waitState(b, "ready");
  await settle(b);
  await b.setWindowSize(1100, 700);
  for (const tool of ["section", "connect", "separate", "split", "region"]) {
    await b.execute((t) => document.querySelector(`#toolbar button[data-tool="${t}"]`)?.click(), tool);
    await settle(b);
    const n = await b.execute(() => document.querySelectorAll(".stage__foot input, .stage__foot select, .stage__foot .seg button").length);
    let bad = 0;
    for (let i = 0; i < n; i++) {
      const r = await b.execute(async (i) => {
        const stage = document.querySelector(".stage");
        const el = [...document.querySelectorAll(".stage__foot input, .stage__foot select, .stage__foot .seg button")][i];
        if (!el || el.disabled || !el.getClientRects().length || el.closest("[hidden], [inert]")) return null;
        stage.scrollTop = stage.scrollHeight;
        await new Promise((r) => setTimeout(r, 30));
        const before = { stage: stage.scrollTop, view: document.querySelector(".stage__view").getBoundingClientRect().top, top: el.getBoundingClientRect().top };
        el.focus();
        if (el.tagName === "BUTTON" || el.type === "checkbox") el.click();
        else if (el.tagName === "SELECT") { el.selectedIndex = (el.selectedIndex + 1) % Math.max(1, el.options.length); el.dispatchEvent(new Event("change", { bubbles: true })); }
        else { el.value = String(Number(el.value || 0) + Number(el.step && el.step !== "any" ? el.step : 1)); el.dispatchEvent(new Event("input", { bubbles: true })); el.dispatchEvent(new Event("change", { bubbles: true })); }
        return { id: el.id || el.textContent.trim().slice(0, 16), ...before };
      }, i);
      if (!r) continue;
      await settle(b);
      await sleep(150);
      const a = await b.execute((id) => {
        const el = document.getElementById(id);
        return { stage: document.querySelector(".stage").scrollTop, view: document.querySelector(".stage__view").getBoundingClientRect().top, top: el ? el.getBoundingClientRect().top : null,
          page: [document.documentElement.scrollTop, document.querySelector(".app").scrollTop, document.querySelector(".work").scrollTop, document.querySelector(".stage__view").scrollTop] };
      }, r.id);
      const jump = a.top === null ? 0 : a.top - r.top;
      if (Math.abs(jump) > 40 || a.page.some(Boolean)) { bad++; console.log(tool, r.id, "jump", Math.round(jump), "stage", r.stage, "->", a.stage, "page", a.page.join(",")); }
    }
    console.log(tool, "controls", n, "jumps", bad);
  }
  await b.deleteSession();
} finally { stop(); }
