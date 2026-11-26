// The pairing card: what to do on the device, the wait or the code, and its buttons.

import { Icon } from "../../components/Icon";
import { useKeepFocus } from "../../components/useKeepFocus";
import { useT } from "../../i18n/react";
import { useViewer } from "../../store/viewer";
import { closePairing, startPairing } from "./pairing";
import { Wait } from "./Wait";
import s from "./SetupCard.module.css";

/** "123456" as "123 456": two groups read and type more easily. */
const spacedCode = (c: string) => c.replace(/^(\d{3})(\d{3})$/, "$1 $2");

export function PairCard() {
  const t = useT();
  const p = useViewer((v) => v.pairing);
  // The buttons change with the phase: the keys stay on the card.
  const focus = useKeepFocus<HTMLDivElement>(p?.phase ?? "");
  if (!p) return null;
  // The steps on the device, in its own words (iOS 27).
  const steps = [
    { title: t("pair.step1.title"), text: t("pair.step1.text") },
    { title: t("pair.step2.title"), text: t("pair.step2.text") },
    { title: t("pair.step3.title"), text: t("pair.step3.text") },
  ];
  return (
    <div {...focus} className={s.pair}>
      <div className={s.head}>
        <h3>{t("pair.heading")}</h3>
        <p>{t("pair.lead")}</p>
      </div>
      <ol className={s.steps}>
        {steps.map((step, i) => {
          // The device connects only after its own passcode was entered (PITFALLS #56): once the
          // code shows, the first two steps are done. While waiting, both may still be ahead.
          const done = (p.phase === "code" && i < 2) || p.phase === "done";
          const now = p.phase === "waiting" ? i < 2 : p.phase === "code" ? i === 2 : false;
          return (
            <li key={i} className={[s.pairStep, done ? s.done : "", now ? s.now : ""].filter(Boolean).join(" ")}>
              <span className={s.mark}>{done ? <Icon name="i-check" /> : String(i + 1)}</span>
              <div className={s.body}>
                <div className={s.title}>{step.title}</div>
                <p className={s.desc}>{step.text}</p>
              </div>
            </li>
          );
        })}
      </ol>
      {p.phase === "waiting" || p.phase === "code" ? (
        <Wait since={p.waitingSince} className={p.phase === "code" ? s.withCode : undefined}>
          {p.phase === "code" && p.code ? (
            <>
              <div className={s.code}>
                <span className={s.codeLabel}>{t("pair.code")}</span>
                <output className={s.digits} aria-label={t("pair.codeAria", { digits: p.code.split("").join(" ") })}>
                  {spacedCode(p.code)}
                </output>
              </div>
              <p className={s.waitMsg} role="status">
                {t("pair.typeCode")}
              </p>
            </>
          ) : (
            <p className={s.waitMsg} role="status">
              {t("pair.shownAs")}
            </p>
          )}
        </Wait>
      ) : p.phase === "done" ? (
        <div className={s.done2} role="status">
          <span className="spinner" />
          <span>{t("pair.paired")}</span>
        </div>
      ) : p.error ? (
        <p className={s.error} role="alert">
          {t.say(p.error)}
        </p>
      ) : null}
      {p.phase === "failed" ? (
        <div className={s.actions}>
          <button className="btn small" type="button" onClick={() => void startPairing()}>
            {t("common.tryAgain")}
          </button>
          <button className="btn small ghost" type="button" onClick={closePairing}>
            {t("common.close")}
          </button>
        </div>
      ) : p.phase !== "done" ? (
        <div className={s.actions}>
          <button className="btn small" type="button" onClick={closePairing}>
            {t("common.cancel")}
          </button>
        </div>
      ) : null}
    </div>
  );
}
