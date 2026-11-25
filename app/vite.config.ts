import { resolve } from "node:path";
import { defineConfig } from "vite";

// Tauri expects a fixed port in dev and serves `dist/` in release builds. Two pages: the main
// window and the Settings window (settings_window.rs opens settings.html).
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    target: "safari16",
    outDir: "dist",
    rollupOptions: { input: { main: resolve(import.meta.dirname, "index.html"), settings: resolve(import.meta.dirname, "settings.html") } },
  },
});
