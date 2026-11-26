// Where the screen is in the video picture and how it turns: pure arithmetic, unit-tested
// (geometry.test.ts).

import type { Rotation } from "../backend/types";

export type Transform = [number, number, number, number, number, number];

export const quarterTurn = (r: Rotation | undefined): boolean => r === "rot90" || r === "rot270";

/** The canvas size for the screen as the user sees it: the panel's pixels, turned. */
export function canvasSize(displayPx: [number, number], rotation: Rotation | undefined): [number, number] {
  const [pw, ph] = displayPx;
  return quarterTurn(rotation) ? [ph, pw] : [pw, ph];
}

/** Maps the panel's pixels (the picture) onto the canvas (the screen as the user sees it), as a
 * canvas transform. The inverse of `Rotation::turn_share` in `crates/core/src/display.rs`. */
export function panelToCanvas(r: Rotation | undefined, pw: number, ph: number): Transform {
  switch (r) {
    case "rot90":
      return [0, 1, -1, 0, ph, 0];
    case "rot180":
      return [-1, 0, 0, -1, pw, ph];
    case "rot270":
      return [0, -1, 1, 0, 0, pw];
    default:
      return [1, 0, 0, 1, 0, 0];
  }
}

/** What `screenRect` needs of a decoded frame. */
export interface FrameRect {
  visibleRect: { x: number; y: number; width: number; height: number } | null;
  displayWidth: number;
  displayHeight: number;
}

/** Where the screen (`display_px`, from the coded picture's top-left corner) is in a decoded
 * frame, in the frame's display size, which `drawImage` draws and which covers its visible rect
 * (the decoder may keep the padding in it, as WebKit's does, or scale the display size). */
export function screenRect(frame: FrameRect, displayPx: [number, number]): [number, number, number, number] {
  const [pw, ph] = displayPx;
  const v = frame.visibleRect;
  if (!v || !v.width || !v.height) return [0, 0, pw, ph];
  const kx = frame.displayWidth / v.width;
  const ky = frame.displayHeight / v.height;
  const x0 = Math.max(0, v.x);
  const y0 = Math.max(0, v.y);
  const x1 = Math.min(pw, v.x + v.width);
  const y1 = Math.min(ph, v.y + v.height);
  return [(x0 - v.x) * kx, (y0 - v.y) * ky, (x1 - x0) * kx, (y1 - y0) * ky];
}
