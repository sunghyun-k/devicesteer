#!/bin/bash
# Builds, signs, notarizes and drafts a DeviceSteer release. Runs only on the release Mac: the one
# whose keychain holds the Developer ID Application identity, the `devicesteer-notary` notarytool
# profile and the updater signing key. See docs/RELEASING.md.
#
#   scripts/release.sh 0.2.0
#
# It never prints a secret: the updater key and its password are read into variables, exported
# only to the one command that signs, and never echoed. Don't add `set -x`.
set -euo pipefail

die() { printf 'release: %s\n' "$*" >&2; exit 1; }
step() { printf '\n==> %s\n' "$*"; }

[ $# -eq 1 ] || die "usage: scripts/release.sh <version>   (for example 0.2.0)"
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
for tool in git gh node npm npx cargo rustup lipo codesign xcrun hdiutil ditto security; do
  command -v "$tool" >/dev/null 2>&1 || die "'$tool' is missing"
done
gh auth status >/dev/null 2>&1 || die "gh isn't signed in: run 'gh auth login'"

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
cargo_version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)
tauri_version=$(node -p 'require("./app/src-tauri/tauri.conf.json").version')
npm_version=$(node -p 'require("./app/package.json").version')
for v in "$cargo_version" "$tauri_version" "$npm_version"; do
  [ "$v" = "$version" ] || die "versions disagree: Cargo.toml $cargo_version, tauri.conf.json $tauri_version, package.json $npm_version; set all three to $version and commit"
done

pubkey=$(node -p 'require("./app/src-tauri/tauri.conf.json").plugins.updater.pubkey')
[ "$pubkey" != "UPDATER_PUBLIC_KEY_NOT_SET" ] && [ -n "$pubkey" ] \
  || die "plugins.updater.pubkey in tauri.conf.json is still the placeholder: see docs/RELEASING.md, 'One-time setup'"
repo=$(gh repo view --json nameWithOwner -q .nameWithOwner)
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
# Only the signing command gets them (below); nothing else inherits them.
unset TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PATH TAURI_SIGNING_PRIVATE_KEY_PASSWORD

for t in aarch64-apple-darwin x86_64-apple-darwin; do
  rustup target list --installed | grep -qx "$t" || rustup target add "$t"
done

# ---------- build ----------

out="$root/target/release-artifacts/$version"
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
(
  cd app
  # The key may be the key itself or the path of its file. --app-version binds the signature to
  # this version (the updater's requireSignedVersion checks it).
  if [ -f "$updater_key" ]; then
    TAURI_SIGNING_PRIVATE_KEY_PATH="$updater_key" TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$updater_key_password" \
      npx tauri signer sign --app-version "$version" "$archive" >/dev/null
  else
    TAURI_SIGNING_PRIVATE_KEY="$updater_key" TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$updater_key_password" \
      npx tauri signer sign --app-version "$version" "$archive" >/dev/null
  fi
)
unset updater_key updater_key_password
[ -s "$archive.sig" ] || die "signing the updater archive made no signature"

step "Making the disk image"
dmg="$out/DeviceSteer_${version}_universal.dmg"
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

printf '\nDraft release %s is ready: review the notes and assets on GitHub, then publish it.\n' "$tag"
printf 'Installed apps see the update once it is published (the updater reads releases/latest).\n'
printf 'Artifacts: %s\n' "$out"
