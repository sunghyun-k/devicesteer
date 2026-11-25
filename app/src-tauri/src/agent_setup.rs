//! Agent setup: puts the `devicesteer` command on the user's PATH and the DeviceSteer skill where
//! coding agents look for skills, both as symlinks into the installed app.
//!
//! Every link goes through one indirection, `~/.local/share/devicesteer/current`, a symlink to
//! the running app's `Contents` folder:
//!
//! ```text
//! ~/.local/bin/devicesteer             -> ~/.local/share/devicesteer/current/MacOS/devicesteer
//! ~/.claude/skills/devicesteer         -> ~/.local/share/devicesteer/current/Resources/skills/devicesteer
//! ~/.agents/skills/devicesteer  (etc.) -> the same
//! ~/.local/share/devicesteer/current   -> /Applications/DeviceSteer.app/Contents
//! ```
//!
//! Why links and not copies: a copy of the CLI or the skill goes stale when the app updates, and
//! the skill tells agents to run `devicesteer skill`, whose guide must match the binary. Why the
//! indirection and not links straight into the bundle: the updater replaces the bundle in place,
//! which keeps any link valid, but the person may move or rename the app; the app repoints
//! `current` at every launch (`refresh`), so one link follows it instead of every agent's.
//!
//! Nothing that isn't ours is ever replaced silently: a file, a folder or a link pointing
//! elsewhere at one of these paths shows as a conflict, and replacing it takes the person's
//! confirmation (`replace`), after which a non-link is moved aside rather than deleted.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// An agent whose skills folder we know. `detect` are folders whose presence means the agent is
/// installed; `skills` is where it reads user-level skills, relative to home.
struct Agent {
    id: &'static str,
    name: &'static str,
    detect: &'static [&'static str],
    skills: &'static str,
}

/// Agents get one link each. `~/.agents/skills` is the shared folder Codex, Gemini CLI, OpenCode
/// and others read, so those agents need no folder of their own here (a second copy of the same
/// skill makes some of them warn about duplicates).
const AGENTS: &[Agent] = &[
    Agent { id: "claude", name: "Claude Code", detect: &[".claude"], skills: ".claude/skills" },
    Agent {
        id: "agents",
        name: "Codex, Gemini CLI, OpenCode and others",
        detect: &[".agents", ".codex", ".gemini", ".config/opencode"],
        skills: ".agents/skills",
    },
    Agent { id: "cursor", name: "Cursor", detect: &[".cursor"], skills: ".cursor/skills" },
    Agent { id: "copilot", name: "GitHub Copilot CLI", detect: &[".copilot"], skills: ".copilot/skills" },
];

const CLI_NAME: &str = "devicesteer";
const SKILL_NAME: &str = "devicesteer";

/// What is at one of our paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LinkState {
    /// Our link, pointing where it should.
    Installed,
    /// Nothing there.
    Missing,
    /// A symlink to somewhere else (shown so the person can decide).
    OtherLink { points_to: String },
    /// A real file or folder.
    NotALink { is_dir: bool },
}

#[derive(Debug, Serialize)]
pub struct Item {
    /// `cli` or an agent id.
    pub id: String,
    pub name: String,
    /// Where the link goes, with the home folder shown as `~`.
    pub path: String,
    /// Whether the agent looks installed. Always true for the CLI.
    pub detected: bool,
    #[serde(flatten)]
    pub link: LinkState,
}

#[derive(Debug, Serialize)]
pub struct Status {
    /// Why setup can't run from this copy of the app (a development build, a translocated or
    /// disk-image copy, Windows), or `None` when it can.
    pub unavailable: Option<String>,
    pub cli: Item,
    /// Whether `~/.local/bin` is on the PATH of the person's login shell. `None` until checked
    /// (it starts a shell) or when the check failed.
    pub on_path: Option<bool>,
    pub agents: Vec<Item>,
}

/// Paths for one home folder and one app bundle; separate from the process so tests can use a
/// temporary home.
pub struct Layout {
    home: PathBuf,
    /// The running app's `Contents` folder, or why there is none usable.
    contents: Result<PathBuf, String>,
}

