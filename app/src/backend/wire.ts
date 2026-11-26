// The daemon's wire format as the shell forwards it (`devicesteer_daemon::wire`): a 4-byte length,
// a kind byte, then for kind 0 a JSON event, for kind 1 a video frame (a 4-byte header length,
// the JSON header, the frame's bytes).

/** A video frame's header. */
export type FrameHeader = { keyframe: boolean; pts_us: number };

export type WireMessage =
  | { kind: "event"; event: { event: string } & Record<string, unknown> }
  | { kind: "frame"; header: FrameHeader; data: Uint8Array }
  | { kind: "unknown" };

const decoder = new TextDecoder();

/** Parses one message. The frame's data is a view into `buf`, not a copy. */
export function parseWire(buf: Uint8Array): WireMessage {
  const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  const kind = buf[4];
  const payload = buf.subarray(5);
  if (kind === 0) return { kind: "event", event: JSON.parse(decoder.decode(payload)) };
  if (kind === 1) {
    const hl = view.getUint32(5);
    const header = JSON.parse(decoder.decode(payload.subarray(4, 4 + hl))) as FrameHeader;
    return { kind: "frame", header, data: payload.subarray(4 + hl) };
  }
  return { kind: "unknown" };
}
