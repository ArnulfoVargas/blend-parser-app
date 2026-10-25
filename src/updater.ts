// Self-update from GitHub Releases: Tauri's updater reads latest.json from the
// newest release, checks its signature against the public key in
// tauri.conf.json, installs it and the app restarts.
import { getVersion } from "@tauri-apps/api/app";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";

export type { Update };

export const currentVersion = () => getVersion();

/** The newer release, or null when this is the latest. Throws when offline. */
export const findUpdate = () => check({ timeout: 15000 });

/** Downloads and installs `update`, reporting progress from 0 to 1 (null while unknown), then restarts. */
export async function installUpdate(update: Update, onProgress: (fraction: number | null) => void) {
  let total = 0;
  let received = 0;
  await update.downloadAndInstall((e) => {
    if (e.event === "Started") total = e.data.contentLength ?? 0;
    else if (e.event === "Progress") {
      received += e.data.chunkLength;
      onProgress(total > 0 ? Math.min(1, received / total) : null);
    } else onProgress(1);
  });
  await relaunch();
}
