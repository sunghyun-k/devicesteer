// The live screen: decodes the daemon's HEVC stream with WebCodecs and draws it on the canvas.
// Not React: frames arrive 20 to 60 times a second and go from the wire to the decoder to the
// canvas without touching component state. One instance (`screen`) lives for the page; the
// screen component only lends it a canvas (`attachCanvas`) and takes it back on unmount, so a
// remount (or StrictMode's second effect run) never makes a second decoder or restarts the stream.
// React learns only what changes rarely: the screen's geometry (`subscribe`, `geometry`).

import type { Config, Orientation } from "../backend/types";
import type { FrameHeader } from "../backend/wire";
import { stats } from "../device/stats";
import { canvasSize, panelToCanvas, screenRect } from "./geometry";

/** What the page around the canvas needs to know: the screen in points, how it is turned, and the
 * picture's size. Replaced (never changed in place) on every `config`, turn or panel change. */
export interface Geometry {
  screen: [number, number];
  rotation: Config["rotation"];
  orientation: Config["orientation"];
  displayPx: [number, number];
  codec: string;
  /** A foldable switched panels: the size is the old panel's, kept for the layout until the new
   * stream's `config`, but no point maps onto it. */
  stale?: boolean;
}

function b64(s: string): Uint8Array {
  return Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
}

class ScreenStream {
  private config: Config | undefined;
  private decoder: VideoDecoder | undefined;
  /** Codec and parameter sets the current decoder was set up with. */
  private decoderKey = "";
  private waitingForKey = true;
  /** Frames that arrive while the decoder is being set up, from the last keyframe on: the device
   * sends one keyframe per stream, so dropping it means nothing decodes. */
  private pending: { header: FrameHeader; data: Uint8Array }[] = [];
  /** When each frame reached the page, by pts, for the decode time. */
  private received = new Map<number, number>();
  /** Bumped by every `config` and by `reset`, so a setup that finishes late gives way. */
  private configGen = 0;
  /** The last picture, kept to draw again when the screen turns between frames (the device sends
   * none while the screen is still) and when a canvas is attached. */
  private lastFrame: VideoFrame | undefined;
  private canvas: HTMLCanvasElement | undefined;
  private ctx: CanvasRenderingContext2D | undefined;
  private listeners = new Set<() => void>();
  private geo: Geometry | undefined;

  /** Called after every picture drawn (the page switches to the live screen on the first). */
  onPicture: () => void = () => {};

  // ---------- for React ----------

  /** The screen's geometry, or undefined while there is no stream (or a foldable switched panels
   * and the new stream's `config` hasn't come). */
  get geometry(): Geometry | undefined {
    return this.geo;
  }

  subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  getGeometry = (): Geometry | undefined => this.geo;

  private publish(): void {
    const c = this.config;
    this.geo = c ? { screen: c.screen, rotation: c.rotation, orientation: c.orientation, displayPx: c.display_px, codec: c.codec } : undefined;
    for (const fn of this.listeners) fn();
  }

  /** Lends the stream a canvas to draw on; the last picture is drawn at once. */
  attachCanvas(canvas: HTMLCanvasElement): void {
    if (this.canvas === canvas) return;
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d") ?? undefined;
    this.sizeCanvas();
    if (this.lastFrame) this.draw(this.lastFrame);
  }

  /** Takes the canvas back (the component unmounted). Decoding goes on. */
  detachCanvas(canvas: HTMLCanvasElement): void {
    if (this.canvas !== canvas) return;
    this.canvas = undefined;
    this.ctx = undefined;
  }

  /** Whether a picture has been decoded since the stream (re)started: what a screenshot needs. */
  get hasPicture(): boolean {
    return !!this.lastFrame;
  }

  /** The last picture as a PNG, at the panel's full resolution turned upright, without the
   * overlay (cursor, labels, touch discs), which is drawn over the canvas in HTML. */
  png(): Promise<Blob | null> {
    const c = this.canvas;
    return c ? new Promise((done) => c.toBlob(done, "image/png")) : Promise.resolve(null);
  }

