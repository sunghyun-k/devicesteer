// Every Tauri command and event the pages use, typed. Nothing else calls `invoke` or `listen`:
// components go through the stores and modules that call these. A command a window calls must be
// in that window's capabilities (src-tauri/capabilities/*.json).

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import type {
  AgentNote,
  AgentSession,
  AgentSessionMessage,
  AgentStatus,
  AppLocation,
  DeviceList,
  KeyboardState,
  SetupProgress,
  SetupStatus,
  SetupStep,
  TouchStep,
} from "./types";

export type { Update };

// ---------- the device's daemon ----------

/** Attaches to `udid`'s daemon; every wire message (devicesteer_daemon::wire) comes to `onMessage`. */
export function attach(udid: string, frames: boolean, onMessage: (m: Uint8Array) => void): Promise<unknown> {
  const channel = new Channel<ArrayBuffer | number[]>();
  channel.onmessage = (m) => onMessage(m instanceof ArrayBuffer ? new Uint8Array(m) : Uint8Array.from(m));
  return invoke("attach", { udid, frames, channel });
}

export const watch = (frames: boolean) => invoke("watch", { frames });
export const detach = () => invoke("detach");

/** A daemon command for the attached device (`devicesteer_daemon::wire::Request`). */
export const send = <T = unknown>(cmd: string, args: unknown = {}) => invoke<T>("send", { cmd, args });

export const sendTouch = (step: TouchStep) => send("touch", step);
export const sendKey = (code: string, down: boolean) => send("keyboard", { code, down });
export const keyboardStatus = () => send<KeyboardState>("keyboard_status");

/** The page's numbers for viewer.log. */
export const report = (stats: unknown) => invoke("report", { stats });

// ---------- devices and sessions ----------

export const listDevices = () => invoke<DeviceList>("list_devices");
export const openAppleDevicesStore = () => invoke("open_apple_devices_store");
/** A device to open right away (`DEVICESTEER_VIEWER_UDID`). */
export const initialUdid = () => invoke<string | null>("initial_udid");
export const knownSessions = () => invoke<Record<string, AgentSession>>("known_sessions");
export const probeSessions = (udids: string[]) => invoke<Record<string, AgentSession>>("probe_sessions", { udids });

export const onAgentSession = (fn: (m: AgentSessionMessage) => void) => listen<AgentSessionMessage>("agent-session", (e) => fn(e.payload));

// ---------- setup and pairing ----------

export const setupStatus = (udid: string) => invoke<SetupStatus>("setup_status", { udid });

export function setupRun(udid: string, step: SetupStep, waitSecs: number, onProgress: (p: SetupProgress) => void): Promise<SetupStatus> {
  const channel = new Channel<SetupProgress>();
  channel.onmessage = onProgress;
  return invoke<SetupStatus>("setup_run", { udid, step, waitSecs, channel });
}

export function setupPairWifi(waitSecs: number, onProgress: (p: SetupProgress) => void): Promise<SetupStatus> {
  const channel = new Channel<SetupProgress>();
  channel.onmessage = onProgress;
  return invoke<SetupStatus>("setup_pair_wifi", { waitSecs, channel });
}

export const setupCancel = () => invoke("setup_cancel");

// ---------- capture, clipboard and keyboard ----------

export const recordingPath = (stamp: string) => invoke<string>("recording_path", { stamp });
export const saveScreenshot = (png: Uint8Array, stamp: string) => invoke<string>("save_screenshot", png, { headers: { "x-stamp": stamp } });
export const reveal = (path: string) => invoke("reveal", { path });

export const clipboardToDevice = () => invoke<Record<string, unknown>>("clipboard_to_device");
export const clipboardFromDevice = () => invoke<Record<string, unknown>>("clipboard_from_device");
/** Returns false where the shell can't keep the clipboards in step (shared_clip.rs). */
export const clipboardSync = (on: boolean) => invoke<boolean>("clipboard_sync", { on });
export const restoreInputSource = () => invoke("restore_input_source");
/** The Mac's input source changed (view_menu.rs watches it, PITFALLS #46). */
export const onInputSourceChanged = (fn: () => void) => listen("input-source-changed", fn);

// ---------- the app menu and windows ----------

export const setShowTouches = (on: boolean) => invoke("set_show_touches", { on });
export const onShowTouches = (fn: (on: boolean) => void) => listen<boolean>("show-touches", (e) => fn(e.payload));
export const setMenuLabels = (labels: Record<string, string>) => invoke("set_menu_labels", { labels });

/** Opens the Settings window at `tab` (settings_window.rs). */
export const openSettings = (tab: "agents" | "updates" | null = null) => invoke("open_settings", { tab });
/** Settings › Agents asks the main window for its move sheet. */
export const onMoveSheet = (fn: () => void) => listen("move-sheet", fn);
export const showMoveSheet = () => invoke("show_move_sheet");
/** The Settings window was asked to show a pane while open. */
export const onSettingsTab = (fn: (tab: string) => void) => listen<string>("settings-tab", (e) => fn(e.payload));
export const setWindowTitle = (title: string) => getCurrentWindow().setTitle(title);

// ---------- the app itself ----------

export const appVersion = () => invoke<string>("app_version");
export const appLocation = () => invoke<AppLocation>("app_location");
export const moveToApplications = (replace: boolean) => invoke("app_move_to_applications", { replace });
export const agentSetupStatus = () => invoke<AgentStatus>("agent_setup_status");
export const agentSetupChange = (id: string, install: boolean, replace: boolean) =>
  invoke<{ note: AgentNote | null }>("agent_setup_change", { id, install, replace });
export const openOnSession = () => invoke<boolean>("open_on_session");
export const setOpenOnSession = (on: boolean) => invoke("set_open_on_session", { on });

export const checkForUpdate = () => check();
export const restartApp = () => relaunch();

/** Downloads and installs `u`, reporting the fraction done when the size is known. */
export async function installUpdate(u: Update, progress: (fraction: number | undefined) => void): Promise<void> {
  let total = 0;
  let got = 0;
  await u.downloadAndInstall((ev) => {
    if (ev.event === "Started") total = ev.data.contentLength ?? 0;
    if (ev.event === "Progress") got += ev.data.chunkLength;
    progress(total ? got / total : undefined);
  });
}
