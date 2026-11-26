// The setup card: the three steps in order, the current one with what to do and its button.

import { SETUP_ORDER, type DeviceInfo, type SetupStep } from "../../backend/types";
import { Icon } from "../../components/Icon";
import { useKeepFocus } from "../../components/useKeepFocus";
import { setupTitle } from "../../device/text";
import { useT } from "../../i18n/react";
import { useViewer } from "../../store/viewer";
import { openPairing } from "./pairing";
import { cancelSetup, runSetup, setupMessage, setupStates } from "./setup";
import { Wait } from "./Wait";
import s from "./SetupCard.module.css";

/** What each step asks of the person, and the button that starts it. */
function useStepText() {
  const t = useT();
  return (step: SetupStep, d: DeviceInfo): { text: string; button?: string; cancel?: boolean; action?: () => void } => {
    switch (step) {
      case "trust":
        // Trust goes over USB only; our own Wi-Fi pairing needs no cable (iOS 27 and later).
        if (!d.transports.includes("usb")) return { text: t("setup.trust.noCable"), button: t("sidebar.pairWifi"), action: openPairing };
        return { text: t("setup.trust.text"), button: t("setup.trust.button"), cancel: true };
      case "developer_mode":
        return { text: t("setup.developerMode.text"), button: t("setup.developerMode.button"), cancel: true };
      case "disk_image":
        return { text: t("setup.diskImage.text"), button: t("setup.diskImage.button") };
    }
  };
}

export function SetupCard() {
  const t = useT();
  const card = useViewer((v) => v.setup);
  const gone = useViewer((v) => !!v.gone);
  const stepText = useStepText();
  // Keep focus on the step's button when it changes (Cancel while running, the step's button
  // after): the old one is gone.
  const focus = useKeepFocus<HTMLDivElement>(`${card?.run?.step ?? ""}|${card?.error?.step ?? ""}`);
  if (!card) return null;
  const d = card.device;
  const states = setupStates(card);
  return (
    <div {...focus}>
      <div className={s.head}>
        <h3>{t("setup.heading")}</h3>
        <p>{t("setup.lead")}</p>
      </div>
      <ol className={s.steps}>
        {SETUP_ORDER.map((step, i) => {
          const state = states[step] ?? "blocked";
          const isRunning = card.run?.step === step;
          const st = stepText(step, d);
          const p = isRunning ? card.run?.progress : undefined;
          const err = card.error?.step === step ? card.error : undefined;
          return (
            <li key={step} className={[s.step, s[state], isRunning ? s.running : ""].filter(Boolean).join(" ")} data-state={state}>
              <span className={s.mark}>{isRunning ? <span className="spinner" /> : state === "done" ? <Icon name="i-check" /> : String(i + 1)}</span>
              <div className={s.body}>
                <div className={s.title}>{setupTitle(step, t)}</div>
                {state === "needed" ? (
                  <>
                    {/* While it runs, the step's own progress says what to do now (it repeats the
                        description at first, then follows the device: restarting, confirm after
                        the restart). */}
                    {!isRunning ? <p className={s.desc}>{st.text}</p> : null}
                    {isRunning && p?.waiting_for_user ? (
                      // Waiting for the person, maybe for minutes: say so plainly, with what to do
                      // now and the time left, so the card doesn't look stuck.
                      <Wait since={card.run?.waitingSince}>
                        <p className={s.waitMsg} role="status">
                          {setupMessage(p.message)}
                        </p>
                      </Wait>
                    ) : isRunning ? (
                      <div className={s.progress}>
                        <span>{p?.message ?? t("setup.starting")}</span>
                        {p?.fraction !== undefined ? <div className={s.bar} style={{ "--f": String(Math.max(0, Math.min(1, p.fraction))) } as React.CSSProperties} /> : null}
                      </div>
                    ) : null}
                    {err ? <p className={s.error}>{t.say(err.text)}</p> : null}
                    {isRunning && st.cancel ? (
                      <div className={s.actions}>
                        <button className="btn small" type="button" onClick={cancelSetup}>
                          {t("common.cancel")}
                        </button>
                      </div>
                    ) : !isRunning && st.button ? (
                      <div className={s.actions}>
                        {/* Nothing to reach while the device is away. */}
                        <button className="btn small" type="button" disabled={gone && !st.action} onClick={() => (st.action ? st.action() : void runSetup(step))}>
                          {err && !st.action ? t("common.tryAgain") : st.button}
                        </button>
                      </div>
                    ) : null}
                  </>
                ) : null}
              </div>
            </li>
          );
        })}
      </ol>
    </div>
  );
}
