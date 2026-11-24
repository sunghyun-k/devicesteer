//! Tauri shell for the viewer: lists devices, attaches to a device's daemon as the human client,
//! and passes its events and video frames to the page.

use std::sync::Arc;

use iphone_use_core::DeviceInfo;
use iphone_use_daemon::{Client, Source};
use serde_json::Value;
use tauri::{
    State,
    ipc::{Channel, InvokeResponseBody},
};
use tokio::sync::Mutex;

/// The daemon connection of the device on screen.
#[derive(Default)]
struct Viewer {
    client: Mutex<Option<Arc<Client>>>,
    forwarder: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The setup step running now (pairing waits minutes for a tap), so Cancel can end it.
    setup: Mutex<Option<tokio::task::AbortHandle>>,
}

/// Lists the devices usbmuxd sees. Errors come back to the page as strings.
#[tauri::command]
async fn list_devices() -> Result<Vec<DeviceInfo>, String> {
    iphone_use_core::list_devices().await.map_err(|e| format!("{e:#}"))
}

/// Which setup steps `udid` still needs (see `iphone_use_core::setup`). Reads only.
#[tauri::command]
async fn setup_status(udid: String) -> Result<iphone_use_core::SetupStatus, String> {
    iphone_use_core::setup::status(Some(&udid)).await.map_err(|e| format!("{e:#}"))
}

/// How long a step waits for the person holding the device (tap Trust; turn on Developer Mode,
/// restart and confirm). Long: the card stays up and Cancel ends it sooner.
const SETUP_WAIT: std::time::Duration = std::time::Duration::from_secs(600);

/// Runs one setup step (`trust`, `developer_mode`, `disk_image`) on `udid`, sending its progress
/// to `channel`, and returns the state afterwards. A step started earlier is cancelled first.
#[tauri::command]
async fn setup_run(
    udid: String,
    step: String,
    channel: Channel<iphone_use_core::setup::Progress>,
    viewer: State<'_, Viewer>,
) -> Result<iphone_use_core::SetupStatus, String> {
    use iphone_use_core::setup::{self, SetupStep};
    let step = SetupStep::parse(&step).map_err(|e| format!("{e:#}"))?;
    let progress: setup::ProgressFn = Arc::new(move |p| {
        let _ = channel.send(p);
    });
    let task = tokio::spawn(async move {
        let u = Some(udid.as_str());
        match step {
            SetupStep::Trust => setup::trust(u, SETUP_WAIT, progress).await,
            SetupStep::DeveloperMode => setup::developer_mode(u, Some(SETUP_WAIT), progress).await,
            SetupStep::DiskImage => setup::disk_image(u, None, progress).await,
        }
    });
    if let Some(old) = viewer.setup.lock().await.replace(task.abort_handle()) {
        old.abort();
    }
    match task.await {
        Ok(r) => r.map_err(|e| format!("{e:#}")),
        Err(e) if e.is_cancelled() => Err("cancelled".into()),
        Err(e) => Err(format!("the setup step failed: {e}")),
    }
}

/// Ends the running setup step, if any. Pairing that the device already accepted stays.
#[tauri::command]
async fn setup_cancel(viewer: State<'_, Viewer>) -> Result<(), String> {
    if let Some(task) = viewer.setup.lock().await.take() {
        task.abort();
    }
    Ok(())
}

