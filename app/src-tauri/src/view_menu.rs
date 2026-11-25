//! The app menu: Tauri's default menu, rebuilt with labels in the viewer's language, plus
//! "Settings…" (⌘,) in the DeviceSteer menu, which opens the Settings window (`settings_window`),
//! and the View menu's "Show Touches" item: whether the page draws the touch discs (the human's
//! and the agent's). The page owns that setting and keeps it in its window settings; the menu only
//! shows it and tells the page when it is chosen.
//!
//! The page owns the language too (app/src/i18n.ts: the catalogs and the person's choice in
//! Settings), so it sends the labels (`set_menu_labels`) at launch and when the language changes;
//! until then the menu is in English.
//!
//! Only macOS has an app menu: on other platforms the window has no menu bar and adding one would
//! give it one, so there Show Touches keeps its saved value and nothing changes it, and the page
//! opens Settings itself (Ctrl+, and its More menu).

use std::sync::Mutex;

use serde::Deserialize;
use tauri::menu::{AboutMetadata, CheckMenuItem, HELP_SUBMENU_ID, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu, WINDOW_SUBMENU_ID};
use tauri::{AppHandle, Emitter, Manager, Wry};

const SETTINGS: &str = "settings";
/// ⌘,: the macOS convention. The page's `viewerShortcut` (app/src/main.ts) keeps it from going to
/// the device, or the menu never sees it.
const SETTINGS_KEY: &str = "CmdOrCtrl+Comma";
const SHOW_TOUCHES: &str = "show-touches";
/// ⇧⌘T: free in the default menu and among the viewer's shortcuts (`viewerShortcut` in
/// app/src/main.ts, which must keep it from going to the device, or the menu never sees it).
const SHOW_TOUCHES_KEY: &str = "CmdOrCtrl+Shift+T";

/// The menu's labels, from the page's catalog (`menu.*` in app/src/locales/*.json), and the
/// Settings window's title. Missing ones stay English.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Labels {
    about: String,
    services: String,
    hide: String,
    hide_others: String,
    show_all: String,
    quit: String,
    file: String,
    close_window: String,
    edit: String,
    undo: String,
    redo: String,
    cut: String,
    copy: String,
    paste: String,
    select_all: String,
    view: String,
    show_touches: String,
    enter_full_screen: String,
    window: String,
    minimize: String,
    zoom: String,
    help: String,
    settings: String,
    pub settings_window: String,
}

impl Default for Labels {
    fn default() -> Self {
        let s = |t: &str| t.to_string();
        Self {
            about: s("About DeviceSteer"),
            services: s("Services"),
            hide: s("Hide DeviceSteer"),
            hide_others: s("Hide Others"),
            show_all: s("Show All"),
            quit: s("Quit DeviceSteer"),
            file: s("File"),
            close_window: s("Close Window"),
            edit: s("Edit"),
            undo: s("Undo"),
            redo: s("Redo"),
            cut: s("Cut"),
            copy: s("Copy"),
            paste: s("Paste"),
            select_all: s("Select All"),
            view: s("View"),
            show_touches: s("Show Touches"),
            enter_full_screen: s("Enter Full Screen"),
            window: s("Window"),
            minimize: s("Minimize"),
            zoom: s("Zoom"),
            help: s("Help"),
            settings: s("Settings…"),
            settings_window: s("Settings"),
        }
    }
}

/// The labels in use, and the Show Touches item, kept so the page can set its check.
#[derive(Default)]
pub struct State {
    pub labels: Mutex<Labels>,
    show_touches: Mutex<Option<CheckMenuItem<Wry>>>,
}

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    app.manage(State::default());
    build(app, &Labels::default(), true)
}

