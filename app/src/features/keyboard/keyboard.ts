// "Type from Mac Keyboard" and the clipboard: the state behind the Keyboard menu. The keys
// themselves go from input/keyboard.ts.
//
// Turning forwarding on attaches a hardware keyboard too: without one, the keys go through the
// keyboard surface, whose input source is stuck (PITFALLS #35), and Control-Space can't switch
// it. The viewer asks for it `while_connected`, so the daemon removes it when the viewer goes
// away or an agent gets the device; the viewer removes it when forwarding stops or the screen
// isn't live. Either way the on-screen keyboard comes back (PITFALLS #36).
//
// "Use Shared Clipboard": the Tauri shell keeps the Mac and device clipboards in step (see
// app/src-tauri/src/shared_clip.rs) while it is on, the screen is live and no agent holds the
// device; the page only tells it when that changes. The text and what was synced last stay in
// the shell. On by default, like Xcode's Device Hub's.

import * as backend from "../../backend/commands";
import type { KeyboardState } from "../../backend/types";
import { stats } from "../../device/stats";
import { deviceKind } from "../../device/text";
import { errorText, t } from "../../i18n/i18n";
import { releaseKeys } from "../../input/keyboard";
import { prefs } from "../../lib/prefs";
import { getViewer, humanHasDevice, setViewer, useViewer, type ViewerState } from "../../store/viewer";
import { showNote } from "../stage/notes";
import { send } from "../../device/connection";

/** The attached keyboard is the one this viewer asked for. */
let ours = false;
/** A connect or disconnect is on its way. */
let busy = false;
/** What the shell was last told: syncing or not (null: nothing yet). */
let clipSyncing: boolean | null = null;

/** Forwarding is on and can work now: the screen is live and no agent holds the device. */
export const forwardingActive = (s: ViewerState = getViewer()): boolean => s.keyboard.forwarding && s.stage === "live" && humanHasDevice(s);

/** What the keyboard button shows: on (keys go, with the hardware keyboard attached), connecting
 * (it is being attached), paused (on, but an agent has the device), or off. */
export function keyboardButtonState(s: Pick<ViewerState, "keyboard" | "control">): "on" | "connecting" | "paused" | "off" {
  if (!s.keyboard.forwarding) return "off";
  if (!humanHasDevice(s)) return "paused";
  return s.keyboard.hardware ? "on" : "connecting";
}

function syncSharedClipboard(): void {
  const s = getViewer();
  const want = s.sharedClipboard && !!s.attachedUdid && s.stage === "live" && humanHasDevice(s);
  if (want === clipSyncing) return;
  clipSyncing = want;
  backend
    .clipboardSync(want)
    .then((supported) => {
      // No way to see password managers' private items on this platform (shared_clip.rs).
      if (supported === false) setViewer({ sharedClipboardUnsupported: true });
    })
    .catch((e) => {
      stats.lastError = String(e);
    });
}

/** Attaches the hardware keyboard while forwarding is active, and removes the one this viewer
 * attached when it isn't. Runs whenever any of that changes (`followKeyboard`); the daemon's
 * `keyboard` event brings the result back here. */
function syncKeyboard(): void {
  syncSharedClipboard();
  const s = getViewer();
  if (busy || !s.attachedUdid) return;
  const want = forwardingActive(s);
  let cmd: "keyboard_connect" | "keyboard_disconnect";
  if (want && !s.keyboard.hardware) cmd = "keyboard_connect";
  else if (!want && ours && s.keyboard.hardware) cmd = "keyboard_disconnect";
  else return;
  busy = true;
  releaseKeys();
  const udid = s.attachedUdid;
  void backend
    .send<KeyboardState>(cmd, cmd === "keyboard_connect" ? { while_connected: true } : {})
    .then((r) => {
      if (udid !== getViewer().attachedUdid) return;
      ours = cmd === "keyboard_connect";
      busy = false;
      onKeyboard(r);
    })
    .catch((e) => {
      busy = false;
      if (udid !== getViewer().attachedUdid) return;
      // Not again until something changes: a connect that fails would fail in a loop.
      if (cmd === "keyboard_connect") {
        setViewer((v) => ({ keyboard: { ...v.keyboard, forwarding: false } }));
        showNote(t("keyboard.connectFailed", { error: errorText(e) }), { error: true });
      }
    });
}

