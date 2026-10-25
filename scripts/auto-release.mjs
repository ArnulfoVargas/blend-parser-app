// Runs only after CI has validated SOURCE_SHA. Never pushes to main.
import { execFileSync, spawnSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

export function compareVersions(a, b) {
  const left = a.split(".").map(Number);
  const right = b.split(".").map(Number);
  for (let i = 0; i < 3; i++) {
    if (left[i] !== right[i]) return left[i] - right[i];
  }
  return 0;
}

export function nextVersion(base, tags, bump = "patch") {
  if (!["patch", "minor", "major"].includes(bump)) throw new Error("Invalid version bump");
  let latest = base;
  for (const tag of tags) {
    const version = /^(?:alpha-|beta-|v)(\d+\.\d+\.\d+)$/.exec(tag)?.[1];
    if (version && compareVersions(version, latest) > 0) latest = version;
  }
  const [major, minor, patch] = latest.split(".").map(Number);
  if (bump === "major") return `${major + 1}.0.0`;
  if (bump === "minor") return `${major}.${minor + 1}.0`;
  return `${major}.${minor}.${patch + 1}`;
}

export function autoRelease() {
  const source = process.env.SOURCE_SHA;
  if (!/^[a-f0-9]{40}$/.test(source ?? "")) throw new Error("SOURCE_SHA must be a commit SHA");
  const git = (...args) => execFileSync("git", args, { encoding: "utf8" }).trim();
  const marker = `release-source-${source}`;
  const output = (tag) => {
    if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `tag=${tag}\n`);
    console.log(`Release: ${tag}`);
    return tag;
  };
  // The marker and version tag are pushed atomically. Reruns reuse the same release.
  for (let attempt = 0; attempt < 10; attempt++) {
    git("fetch", "origin", "--tags");
    if (git("tag", "--list", marker)) {
      const tag = git("tag", "--points-at", `${marker}^{commit}`)
        .split("\n").find((name) => /^alpha-\d+\.\d+\.\d+$/.test(name));
      if (!tag) throw new Error(`Release tag missing for ${marker}`);
      return output(tag);
    }
    if (attempt === 0 && git("rev-parse", "HEAD") !== source) {
      throw new Error("Checkout does not match the validated commit");
    }
    const base = JSON.parse(readFileSync("package.json", "utf8")).version;
    const version = nextVersion(base, git("tag", "--list").split("\n"), process.env.VERSION_BUMP);
    const tag = `alpha-${version}`;
    execFileSync(process.execPath, ["scripts/release.mjs", version], { stdio: "inherit" });
    git("tag", marker);
    const push = spawnSync("git", ["push", "--atomic", "origin", `refs/tags/${tag}`, `refs/tags/${marker}`], { stdio: "inherit" });
    if (push.status === 0) return output(tag);
    // Remove only the two tags we just created locally; never change remote tags.
    git("tag", "-d", tag, marker);
    if (!git("ls-remote", "--tags", "origin", `refs/tags/${tag}`, `refs/tags/${marker}`)) {
      throw new Error("Tag push failed (permissions or network); no version was reserved");
    }
    // Another release won the reservation. Fetch its tags and choose a new version.
  }
  throw new Error("Could not reserve a release version after 10 attempts");
}

if (process.argv[1] === fileURLToPath(import.meta.url)) autoRelease();
