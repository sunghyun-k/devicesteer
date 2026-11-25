/// The app's own commands. Listing them makes each one a permission (`allow-<name>`) that a
/// window gets only through its capability (capabilities/*.json): the Settings window may call
/// the agent setup and update commands but not drive a device. A new command goes here, in
/// `generate_handler!` (src/lib.rs) and in the capability of each window that calls it.
const COMMANDS: &[&str] = &[
    "list_devices",
    "setup_status",
    "setup_run",
    "setup_pair_wifi",
    "setup_cancel",
    "attach",
    "watch",
    "detach",
    "send",
    "clipboard_to_device",
    "clipboard_from_device",
    "clipboard_sync",
    "report",
    "initial_udid",
    "recording_path",
    "save_screenshot",
    "reveal",
    "restore_input_source",
    "agent_setup_status",
    "agent_setup_change",
    "app_location",
    "app_move_to_applications",
    "app_version",
    "set_show_touches",
    "open_settings",
    "show_move_sheet",
];

fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)))
        .expect("failed to run the Tauri build script");
}
