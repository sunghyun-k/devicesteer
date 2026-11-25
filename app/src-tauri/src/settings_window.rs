//! The Settings window: what belongs to the app rather than to a device (the command-line tool,
//! agent skill links, updates), in its own window as Mac apps have it, opened from the app menu
//! (⌘,) or by the page. Its page is app/settings.html; its permissions are
//! capabilities/settings.json, which lets it call only the commands it needs.

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

const LABEL: &str = "settings";

/// The panes the window has (app/src/settings.ts).
const TABS: [&str; 2] = ["agents", "updates"];

/// Opens the Settings window, or brings it to the front if it is open, showing `tab` when given
/// (otherwise the pane last shown).
pub fn open(app: &AppHandle, tab: Option<&str>) -> tauri::Result<()> {
    let tab = tab.filter(|t| TABS.contains(t));
    if let Some(w) = app.get_webview_window(LABEL) {
        if let Some(t) = tab {
            w.emit_to(LABEL, "settings-tab", t)?;
        }
        w.unminimize()?;
        w.show()?;
        return w.set_focus();
    }
    let page = match tab {
        Some(t) => format!("settings.html?tab={t}"),
        None => "settings.html".into(),
    };
    // In the viewer's language (`view_menu::Labels`); the page sets the pane's name as the title
    // once it loads, as Mac settings windows do.
    let title = app.state::<crate::view_menu::State>().labels.lock().unwrap().settings_window.clone();
    let builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(page.into()))
        .title(title)
        .inner_size(560.0, 600.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false);
    // The tabs sit in a toolbar under the title, drawn by the page (titleBarStyle Overlay keeps the
    // title and the traffic lights over it); other platforms keep their usual frame.
    #[cfg(target_os = "macos")]
    let builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay);
    builder.build()?;
    Ok(())
}

/// Opens Settings at `tab` (`agents` or `updates`): the main window's page calls it after the
/// first-launch sheet, and on platforms without an app menu. `async` with nothing to await: a
/// synchronous command runs on the main thread, and a window built there stays blank on Windows
/// (PITFALLS #64).
#[tauri::command]
pub async fn open_settings(app: AppHandle, tab: Option<String>) -> Result<(), String> {
    open(&app, tab.as_deref()).map_err(|e| format!("could not open the Settings window: {e}"))
}

/// Brings the main window forward and asks its page to offer the move to Applications. The
/// Settings window's Agents pane calls it: the move sheet belongs to the main window, which
/// offers it at launch.
#[tauri::command]
pub fn show_move_sheet(app: AppHandle) -> Result<(), String> {
    let main = app.get_webview_window("main").ok_or("the main window is closed")?;
    let _ = main.unminimize();
    main.set_focus().map_err(|e| format!("{e}"))?;
    main.emit_to("main", "move-sheet", ()).map_err(|e| format!("{e}"))
}

/// Closes Settings with the main window: the app quits when its last window closes, and a lone
/// Settings window would keep it running with no way back to the devices.
pub fn on_main_destroyed(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.close();
    }
}
