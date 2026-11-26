// What the Tauri shell and the daemon send the page: mirrors of the Rust types, by the names the
// Rust side uses.

/** Mirrors `devicesteer_core::DeviceInfo`. */
export interface DeviceInfo {
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
export type SetupStep = "trust" | "developer_mode" | "disk_image";
export const SETUP_ORDER: SetupStep[] = ["trust", "developer_mode", "disk_image"];

export type StepState = "done" | "needed" | "blocked";

export interface SetupStatus {
  udid: string;
  transports: ("usb" | "wifi")[];
  ready: boolean;
  step: SetupStep | null;
  steps: { step: SetupStep; state: StepState; detail?: string }[];
  next: string;
}

/** Mirrors `devicesteer_core::DeviceList`. */
export interface DeviceList {
  devices: DeviceInfo[];
  /** Set when usbmuxd isn't running at all (Windows without Apple Devices or iTunes). */
  usbmuxd_missing?: string;
}

/** What a running setup step reports (`devicesteer_core::setup::Progress`). */
export interface SetupProgress {
  step: SetupStep;
  /** The device waits for the person holding it. */
  waiting_for_user: boolean;
  message: string;
  fraction?: number;
  /** Wi-Fi pairing: the code to type on the device, once it asked for one. */
  code?: string;
}

/** How the interface is turned from the panel's native orientation (`devicesteer_core::Rotation`). */
export type Rotation = "rot0" | "rot90" | "rot180" | "rot270";

/** How the screen is turned: the daemon's `orientation` event, also part of `config`. */
export interface Orientation {
  /** The video picture is the panel in its native orientation; this turns it upright. */
  rotation: Rotation;
  orientation: "portrait" | "landscape";
  /** The screen as the user sees it, in points: input points and the overlay use this. */
  screen: [number, number];
}

/** The daemon's `config` event: how to set up the decoder and where the screen is. */
export interface Config extends Orientation {
  codec: string;
  description: string;
  coded: [number, number];
  /** The screen's pixel size in the picture, native orientation: the top-left part of the coded
   * picture. Smaller than the panel when the device scaled it down to fit the stream (a foldable's
   * inner panel, PITFALLS #48); the rest of the picture is padding. */
  display_px: [number, number];
}

/** An agent session as the daemon reports it (`devicesteer_daemon::control::AgentSession`). */
export interface AgentSession {
  id: string;
  name: string;
  owner: "agent" | "delegated_to_user" | "user" | "finished";
  /** Why the agent handed off, while `delegated_to_user`. */
  reason: string | null;
}

export interface ControlEvent {
  /** The session using the device; null when none runs. */
  session: AgentSession | null;
  human_touching: boolean;
  agent_stopped: boolean;
}

export type Point = [number, number];

export interface InputEvent {
  source: "agent" | "human";
  kind: "tap" | "swipe" | "button" | "touch";
  points: Point[];
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
export interface RecordingEvent {
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
export interface DeviceSettings {
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

/** The daemon's `keyboard` event and the keyboard commands' result. */
export type KeyboardState = { hardware: boolean; attached: boolean };

/** One step of a live touch, as `send touch` takes it. `t`: when the pointer was there (ms, the
 * page's clock). */
export type TouchStep = { phase: "down" | "move" | "up"; point: Point; t?: number };

/** A daemon's message through the shell (session_watch.rs): a session started, changed owner or
 * ended on `udid`. */
export interface AgentSessionMessage {
  udid: string;
  session: AgentSession | null;
  started: boolean;
}

// ---------- agent setup (agent_setup.rs, app_location.rs) ----------

export type LinkState =
  | { state: "installed" }
  | { state: "missing" }
  | { state: "other_link"; points_to: string }
  | { state: "not_a_link"; is_dir: boolean };

export type AgentItem = { id: string; name: string; path: string; detected: boolean } & LinkState;

export interface AgentStatus {
  /** Why setup can't run from this copy, in English; `unavailable_code` names it when known. */
  unavailable: string | null;
  unavailable_code: string | null;
  cli: AgentItem;
  on_path: boolean | null;
  agents: AgentItem[];
}

/** What `agent_setup_change` reports when it replaced something (`Note` in crates/agents). */
export interface AgentNote {
  code: string;
  path: string;
  message: string;
}

/** Where the app runs from (app_location.rs). `reason` is set when it should move. */
export interface AppLocation {
  reason: "disk_image" | "translocated" | null;
  exists: boolean;
  existing_version: string | null;
}

/** What `app_move_to_applications` rejects with (app_location.rs `MoveError`). */
export type MoveError = { kind: "exists"; version: string | null } | { kind: "no_permission"; message: string } | { kind: "failed"; message: string };