impl Layout {
    /// For the running app and the user's real home folder.
    pub fn current() -> Option<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        let contents = std::env::current_exe()
            .map_err(|e| format!("could not find the app: {e}"))
            .and_then(|exe| bundle_contents(&exe));
        Some(Self { home, contents })
    }

    #[cfg(test)]
    fn new(home: &Path, contents: Result<PathBuf, String>) -> Self {
        Self { home: home.to_path_buf(), contents }
    }

    fn share(&self) -> PathBuf {
        self.home.join(".local/share/devicesteer")
    }

    /// The indirection every link goes through.
    fn current_link(&self) -> PathBuf {
        self.share().join("current")
    }

    fn bin_dir(&self) -> PathBuf {
        self.home.join(".local/bin")
    }

    /// Where the link `id` goes, and what it should point to.
    fn link(&self, id: &str) -> Option<(PathBuf, PathBuf)> {
        if id == "cli" {
            return Some((self.bin_dir().join(CLI_NAME), self.current_link().join("MacOS").join(CLI_NAME)));
        }
        let agent = AGENTS.iter().find(|a| a.id == id)?;
        Some((
            self.home.join(agent.skills).join(SKILL_NAME),
            self.current_link().join("Resources/skills").join(SKILL_NAME),
        ))
    }

    fn tilde(&self, p: &Path) -> String {
        match p.strip_prefix(&self.home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => p.display().to_string(),
        }
    }

    fn item(&self, id: &str, name: &str, detected: bool) -> Item {
        let (path, target) = self.link(id).expect("known id");
        Item { id: id.into(), name: name.into(), path: self.tilde(&path), detected, link: link_state(&path, &target) }
    }

    pub fn status(&self, on_path: Option<bool>) -> Status {
        let unavailable = if cfg!(unix) { self.contents.as_ref().err().cloned() } else {
            Some("Agent setup isn't available on Windows yet.".into())
        };
        Status {
            unavailable,
            cli: self.item("cli", "devicesteer command", true),
            on_path,
            agents: AGENTS
                .iter()
                .map(|a| self.item(a.id, a.name, a.detect.iter().any(|d| self.home.join(d).is_dir())))
                .collect(),
        }
    }

    /// Points `current` at this app, if anything was set up before. Run at every launch, so links
    /// follow the app when it moves. Does nothing for a copy that can't be set up (a development
    /// build, a disk image): links keep pointing at the installed app.
    pub fn refresh(&self) -> Result<(), String> {
        let Ok(contents) = &self.contents else { return Ok(()) };
        let current = self.current_link();
        match std::fs::read_link(&current) {
            Ok(to) if &to == contents => Ok(()),
            Ok(_) => repoint(&current, contents),
            Err(_) => Ok(()),
        }
    }

    /// Creates the link `id`. A conflict is an error unless `replace`: then a link elsewhere is
    /// removed and a file or folder is moved aside to `<name>.backup-<seconds>`. Returns a note
    /// for the person when something was moved.
    pub fn install(&self, id: &str, replace: bool) -> Result<Option<String>, String> {
        let contents = self.contents.as_ref().map_err(Clone::clone)?;
        let (path, target) = self.link(id).ok_or_else(|| format!("unknown item `{id}`"))?;
        let mut note = None;
        match link_state(&path, &target) {
            LinkState::Installed => {}
            LinkState::Missing => {}
            LinkState::OtherLink { points_to } if replace => {
                std::fs::remove_file(&path).map_err(|e| format!("could not remove {}: {e}", self.tilde(&path)))?;
                note = Some(format!("Replaced the link to {points_to}."));
            }
            LinkState::NotALink { .. } if replace => {
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or_default();
                let mut aside = path.clone().into_os_string();
                aside.push(format!(".backup-{stamp}"));
                let aside = PathBuf::from(aside);
                std::fs::rename(&path, &aside).map_err(|e| format!("could not move {} aside: {e}", self.tilde(&path)))?;
                note = Some(format!("Moved what was there to {}.", self.tilde(&aside)));
            }
            LinkState::OtherLink { .. } | LinkState::NotALink { .. } => {
                return Err(format!("{} already exists and isn't DeviceSteer's; confirm to replace it", self.tilde(&path)));
            }
        }
        // `current` first: a link to it is useless until it exists.
        let current = self.current_link();
        if std::fs::read_link(&current).ok().as_ref() != Some(contents) {
            if std::fs::symlink_metadata(&current).is_ok_and(|m| !m.file_type().is_symlink()) {
                return Err(format!("{} isn't a link; remove it and try again", self.tilde(&current)));
            }
            repoint(&current, contents)?;
        }
        if std::fs::symlink_metadata(&path).is_err() {
            let parent = path.parent().expect("link has a parent");
            std::fs::create_dir_all(parent).map_err(|e| format!("could not create {}: {e}", self.tilde(parent)))?;
            symlink(&target, &path).map_err(|e| format!("could not create {}: {e}", self.tilde(&path)))?;
        }
        Ok(note)
    }

    /// Removes the link `id` if it is ours, then `current` and our folder once no link uses
    /// them. Anything else at the path is left alone.
    pub fn uninstall(&self, id: &str) -> Result<(), String> {
        let (path, target) = self.link(id).ok_or_else(|| format!("unknown item `{id}`"))?;
        match link_state(&path, &target) {
            LinkState::Installed => {
                std::fs::remove_file(&path).map_err(|e| format!("could not remove {}: {e}", self.tilde(&path)))?
            }
            LinkState::Missing => {}
            _ => return Err(format!("{} isn't DeviceSteer's link; it was left as it is", self.tilde(&path))),
        }
        let any_left = std::iter::once("cli")
            .chain(AGENTS.iter().map(|a| a.id))
            .filter_map(|id| self.link(id))
            .any(|(p, t)| link_state(&p, &t) == LinkState::Installed);
        if !any_left {
            let current = self.current_link();
            if std::fs::symlink_metadata(&current).is_ok_and(|m| m.file_type().is_symlink()) {
                let _ = std::fs::remove_file(&current);
            }
            // Only if empty: something else may live there.
            let _ = std::fs::remove_dir(self.share());
        }
        Ok(())
    }

    /// Whether `~/.local/bin` is in `path` (a PATH value).
    fn bin_dir_in(&self, path: &str) -> bool {
        let bin = self.bin_dir();
        std::env::split_paths(path).any(|p| p == bin || p == Path::new("~/.local/bin"))
    }
}

