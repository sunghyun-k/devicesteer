#!/bin/bash
# Builds, signs, notarizes and drafts a DeviceSteer release. Runs only on the release Mac: the one
# whose keychain holds the Developer ID Application identity, the `devicesteer-notary` notarytool
# profile and the updater signing key. See docs/RELEASING.md.
#
#   scripts/release.sh 0.2.0                  macOS only
#   scripts/release.sh --windows 0.2.0        macOS, then Windows
#   scripts/release.sh --windows-only 0.2.0   Windows, added to the existing draft v0.2.0
#
# Windows: the installer is built on GitHub Actions (.github/workflows/release-windows.yml), which
# attaches it to the draft; this script starts that workflow when the draft lacks the installer,
# waits for it, then signs the installer for the updater here (the key never leaves this Mac) and
# adds `windows-x86_64` to the draft's latest.json.
#
# It never prints a secret: the updater key and its password are read into variables, exported
# only to the one command that signs, and never echoed. Don't add `set -x`.
set -euo pipefail

die() { printf 'release: %s\n' "$*" >&2; exit 1; }
step() { printf '\n==> %s\n' "$*"; }

usage="usage: scripts/release.sh [--windows | --windows-only] <version>   (for example 0.2.0)"
mac=1 windows=0
case ${1-} in
  --windows) windows=1; shift ;;
  --windows-only) mac=0 windows=1; shift ;;
  -*) die "$usage" ;;
