// What the main window's launch sheets (AgentSheet, MoveSheet) and the Settings window's Agents
// pane say about agent setup: shared wording for the shapes agent_setup.rs returns.

import type { AgentItem, AgentNote, AgentStatus } from "../../backend/types";
import { hasKey, num, t as translate, type Translate } from "../../i18n/i18n";

export const noteText = (n: AgentNote, t: Translate = translate): string => {
  const key = `agents.note.${n.code}`;
  return hasKey(key) ? t(key, { path: n.path }) : n.message;
};

export function unavailableText(s: AgentStatus, t: Translate): string {
  const key = `agents.unavailable.${s.unavailable_code}`;
  return s.unavailable_code && hasKey(key) ? t(key) : (s.unavailable ?? "");
}

/** An item's name: the shell names the command and the shared skills folder in English. */
export function itemName(i: AgentItem, t: Translate): string {
  if (i.id === "cli") return t("agents.cliName");
  if (i.id === "agents") return t("agents.sharedName");
  return i.name;
}

/** The update's download, with how far it got when the size is known. */
export function downloadingText(version: string, fraction?: number): string {
  const t = translate;
  return fraction === undefined
    ? t("update.downloading", { version })
    : t("update.downloadingPercent", { version, percent: num(Math.floor(fraction * 100) / 100, { style: "percent" }) });
}

export const conflict = (i: AgentItem): boolean => i.state === "other_link" || i.state === "not_a_link";

/** What is at the item's path when it isn't ours, in words. */
export function conflictText(i: AgentItem, t: Translate): string {
  if (i.state === "other_link") return i.points_to ? t("agents.conflictLink", { target: i.points_to }) : t("agents.conflictLinkElsewhere");
  if (i.state === "not_a_link") return t(i.is_dir ? "agents.conflictFolder" : "agents.conflictFile");
  return "";
}
