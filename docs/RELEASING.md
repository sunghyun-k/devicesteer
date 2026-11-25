# Releasing DeviceSteer

Releases are built on one Mac, the **release Mac**: the one whose keychain holds the Developer ID
certificate, the notarization credentials and the updater signing key. `scripts/release.sh` does
the whole release there and stops at a **draft** GitHub release; publishing it is a human step.

## What a release contains

| Asset | For |
| --- | --- |
| `DeviceSteer_<version>_universal.dmg` | People installing for the first time (README). Signed, notarized, stapled. |
| `DeviceSteer.app.tar.gz` | The in-app updater: the same notarized, stapled app. |
| `DeviceSteer.app.tar.gz.sig` | Its updater signature (minisign, made with the updater key). |
| `latest.json` | What the updater reads: version, notes, and the archive's URL and signature for `darwin-aarch64` and `darwin-x86_64` (one universal archive serves both). |

The app is universal (arm64 + x86_64). Inside it:

- `Contents/MacOS/devicesteer-app`: the viewer.
- `Contents/MacOS/devicesteer`: the CLI, which also runs the per-device daemon (`devicesteer
  daemon`). Tauri's `externalBin`, built by `scripts/build-cli.sh`.
- `Contents/Resources/skills/devicesteer/`: the agent skill (`skills/devicesteer` in this
  repository).

Both extras come from `app/src-tauri/tauri.bundle.conf.json`, which bundling passes with
`--config`. They are not in `tauri.conf.json` because Tauri's build script requires every
`externalBin` to exist, which would break `cargo build` and `tauri dev` before the CLI is built.

The app's Settings window (Agents) links `~/.local/bin/devicesteer` and each agent's skill folder to these
through `~/.local/share/devicesteer/current` (see `app/src-tauri/src/agent_setup.rs` for why), so
an update replaces the CLI and the skill along with the app.

## One-time setup on the release Mac

Nothing here goes into the repository except the updater's **public** key.

1. **Developer ID Application certificate** in the login keychain (Xcode › Settings › Accounts ›
   Manage Certificates, or developer.apple.com). Check with
   `security find-identity -v -p codesigning`. If more than one Developer ID Application identity
   is listed, set `DEVICESTEER_SIGN_IDENTITY` to the full name of the one to use.

2. **Notarization credentials** as a notarytool keychain profile named `devicesteer-notary`:

   ```sh
   xcrun notarytool store-credentials devicesteer-notary \
     --apple-id <apple-id> --team-id <team-id>
   ```

   It prompts for an app-specific password (appleid.apple.com › Sign-In and Security), or use
   `--key <AuthKey.p8> --key-id <id> --issuer <uuid>` for an App Store Connect API key.

3. **Updater signing key.** Generate it once, on the release Mac, outside the repository:

   ```sh
   cd app
   npx tauri signer generate -w ~/.tauri/devicesteer.key
   ```

   It asks for a password (recommended) and writes `~/.tauri/devicesteer.key` (private) and
   `~/.tauri/devicesteer.key.pub` (public). Then:

   - Put the **public** key (the contents of `devicesteer.key.pub`) in
     `app/src-tauri/tauri.conf.json` at `plugins.updater.pubkey`, replacing
     `UPDATER_PUBLIC_KEY_NOT_SET`, and commit it. `release.sh` refuses to run while the
     placeholder is there.
   - Store the **private** key and its password in the keychain, where `release.sh` reads them:

     ```sh
     security add-generic-password -U -a "$USER" -s devicesteer-updater-key -w "$(cat ~/.tauri/devicesteer.key)"
     security add-generic-password -U -a "$USER" -s devicesteer-updater-key-password -w '<password>'
     ```

     Alternatively export `TAURI_SIGNING_PRIVATE_KEY` (the key, or the path of its file) and
     `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` before running the script.
   - Back the private key up somewhere safe (a password manager). **Never commit it.** If it is
     lost, installed apps can't verify any later update: users must download the next version by
     hand, and a new key pair starts over.

4. **Tools:** Xcode command-line tools, Rust (`rustup`; the script adds the `aarch64-apple-darwin`
   and `x86_64-apple-darwin` targets), Node.js 20+, and `gh` signed in to GitHub with permission
   to create releases (`gh auth login`).

## Cutting a release

1. Set the version in all three places and commit:
   - `Cargo.toml`: `[workspace.package] version`
   - `app/src-tauri/tauri.conf.json`: `version`
   - `app/package.json`: `version` (and run `npm install` in `app/` so the lock file follows)
2. Push the release branch. The script releases `main` unless `DEVICESTEER_RELEASE_BRANCH` names
   another one, and requires the local branch to match `origin`.
3. Run:

   ```sh
   scripts/release.sh 0.2.0
   ```

   It checks first and builds nothing until everything passes: the branch, a clean tree, HEAD
   pushed, the tag unused, the three versions, the updater public key, the signing identity, the
   notary profile and the updater key. Then it:

   1. builds the universal app with `tauri build`, signing the app and the bundled CLI with the
      hardened runtime;
   2. checks the bundle (both architectures, the CLI's version, the skill, the signatures);
   3. notarizes the app with `xcrun notarytool --keychain-profile devicesteer-notary` and staples
      it;
   4. archives the stapled app as `DeviceSteer.app.tar.gz` and signs it for the updater, bound to
      the version (`plugins.updater.requireSignedVersion` makes the app check that);
   5. makes the disk image, signs, notarizes and staples it;
   6. writes `latest.json` and creates the draft release `v<version>` with all four files.

   Artifacts stay in `target/release-artifacts/<version>/`.
4. On GitHub, review the draft's notes and assets, then **publish** it. The updater reads
   `releases/latest/download/latest.json`, which ignores drafts, so nobody is offered the update
   before that.

## Notes

- Notarization takes minutes and can be slower; the script waits. If Apple rejects a
  submission, the script prints its log and stops.
- The app checks for updates five seconds after launch and on request in Settings › Updates
  (Check Now). The launch check shows a banner (Restart to Update), the Settings one its own
  Install and Restart; both install and relaunch. Device daemons are separate processes
  and keep running the old code until they stop (`devicesteer daemon stop`, or they exit when
  idle).
- Windows releases are not set up yet: `scripts/build-cli.sh` builds the `.exe` sidecar, but
  signing, the installer and the agent setup (which needs symlinks) are still to do.
- A local, unsigned bundle for checking what goes into the app:

  ```sh
  cd app
  npm run bundle -- --debug --bundles app
  ```
