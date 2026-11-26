//! Settings › Agents: the viewer's side of agent setup. The links themselves are made by the
//! `devicesteer-agents` crate, which the CLI (`devicesteer agents`) shares.

use devicesteer_agents::{Layout, Note, Status};
use serde::Serialize;

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
