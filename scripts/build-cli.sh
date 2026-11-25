#!/bin/sh
# Builds the `devicesteer` CLI for the app bundle and puts it where Tauri's `externalBin` looks:
# app/src-tauri/binaries/devicesteer-<target triple>. Tauri runs it as part of
# `beforeBuildCommand` in app/src-tauri/tauri.bundle.conf.json and tells it the target through
# TAURI_ENV_TARGET_TRIPLE (`universal-apple-darwin` for a universal build) and TAURI_ENV_DEBUG.
# The CLI also runs the per-device daemon (`devicesteer daemon`), so it is the only extra binary.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cargo=${CARGO:-cargo}
command -v "$cargo" >/dev/null 2>&1 || cargo="$HOME/.cargo/bin/cargo"

host=$(rustc -vV 2>/dev/null | sed -n 's/^host: //p')
[ -n "$host" ] || host=$("$HOME/.cargo/bin/rustc" -vV | sed -n 's/^host: //p')
triple=${TAURI_ENV_TARGET_TRIPLE:-$host}
if [ "${TAURI_ENV_DEBUG:-false}" = "true" ]; then profile=debug; flag=; else profile=release; flag=--release; fi

out="$root/app/src-tauri/binaries"
mkdir -p "$out"
ext=
case "$triple" in *windows*) ext=.exe ;; esac

build() { # <triple>
  # shellcheck disable=SC2086
  "$cargo" build $flag -p devicesteer-cli --target "$1" --manifest-path "$root/Cargo.toml"
}

target_dir=${CARGO_TARGET_DIR:-$root/target}
if [ "$triple" = "universal-apple-darwin" ]; then
  build aarch64-apple-darwin
  build x86_64-apple-darwin
  # Tauri compiles the app once per architecture for a universal build, and its build script
  # requires `externalBin` for that architecture's triple too; the bundle uses the universal one.
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    cp "$target_dir/$t/$profile/devicesteer" "$out/devicesteer-$t"
  done
  lipo -create -output "$out/devicesteer-$triple" \
    "$target_dir/aarch64-apple-darwin/$profile/devicesteer" \
    "$target_dir/x86_64-apple-darwin/$profile/devicesteer"
else
  build "$triple"
  cp "$target_dir/$triple/$profile/devicesteer$ext" "$out/devicesteer-$triple$ext"
fi
echo "build-cli: $out/devicesteer-$triple$ext"
