// Every tool at small window sizes: screenshots, and what does not fit or scrolls by itself.
//
//   node probe-layout.mjs [model]
//
// For each size and tool it records the controls cut off by the window, the
// containers that scrolled without a scrollbar (overflow hidden scrolled by a
// focus), and whether focusing each field of the tool moved the page.
import path from "node:path";
import { HERE, services, stop, session, waitState, settle, shot, report, click, sleep } from "./lib.mjs";

const MODEL = process.argv[2] ?? process.env.BP_MODEL ?? path.join(HERE, "e2e/fixtures/out/figure.blend");
/** `WxH` or `WxH@zoom`: a zoom over 1 makes the page that much smaller than the window, for screens below the window's minimum. */
const SIZES = (process.env.BP_SIZES ?? "1024x640,1280x720,1440x900").split(",").map((s) => { const [wh, z] = s.split("@"); return [...wh.split("x").map(Number), Number(z ?? 1)]; });
const TOOLS = (process.env.BP_TOOLS ?? "orbit,brush,size,move,rotate,section,connect,separate,split,region,clip,thickness").split(",");

/** Scrolled containers that offer no scrollbar, and visible controls the window cuts off. */
const audit = (b) => b.execute(() => {
  const stuck = [];
  for (const el of [document.documentElement, document.body, ...document.querySelectorAll("*")]) {
    if (el.scrollTop === 0 && el.scrollLeft === 0) continue;
    const s = getComputedStyle(el);
    const scrollable = /(auto|scroll)/.test(s.overflowY) || /(auto|scroll)/.test(s.overflowX);
    if (!scrollable) stuck.push(`${el.tagName.toLowerCase()}${el.id ? "#" + el.id : ""}.${[...el.classList].join(".")} ${el.scrollTop}`);
  }
  const z = Number(document.documentElement.style.zoom) || 1; const W = innerWidth / z, H = innerHeight / z;
  const cut = [];
  for (const el of document.querySelectorAll("button, input, select, output, label")) {
    const r = el.getBoundingClientRect();
    if (!r.width || !r.height || getComputedStyle(el).visibility === "hidden") continue;
    if (el.closest("dialog:not([open]), [hidden], .toolbar, .side, .ams, .visually-hidden")) continue;
    // Below the fold of a column with a scrollbar is reachable; past the sides never is.
    let scroller = el.parentElement;
    while (scroller && !/(auto|scroll)/.test(getComputedStyle(scroller).overflowY)) scroller = scroller.parentElement;
    const below = r.bottom > H + 1 || r.top < -1;
    if (r.right > W + 1 || r.left < -1 || (below && !scroller)) cut.push(`${el.tagName.toLowerCase()}#${el.id || el.textContent.trim().slice(0, 20)}`);
  }
  return { stuck, cut: cut.slice(0, 25), cutCount: cut.length };
});

/** Focuses each visible field under the view and says which ones moved the page. */
const focusAll = (b) => b.execute(async () => {
  const moved = [];
  const fields = [...document.querySelectorAll(".stage__foot input, .stage__foot select, .stage__foot button")]
    .filter((el) => { const r = el.getBoundingClientRect(); return r.width && r.height && !el.closest("[hidden]"); });
  for (const el of fields) {
    el.focus();
    await new Promise((r) => requestAnimationFrame(r));
    const shifted = [document.documentElement, document.body, document.querySelector(".app"), document.querySelector(".work"), document.querySelector(".stage__view")]
      .filter((x) => x && (x.scrollTop || x.scrollLeft));
    if (shifted.length) {
      moved.push(el.id || el.textContent.trim().slice(0, 20));
      for (const x of shifted) { x.scrollTop = 0; x.scrollLeft = 0; }
    }
  }
  document.activeElement?.blur();
  return moved;
});

const results = [];
try {
  await services();
  const b = await session(MODEL);
  await waitState(b, "ready");
  await settle(b);
  for (const [w, h, z] of SIZES) {
    await b.setWindowSize(w, h);
    await b.execute((zoom) => { document.documentElement.style.zoom = zoom === 1 ? "" : String(zoom); }, z);
    await sleep(600);
    const inner = await b.execute(() => [innerWidth, innerHeight]);
    for (const tool of TOOLS) {
      const ok = await b.execute((t) => {
        const btn = document.querySelector(`#toolbar button[data-tool="${t}"]`);
        if (!btn) return false;
        btn.click();
        return true;
      }, tool);
      if (!ok) continue;
      await settle(b);
      await sleep(400);
      const a = await audit(b);
      const moved = await focusAll(b);
      const file = await shot(b, `layout-${w}x${h}${z === 1 ? "" : "@" + z}-${tool}`);
      results.push({ size: `${w}x${h}@${z}`, inner, tool, ...a, moved, file });
      console.log(`${w}x${h}@${z} ${tool}: cut ${a.cutCount} stuck ${a.stuck.length} moved ${moved.length}`, a.cut.slice(0, 6).join(" "), moved.slice(0, 4).join(" "));
    }
    await click(b, '#toolbar button[data-tool="orbit"]');
  }
  await b.deleteSession();
} finally {
  report("layout", results);
  stop();
}