/** Keeps the hardware keyboard and the shared clipboard in step with what they depend on.
 * Installed once for the page. */
export function followKeyboard(): void {
  useViewer.subscribe((s, prev) => {
    if (
      s.stage !== prev.stage ||
      s.control !== prev.control ||
      s.attachedUdid !== prev.attachedUdid ||
      s.keyboard !== prev.keyboard ||
      s.sharedClipboard !== prev.sharedClipboard
    )
      syncKeyboard();
  });
}

export function setForwarding(on: boolean): void {
  prefs.set("keyboard-forwarding", on ? "1" : "0");
  if (!on) releaseKeys();
  setViewer((v) => ({ keyboard: { ...v.keyboard, forwarding: on } }));
}

export const toggleForwarding = () => setForwarding(!getViewer().keyboard.forwarding);

/** The daemon's `keyboard` event, or a keyboard command's result. */
export function onKeyboard(k: Partial<KeyboardState>): void {
  const hardware = !!k.hardware;
  // Someone removed it (the daemon on a control change, or `keyboard disconnect`): not ours now.
  if (!hardware) ours = false;
  const v = getViewer();
  if (v.keyboard.hardware !== hardware) setViewer({ keyboard: { ...v.keyboard, hardware } });
  else syncKeyboard();
}

/** The device is gone from this viewer: its keyboard went with the connection (the daemon
 * removes one asked for `while_connected`). */
export function forgetKeyboard(): void {
  ours = false;
  busy = false;
  releaseKeys(false);
  setViewer((v) => ({ keyboard: { ...v.keyboard, hardware: false } }));
}

/** Toggle Software Keyboard: the Eject key, with a hardware keyboard attached, shows or hides the
 * on-screen keyboard (PITFALLS #39). */
export function toggleSoftwareKeyboard(): void {
  if (!getViewer().keyboard.hardware) {
    showNote(t("keyboard.softwareAlone"), { icon: "i-keyboard" });
    return;
  }
  void send("button", { name: "eject" }).catch(() => {});
}

export function toggleSharedClipboard(): void {
  const on = !getViewer().sharedClipboard;
  prefs.set("shared-clipboard", on ? "1" : "0");
  setViewer({ sharedClipboard: on });
}

// The clipboard, both ways. The text goes between the Mac clipboard and the daemon inside the
// Tauri shell: the page sees only how many characters went, and shows or logs none of it.
// Sending doesn't paste: iOS asks the user to allow a paste that ⌘V or our paste triggers
// (PITFALLS #40), but not one the user makes from the edit menu (PITFALLS #43).

let clipBusy = false;

async function clipboardOp(op: () => Promise<Record<string, unknown>>): Promise<Record<string, unknown> | undefined> {
  if (clipBusy) return;
  clipBusy = true;
  try {
    return await op();
  } catch (e) {
    showNote(errorText(e), { error: true });
  } finally {
    clipBusy = false;
  }
}

export async function sendClipboard(): Promise<void> {
  // The agent's input (and a paste waiting for `paste --restore`) would be disturbed.
  if (!humanHasDevice()) {
    showNote(t("clipboard.agentHasDevice"), { error: true });
    return;
  }
  if (await clipboardOp(backend.clipboardToDevice)) showNote(t("clipboard.sent", { device: deviceKind(getViewer().selected) }), { icon: "i-clip-send" });
}

export async function getClipboard(): Promise<void> {
  const r = await clipboardOp(backend.clipboardFromDevice);
  if (!r) return;
  const device = deviceKind(getViewer().selected);
  if (r.kind === "text") showNote(t("clipboard.got", { device }), { icon: "i-clip-get" });
  else if (r.kind === "empty") showNote(t("clipboard.empty", { device }), { icon: "i-clip-get" });
  else showNote(t("clipboard.notText", { device }), { error: true });
}
