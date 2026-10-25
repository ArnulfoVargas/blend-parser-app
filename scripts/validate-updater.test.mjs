import assert from "node:assert/strict";
import test from "node:test";
import { validateUpdaterManifest } from "./validate-updater.mjs";

function manifest() {
  return {
    version: "0.0.6",
    platforms: Object.fromEntries(["windows-x86_64", "linux-x86_64", "darwin-x86_64", "darwin-aarch64"].map(platform => [
      platform, { signature: `${platform}-signature`, url: `https://example.invalid/${platform}` },
    ])),
  };
}

test("complete releases pass for every release channel", () => {
  for (const prefix of ["alpha-", "beta-", "v"]) {
    assert.doesNotThrow(() => validateUpdaterManifest(manifest(), `${prefix}0.0.6`));
  }
});

test("a missing architecture blocks publication, including either Mac build", () => {
  for (const platform of Object.keys(manifest().platforms)) {
    const incomplete = manifest();
    delete incomplete.platforms[platform];
    assert.throws(() => validateUpdaterManifest(incomplete, "alpha-0.0.6"), { message: `Falta ${platform} o su firma` });
  }
});

test("unsigned or undownloadable updates block publication", () => {
  for (const platform of Object.keys(manifest().platforms)) {
    for (const field of ["signature", "url"]) {
      for (const value of [undefined, "", "   ", 42]) {
        const incomplete = manifest();
        incomplete.platforms[platform][field] = value;
        assert.throws(() => validateUpdaterManifest(incomplete, "alpha-0.0.6"));
      }
    }
  }
});

test("a stale manifest or invalid tag blocks publication", () => {
  assert.throws(() => validateUpdaterManifest(manifest(), "alpha-0.0.7"), /Versión incorrecta/);
  assert.throws(() => validateUpdaterManifest(null, "alpha-0.0.6"), /Versión incorrecta/);
  assert.throws(() => validateUpdaterManifest(manifest(), "main"), /Tag inválido/);
});
