# iPhone Use

Operate a **real iPhone or iPad** connected over USB or Wi-Fi, from macOS or Windows.

> **Status:** rewrite in progress. The Swift/macOS version (up to v0.7.1) was removed; this branch
> rebuilds it as a Rust core with a CLI and a Tauri viewer, talking to the device through
> [idevice](https://github.com/jkcoxson/idevice) instead of Xcode's CoreDevice. Today it can read
> the screen's accessibility tree, tap, swipe and press buttons, launch apps, record the screen,
> change Device Hub's settings (appearance, text size, accessibility switches, VoiceOver, simulated
> location), and show the live screen in a viewer where a human can watch or take over from an agent.

## Requirements

- An iPhone or iPad on iOS 17 or later. `iphone-use setup` makes a new one ready without Xcode:
  it pairs ("Trust This Computer"), shows the Developer Mode switch, and mounts the Developer Disk
  Image (from Xcode's copy when there is one, else Apple's files downloaded from a pinned commit of
  [DeveloperDiskImage](https://github.com/doronz88/DeveloperDiskImage) and checked by SHA-256).
  This repository contains no Apple files. See [docs/SETUP.md](docs/SETUP.md).
- macOS: nothing extra (usbmuxd ships with the OS). Windows: Apple Devices or iTunes, for usbmuxd.
- Rust 1.88+; Node.js 20+ for the viewer app.

## Use

```sh
cargo build
./target/debug/iphone-use devices
./target/debug/iphone-use setup      --udid <UDID>            # trust, Developer Mode, disk image
./target/debug/iphone-use session start --udid <UDID> "turn on dark mode"   # prints a session id
export IPHONE_USE_SESSION=<id>                                # or --session <id> on each command
./target/debug/iphone-use services   --udid <UDID>
./target/debug/iphone-use screenshot --udid <UDID> -o screen.png
./target/debug/iphone-use ui         --udid <UDID>            # elements with @eN refs
./target/debug/iphone-use tap        --udid <UDID> @e3 --label "open Display settings"
                                                              # or a label, or x,y in points
./target/debug/iphone-use swipe      --udid <UDID> 200,700 200,300 --place
./target/debug/iphone-use button     --udid <UDID> home
./target/debug/iphone-use launch     --udid <UDID> com.apple.calculator
./target/debug/iphone-use record start --udid <UDID> -o screen.mp4   # then: record stop
./target/debug/iphone-use settings   --udid <UDID>            # appearance, text size, switches, ...
./target/debug/iphone-use settings set --udid <UDID> appearance dark   # or text-size ax1, voiceover off,
                                                              # location set 37.3349 -122.0090, ...
./target/debug/iphone-use session handoff --udid <UDID> --reason "enter the passcode"
./target/debug/iphone-use session finish  --udid <UDID>
```

**Agents:** run `iphone-use skill` for the usage guide that matches the binary (sessions, handing off
to the user, reading the screen, labels). `skills/iphone-use/SKILL.md` is a bootstrap that points
there.

Output is JSON. Device commands go through a per-device daemon that the first command starts
(`daemon status`, `daemon stop`). The viewer app (`app/`) shows the live screen; while an agent
session runs it glows around the screen and shows the session at the bottom, with Take over / Hand
back (Done after a handoff) and Stop:

```sh
cd app && npm install && npx tauri dev
```

## License

MIT. See [LICENSE](LICENSE). Code adapted from other projects is listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
