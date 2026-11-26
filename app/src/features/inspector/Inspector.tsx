// The inspector: the device's settings (display, accessibility, simulated location) and its info
// (the device and the screen stream's numbers). Laid out like a settings page: section titles
// over rows split by hairlines. The app's own settings are in the Settings window.

import { useEffect, useReducer, useRef, useState } from "react";
import { Icon } from "../../components/Icon";
import { ticks } from "../../device/connection";
import { stats } from "../../device/stats";
import { modelName, osLabel } from "../../device/text";
import type { Key } from "../../i18n/i18n";
import { useT } from "../../i18n/react";
import { avg } from "../../lib/format";
import { prefs } from "../../lib/prefs";
import { screen } from "../../stream/screen";
import { setViewer, useViewer, type StageState } from "../../store/viewer";
import { clearLocation, GLASS_DEFAULT, setSetting, simulateLocation, TEXT_SIZES, TOGGLES } from "./deviceSettings";
import s from "./Inspector.module.css";

/** A slider's track filled up to its thumb: `--p` is how far along it is, from 0 to 1. */
const fill = (value: number, min: number, max: number) => ({ "--p": String((value - min) / (max - min)) }) as React.CSSProperties;

function SettingsPane({ hidden }: { hidden: boolean }) {
  const t = useT();
  const cur = useViewer((v) => v.settings);
  const message = useViewer((v) => v.settingsMessage);
  const busy = useViewer((v) => v.settingsBusy);
  const disabled = useViewer((v) => v.settingsDisabled);
  // While a slider is dragged it shows where it is; the device's value comes back on release.
  const [glassDrag, setGlassDrag] = useState<number | undefined>();
  const [sizeDrag, setSizeDrag] = useState<number | undefined>();
  // Custom while the person is typing a location, before one is set.
  const [custom, setCustom] = useState(false);
  const [lat, setLat] = useState("");
  const [lon, setLon] = useState("");
  const [editing, setEditing] = useState(false);
  const [shown, setShown] = useState(cur.location);

  // A location set elsewhere (or by Simulate) fills the fields, unless the person is typing in them.
  if (cur.location !== shown) {
    setShown(cur.location);
    if (!editing && cur.location) {
      setLat(String(cur.location[0]));
      setLon(String(cur.location[1]));
    }
    if (!cur.location && !editing) setCustom(false);
  }

  const off = (item: string) => cur.unsupported?.includes(item) ?? false;
  const unsupportedNote = (item: string) => (off(item) ? <span className={s.unsupportedNote}>{t("settings.notOnDevice")}</span> : null);
  const rowClass = (item: string) => `row ${off(item) ? s.unsupported : ""}`;

  /** Each text size's name, for the slider's tooltip; the larger ones go by their short names. */
  const textSizeName = (i: number): string => {
    const named: (Key | undefined)[] = ["settings.textSizeXS", "settings.textSizeS", "settings.textSizeM", "settings.textSizeL", "settings.textSizeXL"];
    const key = named[i];
    return key ? t(key) : (TEXT_SIZES[i]?.toUpperCase() ?? "");
  };
  /** An opacity as the slider reads it out, saying so at the default. */
  const glassValue = (o: number): string => {
    const value = t.num(o, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
    return Math.abs(o - GLASS_DEFAULT) < 0.001 ? t("settings.defaultValue", { value }) : value;
  };

  const glass = glassDrag ?? cur.liquid_glass ?? 0.5;
  const sizeIndex = cur.text_size ? TEXT_SIZES.indexOf(cur.text_size) : -1;
  const size = sizeDrag ?? (sizeIndex >= 0 ? sizeIndex : 5.5);
  const sizeTitle = sizeDrag !== undefined || sizeIndex >= 0 ? textSizeName(size) : "";
  const mode = cur.location || custom ? "custom" : "none";

  const simulate = () => simulateLocation(lat, lon);

  // A slider changes the device when it is let go (the native `change` event), not while it moves.
  const glassRef = useRef<HTMLInputElement>(null);
  const sizeRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const g = glassRef.current!;
    const z = sizeRef.current!;
    const onGlass = () => {
      setGlassDrag(undefined);
      void setSetting("liquid-glass", [g.value]);
    };
    const onSize = () => {
      setSizeDrag(undefined);
      void setSetting("text-size", [TEXT_SIZES[Number(z.value)]]);
    };
    g.addEventListener("change", onGlass);
    z.addEventListener("change", onSize);
    return () => {
      g.removeEventListener("change", onGlass);
      z.removeEventListener("change", onSize);
    };
  }, []);

  return (
    <div id="pane-settings" className={`pane ${disabled ? s.disabled : ""}`} role="tabpanel" aria-labelledby="tab-settings" aria-busy={busy || undefined} hidden={hidden}>
      <section className="group">
        <h4>{t("settings.display")}</h4>
        <div className={rowClass("appearance")} data-item="appearance">
          <label htmlFor="appearance">
            {t("settings.appearance")}
            {unsupportedNote("appearance")}
          </label>
          <span className="select">
            <select id="appearance" value={cur.appearance ?? ""} disabled={off("appearance")} onChange={(e) => void setSetting("appearance", [e.target.value])}>
              <option value="" disabled hidden>
                –
              </option>
              <option value="light">{t("settings.light")}</option>
              <option value="dark">{t("settings.dark")}</option>
            </select>
            <Icon name="i-chevron" />
          </span>
        </div>
        <div className={rowClass("liquid_glass")} data-item="liquid_glass">
          <label htmlFor="liquid-glass">
            {t("settings.liquidGlass")}
            {unsupportedNote("liquid_glass")}
          </label>
          <input
            id="liquid-glass"
            className={`slider ${s.glass}`}
            type="range"
            min="0"
            max="1"
            step="0.05"
            value={glass}
            disabled={off("liquid_glass")}
            title={cur.liquid_glass != null ? t("settings.clearToTintedValue", { value: glassValue(cur.liquid_glass) }) : t("settings.clearToTinted")}
            aria-valuetext={glassDrag !== undefined || cur.liquid_glass != null ? glassValue(glass) : undefined}
            style={fill(glass, 0, 1)}
            ref={glassRef}
            onInput={(e) => setGlassDrag(Number(e.currentTarget.value))}
            onChange={() => {}}
          />
        </div>
        <div className={rowClass("text_size")} data-item="text_size">
          <label htmlFor="text-size">
            {t("settings.textSize")}
            {unsupportedNote("text_size")}
          </label>
          <input
            id="text-size"
            className={`slider ticks ${s.textSize}`}
            type="range"
            min="0"
            max="11"
            step="1"
            value={size}
            disabled={off("text_size")}
            title={sizeTitle}
            // Read out the size's name, such as "Large (default)", rather than its step number.
            aria-valuetext={sizeTitle || undefined}
            style={fill(size, 0, 11)}
            ref={sizeRef}
            onInput={(e) => setSizeDrag(Number(e.currentTarget.value))}
            onChange={() => {}}
          />
        </div>
      </section>
      <section className="group">
        <h4>{t("settings.accessibility")}</h4>
        {TOGGLES.map(([key, name]) => (
          <label key={key} className={rowClass(key)} data-item={key}>
            <span>
              {t(TOGGLE_LABELS[key])}
              {unsupportedNote(key)}
            </span>
            <input type="checkbox" role="switch" checked={cur[key] === true} disabled={off(key)} onChange={(e) => void setSetting(name, [e.target.checked ? "on" : "off"])} />
          </label>
        ))}
      </section>
      <section className={`group ${off("location") ? s.unsupported : ""}`} data-item="location">
        <h4>{t("settings.location")}</h4>
        <div className="row">
          <label htmlFor="location-mode">
            {t("settings.simulatedLocation")}
            {unsupportedNote("location")}
          </label>
          <span className="select">
            <select
              id="location-mode"
              value={mode}
              disabled={off("location")}
              onChange={(e) => {
                if (e.target.value === "none") {
                  setCustom(false);
                  clearLocation();
                } else {
                  setCustom(true);
                  // The fields show now; the latitude takes the keys.
                  requestAnimationFrame(() => document.getElementById("latitude")?.focus());
                }
              }}
            >
              <option value="none">{t("settings.locationNone")}</option>
              <option value="custom">{t("settings.locationCustom")}</option>
            </select>
            <Icon name="i-chevron" />
          </span>
        </div>
        <div id="location-edit" className="sub" hidden={mode !== "custom"}>
          <div className={s.coords}>
            {(
              [
                ["latitude", lat, setLat],
                ["longitude", lon, setLon],
              ] as const
            ).map(([id, value, set]) => (
              <input
                key={id}
                id={id}
                type="text"
                inputMode="decimal"
                placeholder={t(`settings.${id}`)}
                aria-label={t(`settings.${id}`)}
                value={value}
                disabled={off("location")}
                onChange={(e) => set(e.target.value)}
                onFocus={() => setEditing(true)}
                onBlur={() => setEditing(false)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") simulate();
                }}
              />
            ))}
          </div>
          <button id="location-set" className={`btn small ${s.set}`} type="button" disabled={off("location")} onClick={simulate}>
            {t("settings.simulate")}
          </button>
        </div>
      </section>
      <p id="settings-status" className={`${s.status} ${message.error ? s.error : ""} ${busy ? s.busy : ""}`} role="status">
        {t.say(message.text)}
      </p>
    </div>
  );
}

