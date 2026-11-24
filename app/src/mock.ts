// Dev-only fake backend for designing the viewer in an ordinary browser: open the Vite dev server
// with `?mock` (http://localhost:1420/?mock). main.ts imports this module only when
// `import.meta.env.DEV` is true, so production builds leave it out.
//
// It answers the Tauri commands the viewer calls, draws a placeholder home screen as the video,
// and lets a person (keys, the floating panel) or a script (`window.mock`) cycle through every
// state: device lists, connecting and errors, agent sessions, cursor moves with labels, human
// touches, settings and the VoiceOver warning.
//
// URL options: `devices=normal|empty|error|slow`, `open=1` (show the first device's screen at
// once, as IPHONE_USE_VIEWER_UDID does), `attach=ok|hang|error`, `watch=ok|hang|error`, `panel=0`
// (no floating panel, for screenshots), `rotation=rot270` (start turned), `theme=light|dark`,
// `session=agent|delegated_to_user|user` (a session already runs when the device is attached),
// `lights=0` (no stand-in for the macOS traffic lights), `setup=ok|deny|fail` (how the setup
// steps end: done, the user taps Don't Trust, or an error), `select=<udid suffix>` (pick that
// device first, such as `select=6` for the one that needs Trust).

import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

type Point = [number, number];
type Owner = "agent" | "delegated_to_user" | "user" | "finished";
type Rotation = "rot0" | "rot90" | "rot180" | "rot270";

const params = new URLSearchParams(location.search);
const SCREEN: Point = [428, 926];
const PX: Point = [1284, 2778];

type SetupStep = "trust" | "developer_mode" | "disk_image";
interface MockDevice {
  udid: string;
  transports: string[];
  name?: string;
  product_type?: string;
  model_name?: string;
  device_class?: string;
  os_version?: string;
  build_version?: string;
  setup?: SetupStep;
  error?: string;
}

// Made-up names only: nothing from a real device.
const DEVICES: MockDevice[] = [
  { udid: "00000000-0000000000000001", transports: ["usb", "wifi"], name: "Test iPhone", product_type: "iPhone13,4", model_name: "iPhone 12 Pro Max", device_class: "iPhone", os_version: "26.1", build_version: "23B85" },
  { udid: "00000000-0000000000000002", transports: ["wifi"], name: "Test iPad", product_type: "iPad14,3", model_name: "iPad Pro 11-inch (4th generation)", device_class: "iPad", os_version: "26.0" },
  { udid: "00000000-0000000000000004", transports: ["usb"], name: "QA iPhone", product_type: "iPhone17,2", model_name: "iPhone 16 Pro Max", device_class: "iPhone", os_version: "26.2" },
  { udid: "00000000-0000000000000005", transports: ["wifi"], name: "Old iPhone", product_type: "iPhone12,8", model_name: "iPhone SE (2nd generation)", device_class: "iPhone", os_version: "18.7" },
  { udid: "00000000-0000000000000003", transports: ["usb"], product_type: "iPhone15,2", model_name: "iPhone 14 Pro", error: "The device is locked. Unlock it and trust this computer, then refresh." },
  // One device per setup step. An untrusted device tells no name, only its model and OS.
  { udid: "00000000-0000000000000006", transports: ["usb"], product_type: "iPhone16,1", model_name: "iPhone 15 Pro", device_class: "iPhone", os_version: "26.1", setup: "trust" },
  { udid: "00000000-0000000000000007", transports: ["usb"], name: "Lab iPhone", product_type: "iPhone14,5", model_name: "iPhone 13", device_class: "iPhone", os_version: "26.0", setup: "developer_mode" },
  { udid: "00000000-0000000000000008", transports: ["usb", "wifi"], name: "Spare iPad", product_type: "iPad13,18", model_name: "iPad (10th generation)", device_class: "iPad", os_version: "26.1", setup: "disk_image" },
];

const SETUP_ORDER: SetupStep[] = ["trust", "developer_mode", "disk_image"];

