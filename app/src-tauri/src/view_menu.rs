//! The View menu's "Show Touches" item: whether the page draws the touch discs (the human's and
//! the agent's). The page owns the setting and keeps it in its window settings; the menu only
//! shows it and tells the page when it is chosen.
//!
//! Only macOS has an app menu (Tauri's default menu, with a View submenu): on other platforms the
//! window has no menu bar and adding one would give it one, so there the setting keeps its saved
//! value and nothing changes it.

use tauri::menu::{CheckMenuItem, Menu, MenuEvent, MenuItemKind, PredefinedMenuItem};
use tauri::{AppHandle, Emitter, Manager, Wry};

const SHOW_TOUCHES: &str = "show-touches";
/// ⇧⌘T: free in the default menu and among the viewer's shortcuts (`viewerShortcut` in
/// app/src/main.ts, which must keep it from going to the device, or the menu never sees it).
const SHOW_TOUCHES_KEY: &str = "CmdOrCtrl+Shift+T";

/// The menu item, kept so the page can set its check at launch.
struct ShowTouches(CheckMenuItem<Wry>);

/// Replaces the app menu with Tauri's default one plus "Show Touches" at the top of View.
pub fn install(app: &AppHandle) -> tauri::Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    let menu = Menu::default(app)?;
    let item = CheckMenuItem::with_id(app, SHOW_TOUCHES, "Show Touches", true, true, Some(SHOW_TOUCHES_KEY))?;
    for kind in menu.items()? {
        if let MenuItemKind::Submenu(view) = kind
            && view.text()? == "View"
        {
            view.prepend_items(&[&item, &PredefinedMenuItem::separator(app)?])?;
        }
    }
    app.set_menu(menu)?;
    app.manage(ShowTouches(item));
    Ok(())
}

/// Passes a choice of "Show Touches" to the page as `show-touches` with the new state. The menu
/// has flipped the check already.
pub fn on_menu_event(app: &AppHandle, event: &MenuEvent) {
    if event.id() != SHOW_TOUCHES {
        return;
    }
    if let Some(item) = app.try_state::<ShowTouches>()
        && let Ok(on) = item.0.is_checked()
    {
        let _ = app.emit("show-touches", on);
    }
}

/// Sets the check of "Show Touches" to the page's saved setting. Does nothing without the menu.
#[tauri::command]
pub fn set_show_touches(on: bool, app: AppHandle) -> Result<(), String> {
    match app.try_state::<ShowTouches>() {
        Some(item) => item.0.set_checked(on).map_err(|e| format!("{e}")),
        None => Ok(()),
    }
}