const TOGGLE_LABELS = {
  reduce_motion: "settings.reduceMotion",
  increase_contrast: "settings.increaseContrast",
  show_borders: "settings.buttonShapes",
  reduce_transparency: "settings.reduceTransparency",
  voiceover: "settings.voiceover",
} as const;

const STREAM_STATES: Partial<Record<StageState, Key>> = {
  intro: "info.streamNotShown",
  setup: "info.streamNeedsSetup",
  unavailable: "device.unavailable",
  disconnected: "device.disconnected",
  connecting: "common.connecting",
  error: "info.streamError",
};

function Out({ id, text, children }: { id: string; text: string; children?: React.ReactNode }) {
  return (
    <output id={`info-${id}`} title={text}>
      {text || "–"}
      {children}
    </output>
  );
}

/** What the Info pane shows of the stream: a copy of the numbers, taken once a second. */
function streamSnapshot() {
  const now = performance.now();
  stats.fpsWindow = stats.fpsWindow.filter((x) => now - x < 1000);
  return {
    geometry: screen.geometry,
    fps: stats.fpsWindow.length,
    decodeMs: stats.decodeMs.slice(-60),
    touchMs: [...stats.touchMs],
    decoded: stats.decoded,
    received: stats.received,
    dropped: stats.dropped,
    supported: stats.supported,
    lastError: stats.lastError,
  };
}

