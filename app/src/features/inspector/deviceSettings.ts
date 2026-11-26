// The inspector's Settings pane: the device's display and accessibility settings and its
// simulated location, read when a device is attached, followed through the daemon's `settings`
// events, and changed one at a time.

import * as backend from "../../backend/commands";
import type { DeviceSettings } from "../../backend/types";
import { errorText, msg } from "../../i18n/i18n";
import { getViewer, setSettingsMessage, setViewer } from "../../store/viewer";

/** Text sizes from smallest to largest, as the CLI names them (`devicesteer_core::TEXT_SIZES`). */
export const TEXT_SIZES = ["xs", "s", "m", "l", "xl", "xxl", "xxxl", "ax1", "ax2", "ax3", "ax4", "ax5"];

/** Liquid Glass's default opacity: an iOS 27.3 iPhone at its default settings read 0.5. */
export const GLASS_DEFAULT = 0.5;

/** On/off items: the field in `DeviceSettings` and the name `settings_set` takes. */
export const TOGGLES = [
  ["reduce_motion", "reduce-motion"],
  ["increase_contrast", "increase-contrast"],
  ["show_borders", "show-borders"],
  ["reduce_transparency", "reduce-transparency"],
  ["voiceover", "voiceover"],
] as const;

/** Merges settings into what is known (an event may carry one key). */
export function showSettings(s: Partial<DeviceSettings>): void {
  setViewer((v) => ({ settings: { ...v.settings, ...s } }));
}

/** Forgets the device's settings (it was let go of). */
export function resetSettings(): void {
  setViewer({ settings: { unsupported: [] } });
}

export async function readSettings(): Promise<void> {
  setSettingsMessage(msg("settings.reading"));
  try {
    showSettings(await backend.send<DeviceSettings>("settings"));
    setSettingsMessage("");
  } catch (e) {
    setSettingsMessage(errorText(e), true);
  }
}

/** Changes one item. On failure the controls go back to the last known values: they show what
 * the store holds, which only a success changes. */
export async function setSetting(name: string, value: (string | number)[]): Promise<void> {
  setViewer({ settingsBusy: true });
  setSettingsMessage(msg("settings.applying"));
  try {
    const r = await backend.send<{ settings: DeviceSettings }>("settings_set", { name, value: value.map(String) });
    showSettings(r.settings);
    setSettingsMessage("");
  } catch (e) {
    // A new object, so controls a person moved go back to the last known values.
    setViewer((v) => ({ settings: { ...v.settings } }));
    setSettingsMessage(errorText(e), true);
  } finally {
    setViewer({ settingsBusy: false });
  }
}

/** Simulates the location typed in, or says why it can't. */
export function simulateLocation(latitude: string, longitude: string): void {
  const [lat, lon] = [latitude, longitude].map((v) => (v.trim() ? Number(v) : NaN));
  if (!(Math.abs(lat) <= 90) || !(Math.abs(lon) <= 180)) {
    setSettingsMessage(msg("settings.locationInvalid"), true);
    return;
  }
  void setSetting("location", ["set", lat, lon]);
}

/** Simulated Location › None: clears a location the device has. */
export function clearLocation(): void {
  if (getViewer().settings.location) void setSetting("location", ["clear"]);
}
