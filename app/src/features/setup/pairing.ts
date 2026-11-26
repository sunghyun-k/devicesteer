// Pairing a device over Wi-Fi: no cable and no device to select yet, so the card takes the stage
// on its own. This computer offers itself on the local network while the card waits
// (`setup_pair_wifi`); the person starts pairing on the device, enters the device's own passcode,
// then the code shown here.

import * as backend from "../../backend/commands";
import type { DeviceInfo } from "../../backend/types";
import { detach, select } from "../../device/connection";
import { report } from "../../device/report";
import { msg, type Text } from "../../i18n/i18n";
import { closeSidebarIfNarrow, getViewer, setSettingsMessage, setStage, setViewer, type Pairing } from "../../store/viewer";
import { loadDevices } from "../devices/deviceList";
import { SETUP_WAIT_S, setupErrorText, waitText } from "./setup";

/** Bumped when the card is put away or pairing starts again, so a late reply doesn't redraw it. */
let pairGen = 0;
/** The device selected before the card opened: Cancel goes back to it. */
let pairBack: DeviceInfo | undefined;

const pairing = (): Pairing | undefined => getViewer().pairing;
const setPairing = (p: Pairing | undefined) => setViewer({ pairing: p });
export const pairRunning = (p = pairing()): boolean => p?.phase === "waiting" || p?.phase === "code";

/** Opens the pairing card and starts pairing. The selected device is let go of first: the stage
 * has room for one thing. */
export function openPairing(): void {
  if (pairing()) {
    document.querySelector<HTMLButtonElement>("#intro-card button")?.focus();
    return;
  }
  const back = getViewer().selected;
  // Not awaited: all but its last call (letting go of the daemon) is done when it returns, and
  // `pairing` must be set at once so a device list that comes back meanwhile leaves the card.
  void detach();
  setViewer({ gone: undefined, selected: undefined, settingsDisabled: true });
  pairBack = back;
  void startPairing();
  setStage("pair");
  closeSidebarIfNarrow();
  setSettingsMessage(msg("settings.afterPairing"));
}

export async function startPairing(): Promise<void> {
  const gen = ++pairGen;
  setPairing({ phase: "waiting" });
  report();
  try {
    const st = await backend.setupPairWifi(SETUP_WAIT_S, (p) => {
      const now = pairing();
      if (gen !== pairGen || !pairRunning(now)) return;
      if (p.code) setPairing({ phase: "code", code: p.code, waitingSince: performance.now() });
      else if (p.waiting_for_user && now?.waitingSince === undefined) setPairing({ ...now!, waitingSince: performance.now() });
      report();
    });
    if (gen !== pairGen) return;
    setPairing({ phase: "done" });
    report();
    pairBack = undefined;
    await loadDevices();
    if (gen !== pairGen) return;
    // Selecting it shows its setup card, or the intro once it is ready, as for any device.
    const d = getViewer().devices.find((x) => x.udid === st.udid) ?? { udid: st.udid, transports: st.transports, setup: st.step ?? undefined };
    void select(d);
  } catch (e) {
    if (gen !== pairGen || !pairing()) return;
    setPairing({ phase: "failed", error: String(e) === "cancelled" ? msg("pair.stopped") : pairErrorText(e) });
    report();
  }
}

/** Puts the card away and stops pairing if it still waits. */
export function hidePairing(): void {
  if (!pairing()) return;
  if (pairRunning()) void backend.setupCancel().catch(() => {});
  pairGen++;
  setPairing(undefined);
}

/** Cancel or Close: back to the device selected before, if it is still there. */
export function closePairing(): void {
  const back = pairBack && getViewer().devices.find((d) => d.udid === pairBack!.udid);
  pairBack = undefined;
  hidePairing();
  if (back) {
    void select(back);
    return;
  }
  setStage("none");
  report();
}

/** A pairing error as the card shows it. The core's waits fail "... within 600s". */
function pairErrorText(e: unknown): Text {
  const s = String(e);
  if (/^no device asked to pair within \d+s/.test(s)) return msg("pair.noDevice", { duration: waitText() });
  if (/^nobody entered the code on the device within \d+s/.test(s)) return msg("pair.noCode", { duration: waitText() });
  return setupErrorText(s.replace(/\. Run `devicesteer setup wifi` again/, ", then press Try Again"));
}
