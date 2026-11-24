# iPhone Use

Operate a **real iPhone or iPad** connected over USB or Wi-Fi, from macOS or Windows.

> **Status:** rewrite in progress. The Swift/macOS version (up to v0.7.1) was removed; this branch
> rebuilds it as a Rust core with a CLI and a Tauri viewer, talking to the device through
> [idevice](https://github.com/jkcoxson/idevice) instead of Xcode's CoreDevice. Today it can list
> devices, list the services a device advertises, and take a screenshot.

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
```

Output is JSON. The viewer app (`app/`) lists devices for now:

```sh
cd app && npm install && npx tauri dev
```

## License

MIT. See [LICENSE](LICENSE). Code adapted from other projects is listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
