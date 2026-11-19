// Shared plumbing for the E2E runs: the services the real app needs (Vite for the
// debug build's devUrl, a virtual display, tauri-driver) and a session per scenario.
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync, existsSync } from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";
import path from "node:path";
import { remote } from "webdriverio";

export const HERE = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
/** The checkout whose UI Vite serves: this one, or a PR's worktree. */
export const ROOT = process.env.BP_ROOT ?? HERE;
export const APP = process.env.BP_APP ?? path.join(HERE, "target/debug/blend-parser-app");
const PORT = Number(process.env.BP_PORT ?? 4444);
export const OUT = process.env.BP_OUT ?? path.join(HERE, "e2e/out");

const children = [];
function start(cmd, args, opts = {}) {
  // Own process group, so stopping it also stops what it started (npx → vite → esbuild).
  const p = spawn(cmd, args, { stdio: ["ignore", "pipe", "pipe"], detached: true, ...opts });
  p.stdout.on("data", (d) => process.env.BP_VERBOSE && process.stdout.write(`[${cmd}] ${d}`));
  p.stderr.on("data", (d) => process.env.BP_VERBOSE && process.stderr.write(`[${cmd}] ${d}`));
  children.push(p);
  return p;
}

async function waitHttp(url, ms = 60000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    try {
      const r = await fetch(url);
      if (r.status < 500) return;
    } catch {}
    await sleep(300);
  }
  throw new Error(`${url} did not answer`);
}

export async function services() {
  if (!existsSync(APP)) throw new Error(`Build the app first: cargo build -p blend-parser-app (${APP})`);
  const env = { ...process.env, DISPLAY: process.env.DISPLAY ?? process.env.BP_DISPLAY ?? ":99", LIBGL_ALWAYS_SOFTWARE: "1", WEBKIT_DISABLE_COMPOSITING_MODE: "1", NO_AT_BRIDGE: "1" };
  if (!process.env.DISPLAY) {
    start("Xvfb", [env.DISPLAY, "-screen", "0", "1600x1000x24", "-nolisten", "tcp"]);
    await sleep(800);
  }
  // A debug build loads the UI from Vite (tauri.conf.json devUrl).
  // A build with the UI inside (BP_EMBEDDED=1, `--features tauri/custom-protocol`) needs no Vite.
  if (!process.env.BP_EMBEDDED) await vite(env);
  start("tauri-driver", ["--port", String(PORT), "--native-port", String(PORT + 1)], { env });
  await waitHttp(`http://127.0.0.1:${PORT}/status`);
  return env;
}

async function vite(env) {
  // Never reuse a server already on the port: it may serve another checkout's UI.
  try { await waitHttp("http://localhost:1420", 500); throw new Error("port 1420 is taken: stop the other Vite first"); } catch (e) { if (e.message.startsWith("port")) throw e; }
  start(path.join(ROOT, "node_modules/.bin/vite"), ["--port", "1420", "--strictPort"], { cwd: ROOT, env });
  await waitHttp("http://localhost:1420");
}

export function stop() {
  for (const c of children.splice(0).reverse()) try { process.kill(-c.pid, "SIGTERM"); } catch {}
}
process.on("exit", stop);
process.on("SIGINT", () => { stop(); process.exit(130); });

/** A fresh app with `model` passed as "Open with…"; one retry, as the last app may still be closing. */
export async function session(model, opts = {}) {
  try {
    return await sessionOnce(model, opts);
  } catch (e) {
    await sleep(3000);
    return sessionOnce(model, opts);
  }
}

async function sessionOnce(model, { lang = "es" } = {}) {
  const args = model ? [model] : [];
  const browser = await remote({
    hostname: "127.0.0.1", port: PORT, logLevel: "error",
    capabilities: { "wdio:enforceWebDriverClassic": true, "tauri:options": { application: APP, args } },
  });
  await browser.setWindowSize(1440, 900).catch(() => {});
  await browser.setTimeout({ script: 600000 }).catch(() => {});
  // Language and a timing probe on every command, kept across reloads in localStorage.
  await browser.execute((l) => {
    const cur = localStorage.getItem("blendparser.lang");
    if (cur !== l) { localStorage.setItem("blendparser.lang", l); window.__bpReloading = true; location.reload(); }
  }, lang);
  await browser.waitUntil(() => browser.execute(() => !window.__bpReloading && document.readyState === "complete" && !!window.__TAURI_INTERNALS__), { timeout: 30000, interval: 100 });
  await probe(browser);
  return browser;
}

/** Wraps the IPC so each command's round trip (UI to Rust and back) is recorded. */
export async function probe(browser) {
  await browser.execute(() => {
    const ipc = window.__TAURI_INTERNALS__;
    if (!ipc || ipc.__probed) return;
    const inner = ipc.invoke.bind(ipc);
    window.__bpTimes = [];
    ipc.invoke = async (cmd, args, opts) => {
      const t0 = performance.now();
      try { return await inner(cmd, args, opts); }
      finally { window.__bpTimes.push({ cmd, ms: performance.now() - t0 }); }
    };
    ipc.__probed = true;
  });
}

export const times = (b) => b.execute(() => (window.__bpTimes ?? []).splice(0));

export async function waitState(b, state, ms = 180000) {
  await b.waitUntil(async () => (await b.execute(() => document.querySelector("[data-app]")?.dataset.state)) === state,
    { timeout: ms, interval: 200, timeoutMsg: `state never became ${state}` });
}

/** Waits until no command is in flight and the busy bar is idle. */
export async function settle(b, ms = 60000) {
  await b.waitUntil(async () => b.execute(() => !document.querySelector("[aria-busy='true']") && !document.querySelector("[data-app]").classList.contains("is-working")),
    { timeout: ms, interval: 100 }).catch(() => {});
  await sleep(250);
}

export async function shot(b, name) {
  mkdirSync(OUT, { recursive: true });
  const file = path.join(OUT, `${name}.png`);
  await b.saveScreenshot(file);
  return file;
}

export function report(name, data) {
  mkdirSync(OUT, { recursive: true });
  writeFileSync(path.join(OUT, `${name}.json`), JSON.stringify(data, null, 2));
}

/** Clicks through JS: WebKitWebDriver's element click refuses elements under a popover or offscreen. */
export const click = (b, sel) => b.execute((s) => { const el = document.querySelector(s); if (!el) return false; el.click(); return true; }, sel);
export const exists = (b, sel) => b.execute((s) => !!document.querySelector(s), sel);
export const visible = (b, sel) => b.execute((s) => { const el = document.querySelector(s); if (!el) return false; const r = el.getBoundingClientRect(); return r.width > 0 && r.height > 0 && getComputedStyle(el).visibility !== "hidden"; }, sel);
export const text = (b, sel) => b.execute((s) => document.querySelector(s)?.textContent?.trim() ?? null, sel);
export async function setValue(b, sel, value, event = "change") {
  return b.execute((s, v, ev) => {
    const el = document.querySelector(s);
    if (!el) return false;
    el.value = String(v);
    el.dispatchEvent(new Event("input", { bubbles: true }));
    if (ev === "change") el.dispatchEvent(new Event("change", { bubbles: true }));
    return true;
  }, sel, value, event);
}
export const key = (b, k) => b.execute((kk) => document.dispatchEvent(new KeyboardEvent("keydown", { key: kk, bubbles: true })), k);
export { sleep };
