# iPhone Use

Operate a **real iPhone or iPad** connected over USB or Wi-Fi, from macOS or Windows.

> **Status:** rewrite in progress. The Swift/macOS version (up to v0.7.1) was removed; this branch
> rebuilds it as a Rust core with a CLI and a Tauri viewer, talking to the device through
> [idevice](https://github.com/jkcoxson/idevice) instead of Xcode's CoreDevice. Today it can read
> the screen's accessibility tree, tap, swipe and press buttons, launch apps, record the screen,
> and show the live screen in a viewer where a human can watch or take over from an agent.

## Requirements

- A device paired with this computer ("Trust This Computer") with Developer Mode on.
- macOS: nothing extra (usbmuxd ships with the OS). Windows: Apple Devices or iTunes, for usbmuxd.
- Rust 1.88+; Node.js 20+ for the viewer app.

## Use

```sh
cargo build
./target/debug/iphone-use devices
./target/debug/iphone-use services   --udid <UDID>
./target/debug/iphone-use screenshot --udid <UDID> -o screen.png
./target/debug/iphone-use ui         --udid <UDID>            # elements with @eN refs
./target/debug/iphone-use tap        --udid <UDID> @e3        # or a label, or x,y in points
./target/debug/iphone-use swipe      --udid <UDID> 200,700 200,300 --place
./target/debug/iphone-use button     --udid <UDID> home
./target/debug/iphone-use launch     --udid <UDID> com.apple.calculator
./target/debug/iphone-use record start --udid <UDID> -o screen.mp4   # then: record stop
```

Output is JSON. Device commands go through a per-device daemon that the first command starts
(`daemon status`, `daemon stop`). The viewer app (`app/`) shows the live screen:

```sh
cd app && npm install && npx tauri dev
```

## License

MIT. See [LICENSE](LICENSE). Code adapted from other projects is listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
