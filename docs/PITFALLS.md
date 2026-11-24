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
| Screen streams (displayservice) | #1, #2, #7, #11, #13, #27, #28 |
| Connection and tunnel | #3, #4, #16 |
| Screenshots | #5 |
| Touch and HID input | #6, #7, #8, #17, #18, #27 |
| Display info | #9, #27 |
| Accessibility tree | #10, #12, #19, #20, #21, #22, #25, #27 |
| Daemon | #14, #23 |
| Device settings | #24 |
| Viewer | #15, #18, #23, #26, #27, #29 |
| Device setup (pairing, Developer Mode, DDI) | #30, #31 |

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

### 19. Some screens answer the accessibility request only after 3 to 4 s

On Control Center's first page (iPhone, iOS 27.2.1, USB) the first pass came 3.1 to 4.2 s after a
request, every time (measured with no re-ask: 3.1, 3.5, 3.5, 3.7, 4.2 s). Under #12's rule of
re-asking every 2.5 s, each re-ask restarted the quiet period before the answer was due, and `ui`
failed after 15 s with nothing received; the screen still answered the moment requests stopped.
The second pass came another 3.2 s later (most screens: 0.1 to 0.5 s).

So `AxClient::read` doubles the wait before each further re-ask (2.5, 5, 10 s) and starts from
the last answer's latency plus 1 s, and the wait for the second pass grows the same way. On that
page a cold `ui` now takes 6 to 8 s and a repeated one ~3.5 s; ordinary screens are unchanged
(1.2 to 2 s).

