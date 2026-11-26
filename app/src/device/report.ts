// The page's report to viewer.log in the state directory: the decoder's numbers and what the
// window shows, so a check of the viewer doesn't need anyone to look. Sent every two seconds while
// attached and after events worth a look (a turn, a recording, the pairing card).

import * as backend from "../backend/commands";
import { say, t } from "../i18n/i18n";
import { avg, fileName, clock } from "../lib/format";
import { indicators } from "../overlay/indicators";
import { screen } from "../stream/screen";
import { activeSession, getViewer, humanHasDevice } from "../store/viewer";
import { panelText } from "../features/session/panelText";
import { setupStates, timeLeft, setupMessage } from "../features/setup/setup";
import { stats } from "./stats";
import { deviceName } from "./text";

export function report(): void {
  const v = getViewer();
  const d = stats.decodeMs.slice(-120);
  const s = activeSession(v);
  const panel = s ? panelText(s, t) : null;
  const device = document.getElementById("device");
  const canvas = document.getElementById("screen") as HTMLCanvasElement | null;
  const o = indicators.snapshot();
  const setup = v.setup;
  const pairing = v.pairing;
  void backend
    .report({
      at: new Date().toISOString(),
      userAgent: navigator.userAgent,
      webcodecs: typeof VideoDecoder !== "undefined",
      supported: stats.supported,
      codec: stats.codec,
      received: stats.received,
      decoded: stats.decoded,
      dropped: stats.dropped,
      errors: stats.errors,
      lastError: stats.lastError,
      fps: stats.fpsWindow.length,
      decodeMsAvg: d.length ? avg(d) : null,
      decodeMsMax: d.length ? Math.max(...d) : null,
      inputs: stats.inputs,
      lastInput: stats.lastInput,
      touchMsAvg: stats.touchMs.length ? avg(stats.touchMs) : null,
      touchMsMax: stats.touchMs.length ? Math.max(...stats.touchMs) : null,
      touchErrors: stats.touchErrors,
      control: v.control,
      // What the window shows, so a check of the viewer doesn't need anyone to look.
      stage: v.stage,
      setup: setup
        ? {
            steps: Object.entries(setupStates(setup)).map(([step, state]) => `${state}${setup.run?.step === step ? " running" : ""}`),
            running: setup.run
              ? {
                  step: setup.run.step,
                  message: setup.run.progress?.message ? setupMessage(setup.run.progress.message) : null,
                  waiting: !!setup.run.progress?.waiting_for_user,
                  left: timeLeftText(setup.run.waitingSince),
                }
              : null,
            error: setup.error ? say(setup.error.text) : null,
            button: document.querySelector("#intro-card button")?.textContent ?? null,
          }
        : null,
      pairing: pairing
        ? {
            phase: pairing.phase,
            code: pairing.code ?? null,
            left: timeLeftText(pairing.waitingSince),
            error: pairing.error ? say(pairing.error) : null,
            buttons: [...document.querySelectorAll("#intro-card button")].map((b) => b.textContent),
          }
        : null,
      wantView: v.wantView,
      zoom: v.zoom,
      scale: Math.round(v.scale * 1000) / 1000,
      device: { kind: device?.dataset.kind ?? null, size: [device?.offsetWidth ?? 0, device?.offsetHeight ?? 0] },
      panels: { sidebar: v.sidebar, inspector: v.inspector },
      panel: panel && s ? { name: s.name, state: panel.state, button: panel.button } : null,
      touchBlock: { on: !!v.attachedUdid && !humanHasDevice(v), hint: o.blockedHint, blocked: stats.touchesBlocked },
      glow: s ? (s.owner !== "agent" ? "human" : "agent") : null,
      labelsShown: stats.labelsShown,
      showTouches: v.showTouches,
      touchDotsOnScreen: o.touchDotsOnScreen,
      agentCursor: o.agentCursor,
      overlay: o.overlay,
      // The canvas holds just the screen: its aspect should match `screen`'s.
      canvas: [canvas?.width ?? 0, canvas?.height ?? 0],
      screen: screen.geometry?.screen ?? null,
      frame: stats.frame ?? null,
      rotation: screen.geometry?.rotation ?? null,
      orientations: stats.orientations,
      window: [window.innerWidth, window.innerHeight],
      visibility: document.visibilityState,
      timelineMs: document.timeline.currentTime,
      session_slot: s ? "panel" : "idle",
      recording: v.recording ? { file: fileName(v.recording.path), label: clock((performance.now() - v.recording.since) / 1000) } : null,
      note: v.note ? v.note.text.replace("{file}", v.note.path ? fileName(v.note.path) : "") : null,
      sessionNotice: v.notice ? `${t("notice.agentStarted", { device: deviceName(v.notice.device, t) })} ${v.notice.session.name}` : null,
      sessions: Object.fromEntries(Object.entries(v.sessions).map(([u, x]) => [u, x.owner])),
    })
    .catch(() => {});
}

function timeLeftText(since: number | undefined): string | null {
  const left = timeLeft(since);
  return left ? t("setup.timeLeft", { time: left.time }) : null;
}
