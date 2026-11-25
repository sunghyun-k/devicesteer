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
//! On Windows the same layout sits under the user's profile folder, with the app's install folder
//! in place of `Contents` (the CLI is `devicesteer.exe` next to the app, the skill in `skills\`).
//! Symlinks there need Developer Mode, so folders are linked with directory junctions and the CLI,
//! a file, with a hard link when a symlink can't be made (see [`symlink`]).
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

const SKILL_NAME: &str = "devicesteer";

/// The CLI's file name, and where it and the skills are in the folder `current` points to.
#[cfg(not(windows))]
mod app_files {
    pub const CLI: &str = "devicesteer";
    pub const CLI_IN_APP: &str = "MacOS/devicesteer";
    pub const SKILLS_IN_APP: &str = "Resources/skills";
}
#[cfg(windows)]
mod app_files {
    pub const CLI: &str = "devicesteer.exe";
    pub const CLI_IN_APP: &str = "devicesteer.exe";
    pub const SKILLS_IN_APP: &str = "skills";
}
use app_files::{CLI, CLI_IN_APP, SKILLS_IN_APP};

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
    /// Names `unavailable` for the page's translations (`agents.unavailable.<code>` in
    /// app/src/locales/*.json); `None` for a reason with an OS error in it, shown in English.
    pub unavailable_code: Option<&'static str>,
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
    /// For the running app and the user's real home folder (the profile folder on Windows, where
    /// HOME is usually unset).
    pub fn current() -> Option<Self> {
        let home = dirs::home_dir()?;
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
            return Some((self.bin_dir().join(CLI), self.current_link().join(CLI_IN_APP)));
        }
        let agent = AGENTS.iter().find(|a| a.id == id)?;
        Some((
            self.home.join(agent.skills).join(SKILL_NAME),
            self.current_link().join(SKILLS_IN_APP).join(SKILL_NAME),
        ))
    }

    fn tilde(&self, p: &Path) -> String {
        match p.strip_prefix(&self.home) {
            Ok(rest) => display(&Path::new("~").join(rest)),
            Err(_) => display(p),
        }
    }

    fn item(&self, id: &str, name: &str, detected: bool) -> Item {
        let (path, target) = self.link(id).expect("known id");
        Item { id: id.into(), name: name.into(), path: self.tilde(&path), detected, link: link_state(&path, &target) }
    }

    pub fn status(&self, on_path: Option<bool>) -> Status {
        Status {
            unavailable: self.contents.as_ref().err().cloned(),
            unavailable_code: self.contents.as_ref().err().and_then(|m| UNAVAILABLE.iter().find(|(_, t)| t == m)).map(|(c, _)| *c),
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
        match link_target(&current) {
            Some(to) if &to == contents => Ok(()),
            Some(_) => repoint(&current, contents),
            None => Ok(()),
        }
    }

    /// Creates the link `id`. A conflict is an error unless `replace`: then a link elsewhere is
    /// removed and a file or folder is moved aside to `<name>.backup-<seconds>`. Returns a note
    /// for the person when something was moved.
    pub fn install(&self, id: &str, replace: bool) -> Result<Option<Note>, String> {
        let contents = self.contents.as_ref().map_err(Clone::clone)?;
        let (path, target) = self.link(id).ok_or_else(|| format!("unknown item `{id}`"))?;
        let mut note = None;
        match link_state(&path, &target) {
            LinkState::Installed => {}
            LinkState::Missing => {}
            LinkState::OtherLink { points_to } if replace => {
                remove_link(&path).map_err(|e| format!("could not remove {}: {e}", self.tilde(&path)))?;
                note = Some(Note { code: "replaced_link", message: format!("Replaced the link to {points_to}."), path: points_to });
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
                let path = self.tilde(&aside);
                note = Some(Note { code: "moved_aside", message: format!("Moved what was there to {path}."), path });
            }
            LinkState::OtherLink { .. } | LinkState::NotALink { .. } => {
                return Err(format!("{} already exists and isn't DeviceSteer's; confirm to replace it", self.tilde(&path)));
            }
        }
        // `current` first: a link to it is useless until it exists.
        let current = self.current_link();
        if link_target(&current).as_ref() != Some(contents) {
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
                remove_link(&path).map_err(|e| format!("could not remove {}: {e}", self.tilde(&path)))?
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
                let _ = remove_link(&current);
            }
            // Only if empty: something else may live there.
            let _ = std::fs::remove_dir(self.share());
        }
        Ok(())
    }

    /// Whether `~/.local/bin` is in `path` (a PATH value).
    fn bin_dir_in(&self, path: &str) -> bool {
        let bin = self.bin_dir();
        std::env::split_paths(path).any(|p| same_dir(&p, &bin) || p == Path::new("~/.local/bin"))
    }
}

