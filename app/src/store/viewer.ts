// The main window's state: one store for what the components show. The modules that talk to the
// daemon and the shell (device/, features/*/ model files) write it with `set`; components read
// it with `useViewer(selector)`. What changes per frame or per pointer event (the picture, touch
// discs, the cursor, decoder numbers) is not here: see stream/, overlay/ and input/.

import { create } from "zustand";
import type { AgentSession, ControlEvent, DeviceInfo, DeviceSettings, SetupProgress, SetupStatus, SetupStep } from "../backend/types";
import type { Text } from "../i18n/i18n";
import { prefs } from "../lib/prefs";

/** What the stage shows: nothing selected; the selected device with View Screen; its setup;
 * pairing a new device over Wi-Fi; one that can't be used; one that left the list (`gone`);
 * connecting; an error; or the live screen once a frame drew. */
export type StageState = "none" | "intro" | "setup" | "pair" | "unavailable" | "disconnected" | "connecting" | "error" | "live";

export type Zoom = "fit" | number;

/** The note over the stage: where a file went, or what failed. With a file, `text` has a `{file}`
 * placeholder where its name goes, in bold. Written once in the language of the moment, as a
 * note stays as it was said. */
export interface Note {
  text: string;
  path?: string;
  error?: boolean;
  icon?: string;
}

/** The setup card: the device it is for, the last state read, the step running with its latest
 * progress (and when it began waiting for the person, which starts the countdown), and the last
 * step's error. */
export interface SetupCard {
  device: DeviceInfo;
  status?: SetupStatus;
  run?: { step: SetupStep; progress?: SetupProgress; waitingSince?: number };
  error?: { step: SetupStep; text: Text };
}

/** Pairing over Wi-Fi: waiting for the device to ask, showing the code it asked for, paired
 * (opening the device), or ended with an error. `waitingSince` (`performance.now()`) starts the
 * countdown; the core waits afresh once it has shown the code, and so does the card. */
export interface Pairing {
  phase: "waiting" | "code" | "done" | "failed";
  code?: string;
  waitingSince?: number;
  error?: Text;
}

export interface KeyboardForwarding {
  /** "Type from Mac Keyboard" is on. */
  forwarding: boolean;
  /** Whether a hardware keyboard is attached (anyone's), as the daemon last said. */
  hardware: boolean;
}

export interface ViewerState {
  // ---------- the window ----------
  sidebar: boolean;
  inspector: boolean;
  tab: "settings" | "info";
  /** The open toolbar menu: keys don't go to the device while one is. */
  openMenu: "keyboard" | "more" | null;

  // ---------- devices ----------
  devices: DeviceInfo[];
  /** Whether the device list has come back once. */
  listed: boolean;
  listLoading: boolean;
  listError: string | undefined;
  /** Whether the last list said usbmuxd isn't there: the sidebar shows how to install it. */
  usbmuxdMissing: boolean;
  search: string;
  selected: DeviceInfo | undefined;
  /** Set while the selected device has left the device list (unplugged, or restarting, which
   * turning on Developer Mode does): what to go back to when the same UDID comes back. The
   * selection stays, so it picks up again by itself. */
  gone: { view: boolean } | undefined;
  /** The agent session of every device that has one, by UDID: the daemons tell the shell when one
   * starts, changes owner or ends (session_watch.rs), and the attached device's control events
   * keep its entry current. The sidebar marks them all. */
  sessions: Record<string, AgentSession>;
  /** An agent started on a device other than the one on screen. */
  notice: { device: DeviceInfo; session: AgentSession } | undefined;

  // ---------- the attached device ----------
  /** The device attached to, once `attach` succeeded. */
  attachedUdid: string | undefined;
  /** Whether the screen should show (frames asked for, or about to be). */
  wantView: boolean;
  stage: StageState;
  /** What the stage says: progress while connecting, the reason for an error. */
  stageText: Text;
  control: ControlEvent;
  /** The session's primary button points itself out (a press was kept from the device). */
  nudge: number;
  zoom: Zoom;
  /** The scale the last layout used, to zoom in and out from. */
  scale: number;
  /** Bumped by every zoom change: the stage scrolls to the middle after its layout. */
  zoomSeq: number;
  showTouches: boolean;

