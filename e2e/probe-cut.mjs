// Where a plane cut is refused: preview_cut at heights 5 %…95 % on the model given.
import path from "node:path";
import { HERE, services, stop, session, waitState } from "./lib.mjs";
const model = process.argv[2] ?? path.join(HERE, "e2e/fixtures/out/figure.blend");
await services();
const b = await session(model);
try {
  await waitState(b, "ready");
  const rows = await b.executeAsync((done) => {
    const I = window.__TAURI_INTERNALS__.invoke;
    I("preview_mesh", {}).then(async (buf) => {
      const leaves = new DataView(buf, 0, 8).getUint32(0, true);
      const p = new Float32Array(buf, 8, leaves * 9);
      let lo = Infinity, hi = -Infinity, cx = 0, cy = 0;
      for (let i = 0; i < p.length; i += 3) { lo = Math.min(lo, p[i + 2]); hi = Math.max(hi, p[i + 2]); }
      const out = [];
      for (let k = 1; k < 20; k++) {
        const z = lo + (hi - lo) * k / 20;
        const r = await I("preview_cut", { point: [cx, cy, z], normal: [0, 0, 1], keepLow: true, keepOther: true, layOnCut: false, selected: [] }).then((v) => "ok " + JSON.stringify(v), (e) => JSON.stringify(e));
        out.push(`${k * 5}% ${r}`);
      }
      done(out);
    });
  });
  console.log(path.basename(model)); for (const r of rows) console.log("  " + r);
} finally { await b.deleteSession().catch(() => {}); stop(); }