/// `p` for the page, with Windows' separators throughout there: our relative paths are written
/// with `/`.
fn display(p: &Path) -> String {
    let s = p.display().to_string();
    if cfg!(windows) { s.replace('/', "\\") } else { s }
}

/// Paths on Windows compare without case, and PATH entries often end in a separator.
fn same_dir(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        let norm = |p: &Path| p.to_string_lossy().trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase();
        norm(a) == norm(b)
    } else {
        a == b
    }
}

/// `p` without the `\\?\` prefix Windows' canonical paths carry, so it compares equal to the same
/// path written plainly (and a junction doesn't store the prefix).
fn plain(p: PathBuf) -> PathBuf {
    #[cfg(windows)]
    if let Some(rest) = p.to_str().and_then(|s| s.strip_prefix(r"\\?\"))
        && !rest.starts_with(r"UNC\")
    {
        return PathBuf::from(rest);
    }
    p
}

/// Where the link `p` points, without the `\\?\` prefix (see [`plain`]).
fn link_target(p: &Path) -> Option<PathBuf> {
    std::fs::read_link(p).ok().map(plain)
}

fn link_state(path: &Path, target: &Path) -> LinkState {
    match std::fs::symlink_metadata(path) {
        Err(_) => LinkState::Missing,
        // Junctions count as symlinks here too.
        Ok(m) if m.file_type().is_symlink() => match link_target(path) {
            Some(to) if to == target => LinkState::Installed,
            Some(to) => LinkState::OtherLink { points_to: to.display().to_string() },
            None => LinkState::OtherLink { points_to: String::new() },
        },
        // The CLI's hard link (see `symlink`): ours when it is the very file.
        #[cfg(windows)]
        Ok(m) if m.is_file() && same_file::is_same_file(path, target).unwrap_or(false) => LinkState::Installed,
        Ok(m) => LinkState::NotALink { is_dir: m.is_dir() },
    }
}

/// Replaces the symlink `link` with one to `to`, atomically: a link made next to it is renamed
/// over it, so an agent running `devicesteer` meanwhile never finds it missing.
#[cfg(not(windows))]
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

/// Replaces the junction `link` with one to `to`. Not atomically: Windows renames only files over
/// files, so the old junction goes first and `current` is missing for that moment.
#[cfg(windows)]
fn repoint(link: &Path, to: &Path) -> Result<(), String> {
    let dir = link.parent().expect("link has a parent");
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    if std::fs::symlink_metadata(link).is_ok() {
        remove_link(link).map_err(|e| format!("could not update {}: {e}", link.display()))?;
    }
    symlink(to, link).map_err(|e| format!("could not update {}: {e}", link.display()))
}

#[cfg(unix)]
fn symlink(to: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(to, link)
}

/// Links `link` to `to` without Developer Mode, which symlinks need on Windows. A folder gets a
/// directory junction: it stores the path, so a link through `current` follows `current` like a
/// symlink does. The CLI, a file, gets a symlink when allowed and a hard link otherwise. A hard
/// link is the file itself, not a path: it stays with the file `current` pointed to when it was
/// made, which the installer overwrites in place on update, but it won't follow the app to another
/// folder until it is set up again.
#[cfg(windows)]
fn symlink(to: &Path, link: &Path) -> std::io::Result<()> {
    if to.is_dir() {
        junction::create(to, link)
    } else {
        std::os::windows::fs::symlink_file(to, link).or_else(|_| std::fs::hard_link(to, link))
    }
}

#[cfg(not(any(unix, windows)))]
fn symlink(_to: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("links aren't supported on this platform"))
}

/// Removes the link `p` itself, never what it points to. Junctions and folder symlinks are
/// folders to Windows and go with `remove_dir` (`is_dir` is false for them, as for any link).
fn remove_link(p: &Path) -> std::io::Result<()> {
    std::fs::remove_file(p).or_else(|e| {
        if cfg!(windows) && std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) {
            std::fs::remove_dir(p)
        } else {
            Err(e)
        }
    })
}

/// The `Contents` folder of the app `exe` runs from, if it is an installed bundle that carries the
/// CLI and the skill.
#[cfg(not(windows))]
fn bundle_contents(exe: &Path) -> Result<PathBuf, String> {
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    let contents = exe
        .parent()
        .filter(|d| d.ends_with("Contents/MacOS"))
        .and_then(Path::parent)
        .filter(|c| c.parent().is_some_and(|a| a.extension().is_some_and(|e| e == "app")))
        .ok_or(DEV_BUILD)?;
    // Gatekeeper runs a quarantined app from a random read-only copy (App Translocation), and a
    // disk image goes away when ejected: a link to either breaks soon. The page offers the move.
    if crate::app_location::temporary(contents).is_some() {
        return Err(MOVE_FIRST.into());
    }
    carries_cli_and_skill(contents)?;
    Ok(contents.to_path_buf())
}

/// The installed app's folder on Windows: the one `exe` is in, with the CLI next to it and the
/// skill under `skills`, as the installer lays them out.
#[cfg(windows)]
fn bundle_contents(exe: &Path) -> Result<PathBuf, String> {
    let exe = plain(exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf()));
    let dir = exe.parent().ok_or(DEV_BUILD)?;
    if dir.ends_with("target/debug") || dir.ends_with("target/release") {
        return Err(DEV_BUILD.into());
    }
    carries_cli_and_skill(dir)?;
    Ok(dir.to_path_buf())
}

