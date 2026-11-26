// The note over the stage: where a file went, or what failed.

import * as backend from "../../backend/commands";
import { errorText } from "../../i18n/i18n";
import { getViewer, setViewer, type Note } from "../../store/viewer";

export function showNote(text: string, o: Omit<Note, "text"> = {}): void {
  setViewer({ note: { text, ...o } });
}

export function hideNote(): void {
  setViewer({ note: undefined });
}

/** Shows `path` in Finder (Explorer). */
export function reveal(path: string | undefined): void {
  if (path) backend.reveal(path).catch((e) => showNote(errorText(e), { error: true }));
}

/** Hides a note that isn't an error (a recording that starts makes the last one stale). */
export function hideNoteUnlessError(): void {
  if (!getViewer().note?.error) hideNote();
}