const state = {
  /** Our hardware keyboard, and what the viewer sent: key steps and typed text, for scripts. */
  hardwareKeyboard: false,
  /** The viewer asked for it `while_connected`: it goes when the viewer detaches or the agent gets the device. */
  keyboardHeld: false,
  keyLog: [] as [string, boolean][],
  typed: [] as string[],
  devices: params.get("devices") ?? "normal",
  attach: params.get("attach") ?? "ok",
  watch: params.get("watch") ?? "ok",
  streaming: false,
  channel: undefined as { id: number } | undefined,
  index: 0,
  frameTimer: undefined as number | undefined,
  pts: 0,
  session: (params.get("session")
    ? { id: "s0", name: "Check the Wi-Fi settings", owner: params.get("session") as Owner, reason: params.get("session") === "delegated_to_user" ? "Enter the passcode on the device" : null }
    : null) as null | { id: string; name: string; owner: Owner; reason: string | null },
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
  rotation: (params.get("rotation") ?? "rot0") as Rotation,
  recording: undefined as { path: string; since: number } | undefined,
  lastReport: undefined as unknown,
  /** How setup steps end (`setup=` in the URL). */
  setupEnd: (params.get("setup") ?? "ok") as "ok" | "deny" | "fail",
  /** Bumped by `setup_cancel`: a running step sees it and stops. */
  setupGen: 0,
};

// ---------- placeholder screen ----------

/** The panel's picture, in its native orientation (portrait), as the device's video has it. */
const screen = new OffscreenCanvas(PX[0], PX[1]);
const upright = new OffscreenCanvas(PX[0], PX[1]);
const quarter = () => state.rotation === "rot90" || state.rotation === "rot270";
/** The screen as the user sees it, in points. */
const turnedScreen = (): Point => (quarter() ? [SCREEN[1], SCREEN[0]] : SCREEN);

/** Draws the screen as the user sees it, then turns it onto the panel as the device does: the
 * video stays portrait and the interface turns inside it (PITFALLS #27). */
function drawScreen(): void {
  const [w, h] = turnedScreen();
  if (quarter()) drawLandscape(w, h);
  else drawPortrait();
  const g = screen.getContext("2d")!;
  // Screen to panel: the inverse of the viewer's panelToCanvas.
  const [pw, ph] = PX;
  const m: Record<Rotation, [number, number, number, number, number, number]> = {
    rot0: [1, 0, 0, 1, 0, 0],
    rot90: [0, -1, 1, 0, 0, ph],
    rot180: [-1, 0, 0, -1, pw, ph],
    rot270: [0, 1, -1, 0, pw, 0],
  };
  g.setTransform(...m[state.rotation]);
  g.drawImage(upright, 0, 0);
  g.setTransform(1, 0, 0, 1, 0, 0);
}

/** A landscape app: a top bar, a sidebar of rows and a content area. */
function drawLandscape(w: number, h: number): void {
  const s = 3;
  upright.width = w * s;
  upright.height = h * s;
  const g = upright.getContext("2d")!;
  const dark = state.settings.appearance !== "light";
  g.fillStyle = dark ? "#1c1c1e" : "#f2f2f7";
  g.fillRect(0, 0, w * s, h * s);
  g.fillStyle = dark ? "#2c2c2e" : "#fff";
  g.fillRect(0, 0, w * s, 52 * s);
  g.fillStyle = dark ? "#fff" : "#000";
  g.font = `600 ${17 * s}px -apple-system, system-ui, sans-serif`;
  g.textBaseline = "middle";
  g.fillText("Landscape", 60 * s, 28 * s);
  for (let i = 0; i < 6; i++) roundRect(g, 60 * s, (72 + i * 52) * s, 240 * s, 40 * s, 10 * s, dark ? "#2c2c2e" : "#fff");
  roundRect(g, 320 * s, 72 * s, (w - 380) * s, (h - 100) * s, 14 * s, `hsl(210 55% ${dark ? 40 : 70}%)`);
  roundRect(g, (w / 2 - 68) * s, (h - 9) * s, 136 * s, 5 * s, 2.5 * s, dark ? "rgba(255,255,255,0.9)" : "rgba(0,0,0,0.8)");
}

