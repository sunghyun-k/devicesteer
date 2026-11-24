import { Channel, invoke } from "@tauri-apps/api/core";

/** Mirrors `iphone_use_core::DeviceInfo`. */
interface DeviceInfo {
  udid: string;
  transports: ("usb" | "wifi")[];
  name?: string;
  product_type?: string;
  device_class?: string;
  os_version?: string;
  build_version?: string;
  error?: string;
}

/** The daemon's `config` event: how to set up the decoder and where the screen is. */
interface Config {
  codec: string;
  description: string;
  coded: [number, number];
  display_px: [number, number];
  screen: [number, number];
}

/** An agent session as the daemon reports it (`iphone_use_daemon::control::AgentSession`). */
interface AgentSession {
  id: string;
  name: string;
  owner: "agent" | "delegated_to_user" | "user" | "finished";
  /** Why the agent handed off, while `delegated_to_user`. */
  reason: string | null;
}

interface ControlEvent {
  /** The session using the device; null when none runs. */
  session: AgentSession | null;
  human_touching: boolean;
  agent_stopped: boolean;
}

interface InputEvent {
  source: "agent" | "human";
  kind: "tap" | "swipe" | "button" | "touch";
  points: [number, number][];
  screen: [number, number];
  duration_ms?: number;
  /** For `touch`: one step of a live touch. */
  phase?: "down" | "move" | "up";
  /** For a `touch` up the daemon did itself: why (timeout, disconnect, control change). */
  lifted?: string;
  /** What the input is for, in a few words (`--label`), shown next to it. */
  label?: string | null;
}

/** Mirrors `iphone_use_core::DeviceSettings`, plus the location the daemon simulated. `null`:
 * unknown or unsupported (named in `unsupported`). A `settings` event can carry only some keys. */
interface Settings {
  appearance: "light" | "dark" | null;
  text_size: string | null;
  liquid_glass: number | null;
  reduce_motion: boolean | null;
  increase_contrast: boolean | null;
  show_borders: boolean | null;
  reduce_transparency: boolean | null;
  voiceover: boolean | null;
  location: [number, number] | null;
  unsupported: string[];
}

const $ = <T extends HTMLElement>(sel: string) => document.querySelector<T>(sel)!;

// ---------- device picker ----------

const picker = $<HTMLElement>("#picker");
const list = $<HTMLUListElement>("#devices");
const status = $<HTMLParagraphElement>("#status");
const refresh = $<HTMLButtonElement>("#refresh");

function row(d: DeviceInfo): HTMLLIElement {
  const li = document.createElement("li");
  const name = document.createElement("div");
  name.className = "name";
  name.textContent = d.name ?? d.product_type ?? "Unknown device";
  const meta = document.createElement("div");
  meta.className = "meta";
  const os = d.os_version ? `${d.device_class ?? "iOS"} ${d.os_version}` : undefined;
  meta.textContent = [d.product_type, os, d.transports.join(" + ").toUpperCase(), d.udid]
    .filter(Boolean)
    .join(" · ");
  li.append(name, meta);
  if (d.error) {
    const err = document.createElement("div");
    err.className = "error";
    err.textContent = d.error;
    li.append(err);
  } else {
    li.classList.add("pickable");
    li.addEventListener("click", () => openViewer(d));
  }
  return li;
}

async function load(): Promise<void> {
  refresh.disabled = true;
  status.textContent = "Looking for devices…";
  try {
    const devices = await invoke<DeviceInfo[]>("list_devices");
    list.replaceChildren(...devices.map(row));
    status.textContent = devices.length
      ? `${devices.length} device${devices.length > 1 ? "s" : ""}. Click one to view it.`
      : "No devices. Connect an iPhone or iPad over USB.";
  } catch (e) {
    list.replaceChildren();
    status.textContent = String(e);
  } finally {
    refresh.disabled = false;
  }
}

refresh.addEventListener("click", load);

// ---------- viewer ----------

const viewer = $<HTMLElement>("#viewer");
const canvas = $<HTMLCanvasElement>("#screen");
const overlay = $<HTMLDivElement>("#overlay");
const stage = $<HTMLDivElement>("#stage");
const viewerStatus = $<HTMLParagraphElement>("#viewer-status");
const glow = $<HTMLDivElement>("#glow");
const sessionIdle = $<HTMLParagraphElement>("#session-idle");
const sessionPanel = $<HTMLDivElement>("#session-panel");
const sessionName = $<HTMLSpanElement>("#session-name");
const sessionState = $<HTMLSpanElement>("#session-state");
const sessionPrimary = $<HTMLButtonElement>("#session-primary");
const sessionStop = $<HTMLButtonElement>("#session-stop");
const ctx = canvas.getContext("2d")!;

