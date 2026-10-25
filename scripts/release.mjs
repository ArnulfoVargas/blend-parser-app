// Bumps the app version everywhere, commits and tags it.
//
//   npm run release -- 0.0.2            -> commit + tag alpha-0.0.2
//   npm run release -- 0.1.0 --channel v -> tag v0.1.0
//   add --push to push main and the tag (this starts the Release workflow)
import { execSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";

const args = process.argv.slice(2);
const version = args.find((a) => !a.startsWith("--"));
const channel = args.includes("--channel") ? args[args.indexOf("--channel") + 1] : "alpha";
const push = args.includes("--push");

if (!version || !/^\d+\.\d+\.\d+$/.test(version)) {
  console.error("Uso: npm run release -- <mayor.menor.parche> [--channel alpha|beta|v] [--push]");
  process.exit(1);
}
const tag = channel === "v" ? `v${version}` : `${channel}-${version}`;
const run = (cmd) => execSync(cmd, { stdio: "inherit" });
const out = (cmd) => execSync(cmd, { encoding: "utf8" }).trim();

if (out("git status --porcelain")) {
  console.error("Hay cambios sin commit. Haz commit o stash antes de publicar.");
  process.exit(1);
}
if (out(`git tag --list ${tag}`)) {
  console.error(`El tag ${tag} ya existe.`);
  process.exit(1);
}

const editJson = (file, edit) => {
  const data = JSON.parse(readFileSync(file, "utf8"));
  edit(data);
  writeFileSync(file, JSON.stringify(data, null, 2) + "\n");
};
editJson("package.json", (p) => (p.version = version));
editJson("package-lock.json", (p) => {
  p.version = version;
  if (p.packages?.[""]) p.packages[""].version = version;
});
editJson("src-tauri/tauri.conf.json", (c) => (c.version = version));
const cargo = readFileSync("Cargo.toml", "utf8");
const bumped = cargo.replace(/(\[workspace\.package\][^[]*?\nversion = ")[^"]+(")/, `$1${version}$2`);
if (bumped === cargo) {
  console.error("No encontré la versión en [workspace.package] de Cargo.toml.");
  process.exit(1);
}
writeFileSync("Cargo.toml", bumped);
run("cargo update --workspace --offline");

run("git add package.json package-lock.json src-tauri/tauri.conf.json Cargo.toml Cargo.lock");
run(`git commit -m "Versión ${version}"`);
run(`git tag -a ${tag} -m "BlendParser ${tag}"`);
console.log(`\nListo: ${tag}.`);
if (push) run(`git push origin HEAD ${tag}`);
else console.log(`Para publicar: git push origin HEAD ${tag}`);
