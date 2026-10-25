import assert from "node:assert/strict";
import test from "node:test";
import { compareVersions, nextVersion } from "./auto-release.mjs";
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, sep } from "node:path";

test("patch follows the highest release across channels, ignoring markers", () => {
  assert.equal(nextVersion("0.0.2", ["alpha-0.0.9", "v0.1.10", "beta-0.1.9", "release-source-abcd", "alpha-invalid"]), "0.1.11");
});
test("base version is a lower bound", () => {
  assert.equal(nextVersion("1.2.3", ["v0.9.9"]), "1.2.4");
  assert.equal(nextVersion("0.0.2", []), "0.0.3");
});
test("explicit minor and major reset lower components", () => {
  assert.equal(nextVersion("0.0.2", ["alpha-0.3.9"], "minor"), "0.4.0");
  assert.equal(nextVersion("0.0.2", ["alpha-0.3.9"], "major"), "1.0.0");
  assert.throws(() => nextVersion("0.0.2", [], "invalid"));
});
test("versions compare numerically", () => {
  assert.ok(compareVersions("0.0.10", "0.0.9") > 0);
  assert.ok(compareVersions("1.0.0", "0.99.99") > 0);
  assert.equal(compareVersions("1.2.3", "1.2.3"), 0);
});

test("real git release updates manifests, preserves main, and is idempotent", () => {
  const root = mkdtempSync(join(tmpdir(), "blendparser-release-"));
  const repo = join(root, "repo");
  const remote = join(root, "remote.git");
  mkdirSync(repo);
  const run = (command, args, cwd = repo, env = process.env) =>
    execFileSync(command, args, { cwd, env, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
  const git = (...args) => run("git", args);
  try {
    run("git", ["init", "--bare", remote], root);
    git("init", "-b", "main");
    git("config", "user.name", "Release Test");
    git("config", "user.email", "test@example.invalid");
    git("remote", "add", "origin", remote);
    mkdirSync(join(repo, "scripts"));
    mkdirSync(join(repo, "src-tauri"));
    copyFileSync(new URL("./release.mjs", import.meta.url), join(repo, "scripts/release.mjs"));
    copyFileSync(new URL("./auto-release.mjs", import.meta.url), join(repo, "scripts/auto-release.mjs"));
    writeFileSync(join(repo, "package.json"), JSON.stringify({ version: "0.0.2", type: "module" }));
    writeFileSync(join(repo, "package-lock.json"), JSON.stringify({ version: "0.0.2", packages: { "": { version: "0.0.2" } } }));
    writeFileSync(join(repo, "src-tauri/tauri.conf.json"), JSON.stringify({ version: "0.0.2" }));
    writeFileSync(join(repo, "Cargo.toml"), '[workspace]\nmembers = ["src-tauri"]\nresolver = "2"\n[workspace.package]\nversion = "0.0.2"\n');
    writeFileSync(join(repo, "src-tauri/Cargo.toml"), '[package]\nname = "release-fixture"\nversion.workspace = true\nedition = "2021"\n[lib]\npath = "lib.rs"\n');
    writeFileSync(join(repo, "src-tauri/lib.rs"), "");
    run("cargo", ["generate-lockfile", "--offline"]);
    git("add", ".");
    git("commit", "-m", "Validated source");
    git("push", "origin", "main");
    const source = git("rev-parse", "HEAD");
    const output = join(root, "output");
    const env = { ...process.env, SOURCE_SHA: source, VERSION_BUMP: "patch", GITHUB_OUTPUT: output };
    run(process.execPath, ["scripts/auto-release.mjs"], repo, env);
    const tags = git("ls-remote", "--tags", "origin");
    assert.match(tags, /refs\/tags\/alpha-0\.0\.3/);
    assert.match(tags, new RegExp(`refs/tags/release-source-${source}`));
    assert.equal(git("ls-remote", "origin", "refs/heads/main").split(/\s/)[0], source);
    for (const file of ["package.json", "package-lock.json", "src-tauri/tauri.conf.json"]) {
      assert.equal(JSON.parse(readFileSync(join(repo, file), "utf8")).version, "0.0.3");
    }
    assert.match(readFileSync(join(repo, "Cargo.lock"), "utf8"), /version = "0.0.3"/);
    run(process.execPath, ["scripts/auto-release.mjs"], repo, env);
    assert.equal(git("ls-remote", "--tags", "origin"), tags);
    assert.equal(readFileSync(output, "utf8"), "tag=alpha-0.0.3\ntag=alpha-0.0.3\n");
  } finally {
    if (!resolve(root).startsWith(resolve(tmpdir()) + sep)) throw new Error("Unsafe temporary path");
    rmSync(root, { recursive: true, force: true });
  }
});
