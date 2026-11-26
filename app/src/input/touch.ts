// Human touches on the live screen. Not React: pointer events go from the canvas's listeners to
// the daemon without touching component state; the screen component installs them on its canvas
// (`touchInput`) and removes them on unmount.
//
// Human input is a live touch: down on pointerdown, the pointer's path as it moves (one step per
// pointer event, with the event's time), up on release. The daemon replays the path at a steady
// rate from those times (PITFALLS #18), so steps need not go out evenly. They go out one at a
// time and in order: an `up` that overtook the last `move` would leave the gesture short, and a
// backlog of moves would make the finger trail the mouse. While a step is in flight, newer moves
// replace the queued one.
//
// While the agent holds the device, the screen takes no touches: one under the agent's input
// would turn its taps into something else, and the daemon refuses them anyway (check_human). A
// press says why, next to the pointer, and points at Take over. A press already down when the
// agent gets the device ends there (`endPress`); the daemon lifts that finger too.

import * as backend from "../backend/commands";
import type { Point, TouchStep } from "../backend/types";
import { stats } from "../device/stats";
import { indicators } from "../overlay/indicators";
import { screen } from "../stream/screen";
import { editing } from "../lib/platform";
import { humanHasDevice } from "../store/viewer";

/** The daemon lifts a finger after 2 s without a step; a mouse held still repeats its point. */
const KEEP_ALIVE_MS = 500;

/** The press in progress. `dead` after the daemon refused a step: the rest of it is ignored. */
let press: { pointerId: number; last: Point; dead: boolean } | undefined;
let steps: TouchStep[] = [];
let stepping = false;
let keepAlive: number | undefined;

/** Whether a finger is down: the device on screen isn't given up for another while it is. */
export const pressing = (): boolean => !!press;

function toScreen(canvas: HTMLCanvasElement, e: PointerEvent): { point: Point; inside: boolean } | undefined {
  const g = screen.geometry;
  if (!g || g.stale) return undefined;
  const r = canvas.getBoundingClientRect();
  const u = (e.clientX - r.left) / r.width;
  const v = (e.clientY - r.top) / r.height;
  const inside = u >= 0 && u <= 1 && v >= 0 && v <= 1;
  const clamp = (n: number) => Math.min(Math.max(n, 0), 1);
  return { point: [clamp(u) * g.screen[0], clamp(v) * g.screen[1]], inside };
}

function queueStep(step: TouchStep): void {
  const last = steps[steps.length - 1];
  if (step.phase === "move" && last?.phase === "move") Object.assign(last, step);
  else steps.push(step);
  void sendSteps();
}

async function sendSteps(): Promise<void> {
  if (stepping) return;
  stepping = true;
  while (steps.length) {
    const step = steps.shift()!;
    const began = performance.now();
    try {
      await backend.sendTouch(step);
      if (step.phase === "move") {
        stats.touchMs.push(performance.now() - began);
        if (stats.touchMs.length > 240) stats.touchMs.splice(0, 120);
      }
    } catch (e) {
      stats.touchErrors++;
      stats.lastError = String(e);
      // Drop the rest of this press; the next `down` starts afresh.
      const next = steps.findIndex((s) => s.phase === "down");
      steps = next < 0 ? [] : steps.slice(next);
      if (press && step.phase !== "up" && next < 0) press.dead = true;
    }
  }
  stepping = false;
}

/** Lifts the finger: at `at` (the pointer's last point by default). Every way a press can end
 * goes through here. */
export function endPress(at?: Point, t?: number): void {
  if (!press) return;
  const p = press;
  press = undefined;
  window.clearInterval(keepAlive);
  if (!p.dead) queueStep({ phase: "up", point: at ?? p.last, t });
}

/** Installs the touch listeners on the screen's canvas; returns their removal. Clicking the screen
 * also gives it the keys, so typing goes on after the pointer leaves it. */
export function touchInput(canvas: HTMLCanvasElement): () => void {
  const down = (e: PointerEvent) => {
    if (editing()) (document.activeElement as HTMLElement).blur();
    canvas.focus({ preventScroll: true });
    if (e.button !== 0 || press) return;
    const at = toScreen(canvas, e);
    if (!at) return;
    if (!humanHasDevice()) {
      indicators.showBlocked(e.clientX, e.clientY);
      return;
    }
    canvas.setPointerCapture(e.pointerId);
    press = { pointerId: e.pointerId, last: at.point, dead: false };
    queueStep({ phase: "down", point: at.point, t: e.timeStamp });
    keepAlive = window.setInterval(() => {
      if (press && !press.dead) queueStep({ phase: "move", point: press.last });
    }, KEEP_ALIVE_MS);
  };
  const move = (e: PointerEvent) => {
    if (!press || press.dead || e.pointerId !== press.pointerId) return;
    const at = toScreen(canvas, e);
    if (!at) return;
    // Leaving the screen lifts the finger at its edge, as a finger sliding off the glass would.
    if (!at.inside) {
      endPress(at.point, e.timeStamp);
      return;
    }
    press.last = at.point;
    queueStep({ phase: "move", point: at.point, t: e.timeStamp });
  };
  const up = (e: PointerEvent) => {
    if (!press || e.pointerId !== press.pointerId) return;
    endPress(toScreen(canvas, e)?.point, e.timeStamp);
  };
  const cancel = (e: PointerEvent) => {
    if (press && e.pointerId === press.pointerId) endPress();
  };
  canvas.addEventListener("pointerdown", down);
  canvas.addEventListener("pointermove", move);
  canvas.addEventListener("pointerup", up);
  // Every way a press can end without a pointerup still lifts the finger.
  canvas.addEventListener("pointercancel", cancel);
  canvas.addEventListener("lostpointercapture", cancel);
  return () => {
    endPress();
    canvas.removeEventListener("pointerdown", down);
    canvas.removeEventListener("pointermove", move);
    canvas.removeEventListener("pointerup", up);
    canvas.removeEventListener("pointercancel", cancel);
    canvas.removeEventListener("lostpointercapture", cancel);
  };
}

/** Ends a press when the window loses focus or hides. Installed once for the page. */
export function installTouchGuards(): void {
  window.addEventListener("blur", () => endPress());
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) endPress();
  });
}
