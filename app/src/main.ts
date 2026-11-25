import { Channel, invoke } from "@tauri-apps/api/core";
import { initAgents, openSettings } from "./agents";
import { listen } from "@tauri-apps/api/event";
import { errorText, localize, num, onLanguageChange, t, type Key } from "./i18n";

// Dev only: `?mock` runs the viewer against a fake backend in a plain browser (see src/mock.ts).
// Vite replaces `import.meta.env.DEV` with false in builds, which drops the mock entirely.
if (import.meta.env.DEV && new URLSearchParams(location.search).has("mock")) (await import("./mock")).installMock();

/** Mirrors `devicesteer_core::DeviceInfo`. */
interface DeviceInfo {
  udid: string;
  transports: ("usb" | "wifi")[];
  name?: string;
  product_type?: string;
  /** "iPhone 12 Pro Max", when the core knows the product type. */
  model_name?: string;
  device_class?: string;
  os_version?: string;
  build_version?: string;
  /** The first setup step the device still needs; absent when it is ready. */
  setup?: SetupStep;
  /** Why DeviceSteer can't work with it at all (older than iOS 27). It has no `setup`. */
  unsupported?: string;
  error?: string;
}

/** Mirrors `devicesteer_core::setup`: the steps that make a device ready, in order. */
type SetupStep = "trust" | "developer_mode" | "disk_image";

interface SetupStatus {
  udid: string;
  transports: ("usb" | "wifi")[];
  ready: boolean;
  step: SetupStep | null;
  steps: { step: SetupStep; state: "done" | "needed" | "blocked"; detail?: string }[];
  next: string;
}

/** What a running setup step reports (`devicesteer_core::setup::Progress`). */
interface SetupProgress {
  step: SetupStep;
  /** The device waits for the person holding it. */
  waiting_for_user: boolean;
  message: string;
  fraction?: number;
  /** Wi-Fi pairing: the code to type on the device, once it asked for one. */
  code?: string;
}

/** How the interface is turned from the panel's native orientation (`devicesteer_core::Rotation`). */
type Rotation = "rot0" | "rot90" | "rot180" | "rot270";

/** How the screen is turned: the daemon's `orientation` event, also part of `config`. */
interface Orientation {
  /** The video picture is the panel in its native orientation; this turns it upright. */
  rotation: Rotation;
  orientation: "portrait" | "landscape";
  /** The screen as the user sees it, in points: input points and the overlay use this. */
  screen: [number, number];
}

/** The daemon's `config` event: how to set up the decoder and where the screen is. */
interface Config extends Orientation {
  codec: string;
  description: string;
  coded: [number, number];
  /** The screen's pixel size in the picture, native orientation: the top-left part of the coded
   * picture. Smaller than the panel when the device scaled it down to fit the stream (a foldable's
   * inner panel, PITFALLS #48); the rest of the picture is padding. */
  display_px: [number, number];
}

/** An agent session as the daemon reports it (`devicesteer_daemon::control::AgentSession`). */
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
  /** For `swipe`: how long it takes; for `tap`: how long the finger stays down from now. */
  duration_ms?: number;
  /** For `touch`: one step of a live touch. */
  phase?: "down" | "move" | "up";
  /** For a `touch` up the daemon did itself: why (timeout, disconnect, control change). */
  lifted?: string;
  /** What the input is for, in a few words (`--label`), shown next to it. */
  label?: string | null;
}

/** The daemon's `recording` event: a recording started or stopped, from any client. */
interface RecordingEvent {
  recording: boolean;
  path: string;
  /** While it runs: seconds since it started. */
  seconds?: number;
  /** When it stopped (`devicesteer_core::RecordSummary`), or why it failed. */
  summary?: { seconds: number; bytes: number; frames: number };
  error?: string;
}

/** Mirrors `devicesteer_core::DeviceSettings`, plus the location the daemon simulated. `null`:
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

type Point = [number, number];

const $ = <T extends HTMLElement>(sel: string) => document.querySelector<T>(sel)!;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className: string, text?: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  e.className = className;
  if (text !== undefined) e.textContent = text;
  return e;
}

function icon(name: string, className = "icon"): SVGSVGElement {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("class", className);
  svg.setAttribute("aria-hidden", "true");
  const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
  use.setAttribute("href", `#${name}`);
  svg.append(use);
  return svg;
}

/** Local settings of the window (panels shown, last device): conveniences only, so storage that
 * is missing or throws just means defaults. */
const prefs = {
  get(key: string): string | null {
    try {
      return localStorage.getItem(`devicesteer.${key}`);
    } catch {
      return null;
    }
  },
  set(key: string, value: string): void {
    try {
      localStorage.setItem(`devicesteer.${key}`, value);
    } catch {
      // Not kept; the default comes back next time.
    }
  },
};

// ---------- window chrome ----------

const app = $<HTMLDivElement>("#app");
const center = $<HTMLElement>("#center");
const root = document.documentElement;

// On macOS the window has no title bar (titleBarStyle Overlay) and the traffic lights sit over
// the top-left corner; the headers keep room for them. Elsewhere the window keeps its frame.
if (/Mac/.test(navigator.platform) && "__TAURI_INTERNALS__" in window) root.classList.add("mac");

/** Sidebar and inspector, each shown or hidden. Narrow windows start with them hidden: they
 * float over the stage there (style.css, "narrow windows"). */
function showPanel(which: "sidebar" | "inspector", on: boolean, remember = true): void {
  app.classList.toggle(`no-${which}`, !on);
  $<HTMLButtonElement>(`#${which}-toggle`).setAttribute("aria-pressed", String(on));
  if (remember) prefs.set(which, on ? "1" : "0");
  layout();
}
const narrow = (px: number) => window.innerWidth <= px;
$<HTMLButtonElement>("#sidebar-toggle").addEventListener("click", () =>
  showPanel("sidebar", app.classList.contains("no-sidebar")),
);
$<HTMLButtonElement>("#inspector-toggle").addEventListener("click", () =>
  showPanel("inspector", app.classList.contains("no-inspector")),
);

// ---------- devices ----------

const availableList = $<HTMLUListElement>("#available");
const unavailableList = $<HTMLUListElement>("#unavailable");
const listMessage = $<HTMLDivElement>("#list-message");
const search = $<HTMLInputElement>("#search");
const refreshBtn = $<HTMLButtonElement>("#refresh");

let devices: DeviceInfo[] = [];
let selected: DeviceInfo | undefined;
let listError: string | undefined;
/** Set while the selected device has left the device list (unplugged, or restarting, which
 * turning on Developer Mode does): what to go back to when the same UDID comes back. The
 * selection stays, so it picks up again by itself. */
let gone: { view: boolean } | undefined;

/** "iOS 26.1", "iPadOS 26.0", when the device said. */
function osLabel(d: DeviceInfo): string | undefined {
  if (!d.os_version) return undefined;
  return `${d.device_class === "iPad" ? "iPadOS" : "iOS"} ${d.os_version}`;
}

function deviceName(d: DeviceInfo): string {
  return d.name ?? modelName(d) ?? t("device.unknown");
}

/** What Apple calls the model, or its identifier when the core doesn't know it. */
const modelName = (d: DeviceInfo) => d.model_name ?? d.product_type;

/** A device that can't be opened: unreadable, or too old. Listed under Unavailable. */
const unusable = (d: DeviceInfo | undefined) => !!d?.error || !!d?.unsupported;

/** What the stage says about a device that is too old. */
function unsupportedText(d: DeviceInfo): string {
  const os = osLabel(d);
  return os ? t("device.unsupportedRuns", { os }) : t("device.unsupported");
}

const isTablet = (d: DeviceInfo | undefined) => d?.device_class === "iPad" || !!d?.product_type?.startsWith("iPad");

function row(d: DeviceInfo, disconnected = false): HTMLLIElement {
  const li = el("li", "dev");
  li.dataset.udid = d.udid;
  li.setAttribute("role", "option");
  li.tabIndex = 0;
  // One line: the name, then its status in gray: the OS version and how it is connected, or why
  // it can't be used. Other states (setting a device up, say) belong in the status too.
  const status = disconnected
    ? t("device.disconnected")
    : d.unsupported
    ? t("device.needsIos27")
    : d.error
    ? t("device.unavailable")
    : d.setup
      ? t("device.setupStatus", { step: setupShort(d.setup) })
      : [d.name ? modelName(d) : undefined, d.transports.includes("usb") ? "USB" : d.transports.length ? "Wi-Fi" : undefined].filter(Boolean).join(" · ");
  const text = el("div", "text");
  text.append(el("span", "name", deviceName(d)), el("span", "status", status));
  li.append(icon(isTablet(d) ? "i-tablet" : "i-phone", "icon glyph"), text);
  // The core's own reasons (`unsupported`, `error`) stay in English.
  li.title = disconnected
    ? t("device.disconnectedHelp")
    : d.unsupported ?? d.error ?? (d.setup ? t("device.needsSetup", { step: setupTitle(d.setup) }) : [deviceName(d), modelName(d), osLabel(d)].filter(Boolean).join(" · "));
  if (unusable(d) || disconnected) li.classList.add("unavailable");
  li.addEventListener("click", () => void select(d));
  li.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      void select(d);
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const rows = [...document.querySelectorAll<HTMLLIElement>(".dev")];
      rows[rows.indexOf(li) + (e.key === "ArrowDown" ? 1 : -1)]?.focus();
    }
  });
  return li;
}

/** The sidebar from `devices`, filtered by the search field, with the selection marked. */
function renderList(): void {
  const q = search.value.trim().toLowerCase();
  const match = (d: DeviceInfo) =>
    !q || [d.name, d.model_name, d.product_type, d.os_version, d.udid].some((s) => s?.toLowerCase().includes(q));
  const shown = devices.filter(match);
  // The selected device stays listed while it is unplugged, so its view doesn't lose its row;
  // under Unavailable once it has left the list.
  const extra = selected && !devices.some((d) => d.udid === selected!.udid) && match(selected) ? selected : undefined;
  const ok = shown.filter((d) => !unusable(d));
  const bad = shown.filter(unusable);
  if (extra && !gone && !unusable(extra)) ok.unshift(extra);
  else if (extra) bad.unshift(extra);
  availableList.replaceChildren(...ok.map((d) => row(d)));
  unavailableList.replaceChildren(...bad.map((d) => row(d, d === extra && !!gone)));
  $<HTMLElement>("#sec-available").hidden = !ok.length;
  $<HTMLElement>("#sec-unavailable").hidden = !bad.length;
  markSelection();
  let m: { title: string; detail: string; cls?: string } | null = null;
  if (listError) m = { title: t("list.error"), detail: listError, cls: "error" };
  else if (!listed) m = { title: t("list.looking"), detail: "", cls: "loading" };
  // A row of the selected device that left the list is enough to drop the empty state.
  else if (!devices.length && !extra) m = { title: t("list.empty"), detail: t("list.emptyDetail") };
  else if (!ok.length && !bad.length) m = { title: t("list.noMatches"), detail: t("list.noMatchesDetail", { query: search.value.trim() }) };
  listMessage.hidden = !m;
  if (m) {
    listMessage.className = `list-message ${m.cls ?? ""}`;
    listMessage.querySelector(".title")!.textContent = m.title;
    listMessage.querySelector(".detail")!.textContent = m.detail;
  }
}

function markSelection(): void {
  const busy = !!activeSession();
  for (const li of document.querySelectorAll<HTMLLIElement>(".dev")) {
    const on = li.dataset.udid === selected?.udid;
    li.setAttribute("aria-selected", String(on));
    li.querySelector(".busy")?.remove();
    if (on && busy) {
      // A blue dot at the row's end, like an unread mark.
      const dot = el("span", "busy");
      dot.title = t("list.sessionRuns");
      li.append(dot);
    }
  }
}

/** Whether the device list has come back once. */
let listed = false;
let loading = false;

async function loadDevices(): Promise<void> {
  if (loading) return;
  loading = true;
  refreshBtn.disabled = true;
  const before = devices;
  try {
    devices = await invoke<DeviceInfo[]>("list_devices");
    listError = undefined;
  } catch (e) {
    devices = [];
    listError = String(e);
  } finally {
    listed = true;
    loading = false;
    refreshBtn.disabled = false;
  }
  // The selected device's details can change (unlocked, trusted, a transport came or went).
  const now = selected && devices.find((d) => d.udid === selected!.udid);
  if (selected && !now && !listError && before.some((d) => d.udid === selected!.udid)) {
    // Only a device the list had: one opened by UDID before the list showed it isn't "gone".
    onSelectedGone();
  } else if (now && gone) {
    onSelectedBack(now);
  } else if (now && selected) {
    const wasError = unusable(selected);
    const wasSetup = !!selected.setup;
    selected = now;
    showDeviceText();
    if (now.setup && !setupRun) showSetup(now);
    if ((wasError || wasSetup) && !unusable(now) && !now.setup && !setupRun) {
      // It became usable: attach now, as if just picked.
      const d = now;
      selected = undefined;
      void select(d);
    }
  }
  renderList();
}

/** The selected device left the list: let go of it (the stream and the daemon's attachment end
 * with the device anyway) but keep it selected, and say what to do. A setup step that is running
 * keeps running: Developer Mode's waits through the restart. */
