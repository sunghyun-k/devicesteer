# Device setup without Xcode

`devicesteer setup` makes a freshly connected iPhone or iPad usable. Three steps, in order:

| Step | JSON name | What happens | Who acts |
|---|---|---|---|
| Trust this computer | `trust` | lockdown `Pair` over USB; the device asks "Trust This Computer?"; the pair record goes to usbmuxd | the user taps Trust and enters the passcode |
| Developer Mode | `developer_mode` | AMFI shows the switch in Settings > Privacy & Security; setup waits while the device restarts | the user turns it on, taps Restart, then Turn On and the passcode |
| Developer tools | `disk_image` | the personalized Developer Disk Image (DDI) is signed for the device by Apple's TSS server, uploaded and mounted | nobody; may be needed again after a restart (PITFALLS #32) |

Code: `crates/core/src/setup.rs`. CLI: `devicesteer setup [status|trust|developer-mode|disk-image|wifi|forget]`.
Viewer: a device that isn't ready is listed with "Needs setup: <step>", and selecting it shows a
card with the three steps instead of View Screen (`renderSetup` in `app/src/main.ts`, Tauri
commands `setup_status`, `setup_run` with progress, `setup_cancel`). The card re-reads the state
every 3 s, so steps done elsewhere (Finder, Settings) show up, and attaches the device once it is
ready. `?mock&select=6` (Trust), `7` (Developer Mode), `8` (developer tools) show each step in a
browser; `setup=deny|fail` makes the steps end badly.
`devices` lists a device that isn't ready with `setup: <first step left>`. Observations behind the
code: `docs/PITFALLS.md` #30 and #31.

