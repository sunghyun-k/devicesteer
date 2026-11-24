//! `iphone-use`: command-line access to a connected iPhone or iPad.
//!
//! Output is JSON on stdout. Errors go to stderr as `{"error": "..."}` with exit code 1.

use std::{path::PathBuf, time::Duration};

mod record;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use iphone_use_core::{
    Button,
    ax::{BuildOptions, Screen}, Controller, MediaMode, Motion, Session, list_devices};
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
    /// Screen-sharing media streams on the device.
    Media {
        #[command(flatten)]
        target: Target,
        #[command(subcommand)]
        action: MediaAction,
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

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match run(cli.command).await {
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
                    let frame = e.frame.context("that element has no frame to tap")?;
                    (frame.center(), json!({ "ref": e.reference, "role": e.role, "frame": frame }))
                } else {
                    let screen = c.ui(BuildOptions::default(), false).await?;
                    save_refs(&c.session.udid, &screen);
                    let e = screen.find(&at).map_err(anyhow::Error::msg)?;
                    let frame = e.frame.context("that element has no frame to tap")?;
                    (frame.center(), serde_json::to_value(e)?)
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
