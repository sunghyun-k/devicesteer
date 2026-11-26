// The Mac keyboard and the window's shortcuts. Not React: key events go from the window's
// listeners to the daemon without touching component state. Installed once for the page
// (`installKeyboard`).
//
// With "Type from Mac Keyboard" on, keys pressed while the pointer is over the screen (or the
// screen has focus) go to the device as HID key downs and ups, by DOM `code`: the physical key,
// so the device's layout and input source decide what it types (PITFALLS #35). Never while a
// field of the viewer has focus, and never the viewer's own shortcuts. Text composed with a Mac
// input method (Hangul) goes over with the shared clipboard (or "Send Clipboard") and a paste on
// the device. On Windows, Ctrl is Command on the device and the Windows key Control
// (keymap.ts), as on an iPad with a PC keyboard whose modifier keys are swapped.

import * as backend from "../backend/commands";
import { stats } from "../device/stats";
import { cmdKey, editing, MAC_KEYS } from "../lib/platform";
import { getViewer } from "../store/viewer";
import { forwardingActive, getClipboard, sendClipboard, toggleForwarding, toggleSoftwareKeyboard } from "../features/keyboard/keyboard";
import { setZoom, zoomBy } from "../features/stage/zoom";
import { openSettings } from "../features/agents/agents";
import { KeyMapper, type KeyStep } from "./keymap";

/** Which device key each computer key presses (Ctrl is Command on Windows, keymap.ts), and what
 * is held, so every key is let go again. */
const keys = new KeyMapper(!MAC_KEYS);
/** Key steps go out one at a time and in order: an up that overtook its down would leave the key held. */
let keyChain: Promise<unknown> = Promise.resolve();

/** Where keys go when the pointer is over it or it has focus: the screen's canvas and its box. */
let target: { canvas: HTMLCanvasElement; over: boolean } | undefined;

function keySteps(steps: KeyStep[]): void {
  for (const s of steps) keyStep(s.code, s.down);
}

function keyStep(code: string, down: boolean): void {
  keyChain = keyChain
    .then(() => backend.sendKey(code, down))
    .catch((e) => {
      stats.lastError = String(e);
    });
}

/** A whole press of `code`: down, a short hold, up. A down and up sent back to back reach the
 * device within a millisecond, shorter than any real press. */
function keyTap(code: string): void {
  keyStep(code, true);
  keyChain = keyChain.then(() => new Promise((r) => setTimeout(r, KEY_TAP_HOLD_MS)));
  keyStep(code, false);
}
const KEY_TAP_HOLD_MS = 30;

/** When the last Caps Lock keydown was sent as a tap (see the keydown listener). */
let capsTappedAt = -Infinity;
/** A Caps Lock keyup this soon after a keydown is that press's release, not another press. */
const CAPS_RELEASE_MS = 1000;

/** Control is down, or was last, with no other key since: what Control-Space looks like in the
 * page, since macOS keeps the Space (PITFALLS #46). */
let ctrlAlone = false;
/** When Control last came up. */
let ctrlUpAt = -Infinity;
/** The Mac's input source change can land just after a quick Control-Space is let go. */
const CTRL_LATE_MS = 500;

/** Lets go of every key held on the device; with `send` false only forgets them (the device went
 * away with its keyboard). */
export function releaseKeys(send = true): void {
  const steps = keys.releaseAll();
  if (send) keySteps(steps);
}

/** The screen's canvas and the box around it take the keys; returns the removal. */
export function keyTarget(canvas: HTMLCanvasElement, box: HTMLElement): () => void {
  const t = { canvas, over: false };
  target = t;
  const enter = () => (t.over = true);
  const leave = () => (t.over = false);
  box.addEventListener("pointerenter", enter);
  box.addEventListener("pointerleave", leave);
  return () => {
    box.removeEventListener("pointerenter", enter);
    box.removeEventListener("pointerleave", leave);
    if (target === t) target = undefined;
  };
}

/** Shortcuts that stay with the viewer and the computer: zoom (⌘0, ⌘9, ⌘+, ⌘−), the keyboard
 * menu's (⌘K, ⌥⌘K, ⇧⌘V, ⇧⌘C) and Settings (⌘,), with Ctrl for ⌘ off macOS; on macOS also the
 * app menu's Show Touches (⇧⌘T), quit, close, hide and minimize. Every other combination goes to
 * the device: ⌘A, ⌘C, ⌘V, ⌘Z… on a Mac, and Ctrl+A, Ctrl+C… on Windows, where Ctrl is Command on
 * the device (keymap.ts). A menu shortcut reaches the menu only if the page doesn't take the key. */
function viewerShortcut(e: KeyboardEvent): boolean {
  if (!cmdKey(e)) return false;
  if (e.shiftKey && (e.code === "KeyV" || e.code === "KeyC" || (MAC_KEYS && e.code === "KeyT"))) return true;
  if (MAC_KEYS && ["KeyQ", "KeyW", "KeyH", "KeyM"].includes(e.code)) return true;
  return ["Equal", "Minus", "Digit0", "Digit9", "KeyK", "Comma", "NumpadAdd", "NumpadSubtract"].includes(e.code);
}