function onSelectedGone(): void {
  if (gone || !selected) return;
  gone = { view: wantView };
  if (!setupRun) void detach();
  settingsPane.classList.add("disabled");
  settingsMessage(() => t("settings.afterReconnect"));
  stageState("disconnected", () => t("stage.disconnected"));
  showDeviceText();
  if (setupFor) renderSetup();
}

/** The same UDID is back: go on where it left off, at its setup step or with its screen. */
function onSelectedBack(now: DeviceInfo): void {
  const was = gone!;
  gone = undefined;
  if (setupRun && setupFor?.udid === now.udid) {
    // The running step brings the card up to date when it ends.
    selected = now;
    setupFor = now;
    settingsMessage(() => t("settings.afterSetup"));
    stageState("setup");
    showDeviceText();
    renderSetup();
    return;
  }
  selected = undefined;
  void select(now, { view: was.view });
}

search.addEventListener("input", renderList);
search.addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown") {
    e.preventDefault();
    document.querySelector<HTMLLIElement>(".dev")?.focus();
  } else if (e.key === "Escape") {
    search.value = "";
    renderList();
  }
});
refreshBtn.addEventListener("click", () => void loadDevices());
// Devices come and go (a cable, Wi-Fi): the list keeps itself current while the window shows.
window.setInterval(() => {
  if (!document.hidden) void loadDevices();
}, 8000);

// ---------- the selected device ----------

const stage = $<HTMLDivElement>("#stage");
const scroller = $<HTMLDivElement>("#scroller");
const deviceEl = $<HTMLDivElement>("#device");
const canvas = $<HTMLCanvasElement>("#screen");
const overlay = $<HTMLDivElement>("#overlay");
const screenBox = $<HTMLDivElement>("#screen-box");
const glow = $<HTMLDivElement>("#glow");
const ctx = canvas.getContext("2d")!;
const introArt = $<HTMLDivElement>("#intro-art");
const viewBtn = $<HTMLButtonElement>("#view-screen");

/** What the stage shows: nothing selected; the selected device with View Screen; its setup;
 * pairing a new device over Wi-Fi; one that can't be used; one that left the list (`gone`);
 * connecting; an error; or the live screen once a frame drew. */
type StageState = "none" | "intro" | "setup" | "pair" | "unavailable" | "disconnected" | "connecting" | "error" | "live";

/** What the stage says, kept to say it again in another language. */
let stageText: () => string = () => "";

/** `text`: the reason or progress to show, written in the current language when it is a function. */
function stageState(state: StageState, text: string | (() => string) = ""): void {
  stageText = typeof text === "function" ? text : () => text;
  stage.dataset.state = state;
  center.classList.toggle("live", state === "live");
  showStageText();
  updateMenu();
  if (state === "live") layout();
  syncKeyboard();
}

function showStageText(): void {
  const state = stage.dataset.state;
  const text = stageText();
  $<HTMLSpanElement>("#intro-busy-text").textContent = state === "connecting" ? text || t("common.connecting") : "";
  $<HTMLParagraphElement>("#intro-reason").textContent = state === "error" || state === "unavailable" || state === "disconnected" ? text : "";
}

function showDeviceText(): void {
  const d = selected;
  const name = d ? deviceName(d) : "";
  const meta = d ? [d.name ? modelName(d) : undefined, osLabel(d)].filter(Boolean).join(" · ") : "";
  $<HTMLSpanElement>("#device-name").textContent = d ? name : pairing ? t("device.new") : t("device.none");
  $<HTMLSpanElement>("#device-meta").textContent = d
    ? gone ? t("device.disconnected") : meta
    : pairing ? t("device.pairingWifi") : listed && !devices.length ? t("device.connectToBegin") : t("device.choose");
  $<HTMLHeadingElement>("#intro-name").textContent = name;
  $<HTMLParagraphElement>("#intro-meta").textContent = d ? ([d.name ? modelName(d) : undefined, osLabel(d)].filter(Boolean).join(" · ")) : "";
  introArt.classList.toggle("tablet", isTablet(d));
  $("#device-glyph use").setAttribute("href", isTablet(d) ? "#i-tablet" : "#i-phone");
  introArt.classList.toggle("unavailable", unusable(d) || !!gone);
  showInfo();
}

let config: Config | undefined;
let decoder: VideoDecoder | undefined;
let waitingForKey = true;
let control: ControlEvent = { session: null, human_touching: false, agent_stopped: false };
/** Labels shown so far, for the report. */
let labelsShown = 0;

/** Decoder numbers, shown in the inspector and reported to the backend's log. */
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
  /** `orientation` events received. */
  orientations: 0,
  /** The first decoded frame's geometry after each config, and the part of it drawn. */
  frame: undefined as { coded: number[]; visible: number[]; display: number[]; drawn: number[] } | undefined,
};
const received = new Map<number, number>();
/** Frames that arrive while the decoder is being set up, from the last keyframe on: the device
 * sends one keyframe per stream, so dropping it means nothing decodes. */
let pending: { header: { keyframe: boolean; pts_us: number }; data: Uint8Array }[] = [];
/** Bumped by every `config` and by leaving the screen, so a setup that finishes late gives way. */
let configGen = 0;
/** Codec and parameter sets the current decoder was set up with. */
let decoderKey = "";

function send(cmd: string, args: unknown = {}): Promise<unknown> {
  return invoke("send", { cmd, args }).catch((e) => {
    showNote(String(e), { error: true });
    throw e;
  });
}

function b64(s: string): Uint8Array {
  return Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
}

const quarterTurn = (r: Rotation | undefined) => r === "rot90" || r === "rot270";

/** Sizes the canvas to the screen as the user sees it: the panel's pixels, turned. */
function sizeCanvas(): void {
  if (!config) return;
  const [pw, ph] = config.display_px;
  const [w, h] = quarterTurn(config.rotation) ? [ph, pw] : [pw, ph];
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
}

/** Maps the panel's pixels (the picture) onto the canvas (the screen as the user sees it), as a
 * canvas transform. The inverse of `Rotation::turn_share` in `crates/core/src/display.rs`. */
function panelToCanvas(r: Rotation | undefined, pw: number, ph: number): [number, number, number, number, number, number] {
  switch (r) {
    case "rot90":
      return [0, 1, -1, 0, ph, 0];
    case "rot180":
      return [-1, 0, 0, -1, pw, ph];
    case "rot270":
      return [0, -1, 1, 0, 0, pw];
    default:
      return [1, 0, 0, 1, 0, 0];
  }
}

/** The last picture, kept to draw again when the screen turns between frames: the device sends
 * none while the screen is still. */
let lastFrame: VideoFrame | undefined;

function drawFrame(frame: VideoFrame): void {
  if (!config) return;
  // The screen is the top-left part of the coded picture; the rest is the encoder's padding
  // to its block size, black. WebKit ignores the source rectangle when it draws a VideoFrame
  // and squeezes the whole picture, padding included, into the destination: a black band
  // at the right and bottom (PITFALLS #26). So the whole frame is drawn scaled up so that
  // the panel fills the canvas, and the canvas clips the padding off. The picture is the panel
  // in its native orientation whichever way the interface is turned (PITFALLS #27): the
  // transform turns it upright, and the padding then lies outside the canvas on every side.
  const [pw, ph] = config.display_px;
  // A visible rect smaller than the screen it holds is the decoder's mistake, not a crop
  // (PITFALLS #57): draw the screen from the coded picture instead.
  const v = frame.visibleRect;
  const cut =
    v && (v.width < pw || v.height < ph) && frame.codedWidth >= pw && frame.codedHeight >= ph
      ? new VideoFrame(frame, { visibleRect: { x: 0, y: 0, width: pw, height: ph }, displayWidth: pw, displayHeight: ph })
      : undefined;
  const src = cut ?? frame;
  // On the decoder's own frame first, so viewer.log shows what the decoder gave.
  let [sx, sy, sw, sh] = screenRect(frame);
  if (cut) [sx, sy, sw, sh] = screenRect(cut);
  const kx = pw / sw;
  const ky = ph / sh;
  ctx.setTransform(...panelToCanvas(config.rotation, pw, ph));
  ctx.drawImage(src, -sx * kx, -sy * ky, src.displayWidth * kx, src.displayHeight * ky);
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  cut?.close();
}

/** The screen turned: the canvas takes the new shape and the last picture is drawn again turned,
 * scaled to fit the window as it is (the window itself never changes size, PITFALLS #29). */
function onOrientation(o: Orientation): void {
  stats.orientations++;
  if (!config) return;
  const turned = config.rotation !== o.rotation;
  config = { ...config, rotation: o.rotation, orientation: o.orientation, screen: o.screen };
  if (!turned) return;
  sizeCanvas();
  if (lastFrame) drawFrame(lastFrame);
  // Points of the old orientation mean nothing on the new screen.
  hideCursor();
  for (const d of fingers.values()) d.remove();
  fingers.clear();
  layout();
  // Reported at once too: the timer that reports stops while the window is hidden.
  window.setTimeout(report, 300);
}

/** A foldable was folded or unfolded and another panel is in use (PITFALLS #47). The picture and
 * every point shown belong to the old panel: the screen keeps the last picture but takes no
 * touches until the daemon's stream on the new panel sends its `config` (a new size, decoder
 * and screen), which `configure` sets up as on any stream start. */
function onPanel(): void {
  config = undefined;
  hideCursor();
  for (const d of fingers.values()) d.remove();
  fingers.clear();
}

