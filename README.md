# DeviceSteer

See and control your connected iPhone or iPad. DeviceSteer operates a **real iPhone or iPad**
connected over USB or Wi-Fi, from macOS or Windows, for you or for an agent.

> **Status:** rewrite in progress. The Swift/macOS version (up to v0.7.1) was removed; this branch
> rebuilds it as a Rust core with a CLI and a Tauri viewer, talking to the device through
> [idevice](https://github.com/jkcoxson/idevice) instead of Xcode's CoreDevice. Today it can read
> the screen's accessibility tree, tap, swipe and press buttons, launch apps, record the screen,
> change the device's settings (appearance, text size, accessibility switches, VoiceOver, simulated
> location), and show the live screen in a viewer where a human can watch or take over from an agent.

## Install

1. Download `DeviceSteer_<version>_universal.dmg` from
   [Releases](https://github.com/sunghyun-k/devicesteer/releases/latest), open it and drag
   DeviceSteer to Applications.
2. Open DeviceSteer. On first launch it offers to set up agents: the `devicesteer` command
   (`~/.local/bin/devicesteer`) and the DeviceSteer skill for the coding agents it finds (Claude
   Code, Codex and others that read `~/.agents/skills`, Cursor, GitHub Copilot CLI). Both are links
   into the app, so they update with it. Change this any time in Settings › Agents (⌘,).
3. If the pane says `~/.local/bin` isn't on your PATH, add it to your shell profile:
   `export PATH="$HOME/.local/bin:$PATH"`.

The app updates itself from GitHub Releases. To build from source instead, see below.

## Requirements

- An iPhone or iPad on iOS / iPadOS 27 or later; `devicesteer devices` marks an older one
  `unsupported` (before iOS 27 the device refuses screen sharing and remote control).
  On Wi-Fi a device shows up when usbmuxd lists it (paired over USB, with "Show this iPhone when
  on Wi-Fi" on), or, on iOS 27 and later, once it was paired over Wi-Fi with `devicesteer setup
  wifi` (no cable; the pairing is started on the device). `devicesteer setup` makes a new one ready without Xcode:
  it pairs ("Trust This Computer"), shows the Developer Mode switch, and mounts the Developer Disk
  Image (from Xcode's copy when there is one, else Apple's files downloaded from a pinned commit of
  [DeveloperDiskImage](https://github.com/doronz88/DeveloperDiskImage) and checked by SHA-256).
  This repository contains no Apple files. See [docs/SETUP.md](docs/SETUP.md).
- macOS: nothing extra (usbmuxd ships with the OS). Windows: Apple Devices or iTunes, for usbmuxd.

  On Windows, USB needs [Apple Devices](https://apps.microsoft.com/detail/9NP83LWLPZ9K) from the
  Microsoft Store (or iTunes): install it, open it once and accept its license, then plug the
  device in again. Without it, devices paired over Wi-Fi still work, and DeviceSteer says what to
  install. See [docs/SETUP.md](docs/SETUP.md#windows).
- To build from source: Rust 1.88+; Node.js 20+ for the viewer app.

## Use

With the app installed, run `devicesteer` instead of `./target/debug/devicesteer`.

```sh
cargo build
./target/debug/devicesteer devices
./target/debug/devicesteer setup      --udid <UDID>            # trust, Developer Mode, disk image
./target/debug/devicesteer setup wifi                          # or pair over Wi-Fi (iOS 27+)
./target/debug/devicesteer session start --udid <UDID> "turn on dark mode"   # prints a session id
export DEVICESTEER_SESSION=<id>                                # or --session <id> on each command
./target/debug/devicesteer services   --udid <UDID>
./target/debug/devicesteer screenshot --udid <UDID> -o screen.png
./target/debug/devicesteer ui         --udid <UDID>            # elements with @eN refs
./target/debug/devicesteer tap        --udid <UDID> @e3 --label "open Display settings"
                                                              # or a label, or x,y in points
./target/debug/devicesteer swipe      --udid <UDID> 200,700 200,300 --place
./target/debug/devicesteer button     --udid <UDID> home
./target/debug/devicesteer launch     --udid <UDID> com.apple.calculator
./target/debug/devicesteer record start --udid <UDID> -o screen.mp4   # then: record stop
./target/debug/devicesteer settings   --udid <UDID>            # appearance, text size, switches, ...
./target/debug/devicesteer settings set --udid <UDID> appearance dark   # or text-size ax1, voiceover off,
                                                              # location set 37.3349 -122.0090, ...
./target/debug/devicesteer session handoff --udid <UDID> --reason "enter the passcode"
./target/debug/devicesteer session finish  --udid <UDID>
```

**Agents:** run `devicesteer skill` for the usage guide that matches the binary (sessions, handing off
to the user, reading the screen, labels). `skills/devicesteer/SKILL.md` is a bootstrap that points
there.

Output is JSON. Device commands go through a per-device daemon that the first command starts
(`daemon status`, `daemon stop`). `screenshot` saves an 8-bit PNG; `--raw` keeps the device's
16-bit one. The viewer app (`app/`) shows the live screen; while an agent
session runs it glows around the screen and shows the session at the bottom, with Take over / Hand
back (Done after a handoff) and Stop. Its interface is in English, Korean, Japanese, Simplified and
Traditional Chinese, Spanish, French, German, Brazilian Portuguese and Italian: it follows the
system's language, and Settings › General picks another (`app/src/locales/`; the CLI stays in
English):

```sh
cd app && npm install && npx tauri dev
```

Releases: see [docs/RELEASING.md](docs/RELEASING.md).

## License

MIT. See [LICENSE](LICENSE). Code adapted from other projects is listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
