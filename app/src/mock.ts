// Dev-only fake backend for designing the viewer in an ordinary browser: open the Vite dev server
// with `?mock` (http://localhost:1420/?mock). main.ts imports this module only when
// `import.meta.env.DEV` is true, so production builds leave it out.
//
// It answers the Tauri commands the viewer calls, draws a placeholder home screen as the video,
// and lets a person (keys, the floating panel) or a script (`window.mock`) cycle through every
// state: device lists, connecting and errors, agent sessions, cursor moves with labels, human
// touches, settings and the VoiceOver warning.
//
// URL options: `devices=normal|empty|error|slow`, `open=1` (open the first device at once),
// `attach=ok|hang|error`, `panel=0` (no floating panel, for screenshots).

import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

type Point = [number, number];
type Owner = "agent" | "delegated_to_user" | "user" | "finished";

const params = new URLSearchParams(location.search);
const SCREEN: Point = [428, 926];
const PX: Point = [1284, 2778];

const DEVICES = [
  { udid: "00000000-0000000000000001", transports: ["usb", "wifi"], name: "Test iPhone", product_type: "iPhone13,4", device_class: "iPhone", os_version: "26.1" },
  { udid: "00000000-0000000000000002", transports: ["wifi"], name: "Test iPad", product_type: "iPad14,3", device_class: "iPad", os_version: "26.0" },
  { udid: "00000000-0000000000000003", transports: ["usb"], product_type: "iPhone15,2", error: "The device is locked. Unlock it and trust this computer, then refresh." },
];

const state = {
  devices: params.get("devices") ?? "normal",
  attach: params.get("attach") ?? "ok",
  channel: undefined as { id: number } | undefined,
  index: 0,
  frameTimer: undefined as number | undefined,
  pts: 0,
  session: null as null | { id: string; name: string; owner: Owner; reason: string | null },
  settings: {
    appearance: "dark" as "light" | "dark" | null,
    text_size: "l" as string | null,
    liquid_glass: 0.35 as number | null,
    reduce_motion: false as boolean | null,
    increase_contrast: false as boolean | null,
    show_borders: false as boolean | null,
    reduce_transparency: false as boolean | null,
    voiceover: false as boolean | null,
    location: null as Point | null,
    unsupported: [] as string[],
  },
  settingsFail: false,
  lastReport: undefined as unknown,
};

// ---------- placeholder screen ----------

const screen = new OffscreenCanvas(PX[0], PX[1]);

/** A generic home screen: wallpaper, status bar, a grid of blank icons, a dock. No real apps. */
function drawScreen(): void {
  const g = screen.getContext("2d")!;
  const dark = state.settings.appearance !== "light";
  const bg = g.createLinearGradient(0, 0, PX[0], PX[1]);
  bg.addColorStop(0, dark ? "#2b3145" : "#a9c1e8");
  bg.addColorStop(0.55, dark ? "#1b1f2c" : "#d7c9ec");
  bg.addColorStop(1, dark ? "#0d0f15" : "#f2d6c9");
  g.fillStyle = bg;
  g.fillRect(0, 0, PX[0], PX[1]);
  const s = 3; // points to pixels
  g.fillStyle = "#fff";
  g.font = `600 ${17 * s}px -apple-system, system-ui, sans-serif`;
  g.textBaseline = "middle";
  g.fillText("9:41", 52 * s, 30 * s);
  g.fillRect(360 * s, 24 * s, 26 * s, 12 * s);
  const hues = [210, 140, 25, 280, 350, 190, 45, 100, 320, 0, 260, 170, 60, 230, 300, 120, 15, 200, 80, 330];
  const cell = 428 / 4;
  for (let i = 0; i < 20; i++) {
    const x = (i % 4) * cell + (cell - 64) / 2;
    const y = 90 + Math.floor(i / 4) * 104;
    roundRect(g, x * s, y * s, 64 * s, 64 * s, 15 * s, `hsl(${hues[i]} 55% ${dark ? 45 : 58}%)`);
    g.fillStyle = "rgba(255,255,255,0.85)";
    roundRect(g, (x + 18) * s, (y + 78) * s, 28 * s, 6 * s, 3 * s, "rgba(255,255,255,0.75)");
  }
  roundRect(g, 14 * s, 806 * s, 400 * s, 92 * s, 34 * s, "rgba(255,255,255,0.22)");
  for (let i = 0; i < 4; i++) roundRect(g, (36 + i * 98) * s, 820 * s, 64 * s, 64 * s, 15 * s, `hsl(${hues[i * 3]} 50% 52%)`);
  roundRect(g, 146 * s, 912 * s, 136 * s, 5 * s, 2.5 * s, "rgba(255,255,255,0.9)");
}

function roundRect(g: OffscreenCanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number, fill: string): void {
  g.fillStyle = fill;
  g.beginPath();
  g.roundRect(x, y, w, h, r);
  g.fill();
}

