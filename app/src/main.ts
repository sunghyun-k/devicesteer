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

interface ControlEvent {
  owner: "shared" | "human";
  agent_stopped: boolean;
  agent_active: boolean;
}

interface InputEvent {
  source: "agent" | "human";
  kind: "tap" | "swipe" | "button";
  points: [number, number][];
  screen: [number, number];
  duration_ms?: number;
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
const badge = $<HTMLSpanElement>("#agent-badge");
const takeOverBtn = $<HTMLButtonElement>("#take-over");
const stopAgentBtn = $<HTMLButtonElement>("#stop-agent");
const ctx = canvas.getContext("2d")!;

let config: Config | undefined;
let decoder: VideoDecoder | undefined;
let waitingForKey = true;
let control: ControlEvent = { owner: "shared", agent_stopped: false, agent_active: false };

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
};
const received = new Map<number, number>();
/** Frames that arrive while the decoder is being set up, from the last keyframe on: the device
 * sends one keyframe per stream, so dropping it means nothing decodes. */
let pending: { header: { keyframe: boolean; pts_us: number }; data: Uint8Array }[] = [];

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
  config = cfg;
  canvas.width = cfg.display_px[0];
  canvas.height = cfg.display_px[1];
  layout();
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
  decoder?.close();
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

function onControl(c: ControlEvent): void {
  control = c;
  badge.hidden = !c.agent_active;
  stage.classList.toggle("agent", c.agent_active && c.owner !== "human");
  stage.classList.toggle("human", c.owner === "human");
  takeOverBtn.textContent = c.owner === "human" ? "Hand back" : "Take over";
  stopAgentBtn.textContent = c.agent_stopped ? "Resume agent" : "Stop agent";
}

/** Where a point of the screen (in points) is on the overlay, in CSS pixels. */
function toOverlay(p: [number, number], screen: [number, number]): [number, number] {
  return [(p[0] / screen[0]) * overlay.clientWidth, (p[1] / screen[1]) * overlay.clientHeight];
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

function onInput(e: InputEvent): void {
  stats.inputs[e.source] = (stats.inputs[e.source] ?? 0) + 1;
  stats.lastInput = `${e.source} ${e.kind} ${JSON.stringify(e.points)}`;
  const color = e.source === "agent" ? "#ff9f0a" : "#0a84ff";
  if (e.kind === "tap" && e.points[0]) {
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
  overlay.style.left = `${r.left - s.left}px`;
  overlay.style.top = `${r.top - s.top}px`;
  overlay.style.width = `${r.width}px`;
  overlay.style.height = `${r.height}px`;
}
new ResizeObserver(layout).observe(stage);

// Human input: a short press is a tap, anything that moved is a swipe of the same length.
let press: { x: number; y: number; t: number; lastMove: number } | undefined;

function toScreen(e: PointerEvent): [number, number] | undefined {
  if (!config) return undefined;
  const r = canvas.getBoundingClientRect();
  return [((e.clientX - r.left) / r.width) * config.screen[0], ((e.clientY - r.top) / r.height) * config.screen[1]];
}

canvas.addEventListener("pointerdown", (e) => {
  canvas.setPointerCapture(e.pointerId);
  press = { x: e.clientX, y: e.clientY, t: performance.now(), lastMove: performance.now() };
});
canvas.addEventListener("pointermove", () => {
  if (press) press.lastMove = performance.now();
});
canvas.addEventListener("pointerup", (e) => {
  if (!press || !config) return;
  const start = press;
  press = undefined;
  const now = performance.now();
  const moved = Math.hypot(e.clientX - start.x, e.clientY - start.y);
  const r = canvas.getBoundingClientRect();
  const scale = config.screen[0] / r.width;
  const from: [number, number] = [((start.x - r.left) * scale), ((start.y - r.top) * scale)];
  const to = toScreen(e)!;
  if (moved < 6) {
    void send("tap", { point: to });
  } else {
    const duration = Math.min(Math.max(now - start.t, 100), 2000);
    // Resting before release means "put it here", not "fling".
    void send("swipe", { from, to, duration_ms: Math.round(duration), place: now - start.lastMove > 120 });
  }
});

$<HTMLButtonElement>("#home").addEventListener("click", () => void send("button", { name: "home" }));
takeOverBtn.addEventListener("click", () =>
  void send("control", { action: control.owner === "human" ? "release" : "take_over" }),
);
stopAgentBtn.addEventListener("click", () =>
  void send("control", { action: control.agent_stopped ? "resume_agent" : "stop_agent" }),
);

let statsTimer: number | undefined;

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
      control,
      badgeVisible: !badge.hidden,
      ripplesOnScreen: overlay.childElementCount,
      overlay: [overlay.clientWidth, overlay.clientHeight],
    },
  });
}

async function openViewer(d: DeviceInfo): Promise<void> {
  picker.hidden = true;
  viewer.hidden = false;
  $<HTMLSpanElement>("#device-name").textContent = d.name ?? d.product_type ?? d.udid;
  viewerStatus.textContent = "Connecting…";
  const channel = new Channel<ArrayBuffer | number[]>();
  channel.onmessage = (m) => onMessage(m instanceof ArrayBuffer ? new Uint8Array(m) : Uint8Array.from(m));
  try {
    await invoke("attach", { udid: d.udid, channel });
  } catch (e) {
    viewerStatus.textContent = String(e);
    return;
  }
  statsTimer = window.setInterval(() => {
    showStats();
    if (Math.round(performance.now() / 1000) % 2 === 0) report();
  }, 1000);
}

$<HTMLButtonElement>("#back").addEventListener("click", async () => {
  window.clearInterval(statsTimer);
  await invoke("detach");
  decoder?.close();
  decoder = undefined;
  config = undefined;
  viewer.hidden = true;
  picker.hidden = false;
});

void (async () => {
  const udid = await invoke<string | null>("initial_udid");
  await load();
  if (udid) void openViewer({ udid, transports: [] });
})();
