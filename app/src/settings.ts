// The Settings window (settings.html, opened by settings_window.rs): the General pane (the
// viewer's language, i18n.ts), the Agents pane, which
// installs or removes the `devicesteer` command and the skill links for the coding agents on this
// Mac (agent_setup.rs does the work and explains the links), and the Updates pane. The main
// window keeps the launch sheets and the update banner (agents.ts).

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { $, el, pref, conflict, conflictText, downloadingText, isMac, isWindows, itemName, noteText, unavailableText, type AppLocation, type Item, type Status } from "./agent-common";
import { LANGUAGES, LANGUAGE_NAMES, errorText, languageSetting, localize, onLanguageChange, setLanguageSetting, t, type Language } from "./i18n";

// Dev only: `settings.html?mock` runs the window against the fake backend (see src/mock.ts).
if (import.meta.env.DEV && new URLSearchParams(location.search).has("mock")) (await import("./mock")).installMock("settings");

// The title and the traffic lights sit over the toolbar (titleBarStyle Overlay); it keeps room.
if (isMac()) document.documentElement.classList.add("mac");

// ---------- panes ----------

const TABS = { general: "prefs.general", agents: "prefs.agents", updates: "prefs.updates" } as const;
type Tab = keyof typeof TABS;
const isTab = (x: unknown): x is Tab => typeof x === "string" && x in TABS;
let shown: Tab = "general";

/** Mac settings windows are titled by the pane they show. */
function showTitle(): void {
  document.title = t(TABS[shown]);
  void getCurrentWindow().setTitle(document.title).catch(() => {});
}

function showTab(tab: Tab): void {
  for (const x of Object.keys(TABS) as Tab[]) {
    $<HTMLButtonElement>(`#tab-${x}`).setAttribute("aria-selected", String(x === tab));
    $<HTMLElement>(`#pane-${x}`).hidden = x !== tab;
  }
  pref.set("settings-tab", tab);
  shown = tab;
  showTitle();
  if (tab === "agents") void refresh();
}

