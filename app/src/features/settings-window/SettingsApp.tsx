// The Settings window: the app's settings, apart from any device, opened from DeviceSteer ›
// Settings… (⌘,) (settings_window.rs). Its state is settingsWindow.ts.

import { useEffect, type KeyboardEvent } from "react";
import type { AgentItem } from "../../backend/types";
import { Icon, IconSprite } from "../../components/Icon";
import { LANGUAGES, LANGUAGE_NAMES, setLanguageSetting, type Language } from "../../i18n/i18n";
import { useLanguageSetting, useT } from "../../i18n/react";
import { isWindows } from "../../lib/platform";
import { conflict, conflictText, itemName, unavailableText } from "../agents/agentText";
import { change, checkForUpdate, installUpdate, moveToApplications, replace, setOpenOnSession, showTab, showTitle, TABS, useSettingsWindow, type Tab } from "./settingsWindow";
import s from "./SettingsApp.module.css";

const ICONS: Record<Tab, string> = { general: "i-gear", agents: "i-terminal", updates: "i-download" };

function GeneralPane() {
  const t = useT();
  const openOnSession = useSettingsWindow((w) => w.openOnSession);
  const setting = useLanguageSetting();
  return (
    <>
      <section className="group">
        <h4>{t("prefs.languageSection")}</h4>
        <div className="row">
          <label htmlFor="language">{t("prefs.language")}</label>
          <span className="select">
            {/* The system's language first, then each language in its own words. */}
            <select id="language" value={setting} onChange={(e) => setLanguageSetting(e.target.value as Language | "system")}>
              <option value="system">{t("prefs.languageSystem")}</option>
              {LANGUAGES.map((l) => (
                <option key={l} value={l} lang={l}>
                  {LANGUAGE_NAMES[l]}
                </option>
              ))}
            </select>
            <Icon name="i-chevron" />
          </span>
        </div>
      </section>
      <p className={s.lead}>{t("prefs.languageNote")}</p>
      <section className="group">
        <h4>{t("prefs.sessionsSection")}</h4>
        <div className="row">
          <label htmlFor="open-on-session">{t("prefs.openOnSession")}</label>
          <input id="open-on-session" type="checkbox" role="switch" checked={openOnSession} onChange={(e) => setOpenOnSession(e.target.checked)} />
        </div>
      </section>
      <p className={s.lead}>{t("prefs.openOnSessionNote")}</p>
    </>
  );
}

/** One switch row: the name, the path, and what is wrong if anything is. */
function ItemRow({ i, extra }: { i: AgentItem; extra?: React.ReactNode }) {
  const t = useT();
  const off = useSettingsWindow((w) => !!w.status?.unavailable);
  const busy = useSettingsWindow((w) => w.busy);
  const sure = useSettingsWindow((w) => w.confirming === i.id);
  const installed = i.state === "installed";
  return (
    <div className="row agent-row" data-id={i.id}>
      <div className="text">
        <div className="name">{itemName(i, t)}</div>
        <div className="path mono">{i.path}</div>
        {!i.detected && !installed ? <div className="note">{t(isWindows() ? "prefs.notFoundPc" : "prefs.notFoundMac")}</div> : null}
        {conflict(i) ? <div className="note warn">{conflictText(i, t)}</div> : null}
        {extra}
      </div>
      {conflict(i) ? (
        <button className={`btn small${sure ? " danger" : ""}`} type="button" disabled={off || busy} title={t("prefs.replaceHelp")} onClick={() => replace(i)}>
          {t(sure ? "prefs.confirmReplace" : "prefs.replace")}
        </button>
      ) : (
        <input
          type="checkbox"
          role="switch"
          aria-label={itemName(i, t)}
          checked={installed}
          disabled={off || busy || (!i.detected && !installed)}
          onChange={(e) => void change(i, e.target.checked)}
        />
      )}
    </div>
  );
}