/** Stands in for WebCodecs: every "decoded" frame is a picture of the placeholder screen. */
class FakeDecoder {
  state: "unconfigured" | "configured" | "closed" = "unconfigured";
  constructor(private init: VideoDecoderInit) {}
  static async isConfigSupported(config: VideoDecoderConfig) {
    return { supported: true, config };
  }
  configure() {
    this.state = "configured";
  }
  decode(chunk: EncodedVideoChunk) {
    const frame = new VideoFrame(screen, { timestamp: chunk.timestamp });
    queueMicrotask(() => this.init.output(frame));
  }
  close() {
    this.state = "closed";
  }
}

// ---------- the wire ----------

/** Sends one message the way the Tauri shell forwards the daemon's wire format. */
function emit(kind: 0 | 1, payload: Uint8Array): void {
  if (!state.channel) return;
  const buf = new Uint8Array(5 + payload.length);
  new DataView(buf.buffer).setUint32(0, 1 + payload.length);
  buf[4] = kind;
  buf.set(payload, 5);
  window.__TAURI_INTERNALS__.runCallback(state.channel.id, { index: state.index++, message: buf.buffer });
}

function event(ev: Record<string, unknown>): void {
  emit(0, new TextEncoder().encode(JSON.stringify(ev)));
}

function frame(keyframe: boolean): void {
  const header = new TextEncoder().encode(JSON.stringify({ keyframe, pts_us: (state.pts += 33_333) }));
  const p = new Uint8Array(4 + header.length + 8);
  new DataView(p.buffer).setUint32(0, header.length);
  p.set(header, 4);
  emit(1, p);
}

function control(): void {
  event({ event: "control", session: state.session, human_touching: false, agent_stopped: false });
}

function settingsEvent(): void {
  event({ event: "settings", ...state.settings });
}

function startStream(): void {
  drawScreen();
  event({ event: "config", codec: "hvc1.mock", description: "", coded: [PX[0], PX[1] + 6], display_px: PX, screen: SCREEN });
  window.setTimeout(() => {
    frame(true);
    state.frameTimer = window.setInterval(() => frame(false), 1000 / 30);
  }, 120);
  control();
}

function input(source: "agent" | "human", kind: string, points: Point[], extra: Record<string, unknown> = {}): void {
  event({ event: "input", source, kind, points, screen: SCREEN, ...extra });
}

const sleep = (ms: number) => new Promise((r) => window.setTimeout(r, ms));

// ---------- commands ----------

async function handle(cmd: string, args: Record<string, unknown>): Promise<unknown> {
  switch (cmd) {
    case "initial_udid":
      return params.get("open") ? DEVICES[0].udid : null;
    case "list_devices":
      if (state.devices === "slow") await sleep(2500);
      else await sleep(250);
      if (state.devices === "error") throw "usbmuxd is not running. Install iTunes or Apple Devices, or start usbmuxd, then refresh.";
      return state.devices === "empty" ? [] : DEVICES;
    case "attach":
      if (state.attach === "hang") return new Promise(() => {});
      await sleep(600);
      if (state.attach === "error") throw "connection to the device failed: the tunnel did not come up. Unlock the device and try again.";
      state.channel = args.channel as { id: number };
      state.index = 0;
      window.setTimeout(startStream, 50);
      return {};
    case "detach":
      window.clearInterval(state.frameTimer);
      state.channel = undefined;
      return null;
    case "report":
      state.lastReport = args.stats;
      return null;
    case "send":
      return send(String(args.cmd), (args.args ?? {}) as Record<string, unknown>);
  }
  throw `mock: unknown command ${cmd}`;
}

async function send(cmd: string, a: Record<string, unknown>): Promise<unknown> {
  switch (cmd) {
    case "settings":
      await sleep(300);
      return state.settings;
    case "settings_set": {
      await sleep(400);
      if (state.settingsFail) throw "settings_set failed: the device did not answer. Try again.";
      const [v, x, y] = (a.value as string[]) ?? [];
      const key = String(a.name).replace(/-/g, "_");
      const s = state.settings as Record<string, unknown>;
      if (key === "location") s.location = v === "set" ? [Number(x), Number(y)] : null;
      else if (key === "liquid_glass") s.liquid_glass = Number(v);
      else if (key === "appearance" || key === "text_size") s[key] = v;
      else s[key === "show_borders" ? "show_borders" : key] = v === "on";
      if (key === "appearance") drawScreen();
      return { settings: state.settings };
    }
    case "touch": {
      const t = a as { phase: string; point: Point };
      input("human", "touch", [t.point], { phase: t.phase });
      return {};
    }
    case "button":
      return {};
    case "control": {
      const s = state.session;
      if (!s) throw "no agent session runs";
      const action = a.action;
      if (action === "take_over") s.owner = "user";
      else if (action === "hand_back" || action === "done") s.owner = "agent";
      else if (action === "stop") s.owner = "finished";
      s.reason = s.owner === "delegated_to_user" ? s.reason : null;
      control();
      if (s.owner === "finished") window.setTimeout(() => ((state.session = null), control()), 300);
      return {};
    }
  }
  return {};
}

