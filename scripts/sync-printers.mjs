// Manual maintenance command (network required); never run by the build or app.
// Cargo bakes the resulting versioned JSON into Rust constants offline.
// Rebuilds crates/bp-3mf/src/catalog.json from Bambu Studio's own printer profiles,
// so every printer the app offers names presets Bambu Studio really ships.
//
//   node scripts/sync-printers.mjs [commit]      (default: the latest master commit)
//
// For each vendor in resources/profiles it reads every instantiable machine preset,
// resolves `inherits`, and keeps single-extruder printers: their print area and height,
// the zones of the bed they keep free, and per nozzle the machine preset, a process (the
// machine's default, or the compatible one closest to half the nozzle) with its layer
// height, a PLA filament preset, and the nozzle's volume below the cutter (the least
// Bambu Studio flushes at a swap).
// Printers with two extruders are left out: their 3MF needs another filament layout.
import { writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = 'bambulab/BambuStudio';
const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const OUT = join(ROOT, 'crates/bp-3mf/src/catalog.json');

const commit = process.argv[2] ?? (await (await fetch(`https://api.github.com/repos/${REPO}/commits/master`)).json()).sha;
const raw = (path) => `https://raw.githubusercontent.com/${REPO}/${commit}/resources/profiles/${path.split('/').map(encodeURIComponent).join('/')}`;

async function get(path) {
  for (let attempt = 0; ; attempt++) {
    const r = await fetch(raw(path));
    if (r.ok) return r.json();
    if (r.status === 404 || attempt === 3) throw new Error(`${path}: ${r.status}`);
    await new Promise((res) => setTimeout(res, 500 * (attempt + 1)));
  }
}

/** Runs `fn` over `items`, a few at a time; failures become null. */
async function all(items, fn, width = 16) {
  const out = new Array(items.length);
  let next = 0;
  await Promise.all(Array.from({ length: width }, async () => {
    while (next < items.length) {
      const i = next++;
      out[i] = await fn(items[i]).catch(() => null);
    }
  }));
  return out;
}

// VENDORS=BBL,Creality,... skips listing the folder through the GitHub API (rate-limited without a token).
const vendors = process.env.VENDORS
  ? process.env.VENDORS.split(',').map((v) => v.trim()).filter(Boolean)
  : (await (await fetch(`https://api.github.com/repos/${REPO}/contents/resources/profiles?ref=${commit}`)).json())
      .filter((e) => e.type === 'file' && e.name.endsWith('.json'))
      .map((e) => e.name.replace(/\.json$/, ''));

const first = (v) => (Array.isArray(v) ? v[0] : v);
/** `bed_exclude_area` is one polygon of "XxY" points; the app keeps its bounding box. */
const excludeOf = (m) => {
  const pts = [].concat(m.bed_exclude_area ?? []).map((p) => p.split('x').map(Number)).filter((p) => p.length === 2 && p.every(Number.isFinite));
  if (pts.length < 3) return [];
  const xs = pts.map((p) => p[0]);
  const ys = pts.map((p) => p[1]);
  const box = [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
  return box[2] > box[0] && box[3] > box[1] ? [box] : [];
};
const printers = [];

for (const vendor of vendors) {
  const index = await get(`${vendor}.json`).catch(() => null);
  if (!index?.machine_list) continue;
  const load = async (list) => {
    const byName = new Map();
    const docs = await all(list, (e) => get(`${vendor}/${e.sub_path}`));
    list.forEach((e, i) => docs[i] && byName.set(e.name, docs[i]));
    return byName;
  };
  const machines = await load(index.machine_list);
  const processes = await load(index.process_list ?? []);
  const filaments = await load(index.filament_list ?? []);
  const resolve = (map, name, seen = new Set()) => {
    const doc = map.get(name);
    if (!doc || seen.has(name)) return {};
    seen.add(name);
    return { ...(doc.inherits ? resolve(map, doc.inherits, seen) : {}), ...doc };
  };
  const usable = (map, name) => map.get(name)?.instantiation === 'true';
  const printersOf = (map, name) => [].concat(resolve(map, name).compatible_printers ?? []);
  // Instantiable presets that list `machine` among their compatible printers.
  const compatible = (map, machine) => [...map.keys()].filter((n) => usable(map, n) && printersOf(map, n).includes(machine));

  const layerOf = (process) => Number(first(resolve(processes, process).layer_height));
  // A few defaults point at another nozzle's process; a layer over 80 % of the nozzle gives them away.
  const fitsNozzle = (p, diameter) => layerOf(p) > 0 && layerOf(p) <= diameter * 0.8;
  const pickProcess = (m, name, diameter) => {
    const def = m.default_print_profile;
    if (def && usable(processes, def) && fitsNozzle(def, diameter)) return def;
    const near = (p) => Math.abs(layerOf(p) - diameter / 2);
    return compatible(processes, name).filter((p) => fitsNozzle(p, diameter)).sort((a, b) => near(a) - near(b))[0] ?? null;
  };

  const isPla = (n) => /\bPLA\b/.test(n) && !/PLA-[CG]F/.test(n) && usable(filaments, n);
  const generic = (list) => list.find((n) => /Generic/i.test(n)) ?? list[0];
  const pickFilament = (m, name) => {
    const def = first(m.default_filament_profile);
    if (def && isPla(def)) return def;
    const fits = compatible(filaments, name).filter(isPla);
    const open = [...filaments.keys()].filter((n) => isPla(n) && printersOf(filaments, n).length === 0);
    return generic(fits) ?? generic(open) ?? null;
  };

  const models = new Map();
  for (const [name, doc] of machines) {
    if (doc.instantiation !== 'true' || !/ nozzle$/.test(name)) continue;
    const m = resolve(machines, name);
    const nozzles = [].concat(m.nozzle_diameter ?? []);
    if (nozzles.length !== 1 || !m.printer_model || !m.printable_area) continue;
    const pts = [].concat(m.printable_area).map((p) => p.split('x').map(Number));
    const height = Number(first(m.printable_height));
    if (!(height > 0) || pts.some((p) => p.some(Number.isNaN))) continue;
    const xs = pts.map((p) => p[0]);
    const ys = pts.map((p) => p[1]);
    const diameter = Number(nozzles[0]);
    const model = models.get(m.printer_model) ?? {
      id: m.printer_model,
      vendor: vendor === 'BBL' ? 'Bambu Lab' : index.name ?? vendor,
      name: m.printer_model,
      area: [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)],
      height,
      exclude: excludeOf(m),
      nozzles: [],
    };
    if (model.nozzles.some((n) => n.diameter === diameter)) continue;
    const process = pickProcess(m, name, diameter);
    const layer = process ? layerOf(process) : NaN;
    const volume = Number(first(m.nozzle_volume));
    model.nozzles.push({
      diameter,
      printer: name,
      process,
      layer: layer > 0 ? layer : null,
      filament: pickFilament(m, name),
      flush_min: volume > 0 ? volume : null,
    });
    models.set(m.printer_model, model);
  }
  for (const m of models.values()) {
    m.nozzles.sort((a, b) => a.diameter - b.diameter);
    printers.push(m);
  }
  console.log(`${vendor}: ${models.size} impresoras`);
}

// Bambu Lab first, as Bambu Studio lists it; then the other vendors by name.
const order = (p) => (p.vendor === 'Bambu Lab' ? '0' : '1') + p.vendor + ' ' + p.name;
printers.sort((a, b) => order(a).localeCompare(order(b), 'en', { numeric: true }));
writeFileSync(OUT, JSON.stringify({ source: `${REPO}@${commit}`, printers }, null, 1) + '\n');
console.log(`${printers.length} impresoras en ${OUT}`);
