// Screenshot and recording. The daemon records (one recording per device); the Record button
// starts and stops it, and the indicator follows the daemon's `recording` events, so a recording
// the CLI or an agent started shows here too and can be stopped here.

import * as backend from "../../backend/commands";
import type { RecordingEvent } from "../../backend/types";
import { report } from "../../device/report";
import { errorText, t } from "../../i18n/i18n";
import { clock, stamp } from "../../lib/format";
import { screen } from "../../stream/screen";
import { getViewer, setViewer } from "../../store/viewer";
import { hideNoteUnlessError, showNote } from "./notes";

/** Saves the canvas, which holds the last frame at the panel's full resolution, turned upright. */
export async function saveScreenshot(): Promise<void> {
  if (!screen.hasPicture) return;
  try {
    const blob = await screen.png();
    if (!blob) throw t("screenshot.notEncoded");
    const png = new Uint8Array(await blob.arrayBuffer());
    const path = await backend.saveScreenshot(png, stamp());
    showNote(t("screenshot.saved"), { path, icon: "i-camera" });
  } catch (e) {
    showNote(t("screenshot.failed", { error: errorText(e) }), { error: true });
  }
}

/** Starts a recording, or stops the one running. */
export async function toggleRecording(): Promise<void> {
  try {
    if (getViewer().recording) {
      await backend.send("record_stop");
    } else {
      const path = await backend.recordingPath(stamp());
      await backend.send("record_start", { path });
    }
  } catch (e) {
    showNote(errorText(e), { error: true });
  }
}

/** The daemon's `recording` event. */
export function onRecording(e: RecordingEvent): void {
  if (e.recording) {
    setViewer({ recording: { path: e.path, since: performance.now() - (e.seconds ?? 0) * 1000 } });
    hideNoteUnlessError();
  } else {
    setViewer({ recording: undefined });
    if (e.error) showNote(t("record.failed", { error: e.error }), { error: true });
    else {
      setViewer({ recordedPath: e.path });
      showNote(e.summary ? t("record.savedLength", { length: clock(e.summary.seconds) }) : t("record.saved"), { path: e.path });
    }
  }
  report();
}