Re-asks now go out on a new connection (#25); this page still reads in 3.5 to 3.7 s.

Not this: a running stopwatch (a label changing every frame) answered in 1.3 s, so "the screen is
animating" is not what delays an answer. Safari's start page timed out once while a Wi-Fi password
prompt was up on the device and read normally (1.6 s) after it was dismissed; not reproduced.

### 20. Where to tap an element is not always its frame's middle

- **Switch rows.** In Settings a switch row's element spans the label and the switch
  (`{{40, 238}, {348, 28}}`); tapping its middle hit the label and did nothing, twice. Tapping
  26 pt in from the trailing end (the switch) toggled it, and the value went `0` → `1` → `0`.
  `Screen::tap_point` does that for `switch` elements more than three times as wide as tall;
  Control Center's square switch tiles are tapped in the middle. Untested: right-to-left layouts,
  where the switch is at the leading end.
- **Full-screen dismiss buttons.** Context menus and Safari's page menu come with a button that
  covers the whole screen ("close menu"), listed after the menu items; its middle lies on an item.
  The first fix (the nearest point not inside any smaller element) landed 6 pt above the first
  item, on the menu's own background, which is not an element, and the menu stayed open. Points
  now keep 24 pt clear of the smaller elements when the frame allows (8 pt, then 0 pt when not).
- **Hidden under bars.** Web content scrolls under Safari's bottom bar, and its elements there
  keep their frames (a link at y 862 under the address field). The covered-point rule moves the
  tap off the field, but the bar's background still swallows it. Scroll the element up first.
- Frames of other processes can be in the wrong space altogether: #21.

### 21. Hosted views: elements of another process, some with frames in their own space

`ui` lists elements by pid, and several processes draw into one screen. The application element
belongs to the app; other pids appear only on elements (`ui` lists them as `hosted` apps):

- Web content (Safari, an app's web view) is a WebKit content process, a new pid per site. Its
  frames are screen points, and a tap by ref on a page's menu button opened that menu.
- The keyboard is the app's own pid (keys came with the host's pid); one unlabeled InputUI button
  sat at `{{30, 0}, {368, 44}}`, over the status bar, so probably in its own space too.
- **Share sheet** (SharingUIService) and the map in a location-permission alert: frames are
  relative to the hosted view. The share sheet's first icon read `{{32.7, 32.8}, {64, 64}}` while
  it showed at about (42, 544) on the screen; the activation point (attribute 1) is in the same
  local space. The dump has no element for the host view, so there is no offset to add. Tapping
  these by ref misses; use coordinates from a screenshot.

Possible fixes, untried: ask the device for the hosted element's parent or its host view with a
single-attribute request (the payload has an `Answer` kind), or match the share sheet's layout
against a screenshot.

### 22. What the accessibility tree has and lacks (survey)

Read on the test iPhone (iOS 27.2.1, Korean, USB, portrait) through the daemon; the full table is
not kept in the repository.

- **Off-screen rows are not in the tree**, `--all` or not: table and collection cells exist only
  while on screen. Settings' top list took 4 scroll steps (~3 s each with the reading) to read to
  the end; when two readings in a row are the same, the end is reached. Web content and context
  menus do list elements beyond the screen with `--all`.
- **The tree is flat**: the application element's children are the leaves; containers do not come
  with the dump, so "which row does this label belong to" is not answerable (a third-party list
  read as separate text pieces per row, none of them a button).
- **System screens are SpringBoard**, whose application element has no label (`" "`). Home screen,
  Control Center (three pages; one dump shows one page), the lock screen / notification list, the
  app switcher and system alerts are told apart only by identifiers (`spotlight-pill`,
  `Page control`, `cc-*`, `*-orb-button`, `card:<bundle id>:sceneID:...`, `kTCCService*`), which
  only the second pass has. `ui` therefore waits for that pass when SpringBoard is the only app
  (+0.1 to 3 s).
- **The app switcher opened with a double home press listed no elements at all** (only the
  application) on two readings 4 s apart; after a 30 pt drag the five cards appeared. Another time
  it listed them at once. Cards are role 4 with a custom "close" action and identifier
  `card:<bundle id>:...`.
- **Edge gestures:** a swipe from the top edge opened Control Center and the notifications; a swipe
  up from the bottom edge (y 925 of 926, fast or slow) only scrolled the app: the home-indicator
  gesture never fired. Home (`button home`) and a double home press (app switcher) work.
- **Values:** Settings switches give `0`/`1`, Control Center's give localized words (off / on);
  web headings give their level (`2`); tab bar items `1`/`0` beside the `selected` trait; page
  controls "page n of m". Calculator's display still has its number only in the identifier (#10).
- **Labels** carried U+FFFC (an inline icon), U+200E, U+2060 and no-break spaces; `ui` drops the
  first three kinds and turns no-break spaces into spaces. Control Center listed the battery twice
  with the same frame; `ui` prints it once.
- Role numbers seen (attribute 45): 1 application, 2 button (also keys, home screen icons),
  3 switch, 4 plain (text, app switcher cards), 6 header, 7 image, 13 adjustable, 14 static text,
  17 text field. The trait bits name every role correctly; no role was left `other` that had a
  label. Unknown trait bits seen: 22 (home screen icons), 23 + 58 (status bar items in Control
  Center), 33 (nearly everything), 62 (app switcher cards).
- **App names:** CoreDevice's `listapps` did not answer within 12 s (all option combinations);
  installation_proxy's `Browse` with four attributes listed 260 apps in 0.19 s, with localized
  display names. SpringBoard is not in that list.

### 23. A viewer that came back saw a black screen

Back in the viewer, then the same device again: input worked but nothing was drawn (iPhone, iOS
27.2.1, USB). Two faults, either enough for it:

- **The old connection never closed.** Dropping a daemon `Client` dropped its tasks' handles,
  which doesn't end tasks; the reader held the socket's read half, so the daemon kept counting the
  viewer (`daemon status` showed `viewers` 1 after Back, 2 after coming back). The returning
  viewer was then a late joiner to a running stream (#11): it got the cached `config`, the
  keyframe request went unanswered (#13), and the stream restarted with a second `config` before
  its keyframe. `Client` now aborts its tasks on drop.
- **The page swapped decoders too late for that second `config`.** `configure` awaited
  `isConfigSupported` with the old decoder still in place; the restarted stream's only keyframe
  arrived meanwhile and went to the old decoder, which was closed right after, and the new one
  dropped every frame waiting for a keyframe. Now a `config` with the same parameter sets keeps
  the decoder, a different one drops it at once (later frames queue from their keyframe), and a
  setup that a newer `config` or Back overtook gives way.

Checked after the fix: Back and back again four times, the same with another frame subscriber
keeping the stream running (each return dropped ~17 frames before the restart's keyframe), and a
viewer process started and killed three times; the first frame came every time and `viewers`
went back to 0.

### 24. Device settings (configuration and location services)

Measured on the test iPhone (iOS 27.2.1, USB) with `settings` / `settings set`
(`crates/core/src/settings.rs`):

- **A call sometimes gets a bodyless reply and nothing after it.** The answer to a get came as an
  empty `Reply` message on the reply stream, and idevice's `recv` skips bodyless messages and
  waits for the next one, which never came: a `settings set` in the daemon hung for minutes,
  holding the device lock. About 1 call in 80 through the daemon (a read right after a set, a
  read while a viewer attached; two in a row once); not seen on a fresh tunnel per command, but
  not ruled out there. Each call now has a 1.5 s limit and up to three tries on new connections
  (calls answer in 10 to 50 ms); the final error is not worded as a connection failure, since
  the tunnel is fine and the daemon would drop its session on those words.
- **Setting VoiceOver answers only after it has started or stopped:** 3.1 to 3.5 s, both ways
  (8 s limit). Turning it on shows a gesture tutorial, and closing that a notification permission
  prompt from VoiceOver (declined on the test device). Turning it off over CoreDevice works while
  it is on.
- **Accessibility text sizes (`ax1` .. `ax5`) are refused** (CoreDevice error 21063, "Enable
  Larger Accessibility Sizes on the device before setting an accessibility text size") while the
  device's Larger Accessibility Sizes switch is off. `xs` .. `xxxl` work. The error shown is the
  device's `NSLocalizedDescription`.
- **`showBorders` is Button Shapes** (Accessibility > Display & Text Size), not a layout-debug
  overlay. Increase Contrast and Reduce Transparency showed up on the same page.
- **Liquid Glass opacity is a Float:** 0.8 reads back as 0.800000011920929; output is rounded to
  two places. idevice has no getter for it, nor for Increase Contrast or VoiceOver; those go out as
  raw actions (`getliquidglassconfiguration`, `getdeviceincreasecontrast`, `get`/`setvoiceover`
  with key `voiceOverConfiguration`) on the same service.
- **The simulated location has no getter.** `setsimulatedlocation` / `clearsimulatedlocation`
  (feature `com.apple.coredevice.feature.simulatelocation` on
  `com.apple.coredevice.locationservice`) moved Maps to the given point and back. The daemon
  reports the location it set; a restarted daemon reports `null` while the device may still be
  simulating one, so `location clear` is always safe to send.
- All other items changed on the device and read back as set, through the CLI and the viewer's
  panel: dark appearance and larger text showed on the home screen at once.

### 25. The accessibility service goes silent: one subscription, or the whole daemon

Test iPhone, iOS 27.2.1, USB, home screen. `ui` failed every time ("no accessibility dump within
15 s"), through the daemon and with `--no-daemon`, and a build of the last commit where it had
worked failed the same way, so the device was the cause, not the code.

- **The whole service.** A new connection to `remoteAXService` got no message at all: not the
  greeting a working service sends within milliseconds (`_notifyOfPublishedCapabilities:`, then a
  `hostAPIVersion` call), no reply to calls that expect one (`deviceApiVersion`,
  `deviceCapabilities`), no dump. Turning VoiceOver on did not help. Killing `axauditd`
  (`/System/Library/PrivateFrameworks/AccessibilityAudit.framework/Support/axauditd`, through
  CoreDevice's `sendsignaltoprocess`) fixed it: launchd started a new one on the next connection,
  which answered at once. How it got stuck is not known: VoiceOver had been switched several
  times that day while the daemon held a subscription, but a dozen switches in a row did not stick
  it again.
- **One subscription.** Switching VoiceOver on or off (from `settings set` or the device) silences
  every subscription that is open at that moment, for good, while its connection stays open: no
  dump, no error, re-asks unanswered. A new connection answers at once. Seen six times in six
  switches with the daemon holding a subscription.
- **Right after a restart** the new `axauditd` greets a connection at once but leaves the first
  request unanswered for 2.5 to 8 s; a request on a newer connection then answers. Re-asking on
  the same connection did not get an answer within 15 s.

So `AxClient` sends each re-ask on a new connection (a request costs the same there), and a
connection that has heard nothing 3 s after it opened counts as a stuck service (`AxUnresponsive`):
`Controller` kills `axauditd` once and reads again. The daemon also drops its subscription when it
switches VoiceOver. Costs: a subscription silenced by a VoiceOver switch elsewhere makes one `ui`
take ~5 s instead of ~2 s; a restart makes it take 6 to 11 s. `axauditd` is the Accessibility Audit
daemon that Xcode's Accessibility Inspector talks to; restarting it changed nothing visible on the
device (VoiceOver and the other accessibility settings are not its).

### 26. WebKit draws a whole VideoFrame, padding included, whatever source rectangle it is given

The device codes the screen into a picture padded to the encoder's block size (a 1284x2778 screen
in a 1296x2816 picture, PITFALLS #11), and the decoded `VideoFrame` keeps the padding: its coded
size, visible rect and display size are all 1296x2816. `drawImage(frame, sx, sy, sw, sh, dx, dy,
dw, dh)` with the screen as the source rectangle should copy only the screen, but WebKit (the
macOS web view) drew the whole picture scaled into the destination: the viewer showed black bands
11 px wide at the right and 37 px tall at the bottom of a 1284x2778 canvas, the padding squeezed
in. The viewer now draws the whole frame scaled up so the screen fills the canvas
(`drawImage(frame, dx, dy, dw, dh)` with the destination larger than the canvas) and lets the
canvas clip the padding off. It reports the frame's geometry as `frame` in `viewer.log`.

### 27. Landscape: only the interface turns; nothing announces it

Test iPhone (iOS 27.2.1, USB), rotation lock off, Safari in front, turned by hand between
portrait and both landscapes while `examples/rotation_probe.rs` (core) watched the stream and
polled `getdisplayinfo`.

- **The video doesn't turn.** In landscape the coded picture stays 1296x2816 (portrait, the
  padding of #11 still at the right and bottom), and the landscape interface is drawn turned inside
  it: with `rot270` the screen's top-left corner is at the picture's top-right. A turn brings no
  keyframe, no new parameter sets and no change in the RTP header extension (profile `0x9201` or
  `0x9211`, one word whose last two bytes count up per frame; nothing in it followed the turns).
  So the viewer turns the picture itself, and a recording gets the rotation at its start as the MP4
  matrix (a track has one matrix; a turn during a recording plays sideways from there).
- **`getdisplayinfo`'s `currentOrientation` is the interface orientation**, not the device's:
  `rot0` in portrait, `rot270` and `rot90` in the two landscapes, and `rot0` on the home screen
  while the phone was held in landscape (the home screen didn't turn; the screenshot and `ui` were
  portrait too). It is the one signal; nothing is pushed, so the daemon polls it every 0.5 s while its
  device session is open (not only while a client is connected: the CLI connects per command, and
  `ui` right after `button home` still said `landscape` on the portrait home screen, so a `tap`
  would have been turned wrongly) and sends an `orientation` event on a change.
- **When it changes.** Switching between Safari (landscape) and the home screen (portrait) with
  `button home` and `launch`, and recording through the daemon: the picture started to turn ~1.4 s
  after a home press and ~0.4 s after a launch (the app's zoom animation, drawn turned inside the
  portrait picture), and `getdisplayinfo` changed at about the end of that animation. Polling every
  0.5 s, the `orientation` event came 0.1 to 0.4 s after the picture had turned, so the viewer showed
  the new screen sideways for that long. The daemon now polls every 0.1 s for 3 s after input; the
  event then came 0.05 to 0.15 s after the animation ended. A turn by hand still waits for the 0.5 s
  poll.
- **A new connection per call sometimes gets no answer.** Asked every 250 ms on a new
  `deviceinfo` connection, 11 of about 600 calls got nothing within 2 s (and `display` through the
  CLI hung for the 12 s step timeout in 2 of 5 tries during that time). On one kept connection,
  about 600 calls in a row over 2.5 minutes all answered, in 6 to 12 ms. `DisplayPoller` keeps its connection, gives a call 1.5 s
  and asks again on a new connection.
- **Touches and `ui` frames are in the screen as the user sees it.** `ui` frames in landscape were
  landscape points (application frame 926x428; native toolbar buttons and web content both matched
  the screenshot), and a vertical `swipe` in landscape points turned onto the panel by
  `Display::report_point` (the old #23 mapping) scrolled the page vertically. With a stale portrait
  display the same swipe was clamped and did nothing, which is why the daemon follows the turns.
- Seen in passing, not looked into: while the probe ran a media session of its own, the daemon
  starting one stopped the probe's audio stream for good (its video went on), and a later daemon
  restart and recording ended the probe's video too; `media status` then listed no sessions at
  all. Measure turns through the daemon (`events --frames`) instead.

### 28. A recording started while a viewer watches got no keyframe

`record start` with the viewer open failed at `record stop` with "no keyframe arrived": the
recorder joined a running stream, whose only keyframe had gone out when the viewer subscribed
(#11), and keyframe requests are ignored (#13). Recordings had only been tried with no viewer, when
`record start` starts the stream itself. The daemon now restarts a running stream when a recording
starts; viewers take the new `config` and keyframe as on any restart (#23). Measured with the viewer
open (iPhone, iOS 27.2.1, USB): the recording got its keyframe, and the viewer kept decoding with no
errors.

### 29. The viewer's window: sizes and hidden windows

Seen while the viewer followed turns (#27) on macOS, Tauri dev build:

- **The window no longer changes size, by the user's preference.** The viewer used to reshape its
  window when the screen turned (and for a landscape screen on arrival), remembering a size per
  orientation. That code is gone: the window keeps whatever size the user gave it, and the screen
  is scaled to fit inside it, centered, in either orientation. What was learned while it existed,
  in case it comes back: a size worked out from the page drifted (the page came out 32 px shorter
  than asked, on every turn: 473, 422, 390, 358 px tall), so sizes had to be kept as the window
  reported them (`inner_size`) and restored exactly; and window calls from a Tauri command must
  run in an async command, since a sync one runs on the main thread, which the window calls wait
  for.
- **A hidden window stops its timers.** A viewer on another Space (or fully covered) stopped
  sending its 2-second reports to `viewer.log` a few seconds after it opened, while it still took
  events. The page also reports right after a turn and a recording event, so a check doesn't
  depend on the timer.

### 30. Pairing: the device answers "pending" until someone taps Trust, and only when unlocked

`lockdown Pair` doesn't block while the "Trust This Computer?" alert is up. The device answers at
once with `PairingDialogResponsePending`, and the host has to send the same request again
(idevice's `LockdownClient::pair` resends it every second; `pair_once` returns after one try).
While the device is locked it answers `PasswordProtected` and shows no alert at all: the alert
appears once it is unlocked. "Don't Trust" gives `UserDeniedPairing`, and the device doesn't ask
again until the cable is unplugged and plugged back in.

Each request carries a freshly generated host certificate (a 2048-bit RSA key): `pair` builds it
once and resends it; looping over `pair_once` would build a new key every second. In an
unoptimized build the key takes seconds, so the workspace builds `rsa` and `num-bigint-dig` with
`opt-level = 3` in the dev profile.

The record isn't kept by idevice: the caller hands it to usbmuxd (`SavePairRecord`), which stores it
where the OS looks (`/var/db/lockdown` on macOS; Apple's usbmuxd on Windows has its own store).
`setup trust` opens a lockdown session with the new record before saving it, so a bad record never
replaces a good one.

### 31. `LookupImage` says nothing is mounted while the disk image is mounted

iPhone, iOS 27.2.1, after Xcode 27 had prepared the device: `mobile_image_mounter`'s `LookupImage`
returned no signature for `Personalized`, `Developer` or `Cryptex`, while the developer services
(`com.apple.instruments.dtservicehub`, CoreDevice's app service) were listed over RSD and worked.
`CopyDevices` listed the image: `MountPath` `/System/Developer`, `DiskImageType` `Personalized`,
`PersonalizedImageType` `DeveloperDiskImage`, `IsMounted` true, with its backing image inside
cryptexd's store (`com.apple.security.cryptexd/.../com.apple.MobileAsset.DDI`). Xcode 27 seems to
have the image mounted through cryptexd from a MobileAsset, and `LookupImage` only knows images
uploaded through the mounter itself.

So setup reads `CopyDevices` and looks for a mounted entry at `/System/Developer` (`/Developer`
before iOS 17).

The image mounter also refuses personalized images while Developer Mode is off, so the disk image
is checked only after Developer Mode reads on.

