// The region brush on the figure: a rough stroke, the suggestions it offers, taking one and extracting it.
import path from "node:path";
import { HERE, services, stop, session, waitState, settle, shot, sleep } from "./lib.mjs";
const MODEL = process.argv[2] ?? path.join(HERE, "e2e/fixtures/out/figure.blend");
let failed = 0;
const check = (name, ok, d) => { if (!ok) failed++; console.log(ok ? "ok  " : "FAIL", name, d ?? ""); };
/** A left-button drag across the view, from and to fractions of the canvas. */
const drag = (b, from, to) => b.execute(async (from, to) => {
  const c = document.querySelector("#viewport");
  const r = c.getBoundingClientRect();
  const at = (f) => ({ clientX: r.left + r.width * f[0], clientY: r.top + r.height * f[1] });
  const o = { bubbles: true, button: 0, buttons: 1, pointerId: 3, pointerType: "mouse" };
  c.dispatchEvent(new PointerEvent("pointerdown", { ...o, ...at(from) }));
  for (let i = 1; i <= 12; i++) {
    const f = [from[0] + (to[0] - from[0]) * i / 12, from[1] + (to[1] - from[1]) * i / 12];
    c.dispatchEvent(new PointerEvent("pointermove", { ...o, ...at(f) }));
    await new Promise((res) => setTimeout(res, 60));
  }
  window.dispatchEvent(new PointerEvent("pointerup", { ...o, buttons: 0, ...at(to) }));
  c.dispatchEvent(new PointerEvent("pointerup", { ...o, buttons: 0, ...at(to) }));
}, from, to);
try {
  await services();
  const b = await session(MODEL);
  await waitState(b, "ready");
  await settle(b);
  await b.setWindowSize(1280, 800);
  await b.execute(() => document.querySelector('#toolbar button[data-tool="region"]').click());
  await settle(b);
  await sleep(500);
  const objects0 = await b.execute(() => document.querySelector('[data-tab="cut"]').textContent.trim());
  // A short stroke across the upper part of the figure (the head).
  // On the hat, at the top of the head.
  await drag(b, [0.44, 0.13], [0.5, 0.15]);
  await settle(b);
  await sleep(1500);
  const sel = await b.execute(() => document.querySelector("#reg-count").textContent);
  check("the stroke selected something", /\d/.test(sel) && !/^0/.test(sel), sel);
  const sugg = await b.execute(() => [...document.querySelectorAll(".reg-suggestion")].map((x) => `${x.dataset.kind}:${x.textContent}`));
  check("suggestions are offered", sugg.length > 0, sugg.join(" | "));
  await shot(b, "region-suggest");
  // Hovering one shows it; taking it makes it the selection.
  await b.execute(() => { const x = document.querySelector('.reg-suggestion[data-kind="colour"]') ?? document.querySelector(".reg-suggestion"); x?.dispatchEvent(new PointerEvent("pointerenter")); });
  await sleep(300);
  await shot(b, "region-suggest-hover");
  await b.execute(() => (document.querySelector('.reg-suggestion[data-kind="colour"]') ?? document.querySelector(".reg-suggestion"))?.click());
  await settle(b);
  await sleep(2000);
  await settle(b);
  const state = await b.execute(() => ({ count: document.querySelector("#reg-count").textContent, status: document.querySelector("#reg-status").textContent, apply: !document.querySelector("#reg-apply").disabled, step2: !document.querySelector("#reg-step2").disabled }));
  check("the taken suggestion can be separated", state.apply && state.step2, JSON.stringify(state));
  await shot(b, "region-taken");
  await b.execute(() => document.querySelector("#reg-apply").click());
  await settle(b);
  await sleep(1000);
  const toast = await b.execute(() => document.querySelector("#toast p")?.textContent);
  check("extracted into an object of its own", /extra|Extra/.test(toast ?? ""), toast);
  await shot(b, "region-extracted");
  await b.deleteSession();
} finally { stop(); process.exitCode = failed ? 1 : 0; }
