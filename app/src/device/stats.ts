// The numbers the page keeps about the stream and input: shown in the inspector's Info pane and
// reported to the backend's log (viewer.log, report.ts). Plain mutable data, written on the hot
// paths without touching React; the Info pane reads it once a second.

export const stats = {
  received: 0,
  decoded: 0,
  dropped: 0,
  errors: 0,
  lastError: "",
  supported: undefined as boolean | undefined,
  codec: "",
  /** Time from a frame reaching the page to the decoder handing out its picture. */
  decodeMs: [] as number[],
  fpsWindow: [] as number[],
  inputs: { agent: 0, human: 0 } as Record<string, number>,
  lastInput: "",
  /** Round trip of a human touch step: sent from the page until the daemon wrote the report. */
  touchMs: [] as number[],
  touchErrors: 0,
  /** `orientation` events received. */
  orientations: 0,
  /** The first decoded frame's geometry after each config, and the part of it drawn. */
  frame: undefined as { coded: number[]; visible: number[]; display: number[]; drawn: number[] } | undefined,
  /** Labels shown so far. */
  labelsShown: 0,
  /** Presses the viewer kept from the device while the agent had it. */
  touchesBlocked: 0,
};

/** Clears what belongs to one device's attachment. */
export function resetStats(): void {
  Object.assign(stats, { received: 0, decoded: 0, dropped: 0, errors: 0, lastError: "", decodeMs: [], touchMs: [], fpsWindow: [] });
}