  // ---------- the inspector ----------
  settings: Partial<DeviceSettings>;
  /** What the settings pane says; an error is shown as one. */
  settingsMessage: { text: Text; error: boolean };
  settingsBusy: boolean;
  /** No device to change settings on: the pane shows only its message. */
  settingsDisabled: boolean;

  // ---------- keyboard and clipboard ----------
  keyboard: KeyboardForwarding;
  sharedClipboard: boolean;
  /** The shell can't keep the clipboards in step here (shared_clip.rs). */
  sharedClipboardUnsupported: boolean;

  // ---------- capture ----------
  /** The running recording, with its start on the page's clock. */
  recording: { path: string; since: number } | undefined;
  /** The last recording saved, for the More menu. */
  recordedPath: string | undefined;
  note: Note | undefined;

  // ---------- setup and pairing ----------
  setup: SetupCard | undefined;
  pairing: Pairing | undefined;
}

const narrow = (px: number) => window.innerWidth <= px;

export const useViewer = create<ViewerState>()(() => ({
  // Narrow windows start with the panels hidden: they float over the stage there.
  sidebar: !narrow(720) && prefs.get("sidebar") !== "0",
  inspector: !narrow(1000) && prefs.get("inspector") !== "0",
  tab: prefs.get("tab") === "info" ? "info" : "settings",
  openMenu: null,

  devices: [],
  listed: false,
  listLoading: false,
  listError: undefined,
  usbmuxdMissing: false,
  search: "",
  selected: undefined,
  gone: undefined,
  sessions: {},
  notice: undefined,

  attachedUdid: undefined,
  wantView: false,
  stage: "none",
  stageText: "",
  control: { session: null, human_touching: false, agent_stopped: false },
  nudge: 0,
  zoom: "fit",
  scale: 1,
  zoomSeq: 0,
  showTouches: prefs.get("show-touches") !== "0",

  settings: {},
  settingsMessage: { text: "", error: false },
  settingsBusy: false,
  settingsDisabled: true,

  keyboard: { forwarding: prefs.get("keyboard-forwarding") === "1", hardware: false },
  sharedClipboard: prefs.get("shared-clipboard") !== "0",
  sharedClipboardUnsupported: false,

  recording: undefined,
  recordedPath: undefined,
  note: undefined,

  setup: undefined,
  pairing: undefined,
}));

export const getViewer = useViewer.getState;
export const setViewer = useViewer.setState;

/** The session using the attached device, unless it finished. */
export const activeSession = (s: Pick<ViewerState, "control"> = getViewer()): AgentSession | null =>
  s.control.session && s.control.session.owner !== "finished" ? s.control.session : null;

/** The human may send input: no agent session, or one that gave the device to the human. */
export const humanHasDevice = (s: Pick<ViewerState, "control"> = getViewer()): boolean => activeSession(s)?.owner !== "agent";

/** The session using `udid`, if one runs. */
export function sessionOf(s: Pick<ViewerState, "control" | "attachedUdid" | "sessions">, udid: string | undefined): AgentSession | undefined {
  if (!udid) return undefined;
  if (udid === s.attachedUdid) return activeSession(s) ?? undefined;
  return s.sessions[udid];
}

/** What the stage shows, and what it says, written in the current language when it is a message. */
export function setStage(stage: StageState, text: Text = ""): void {
  setViewer({ stage, stageText: text });
}

/** What the settings pane says. */
export function setSettingsMessage(text: Text, error = false): void {
  setViewer({ settingsMessage: { text, error } });
}

/** Shows or hides a side panel, remembering the choice unless `remember` is false (a narrow window
 * hiding one by itself). */
export function showPanel(which: "sidebar" | "inspector", on: boolean, remember = true): void {
  if (remember) prefs.set(which, on ? "1" : "0");
  setViewer({ [which]: on });
}

/** Hides the sidebar after picking in a narrow window, where it floats over the stage. */
export function closeSidebarIfNarrow(): void {
  if (narrow(720)) showPanel("sidebar", false, false);
}
