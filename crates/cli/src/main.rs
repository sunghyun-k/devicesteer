//! `iphone-use`: command-line access to a connected iPhone or iPad.
//!
//! Output is JSON on stdout. Errors go to stderr as `{"error": "..."}` with exit code 1.

use std::{path::PathBuf, time::Duration};

mod record;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use iphone_use_core::{
    Button, Controller, MediaMode, Motion, Session, Setting,
    ax::{BuildOptions, Screen},
    list_devices,
};
use iphone_use_daemon::{Client, Source};
use serde_json::json;

#[derive(Parser)]
#[command(name = "iphone-use", version, about = "Operate a real iPhone or iPad over USB or Wi-Fi")]
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
}

impl Target {
    async fn client(&self) -> Result<Client> {
        Client::connect(self.udid.as_deref(), Source::Agent).await
    }
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
        /// Panel to capture (a display `uniqueId`); defaults to the device's primary panel.
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
        /// `x,y`, `@eN`, or a label.
        at: String,
    },
    /// Drag one finger from `x,y` to `x,y` (points).
    Swipe {
        #[command(flatten)]
        target: Target,
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
    /// Launch an app by bundle id, such as com.apple.calculator.
    Launch {
        #[command(flatten)]
        target: Target,
        bundle_id: String,
        /// Kill the running instance first.
        #[arg(long)]
        relaunch: bool,
    },
    /// Press a hardware button: home, lock, volume_up, volume_down.
    Button {
        #[command(flatten)]
        target: Target,
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
    /// Device Hub's settings: appearance, text size, Liquid Glass, accessibility switches,
    /// VoiceOver and the simulated location. Without an action, prints the current values; items
    /// the device can't read are null and listed in `unsupported`.
    Settings {
        #[command(flatten)]
        target: Target,
        #[command(subcommand)]
        action: Option<SettingsAction>,
    },
    /// Screen-sharing media streams on the device.
    Media {
        #[command(flatten)]
        target: Target,
        #[command(subcommand)]
        action: MediaAction,
    },
}

#[derive(Subcommand)]
enum DaemonAction {
    /// The daemon's state: connection, media streams, control, recording, clients.
    Status,
    /// Stop the daemon (it stops its media streams first).
    Stop,
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

fn main() {
    let cli = Cli::parse();
    // The daemon runs on a runtime of its own (see `iphone_use_daemon::run_blocking`).
    if let Command::Daemon { target, action: None } = &cli.command {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_env("IPHONE_USE_LOG")
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .with_writer(std::io::stderr)
            .init();
        let r = tokio::runtime::Runtime::new()
            .map_err(anyhow::Error::from)
            .and_then(|rt| rt.block_on(iphone_use_core::resolve_udid(target.udid.as_deref())))
            .and_then(|udid| iphone_use_daemon::run_blocking(&udid));
        if let Err(e) = r {
            eprintln!("{}", json!({ "error": format!("{e:#}") }));
            std::process::exit(1);
        }
        return;
    }
    let rt = tokio::runtime::Runtime::new().expect("a tokio runtime");
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
        Command::Devices => json!({ "devices": list_devices().await? }),
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
            json!({ "udid": session.udid, "display": d, "screen": d.size() })
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
                    iphone_use_daemon::wire::Message::Json(mut v) => {
                        v["t_ms"] = json!(at);
                        if let Some(d) = v.get_mut("description") {
                            *d = json!("<hvcC>");
                        }
                        println!("{v}");
                    }
                    iphone_use_daemon::wire::Message::Frame { mut header, data } => {
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
        Command::Tap { target, at } if !target.no_daemon => {
            let args = match parse_point(&at) {
                Ok(p) => json!({ "point": [p.0, p.1] }),
                Err(_) => json!({ "target": at }),
            };
            target.client().await?.request("tap", args).await?
        }
        Command::Swipe { target, from, to, duration, place } if !target.no_daemon => {
            let (from, to) = (parse_point(&from)?, parse_point(&to)?);
            let args = json!({ "from": [from.0, from.1], "to": [to.0, to.1], "duration_ms": duration, "place": place });
            target.client().await?.request("swipe", args).await?
        }
        Command::Button { target, name } if !target.no_daemon => {
            target.client().await?.request("button", json!({ "name": name })).await?
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
                let mut v = serde_json::to_value(&screen)?;
                v["ms"] = json!(began.elapsed().as_millis() as u64);
                Ok(v)
            })
            .await?
        }
        Command::Tap { target, at } => {
            with_controller(&target, async |c| {
                let (point, element) = if let Ok(p) = parse_point(&at) {
                    (p, json!(null))
                } else if at.starts_with('@') {
                    let saved = load_refs(&c.session.udid)?;
                    let e = saved.find(&at).map_err(anyhow::Error::msg)?;
                    let point = saved.tap_point(e).context("that element has no frame to tap")?;
                    (point, json!({ "ref": e.reference, "role": e.role, "frame": e.frame }))
                } else {
                    let screen = c.ui(BuildOptions::default(), false).await?;
                    save_refs(&c.session.udid, &screen);
                    let e = screen.find(&at).map_err(anyhow::Error::msg)?;
                    let point = screen.tap_point(e).context("that element has no frame to tap")?;
                    (point, serde_json::to_value(e)?)
                };
                let sent = c.tap(point).await?;
                Ok(json!({ "tapped": [point.0, point.1], "element": element, "screen": sent.screen, "ms": sent.ms }))
            })
            .await?
        }
        Command::Swipe { target, from, to, duration, place } => {
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
        Command::Button { target, name } => {
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
                let status = iphone_use_core::server_status(session.adapter(), session.rsd()).await?;
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
    iphone_use_core::paths::state_dir().join(format!("{udid}.refs.json"))
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
