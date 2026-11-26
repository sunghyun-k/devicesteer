// The main window's launch sheets and update banner: moving the app to Applications when it runs
// from its disk image (app/src-tauri/src/app_location.rs), then installing the `devicesteer`
// command and the DeviceSteer skill for the coding agents on this Mac (agent_setup.rs does the
// work and explains the links), and the banner that offers an update. The Settings window
// changes the links later and checks for updates on request.

import { create } from "zustand";
import * as backend from "../../backend/commands";
import type { AgentStatus, AppLocation, MoveError } from "../../backend/types";
import { errorText, msg, t, type Text } from "../../i18n/i18n";
import { prefs } from "../../lib/prefs";
import { downloadingText } from "./agentText";

interface AgentsState {
  appVersion: string;
  appLocation: AppLocation | undefined;
  /** The first-launch sheet's list: what it would install, checked; conflicts are left to the
   * Settings window. */
  sheet: { status: AgentStatus; busy: boolean; error: string } | undefined;
  /** The move sheet: `replacing` once the person was told an app is already in Applications (the
   * next press replaces it). */
  move: { replacing: { version: string | null } | undefined; busy: boolean; error: Text } | undefined;
  /** The update found at launch; `text` while it downloads or after it failed. */
  update: { update: backend.Update; text: string | undefined; busy: boolean } | undefined;
}

export const useAgents = create<AgentsState>()(() => ({
  appVersion: "",
  appLocation: undefined,
  sheet: undefined,
  move: undefined,
  update: undefined,
}));

const get = useAgents.getState;
const set = useAgents.setState;

/** Opens the Settings window at `tab` (settings_window.rs). */
export function openSettings(tab?: "agents" | "updates"): void {
  // Logged: a failure dropped silently left a blank window with no clue why (PITFALLS #64).
  void backend.openSettings(tab ?? null).catch((e) => console.error("could not open the Settings window:", e));
}

// ---------- first launch ----------

async function offerOnFirstLaunch(): Promise<void> {
  if (prefs.get("agent-setup-offered")) return;
  let s: AgentStatus;
  try {
    s = await backend.agentSetupStatus();
  } catch {
    return;
  }
  // A development build can't set anything up; ask again in the installed app.
  if (s.unavailable) return;
  prefs.set("agent-setup-offered", "1");
  const todo = [s.cli, ...s.agents.filter((a) => a.detected)].some((i) => i.state === "missing");
  if (!todo) return;
  set({ sheet: { status: s, busy: false, error: "" } });
}

export const closeAgentSheet = () => set({ sheet: undefined });

/** Set Up: installs the checked items. */
export async function setUpAgents(ids: string[]): Promise<void> {
  const sheet = get().sheet;
  if (!sheet) return;
  set({ sheet: { ...sheet, busy: true, error: "" } });
  const failed: string[] = [];
  for (const id of ids) {
    try {
      await backend.agentSetupChange(id, true, false);
    } catch (x) {
      failed.push(errorText(x));
    }
  }
  if (failed.length) {
    set({ sheet: { ...sheet, busy: false, error: failed.join(" ") } });
    return;
  }
  closeAgentSheet();
  // The tool works only once its folder is on PATH: Settings › Agents says how to add it.
  const s = await backend.agentSetupStatus().catch(() => undefined);
  if (s?.cli.state === "installed" && s.on_path === false) openSettings("agents");
}

// ---------- move to Applications ----------

function openMoveSheet(): void {
  set({ move: { replacing: undefined, busy: false, error: "" } });
}

/** The sheet closed by itself (Escape): nothing is decided. */
export const closeMoveSheet = () => set({ move: undefined });

/** Not Now: asked again by the next version; until then Settings › Agents keeps the hint and a
 * button. */
export function declineMove(): void {
  prefs.set("move-declined", get().appVersion);
  set({ move: undefined });
}

/** Move to Applications, or Replace once told an app is there. */
export async function moveApp(): Promise<void> {
  const m = get().move;
  if (!m) return;
  const loc = get().appLocation;
  if (loc?.exists && !m.replacing) {
    set({ move: { ...m, replacing: { version: loc.existing_version } } });
    return;
  }
  set({ move: { ...m, busy: true, error: "" } });
  try {
    // On success the app quits and the copy in Applications opens, so this doesn't come back.
    await backend.moveToApplications(!!m.replacing);
  } catch (x) {
    const err = x as MoveError | string;
    if (typeof err === "object" && err.kind === "exists") set({ move: { replacing: { version: err.version }, busy: false, error: "" } });
    else if (typeof err === "object" && err.kind === "no_permission") set({ move: { ...m, busy: false, error: msg("move.noPermission") } });
    else set({ move: { ...m, busy: false, error: msg("move.failed", { error: typeof err === "object" ? err.message : err }) } });
  }
}

/** Offers the move when the app runs from somewhere that won't last; true when it did. */
async function offerMove(): Promise<boolean> {
  let loc: AppLocation;
  try {
    loc = await backend.appLocation();
  } catch {
    return false;
  }
  set({ appLocation: loc });
  if (!loc.reason) return false;
  if (prefs.get("move-declined") !== get().appVersion) openMoveSheet();
  return true;
}

// ---------- updates ----------

/** Looks for an update once after launch, quietly: only a found one shows, as the banner. */
async function checkForUpdate(): Promise<void> {
  try {
    const u = await backend.checkForUpdate();
    if (u) set({ update: { update: u, text: undefined, busy: false } });
  } catch {
    // Offline or GitHub unreachable: Settings › Updates says so when asked.
  }
}

export async function installUpdate(): Promise<void> {
  const cur = get().update;
  if (!cur) return;
  const u = cur.update;
  set({ update: { ...cur, busy: true } });
  try {
    await backend.installUpdate(u, (fraction) => set({ update: { update: u, busy: true, text: downloadingText(u.version, fraction) } }));
    // The device daemons are separate processes and keep running; the links to the CLI and the
    // skill follow the app, so they point at the new version once it starts.
    await backend.restartApp();
  } catch (e) {
    set({ update: { update: u, busy: false, text: t("update.failed", { error: String(e) }) } });
  }
}

export const dismissUpdate = () => set({ update: undefined });

/** Starts the sheets and the update check. Called once for the main window. */
export function initAgents(): void {
  void (async () => {
    set({ appVersion: await backend.appVersion().catch(() => "") });
    // A copy that should move can't set up agents; the moved copy offers that when it opens.
    if (!(await offerMove())) await offerOnFirstLaunch();
  })();
  // Settings › Agents' Move button: the sheet belongs to this window (settings_window.rs).
  void backend
    .onMoveSheet(async () => {
      if (!get().appLocation) set({ appLocation: await backend.appLocation().catch(() => undefined) });
      if (get().appLocation?.reason && !get().move) openMoveSheet();
    })
    .catch(() => {});
  // Development builds aren't released, so there is nothing to update them to.
  if (!import.meta.env.DEV) window.setTimeout(() => void checkForUpdate(), 5000);
}
