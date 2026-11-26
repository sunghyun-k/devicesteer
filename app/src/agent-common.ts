// What the main window's launch sheets (agents.ts) and the Settings window's Agents pane
// (settings.ts) share: the shapes agent_setup.rs and app_location.rs return, and small helpers.

import { hasKey, num, t } from "./i18n";

export type LinkState =
  | { state: "installed" }
  | { state: "missing" }
  | { state: "other_link"; points_to: string }
  | { state: "not_a_link"; is_dir: boolean };

export type Item = { id: string; name: string; path: string; detected: boolean } & LinkState;

export interface Status {
  /** Why setup can't run from this copy, in English; `unavailable_code` names it when known. */
  unavailable: string | null;
  unavailable_code: string | null;
  cli: Item;
  on_path: boolean | null;
  agents: Item[];
}

/** Where the app runs from (app_location.rs). `reason` is set when it should move. */
export interface AppLocation {
  reason: "disk_image" | "translocated" | null;
  exists: boolean;
  existing_version: string | null;
}

export const $ = <T extends HTMLElement>(sel: string) => document.querySelector<T>(sel)!;

export function el<K extends keyof HTMLElementTagNameMap>(tag: K, className: string, text?: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined) e.textContent = text;
  return e;
}

/** Remembered per viewer; never needed for correctness. Both windows share the origin, so the
 * keys are shared too. */
export const pref = {
  get(key: string): string | null {
    try {
      return localStorage.getItem(`devicesteer.${key}`);
    } catch {
      return null;
    }
  },
  set(key: string, value: string): void {
    try {
      localStorage.setItem(`devicesteer.${key}`, value);
    } catch {
      // Private mode or blocked storage: a sheet may show again, nothing worse.
    }
  },
};

/** What `agent_setup_change` reports when it replaced something (`Note` in crates/agents). */
export interface Note {
  code: string;
  path: string;
  message: string;
}

export const noteText = (n: Note): string => {
  const key = `agents.note.${n.code}`;
  return hasKey(key) ? t(key, { path: n.path }) : n.message;
};

export function unavailableText(s: Status): string {
  const key = `agents.unavailable.${s.unavailable_code}`;
  return s.unavailable_code && hasKey(key) ? t(key) : (s.unavailable ?? "");
}

/** An item's name: the shell names the command and the shared skills folder in English. */
export function itemName(i: Item): string {
  if (i.id === "cli") return t("agents.cliName");
  if (i.id === "agents") return t("agents.sharedName");
  return i.name;
}

/** The update's download, with how far it got when the size is known. */
export function downloadingText(version: string, fraction?: number): string {
  return fraction === undefined
    ? t("update.downloading", { version })
    : t("update.downloadingPercent", { version, percent: num(Math.floor(fraction * 100) / 100, { style: "percent" }) });
}

export const conflict = (i: Item) => i.state === "other_link" || i.state === "not_a_link";

/** What is at the item's path when it isn't ours, in words. */
export function conflictText(i: Item): string {
  if (i.state === "other_link") return i.points_to ? t("agents.conflictLink", { target: i.points_to }) : t("agents.conflictLinkElsewhere");
  if (i.state === "not_a_link") return t(i.is_dir ? "agents.conflictFolder" : "agents.conflictFile");
  return "";
}

/** macOS in the app: the window has an overlay title bar there (main.ts, settings.ts). */
export const isMac = () => /Mac/.test(navigator.platform) && "__TAURI_INTERNALS__" in window;
export const isWindows = () => /Win/.test(navigator.platform);
