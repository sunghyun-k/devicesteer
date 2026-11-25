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
| Touch and HID input | #6, #7, #8, #17, #18, #27, #34, #35, #36, #39 |
| Display info | #9, #27 |
| Accessibility tree | #10, #12, #19, #20, #21, #22, #25, #27 |
| Daemon | #14, #23 |
| Device settings | #24 |
| Viewer | #15, #18, #23, #26, #27, #29, #33, #34 |
| Device setup (pairing, Developer Mode, DDI) | #30, #31, #32 |
| Keyboard, typing and clipboard | #35, #36, #37, #38, #39, #40, #41, #42 |
| Windows host | #49, #50, #51, #52 |

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
Xcode's Device Hub's per-frame ACK / RCTL feedback (`build_frame_ack`, `build_rctl`) first; idevice notes
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
(iOS 27.2.1, USB, 60 Hz panel) with `DEVICESTEER_INPUT_TRACE` (`crates/core/src/trace.rs`), the
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

### 32. The disk image Xcode 27 mounted can't be unmounted through the image mounter

iPhone, iOS 27.2.1, USB, the image from #31 (backing image in cryptexd's MobileAsset store):
`mobile_image_mounter` `UnmountImage` for `/System/Developer` failed with MobileStorage error -2,
"Failed to unload launchd jobs", and the image stayed mounted. Same with DeviceSteer's daemon
stopped (no tunnel of ours open); macOS's own `CoreDeviceService` and `remotepairingd` were running,
as they do whenever a device is connected. Nothing on the device changed.

**A restart didn't take it away either.** The owner restarted the same iPhone and unlocked it; the
device was back in usbmuxd about 20 s after it left, and the first `setup status` already found
the image mounted, with the same backing image in cryptexd's MobileAsset store. macOS's
`CoreDeviceService` logged nothing in that window, so the device itself mounted it again at boot:
once a DDI arrived as a MobileAsset (here through Xcode 27), it stays across restarts. Older
setups (an image uploaded through the mounter, as `setup disk-image` does) are documented to go
away at every restart; this was not observed here.

After the restart DeviceSteer's daemon (left running) reported `connected: false`, and the next
`ui` reconnected it (2.8 s); `ui`, `tap` and a video `media probe` worked.

So: the code never unmounts, and the upload-and-mount path of `setup disk-image` can only be tried
on a device without the MobileAsset image (one Xcode 27 never prepared, or after erasing it). Up to
the TSS signature it was checked on this device without changing it (Xcode's copy and the
download both got a ticket).

### 33. `trafficLightPosition` doesn't place the buttons' top edge at `y`

Observed on macOS with Tauri 2 (`titleBarStyle: Overlay`), measuring the window's Close button
through the accessibility tree: `y: 15` put the 16-point buttons' top 5 points down (their middle at
13), and `y: 25` put it 15 points down (middle at 23). `x` matched the left edge. So `y` is 10
points more than the top edge it gives.

The viewer's headers are 46 points tall (`--head-h` in `app/src/style.css`) and the buttons sit in
their middle, so the config has `y: 25`. Change both together, and measure again: the offset may
change with Tauri or macOS.

### 34. Touch indicators: the end has to start when the finger lifts

The viewer's touch disc grows and fades when the finger lifts. It started visibly late. Measured
on the test iPhone (iOS 27.2.1, USB) with `DEVICESTEER_INPUT_TRACE` (`hid` lines) against the
viewer's own times on the same wall clock:

- **Agent tap: ~270 ms late.** The daemon sent the `tap` event before `Controller::tap`, which
  opens HID (~700 ms on the first input of a session), waits the gap after the last touch (up to
  150 ms), then holds 50 ms. The viewer glided the cursor there (160 ms), then pressed for 160 ms:
  the end started 320 ms after the event, 270 ms after the device's release. On a first tap it
  started ~590 ms *before* the touch. Now the event goes out as the finger goes down (`on_contact`)
  with `duration_ms` (the 50 ms hold), and the viewer draws the disc where the finger lands and
  ends it `duration_ms` later, while the cursor glides there on its own: the end starts within
  2 ms of the release, first tap included.