fn link_state(path: &Path, target: &Path) -> LinkState {
    match std::fs::symlink_metadata(path) {
        Err(_) => LinkState::Missing,
        Ok(m) if m.file_type().is_symlink() => match std::fs::read_link(path) {
            Ok(to) if to == target => LinkState::Installed,
            Ok(to) => LinkState::OtherLink { points_to: to.display().to_string() },
            Err(_) => LinkState::OtherLink { points_to: String::new() },
        },
        Ok(m) => LinkState::NotALink { is_dir: m.is_dir() },
    }
}

/// Replaces the symlink `link` with one to `to`, atomically: a link made next to it is renamed
/// over it, so an agent running `devicesteer` meanwhile never finds it missing.
fn repoint(link: &Path, to: &Path) -> Result<(), String> {
    let dir = link.parent().expect("link has a parent");
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let tmp = dir.join(format!(".current-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    symlink(to, &tmp).map_err(|e| format!("could not create {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, link).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not update {}: {e}", link.display())
    })
}

#[cfg(unix)]
fn symlink(to: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(to, link)
}

#[cfg(not(unix))]
fn symlink(_to: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("symlinks need Developer Mode on Windows"))
}

/// The `Contents` folder of the app `exe` runs from, if it is an installed bundle that carries the
/// CLI and the skill.
fn bundle_contents(exe: &Path) -> Result<PathBuf, String> {
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    let contents = exe
        .parent()
        .filter(|d| d.ends_with("Contents/MacOS"))
        .and_then(Path::parent)
        .filter(|c| c.parent().is_some_and(|a| a.extension().is_some_and(|e| e == "app")))
        .ok_or("Agent setup works in the installed app, not in a development build.")?;
    let s = contents.to_string_lossy();
    // Gatekeeper runs a quarantined app from a random read-only copy (App Translocation), and a
    // disk image goes away when ejected: a link to either breaks soon.
    if s.contains("/AppTranslocation/") || s.starts_with("/Volumes/") {
        return Err("Move DeviceSteer to the Applications folder and open it from there to set up agents.".into());
    }
    if !contents.join("MacOS").join(CLI_NAME).is_file() || !contents.join("Resources/skills").join(SKILL_NAME).join("SKILL.md").is_file() {
        return Err("This build of the app doesn't include the command-line tool and the skill.".into());
    }
    Ok(contents.to_path_buf())
}

