// The inspector's Agents pane and the first-launch sheet: install the `devicesteer` command and
// the DeviceSteer skill for the coding agents on this Mac (app/src-tauri/src/agent_setup.rs does
// the work and explains the links), plus updates from GitHub Releases.

import { invoke } from "@tauri-apps/api/core";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

type LinkState =
  | { state: "installed" }
  | { state: "missing" }
  | { state: "other_link"; points_to: string }
  | { state: "not_a_link"; is_dir: boolean };

type Item = { id: string; name: string; path: string; detected: boolean } & LinkState;

interface Status {
  unavailable: string | null;
  cli: Item;
  on_path: boolean | null;
  agents: Item[];
}

const $ = <T extends HTMLElement>(sel: string) => document.querySelector<T>(sel)!;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className: string, text?: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined) e.textContent = text;
  return e;
}

/** Remembered per viewer; never needed for correctness. */
const pref = {
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
      // Private mode or blocked storage: the sheet may show again, nothing worse.
    }
  },
};

let status: Status | undefined;
let busy = false;
/** The item a Replace button was pressed for: the next press confirms. */
let confirming: string | undefined;

const statusLine = $<HTMLParagraphElement>("#agent-status");

function say(text: string, error = false): void {
  statusLine.textContent = text;
  statusLine.classList.toggle("error", error);
}

const conflict = (i: Item) => i.state === "other_link" || i.state === "not_a_link";

/** What is at the item's path when it isn't ours, in words. */
function conflictText(i: Item): string {
  if (i.state === "other_link") return `A link to ${i.points_to || "somewhere else"} is already there.`;
  if (i.state === "not_a_link") return `A ${i.is_dir ? "folder" : "file"} is already there. Replacing it moves it aside.`;
  return "";
}

async function refresh(): Promise<void> {
  try {
    status = await invoke<Status>("agent_setup_status");
    render();
  } catch (e) {
    say(String(e), true);
  }
}

async function change(i: Item, install: boolean, replace = false): Promise<boolean> {
  busy = true;
  render();
  try {
    const r = await invoke<{ note: string | null }>("agent_setup_change", { id: i.id, install, replace });
    say(r.note ?? (install ? `Installed ${i.path}.` : `Removed ${i.path}.`));
    return true;
  } catch (e) {
    say(String(e), true);
    return false;
  } finally {
    busy = false;
    confirming = undefined;
    await refresh();
  }
}

/** One switch row: the name, the path, and what is wrong if anything is. */
function itemRow(i: Item, extra?: HTMLElement): HTMLDivElement {
  const off = !!status?.unavailable;
  const row = el("div", "row agent-row");
  row.dataset.id = i.id;
  const text = el("div", "text");
  text.append(el("div", "name", i.name), el("div", "path mono", i.path));
  if (!i.detected && i.state !== "installed") text.append(el("div", "note", "Not found on this Mac"));
  if (conflict(i)) text.append(el("div", "note warn", conflictText(i)));
  if (extra) text.append(extra);
  row.append(text);
  if (conflict(i)) {
    const sure = confirming === i.id;
    const b = el("button", `btn small${sure ? " danger" : ""}`, sure ? "Confirm Replace" : "Replace…");
    b.type = "button";
    b.disabled = off || busy;
    b.title = "Replace what is there with DeviceSteer’s link";
    b.addEventListener("click", () => {
      if (!sure) {
        confirming = i.id;
        render();
      } else void change(i, true, true);
    });
    row.append(b);
  } else {
    const sw = el("input", "");
    sw.type = "checkbox";
    sw.setAttribute("role", "switch");
    sw.setAttribute("aria-label", i.name);
    sw.checked = i.state === "installed";
    sw.disabled = off || busy || (!i.detected && !sw.checked);
    sw.addEventListener("change", () => void change(i, sw.checked));
    row.append(sw);
  }
  return row;
}

function render(): void {
  const s = status;
  if (!s) return;
  const note = $<HTMLParagraphElement>("#agent-unavailable");
  note.hidden = !s.unavailable;
  note.textContent = s.unavailable ?? "";
  let pathNote: HTMLElement | undefined;
  if (s.cli.state === "installed" && s.on_path === false) {
    pathNote = el("div", "note warn", "~/.local/bin isn’t on your shell’s PATH. Add it to your shell profile, e.g. export PATH=\"$HOME/.local/bin:$PATH\" in ~/.zshrc, then open a new terminal.");
  }
  $<HTMLDivElement>("#agent-cli").replaceChildren(itemRow(s.cli, pathNote));
  $<HTMLDivElement>("#agent-skills").replaceChildren(...s.agents.map((a) => itemRow(a)));
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
      text.append(el("div", "note warn", `${conflictText(i)} Replace it from the Agents pane if you want.`));
    } else box.checked = true;
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
  status = s;
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
  await refresh();
  if (failed.length) {
    err.textContent = failed.join(" ");
    return;
  }
  sheet.close();
  if (status?.cli.state === "installed" && status.on_path === false) {
    say("~/.local/bin isn’t on your shell’s PATH: see Command-Line Tool above.", true);
    $<HTMLButtonElement>("#tab-app").click();
  }
});

// ---------- updates ----------

const banner = $<HTMLDivElement>("#update-banner");
const updateText = $<HTMLSpanElement>("#update-text");
const checkBtn = $<HTMLButtonElement>("#update-check");
let pendingUpdate: Update | undefined;

async function checkForUpdate(quiet: boolean): Promise<void> {
  checkBtn.disabled = true;
  if (!quiet) updateText.textContent = "Checking…";
  try {
    const u = await check();
    pendingUpdate = u ?? undefined;
    if (u) {
      updateText.textContent = `Version ${u.version} is available.`;
      $<HTMLParagraphElement>("#update-banner-text").textContent = `DeviceSteer ${u.version} is available.`;
      banner.hidden = false;
    } else if (!quiet) updateText.textContent = "DeviceSteer is up to date.";
  } catch (e) {
    if (!quiet) updateText.textContent = `Couldn’t check for updates: ${e}`;
  } finally {
    checkBtn.disabled = false;
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

checkBtn.addEventListener("click", () => void checkForUpdate(false));
$<HTMLButtonElement>("#update-install").addEventListener("click", () => void installUpdate());
$<HTMLButtonElement>("#update-dismiss").addEventListener("click", () => (banner.hidden = true));

/** Called by main.ts when the Agents pane shows. */
export function showAgentsPane(): void {
  if (!busy) void refresh();
}

export function initAgents(): void {
  void invoke<string>("app_version")
    .then((v) => ($<HTMLOutputElement>("#app-version").value = v))
    .catch(() => {});
  void offerOnFirstLaunch();
  // Development builds aren't released, so there is nothing to update them to.
  if (!import.meta.env.DEV) window.setTimeout(() => void checkForUpdate(true), 5000);
}