/// Sets the app menu: Tauri's default one (`Menu::default`), in `l`'s words, with "Settings…" in
/// the app menu and "Show Touches" (checked when `touches`) at the top of View.
fn build(app: &AppHandle, l: &Labels, touches: bool) -> tauri::Result<()> {
    if !cfg!(target_os = "macos") {
        return Ok(());
    }
    let pkg = app.package_info();
    let config = app.config();
    let about = AboutMetadata {
        name: Some(pkg.name.clone()),
        version: Some(pkg.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };
    let sep = || PredefinedMenuItem::separator(app);
    let show_touches = CheckMenuItem::with_id(app, SHOW_TOUCHES, &l.show_touches, true, touches, Some(SHOW_TOUCHES_KEY))?;
    let settings = MenuItem::with_id(app, SETTINGS, &l.settings, true, Some(SETTINGS_KEY))?;
    let menu = Menu::with_items(
        app,
        &[
            // Settings goes in its own section after About, where every Mac app has it.
            &Submenu::with_items(
                app,
                &pkg.name,
                true,
                &[
                    &PredefinedMenuItem::about(app, Some(&l.about), Some(about))?,
                    &sep()?,
                    &settings,
                    &sep()?,
                    &PredefinedMenuItem::services(app, Some(&l.services))?,
                    &sep()?,
                    &PredefinedMenuItem::hide(app, Some(&l.hide))?,
                    &PredefinedMenuItem::hide_others(app, Some(&l.hide_others))?,
                    &PredefinedMenuItem::show_all(app, Some(&l.show_all))?,
                    &sep()?,
                    &PredefinedMenuItem::quit(app, Some(&l.quit))?,
                ],
            )?,
            &Submenu::with_items(app, &l.file, true, &[&PredefinedMenuItem::close_window(app, Some(&l.close_window))?])?,
            &Submenu::with_items(
                app,
                &l.edit,
                true,
                &[
                    &PredefinedMenuItem::undo(app, Some(&l.undo))?,
                    &PredefinedMenuItem::redo(app, Some(&l.redo))?,
                    &sep()?,
                    &PredefinedMenuItem::cut(app, Some(&l.cut))?,
                    &PredefinedMenuItem::copy(app, Some(&l.copy))?,
                    &PredefinedMenuItem::paste(app, Some(&l.paste))?,
                    &PredefinedMenuItem::select_all(app, Some(&l.select_all))?,
                ],
            )?,
            &Submenu::with_items(
                app,
                &l.view,
                true,
                &[&show_touches, &sep()?, &PredefinedMenuItem::fullscreen(app, Some(&l.enter_full_screen))?],
            )?,
            // The ids make AppKit treat these as the Window and Help menus (the window list, the
            // help search field).
            &Submenu::with_id_and_items(
                app,
                WINDOW_SUBMENU_ID,
                &l.window,
                true,
                &[
                    &PredefinedMenuItem::minimize(app, Some(&l.minimize))?,
                    &PredefinedMenuItem::maximize(app, Some(&l.zoom))?,
                    &sep()?,
                    &PredefinedMenuItem::close_window(app, Some(&l.close_window))?,
                ],
            )?,
            &Submenu::with_id_and_items(app, HELP_SUBMENU_ID, &l.help, true, &[])?,
        ],
    )?;
    app.set_menu(menu)?;
    *app.state::<State>().show_touches.lock().unwrap() = Some(show_touches);
    Ok(())
}

/// Opens Settings, or passes a choice of "Show Touches" to the page as `show-touches` with the new
/// state. The menu has flipped the check already.
pub fn on_menu_event(app: &AppHandle, event: &MenuEvent) {
    if event.id() == SETTINGS {
        if let Err(e) = crate::settings_window::open(app, None) {
            eprintln!("could not open the Settings window: {e}");
        }
        return;
    }
    if event.id() != SHOW_TOUCHES {
        return;
    }
    if let Some(on) = touches_checked(app) {
        let _ = app.emit("show-touches", on);
    }
}

fn touches_checked(app: &AppHandle) -> Option<bool> {
    app.state::<State>().show_touches.lock().unwrap().as_ref().and_then(|i| i.is_checked().ok())
}

/// Sets the check of "Show Touches" to the page's saved setting. Does nothing without the menu.
#[tauri::command]
pub fn set_show_touches(on: bool, app: AppHandle) -> Result<(), String> {
    match app.state::<State>().show_touches.lock().unwrap().as_ref() {
        Some(item) => item.set_checked(on).map_err(|e| format!("{e}")),
        None => Ok(()),
    }
}

/// Rebuilds the menu with the page's labels, in the language it shows, keeping Show Touches'
/// check. The Settings window takes its title from them too.
#[tauri::command]
pub fn set_menu_labels(labels: Labels, app: AppHandle) -> Result<(), String> {
    let touches = touches_checked(&app).unwrap_or(true);
    *app.state::<State>().labels.lock().unwrap() = labels.clone();
    build(&app, &labels, touches).map_err(|e| format!("could not set the menu: {e}"))
}
