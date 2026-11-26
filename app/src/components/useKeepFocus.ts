// Keeps the keys on a card whose buttons change: when `key` changes and the card had focus (its
// focused button may have just been replaced, which leaves the focus on the page's body), its
// first button takes it.

import { useEffect, useRef, type FocusEvent } from "react";

export function useKeepFocus<T extends HTMLElement>(key: string) {
  const root = useRef<T>(null);
  const had = useRef(false);
  useEffect(() => {
    const r = root.current;
    if (!r || !had.current) return;
    if (r.contains(document.activeElement) && document.activeElement?.tagName === "BUTTON") return;
    if (document.activeElement === document.body || r.contains(document.activeElement)) r.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
  }, [key]);
  return {
    ref: root,
    onFocus: () => {
      had.current = true;
    },
    onBlur: (e: FocusEvent) => {
      // Focus that moved elsewhere on the page; a button that went away gives no `relatedTarget`.
      if (e.relatedTarget && !root.current?.contains(e.relatedTarget as Node)) had.current = false;
    },
  };
}
