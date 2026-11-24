import { defineConfig } from "vite";

// Tauri expects a fixed port in dev and serves `dist/` in release builds.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: { target: "safari16", outDir: "dist" },
});