function InfoPane({ hidden }: { hidden: boolean }) {
  const t = useT();
  const d = useViewer((v) => v.selected);
  const stage = useViewer((v) => v.stage);
  // The numbers change with every frame: copy them once a second while attached, and when the
  // pane shows or the stage changes.
  const [n, refresh] = useReducer(streamSnapshot, undefined, streamSnapshot);
  useEffect(() => {
    if (hidden) return;
    refresh();
    ticks.addEventListener("tick", refresh);
    return () => ticks.removeEventListener("tick", refresh);
  }, [hidden, stage]);
  if (hidden) return <div id="pane-info" className="pane" role="tabpanel" aria-labelledby="tab-info" hidden />;

  const g = n.geometry;
  const live = stage === "live";
  const dm = n.decodeMs;
  const state = stage === "live" ? (n.supported === false ? "info.streamUnsupported" : "info.streamLive") : STREAM_STATES[stage];
  const ms = (x: number, digits: number) => t("info.ms", { ms: t.num(x, { minimumFractionDigits: digits, maximumFractionDigits: digits }) });

  return (
    <div id="pane-info" className="pane" role="tabpanel" aria-labelledby="tab-info">
      <section className="group info">
        <h4>{t("info.device")}</h4>
        <div className="row">
          <span>{t("info.name")}</span>
          <Out id="name" text={d?.name ?? ""} />
        </div>
        <div className="row">
          <span>{t("info.model")}</span>
          {/* The model's name, and its identifier in small gray text when the two differ. */}
          <Out id="model" text={d ? (modelName(d) ?? "") : ""}>
            {d?.model_name && d.product_type ? (
              <>
                {" "}
                <small className="code">{d.product_type}</small>
              </>
            ) : null}
          </Out>
        </div>
        <div className="row">
          <span>{t("info.os")}</span>
          <Out id="os" text={d ? [osLabel(d), d.build_version ? `(${d.build_version})` : ""].filter(Boolean).join(" ") : ""} />
        </div>
        <div className="row">
          <span>{t("info.connection")}</span>
          <Out id="connection" text={d ? d.transports.map((x) => (x === "wifi" ? "Wi-Fi" : "USB")).join(t("common.listSeparator")) : ""} />
        </div>
        <div className="row stack">
          <span>UDID</span>
          <output id="info-udid" className="mono" title={d?.udid ?? ""}>
            {d?.udid || "–"}
          </output>
        </div>
      </section>
      <section className="group info">
        <h4>{t("info.screenStream")}</h4>
        <div className="row">
          <span>{t("info.status")}</span>
          <Out id="stream" text={state ? t(state) : ""} />
        </div>
        <div className="row">
          <span>{t("info.video")}</span>
          <Out id="video" text={live && g ? `${g.codec.split(".")[0].toUpperCase()} ${g.displayPx.join("×")}` : ""} />
        </div>
        <div className="row">
          <span>{t("info.screen")}</span>
          <Out id="screen" text={live && g ? t(g.orientation === "landscape" ? "info.screenLandscape" : "info.screenPortrait", { size: g.screen.join("×") }) : ""} />
        </div>
        <div className="row">
          <span>{t("info.frameRate")}</span>
          <Out id="fps" text={live ? t("info.fps", { fps: n.fps }) : ""} />
        </div>
        <div className="row">
          <span>{t("info.decoded")}</span>
          <Out
            id="decoded"
            text={
              live
                ? [t("info.decodedOf", { decoded: n.decoded, received: n.received }), n.dropped ? t("info.dropped", { count: n.dropped }) : ""]
                    .filter(Boolean)
                    .join(t("common.listSeparator"))
                : ""
            }
          />
        </div>
        <div className="row">
          <span>{t("info.decodeTime")}</span>
          <Out id="decode" text={live && dm.length ? ms(avg(dm), 1) : ""} />
        </div>
        <div className="row">
          <span>{t("info.touchRoundTrip")}</span>
          <Out id="touch" text={n.touchMs.length ? ms(avg(n.touchMs), 0) : ""} />
        </div>
      </section>
      <p id="info-error" className="error-text">
        {n.lastError}
      </p>
    </div>
  );
}

