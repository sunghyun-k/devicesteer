import { invoke } from "@tauri-apps/api/core";

/** Mirrors `iphone_use_core::DeviceInfo`. */
interface DeviceInfo {
  udid: string;
  transports: ("usb" | "wifi")[];
  name?: string;
  product_type?: string;
  device_class?: string;
  os_version?: string;
  build_version?: string;
  error?: string;
}

const list = document.querySelector<HTMLUListElement>("#devices")!;
const status = document.querySelector<HTMLParagraphElement>("#status")!;
const refresh = document.querySelector<HTMLButtonElement>("#refresh")!;

function row(d: DeviceInfo): HTMLLIElement {
  const li = document.createElement("li");
  const name = document.createElement("div");
  name.className = "name";
  name.textContent = d.name ?? d.product_type ?? "Unknown device";
  const meta = document.createElement("div");
  meta.className = "meta";
  const os = d.os_version ? `${d.device_class ?? "iOS"} ${d.os_version}` : undefined;
  meta.textContent = [d.product_type, os, d.transports.join(" + ").toUpperCase(), d.udid]
    .filter(Boolean)
    .join(" · ");
  li.append(name, meta);
  if (d.error) {
    const err = document.createElement("div");
    err.className = "error";
    err.textContent = d.error;
    li.append(err);
  }
  return li;
}

async function load(): Promise<void> {
  refresh.disabled = true;
  status.textContent = "Looking for devices…";
  try {
    const devices = await invoke<DeviceInfo[]>("list_devices");
    list.replaceChildren(...devices.map(row));
    status.textContent = devices.length
      ? `${devices.length} device${devices.length > 1 ? "s" : ""}`
      : "No devices. Connect an iPhone or iPad over USB.";
  } catch (e) {
    list.replaceChildren();
    status.textContent = String(e);
  } finally {
    refresh.disabled = false;
  }
}

refresh.addEventListener("click", load);
load();
