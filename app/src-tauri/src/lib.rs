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
}

/// Lists the devices usbmuxd sees. Errors come back to the page as strings.
#[tauri::command]
async fn list_devices() -> Result<Vec<DeviceInfo>, String> {
    iphone_use_core::list_devices().await.map_err(|e| format!("{e:#}"))
}

/// Attaches to `udid`'s daemon (starting it if needed) and streams its events and frames to
/// `channel`, each as one wire message (see `iphone_use_daemon::wire`), so the page parses one
/// format for both.
#[tauri::command]
async fn attach(udid: String, channel: Channel<InvokeResponseBody>, viewer: State<'_, Viewer>) -> Result<Value, String> {
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
        .request("subscribe", serde_json::json!({ "frames": true }))
        .await
        .map_err(|e| format!("{e:#}"))
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(Viewer::default())
        .invoke_handler(tauri::generate_handler![list_devices, attach, detach, send, report, initial_udid])
        .run(tauri::generate_context!())
        .expect("error while running the iphone-use app");
}
