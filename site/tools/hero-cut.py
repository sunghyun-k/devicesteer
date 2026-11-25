#!/usr/bin/env python3
"""Cut the homepage hero clip from a raw take, and remap its input timeline.

    python3 site/tools/hero-cut.py <take.mp4> <timeline.json> <out-dir>

The take is a `devicesteer record` MP4 of an agent session; the timeline is its input events
(`inputs[].t_ms`, `kind`, `label`). The output is hero.webm (AV1), hero.mp4 (H.264), hero-poster.webp
(the first frame) and hero-inputs.json: the inputs at their times in the cut, for INPUTS in
site/hero.js. The media is not committed (AGENTS.md: large media goes to external storage).

EDITS lists the source ranges kept, in order, with a playback speed. They were chosen from
`ffmpeg -vf freezedetect=n=0.0005:d=0.1` on the take, not by eye: each input keeps 0.6 s or more
before it (the overlay's 320 ms glide plus a beat), the press, and the screen's whole reaction
until the frame is still again; idle stretches between them are cut to about half a second. Only
frames freezedetect calls still are sped up, so no motion changes pace. A new take needs new
EDITS: rerun freezedetect and keep the same rules.

The take ends in Dark mode and starts in Light, so the clip ends with a crossfade from the last
(dark) Home Screen into the first (light) one: the loop restarts on the very frame it fades to.
"""
import json
import os
import subprocess
import sys

# (source start s, source end s, speed). Comments: what the source range shows.
EDITS = [
    (3.80, 5.77, 1),   # still Home Screen, glide in, tap Settings (4.719), Settings opens (motion to 5.67)
    (5.77, 6.37, 4),   # still: Settings open, nothing moves
    (6.37, 7.00, 1),   # a last settle (6.37-6.53), then a short pause
    (8.79, 10.75, 1),  # glide in, tap Appearance (9.393), push transition (to 10.27), pause
    (12.47, 14.55, 1), # glide in, tap Dark (13.072), the dark fade (to 14.05), pause
    (15.06, 15.90, 1), # pause, Home pressed (15.661); the device answers at 16.60
    (15.90, 16.56, 3), # still: waiting for the device
    (16.56, 18.25, 1), # back to the Home Screen (to 17.83), still
]
# The seam: the last LOOP_FADE seconds fade into the first frame of the cut (light Home Screen).
LOOP_FADE = 0.4
CROP = "crop=1398:2034:0:0"  # the screen: the take's coded picture has 10 + 14 px of padding
SIZE = "scale=720:1048:flags=lanczos"
FPS = 30


def out_time(t):
    """Where source time `t` (s) lands in the cut, or None if it was cut."""
    at = 0.0
    for start, end, speed in EDITS:
        if start <= t < end:
            return at + (t - start) / speed
        at += (end - start) / speed
    return None


def main(take, timeline, out):
    parts, labels = [], []
    for i, (start, end, speed) in enumerate(EDITS):
        parts.append(f"[s{i}]trim={start}:{end},setpts=(PTS-STARTPTS)/{speed}[p{i}]")
        labels.append(f"[p{i}]")
    n = len(EDITS)
    total = sum((e - s) / v for s, e, v in EDITS)
    first = EDITS[0][0]
    graph = ";".join([
        f"[0:v]{CROP},fps={FPS},{SIZE},format=yuv420p,split={n + 1}" + "".join(f"[s{i}]" for i in range(n)) + "[h]",
        *parts,
        "".join(labels) + f"concat=n={n}:v=1:a=0,fps={FPS},settb=1/{FPS}[body]",
        f"[h]trim={first}:{first + LOOP_FADE},setpts=PTS-STARTPTS,settb=1/{FPS}[head]",
        f"[body][head]xfade=transition=fade:duration={LOOP_FADE}:offset={total - LOOP_FADE:.3f}[cut]",
    ])
    tmp = f"{out}/hero-cut.mov"  # an intermediate, removed at the end
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-i", take, "-filter_complex", graph, "-map", "[cut]",
                    "-an", "-c:v", "prores_ks", "-profile:v", "3", tmp], check=True)
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-i", tmp, "-an", "-c:v", "libx264", "-crf", "23",
                    "-preset", "slow", "-profile:v", "high", "-pix_fmt", "yuv420p", "-movflags", "+faststart",
                    f"{out}/hero.mp4"], check=True)
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-i", tmp, "-an", "-c:v", "libsvtav1", "-crf", "34",
                    "-preset", "6", "-pix_fmt", "yuv420p", f"{out}/hero.webm"], check=True,
                   env={**os.environ, "SVT_LOG": "1"})
    subprocess.run(["ffmpeg", "-y", "-v", "error", "-i", tmp, "-frames:v", "1", f"{out}/hero-poster.png"], check=True)
    subprocess.run(["cwebp", "-quiet", "-q", "80", f"{out}/hero-poster.png", "-o", f"{out}/hero-poster.webp"], check=True)

    os.remove(tmp)
    os.remove(f"{out}/hero-poster.png")

    inputs = []
    for e in json.load(open(timeline))["inputs"]:
        t = out_time(e["t_ms"] / 1000)
        if t is None:
            sys.exit(f"input at {e['t_ms']} ms falls in a cut: widen EDITS")
        inputs.append({"t": round(t * 1000), "kind": e["kind"], "label": e["label"], "source_t": e["t_ms"]})
    json.dump({"duration_ms": round(total * 1000), "inputs": inputs}, open(f"{out}/hero-inputs.json", "w"), indent=2)
    at = 0.0
    for s, e, v in EDITS:
        d = (e - s) / v
        print(f"{s:6.2f}-{e:6.2f} x{v} -> {at:5.2f}-{at + d:5.2f}")
        at += d
    print(f"loop fade {LOOP_FADE}s at {total - LOOP_FADE:.2f}; total {total:.2f}s")
    print(json.dumps(inputs))


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    main(*sys.argv[1:])
