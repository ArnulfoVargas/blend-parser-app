// Split for the printer at a size typed in the panel: a cylinder made 600 mm tall is cut so every piece fits.
import path from "node:path";
import { HERE, services, stop, session, waitState, settle, shot, sleep } from "./lib.mjs";
const MODEL = process.argv[2] ?? path.join(HERE, "e2e/fixtures/out/cylinder.blend");
let failed = 0;
const check = (name, ok, d) => { if (!ok) failed++; console.log(ok ? "ok  " : "FAIL", name, d ?? ""); };
try {
  await services();
  const b = await session(MODEL);
  await waitState(b, "ready");
  await settle(b);
  await b.execute(() => document.querySelector('#toolbar button[data-tool="split"]').click());
  await settle(b);
  await sleep(800);
  const before = await b.execute(() => [document.querySelector("#spl-sx").value, document.querySelector("#spl-sz").value, document.querySelector("#spl-summary").textContent]);
  check("size fields show the model's size", before[0] && before[1], before.join(" | "));
  await b.execute(() => { const z = document.querySelector("#spl-sz"); z.value = "600"; z.dispatchEvent(new Event("input", { bubbles: true })); });
  const x = await b.execute(() => document.querySelector("#spl-sx").value);
  check("proportional fills the other sizes", Number(x) > 500, x);
  await b.execute(() => document.querySelector("#spl-resize").click());
  await settle(b);
  await sleep(1500);
  await settle(b);
  const after = await b.execute(() => ({ z: document.querySelector("#spl-sz").value, cuts: [0, 1, 2].map((a) => document.querySelector(`[data-spl-count="${a}"]`).textContent), summary: document.querySelector("#spl-summary").textContent, apply: !document.querySelector("#spl-apply").disabled }));
  check("model resized and cuts proposed", Number(after.z) === 600 && after.cuts.some((c) => c !== "0"), JSON.stringify(after));
  check("the split can be applied", after.apply, after.summary);
  await shot(b, "split-size");
  await b.deleteSession();
} finally { stop(); process.exitCode = failed ? 1 : 0; }