/** A generic home screen: wallpaper, status bar, a grid of blank icons, a dock. No real apps. */
function drawPortrait(): void {
  upright.width = PX[0];
  upright.height = PX[1];
  const g = upright.getContext("2d")!;
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

/** The coded picture: the screen in its top-left corner, padded with black to the encoder's
 * block size, as the device sends it. */
const CODED: Point = [Math.ceil(PX[0] / 16) * 16, Math.ceil(PX[1] / 64) * 64];
const coded = new OffscreenCanvas(CODED[0], CODED[1]);

/** Stands in for WebCodecs: every "decoded" frame is a padded picture of the placeholder screen,
 * with the padding inside its visible rect. */
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
    const g = coded.getContext("2d")!;
    g.fillStyle = "#000";
    g.fillRect(0, 0, CODED[0], CODED[1]);
    g.drawImage(screen, 0, 0);
    const frame = new VideoFrame(coded, { timestamp: chunk.timestamp });
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
  // As the daemon does: after the control event, the agent getting the device removes the
  // viewer's keyboard.
  if (state.session?.owner === "agent" && state.keyboardHeld) {
    state.keyboardHeld = false;
    state.hardwareKeyboard = false;
    event({ event: "keyboard", hardware: false, attached: false });
  }
}

function settingsEvent(): void {
  event({ event: "settings", ...state.settings });
}

function startStream(): void {
  if (state.streaming) return;
  state.streaming = true;
  drawScreen();
  event({ event: "config", codec: "hvc1.mock", description: "", coded: CODED, display_px: PX, ...orientation() });
  window.setTimeout(() => {
    if (!state.streaming) return;
    frame(true);
    state.frameTimer = window.setInterval(() => frame(false), 1000 / 30);
  }, 120);
}

function stopStream(): void {
  state.streaming = false;
  window.clearInterval(state.frameTimer);
}

function orientation() {
  const screen = turnedScreen();
  return { rotation: state.rotation, orientation: screen[0] > screen[1] ? "landscape" : "portrait", screen };
}

function input(source: "agent" | "human", kind: string, points: Point[], extra: Record<string, unknown> = {}): void {
  event({ event: "input", source, kind, points, screen: turnedScreen(), ...extra });
}

const sleep = (ms: number) => new Promise((r) => window.setTimeout(r, ms));

// ---------- setup ----------

function setupStatus(d: MockDevice) {
  const at = d.setup ? SETUP_ORDER.indexOf(d.setup) : SETUP_ORDER.length;
  const next: Record<SetupStep, string> = {
    trust: `run \`iphone-use setup trust --udid ${d.udid}\` and ask the user to unlock the device, tap Trust and enter its passcode`,
    developer_mode: `run \`iphone-use setup developer-mode --udid ${d.udid}\`, then ask the user to turn on Developer Mode`,
    disk_image: `run \`iphone-use setup disk-image --udid ${d.udid}\` (no action on the device is needed)`,
  };
  return {
    udid: d.udid,
    transports: d.transports,
    ready: !d.setup,
    step: d.setup ?? null,
    steps: SETUP_ORDER.map((step, i) => ({ step, state: i < at ? "done" : i === at ? "needed" : "blocked" })),
    next: d.setup ? next[d.setup] : "the device is ready",
  };
}