let config: Config | undefined;
let decoder: VideoDecoder | undefined;
let waitingForKey = true;
let control: ControlEvent = { session: null, human_touching: false, agent_stopped: false };
/** Labels shown so far, for the report. */
let labelsShown = 0;

/** Decoder numbers, shown under the screen and reported to the backend's log. */
const stats = {
  received: 0,
  decoded: 0,
  dropped: 0,
  errors: 0,
  lastError: "",
  supported: undefined as boolean | undefined,
  codec: "",
  // Time from a frame reaching the page to the decoder handing out its picture.
  decodeMs: [] as number[],
  fpsWindow: [] as number[],
  inputs: { agent: 0, human: 0 } as Record<string, number>,
  lastInput: "",
  // Round trip of a human touch step: sent from the page until the daemon wrote the report.
  touchMs: [] as number[],
  touchErrors: 0,
};
const received = new Map<number, number>();
/** Frames that arrive while the decoder is being set up, from the last keyframe on: the device
 * sends one keyframe per stream, so dropping it means nothing decodes. */
let pending: { header: { keyframe: boolean; pts_us: number }; data: Uint8Array }[] = [];
/** Bumped by every `config` and by leaving the viewer, so a setup that finishes late gives way. */
let configGen = 0;
/** Codec and parameter sets the current decoder was set up with. */
let decoderKey = "";

function send(cmd: string, args: unknown = {}): Promise<unknown> {
  return invoke("send", { cmd, args }).catch((e) => {
    viewerStatus.textContent = String(e);
    throw e;
  });
}

function b64(s: string): Uint8Array {
  return Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
}

async function configure(cfg: Config): Promise<void> {
  const gen = ++configGen;
  config = cfg;
  canvas.width = cfg.display_px[0];
  canvas.height = cfg.display_px[1];
  layout();
  // A viewer joining a running stream gets the last config and then, once the stream restarts
  // for a keyframe (PITFALLS #13), a second one right before that keyframe. Same parameter sets:
  // the decoder in use takes the new keyframe. Different: the old decoder is dropped now, not
  // after the support check below, or it swallows the stream's only keyframe while the check
  // runs and the new decoder never sees one (PITFALLS #23).
  const key = `${cfg.codec}|${cfg.description}`;
  if (decoder?.state === "configured" && key === decoderKey) return;
  decoder?.close();
  decoder = undefined;
  decoderKey = "";
  pending = [];
  if (typeof VideoDecoder === "undefined") {
    stats.supported = false;
    stats.lastError = "WebCodecs VideoDecoder is not available in this web view";
    return;
  }
  const decoderConfig: VideoDecoderConfig = {
    codec: cfg.codec,
    description: b64(cfg.description),
    optimizeForLatency: true,
    hardwareAcceleration: "prefer-hardware",
  };
  try {
    const support = await VideoDecoder.isConfigSupported(decoderConfig);
    stats.supported = !!support.supported;
    stats.codec = cfg.codec;
    if (!support.supported) {
      stats.lastError = `decoder does not support ${cfg.codec}`;
      return;
    }
  } catch (e) {
    stats.supported = false;
    stats.lastError = String(e);
    return;
  }
  if (gen !== configGen) return;
  decoder = new VideoDecoder({
    output: (frame) => {
      const t = received.get(frame.timestamp);
      if (t !== undefined) {
        stats.decodeMs.push(performance.now() - t);
        received.delete(frame.timestamp);
      }
      // The screen is the top-left part of the coded picture; the rest is padding.
      ctx.drawImage(frame, 0, 0, canvas.width, canvas.height, 0, 0, canvas.width, canvas.height);
      frame.close();
      stats.decoded++;
      stats.fpsWindow.push(performance.now());
    },
    error: (e) => {
      stats.errors++;
      stats.lastError = String(e);
      waitingForKey = true;
    },
  });
  decoder.configure(decoderConfig);
  decoderKey = key;
  waitingForKey = true;
  const queued = pending;
  pending = [];
  for (const f of queued) onFrame(f.header, f.data, false);
}

function onFrame(header: { keyframe: boolean; pts_us: number }, data: Uint8Array, fresh = true): void {
  if (fresh) stats.received++;
  if (!decoder || decoder.state !== "configured") {
    if (header.keyframe) pending = [{ header, data: data.slice() }];
    else if (pending.length && pending.length < 600) pending.push({ header, data: data.slice() });
    return;
  }
  if (waitingForKey && !header.keyframe) {
    stats.dropped++;
    return;
  }
  waitingForKey = false;
  received.set(header.pts_us, performance.now());
  if (received.size > 120) received.clear();
  try {
    decoder.decode(new EncodedVideoChunk({ type: header.keyframe ? "key" : "delta", timestamp: header.pts_us, data }));
  } catch (e) {
    stats.errors++;
    stats.lastError = String(e);
    waitingForKey = true;
  }
}