const DEV_BUILD: &str = "Agent setup works in the installed app, not in a development build.";
const MOVE_FIRST: &str = "Move DeviceSteer to the Applications folder and open it from there to set up agents.";
const INCOMPLETE_BUILD: &str = "This build of the app doesn't include the command-line tool and the skill.";

/// The reasons setup can't run that the page translates, by code (`Status::unavailable_code`).
const UNAVAILABLE: [(&str, &str); 3] =
    [("dev_build", DEV_BUILD), ("move_to_applications", MOVE_FIRST), ("incomplete_build", INCOMPLETE_BUILD)];

fn carries_cli_and_skill(contents: &Path) -> Result<(), String> {
    if !contents.join(CLI_IN_APP).is_file() || !contents.join(SKILLS_IN_APP).join(SKILL_NAME).join("SKILL.md").is_file() {
        return Err(INCOMPLETE_BUILD.into());
    }
    Ok(())
}

/// The PATH a new terminal gets on Windows: the user's and the system's, from the registry. The
/// app's own PATH is the one Explorer had when it started, so a change the person just made would
/// show only after relaunching.
#[cfg(windows)]
mod windows_path {
    use windows_sys::Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW},
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// A string value, with `%VARIABLES%` expanded (RegGetValueW does that for REG_EXPAND_SZ).
    fn value(key: HKEY, subkey: &str, name: &str) -> Option<String> {
        let (subkey, name) = (wide(subkey), wide(name));
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
        let mut len = 0u32;
        // SAFETY: the first call only sizes the buffer; the second writes at most `len` bytes.
        unsafe {
            let r = RegGetValueW(key, subkey.as_ptr(), name.as_ptr(), flags, std::ptr::null_mut(), std::ptr::null_mut(), &mut len);
            if r != ERROR_SUCCESS {
                return None;
            }
            let mut buf = vec![0u16; (len as usize).div_ceil(2)];
            let r = RegGetValueW(key, subkey.as_ptr(), name.as_ptr(), flags, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len);
            if r != ERROR_SUCCESS {
                return None;
            }
            let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..n]))
        }
    }

    pub fn user_and_system() -> Option<String> {
        let system = value(HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment", "Path");
        let user = value(HKEY_CURRENT_USER, "Environment", "Path");
        match (user, system) {
            (None, None) => None,
            (u, s) => Some([s, u].into_iter().flatten().collect::<Vec<_>>().join(";")),
        }
    }
}

/// The PATH of the person's login shell, which is what their terminal and their agents get; the
/// app's own PATH comes from launchd and leaves out what shell profiles add. Interactive too, since
/// many people set PATH in `.zshrc`. `None` when the shell fails or takes too long.
#[cfg(unix)]
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
    let layout = Layout::current().ok_or("could not find your home folder")?;
    #[cfg(unix)]
    let on_path = login_shell_path().await.map(|p| layout.bin_dir_in(&p));
    #[cfg(windows)]
    let on_path = Some(windows_path::user_and_system().is_some_and(|p| layout.bin_dir_in(&p)));
    #[cfg(not(any(unix, windows)))]
    let on_path = None;
    Ok(layout.status(on_path))
}

