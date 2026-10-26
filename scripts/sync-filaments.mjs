// Manual maintenance command (network required); never run by the build or app.
// Rebuilds crates/bp-3mf/src/filaments.json from Bambu Studio's own filament
// colour list and filament presets, so the colours the app offers are the ones
// Bambu Lab sells and the 3MF names presets Bambu Studio really ships.
//
//   node scripts/sync-filaments.mjs [commit]      (default: the latest master commit)
//
// Keeps single-colour filaments (gradients and multi-colour spools are not one
// slot colour), with their Spanish and English names. For each filament type it
// records the material Bambu Studio gives it (`filament_type`, through
// `inherits`) and the printers it has a preset for, as the preset name's suffix
// ("@BBL X1C", "@BBL A1 0.2 nozzle").
import { writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO = 'bambulab/BambuStudio';
const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const OUT = join(ROOT, 'crates/bp-3mf/src/filaments.json');

const commit = process.argv[2] ?? (await (await fetch(`https://api.github.com/repos/${REPO}/commits/master`)).json()).sha;
if (!/^[0-9a-f]{40}$/.test(commit ?? '')) throw new Error(`not a commit: ${commit}`);
const raw = (path) => `https://raw.githubusercontent.com/${REPO}/${commit}/resources/profiles/${path.split('/').map(encodeURIComponent).join('/')}`;

async function get(path) {
  for (let attempt = 0; ; attempt++) {
    const r = await fetch(raw(path));
    if (r.ok) return r.json();
    if (r.status === 404 || attempt === 3) throw new Error(`${path}: ${r.status}`);
    await new Promise((res) => setTimeout(res, 500 * (attempt + 1)));
  }
}

const index = await get('BBL.json');
const paths = new Map(index.filament_list.map((f) => [f.name, `BBL/${f.sub_path}`]));

/** A preset's `filament_type`, following `inherits` up to where it is set. */
async function materialOf(name, depth = 0) {
  const path = paths.get(name);
  if (!path || depth > 8) return null;
  const p = await get(path);
  const t = Array.isArray(p.filament_type) ? p.filament_type[0] : p.filament_type;
  return t ?? (p.inherits ? materialOf(p.inherits, depth + 1) : null);
}

const colours = (await get('BBL/filament/filaments_color_codes.json')).data;
const single = colours.filter((c) => c.fila_color_type === '单色' && c.fila_color?.length === 1 && /^#[0-9A-Fa-f]{8}$/.test(c.fila_color[0]));

const types = {};
for (const type of [...new Set(single.map((c) => c.fila_type))].sort()) {
  const prefix = `Bambu ${type} `;
  const named = [...paths.keys()].filter((n) => n.toLowerCase().startsWith(prefix.toLowerCase()));
  const base = named.find((n) => n.endsWith('@base'));
  types[type] = {
    // The preset names as Bambu Studio spells them ("Support For PLA").
    preset: base ? base.slice(0, -' @base'.length) : null,
    material: base ? await materialOf(base) : null,
    printers: named.filter((n) => n.includes(' @BBL ')).map((n) => n.slice(n.indexOf('@BBL '))).sort(),
  };
}

const out = {
  source: { repo: REPO, commit, files: ['resources/profiles/BBL/filament/filaments_color_codes.json', 'resources/profiles/BBL.json'] },
  types,
  colours: single
    .map((c) => ({
      id: c.fila_color_code,
      type: c.fila_type,
      code: c.color_code ?? '',
      hex: c.fila_color[0].slice(0, 7).toUpperCase(),
      es: c.fila_color_name?.es ?? c.fila_color_name?.en ?? '',
      en: c.fila_color_name?.en ?? '',
    }))
    .sort((a, b) => a.type.localeCompare(b.type) || a.code.localeCompare(b.code) || a.id.localeCompare(b.id)),
};
const ids = new Set(out.colours.map((c) => c.id));
if (ids.size !== out.colours.length) throw new Error('repeated colour ids');
writeFileSync(OUT, `${JSON.stringify(out, null, 1)}\n`);
const missing = Object.entries(types).filter(([, t]) => !t.material).map(([n]) => n);
console.log(`${out.colours.length} colours in ${Object.keys(types).length} types from ${commit}${missing.length ? `; no preset for ${missing.join(', ')}` : ''}`);
