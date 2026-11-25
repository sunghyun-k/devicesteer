//! The viewer's preferences that the CLI and the daemon also read, in `preferences.json` in the
//! state directory. The viewer's own look-and-feel settings stay in its web storage; only what
//! another process acts on lives here.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Open the viewer (or bring its window forward) when an agent starts a session, so the
    /// person sees what the agent does. Settings › General.
    pub open_viewer_on_session: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self { open_viewer_on_session: true }
    }
}

fn path() -> PathBuf {
    crate::paths::state_dir().join("preferences.json")
}

impl Preferences {
    /// The saved preferences, or the defaults when there are none or the file can't be read: a
    /// broken file must not stop an agent's command.
    pub fn load() -> Self {
        std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let p = path();
        // Written whole and renamed, so a reader never sees half a file.
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?).with_context(|| format!("could not write {}", tmp.display()))?;
        std::fs::rename(&tmp, &p).with_context(|| format!("could not write {}", p.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_keys_take_the_defaults() {
        let p: Preferences = serde_json::from_str("{}").unwrap();
        assert!(p.open_viewer_on_session);
        let p: Preferences = serde_json::from_str(r#"{"open_viewer_on_session": false, "later": 1}"#).unwrap();
        assert!(!p.open_viewer_on_session);
    }
}