async function configure(cfg: Config): Promise<void> {
  const gen = ++configGen;
  config = cfg;
  sizeCanvas();
  stats.frame = undefined;
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
    // Without the size, WebView2 (Chromium) gave every frame a 1280x720 visible rect and display
    // size, a corner of the real picture (PITFALLS #57).
    codedWidth: cfg.coded[0],
    codedHeight: cfg.coded[1],
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
      drawFrame(frame);
      lastFrame?.close();
      lastFrame = frame;
      if (stage.dataset.state !== "live" && wantView) stageState("live");
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

/** Where the screen (`display_px`, from the coded picture's top-left corner) is in a decoded
 * frame, in the frame's display size, which `drawImage` draws and which covers its visible rect
 * (the decoder may keep the padding in it, as WebKit's does, or scale the display size). */
function screenRect(frame: VideoFrame): [number, number, number, number] {
  const [pw, ph] = config?.display_px ?? [canvas.width, canvas.height];
  const v = frame.visibleRect;
  if (!v || !v.width || !v.height) return [0, 0, pw, ph];
  const kx = frame.displayWidth / v.width;
  const ky = frame.displayHeight / v.height;
  const x0 = Math.max(0, v.x);
  const y0 = Math.max(0, v.y);
  const x1 = Math.min(pw, v.x + v.width);
  const y1 = Math.min(ph, v.y + v.height);
  const rect: [number, number, number, number] = [(x0 - v.x) * kx, (y0 - v.y) * ky, (x1 - x0) * kx, (y1 - y0) * ky];
  if (!stats.frame) {
    stats.frame = {
      coded: [frame.codedWidth, frame.codedHeight],
      visible: [v.x, v.y, v.width, v.height],
      display: [frame.displayWidth, frame.displayHeight],
      drawn: rect.map((n) => Math.round(n * 100) / 100),
    };
  }
  return rect;
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

// ---------- agent session ----------

const sessionIdle = $<HTMLParagraphElement>("#session-idle");
const sessionPanel = $<HTMLDivElement>("#session-panel");
const sessionName = $<HTMLSpanElement>("#session-name");
const sessionState = $<HTMLSpanElement>("#session-state");
const sessionPrimary = $<HTMLButtonElement>("#session-primary");
const sessionStop = $<HTMLButtonElement>("#session-stop");

const activeSession = () => (control.session && control.session.owner !== "finished" ? control.session : null);

/** What the panel says and does for each owner: status text, the main button and its action. */
function panelFor(s: AgentSession): { state: string; button: string; action: string } {
  switch (s.owner) {
    case "delegated_to_user":
      // The reason is the agent's own words, in whatever language it wrote them.
      return { state: t("session.waitingForYou", { reason: s.reason ?? "" }), button: t("session.done"), action: "done" };
    case "user":
      return { state: t("session.youInControl"), button: t("session.handBack"), action: "hand_back" };
    default:
      return { state: t("session.agentInControl"), button: t("session.takeOver"), action: "take_over" };
  }
}

/** The session's glow and panel stay up for as long as the session runs, not only while input
 * arrives, so the person watching always knows an agent holds the device. A session that starts
 * while the screen isn't shown shows it: the person should see what the agent does. */
function onControl(c: ControlEvent): void {
  const was = activeSession()?.id ?? null;
  control = c;
  const s = activeSession();
  // The cursor marks where this session's agent works; it goes with the session. Input sent
  // without a session keeps it until the device changes.
  if (was && s?.id !== was) hideCursor();
  sessionPanel.hidden = !s;
  sessionIdle.hidden = !!s;
  glow.hidden = !s;
  markSelection();
  syncKeyboard();
  syncTouchBlock();
  if (!s) return;
  if (s.id !== was && !wantView && attachedUdid) void view();
  const human = s.owner !== "agent";
  glow.classList.toggle("human", human);
  sessionPanel.classList.toggle("human", human);
  const p = panelFor(s);
  sessionName.textContent = s.name;
  sessionState.textContent = p.state;
  sessionState.title = p.state;
  sessionPrimary.textContent = p.button;
  sessionPrimary.dataset.action = p.action;
}

sessionPrimary.addEventListener("click", () => {
  if (sessionPrimary.dataset.action) void send("control", { action: sessionPrimary.dataset.action });
});
sessionStop.addEventListener("click", () => void send("control", { action: "stop" }));

/** Where a point of the screen (in points) is on the overlay, in CSS pixels. */
function toOverlay(p: [number, number], screen: [number, number]): [number, number] {
  return [(p[0] / screen[0]) * overlay.clientWidth, (p[1] / screen[1]) * overlay.clientHeight];
}

/** Whether a label that starts `offset` px right of overlay point `x` stays inside the stage. The
 * window keeps whatever size the user gave it (PITFALLS #29), so in a narrow one a label beside a
 * point near the right edge would be cut off: it goes on the left of the point instead. */
function fitsRight(label: HTMLElement, x: number, offset: number): boolean {
  return overlay.getBoundingClientRect().left + x + offset + label.offsetWidth <= stage.getBoundingClientRect().right - 8;
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
  if (at && !fitsRight(b, x, 14)) b.classList.add("left");
  labelsShown++;
  b.addEventListener("animationend", () => b.remove());
}

// ---------- touch indicators ----------
//
// A finger on the glass shows as a translucent white disc the size of a fingertip (56 points, so
// it scales with the screen), as iOS touch indicators in screen recordings look. Every touch has
// two sequences: the start, where the disc appears at once where the finger lands, and the end,
// where it grows a little and fades where the finger lifts. A tap plays both in place; a drag
// plays the start, follows the finger while it is held, and plays the end only at the release
// point, so nothing at the start of a drag looks like a release.

/** How long the end sequence takes. */
const RELEASE_MS = 350;
/** How long a tap holds the finger down when the event doesn't say (`TAP_HOLD` in the core). */
const TAP_HOLD_MS = 50;

// "Show Touches" (View menu, ⇧⌘T; app/src-tauri/src/view_menu.rs) turns the discs off, the
// human's and the agent's alike. The agent cursor and labels stay: they say where the agent is
// working, which the discs only repeat.
let showTouches = prefs.get("show-touches") !== "0";

function setShowTouches(on: boolean): void {
  showTouches = on;
  prefs.set("show-touches", on ? "1" : "0");
  if (on) return;
  for (const d of overlay.querySelectorAll(".touch-dot")) d.remove();
  fingers.clear();
}

void listen<boolean>("show-touches", (e) => setShowTouches(e.payload));
// The menu starts checked; make it match what was saved.
void invoke("set_show_touches", { on: showTouches }).catch(() => {});

/** A disc at `at`. With touches hidden it is never put on the overlay, so callers animate and
 * remove it as usual and nothing shows. */
function touchDot(at: [number, number]): HTMLDivElement {
  const d = document.createElement("div");
  d.className = "touch-dot";
  d.style.left = `${at[0]}px`;
  d.style.top = `${at[1]}px`;
  if (showTouches) overlay.append(d);
  return d;
}

/** The end sequence: the disc grows as it fades, then goes. */
function releaseDot(d: HTMLDivElement): void {
  d.classList.add("released");
  d.animate(
    [
      { opacity: 1, transform: "scale(1)" },
      { opacity: 0, transform: "scale(1.4)" },
    ],
    { duration: RELEASE_MS, easing: "ease-out", fill: "forwards" },
  );
  // A timer, not the animation's finish: WebKit holds animations while the window is hidden.
  window.setTimeout(() => d.remove(), RELEASE_MS + 30);
}

/** The disc of a live touch on the overlay, by source, while the finger is down. */
const fingers = new Map<string, HTMLDivElement>();

function onTouch(e: InputEvent): void {
  const p = e.points[0];
  const dot = fingers.get(e.source);
  if (!p || !e.screen[0]) {
    // Lifted by the daemon without a point: end where the disc is.
    if (dot) releaseDot(dot);
    fingers.delete(e.source);
    return;
  }
  const at = toOverlay(p, e.screen);
  if (e.phase === "down" || !dot) {
    dot?.remove();
    fingers.set(e.source, touchDot(at));
    return;
  }
  dot.style.left = `${at[0]}px`;
  dot.style.top = `${at[1]}px`;
  if (e.phase === "up") {
    releaseDot(dot);
    fingers.delete(e.source);
  }
}

// ---------- agent cursor ----------
//
// The agent's touches show as a mouse-style arrow that stays where the agent last touched, so the
// person watching can tell where it is working between inputs. It glides from one touch to the
// next and along swipes; a ring under the tip shows while the finger is down. A tap is the
// exception: its disc is drawn where the finger lands while the cursor glides there, because the
// daemon sends the tap as the finger goes down and lifts it `duration_ms` (50 ms) later, well
// before a glide would arrive (PITFALLS #34). Only agent input moves the cursor: the human's own
// pointer is visible already, and their touches keep the dot.

const AGENT_COLOR = "#ff9f0a";
const HUMAN_COLOR = "#0a84ff";
/** How long the cursor takes to reach a new touch before pressing there. Short: the input event
 * arrives as the daemon sends the touch, so a long glide would trail the device. */
const GLIDE_MS = 160;
/** Glide between the steps of a live agent touch, which come at the agent's pace. */
const STEP_GLIDE_MS = 120;

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

/** Puts the finger down at the tip (the start sequence) or lifts it (the end sequence). */
function setPressed(on: boolean): void {
  cursor.classList.toggle("pressing", on);
}

/** Lifts the finger at the tip now, playing the end sequence even if the press never showed
 * (an `up` that came before the glide to its `down` finished). */
function liftCursor(): void {
  if (!cursor.classList.contains("pressing")) {
    setPressed(true);
    void cursor.offsetWidth; // Start the transition from pressed, not from nothing.
  }
  setPressed(false);
}

/** A tap's disc where the finger lands, ended as the finger lifts, `ms` after it came down. */
function tapDot(at: [number, number], ms: number | undefined): void {
  const d = touchDot(at);
  window.setTimeout(() => releaseDot(d), ms ?? TAP_HOLD_MS);
}

/** Puts the label next to the cursor, replacing the one before. It fades after a few seconds;
 * the cursor stays. */
function labelCursor(text: string, at: [number, number], screen: [number, number]): void {
  cursorLabel?.remove();
  const b = document.createElement("div");
  b.className = "label-bubble at-cursor";
  b.textContent = text;
  b.style.setProperty("--c", AGENT_COLOR);
  cursor.append(b);
  // The first input's label comes before the cursor shows, and a hidden one measures no width.
  const hidden = cursor.hidden;
  cursor.hidden = false;
  if (!fitsRight(b, toOverlay(at, screen)[0], 16)) b.classList.add("left");
  cursor.hidden = hidden;
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
  if (e.label) labelCursor(e.label, p, screen);
  if (e.kind === "tap") {
    tapDot(toOverlay(p, screen), e.duration_ms);
    void moveCursor([p], screen, GLIDE_MS);
  } else if (e.kind === "swipe" && q) {
    void moveCursor([p], screen, GLIDE_MS).then(() => {
      if (cursorAt?.p !== p) return;
      setPressed(true);
      return moveCursor([q], screen, e.duration_ms ?? 300, "ease-in-out");
    }).then(() => {
      if (cursorAt?.p !== q) return; // Overtaken by a newer input.
      setPressed(false);
    });
  } else if (e.kind === "touch") {
    if (e.phase === "down") {
      void moveCursor([p], screen, GLIDE_MS).then(() => {
        if (cursorAt?.p !== p) return;
        setPressed(true);
      });
    } else if (e.phase === "move") {
      setPressed(true);
      void moveCursor([p], screen, STEP_GLIDE_MS, "linear");
    } else {
      // The daemon sends the up once the finger has lifted: end now, and let the cursor glide on.
      liftCursor();
      void moveCursor([p], screen, STEP_GLIDE_MS, "linear");
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
  if (e.label) {
    const p = e.points[0];
    showLabel(e.label, p && e.screen[0] ? toOverlay(p, e.screen) : undefined, HUMAN_COLOR);
  }
  if (e.kind === "touch") {
    onTouch(e);
  } else if (e.kind === "tap" && e.points[0]) {
    tapDot(toOverlay(e.points[0], e.screen), e.duration_ms);
  } else if (e.kind === "swipe" && e.points.length === 2) {
    const [a, b] = [toOverlay(e.points[0], e.screen), toOverlay(e.points[1], e.screen)];
    const d = touchDot(b);
    const ms = e.duration_ms ?? 300;
    d.animate(
      [
        { left: `${a[0]}px`, top: `${a[1]}px` },
        { left: `${b[0]}px`, top: `${b[1]}px` },
      ],
      { duration: ms, easing: "ease-in-out" },
    );
    window.setTimeout(() => releaseDot(d), ms);
  }
}

/** Parses one wire message (see `devicesteer_daemon::wire`). */
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
      case "orientation":
        onOrientation(ev as Orientation);
        break;
      case "panel":
        onPanel();
        break;
      case "recording":
        onRecording(ev as RecordingEvent);
        break;
      case "keyboard":
        onKeyboard(ev as KeyboardState);
        break;
    }
  } else if (kind === 1) {
    const hl = view.getUint32(5);
    const header = JSON.parse(new TextDecoder().decode(payload.subarray(4, 4 + hl)));
    if (wantView) onFrame(header, payload.subarray(4 + hl));
  }
}

// ---------- the device frame and zoom ----------
//
// The screen is drawn as it is: a plain rectangle with no hardware around it (style.css, #bezel,
// says why the corners are square). Hardware buttons are
// real controls in the device bar, not pictures. The zoom is the size of one screen point in CSS
// pixels: "fit" works it out from the room the stage has; "actual size" is one point per CSS pixel.

type Zoom = "fit" | number;
let zoom: Zoom = "fit";
/** The scale the last layout used, to zoom in and out from. */
let scale = 1;
const ZOOM_MIN = 0.2;
const ZOOM_MAX = 3;

/** The kind of screen. `pad` and `rim` stay for the layout: the screen has no frame, so they are
 * zero. */
function frameFor(w: number, h: number) {
  const m = Math.min(w, h);
  const long = Math.max(w, h) / m;
  const kind = isTablet(selected) || long < 1.6 ? "tablet" : long < 1.9 ? "classic" : "modern";
  const portrait = h >= w;
  return { kind, portrait, pad: [0, 0, 0, 0], rim: 0 };
}

/** Sizes the device to the zoom and keeps the overlay on the screen. */
function layout(): void {
  if (!config) return;
  const [w, h] = config.screen;
  const f = frameFor(w, h);
  const [t, r, b, l] = f.pad;
  const outerW = w + l + r + 2 * f.rim;
  const outerH = h + t + b + 2 * f.rim;
  const cs = getComputedStyle(scroller);
  const availW = scroller.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight);
  const availH = scroller.clientHeight - parseFloat(cs.paddingTop) - parseFloat(cs.paddingBottom);
  const fit = Math.max(0.05, Math.min(availW / outerW, availH / outerH, ZOOM_MAX));
  scale = zoom === "fit" ? fit : zoom;
  const s = scale;
  deviceEl.dataset.kind = f.kind;
  deviceEl.dataset.orient = f.portrait ? "portrait" : "landscape";
  const px = (n: number) => `${n * s}px`;
  deviceEl.style.width = px(w + l + r);
  deviceEl.style.height = px(h + t + b);
  const set = (k: string, v: string) => deviceEl.style.setProperty(k, v);
  set("--rim", `${Math.max(1.5, f.rim * s)}px`);
  // Touch discs are a fingertip on the screen (56 points).
  overlay.style.setProperty("--touch", `${Math.max(20, 56 * s)}px`);
  placeCursor();
  showZoom();
}
new ResizeObserver(() => layout()).observe(scroller);

function setZoom(z: Zoom): void {
  if (!config) return;
  zoom = typeof z === "number" ? Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, z)) : z;
  layout();
  // Zoom about the middle of what shows.
  scroller.scrollLeft = (scroller.scrollWidth - scroller.clientWidth) / 2;
  scroller.scrollTop = (scroller.scrollHeight - scroller.clientHeight) / 2;
}

function showZoom(): void {
  $<HTMLButtonElement>("#zoom-fit").setAttribute("aria-pressed", String(zoom === "fit"));
  $<HTMLButtonElement>("#zoom-actual").setAttribute("aria-pressed", String(zoom !== "fit" && Math.abs(scale - 1) < 0.001));
  $<HTMLButtonElement>("#zoom-in").disabled = scale >= ZOOM_MAX - 0.001;
  $<HTMLButtonElement>("#zoom-out").disabled = scale <= ZOOM_MIN + 0.001;
  stage.dataset.zoom = zoom === "fit" ? "fit" : String(Math.round(scale * 100));
}

