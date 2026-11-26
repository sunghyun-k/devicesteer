// Offered at launch when the app runs from its disk image or a Gatekeeper copy (agents.ts), and
// from Settings › Agents' Move button.

import { Sheet } from "../../components/Sheet";
import { useT } from "../../i18n/react";
import { closeMoveSheet, declineMove, moveApp, useAgents } from "./agents";

export function MoveSheet() {
  const t = useT();
  const move = useAgents((a) => a.move);
  const reason = useAgents((a) => a.appLocation?.reason);
  const appVersion = useAgents((a) => a.appVersion);
  let lead = `${t(reason === "translocated" ? "move.fromTranslocated" : "move.fromDiskImage")} ${t("move.why")}`;
  const r = move?.replacing;
  if (r) {
    if (!r.version) lead = appVersion ? t("move.replaceOtherWithThis", { version: appVersion }) : t("move.replaceOther");
    else lead = appVersion ? t("move.replaceVersionWithThis", { old: r.version, version: appVersion }) : t("move.replaceVersion", { old: r.version });
  }
  return (
    <Sheet id="move-sheet" titleId="move-sheet-title" open={!!move} onSubmit={() => void moveApp()} onCancel={closeMoveSheet}>
      <h3 id="move-sheet-title">{t("move.title")}</h3>
      <p id="move-sheet-lead" className="lead">
        {lead}
      </p>
      <p id="move-sheet-error" className="error-text">
        {move ? t.say(move.error) : ""}
      </p>
      <div className="actions">
        <button id="move-sheet-later" className="btn ghost" type="button" onClick={declineMove}>
          {t("common.notNow")}
        </button>
        <button id="move-sheet-ok" className="btn primary" type="submit" disabled={move?.busy}>
          {move?.busy ? t("move.moving") : r ? t("move.replace") : t("move.ok")}
        </button>
      </div>
    </Sheet>
  );
}
