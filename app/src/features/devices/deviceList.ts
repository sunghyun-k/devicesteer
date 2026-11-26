// The device list: read from the shell, kept current while the window shows, and what happens to
// the selected device when it leaves the list or comes back.

import * as backend from "../../backend/commands";
import type { DeviceInfo } from "../../backend/types";
import { detach, reselect, select } from "../../device/connection";
import { unusable } from "../../device/text";
import { msg } from "../../i18n/i18n";
import { getViewer, setSettingsMessage, setStage, setViewer } from "../../store/viewer";
import { showSetup } from "../setup/setup";
import { showNote } from "../stage/notes";
import { errorText } from "../../i18n/i18n";

export async function loadDevices(): Promise<void> {
  if (getViewer().listLoading) return;
  setViewer({ listLoading: true });
  const before = getViewer().devices;
  try {
    const list = await backend.listDevices();
    setViewer({ devices: list.devices, usbmuxdMissing: !!list.usbmuxd_missing, listError: undefined });
  } catch (e) {
    setViewer({ devices: [], usbmuxdMissing: false, listError: String(e) });
  } finally {
    setViewer({ listed: true, listLoading: false });
  }
  const v = getViewer();
  const selected = v.selected;
  // The selected device's details can change (unlocked, trusted, a transport came or went).
  const now = selected && v.devices.find((d) => d.udid === selected.udid);
  if (selected && !now && !v.listError && before.some((d) => d.udid === selected.udid)) {
    // Only a device the list had: one opened by UDID before the list showed it isn't "gone".
    onSelectedGone();
  } else if (now && v.gone) {
    onSelectedBack(now);
  } else if (now && selected) {
    const wasError = unusable(selected);
    const wasSetup = !!selected.setup;
    setViewer({ selected: now });
    const running = !!getViewer().setup?.run;
    if (now.setup && !running) showSetup(now);
    // It became usable: attach now, as if just picked.
    if ((wasError || wasSetup) && !unusable(now) && !now.setup && !running) reselect(now);
  }
}

/** The selected device left the list: let go of it (the stream and the daemon's attachment end
 * with the device anyway) but keep it selected, and say what to do. A setup step that is running
 * keeps running: Developer Mode's waits through the restart. */
function onSelectedGone(): void {
  const v = getViewer();
  if (v.gone || !v.selected) return;
  setViewer({ gone: { view: v.wantView }, settingsDisabled: true });
  if (!v.setup?.run) void detach();
  setSettingsMessage(msg("settings.afterReconnect"));
  setStage("disconnected", msg("stage.disconnected"));
}

/** The same UDID is back: go on where it left off, at its setup step or with its screen. */
function onSelectedBack(now: DeviceInfo): void {
  const v = getViewer();
  const was = v.gone!;
  setViewer({ gone: undefined });
  if (v.setup?.run && v.setup.device.udid === now.udid) {
    // The running step brings the card up to date when it ends.
    setViewer({ selected: now, setup: { ...v.setup, device: now } });
    setSettingsMessage(msg("settings.afterSetup"));
    setStage("setup");
    return;
  }
  reselect(now, { view: was.view });
}

/** Devices come and go (a cable, Wi-Fi): the list keeps itself current while the window shows.
 * While usbmuxd is missing, more often, so the card goes soon after Apple Devices starts.
 * Installed once for the page. */
export function followDevices(): void {
  let since = 0;
  window.setInterval(() => {
    since += 2;
    if (document.hidden || since < (getViewer().usbmuxdMissing ? 2 : 8)) return;
    since = 0;
    void loadDevices();
  }, 2000);
  // Back from the Store or from Apple Devices' license: look again at once.
  window.addEventListener("focus", () => {
    if (getViewer().usbmuxdMissing) void loadDevices();
  });
}

/** The Microsoft Store page of Apple Devices (Windows without usbmuxd). */
export function openAppleDevicesStore(): void {
  backend.openAppleDevicesStore().catch((e) => showNote(errorText(e), { error: true }));
}

/** Picks a device in the list. */
export const pick = (d: DeviceInfo) => void select(d);
