import { defineConfig } from "vite";

// Tauri expects a fixed port and does not want the screen cleared.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true, watch: { ignored: ["**/src-tauri/**", "**/target/**"] } },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: { target: "es2021", sourcemap: false, chunkSizeWarningLimit: 1200 },
});
