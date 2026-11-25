//! Where DeviceSteer keeps per-device state on the host.

use std::path::PathBuf;

/// The state directory: `~/Library/Application Support/devicesteer` on macOS,
/// `%LOCALAPPDATA%\devicesteer` on Windows, `$XDG_STATE_HOME/devicesteer` elsewhere.
pub fn state_dir() -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
    };
    let dir = base.unwrap_or_else(std::env::temp_dir).join("devicesteer");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// The file that names the screen-sharing client sessions whose streams may still run on `udid`.
pub(crate) fn media_marker(udid: &str) -> PathBuf {
    state_dir().join(format!("{udid}.media"))
}