- **Human click: ~95 ms late.** A click with no movement had its `up` wait for two things meant
  for drags: the 20 ms replay delay (#18), and the four still reports before a release (#17,
  ~65 ms) because the down's report was more than 50 ms old. A touch that never left 4 points
  around where it went down starts no scroll, so it has no speed to cancel: it skips the still
  reports, and the `up` lifts at once once the rest of the path stays at its last point
  (`Resampler::settled`). From the `up` request to the release: ~0.3 ms (was 92 to 103 ms).
- **Agent live touch `up`**: the viewer used to wait for a 120 ms glide to the up's point before
  ending; it now ends at once and lets the cursor finish the glide.

Swipes still play the cursor's 160 ms glide to the start point before the press, so their end is
about that late; not changed here.

### 35. The keyboard surface types the last hardware-keyboard input source, and that sticks

Every HID connection has a keyboard surface (product "CoreDevice keyboard", usage page 0x07,
sender 512 on the test iPhone) that takes 39-byte bitmap reports. iOS treats it as a *software*
keyboard: the on-screen keyboard stays up, and the layout it types with is not the on-screen
keyboard's language. Measured on the test iPhone (iOS 27.2.1):

- At first it typed Latin in every app, with the on-screen keyboard on Korean too: Hangul sent as
  Dubeolsik keys came out as `dkssud`. The Swift implementation had seen Dubeolsik in some apps on
  the same model (old #11, #58), which looked like a per-app difference.
- After a hardware keyboard (#36) was switched to the Korean source with ctrl+space and removed,
  the surface typed Dubeolsik jamo in every app, until a hardware keyboard selected Latin again.

So the surface uses the input source the **last hardware keyboard** left behind, device-wide, and
it survives detaching, the end of the connection and other clients. Xcode's Device Hub's keys with
"Simulate Hardware Keyboard" off go the same way: they follow that stuck source, not the Mac's
layout.

Consequences:

- Never assume what a key on the surface types. `type` doesn't use the surface for text; it
  attaches its own hardware keyboard and sets the source (#38).
- To put a device back after testing: `keyboard connect`, `key lang2` (Latin), `keyboard
  disconnect`. A probe key into an empty field shows which source is active.
- The viewer's "Type from Mac Keyboard" sends physical key positions (DOM `code`), so what comes
  out is whatever this source makes of them, like on a real keyboard. Through the surface alone
  the source is stuck and Control-Space doesn't switch it, so turning it on attaches a hardware
  keyboard (#36) for as long as it is on and the viewer has the device.

### 36. A hardware keyboard: attach, detach and what the device does

idevice's `mainKeyboard` (`createService` on the universal HID service, then usage reports) makes
a keyboard the device takes for a real one. Observed:

- **Attach:** the on-screen keyboard hides at once. Keys sent ~250 ms after attaching were lost,
  ~800 ms worked (iphone-cli's measurement, matched here); we wait 700 ms with an empty report in
  the middle.
- **Its source:** it starts on the language the on-screen keyboard showed when it attached, then
  follows ctrl+space and LANG2 (#38). Hangul composes with the Korean source.
- **Detach** (`removeService`, or the HID connection closing): the on-screen keyboard comes back
  within ~0.5 s, on the language of the source at detach. The source sticks for the surface (#35).
- It lasts only as long as the HID connection that made it. The controller remembers that one
  was asked for (`keyboard connect`) and makes it again when it reopens HID.
- The on-screen keys are hidden while it is attached, so tapping keys (`type --method tap`)
  detaches it for the typing and attaches it again afterwards.

Detach after tests: a keyboard left attached keeps the owner's on-screen keyboard hidden.

### 37. Reading the on-screen keyboard and the field around keyboard changes

- A reading right after a tap on the next-keyboard key (or Shift) can still list the old keys,
  or none while they redraw: poll until the language changed (`KEYS_WAIT`).
- A reading right after keys can return the field's value from before them (like #12): when the
  value hasn't changed, read again (`STALE_READS`) before deciding the keys did nothing.
- A tap on the next-keyboard key was sometimes taken for a long press: the device lists the
  keyboards over the keys instead of switching. The key's value names the keyboard a tap would
  pick; that item is tapped in the list.
- On a new HID connection, the first key reports of the first `type` were all lost once. An
  empty keyboard report right after opening (with the touch warm-up) fixed it.
- Readings take 1.5 to 2.5 s each, so a `type` that reads per character is slow (the tap method
  took 51 to 58 s for a 20-character mixed string; #38 reads twice).

### 38. Streaming text through a hardware keyboard

`type` (method `stream`) attaches a hardware keyboard (#36), sends every key with the source
switches at the script boundaries, removes it, and reads the field once. Measured on the test
iPhone, Notes and Settings search:

- **Starting source** = the on-screen keyboard's language at attach. With Hangul in the text,
  the on-screen keyboard is switched to Korean first, so Korean is one of the two recent sources.
- **LANG2** (usage 0x91) selects the Latin source, from any state. LANG1, Caps Lock, right Alt and
  right Cmd did **not** select Korean.
- **ctrl+space** switches to the source used before, so after LANG2 it goes back to Korean. It
  brings up a list of sources for ~1.5 s, which takes the next **space, Return or LANG2** for
  itself (a space 1.0 s after was lost, 1.5 s after was typed); letters go through. So the
  stream waits out the list (`SOURCE_LIST`, 1.7 s) before whitespace or another switch.
- A space after a composing Hangul syllable was lost when the switch to Latin came after it:
  switch right after the Hangul letters, before the spaces.
- The first key after attaching on a fresh connection lost its Shift once: an empty report
  before the first key (#36).
- Autocorrection and autocapitalization apply as on a real keyboard (`x` into an empty note came
  out `X`). The final reading catches that; the text is then deleted and typed again by tapping
  (`stream+tap` in the result).

Times (one reading before, one after): "Hello World" 5.7 s, "Bluetooth" 6.2 s, "안녕하세요
반갑습니다" 12.5 s (includes switching the on-screen keyboard to Korean), "Hi 안녕하세요 Seoul
1호선!" 13.7 s with 5 switches, against 16 to 58 s for tapping. Only Latin and Korean are known;
with a third input source installed, ctrl+space order is unverified.

### 39. The Eject key shows and hides the on-screen keyboard

With a hardware keyboard attached, consumer usage 0x0C/0xB8 (Eject, as Xcode's Device Hub's "Toggle
Software Keyboard" sends) brings the on-screen keyboard back, and again hides it. It shows the
current hardware source's language; switching it there with the globe key changes that source
too (the way to change the stuck source from Xcode's Device Hub). Without a hardware keyboard it was
not tried. `button eject` and the viewer's menu send it.

### 40. A clipboard paste asks every time

Re-checked on the new path (old #30): setting the clipboard through the pasteboard service and
pressing cmd+v in Notes shows the paste prompt on every paste, from the same client too. The
prompt is the owner's to answer, so `paste` reports it (`prompt: true`), keeps the owner's saved
clipboard, and the agent restores it with `paste --restore` after the owner has answered. That
makes `paste` a fallback for what `type` can't send (emoji), not the default.

### 41. No layout-independent text input was found

Looked for a way to put text into a field without keys:

- **Accessibility audit server** (`com.apple.accessibility.axAuditDaemon.remoteserver.shim.remote`,
  RSD checkin + DTX, protocol version 157): its selectors include
  `deviceElement:setValue:attribute:` and `deviceElement:performAction:withValue:`. With guessed
  argument shapes it answered NSError code 1 or dropped the connection; one drop also closed our
  own accessibility subscription ("accessibility connection closed"), recovered on the next call.
  Stopped there: the shapes are unknown and a wrong one disturbs the owner's other AX clients.
- **UniversalHID**: the event type enum has a unicode case, but no service wraps it; the device's
  HID features are button, digitizer, keyboard and vendor-defined only.
- **Xcode's Device Hub** has no text path of its own: its keys are HID (#35).

### 42. An empty field reads as its placeholder

Settings' search field reads its placeholder ("Search", localized) as its value while empty,
and once typed into reads the text alone. Checking "the value grew by the typed characters"
against the placeholder failed, and deleting back to it removed what had been typed. Treat a
reading that no longer contains the value from before as an empty field (`typing::baseline`).

### 43. A paste from the edit menu doesn't ask

Setting the clipboard through the pasteboard service (`clipboard_set`, the viewer's "Send
Clipboard") and then pasting with a long press and Paste in the edit menu inserted the
text in Notes with no allow-paste prompt (iOS 26, iPhone). The prompt of #40 comes with a
programmatic paste such as cmd+v; the edit menu's Paste counts as the user's own. So the viewer
sends the clipboard without pasting and leaves the paste to the person at the device.

Also seen: right after the service sets the clipboard, the keyboard's suggestion bar offers it as
"1 item, paste from dtpasteboardd" (localized), naming the service as the source. The service has
no way to empty the clipboard: putting back an empty one writes an empty text item.

### 44. Polling the clipboard for the shared clipboard

The viewer's "Use Shared Clipboard" polls both clipboards (`app/src-tauri/src/shared_clip.rs`).
Observed on an iPhone (iOS 27, USB):

- **Reading the device clipboard** through the pasteboard service takes about 35 ms (one service
  connection per read). It shows nothing on the device: no "pasted from" banner, no prompt (those
  come with an app reading the clipboard, not with the service). The daemon holds the device lock
  for the read, so a tap or key sent meanwhile waits that long; at one read per 1.5 s that is
  rare and short. Not yet measured over Wi-Fi.
- **The Mac's `changeCount`** moved by 3 for one `clearContents` and `setString:forType:`. Record
  the count after writing instead of predicting it, or the next look takes our own write for a
  new copy and sends it back.
- **Universal Clipboard** didn't get in the way of checking it: with syncing off, a Mac copy
  didn't reach what the pasteboard service reads within 8 s. Quit Xcode's Device Hub before
  checking the feature: its own shared clipboard syncs too.
- Password managers mark copies with `org.nspasteboard.ConcealedType` (and
  `org.nspasteboard.TransientType` for one-off items); those are never sent. A marked dummy stayed
  on the Mac.

### 45. One Caps Lock press can reach the page as two

The viewer forwards the Mac keyboard from the page's key events (WKWebView). Caps Lock has no
ordinary press and release there: WebKit reports changes of the Caps Lock state. On a Mac whose
Caps Lock switches the input source to and from ABC (ABC and Korean 2-Set; macOS 26), one quick
press arrived as:

- `keydown CapsLock` at the press,
- `keydown` with code and key `Unidentified` (keyCode 0) about 10 ms later, never followed by a
  keyup: the Mac switching its input source,
- `keyup CapsLock` at the release, about 100 ms after the keydown.

The viewer used to send each Caps Lock event as a whole press, as fits a Mac where Caps Lock only
locks (a keydown when it locks, a keyup when it unlocks, each a separate press). Here that made
two presses for one: the device (a Korean layout, where Caps Lock switches to and from Latin)
showed the switch and went straight back. So a keyup within a second of a keydown counts as that
press's release and sends nothing; the keydown already sent a tap (down, 30 ms, up), so Caps
Lock is never left held on the device. The cost: on a Mac where Caps Lock only locks, unlocking
within a second of locking reaches the device as one press, not two. The `Unidentified` keydown
is dropped: the device has no usage for it.

Control-Space is handled separately (#46).

### 46. Control-Space never reaches the page

Control-Space is the Mac's "Select the previous input source" shortcut (on by default). macOS
takes it before the window: with the viewer forwarding the keyboard, one Control-Space arrived in
the page (WKWebView; macOS 26) as `keydown ControlLeft` and `keyup ControlLeft` about 300 ms apart
and nothing in between: no Space, no `Unidentified` keydown (unlike Caps Lock, #45). From the
page alone it can't be told from a lone Control tap.

What does happen is the Mac's input source changing. The Tauri shell watches
`kTISNotifySelectedKeyboardInputSourceChanged` (a distributed notification) and tells the page.
The page treats a change while Control is down with no other key since (or let go within 500 ms,
for a quick press) as the shortcut: it sends the device a Space under the Control it already sent
(or a whole Control-Space if Control is already up), and asks the shell to select the Mac's
previous input source again, so only the device switches. The shell ignores the notification its
own reselection causes. A Caps Lock keydown clears the "Control alone" state, so a Caps Lock that
switches the Mac's input source (#45) isn't also sent as a Space.

With the shortcut turned off in the Mac's settings, the Space reaches the page as usual and goes
over as a normal key; no input source change follows, so nothing doubles.

### 47. A foldable switches panels when folded or unfolded; nothing follows by itself

Foldable iPhone (iOS 27.3, USB), folded and unfolded by hand while `examples/fold_probe.rs` (core)
polled `getdisplayinfo` every 0.1 s and `devicesteer events --frames` watched the daemon's stream.

- **`getdisplayinfo` lists both panels at all times**: the cover panel (`displayId` 1,
  `primary: true`, 1398x2034 px, 466x678 points) and the inner panel (`displayId` 2,
  `primary: false`, 2007x2853 px, 669x951 points), each with its own `uniqueId`. Folding or
  unfolding flips `active` between them; size, ids and `primary` never change. The panel coming
  into use reports `backlightState: activeOn` 0.2 to 0.7 s before `active` flips, and the one going
  out of use goes `off` about 1 to 3 s after.
- **The panel not in use keeps an orientation of its own**: once folded, the inner panel said
  `rot90` while the cover was `rot0`, and still `rot90` right after unfolding. In fact neither
  panel's `currentOrientation` ever moves; the device's orientation is what follows a turn (#48).
- **The inner panel's `frame` and `nativeSize` (1878x2670) are smaller than its `bounds`
  (2007x2853)**; screenshots of it were 2007x2853, so `bounds` stays the size to use.
- **A sized wireless display appears while a stream runs**: `wireless0` (`displayId` 4) gets
  `bounds` 1408x2048 at scale 1, and the inner panel's video arrived scaled to 1408x2016 coded (#48).
  It is never `active`, so picking by `active` (#9) still finds the panel.
- **Nothing is pushed.** Idle connections to `deviceinfo` and `displayservice` received nothing
  through four folds and unfolds, so the daemon learns of a switch from its display poll (#27), up to
  0.5 s late.
- **A running stream stays on its `displayId`.** Folded, the daemon's inner-panel stream went on
  at 60 frames per second for about 3 s (the inner panel's last pictures) and then sent about one
  small frame per second: no new parameter sets, no keyframe, no error, so the viewer kept showing the
  inner panel. The quiet-video restart never fired, since frames still came. Unfolded again,
  the same stream picked up at 60 frames per second by itself.
- **A screenshot without `displayUniqueID` captures the cover panel** even while unfolded (old
  #51, #5): black.

So on a panel change (`Display::same_panel`) the controller closes HID (its connection is bound to
the old panel's touchscreen, `touchscreen(<uniqueId>)`) and restarts our media session on the new
`displayId` (stopping only our streams, #1); the daemon attaches the frame stream to it, which sends
a `config` with the new picture, and sends a `panel` event first, on which the viewer drops the old
config and takes no touches until the new one. `screenshot` without `--display` captures the active
panel. A recording keeps only frames of its first picture size (an MP4 track has one set of
parameter sets) and reports the rest as `frames_of_another_picture`.

### 48. A foldable: the inner panel's picture is scaled down, and the panels' orientation never moves

Foldable iPhone (iOS 27.3, USB), rotation lock off. The owner turned it by hand between portrait
and both landscapes, unfolded on the home screen and in Safari, then folded and unfolded, while
`examples/turn_probe.rs` (daemon) polled `getdisplayinfo` every 0.1 s and saved the daemon's
stream as raw HEVC, one file per `config`, decoded with ffmpeg afterwards.

- **The inner panel arrives scaled down, with padding after it.** Its 2007x2853 px came as a
  1408x2016 picture (the sized wireless display of #47 is 1408x2048): the screen fills columns 0 to
  1407 and rows 0 to 2001, the panel scaled by 1408/2007 to fit the picture's width, and rows 2002
  to 2015 are the encoder's padding. The cover panel (1398x2034 in 1408x2048) and a phone with one
  panel (#11) aren't scaled. The daemon used to send the panel's own size as `display_px`; the
  viewer, sizing its canvas to that and scaling the picture to fill it (#26), stretched the padding
  in as a black band: along the bottom of the panel, which `rot90` turned to the left edge.
  `Display::picture_px` now gives the screen's size inside the picture (the panel scaled down to
  fit it, never up), and `config` sends that as `display_px`.
- **The video doesn't turn**, as on a phone with one panel (#27): the picture stays the panel in
  its native orientation, and the interface, the home screen included (it turns on this phone),
  is drawn turned inside it. In `landscapeLeft` the interface's top lies along the picture's right
  edge (`rot270`); in `landscapeRight` along its left edge (`rot90`). The same on the cover panel.
- **Each panel's `currentOrientation` stays put.** Through every turn, folded or unfolded, the
  inner panel said `rot90` and the cover `rot0`, and `nativeOrientation` was `rot0` on both. The
  daemon took the inner panel's `rot90` at face value, so the viewer turned an upright portrait
  screen on its side and never followed a turn: the whole of what looked like "rotation isn't
  followed".
- **The device's orientation follows.** The answer has a top-level `orientation` with
  `currentDeviceOrientation`, `currentDeviceNonFlatOrientation` (`portrait`, `landscapeLeft`,
  `landscapeRight`) and `currentDeviceOrientationLocked`. It changed at about the start of the
  interface's turn animation, which ended within 0.25 s, so the 0.5 s display poll (#27) keeps the
  viewer at most about half a second behind. On a foldable (`active` marks on the panels) the
  rotation now comes from `currentDeviceNonFlatOrientation`, which a phone laid flat leaves at the
  last upright orientation; without it, the panel's own. This is the device's orientation, not the
  interface's: an app that stays in portrait while the phone is held sideways would be shown turned,
  and touches placed turned. Not seen yet; no app like that was tried. A phone with one panel
  keeps using `currentOrientation`, which there is the interface orientation (#27).
- Touches after a turn on this phone were not tried (the owner's device; no input was sent). They
  go through the same `Display::report_point` as on a phone with one panel, which #27 checked.
- **Recordings keep the padding.** An MP4 has the coded picture with no crop, on every device (#11):
  14 rows at the bottom of the inner panel's picture (at the left once the track matrix turns a
  landscape recording). Cropping would need a conformance window in the SPS or a `clap` box; not
  done.

### 49. A device older than iOS 17 looked ready

iPhone 7 Plus (iOS 15.8.8, USB), trusted. `devices` listed it with no `setup` and no `error`: the
probe treated Developer Mode as on before iOS 16 (there is none) and found an image mounted at
`/Developer` (where iOS 16 and earlier mount theirs), so every step read as done, and the viewer
would attach it and fail somewhere inside the tunnel with a "connection" error. Nothing DeviceSteer uses exists
before iOS 17 (CoreDeviceProxy, RSD, RemoteXPC, the personalized image), so a device below 17 is
now `unsupported` from lockdown's public `ProductVersion` and no step is probed; the refusal comes
before any tunnel and doesn't say "connection" or "tunnel".

### 50. A device on Wi-Fi isn't listed by usbmuxd, even with Wi-Fi sync on

macOS 27, usbmuxd from MobileDevice.framework. Two devices on the same network that Xcode listed as
"available (paired)" for running apps over Wi-Fi: a foldable iPhone on iOS 27.3 and an iPad on
iPadOS 27.2.1. usbmuxd's `ListDevices` returned only the USB device, with no `Network` entry, so
`devices` couldn't list them; our code drops nothing (`transport_of` maps `Network` to Wi-Fi and
neither the daemon nor the viewer filters).

- **Xcode doesn't find them through usbmuxd.** Both advertise `_remotepairing._tcp` (port 49152),
  the remote pairing service CoreDevice pairs with on its own; `devicectl list devices` shows them
  from that. That pairing is not a lockdown pair record and usbmuxd doesn't use it.
- **The iPhone advertised `_apple-mobdev2._tcp`** (Wi-Fi sync on) as `<wifi mac>@<link-local
  ip>-supportsRP-26`, the MAC matched the `WiFiMACAddress` in usbmuxd's pair record for it, and
  lockdown on port 62078 over Wi-Fi accepted a `StartSession` with that record. usbmuxd still didn't
  list it. Maybe macOS usbmuxd leaves devices advertising `supportsRP` to CoreDevice; not confirmed
  (usbmuxd logs nothing about it).
- **The iPad didn't advertise `_apple-mobdev2._tcp`** and reset a lockdown connection on 62078: Wi-Fi
  sync is off on it.

So Wi-Fi works only when usbmuxd lists the device. Without that, listing and opening a Wi-Fi device
needs a path of our own: either find `_apple-mobdev2._tcp` devices by Bonjour and reach lockdown
over TCP with usbmuxd's pair record (idevice's `TcpProvider`; needs Wi-Fi sync and an earlier USB
pairing), or pair with `_remotepairing._tcp` ourselves (idevice `remote_pairing`:
`RemotePairingClient`, `RpPairingFile`, `connect_tls_psk_tunnel_native`; a pairing of our own, which
the device asks the user to allow). Neither is built.

### 49. Windows: the main thread's 1 MiB stack overflows on every device command

Windows PC, debug build of the CLI. Every device command ended with exit code 0xC00000FD (stack
overflow) before printing anything.
The futures for the tunnel and its services (idevice's RSD and RemoteXPC state machines and the
userspace TCP stack) are large, larger still unoptimized, and `block_on` polls the outermost one
on the calling thread. That fits in the 8 MiB main thread of macOS and Linux; Windows gives the
main thread 1 MiB (the linker's default), and tokio's worker threads get 2 MiB everywhere.

Linking with `/STACK:8388608` fixed it, but only for the binaries that pass the flag and only on
the main thread. The CLI now runs every command on a thread it starts with 8 MiB, and gives the
runtime's workers the same (`STACK_SIZE` in `crates/cli/src/main.rs`); the daemon runs on that
thread too. The viewer only talks to the daemon, whose client futures are small.

### 50. Windows: a child inherits every inheritable handle, so the daemon held the CLI's stdout

Windows PC. `devicesteer session status | Out-String` (or any reader of the CLI's output through a
pipe: `| jq`, an agent's shell tool) printed nothing and waited until the daemon the command had
started exited, ten idle minutes later. Run in a console it returned at once.

`CreateProcess` with handle inheritance on passes the child every inheritable handle of the
parent, not only the three it names as the child's stdin, stdout and stderr, and a process's own
standard handles are inheritable. The daemon got `null` and a log file as its standard handles,
but also a copy of the pipe the CLI's stdout wrote to; the reader saw no end of file while the
daemon lived. (On Unix, Rust opens everything close-on-exec, so only the named handles pass.)

`spawn_daemon` clears the inherit flag on the CLI's own standard handles before starting the
daemon (`keep_std_handles_from_children` in `crates/daemon/src/client.rs`). Rust's own spawns
don't rely on the flag: they duplicate the handles they pass.

### 51. Windows: Apple Devices' usbmuxd can miss a device connected before it started

Windows PC with the Store app "Apple Devices" (its usbmuxd is `AppleMobileDeviceProcess`), seen by
the owner. An iPad that was already plugged in when the service started was not in usbmuxd's list,
so `devices` was empty and commands said no device was connected, while Windows itself showed the
device. Unplugging it and plugging it back in made usbmuxd list it.

The errors for a missing device and an empty `devices` list on Windows say to replug it
(`missing_device_hint` in `crates/core/src/session.rs`). Not yet known: whether restarting the
Apple Mobile Device Service alone also recovers it.

### 52. Windows: the check after mounting the disk image stalled

Windows PC, iPad on iPadOS 27.2.1 over USB, `setup disk-image --no-daemon`. The image was
downloaded, signed and mounted in 17 s (the mounter answered the mount), then the command failed
with `mounted image list timed out after 12s (connection stalled)`; `setup status` right after said
the image was mounted. That check opens its own `mobile_image_mounter` connection and asks for
`CopyDevices` while the mount's mounter and lockdown connections were still open.

`setup::disk_image` now drops both before the check, and retries a failed check once after a
second, saying the mount itself succeeded if it fails again. Not confirmed on the device yet: the
image stays mounted until the device restarts, so the failing run couldn't be repeated.
