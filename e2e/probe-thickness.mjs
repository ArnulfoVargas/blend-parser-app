// The thickness tool shows the wall under the pointer before any click.
import path from "node:path";
import { HERE, services, stop, session, waitState, settle, shot, sleep } from "./lib.mjs";
const MODEL = process.argv[2] ?? path.join(HERE, "e2e/fixtures/out/cylinder.blend");
let failed = 0;
const check = (name, ok, d) => { if (!ok) failed++; console.log(ok ? "ok  " : "FAIL", name, d ?? ""); };
const hover = (b, f) => b.execute((f) => {
  const c = document.querySelector("#viewport");
  const r = c.getBoundingClientRect();
  c.dispatchEvent(new PointerEvent("pointermove", { bubbles: true, buttons: 0, clientX: r.left + r.width * f[0], clientY: r.top + r.height * f[1], pointerType: "mouse" }));
}, f);
try {
  await services();
  const b = await session(MODEL);
  await waitState(b, "ready");
  await settle(b);
  await b.setWindowSize(1280, 800);
  await b.execute(() => document.querySelector('#toolbar button[data-tool="thickness"]').click());
  await settle(b);
  await hover(b, [0.5, 0.55]);
  await sleep(800);
  const text = await b.execute(() => document.querySelector("#thickness-out").textContent);
  check("hovering shows the wall before clicking", /mm/.test(text), text);
  await shot(b, "thickness-hover");
  await b.execute(() => document.querySelector("#viewport").dispatchEvent(new PointerEvent("pointerleave")));
  await sleep(500);
  const gone = await b.execute(() => document.querySelector("#thickness-out").textContent);
  check("leaving the model takes the preview away", !/mm/.test(gone), gone);
  await b.deleteSession();
} finally { stop(); process.exitCode = failed ? 1 : 0; }
