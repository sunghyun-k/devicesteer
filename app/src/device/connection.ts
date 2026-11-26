// The selected device's connection: selecting a device attaches to its daemon without video (the
// settings, the VoiceOver warning and the agent session work from there). View Screen asks for the
// frames; a session that starts asks for them too. Stop Viewing drops them and the stream stops,
// unless something else needs it. The daemon's messages for the attachment are dispatched here:
// video to the stream, input to the overlay, the rest to the store.

import * as backend from "../backend/commands";
import type { Config, ControlEvent, DeviceInfo, DeviceSettings, InputEvent, KeyboardState, Orientation, RecordingEvent } from "../backend/types";
import { parseWire } from "../backend/wire";
import { errorText, msg, t, type Text } from "../i18n/i18n";
import { endPress } from "../input/touch";
import { prefs } from "../lib/prefs";
import { indicators } from "../overlay/indicators";
import { screen } from "../stream/screen";
import { closeSidebarIfNarrow, getViewer, sessionOf, setSettingsMessage, setStage, setViewer } from "../store/viewer";
import { onRecording } from "../features/stage/capture";
import { showNote } from "../features/stage/notes";
import { readSettings, resetSettings, showSettings } from "../features/inspector/deviceSettings";
import { forgetKeyboard, onKeyboard } from "../features/keyboard/keyboard";
import { hidePairing } from "../features/setup/pairing";
import { hideSetup, showSetup } from "../features/setup/setup";
import { hideSessionNotice, onControl } from "../features/session/session";
import { report } from "./report";
import { resetStats } from "./stats";
import { deviceName, osLabel, unusable } from "./text";

/** Which attachment the channel's messages belong to. */
let viewerGen = 0;
let statsTimer: number | undefined;
/** Devices whose screen was showing when the person picked another: shown again when they come
 * back. In memory only: after a restart, picking a device attaches without video as before, since
 * the stream lights the device's screen-sharing indicator, which nobody asked for yet. */
const viewed = new Set<string>();
/** When the daemon connection last closed: one that closes again within this is left closed
 * (a daemon that crashes as it starts), instead of starting it over and over. */
let lastReattach = -Infinity;
const REATTACH_AFTER_MS = 5000;

/** Every listener that wants the attachment's ticks (the Info pane redraws once a second). */
export const ticks = new EventTarget();

/** A daemon command; a failure shows as a note and still rejects. */
export function send(cmd: string, args: unknown = {}): Promise<unknown> {
  return backend.send(cmd, args).catch((e) => {
    showNote(String(e), { error: true });
    throw e;
  });
}

// The stream's first picture after it (re)started switches the stage to the live screen.
screen.onPicture = () => {
  const s = getViewer();
  if (s.stage !== "live" && s.wantView) setStage("live");
};

/** Parses one wire message and hands it on. */
function onMessage(buf: Uint8Array): void {
  const m = parseWire(buf);
  if (m.kind === "frame") {
    if (getViewer().wantView) screen.frame(m.header, m.data);
    return;
  }
  if (m.kind !== "event") return;
  const ev = m.event;
  switch (ev.event) {
    case "config":
      void screen.configure(ev as unknown as Config);
      break;
    case "control":
      onControl(ev as unknown as ControlEvent);
      break;
    case "input":
      indicators.input(ev as unknown as InputEvent);
      break;
    case "settings":
      showSettings(ev as unknown as Partial<DeviceSettings>);
      break;
    case "orientation":
      // Points of the old orientation mean nothing on the new screen.
      if (screen.orientation(ev as unknown as Orientation)) {
        indicators.clear();
        // Reported at once too: the timer that reports stops while the window is hidden.
        window.setTimeout(report, 300);
      }
      break;
    case "panel":
      // A foldable was folded or unfolded and another panel is in use (PITFALLS #47). The picture
      // and every point shown belong to the old panel: the screen keeps the last picture but takes
      // no touches until the daemon's stream on the new panel sends its `config`.
      screen.panel();
      indicators.clear();
      break;
    case "recording":
      onRecording(ev as unknown as RecordingEvent);
      break;
    case "keyboard":
      onKeyboard(ev as unknown as KeyboardState);
      break;
    case "daemon_closed":
      onDaemonClosed();
      break;
  }
}

/** Drops the video and everything drawn from it; the attachment stays. */
function resetScreen(): void {
  endPress();
  screen.reset();
  indicators.clear();
  setViewer({ zoom: "fit" });
}

/** Leaves the attached device: everything about it goes. */
export async function detach(): Promise<void> {
  hideSetup();
  hidePairing();
  window.clearInterval(statsTimer);
  viewerGen++;
  const was = getViewer().attachedUdid;
  setViewer({ wantView: false, attachedUdid: undefined, recording: undefined, note: undefined });
  forgetKeyboard();
  resetScreen();
  resetSettings();
  onControl({ session: null, human_touching: false, agent_stopped: false });
  setSettingsMessage("");
  resetStats();
  if (was) await backend.detach().catch(() => {});
}

