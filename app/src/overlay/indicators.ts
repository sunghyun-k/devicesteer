// What is drawn over the live screen: touch discs, the agent's cursor and its labels, and the hint
// that a press was kept from the device. Not React: input events come at the agent's or the
// pointer's pace and each one starts web animations and timers on its own elements. The screen
// component lends the overlay element (`attach`); everything here is appended to it.
//
// Touch indicators: a finger on the glass shows as a translucent white disc the size of a
// fingertip (56 points, so it scales with the screen), as iOS touch indicators in screen
// recordings look. Every touch has two sequences: the start, where the disc appears at once where
// the finger lands, and the end, where it grows a little and fades where the finger lifts. A tap
// plays both in place; a drag plays the start, follows the finger while it is held, and plays the
// end only at the release point, so nothing at the start of a drag looks like a release.
//
// The agent cursor: the agent's touches show as a mouse-style arrow that stays where the agent
// last touched, so the person watching can tell where it is working between inputs. It glides
// from one touch to the next and along swipes; a ring under the tip shows while the finger is
// down. A tap is the exception: its disc is drawn where the finger lands while the cursor glides
// there, because the daemon sends the tap as the finger goes down and lifts it `duration_ms`
// (50 ms) later, well before a glide would arrive (PITFALLS #34). Only agent input moves the
// cursor: the human's own pointer is visible already, and their touches keep the dot.

import type { InputEvent, Point } from "../backend/types";
import { stats } from "../device/stats";
import s from "./overlay.module.css";

/** How long the end sequence takes. */
const RELEASE_MS = 350;
/** How long a tap holds the finger down when the event doesn't say (`TAP_HOLD` in the core). */
const TAP_HOLD_MS = 50;
const AGENT_COLOR = "#ff9f0a";
const HUMAN_COLOR = "#0a84ff";
/** How long the cursor takes to reach a new touch before pressing there. Short: the input event
 * arrives as the daemon sends the touch, so a long glide would trail the device. */
const GLIDE_MS = 160;
/** Glide between the steps of a live agent touch, which come at the agent's pace. */
const STEP_GLIDE_MS = 120;
const BLOCKED_HINT_MS = 2400;

const translate = (o: Point) => `translate(${o[0]}px, ${o[1]}px)`;

class Indicators {
  private overlay: HTMLElement | undefined;
  /** What a label must stay inside: the stage. */
  private bounds: HTMLElement | undefined;
  private cursor: HTMLDivElement;
  private blockedHint: HTMLDivElement;
  private blockedTimer: number | undefined;
  /** The disc of a live touch, by source, while the finger is down. */
  private fingers = new Map<string, HTMLDivElement>();
  private dots = new Set<HTMLDivElement>();
  /** Where the cursor rests, in screen points, so a re-layout can place it again. */
  private cursorAt: { p: Point; screen: [number, number] } | undefined;
  private cursorAnim: Animation | undefined;
  private cursorLabel: HTMLDivElement | undefined;
  /** For the report: how often the cursor moved and the last label it carried. */
  private cursorMoves = 0;
  private cursorLastLabel: string | null = null;

  // "Show Touches" (View menu, ⇧⌘T; app/src-tauri/src/view_menu.rs) turns the discs off, the
  // human's and the agent's alike. The agent cursor and labels stay: they say where the agent is
  // working, which the discs only repeat.
  showTouches = true;

  /** Called with true when a press was kept from the device, false when the hint goes. */
  onBlocked: (on: boolean) => void = () => {};

  constructor() {
    this.cursor = document.createElement("div");
    this.cursor.className = s.agentCursor;
    this.cursor.hidden = true;
    this.cursor.innerHTML =
      `<div class="${s.press}"></div>` +
      // The tip is at (0, 0) of the element: the path's tip is at (1.5, 1.5) and the SVG sits 1.5 px
      // up and left. A macOS-style arrow: a tall triangle with a short tail out of its lower edge.
      `<svg class="${s.arrow}" viewBox="0 0 20 27" width="20" height="27" aria-hidden="true">` +
      '<path d="M1.5 1.5 V20.6 L6.1 16.3 L9.3 23.9 L12.6 22.5 L9.4 15.1 H15.6 Z"/></svg>';
    this.cursor.style.setProperty("--c", AGENT_COLOR);
    this.blockedHint = document.createElement("div");
    this.blockedHint.className = s.blockedHint;
    this.blockedHint.setAttribute("role", "status");
    this.blockedHint.hidden = true;
  }

