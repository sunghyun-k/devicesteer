// How a device is named and described in the sidebar, the header and notes.

import type { DeviceInfo, SetupStep } from "../backend/types";
import type { Translate } from "../i18n/i18n";

/** "iOS 26.1", "iPadOS 26.0", when the device said. */
export function osLabel(d: DeviceInfo): string | undefined {
  if (!d.os_version) return undefined;
  return `${d.device_class === "iPad" ? "iPadOS" : "iOS"} ${d.os_version}`;
}

/** What Apple calls the model, or its identifier when the core doesn't know it. */
export const modelName = (d: DeviceInfo): string | undefined => d.model_name ?? d.product_type;

export function deviceName(d: DeviceInfo, t: Translate): string {
  return d.name ?? modelName(d) ?? t("device.unknown");
}

/** The model and OS, after the name: the model only when the name isn't it already. */
export const deviceMeta = (d: DeviceInfo): string => [d.name ? modelName(d) : undefined, osLabel(d)].filter(Boolean).join(" · ");

/** A device that can't be opened: unreadable, or too old. Listed under Unavailable. */
export const unusable = (d: DeviceInfo | undefined): boolean => !!d?.error || !!d?.unsupported;

export const isTablet = (d: DeviceInfo | undefined): boolean => d?.device_class === "iPad" || !!d?.product_type?.startsWith("iPad");

/** "iPhone" or "iPad", as the device in a note. */
export const deviceKind = (d: DeviceInfo | undefined): string => (isTablet(d) ? "iPad" : "iPhone");

/** Step names in the setup card. */
export const setupTitle = (step: SetupStep, t: Translate): string =>
  t(({ trust: "setup.trust.title", developer_mode: "setup.developerMode.title", disk_image: "setup.diskImage.title" } as const)[step]);

/** Short step names, for a sidebar row's status, different at their start: a narrow sidebar cuts
 * the status off. */
export const setupShort = (step: SetupStep, t: Translate): string =>
  t(({ trust: "setup.trust.short", developer_mode: "setup.developerMode.short", disk_image: "setup.diskImage.short" } as const)[step]);