/** What the panel says and does for each owner: status text, the main button and its action. */
function panelFor(s: AgentSession): { state: string; button: string; action: string } {
  switch (s.owner) {
    case "delegated_to_user":
      return { state: `Waiting for you: ${s.reason ?? ""}`, button: "Done", action: "done" };
    case "user":
      return { state: "You are in control", button: "Hand back", action: "hand_back" };
    default:
      return { state: "Agent is in control", button: "Take over", action: "take_over" };
  }
}

/** The session's glow and panel stay up for as long as the session runs, not only while input
 * arrives, so the person watching always knows an agent holds the device. */
function onControl(c: ControlEvent): void {
  const was = control.session && control.session.owner !== "finished" ? control.session.id : null;
  control = c;
  const s = c.session && c.session.owner !== "finished" ? c.session : null;
  // The cursor marks where this session's agent works; it goes with the session. Input sent
  // without a session keeps it until the device changes.
  if (was && s?.id !== was) hideCursor();
  sessionPanel.hidden = !s;
  sessionIdle.hidden = !!s;
  glow.hidden = !s;
  if (!s) return;
  const human = s.owner !== "agent";
  glow.classList.toggle("human", human);
  sessionPanel.classList.toggle("human", human);
  const p = panelFor(s);
  sessionName.textContent = s.name;
  sessionState.textContent = p.state;
  sessionState.title = p.state;
  sessionPrimary.textContent = p.button;
  sessionPrimary.dataset.action = p.action;
  layout();
}

/** Where a point of the screen (in points) is on the overlay, in CSS pixels. */
function toOverlay(p: [number, number], screen: [number, number]): [number, number] {
  return [(p[0] / screen[0]) * overlay.clientWidth, (p[1] / screen[1]) * overlay.clientHeight];
}

/** Shows an input's label next to where it happened (or at the bottom, for a button) for a
 * moment. */
function showLabel(text: string | null | undefined, at: [number, number] | undefined, color: string): void {
  if (!text) return;
  const b = document.createElement("div");
  b.className = "label-bubble";
  b.textContent = text;
  b.style.setProperty("--c", color);
  const [x, y] = at ?? [overlay.clientWidth / 2, overlay.clientHeight - 24];
  if (!at) b.classList.add("bottom");
  b.style.left = `${x}px`;
  b.style.top = `${y}px`;
  overlay.append(b);
  labelsShown++;
  b.addEventListener("animationend", () => b.remove());
}

function ripple(at: [number, number], color: string): HTMLDivElement {
  const r = document.createElement("div");
  r.className = "ripple";
  r.style.setProperty("--c", color);
  r.style.left = `${at[0]}px`;
  r.style.top = `${at[1]}px`;
  overlay.append(r);
  r.addEventListener("animationend", () => r.remove());
  return r;
}

/** The finger of a live touch on the overlay, by source, while it is down. */
const fingers = new Map<string, HTMLDivElement>();

function onTouch(e: InputEvent, color: string): void {
  const p = e.points[0];
  if (!p || !e.screen[0]) {
    fingers.get(e.source)?.remove();
    fingers.delete(e.source);
    return;
  }
  const at = toOverlay(p, e.screen);
  let dot = fingers.get(e.source);
  if (e.phase === "down") {
    ripple(at, color);
    dot?.remove();
    dot = undefined;
  }
  if (e.phase === "up") {
    dot?.remove();
    fingers.delete(e.source);
    ripple(at, color);
    return;
  }
  if (!dot) {
    dot = document.createElement("div");
    dot.className = "dot";
    dot.style.setProperty("--c", color);
    overlay.append(dot);
    fingers.set(e.source, dot);
  }
  dot.style.left = `${at[0]}px`;
  dot.style.top = `${at[1]}px`;
}

// ---------- agent cursor ----------
//
// The agent's touches show as a mouse-style arrow that stays where the agent last touched, so the
// person watching can tell where it is working between inputs. It glides from one touch to the
// next and along swipes; a ring under the tip shows while the finger is down. Only agent input
// moves it: the human's own pointer is visible already, and their touches keep the dot.

const AGENT_COLOR = "#ff9f0a";
const HUMAN_COLOR = "#0a84ff";
/** How long the cursor takes to reach a new touch before pressing there. Short: the input event
 * arrives as the daemon sends the touch, so a long glide would trail the device. */
const GLIDE_MS = 160;
/** Glide between the steps of a live agent touch, which come at the agent's pace. */
const STEP_GLIDE_MS = 120;
/** How long a tap shows as pressed. */
const TAP_PRESS_MS = 160;