$<HTMLButtonElement>("#zoom-in").addEventListener("click", () => setZoom(scale * 1.25));
$<HTMLButtonElement>("#zoom-out").addEventListener("click", () => setZoom(scale / 1.25));
$<HTMLButtonElement>("#zoom-fit").addEventListener("click", () => setZoom("fit"));
$<HTMLButtonElement>("#zoom-actual").addEventListener("click", () => setZoom(1));
window.addEventListener("keydown", (e) => {
  if (!(e.metaKey || e.ctrlKey) || stage.dataset.state !== "live") return;
  const z: Record<string, () => void> = {
    "=": () => setZoom(scale * 1.25),
    "+": () => setZoom(scale * 1.25),
    "-": () => setZoom(scale / 1.25),
    "0": () => setZoom(1),
    "9": () => setZoom("fit"),
  };
  if (z[e.key]) {
    e.preventDefault();
    e.stopImmediatePropagation();
    z[e.key]();
  }
});

// ---------- human input ----------
//
// Human input is a live touch: down on pointerdown, the pointer's path as it moves (one step per
// pointer event, with the event's time), up on release. The daemon replays the path at a steady
// rate from those times (PITFALLS #18), so steps need not go out evenly. They go out one at a
// time and in order: an `up` that overtook the last `move` would leave the gesture short, and a
// backlog of moves would make the finger trail the mouse. While a step is in flight, newer moves
// replace the queued one.

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
  if (!humanHasDevice()) {
    touchBlocked(e);
    return;
  }
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

// While the agent holds the device, the screen takes no touches: one under the agent's input
// would turn its taps into something else, and the daemon refuses them anyway (check_human). A
// press says why, next to the pointer, and points at Take over. A press already down when the
// agent gets the device ends there; the daemon lifts that finger too.
const blockedHint = document.createElement("div");
blockedHint.className = "blocked-hint";
blockedHint.setAttribute("role", "status");
blockedHint.textContent = t("session.blockedHint");
blockedHint.hidden = true;
overlay.append(blockedHint);
let blockedHintTimer: number | undefined;
/** For the report: presses the viewer kept from the device. */
let touchesBlocked = 0;

function syncTouchBlock(): void {
  const blocked = !humanHasDevice();
  screenBox.classList.toggle("agent-has-device", blocked);
  if (!blocked) {
    hideBlockedHint();
    return;
  }
  endPress();
}

function hideBlockedHint(): void {
  window.clearTimeout(blockedHintTimer);
  blockedHint.hidden = true;
  sessionPrimary.classList.remove("nudge");
}

function touchBlocked(e: PointerEvent): void {
  touchesBlocked++;
  const r = overlay.getBoundingClientRect();
  const x = e.clientX - r.left;
  const y = e.clientY - r.top;
  blockedHint.hidden = false;
  blockedHint.style.left = `${x}px`;
  blockedHint.style.top = `${y}px`;
  blockedHint.classList.toggle("left", !fitsRight(blockedHint, x, 14));
  // Restart the fade and the nudge for every press, not only the first.
  for (const el of [blockedHint, sessionPrimary]) {
    el.classList.remove("nudge");
    void el.offsetWidth;
    el.classList.add("nudge");
  }
  window.clearTimeout(blockedHintTimer);
  blockedHintTimer = window.setTimeout(hideBlockedHint, BLOCKED_HINT_MS);
}
const BLOCKED_HINT_MS = 2400;

for (const b of document.querySelectorAll<HTMLButtonElement>("#device-bar [data-button]"))
  b.addEventListener("click", () => void send("button", { name: b.dataset.button }).catch(() => {}));

// ---------- the Mac keyboard ----------
//
// With "Type from Mac Keyboard" on, keys pressed while the pointer is over the screen (or the
// screen has focus) go to the device as HID key downs and ups, by DOM `code`: the physical key,
// so the device's layout and input source decide what it types (PITFALLS #35). Never while a
// field of the viewer has focus, and never the viewer's own shortcuts. Text composed with a Mac
// input method (Hangul) goes over with the shared clipboard (or "Send Clipboard") and a paste on
// the device.
//
// Turning it on attaches a hardware keyboard too: without one, the keys go through the
// keyboard surface, whose input source is stuck (PITFALLS #35), and Control-Space can't switch
// it. The viewer asks for it `while_connected`, so the daemon removes it when the viewer goes
// away or an agent gets the device; the viewer removes it when forwarding stops or the screen
// isn't live. Either way the on-screen keyboard comes back (PITFALLS #36).

type KeyboardState = { hardware: boolean; attached: boolean };

const kbButton = $<HTMLButtonElement>("#keyboard");
const kbMenu = $<HTMLDivElement>("#keyboard-menu");
const kbForward = $<HTMLButtonElement>("#kb-forward");
const kbSoftware = $<HTMLButtonElement>("#kb-software");
menu(kbButton, kbMenu);

let forwarding = prefs.get("keyboard-forwarding") === "1";
/** Whether a hardware keyboard is attached (anyone's), as the daemon last said. */
let hardwareKeyboard = false;
/** The attached keyboard is the one this viewer asked for. */
let kbOurs = false;
/** A connect or disconnect is on its way. */
let kbBusy = false;
let overScreen = false;
/** Keys sent down, by code, so every one is let go again. */
const keysDown = new Set<string>();
/** Key steps go out one at a time and in order: an up that overtook its down would leave the key held. */
let keyChain: Promise<unknown> = Promise.resolve();