/** Keys go to the device now. */
function keysGo(): boolean {
  const s = getViewer();
  return (
    forwardingActive(s) &&
    s.keyboard.hardware &&
    !editing() &&
    !!target &&
    (target.over || document.activeElement === target.canvas) &&
    s.openMenu === null
  );
}

const live = () => getViewer().stage === "live";

function onZoomKey(e: KeyboardEvent): void {
  if (!(e.metaKey || e.ctrlKey) || !live()) return;
  const z: Record<string, () => void> = {
    "=": () => zoomBy(1.25),
    "+": () => zoomBy(1.25),
    "-": () => zoomBy(1 / 1.25),
    "0": () => setZoom(1),
    "9": () => setZoom("fit"),
  };
  if (z[e.key]) {
    e.preventDefault();
    e.stopImmediatePropagation();
    z[e.key]();
  }
}

function onKeyDown(e: KeyboardEvent): void {
  if (cmdKey(e) && e.code === "KeyK" && live()) {
    e.preventDefault();
    if (e.altKey) toggleSoftwareKeyboard();
    else toggleForwarding();
    return;
  }
  // Not while a field of the viewer has focus: there ⇧⌘V and ⇧⌘C are the field's.
  if (cmdKey(e) && e.shiftKey && !e.altKey && (e.code === "KeyV" || e.code === "KeyC") && live() && !editing()) {
    e.preventDefault();
    if (!e.repeat) void (e.code === "KeyV" ? sendClipboard() : getClipboard());
    return;
  }
  if (!keysGo() || viewerShortcut(e)) return;
  e.preventDefault();
  e.stopPropagation();
  // Caps Lock is never held on the device: each press goes over as a tap, which on a Korean
  // layout switches the input source. What WebKit reports for one press depends on the Mac
  // (PITFALLS #45): a keydown when it locks and a keyup when it unlocks, or, where Caps Lock
  // switches the Mac's input source, a keydown on press and a keyup on release. The keyup
  // listener tells them apart by time.
  if (e.code !== "Unidentified" && e.code !== "") ctrlAlone = e.code.startsWith("Control");
  if (e.code === "CapsLock") {
    if (!e.repeat) {
      capsTappedAt = e.timeStamp;
      keyTap(e.code);
    }
    return;
  }
  // The Mac switching its input source with Caps Lock adds a keydown with no code and no keyup
  // (PITFALLS #45): nothing for the device.
  if (e.code === "Unidentified" || e.code === "") return;
  keySteps(keys.down(e.code, e.getModifierState("AltGraph")));
  // With ⌘ held, macOS sends no keyup for the other keys: let go of this one at once.
  if (MAC_KEYS && e.metaKey && !e.code.startsWith("Meta")) keySteps(keys.up(e.code));
}

function onKeyUp(e: KeyboardEvent): void {
  if (e.code === "CapsLock" && keysGo()) {
    e.preventDefault();
    // The release of the press the keydown already sent: a second tap would switch the input
    // source back (PITFALLS #45).
    const release = e.timeStamp - capsTappedAt < CAPS_RELEASE_MS;
    capsTappedAt = -Infinity;
    if (!release) keyTap(e.code);
    return;
  }
  if (!keys.held().includes(e.code)) return;
  e.preventDefault();
  keySteps(keys.up(e.code));
  if (e.code.startsWith("Control")) ctrlUpAt = e.timeStamp;
  // Letting go of ⌘ lets go of whatever macOS didn't report.
  if (MAC_KEYS && e.code.startsWith("Meta")) releaseKeys();
}

// Control-Space switched the Mac's input source, as the Mac's "Select the previous input
// source" shortcut: the page saw only Control (PITFALLS #46). Give the device its Space and put
// the Mac's input source back, so only the device switches. Caps Lock switching the Mac's input
// source (PITFALLS #45) has its own path: its keydown clears `ctrlAlone`.
function onInputSourceChanged(): void {
  if (!keysGo() || !ctrlAlone) return;
  if (keys.held().some((c) => c.startsWith("Control"))) keyTap("Space");
  else if (performance.now() - ctrlUpAt < CTRL_LATE_MS) {
    keyStep("ControlLeft", true);
    keyTap("Space");
    keyStep("ControlLeft", false);
  } else return;
  void backend.restoreInputSource().catch(() => {});
}

/** Installs the window's key listeners, in the order they must see a key: zoom first (it stops
 * the key), then forwarding, then Settings. `appMenu`: the window has an app menu with Settings…
 * (macOS, view_menu.rs, ⌘,); elsewhere Ctrl+, opens it from here. */
export function installKeyboard(appMenu: boolean): void {
  window.addEventListener("keydown", onZoomKey);
  window.addEventListener("keydown", onKeyDown);
  window.addEventListener("keyup", onKeyUp);
  window.addEventListener("blur", () => releaseKeys());
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) releaseKeys();
  });
  void backend.onInputSourceChanged(onInputSourceChanged).catch(() => {});
  if (!appMenu) {
    window.addEventListener("keydown", (e) => {
      if (e.ctrlKey && !e.shiftKey && !e.altKey && !e.metaKey && e.code === "Comma") {
        e.preventDefault();
        openSettings();
      }
    });
  }
}
