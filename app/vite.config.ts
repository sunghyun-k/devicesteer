import { resolve } from "node:path";
import babel from "@rolldown/plugin-babel";
import react, { reactCompilerPreset } from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Tauri expects a fixed port in dev and serves `dist/` in release builds. Two pages: the main
// window and the Settings window (settings_window.rs opens settings.html), each its own React root.
// React Compiler memoizes components and hooks; the stream and input modules aren't React and
// never run through it.
export default defineConfig({
  clearScreen: false,
  plugins: [react(), babel({ presets: [reactCompilerPreset()] })],
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    target: "safari16",
    outDir: "dist",
    rollupOptions: { input: { main: resolve(import.meta.dirname, "index.html"), settings: resolve(import.meta.dirname, "settings.html") } },
  },
  test: {
    include: ["src/**/*.test.ts"],
  },
});