/// Attaches to `udid`'s daemon (starting it if needed) and streams its events, and its video
/// frames when `frames`, to `channel`, each as one wire message (see `iphone_use_daemon::wire`),
/// so the page parses one format for both. A selected device is attached without frames: its
/// settings and agent session show before anyone asks for the screen.
#[tauri::command]
async fn attach(
    udid: String,
    frames: bool,
    channel: Channel<InvokeResponseBody>,
    viewer: State<'_, Viewer>,
) -> Result<Value, String> {
    detach_inner(&viewer).await;
    let mut client = Client::connect(Some(&udid), Source::Human).await.map_err(|e| format!("{e:#}"))?;
    let mut events = client.take_events().ok_or("events already taken")?;
    let client = Arc::new(client);
    let forwarder = tokio::spawn(async move {
        while let Some(m) = events.recv().await {
            if channel.send(InvokeResponseBody::Raw(m.encode())).is_err() {
                break;
            }
        }
    });
    *viewer.forwarder.lock().await = Some(forwarder);
    *viewer.client.lock().await = Some(client.clone());
    client
        .request("subscribe", serde_json::json!({ "frames": frames }))
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Starts or stops the video frames of the attached device, keeping its events.
#[tauri::command]
async fn watch(frames: bool, viewer: State<'_, Viewer>) -> Result<Value, String> {
    let client = viewer.client.lock().await.clone().ok_or("not attached to a device")?;
    client.request("subscribe", serde_json::json!({ "frames": frames })).await.map_err(|e| format!("{e:#}"))
}

async fn detach_inner(viewer: &Viewer) {
    if let Some(f) = viewer.forwarder.lock().await.take() {
        f.abort();
    }
    viewer.client.lock().await.take();
}

#[tauri::command]
async fn detach(viewer: State<'_, Viewer>) -> Result<(), String> {
    detach_inner(&viewer).await;
    Ok(())
}

/// Sends one command to the daemon as the human: `tap`, `swipe`, `button`, `control`, ...
#[tauri::command]
async fn send(cmd: String, args: Value, viewer: State<'_, Viewer>) -> Result<Value, String> {
    let client = viewer.client.lock().await.clone().ok_or("not attached to a device")?;
    client.request(&cmd, args).await.map_err(|e| format!("{e:#}"))
}

/// The page's decoder numbers, appended to `viewer.log` in the state directory, so a check of
/// the viewer doesn't need anyone to look at the window.
#[tauri::command]
fn report(stats: Value) {
    use std::io::Write;
    let path = iphone_use_core::paths::state_dir().join("viewer.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{stats}");
    }
}

/// A device to open right away (`IPHONE_USE_VIEWER_UDID`), for launching the viewer on one device
/// from a script.
#[tauri::command]
fn initial_udid() -> Option<String> {
    std::env::var("IPHONE_USE_VIEWER_UDID").ok().filter(|u| !u.is_empty())
}

/// Where a recording started from the viewer goes: the user's videos folder (Movies on macOS),
/// named by `stamp`, the page's local date and time.
#[tauri::command]
fn recording_path(app: tauri::AppHandle, stamp: String) -> Result<String, String> {
    use tauri::Manager;
    let dir = app
        .path()
        .video_dir()
        .or_else(|_| app.path().home_dir())
        .map_err(|e| format!("no folder for recordings: {e}"))?;
    let name: String = stamp.chars().filter(|c| c.is_ascii_alphanumeric() || " -.".contains(*c)).collect();
    Ok(dir.join(format!("iphone-use {name}.mp4")).to_string_lossy().into_owned())
}

/// Saves a PNG of the screen the page drew (the request body) to the user's pictures folder (or
/// home), named by the `x-stamp` header, the page's local date and time. Returns its path.
#[tauri::command]
fn save_screenshot(app: tauri::AppHandle, request: tauri::ipc::Request<'_>) -> Result<String, String> {
    use tauri::Manager;
    let tauri::ipc::InvokeBody::Raw(png) = request.body() else {
        return Err("save_screenshot takes the PNG as the request body".into());
    };
    let stamp = request.headers().get("x-stamp").and_then(|v| v.to_str().ok()).unwrap_or("screenshot");
    let name: String = stamp.chars().filter(|c| c.is_ascii_alphanumeric() || " -.".contains(*c)).collect();
    let dir = app
        .path()
        .picture_dir()
        .or_else(|_| app.path().home_dir())
        .map_err(|e| format!("no folder for screenshots: {e}"))?;
    let path = dir.join(format!("iphone-use {name}.png"));
    std::fs::write(&path, png).map_err(|e| format!("could not save {}: {e}", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}

/// Shows a file in Finder (or Explorer), selected.
#[tauri::command]
fn reveal(path: String) -> Result<(), String> {
    let spawned = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg("-R").arg(&path).spawn()
    } else if cfg!(windows) {
        std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn()
    } else {
        let dir = std::path::Path::new(&path).parent().map(|d| d.to_path_buf()).unwrap_or_default();
        std::process::Command::new("xdg-open").arg(dir).spawn()
    };
    spawned.map(|_| ()).map_err(|e| format!("could not show {path}: {e}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(Viewer::default())
        .invoke_handler(tauri::generate_handler![list_devices, setup_status, setup_run, setup_cancel, attach, watch, detach, send, report, initial_udid, recording_path, save_screenshot, reveal])
        .run(tauri::generate_context!())
        .expect("error while running the iphone-use app");
}