function keyStep(code: string, down: boolean): void {
  keyChain = keyChain
    .then(() => invoke("send", { cmd: "keyboard", args: { code, down } }))
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

// Control-Space switched the Mac's input source, as the Mac's "Select the previous input
// source" shortcut: the page saw only Control (PITFALLS #46). Give the device its Space and put
// the Mac's input source back, so only the device switches. Caps Lock switching the Mac's input
// source (PITFALLS #45) has its own path: its keydown clears `ctrlAlone`.
void listen("input-source-changed", () => {
  if (!keysGo() || !ctrlAlone) return;
  if ([...keysDown].some((c) => c.startsWith("Control"))) keyTap("Space");
  else if (performance.now() - ctrlUpAt < CTRL_LATE_MS) {
    keyStep("ControlLeft", true);
    keyTap("Space");
    keyStep("ControlLeft", false);
  } else return;
  void invoke("restore_input_source").catch(() => {});
});

function releaseKeys(): void {
  for (const code of keysDown) keyStep(code, false);
  keysDown.clear();
}

/** The human may send input: no agent session, or one that gave the device to the human. */
const humanHasDevice = (): boolean => activeSession()?.owner !== "agent";

// "Use Shared Clipboard": the Tauri shell keeps the Mac and device clipboards in step (see
// app/src-tauri/src/shared_clip.rs) while it is on, the screen is live and no agent holds the
// device; the page only tells it when that changes. The text and what was synced last stay in
// the shell. On by default, like Xcode's Device Hub's.
const clipShared = $<HTMLButtonElement>("#kb-clip-shared");
let sharedClipboard = prefs.get("shared-clipboard") !== "0";
/** What the shell was last told: syncing or not (null: nothing yet). */
let clipSyncing: boolean | null = null;

function syncSharedClipboard(): void {
  clipShared.setAttribute("aria-checked", String(sharedClipboard));
  const want = sharedClipboard && !!attachedUdid && stage.dataset.state === "live" && humanHasDevice();
  if (want === clipSyncing) return;
  clipSyncing = want;
  invoke("clipboard_sync", { on: want })
    .then((supported) => {
      if (supported !== false) return;
      // No way to see password managers' private items on this platform (shared_clip.rs).
      clipShared.disabled = true;
      clipShared.title = t("keyboard.sharedMacOnly");
    })
    .catch((e) => {
      stats.lastError = String(e);
    });
}

/** Forwarding is on and can work now: the screen is live and no agent holds the device. */
const forwardingActive = (): boolean => forwarding && stage.dataset.state === "live" && humanHasDevice();

function showKeyboardState(): void {
  // on: keys go, with the hardware keyboard attached; connecting: it is being attached;
  // paused: on, but an agent has the device; off.
  const state = !forwarding
    ? "off"
    : !humanHasDevice()
      ? "paused"
      : hardwareKeyboard
        ? "on"
        : "connecting";
  kbButton.dataset.forwarding = state;
  kbButton.title = t(({ on: "keyboard.stateOn", connecting: "keyboard.stateConnecting", paused: "keyboard.statePaused", off: "keyboard.label" } as const)[state]);
  kbButton.setAttribute("aria-label", kbButton.title);
  kbForward.setAttribute("aria-checked", String(forwarding));
  kbSoftware.disabled = !hardwareKeyboard;
}

/** Attaches the hardware keyboard while forwarding is active, and removes the one this viewer
 * attached when it isn't. Called whenever any of that changes; the daemon's `keyboard` event
 * brings the result back here. */
function syncKeyboard(): void {
  syncSharedClipboard();
  showKeyboardState();
  if (kbBusy || !attachedUdid) return;
  const want = forwardingActive();
  let cmd: string;
  if (want && !hardwareKeyboard) cmd = "keyboard_connect";
  else if (!want && kbOurs && hardwareKeyboard) cmd = "keyboard_disconnect";
  else return;
  kbBusy = true;
  releaseKeys();
  const udid = attachedUdid;
  void invoke("send", { cmd, args: cmd === "keyboard_connect" ? { while_connected: true } : {} })
    .then((r) => {
      if (udid !== attachedUdid) return;
      kbOurs = cmd === "keyboard_connect";
      kbBusy = false;
      onKeyboard(r as KeyboardState);
    })
    .catch((e) => {
      kbBusy = false;
      if (udid !== attachedUdid) return;
      // Not again until something changes: a connect that fails would fail in a loop.
      if (cmd === "keyboard_connect") {
        forwarding = false;
        showKeyboardState();
        showNote(t("keyboard.connectFailed", { error: errorText(e) }), { error: true });
      }
    });
}

function setForwarding(on: boolean): void {
  forwarding = on;
  prefs.set("keyboard-forwarding", on ? "1" : "0");
  if (!on) releaseKeys();
  syncKeyboard();
}

function onKeyboard(k: Partial<KeyboardState>): void {
  hardwareKeyboard = !!k.hardware;
  // Someone removed it (the daemon on a control change, or `keyboard disconnect`): not ours now.
  if (!hardwareKeyboard) kbOurs = false;
  syncKeyboard();
}

/** The device is gone from this viewer: its keyboard went with the connection (the daemon
 * removes one asked for `while_connected`). */
function forgetKeyboard(): void {
  hardwareKeyboard = false;
  kbOurs = false;
  kbBusy = false;
  keysDown.clear();
  showKeyboardState();
  syncSharedClipboard();
}

/** A field of the viewer has the keys. */
function editing(): boolean {
  const a = document.activeElement;
  return (
    a instanceof HTMLInputElement ||
    a instanceof HTMLTextAreaElement ||
    a instanceof HTMLSelectElement ||
    (a instanceof HTMLElement && a.isContentEditable)
  );
}

/** ⌘ shortcuts that stay with the viewer and the Mac: zoom (⌘0, ⌘9, ⌘+, ⌘−), the keyboard
 * menu's (⌘K, ⌥⌘K, ⇧⌘V, ⇧⌘C), the app menu's Show Touches (⇧⌘T) and Settings (⌘,), and quit,
 * close, hide, minimize. Every other ⌘ combination goes to the device (⌘A, ⌘C, ⌘V, ⌘Z…). A menu
 * shortcut reaches the menu only if the page doesn't take the key. */
function viewerShortcut(e: KeyboardEvent): boolean {
  if (!e.metaKey) return false;
  if (e.shiftKey && (e.code === "KeyV" || e.code === "KeyC" || e.code === "KeyT")) return true;
  return ["Equal", "Minus", "Digit0", "Digit9", "KeyK", "KeyQ", "KeyW", "KeyH", "KeyM", "Comma", "NumpadAdd", "NumpadSubtract"].includes(e.code);
}

function keysGo(): boolean {
  return (
    forwardingActive() &&
    hardwareKeyboard &&
    !editing() &&
    (overScreen || document.activeElement === canvas) &&
    kbMenu.hidden &&
    $<HTMLDivElement>("#more-menu").hidden
  );
}

window.addEventListener("keydown", (e) => {
  if (e.metaKey && e.code === "KeyK" && stage.dataset.state === "live") {
    e.preventDefault();
    if (e.altKey) toggleSoftwareKeyboard();
    else setForwarding(!forwarding);
    return;
  }
  // Not while a field of the viewer has focus: there ⇧⌘V and ⇧⌘C are the field's.
  if (e.metaKey && e.shiftKey && !e.altKey && (e.code === "KeyV" || e.code === "KeyC") && stage.dataset.state === "live" && !editing()) {
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
  keysDown.add(e.code);
  keyStep(e.code, true);
  // With ⌘ held, macOS sends no keyup for the other keys: let go of this one at once.
  if (e.metaKey && !e.code.startsWith("Meta")) {
    keysDown.delete(e.code);
    keyStep(e.code, false);
  }
});

window.addEventListener("keyup", (e) => {
  if (e.code === "CapsLock" && keysGo()) {
    e.preventDefault();
    // The release of the press the keydown already sent: a second tap would switch the input
    // source back (PITFALLS #45).
    const release = e.timeStamp - capsTappedAt < CAPS_RELEASE_MS;
    capsTappedAt = -Infinity;
    if (!release) keyTap(e.code);
    return;
  }
  if (!keysDown.has(e.code)) return;
  e.preventDefault();
  keysDown.delete(e.code);
  keyStep(e.code, false);
  if (e.code.startsWith("Control")) ctrlUpAt = e.timeStamp;
  // Letting go of ⌘ lets go of whatever macOS didn't report.
  if (e.code.startsWith("Meta")) releaseKeys();
});

window.addEventListener("blur", releaseKeys);
document.addEventListener("visibilitychange", () => {
  if (document.hidden) releaseKeys();
});
screenBox.addEventListener("pointerenter", () => (overScreen = true));
screenBox.addEventListener("pointerleave", () => (overScreen = false));
// Clicking the screen gives it the keys, so typing goes on after the pointer leaves it.
canvas.tabIndex = 0;
canvas.addEventListener("pointerdown", () => {
  if (editing()) (document.activeElement as HTMLElement).blur();
  canvas.focus({ preventScroll: true });
});

kbForward.addEventListener("click", () => setForwarding(!forwarding));

function toggleSoftwareKeyboard(): void {
  if (!hardwareKeyboard) {
    showNote(t("keyboard.softwareAlone"), { icon: "i-keyboard" });
    return;
  }
  // The Eject key: with a hardware keyboard attached, iOS shows or hides the on-screen keyboard
  // (PITFALLS #39).
  void send("button", { name: "eject" }).catch(() => {});
}
kbSoftware.addEventListener("click", toggleSoftwareKeyboard);

// The clipboard, both ways. The text goes between the Mac clipboard and the daemon inside the
// Tauri shell: the page sees only how many characters went, and shows or logs none of it.
// Sending doesn't paste: iOS asks the user to allow a paste that ⌘V or our paste triggers
// (PITFALLS #40), but not one the user makes from the edit menu (PITFALLS #43).
const clipSend = $<HTMLButtonElement>("#kb-clip-send");
const clipGet = $<HTMLButtonElement>("#kb-clip-get");
let clipBusy = false;

async function clipboardOp(cmd: "clipboard_to_device" | "clipboard_from_device"): Promise<Record<string, unknown> | undefined> {
  if (clipBusy) return;
  clipBusy = true;
  try {
    return (await invoke(cmd)) as Record<string, unknown>;
  } catch (e) {
    showNote(errorText(e), { error: true });
  } finally {
    clipBusy = false;
  }
}

async function sendClipboard(): Promise<void> {
  // The agent's input (and a paste waiting for `paste --restore`) would be disturbed.
  if (!humanHasDevice()) {
    showNote(t("clipboard.agentHasDevice"), { error: true });
    return;
  }
  if (await clipboardOp("clipboard_to_device")) showNote(t("clipboard.sent", { device: deviceKind() }), { icon: "i-clip-send" });
}

async function getClipboard(): Promise<void> {
  const r = await clipboardOp("clipboard_from_device");
  if (!r) return;
  const device = deviceKind();
  if (r.kind === "text") showNote(t("clipboard.got", { device }), { icon: "i-clip-get" });
  else if (r.kind === "empty") showNote(t("clipboard.empty", { device }), { icon: "i-clip-get" });
  else showNote(t("clipboard.notText", { device }), { error: true });
}

clipShared.addEventListener("click", () => {
  sharedClipboard = !sharedClipboard;
  prefs.set("shared-clipboard", sharedClipboard ? "1" : "0");
  syncSharedClipboard();
});
clipSend.addEventListener("click", () => void sendClipboard());
clipGet.addEventListener("click", () => void getClipboard());
showKeyboardState();

// ---------- notes: where a file went, or what failed ----------

const note = $<HTMLDivElement>("#note");
const noteText = $<HTMLParagraphElement>("#note-text");
const noteReveal = $<HTMLButtonElement>("#note-reveal");
let notePath: string | undefined;
/** The last recording saved, for the More menu. */
let recordedPath: string | undefined;

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** "iPhone" or "iPad", as the device in a note. */
const deviceKind = () => (isTablet(selected) ? "iPad" : "iPhone");

/** The note over the stage. With a file to show, `text` has a `{file}` placeholder where its
 * name goes, in bold. */
function showNote(text: string, o: { path?: string; error?: boolean; icon?: string } = {}): void {
  notePath = o.path;
  note.hidden = false;
  note.classList.toggle("error", !!o.error);
  $<HTMLElement>("#note-icon").querySelector("use")!.setAttribute("href", `#${o.error ? "i-alert" : (o.icon ?? "i-check")}`);
  noteText.replaceChildren();
  if (o.path) {
    const name = document.createElement("strong");
    name.textContent = fileName(o.path);
    const [before, after = ""] = text.split("{file}");
    noteText.append(before, name, after);
    noteText.title = o.path;
  } else {
    noteText.textContent = text;
    noteText.title = text;
  }
  noteReveal.hidden = !o.path;
}

function reveal(path: string | undefined): void {
  if (path) invoke("reveal", { path }).catch((e) => showNote(errorText(e), { error: true }));
}
const showRevealText = () => (noteReveal.textContent = t(navigator.userAgent.includes("Windows") ? "note.showInExplorer" : "note.showInFinder"));
showRevealText();
noteReveal.addEventListener("click", () => reveal(notePath));
$<HTMLButtonElement>("#note-dismiss").addEventListener("click", () => (note.hidden = true));

/** A local date and time for file names, such as 2026-11-24 18.52.07. */
function stamp(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}.${p(d.getMinutes())}.${p(d.getSeconds())}`;
}

// ---------- screenshot ----------
//
// The canvas holds the last frame at the panel's full resolution, turned upright: that is the
// screenshot, without the overlay (cursor, labels, touch discs), which is drawn over it in HTML.

const shotBtn = $<HTMLButtonElement>("#shot");
shotBtn.addEventListener("click", async () => {
  if (!lastFrame) return;
  shotBtn.disabled = true;
  try {
    const blob = await new Promise<Blob | null>((done) => canvas.toBlob(done, "image/png"));
    if (!blob) throw t("screenshot.notEncoded");
    const png = new Uint8Array(await blob.arrayBuffer());
    const path = await invoke<string>("save_screenshot", png, { headers: { "x-stamp": stamp() } });
    showNote(t("screenshot.saved"), { path, icon: "i-camera" });
  } catch (e) {
    showNote(t("screenshot.failed", { error: errorText(e) }), { error: true });
  } finally {
    shotBtn.disabled = false;
  }
});

// ---------- recording ----------
//
// The daemon records (one recording per device); this button starts and stops it, and the
// indicator follows the daemon's `recording` events, so a recording the CLI or an agent started
// shows here too and can be stopped here.

const recordBtn = $<HTMLButtonElement>("#record");
const recordLabel = $<HTMLSpanElement>("#record-label");
/** The running recording, with its start on the page's clock. */
let recording: { path: string; since: number } | undefined;
let recordTimer: number | undefined;

function clock(seconds: number): string {
  const s = Math.floor(seconds);
  const h = Math.floor(s / 3600);
  const mm = String(Math.floor((s % 3600) / 60)).padStart(h ? 2 : 1, "0");
  const ss = String(s % 60).padStart(2, "0");
  return h ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

function showRecording(): void {
  window.clearInterval(recordTimer);
  recordBtn.classList.toggle("recording", !!recording);
  recordBtn.setAttribute("aria-pressed", String(!!recording));
  recordBtn.querySelector("use")!.setAttribute("href", recording ? "#i-stop" : "#i-record");
  if (!recording) {
    recordLabel.textContent = t("record.label");
    recordBtn.title = t("record.help");
    recordBtn.setAttribute("aria-label", t("record.label"));
    return;
  }
  const r = recording;
  const tick = () => (recordLabel.textContent = clock((performance.now() - r.since) / 1000));
  tick();
  recordTimer = window.setInterval(tick, 500);
  recordBtn.title = t("record.stopFile", { file: fileName(r.path) });
  recordBtn.setAttribute("aria-label", t("record.stop"));
}

function onRecording(e: RecordingEvent): void {
  if (e.recording) {
    recording = { path: e.path, since: performance.now() - (e.seconds ?? 0) * 1000 };
    if (!note.classList.contains("error")) note.hidden = true;
  } else {
    recording = undefined;
    if (e.error) showNote(t("record.failed", { error: e.error }), { error: true });
    else {
      recordedPath = e.path;
      showNote(e.summary ? t("record.savedLength", { length: clock(e.summary.seconds) }) : t("record.saved"), { path: e.path });
    }
  }
  showRecording();
  updateMenu();
  report();
}

recordBtn.addEventListener("click", async () => {
  recordBtn.disabled = true;
  try {
    if (recording) {
      await invoke("send", { cmd: "record_stop", args: {} });
    } else {
      const path = await invoke<string>("recording_path", { stamp: stamp() });
      await invoke("send", { cmd: "record_start", args: { path } });
    }
  } catch (e) {
    showNote(errorText(e), { error: true });
  } finally {
    recordBtn.disabled = false;
  }
});

// ---------- menus ----------

/** A button that opens a menu under it: Escape and a click elsewhere close it, the arrow keys move
 * through its items. Returns the opener. */
function menu(btn: HTMLButtonElement, list: HTMLElement): (open: boolean) => void {
  const open = (on: boolean) => {
    list.hidden = !on;
    btn.setAttribute("aria-expanded", String(on));
    if (on) list.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  };
  btn.addEventListener("click", () => open(list.hidden));
  document.addEventListener("pointerdown", (e) => {
    if (!list.hidden && (e.target as Element).closest(".menu-anchor") !== btn.parentElement) open(false);
  });
  list.addEventListener("keydown", (e) => {
    const items = [...list.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "Escape") {
      open(false);
      btn.focus();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      items[(i + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
    }
  });
  list.addEventListener("click", (e) => {
    if ((e.target as Element).closest("button")) open(false);
  });
  return open;
}

menu($<HTMLButtonElement>("#more"), $<HTMLDivElement>("#more-menu"));

function updateMenu(): void {
  $<HTMLButtonElement>("#menu-stop-view").disabled = !wantView;
  $<HTMLButtonElement>("#menu-reveal").disabled = !recordedPath;
}

$<HTMLButtonElement>("#menu-stop-view").addEventListener("click", () => void stopView());
$<HTMLButtonElement>("#menu-reveal").addEventListener("click", () => reveal(recordedPath));
// Settings is in the app menu on macOS (view_menu.rs, ⌘,); elsewhere the window has no menu bar,
// so the More menu and Ctrl+, open it.
if (root.classList.contains("mac")) for (const e of document.querySelectorAll("[data-no-app-menu]")) e.remove();
else {
  $<HTMLButtonElement>("#menu-settings").addEventListener("click", () => openSettings());
  window.addEventListener("keydown", (e) => {
    if (e.ctrlKey && !e.shiftKey && !e.altKey && !e.metaKey && e.code === "Comma") {
      e.preventDefault();
      openSettings();
    }
  });
}

// ---------- inspector: settings ----------

const settingsPane = $<HTMLElement>("#pane-settings");
const settingsStatus = $<HTMLParagraphElement>("#settings-status");
const voWarning = $<HTMLDivElement>("#vo-warning");
const appearance = $<HTMLSelectElement>("#appearance");
const liquidGlass = $<HTMLInputElement>("#liquid-glass");
const textSize = $<HTMLInputElement>("#text-size");
const locationMode = $<HTMLSelectElement>("#location-mode");
const locationEdit = $<HTMLDivElement>("#location-edit");
const latitude = $<HTMLInputElement>("#latitude");
const longitude = $<HTMLInputElement>("#longitude");

/** Text sizes from smallest to largest, as the CLI names them (`devicesteer_core::TEXT_SIZES`). */
const TEXT_SIZES = ["xs", "s", "m", "l", "xl", "xxl", "xxxl", "ax1", "ax2", "ax3", "ax4", "ax5"];
/** Each text size's name, for the slider's tooltip; the larger ones go by their short names. */
function textSizeName(i: number): string {
  const named: (Key | undefined)[] = ["settings.textSizeXS", "settings.textSizeS", "settings.textSizeM", "settings.textSizeL", "settings.textSizeXL"];
  const key = named[i];
  return key ? t(key) : (TEXT_SIZES[i]?.toUpperCase() ?? "");
}
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
  return settingsPane.querySelector<HTMLElement>(`[data-item="${name}"]`)!;
}

/** Fills a slider's track up to its thumb. */
function fill(s: HTMLInputElement): void {
  const min = Number(s.min);
  const max = Number(s.max);
  s.style.setProperty("--p", `${((Number(s.value) - min) / (max - min)) * 100}%`);
}

/** Shows the settings, merged into what is known (an event may carry one key). */
function showSettings(s: Partial<Settings>): void {
  settings = { ...settings, ...s };
  const cur = settings;
  voWarning.hidden = cur.voiceover !== true;
  appearance.value = cur.appearance ?? "";
  if (cur.liquid_glass != null) liquidGlass.value = String(cur.liquid_glass);
  liquidGlass.title =
    cur.liquid_glass != null
      ? t("settings.clearToTintedValue", { value: num(cur.liquid_glass, { minimumFractionDigits: 2, maximumFractionDigits: 2 }) })
      : t("settings.clearToTinted");
  const size = cur.text_size ? TEXT_SIZES.indexOf(cur.text_size) : -1;
  if (size >= 0) textSize.value = String(size);
  textSize.title = size >= 0 ? textSizeName(size) : "";
  fill(liquidGlass);
  fill(textSize);
  for (const [key] of TOGGLES) {
    item(key).querySelector<HTMLInputElement>("input")!.checked = cur[key] === true;
  }
  const editing = document.activeElement === latitude || document.activeElement === longitude;
  if (cur.location) {
    locationMode.value = "custom";
    if (!editing) {
      latitude.value = String(cur.location[0]);
      longitude.value = String(cur.location[1]);
    }
  } else if (!editing) {
    locationMode.value = "none";
  }
  locationEdit.hidden = locationMode.value !== "custom";
  for (const e of settingsPane.querySelectorAll<HTMLElement>("[data-item]")) {
    const off = cur.unsupported?.includes(e.dataset.item!) ?? false;
    e.classList.toggle("unsupported", off);
    for (const input of e.querySelectorAll<HTMLInputElement | HTMLButtonElement | HTMLSelectElement>("input, button, select")) {
      input.disabled = off;
    }
  }
}

/** What the settings pane says, kept to say it again in another language. */
let settingsText: () => string = () => "";

/** `text`: written in the current language when it is a function; a string is shown as it is
 * (the daemon's errors). */
function settingsMessage(text: string | (() => string), error = false): void {
  settingsText = typeof text === "function" ? text : () => text;
  settingsStatus.textContent = settingsText();
  settingsStatus.classList.toggle("error", error);
}

async function readSettings(): Promise<void> {
  settingsMessage(() => t("settings.reading"));
  try {
    showSettings(await invoke<Settings>("send", { cmd: "settings", args: {} }));
    settingsMessage("");
  } catch (e) {
    settingsMessage(errorText(e), true);
  }
}

/** Changes one item; on failure the controls go back to the last known values. */
async function setSetting(name: string, value: (string | number)[]): Promise<void> {
  settingsPane.setAttribute("aria-busy", "true");
  settingsMessage(() => t("settings.applying"));
  try {
    const r = await invoke<{ settings: Settings }>("send", { cmd: "settings_set", args: { name, value: value.map(String) } });
    showSettings(r.settings);
    settingsMessage("");
  } catch (e) {
    showSettings({});
    settingsMessage(errorText(e), true);
  } finally {
    settingsPane.removeAttribute("aria-busy");
  }
}

appearance.addEventListener("change", () => void setSetting("appearance", [appearance.value]));
liquidGlass.addEventListener("input", () => fill(liquidGlass));
liquidGlass.addEventListener("change", () => void setSetting("liquid-glass", [liquidGlass.value]));
textSize.addEventListener("input", () => {
  fill(textSize);
  textSize.title = textSizeName(Number(textSize.value));
});
textSize.addEventListener("change", () => void setSetting("text-size", [TEXT_SIZES[Number(textSize.value)]]));
for (const [key, name] of TOGGLES) {
  const box = item(key).querySelector<HTMLInputElement>("input")!;
  box.addEventListener("change", () => void setSetting(name, [box.checked ? "on" : "off"]));
}
locationMode.addEventListener("change", () => {
  if (locationMode.value === "none") {
    locationEdit.hidden = true;
    if (settings.location) void setSetting("location", ["clear"]);
  } else {
    locationEdit.hidden = false;
    latitude.focus();
  }
});
function simulateLocation(): void {
  const [lat, lon] = [latitude.value, longitude.value].map((v) => (v.trim() ? Number(v) : NaN));
  if (!(Math.abs(lat) <= 90) || !(Math.abs(lon) <= 180)) {
    settingsMessage(() => t("settings.locationInvalid"), true);
    return;
  }
  void setSetting("location", ["set", lat, lon]);
}
$<HTMLButtonElement>("#location-set").addEventListener("click", simulateLocation);
for (const f of [latitude, longitude]) {
  f.addEventListener("keydown", (e) => {
    if (e.key === "Enter") simulateLocation();
  });
}
$<HTMLButtonElement>("#vo-off").addEventListener("click", () => void setSetting("voiceover", ["off"]));

// ---------- inspector: tabs and info ----------

const TABS = ["settings", "info"];
for (const tab of TABS) {
  $<HTMLButtonElement>(`#tab-${tab}`).addEventListener("click", () => {
    for (const t of TABS) {
      $<HTMLButtonElement>(`#tab-${t}`).setAttribute("aria-selected", String(t === tab));
      $<HTMLElement>(`#pane-${t}`).hidden = t !== tab;
    }
    prefs.set("tab", tab);
    showInfo();
  });
}
const savedTab = prefs.get("tab");
if (savedTab && savedTab !== "settings" && TABS.includes(savedTab)) $<HTMLButtonElement>(`#tab-${savedTab}`).click();
initAgents();

