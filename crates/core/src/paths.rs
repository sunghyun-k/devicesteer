//! Where DeviceSteer keeps per-device state on the host.

use std::path::{Path, PathBuf};

/// The state directory: `~/Library/Application Support/devicesteer` on macOS,
/// `%LOCALAPPDATA%\com.sunghyunkim.devicesteer` on Windows, `$XDG_STATE_HOME/devicesteer`
/// elsewhere.
///
/// Not `%LOCALAPPDATA%\devicesteer` on Windows, which builds up to 1.0.0-beta.2 used: that is
/// also where the installer puts the app (`%LOCALAPPDATA%\DeviceSteer`, the same folder on a
/// case-insensitive disk), so Wi-Fi pairing records and the downloaded disk image lived among the
/// installer's files, an uninstall left the folder behind, and its "Delete the application data"
/// box missed them. The app's identifier is the folder that box deletes (Tauri keeps WebView2's
/// data there too). [`migrate_legacy`] moves the old state over.
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
    let base = base.unwrap_or_else(std::env::temp_dir);
    let dir = base.join(if cfg!(windows) { WINDOWS_DIR } else { "devicesteer" });
    let _ = std::fs::create_dir_all(&dir);
    if cfg!(windows) {
        static MIGRATED: std::sync::Once = std::sync::Once::new();
        MIGRATED.call_once(|| migrate_legacy(&base.join("devicesteer"), &dir));
    }
    dir
}

/// The Windows state folder's name: the app's bundle identifier (`tauri.conf.json`).
const WINDOWS_DIR: &str = "com.sunghyunkim.devicesteer";

/// Moves what earlier builds kept in `old` (the install folder, by default) into `new`: pairing
/// records, the disk image cache, preferences, where the viewer runs from, the media markers that let a later run stop streams
/// a crashed one left running (PITFALLS #1), the agents' element refs and the logs. Only these, by
/// name: the folder also holds the installed app. Best effort and safe to repeat: an entry `new`
/// already has stays where it is, and one in use (a log an old daemon still writes) fails to move
/// and is tried again next time.
fn migrate_legacy(old: &Path, new: &Path) {
    let Ok(entries) = std::fs::read_dir(old) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let to = new.join(&name);
        if is_legacy_state(&name.to_string_lossy()) && !to.exists() {
            let _ = std::fs::rename(entry.path(), to);
        }
    }
}

fn is_legacy_state(name: &str) -> bool {
    matches!(name, "ddi" | "remote-pairing" | "preferences.json" | "app_path")
        || [".media", ".refs.json", ".log"].iter().any(|ext| name.ends_with(ext))
}

/// The file that names the screen-sharing client sessions whose streams may still run on `udid`.
pub(crate) fn media_marker(udid: &str) -> PathBuf {
    state_dir().join(format!("{udid}.media"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_only_the_state_from_the_install_folder() {
        let root = std::env::temp_dir().join(format!("devicesteer-migrate-{}", std::process::id()));
        let (old, new) = (root.join("old"), root.join("new"));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["remote-pairing", "ddi/abc", "skills/devicesteer"] {
            std::fs::create_dir_all(old.join(d)).unwrap();
        }
        for f in ["remote-pairing/X.plist", "preferences.json", "U.media", "U.daemon.log", "U.refs.json", "devicesteer.exe", "uninstall.exe", "U.record.json"] {
            std::fs::write(old.join(f), f).unwrap();
        }
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("preferences.json"), "newer").unwrap();

        migrate_legacy(&old, &new);

        assert!(new.join("remote-pairing/X.plist").exists());
        assert!(new.join("ddi/abc").is_dir());
        for f in ["U.media", "U.daemon.log", "U.refs.json"] {
            assert!(new.join(f).exists(), "{f} moved");
        }
        // What `new` already has wins; the installer's files and transient state stay.
        assert_eq!(std::fs::read_to_string(new.join("preferences.json")).unwrap(), "newer");
        for f in ["preferences.json", "devicesteer.exe", "uninstall.exe", "skills/devicesteer", "U.record.json"] {
            assert!(old.join(f).exists(), "{f} stayed");
        }
        migrate_legacy(&root.join("missing"), &new);
        let _ = std::fs::remove_dir_all(&root);
    }
}
