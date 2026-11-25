// The main window's launch sheets: moving the app to Applications when it runs from its disk
// image (app/src-tauri/src/app_location.rs), then installing the `devicesteer` command and the
// DeviceSteer skill for the coding agents on this Mac (agent_setup.rs does the work and explains
// the links), and the banner that offers an update. The Settings window (settings.ts) changes the
// links later and checks for updates on request.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { $, el, pref, conflict, conflictText, CLI_NEEDED, type AppLocation, type Status } from "./agent-common";

let appLocation: AppLocation | undefined;
let appVersion = "";

/** Opens the Settings window at `tab` (settings_window.rs). */
export function openSettings(tab?: "agents" | "updates"): void {
  void invoke("open_settings", { tab: tab ?? null }).catch(() => {});
}

// ---------- first launch ----------

const sheet = $<HTMLDialogElement>("#agent-sheet");

/** The sheet's list: what it would install, checked; conflicts are left to the pane. */
function renderSheet(s: Status): void {
  const rows: HTMLElement[] = [];
  for (const i of [s.cli, ...s.agents.filter((a) => a.detected)]) {
    const row = el("label", "row agent-row");
    const text = el("div", "text");
    text.append(el("div", "name", i.id === "cli" ? "Command-line tool" : `Skill for ${i.name}`), el("div", "path mono", i.path));
    const box = el("input", "");
    box.type = "checkbox";
    box.setAttribute("role", "switch");
    box.value = i.id;
    if (i.state === "installed") {
      box.checked = true;
      box.disabled = true;
      text.append(el("div", "note", "Already set up"));
    } else if (conflict(i)) {
      box.disabled = true;
      text.append(el("div", "note warn", `${conflictText(i)} Replace it in Settings › Agents if you want.`));
    } else {
      box.checked = true;
      if (i.id === "cli") {
        const note = el("div", "note warn", CLI_NEEDED);
        note.hidden = true;
        text.append(note);
        box.addEventListener("change", () => (note.hidden = box.checked));
      }
    }
    row.append(text, box);
    rows.push(row);
  }
  $<HTMLDivElement>("#agent-sheet-list").replaceChildren(...rows);
}

async function offerOnFirstLaunch(): Promise<void> {
  if (pref.get("agent-setup-offered")) return;
  let s: Status;
  try {
    s = await invoke<Status>("agent_setup_status");
  } catch {
    return;
  }
  // A development build can't set anything up; ask again in the installed app.
  if (s.unavailable) return;
  pref.set("agent-setup-offered", "1");
  const todo = [s.cli, ...s.agents.filter((a) => a.detected)].some((i) => i.state === "missing");
  if (!todo) return;
  renderSheet(s);
  sheet.showModal();
}

$<HTMLButtonElement>("#agent-sheet-later").addEventListener("click", () => sheet.close());
$<HTMLFormElement>("#agent-sheet-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const ok = $<HTMLButtonElement>("#agent-sheet-ok");
  const err = $<HTMLParagraphElement>("#agent-sheet-error");
  const ids = [...sheet.querySelectorAll<HTMLInputElement>("input:checked:not(:disabled)")].map((b) => b.value);
  ok.disabled = true;
  err.textContent = "";
  const failed: string[] = [];
  for (const id of ids) {
    try {
      await invoke("agent_setup_change", { id, install: true, replace: false });
    } catch (x) {
      failed.push(String(x));
    }
  }
  ok.disabled = false;
  if (failed.length) {
    err.textContent = failed.join(" ");
    return;
  }
  sheet.close();
  // The tool works only once its folder is on PATH: Settings › Agents says how to add it.
  const s = await invoke<Status>("agent_setup_status").catch(() => undefined);
  if (s?.cli.state === "installed" && s.on_path === false) openSettings("agents");
});

// ---------- move to Applications ----------

const moveSheet = $<HTMLDialogElement>("#move-sheet");
const moveLead = $<HTMLParagraphElement>("#move-sheet-lead");
const moveOk = $<HTMLButtonElement>("#move-sheet-ok");
const moveError = $<HTMLParagraphElement>("#move-sheet-error");
/** Set once the person was told an app is already in Applications: the next press replaces it. */
let moveReplacing = false;

/** What `app_move_to_applications` rejects with (app_location.rs `MoveError`). */
type MoveError = { kind: "exists"; version: string | null } | { kind: "no_permission"; message: string } | { kind: "failed"; message: string };

function moveIntro(): string {
  const where =
    appLocation?.reason === "translocated"
      ? "DeviceSteer is running from a temporary copy macOS made of the download."
      : "DeviceSteer is running from its disk image.";
  return `${where} Move it to the Applications folder so coding agents can use it and it can update itself. It reopens from there.`;
}

