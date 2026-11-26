// The zoom: the size of one screen point in CSS pixels. "Fit" works it out from the room the stage
// has (Stage.tsx's layout); "actual size" is one point per CSS pixel.

import { screen } from "../../stream/screen";
import { getViewer, setViewer, type Zoom } from "../../store/viewer";

export const ZOOM_MIN = 0.2;
export const ZOOM_MAX = 3;

export function setZoom(z: Zoom): void {
  if (!screen.geometry) return;
  const zoom = typeof z === "number" ? Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, z)) : z;
  // The stage lays the screen out again and scrolls to the middle of what shows.
  setViewer((s) => ({ zoom, zoomSeq: s.zoomSeq + 1 }));
}

/** Zooms in (`k` > 1) or out from the scale the last layout used. */
export const zoomBy = (k: number) => setZoom(getViewer().scale * k);
