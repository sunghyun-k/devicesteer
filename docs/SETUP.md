# Device setup without Xcode

`iphone-use setup` makes a freshly connected iPhone or iPad usable. Three steps, in order:

| Step | JSON name | What happens | Who acts |
|---|---|---|---|
| Trust this computer | `trust` | lockdown `Pair` over USB; the device asks "Trust This Computer?"; the pair record goes to usbmuxd | the user taps Trust and enters the passcode |
| Developer Mode | `developer_mode` | AMFI shows the switch in Settings > Privacy & Security; setup waits while the device restarts | the user turns it on, taps Restart, then Turn On and the passcode |
| Developer tools | `disk_image` | the personalized Developer Disk Image (DDI) is signed for the device by Apple's TSS server, uploaded and mounted | nobody; may be needed again after a restart (PITFALLS #32) |

Code: `crates/core/src/setup.rs`. CLI: `iphone-use setup [status|trust|developer-mode|disk-image]`.
`devices` lists a device that isn't ready with `setup: <first step left>`. Observations behind the
code: `docs/PITFALLS.md` #30 and #31.

## How each step is detected

- **trust**: usbmuxd has a pair record for the UDID (`ReadPairRecord`) and lockdown accepts a
  session with it. A record the device rejects (`InvalidHostID`: Trust settings reset, the device
  erased) counts as not trusted.
- **developer_mode**: lockdown value `DeveloperModeStatus` in domain `com.apple.security.mac.amfi`
  (treated as on before iOS 16, which has no Developer Mode).
- **disk_image**: `mobile_image_mounter` `CopyDevices` lists a mounted image at `/System/Developer`
  (`/Developer` before iOS 17). Not `LookupImage` (PITFALLS #31).

All three go through lockdown, not the tunnel, so they work before the developer services exist.

## Where the Developer Disk Image comes from

Apple ships the iOS 17+ personalized DDI only inside Xcode; there is no separate download from
Apple. **This repository contains no Apple files**: the image is read from Xcode's copy or
downloaded on the user's computer, never committed or bundled. `setup disk-image` looks, in order:

1. `--ddi <folder>` or `IPHONE_USE_DDI`: a folder with `BuildManifest.plist` and the files it
   names (or `Image.dmg` and `Image.dmg.trustcache`).
2. On macOS, `/Library/Developer/DeveloperDiskImages/iOS_DDI/Restore`, which Xcode installs (also
   through its first-launch platform install). Skipped when its manifest doesn't list the device,
   so an old Xcode doesn't block a newer phone.
3. The download cache: `<state dir>/ddi/<commit>/` (`~/Library/Application Support/iphone-use` on
   macOS, `%LOCALAPPDATA%\iphone-use` on Windows).
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
