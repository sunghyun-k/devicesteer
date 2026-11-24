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
| Screen streams (displayservice) | #1, #2 |
| Connection and tunnel | #3, #4 |
| Screenshots | #5 |
| Touch and HID input (carried over, unverified on the new path) | #6 |

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
