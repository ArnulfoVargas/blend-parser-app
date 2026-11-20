// Right click on the model: clone with overflow to a new plate, move between
// plates, the active plate's number. Screenshots and checks in out/plates.json.
import path from "node:path";
import { HERE, services, stop, session, waitState, settle, shot, report, sleep } from "./lib.mjs";
const MODEL = process.argv[2] ?? path.join(HERE, "e2e/fixtures/out/cylinder.blend");
const checks = [];
const check = (name, ok, detail) => { checks.push({ name, ok: !!ok, detail }); console.log(ok ? "ok  " : "FAIL", name, detail ?? ""); };
/** A right click (down and up without moving) at the canvas centre or at a given point. */
const rightClick = (b, at) => b.execute((at) => {
  const c = document.querySelector("#viewport");
  const r = c.getBoundingClientRect();
  const x = at ? at[0] : r.left + r.width / 2, y = at ? at[1] : r.top + r.height / 2;
  const o = { bubbles: true, button: 2, buttons: 2, clientX: x, clientY: y, pointerId: 9, pointerType: "mouse" };
  c.dispatchEvent(new PointerEvent("pointerdown", o));
  window.dispatchEvent(new PointerEvent("pointerup", { ...o, buttons: 0 }));
  return [x, y];
}, at ?? null);
const menu = (b) => b.execute(() => {
  const m = document.querySelector("#ctx-menu");
  return { open: m.matches(":popover-open"), title: document.querySelector("#ctx-title").textContent,
    items: [...m.querySelectorAll("[data-ctx]")].filter((i) => !i.hidden).map((i) => `${i.dataset.ctx}${i.disabled ? "(off)" : ""}`) };
});
const plates = (b) => b.execute(() => [...document.querySelectorAll(".plate-tag")].filter((t) => !t.hidden).map((t) => `${t.textContent}${t.getAttribute("aria-pressed") === "true" ? "*" : ""}`));
try {
  await services();
  const b = await session(MODEL);
  await waitState(b, "ready");
  await settle(b);
  await b.setWindowSize(1280, 800);
  await sleep(500);
  await rightClick(b);
  let m = await menu(b);
  check("menu opens over the model", m.open && m.items.includes("clone"), JSON.stringify(m));
  check("move is off with one plate", m.items.includes("move(off)"));
  await shot(b, "plates-menu");
  // Clone 30: 25 fit the A1 plate with the original, the rest spill to a new plate.
  await b.execute(() => document.querySelector('[data-ctx="clone"]').click());
  await sleep(300);
  check("clone dialog opens", await b.execute(() => document.querySelector("#clone-dialog").open));
  await b.execute(() => { document.querySelector("#clone-count").value = "30"; document.querySelector('#clone-dialog [value="ok"]').click(); });
  await settle(b);
  await sleep(500);
  const after = await b.execute(() => ({ toast: document.querySelector("#toast p")?.textContent, objects: new Set(window.__bpLast?.parts?.map((p) => p.object) ?? []).size }));
  console.log("toast:", after.toast);
  const tags = await plates(b);
  check("cloning spilled to a second plate with numbers", tags.length >= 2, tags.join(" "));
  await b.execute(() => document.querySelector("#toolbar-expand") && 0);
  await shot(b, "plates-cloned");
  // Plate 2's tag makes it active.
  await b.execute(() => document.querySelectorAll(".plate-tag")[1]?.click());
  await sleep(300);
  check("clicking a plate number activates it", (await plates(b))[1]?.endsWith("*"), (await plates(b)).join(" "));
  // Menu over a copy now offers Move: look for a point over an object.
  const box = await b.execute(() => { const r = document.querySelector("#viewport").getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; });
  for (let gy = 0.2; gy < 0.9; gy += 0.1) {
    for (let gx = 0.1; gx < 0.9; gx += 0.05) {
      await rightClick(b, [box[0] + box[2] * gx, box[1] + box[3] * gy]);
      m = await menu(b);
      if (m.items.includes("move")) break;
      await b.keys("Escape");
    }
    if (m.items.includes("move")) break;
  }
  check("move is on with two plates", m.items.includes("move"), JSON.stringify(m));
  await b.execute(() => document.querySelector('[data-ctx="move"]')?.click());
  await sleep(300);
  const options = await b.execute(() => [...document.querySelectorAll("#move-plates label")].map((l) => l.textContent));
  check("move dialog lists the other plates and a new one", options.length >= 2, options.join(" | "));
  await b.execute(() => document.querySelector('#move-dialog [value="cancel"]').click());
  // Esc closes the menu.
  await rightClick(b);
  await b.keys("Escape");
  await sleep(200);
  check("Esc closes the menu", !(await menu(b)).open);
  // Undo the clone.
  await b.execute(() => document.querySelector("#undo").click());
  await settle(b);
  check("undo brings back one plate", (await plates(b)).length === 0, (await plates(b)).join(" "));
  await b.deleteSession();
} finally {
  report("plates", checks);
  stop();
}
