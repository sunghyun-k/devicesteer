import assert from "node:assert/strict";
import { test } from "vitest";
import { canvasSize, panelToCanvas, screenRect } from "./geometry";

const apply = (m: number[], [x, y]: [number, number]) => [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]];

test("the canvas turns with the interface", () => {
  assert.deepEqual(canvasSize([1284, 2778], "rot0"), [1284, 2778]);
  assert.deepEqual(canvasSize([1284, 2778], "rot270"), [2778, 1284]);
});

test("rot270: the panel's top-left corner is the screen's bottom-left", () => {
  const m = panelToCanvas("rot270", 1284, 2778);
  assert.deepEqual(apply(m, [0, 0]), [0, 1284]);
  assert.deepEqual(apply(m, [1284, 2778]), [2778, 0]);
});

test("rot90 and rot180 keep the panel on the canvas", () => {
  assert.deepEqual(apply(panelToCanvas("rot90", 10, 20), [10, 20]), [0, 10]);
  assert.deepEqual(apply(panelToCanvas("rot180", 10, 20), [10, 20]), [0, 0]);
});

test("WebKit's frame: padding inside the visible rect (PITFALLS #26)", () => {
  const f = { visibleRect: { x: 0, y: 0, width: 1296, height: 2816 }, displayWidth: 1296, displayHeight: 2816 };
  assert.deepEqual(screenRect(f, [1284, 2778]), [0, 0, 1284, 2778]);
});

test("a scaled display size scales the screen's rectangle", () => {
  const f = { visibleRect: { x: 0, y: 0, width: 1296, height: 2816 }, displayWidth: 648, displayHeight: 1408 };
  assert.deepEqual(screenRect(f, [1284, 2778]), [0, 0, 642, 1389]);
});

test("no visible rect: the whole screen", () => {
  assert.deepEqual(screenRect({ visibleRect: null, displayWidth: 1, displayHeight: 1 }, [5, 6]), [0, 0, 5, 6]);
});