function out(id: string, text: string): void {
  const o = $<HTMLOutputElement>(`#info-${id}`);
  o.value = text || "–";
  o.title = text;
}

const avg = (xs: number[]) => (xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : 0);

function showInfo(): void {
  const d = selected;
  out("name", d?.name ?? "");
  // The model's name, and its identifier in small gray text when the two differ.
  out("model", d ? (modelName(d) ?? "") : "");
  if (d?.model_name && d.product_type) {
    const code = el("small", "code", d.product_type);
    $<HTMLOutputElement>("#info-model").append(" ", code);
  }
  out("os", d ? [osLabel(d), d.build_version ? `(${d.build_version})` : ""].filter(Boolean).join(" ") : "");
  out("connection", d ? d.transports.map((x) => (x === "wifi" ? "Wi-Fi" : "USB")).join(t("common.listSeparator")) : "");
  out("udid", d?.udid ?? "");
  const now = performance.now();
  stats.fpsWindow = stats.fpsWindow.filter((t) => now - t < 1000);
  const dm = stats.decodeMs.slice(-60);
  const states: Record<string, Key | undefined> = {
    intro: "info.streamNotShown",
    setup: "info.streamNeedsSetup",
    unavailable: "device.unavailable",
    disconnected: "device.disconnected",
    connecting: "common.connecting",
    error: "info.streamError",
    live: stats.supported === false ? "info.streamUnsupported" : "info.streamLive",
  };
  const state = states[stage.dataset.state ?? "none"];
  out("stream", state ? t(state) : "");
  const live = stage.dataset.state === "live";
  const ms = (n: number, digits: number) => t("info.ms", { ms: num(n, { minimumFractionDigits: digits, maximumFractionDigits: digits }) });
  out("video", live && config ? `${config.codec.split(".")[0].toUpperCase()} ${config.display_px.join("×")}` : "");
  out("screen", live && config ? t(config.orientation === "landscape" ? "info.screenLandscape" : "info.screenPortrait", { size: config.screen.join("×") }) : "");
  out("fps", live ? t("info.fps", { fps: stats.fpsWindow.length }) : "");
  out(
    "decoded",
    live
      ? [t("info.decodedOf", { decoded: stats.decoded, received: stats.received }), stats.dropped ? t("info.dropped", { count: stats.dropped }) : ""].filter(Boolean).join(t("common.listSeparator"))
      : "",
  );
  out("decode", live && dm.length ? ms(avg(dm), 1) : "");
  out("touch", stats.touchMs.length ? ms(avg(stats.touchMs), 0) : "");
  $<HTMLParagraphElement>("#info-error").textContent = stats.lastError;
}

// ---------- setup: a device that isn't ready ----------

/** Step names in the setup card, and short ones for a sidebar row's status. */
const setupTitle = (step: SetupStep): string =>
  t(({ trust: "setup.trust.title", developer_mode: "setup.developerMode.title", disk_image: "setup.diskImage.title" } as const)[step]);
/** Short, and different at their start: a narrow sidebar cuts the status off. */
const setupShort = (step: SetupStep): string =>
  t(({ trust: "setup.trust.short", developer_mode: "setup.developerMode.short", disk_image: "setup.diskImage.short" } as const)[step]);
const SETUP_ORDER: SetupStep[] = ["trust", "developer_mode", "disk_image"];

const introCard = $<HTMLDivElement>("#intro-card");
/** The device the card is for. */
let setupFor: DeviceInfo | undefined;
/** The last state read for it (`setup_status`, or a step's result). */
let setupStatus: SetupStatus | undefined;
/** The step running now, with its latest progress, and when it began waiting for the person
 * holding the device (`performance.now()`), which starts the countdown. */
let setupRun: { step: SetupStep; progress?: SetupProgress; waitingSince?: number } | undefined;
let setupError: { step: SetupStep; text: string } | undefined;
let setupTimer: number | undefined;
/** Ticks the countdown on the card while a step waits for the person. */
let setupTick: number | undefined;
/** How long a step waits for the person holding the device: tapping Trust, or turning Developer
 * Mode on, which takes a restart and a confirmation. Passed to `setup_run`, so the countdown on
 * the card ends when the core gives up. Dev builds take `?setupwait=<seconds>` to see the end
 * without waiting ten minutes. */
const SETUP_WAIT_S = (import.meta.env.DEV && Number(new URLSearchParams(location.search).get("setupwait"))) || 600;
/** Bumped when the card is put away, so a late reply doesn't redraw it. */
let setupGen = 0;
/** Re-read the state this often while the card shows: the person may have done a step on the
 * device by themselves (trusted it in Finder, turned Developer Mode on). */
const SETUP_POLL_MS = 3000;

/** What each step asks of the person, and the button that starts it. */
function setupText(step: SetupStep, d: DeviceInfo): { text: string; button?: string; cancel?: boolean; action?: () => void } {
  switch (step) {
    case "trust":
      // Trust goes over USB only; our own Wi-Fi pairing needs no cable (iOS 27 and later).
      if (!d.transports.includes("usb"))
        return { text: t("setup.trust.noCable"), button: t("sidebar.pairWifi"), action: openPairing };
      return { text: t("setup.trust.text"), button: t("setup.trust.button"), cancel: true };
    case "developer_mode":
      return { text: t("setup.developerMode.text"), button: t("setup.developerMode.button"), cancel: true };
    case "disk_image":
      return { text: t("setup.diskImage.text"), button: t("setup.diskImage.button") };
  }
}

/** Each step's state: from the last reading, or before there is one, from the device list. */
function setupStates(d: DeviceInfo): Record<SetupStep, "done" | "needed" | "blocked"> {
  const out = {} as Record<SetupStep, "done" | "needed" | "blocked">;
  if (setupStatus) {
    for (const s of setupStatus.steps) out[s.step] = s.state;
    return out;
  }
  const at = SETUP_ORDER.indexOf(d.setup ?? "disk_image");
  SETUP_ORDER.forEach((s, i) => (out[s] = i < at ? "done" : i === at ? "needed" : "blocked"));
  return out;
}

/** Shows the setup card for `d` and keeps it current. */
function showSetup(d: DeviceInfo): void {
  if (setupFor?.udid !== d.udid) {
    hideSetup();
    setupFor = d;
    setupTimer = window.setInterval(() => void pollSetup(), SETUP_POLL_MS);
    void pollSetup();
  }
  setupFor = d;
  renderSetup();
}

/** Puts the card away and cancels a step still running (pairing waits minutes for a tap). */
function hideSetup(): void {
  if (setupRun) void invoke("setup_cancel").catch(() => {});
  setupGen++;
  window.clearInterval(setupTimer);
  setupTimer = undefined;
  setupFor = undefined;
  setupStatus = undefined;
  setupRun = undefined;
  setupError = undefined;
  stopSetupTick();
  introCard.replaceChildren();
}

async function pollSetup(): Promise<void> {
  const d = setupFor;
  // A running step reads the device itself; a second reading meanwhile could get in its way.
  if (!d || setupRun || document.hidden) return;
  const gen = setupGen;
  try {
    const st = await invoke<SetupStatus>("setup_status", { udid: d.udid });
    if (gen === setupGen && !setupRun) onSetupStatus(st);
  } catch {
    // Unplugged or restarting: the device list says so; the card keeps its last state.
  }
}

/** Takes a new reading: redraws, mirrors it in the sidebar row, and leaves setup once ready. */
function onSetupStatus(st: SetupStatus): void {
  const d = setupFor;
  if (!d) return;
  setupStatus = st;
  const listed = devices.find((x) => x.udid === d.udid);
  for (const x of [d, listed, selected?.udid === d.udid ? selected : undefined]) {
    if (x) x.setup = st.step ?? undefined;
  }
  renderList();
  if (st.ready) {
    // Ready: the usual intro with View Screen, attached as if just picked.
    hideSetup();
    const now = selected;
    selected = undefined;
    if (now) void select(now);
    return;
  }
  renderSetup();
}