  /** Lends the overlay element; what was drawn moves to it. */
  attach(overlay: HTMLElement, bounds: HTMLElement): void {
    this.overlay = overlay;
    this.bounds = bounds;
    overlay.append(this.cursor, this.blockedHint);
  }

  detach(overlay: HTMLElement): void {
    if (this.overlay !== overlay) return;
    this.clear();
    this.hideBlocked();
    this.cursor.remove();
    this.blockedHint.remove();
    this.overlay = undefined;
    this.bounds = undefined;
  }

  setBlockedText(text: string): void {
    this.blockedHint.textContent = text;
  }

  setShowTouches(on: boolean): void {
    this.showTouches = on;
    if (on) return;
    for (const d of this.dots) d.remove();
    this.dots.clear();
    this.fingers.clear();
  }

  /** Where a point of the screen (in points) is on the overlay, in CSS pixels. */
  private toOverlay(p: Point, screen: [number, number]): Point {
    const o = this.overlay;
    if (!o) return [0, 0];
    return [(p[0] / screen[0]) * o.clientWidth, (p[1] / screen[1]) * o.clientHeight];
  }

  /** Whether a label that starts `offset` px right of overlay point `x` stays inside the stage.
   * The window keeps whatever size the user gave it (PITFALLS #29), so in a narrow one a label
   * beside a point near the right edge would be cut off: it goes on the left of the point instead. */
  private fitsRight(label: HTMLElement, x: number, offset: number): boolean {
    if (!this.overlay || !this.bounds) return true;
    return this.overlay.getBoundingClientRect().left + x + offset + label.offsetWidth <= this.bounds.getBoundingClientRect().right - 8;
  }

  // ---------- labels ----------

  /** Shows an input's label next to where it happened (or at the bottom, for a button) for a
   * moment. */
  private showLabel(text: string | null | undefined, at: Point | undefined, color: string): void {
    const o = this.overlay;
    if (!text || !o) return;
    const b = document.createElement("div");
    b.className = s.labelBubble;
    b.textContent = text;
    b.style.setProperty("--c", color);
    const [x, y] = at ?? [o.clientWidth / 2, o.clientHeight - 24];
    if (!at) b.classList.add(s.bottom);
    b.style.left = `${x}px`;
    b.style.top = `${y}px`;
    o.append(b);
    if (at && !this.fitsRight(b, x, 14)) b.classList.add(s.left);
    stats.labelsShown++;
    b.addEventListener("animationend", () => b.remove());
  }

  // ---------- touch discs ----------

  /** A disc at `at`. With touches hidden it is never put on the overlay, so callers animate and
   * remove it as usual and nothing shows. */
  private touchDot(at: Point): HTMLDivElement {
    const d = document.createElement("div");
    d.className = s.touchDot;
    d.style.left = `${at[0]}px`;
    d.style.top = `${at[1]}px`;
    if (this.showTouches && this.overlay) {
      this.overlay.append(d);
      this.dots.add(d);
    }
    return d;
  }

  /** The end sequence: the disc grows as it fades, then goes. */
  private releaseDot(d: HTMLDivElement): void {
    d.animate(
      [
        { opacity: 1, transform: "scale(1)" },
        { opacity: 0, transform: "scale(1.4)" },
      ],
      { duration: RELEASE_MS, easing: "ease-out", fill: "forwards" },
    );
    // A timer, not the animation's finish: WebKit holds animations while the window is hidden.
    window.setTimeout(() => this.removeDot(d), RELEASE_MS + 30);
  }

  private removeDot(d: HTMLDivElement): void {
    d.remove();
    this.dots.delete(d);
  }

  private onTouch(e: InputEvent): void {
    const p = e.points[0];
    const dot = this.fingers.get(e.source);
    if (!p || !e.screen[0]) {
      // Lifted by the daemon without a point: end where the disc is.
      if (dot) this.releaseDot(dot);
      this.fingers.delete(e.source);
      return;
    }
    const at = this.toOverlay(p, e.screen);
    if (e.phase === "down" || !dot) {
      if (dot) this.removeDot(dot);
      this.fingers.set(e.source, this.touchDot(at));
      return;
    }
    dot.style.left = `${at[0]}px`;
    dot.style.top = `${at[1]}px`;
    if (e.phase === "up") {
      this.releaseDot(dot);
      this.fingers.delete(e.source);
    }
  }

  /** A tap's disc where the finger lands, ended as the finger lifts, `ms` after it came down. */
  private tapDot(at: Point, ms: number | undefined): void {
    const d = this.touchDot(at);
    window.setTimeout(() => this.releaseDot(d), ms ?? TAP_HOLD_MS);
  }

  // ---------- the agent cursor ----------

