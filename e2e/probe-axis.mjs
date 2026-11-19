// The axis cut (develop, #67) at 5 %…95 % on the model given, undone after each.
import path from "node:path";
import { HERE, services, stop, session, waitState } from "./lib.mjs";
const model = process.argv[2];
await services();
const b = await session(model);
try {
  await waitState(b, "ready");
  const rows = await b.executeAsync((done) => (async () => {
    const I = window.__TAURI_INTERNALS__.invoke, out = [];
    for (let k = 1; k < 20; k += 2) {
      const r = await I("cut_model", { axis: 2, t: k / 20, keepLow: true, keepOther: true, joint: null }).then((v) => "ok " + (v.plate?.objects ?? v.summary?.plate?.objects), (e) => JSON.stringify(e));
      out.push(`${k * 5}% ${r}`);
      await I("undo_cut", {}).catch(() => {});
    }
    return out;
  })().then(done));
  console.log(path.basename(model), rows.join(" | "));
} finally { await b.deleteSession().catch(() => {}); stop(); }
