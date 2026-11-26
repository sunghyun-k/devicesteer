// The main window (index.html): the viewer. Nothing talks to the backend while modules load:
// `startViewer` starts that, after the mock (if any) is installed.

// First, so the components' CSS modules come after it and win where they refine it.
import "./styles/global.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app/App";
import { startViewer } from "./app/start";
import { followLanguageChanges } from "./i18n/i18n";

// Dev only: `?mock` runs the viewer against a fake backend in a plain browser (see mock/mock.ts).
// Vite replaces `import.meta.env.DEV` with false in builds, which drops the mock entirely.
if (import.meta.env.DEV && new URLSearchParams(location.search).has("mock")) (await import("./mock/mock")).installMock();

followLanguageChanges();
startViewer();
createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