/** Plays one setup step the way the core reports it, then moves the device to the next step. */
async function setupRun(d: MockDevice, step: SetupStep, channel: { id: number }): Promise<unknown> {
  const gen = ++state.setupGen;
  let index = 0;
  const say = (message: string, waiting = false, fraction?: number) => {
    if (gen !== state.setupGen) throw "cancelled";
    window.__TAURI_INTERNALS__.runCallback(channel.id, { index: index++, message: { step, waiting_for_user: waiting, message, fraction } });
  };
  const wait = async (ms: number) => {
    await sleep(ms);
    if (gen !== state.setupGen) throw "cancelled";
  };
  const fail = (text: string) => {
    if (state.setupEnd === "fail") throw text;
  };
  if (step === "trust") {
    say("Unlock the device, then tap Trust and enter its passcode", true);
    await wait(2500);
    if (state.setupEnd === "deny")
      throw "the user tapped Don't Trust. Unplug the cable and plug it back in so the device asks again, then run `iphone-use setup trust` again";
    fail("pairing failed (connection error: the device closed the connection)");
    say("Trusted. Saving the pair record");
    await wait(500);
  } else if (step === "developer_mode") {
    say("On the device: Settings > Privacy & Security > Developer Mode, turn it on and tap Restart. After the restart, unlock it, tap Turn On and enter the passcode", true);
    await wait(2500);
    fail("revealing the Developer Mode switch failed (connection error: no such service)");
    say("The device is restarting", true);
    await wait(2500);
    say("Unlock the device, tap Turn On and enter the passcode", true);
    await wait(2000);
  } else {
    for (let i = 0; i < 3; i++) {
      say(`Downloading the developer disk image (${["BuildManifest.plist", "Image.dmg", "Image.dmg.trustcache"][i]})`, false, i / 3);
      await wait(600);
    }
    say("Using the developer disk image from the downloaded developer disk image");
    say("Asking Apple to sign the disk image for this device");
    await wait(800);
    fail("mounting the developer disk image failed: TSS server responded with non-success status (STATUS=94)");
    for (let f = 0; f <= 1.0001; f += 0.1) {
      say("Uploading the disk image", false, f);
      await wait(150);
    }
    say("Mounted");
  }
  d.setup = SETUP_ORDER[SETUP_ORDER.indexOf(step) + 1];
  return setupStatus(d);
}

// ---------- commands ----------

