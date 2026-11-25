#!/bin/sh
# Checks that the version is the same in the three places a release reads it from, and that it is
# the version being released. Shared by scripts/release.sh (macOS) and the Windows release
# workflow, so both refuse the same mismatches.
#
#   scripts/check-version.sh 0.2.0
set -eu

[ $# -eq 1 ] || { echo "usage: scripts/check-version.sh <version>" >&2; exit 1; }
version=${1#v}
cd "$(dirname "$0")/.."

cargo_version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)
tauri_version=$(node -p 'require("./app/src-tauri/tauri.conf.json").version')
npm_version=$(node -p 'require("./app/package.json").version')
for v in "$cargo_version" "$tauri_version" "$npm_version"; do
  if [ "$v" != "$version" ]; then
    echo "versions disagree: Cargo.toml $cargo_version, tauri.conf.json $tauri_version, package.json $npm_version; set all three to $version and commit" >&2
    exit 1
  fi
done