/// What `install` did besides making the link, for the person: `code` names the message in the
/// page's catalogs (`agents.note.<code>`), `path` fills it; `message` is the English text.
#[derive(Debug, Serialize)]
pub struct Note {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct Change {
    pub note: Option<Note>,
}

/// Installs (`install`) or removes the link `id` (`cli` or an agent id). `replace` is the person's
/// confirmation to replace something that isn't ours.
#[tauri::command]
pub async fn agent_setup_change(id: String, install: bool, replace: bool) -> Result<Change, String> {
    let layout = Layout::current().ok_or("could not find your home folder")?;
    if install {
        Ok(Change { note: layout.install(&id, replace)? })
    } else {
        layout.uninstall(&id)?;
        Ok(Change { note: None })
    }
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::*;

    /// A temporary home and a fake app with the CLI and the skill, laid out as on this platform.
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("devicesteer-agent-setup-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("home")).unwrap();
            let root = plain(root.canonicalize().unwrap());
            for app in ["A", "B"] {
                let c = root.join(Self::app_dir(app));
                std::fs::create_dir_all(c.join(CLI_IN_APP).parent().unwrap()).unwrap();
                std::fs::write(c.join(CLI_IN_APP), "#!/bin/sh\n").unwrap();
                std::fs::create_dir_all(c.join(SKILLS_IN_APP).join("devicesteer")).unwrap();
                std::fs::write(c.join(SKILLS_IN_APP).join("devicesteer/SKILL.md"), app).unwrap();
            }
            Self { root }
        }
        /// What `current` points to for app `app`: the bundle's `Contents`, or the install folder.
        fn app_dir(app: &str) -> String {
            if cfg!(windows) { format!("{app}/DeviceSteer") } else { format!("{app}/DeviceSteer.app/Contents") }
        }
        fn home(&self) -> PathBuf {
            self.root.join("home")
        }
        fn layout(&self, app: &str) -> Layout {
            Layout::new(&self.home(), Ok(self.root.join(Self::app_dir(app))))
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
        let st = l.status(None);
        assert_eq!(st.cli.link, LinkState::Installed);
        assert_eq!(st.agents.iter().find(|a| a.id == "claude").unwrap().link, LinkState::Installed);
        let skill = f.home().join(".claude/skills/devicesteer/SKILL.md");
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), "A");
        assert!(f.home().join(".local/bin").join(CLI).is_file());

        // The app moved: launching the copy at B repoints every skill link at once.
        f.layout("B").refresh().unwrap();
        assert_eq!(std::fs::read_to_string(&skill).unwrap(), "B");

