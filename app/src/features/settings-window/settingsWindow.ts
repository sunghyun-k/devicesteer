// The Settings window's state (settings.html, opened by settings_window.rs): the General pane
// (the viewer's language, i18n.ts; opening the viewer on a session), the Agents pane, which
// installs or removes the `devicesteer` command and the skill links for the coding agents on this
// computer (agent_setup.rs does the work and explains the links), and the Updates pane. The main
// window keeps the launch sheets and the update banner (features/agents).

import { create } from "zustand";
import * as backend from "../../backend/commands";
import type { AgentItem, AgentStatus, AppLocation } from "../../backend/types";
import { errorText, msg, t, type Text } from "../../i18n/i18n";
import { prefs } from "../../lib/prefs";
import { downloadingText, noteText } from "../agents/agentText";

export const TABS = { general: "prefs.general", agents: "prefs.agents", updates: "prefs.updates" } as const;
export type Tab = keyof typeof TABS;
export const isTab = (x: unknown): x is Tab => typeof x === "string" && x in TABS;

interface SettingsWindowState {
  tab: Tab;
  version: string;
  openOnSession: boolean;
  // ---------- agents ----------
  status: AgentStatus | undefined;
  appLocation: AppLocation | undefined;
  busy: boolean;
  /** The item a Replace button was pressed for: the next press confirms. */
  confirming: string | undefined;
  /** The Agents pane's status line: what the last change did, or what failed. */
  said: { text: string; error: boolean };
  // ---------- updates ----------
  update: backend.Update | undefined;
  checking: boolean;
  installing: boolean;
  /** What the Updates pane says, as messages so a language change says it again. */
  updateText: Text;
  /** What the found update's note says: ready, downloading, or failed. */
  foundText: Text;
}

export const useSettingsWindow = create<SettingsWindowState>()(() => ({
  tab: "general",
  version: "",
  openOnSession: true,
  status: undefined,
  appLocation: undefined,
  busy: false,
  confirming: undefined,
  said: { text: "", error: false },
  update: undefined,
  checking: false,
  installing: false,
  updateText: msg("prefs.updatesFrom"),
  foundText: "",
}));

const get = useSettingsWindow.getState;
const set = useSettingsWindow.setState;

/** Mac settings windows are titled by the pane they show. */
export function showTitle(): void {
  document.title = t(TABS[get().tab]);
  void backend.setWindowTitle(document.title).catch(() => {});
}

export function showTab(tab: Tab): void {
  prefs.set("settings-tab", tab);
  set({ tab });
  showTitle();
  if (tab === "agents") void refresh();
}

// ---------- general ----------

export function setOpenOnSession(on: boolean): void {
  set({ openOnSession: on });
  // The file couldn't be written: the switch shows what still holds.
  backend.setOpenOnSession(on).catch(() => set({ openOnSession: !on }));
}

// ---------- agents ----------

function say(text: string, error = false): void {
  set({ said: { text, error } });
}

export async function refresh(): Promise<void> {
  if (get().busy) return;
  try {
    set({ status: await backend.agentSetupStatus() });
  } catch (e) {
    say(errorText(e), true);
  }
}

export async function change(i: AgentItem, install: boolean, replace = false): Promise<void> {
  set({ busy: true });
  try {
    const r = await backend.agentSetupChange(i.id, install, replace);
    say(r.note ? noteText(r.note) : t(install ? "prefs.installed" : "prefs.removed", { path: i.path }));
  } catch (e) {
    say(errorText(e), true);
  } finally {
    set({ busy: false, confirming: undefined });
    await refresh();
  }
}

/** A Replace button: the first press asks, the second replaces. */
export function replace(i: AgentItem): void {
  if (get().confirming !== i.id) set({ confirming: i.id });
  else void change(i, true, true);
}

/** The move sheet is the main window's: it comes forward with the sheet open. */
export function moveToApplications(): void {
  void backend.showMoveSheet().catch((e) => say(errorText(e), true));
}

// ---------- updates ----------

export async function checkForUpdate(): Promise<void> {
  set({ checking: true, updateText: msg("prefs.checking") });
  try {
    const u = await backend.checkForUpdate();
    if (u) set({ update: u, updateText: msg("prefs.versionAvailable", { version: u.version }), foundText: msg("prefs.updateReady", { version: u.version }) });
    else set({ update: undefined, updateText: msg("prefs.upToDate") });
  } catch (e) {
    set({ updateText: msg("prefs.checkFailed", { error: String(e) }) });
  } finally {
    set({ checking: false });
  }
}

export async function installUpdate(): Promise<void> {
  const u = get().update;
  if (!u) return;
  set({ installing: true });
  try {
    await backend.installUpdate(u, (fraction) => set({ foundText: downloadingText(u.version, fraction) }));
    await backend.restartApp();
  } catch (e) {
    set({ installing: false, foundText: t("update.failed", { error: String(e) }) });
  }
}

/** Starts the window: its version, where the app runs, the pane asked for, and the events that
 * change them. Called once. */
export function startSettingsWindow(): void {
  void (async () => {
    set({ version: await backend.appVersion().catch(() => "") });
    set({ appLocation: await backend.appLocation().catch(() => undefined) });
  })();
  // Kept in the state directory, not web storage: the daemon reads it when an agent starts a
  // session and no viewer runs (devicesteer_daemon::viewer).
  void backend
    .openOnSession()
    .then((on) => set({ openOnSession: on }))
    .catch(() => {});
  // Opened again for a given pane while it is open (the main window's first-launch sheet).
  void backend.onSettingsTab((tab) => isTab(tab) && showTab(tab)).catch(() => {});
  // Coming back to the window: the person may have changed the links in a terminal meanwhile.
  window.addEventListener("focus", () => {
    if (get().tab === "agents") void refresh();
  });
  const asked = new URLSearchParams(location.search).get("tab");
  const saved = prefs.get("settings-tab");
  showTab(isTab(asked) ? asked : isTab(saved) ? saved : "general");
}
