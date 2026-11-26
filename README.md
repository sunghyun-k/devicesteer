# DeviceSteer

See and control your connected iPhone or iPad. DeviceSteer operates a **real iPhone or iPad**
connected over USB or Wi-Fi, from macOS or Windows, for you or for an agent.

https://github.com/user-attachments/assets/87d2fa16-3cd8-4961-99bb-e5d61fb6d757

See it on the homepage: [devicesteer.sunghyunkim.com](https://devicesteer.sunghyunkim.com)

An agent reads the screen's accessibility tree, taps, swipes, types, presses buttons, launches
apps, records the screen and changes the device's settings (appearance, text size, Liquid Glass,
accessibility switches, VoiceOver, simulated location). The viewer shows the live screen, where you
watch the agent work and can take over at any moment. No Xcode needed: DeviceSteer talks to the
device through [idevice](https://github.com/jkcoxson/idevice).

DeviceSteer 1.0 is in beta.

## Install

### macOS

1. Download [`DeviceSteer_universal.dmg`](https://github.com/sunghyun-k/devicesteer/releases/latest/download/DeviceSteer_universal.dmg) (the latest
   [release](https://github.com/sunghyun-k/devicesteer/releases/latest)), open it and drag
   DeviceSteer to Applications.
2. Open DeviceSteer. On first launch it offers to set up agents: the `devicesteer` command
   (`~/.local/bin/devicesteer`) and the DeviceSteer skill for the coding agents it finds (Claude
   Code, Codex and others that read `~/.agents/skills`, Cursor, GitHub Copilot CLI). Both are links
   into the app, so they update with it. Change this any time in Settings › Agents (⌘,).
3. If the pane says `~/.local/bin` isn't on your PATH, add it to your shell profile:
   `export PATH="$HOME/.local/bin:$PATH"`.

### Windows (10 and 11, x64)

1. For USB, install **Apple Devices** from the Microsoft Store (or iTunes), open it once and accept
   its license: it brings the usbmuxd service DeviceSteer talks to. A device paired over Wi-Fi
   needs neither.
2. Install DeviceSteer from PowerShell (recommended):

   ```powershell
   irm https://github.com/sunghyun-k/devicesteer/releases/latest/download/install.ps1 | iex
   ```

   It downloads the latest installer, checks it against the release's `SHA256SUMS` and installs it
   for your account only (in `%LOCALAPPDATA%\DeviceSteer`), without administrator rights. Or
   download [`DeviceSteer_x64-setup.exe`](https://github.com/sunghyun-k/devicesteer/releases/latest/download/DeviceSteer_x64-setup.exe) (the latest
   [release](https://github.com/sunghyun-k/devicesteer/releases/latest)) and run it.
3. DeviceSteer isn't code-signed yet, so Windows may stop it:
   - **SmartScreen** stops the installer when a browser downloaded it ("Windows protected your
     PC"): click **More info**, then **Run anyway**. The PowerShell install doesn't hit this.
   - **Smart App Control** (Windows 11) blocks unsigned apps outright, however they were
     downloaded, and has no exception for a single app. While it is on, DeviceSteer can't be
     installed; it is listed under Windows Security › App & browser control › Smart App Control
     settings.
4. Open DeviceSteer. It offers to set up agents as on macOS: `%USERPROFILE%\.local\bin\devicesteer.exe`
   and the skill links. If `%USERPROFILE%\.local\bin` isn't on your PATH, add it to your user Path
   (Edit environment variables for your account) and open a new terminal.
5. Pairing a device over Wi-Fi (**Pair over Wi-Fi**, or `devicesteer setup wifi`) is the one time
   DeviceSteer accepts a connection from the network, so Windows Defender Firewall asks then
   whether to allow it: allow it on **private networks**, or the device can't connect to pair.
   Finding and using devices doesn't ask.

Before uninstalling, turn the items off in Settings › Agents: the uninstaller removes the app's
folder but not the links it made (see [docs/SETUP.md](docs/SETUP.md#uninstalling)).

The app updates itself from GitHub Releases. To build from source instead, see
[Build from source](#build-from-source).

## Requirements

- An iPhone or iPad on iOS / iPadOS 27 or later; `devicesteer devices` marks an older one
  `unsupported` (before iOS 27 the device refuses screen sharing and remote control).
  On Wi-Fi a device shows up when usbmuxd lists it (paired over USB, with "Show this iPhone when
  on Wi-Fi" on), or once it was paired over Wi-Fi with `devicesteer setup wifi` or the viewer's
  Pair over Wi-Fi (no cable; the pairing is started on the device). `devicesteer setup` makes a new one ready without Xcode:
  it pairs ("Trust This Computer"), shows the Developer Mode switch, and mounts the Developer Disk
  Image (from Xcode's copy when there is one, else Apple's files downloaded from a pinned commit of
  [DeveloperDiskImage](https://github.com/doronz88/DeveloperDiskImage) and checked by SHA-256).
  This repository contains no Apple files. See [docs/SETUP.md](docs/SETUP.md).
- macOS: nothing extra (usbmuxd ships with the OS). Windows 10 or 11 (x64): for USB,
  [Apple Devices](https://apps.microsoft.com/detail/9NP83LWLPZ9K) from the Microsoft Store (or
  iTunes), opened once with its license accepted; devices paired over Wi-Fi work without it. See
  [docs/SETUP.md](docs/SETUP.md#windows).
- To build from source: Rust 1.88+; Node.js 20.19+ or 22.12+ for the viewer app.

## Use

```sh
devicesteer devices
devicesteer setup      --udid <UDID>            # trust, Developer Mode, disk image
devicesteer setup wifi                          # or pair over Wi-Fi, no cable
devicesteer session start --udid <UDID> "turn on dark mode"   # prints a session id
export DEVICESTEER_SESSION=<id>                 # or --session <id> on each command
devicesteer screenshot --udid <UDID> -o screen.png
devicesteer ui         --udid <UDID>            # elements with @eN refs
devicesteer tap        --udid <UDID> @e3 --label "open Display settings"
                                                # or a label, or x,y in points
devicesteer swipe      --udid <UDID> 200,700 200,300
devicesteer type       --udid <UDID> "hello"    # paste for emoji; key for return, cmd+a, ...
devicesteer button     --udid <UDID> home
devicesteer launch     --udid <UDID> com.apple.calculator
devicesteer record start --udid <UDID> -o screen.mp4   # then: record stop
devicesteer settings set --udid <UDID> appearance dark # or text-size ax1, voiceover off,
                                                # location set 37.3349 -122.0090, ...
devicesteer session handoff --udid <UDID> --reason "enter the passcode"
devicesteer session finish  --udid <UDID>
```

Output is JSON. Device commands go through a per-device daemon that the first command starts
(`daemon status`, `daemon stop`). `screenshot` saves an 8-bit PNG; `--raw` keeps the device's
16-bit one. `devicesteer --help` lists every command.

**Agents:** run `devicesteer skill` for the usage guide that matches the binary (sessions, handing
off to the user, reading the screen, labels). `skills/devicesteer/SKILL.md` is a bootstrap that
points there.

### The viewer

The viewer shows the live screen. While an agent session runs it glows around the screen and shows
the session at the bottom, with Take over / Hand back (Done after a handoff) and Stop. An agent's
`session start` shows that device's screen at once: the viewer selects it (or, while you watch
another device, shows a notice with View), comes forward without taking the keyboard, and opens
for the device when it isn't running (Settings › General › "Open the viewer when an agent starts
a session", on by default; `DEVICESTEER_NO_VIEWER=1` turns it off for scripts and CI). The sidebar
marks every device with a session: blue while the agent works, amber while it waits for you or you
have it. A device whose screen was showing when you switched away shows it again when you come
back.

Its interface is in English, Korean, Japanese, Simplified and Traditional Chinese, Spanish, French,
German, Brazilian Portuguese and Italian. It follows the system's language; Settings › General
picks another. The CLI stays in English.

## Build from source

```sh
cargo build
./target/debug/devicesteer devices
cd app && npm install && npx tauri dev    # the viewer
```

Releases: see [docs/RELEASING.md](docs/RELEASING.md).

## License

MIT. See [LICENSE](LICENSE). Code adapted from other projects is listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
