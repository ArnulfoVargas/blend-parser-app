// Publishes the Windows installers once SignPath has signed them (Authenticode).
//
//   node scripts/publish-signed-windows.mjs <signed-dir> <tag>
//
// Authenticode changes the installers' bytes, so the updater signatures made at
// build time no longer match: each installer is signed again with the updater
// key (TAURI_SIGNING_PRIVATE_KEY), uploaded to the release with its .sig, and
// its entries in latest.json are rewritten. Linux entries already there are kept.
import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

/** The signed installers in `dir`: the NSIS setup and the MSI, searched recursively. */
export function findInstallers(dir) {
  const files = readdirSync(dir, { recursive: true }).map((f) => join(dir, String(f)));
  const nsis = files.find((f) => /_x64-setup\.exe$/.test(f));
  const msi = files.find((f) => /_x64_[\w-]+\.msi$/.test(f));
  if (!nsis || !msi) throw new Error(`Faltan instaladores firmados en ${dir}`);
  return { nsis, msi };
}

/** latest.json with the Windows entries replaced; NSIS is the default, as tauri-action's updaterJsonPreferNsis. */
export function mergeManifest(manifest, { version, pubDate, nsis, msi }) {
  const base = manifest?.version === version ? manifest : { version, notes: manifest?.notes ?? "", platforms: {} };
  return {
    ...base,
    version,
    pub_date: base.pub_date ?? pubDate,
    platforms: {
      ...base.platforms,
      "windows-x86_64": nsis,
      "windows-x86_64-nsis": nsis,
      "windows-x86_64-msi": msi,
    },
  };
}

function main([dir, tag]) {
  if (!dir || !tag) throw new Error("Uso: publish-signed-windows.mjs <carpeta-firmada> <tag>");
  const repo = process.env.GH_REPO;
  const version = tag.replace(/^(alpha-|beta-|v)/, "");
  const run = (cmd, args, opts = {}) => execFileSync(cmd, args, { stdio: ["ignore", "pipe", "inherit"], encoding: "utf8", shell: process.platform === "win32", ...opts });
  const { nsis, msi } = findInstallers(dir);

  const entry = (file) => {
    run("npx", ["tauri", "signer", "sign", file]); // reads TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)
    const name = file.split(/[\\/]/).pop();
    return { signature: readFileSync(`${file}.sig`, "utf8").trim(), url: `https://github.com/${repo}/releases/download/${tag}/${name}` };
  };
  const signed = { nsis: entry(nsis), msi: entry(msi) };

  run("gh", ["release", "upload", tag, nsis, `${nsis}.sig`, msi, `${msi}.sig`, "--clobber"]);

  const manifestPath = join(dir, "latest.json");
  try {
    run("gh", ["release", "download", tag, "--pattern", "latest.json", "--dir", dir, "--clobber"]);
  } catch {
    // First platform to finish: there is no manifest yet.
  }
  const current = existsSync(manifestPath) ? JSON.parse(readFileSync(manifestPath, "utf8")) : null;
  const merged = mergeManifest(current, { version, pubDate: new Date().toISOString(), ...signed });
  writeFileSync(manifestPath, JSON.stringify(merged, null, 2));
  run("gh", ["release", "upload", tag, manifestPath, "--clobber"]);
  console.log(`Instaladores firmados publicados en ${tag}: ${[nsis, msi].join(", ")}`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) main(process.argv.slice(2));
