//! Agent sessions on every device, not only the one attached: the daemons tell the viewer when a
//! session starts, changes owner or ends (`devicesteer_daemon::viewer`), and the page marks the
//! devices in the sidebar, shows a started session's device, or a notice when another device is
//! on screen. A session start also brings the window forward when Settings › General says so.
//!
//! The page may load after a message came (the app was launched for a session), so the shell keeps
//! the latest session per device and the page reads it at start (`known_sessions`).

use std::{collections::HashMap, sync::Mutex};

use devicesteer_core::preferences::Preferences;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager};

/// The latest session per device, from the daemons' messages and the page's probes.
#[derive(Default)]
pub struct Sessions(Mutex<HashMap<String, Value>>);

impl Sessions {
    fn set(&self, udid: &str, session: Option<Value>) {
        let mut m = self.0.lock().unwrap();
        match session {
            Some(s) => m.insert(udid.to_string(), s),
            None => m.remove(udid),
        };
    }
}

/// Listens for the daemons' messages. Started once, at launch. Another copy of the app already
/// listening keeps them; this one works without, as before.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let listener = match devicesteer_daemon::viewer::Listener::bind().await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("not listening for agent sessions: {e:#}");
                return;
            }
        };
        listener
            .serve(move |udid, session, started| {
                app.state::<Sessions>().set(&udid, session.clone());
                let _ = app.emit_to("main", "agent-session", json!({ "udid": udid, "session": session, "started": started }));
                if started && Preferences::load().open_viewer_on_session {
                    bring_forward(&app);
                }
            })
            .await;
    });
}

/// Shows the main window, restored from the Dock or taskbar if it was minimised, without taking
/// the keyboard from the app in front (usually the terminal the agent runs in): someone typing
/// there would otherwise type into the viewer.
///
/// macOS: the window is ordered to the front of the screen (`orderFrontRegardless`) without
/// activating the app. Windows: restored without activation (`SW_SHOWNOACTIVATE`) and its taskbar
/// button flashes; Windows doesn't let a background app put its window over the foreground one.
pub fn bring_forward(app: &AppHandle) {
    let Some(w) = app.get_webview_window("main") else { return };
    #[cfg(target_os = "macos")]
    {
        let w2 = w.clone();
        let _ = w.run_on_main_thread(move || {
            let Ok(ns) = w2.ns_window() else { return };
            // SAFETY: `ns` is the window's live NSWindow, used on the main thread. Neither
            // message activates the app.
            unsafe {
                let ns: &objc2::runtime::AnyObject = &*(ns as *const objc2::runtime::AnyObject);
                let minimized: bool = objc2::msg_send![ns, isMiniaturized];
                if minimized {
                    let _: () = objc2::msg_send![ns, deminiaturize: Option::<&objc2::runtime::AnyObject>::None];
                }
                let _: () = objc2::msg_send![ns, orderFrontRegardless];
            }
        });
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible, SW_SHOWNOACTIVATE, ShowWindow};
        if let Ok(hwnd) = w.hwnd() {
            let hwnd = hwnd.0 as windows_sys::Win32::Foundation::HWND;
            // SAFETY: the window's own handle, alive while `w` is.
            unsafe {
                if IsIconic(hwnd) != 0 || IsWindowVisible(hwnd) == 0 {
                    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                }
            }
        }
        let _ = w.request_user_attention(Some(tauri::UserAttentionType::Informational));
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = w.unminimize();
        let _ = w.show();
    }
}

/// The sessions the daemons told of since launch, by UDID.
#[tauri::command]
pub fn known_sessions(sessions: tauri::State<'_, Sessions>) -> HashMap<String, Value> {
    sessions.0.lock().unwrap().clone()
}

/// Asks the running daemons of `udids` for their session, once, for sessions that started before
/// the viewer did. Starts no daemon. Returns the devices that have one.
#[tauri::command]
pub async fn probe_sessions(udids: Vec<String>, sessions: tauri::State<'_, Sessions>) -> Result<HashMap<String, Value>, String> {
    let found = futures_join(udids).await;
    for (u, s) in &found {
        sessions.set(u, Some(s.clone()));
    }
    Ok(found)
}

async fn futures_join(udids: Vec<String>) -> HashMap<String, Value> {
    let tasks: Vec<_> = udids
        .into_iter()
        .map(|u| tauri::async_runtime::spawn(async move { devicesteer_daemon::viewer::probe(&u).await.map(|s| (u, s)) }))
        .collect();
    let mut out = HashMap::new();
    for t in tasks {
        if let Ok(Some((u, s))) = t.await {
            out.insert(u, s);
        }
    }
    out
}

/// Settings › General › "Open the viewer when an agent starts a session".
#[tauri::command]
pub fn open_on_session() -> bool {
    Preferences::load().open_viewer_on_session
}

#[tauri::command]
pub fn set_open_on_session(on: bool) -> Result<(), String> {
    let mut p = Preferences::load();
    p.open_viewer_on_session = on;
    p.save().map_err(|e| format!("{e:#}"))
}

/// Records where this app runs from, for daemons that launch it from a CLI outside the app (the
/// agent setup's link on Windows is a hard link; `devicesteer_daemon::viewer::app_candidates`).
/// Release builds only: a dev build's binary needs the Vite server.
pub fn record_app_path() {
    if cfg!(debug_assertions) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let path = if cfg!(target_os = "macos") {
        match exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")) {
            Some(app) => app.to_path_buf(),
            None => return,
        }
    } else {
        exe
    };
    // A copy on a disk image or Gatekeeper's translocated copy goes away.
    #[cfg(target_os = "macos")]
    if crate::app_location::temporary(&path).is_some() {
        return;
    }
    let _ = std::fs::write(devicesteer_daemon::viewer::app_path_file(), path.to_string_lossy().as_bytes());
}

/// The device the app was launched for (`--udid`, from a daemon whose session started), if any.
pub fn launch_udid() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == devicesteer_daemon::viewer::UDID_ARG {
            return args.next().filter(|u| !u.is_empty());
        }
    }
    None
}
