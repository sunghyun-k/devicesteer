# Pitfalls and reverse-engineering notes

Everything here was observed on real devices. Code comments cite entries as `PITFALLS #N`.
**If some code looks odd, read the entry it cites before "fixing" it.**

- Entries are numbered in the order they were found. **Numbers never change.** Add new ones at the
  end. When a later entry changes the picture, add a pointer to the old one instead of rewriting it.
- Keep private data out: no real names, device names, account details or third-party app names.
- This file restarted with the Rust rewrite. "Old #N" refers to `docs/PITFALLS.md` of the Swift
  implementation (git history up to `v0.7.1`). Those observations were made through Xcode's
  CoreDevice stack, not idevice; re-check them on the new path before relying on them.

## Index by topic

| Topic | Entries |
|---|---|
| Screen streams (displayservice) | #1, #2, #7, #11 |
| Connection and tunnel | #3, #4 |
| Screenshots | #5 |
| Touch and HID input | #6, #7, #8 |
| Display info | #9 |
| Accessibility tree | #10 |

## Entries

### 1. Never send `stopAll` to the display service

Old #10: after a stream teardown with `DisplayService.stop_all_streams`, `devicectl device capture
screenshot` stopped responding until the device was rebooted. The device's media daemon was wedged.

`stopAll` also ends **every** session's streams, not just ours: another viewer, a recording, or
Xcode's own Device Hub stream on the same device.

idevice's `DisplayServiceClient` only stops streams with `stopAll: true`. Don't call that. The
device also accepts a list of streams to stop, each named by the `RemoteSSRC` of its stream
configuration (observed by iphone-cli on iOS 27.2: stopping a recording's video that way left the
audio running and touches working). Stop only the streams this session started, found by that
`RemoteSSRC`. If our streams can't be found or won't stop, report an error; don't fall back to
`stopAll`. (iphone-cli's `src/device/media.rs` shows the request shapes; it does fall back to
`stopAll` in one case, which we don't copy.)

### 2. Old observations about streams and input

- Old #9: on build 24B5084k, touches went through `universalhidservice` with no media stream at all.
  iOS 27 added a screen-sharing path (`com.apple.coredevice.displayservice`) that iphone-cli uses
  for HID input; whether input on the new path needs a running stream is to be measured.
- The stream is plaintext RTP/HEVC over UDP inside the tunnel (idevice
  `core_device::display_stream`). jktcp 0.1.8 has `AdapterHandle::bind_udp` for that.

### 3. One RSD peer per tunnel

Old #44: opening a second RSD connection over the **same** CoreDevice tunnel that macOS's `remoted`
uses displaced `remoted`, and Xcode could not build to the device until it was replugged. The
device keeps one RSD peer per tunnel.

The Rust path opens its **own** tunnel through lockdown's CoreDeviceProxy (as pymobiledevice3 and
iphone-cli do), so it should not share `remoted`'s tunnel. Still unverified: after touching
connection code, run an Xcode build to the device while a session is open.

### 4. Wi-Fi needs a heartbeat, and timing differs from USB

- Over Wi-Fi the device closes CoreDeviceProxy and its other lockdown services unless a client
  answers its heartbeat ("Marco"/"Polo"). `Session::open` keeps a heartbeat task for Wi-Fi routes.
- Old #15–#22 were all Wi-Fi-only timing bugs (lost reports when a connection closed before the
  device read them, the HID service going quiet after an answered request). Test input and
  teardown changes on Wi-Fi too.

### 5. Screenshots

- Old #13: captures arrive as 16-bit PNG (about 10 MB for a 1290×2796 screen). Re-encoding to 8-bit
  makes them ~30× smaller. `screenshot` does not re-encode yet.
  Still true on the idevice path: an iPhone on iOS 27.2.1 over USB returned a 16-bit RGBA PNG of
  8.6 MB for 1284×2778 (about 2.9 s for the whole command in a debug build).
- Old #25: a capture succeeds with a black image while the screen is off.
- Old #51: a foldable iPhone has two panels. Without `displayUniqueID` the device captures the
  first one (the cover panel), which is black while unfolded. Pick the `active` panel from
  `getdisplayinfo` and pass its `uniqueId` (`screenshot --display`).
- Old #55: screenshot and HID services live on the Developer Disk Image. Until it is mounted, RSD
  doesn't list them.

### 6. Touch and HID input (to re-check)

- Old #8: touch coordinates are normalized to 0..65535; out-of-range values are dropped silently.
- Old #23, #51: touch coordinates follow the panel's native orientation (`nativeOrientation` plus
  `currentOrientation`), so an iPad (`rot270`) or a sideways foldable needs rotating.
- Old #54: reports sent right after a new HID connection opens are dropped while the device is busy;
  waiting 0.4 s before the first report fixed it. iphone-cli similarly waits after a new HID
  connection and sends a "released" report first.
- Old #16: unanswered reports need an HTTP/2 PING round trip before closing, or the tail is lost on
  Wi-Fi.
- Old #17: wait about 0.8 s after a home-button release before closing.

### 7. Touches need a running media stream; buttons don't

Re-checked on the idevice path (iPhone, iOS 27.2.1, USB): with no displayservice stream running,
touch reports on `hid.universalhidservice` were dropped (a back-button tap and a scroll did
nothing; one earlier tap went through, probably while the device was still tearing down the
previous command's stream). A home press on `hid.indigo` worked without any stream.

So `Controller` starts a media session before opening HID. The audio stream alone is enough and
costs almost nothing (~1.7 KB/s, 100 packets/s); the video stream adds ~200 KB/s on a still home
screen. If the device refuses audio with CoreDevice error 9022 ("microphone or camera in use":
a call, a screen recording), the video stream sets up the session instead. A session whose
packets stop for 2.5 s no longer opens the gate and is restarted before the next input.

Measured: starting the audio stream took ~0.2 s to the first packet; audio + video ~0.33 s.
Stopping by `RemoteSSRC` (#1) left `sessions: []` in the server status every time.

### 8. HID connection warm-up

From iphone-cli's measurements, kept as is (see `crates/core/src/input.rs`):

- Wait for the reply to the connected-services list, then 0.3 s more: the device makes a new
  connection's HID devices one after another, and a touch sent before the touchscreen is open is
  held and delivered later all at once (an edge swipe then becomes a jump).
- The first report on a new connection is sometimes lost with its whole gesture. Sending a
  "released" report first (no finger down, so it does nothing) avoids that.
- A tap is contact, 50 ms, release. Keep 150 ms between a release and the next contact (or two
  taps become a double tap), and 0.3 s after the last release before closing the connection
  (0.8 s after a button, old #17).

On the test iPhone over USB, with a new tunnel, media session and HID connection per command, a
tap command took ~1.2 s end to end, a 400 ms scroll ~1.7 s, a home press ~2.4 s (the indigo
connection plus the 0.8 s settle). The daemon keeps all three open, which removes most of that.

### 9. `getdisplayinfo` on a phone with one panel

The answer lists the panel plus sizeless "wireless" displays. Only foldables mark their panels with
`active` and give them a `uniqueId`; a phone with one panel has neither key (iOS 27.2.1). Pick the
sized display with `primary: true` there. Its touchscreen's HID product name is
`CoreDevice touchscreen(nil)`, so "nil" stands in for the missing id. `displayId` (1 for the
panel) is what the video stream's `VideoStreamForDisplayID` takes.

### 10. Reading the accessibility tree

`remoteAXService` over DTX, as iphone-cli does it (`crates/core/src/ax/`):

- `clientNeedsAccessibility:` asks for a dump; it comes back as `processDataFromRemoteDevice:`,
  two NSKeyedArchive layers deep, in two passes: labels/frames/roles/traits/values first,
  identifiers about 0.5 s later. Every new request restarts the device's quiet period before it
  answers, so ask once and wait; ask again only after 6.5 s without an answer. A dump arriving
  within 0.7 s of a request answers an older one (or was sent unasked) and is not taken.
- Measured on the test iPhone (iOS 27.2.1, USB, new tunnel per command): the first pass took
  0.8 to 1.6 s on a still screen; with `--full` 2.0 to 2.9 s. A `ui` command took ~2.4 s end to end.
- Right after `launch`, the first reading had no elements (the app was still animating in); the
  next one, a second later, was complete.
- Calculator's display has no label or value: the number is only in the second pass's
  identifier (`StandardInputView;value:15`). So `ui` keeps containers that carry an identifier,
  and reading such values needs `ui --full`.
- Labels are localized (the test device is Korean). Labels compared in code would have to be
  per-language; `tap <label>` takes whatever the screen shows.

### 11. The screen video stream

Measured on the test iPhone (iOS 27.2.1, USB):

- HEVC over plaintext RTP, payload type 100, RTP clock 24 kHz (as iphone-cli measured). The
  parameter sets come in band with **one keyframe at the start of the stream**; the device sends
  no other keyframe unasked. `MediaSession::start` waits for the first packet before it returns,
  so anyone who subscribes after that has missed the keyframe and can decode nothing. The session
  therefore keeps one receiver made before the streams start (`subscribe` hands it to the first
  caller; the channel holds 4096 packets so the keyframe survives the wait).
- First frame ~0.32 to 0.38 s after asking for the stream. Frames come only when the screen
  changes, up to 50 to 60 per second while scrolling; a "still" screen still gave ~20 per second
  while the screen-sharing indicator (a pill at the top left) animated.
- The coded picture is 1296x2816 for a 1284x2778 panel, with no conformance window in the SPS:
  the screen sits at the top left and the rest is black padding (12 px right, 38 px bottom).
  Players show the padding; the viewer has to crop to the panel's pixel size from
  `getdisplayinfo`.
- **Another process's session is not a leftover.** The first version swept every session in the
  marker file before starting its own; a `swipe` run during a recording then stopped the
  recording's video (by name, correctly, but the wrong session), and the file ended after a second.
  The marker now records the owning pid, and only sessions of dead processes are swept (Windows
  can't check yet and sweeps none).
- Whether the device honors RTCP PLI/FIR keyframe requests is not verified yet.