export async function select(d: DeviceInfo, opts: { view?: boolean } = {}): Promise<void> {
  hideSessionNotice(d.udid);
  const s = getViewer();
  // Back to a device whose screen showed when the person left it, or one an agent works on.
  const view = opts.view ?? (viewed.has(d.udid) || !!sessionOf(s, d.udid));
  if (s.selected?.udid === d.udid && s.attachedUdid === d.udid) {
    if (view) void showScreen();
    return;
  }
  // Its row while it is away: it is still selected and picks up by itself when it is back.
  if (s.gone && s.selected?.udid === d.udid) return;
  await detach();
  setViewer({ gone: undefined, selected: d, settingsDisabled: unusable(d) || !!d.setup });
  prefs.set("device", d.udid);
  closeSidebarIfNarrow();
  if (d.unsupported) {
    // Not even setup: no step makes it usable, and the daemon refuses it.
    setSettingsMessage(msg("settings.unsupported"));
    setStage("unavailable", unsupportedText(d));
    return;
  }
  if (d.error) {
    setSettingsMessage(msg("settings.afterAvailable"));
    // The core's reason, in English.
    setStage("unavailable", d.error);
    return;
  }
  if (d.setup) {
    // Nothing to attach to yet: the daemon needs a device that is set up.
    setSettingsMessage(msg("settings.afterSetup"));
    setStage("setup");
    showSetup(d);
    return;
  }
  setStage(view ? "connecting" : "intro");
  if (view) {
    setViewer({ wantView: true });
    viewed.add(d.udid);
  }
  await attach(d);
}

/** What the stage says about a device that is too old. */
function unsupportedText(d: DeviceInfo): Text {
  const os = osLabel(d);
  return os ? msg("device.unsupportedRuns", { os }) : msg("device.unsupported");
}

async function attach(d: DeviceInfo): Promise<void> {
  const gen = ++viewerGen;
  setSettingsMessage(msg("common.connecting"));
  const frames = getViewer().wantView;
  try {
    // Messages of an earlier attachment can still be in flight after another device was picked.
    await backend.attach(d.udid, frames, (m) => {
      if (gen === viewerGen) onMessage(m);
    });
  } catch (e) {
    if (gen !== viewerGen) return;
    setSettingsMessage("");
    setStage("error", msg("stage.connectFailed", { device: deviceName(d, t), error: errorText(e) }));
    return;
  }
  if (gen !== viewerGen) return;
  setViewer({ attachedUdid: d.udid });
  // The screen was asked for while attaching (a session started, View Screen): ask now.
  if (getViewer().wantView && !frames) void showScreen();
  // VoiceOver changes what taps do, so its warning shows whether or not the inspector is open.
  void readSettings();
  void backend
    .keyboardStatus()
    .then((k) => onKeyboard(k))
    .catch(() => {});
  statsTimer = window.setInterval(() => {
    ticks.dispatchEvent(new Event("tick"));
    if (Math.round(performance.now() / 1000) % 2 === 0) report();
  }, 1000);
}

/** View Screen: asks for the frames. */
export async function showScreen(): Promise<void> {
  const s = getViewer();
  const d = s.selected;
  if (!d || unusable(d) || d.setup || s.gone) return;
  setViewer({ wantView: true });
  viewed.add(d.udid);
  if (s.stage !== "live") setStage("connecting", msg("stage.starting"));
  if (!s.attachedUdid) {
    // Still attaching (it asks for frames now), or it failed: try again.
    if (s.stage === "error" || !viewerGen) await attach(d);
    return;
  }
  const gen = viewerGen;
  try {
    await backend.watch(true);
  } catch (e) {
    if (gen !== viewerGen) return;
    // The daemon went away while attached (it exited idle or was stopped to load a new build):
    // attach again, which starts a new one.
    const now = getViewer().selected;
    if (String(e).includes("daemon connection closed") && now) {
      setViewer({ attachedUdid: undefined });
      await attach(now);
      return;
    }
    setStage("error", msg("stage.startFailed", { error: errorText(e) }));
  }
}

/** Stop Viewing: the frames go, the attachment stays. */
export async function stopScreen(): Promise<void> {
  const s = getViewer();
  if (!s.wantView) return;
  setViewer({ wantView: false });
  if (s.selected) viewed.delete(s.selected.udid);
  resetScreen();
  setStage("intro");
  if (s.attachedUdid) await backend.watch(false).catch(() => {});
}

/** Try Again on an error. */
export function retry(): void {
  const s = getViewer();
  const d = s.selected;
  if (!d) return;
  if (s.attachedUdid) void showScreen();
  else {
    setViewer({ selected: undefined });
    void select(d, { view: s.wantView });
  }
}

/** Selects `d` afresh, as if just picked (it became usable, or its setup finished). */
export function reselect(d: DeviceInfo, opts: { view?: boolean } = {}): void {
  setViewer({ selected: undefined });
  void select(d, opts);
}

/** The attached device's daemon went away (stopped to load a new build, or it crashed): attach
 * again, which starts a new one, so the screen and the session panel keep updating. */
function onDaemonClosed(): void {
  const s = getViewer();
  if (!s.selected || !s.attachedUdid || s.gone) return;
  const d = s.selected;
  const show = s.wantView;
  setViewer({ attachedUdid: undefined });
  if (performance.now() - lastReattach < REATTACH_AFTER_MS) {
    setStage("error", msg("stage.daemonClosed"));
    return;
  }
  lastReattach = performance.now();
  reselect(d, { view: show });
}