esac
[ $# -eq 1 ] || die "$usage"
version=${1#v}
tag="v$version"
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || die "'$1' isn't a version like 0.2.0"

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
branch=${DEVICESTEER_RELEASE_BRANCH:-main}
notary_profile=devicesteer-notary
key_item=devicesteer-updater-key
key_password_item=devicesteer-updater-key-password
export PATH="$HOME/.cargo/bin:$PATH"

# ---------- checks: nothing is built until all pass ----------

step "Checking the machine"
[ "$(uname -s)" = Darwin ] || die "releases are built on macOS"
tools="git gh node npm npx security"
[ "$mac" = 1 ] && tools="$tools cargo rustup lipo codesign xcrun hdiutil ditto"
for tool in $tools; do
  command -v "$tool" >/dev/null 2>&1 || die "'$tool' is missing"
done
gh auth status >/dev/null 2>&1 || die "gh isn't signed in: run 'gh auth login'"
repo=$(gh repo view --json nameWithOwner -q .nameWithOwner)
# Asset names carry no version: the homepage links to releases/latest/download/<name>, so they must
# stay the same from release to release (docs/RELEASING.md).
installer="DeviceSteer_x64-setup.exe"

if [ "$mac" = 1 ]; then

step "Checking the tree"
[ "$(git rev-parse --abbrev-ref HEAD)" = "$branch" ] \
  || die "on branch $(git rev-parse --abbrev-ref HEAD), not $branch (set DEVICESTEER_RELEASE_BRANCH to release another branch)"
[ -z "$(git status --porcelain)" ] || die "the working tree isn't clean: commit or remove the changes first"
git fetch --quiet origin "$branch" --tags
[ "$(git rev-parse HEAD)" = "$(git rev-parse "origin/$branch")" ] \
  || die "HEAD isn't origin/$branch: push (or pull) first, so the release points at a published commit"
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null || [ -n "$(git ls-remote --tags origin "refs/tags/$tag")" ]; then
  die "tag $tag already exists"
fi

step "Checking the version"
scripts/check-version.sh "$version" || die "fix the versions first"

pubkey=$(node -p 'require("./app/src-tauri/tauri.conf.json").plugins.updater.pubkey')
[ "$pubkey" != "UPDATER_PUBLIC_KEY_NOT_SET" ] && [ -n "$pubkey" ] \
  || die "plugins.updater.pubkey in tauri.conf.json is still the placeholder: see docs/RELEASING.md, 'One-time setup'"
node -e 'process.exit(require("./app/src-tauri/tauri.conf.json").plugins.updater.endpoints.some(e => e.includes("github.com/" + process.argv[1] + "/")) ? 0 : 1)' "$repo" \
  || die "the updater endpoint in tauri.conf.json isn't on github.com/$repo"

step "Checking the signing identity"
identity=${DEVICESTEER_SIGN_IDENTITY:-}
if [ -z "$identity" ]; then
  identities=$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | sort -u)
  count=$(printf '%s' "$identities" | grep -c . || true)
  [ "$count" -eq 1 ] || die "found $count Developer ID Application identities; set DEVICESTEER_SIGN_IDENTITY to the one to use"
  identity=$identities
fi
security find-identity -v -p codesigning | grep -qF "\"$identity\"" || die "no valid signing identity '$identity' in the keychain"
echo "Signing as: $identity"

step "Checking notarization credentials"
xcrun notarytool history --keychain-profile "$notary_profile" >/dev/null 2>&1 \
  || die "notarytool profile '$notary_profile' is missing or invalid: see docs/RELEASING.md"
fi # mac

if [ "$windows" = 1 ]; then
  step "Checking the Windows workflow"
  git fetch --quiet origin "$branch"
  git cat-file -e "origin/$branch:.github/workflows/release-windows.yml" 2>/dev/null \
    || die "origin/$branch has no .github/workflows/release-windows.yml, which builds the installer"
fi

if [ "$mac" = 0 ]; then
  step "Checking the draft $tag"
  [ "$(gh release view "$tag" --json isDraft -q .isDraft 2>/dev/null)" = true ] \
    || die "no draft release $tag: run scripts/release.sh $version (or --windows) first; a published release isn't changed"
fi

step "Checking the updater key"
updater_key=${TAURI_SIGNING_PRIVATE_KEY:-${TAURI_SIGNING_PRIVATE_KEY_PATH:-}}
if [ -z "$updater_key" ]; then
  updater_key=$(security find-generic-password -s "$key_item" -w 2>/dev/null) \
    || die "no updater key: set TAURI_SIGNING_PRIVATE_KEY or store it in the keychain as '$key_item' (docs/RELEASING.md)"
fi
updater_key_password=${TAURI_SIGNING_PRIVATE_KEY_PASSWORD-}
if [ -z "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD+set}" ]; then
  updater_key_password=$(security find-generic-password -s "$key_password_item" -w 2>/dev/null || true)
fi
# Only the signing command gets them (sign_updater); nothing else inherits them.
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PATH TAURI_SIGNING_PRIVATE_KEY_PASSWORD

# Signs `file` for the updater, writing `file.sig`. The key may be the key itself or the path of its
# file. --app-version binds the signature to this version (the updater's requireSignedVersion
# checks it).
sign_updater() {
  local file=$1
  (
    cd "$root/app"
    if [ -f "$updater_key" ]; then
      TAURI_SIGNING_PRIVATE_KEY_PATH="$updater_key" TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$updater_key_password" \
        npx tauri signer sign --app-version "$version" "$file" >/dev/null
    else
      TAURI_SIGNING_PRIVATE_KEY="$updater_key" TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$updater_key_password" \
        npx tauri signer sign --app-version "$version" "$file" >/dev/null
    fi
  )
  [ -s "$file.sig" ] || die "signing $(basename "$file") for the updater made no signature"
}

out="$root/target/release-artifacts/$version"

# ---------- macOS ----------

if [ "$mac" = 1 ]; then
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  rustup target list --installed | grep -qx "$t" || rustup target add "$t"
done

rm -rf "$out"
mkdir -p "$out"

step "Building the universal app (signed, not yet notarized)"
(
  cd app
  npm ci
  # Tauri notarizes by itself when it finds Apple ID or API key variables; this script does it
  # with the keychain profile instead, after the build, so clear them.
  env -u APPLE_ID -u APPLE_PASSWORD -u APPLE_TEAM_ID -u APPLE_API_KEY -u APPLE_API_ISSUER -u APPLE_API_KEY_PATH \
    APPLE_SIGNING_IDENTITY="$identity" \
    npx tauri build --target universal-apple-darwin --bundles app --config src-tauri/tauri.bundle.conf.json
)
built="$root/target/universal-apple-darwin/release/bundle/macos/DeviceSteer.app"
[ -d "$built" ] || die "the build made no app at $built"
app="$out/DeviceSteer.app"
ditto "$built" "$app"

step "Checking the bundle"
cli="$app/Contents/MacOS/devicesteer"
[ -x "$cli" ] || die "the CLI is missing from the bundle"
for arch in arm64 x86_64; do
  lipo -archs "$cli" | grep -qw "$arch" || die "the bundled CLI lacks $arch"
  lipo -archs "$app/Contents/MacOS/devicesteer-app" | grep -qw "$arch" || die "the app binary lacks $arch"
done
[ -f "$app/Contents/Resources/skills/devicesteer/SKILL.md" ] || die "the skill is missing from the bundle"
[ "$("$cli" --version | awk '{print $2}')" = "$version" ] || die "the bundled CLI reports another version"
codesign --verify --deep --strict "$app" || die "the app's signature doesn't verify"
# Captured first: with pipefail, `grep -q` quitting early would fail the pipeline on SIGPIPE.
cli_signature=$(codesign -dv "$cli" 2>&1)
grep -q 'flags=.*runtime' <<<"$cli_signature" || die "the CLI isn't signed with the hardened runtime"

# Waits for notarization of one file and fails with Apple's log when it is rejected.
notarize() {
  local file=$1 result id status
  result=$(xcrun notarytool submit "$file" --keychain-profile "$notary_profile" --wait --output-format json)
  id=$(node -p 'JSON.parse(process.argv[1]).id' "$result")
  status=$(node -p 'JSON.parse(process.argv[1]).status' "$result")
  if [ "$status" != "Accepted" ]; then
    xcrun notarytool log "$id" --keychain-profile "$notary_profile" >&2 || true
    die "notarization of $(basename "$file") ended '$status' (submission $id)"
  fi
  echo "Notarized $(basename "$file") ($id)"
}

step "Notarizing the app"
ditto -c -k --keepParent "$app" "$out/notarize.zip"
notarize "$out/notarize.zip"
rm "$out/notarize.zip"
xcrun stapler staple "$app"
spctl --assess --type execute -vv "$app"

step "Making the updater archive"
# After stapling, so updates install a notarized app that opens offline too.
archive="$out/DeviceSteer.app.tar.gz"
COPYFILE_DISABLE=1 tar -czf "$archive" -C "$out" DeviceSteer.app
sign_updater "$archive"

step "Making the disk image"
dmg="$out/DeviceSteer_universal.dmg"
stage="$out/dmg"
mkdir -p "$stage"
ditto "$app" "$stage/DeviceSteer.app"
ln -s /Applications "$stage/Applications"
hdiutil create -volname DeviceSteer -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$dmg" >/dev/null
rm -rf "$stage"
codesign --sign "$identity" --timestamp "$dmg"
notarize "$dmg"
xcrun stapler staple "$dmg"

step "Writing latest.json"
url="https://github.com/$repo/releases/download/$tag/DeviceSteer.app.tar.gz"
# A universal app serves both architectures from one archive.
node -e '
  const [version, url, sigFile, out] = process.argv.slice(1);
  const signature = require("fs").readFileSync(sigFile, "utf8").trim();
  const entry = { signature, url };
  const manifest = {
    version,
    notes: `See https://github.com/${url.split("/").slice(3, 5).join("/")}/releases/tag/v${version}`,
    pub_date: new Date().toISOString(),
    platforms: { "darwin-aarch64": entry, "darwin-x86_64": entry },
  };
  require("fs").writeFileSync(out, JSON.stringify(manifest, null, 2) + "\n");
' "$version" "$url" "$archive.sig" "$out/latest.json"

step "Creating the draft release $tag"
gh release create "$tag" --draft --target "$(git rev-parse HEAD)" --title "DeviceSteer $version" --generate-notes \
  "$dmg" "$archive" "$archive.sig" "$out/latest.json"

fi # mac

# ---------- Windows ----------

# Starts release-windows.yml for this version and waits for it; it attaches the installer to the
# draft. The workflow file is read from $branch, so it must be on that branch.
run_windows_workflow() {
  local since started id
  since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  started=$(gh workflow run release-windows.yml --ref "$branch" -f version="$version" 2>&1) \
    || die "could not start the Windows workflow: $started"
  # gh prints the new run's URL; older versions print nothing useful, so fall back to the newest
  # dispatched run.
  id=$(sed -n 's|.*/actions/runs/\([0-9][0-9]*\).*|\1|p' <<<"$started" | head -1)
  if [ -z "$id" ]; then
    for _ in 1 2 3 4 5 6; do
      sleep 5
      id=$(gh run list --workflow release-windows.yml --event workflow_dispatch --branch "$branch" --created ">=$since" --limit 1 --json databaseId -q '.[0].databaseId' 2>/dev/null) || id=
      [ -n "$id" ] && break
    done
  fi
  [ -n "$id" ] || die "started the Windows workflow but couldn't find its run: watch it on GitHub, then run scripts/release.sh --windows-only $version"
  echo "Waiting for https://github.com/$repo/actions/runs/$id (the build takes a while)"
  wait_for_run "$id"
}

# Polls a workflow run until it completes; fails unless it succeeded. Not `gh run watch`: it gives
# up on the first API error, and right after dispatch the API answers 404 for a run it just
# created (it showed up and succeeded later), so a release failed with nothing wrong. Errors
# (404, 5xx, network) are retried with a backoff for about five minutes, counted from the last
# good answer; a run that completes with any conclusion but success fails at once.
wait_for_run() {
  local id=$1 url="https://github.com/$repo/actions/runs/$1" state errors=0
  while :; do
    if state=$(gh run view "$id" --json status,conclusion -q '.status + " " + .conclusion' 2>&1); then
      errors=0
      case $state in
        "completed success") return 0 ;;
        completed\ *) die "the Windows workflow ended with '${state#completed }': see $url, then run scripts/release.sh --windows-only $version" ;;
      esac
      sleep 30
    else
      errors=$((errors + 1))
      [ "$errors" -le 8 ] \
        || die "couldn't read the Windows workflow's status from GitHub ($state): see $url; once it succeeds, run scripts/release.sh --windows-only $version"
      sleep $((errors < 5 ? 5 << (errors - 1) : 60)) # 5, 10, 20, 40, then 60 s: about 5 minutes
    fi
  done
}