/// The PATH of the person's login shell, which is what their terminal and their agents get; the
/// app's own PATH comes from launchd and leaves out what shell profiles add. Interactive too, since
/// many people set PATH in `.zshrc`. `None` when the shell fails or takes too long.
async fn login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    const MARK: &str = "__DEVICESTEER_PATH__";
    let mut cmd = tokio::process::Command::new(shell);
    cmd.args(["-ilc", &format!("printf '\\n{MARK}%s\\n' \"$PATH\"")])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(std::time::Duration::from_secs(5), cmd.output()).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().rev().find_map(|l| l.strip_prefix(MARK))?;
    Some(line.to_string())
}

/// Points `current` at this app if setup was done before. Errors are only logged: the viewer works
/// without it.
pub fn refresh_on_launch() {
    if let Some(layout) = Layout::current()
        && let Err(e) = layout.refresh()
    {
        eprintln!("agent setup: {e}");
    }
}

/// What is installed, and whether `~/.local/bin` is on the login shell's PATH.
#[tauri::command]
pub async fn agent_setup_status() -> Result<Status, String> {
    let layout = Layout::current().ok_or("HOME isn't set")?;
    let on_path = if cfg!(unix) { login_shell_path().await.map(|p| layout.bin_dir_in(&p)) } else { None };
    Ok(layout.status(on_path))
}

#[derive(Debug, Serialize)]
pub struct Change {
    pub note: Option<String>,
}

