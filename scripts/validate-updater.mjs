import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const platforms = ["windows-x86_64", "linux-x86_64", "darwin-x86_64", "darwin-aarch64"];

/** Refuse to publish a release if an architecture or its updater signature is missing. */
export function validateUpdaterManifest(manifest, tag) {
  if (!/^(alpha-|beta-|v)\d+\.\d+\.\d+$/.test(tag)) throw new Error("Tag inválido");
  const version = tag.replace(/^(alpha-|beta-|v)/, "");
  if (manifest?.version !== version) throw new Error("Versión incorrecta en latest.json");
  for (const platform of platforms) {
    const entry = manifest.platforms?.[platform];
    if (typeof entry?.signature !== "string" || !entry.signature.trim()
      || typeof entry?.url !== "string" || !entry.url.trim()) {
      throw new Error(`Falta ${platform} o su firma`);
    }
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [file, tag] = process.argv.slice(2);
  if (!file || !tag) throw new Error("Uso: validate-updater.mjs <latest.json> <tag>");
  validateUpdaterManifest(JSON.parse(readFileSync(file, "utf8")), tag);
  console.log("latest.json válido para Windows, Linux y macOS (Intel y Apple Silicon)");
}
