//! Tauri shell for the viewer. For now it only lists devices.

use iphone_use_core::DeviceInfo;

/// Lists the devices usbmuxd sees. Errors come back to the page as strings.
#[tauri::command]
async fn list_devices() -> Result<Vec<DeviceInfo>, String> {
    iphone_use_core::list_devices()
        .await
        .map_err(|e| format!("{e:#}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![list_devices])
        .run(tauri::generate_context!())
        .expect("error while running the iphone-use app");
}
