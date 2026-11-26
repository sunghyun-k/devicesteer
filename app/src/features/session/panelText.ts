// What the session panel says for each owner: the state and the main button. Shared by the panel
// and the report.

import type { AgentSession } from "../../backend/types";
import type { Translate } from "../../i18n/i18n";

export function panelText(s: AgentSession, t: Translate): { state: string; button: string } {
  switch (s.owner) {
    case "delegated_to_user":
      // The reason is the agent's own words, in whatever language it wrote them.
      return { state: t("session.waitingForYou", { reason: s.reason ?? "" }), button: t("session.done") };
    case "user":
      return { state: t("session.youInControl"), button: t("session.handBack") };
    default:
      return { state: t("session.agentInControl"), button: t("session.takeOver") };
  }
}

/** A session's state in a few words, for the sidebar and the notice. */
export function sessionShort(s: AgentSession, t: Translate): string {
  return t(s.owner === "agent" ? "session.agentInControl" : s.owner === "user" ? "session.youInControl" : "list.sessionWaiting");
}
