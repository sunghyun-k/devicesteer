//! The local transport: a Unix socket in the state directory on macOS and Linux, a named pipe
//! (`\\.\pipe\devicesteer-<udid>`) limited to the current user on Windows.

#[cfg(unix)]
use std::path::PathBuf;

use anyhow::Result;

/// Where the daemon for `udid` listens.
#[cfg(unix)]
pub fn socket_path(udid: &str) -> PathBuf {
    let p = devicesteer_core::paths::state_dir().join(format!("{udid}.sock"));
    // Unix socket paths are limited to about 104 bytes on macOS.
    if p.as_os_str().len() < 100 {
        p
    } else {
        std::env::temp_dir().join(format!("devicesteer-{udid}.sock"))
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
    use anyhow::{Context, bail};
    use std::{ffi::c_void, sync::Mutex, time::Duration};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_PIPE_BUSY, HANDLE, LocalFree},
        Security::{
            Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
            GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    pub type Stream = NamedPipeServer;

    /// The pipe for `udid`. Pipe names are machine-wide, so a second user's daemon for the same
    /// device fails to bind instead of sharing it: one process can own a device's tunnel anyway.
    pub fn pipe_name(udid: &str) -> String {
        format!(r"\\.\pipe\devicesteer-{udid}")
    }

    /// A security descriptor that lets only the current user open the pipe. The default one also
    /// lets in LocalSystem, Administrators and, read-only, Everyone; and the pipe takes input to
    /// someone's phone.
    struct OwnerOnly(PSECURITY_DESCRIPTOR);

    impl OwnerOnly {
        fn new() -> Result<Self> {
            let sid = current_user_sid()?;
            // Protected DACL (`P`, no inherited entries) with one entry: generic all for the user.
            let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})").encode_utf16().chain([0]).collect();
            let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            // SAFETY: `sddl` is NUL-terminated; `sd` is freed with LocalFree in Drop.
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut())
            };
            if ok == 0 {
                bail!("could not build the pipe's access list: {}", std::io::Error::last_os_error());
            }
            Ok(Self(sd))
        }

        fn attributes(&self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.0,
                bInheritHandle: 0,
            }
        }
    }

    impl Drop for OwnerOnly {
        fn drop(&mut self) {
            // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
            unsafe { LocalFree(self.0) };
        }
    }

    /// The current user's SID as a string (`S-1-5-21-...`).
    fn current_user_sid() -> Result<String> {
        // SAFETY: plain Win32 calls on our own process token; buffers are sized by the first call
        // and the token handle and the SID string are released before returning.
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                bail!("could not open the process token: {}", std::io::Error::last_os_error());
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
            // u64s, so the buffer is aligned for TOKEN_USER.
            let mut buf = vec![0u64; (len as usize).div_ceil(8)];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
            let err = std::io::Error::last_os_error();
            CloseHandle(token);
            if ok == 0 {
                bail!("could not read the process token: {err}");
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut wide: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut wide) == 0 {
                bail!("could not read the user's SID: {}", std::io::Error::last_os_error());
            }
            let n = (0..).take_while(|&i| *wide.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(wide, n));
            LocalFree(wide as *mut c_void);
            Ok(sid)
        }
    }

    /// A pipe server accepts one client per instance, so the listener keeps the next instance
    /// ready and makes another as each one connects; with none waiting, a client gets "not found"
    /// and would start a second daemon.
    pub struct Listener {
        name: String,
        security: OwnerOnly,
        next: Mutex<Option<NamedPipeServer>>,
    }

    impl Listener {
        fn instance(&self, first: bool) -> std::io::Result<NamedPipeServer> {
            let mut attrs = self.security.attributes();
            let mut opts = ServerOptions::new();
            opts.first_pipe_instance(first).reject_remote_clients(true);
            // SAFETY: `attrs` and the descriptor it points to outlive the call.
            unsafe { opts.create_with_security_attributes_raw(&self.name, (&raw mut attrs).cast()) }
        }

        pub async fn bind(udid: &str) -> Result<Self> {
            let name = pipe_name(udid);
            let mut listener = Self { name, security: OwnerOnly::new()?, next: Mutex::new(None) };
            // The pipe disappears with the last handle to it, so there is nothing stale to clear:
            // if the first instance can't be made, a daemon runs.
            let first = listener.instance(true).map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    anyhow::anyhow!("a daemon for {udid} already runs at {}", listener.name)
                } else {
                    anyhow::Error::from(e).context(format!("could not listen on {}", listener.name))
                }
            })?;
            *listener.next.get_mut().unwrap() = Some(first);
            Ok(listener)
        }

        pub async fn accept(&self) -> Result<Stream> {
            let server = self.next.lock().unwrap().take();
            let server = match server {
                Some(s) => s,
                None => self.instance(false).with_context(|| format!("could not listen on {}", self.name))?,
            };
            server.connect().await?;
            *self.next.lock().unwrap() = Some(self.instance(false).with_context(|| format!("could not listen on {}", self.name))?);
            Ok(server)
        }

        pub fn remove(&self) {}
    }

    pub async fn connect(udid: &str) -> Result<tokio::net::windows::named_pipe::NamedPipeClient> {
        let name = pipe_name(udid);
        // Busy: every instance is taken for the moment (the daemon is between a connect and
        // making the next instance). Waiting briefly beats starting a second daemon.
        for _ in 0..40 {
            match ClientOptions::new().open(&name) {
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                r => return Ok(r?),
            }
        }
        bail!("the daemon's pipe {name} stayed busy")
    }
}

pub use imp::{Listener, connect};
#[cfg(windows)]
pub use imp::pipe_name;
