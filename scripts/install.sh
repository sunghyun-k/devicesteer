#!/bin/sh
# Installs or updates DeviceSteer on macOS from GitHub Releases, from a terminal:
#
#   curl -fsSL https://github.com/sunghyun-k/devicesteer/releases/latest/download/install.sh | sh
#
# Options go after `sh -s --` when the script is piped:
#
#   curl -fsSL .../install.sh | sh -s -- --version 1.0.0
#
#   --version <v>  install that release instead of the latest (also DEVICESTEER_VERSION)
#   --dir <folder> install DeviceSteer.app into <folder> instead of /Applications
#   --quit         quit a running DeviceSteer first, instead of stopping with a message
#   --no-agents    don't set up the devicesteer command and the agent skill
#
# It downloads the release's DeviceSteer.app.tar.gz (the same notarized, stapled app the in-app
# updater installs) and SHA256SUMS, checks the archive against it and the app's code signature, and
# swaps the app into /Applications (~/Applications when /Applications isn't writable; wherever an
# existing copy is, to update it). Then it runs the app's own `devicesteer agents install`, which
# links the devicesteer command and the skill as the app's Settings › Agents does. Files fetched
# with curl carry no quarantine, so Gatekeeper doesn't stop the first launch; the signature check
# above stands in for it. The checksum catches a broken download, not a compromised release: both
# files come from the same place.
#
# POSIX sh: `curl | sh` runs it with whatever sh is, not bash.
set -eu

repo=sunghyun-k/devicesteer
app=DeviceSteer.app
archive=DeviceSteer.app.tar.gz

say() { printf '%s\n' "$*"; }
fail() { printf 'install.sh: %s\n' "$*" >&2; exit 1; }

version=${DEVICESTEER_VERSION:-}
dir=
quit=0
agents=1
while [ $# -gt 0 ]; do
  case $1 in
    --version) [ $# -ge 2 ] || fail "--version needs a version, for example 1.0.0"; version=$2; shift 2 ;;
    --version=*) version=${1#*=}; shift ;;
    --dir) [ $# -ge 2 ] || fail "--dir needs a folder"; dir=$2; shift 2 ;;
    --dir=*) dir=${1#*=}; shift ;;
    --quit) quit=1; shift ;;
    --no-agents) agents=0; shift ;;
    -h | --help) say "usage: install.sh [--version <v>] [--dir <folder>] [--quit] [--no-agents]"; exit 0 ;;
    *) fail "unknown option '$1' (options: --version <v>, --dir <folder>, --quit, --no-agents)" ;;
  esac
done

if [ "$(uname -s)" != Darwin ]; then
  say "This installer is for macOS. On Windows 10 or 11 (x64), run this in PowerShell:"
  say ""
  say "  irm https://github.com/$repo/releases/latest/download/install.ps1 | iex"
  say ""
  say "DeviceSteer runs on macOS and Windows only."
  exit 1
fi
for tool in curl tar shasum ditto codesign; do
  command -v "$tool" >/dev/null 2>&1 || fail "'$tool' is missing"
done

# ---------- which release ----------