function showReplace(version: string | null): void {
  moveReplacing = true;
  const old = version ? `DeviceSteer ${version}` : "another copy of DeviceSteer";
  const ours = appVersion ? ` with this one (${appVersion})` : "";
  moveLead.textContent = `The Applications folder already has ${old}. Replace it${ours}? The old one goes to the Trash.`;
  moveOk.textContent = "Replace";
}

function openMoveSheet(): void {
  moveReplacing = false;
  moveLead.textContent = moveIntro();
  moveOk.textContent = "Move to Applications";
  moveOk.disabled = false;
  moveError.textContent = "";
  moveSheet.showModal();
}

$<HTMLButtonElement>("#move-sheet-later").addEventListener("click", () => {
  // Asked again by the next version; until then Settings › Agents keeps the hint and a button.
  pref.set("move-declined", appVersion);
  moveSheet.close();
});
// Settings › Agents' Move button: the sheet belongs to this window (settings_window.rs).
void listen("move-sheet", async () => {
  appLocation ??= await invoke<AppLocation>("app_location").catch(() => undefined);
  if (appLocation?.reason && !moveSheet.open) openMoveSheet();
}).catch(() => {});
$<HTMLFormElement>("#move-sheet-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  if (appLocation?.exists && !moveReplacing) {
    showReplace(appLocation.existing_version);
    return;
  }
  const label = moveOk.textContent;
  moveOk.disabled = true;
  moveOk.textContent = "Moving…";
  moveError.textContent = "";
  try {
    // On success the app quits and the copy in Applications opens, so this doesn't come back.
    await invoke("app_move_to_applications", { replace: moveReplacing });
  } catch (x) {
    moveOk.disabled = false;
    moveOk.textContent = label;
    const err = x as MoveError | string;
    if (typeof err === "object" && err.kind === "exists") showReplace(err.version);
    else if (typeof err === "object" && err.kind === "no_permission") {
      moveError.textContent = "DeviceSteer can’t write to the Applications folder. In Finder, drag DeviceSteer into Applications, then open it from there.";
    } else moveError.textContent = `The move failed: ${typeof err === "object" ? err.message : err}`;
  }
});

/** Offers the move when the app runs from somewhere that won't last; true when it did. */
async function offerMove(): Promise<boolean> {
  try {
    appLocation = await invoke<AppLocation>("app_location");
  } catch {
    return false;
  }
  if (!appLocation.reason) return false;
  if (pref.get("move-declined") !== appVersion) openMoveSheet();
  return true;
}

// ---------- updates ----------

const banner = $<HTMLDivElement>("#update-banner");
let pendingUpdate: Update | undefined;

/** Looks for an update once after launch, quietly: only a found one shows, as the banner. */
async function checkForUpdate(): Promise<void> {
  try {
    const u = await check();
    pendingUpdate = u ?? undefined;
    if (u) {
      $<HTMLParagraphElement>("#update-banner-text").textContent = `DeviceSteer ${u.version} is available.`;
      banner.hidden = false;
    }
  } catch {
    // Offline or GitHub unreachable: Settings › Updates says so when asked.
  }
}

async function installUpdate(): Promise<void> {
  const u = pendingUpdate;
  if (!u) return;
  const b = $<HTMLButtonElement>("#update-install");
  b.disabled = true;
  const text = $<HTMLParagraphElement>("#update-banner-text");
  let total = 0;
  let got = 0;
  try {
    await u.downloadAndInstall((ev) => {
      if (ev.event === "Started") total = ev.data.contentLength ?? 0;
      if (ev.event === "Progress") got += ev.data.chunkLength;
      text.textContent = total ? `Downloading DeviceSteer ${u.version}… ${Math.round((got / total) * 100)}%` : `Downloading DeviceSteer ${u.version}…`;
    });
    // The device daemons are separate processes and keep running; the links to the CLI and the
    // skill follow the app, so they point at the new version once it starts.
    await relaunch();
  } catch (e) {
    text.textContent = `The update failed: ${e}`;
    b.disabled = false;
  }
}

$<HTMLButtonElement>("#update-install").addEventListener("click", () => void installUpdate());
$<HTMLButtonElement>("#update-dismiss").addEventListener("click", () => (banner.hidden = true));

export function initAgents(): void {
  void (async () => {
    appVersion = await invoke<string>("app_version").catch(() => "");
    // A copy that should move can't set up agents; the moved copy offers that when it opens.
    if (!(await offerMove())) await offerOnFirstLaunch();
  })();
  // Development builds aren't released, so there is nothing to update them to.
  if (!import.meta.env.DEV) window.setTimeout(() => void checkForUpdate(), 5000);
}