  // ---------- the daemon's events ----------

  async configure(cfg: Config): Promise<void> {
    const gen = ++this.configGen;
    this.config = cfg;
    this.sizeCanvas();
    stats.frame = undefined;
    this.publish();
    // A viewer joining a running stream gets the last config and then, once the stream restarts
    // for a keyframe (PITFALLS #13), a second one right before that keyframe. Same parameter sets:
    // the decoder in use takes the new keyframe. Different: the old decoder is dropped now, not
    // after the support check below, or it swallows the stream's only keyframe while the check
    // runs and the new decoder never sees one (PITFALLS #23).
    const key = `${cfg.codec}|${cfg.description}`;
    if (this.decoder?.state === "configured" && key === this.decoderKey) return;
    this.decoder?.close();
    this.decoder = undefined;
    this.decoderKey = "";
    this.pending = [];
    if (typeof VideoDecoder === "undefined") {
      stats.supported = false;
      stats.lastError = "WebCodecs VideoDecoder is not available in this web view";
      return;
    }
    const decoderConfig: VideoDecoderConfig = {
      codec: cfg.codec,
      description: b64(cfg.description),
      // Without the size, WebView2 (Chromium) gave every frame a 1280x720 visible rect and display
      // size, a corner of the real picture (PITFALLS #57).
      codedWidth: cfg.coded[0],
      codedHeight: cfg.coded[1],
      optimizeForLatency: true,
      hardwareAcceleration: "prefer-hardware",
    };
    // On macOS this is the system WebKit's decoder: macOS 14 is the oldest that decodes hvc1, hence
    // the app's minimum system version (PITFALLS #69).
    try {
      const support = await VideoDecoder.isConfigSupported(decoderConfig);
      stats.supported = !!support.supported;
      stats.codec = cfg.codec;
      if (!support.supported) {
        stats.lastError = `decoder does not support ${cfg.codec}`;
        return;
      }
    } catch (e) {
      stats.supported = false;
      stats.lastError = String(e);
      return;
    }
    if (gen !== this.configGen) return;
    const decoder = new VideoDecoder({
      output: (frame) => {
        const t = this.received.get(frame.timestamp);
        if (t !== undefined) {
          stats.decodeMs.push(performance.now() - t);
          this.received.delete(frame.timestamp);
        }
        this.draw(frame);
        this.lastFrame?.close();
        this.lastFrame = frame;
        stats.decoded++;
        stats.fpsWindow.push(performance.now());
        this.onPicture();
      },
      error: (e) => {
        stats.errors++;
        stats.lastError = String(e);
        this.waitingForKey = true;
      },
    });
    this.decoder = decoder;
    decoder.configure(decoderConfig);
    this.decoderKey = key;
    this.waitingForKey = true;
    const queued = this.pending;
    this.pending = [];
    for (const f of queued) this.frame(f.header, f.data, false);
  }

  frame(header: FrameHeader, data: Uint8Array, fresh = true): void {
    if (fresh) stats.received++;
    const decoder = this.decoder;
    if (!decoder || decoder.state !== "configured") {
      if (header.keyframe) this.pending = [{ header, data: data.slice() }];
      else if (this.pending.length && this.pending.length < 600) this.pending.push({ header, data: data.slice() });
      return;
    }
    if (this.waitingForKey && !header.keyframe) {
      stats.dropped++;
      return;
    }
    this.waitingForKey = false;
    this.received.set(header.pts_us, performance.now());
    if (this.received.size > 120) this.received.clear();
    try {
      decoder.decode(new EncodedVideoChunk({ type: header.keyframe ? "key" : "delta", timestamp: header.pts_us, data }));
    } catch (e) {
      stats.errors++;
      stats.lastError = String(e);
      this.waitingForKey = true;
    }
  }