function AgentsPane() {
  const t = useT();
  const status = useSettingsWindow((w) => w.status);
  const moveReason = useSettingsWindow((w) => w.appLocation?.reason);
  const said = useSettingsWindow((w) => w.said);
  let pathNote: React.ReactNode = null;
  if (status?.cli.state === "installed" && status.on_path === false) {
    pathNote = (
      <div className="note warn">
        {isWindows()
          ? t("prefs.pathWindows", { dir: "%USERPROFILE%\\.local\\bin" })
          : t("prefs.pathShell", { dir: "~/.local/bin", line: 'export PATH="$HOME/.local/bin:$PATH"', file: "~/.zshrc" })}
      </div>
    );
  } else if (status?.cli.state === "missing" && !status.unavailable) {
    // A warning once a skill is installed without it: that skill can't do anything.
    const skills = status.agents.some((a) => a.state === "installed");
    pathNote = <div className={`note${skills ? " warn" : ""}`}>{t("agents.cliNeeded")}</div>;
  }
  return (
    <>
      <p className={s.lead}>{t("prefs.agentsLead")}</p>
      {status?.unavailable ? (
        <div id="agent-unavailable" className={s.note}>
          <p id="agent-unavailable-text">{unavailableText(status, t)}</p>
          {moveReason ? (
            <button id="agent-move" className="btn small" type="button" onClick={moveToApplications}>
              {t("prefs.moveToApplications")}
            </button>
          ) : null}
        </div>
      ) : null}
      <section className="group agents">
        <h4>{t("prefs.commandLineTool")}</h4>
        <div id="agent-cli">{status ? <ItemRow i={status.cli} extra={pathNote} /> : null}</div>
      </section>
      <section className="group agents">
        <h4>{t("prefs.agentSkills")}</h4>
        <div id="agent-skills">
          {status?.agents.map((a) => (
            <ItemRow key={a.id} i={a} />
          ))}
        </div>
      </section>
      <p id="agent-status" className={`${s.status} ${said.error ? s.error : ""}`} role="status">
        {said.text}
      </p>
    </>
  );
}

function UpdatesPane() {
  const t = useT();
  const w = useSettingsWindow();
  return (
    <>
      <section className="group info">
        <h4>{t("prefs.updates")}</h4>
        <div className="row">
          <span>{t("prefs.version")}</span>
          <output id="app-version">{w.version}</output>
        </div>
        <div className="row">
          <span id="update-text" className={s.updateText}>
            {t.say(w.updateText)}
          </span>
          <button id="update-check" className="btn small" type="button" disabled={w.checking || w.installing} onClick={() => void checkForUpdate()}>
            {t("prefs.checkNow")}
          </button>
        </div>
      </section>
      {w.update ? (
        <div id="update-found" className={s.note}>
          <p id="update-found-text">{t.say(w.foundText)}</p>
          <button id="update-install" className="btn small" type="button" disabled={w.installing} onClick={() => void installUpdate()}>
            {t("prefs.installRestart")}
          </button>
        </div>
      ) : null}
    </>
  );
}

export function SettingsApp() {
  const t = useT();
  const tab = useSettingsWindow((w) => w.tab);
  // The title names the pane, in the language of the moment.
  useEffect(() => showTitle(), [t]);
  const tabs = Object.keys(TABS) as Tab[];
  // Arrow keys move between the panes, as in any tab list.
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    const next = tabs[(tabs.indexOf(tab) + (e.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length];
    showTab(next);
    document.getElementById(`tab-${next}`)?.focus();
  };
  return (
    <>
      <IconSprite />
      <div id="prefs" className={s.prefs}>
        {/* A toolbar of panes under the title, like a Mac settings window. */}
        <header className={s.head} data-tauri-drag-region>
          <div className={s.tabs} role="tablist" aria-label={t("prefs.title")} data-tauri-drag-region onKeyDown={onKeyDown}>
            {tabs.map((x) => (
              <button key={x} id={`tab-${x}`} role="tab" type="button" aria-selected={x === tab} aria-controls={`pane-${x}`} onClick={() => showTab(x)}>
                <Icon name={ICONS[x]} />
                <span>{t(TABS[x])}</span>
              </button>
            ))}
          </div>
        </header>
        <div id="pane-general" className="pane" role="tabpanel" aria-labelledby="tab-general" hidden={tab !== "general"}>
          <GeneralPane />
        </div>
        <div id="pane-agents" className="pane" role="tabpanel" aria-labelledby="tab-agents" hidden={tab !== "agents"}>
          <AgentsPane />
        </div>
        <div id="pane-updates" className="pane" role="tabpanel" aria-labelledby="tab-updates" hidden={tab !== "updates"}>
          <UpdatesPane />
        </div>
      </div>
    </>
  );
}
