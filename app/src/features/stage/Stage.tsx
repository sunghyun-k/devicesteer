// The stage: before the screen shows, what is selected and how to see it (or its setup card, or
// the pairing card); then the live screen, scaled to the zoom.
//
// The screen is drawn as it is: a plain rectangle with no hardware around it. Hardware buttons
// are real controls in the device bar, not pictures. The canvas, the overlay and the input on
// them belong to stream/, overlay/ and input/: this component lends them its elements and lays
// them out, and nothing here changes per frame.

import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import { Icon } from "../../components/Icon";
import { retry, showScreen } from "../../device/connection";
import { deviceMeta, deviceName, isTablet, unusable } from "../../device/text";
import { useT } from "../../i18n/react";
import { keyTarget } from "../../input/keyboard";
import { touchInput } from "../../input/touch";
import { indicators } from "../../overlay/indicators";
import { screen } from "../../stream/screen";
import { activeSession, humanHasDevice, setViewer, useViewer } from "../../store/viewer";
import { PairCard } from "../setup/PairCard";
import { SetupCard } from "../setup/SetupCard";
import { ZOOM_MAX } from "./zoom";
import s from "./Stage.module.css";

function Intro() {
  const t = useT();
  const stage = useViewer((v) => v.stage);
  const text = useViewer((v) => v.stageText);
  const d = useViewer((v) => v.selected);
  const gone = useViewer((v) => !!v.gone);
  const setup = useViewer((v) => !!v.setup);
  const pairing = useViewer((v) => !!v.pairing);
  if (stage === "live") return null;
  // Nothing selected: an empty stage.
  if (stage === "none") return <div id="intro" className={s.intro} />;
  const card = pairing ? <PairCard /> : setup ? <SetupCard /> : null;
  if (stage === "pair")
    return (
      <div id="intro" className={`${s.intro} ${s.pair}`}>
        {card ? (
          <div id="intro-card" className={`${s.card} surface`}>
            {card}
          </div>
        ) : null}
      </div>
    );
  const reason = stage === "error" || stage === "unavailable" || stage === "disconnected";
  const artClass = [s.art, isTablet(d) ? s.tablet : "", unusable(d) || gone ? s.unavailable : ""].filter(Boolean).join(" ");
  return (
    <div id="intro" className={s.intro}>
      <div id="intro-art" className={artClass} aria-hidden="true" />
      <h2 id="intro-name" className={s.name}>
        {d ? deviceName(d, t) : ""}
      </h2>
      <p id="intro-meta" className={s.meta}>
        {d ? deviceMeta(d) : ""}
      </p>
      {stage === "intro" ? (
        <button id="view-screen" className={`btn primary large ${s.view}`} type="button" onClick={() => void showScreen()}>
          <Icon name="i-screen" />
          <span>{t("stage.viewScreen")}</span>
        </button>
      ) : null}
      {stage === "connecting" ? (
        <div id="intro-busy" className={s.busy}>
          <span className="spinner" aria-hidden="true" />
          <span id="intro-busy-text">{t.say(text) || t("common.connecting")}</span>
        </div>
      ) : null}
      {reason ? (
        <p id="intro-reason" className={`${s.reason} ${stage === "error" ? s.error : ""}`}>
          {t.say(text)}
        </p>
      ) : null}
      {stage === "error" ? (
        <div id="intro-buttons" className={s.buttons}>
          <button id="intro-retry" className="btn primary" type="button" onClick={retry}>
            {t("common.tryAgain")}
          </button>
        </div>
      ) : null}
      {card ? (
        <div id="intro-card" className={`${s.card} surface`}>
          {card}
        </div>
      ) : null}
    </div>
  );
}

/** The kind of screen, for the report and the layout's data attributes. */
function frameKind(w: number, h: number, tablet: boolean): { kind: string; portrait: boolean } {
  const m = Math.min(w, h);
  const long = Math.max(w, h) / m;
  return { kind: tablet || long < 1.6 ? "tablet" : long < 1.9 ? "classic" : "modern", portrait: h >= w };
}

