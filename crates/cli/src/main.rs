//! `devicesteer`: command-line access to a connected iPhone or iPad.
//!
//! Output is JSON on stdout (except `skill`, which prints the agent guide as Markdown). Errors go
//! to stderr as `{"error": "..."}` with exit code 1.

use std::{path::PathBuf, time::Duration};

mod record;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use devicesteer_core::{
    Button, Controller, MediaMode, Motion, Session, Setting,
    ax::{BuildOptions, Screen},
    list_devices,
};
use devicesteer_daemon::{Client, Source};
use serde_json::json;

#[derive(Parser)]
#[command(name = "devicesteer", version, about = "Operate a real iPhone or iPad over USB or Wi-Fi")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Args)]
struct Target {
    /// Device to use. Required when more than one device is connected.
    #[arg(long, global = true)]
    udid: Option<String>,
    /// Talk to the device directly instead of through its daemon (slower: a new tunnel and
    /// media session per command, and the viewer doesn't see the input).
    #[arg(long, global = true)]
    no_daemon: bool,
    /// The agent session the command belongs to (printed by `session start`).
    #[arg(long, global = true, env = "DEVICESTEER_SESSION")]
    session: Option<String>,
}

impl Target {
    async fn client(&self) -> Result<Client> {
        Ok(Client::connect(self.udid.as_deref(), Source::Agent).await?.with_session(self.session.clone()))
    }
}

