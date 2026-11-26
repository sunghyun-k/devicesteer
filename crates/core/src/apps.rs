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

/// Widgets on the home screen and the lock screen are drawn by these processes
/// (`com.apple.chrono.WidgetRenderer-Default`, `-StandBy`, ...), which the accessibility tree lists
/// as applications of their own beside SpringBoard (PITFALLS #60).
const WIDGET_RENDERER: &str = "com.apple.chrono.WidgetRenderer";

/// A process that draws only inside SpringBoard's screens, though the tree lists it as an
/// application.
pub fn draws_in_springboard(bundle_id: Option<&str>) -> bool {
    bundle_id.is_some_and(|id| id.starts_with(WIDGET_RENDERER))
}

/// SpringBoard, or a process that draws inside it: with only these on screen, a system screen
/// shows.
pub fn is_system_ui(bundle_id: Option<&str>) -> bool {
    bundle_id == Some(SPRINGBOARD) || draws_in_springboard(bundle_id)
}

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

/// `file:///private/var/x/` and `/var/x` are the same place. A `file://` URL escapes spaces and
/// other bytes (`/Applications/Siri%20AI.app`), which `ui` showed as the app's name (PITFALLS #71).
fn normalize(path: &str) -> String {
    let decoded;
    let p = match path.strip_prefix("file://") {
        Some(url) => {
            decoded = percent_decode(url);
            decoded.as_str()
        }
        None => path,
    };
    let p = p.strip_prefix("/private").filter(|r| r.starts_with('/')).unwrap_or(p);
    p.trim_end_matches('/').to_string()
}

/// `%XX` escapes turned back into bytes; a malformed escape is kept as it is.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
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
    fn widget_renderers_count_as_system_ui() {
        assert!(is_system_ui(Some(SPRINGBOARD)));
        assert!(is_system_ui(Some("com.apple.chrono.WidgetRenderer-Default")));
        assert!(draws_in_springboard(Some("com.apple.chrono.WidgetRenderer-StandBy")));
        assert!(!draws_in_springboard(Some(SPRINGBOARD)));
        assert!(!is_system_ui(Some("com.apple.Preferences")));
        assert!(!is_system_ui(None));
    }

    #[test]
    fn file_urls_are_unescaped() {
        let d = dir();
        let p = d.describe("file:///Applications/Siri%20AI.app/Siri%20AI");
        assert_eq!(p.name.as_deref(), Some("Siri AI"));
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("%ED%95%9C"), "한");
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