const cursor = document.createElement("div");
cursor.className = "agent-cursor";
cursor.hidden = true;
cursor.innerHTML =
  '<div class="press"></div>' +
  // The tip is at (0, 0) of the element: the path's tip is at (1.5, 1.5) and the SVG sits 1.5 px up
  // and left. A macOS-style arrow: a tall triangle with a short tail out of its lower edge.
  '<svg class="arrow" viewBox="0 0 20 27" width="20" height="27" aria-hidden="true">' +
  '<path d="M1.5 1.5 V20.6 L6.1 16.3 L9.3 23.9 L12.6 22.5 L9.4 15.1 H15.6 Z"/></svg>';
cursor.style.setProperty("--c", AGENT_COLOR);
overlay.append(cursor);

/** Where the cursor rests, in screen points, so a re-layout can place it again. */
let cursorAt: { p: Point; screen: [number, number] } | undefined;
let cursorAnim: Animation | undefined;
let cursorPressTimer: number | undefined;
let cursorLabel: HTMLDivElement | undefined;
/** For the report: how often the cursor moved and the last label it carried. */
let cursorMoves = 0;
let cursorLastLabel: string | null = null;

const translate = (o: Point) => `translate(${o[0]}px, ${o[1]}px)`;

/** Where the cursor is drawn right now, mid-glide included. */
function cursorNow(): Point | undefined {
  if (cursor.hidden) return undefined;
  const m = new DOMMatrixReadOnly(getComputedStyle(cursor).transform);
  return [m.m41, m.m42];
}

/** Glides the cursor along `path` (screen points) over `ms`, or puts it there at once when it
 * was hidden. Resolves when it arrives; a newer move cancels this one, which still resolves. */
function moveCursor(path: Point[], screen: [number, number], ms: number, easing = "ease-out"): Promise<void> {
  const end = path[path.length - 1];
  if (!end || !screen[0]) return Promise.resolve();
  cursorMoves++;
  const from = cursorNow();
  cursorAnim?.cancel();
  cursorAnim = undefined;
  cursorAt = { p: end, screen };
  const target = toOverlay(end, screen);
  if (cursor.hidden) {
    cursor.hidden = false;
    cursor.style.transform = translate(target);
    cursor.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 150 });
    return Promise.resolve();
  }
  const frames = [from ?? target, ...path.map((p) => toOverlay(p, screen))].map((o) => ({ transform: translate(o) }));
  cursor.style.transform = translate(target);
  if (ms <= 0) return Promise.resolve();
  const anim = cursor.animate(frames, { duration: ms, easing });
  cursorAnim = anim;
  // A timer, not the animation's finish: WebKit holds animations while the window is hidden or
  // covered, and the press must still end and the next input still follow.
  return new Promise((done) =>
    window.setTimeout(() => {
      if (cursorAnim === anim) cursorAnim = undefined;
      done();
    }, ms),
  );
}

function setPressed(on: boolean): void {
  window.clearTimeout(cursorPressTimer);
  cursor.classList.toggle("pressing", on);
}

/** A short press at the tip, for a tap or the ends of a swipe. */
function pressBriefly(): void {
  setPressed(true);
  if (cursorAt) ripple(toOverlay(cursorAt.p, cursorAt.screen), AGENT_COLOR);
  cursorPressTimer = window.setTimeout(() => setPressed(false), TAP_PRESS_MS);
}

/** Puts the label next to the cursor, replacing the one before. It fades after a few seconds;
 * the cursor stays. */
function labelCursor(text: string): void {
  cursorLabel?.remove();
  const b = document.createElement("div");
  b.className = "label-bubble at-cursor";
  b.textContent = text;
  b.style.setProperty("--c", AGENT_COLOR);
  cursor.append(b);
  cursorLabel = b;
  cursorLastLabel = text;
  labelsShown++;
  b.addEventListener("animationend", () => {
    b.remove();
    if (cursorLabel === b) cursorLabel = undefined;
  });
}

function hideCursor(): void {
  cursorAnim?.cancel();
  cursorAnim = undefined;
  setPressed(false);
  cursorLabel?.remove();
  cursorLabel = undefined;
  cursorAt = undefined;
  cursor.hidden = true;
}

function placeCursor(): void {
  if (cursorAt && !cursorAnim) cursor.style.transform = translate(toOverlay(cursorAt.p, cursorAt.screen));
}

