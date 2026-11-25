# Device setup without Xcode

`devicesteer setup` makes a freshly connected iPhone or iPad usable. Three steps, in order:

| Step | JSON name | What happens | Who acts |
|---|---|---|---|
| Trust this computer | `trust` | lockdown `Pair` over USB; the device asks "Trust This Computer?"; the pair record goes to usbmuxd | the user taps Trust and enters the passcode |
| Developer Mode | `developer_mode` | AMFI shows the switch in Settings > Privacy & Security; setup waits while the device restarts | the user turns it on, taps Restart, then Turn On and the passcode |
| Developer tools | `disk_image` | the personalized Developer Disk Image (DDI) is signed for the device by Apple's TSS server, uploaded and mounted | nobody; may be needed again after a restart (PITFALLS #32) |

Code: `crates/core/src/setup.rs`. CLI: `devicesteer setup [status|trust|developer-mode|disk-image]`.
Viewer: a device that isn't ready is listed with "Needs setup: <step>", and selecting it shows a
card with the three steps instead of View Screen (`renderSetup` in `app/src/main.ts`, Tauri
commands `setup_status`, `setup_run` with progress, `setup_cancel`). The card re-reads the state
every 3 s, so steps done elsewhere (Finder, Settings) show up, and attaches the device once it is
ready. `?mock&select=6` (Trust), `7` (Developer Mode), `8` (developer tools) show each step in a
browser; `setup=deny|fail` makes the steps end badly.
`devices` lists a device that isn't ready with `setup: <first step left>`. Observations behind the
code: `docs/PITFALLS.md` #30 and #31.

**Older than iOS 17: unsupported.** DeviceSteer reaches everything through CoreDeviceProxy, RSD and
RemoteXPC, which a device has only from iOS 17. Such a device is listed with `unsupported` (a
sentence saying why) and no `setup`: no step is checked or run, `setup status` says `ready: false`
with every step `blocked`, the setup commands refuse it, and the daemon client and `Session::open`
refuse it before any tunnel is tried (the message doesn't say "connection", so the daemon doesn't
retry it). The viewer lists it under Unavailable as "Needs iOS 17 or later" and explains instead of
showing setup (`?mock&select=9`). The OS version comes from lockdown's public values, so this works
before trust too. PITFALLS #49.

## How each step is detected

- **trust**: usbmuxd has a pair record for the UDID (`ReadPairRecord`) and lockdown accepts a
  session with it. A record the device rejects (`InvalidHostID`: Trust settings reset, the device
  erased) counts as not trusted.
- **developer_mode**: lockdown value `DeveloperModeStatus` in domain `com.apple.security.mac.amfi`.
- **disk_image**: `mobile_image_mounter` `CopyDevices` lists a mounted image at `/System/Developer`.
  Not `LookupImage` (PITFALLS #31).

All three go through lockdown, not the tunnel, so they work before the developer services exist.

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
   macOS, `%LOCALAPPDATA%\devicesteer` on Windows).
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

Pairing saves the record through Apple's usbmuxd (installed with Apple Devices or iTunes), which
keeps its own store. There is no Xcode copy on Windows, so the disk image comes from `--ddi` or
the download.

If `devices` lists nothing while the device is plugged in, unplug it and plug it back in: Apple
Devices' usbmuxd can miss a device that was connected before its service started (PITFALLS #51).
