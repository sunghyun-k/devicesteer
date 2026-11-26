// Offered once, on the first launch of an installed app: installs the `devicesteer` command and
// the skill for the coding agents found on this computer (agents.ts).

import { useState } from "react";
import type { AgentItem } from "../../backend/types";
import { Sheet } from "../../components/Sheet";
import { useT } from "../../i18n/react";
import { conflict, conflictText, itemName } from "./agentText";
import { closeAgentSheet, setUpAgents, useAgents } from "./agents";

function Row({ i, checked, onChange }: { i: AgentItem; checked: boolean; onChange: (on: boolean) => void }) {
  const t = useT();
  const installed = i.state === "installed";
  const blocked = conflict(i);
  return (
    <label className="row agent-row">
      <div className="text">
        <div className="name">{i.id === "cli" ? t("agents.cliTool") : t("agents.skillFor", { name: itemName(i, t) })}</div>
        <div className="path mono">{i.path}</div>
        {installed ? <div className="note">{t("agents.alreadySetUp")}</div> : null}
        {blocked ? <div className="note warn">{`${conflictText(i, t)} ${t("agents.replaceInSettings")}`}</div> : null}
        {!installed && !blocked && i.id === "cli" && !checked ? <div className="note warn">{t("agents.cliNeeded")}</div> : null}
      </div>
      <input type="checkbox" role="switch" value={i.id} checked={installed || (!blocked && checked)} disabled={installed || blocked} onChange={(e) => onChange(e.target.checked)} />
    </label>
  );
}

export function AgentSheet() {
  const t = useT();
  const sheet = useAgents((a) => a.sheet);
  // What is unchecked: everything that can be installed starts checked.
  const [off, setOff] = useState<string[]>([]);
  const items = sheet ? [sheet.status.cli, ...sheet.status.agents.filter((a) => a.detected)] : [];
  const ids = items.filter((i) => i.state !== "installed" && !conflict(i) && !off.includes(i.id)).map((i) => i.id);
  return (
    <Sheet id="agent-sheet" titleId="agent-sheet-title" open={!!sheet} onSubmit={() => void setUpAgents(ids)} onCancel={closeAgentSheet}>
      <h3 id="agent-sheet-title">{t("agentSheet.title")}</h3>
      <p className="lead">{t("agentSheet.lead")}</p>
      <div id="agent-sheet-list" className="group agents">
        {items.map((i) => (
          <Row key={i.id} i={i} checked={!off.includes(i.id)} onChange={(on) => setOff((o) => (on ? o.filter((x) => x !== i.id) : [...o, i.id]))} />
        ))}
      </div>
      <p id="agent-sheet-error" className="error-text">
        {sheet?.error}
      </p>
      <div className="actions">
        <button id="agent-sheet-later" className="btn ghost" type="button" onClick={closeAgentSheet}>
          {t("common.notNow")}
        </button>
        <button id="agent-sheet-ok" className="btn primary" type="submit" disabled={sheet?.busy}>
          {t("agentSheet.setUp")}
        </button>
      </div>
    </Sheet>
  );
}
