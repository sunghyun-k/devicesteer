// Starts the main window's work that isn't a component's: the listeners and timers that live as
// long as the page, and the first look at the devices. Runs once, before the first render, so
// StrictMode's double effects never start any of it twice.

import * as backend from "../backend/commands";
import type { AgentSession } from "../backend/types";
import { select } from "../device/connection";
import { unusable } from "../device/text";
import { onLanguageChange, t } from "../i18n/i18n";
import { installKeyboard } from "../input/keyboard";
import { installTouchGuards } from "../input/touch";
import { isMac } from "../lib/platform";
import { prefs } from "../lib/prefs";
import { indicators } from "../overlay/indicators";
import { getViewer, setViewer, useViewer } from "../store/viewer";
import { initAgents } from "../features/agents/agents";
import { followDevices, loadDevices } from "../features/devices/deviceList";
import { followKeyboard } from "../features/keyboard/keyboard";
import { followSessions } from "../features/session/session";

/** The app menu's labels (view_menu.rs), which the shell can't translate itself. */
const MENU_KEYS = [
  "about", "services", "hide", "hideOthers", "showAll", "quit", "file", "closeWindow", "edit", "undo", "redo", "cut", "copy",
  "paste", "selectAll", "view", "showTouches", "enterFullScreen", "window", "minimize", "zoom", "help", "settings", "settingsWindow",
] as const;

function sendMenuLabels(): void {
  const labels = Object.fromEntries(MENU_KEYS.map((k) => [k, t(`menu.${k}`, { app: "DeviceSteer" })]));
  void backend.setMenuLabels(labels).catch(() => {});
}

/** "Show Touches" (View menu, ⇧⌘T; view_menu.rs) turns the touch discs off. */
function followShowTouches(): void {
  const apply = (on: boolean) => {
    indicators.setShowTouches(on);
    prefs.set("show-touches", on ? "1" : "0");
  };
  indicators.setShowTouches(getViewer().showTouches);
  useViewer.subscribe((s, prev) => {
    if (s.showTouches !== prev.showTouches) apply(s.showTouches);
  });
  void backend.onShowTouches((on) => setViewer({ showTouches: on })).catch(() => {});
  // The menu starts checked; make it match what was saved.
  void backend.setShowTouches(getViewer().showTouches).catch(() => {});
}

export function startViewer(): void {
  // On macOS the window has no title bar (titleBarStyle Overlay) and the traffic lights sit over
  // the top-left corner; the headers keep room for them. Elsewhere the window keeps its frame.
  const mac = isMac();
  if (mac) document.documentElement.classList.add("mac");
  sendMenuLabels();
  onLanguageChange(sendMenuLabels);
  followShowTouches();
  // A press kept from the device points at the session's Take over.
  indicators.onBlocked = (on) => setViewer((v) => ({ nudge: on ? v.nudge + 1 : 0 }));
  installTouchGuards();
  installKeyboard(mac);
  followKeyboard();
  followSessions();
  followDevices();
  initAgents();
  void firstLook();
}

/** The devices, the sessions already running, and which device to open. */
async function firstLook(): Promise<void> {
  // A device to open right away (`DEVICESTEER_VIEWER_UDID`): selected and shown.
  const udid = await backend.initialUdid().catch(() => null);
  const known = await backend.knownSessions().catch(() => ({}) as Record<string, AgentSession>);
  setViewer((v) => ({ sessions: { ...v.sessions, ...known } }));
  await loadDevices();
  // Sessions that started before the viewer did: one question to each device's running daemon.
  const v = getViewer();
  const asked = v.devices.filter((d) => !unusable(d) && !d.setup && !v.sessions[d.udid]).map((d) => d.udid);
  if (asked.length) {
    const found = await backend.probeSessions(asked).catch(() => ({}) as Record<string, AgentSession>);
    setViewer((x) => ({ sessions: { ...x.sessions, ...found } }));
  }
  const devices = getViewer().devices;
  if (udid) {
    void select(devices.find((d) => d.udid === udid) ?? { udid, transports: [] }, { view: true });
    return;
  }
  const last = prefs.get("device");
  // A device an agent works on comes first: that is what the person opened the viewer to see.
  const sessions = getViewer().sessions;
  const working = devices.find((d) => !unusable(d) && sessions[d.udid]?.owner === "agent");
  const pick = working ?? devices.find((d) => d.udid === last && !unusable(d)) ?? devices.find((d) => !unusable(d));
  // Pair over Wi-Fi… pressed while the list loaded: the card stays.
  if (getViewer().pairing) return;
  if (pick) void select(pick);
}