for (const x of Object.keys(TABS) as Tab[]) $<HTMLButtonElement>(`#tab-${x}`).addEventListener("click", () => showTab(x));
// Arrow keys move between the panes, as in any tab list.
$<HTMLDivElement>(".prefs-tabs").addEventListener("keydown", (e) => {
  if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
  const tabs = Object.keys(TABS) as Tab[];
  const at = tabs.findIndex((t) => $<HTMLButtonElement>(`#tab-${t}`).getAttribute("aria-selected") === "true");
  const next = tabs[(at + (e.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length];
  showTab(next);
  $<HTMLButtonElement>(`#tab-${next}`).focus();
});
// Opened again for a given pane while it is open (the main window's first-launch sheet).
void listen<string>("settings-tab", (e) => isTab(e.payload) && showTab(e.payload)).catch(() => {});

// ---------- general ----------

const languagePicker = $<HTMLSelectElement>("#language");

/** The picker: the system's language first, then each language in its own words. */
function renderLanguages(): void {
  const system = new Option(t("prefs.languageSystem"), "system");
  const options = LANGUAGES.map((l) => new Option(LANGUAGE_NAMES[l], l));
  for (const o of options) o.lang = o.value;
  languagePicker.replaceChildren(system, ...options);
  languagePicker.value = languageSetting();
}
languagePicker.addEventListener("change", () => setLanguageSetting(languagePicker.value as Language | "system"));

// ---------- agents ----------

let status: Status | undefined;
let appLocation: AppLocation | undefined;
let busy = false;
/** The item a Replace button was pressed for: the next press confirms. */
let confirming: string | undefined;

const statusLine = $<HTMLParagraphElement>("#agent-status");

function say(text: string, error = false): void {
  statusLine.textContent = text;
  statusLine.classList.toggle("error", error);
}

async function refresh(): Promise<void> {
  if (busy) return;
  try {
    status = await invoke<Status>("agent_setup_status");
    render();
  } catch (e) {
    say(errorText(e), true);
  }
}

async function change(i: Item, install: boolean, replace = false): Promise<boolean> {
  busy = true;
  render();
  try {
    const r = await invoke<{ note: Parameters<typeof noteText>[0] | null }>("agent_setup_change", { id: i.id, install, replace });
    say(r.note ? noteText(r.note) : t(install ? "prefs.installed" : "prefs.removed", { path: i.path }));
    return true;
  } catch (e) {
    say(errorText(e), true);
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
  text.append(el("div", "name", itemName(i)), el("div", "path mono", i.path));
  if (!i.detected && i.state !== "installed") text.append(el("div", "note", t(isWindows() ? "prefs.notFoundPc" : "prefs.notFoundMac")));
  if (conflict(i)) text.append(el("div", "note warn", conflictText(i)));
  if (extra) text.append(extra);
  row.append(text);
  if (conflict(i)) {
    const sure = confirming === i.id;
    const b = el("button", `btn small${sure ? " danger" : ""}`, t(sure ? "prefs.confirmReplace" : "prefs.replace"));
    b.type = "button";
    b.disabled = off || busy;
    b.title = t("prefs.replaceHelp");
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
    sw.setAttribute("aria-label", itemName(i));
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
  $<HTMLDivElement>("#agent-unavailable").hidden = !s.unavailable;
  $<HTMLParagraphElement>("#agent-unavailable-text").textContent = unavailableText(s);
  $<HTMLButtonElement>("#agent-move").hidden = !appLocation?.reason;
  let pathNote: HTMLElement | undefined;
  if (s.cli.state === "installed" && s.on_path === false) {
    pathNote = el(
      "div",
      "note warn",
      isWindows() ? t("prefs.pathWindows", { dir: "%USERPROFILE%\\.local\\bin" }) : t("prefs.pathShell", { dir: "~/.local/bin", line: 'export PATH="$HOME/.local/bin:$PATH"', file: "~/.zshrc" }),
    );
  } else if (s.cli.state === "missing" && !s.unavailable) {
    // A warning once a skill is installed without it: that skill can't do anything.
    const skills = s.agents.some((a) => a.state === "installed");
    pathNote = el("div", `note${skills ? " warn" : ""}`, t("agents.cliNeeded"));
  }
  $<HTMLDivElement>("#agent-cli").replaceChildren(itemRow(s.cli, pathNote));
  $<HTMLDivElement>("#agent-skills").replaceChildren(...s.agents.map((a) => itemRow(a)));
}

// The move sheet is the main window's: it comes forward with the sheet open.
$<HTMLButtonElement>("#agent-move").addEventListener("click", () => void invoke("show_move_sheet").catch((e) => say(errorText(e), true)));

// Coming back to the window: the person may have changed the links in a terminal meanwhile.
window.addEventListener("focus", () => {
  if (!$<HTMLElement>("#pane-agents").hidden) void refresh();
});

// ---------- updates ----------

const updateText = $<HTMLSpanElement>("#update-text");
const checkBtn = $<HTMLButtonElement>("#update-check");
const found = $<HTMLDivElement>("#update-found");
const foundText = $<HTMLParagraphElement>("#update-found-text");
const installBtn = $<HTMLButtonElement>("#update-install");
let pendingUpdate: Update | undefined;
/** What the Updates pane says, kept to say it again in another language. */
let updateState: { text: () => string; found?: () => string } = { text: () => t("prefs.updatesFrom") };

function showUpdateState(): void {
  updateText.textContent = updateState.text();
  if (updateState.found) foundText.textContent = updateState.found();
}

async function checkForUpdate(): Promise<void> {
  checkBtn.disabled = true;
  updateState = { text: () => t("prefs.checking") };
  showUpdateState();
  try {
    const u = await check();
    pendingUpdate = u ?? undefined;
    found.hidden = !u;
    if (u) {
      const version = u.version;
      updateState = { text: () => t("prefs.versionAvailable", { version }), found: () => t("prefs.updateReady", { version }) };
    } else updateState = { text: () => t("prefs.upToDate") };
  } catch (e) {
    updateState = { text: () => t("prefs.checkFailed", { error: String(e) }) };
  } finally {
    showUpdateState();
    checkBtn.disabled = false;
  }
}

async function installUpdate(): Promise<void> {
  const u = pendingUpdate;
  if (!u) return;
  installBtn.disabled = true;
  checkBtn.disabled = true;
  let total = 0;
  let got = 0;
  try {
    await u.downloadAndInstall((ev) => {
      if (ev.event === "Started") total = ev.data.contentLength ?? 0;
      if (ev.event === "Progress") got += ev.data.chunkLength;
      foundText.textContent = downloadingText(u.version, total ? got / total : undefined);
    });
    await relaunch();
  } catch (e) {
    foundText.textContent = t("update.failed", { error: String(e) });
    installBtn.disabled = false;
    checkBtn.disabled = false;
  }
}

checkBtn.addEventListener("click", () => void checkForUpdate());
installBtn.addEventListener("click", () => void installUpdate());

// ---------- start ----------

localize();
renderLanguages();
showUpdateState();
onLanguageChange(() => {
  showTitle();
  renderLanguages();
  showUpdateState();
  render();
});

void (async () => {
  $<HTMLOutputElement>("#app-version").value = await invoke<string>("app_version").catch(() => "");
  appLocation = await invoke<AppLocation>("app_location").catch(() => undefined);
  render();
})();
const asked = new URLSearchParams(location.search).get("tab");
const saved = pref.get("settings-tab");
showTab(isTab(asked) ? asked : isTab(saved) ? saved : "general");