/// What an input does, in 3 to 6 words, shown next to it in the viewer.
#[derive(clap::Args)]
struct Label {
    /// What this input does, in 3 to 6 words ("open the Wi-Fi settings"). The viewer shows it next
    /// to the touch, so the person watching knows what the agent is doing.
    #[arg(long)]
    label: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// List connected devices (usbmuxd, USB and Wi-Fi).
    Devices,
    /// Open a tunnel and list the services the device advertises over RSD.
    Services {
        #[command(flatten)]
        target: Target,
    },
    /// Capture the screen as PNG.
    Screenshot {
        #[command(flatten)]
        target: Target,
        /// Where to write the PNG.
        #[arg(short, long, default_value = "screenshot.png")]
        output: PathBuf,
        /// Panel to capture (a display `uniqueId`); defaults to the panel in use (a foldable's inner
        /// or cover panel, whichever is active).
        #[arg(long)]
        display: Option<String>,
    },
    /// Show the display in use: size in points, scale, rotation.
    Display {
        #[command(flatten)]
        target: Target,
    },
    /// Read the screen's accessibility tree: elements with role, label, value, frame and an
    /// `@eN` ref that `tap` takes until the next `ui`.
    Ui {
        #[command(flatten)]
        target: Target,
        /// Also list containers with nothing to read and elements outside the screen.
        #[arg(long)]
        all: bool,
        /// Wait for the second pass, which adds accessibility identifiers (slower).
        #[arg(long)]
        full: bool,
    },
    /// Tap a point (`x,y` in screen points), an element by `@eN` ref from the last `ui`, or an
    /// element by its label (read fresh; exact match first, then a unique substring).
    Tap {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        label: Label,
        /// `x,y`, `@eN`, or a label.
        at: String,
    },
    /// Drag one finger from `x,y` to `x,y` (points).
    Swipe {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        label: Label,
        from: String,
        to: String,
        /// How long the drag takes, in milliseconds.
        #[arg(long, default_value_t = 300)]
        duration: u64,
        /// Slow to a stop before lifting, so content moves by the finger's distance and stays
        /// (a scroll step). Without it the content keeps coasting.
        #[arg(long)]
        place: bool,
    },
    /// Type text into the focused field: Latin, digits, symbols and Hangul, whatever language the
    /// on-screen keyboard shows. The keys go through a temporary hardware keyboard and the field
    /// is checked once at the end; if it doesn't match, the text is typed again on the on-screen
    /// keys (the result says so). Fails without typing when no field has the keyboard.
    Type {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        label: Label,
        text: String,
        /// Press Return after the text.
        #[arg(long)]
        enter: bool,
        /// Send the keys without looking at the screen first (no focus check, no check of the
        /// result).
        #[arg(long)]
        force: bool,
        /// `stream` (default): all keys at once through a temporary hardware keyboard, checked
        /// once at the end. `tap`: slower, checks each script and taps the on-screen keys where
        /// the HID keys type the other one.
        #[arg(long, value_parser = ["stream", "tap"], default_value = "stream")]
        method: String,
    },
    /// Paste text through the device clipboard (for emoji and other characters `type` can't
    /// send). The clipboard's contents are saved first and put back after the paste; they are
    /// never printed. The device may ask the user to allow the paste: never allow it for them.
    Paste {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        label: Label,
        /// The text to paste.
        #[arg(required_unless_present_any = ["file", "restore"])]
        text: Option<String>,
        /// Paste this UTF-8 file's contents instead (keeps the text out of the command line).
        #[arg(long, conflicts_with_all = ["text", "restore"])]
        file: Option<PathBuf>,
        /// Put back the clipboard contents a paste kept because the device asked to allow it.
        #[arg(long)]
        restore: bool,
        /// Paste even if no field seems to have the keyboard.
        #[arg(long)]
        force: bool,
    },
    /// The device clipboard on its own, with no paste. `set` replaces it with text (the user's
    /// next paste inserts it; iOS may ask them to allow that). `get --out FILE` writes the text it
    /// holds to FILE and prints only its kind and length. Only when the user asks: it is theirs.
    Clipboard {
        #[command(flatten)]
        target: Target,
        #[command(subcommand)]
        action: ClipboardAction,
    },
    /// The hardware keyboard: `connect` attaches one the device takes for a real keyboard (the
    /// on-screen keyboard hides; keys follow the device's input source, ctrl+space switches it),
    /// `disconnect` removes it (the on-screen keyboard comes back), `status` tells. `type` works
    /// either way.
    Keyboard {
        #[command(flatten)]
        target: Target,
        #[arg(value_parser = ["connect", "disconnect", "status"], default_value = "status")]
        action: String,
    },
    /// Press a key: return, backspace (also `delete`), forward_delete, tab, escape, up, down, left,
    /// right, home, end, pageup, pagedown, space, a single character, or a combination joined
    /// with + such as cmd+a, cmd+c, cmd+v, cmd+z, cmd+shift+z, shift+tab, ctrl+space (next
    /// input source).
    Key {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        label: Label,
        name: String,
    },
    /// Launch an app by bundle id, such as com.apple.calculator.
    Launch {
        #[command(flatten)]
        target: Target,
        bundle_id: String,
        /// Kill the running instance first.
        #[arg(long)]
        relaunch: bool,
    },
    /// Press a hardware button: home, lock, volume_up, volume_down, or eject (with a hardware
    /// keyboard connected, shows or hides the on-screen keyboard).
    Button {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        label: Label,
        name: String,
    },
    /// Record the screen to an MP4 (the device's HEVC stream, not re-encoded).
    Record {
        #[command(flatten)]
        target: Target,
        #[command(subcommand)]
        action: RecordAction,
    },
    /// The per-device daemon the other commands go through. Without an action it runs in the
    /// foreground (commands start it on demand, so this is rarely needed).
    Daemon {
        #[command(flatten)]
        target: Target,
        #[command(subcommand)]
        action: Option<DaemonAction>,
    },
    /// Print the daemon's events as JSON lines: control (who drives), input (gestures with their
    /// points), config and, with --frames, one line per video frame.
    Events {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        frames: bool,
        /// Stop after this many seconds.
        #[arg(long)]
        seconds: Option<f64>,
    },
    /// The device's settings: appearance, text size, Liquid Glass, accessibility switches,
    /// VoiceOver and the simulated location. Without an action, prints the current values; items
    /// the device can't read are null and listed in `unsupported`.
    Settings {
        #[command(flatten)]
        target: Target,
        #[command(subcommand)]
        action: Option<SettingsAction>,
    },
    /// An agent session: one per goal and device. The viewer shows it, and the person watching
    /// can take over or stop it. `start` prints the id the other commands take with --session.
    Session {
        #[command(flatten)]
        target: Target,
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Make a device ready without Xcode: trust this computer, Developer Mode, the Developer Disk
    /// Image. Without an action, does every step that isn't done, waiting for the user where the
    /// device needs them. Prints the state afterwards with `next`; progress goes to stderr as JSON
    /// lines.
    Setup {
        #[command(flatten)]
        target: Target,
        #[command(subcommand)]
        action: Option<SetupAction>,
        /// How long to wait for the user at a step that needs them (tap Trust, turn on Developer
        /// Mode and restart), in seconds. 0 with `developer-mode`: show the switch and return.
        #[arg(long, global = true, default_value_t = 180)]
        wait: u64,
        /// A folder with a personalized Developer Disk Image (BuildManifest.plist and the files it
        /// names) to use instead of Xcode's copy or the download. Also `DEVICESTEER_DDI`.
        #[arg(long, global = true)]
        ddi: Option<PathBuf>,
    },
    /// Print the guide for agents that matches this version of DeviceSteer (Markdown).
    Skill,
    /// Screen-sharing media streams on the device.
    Media {
        #[command(flatten)]
        target: Target,
        #[command(subcommand)]
        action: MediaAction,
    },
}

#[derive(Subcommand)]
enum SetupAction {
    /// Which steps are done and what to do next. Reads only; changes nothing.
    Status,
    /// Pair over USB: the device asks "Trust This Computer?"; the user taps Trust and enters the
    /// passcode. Waits for them up to --wait.
    Trust,
    /// Show the Developer Mode switch in Settings > Privacy & Security, then wait (up to --wait)
    /// while the user turns it on, the device restarts and they confirm with the passcode.
    DeveloperMode,
    /// Mount the personalized Developer Disk Image (Apple signs it for this device). A restart
    /// can unmount it. No action on the device.
    DiskImage,
    /// Pair a device over Wi-Fi, without a cable (iOS 27 and later). This computer offers itself
    /// as "DeviceSteer"; the user picks it on the device (Settings > Privacy & Security > Developer
    /// Mode > Pair with "DeviceSteer"), enters the device's passcode, then the 6-digit code this
    /// prints in a progress line (`code`). Waits up to --wait for each.
    Wifi,
    /// Forget this computer's Wi-Fi pairing with --udid. The device keeps its side until the user
    /// removes it there.
    Forget,
}

#[derive(Subcommand)]
enum DaemonAction {
    /// The daemon's state: connection, media streams, control, recording, clients.
    Status,
    /// Stop the daemon (it stops its media streams first).
    Stop,
}

#[derive(Subcommand)]
enum SessionAction {
    /// Start a session for one goal, named after it ("turn on dark mode"). Refused while another
    /// session uses the device.
    Start { name: String },
    /// Hand the device to the user, saying what they should do ("enter the passcode"). Input is
    /// refused until they press Done in the viewer or you run `session resume`.
    Handoff {
        #[arg(long)]
        reason: String,
    },
    /// Take the device back after a handoff, or after the user handed it back.
    Resume,
    /// End the session. With --force, end whichever session uses the device (for the user, when
    /// its agent is gone).
    Finish {
        #[arg(long)]
        force: bool,
    },
    /// The session's state: owner (agent, delegated_to_user, user, finished), handoff reason,
    /// time left before it expires. Without --session, the session using the device.
    Status,
}

#[derive(Subcommand)]
enum ClipboardAction {
    /// Replace the clipboard with text, without pasting it.
    Set {
        #[arg(required_unless_present = "file")]
        text: Option<String>,
        /// Use this UTF-8 file's contents instead (keeps the text out of the command line).
        #[arg(long, conflicts_with = "text")]
        file: Option<PathBuf>,
    },
    /// Write the clipboard's text to a file; prints its kind (text, image, empty, other) and length.
    Get {
        /// Where the text goes. It is never printed.
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum RecordAction {
    /// Start recording in the background; `record stop` ends it.
    Start {
        #[arg(short, long, default_value = "recording.mp4")]
        output: PathBuf,
    },
    /// Stop the background recording and print its summary.
    Stop,
    /// Record in the foreground until `seconds` pass, Ctrl-C, or `record stop`.
    Run {
        #[arg(short, long, default_value = "recording.mp4")]
        output: PathBuf,
        #[arg(long)]
        seconds: Option<f64>,
    },
}

#[derive(Subcommand)]
enum SettingsAction {
    /// Change one item and print the values after it: `appearance dark|light`,
    /// `text-size xs|s|m|l|xl|xxl|xxxl|ax1..ax5`, `liquid-glass <0..1>`,
    /// `reduce-motion|increase-contrast|show-borders|reduce-transparency|voiceover on|off`,
    /// `location set <latitude> <longitude>`, `location clear`. A simulated location stays until
    /// cleared.
    Set {
        name: String,
        #[arg(required = true, num_args = 1.., allow_hyphen_values = true)]
        value: Vec<String>,
    },
}

#[derive(Subcommand)]
enum MediaAction {
    /// The device's media server status (all clients' streams).
    Status,
    /// Start an input-only media session, report its numbers after `seconds`, and stop it.
    Probe {
        #[arg(long, default_value_t = 3)]
        seconds: u64,
        /// Also start the video stream.
        #[arg(long)]
        video: bool,
    },
}

/// The stack for the thread that runs commands, and for the runtime's workers.
///
/// The async state machines for the tunnel and its services are large, much larger in debug
/// builds, and `block_on` polls them on the calling thread. That fits in the 8 MiB main thread of
/// macOS and Linux but overflows Windows' 1 MiB one (0xC00000FD on every device command). A
/// thread of our own gets the same room everywhere, without a linker flag that only covers the
/// main thread and only for builds that pass it.
const STACK_SIZE: usize = 8 * 1024 * 1024;

fn main() {
    let cli = Cli::parse();
    if let Command::Skill = cli.command {
        print!("{}", skill_guide());
        return;
    }
    let commands = std::thread::Builder::new()
        .name("devicesteer".into())
        .stack_size(STACK_SIZE)
        .spawn(move || run_command(cli))
        .expect("a thread for the command");
    if commands.join().is_err() {
        // The panic message is already on stderr.
        std::process::exit(101);
    }
}

fn run_command(cli: Cli) {
    // The daemon runs on a runtime of its own (see `devicesteer_daemon::run_blocking`).
    if let Command::Daemon { target, action: None } = &cli.command {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_env("DEVICESTEER_LOG")
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .with_writer(std::io::stderr)
            .init();
        let r = tokio::runtime::Runtime::new()
            .map_err(anyhow::Error::from)
            .and_then(|rt| rt.block_on(devicesteer_core::resolve_udid(target.udid.as_deref())))
            .and_then(|udid| devicesteer_daemon::run_blocking(&udid));
        if let Err(e) = r {
            eprintln!("{}", json!({ "error": format!("{e:#}") }));
            std::process::exit(1);
        }
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(STACK_SIZE)
        .build()
        .expect("a tokio runtime");
    match rt.block_on(run(cli.command)) {
        Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
        Err(e) => {
            eprintln!("{}", json!({ "error": format!("{e:#}") }));
            std::process::exit(1);
        }
    }
}

async fn run(command: Command) -> Result<serde_json::Value> {
    Ok(match command {
        Command::Skill => unreachable!("handled in main"),
        Command::Session { target, action } => {
            let client = target.client().await?;
            let args = match action {
                SessionAction::Start { name } => json!({ "action": "start", "name": name }),
                SessionAction::Handoff { reason } => json!({ "action": "handoff", "reason": reason }),
                SessionAction::Resume => json!({ "action": "resume" }),
                SessionAction::Finish { force } => json!({ "action": "finish", "force": force }),
                SessionAction::Status => json!({ "action": "status" }),
            };
            let started = args["action"] == "start";
            let mut v = client.request("session", args).await?;
            if started && let Some(id) = v["id"].as_str() {
                v = json!({
                    "session": v.clone(),
                    "next": format!("pass --session {id} with every command of this goal (or export DEVICESTEER_SESSION={id}), and run `devicesteer session finish --session {id}` when the goal is done"),
                });
            }
            v
        }
        Command::Devices => {
            let devices = list_devices().await?;
            let hint = devicesteer_core::missing_device_hint();
            if devices.is_empty() && !hint.is_empty() {
                json!({ "devices": devices, "hint": format!("usbmuxd lists no device{hint}") })
            } else {
                json!({ "devices": devices })
            }
        }
        Command::Setup { target, action, wait, ddi } => {
            use devicesteer_core::setup;
            let udid = target.udid.as_deref();
            let wait_for = Duration::from_secs(wait);
            // Progress goes to stderr, one JSON object per line, so stdout stays one result.
            let progress: setup::ProgressFn = std::sync::Arc::new(|p: setup::Progress| {
                eprintln!("{}", json!({ "progress": p }));
            });
            let status = match action {
                Some(SetupAction::Status) => setup::status(udid).await?,
                Some(SetupAction::Trust) => setup::trust(udid, wait_for, progress).await?,
                Some(SetupAction::DeveloperMode) => {
                    setup::developer_mode(udid, (wait > 0).then_some(wait_for), progress).await?
                }
                Some(SetupAction::DiskImage) => setup::disk_image(udid, ddi.as_deref(), progress).await?,
                Some(SetupAction::Wifi) => setup::pair_wifi(wait_for, progress).await?,
                Some(SetupAction::Forget) => {
                    let u = udid.context("pass --udid with the device whose Wi-Fi pairing to forget")?;
                    return Ok(json!({ "udid": u, "forgot": setup::forget_wifi(u)? }));
                }
                None => setup::run(udid, wait_for, ddi.as_deref(), progress).await?,
            };
            serde_json::to_value(status)?
        }
        Command::Services { target } => {
            let session = Session::open(target.udid.as_deref()).await?;
            json!({
                "udid": session.udid,
                "transport": session.transport,
                "services": session.services(),
            })
        }
        Command::Screenshot { target, output, display } => {
            let mut session = Session::open(target.udid.as_deref()).await?;
            let png = session.screenshot(display.as_deref()).await?;
            std::fs::write(&output, &png)
                .with_context(|| format!("could not write {}", output.display()))?;
            let (width, height) = png_size(&png).unzip();
            json!({
                "udid": session.udid,
                "transport": session.transport,
                "path": output,
                "bytes": png.len(),
                "width": width,
                "height": height,
            })
        }
        Command::Display { target } => {
            let mut session = Session::open(target.udid.as_deref()).await?;
            let d = session.display().await?;
            json!({ "udid": session.udid, "display": d, "screen": d.size(), "orientation": d.orientation() })
        }
        Command::Daemon { target, action } => match action {
            Some(DaemonAction::Status) => target.client().await?.request("status", json!({})).await?,
            Some(DaemonAction::Stop) => target.client().await?.request("shutdown", json!({})).await?,
            None => unreachable!("handled in main"),
        },
        Command::Events { target, frames, seconds } => {
            let mut client = target.client().await?;
            let mut events = client.take_events().context("events already taken")?;
            let began = std::time::Instant::now();
            let sub = client.request("subscribe", json!({ "frames": frames })).await?;
            println!("{}", json!({ "subscribed": sub, "ms": began.elapsed().as_millis() as u64 }));
            let deadline = seconds.map(|s| began + Duration::from_secs_f64(s));
            let (mut n, mut bytes) = (0u64, 0u64);
            loop {
                let left = deadline.map(|d| d.saturating_duration_since(std::time::Instant::now()));
                let next = match left {
                    Some(l) => match tokio::time::timeout(l, events.recv()).await {
                        Ok(m) => m,
                        Err(_) => break,
                    },
                    None => events.recv().await,
                };
                let Some(m) = next else { break };
                let at = began.elapsed().as_millis() as u64;
                match m {
                    devicesteer_daemon::wire::Message::Json(mut v) => {
                        v["t_ms"] = json!(at);
                        if let Some(d) = v.get_mut("description") {
                            *d = json!("<hvcC>");
                        }
                        println!("{v}");
                    }
                    devicesteer_daemon::wire::Message::Frame { mut header, data } => {
                        n += 1;
                        bytes += data.len() as u64;
                        header["bytes"] = json!(data.len());
                        header["t_ms"] = json!(at);
                        println!("{header}");
                    }
                }
            }
            json!({ "frames": n, "bytes": bytes, "seconds": began.elapsed().as_secs_f64() })
        }
        Command::Ui { target, all, full } if !target.no_daemon => {
            target.client().await?.request("ui", json!({ "all": all, "full": full })).await?
        }
        Command::Tap { target, at, label } if !target.no_daemon => {
            let mut args = match parse_point(&at) {
                Ok(p) => json!({ "point": [p.0, p.1] }),
                Err(_) => json!({ "target": at }),
            };
            args["label"] = json!(label.label);
            target.client().await?.request("tap", args).await?
        }
        Command::Swipe { target, from, to, duration, place, label } if !target.no_daemon => {
            let (from, to) = (parse_point(&from)?, parse_point(&to)?);
            let args = json!({ "from": [from.0, from.1], "to": [to.0, to.1], "duration_ms": duration, "place": place, "label": label.label });
            target.client().await?.request("swipe", args).await?
        }
        Command::Button { target, name, label } if !target.no_daemon => {
            target.client().await?.request("button", json!({ "name": name, "label": label.label })).await?
        }
        Command::Type { target, text, label, enter, force, method } => {
            let args = json!({ "text": text, "enter": enter, "force": force, "method": method, "label": label.label });
            target.client().await?.request("type", args).await?
        }
        Command::Paste { target, text, file, restore, force, label } => {
            let args = if restore {
                json!({ "restore": true })
            } else {
                let text = match file {
                    Some(f) => std::fs::read_to_string(&f).with_context(|| format!("could not read {}", f.display()))?,
                    None => text.unwrap_or_default(),
                };
                json!({ "text": text, "force": force, "label": label.label })
            };
            target.client().await?.request("paste", args).await?
        }
        Command::Clipboard { target, action: ClipboardAction::Set { text, file } } => {
            let text = match file {
                Some(f) => std::fs::read_to_string(&f).with_context(|| format!("could not read {}", f.display()))?,
                None => text.unwrap_or_default(),
            };
            target.client().await?.request("clipboard_set", json!({ "text": text })).await?
        }
        Command::Clipboard { target, action: ClipboardAction::Get { out } } => {
            let mut r = target.client().await?.request("clipboard_get", json!({})).await?;
            // The contents go to the file, never to stdout.
            if let Some(text) = r.as_object_mut().and_then(|o| o.remove("text")) {
                let text = text.as_str().unwrap_or_default();
                std::fs::write(&out, text).with_context(|| format!("could not write {}", out.display()))?;
                r["out"] = json!(out);
            }
            r
        }
        Command::Keyboard { target, action } => {
            target.client().await?.request(&format!("keyboard_{action}"), json!({})).await?
        }
        Command::Key { target, name, label } => {
            target.client().await?.request("key", json!({ "name": name, "label": label.label })).await?
        }
        Command::Launch { target, bundle_id, relaunch } if !target.no_daemon => {
            let args = json!({ "bundle_id": bundle_id, "relaunch": relaunch });
            target.client().await?.request("launch", args).await?
        }
        Command::Record { target, action: RecordAction::Start { output } } if !target.no_daemon => {
            let path = std::path::absolute(&output)?;
            target.client().await?.request("record_start", json!({ "path": path })).await?
        }
        Command::Record { target, action: RecordAction::Stop } if !target.no_daemon => {
            target.client().await?.request("record_stop", json!({})).await?
        }
        Command::Settings { target, action } if !target.no_daemon => match action {
            None => target.client().await?.request("settings", json!({})).await?,
            Some(SettingsAction::Set { name, value }) => {
                Setting::parse(&name, &value)?;
                target.client().await?.request("settings_set", json!({ "name": name, "value": value })).await?
            }
        },
        Command::Settings { target, action } => {
            let mut session = Session::open(target.udid.as_deref()).await?;
            let set = match action {
                Some(SettingsAction::Set { name, value }) => {
                    let setting = Setting::parse(&name, &value)?;
                    session.apply_setting(&setting).await?;
                    Some(setting.name())
                }
                None => None,
            };
            // The same shapes as through the daemon, which also knows the location it set.
            let settings = session.settings().await?;
            match set {
                Some(set) => json!({ "set": set, "settings": settings }),
                None => serde_json::to_value(settings)?,
            }
        }
        Command::Ui { target, all, full } => {
            with_controller(&target, async |c| {
                let began = std::time::Instant::now();
                let screen = c.ui(BuildOptions { all }, full).await?;
                save_refs(&c.session.udid, &screen);
                let orientation = c.display().await?.orientation();
                let mut v = serde_json::to_value(&screen)?;
                v["orientation"] = json!(orientation);
                v["ms"] = json!(began.elapsed().as_millis() as u64);
                Ok(v)
            })
            .await?
        }
        Command::Tap { target, at, .. } => {
            with_controller(&target, async |c| {
                let (point, element) = if let Ok(p) = parse_point(&at) {
                    (p, json!(null))
                } else if at.starts_with('@') {
                    let saved = load_refs(&c.session.udid)?;
                    let e = saved.find(&at).map_err(anyhow::Error::msg)?;
                    let point = saved.tap_point(e).context("that element has no frame in screen points (widgets have none); tap the point a screenshot shows it at")?;
                    (point, json!({ "ref": e.reference, "role": e.role, "frame": e.frame }))
                } else {
                    let screen = c.ui(BuildOptions::default(), false).await?;
                    save_refs(&c.session.udid, &screen);
                    let e = screen.find(&at).map_err(anyhow::Error::msg)?;
                    let point = screen.tap_point(e).context("that element has no frame in screen points (widgets have none); tap the point a screenshot shows it at")?;
                    (point, serde_json::to_value(e)?)
                };
                let sent = c.tap(point, || {}).await?;
                Ok(json!({ "tapped": [point.0, point.1], "element": element, "screen": sent.screen, "ms": sent.ms }))
            })
            .await?
        }
        Command::Swipe { target, from, to, duration, place, .. } => {
            let (from, to) = (parse_point(&from)?, parse_point(&to)?);
            let motion = if place { Motion::Place } else { Motion::Flick };
            with_controller(&target, async |c| {
                let sent = c.drag(from, to, Duration::from_millis(duration), motion).await?;
                Ok(json!({ "from": [from.0, from.1], "to": [to.0, to.1], "screen": sent.screen, "ms": sent.ms }))
            })
            .await?
        }
        Command::Launch { target, bundle_id, relaunch } => {
            let mut session = Session::open(target.udid.as_deref()).await?;
            let pid = session.launch(&bundle_id, relaunch).await?;
            json!({ "udid": session.udid, "launched": bundle_id, "pid": pid })
        }
        Command::Button { target, name, .. } => {
            let button = Button::parse(&name)?;
            with_controller(&target, async |c| {
                c.button(button).await?;
                Ok(json!({ "pressed": button }))
            })
            .await?
        }
        Command::Record { target, action } => match action {
            RecordAction::Run { output, seconds } => record::run(target.udid.as_deref(), &output, seconds).await?,
            RecordAction::Start { output } => record::start(target.udid.as_deref(), &output).await?,
            RecordAction::Stop => record::stop(target.udid.as_deref()).await?,
        },
        Command::Media { target, action } => match action {
            MediaAction::Status => {
                let session = Session::open(target.udid.as_deref()).await?;
                let status = devicesteer_core::server_status(session.adapter(), session.rsd()).await?;
                json!({ "udid": session.udid, "status": plist_to_json(&status) })
            }
            MediaAction::Probe { seconds, video } => {
                let mode = if video { MediaMode::Video } else { MediaMode::InputOnly };
                let session = Session::open(target.udid.as_deref()).await?;
                let mut c = Controller::new(session, mode);
                let began = std::time::Instant::now();
                let r = async {
                    c.ensure_media().await?;
                    let started_ms = began.elapsed().as_millis() as u64;
                    tokio::time::sleep(Duration::from_secs(seconds)).await;
                    let status = c.media_status().await?;
                    Ok::<_, anyhow::Error>(json!({ "started_ms": started_ms, "media": status }))
                }
                .await;
                let closed = c.close().await;
                let mut v = r?;
                closed?;
                v["stopped"] = json!(true);
                v
            }
        },
    })
}

/// The agent guide, with the version it belongs to.
fn skill_guide() -> String {
    let v = env!("CARGO_PKG_VERSION");
    format!(
        "[devicesteer:skill] DeviceSteer usage guide v{v}\n\n{}\n[devicesteer:skill] end of guide v{v}\n",
        include_str!("skill.md").trim_end()
    )
}

/// Opens a controller for one command, runs `f`, and always closes it (stopping only our streams).
async fn with_controller(
    target: &Target,
    f: impl AsyncFnOnce(&mut Controller) -> Result<serde_json::Value>,
) -> Result<serde_json::Value> {
    let session = Session::open(target.udid.as_deref()).await?;
    let udid = session.udid.clone();
    let mut c = Controller::new(session, MediaMode::InputOnly);
    let r = f(&mut c).await;
    let closed = c.close().await;
    let mut v = r?;
    closed?;
    v["udid"] = json!(udid);
    Ok(v)
}

/// Where the last `ui` reading's refs are kept between CLI runs. Only refs, roles and frames:
/// labels and values can be personal data and stay out of files.
fn refs_path(udid: &str) -> PathBuf {
    devicesteer_core::paths::state_dir().join(format!("{udid}.refs.json"))
}

fn save_refs(udid: &str, screen: &Screen) {
    let mut slim = screen.clone();
    slim.apps.clear();
    for e in &mut slim.elements {
        e.label = None;
        e.value = None;
        e.identifier = None;
    }
    if let Ok(bytes) = serde_json::to_vec(&slim) {
        let _ = std::fs::write(refs_path(udid), bytes);
    }
}

fn load_refs(udid: &str) -> Result<Screen> {
    let bytes = std::fs::read(refs_path(udid)).context("no earlier `ui` reading to take refs from; run `ui` first")?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// `x,y` in points.
fn parse_point(s: &str) -> Result<(f64, f64)> {
    let (x, y) = s
        .split_once(',')
        .with_context(|| format!("`{s}` is not a point; write it as x,y in screen points, such as 200,400"))?;
    let num = |v: &str| v.trim().parse::<f64>().with_context(|| format!("`{v}` in `{s}` is not a number"));
    Ok((num(x)?, num(y)?))
}

/// A plist value as JSON (data as its length, dates as strings).
fn plist_to_json(v: &plist::Value) -> serde_json::Value {
    use plist::Value as P;
    match v {
        P::Array(a) => a.iter().map(plist_to_json).collect(),
        P::Dictionary(d) => d.iter().map(|(k, v)| (k.clone(), plist_to_json(v))).collect::<serde_json::Map<_, _>>().into(),
        P::Boolean(b) => json!(b),
        P::Data(d) => json!(format!("<{} bytes>", d.len())),
        P::Date(d) => json!(format!("{d:?}")),
        P::Real(r) => json!(r),
        P::Integer(i) => i.as_signed().map(|n| json!(n)).or_else(|| i.as_unsigned().map(|n| json!(n))).unwrap_or(json!(null)),
        P::String(s) => json!(s),
        P::Uid(u) => json!(u.get()),
        _ => json!(null),
    }
}

/// Width and height from a PNG's IHDR chunk.
fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" || &png[12..16] != b"IHDR" {
        return None;
    }
    let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    Some((be(&png[16..20]), be(&png[20..24])))
}