/** Shows one agent input on the cursor. */
function onAgentInput(e: InputEvent): void {
  const [p, q] = e.points;
  const screen = e.screen;
  if (!p || !screen[0]) {
    // A button press, or a live touch the daemon lifted without a point: the finger is up.
    if (e.kind === "touch") setPressed(false);
    if (e.label) showLabel(e.label, undefined, AGENT_COLOR);
    return;
  }
  if (e.label) labelCursor(e.label);
  if (e.kind === "tap") {
    void moveCursor([p], screen, GLIDE_MS).then(() => {
      if (cursorAt?.p === p) pressBriefly();
    });
  } else if (e.kind === "swipe" && q) {
    void moveCursor([p], screen, GLIDE_MS).then(() => {
      if (cursorAt?.p !== p) return;
      setPressed(true);
      ripple(toOverlay(p, screen), AGENT_COLOR);
      return moveCursor([q], screen, e.duration_ms ?? 300, "ease-in-out");
    }).then(() => {
      if (cursorAt?.p !== q) return; // Overtaken by a newer input.
      setPressed(false);
      ripple(toOverlay(q, screen), AGENT_COLOR);
    });
  } else if (e.kind === "touch") {
    if (e.phase === "down") {
      void moveCursor([p], screen, GLIDE_MS).then(() => {
        if (cursorAt?.p !== p) return;
        setPressed(true);
        ripple(toOverlay(p, screen), AGENT_COLOR);
      });
    } else if (e.phase === "move") {
      setPressed(true);
      void moveCursor([p], screen, STEP_GLIDE_MS, "linear");
    } else {
      void moveCursor([p], screen, STEP_GLIDE_MS, "linear").then(() => {
        setPressed(false);
        ripple(toOverlay(p, screen), AGENT_COLOR);
      });
    }
  }
}

function onInput(e: InputEvent): void {
  if (e.kind !== "touch" || e.phase !== "move") {
    stats.inputs[e.source] = (stats.inputs[e.source] ?? 0) + 1;
    stats.lastInput = `${e.source} ${e.kind}${e.phase ? ` ${e.phase}` : ""} ${JSON.stringify(e.points)}`;
  }
  if (e.source === "agent") {
    onAgentInput(e);
    return;
  }
  const color = HUMAN_COLOR;
  if (e.label) {
    const p = e.points[0];
    showLabel(e.label, p && e.screen[0] ? toOverlay(p, e.screen) : undefined, color);
  }
  if (e.kind === "touch") {
    onTouch(e, color);
  } else if (e.kind === "tap" && e.points[0]) {
    ripple(toOverlay(e.points[0], e.screen), color);
  } else if (e.kind === "swipe" && e.points.length === 2) {
    const [a, b] = [toOverlay(e.points[0], e.screen), toOverlay(e.points[1], e.screen)];
    ripple(a, color);
    const dot = document.createElement("div");
    dot.className = "dot";
    dot.style.setProperty("--c", color);
    overlay.append(dot);
    dot.animate(
      [
        { left: `${a[0]}px`, top: `${a[1]}px` },
        { left: `${b[0]}px`, top: `${b[1]}px` },
      ],
      { duration: e.duration_ms ?? 300, easing: "ease-out", fill: "forwards" },
    ).onfinish = () => {
      dot.remove();
      ripple(b, color);
    };
  }
}

/** Parses one wire message (see `iphone_use_daemon::wire`). */
function onMessage(buf: Uint8Array): void {
  const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  const kind = buf[4];
  const payload = buf.subarray(5);
  if (kind === 0) {
    const ev = JSON.parse(new TextDecoder().decode(payload));
    switch (ev.event) {
      case "config":
        void configure(ev as Config);
        break;
      case "control":
        onControl(ev as ControlEvent);
        break;
      case "input":
        onInput(ev as InputEvent);
        break;
      case "settings":
        showSettings(ev as Partial<Settings>);
        break;
    }
  } else if (kind === 1) {
    const hl = view.getUint32(5);
    const header = JSON.parse(new TextDecoder().decode(payload.subarray(4, 4 + hl)));
    onFrame(header, payload.subarray(4 + hl));
  }
}

/** Keeps the overlay exactly over the drawn canvas. */
function layout(): void {
  const r = canvas.getBoundingClientRect();
  const s = stage.getBoundingClientRect();
  for (const el of [overlay, glow]) {
    el.style.left = `${r.left - s.left}px`;
    el.style.top = `${r.top - s.top}px`;
    el.style.width = `${r.width}px`;
    el.style.height = `${r.height}px`;
  }
  placeCursor();
}
new ResizeObserver(layout).observe(stage);

// Human input is a live touch: down on pointerdown, the pointer's path as it moves (one step per
// pointer event, with the event's time), up on release. The daemon replays the path at a steady
// rate from those times (PITFALLS #18), so steps need not go out evenly. They go out one at a
// time and in order: an `up` that overtook the last `move` would leave the gesture short, and a
// backlog of moves would make the finger trail the mouse. While a step is in flight, newer moves
// replace the queued one.

