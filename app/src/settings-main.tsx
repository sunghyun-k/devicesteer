// The Settings window (settings.html, opened by settings_window.rs). Nothing talks to the backend
// while modules load: `startSettingsWindow` starts that, after the mock (if any) is installed.

// First, so the components' CSS modules come after it and win where they refine it.
import "./styles/global.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { SettingsApp } from "./features/settings-window/SettingsApp";
import { startSettingsWindow } from "./features/settings-window/settingsWindow";
import { followLanguageChanges } from "./i18n/i18n";
import { isMac } from "./lib/platform";

// Dev only: `settings.html?mock` runs the window against the fake backend (see mock/mock.ts).
if (import.meta.env.DEV && new URLSearchParams(location.search).has("mock")) (await import("./mock/mock")).installMock("settings");

// The title and the traffic lights sit over the toolbar (titleBarStyle Overlay); it keeps room.
if (isMac()) document.documentElement.classList.add("mac");
followLanguageChanges();
startSettingsWindow();
createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <SettingsApp />
  </StrictMode>,
);
