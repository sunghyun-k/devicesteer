// The device list: the devices the shell lists, filtered by the search field, under Available and
// Unavailable, with the selection and every device an agent session runs on marked.

import type { KeyboardEvent } from "react";
import type { DeviceInfo } from "../../backend/types";
import { Icon } from "../../components/Icon";
import { deviceName, isTablet, modelName, osLabel, setupShort, setupTitle, unusable } from "../../device/text";
import { useT } from "../../i18n/react";
import { sessionOf, setViewer, useViewer } from "../../store/viewer";
import { sessionShort } from "../session/panelText";
import { openPairing } from "../setup/pairing";
import { loadDevices, openAppleDevicesStore, pick } from "./deviceList";
import s from "./Sidebar.module.css";

const focusRow = (from: Element | null, step: 1 | -1) => {
  const rows = [...document.querySelectorAll<HTMLLIElement>("#device-list [role=option]")];
  rows[rows.indexOf(from as HTMLLIElement) + step]?.focus();
};

function Row({ d, disconnected }: { d: DeviceInfo; disconnected: boolean }) {
  const t = useT();
  const selected = useViewer((v) => v.selected?.udid === d.udid);
  // The session shows on rows of devices that can be used.
  const session = useViewer((v) => (unusable(d) || disconnected ? undefined : sessionOf(v, d.udid)));
  // One line: the name, then its status in gray: the OS version and how it is connected, or why
  // it can't be used. Other states (setting a device up, say) belong in the status too.
  const status = disconnected
    ? t("device.disconnected")
    : d.unsupported
      ? t("device.needsIos27")
      : d.error
        ? t("device.unavailable")
        : d.setup
          ? t("device.setupStatus", { step: setupShort(d.setup, t) })
          : [d.name ? modelName(d) : undefined, d.transports.includes("usb") ? "USB" : d.transports.length ? "Wi-Fi" : undefined].filter(Boolean).join(" · ");
  // The core's own reasons (`unsupported`, `error`) stay in English.
  const title = disconnected
    ? t("device.disconnectedHelp")
    : (d.unsupported ?? d.error ?? (d.setup ? t("device.needsSetup", { step: setupTitle(d.setup, t) }) : [deviceName(d, t), modelName(d), osLabel(d)].filter(Boolean).join(" · ")));
  // An agent session's state replaces the gray status, and a dot at the row's end takes the
  // session strip's colour. The goal is in the tooltip and the accessible name.
  const goal = session ? t("list.sessionGoal", { state: sessionShort(session, t), goal: session.name }) : undefined;
  const classes = [s.dev, unusable(d) || disconnected ? s.unavailable : "", session ? s.hasSession : ""].filter(Boolean).join(" ");
  const onKeyDown = (e: KeyboardEvent<HTMLLIElement>) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      pick(d);
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      focusRow(e.currentTarget, e.key === "ArrowDown" ? 1 : -1);
    }
  };
  return (
    <li
      className={classes}
      data-udid={d.udid}
      role="option"
      tabIndex={0}
      aria-selected={selected}
      aria-description={goal}
      title={title}
      onClick={() => pick(d)}
      onKeyDown={onKeyDown}
    >
      <Icon name={isTablet(d) ? "i-tablet" : "i-phone"} className={`icon ${s.glyph}`} />
      <div className={s.text}>
        <span className={s.name}>{deviceName(d, t)}</span>
        <span className={s.status}>{session ? sessionShort(session, t) : status}</span>
      </div>
      {session ? <span className={s.busy} data-owner={session.owner} title={goal} aria-hidden="true" /> : null}
    </li>
  );
}