  /** Where the cursor is drawn right now, mid-glide included. */
  private cursorNow(): Point | undefined {
    if (this.cursor.hidden) return undefined;
    const m = new DOMMatrixReadOnly(getComputedStyle(this.cursor).transform);
    return [m.m41, m.m42];
  }

  /** Glides the cursor along `path` (screen points) over `ms`, or puts it there at once when it
   * was hidden. Resolves when it arrives; a newer move cancels this one, which still resolves. */
  private moveCursor(path: Point[], screen: [number, number], ms: number, easing = "ease-out"): Promise<void> {
    const end = path[path.length - 1];
    if (!end || !screen[0]) return Promise.resolve();
    const cursor = this.cursor;
    this.cursorMoves++;
    const from = this.cursorNow();
    this.cursorAnim?.cancel();
    this.cursorAnim = undefined;
    this.cursorAt = { p: end, screen };
    const target = this.toOverlay(end, screen);
    if (cursor.hidden) {
      cursor.hidden = false;
      cursor.style.transform = translate(target);
      cursor.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 150 });
      return Promise.resolve();
    }
    const frames = [from ?? target, ...path.map((p) => this.toOverlay(p, screen))].map((o) => ({ transform: translate(o) }));
    cursor.style.transform = translate(target);
    if (ms <= 0) return Promise.resolve();
    const anim = cursor.animate(frames, { duration: ms, easing });
    this.cursorAnim = anim;
    // A timer, not the animation's finish: WebKit holds animations while the window is hidden or
    // covered, and the press must still end and the next input still follow.
    return new Promise((done) =>
      window.setTimeout(() => {
        if (this.cursorAnim === anim) this.cursorAnim = undefined;
        done();
      }, ms),
    );
  }

  /** Puts the finger down at the tip (the start sequence) or lifts it (the end sequence). */
  private setPressed(on: boolean): void {
    this.cursor.classList.toggle(s.pressing, on);
  }

  /** Lifts the finger at the tip now, playing the end sequence even if the press never showed
   * (an `up` that came before the glide to its `down` finished). */
  private liftCursor(): void {
    if (!this.cursor.classList.contains(s.pressing)) {
      this.setPressed(true);
      void this.cursor.offsetWidth; // Start the transition from pressed, not from nothing.
    }
    this.setPressed(false);
  }

  /** Puts the label next to the cursor, replacing the one before. It fades after a few seconds;
   * the cursor stays. */
  private labelCursor(text: string, at: Point, screen: [number, number]): void {
    this.cursorLabel?.remove();
    const b = document.createElement("div");
    b.className = `${s.labelBubble} ${s.atCursor}`;
    b.textContent = text;
    b.style.setProperty("--c", AGENT_COLOR);
    this.cursor.append(b);
    // The first input's label comes before the cursor shows, and a hidden one measures no width.
    const hidden = this.cursor.hidden;
    this.cursor.hidden = false;
    if (!this.fitsRight(b, this.toOverlay(at, screen)[0], 16)) b.classList.add(s.left);
    this.cursor.hidden = hidden;
    this.cursorLabel = b;
    this.cursorLastLabel = text;
    stats.labelsShown++;
    b.addEventListener("animationend", () => {
      b.remove();
      if (this.cursorLabel === b) this.cursorLabel = undefined;
    });
  }

  hideCursor(): void {
    this.cursorAnim?.cancel();
    this.cursorAnim = undefined;
    this.setPressed(false);
    this.cursorLabel?.remove();
    this.cursorLabel = undefined;
    this.cursorAt = undefined;
    this.cursor.hidden = true;
  }

  /** Places the resting cursor again after the screen changed size. */
  relayout(): void {
    if (this.cursorAt && !this.cursorAnim) this.cursor.style.transform = translate(this.toOverlay(this.cursorAt.p, this.cursorAt.screen));
  }

  /** Points of the old screen mean nothing on a turned or new one: the cursor and live discs go. */
  clear(): void {
    this.hideCursor();
    for (const d of this.fingers.values()) this.removeDot(d);
    this.fingers.clear();
  }

  /** Shows one agent input on the cursor. */
  private onAgentInput(e: InputEvent): void {
    const [p, q] = e.points;
    const screen = e.screen;
    if (!p || !screen[0]) {
      // A button press, or a live touch the daemon lifted without a point: the finger is up.
      if (e.kind === "touch") this.setPressed(false);
      if (e.label) this.showLabel(e.label, undefined, AGENT_COLOR);
      return;
    }
    if (e.label) this.labelCursor(e.label, p, screen);
    if (e.kind === "tap") {
      this.tapDot(this.toOverlay(p, screen), e.duration_ms);
      void this.moveCursor([p], screen, GLIDE_MS);
    } else if (e.kind === "swipe" && q) {
      void this.moveCursor([p], screen, GLIDE_MS)
        .then(() => {
          if (this.cursorAt?.p !== p) return;
          this.setPressed(true);
          return this.moveCursor([q], screen, e.duration_ms ?? 300, "ease-in-out");
        })
        .then(() => {
          if (this.cursorAt?.p !== q) return; // Overtaken by a newer input.
          this.setPressed(false);
        });
    } else if (e.kind === "touch") {
      if (e.phase === "down") {
        void this.moveCursor([p], screen, GLIDE_MS).then(() => {
          if (this.cursorAt?.p !== p) return;
          this.setPressed(true);
        });
      } else if (e.phase === "move") {
        this.setPressed(true);
        void this.moveCursor([p], screen, STEP_GLIDE_MS, "linear");
      } else {
        // The daemon sends the up once the finger has lifted: end now, and let the cursor glide on.
        this.liftCursor();
        void this.moveCursor([p], screen, STEP_GLIDE_MS, "linear");
      }
    }
  }

  /** The daemon's `input` event: someone's input reached the device. */
  input(e: InputEvent): void {
    if (e.kind !== "touch" || e.phase !== "move") {
      stats.inputs[e.source] = (stats.inputs[e.source] ?? 0) + 1;
      stats.lastInput = `${e.source} ${e.kind}${e.phase ? ` ${e.phase}` : ""} ${JSON.stringify(e.points)}`;
    }
    if (!this.overlay) return;
    if (e.source === "agent") {
      this.onAgentInput(e);
      return;
    }
    if (e.label) {
      const p = e.points[0];
      this.showLabel(e.label, p && e.screen[0] ? this.toOverlay(p, e.screen) : undefined, HUMAN_COLOR);
    }
    if (e.kind === "touch") {
      this.onTouch(e);
    } else if (e.kind === "tap" && e.points[0]) {
      this.tapDot(this.toOverlay(e.points[0], e.screen), e.duration_ms);
    } else if (e.kind === "swipe" && e.points.length === 2) {
      const [a, b] = [this.toOverlay(e.points[0], e.screen), this.toOverlay(e.points[1], e.screen)];
      const d = this.touchDot(b);
      const ms = e.duration_ms ?? 300;
      d.animate(
        [
          { left: `${a[0]}px`, top: `${a[1]}px` },
          { left: `${b[0]}px`, top: `${b[1]}px` },
        ],
        { duration: ms, easing: "ease-in-out" },
      );
      window.setTimeout(() => this.releaseDot(d), ms);
    }
  }

  // ---------- a press kept from the device ----------

  /** Says why a press at (clientX, clientY) did nothing: the agent has the device. */
  showBlocked(clientX: number, clientY: number): void {
    const o = this.overlay;
    if (!o) return;
    stats.touchesBlocked++;
    const r = o.getBoundingClientRect();
    const x = clientX - r.left;
    const y = clientY - r.top;
    const h = this.blockedHint;
    h.hidden = false;
    h.style.left = `${x}px`;
    h.style.top = `${y}px`;
    h.classList.toggle(s.left, !this.fitsRight(h, x, 14));
    // Restart the fade for every press, not only the first.
    h.classList.remove(s.nudge);
    void h.offsetWidth;
    h.classList.add(s.nudge);
    this.onBlocked(true);
    window.clearTimeout(this.blockedTimer);
    this.blockedTimer = window.setTimeout(() => this.hideBlocked(), BLOCKED_HINT_MS);
  }

  hideBlocked(): void {
    window.clearTimeout(this.blockedTimer);
    if (this.blockedHint.hidden) return;
    this.blockedHint.hidden = true;
    this.onBlocked(false);
  }

  /** For the report. */
  snapshot() {
    const o = this.overlay;
    return {
      touchDotsOnScreen: this.dots.size,
      blockedHint: !this.blockedHint.hidden,
      overlay: [o?.clientWidth ?? 0, o?.clientHeight ?? 0],
      agentCursor: this.cursor.hidden
        ? null
        : {
            at: this.cursorAt?.p ?? null,
            drawnAt: this.cursorNow(),
            pressing: this.cursor.classList.contains(s.pressing),
            gliding: !!this.cursorAnim,
            label: this.cursorLabel?.textContent ?? null,
            lastLabel: this.cursorLastLabel,
            moves: this.cursorMoves,
          },
    };
  }
}

/** The page's one overlay. */
export const indicators = new Indicators();
