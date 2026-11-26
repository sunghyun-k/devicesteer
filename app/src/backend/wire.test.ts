import assert from "node:assert/strict";
import { test } from "vitest";
import { parseWire } from "./wire";

const message = (kind: number, payload: Uint8Array) => {
  const buf = new Uint8Array(5 + payload.length);
  new DataView(buf.buffer).setUint32(0, 1 + payload.length);
  buf[4] = kind;
  buf.set(payload, 5);
  return buf;
};

test("an event", () => {
  const m = parseWire(message(0, new TextEncoder().encode('{"event":"panel"}')));
  assert.deepEqual(m, { kind: "event", event: { event: "panel" } });
});

test("a frame, inside a larger buffer", () => {
  const header = new TextEncoder().encode('{"keyframe":true,"pts_us":33333}');
  const p = new Uint8Array(4 + header.length + 3);
  new DataView(p.buffer).setUint32(0, header.length);
  p.set(header, 4);
  p.set([7, 8, 9], 4 + header.length);
  const whole = new Uint8Array(10 + 5 + p.length);
  whole.set(message(1, p), 10);
  const m = parseWire(whole.subarray(10));
  assert.equal(m.kind, "frame");
  if (m.kind !== "frame") return;
  assert.deepEqual(m.header, { keyframe: true, pts_us: 33333 });
  assert.deepEqual([...m.data], [7, 8, 9]);
});