function DeviceList() {
  const t = useT();
  const devices = useViewer((v) => v.devices);
  const selected = useViewer((v) => v.selected);
  const gone = useViewer((v) => !!v.gone);
  const search = useViewer((v) => v.search);
  const listed = useViewer((v) => v.listed);
  const listError = useViewer((v) => v.listError);
  const usbmuxdMissing = useViewer((v) => v.usbmuxdMissing);

  const q = search.trim().toLowerCase();
  const match = (d: DeviceInfo) => !q || [d.name, d.model_name, d.product_type, d.os_version, d.udid].some((x) => x?.toLowerCase().includes(q));
  const shown = devices.filter(match);
  // The selected device stays listed while it is unplugged, so its view doesn't lose its row;
  // under Unavailable once it has left the list.
  const extra = selected && !devices.some((d) => d.udid === selected.udid) && match(selected) ? selected : undefined;
  const ok = shown.filter((d) => !unusable(d));
  const bad = shown.filter(unusable);
  if (extra && !gone && !unusable(extra)) ok.unshift(extra);
  else if (extra) bad.unshift(extra);

  let m: { title: string; detail: string; kind?: "error" | "loading" } | null = null;
  if (listError) m = { title: t("list.error"), detail: listError, kind: "error" };
  else if (!listed) m = { title: t("list.looking"), detail: "", kind: "loading" };
  // A row of the selected device that left the list is enough to drop the empty state.
  // The card says why nothing is listed and what to do.
  else if (!devices.length && !extra) m = usbmuxdMissing ? null : { title: t("list.empty"), detail: t("list.emptyDetail") };
  else if (!ok.length && !bad.length) m = { title: t("list.noMatches"), detail: t("list.noMatchesDetail", { query: search.trim() }) };

  return (
    <nav id="device-list" className={s.list}>
      {/* Windows without Apple Devices: nothing answers where usbmuxd should be, so no USB device
          can show up. Shown while the list says so. */}
      {usbmuxdMissing ? (
        <div id="mux-card" className={`${s.mux} surface`} role="status">
          <p className={s.messageTitle}>{t("list.noUsbmuxd.title")}</p>
          <p className={s.muxDetail}>{t("list.noUsbmuxd.detail")}</p>
          <button id="mux-store" className="btn primary small" type="button" onClick={openAppleDevicesStore}>
            {t("list.noUsbmuxd.store")}
          </button>
          <p className={s.note}>
            <span className="spinner" aria-hidden="true" />
            <span>{t("list.noUsbmuxd.waiting")}</span>
          </p>
        </div>
      ) : null}
      {ok.length ? (
        <section>
          <h3>{t("sidebar.available")}</h3>
          <ul role="listbox" aria-label={t("sidebar.availableDevices")}>
            {ok.map((d) => (
              <Row key={d.udid} d={d} disconnected={false} />
            ))}
          </ul>
        </section>
      ) : null}
      {bad.length ? (
        <section>
          <h3>{t("sidebar.unavailable")}</h3>
          <ul role="listbox" aria-label={t("sidebar.unavailableDevices")}>
            {bad.map((d) => (
              <Row key={d.udid} d={d} disconnected={d === extra && gone} />
            ))}
          </ul>
        </section>
      ) : null}
      {m ? (
        <div className={`${s.message} ${m.kind === "error" ? s.error : ""}`}>
          {m.kind === "loading" ? <span className="spinner" aria-hidden="true" /> : null}
          <p className={s.messageTitle}>{m.title}</p>
          <p className={s.detail}>{m.detail}</p>
        </div>
      ) : null}
    </nav>
  );
}

export function Sidebar({ hidden }: { hidden: boolean }) {
  const t = useT();
  const search = useViewer((v) => v.search);
  const loading = useViewer((v) => v.listLoading);
  return (
    <aside id="sidebar" className={s.sidebar} aria-label={t("sidebar.devices")} hidden={hidden}>
      <header className={s.head} data-tauri-drag-region>
        <span className={s.lights} aria-hidden="true" />
        <span className={s.grow} data-tauri-drag-region />
      </header>
      <div className={s.titleRow}>
        <h2 className={s.title}>{t("sidebar.devices")}</h2>
        <span className={s.grow} />
        <button id="refresh" className="icon-btn" type="button" aria-label={t("sidebar.refresh")} title={t("sidebar.refresh")} disabled={loading} onClick={() => void loadDevices()}>
          <Icon name="i-refresh" />
        </button>
      </div>
      <label className={s.search}>
        <Icon name="i-search" />
        <input
          id="search"
          type="search"
          placeholder={t("sidebar.search")}
          autoComplete="off"
          spellCheck={false}
          aria-label={t("sidebar.searchDevices")}
          value={search}
          onChange={(e) => setViewer({ search: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              document.querySelector<HTMLLIElement>("#device-list [role=option]")?.focus();
            } else if (e.key === "Escape") setViewer({ search: "" });
          }}
        />
      </label>
      <DeviceList />
      {/* Wi-Fi pairing needs no cable, so a device that isn't listed yet can start here. */}
      <footer className={s.foot}>
        <button id="pair-wifi" className="btn ghost small" type="button" title={t("sidebar.pairWifiHelp")} onClick={openPairing}>
          <Icon name="i-wifi" />
          <span>{t("sidebar.pairWifi")}</span>
        </button>
      </footer>
    </aside>
  );
}
