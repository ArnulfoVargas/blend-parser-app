import assert from "node:assert/strict";
import test from "node:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { findInstallers, mergeManifest } from "./publish-signed-windows.mjs";

const nsis = { signature: "nsis-sig", url: "https://example/setup.exe" };
const msi = { signature: "msi-sig", url: "https://example/app.msi" };

test("windows entries are replaced and Linux and both macOS architectures are kept", () => {
  const current = {
    version: "0.0.3",
    notes: "notas",
    pub_date: "2026-10-25T00:00:00Z",
    platforms: {
      "linux-x86_64": { signature: "l", url: "u" },
      "darwin-x86_64": { signature: "mac-intel", url: "https://example/intel.app.tar.gz" },
      "darwin-aarch64": { signature: "mac-arm", url: "https://example/arm.app.tar.gz" },
      "windows-x86_64": { signature: "old", url: "old" },
    },
  };
  const merged = mergeManifest(current, { version: "0.0.3", pubDate: "later", nsis, msi });
  assert.deepEqual(merged.platforms["linux-x86_64"], { signature: "l", url: "u" });
  assert.deepEqual(merged.platforms["darwin-x86_64"], current.platforms["darwin-x86_64"]);
  assert.deepEqual(merged.platforms["darwin-aarch64"], current.platforms["darwin-aarch64"]);
  assert.deepEqual(merged.platforms["windows-x86_64"], nsis);
  assert.deepEqual(merged.platforms["windows-x86_64-nsis"], nsis);
  assert.deepEqual(merged.platforms["windows-x86_64-msi"], msi);
  assert.equal(merged.pub_date, "2026-10-25T00:00:00Z");
  assert.equal(merged.notes, "notas");
});

test("a manifest from another version is not reused", () => {
  const stale = { version: "0.0.2", notes: "viejas", platforms: { "linux-x86_64": { signature: "l", url: "u" } } };
  const merged = mergeManifest(stale, { version: "0.0.3", pubDate: "now", nsis, msi });
  assert.equal(merged.version, "0.0.3");
  assert.equal(merged.pub_date, "now");
  assert.equal(merged.platforms["linux-x86_64"], undefined);
});

test("no manifest yet starts a new one", () => {
  const merged = mergeManifest(null, { version: "0.0.3", pubDate: "now", nsis, msi });
  assert.deepEqual(Object.keys(merged.platforms).sort(), ["windows-x86_64", "windows-x86_64-msi", "windows-x86_64-nsis"]);
});

test("installers are found inside the signed artifact's folders", () => {
  const dir = mkdtempSync(join(tmpdir(), "signed-"));
  try {
    mkdirSync(join(dir, "nsis"));
    mkdirSync(join(dir, "msi"));
    writeFileSync(join(dir, "nsis", "BlendParser_0.0.3_x64-setup.exe"), "");
    writeFileSync(join(dir, "msi", "BlendParser_0.0.3_x64_en-US.msi"), "");
    const found = findInstallers(dir);
    assert.match(found.nsis, /BlendParser_0\.0\.3_x64-setup\.exe$/);
    assert.match(found.msi, /BlendParser_0\.0\.3_x64_en-US\.msi$/);
    rmSync(join(dir, "msi"), { recursive: true });
    assert.throws(() => findInstallers(dir));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
