// Agent sessions: the attached device's (its daemon's `control` events, the panel's Take over /
// Hand back / Done / Stop) and every other device's (the daemons' messages through the shell).
//
// The session's glow and panel stay up for as long as the session runs, not only while input
// arrives, so the person watching always knows an agent holds the device. A session that starts
// while the screen isn't shown shows it: the person should see what the agent does.
//
// A session that starts on another device shows its screen at once: the device is selected when
// nothing else is, or when the device on screen is only selected, not watched. A device the person
// watches or works on (an agent session, a recording, a setup step, pairing) is never taken away:
// a notice says where the agent started, with View.

import * as backend from "../../backend/commands";
import type { AgentSession, AgentSessionMessage, ControlEvent, DeviceInfo } from "../../backend/types";
import { select, send, showScreen } from "../../device/connection";
import { endPress, pressing } from "../../input/touch";
import { indicators } from "../../overlay/indicators";
import { activeSession, getViewer, humanHasDevice, setViewer } from "../../store/viewer";
import { loadDevices } from "../devices/deviceList";

/** What the panel's main button does for each owner. */
export function panelAction(s: AgentSession): "done" | "hand_back" | "take_over" {
  return s.owner === "delegated_to_user" ? "done" : s.owner === "user" ? "hand_back" : "take_over";
}

/** The panel's buttons. */
export const controlSession = (action: "done" | "hand_back" | "take_over" | "stop") => void send("control", { action }).catch(() => {});

function setSession(udid: string, s: AgentSession | null): void {
  setViewer((v) => {
    const sessions = { ...v.sessions };
    if (s) sessions[udid] = s;
    else delete sessions[udid];
    return { sessions };
  });
}

/** The attached device's `control` event. */
export function onControl(c: ControlEvent): void {
  const v = getViewer();
  const was = activeSession(v)?.id ?? null;
  setViewer({ control: c });
  const s = activeSession({ control: c });
  if (v.attachedUdid) {
    setSession(v.attachedUdid, s);
    if (!s) hideSessionNotice(v.attachedUdid);
  }
  // The cursor marks where this session's agent works; it goes with the session. Input sent
  // without a session keeps it until the device changes.
  if (was && s?.id !== was) indicators.hideCursor();
  // The agent got the device: a press already down ends here; otherwise the hint can go.
  if (humanHasDevice({ control: c })) indicators.hideBlocked();
  else endPress();
  if (s && s.id !== was && !getViewer().wantView && v.attachedUdid) void showScreen();
}

export function showSessionNotice(device: DeviceInfo, session: AgentSession): void {
  setViewer({ notice: { device, session } });
}

/** Hides the notice; with `udid`, only when it is about that device. */
export function hideSessionNotice(udid?: string): void {
  const n = getViewer().notice;
  if (!n || (udid && n.device.udid !== udid)) return;
  setViewer({ notice: undefined });
}

/** The notice's View: that device's screen. */
export function viewNoticeDevice(): void {
  const n = getViewer().notice;
  hideSessionNotice();
  if (n) void select(getViewer().devices.find((x) => x.udid === n.device.udid) ?? n.device, { view: true });
}

/** Whether the device on screen may give way to one where a session just started: the person
 * isn't watching it or doing anything with it. */
function mayLeaveSelected(): boolean {
  const v = getViewer();
  if (!v.selected || v.gone) return true;
  return !v.wantView && !activeSession(v) && !v.recording && !v.setup?.run && !v.pairing && !pressing() && !v.control.human_touching;
}

/** A daemon's message: a session started, changed owner or ended on `udid`. */
function onAgentSession(p: AgentSessionMessage): void {
  const v = getViewer();
  // The attached device's control events say the same, and say it first.
  if (p.udid !== v.attachedUdid) setSession(p.udid, p.session);
  if (!p.session) hideSessionNotice(p.udid);
  if (!p.started || !p.session) return;
  const d = v.devices.find((x) => x.udid === p.udid) ?? { udid: p.udid, transports: [] };
  // A device that isn't listed yet (Wi-Fi, before the next look): the list catches up.
  if (!v.devices.some((x) => x.udid === p.udid)) void loadDevices();
  if (v.selected?.udid === p.udid) {
    if (v.attachedUdid === p.udid) void showScreen();
    else if (!v.gone) void select(d, { view: true });
    return;
  }
  if (mayLeaveSelected()) void select(d, { view: true });
  else showSessionNotice(d, p.session);
}

/** Follows the sessions on every device. Installed once for the page. */
export function followSessions(): void {
  void backend.onAgentSession(onAgentSession).catch(() => {});
}