/// Installs (`install`) or removes the link `id` (`cli` or an agent id). `replace` is the person's
/// confirmation to replace something that isn't ours.
#[tauri::command]
pub async fn agent_setup_change(id: String, install: bool, replace: bool) -> Result<Change, String> {
    let layout = Layout::current().ok_or("HOME isn't set")?;
    if install {
        Ok(Change { note: layout.install(&id, replace)? })
    } else {
        layout.uninstall(&id)?;
        Ok(Change { note: None })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A temporary home and a fake app bundle with the CLI and the skill.
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("devicesteer-agent-setup-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("home")).unwrap();
            for app in ["A", "B"] {
                let c = root.join(format!("{app}/DeviceSteer.app/Contents"));
                std::fs::create_dir_all(c.join("MacOS")).unwrap();
                std::fs::write(c.join("MacOS/devicesteer"), "#!/bin/sh\n").unwrap();
                std::fs::create_dir_all(c.join("Resources/skills/devicesteer")).unwrap();
                std::fs::write(c.join("Resources/skills/devicesteer/SKILL.md"), app).unwrap();
            }
            Self { root }
        }
        fn home(&self) -> PathBuf {
            self.root.join("home")
        }
        fn layout(&self, app: &str) -> Layout {
            Layout::new(&self.home(), Ok(self.root.join(format!("{app}/DeviceSteer.app/Contents"))))
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn install_follow_move_uninstall() {
        let f = Fixture::new("cycle");
        std::fs::create_dir_all(f.home().join(".claude")).unwrap();
        let l = f.layout("A");
        let st = l.status(None);
        assert_eq!(st.cli.link, LinkState::Missing);
        assert!(st.agents.iter().find(|a| a.id == "claude").unwrap().detected);
        assert!(!st.agents.iter().find(|a| a.id == "cursor").unwrap().detected);

        l.install("cli", false).unwrap();
        l.install("claude", false).unwrap();
        assert_eq!(l.status(None).cli.link, LinkState::Installed);
        let skill = f.home().join(".claude/skills/devicesteer/SKILL.md");
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), "A");
        assert!(f.home().join(".local/bin/devicesteer").is_file());

        // The app moved: launching the copy at B repoints every link at once.
        f.layout("B").refresh().unwrap();
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), "B");

        l.uninstall("cli").unwrap();
        assert!(f.home().join(".local/share/devicesteer/current").exists());
        l.uninstall("claude").unwrap();
        assert!(std::fs::symlink_metadata(f.home().join(".local/share/devicesteer")).is_err());
        assert!(f.home().join(".claude/skills").is_dir());
    }

    #[test]
    fn refresh_without_setup_creates_nothing() {
        let f = Fixture::new("fresh");
        f.layout("A").refresh().unwrap();
        assert!(std::fs::symlink_metadata(f.home().join(".local/share/devicesteer")).is_err());
    }

    #[test]
    fn conflicts_need_confirmation() {
        let f = Fixture::new("conflict");
        let l = f.layout("A");
        let dir = f.home().join(".agents/skills/devicesteer");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "mine").unwrap();
        let other = f.home().join(".local/bin/devicesteer");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("/somewhere/else", &other).unwrap();

        let st = l.status(None);
        assert_eq!(st.cli.link, LinkState::OtherLink { points_to: "/somewhere/else".into() });
        assert_eq!(st.agents.iter().find(|a| a.id == "agents").unwrap().link, LinkState::NotALink { is_dir: true });

        assert!(l.install("cli", false).is_err());
        assert!(l.install("agents", false).is_err());
        assert!(l.uninstall("cli").is_err());
        assert_eq!(std::fs::read_link(&other).unwrap(), PathBuf::from("/somewhere/else"));
        assert_eq!(std::fs::read_to_string(dir.join("SKILL.md")).unwrap(), "mine");

        assert!(l.install("cli", true).unwrap().is_some());
        let note = l.install("agents", true).unwrap().unwrap();
        assert!(note.contains(".backup-"), "{note}");
        let st = l.status(None);
        assert_eq!(st.cli.link, LinkState::Installed);
        assert_eq!(st.agents.iter().find(|a| a.id == "agents").unwrap().link, LinkState::Installed);
        let backup = std::fs::read_dir(f.home().join(".agents/skills"))
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().starts_with("devicesteer.backup-"))
            .unwrap();
        assert_eq!(std::fs::read_to_string(backup.path().join("SKILL.md")).unwrap(), "mine");
    }

    #[test]
    fn unavailable_build_installs_nothing() {
        let f = Fixture::new("dev");
        let l = Layout::new(&f.home(), Err("dev".into()));
        assert_eq!(l.status(None).unavailable.as_deref(), Some("dev"));
        assert!(l.install("cli", false).is_err());
        l.refresh().unwrap();
        assert!(std::fs::symlink_metadata(f.home().join(".local")).is_err());
    }

    #[test]
    fn finds_the_bundle() {
        let f = Fixture::new("bundle");
        let contents = f.root.join("A/DeviceSteer.app/Contents").canonicalize().unwrap();
        std::fs::write(contents.join("MacOS/devicesteer-app"), "").unwrap();
        assert_eq!(bundle_contents(&contents.join("MacOS/devicesteer-app")).unwrap(), contents);
        // A development build runs from target/debug.
        assert!(bundle_contents(&f.root.join("target/debug/devicesteer-app")).is_err());
        // A bundle without the CLI (built without tauri.bundle.conf.json).
        std::fs::remove_file(contents.join("MacOS/devicesteer")).unwrap();
        assert!(bundle_contents(&contents.join("MacOS/devicesteer-app")).unwrap_err().contains("doesn't include"));
    }

    #[test]
    fn path_check() {
        let f = Fixture::new("path");
        let l = f.layout("A");
        let bin = f.home().join(".local/bin");
        assert!(l.bin_dir_in(&format!("/usr/bin:{}", bin.display())));
        assert!(!l.bin_dir_in("/usr/bin:/bin"));
    }
}
