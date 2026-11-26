// The middle column: the selected device's header and toolbar, notices, the stage with the device
// or its intro, and under it the session slot and the device bar.

import { Icon } from "../../components/Icon";
import { Menu, MenuItem, MenuSeparator } from "../../components/Menu";
import { stopScreen } from "../../device/connection";
import { deviceMeta, deviceName, isTablet } from "../../device/text";
import { useT } from "../../i18n/react";
import { isMac } from "../../lib/platform";
import { useViewer } from "../../store/viewer";
import { openSettings } from "../agents/agents";
import { KeyboardMenu } from "../keyboard/KeyboardMenu";
import { SessionDock } from "../session/SessionDock";
import { Banners } from "./Banners";
import { DeviceBar } from "./DeviceBar";
import { reveal } from "./notes";
import { Stage } from "./Stage";
import { setZoom, zoomBy, ZOOM_MAX, ZOOM_MIN } from "./zoom";
import s from "./Center.module.css";
import b from "./Bottom.module.css";

function Title() {
  const t = useT();
  const d = useViewer((v) => v.selected);
  const gone = useViewer((v) => !!v.gone);
  const pairing = useViewer((v) => !!v.pairing);
  const empty = useViewer((v) => v.listed && !v.devices.length);
  const name = d ? deviceName(d, t) : pairing ? t("device.new") : t("device.none");
  const meta = d ? (gone ? t("device.disconnected") : deviceMeta(d)) : pairing ? t("device.pairingWifi") : empty ? t("device.connectToBegin") : t("device.choose");
  return (
    <>
      <Icon name={isTablet(d) ? "i-tablet" : "i-phone"} className={`icon ${s.glyph}`} />
      <div className={s.title} data-tauri-drag-region>
        <span id="device-name" className={s.name} data-tauri-drag-region>
          {name}
        </span>
        <span id="device-meta" className={s.meta} data-tauri-drag-region>
          {meta}
        </span>
      </div>
    </>
  );
}

function ZoomGroup({ hidden }: { hidden: boolean }) {
  const t = useT();
  const zoom = useViewer((v) => v.zoom);
  const scale = useViewer((v) => v.scale);
  return (
    <div className={s.group} id="zoom-group" role="group" aria-label={t("zoom.group")} hidden={hidden}>
      <button id="zoom-out" className="icon-btn" type="button" aria-label={t("zoom.out")} title={t("zoom.outHelp")} disabled={scale <= ZOOM_MIN + 0.001} onClick={() => zoomBy(1 / 1.25)}>
        <Icon name="i-zoom-out" />
      </button>
      <button id="zoom-fit" className="icon-btn" type="button" aria-label={t("zoom.fit")} title={t("zoom.fitHelp")} aria-pressed={zoom === "fit"} onClick={() => setZoom("fit")}>
        <Icon name="i-fit" />
      </button>
      <button
        id="zoom-actual"
        className="icon-btn"
        type="button"
        aria-label={t("zoom.actual")}
        title={t("zoom.actualHelp")}
        aria-pressed={zoom !== "fit" && Math.abs(scale - 1) < 0.001}
        onClick={() => setZoom(1)}
      >
        <Icon name="i-zoom-actual" />
      </button>
      <button id="zoom-in" className="icon-btn" type="button" aria-label={t("zoom.in")} title={t("zoom.inHelp")} disabled={scale >= ZOOM_MAX - 0.001} onClick={() => zoomBy(1.25)}>
        <Icon name="i-zoom-in" />
      </button>
    </div>
  );
}

function MoreMenu() {
  const t = useT();
  const wantView = useViewer((v) => v.wantView);
  const recordedPath = useViewer((v) => v.recordedPath);
  // Settings is in the app menu on macOS (view_menu.rs, ⌘,); elsewhere the window has no menu
  // bar, so the More menu and Ctrl+, open it.
  const appMenu = isMac();
  return (
    <div className={s.group}>
      <Menu name="more" id="more" menuId="more-menu" icon="i-more" label={t("more.button")} title={t("more.button")}>
        <MenuItem id="menu-stop-view" icon="i-eye-off" label={t("more.stopViewing")} disabled={!wantView} onSelect={() => void stopScreen()} />
        <MenuItem id="menu-reveal" icon="i-folder" label={t("more.showLastRecording")} disabled={!recordedPath} onSelect={() => reveal(recordedPath)} />
        {appMenu ? null : (
          <>
            <MenuSeparator />
            <MenuItem id="menu-settings" icon="i-sliders" label={t("more.settings")} shortcut={t("more.settingsKey")} onSelect={() => openSettings()} />
          </>
        )}
      </Menu>
    </div>
  );
}

export function Center() {
  const live = useViewer((v) => v.stage === "live");
  return (
    <main id="center" className={s.center}>
      <header className={s.head} data-tauri-drag-region>
        <Title />
        <div className={s.toolbar}>
          <ZoomGroup hidden={!live} />
          <KeyboardMenu className={s.group} hidden={!live} />
          <MoreMenu />
        </div>
      </header>
      <Banners />
      <Stage />
      <div className={b.bottom}>
        <SessionDock />
        <DeviceBar />
      </div>
    </main>
  );
}
