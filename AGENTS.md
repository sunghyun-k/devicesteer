# AGENTS.md

Guide for anyone changing the code of this repository: coding agents and human contributors alike.

## Essentials

1. **Branch:** the rewrite happens on `next`. Don't commit to `main`.
2. **Language:** reply in the user's language; write code, comments, docs and commits in English.
3. **Odd code:** before changing it, read the `PITFALLS #N` entry it cites in `docs/PITFALLS.md`.
4. **Real devices are personal devices:** pass `--udid`, look at the screen before any input, never
   approve prompts, restore what you change, never send `stopAll` to the display service. Details
   in "Verifying on a real device".
5. **Don't commit or push unless asked.**

## Language

- Reply to the user in the user's language.
- Everything that lands in the repository is English: code comments, doc comments, CLI help,
  output, error messages, Markdown and commit messages.
- Keep a non-English string literal only when the code compares it against what the device shows or
  types, with a short English comment saying why.

## What this is

A rewrite of iphone-use (formerly a Swift CLI on Xcode's private CoreDevice stack) as a Rust core
that works on macOS and Windows, with no Xcode dependency. The device layer is
[idevice](https://github.com/jkcoxson/idevice):

```text
usbmuxd (USB or Wi-Fi) -> lockdown -> CoreDeviceProxy -> CDTunnel
  -> userspace TCP stack (jktcp) -> RSD handshake -> RemoteXPC services
```

Planned shape: one daemon per device owns the tunnel and the screen stream; the CLI, an MCP server
for agents, and the Tauri viewer (live screen, human control, take over/stop while an agent drives,
touch indicators, recording) all attach to it.

There is no public API for any of this, so **observation, not documentation, is the source of
truth.** `docs/PITFALLS.md` records why things are the way they are.

## Layout

```text
Cargo.toml          Workspace; pins idevice to a git commit (see below)
crates/core/        iphone-use-core: device list, tunnel session, RSD services, screenshot
crates/cli/         iphone-use-cli, binary `iphone-use`: devices, services, screenshot (JSON output)
app/                Tauri v2 viewer: Vite + TypeScript front end
app/src-tauri/      Tauri shell (workspace member), depends on iphone-use-core
docs/PITFALLS.md    Numbered observations, cited from code as PITFALLS #N
```

### The idevice pin

`idevice` is a git dependency pinned to a commit (`rev = ...` in the root `Cargo.toml`), not the
crates.io release: fixes to the tunnel, usbmuxd, RSD and TSS code are unreleased. Move the pin on
purpose, and re-run the device checks below when you do. It is built with the `ring` TLS backend so
Windows builds need no CMake or NASM.

### Borrowed code

[iphone-cli](https://github.com/HyunSeob/iphone-cli) (MIT, also on idevice) is the reference for
what idevice lacks: stopping only our own display streams, HID input timing, waiting for the first
media packet and reconnecting. Don't vendor it wholesale; write ours on idevice. When you copy or
closely adapt code from it (or from any other project), say so in a comment at the top of the file
with the source path, and add the project's MIT notice to `THIRD_PARTY_NOTICES.md`.

## Build and run

```sh
cargo build                                    # core + CLI + Tauri shell
./target/debug/iphone-use devices

cd app && npm install
npx tauri dev                                  # viewer, hot reload
npx tauri build --bundles app                  # macOS .app (use nsis on Windows)
```

There is no test suite against devices; a clean `cargo build` and `cargo clippy --workspace` is the
minimum for every change. Pure logic (parsing, geometry) should get unit tests.

## Conventions

- **Comments explain why, not what.** The best comment says what breaks if the code is done the
  obvious way. When you learn such a thing by observation, add a `docs/PITFALLS.md` entry and cite it.
- **The core crate owns all device I/O.** The CLI, the app and later the daemon/MCP stay thin.
- **Output:** machine-readable output is JSON on stdout. Errors go to stderr as
  `{"error": "..."}` with exit code 1, and tell the reader what to do next: agents follow them
  literally.
- **Connection-failure messages say "connection" or "tunnel".** The future daemon drops its cached
  session when it sees those words.
- **Keep private data out of the repository:** no real names, device names, SSIDs, UDIDs, message
  contents or third-party app specifics in code, docs, examples or commit messages.

## Verifying on a real device: rules you must follow

A connected phone or tablet is usually **someone's personal device**, with messaging and banking apps
on it.

- **If several devices are connected, the owner decides which one to test on.** Always pass `--udid`.
- **Look at the current screen (`screenshot`) before any input.** The owner may have just been using
  another app. If the screen isn't what you expected, stop and ask.
- **Back up the clipboard before pasting** and restore it afterwards. Handle the contents only
  through files; never print them into logs.
- **Never approve a permission or consent prompt** on the owner's behalf. Decline it or hand it to
  the owner.
- **Restore anything the owner would notice**: input source, volume, brightness, settings. If you
  can't restore something, say so.
- **Never send `stopAll` to the display service** (or anything like `stop_all_streams`). It ends
  other sessions' streams and has wedged the device's media daemon until a reboot (PITFALLS #1).
  Stop only the streams you started.
- **Don't record private data you see on screen** (message previews, codes, account details)
  anywhere, including chat and reports. Don't commit screenshots or device logs.
- **Destructive actions** (delete, send, report, purchase) only when the owner explicitly names the
  target.
- **Device syslog contains personal data:** filter by process, look only at the lines you need,
  never save or paste it wholesale.

**A dedicated test device.** The owner may say a device is test equipment and lift some of these
rules for it (approving prompts, changing settings, installing apps). That covers only the device
and the rules they named, in that session. The rules about private data and about `stopAll` always
hold. Say at the end what you left changed.

**Device-specific traps** (see `docs/PITFALLS.md`): USB and Wi-Fi share code but not timing, so test
input and teardown on Wi-Fi too; iPad touch coordinates follow the panel's native orientation;
foldables have two panels.

## Commits

- **Agents: don't commit or push unless asked.**
- **Subject:** one line, English, imperative, saying what now works or what changed.
- **Body:** the changes, as bullet points. Non-obvious reasoning goes in code comments and
  `docs/PITFALLS.md`; the message points to them.
- **Before committing, remove investigation leftovers:** code marked `TEMP`, ad-hoc debug
  environment variables, temporary subcommands.
- **Don't commit** screenshots, device logs, or anything captured from a personal device
  (`.gitignore` ignores `*.png` outside the app icons for that reason).