type Point = [number, number];
/** `t`: when the pointer was there (ms, the page's clock). */
type Step = { phase: "down" | "move" | "up"; point: Point; t?: number };

/** The press in progress. `dead` after the daemon refused a step: the rest of it is ignored. */
let press: { pointerId: number; last: Point; dead: boolean } | undefined;
let steps: Step[] = [];
let stepping = false;
let keepAlive: number | undefined;
/** The daemon lifts a finger after 2 s without a step; a mouse held still repeats its point. */
const KEEP_ALIVE_MS = 500;

function toScreen(e: PointerEvent): { point: Point; inside: boolean } | undefined {
  if (!config) return undefined;
  const r = canvas.getBoundingClientRect();
  const u = (e.clientX - r.left) / r.width;
  const v = (e.clientY - r.top) / r.height;
  const inside = u >= 0 && u <= 1 && v >= 0 && v <= 1;
  const clamp = (n: number) => Math.min(Math.max(n, 0), 1);
  return { point: [clamp(u) * config.screen[0], clamp(v) * config.screen[1]], inside };
}

function queueStep(step: Step): void {
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
      await invoke("send", { cmd: "touch", args: step });
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

function endPress(at?: Point, t?: number): void {
  if (!press) return;
  const p = press;
  press = undefined;
  window.clearInterval(keepAlive);
  if (!p.dead) queueStep({ phase: "up", point: at ?? p.last, t });
}

canvas.addEventListener("pointerdown", (e) => {
  if (e.button !== 0 || press) return;
  const at = toScreen(e);
  if (!at) return;
  canvas.setPointerCapture(e.pointerId);
  press = { pointerId: e.pointerId, last: at.point, dead: false };
  queueStep({ phase: "down", point: at.point, t: e.timeStamp });
  keepAlive = window.setInterval(() => {
    if (press && !press.dead) queueStep({ phase: "move", point: press.last });
  }, KEEP_ALIVE_MS);
});

canvas.addEventListener("pointermove", (e) => {
  if (!press || press.dead || e.pointerId !== press.pointerId) return;
  const at = toScreen(e);
  if (!at) return;
  // Leaving the screen lifts the finger at its edge, as a finger sliding off the glass would.
  if (!at.inside) {
    endPress(at.point, e.timeStamp);
    return;
  }
  press.last = at.point;
  queueStep({ phase: "move", point: at.point, t: e.timeStamp });
});

canvas.addEventListener("pointerup", (e) => {
  if (!press || e.pointerId !== press.pointerId) return;
  endPress(toScreen(e)?.point, e.timeStamp);
});
// Every way a press can end without a pointerup still lifts the finger.
for (const type of ["pointercancel", "lostpointercapture"] as const) {
  canvas.addEventListener(type, (e) => {
    if (press && e.pointerId === press.pointerId) endPress();
  });
}
window.addEventListener("blur", () => endPress());
document.addEventListener("visibilitychange", () => {
  if (document.hidden) endPress();
});

$<HTMLButtonElement>("#home").addEventListener("click", () => void send("button", { name: "home" }));
sessionPrimary.addEventListener("click", () => {
  if (sessionPrimary.dataset.action) void send("control", { action: sessionPrimary.dataset.action });
});
sessionStop.addEventListener("click", () => void send("control", { action: "stop" }));

// ---------- device settings ----------

const settingsPanel = $<HTMLElement>("#settings");
const settingsToggle = $<HTMLButtonElement>("#settings-toggle");
const settingsStatus = $<HTMLParagraphElement>("#settings-status");
const voWarning = $<HTMLParagraphElement>("#vo-warning");
const liquidGlass = $<HTMLInputElement>("#liquid-glass");
const textSize = $<HTMLInputElement>("#text-size");
const latitude = $<HTMLInputElement>("#latitude");
const longitude = $<HTMLInputElement>("#longitude");

/** Text sizes from smallest to largest, as the CLI names them (`iphone_use_core::TEXT_SIZES`). */
const TEXT_SIZES = ["xs", "s", "m", "l", "xl", "xxl", "xxxl", "ax1", "ax2", "ax3", "ax4", "ax5"];
/** On/off items: the field in `Settings` and the name `settings_set` takes. */
const TOGGLES: [keyof Settings, string][] = [
  ["reduce_motion", "reduce-motion"],
  ["increase_contrast", "increase-contrast"],
  ["show_borders", "show-borders"],
  ["reduce_transparency", "reduce-transparency"],
  ["voiceover", "voiceover"],
];

let settings: Partial<Settings> = {};

function item(name: string): HTMLElement {
  return settingsPanel.querySelector<HTMLElement>(`[data-item="${name}"]`)!;
}

/** Shows the settings, merged into what is known (an event may carry one key). */
function showSettings(s: Partial<Settings>): void {
  settings = { ...settings, ...s };
  const cur = settings;
  voWarning.hidden = cur.voiceover !== true;
  for (const radio of settingsPanel.querySelectorAll<HTMLInputElement>('input[name="appearance"]')) {
    radio.checked = radio.value === cur.appearance;
  }
  if (cur.liquid_glass != null) liquidGlass.value = String(cur.liquid_glass);
  $<HTMLOutputElement>("#liquid-glass-value").value = cur.liquid_glass != null ? cur.liquid_glass.toFixed(2) : "–";
  const size = cur.text_size ? TEXT_SIZES.indexOf(cur.text_size) : -1;
  if (size >= 0) textSize.value = String(size);
  $<HTMLOutputElement>("#text-size-value").value = cur.text_size ?? "–";
  for (const [key] of TOGGLES) {
    item(key).querySelector<HTMLInputElement>("input")!.checked = cur[key] === true;
  }
  $<HTMLOutputElement>("#location-value").value = cur.location
    ? `${cur.location[0].toFixed(4)}, ${cur.location[1].toFixed(4)}`
    : "not simulated";
  if (cur.location && document.activeElement !== latitude && document.activeElement !== longitude) {
    latitude.value = String(cur.location[0]);
    longitude.value = String(cur.location[1]);
  }
  for (const el of settingsPanel.querySelectorAll<HTMLElement>("[data-item]")) {
    const off = cur.unsupported?.includes(el.dataset.item!) ?? false;
    el.classList.toggle("unsupported", off);
    for (const input of el.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input, button")) input.disabled = off;
  }
}

function settingsMessage(text: string, error = false): void {
  settingsStatus.textContent = text;
  settingsStatus.classList.toggle("error", error);
}

async function readSettings(): Promise<void> {
  settingsMessage("Reading…");
  try {
    showSettings(await invoke<Settings>("send", { cmd: "settings", args: {} }));
    settingsMessage("");
  } catch (e) {
    settingsMessage(String(e), true);
  }
}

/** Changes one item; on failure the controls go back to the last known values. */
async function setSetting(name: string, value: (string | number)[]): Promise<void> {
  settingsPanel.setAttribute("aria-busy", "true");
  settingsMessage("Applying…");
  try {
    const r = await invoke<{ settings: Settings }>("send", { cmd: "settings_set", args: { name, value: value.map(String) } });
    showSettings(r.settings);
    settingsMessage("");
  } catch (e) {
    showSettings({});
    settingsMessage(String(e), true);
  } finally {
    settingsPanel.removeAttribute("aria-busy");
  }
}

settingsToggle.addEventListener("click", () => {
  const open = settingsPanel.hidden;
  settingsPanel.hidden = !open;
  settingsToggle.setAttribute("aria-expanded", String(open));
  layout();
  if (open) void readSettings();
});
for (const radio of settingsPanel.querySelectorAll<HTMLInputElement>('input[name="appearance"]')) {
  radio.addEventListener("change", () => void setSetting("appearance", [radio.value]));
}
liquidGlass.addEventListener("input", () => {
  $<HTMLOutputElement>("#liquid-glass-value").value = Number(liquidGlass.value).toFixed(2);
});
liquidGlass.addEventListener("change", () => void setSetting("liquid-glass", [liquidGlass.value]));
textSize.addEventListener("input", () => {
  $<HTMLOutputElement>("#text-size-value").value = TEXT_SIZES[Number(textSize.value)];
});
textSize.addEventListener("change", () => void setSetting("text-size", [TEXT_SIZES[Number(textSize.value)]]));
for (const [key, name] of TOGGLES) {
  const box = item(key).querySelector<HTMLInputElement>("input")!;
  box.addEventListener("change", () => void setSetting(name, [box.checked ? "on" : "off"]));
}
$<HTMLButtonElement>("#location-set").addEventListener("click", () => {
  const [lat, lon] = [latitude.value, longitude.value].map((v) => (v.trim() ? Number(v) : NaN));
  if (!(Math.abs(lat) <= 90) || !(Math.abs(lon) <= 180)) {
    settingsMessage("Enter a latitude (-90 to 90) and a longitude (-180 to 180) in degrees.", true);
    return;
  }
  void setSetting("location", ["set", lat, lon]);
});
$<HTMLButtonElement>("#location-clear").addEventListener("click", () => void setSetting("location", ["clear"]));
$<HTMLButtonElement>("#vo-off").addEventListener("click", () => void setSetting("voiceover", ["off"]));

let statsTimer: number | undefined;
/** Which visit to the viewer the channel's messages belong to. */
let viewerGen = 0;

function showStats(): void {
  const now = performance.now();
  stats.fpsWindow = stats.fpsWindow.filter((t) => now - t < 1000);
  const d = stats.decodeMs.slice(-60);
  const avg = d.length ? d.reduce((a, b) => a + b, 0) / d.length : 0;
  const parts = [
    config ? `${config.codec} ${config.coded.join("×")}` : "waiting for video…",
    `${stats.fpsWindow.length} fps`,
    `decoded ${stats.decoded}/${stats.received}`,
    d.length ? `decode ${avg.toFixed(1)} ms` : "",
    stats.supported === false ? "decoder unsupported" : "",
    stats.lastError,
  ];
  viewerStatus.textContent = parts.filter(Boolean).join(" · ");
}

function report(): void {
  const d = stats.decodeMs.slice(-120);
  void invoke("report", {
    stats: {
      at: new Date().toISOString(),
      userAgent: navigator.userAgent,
      webcodecs: typeof VideoDecoder !== "undefined",
      supported: stats.supported,
      codec: stats.codec,
      received: stats.received,
      decoded: stats.decoded,
      dropped: stats.dropped,
      errors: stats.errors,
      lastError: stats.lastError,
      fps: stats.fpsWindow.length,
      decodeMsAvg: d.length ? d.reduce((a, b) => a + b, 0) / d.length : null,
      decodeMsMax: d.length ? Math.max(...d) : null,
      inputs: stats.inputs,
      lastInput: stats.lastInput,
      touchMsAvg: stats.touchMs.length ? stats.touchMs.reduce((a, b) => a + b, 0) / stats.touchMs.length : null,
      touchMsMax: stats.touchMs.length ? Math.max(...stats.touchMs) : null,
      touchErrors: stats.touchErrors,
      control,
      // What the session panel shows, so a check of the viewer doesn't need anyone to look.
      panel: sessionPanel.hidden
        ? null
        : { name: sessionName.textContent, state: sessionState.textContent, button: sessionPrimary.textContent },
      glow: glow.hidden ? null : glow.classList.contains("human") ? "human" : "agent",
      labelsShown,
      ripplesOnScreen: overlay.querySelectorAll(".ripple").length,
      agentCursor: cursor.hidden
        ? null
        : {
            at: cursorAt?.p ?? null,
            drawnAt: cursorNow(),
            pressing: cursor.classList.contains("pressing"),
            gliding: !!cursorAnim,
            label: cursorLabel?.textContent ?? null,
            lastLabel: cursorLastLabel,
            moves: cursorMoves,
          },
      overlay: [overlay.clientWidth, overlay.clientHeight],
      visibility: document.visibilityState,
      timelineMs: document.timeline.currentTime,
      session_slot: sessionIdle.hidden ? "panel" : "idle",
    },
  });
}

async function openViewer(d: DeviceInfo): Promise<void> {
  picker.hidden = true;
  viewer.hidden = false;
  $<HTMLSpanElement>("#device-name").textContent = d.name ?? d.product_type ?? d.udid;
  viewerStatus.textContent = "Connecting…";
  const channel = new Channel<ArrayBuffer | number[]>();
  const gen = ++viewerGen;
  channel.onmessage = (m) => {
    // Messages of an earlier visit can still be in flight after Back.
    if (gen === viewerGen) onMessage(m instanceof ArrayBuffer ? new Uint8Array(m) : Uint8Array.from(m));
  };
  try {
    await invoke("attach", { udid: d.udid, channel });
  } catch (e) {
    viewerStatus.textContent = String(e);
    return;
  }
  // VoiceOver changes what taps do, so its warning shows whether or not the panel is open.
  void readSettings();
  statsTimer = window.setInterval(() => {
    showStats();
    if (Math.round(performance.now() / 1000) % 2 === 0) report();
  }, 1000);
}

$<HTMLButtonElement>("#back").addEventListener("click", async () => {
  endPress();
  window.clearInterval(statsTimer);
  viewerGen++;
  await invoke("detach");
  configGen++;
  decoder?.close();
  decoder = undefined;
  decoderKey = "";
  pending = [];
  received.clear();
  waitingForKey = true;
  config = undefined;
  settings = {};
  onControl({ session: null, human_touching: false, agent_stopped: false });
  hideCursor();
  voWarning.hidden = true;
  settingsMessage("");
  viewer.hidden = true;
  picker.hidden = false;
});

void (async () => {
  const udid = await invoke<string | null>("initial_udid");
  await load();
  if (udid) void openViewer({ udid, transports: [] });
})();
