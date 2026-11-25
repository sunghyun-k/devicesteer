//! The local transport: a Unix socket in the state directory on macOS and Linux, a named pipe
//! (`\\.\pipe\devicesteer-<udid>`) limited to the current user on Windows.

#[cfg(unix)]
use std::path::PathBuf;

use anyhow::Result;

/// Where the daemon for `udid` listens.
#[cfg(unix)]
pub fn socket_path(udid: &str) -> PathBuf {
    socket_path_in(&devicesteer_core::paths::state_dir(), &std::env::temp_dir(), udid)
}

/// The longest socket path `bind` takes: `sun_path` holds 104 bytes on macOS and 108 on Linux,
/// with the terminating NUL.
#[cfg(unix)]
const MAX_SOCKET_PATH: usize = if cfg!(target_os = "macos") { 103 } else { 107 };

/// The first of these that fits in [`MAX_SOCKET_PATH`]:
///
/// 1. `<state dir>/<udid>.sock` and 2. `<temp dir>/devicesteer-<udid>.sock`: the paths earlier
///    builds used, in their order, so a daemon one of them started is still found and none starts
///    a second daemon (which would fight it for the device's tunnel).
/// 3. `<state dir>/<hash>.sock` and 4. `<temp dir>/devicesteer-<hash>.sock`, with a short hash of
///    the UDID. A 40-character UDID (devices from before 2018) made both of the first two too long
///    on macOS, so the daemon couldn't listen at all.
///
/// Both directories belong to the user: the state dir is in their home, and macOS's temp dir is
/// per user (mode 700). The CLI and the viewer both call this, so they agree on the path.
#[cfg(unix)]
fn socket_path_in(state: &std::path::Path, temp: &std::path::Path, udid: &str) -> PathBuf {
    let hash = format!("{:016x}", fnv1a(udid.as_bytes()));
    let candidates = [
        // Earlier builds took this one only below 100 bytes; keep that so they agree.
        Some(state.join(format!("{udid}.sock"))).filter(|p| p.as_os_str().len() < 100),
        Some(temp.join(format!("devicesteer-{udid}.sock"))),
        Some(state.join(format!("{hash}.sock"))),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|p| p.as_os_str().len() <= MAX_SOCKET_PATH)
        .unwrap_or_else(|| temp.join(format!("devicesteer-{hash}.sock")))
}

/// FNV-1a, 64-bit: stable across builds and Rust versions (std's hasher promises neither), so a
/// CLI and a viewer built apart still find the same socket.
#[cfg(unix)]
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::Path;

    // A typical home and the per-user temp dir macOS gives (`$TMPDIR`).
    const STATE: &str = "/Users/someone/Library/Application Support/devicesteer";
    const TEMP: &str = "/var/folders/wx/t_gxtcwd4rxgxftr41tfkj2r0000gn/T/";
    const OLD_UDID: &str = "0123456789abcdef0123456789abcdef01234567";
    const NEW_UDID: &str = "00008140-001A2B3C4D5E6F70";

    #[test]
    fn a_40_character_udid_fits() {
        let p = socket_path_in(Path::new(STATE), Path::new(TEMP), OLD_UDID);
        assert!(p.as_os_str().len() <= MAX_SOCKET_PATH, "{} is {} bytes", p.display(), p.as_os_str().len());
        assert!(p.starts_with(STATE));
        // The same every time, and not another device's.
        assert_eq!(p, socket_path_in(Path::new(STATE), Path::new(TEMP), OLD_UDID));
        assert_ne!(p, socket_path_in(Path::new(STATE), Path::new(TEMP), &OLD_UDID.replace('7', "8")));
        // Even with a long home, in the temp dir.
        let long = format!("/Users/{}/Library/Application Support/devicesteer", "x".repeat(60));
        let p = socket_path_in(Path::new(&long), Path::new(TEMP), OLD_UDID);
        assert!(p.as_os_str().len() <= MAX_SOCKET_PATH && p.starts_with(TEMP));
    }

    #[test]
    fn paths_that_fit_before_stay_the_same() {
        let p = socket_path_in(Path::new(STATE), Path::new(TEMP), NEW_UDID);
        assert_eq!(p, Path::new(STATE).join(format!("{NEW_UDID}.sock")));
        let long = format!("/Users/{}/Library/Application Support/devicesteer", "x".repeat(40));
        let p = socket_path_in(Path::new(&long), Path::new(TEMP), NEW_UDID);
        assert_eq!(p, Path::new(TEMP).join(format!("devicesteer-{NEW_UDID}.sock")));
    }

    #[test]
    fn fnv1a_is_the_published_function() {
        // Test vectors from the FNV reference.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
