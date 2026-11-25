// What the main window's launch sheets (agents.ts) and the Settings window's Agents pane
// (settings.ts) share: the shapes agent_setup.rs and app_location.rs return, and small helpers.

export type LinkState =
  | { state: "installed" }
  | { state: "missing" }
  | { state: "other_link"; points_to: string }
  | { state: "not_a_link"; is_dir: boolean };

export type Item = { id: string; name: string; path: string; detected: boolean } & LinkState;

export interface Status {
  unavailable: string | null;
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

export const CLI_NEEDED = "Agents need the command-line tool to operate devices. Without it, the skills won’t work.";

export const conflict = (i: Item) => i.state === "other_link" || i.state === "not_a_link";

/** What is at the item's path when it isn't ours, in words. */
export function conflictText(i: Item): string {
  if (i.state === "other_link") return `A link to ${i.points_to || "somewhere else"} is already there.`;
  if (i.state === "not_a_link") return `A ${i.is_dir ? "folder" : "file"} is already there. Replacing it moves it aside.`;
  return "";
}

/** macOS in the app: the window has an overlay title bar there (main.ts, settings.ts). */
export const isMac = () => /Mac/.test(navigator.platform) && "__TAURI_INTERNALS__" in window;
