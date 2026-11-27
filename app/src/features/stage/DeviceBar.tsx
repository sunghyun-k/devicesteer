// The device bar under the screen: the hardware buttons, then screenshot and record.

import { useEffect, useState } from "react";
import { Icon } from "../../components/Icon";
import { send } from "../../device/connection";
import { t as translate } from "../../i18n/i18n";
import { useT } from "../../i18n/react";
import { clock, fileName } from "../../lib/format";
import { humanHasDevice, useViewer } from "../../store/viewer";
import { saveScreenshot, toggleRecording } from "./capture";
import { showNote } from "./notes";
import s from "./Bottom.module.css";

const BUTTONS = [
  ["home", "i-home", "deviceBar.home", "deviceBar.homeHelp"],
  ["lock", "i-power", "deviceBar.lock", "deviceBar.lockHelp"],
  ["volume_down", "i-vol-down", "deviceBar.volumeDown", "deviceBar.volumeDownHelp"],
  ["volume_up", "i-vol-up", "deviceBar.volumeUp", "deviceBar.volumeUpHelp"],
] as const;

/** Press a hardware button, unless the agent has the device: the disabled button can't be
 * clicked, but the session can change between render and click, and this is the one way in. */
function press(name: string): void {
  if (!humanHasDevice()) {
    showNote(translate("session.blockedHint"), { error: true });
    return;
  }
  void send("button", { name }).catch(() => {});
}

/** A recording's running time, ticking twice a second. */
function useElapsed(since: number | undefined): string {
  const [now, setNow] = useState(() => performance.now());
  useEffect(() => {
    if (since === undefined) return;
    const tick = () => setNow(performance.now());
    const first = requestAnimationFrame(tick);
    const timer = window.setInterval(tick, 500);
    return () => {
      cancelAnimationFrame(first);
      window.clearInterval(timer);
    };
  }, [since]);
  return since === undefined ? "" : clock(Math.max(0, now - since) / 1000);
}

function RecordButton() {
  const t = useT();
  const recording = useViewer((v) => v.recording);
  const elapsed = useElapsed(recording?.since);
  const [busy, setBusy] = useState(false);
  const onClick = async () => {
    setBusy(true);
    await toggleRecording();
    setBusy(false);
  };
  return (
    <button
      id="record"
      className={`${s.tool} ${s.record} ${recording ? s.recording : ""}`}
      type="button"
      aria-pressed={!!recording}
      aria-label={recording ? t("record.stop") : t("record.label")}
      title={recording ? t("record.stopFile", { file: fileName(recording.path) }) : t("record.help")}
      disabled={busy}
      onClick={() => void onClick()}
    >
      <Icon name={recording ? "i-stop" : "i-record"} />
      <span id="record-label" className={s.recordLabel}>
        {recording ? elapsed : t("record.label")}
      </span>
    </button>
  );
}

function ShotButton() {
  const t = useT();
  const [busy, setBusy] = useState(false);
  const onClick = async () => {
    setBusy(true);
    await saveScreenshot();
    setBusy(false);
  };
  return (
    <button id="shot" className={s.tool} type="button" title={t("deviceBar.screenshotHelp")} disabled={busy} onClick={() => void onClick()}>
      <Icon name="i-camera" />
      <span>{t("deviceBar.screenshot")}</span>
    </button>
  );
}

export function DeviceBar() {
  const t = useT();
  const live = useViewer((v) => v.stage === "live");
  // Like touches and keys, the hardware buttons wait while the agent has the device; screenshot
  // and record don't touch it, so they stay.
  const blocked = useViewer((v) => !humanHasDevice(v));
  return (
    <div id="controls" className={`${s.controls} ${live ? "" : s.away}`}>
      {/* A toolbar, not a field: the hardware buttons, a separator, then screenshot and record, all
          as the same plain buttons. */}
      <div id="device-bar" className={s.bar} role="toolbar" aria-label={t("deviceBar.label")}>
        <div className={s.row} role="group" aria-label={t("deviceBar.hardware")}>
          {BUTTONS.map(([name, icon, label, help]) => (
            <button
              key={name}
              className={s.tool}
              type="button"
              data-button={name}
              title={blocked ? t("session.blockedHint") : t(help)}
              disabled={blocked}
              onClick={() => press(name)}
            >
              <Icon name={icon} />
              <span>{t(label)}</span>
            </button>
          ))}
        </div>
        <span className={s.sep} aria-hidden="true" />
        <div className={s.row} role="group" aria-label={t("deviceBar.capture")}>
          <ShotButton />
          <RecordButton />
        </div>
      </div>
    </div>
  );
}
