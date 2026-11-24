//! The local transport: a Unix socket in the state directory on macOS and Linux. Windows will use
//! a named pipe (`\\.\pipe\iphone-use-<udid>`); that side is not written yet.

use std::path::PathBuf;

use anyhow::Result;

/// Where the daemon for `udid` listens.
pub fn socket_path(udid: &str) -> PathBuf {
    let p = iphone_use_core::paths::state_dir().join(format!("{udid}.sock"));
    // Unix socket paths are limited to about 104 bytes on macOS.
    if p.as_os_str().len() < 100 {
        p
    } else {
        std::env::temp_dir().join(format!("iphone-use-{udid}.sock"))
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use anyhow::{Context, bail};
    use tokio::net::{UnixListener, UnixStream};

    pub type Stream = UnixStream;

    pub struct Listener {
        inner: UnixListener,
        path: PathBuf,
    }

    impl Listener {
        pub async fn bind(udid: &str) -> Result<Self> {
            let path = socket_path(udid);
            if path.exists() {
                if UnixStream::connect(&path).await.is_ok() {
                    bail!("a daemon for {udid} already runs at {}", path.display());
                }
                // Left by a daemon that died.
                let _ = std::fs::remove_file(&path);
            }
            let inner = UnixListener::bind(&path).with_context(|| format!("could not listen on {}", path.display()))?;
            Ok(Self { inner, path })
        }

        pub async fn accept(&self) -> Result<Stream> {
            Ok(self.inner.accept().await?.0)
        }

        pub fn remove(&self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    pub async fn connect(udid: &str) -> Result<Stream> {
        Ok(UnixStream::connect(socket_path(udid)).await?)
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use anyhow::bail;

    pub type Stream = tokio::net::TcpStream;

    pub struct Listener;

    impl Listener {
        pub async fn bind(_udid: &str) -> Result<Self> {
            bail!("the daemon's named-pipe transport is not written yet on Windows; run commands without the daemon")
        }

        pub async fn accept(&self) -> Result<Stream> {
            bail!("not available on Windows yet")
        }

        pub fn remove(&self) {}
    }

    pub async fn connect(_udid: &str) -> Result<Stream> {
        bail!("the daemon's named-pipe transport is not written yet on Windows")
    }
}

pub use imp::{Listener, connect};