        l.uninstall("cli").unwrap();
        assert!(f.home().join(".local/share/devicesteer/current").exists());
        // Removing a link leaves what it pointed to.
        assert!(f.root.join(Fixture::app_dir("A")).join(CLI_IN_APP).is_file());
        l.uninstall("claude").unwrap();
        assert!(std::fs::symlink_metadata(f.home().join(".local/share/devicesteer")).is_err());
        assert!(f.home().join(".claude/skills").is_dir());
        assert!(f.root.join(Fixture::app_dir("B")).join(SKILLS_IN_APP).join("devicesteer/SKILL.md").is_file());
    }

    #[test]
    fn refresh_without_setup_creates_nothing() {
        let f = Fixture::new("fresh");
        f.layout("A").refresh().unwrap();
        assert!(std::fs::symlink_metadata(f.home().join(".local/share/devicesteer")).is_err());
    }

    /// A link to somewhere else at the CLI's path on Unix, at a skill's on Windows (where a file
    /// symlink needs Developer Mode, and a junction is the link people make there).
    #[test]
    fn conflicts_need_confirmation() {
        let f = Fixture::new("conflict");
        let l = f.layout("A");
        let dir = f.home().join(".agents/skills/devicesteer");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "mine").unwrap();
        let elsewhere = f.root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let (other_id, other) = if cfg!(windows) {
            ("claude", f.home().join(".claude/skills/devicesteer"))
        } else {
            ("cli", f.home().join(".local/bin/devicesteer"))
        };
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&elsewhere, &other).unwrap();
        #[cfg(windows)]
        junction::create(&elsewhere, &other).unwrap();
        let points_to = elsewhere.display().to_string();

        let st = l.status(None);
        let item = |st: &Status, id: &str| if id == "cli" { st.cli.link.clone() } else { st.agents.iter().find(|a| a.id == id).unwrap().link.clone() };
        assert_eq!(item(&st, other_id), LinkState::OtherLink { points_to: points_to.clone() });
        assert_eq!(item(&st, "agents"), LinkState::NotALink { is_dir: true });

        assert!(l.install(other_id, false).is_err());
        assert!(l.install("agents", false).is_err());
        assert!(l.uninstall(other_id).is_err());
        assert_eq!(link_target(&other), Some(elsewhere.clone()));
        assert_eq!(std::fs::read_to_string(dir.join("SKILL.md")).unwrap(), "mine");

        assert!(l.install(other_id, true).unwrap().is_some());
        // Replacing the link removed the link, not the folder it pointed to.
        assert!(elsewhere.is_dir());
        let note = l.install("agents", true).unwrap().unwrap();
        assert!(note.path.contains(".backup-"), "{}", note.message);
        assert_eq!(note.code, "moved_aside");
        let st = l.status(None);
        assert_eq!(item(&st, other_id), LinkState::Installed);
        assert_eq!(item(&st, "agents"), LinkState::Installed);
        let backup = std::fs::read_dir(f.home().join(".agents/skills"))
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().starts_with("devicesteer.backup-"))
            .unwrap();
        assert_eq!(std::fs::read_to_string(backup.path().join("SKILL.md")).unwrap(), "mine");
    }

    /// A file at the CLI's path that isn't ours (on Windows our CLI may be a hard link, which is
    /// a plain file too).
    #[test]
    fn a_foreign_cli_file_is_a_conflict() {
        let f = Fixture::new("foreign-cli");
        let l = f.layout("A");
        let cli = f.home().join(".local/bin").join(CLI);
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::write(&cli, "someone else's").unwrap();
        assert_eq!(l.status(None).cli.link, LinkState::NotALink { is_dir: false });
        assert!(l.install("cli", false).is_err());
        assert_eq!(std::fs::read_to_string(&cli).unwrap(), "someone else's");
    }

    /// Without Developer Mode the CLI is a hard link (see `symlink`): it counts as ours, and
    /// removing it leaves the app's copy.
    #[cfg(windows)]
    #[test]
    fn a_hard_linked_cli_is_ours() {
        let f = Fixture::new("hard-link");
        let l = f.layout("A");
        l.install("claude", false).unwrap();
        let (cli, target) = l.link("cli").unwrap();
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::hard_link(&target, &cli).unwrap();
        assert!(!std::fs::symlink_metadata(&cli).unwrap().file_type().is_symlink());
        assert_eq!(l.status(None).cli.link, LinkState::Installed);
        l.install("cli", false).unwrap();
        l.uninstall("cli").unwrap();
        assert!(std::fs::symlink_metadata(&cli).is_err());
        assert!(f.root.join(Fixture::app_dir("A")).join(CLI_IN_APP).is_file());
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

    /// The page translates the reasons it knows by code; others it shows as they are.
    #[test]
    fn unavailable_reasons_have_codes() {
        let f = Fixture::new("codes");
        let status = |reason: &str| Layout::new(&f.home(), Err(reason.into())).status(None);
        assert_eq!(status(DEV_BUILD).unavailable_code, Some("dev_build"));
        assert_eq!(status(MOVE_FIRST).unavailable_code, Some("move_to_applications"));
        assert_eq!(status(INCOMPLETE_BUILD).unavailable_code, Some("incomplete_build"));
        assert_eq!(status("could not find the app: gone").unavailable_code, None);
    }

    #[test]
    fn finds_the_bundle() {
        let f = Fixture::new("bundle");
        let contents = f.root.join(Fixture::app_dir("A"));
        let app_exe = if cfg!(windows) { contents.join("devicesteer-app.exe") } else { contents.join("MacOS/devicesteer-app") };
        std::fs::write(&app_exe, "").unwrap();
        assert_eq!(bundle_contents(&app_exe).unwrap(), contents);
        // A development build runs from target/debug.
        assert!(bundle_contents(&f.root.join("target/debug/devicesteer-app")).is_err());
        // A build without the CLI (built without tauri.bundle.conf.json).
        std::fs::remove_file(contents.join(CLI_IN_APP)).unwrap();
        assert!(bundle_contents(&app_exe).unwrap_err().contains("doesn't include"));
    }

    #[test]
    fn path_check() {
        let f = Fixture::new("path");
        let l = f.layout("A");
        let bin = f.home().join(".local/bin");
        let (sys, sep) = if cfg!(windows) { (r"C:\Windows\System32", ";") } else { ("/usr/bin", ":") };
        assert!(l.bin_dir_in(&format!("{sys}{sep}{}", bin.display())));
        assert!(!l.bin_dir_in(sys));
        #[cfg(windows)]
        assert!(l.bin_dir_in(&format!("{sys};{}\\", bin.display().to_string().to_uppercase())));
    }
}
