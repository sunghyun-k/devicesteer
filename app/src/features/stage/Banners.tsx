// Notices over the stage: the VoiceOver warning, an update, a session that started on another
// device, and the note (where a file went, or what failed).

import { Icon } from "../../components/Icon";
import { deviceName } from "../../device/text";
import { useT } from "../../i18n/react";
import { fileName } from "../../lib/format";
import { isWindows } from "../../lib/platform";
import { useViewer } from "../../store/viewer";
import { UpdateBanner } from "../agents/UpdateBanner";
import { setSetting } from "../inspector/deviceSettings";
import { hideSessionNotice, viewNoticeDevice } from "../session/session";
import { useAgents } from "../agents/agents";
import { hideNote, reveal } from "./notes";
import s from "./Banners.module.css";

function Dismiss({ id, onClick }: { id: string; onClick: () => void }) {
  const t = useT();
  return (
    <button id={id} className="icon-btn small" type="button" aria-label={t("common.dismiss")} title={t("common.dismiss")} onClick={onClick}>
      <Icon name="i-close" />
    </button>
  );
}

function VoiceOverWarning() {
  const t = useT();
  return (
    <div id="vo-warning" className={`${s.banner} ${s.danger}`} role="alert">
      <Icon name="i-alert" />
      <p>
        <strong>{t("voiceover.on")}</strong> <span>{t("voiceover.warning")}</span>
      </p>
      <button id="vo-off" className="btn small" type="button" onClick={() => void setSetting("voiceover", ["off"])}>
        {t("voiceover.turnOff")}
      </button>
    </div>
  );
}

function SessionNotice() {
  const t = useT();
  const notice = useViewer((v) => v.notice);
  if (!notice) return null;
  const head = t("notice.agentStarted", { device: deviceName(notice.device, t) });
  return (
    <div id="session-notice" className={`${s.banner} ${s.notice} ${notice.session.owner !== "agent" ? s.human : ""}`} role="status">
      <span className={s.noticeDot} aria-hidden="true" />
      {/* The goal is the agent's own words, in whatever language it wrote them. */}
      <p id="session-notice-text" title={`${head} ${notice.session.name}`}>
        <strong>{head}</strong> {notice.session.name}
      </p>
      <button id="session-notice-view" className="btn small" type="button" onClick={viewNoticeDevice}>
        {t("notice.view")}
      </button>
      <Dismiss id="session-notice-dismiss" onClick={() => hideSessionNotice()} />
    </div>
  );
}

function NoteBanner() {
  const t = useT();
  const note = useViewer((v) => v.note);
  if (!note) return null;
  let text: React.ReactNode = note.text;
  if (note.path) {
    const [before, after = ""] = note.text.split("{file}");
    text = (
      <>
        {before}
        <strong>{fileName(note.path)}</strong>
        {after}
      </>
    );
  }
  return (
    <div id="note" className={`${s.banner} ${note.error ? s.error : ""}`} role="status">
      <Icon name={note.error ? "i-alert" : (note.icon ?? "i-check")} />
      <p id="note-text" title={note.path ?? note.text}>
        {text}
      </p>
      {note.path ? (
        <button id="note-reveal" className="btn small" type="button" onClick={() => reveal(note.path)}>
          {t(isWindows() ? "note.showInExplorer" : "note.showInFinder")}
        </button>
      ) : null}
      <Dismiss id="note-dismiss" onClick={hideNote} />
    </div>
  );
}

export function Banners() {
  const voiceover = useViewer((v) => v.settings.voiceover === true);
  const notice = useViewer((v) => !!v.notice);
  const note = useViewer((v) => !!v.note);
  const update = useAgents((a) => !!a.update);
  // Takes no room while nothing shows.
  if (!voiceover && !notice && !note && !update) return null;
  return (
    <div id="banners" className={s.banners}>
      {voiceover ? <VoiceOverWarning /> : null}
      <UpdateBanner className={s.banner} />
      <SessionNotice />
      <NoteBanner />
    </div>
  );
}
