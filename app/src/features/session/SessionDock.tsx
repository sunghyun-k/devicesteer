// The session slot under the screen: who controls the device, like a screen-sharing indicator (a
// tinted strip with a live dot, the state, the agent's goal and the actions), or "No agent
// session". It always takes the same room.

import { useLayoutEffect, useRef } from "react";
import { useT } from "../../i18n/react";
import { activeSession, useViewer } from "../../store/viewer";
import { panelText } from "./panelText";
import { controlSession, panelAction } from "./session";
import s from "./SessionDock.module.css";

export function SessionDock() {
  const t = useT();
  const session = useViewer((v) => activeSession(v));
  const live = useViewer((v) => v.stage === "live");
  const nudge = useViewer((v) => v.nudge);
  const primary = useRef<HTMLButtonElement>(null);

  // Restart the nudge for every press kept from the device, not only the first.
  useLayoutEffect(() => {
    const b = primary.current;
    if (!b || !nudge) return;
    b.style.animation = "none";
    void b.offsetWidth;
    b.style.animation = "";
  }, [nudge]);

  if (!session) {
    return (
      <div id="session-dock" className={s.dock}>
        <p id="session-idle" className={`${s.idle} ${live ? "" : s.away}`}>
          <span className={s.dotIdle} aria-hidden="true" />
          <span>{t("session.none")}</span>
        </p>
      </div>
    );
  }
  const p = panelText(session, t);
  const human = session.owner !== "agent";
  return (
    <div id="session-dock" className={s.dock}>
      <div id="session-panel" className={`${s.panel} ${human ? s.human : ""}`} role="region" aria-label={t("session.region")}>
        <span className={s.dot} aria-hidden="true" />
        <div className={s.text}>
          <span id="session-state" className={s.state} role="status" title={p.state}>
            {p.state}
          </span>
          <span id="session-name" className={s.name}>
            {session.name}
          </span>
        </div>
        <button id="session-stop" className={`btn small ghost ${s.stop}`} type="button" onClick={() => controlSession("stop")}>
          {t("session.stop")}
        </button>
        <button id="session-primary" ref={primary} className={`btn small ${nudge ? s.nudge : ""}`} type="button" onClick={() => controlSession(panelAction(session))}>
          {p.button}
        </button>
      </div>
    </div>
  );
}