if [ "$windows" = 1 ]; then
  # Captured first: with pipefail, `grep -q` quitting early would fail the pipeline on SIGPIPE.
  has_installer() { local names; names=$(gh release view "$tag" --json assets -q '.assets[].name') && grep -qxF "$installer" <<<"$names"; }
  if has_installer; then
    step "Using the Windows installer already on the draft"
  else
    step "Building the Windows installer on GitHub Actions"
    run_windows_workflow
    has_installer || die "the Windows workflow finished but the draft has no $installer"
  fi

  step "Signing the Windows installer for the updater"
  win="$out/windows"
  rm -rf "$win"
  mkdir -p "$win"
  gh release download "$tag" --pattern "$installer" --pattern latest.json --dir "$win"
  [ -s "$win/$installer" ] && [ -s "$win/latest.json" ] || die "could not download $installer and latest.json from the draft"
  sign_updater "$win/$installer"

  step "Adding windows-x86_64 to latest.json"
  url="https://github.com/$repo/releases/download/$tag/$installer"
  node -e '
    const fs = require("fs");
    const [version, url, sigFile, file] = process.argv.slice(1);
    const manifest = JSON.parse(fs.readFileSync(file, "utf8"));
    if (manifest.version !== version) throw new Error(`latest.json on the draft is for ${manifest.version}, not ${version}`);
    // The updater looks up "<os>-<arch>-<installer>" first, then "<os>-<arch>".
    manifest.platforms["windows-x86_64"] = { signature: fs.readFileSync(sigFile, "utf8").trim(), url };
    fs.writeFileSync(file, JSON.stringify(manifest, null, 2) + "\n");
  ' "$version" "$url" "$win/$installer.sig" "$win/latest.json"
  gh release upload "$tag" "$win/$installer.sig" "$win/latest.json" --clobber
fi
unset updater_key updater_key_password

printf '\nDraft release %s is ready: review the notes and assets on GitHub, then publish it.\n' "$tag"
printf 'Installed apps see the update once it is published (the updater reads releases/latest).\n'
[ "$windows" = 1 ] || printf 'No Windows build: add it to this draft with scripts/release.sh --windows-only %s\n' "$version"
printf 'Artifacts: %s\n' "$out"