async function runSetup(step: SetupStep): Promise<void> {
  const d = setupFor;
  if (!d || setupRun) return;
  const gen = setupGen;
  const channel = new Channel<SetupProgress>();
  channel.onmessage = (p) => {
    if (gen !== setupGen || !setupRun) return;
    setupRun.progress = p;
    // The core starts its clock as it first asks for the person (`trust`, `developer_mode`).
    if (p.waiting_for_user && setupRun.waitingSince === undefined) {
      setupRun.waitingSince = performance.now();
      startSetupTick();
    }
    renderSetup();
  };
  setupRun = { step };
  setupError = undefined;
  renderSetup();
  let waited = false;
  try {
    const st = await invoke<SetupStatus>("setup_run", { udid: d.udid, step, waitSecs: SETUP_WAIT_S, channel });
    if (gen !== setupGen) return;
    waited = setupRun?.waitingSince !== undefined;
    setupRun = undefined;
    stopSetupTick();
    // Developer Mode ends its wait without an error: the state afterwards still needs the step.
    if (waited && st.steps.find((x) => x.step === step)?.state === "needed") setupError = { step, text: setupTimeoutText(step) };
    onSetupStatus(st);
  } catch (e) {
    if (gen !== setupGen) return;
    waited = setupRun?.waitingSince !== undefined;
    setupRun = undefined;
    stopSetupTick();
    // Pairing that nobody answered fails "... within 600s" (core `setup::trust`).
    if (String(e) !== "cancelled") setupError = { step, text: waited && /\bwithin \d+s\b/.test(String(e)) ? setupTimeoutText(step) : setupErrorText(e) };
    renderSetup();
    void pollSetup();
  }
}

/** How long a wait for the person lasts, in words: "10 minutes". */
function waitText(): string {
  const m = SETUP_WAIT_S / 60;
  const [n, unit] = Number.isInteger(m) ? [m, "minute"] : [SETUP_WAIT_S, "second"];
  return num(n, { style: "unit", unit, unitDisplay: "long" });
}

/** What the card says when the wait ran out. */
function setupTimeoutText(step: SetupStep): string {
  return t(step === "trust" ? "setup.trust.timeout" : "setup.developerMode.timeout", { duration: waitText() });
}

/** The time left to wait, as "9:41 left". */
function setupTimeLeft(): { text: string; low: boolean } | undefined {
  return timeLeft(setupRun?.waitingSince);
}

function timeLeft(since: number | undefined): { text: string; low: boolean } | undefined {
  if (since === undefined) return undefined;
  const left = Math.max(0, Math.ceil(SETUP_WAIT_S - (performance.now() - since) / 1000));
  return { text: t("setup.timeLeft", { time: `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}` }), low: left < 60 };
}

function startSetupTick(): void {
  stopSetupTick();
  // Only the time changes each second: a full redraw would restart the pulse and the spinner.
  // The pairing card shares it: only one of the two cards shows at a time.
  setupTick = window.setInterval(() => {
    const t = setupRun ? setupTimeLeft() : timeLeft(pairing?.waitingSince);
    const out = introCard.querySelector<HTMLElement>(".wait-time");
    if (!t || !out) return;
    out.textContent = t.text;
    out.classList.toggle("low", t.low);
  }, 1000);
}

function stopSetupTick(): void {
  window.clearInterval(setupTick);
  setupTick = undefined;
}

/** A progress message as the card shows it: the core writes "Settings > Privacy & Security" and
 * leaves the final period off. It stays in English: the core has no codes for its messages. */
function setupMessage(m: string): string {
  const t = m.replace(/ > /g, " › ");
  return /[.!?…]$/.test(t) ? t : `${t}.`;
}

/** A setup error as the viewer shows it: the core writes for agents ("run `devicesteer setup
 * trust` again"); here the step's own button does that. It stays in English, as the core wrote
 * it. */