**Older than iOS 27: unsupported.** DeviceSteer reaches everything through CoreDeviceProxy, RSD and
RemoteXPC, which a device has only from iOS 17, and iOS 17 to 26 refuse the screen-sharing session
that the live screen, recording and touch input need ("Remote control requires iOS 27.0 or later on
this device", CoreDevice error 9021), and have no accessibility service for `ui` (PITFALLS #65).
Such a device is listed with `unsupported` (a
sentence saying why) and no `setup`: no step is checked or run, `setup status` says `ready: false`
with every step `blocked`, the setup commands refuse it, and the daemon client and `Session::open`
refuse it before any tunnel is tried (the message doesn't say "connection", so the daemon doesn't
retry it). The viewer lists it under Unavailable as "Needs iOS 27 or later" and explains instead of
showing setup (`?mock&select=9`). The OS version comes from lockdown's public values, so this works
before trust too. PITFALLS #49, #65.

## Pairing over Wi-Fi (iOS 27 and later)

A device on the same network can be paired without a cable: `devicesteer setup wifi`, or **Pair
over Wi-Fi** in the viewer. This is CoreDevice's remote pairing, the one Xcode 27's "Pair Nearby
Device" makes, not a lockdown pair record: usbmuxd doesn't know about it, and it is DeviceSteer's
own (Xcode's pairing can't be read). The pairing starts on the device:

1. On the device: Settings > Privacy & Security > Developer Mode. Under **Other Devices**, tap
   **Pair with "DeviceSteer"**. It asks "Pair with 'DeviceSteer'?" (the computer will be able to
   reach its settings and data over Ethernet or Wi-Fi): tap **Pair**.
2. The device asks for **its own passcode**. This is not DeviceSteer's code.
3. It then asks for "the 6-digit code shown on DeviceSteer". The CLI prints it in a progress line
   (`"code"`); the viewer shows it large. Type it on the device.

Meanwhile this computer advertises `_remotepairing-pairable-host._tcp` and waits (`--wait`, for the
device and again for the code). The pairing is kept in the state directory under
`remote-pairing/<UDID>.plist` (owner-only: it holds the private key), with what the device last
said about itself in `<UDID>.json`; `devicesteer setup forget --udid <UDID>` deletes both. The
device keeps its side until it is removed there.

Afterwards the device is listed with `transports: ["wifi"]` while it is awake on the network: it is
found by its `_remotepairing._tcp` advert, whose `authTag` only our stored pairing can check (the
advert's identifier changes all the time). Every command reaches it through a TLS-PSK tunnel made
after pair-verify, and USB or usbmuxd's Wi-Fi is used first when the device is there too. Trust is
done by the pairing; `developer_mode` and `disk_image` are read and done through the tunnel (lockdown
and the image mounter are RSD services there). Code: `crates/core/src/remote.rs`;
observations: `docs/PITFALLS.md` #50, #55, #56.

## How each step is detected

- **trust**: usbmuxd has a pair record for the UDID (`ReadPairRecord`) and lockdown accepts a
  session with it. A record the device rejects (`InvalidHostID`: Trust settings reset, the device
  erased) counts as not trusted.
- **developer_mode**: lockdown value `DeveloperModeStatus` in domain `com.apple.security.mac.amfi`.
- **disk_image**: `mobile_image_mounter` `CopyDevices` lists a mounted image at `/System/Developer`.
  Not `LookupImage` (PITFALLS #31).

All three go through lockdown, not the tunnel, so they work before the developer services exist.
For a device paired over Wi-Fi they go through the tunnel's lockdown and image mounter instead.

## Where the Developer Disk Image comes from

Apple ships the iOS 17+ personalized DDI only inside Xcode; there is no separate download from
Apple. **This repository contains no Apple files**: the image is read from Xcode's copy or
downloaded on the user's computer, never committed or bundled. `setup disk-image` looks, in order:

1. `--ddi <folder>` or `DEVICESTEER_DDI`: a folder with `BuildManifest.plist` and the files it
   names (or `Image.dmg` and `Image.dmg.trustcache`).
2. On macOS, `/Library/Developer/DeveloperDiskImages/iOS_DDI/Restore`, which Xcode installs (also
   through its first-launch platform install). Skipped when its manifest doesn't list the device,
   so an old Xcode doesn't block a newer phone.
3. The download cache: `<state dir>/ddi/<commit>/` (`~/Library/Application Support/devicesteer` on
   macOS, `%LOCALAPPDATA%\com.sunghyunkim.devicesteer` on Windows).
4. A download of Apple's files from the
   [DeveloperDiskImage](https://github.com/doronz88/DeveloperDiskImage) project
   (`PersonalizedImages/Xcode_iOS_DDI_Personalized`, the same files pymobiledevice3 uses), pinned
   to a commit and checked against SHA-256 hashes in `setup.rs` (`DDI_REPO_COMMIT`, `DDI_FILES`).

Whatever the source, the image only mounts with Apple's signature: TSS signs the manifest for
this device's ECID and nonce, and the device checks the image against it. A modified image can't
be mounted.

To move the pin: pick a newer commit of that project, download the three files, put their SHA-256
in `DDI_FILES` and the commit in `DDI_REPO_COMMIT`, and mount on a device. The mirror's iOS image
has one build identity per board, so a device newer than its Xcode needs a newer pin (or
`--ddi` pointing at a newer Xcode's `Restore` folder).

## What leaves the computer

- The TSS request to Apple (`gs.apple.com`): the device's chip and board ids, ECID and nonce, as
  Xcode sends them.
- The download from GitHub (`raw.githubusercontent.com`), once per pin, only when no local copy
  fits. Nothing about the device is sent.

## Windows

Wi-Fi discovery asks Windows' own mDNS responder (the DNS Client service, through
`DnsServiceBrowse`), and pairing advertises with its own mDNS socket (`mdns-sd`); neither needs
Bonjour or anything else installed. Finding and using devices opens no socket that accepts
connections, so Windows Defender Firewall doesn't ask about it. Pairing does: the device connects
to this computer, so the first `setup wifi` (or Pair over Wi-Fi in the viewer) from each copy of
the program makes the firewall ask whether DeviceSteer may accept connections. Allow it on private
networks, or the device can't connect to pair. A per-user install can't add that rule beforehand
(it needs administrator rights). If it was refused, allowing DeviceSteer under Windows Security ›
Firewall & network protection › Allow an app through firewall fixes it (PITFALLS #59, #68). A
device paired over Wi-Fi needs no usbmuxd, so it works without Apple Devices installed.

USB pairing saves the record through Apple's usbmuxd (installed with Apple Devices or iTunes), which
keeps its own store. There is no Xcode copy on Windows, so the disk image comes from `--ddi` or
the download.

If `devices` lists nothing while the device is plugged in, unplug it and plug it back in: Apple
Devices' usbmuxd can miss a device that was connected before its service started (PITFALLS #53).

For USB, install Apple Devices from the Microsoft Store
(`ms-windows-store://pdp/?ProductId=9NP83LWLPZ9K`), or iTunes, then open it once and accept its
license: its usbmuxd starts only after that (PITFALLS #53). Then plug the device in again. Until
then nothing answers on `127.0.0.1:27015`: `devices` lists only the Wi-Fi-paired devices, with a
`note` saying what to install, and other commands fail with the same advice. The viewer shows it as
a card with a button to the Store page, and lists USB devices by itself once usbmuxd answers.

### Installing

The recommended way is the PowerShell installer:

```powershell
irm https://github.com/sunghyun-k/devicesteer/releases/latest/download/install.ps1 | iex
# a given version:
& ([scriptblock]::Create((irm https://github.com/sunghyun-k/devicesteer/releases/latest/download/install.ps1))) -Version 1.0.0
```

`install.ps1` (`scripts/install.ps1`, attached to every release) finds the release through GitHub's
API, downloads the installer with `curl.exe`, checks its SHA-256 against the release's
`SHA256SUMS` and runs it silently (`/S`). Files fetched that way carry no Mark of the Web, so
SmartScreen doesn't stop the unsigned installer. The checksum catches a broken download; it can't
catch a compromised release, since both files come from the same place.

The installer itself,
[`DeviceSteer_x64-setup.exe`](https://github.com/sunghyun-k/devicesteer/releases/latest/download/DeviceSteer_x64-setup.exe)
on [Releases](https://github.com/sunghyun-k/devicesteer/releases/latest)
(`DeviceSteer_<version>_x64-setup.exe` up to 1.0.0-beta.4), is an NSIS installer for x64 Windows 10
and 11. It installs for the current user, without administrator rights, into
`%LOCALAPPDATA%\DeviceSteer`: the app (`devicesteer-app.exe`), the CLI (`devicesteer.exe`, which
also runs the daemons) and the skill (`skills\devicesteer`). Pairing records, the disk image and
logs are kept apart, in `%LOCALAPPDATA%\com.sunghyunkim.devicesteer`: updates and uninstalling keep
them, unless you tick "Delete the application data" when you uninstall. (Up to 1.0.0-beta.2 they
were in the install folder; a later version moves them the first time it runs.) WebView2 comes
with Windows; on the rare system without it, the installer downloads it.

It isn't code-signed yet. SmartScreen shows "Windows protected your PC" for an unsigned installer
a browser downloaded: **More info**, then **Run anyway**. Expect it again for each new version, since an
unsigned file's reputation starts over with every build. Smart App Control (Windows 11; on for some
new installations) blocks unsigned apps and offers no per-app exception, so DeviceSteer can't be
installed while it is on. Updates the in-app updater downloads carry no Mark of the Web, so
SmartScreen shouldn't ask for them; Smart App Control still checks them.

The installer and the uninstaller stop running `devicesteer.exe` processes first (the daemons
outlive the app, and Windows can't replace or delete a running exe; `app/src-tauri/windows/installer-hooks.nsh`).
The next command starts its daemon again.

### Uninstalling

Settings › Agents on Windows makes these, outside the app's folder:

| Path | What it is |
|---|---|
| `%USERPROFILE%\.local\bin\devicesteer.exe` | The CLI: a hard link to the installed `devicesteer.exe` (a symlink needs Developer Mode) |
| `%USERPROFILE%\.local\share\devicesteer\current` | A directory junction to the app's folder |
| `%USERPROFILE%\.claude\skills\devicesteer`, `.agents\skills\devicesteer`, `.cursor\skills\devicesteer`, `.copilot\skills\devicesteer` | Directory junctions to `current\skills\devicesteer` |

The uninstaller removes only the app's folder, shortcuts and registry entries, so **turn the items
off in Settings › Agents before uninstalling**. Otherwise the junctions are left pointing at a
missing folder (delete them; deleting a junction never deletes what it points to), and the CLI's
hard link is left as a working copy of the CLI of the version it was set up with: delete
`%USERPROFILE%\.local\bin\devicesteer.exe` too.

Why the uninstaller doesn't remove them itself: Tauri's installer runs the same uninstaller, with
nothing to tell the two apart, when a newer version is installed by hand and "Uninstall before
installing" (its default) is chosen, and removing the links there would undo the agent setup on
every manual upgrade. The in-app updater doesn't uninstall: it overwrites the files in place, which
keeps the hard link pointing at the new CLI. After a manual upgrade with "Uninstall before
installing", the junctions work again (same folder), but the hard link still holds the old CLI and
Settings › Agents lists it as not DeviceSteer's: replace it there (the old one is moved aside to
`devicesteer.exe.backup-<seconds>`, which can be deleted), or choose "Don't uninstall" when
upgrading by hand.