/** The live screen: the canvas the stream draws on and the overlay over it, sized to the zoom. */
function Screen({ stageEl }: { stageEl: React.RefObject<HTMLDivElement | null> }) {
  const t = useT();
  const geometry = useSyncExternalStore(screen.subscribe, screen.getGeometry);
  const live = useViewer((v) => v.stage === "live");
  const zoom = useViewer((v) => v.zoom);
  const zoomSeq = useViewer((v) => v.zoomSeq);
  const tablet = useViewer((v) => isTablet(v.selected));
  const session = useViewer((v) => activeSession(v));
  const blocked = useViewer((v) => !humanHasDevice(v));
  const scroller = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const box = useRef<HTMLDivElement>(null);
  const overlay = useRef<HTMLDivElement>(null);
  /** The room the scroller has for the device: its size inside its padding. */
  const [room, setRoom] = useState<[number, number]>([0, 0]);

  // The stream, the overlay and the input get the elements for as long as this is mounted.
  // Attaching twice (StrictMode) or again after a remount only lends them the elements again:
  // the decoder and the stream live on in stream/screen.ts.
  useEffect(() => {
    const c = canvas.current!;
    const o = overlay.current!;
    screen.attachCanvas(c);
    indicators.attach(o, stageEl.current!);
    const untouch = touchInput(c);
    const unkey = keyTarget(c, box.current!);
    return () => {
      unkey();
      untouch();
      indicators.detach(o);
      screen.detachCanvas(c);
    };
  }, [stageEl]);

  useEffect(() => {
    indicators.setBlockedText(t("session.blockedHint"));
  }, [t]);

  useLayoutEffect(() => {
    const el = scroller.current!;
    const measure = () => {
      const cs = getComputedStyle(el);
      setRoom([
        el.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight),
        el.clientHeight - parseFloat(cs.paddingTop) - parseFloat(cs.paddingBottom),
      ]);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const [w, h] = geometry?.screen ?? [0, 0];
  const fit = w && h ? Math.max(0.05, Math.min(room[0] / w, room[1] / h, ZOOM_MAX)) : 1;
  const scale = zoom === "fit" ? fit : zoom;
  const f = frameKind(w || 1, h || 1, tablet);

  // The scale the layout used, for zooming in and out from it and for the zoom buttons.
  useEffect(() => {
    if (geometry) setViewer({ scale });
  }, [geometry, scale]);

  // The cursor rests on a screen point: place it again on the new size.
  useLayoutEffect(() => {
    indicators.relayout();
  }, [w, h, scale]);

  // Zoom about the middle of what shows.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el || !zoomSeq) return;
    el.scrollLeft = (el.scrollWidth - el.clientWidth) / 2;
    el.scrollTop = (el.scrollHeight - el.clientHeight) / 2;
  }, [zoomSeq]);

  const deviceStyle = geometry ? ({ width: `${w * scale}px`, height: `${h * scale}px`, "--rim": "1.5px" } as React.CSSProperties) : undefined;
  return (
    <div id="scroller" ref={scroller} className={`${s.scroller} ${live ? "" : s.away}`}>
      <div id="device" className={s.device} data-kind={geometry ? f.kind : undefined} data-orient={geometry ? (f.portrait ? "portrait" : "landscape") : undefined} style={deviceStyle}>
        <div id="glow" className={`${s.glow} ${session ? "" : s.off} ${session && session.owner !== "agent" ? s.human : ""}`} />
        <div className={s.bezel}>
          <div id="screen-box" ref={box} className={`${s.screenBox} ${blocked ? s.agentHasDevice : ""}`}>
            <canvas id="screen" ref={canvas} className={s.screen} tabIndex={0} />
            {/* Touch discs are a fingertip on the screen (56 points). */}
            <div id="overlay" ref={overlay} className={s.overlay} style={{ "--touch": `${Math.max(20, 56 * scale)}px` } as React.CSSProperties} />
          </div>
        </div>
      </div>
    </div>
  );
}

export function Stage() {
  const stageEl = useRef<HTMLDivElement>(null);
  const state = useViewer((v) => v.stage);
  const zoom = useViewer((v) => v.zoom);
  const scale = useViewer((v) => v.scale);
  return (
    <div id="stage" ref={stageEl} className={s.stage} data-state={state} data-zoom={zoom === "fit" ? "fit" : String(Math.round(scale * 100))}>
      <Intro />
      <Screen stageEl={stageEl} />
    </div>
  );
}
