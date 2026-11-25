//! Moving the app to the Applications folder when it runs from somewhere that won't last: the disk
//! image it came in, or the random read-only copy Gatekeeper runs a quarantined app from (App
//! Translocation). Agent setup links into the app, so it refuses both (agent_setup.rs); this is
//! what the page offers instead.
//!
//! The move copies the running bundle with `ditto`, which keeps the code signature intact, and
//! swaps it into place in one rename. An app already there goes to the Trash, and only after the
//! person confirmed (the page shows its version). The copy is relaunched once this process exits.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// Why the app at a path won't last there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// Under `/Volumes`: a disk image, gone when ejected.
    DiskImage,
    /// A Gatekeeper copy under `.../AppTranslocation/`, gone when the app quits.
    Translocated,
}

/// Whether the app bundle (or anything inside it) at `path` runs from a place that goes away.
pub fn temporary(path: &Path) -> Option<Reason> {
    let s = path.to_string_lossy();
    if s.contains("/AppTranslocation/") {
        Some(Reason::Translocated)
    } else if s.starts_with("/Volumes/") {
        Some(Reason::DiskImage)
    } else {
        None
    }
}

/// The `.app` folder `exe` belongs to, if it runs from one.
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    exe.parent()
        .filter(|d| d.ends_with("Contents/MacOS"))
        .and_then(Path::parent)
        .and_then(Path::parent)
        .filter(|a| a.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
}

/// What the page needs to offer the move.
#[derive(Debug, Serialize)]
pub struct Location {
    /// Why the app should move, or `None` when it is fine where it is (or isn't a bundle).
    pub reason: Option<Reason>,
    /// Whether an app with this name is already in the Applications folder.
    pub exists: bool,
    /// That app's version, when it could be read.
    pub existing_version: Option<String>,
}

/// Why the move didn't happen. The page reads `kind`.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MoveError {
    /// An app is already there and the person hasn't confirmed replacing it.
    Exists { version: Option<String> },
    /// The Applications folder (or the app in it) isn't writable for this user.
    NoPermission { message: String },
    Failed { message: String },
}

impl MoveError {
    fn io(what: &str, e: std::io::Error) -> Self {
        let message = format!("{what}: {e}");
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            Self::NoPermission { message }
        } else {
            Self::Failed { message }
        }
    }
}

/// The folders a move touches; separate from the process so tests can use temporary ones.
pub struct Mover {
    applications: PathBuf,
    trash: PathBuf,
}

impl Mover {
    fn current() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        Self { applications: PathBuf::from("/Applications"), trash: home.join(".Trash") }
    }

    /// Where `bundle` goes: the Applications folder, under the same name.
    fn destination(&self, bundle: &Path) -> PathBuf {
        self.applications.join(bundle.file_name().unwrap_or(std::ffi::OsStr::new("DeviceSteer.app")))
    }

    fn location(&self, bundle: &Path) -> Location {
        let dest = self.destination(bundle);
        let exists = std::fs::symlink_metadata(&dest).is_ok();
        Location {
            reason: temporary(bundle),
            exists,
            existing_version: if exists { bundle_version(&dest) } else { None },
        }
    }

    /// Copies `bundle` into the Applications folder and returns where it went. An app already
    /// there is an error unless `replace`; then it goes to the Trash.
    fn move_in(&self, bundle: &Path, replace: bool) -> Result<PathBuf, MoveError> {
        let dest = self.destination(bundle);
        let exists = std::fs::symlink_metadata(&dest).is_ok();
        if exists && !replace {
            return Err(MoveError::Exists { version: bundle_version(&dest) });
        }
        // Copy next to the destination first, so a failed copy leaves nothing half done and the
        // swap is a rename on one volume. Creating the folder is also the permission check.
        let staging = self.applications.join(format!(".DeviceSteer-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir(&staging).map_err(|e| MoveError::io("could not write to the Applications folder", e))?;
        let result = self.copy_and_swap(bundle, &staging, &dest, exists);
        let _ = std::fs::remove_dir_all(&staging);
        result.map(|()| dest)
    }

    fn copy_and_swap(&self, bundle: &Path, staging: &Path, dest: &Path, exists: bool) -> Result<(), MoveError> {
        let copy = staging.join(dest.file_name().expect("destination has a name"));
        let out = std::process::Command::new("/usr/bin/ditto")
            .arg(bundle)
            .arg(&copy)
            .output()
            .map_err(|e| MoveError::Failed { message: format!("could not run ditto: {e}") })?;
        if !out.status.success() {
            let message = format!("could not copy the app: {}", String::from_utf8_lossy(&out.stderr).trim());
            return Err(if message.contains("Permission denied") {
                MoveError::NoPermission { message }
            } else {
                MoveError::Failed { message }
            });
        }
        // The copy carries the download's quarantine flag. Gatekeeper already let this app run;
        // left in place, the flag makes Gatekeeper translocate the copy too, because only a move
        // made in Finder clears its translocation bit, and the app would land right back here.
        let _ = std::process::Command::new("/usr/bin/xattr").arg("-dr").arg("com.apple.quarantine").arg(&copy).output();
        if exists {
            let to = trash_path(&self.trash, dest, bundle_version(dest).as_deref());
            std::fs::create_dir_all(&self.trash).map_err(|e| MoveError::io("could not create the Trash", e))?;
            std::fs::rename(dest, &to).map_err(|e| MoveError::io("could not move the old app to the Trash", e))?;
        }
        std::fs::rename(&copy, dest).map_err(|e| MoveError::io("could not put the app in the Applications folder", e))
    }
}

/// A name in `trash` for the app at `dest` that isn't taken: `DeviceSteer 1.0.0.app`, then with
/// a number.
fn trash_path(trash: &Path, dest: &Path, version: Option<&str>) -> PathBuf {
    let stem = dest.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "DeviceSteer".into());
    let base = match version {
        Some(v) if !v.is_empty() => format!("{stem} {v}"),
        _ => stem,
    };
    let mut n = 1;
    loop {
        let name = if n == 1 { format!("{base}.app") } else { format!("{base} {n}.app") };
        let p = trash.join(name);
        if std::fs::symlink_metadata(&p).is_err() {
            return p;
        }
        n += 1;
    }
}

