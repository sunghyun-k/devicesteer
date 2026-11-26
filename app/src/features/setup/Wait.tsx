// The box a step shows while it waits for the person holding the device: a live dot, the time
// left (ticking each second), and what to do now. Shared by the setup and pairing cards.

import { useEffect, useState, type ReactNode } from "react";
import { useT } from "../../i18n/react";
import { timeLeft } from "./setup";
import s from "./SetupCard.module.css";

/** `performance.now()`, once a second while `on`. Only the time text changes each second: the
 * pulse and the spinner keep their elements and their animations. */
function useNow(on: boolean): number {
  const [now, setNow] = useState(() => performance.now());
  useEffect(() => {
    if (!on) return;
    const timer = window.setInterval(() => setNow(performance.now()), 1000);
    return () => window.clearInterval(timer);
  }, [on]);
  return now;
}

export function Wait({ since, className, children }: { since: number | undefined; className?: string; children: ReactNode }) {
  const t = useT();
  const now = useNow(since !== undefined);
  const left = timeLeft(since, Math.max(now, since ?? 0));
  return (
    <div className={`${s.wait} ${className ?? ""}`}>
      <div className={s.waitHead}>
        <span className={s.pulse} />
        <span className={s.waitLabel}>{t("setup.waitingOnDevice")}</span>
        <span className={`${s.waitTime} ${left?.low ? s.low : ""}`}>{left ? t("setup.timeLeft", { time: left.time }) : ""}</span>
      </div>
      {children}
    </div>
  );
}