function setupErrorText(e: unknown): string {
  const t = String(e)
    .replace(/:? run `devicesteer setup[^`]*` first/g, "")
    .replace(/,? (?:then )?run `devicesteer setup(?: [\w-]+)?`( again)?/g, (_m, again) => (again ? ", then press Try Again" : ""))
    .replace(/`([^`]+)`/g, "$1");
  return t.charAt(0).toUpperCase() + t.slice(1) + (/[.!?]$/.test(t) ? "" : ".");
}

function cancelSetup(): void {
  void invoke("setup_cancel").catch(() => {});
}

/** The card: the three steps in order, the current one with what to do and its button. */
function renderSetup(): void {
  const d = setupFor;
  if (!d) return;
  const states = setupStates(d);
  const card = el("div", "setup");
  const head = el("div", "setup-head");
  head.append(el("h3", "", t("setup.heading")), el("p", "", t("setup.lead")));
  const list = el("ol", "setup-steps");
  SETUP_ORDER.forEach((step, i) => {
    const state = states[step] ?? "blocked";
    const running = setupRun?.step === step;
    const li = el("li", `setup-step ${state}${running ? " running" : ""}`);
    const mark = el("span", "mark");
    if (running) mark.append(el("span", "spinner"));
    else if (state === "done") mark.append(icon("i-check"));
    else mark.textContent = String(i + 1);
    const body = el("div", "body");
    body.append(el("div", "title", setupTitle(step)));
    if (state === "needed") {
      const st = setupText(step, d);
      const p = running ? setupRun?.progress : undefined;
      // While it runs, the step's own progress says what to do now (it repeats the description
      // at first, then follows the device: restarting, confirm after the restart).
      if (!running) body.append(el("p", "desc", st.text));
      if (running && p?.waiting_for_user) {
        // Waiting for the person, maybe for minutes: say so plainly, with what to do now and the
        // time left, so the card doesn't look stuck.
        const box = el("div", "wait");
        const top = el("div", "wait-head");
        const left = setupTimeLeft();
        const time = el("span", `wait-time${left?.low ? " low" : ""}`, left?.text ?? "");
        top.append(el("span", "pulse"), el("span", "wait-label", t("setup.waitingOnDevice")), time);
        const msg = el("p", "wait-msg", setupMessage(p.message));
        msg.setAttribute("role", "status");
        box.append(top, msg);
        body.append(box);
      } else if (running) {
        const line = el("div", "progress");
        line.append(el("span", "msg", p?.message ?? t("setup.starting")));
        if (p?.fraction !== undefined) {
          const bar = el("div", "bar");
          bar.style.setProperty("--f", String(Math.max(0, Math.min(1, p.fraction))));
          line.append(bar);
        }
        body.append(line);
      }
      if (setupError?.step === step) body.append(el("p", "error", setupError.text));
      const actions = el("div", "actions");
      if (running && st.cancel) {
        const b = el("button", "btn small", t("common.cancel"));
        b.type = "button";
        b.addEventListener("click", cancelSetup);
        actions.append(b);
      } else if (!running && st.button) {
        const b = el("button", "btn small", setupError?.step === step && !st.action ? t("common.tryAgain") : st.button);
        b.type = "button";
        // Nothing to reach while the device is away; Cancel above stays.
        b.disabled = !!gone && !st.action;
        b.addEventListener("click", () => (st.action ? st.action() : void runSetup(step)));
        actions.append(b);
      }
      if (actions.childElementCount) body.append(actions);
    }
    li.append(mark, body);
    list.append(li);
  });
  card.append(head, list);
  // Keep focus on the step's button across redraws (progress redraws often).
  const hadFocus = introCard.contains(document.activeElement);
  introCard.replaceChildren(card);
  if (hadFocus) introCard.querySelector<HTMLButtonElement>(".actions .btn")?.focus();
}

/** The card's state, for the report. */
function setupReport(): unknown {
  if (!setupFor) return null;
  return {
    steps: [...introCard.querySelectorAll(".setup-step")].map((li) => li.className.replace("setup-step ", "")),
    running: setupRun
      ? { step: setupRun.step, message: setupRun.progress?.message ?? null, waiting: !!setupRun.progress?.waiting_for_user, left: setupTimeLeft()?.text ?? null }
      : null,
    error: setupError?.text ?? null,
    button: introCard.querySelector(".actions .btn")?.textContent ?? null,
  };
}

// ---------- pairing a device over Wi-Fi ----------
//
// No cable and no device to select yet: the card takes the stage on its own. This computer
// offers itself on the local network while the card waits (`setup_pair_wifi`); the person starts
// pairing on the device, enters the device's own passcode, then the code shown here.

/** The pairing on the card: waiting for the device to ask, showing the code it asked for, paired
 * (opening the device), or ended with an error. `waitingSince` (`performance.now()`) starts the
 * countdown; the core waits afresh once it has shown the code, and so does the card. */
let pairing:
  | { phase: "waiting" | "code" | "done" | "failed"; code?: string; waitingSince?: number; error?: string }
  | undefined;
/** Bumped when the card is put away or pairing starts again, so a late reply doesn't redraw it. */
let pairGen = 0;
/** The device selected before the card opened: Cancel goes back to it. */
let pairBack: DeviceInfo | undefined;

/** The steps on the device, in its own words (iOS 27). */
const pairSteps = (): { title: string; text: string }[] => [
  { title: t("pair.step1.title"), text: t("pair.step1.text") },
  { title: t("pair.step2.title"), text: t("pair.step2.text") },
  { title: t("pair.step3.title"), text: t("pair.step3.text") },
];

const pairRunning = () => pairing?.phase === "waiting" || pairing?.phase === "code";

/** Opens the pairing card and starts pairing. The selected device is let go of first: the stage
 * has room for one thing. */
function openPairing(): void {
  if (pairing) {
    introCard.querySelector<HTMLButtonElement>(".actions .btn")?.focus();
    return;
  }
  const back = selected;
  // Not awaited: all but its last call (letting go of the daemon) is done when it returns, and
  // `pairing` must be set at once so a device list that comes back meanwhile leaves the card.
  void detach();
  gone = undefined;
  selected = undefined;
  pairBack = back;
  void startPairing();
  stageState("pair");
  showDeviceText();
  renderList();
  if (narrow(720)) showPanel("sidebar", false, false);
  settingsPane.classList.add("disabled");
  settingsMessage(() => t("settings.afterPairing"));
}

async function startPairing(): Promise<void> {
  const gen = ++pairGen;
  pairing = { phase: "waiting" };
  const channel = new Channel<SetupProgress>();
  channel.onmessage = (p) => {
    if (gen !== pairGen || !pairRunning()) return;
    if (p.code) pairing = { phase: "code", code: p.code, waitingSince: performance.now() };
    else if (p.waiting_for_user && pairing!.waitingSince === undefined) pairing!.waitingSince = performance.now();
    startSetupTick();
    renderPairing();
  };
  renderPairing();
  try {
    const st = await invoke<SetupStatus>("setup_pair_wifi", { waitSecs: SETUP_WAIT_S, channel });
    if (gen !== pairGen) return;
    stopSetupTick();
    pairing = { phase: "done" };
    renderPairing();
    pairBack = undefined;
    await loadDevices();
    if (gen !== pairGen) return;
    // Selecting it shows its setup card, or the intro once it is ready, as for any device.
    const d = devices.find((x) => x.udid === st.udid) ?? { udid: st.udid, transports: st.transports, setup: st.step ?? undefined };
    void select(d);
  } catch (e) {
    if (gen !== pairGen || !pairing) return;
    stopSetupTick();
    pairing = { phase: "failed", error: String(e) === "cancelled" ? t("pair.stopped") : pairErrorText(e) };
    renderPairing();
  }
}

/** Puts the card away and stops pairing if it still waits. */
function hidePairing(): void {
  if (!pairing) return;
  if (pairRunning()) void invoke("setup_cancel").catch(() => {});
  pairGen++;
  pairing = undefined;
  stopSetupTick();
  introCard.replaceChildren();
}

/** Cancel or Close: back to the device selected before, if it is still there. */
function closePairing(): void {
  const back = pairBack && devices.find((d) => d.udid === pairBack!.udid);
  pairBack = undefined;
  hidePairing();
  if (back) {
    void select(back);
    return;
  }
  stageState("none");
  showDeviceText();
  report();
}

/** A pairing error as the card shows it. The core's waits fail "... within 600s". */
function pairErrorText(e: unknown): string {
  const s = String(e);
  if (/^no device asked to pair within \d+s/.test(s)) return t("pair.noDevice", { duration: waitText() });
  if (/^nobody entered the code on the device within \d+s/.test(s)) return t("pair.noCode", { duration: waitText() });
  return setupErrorText(s.replace(/\. Run `devicesteer setup wifi` again/, ", then press Try Again"));
}

/** "123456" as "123 456": two groups read and type more easily. */
const spacedCode = (c: string) => c.replace(/^(\d{3})(\d{3})$/, "$1 $2");

/** The card: what to do on the device, the wait or the code, and its buttons. */
function renderPairing(): void {
  const p = pairing;
  if (!p) return;
  const card = el("div", "setup pair");
  const head = el("div", "setup-head");
  head.append(el("h3", "", t("pair.heading")), el("p", "", t("pair.lead")));
  const list = el("ol", "pair-steps");
  pairSteps().forEach((s, i) => {
    // The device connects only after its own passcode was entered (PITFALLS #56): once the code
    // shows, the first two steps are done. While waiting, both may still be ahead.
    const done = (p.phase === "code" && i < 2) || p.phase === "done";
    const now = p.phase === "waiting" ? i < 2 : p.phase === "code" ? i === 2 : false;
    const li = el("li", `pair-step${done ? " done" : ""}${now ? " now" : ""}`);
    const mark = el("span", "mark");
    if (done) mark.append(icon("i-check"));
    else mark.textContent = String(i + 1);
    const body = el("div", "body");
    body.append(el("div", "title", s.title), el("p", "desc", s.text));
    li.append(mark, body);
    list.append(li);
  });
  card.append(head, list);

  if (p.phase === "waiting" || p.phase === "code") {
    const box = el("div", `wait${p.phase === "code" ? " with-code" : ""}`);
    const top = el("div", "wait-head");
    const left = timeLeft(p.waitingSince);
    top.append(el("span", "pulse"), el("span", "wait-label", t("setup.waitingOnDevice")), el("span", `wait-time${left?.low ? " low" : ""}`, left?.text ?? ""));
    box.append(top);
    if (p.phase === "code" && p.code) {
      const code = el("div", "pair-code");
      code.append(el("span", "pair-code-label", t("pair.code")));
      const digits = el("output", "pair-code-digits", spacedCode(p.code));
      digits.setAttribute("aria-label", t("pair.codeAria", { digits: p.code.split("").join(" ") }));
      code.append(digits);
      const msg = el("p", "wait-msg", t("pair.typeCode"));
      msg.setAttribute("role", "status");
      box.append(code, msg);
    } else {
      const msg = el("p", "wait-msg", t("pair.shownAs"));
      msg.setAttribute("role", "status");
      box.append(msg);
    }
    card.append(box);
  } else if (p.phase === "done") {
    const line = el("div", "pair-done");
    line.append(el("span", "spinner"), el("span", "", t("pair.paired")));
    line.setAttribute("role", "status");
    card.append(line);
  } else if (p.error) {
    const err = el("p", "error", p.error);
    err.setAttribute("role", "alert");
    card.append(err);
  }

  const actions = el("div", "actions");
  if (p.phase === "failed") {
    const again = el("button", "btn small", t("common.tryAgain"));
    again.type = "button";
    again.addEventListener("click", () => void startPairing());
    const close = el("button", "btn small ghost", t("common.close"));
    close.type = "button";
    close.addEventListener("click", closePairing);
    actions.append(again, close);
  } else if (p.phase !== "done") {
    const cancel = el("button", "btn small", t("common.cancel"));
    cancel.type = "button";
    cancel.addEventListener("click", closePairing);
    actions.append(cancel);
  }
  if (actions.childElementCount) card.append(actions);
  const hadFocus = introCard.contains(document.activeElement);
  introCard.replaceChildren(card);
  if (hadFocus) introCard.querySelector<HTMLButtonElement>(".actions .btn")?.focus();
  // Nothing is attached while the card shows, so nothing else reports it.
  report();
}

/** The card's state, for the report. */
function pairReport(): unknown {
  if (!pairing) return null;
  return {
    phase: pairing.phase,
    code: pairing.code ?? null,
    left: timeLeft(pairing.waitingSince)?.text ?? null,
    error: pairing.error ?? null,
    buttons: [...introCard.querySelectorAll(".actions .btn")].map((b) => b.textContent),
  };
}

$<HTMLButtonElement>("#pair-wifi").addEventListener("click", openPairing);

// ---------- attaching, viewing ----------
//
// Picking a device attaches to its daemon without video: the settings, the VoiceOver warning and
// the agent session work from there. View Screen asks for the frames; a session that starts asks
// for them too. Stop Viewing drops them and the stream stops, unless something else needs it.

/** The device attached to, once `attach` succeeded. */
let attachedUdid: string | undefined;
/** Whether the screen should show (frames asked for, or about to be). */
let wantView = false;
let statsTimer: number | undefined;
/** Which attachment the channel's messages belong to. */
let viewerGen = 0;

/** Drops the video and everything drawn from it; the attachment stays. */
function resetScreen(): void {
  endPress();
  configGen++;
  decoder?.close();
  decoder = undefined;
  decoderKey = "";
  pending = [];
  received.clear();
  waitingForKey = true;
  config = undefined;
  lastFrame?.close();
  lastFrame = undefined;
  hideCursor();
  for (const d of fingers.values()) d.remove();
  fingers.clear();
  stats.frame = undefined;
  zoom = "fit";
}

/** Leaves the attached device: everything about it goes. */
async function detach(): Promise<void> {
  hideSetup();
  hidePairing();
  window.clearInterval(statsTimer);
  viewerGen++;
  wantView = false;
  const was = attachedUdid;
  attachedUdid = undefined;
  forgetKeyboard();
  resetScreen();
  recording = undefined;
  showRecording();
  note.hidden = true;
  settings = {};
  showSettings({ unsupported: [] });
  onControl({ session: null, human_touching: false, agent_stopped: false });
  voWarning.hidden = true;
  settingsMessage("");
  Object.assign(stats, { received: 0, decoded: 0, dropped: 0, errors: 0, lastError: "", decodeMs: [], touchMs: [], fpsWindow: [] });
  if (was) await invoke("detach").catch(() => {});
}

async function select(d: DeviceInfo, opts: { view?: boolean } = {}): Promise<void> {
  if (selected?.udid === d.udid && attachedUdid === d.udid) {
    if (opts.view) void view();
    return;
  }
  // Its row while it is away: it is still selected and picks up by itself when it is back.
  if (gone && selected?.udid === d.udid) return;
  await detach();
  gone = undefined;
  selected = d;
  prefs.set("device", d.udid);
  showDeviceText();
  renderList();
  if (narrow(720)) showPanel("sidebar", false, false);
  settingsPane.classList.toggle("disabled", unusable(d) || !!d.setup);
  if (d.unsupported) {
    // Not even setup: no step makes it usable, and the daemon refuses it.
    settingsMessage(() => t("settings.unsupported"));
    stageState("unavailable", () => unsupportedText(d));
    return;
  }
  if (d.error) {
    settingsMessage(() => t("settings.afterAvailable"));
    // The core's reason, in English.
    stageState("unavailable", d.error);
    return;
  }
  if (d.setup) {
    // Nothing to attach to yet: the daemon needs a device that is set up.
    settingsMessage(() => t("settings.afterSetup"));
    stageState("setup");
    showSetup(d);
    return;
  }
  stageState(opts.view ? "connecting" : "intro");
  if (opts.view) wantView = true;
  await attach(d);
}

async function attach(d: DeviceInfo): Promise<void> {
  const channel = new Channel<ArrayBuffer | number[]>();
  const gen = ++viewerGen;
  channel.onmessage = (m) => {
    // Messages of an earlier attachment can still be in flight after another device was picked.
    if (gen === viewerGen) onMessage(m instanceof ArrayBuffer ? new Uint8Array(m) : Uint8Array.from(m));
  };
  settingsMessage(() => t("common.connecting"));
  try {
    await invoke("attach", { udid: d.udid, frames: wantView, channel });
  } catch (e) {
    if (gen !== viewerGen) return;
    settingsMessage("");
    stageState("error", () => t("stage.connectFailed", { device: deviceName(d), error: errorText(e) }));
    return;
  }
  if (gen !== viewerGen) return;
  attachedUdid = d.udid;
  // VoiceOver changes what taps do, so its warning shows whether or not the inspector is open.
  void readSettings();
  void invoke("send", { cmd: "keyboard_status", args: {} })
    .then((k) => onKeyboard(k as KeyboardState))
    .catch(() => {});
  statsTimer = window.setInterval(() => {
    showInfo();
    if (Math.round(performance.now() / 1000) % 2 === 0) report();
  }, 1000);
}

async function view(): Promise<void> {
  if (!selected || unusable(selected) || selected.setup || gone) return;
  wantView = true;
  if (stage.dataset.state !== "live") stageState("connecting", () => t("stage.starting"));
  if (!attachedUdid) {
    // Still attaching (it asks for frames now), or it failed: try again.
    if (stage.dataset.state === "error" || !viewerGen) await attach(selected);
    return;
  }
  const gen = viewerGen;
  try {
    await invoke("watch", { frames: true });
  } catch (e) {
    if (gen !== viewerGen) return;
    // The daemon went away while attached (it exited idle or was stopped to load a new build):
    // attach again, which starts a new one.
    if (String(e).includes("daemon connection closed") && selected) {
      attachedUdid = undefined;
      await attach(selected);
      return;
    }
    stageState("error", () => t("stage.startFailed", { error: errorText(e) }));
  }
}

async function stopView(): Promise<void> {
  if (!wantView) return;
  wantView = false;
  resetScreen();
  stageState("intro");
  if (attachedUdid) await invoke("watch", { frames: false }).catch(() => {});
}

viewBtn.addEventListener("click", () => void view());
$<HTMLButtonElement>("#intro-retry").addEventListener("click", () => {
  if (!selected) return;
  const d = selected;
  if (attachedUdid) void view();
  else {
    selected = undefined;
    void select(d, { view: wantView });
  }
});

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
      decodeMsAvg: d.length ? avg(d) : null,
      decodeMsMax: d.length ? Math.max(...d) : null,
      inputs: stats.inputs,
      lastInput: stats.lastInput,
      touchMsAvg: stats.touchMs.length ? avg(stats.touchMs) : null,
      touchMsMax: stats.touchMs.length ? Math.max(...stats.touchMs) : null,
      touchErrors: stats.touchErrors,
      control,
      // What the window shows, so a check of the viewer doesn't need anyone to look.
      stage: stage.dataset.state,
      setup: setupReport(),
      pairing: pairReport(),
      wantView,
      zoom: zoom === "fit" ? "fit" : zoom,
      scale: Math.round(scale * 1000) / 1000,
      device: { kind: deviceEl.dataset.kind ?? null, size: [deviceEl.offsetWidth, deviceEl.offsetHeight] },
      panels: { sidebar: !app.classList.contains("no-sidebar"), inspector: !app.classList.contains("no-inspector") },
      panel: sessionPanel.hidden
        ? null
        : { name: sessionName.textContent, state: sessionState.textContent, button: sessionPrimary.textContent },
      touchBlock: { on: screenBox.classList.contains("agent-has-device"), hint: !blockedHint.hidden, blocked: touchesBlocked },
      glow: glow.hidden ? null : glow.classList.contains("human") ? "human" : "agent",
      labelsShown,
      showTouches,
      touchDotsOnScreen: overlay.querySelectorAll(".touch-dot").length,
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
      // The canvas holds just the screen: its aspect should match `screen`'s.
      canvas: [canvas.width, canvas.height],
      screen: config?.screen ?? null,
      frame: stats.frame ?? null,
      rotation: config?.rotation ?? null,
      orientations: stats.orientations,
      window: [window.innerWidth, window.innerHeight],
      visibility: document.visibilityState,
      timelineMs: document.timeline.currentTime,
      session_slot: sessionIdle.hidden ? "panel" : "idle",
      recording: recording ? { file: fileName(recording.path), label: recordLabel.textContent } : null,
      note: note.hidden ? null : noteText.textContent,
    },
  });
}

// ---------- language ----------
//
// Another language picked in Settings (i18n.ts): `localize` has translated the static HTML; the
// parts drawn from code are drawn again. Notes and errors already shown stay as they were.

/** The app menu's labels (view_menu.rs), which the shell can't translate itself. */
const MENU_KEYS = [
  "about", "services", "hide", "hideOthers", "showAll", "quit", "file", "closeWindow", "edit", "undo", "redo", "cut", "copy",
  "paste", "selectAll", "view", "showTouches", "enterFullScreen", "window", "minimize", "zoom", "help", "settings", "settingsWindow",
] as const;

function sendMenuLabels(): void {
  const labels = Object.fromEntries(MENU_KEYS.map((k) => [k, t(`menu.${k}`, { app: "DeviceSteer" })]));
  void invoke("set_menu_labels", { labels }).catch(() => {});
}

onLanguageChange(() => {
  sendMenuLabels();
  renderList();
  showDeviceText();
  showStageText();
  settingsStatus.textContent = settingsText();
  onControl(control);
  blockedHint.textContent = t("session.blockedHint");
  showKeyboardState();
  if (clipShared.disabled) clipShared.title = t("keyboard.sharedMacOnly");
  showRevealText();
  showRecording();
  showSettings({});
  showInfo();
  if (setupFor) renderSetup();
  if (pairing) renderPairing();
});

// ---------- start ----------

localize();
sendMenuLabels();
showPanel("sidebar", !narrow(720) && prefs.get("sidebar") !== "0", false);
showPanel("inspector", !narrow(1000) && prefs.get("inspector") !== "0", false);
settingsPane.classList.add("disabled");
stageState("none");
showDeviceText();
renderList();
updateMenu();
void (async () => {
  // A device to open right away (`DEVICESTEER_VIEWER_UDID`): selected and shown.
  const udid = await invoke<string | null>("initial_udid");
  await loadDevices();
  if (udid) {
    void select(devices.find((d) => d.udid === udid) ?? { udid, transports: [] }, { view: true });
    return;
  }
  const last = prefs.get("device");
  const pick = devices.find((d) => d.udid === last && !unusable(d)) ?? devices.find((d) => !unusable(d));
  // Pair over Wi-Fi… pressed while the list loaded: the card stays.
  if (pairing) return;
  if (pick) void select(pick);
  else showDeviceText();
})();
