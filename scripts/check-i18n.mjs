// Checks the interface catalogues beyond what TypeScript already enforces
// (every catalogue has exactly the keys of es.ts):
// - every key index.html names exists;
// - every error key the backend sends exists;
// - every progress step Blender's extractor and the backend report exists;
// - no catalogue has an empty text, and every text keeps the placeholders of the Spanish one.
//
//   node scripts/check-i18n.mjs
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const read = (p) => readFileSync(join(ROOT, p), 'utf8');

/** "key": "text" or "key": { one: "…", other: "…" }, as the catalogues are written. */
function entries(file) {
  const src = read(file);
  const out = new Map();
  const re = /^\s*"([\w.]+)":\s*(?:"((?:[^"\\]|\\.)*)"|\{([\s\S]*?)\}),?\s*$/gm;
  for (const m of src.matchAll(re)) {
    const texts = m[2] !== undefined ? [m[2]] : [...m[3].matchAll(/(?:one|other):\s*"((?:[^"\\]|\\.)*)"/g)].map((x) => x[1]);
    out.set(m[1], texts);
  }
  return out;
}

const dir = 'src/i18n';
const langs = readdirSync(join(ROOT, dir)).filter((f) => /^[a-z]{2}\.ts$/.test(f));
const es = entries(`${dir}/es.ts`);
const problems = [];
const holes = (texts) => new Set(texts.flatMap((t) => [...t.matchAll(/\{(\w+)\}/g)].map((m) => m[1])));

for (const f of langs) {
  const cat = entries(`${dir}/${f}`);
  for (const [key, texts] of cat) {
    if (texts.some((t) => t.trim() === '')) problems.push(`${f}: ${key} está vacío`);
    const want = holes(es.get(key) ?? []);
    const have = holes(texts);
    for (const h of want) if (!have.has(h)) problems.push(`${f}: ${key} no usa {${h}}`);
    for (const h of have) if (!want.has(h)) problems.push(`${f}: ${key} usa {${h}}, que es.ts no tiene`);
  }
  for (const key of es.keys()) if (!cat.has(key)) problems.push(`${f}: falta ${key}`);
}

const need = (key, where) => es.has(key) || problems.push(`${where}: ${key} no está en es.ts`);

const html = read('index.html');
for (const m of html.matchAll(/data-i18n="([^"]+)"/g)) need(m[1], 'index.html');
for (const m of html.matchAll(/data-i18n-attr="([^"]+)"/g)) for (const pair of m[1].split(';')) need(pair.split(':')[1], 'index.html');

for (const f of readdirSync(join(ROOT, 'src-tauri/src'))) {
  for (const m of read(`src-tauri/src/${f}`).matchAll(/"(error\.\w+)"/g)) need(m[1], `src-tauri/src/${f}`);
}
for (const m of read('crates/bp-blend/src/extract.py').matchAll(/progress\("(\w+)"/g)) need(`progress.${m[1]}`, 'extract.py');
for (const m of read('src-tauri/src/commands.rs').matchAll(/emit\("(\w+)"\)/g)) need(`progress.${m[1]}`, 'commands.rs');
for (const kind of ['flat', 'image', 'vertex']) need(`material.${kind}`, 'extract.py');

if (problems.length) {
  console.error(problems.join('\n'));
  process.exit(1);
}
console.log(`${langs.length} catálogos, ${es.size} claves: todo en orden.`);
