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
| Screen streams (displayservice) | #1, #2, #7, #11, #13 |
| Connection and tunnel | #3, #4, #16 |
| Screenshots | #5 |
| Touch and HID input | #6, #7, #8, #17, #18 |
| Display info | #9 |
| Accessibility tree | #10, #12 |
| Daemon | #14 |
| Viewer | #15, #18 |

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
connection code, run an Xcode build to the device while a session is open. (Checked once: #16.)

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
- Keyframe requests: see #13.

### 12. After input, the first accessibility request goes unanswered

With the subscription kept open (the daemon), a request sent after the screen changed (a tap
that opened an app, then a home press) got no dump at all; the next request, sent 6.5 s later
under the old re-ask rule, was answered in 1.25 s. Every time, on the test iPhone (iOS 27.2.1).
Back-to-back `ui` calls on an unchanged screen were answered on the first request (1.6 s).

So after input (`Controller` tracks it), `AxClient::read` sends a second request 0.8 s after the
first, and re-asks every 2.5 s instead of 6.5 s (answers came 1.2 to 1.6 s after the request they
answered). A label tap right after navigating dropped from 7.8 s to 2.1 to 2.5 s; a 0.3 s second
request was faster when it worked (1.6 s) but sometimes was ignored too (4.1 s). With no input
since the last reading there is one request, since a second would restart the quiet period.

### 13. The video ignores keyframe requests

RTCP PLI + FIR (idevice's `build_keyframe_request`, compound with RR and SDES), sent from our video
socket to the device's RTP port 50001 and to 50002, produced no keyframe within 0.8 s. The
daemon then restarts the media session, which brings a keyframe with the first frame
(~1.1 s for a viewer joining a running stream). Restarting also re-opens HID, so an input that
races the restart waits for it. Untested: an RTCP receiver report stream (`build_liveness`) or
Device Hub's per-frame ACK / RCTL feedback (`build_frame_ack`, `build_rctl`) first; idevice notes
the encoder relies on RCTL for reference management during heavy motion.

### 14. The daemon runs on one thread

The futures over the tunnel (idevice's `RsdService::connect_rsd`, the HID and media clients) hold
references across awaits, and rustc can't prove such futures `Send` for every lifetime
("implementation of `Send` is not general enough"); the error moved each time one call was
fixed. The daemon runs on a current-thread runtime with a `LocalSet` and spawns client tasks with
`spawn_local`. Device work is serialized anyway; frame forwarding and the tunnel's TCP stack share
that thread (~1 to 6 Mbit/s of video).

Measured through the daemon on the test iPhone (USB): first command after start ~7.9 s (daemon
start, tunnel, first accessibility dump); then `ui` 1.6 s, home button 0.1 to 0.7 s, coordinate
tap ~0.05 s on the device side, label tap after navigation 2.1 to 2.5 s; a viewer's first frame
0.31 s after subscribing.

### 15. The viewer decodes HEVC with WebCodecs in WKWebView

On macOS (Apple silicon, the Tauri dev build), `VideoDecoder.isConfigSupported` accepted
`hvc1.1.6.L150.B0` with the `hvcC` record as `description`, and the stream decoded with no errors:
22 to 26 frames per second on a mostly still screen, 6 to 13 ms on average from a frame reaching
the page to its picture (worst 116 ms, the first keyframe). The page reports these numbers to
`viewer.log` in the state directory.

- `isConfigSupported` is asynchronous, and the stream's only keyframe (#11) arrived while it
  ran. Frames that arrive before the decoder is configured are kept from the last keyframe on
  and fed to it afterwards; dropping them meant nothing ever decoded.
- The page draws only the top-left `display_px` of each picture (the padding of #11).
- A tap sent by the agent reached the viewer as an `input` event, and the next video frame came
  ~50 ms later (that includes the 50 ms tap hold; the time to the glass was not measured).
- Windows (WebView2) is untested. If a web view lacks HEVC in WebCodecs, the fallback is to
  decode in the Tauri process (VideoToolbox / Media Foundation) and send pictures, or to have the
  daemon send JPEG snapshots.

### 16. Xcode keeps working while our tunnel is open (#3 checked)

With the daemon holding its own tunnel, an audio + video media session, HID and the accessibility
subscription (iPhone, iOS 27.2.1, USB): `devicectl list devices`, `device info details` (tunnel
`connected`, DDI services available), `device info apps` and `device info processes` all
succeeded; `xcodebuild -showdestinations` listed the device; and `xcodebuild build -destination
id=<udid>` of a small app succeeded in 12 s. Our session survived it: streams kept flowing and a
tap afterwards worked. Installing and launching from Xcode (`xcodebuild test` or Run) was not
tried.

### 17. Live touch: a finger that rests has to look still

The viewer drives a touch live (`touch` down / move / up, one report per step). Measured on the
test iPhone (iOS 27.2.1, USB) in Settings:

- **Reports at the same point are not samples.** A drag of 200 pt at ~600 pt/s, a 350 ms rest,
  then the release flung the list ~400 pt: the device took the speed of the last reports that
  moved, however long ago. Three reports at exactly the resting point (16 ms apart) before the
  release changed nothing; nor did a wobble of 1 report unit (0.007 pt). Four reports 16 ms apart
  that wobble by 51 units (about a pixel) made the list stop where the finger did (~180 of 200 pt,
  the rest is touch slop), in both directions. A release while moving still coasts as before. So
  `Input::touch` sends those before an `up` that comes 50 ms or more after the last report;
  `Motion::Place` gets the same effect from its ease-out.
- **A contact with no further reports stays down.** Holding for 1.5 s with no reports opened an
  icon's context menu, and holding on went into home screen editing, as a finger does. So a still
  finger needs no repeated reports; the viewer's keep-alive `move` (every 0.5 s, for the daemon's
  2 s lift timer) is not sent on to the device when the point didn't change.
- Down and up back to back (a trackpad tap-to-click) tapped a row once the up waited for the
  50 ms tap hold. A swipe from x = 1 pt went back a page.
- Latency: a `move` step's round trip through the daemon was ~1 ms (client on the same Mac). From
  a 40 pt step with the finger already scrolling to the first video frame showing it at the host:
  45 to 64 ms (median 54 ms, six steps). The viewer adds its decode time (#15).

### 18. Live touch: steps must reach the device as an even path

Dragging in the viewer scrolled the phone's list in visible jerks. Measured on the test iPhone
(iOS 27.2.1, USB, 60 Hz panel) with `IPHONE_USE_INPUT_TRACE` (`crates/core/src/trace.rs`), the
viewer's own per-press counts, and `examples/touch_bench.rs`:

- **The viewer sent two moves a second.** pointermove fired 60 to 100 times a second, but after
  the first press or two, no move left the page except the 0.5 s keep-alive: the page batched
  moves per animation frame, and a press that ended with a frame request pending cancelled it
  without clearing its handle, so no later press asked for a frame again. The page now sends a
  step per pointer event, with the event's `timeStamp` as `t`.
- **The rest of the path was fine.** A 60 Hz client through the daemon (video subscribed) put 61
  reports a second on the device, 0.5 to 1.5 ms round trip per step (debug build; release ~0.5 ms),
  writes taking 0.1 to 0.6 ms. Debug against release made no visible difference. The single-thread
  daemon (#14) never held a report back by more than a few ms.
- **Even 60 Hz reports still judder against a 60 Hz panel.** The video stream sends a frame only
  when the screen changes, at the device's own pts, so missing frames and the list's shift between
  frames (row profiles of a recording, matched frame to frame) show what the glass did. Reports
  written as they arrived (60 Hz, up to 6 ms of jitter, as the page adds) missed 3 to 6 display
  frames in half of the 0.6 s swipes; at 30 Hz, 6 to 8 per swipe.
- **So the daemon replays the path** (`Resampler`, `TOUCH_TICK` in the daemon): moves only extend
  it, with each point at the client's time (mapped by the smallest send-to-arrival offset seen),
  and a 120 Hz tick reports where the pointer was 20 ms before, interpolated. After that: no missed
  frames at 60 Hz with jitter (0 to 1 per swipe), the per-frame shift as even as before
  (unevenness over 25 %: 5 to 7 frames of ~137), and a 30 Hz client as smooth (9 of 139). The
  reports themselves went out 8.3 ms apart with a speed that varied by 1 to 2 %.
- **The delay grows with the client's spacing and never shrinks within a touch.** A fixed 24 ms
  left a 30 Hz client uneven (112 of 136: the playback caught up with the path and waited); a delay
  that followed the jitter both ways kept changing the playback speed and was worse at 60 Hz (35
  of 137). Growing only, at 0.1 ms per ms, was as good as the fixed delay at 60 Hz and smooth at 30 Hz.
- **An `up` plays the rest of the path first**, then lifts as #17 says: resting before the release
  still stops the list where the finger stopped, and a release while moving still coasts
  (480 to 640 pt after a 380 pt drag, about as before).
- Cost: the finger trails the pointer by the delay (20 ms at the viewer's pace), on top of #17's
  ~54 ms to the host. A down is written at once, so taps don't wait.