export function Inspector({ hidden }: { hidden: boolean }) {
  const t = useT();
  const tab = useViewer((v) => v.tab);
  const pick = (x: "settings" | "info") => {
    prefs.set("tab", x);
    setViewer({ tab: x });
  };
  return (
    <aside id="inspector" className={s.inspector} aria-label={t("inspector.toggle")} hidden={hidden}>
      <header className={s.head} data-tauri-drag-region>
        <span className={s.grow} data-tauri-drag-region />
        {/* An icon-only segmented picker for the two panes, left of the inspector toggle. */}
        <div className={s.segmented} role="tablist" aria-label={t("inspector.toggle")}>
          <button id="tab-settings" role="tab" type="button" aria-selected={tab === "settings"} aria-controls="pane-settings" aria-label={t("inspector.settings")} title={t("inspector.settings")} onClick={() => pick("settings")}>
            <Icon name="i-sliders" />
          </button>
          <button id="tab-info" role="tab" type="button" aria-selected={tab === "info"} aria-controls="pane-info" aria-label={t("inspector.info")} title={t("inspector.info")} onClick={() => pick("info")}>
            <Icon name="i-info" />
          </button>
        </div>
      </header>
      <SettingsPane hidden={tab !== "settings"} />
      <InfoPane hidden={tab !== "info" || hidden} />
    </aside>
  );
}