/// The `CFBundleShortVersionString` of the app at `app`.
fn bundle_version(app: &Path) -> Option<String> {
    let out = std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(app.join("Contents/Info.plist"))
        .output()
        .ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !v.is_empty()).then_some(v)
}

fn running_bundle() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|exe| bundle_of(&exe))
}

/// Whether the running app should move to the Applications folder, and what is there now.
#[tauri::command]
pub fn app_location() -> Location {
    match running_bundle() {
        Some(b) if cfg!(target_os = "macos") => Mover::current().location(&b),
        _ => Location { reason: None, exists: false, existing_version: None },
    }
}

/// Moves the running app to the Applications folder, opens the copy and quits. `replace` is the
/// person's confirmation to put an app already there in the Trash.
#[tauri::command]
pub fn app_move_to_applications(app: tauri::AppHandle, replace: bool) -> Result<(), MoveError> {
    // The installer puts the app where it stays on Windows, and `app_location` never asks for a
    // move there; the rest (ditto, the Trash, `open`) is macOS only.
    if !cfg!(target_os = "macos") {
        return Err(MoveError::Failed { message: "moving the app is only needed on macOS".into() });
    }
    let bundle = running_bundle().ok_or(MoveError::Failed { message: "this copy of DeviceSteer isn't an app bundle".into() })?;
    if temporary(&bundle).is_none() {
        return Err(MoveError::Failed { message: "DeviceSteer is already installed".into() });
    }
    let dest = Mover::current().move_in(&bundle, replace)?;
    // `open` brings an app with our bundle id to the front instead of starting another while this
    // process runs, so a shell waits for it to end first (up to 10 s), then opens the copy.
    let waiter = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            r#"i=0; while kill -0 "$1" 2>/dev/null && [ $i -lt 100 ]; do sleep 0.1; i=$((i+1)); done; exec /usr/bin/open "$2""#,
            "sh",
        ])
        .arg(std::process::id().to_string())
        .arg(&dest)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(e) = waiter {
        return Err(MoveError::Failed {
            message: format!("DeviceSteer is in the Applications folder but could not be reopened ({e}); quit and open it from there"),
        });
    }
    app.exit(0);
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("devicesteer-app-location-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            for d in ["Applications", "Trash", "Volumes"] {
                std::fs::create_dir_all(root.join(d)).unwrap();
            }
            Self { root }
        }
        fn mover(&self) -> Mover {
            Mover { applications: self.root.join("Applications"), trash: self.root.join("Trash") }
        }
        /// A minimal bundle at `dir/DeviceSteer.app` with `version`.
        fn bundle(&self, dir: &str, version: &str) -> PathBuf {
            let app = self.root.join(dir).join("DeviceSteer.app");
            std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
            std::fs::write(app.join("Contents/MacOS/devicesteer-app"), version).unwrap();
            std::fs::write(
                app.join("Contents/Info.plist"),
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>"#
                ),
            )
            .unwrap();
            app
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn temporary_places() {
        assert_eq!(temporary(Path::new("/Volumes/DeviceSteer/DeviceSteer.app")), Some(Reason::DiskImage));
        assert_eq!(
            temporary(Path::new("/private/var/folders/xy/abc/T/AppTranslocation/1234-5678/d/DeviceSteer.app/Contents")),
            Some(Reason::Translocated)
        );
        assert_eq!(temporary(Path::new("/Applications/DeviceSteer.app")), None);
        assert_eq!(temporary(Path::new("/Users/someone/Applications/DeviceSteer.app")), None);
        // Only the root `/Volumes`, not a folder that happens to be named so.
        assert_eq!(temporary(Path::new("/Users/someone/Volumes/DeviceSteer.app")), None);
    }

    #[test]
    fn finds_the_bundle() {
        let f = Fixture::new("bundle");
        let app = f.bundle("Volumes", "1.0.0").canonicalize().unwrap();
        assert_eq!(bundle_of(&app.join("Contents/MacOS/devicesteer-app")).unwrap(), app);
        assert!(bundle_of(&f.root.join("target/debug/devicesteer-app")).is_none());
    }

    #[test]
    fn moves_into_an_empty_folder() {
        let f = Fixture::new("empty");
        let src = f.bundle("Volumes", "1.0.1");
        let m = f.mover();
        let loc = m.location(&src);
        assert!(!loc.exists && loc.existing_version.is_none());
        let dest = m.move_in(&src, false).unwrap();
        assert_eq!(dest, f.root.join("Applications/DeviceSteer.app"));
        assert_eq!(std::fs::read_to_string(dest.join("Contents/MacOS/devicesteer-app")).unwrap(), "1.0.1");
        // The source stays (it is the running app), and no staging folder is left behind.
        assert!(src.is_dir());
        let left: Vec<_> = std::fs::read_dir(f.root.join("Applications")).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from("DeviceSteer.app")]);
    }

    #[test]
    fn replacing_needs_confirmation_and_trashes_the_old_app() {
        let f = Fixture::new("replace");
        let src = f.bundle("Volumes", "1.0.1");
        f.bundle("Applications", "1.0.0");
        let m = f.mover();
        let loc = m.location(&src);
        assert!(loc.exists);
        assert_eq!(loc.existing_version.as_deref(), Some("1.0.0"));

        assert_eq!(m.move_in(&src, false), Err(MoveError::Exists { version: Some("1.0.0".into()) }));
        let dest = f.root.join("Applications/DeviceSteer.app");
        assert_eq!(std::fs::read_to_string(dest.join("Contents/MacOS/devicesteer-app")).unwrap(), "1.0.0");

        m.move_in(&src, true).unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("Contents/MacOS/devicesteer-app")).unwrap(), "1.0.1");
        let trashed = f.root.join("Trash/DeviceSteer 1.0.0.app");
        assert_eq!(std::fs::read_to_string(trashed.join("Contents/MacOS/devicesteer-app")).unwrap(), "1.0.0");
    }

    #[test]
    fn trash_names_dont_collide() {
        let f = Fixture::new("trash");
        let trash = f.root.join("Trash");
        let dest = Path::new("/Applications/DeviceSteer.app");
        assert_eq!(trash_path(&trash, dest, Some("1.0.0")), trash.join("DeviceSteer 1.0.0.app"));
        std::fs::create_dir(trash.join("DeviceSteer 1.0.0.app")).unwrap();
        assert_eq!(trash_path(&trash, dest, Some("1.0.0")), trash.join("DeviceSteer 1.0.0 2.app"));
        assert_eq!(trash_path(&trash, dest, None), trash.join("DeviceSteer.app"));
    }

    #[test]
    fn unwritable_applications_folder_is_no_permission() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new("perm");
        let src = f.bundle("Volumes", "1.0.1");
        let apps = f.root.join("Applications");
        std::fs::set_permissions(&apps, std::fs::Permissions::from_mode(0o555)).unwrap();
        let r = f.mover().move_in(&src, false);
        std::fs::set_permissions(&apps, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(r, Err(MoveError::NoPermission { .. })), "{r:?}");
    }
}
