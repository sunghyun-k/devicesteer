// Setup: a device that isn't ready (not trusted, Developer Mode off, no disk image) gets a card
// with the three steps instead of View Screen, kept current while it shows.

import * as backend from "../../backend/commands";
import { SETUP_ORDER, type DeviceInfo, type SetupStatus, type SetupStep, type StepState } from "../../backend/types";
import { reselect } from "../../device/connection";
import { msg, num, type Text } from "../../i18n/i18n";
import { getViewer, setViewer, type SetupCard } from "../../store/viewer";

/** How long a step waits for the person holding the device: tapping Trust, or turning Developer
 * Mode on, which takes a restart and a confirmation. Passed to `setup_run`, so the countdown on
 * the card ends when the core gives up. Dev builds take `?setupwait=<seconds>` to see the end
 * without waiting ten minutes. */
export const SETUP_WAIT_S = (import.meta.env.DEV && Number(new URLSearchParams(location.search).get("setupwait"))) || 600;
/** Re-read the state this often while the card shows: the person may have done a step on the
 * device by themselves (trusted it in Finder, turned Developer Mode on). */
const SETUP_POLL_MS = 3000;

/** Bumped when the card is put away, so a late reply doesn't redraw it. */
let setupGen = 0;
let pollTimer: number | undefined;

const card = (): SetupCard | undefined => getViewer().setup;
const patch = (p: Partial<SetupCard>) => setViewer((v) => (v.setup ? { setup: { ...v.setup, ...p } } : {}));

/** Each step's state: from the last reading, or before there is one, from the device list. */
export function setupStates(c: SetupCard): Record<SetupStep, StepState> {
  const out = {} as Record<SetupStep, StepState>;
  if (c.status) {
    for (const s of c.status.steps) out[s.step] = s.state;
    return out;
  }
  const at = SETUP_ORDER.indexOf(c.device.setup ?? "disk_image");
  SETUP_ORDER.forEach((s, i) => (out[s] = i < at ? "done" : i === at ? "needed" : "blocked"));
  return out;
}

/** Shows the setup card for `d` and keeps it current. */
export function showSetup(d: DeviceInfo): void {
  if (card()?.device.udid !== d.udid) {
    hideSetup();
    setViewer({ setup: { device: d } });
    pollTimer = window.setInterval(() => void pollSetup(), SETUP_POLL_MS);
    void pollSetup();
    return;
  }
  patch({ device: d });
}

/** Puts the card away and cancels a step still running (pairing waits minutes for a tap). */
export function hideSetup(): void {
  if (card()?.run) void backend.setupCancel().catch(() => {});
  setupGen++;
  window.clearInterval(pollTimer);
  pollTimer = undefined;
  setViewer({ setup: undefined });
}

async function pollSetup(): Promise<void> {
  const c = card();
  // A running step reads the device itself; a second reading meanwhile could get in its way.
  if (!c || c.run || document.hidden) return;
  const gen = setupGen;
  try {
    const st = await backend.setupStatus(c.device.udid);
    if (gen === setupGen && !card()?.run) onSetupStatus(st);
  } catch {
    // Unplugged or restarting: the device list says so; the card keeps its last state.
  }
}

/** Takes a new reading: redraws, mirrors it in the sidebar row, and leaves setup once ready. */
function onSetupStatus(st: SetupStatus): void {
  const c = card();
  if (!c) return;
  const step = st.step ?? undefined;
  const v = getViewer();
  const device = { ...c.device, setup: step };
  setViewer({
    setup: { ...c, device, status: st },
    devices: v.devices.map((x) => (x.udid === device.udid ? { ...x, setup: step } : x)),
    selected: v.selected?.udid === device.udid ? { ...v.selected, setup: step } : v.selected,
  });
  if (st.ready) {
    // Ready: the usual intro with View Screen, attached as if just picked.
    hideSetup();
    const now = getViewer().selected;
    if (now) reselect(now);
  }
}

/** Runs one step. */
export async function runSetup(step: SetupStep): Promise<void> {
  const c = card();
  if (!c || c.run) return;
  const gen = setupGen;
  patch({ run: { step }, error: undefined });
  const waited = () => card()?.run?.waitingSince !== undefined;
  try {
    const st = await backend.setupRun(c.device.udid, step, SETUP_WAIT_S, (p) => {
      const run = card()?.run;
      if (gen !== setupGen || !run) return;
      // The core starts its clock as it first asks for the person (`trust`, `developer_mode`).
      patch({ run: { ...run, progress: p, waitingSince: run.waitingSince ?? (p.waiting_for_user ? performance.now() : undefined) } });
    });
    if (gen !== setupGen) return;
    const w = waited();
    // Developer Mode ends its wait without an error: the state afterwards still needs the step.
    patch({ run: undefined, error: w && st.steps.find((x) => x.step === step)?.state === "needed" ? { step, text: setupTimeoutText(step) } : undefined });
    onSetupStatus(st);
  } catch (e) {
    if (gen !== setupGen) return;
    const w = waited();
    // Pairing that nobody answered fails "... within 600s" (core `setup::trust`).
    const error = String(e) === "cancelled" ? undefined : { step, text: w && /\bwithin \d+s\b/.test(String(e)) ? setupTimeoutText(step) : setupErrorText(e) };
    patch({ run: undefined, error });
    void pollSetup();
  }
}

export function cancelSetup(): void {
  void backend.setupCancel().catch(() => {});
}

/** How long a wait for the person lasts, in words: "10 minutes". */
export function waitText(): string {
  const m = SETUP_WAIT_S / 60;
  const [n, unit] = Number.isInteger(m) ? [m, "minute"] : [SETUP_WAIT_S, "second"];
  return num(n, { style: "unit", unit, unitDisplay: "long" });
}

/** What the card says when the wait ran out. The duration is written when it shows. */
function setupTimeoutText(step: SetupStep): Text {
  return msg(step === "trust" ? "setup.trust.timeout" : "setup.developerMode.timeout", { duration: waitText() });
}

/** The time left of a wait that began at `since`, as "9:41 left" (`low` in its last minute). */
export function timeLeft(since: number | undefined, now = performance.now()): { time: string; low: boolean } | undefined {
  if (since === undefined) return undefined;
  const left = Math.max(0, Math.ceil(SETUP_WAIT_S - (now - since) / 1000));
  return { time: `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}`, low: left < 60 };
}

/** A progress message as the card shows it: the core writes "Settings > Privacy & Security" and
 * leaves the final period off. It stays in English: the core has no codes for its messages. */
export function setupMessage(m: string): string {
  const t = m.replace(/ > /g, " › ");
  return /[.!?…]$/.test(t) ? t : `${t}.`;
}

/** A setup error as the viewer shows it: the core writes for agents ("run `devicesteer setup
 * trust` again"); here the step's own button does that. It stays in English, as the core wrote
 * it. */
export function setupErrorText(e: unknown): string {
  const t = String(e)
    .replace(/:? run `devicesteer setup[^`]*` first/g, "")
    .replace(/,? (?:then )?run `devicesteer setup(?: [\w-]+)?`( again)?/g, (_m, again) => (again ? ", then press Try Again" : ""))
    .replace(/`([^`]+)`/g, "$1");
  return t.charAt(0).toUpperCase() + t.slice(1) + (/[.!?]$/.test(t) ? "" : ".");
}