// ---------- scripted states ----------

const LABELS = ["Open Settings", "Tap Wi-Fi", "Scroll to Privacy & Security", "Search for an app by name in the field at the top", "Back"];
let labelIndex = 0;

const mock = {
  /** `owner` null ends the session. */
  session(owner: Owner | null = "agent", name = "Check the Wi-Fi settings", reason: string | null = null) {
    if (!owner) state.session = null;
    else {
      state.session = { id: state.session?.id ?? `s${Date.now()}`, name, owner, reason };
      if (owner === "delegated_to_user" && !reason) state.session.reason = "Enter the passcode on the device";
    }
    control();
  },
  tap(x = 120 + Math.random() * 200, y = 200 + Math.random() * 500, label: string | null = LABELS[labelIndex++ % LABELS.length]) {
    input("agent", "tap", [[x, y]], { label });
  },
  swipe(from: Point = [214, 700], to: Point = [214, 260], label: string | null = "Scroll down the list") {
    input("agent", "swipe", [from, to], { label, duration_ms: 400 });
  },
  button(label = "Go home") {
    input("agent", "button", [], { label });
  },
  humanTap(x = 214, y = 463) {
    input("human", "touch", [[x, y]], { phase: "down" });
    window.setTimeout(() => input("human", "touch", [[x, y]], { phase: "up" }), 160);
  },
  /** A human finger held down, for a screenshot of the dot. */
  humanHold(x = 300, y = 520) {
    input("human", "touch", [[x, y]], { phase: "down" });
  },
  humanLift(x = 300, y = 520) {
    input("human", "touch", [[x, y]], { phase: "up" });
  },
  settings(s: Partial<typeof state.settings>) {
    Object.assign(state.settings, s);
    if ("appearance" in s) drawScreen();
    settingsEvent();
  },
  voiceover(on = true) {
    mock.settings({ voiceover: on });
  },
  unsupported(items: string[] = ["liquid_glass", "reduce_transparency"]) {
    mock.settings({ unsupported: items, liquid_glass: null, reduce_transparency: null });
  },
  settingsFail(on = true) {
    state.settingsFail = on;
  },
  devices(kind: "normal" | "empty" | "error" | "slow") {
    state.devices = kind;
  },
  attach(kind: "ok" | "hang" | "error") {
    state.attach = kind;
  },
  report: () => state.lastReport,
};

declare global {
  interface Window {
    mock: typeof mock;
    __TAURI_INTERNALS__: { runCallback(id: number, data: unknown): void };
  }
}

// ---------- floating panel ----------

const KEYS: [string, string, () => void][] = [
  ["1", "Agent session", () => mock.session("agent")],
  ["2", "Waiting for you", () => mock.session("delegated_to_user")],
  ["3", "You are in control", () => mock.session("user")],
  ["0", "No session", () => mock.session(null)],
  ["t", "Agent tap + label", () => mock.tap()],
  ["s", "Agent swipe", () => mock.swipe()],
  ["b", "Agent button", () => mock.button()],
  ["h", "Human tap", () => mock.humanTap()],
  ["v", "VoiceOver on/off", () => mock.voiceover(!state.settings.voiceover)],
  ["u", "Unsupported settings", () => mock.unsupported()],
  ["f", "Settings fail", () => mock.settingsFail(!state.settingsFail)],
];

function panel(): void {
  const el = document.createElement("div");
  el.id = "mock-panel";
  el.style.cssText =
    "position:fixed;left:8px;bottom:8px;z-index:100;font:11px/1.4 ui-monospace,monospace;" +
    "background:rgba(0,0,0,.75);color:#fff;border-radius:8px;padding:6px 8px;max-width:180px;";
  el.innerHTML = `<b>mock</b> (\` hides)<br>${KEYS.map(([k, t]) => `<kbd>${k}</kbd> ${t}`).join("<br>")}`;
  document.body.append(el);
  window.addEventListener("keydown", (e) => {
    if (e.target instanceof HTMLInputElement) return;
    if (e.key === "`") el.hidden = !el.hidden;
    KEYS.find(([k]) => k === e.key)?.[2]();
  });
}

export function installMock(): void {
  mockWindows("main");
  mockIPC((cmd, args) => handle(cmd, (args ?? {}) as Record<string, unknown>));
  (window as unknown as { VideoDecoder: unknown }).VideoDecoder = FakeDecoder;
  window.mock = mock;
  // Module scripts run after the document is parsed, so the body is there.
  if (params.get("panel") !== "0") panel();
}