if [ -n "$version" ]; then
  version=${version#v}
  tag="v$version"
else
  # GitHub redirects releases/latest to releases/tag/<tag>; reading the tag once, then downloading
  # by tag, keeps the archive and SHA256SUMS from two different releases if one is published
  # meanwhile. No API call: unauthenticated API requests are rate-limited.
  latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest") \
    || fail "could not reach github.com/$repo to find the latest release"
  tag=${latest##*/}
  case $tag in
    v[0-9]*) version=${tag#v} ;;
    *) fail "could not find the latest release (github.com answered $latest)" ;;
  esac
fi
base="https://github.com/$repo/releases/download/$tag"

# ---------- where ----------

if [ -n "$dir" ]; then
  mkdir -p "$dir" || fail "could not create $dir"
  dest_dir=$(cd "$dir" && pwd)
elif [ -d "/Applications/$app" ]; then
  dest_dir=/Applications
elif [ -d "$HOME/Applications/$app" ]; then
  dest_dir=$HOME/Applications
elif [ -w /Applications ]; then
  dest_dir=/Applications
else
  dest_dir=$HOME/Applications
  mkdir -p "$dest_dir" || fail "could not create $dest_dir"
fi
dest="$dest_dir/$app"
[ -w "$dest_dir" ] || fail "can't write to $dest_dir; pass --dir <folder> to install somewhere else"
case $dest_dir in
  /Volumes/*) fail "$dest_dir is on a disk image or another volume; install into /Applications or your home folder" ;;
esac

previous=
if [ -d "$dest" ]; then
  previous=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$dest/Contents/Info.plist" 2>/dev/null) || previous=unknown
fi

# The viewer of the copy being replaced. Matched by its path, so a DeviceSteer elsewhere (another
# folder, a development build) is left alone.
viewer_pids() { pgrep -f "^$dest/Contents/MacOS/devicesteer-app" 2>/dev/null || true; }

pids=$(viewer_pids)
if [ -n "$pids" ]; then
  if [ "$quit" = 1 ]; then
    say "Quitting DeviceSteer..."
    # SIGTERM, not AppleScript's `quit`: sending Apple events from a terminal asks the person for
    # Automation permission, which an SSH session or an agent can't answer.
    # shellcheck disable=SC2086 # one pid per word
    kill $pids 2>/dev/null || true
    n=0
    while [ -n "$(viewer_pids)" ]; do
      n=$((n + 1))
      [ "$n" -le 20 ] || fail "DeviceSteer didn't quit; quit it (DeviceSteer › Quit DeviceSteer) and run this again"
      sleep 0.5
    done
  else
    fail "DeviceSteer is running from $dest. Quit it (DeviceSteer › Quit DeviceSteer) and run this again, or pass --quit to quit it (piped: curl -fsSL .../install.sh | sh -s -- --quit)"
  fi
fi

# ---------- download and check ----------

tmp=$(mktemp -d "${TMPDIR:-/tmp}/devicesteer-install.XXXXXX")
staging=
cleanup() {
  rm -rf "$tmp"
  [ -z "$staging" ] || rm -rf "$staging"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

say "Downloading DeviceSteer $version..."
curl -fsSL --retry 3 -o "$tmp/$archive" "$base/$archive" \
  || fail "could not download $base/$archive (is $tag a DeviceSteer release?)"
curl -fsSL --retry 3 -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" \
  || fail "release $tag has no SHA256SUMS to check the app against (releases up to 1.0.0-beta.4 have none for macOS): download the disk image from https://github.com/$repo/releases/tag/$tag instead"

# Lines are "<hash>  <name>" (shasum's and sha256sum's format; a `*` marks binary mode).
expected=$(awk -v n="$archive" '$2 == n || $2 == "*" n { print tolower($1) }' "$tmp/SHA256SUMS")
[ -n "$expected" ] \
  || fail "SHA256SUMS of $tag doesn't list $archive (releases up to 1.0.0-beta.4 don't): download the disk image from https://github.com/$repo/releases/tag/$tag instead"
actual=$(shasum -a 256 "$tmp/$archive" | awk '{ print tolower($1) }')
[ "$actual" = "$expected" ] || fail "the downloaded app's SHA-256 is $actual, not $expected; nothing was installed"

mkdir "$tmp/app"
tar -xzf "$tmp/$archive" -C "$tmp/app" || fail "could not unpack $archive"
[ -d "$tmp/app/$app" ] || fail "$archive has no $app"
codesign --verify --deep --strict "$tmp/app/$app" 2>/dev/null \
  || fail "the downloaded app's code signature doesn't verify; nothing was installed"

# ---------- install ----------

# Copied next to the destination first, then renamed into place, so a failure halfway never
# leaves a broken app at $dest. ditto keeps the signature intact.
staging="$dest_dir/.$app.install-$$"
rm -rf "$staging"
ditto "$tmp/app/$app" "$staging" || fail "could not copy the app into $dest_dir"
if [ -d "$dest" ]; then
  old="$dest_dir/.$app.old-$$"
  mv "$dest" "$old" || fail "could not move the old $dest aside; nothing was changed"
  if ! mv "$staging" "$dest"; then
    mv "$old" "$dest" || true
    fail "could not put the new app at $dest; the old one was kept"
  fi
  rm -rf "$old"
else
  mv "$staging" "$dest" || fail "could not put the app at $dest"
fi
staging=

if [ "$previous" = "$version" ]; then
  say "DeviceSteer $version is installed at $dest (reinstalled over the same version)."
elif [ -n "$previous" ]; then
  say "DeviceSteer $version is installed at $dest (updated from $previous)."
else
  say "DeviceSteer $version is installed at $dest."
fi

# Device daemons run the CLI from the bundle and keep the old code until they stop.
if [ -n "$previous" ] && pgrep -f "^$dest/Contents/MacOS/devicesteer daemon" >/dev/null 2>&1; then
  say "Device daemons of the old version are still running; they pick up the new one once they stop (devicesteer daemon stop, or when idle)."
fi

# ---------- agents ----------

cli="$dest/Contents/MacOS/devicesteer"
if [ "$agents" = 0 ]; then
  say "Agents weren't set up (--no-agents). Run $cli agents install, or use DeviceSteer › Settings › Agents."
elif ! "$cli" agents --help >/dev/null 2>&1; then
  say "This version can't set up agents from the command line: open DeviceSteer and use Settings › Agents."
elif result=$("$cli" agents install); then
  # The result is JSON; osascript's JavaScript reads it without any other tool.
  osascript -l JavaScript - "$result" <<'JS' 2>/dev/null || say "$result"
function run(argv) {
  const r = JSON.parse(argv[0]);
  const out = [];
  const cli = r.installed.find((i) => i.id === "cli");
  if (cli) out.push(`The devicesteer command: ${cli.path}`);
  for (const i of r.installed.filter((i) => i.id !== "cli")) out.push(`The DeviceSteer skill for ${i.name}: ${i.path}`);
  if (r.installed.length === 1 && cli) out.push("No coding agent was found; the skill can be added later: devicesteer agents install <id>");
  for (const i of r.skipped) out.push(`Skipped ${i.path}: ${i.error}`);
  for (const n of r.notes) out.push(n);
  if (r.skipped.length) out.push("To replace what is there (it is moved aside, not deleted): devicesteer agents install --replace " + r.skipped.map((i) => i.id).join(" "));
  if (r.on_path === false) out.push('~/.local/bin isn\'t on your PATH: add  export PATH="$HOME/.local/bin:$PATH"  to your shell profile.');
  return out.join("\n");
}
JS
else
  say "Setting up agents failed (see above): open DeviceSteer and use Settings › Agents."
fi

say ""
say "Next: connect an iPhone or iPad and open DeviceSteer, or run: devicesteer setup"
