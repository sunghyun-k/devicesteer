# Releasing DeviceSteer

Releases are built on one Mac, the **release Mac**: the one whose keychain holds the Developer ID
certificate, the notarization credentials and the updater signing key. `scripts/release.sh` does
the whole release there and stops at a **draft** GitHub release; publishing it is a human step.

The Windows installer is built on GitHub Actions (`.github/workflows/release-windows.yml`), which
attaches it to the draft. Its updater signature is still made on the release Mac, so the updater
key never leaves that Mac's keychain: `scripts/release.sh --windows` starts the workflow, waits for
it, signs the installer and adds it to `latest.json`.

## What a release contains

| Asset | For |
| --- | --- |
| `DeviceSteer_<version>_universal.dmg` | People installing for the first time (README). Signed, notarized, stapled. |
| `DeviceSteer.app.tar.gz` | The in-app updater: the same notarized, stapled app. |
| `DeviceSteer.app.tar.gz.sig` | Its updater signature (minisign, made with the updater key). |
| `DeviceSteer_<version>_x64-setup.exe` | Windows: the NSIS installer, for first installs and for the in-app updater. Not code-signed yet (see "Windows" below). |
| `DeviceSteer_<version>_x64-setup.exe.sig` | Its updater signature, made on the release Mac. |
| `install.ps1` | The PowerShell installer (`scripts/install.ps1` at the release's commit): `irm https://github.com/<repo>/releases/latest/download/install.ps1 \| iex`. |
| `SHA256SUMS` | SHA-256 of the Windows installer and `install.ps1`; `install.ps1` checks the installer against it. |
| `latest.json` | What the updater reads: version, notes, and the archive's URL and signature for `darwin-aarch64` and `darwin-x86_64` (one universal archive serves both), and `windows-x86_64` for the installer. |

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

The Windows installer puts the same three in the app's folder (`%LOCALAPPDATA%\DeviceSteer` by
default, per user): `devicesteer-app.exe`, `devicesteer.exe` next to it, and
`skills\devicesteer\`. `app/src-tauri/windows/installer-hooks.nsh` adds to Tauri's installer.

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
   to create releases and run workflows (`gh auth login`).

Windows needs nothing on the release Mac beyond this: the installer is built by GitHub's Windows
runner, which holds no secret.

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

   With `--windows` (`scripts/release.sh --windows 0.2.0`), it then also checks that
   `release-windows.yml` is on the release branch (before building anything) and, after the draft
   exists:

   7. starts the **Windows release** workflow for the version unless the draft already has
      `DeviceSteer_<version>_x64-setup.exe`, and waits for it. The workflow reads the draft's
      commit, checks the three versions there, builds the NSIS installer from that commit on a
      GitHub-hosted Windows runner (`build-windows.yml`) and uploads it to the draft with
      `install.ps1` and `SHA256SUMS`. It refuses a release that isn't a draft. While waiting, the
      script retries GitHub API errors (the 404 GitHub answers for a run it has just started, 5xx,
      network) with a backoff for about five minutes before giving up; a run that ends in anything
      but success stops the script at once with the run's URL;
   8. downloads the installer and the draft's `latest.json`, signs the installer for the updater
      (bound to the version, like the macOS archive), adds `windows-x86_64` (its URL and
      signature) to `latest.json`, and uploads `latest.json` and the `.sig` to the draft.

   `scripts/release.sh --windows-only 0.2.0` does only 7 and 8, for a draft made without
   `--windows` or after a failed Windows build. It needs the draft and changes nothing on a
   published release. The workflow can also be run by hand (Actions › Windows release › Run
   workflow, or `gh workflow run release-windows.yml -f version=0.2.0`); the installer then waits
   on the draft until `--windows-only` signs it. Without the Windows steps the release is macOS
   only, as before: `latest.json` has no `windows-x86_64` and Windows apps aren't offered it.
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
- Release builds on Windows: `build-windows.yml` also runs on pull requests and pushes to `main`
  that touch the app, the crates or the skill, and keeps the installer as a workflow artifact for a
  week, which is the way to try a change on a Windows PC before a release. `scripts/build-cli.sh`
  runs there under Git for Windows' `sh`.
- A local, unsigned bundle for checking what goes into the app:

  ```sh
  cd app
  npm run bundle -- --debug --bundles app
  ```

## Windows

- **Not code-signed.** SmartScreen warns about the installer ("More info", then "Run anyway") and
  Smart App Control on Windows 11 blocks it; README tells users. The plan is SignPath's free
  signing for open-source projects, once the project qualifies: `build-windows.yml` has the
  outline where its steps go. NSIS can't be signed from the outside only, so the exes inside are
  signed before bundling and the installer after it (two signing requests, each approved by hand).
  The updater signature is made after both, on the release Mac, so it covers the final file.
- **Version information.** SignPath requires every signed file to name the product and its version:
  Tauri gives `devicesteer-app.exe` both, `crates/cli/build.rs` gives `devicesteer.exe` ProductName
  `DeviceSteer` and ProductVersion (the package version). `build-windows.yml` checks both.
- **Why the updater key stays on the Mac:** the Windows build could sign with the key in GitHub
  secrets, but then the key would live in two places, one of them a CI service. Signing on the
  Mac costs a download and an upload per release.
- **Installer contents** are checked by the workflow with 7-Zip (`devicesteer-app.exe`,
  `devicesteer.exe`, `skills\devicesteer\SKILL.md`), as is the CLI's `--version`.
