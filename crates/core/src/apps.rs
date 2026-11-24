//! Which app a process is: its display name and bundle id, from its executable path.
//!
//! The accessibility tree names elements only by pid. CoreDevice's process list gives each pid's
//! executable path, and installation_proxy maps bundle paths to names and bundle ids.

use std::collections::HashMap;

/// Bundles installation_proxy does not list, by their folder name.
const UNLISTED: &[(&str, &str, &str)] = &[("SpringBoard.app", "SpringBoard", SPRINGBOARD)];

/// The home screen, Control Center, the lock screen and notifications, the app switcher and
/// system alerts all belong to this process.
pub const SPRINGBOARD: &str = "com.apple.springboard";

/// A process as `ui` names it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProcName {
    pub name: Option<String>,
    pub bundle_id: Option<String>,
}

/// Installed apps by bundle path.
#[derive(Debug, Default)]
pub struct AppDirectory {
    by_path: HashMap<String, (String, String)>,
}

impl AppDirectory {
    /// From (bundle path, display name, bundle id) triples.
    pub fn new(apps: Vec<(String, String, String)>) -> Self {
        let by_path = apps.into_iter().map(|(path, name, id)| (normalize(&path), (name, id))).collect();
        Self { by_path }
    }

    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }

    /// Whether the bundle `exe` belongs to is listed (or needs no listing).
    pub fn knows(&self, exe: &str) -> bool {
        match bundle_dir(&normalize(exe)) {
            Some(dir) => self.by_path.contains_key(dir) || dir.ends_with(".appex") || dir.ends_with(".xpc") || unlisted(dir).is_some(),
            None => true,
        }
    }

    /// The name and bundle id of the process running `exe`.
    pub fn describe(&self, exe: &str) -> ProcName {
        let exe = normalize(exe);
        let Some(dir) = bundle_dir(&exe) else {
            return ProcName { name: exe.rsplit('/').next().map(str::to_string), bundle_id: None };
        };
        if let Some((name, id)) = self.by_path.get(dir) {
            return ProcName { name: Some(name.clone()), bundle_id: Some(id.clone()) };
        }
        if let Some((name, id)) = unlisted(dir) {
            return ProcName { name: Some(name.into()), bundle_id: Some(id.into()) };
        }
        // Extensions and XPC services: the folder name, which for many is the bundle id.
        let folder = dir.rsplit('/').next().unwrap_or(dir);
        let stem = folder.rsplit_once('.').map_or(folder, |(s, _)| s);
        let looks_like_id = stem.starts_with("com.") && stem.matches('.').count() >= 2;
        ProcName { name: Some(stem.to_string()), bundle_id: looks_like_id.then(|| stem.to_string()) }
    }
}

fn unlisted(dir: &str) -> Option<(&'static str, &'static str)> {
    let folder = dir.rsplit('/').next()?;
    UNLISTED.iter().find(|(f, _, _)| *f == folder).map(|(_, n, id)| (*n, *id))
}

/// `file:///private/var/x/` and `/var/x` are the same place.
fn normalize(path: &str) -> String {
    let p = path.strip_prefix("file://").unwrap_or(path);
    let p = p.strip_prefix("/private").filter(|r| r.starts_with('/')).unwrap_or(p);
    p.trim_end_matches('/').to_string()
}

/// The innermost bundle folder (`.app`, `.appex`, `.xpc`) on an executable's path.
fn bundle_dir(exe: &str) -> Option<&str> {
    let mut end = None;
    for (i, _) in exe.match_indices('/') {
        let head = &exe[..i];
        if [".app", ".appex", ".xpc"].iter().any(|ext| head.ends_with(ext)) {
            end = Some(i);
        }
    }
    end.map(|i| &exe[..i])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> AppDirectory {
        AppDirectory::new(vec![
            ("/private/var/containers/Bundle/Application/X/Calculator.app".into(), "Calculator".into(), "com.apple.calculator".into()),
            ("/Applications/Preferences.app".into(), "Settings".into(), "com.apple.Preferences".into()),
        ])
    }

    #[test]
    fn names_apps_from_their_executables() {
        let d = dir();
        let calc = d.describe("file:///private/var/containers/Bundle/Application/X/Calculator.app/Calculator");
        assert_eq!(calc, ProcName { name: Some("Calculator".into()), bundle_id: Some("com.apple.calculator".into()) });
        assert_eq!(d.describe("file:///Applications/Preferences.app/Preferences").bundle_id.as_deref(), Some("com.apple.Preferences"));
        let sb = d.describe("file:///System/Library/CoreServices/SpringBoard.app/SpringBoard");
        assert_eq!(sb.bundle_id.as_deref(), Some(SPRINGBOARD));
    }

    #[test]
    fn names_extensions_and_services_by_folder() {
        let d = dir();
        let web = d.describe("file:///System/Library/Frameworks/WebKit.framework/XPCServices/com.apple.WebKit.WebContent.xpc/com.apple.WebKit.WebContent");
        assert_eq!(web.bundle_id.as_deref(), Some("com.apple.WebKit.WebContent"));
        let ext = d.describe("file:///private/var/containers/Bundle/Application/X/Calculator.app/PlugIns/CalculatorWidget.appex/CalculatorWidget");
        assert_eq!(ext, ProcName { name: Some("CalculatorWidget".into()), bundle_id: None });
        assert!(d.knows("file:///Applications/Preferences.app/Preferences"));
        assert!(!d.knows("file:///Applications/Other.app/Other"));
        assert_eq!(d.describe("file:///usr/libexec/backboardd").name.as_deref(), Some("backboardd"));
    }
}