  /** The screen turned: the canvas takes the new shape and the last picture is drawn again turned.
   * Returns whether it turned (the caller drops what was drawn in the old orientation). */
  orientation(o: Orientation): boolean {
    stats.orientations++;
    if (!this.config) return false;
    const turned = this.config.rotation !== o.rotation;
    this.config = { ...this.config, rotation: o.rotation, orientation: o.orientation, screen: o.screen };
    if (!turned) return false;
    this.sizeCanvas();
    if (this.lastFrame) this.draw(this.lastFrame);
    this.publish();
    return true;
  }

  /** A foldable switched panels (PITFALLS #47): the picture belongs to the old panel. The canvas
   * keeps it but nothing maps onto it until the new stream's `config`. */
  panel(): void {
    this.config = undefined;
    const g = this.geo;
    this.geo = g ? { ...g, stale: true } : undefined;
    for (const fn of this.listeners) fn();
  }

  /** Drops the video and everything drawn from it. */
  reset(): void {
    this.configGen++;
    this.decoder?.close();
    this.decoder = undefined;
    this.decoderKey = "";
    this.pending = [];
    this.received.clear();
    this.waitingForKey = true;
    this.config = undefined;
    this.lastFrame?.close();
    this.lastFrame = undefined;
    stats.frame = undefined;
    this.publish();
  }

  // ---------- drawing ----------

  /** Sizes the canvas to the screen as the user sees it: the panel's pixels, turned. */
  private sizeCanvas(): void {
    const c = this.canvas;
    if (!c || !this.config) return;
    const [w, h] = canvasSize(this.config.display_px, this.config.rotation);
    if (c.width !== w || c.height !== h) {
      c.width = w;
      c.height = h;
    }
  }

  private draw(frame: VideoFrame): void {
    const ctx = this.ctx;
    const config = this.config;
    if (!config || !ctx) return;
    // The screen is the top-left part of the coded picture; the rest is the encoder's padding
    // to its block size, black. WebKit ignores the source rectangle when it draws a VideoFrame
    // and squeezes the whole picture, padding included, into the destination: a black band
    // at the right and bottom (PITFALLS #26). So the whole frame is drawn scaled up so that
    // the panel fills the canvas, and the canvas clips the padding off. The picture is the panel
    // in its native orientation whichever way the interface is turned (PITFALLS #27): the
    // transform turns it upright, and the padding then lies outside the canvas on every side.
    const [pw, ph] = config.display_px;
    // A visible rect smaller than the screen it holds is the decoder's mistake, not a crop
    // (PITFALLS #57): draw the screen from the coded picture instead.
    const v = frame.visibleRect;
    const cut =
      v && (v.width < pw || v.height < ph) && frame.codedWidth >= pw && frame.codedHeight >= ph
        ? new VideoFrame(frame, { visibleRect: { x: 0, y: 0, width: pw, height: ph }, displayWidth: pw, displayHeight: ph })
        : undefined;
    const src = cut ?? frame;
    // On the decoder's own frame first, so viewer.log shows what the decoder gave.
    let [sx, sy, sw, sh] = this.rect(frame);
    if (cut) [sx, sy, sw, sh] = this.rect(cut);
    const kx = pw / sw;
    const ky = ph / sh;
    ctx.setTransform(...panelToCanvas(config.rotation, pw, ph));
    ctx.drawImage(src, -sx * kx, -sy * ky, src.displayWidth * kx, src.displayHeight * ky);
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    cut?.close();
  }

  /** `screenRect`, noting the first frame's geometry after each config for viewer.log. */
  private rect(frame: VideoFrame): [number, number, number, number] {
    const px = this.config?.display_px ?? [this.canvas?.width ?? 0, this.canvas?.height ?? 0];
    const rect = screenRect(frame, px);
    const v = frame.visibleRect;
    if (!stats.frame && v && v.width && v.height) {
      stats.frame = {
        coded: [frame.codedWidth, frame.codedHeight],
        visible: [v.x, v.y, v.width, v.height],
        display: [frame.displayWidth, frame.displayHeight],
        drawn: rect.map((n) => Math.round(n * 100) / 100),
      };
    }
    return rect;
  }
}

/** The page's one stream. */
export const screen = new ScreenStream();
