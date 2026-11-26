// Which computer the page runs on. Read after the mock (if any) is installed: it sets up the Tauri
// internals that tell the app from a plain browser.

/** macOS in the app: the windows have an overlay title bar there and the traffic lights sit over
 * the top-left corner; elsewhere the window keeps its frame. */
export const isMac = (): boolean => /Mac/.test(navigator.platform) && "__TAURI_INTERNALS__" in window;
export const isWindows = (): boolean => /Win/.test(navigator.platform);

/** The viewer's shortcut modifier is ⌘ on macOS, Ctrl elsewhere (`cmdKey`). On Windows the
 * Windows key (what the page sees as ⌘) belongs to the OS: Win+K opens Cast, Win+V the clipboard
 * history. */
export const MAC_KEYS = /Mac/.test(navigator.platform);

export const cmdKey = (e: KeyboardEvent): boolean => (MAC_KEYS ? e.metaKey : e.ctrlKey && !e.metaKey);

/** A field of the page has the keys. */
export function editing(): boolean {
  const a = document.activeElement;
  return (
    a instanceof HTMLInputElement ||
    a instanceof HTMLTextAreaElement ||
    a instanceof HTMLSelectElement ||
    (a instanceof HTMLElement && a.isContentEditable)
  );
}