async function handle(cmd: string, args: Record<string, unknown>): Promise<unknown> {
  switch (cmd) {
    case "initial_udid":
      if (params.get("select")) return DEVICES.find((d) => d.udid.endsWith(params.get("select")!))?.udid ?? null;
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
      window.setTimeout(() => {
        control();
        if (args.frames) startStream();
      }, 50);
      return {};
    case "watch":
      if (args.frames) {
        if (state.watch === "hang") return new Promise(() => {});
        await sleep(500);
        if (state.watch === "error") throw "the screen stream did not start: no media packet arrived. Try again.";
        startStream();
      } else stopStream();
      return {};
    case "setup_status": {
      await sleep(150);
      const d = DEVICES.find((x) => x.udid === args.udid);
      if (!d) throw `device ${args.udid} is not connected (no usbmuxd connection). Run \`devices\` to see what is`;
      return setupStatus(d);
    }
    case "setup_run": {
      const d = DEVICES.find((x) => x.udid === args.udid);
      if (!d) throw "the device is not connected";
      return setupRun(d, args.step as SetupStep, args.channel as { id: number });
    }
    case "setup_cancel":
      state.setupGen++;
      return null;
    case "detach":
      if (state.keyboardHeld) state.keyboardHeld = state.hardwareKeyboard = false;
      stopStream();
      state.channel = undefined;
      return null;
    case "save_screenshot":
      return "/Users/me/Pictures/iphone-use 2026-11-24 18.52.07.png";
    case "report":
      state.lastReport = args.stats;
      return null;
    case "recording_path":
      return `/Users/me/Movies/iphone-use ${args.stamp}.mp4`;
    case "reveal":
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
    case "keyboard":
      state.keyLog.push([String(a.code), Boolean(a.down)]);
      return { down: a.down };
    case "keyboard_connect":
    case "keyboard_disconnect":
      await sleep(300);
      state.hardwareKeyboard = cmd === "keyboard_connect";
      state.keyboardHeld = state.hardwareKeyboard && a.while_connected === true;
      return { event: "keyboard", hardware: state.hardwareKeyboard, attached: state.hardwareKeyboard };
    case "keyboard_status":
      return { event: "keyboard", hardware: state.hardwareKeyboard, attached: state.hardwareKeyboard };
    case "type":
      await sleep(600);
      state.typed.push(String(a.text));
      return { method: "stream", chars: String(a.text).length, verified: true };
    case "record_start":
      await sleep(200);
      return mock.recordStart(String(a.path));
    case "record_stop":
      await sleep(300);
      return mock.recordStop();
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
  /** Key steps (`[code, down]`) and texts the viewer sent; reading empties them. */
  takeKeys() {
    return state.keyLog.splice(0);
  },
  takeTyped() {
    return state.typed.splice(0);
  },
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
    input("agent", "tap", [[x, y]], { label, duration_ms: 50 });
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
  /** Turns the interface; the next frame shows it, as on the device. */
  rotate(r: Rotation = state.rotation === "rot0" ? "rot270" : "rot0") {
    state.rotation = r;
    drawScreen();
    event({ event: "orientation", ...orientation() });
  },
  /** A recording started here or, with no path, by the CLI. */
  recordStart(path = "/Users/me/Desktop/screen.mp4") {
    if (state.recording) throw "a recording already runs; end it with `record stop` first";
    state.recording = { path, since: performance.now() };
    event({ event: "recording", recording: true, path, seconds: 0 });
    return { recording: path };
  },
  recordStop(fail = false) {
    const r = state.recording;
    if (!r) throw "no recording runs; start one with `record start`";
    state.recording = undefined;
    const seconds = (performance.now() - r.since) / 1000;
    const summary = { path: r.path, seconds, bytes: 1_200_000, frames: Math.round(seconds * 30) };
    event({ event: "recording", recording: false, path: r.path, ...(fail ? { error: "no keyframe arrived" } : { summary }) });
    return summary;
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
  /** Puts a device at a setup step (or ready with null), as if the person did steps elsewhere. */
  setup(udidSuffix: string, step: SetupStep | null) {
    const d = DEVICES.find((x) => x.udid.endsWith(udidSuffix));
    if (d) d.setup = step ?? undefined;
  },
  setupEnd(end: "ok" | "deny" | "fail") {
    state.setupEnd = end;
  },
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
  ["r", "Turn the screen", () => mock.rotate()],
  ["c", "CLI record start/stop", () => (state.recording ? mock.recordStop() : mock.recordStart())],
  ["e", "Setup steps end: ok/deny/fail", () => mock.setupEnd(state.setupEnd === "ok" ? "deny" : state.setupEnd === "deny" ? "fail" : "ok")],
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
    if (e.target instanceof HTMLInputElement || e.metaKey || e.ctrlKey) return;
    if (e.key === "`") el.hidden = !el.hidden;
    KEYS.find(([k]) => k === e.key)?.[2]();
  });
}

/** The three dots where macOS draws the traffic lights (trafficLightPosition in tauri.conf.json),
 * so the headers' room for them reads. */
function lights(): void {
  const style = document.createElement("style");
  style.textContent =
    ":root.mac body::after{content:'';position:fixed;z-index:50;left:17px;top:16px;pointer-events:none;" +
    "width:14px;height:14px;border-radius:50%;background:#ff5f57;box-shadow:23px 0 #febc2e,46px 0 #28c840}";
  document.head.append(style);
}

export function installMock(): void {
  const theme = params.get("theme");
  if (theme) document.documentElement.dataset.theme = theme;
  if (params.get("lights") !== "0") lights();
  mockWindows("main");
  mockIPC((cmd, args) => handle(cmd, (args ?? {}) as Record<string, unknown>));
  (window as unknown as { VideoDecoder: unknown }).VideoDecoder = FakeDecoder;
  window.mock = mock;
  // Module scripts run after the document is parsed, so the body is there.
  if (params.get("panel") !== "0") panel();
}
