//! A connection to a device's daemon, started on demand.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::{
    control::Source,
    transport,
    wire::{self, Message},
};

/// Requests waiting for their answer by id; `None` once the connection is gone, so a request made
/// after that fails at once instead of waiting for an answer that can't come.
type Pending = Arc<Mutex<Option<HashMap<u64, oneshot::Sender<Value>>>>>;

pub struct Client {
    pub udid: String,
    source: Source,
    /// The agent session the requests belong to (`session start`), sent with each one.
    session: Option<String>,
    next_id: AtomicU64,
    pending: Pending,
    out: mpsc::Sender<Message>,
    events: Option<mpsc::Receiver<Message>>,
    _reader: tokio::task::JoinHandle<()>,
    _writer: tokio::task::JoinHandle<()>,
}

/// The `devicesteer` binary that runs daemons: `DEVICESTEER_BIN`, this executable when it is the
/// CLI, the CLI next to this executable (the viewer ships with it), or `devicesteer` on PATH.
fn cli_binary() -> PathBuf {
    if let Some(p) = std::env::var_os("DEVICESTEER_BIN") {
        return PathBuf::from(p);
    }
    let exe = std::env::current_exe().ok();
    let name = if cfg!(windows) { "devicesteer.exe" } else { "devicesteer" };
    if let Some(exe) = &exe {
        if exe.file_name().is_some_and(|n| n == name) {
            return exe.clone();
        }
        if let Some(sibling) = exe.parent().map(|d| d.join(name))
            && sibling.exists()
        {
            return sibling;
        }
    }
    PathBuf::from(name)
}

fn daemon_log(udid: &str) -> PathBuf {
    devicesteer_core::paths::state_dir().join(format!("{udid}.daemon.log"))
}

fn spawn_daemon(udid: &str) -> Result<std::process::Child> {
    let mut cmd = std::process::Command::new(cli_binary());
    cmd.args(["daemon", "--udid", udid])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(daemon_log(udid))?);
    // Its own process group, so closing the terminal that started it does not end it.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    // The same on Windows, where it also needs no console of its own: started from the viewer
    // (a GUI process), the console-subsystem CLI would otherwise open a console window.
    #[cfg(windows)]
    {
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        std::os::windows::process::CommandExt::creation_flags(&mut cmd, DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        keep_std_handles_from_children();
    }
    cmd.spawn().with_context(|| format!("could not start the daemon ({})", cli_binary().display()))
}

/// Stops children from inheriting this process's stdin, stdout and stderr (PITFALLS #50).
///
/// Windows hands a child every inheritable handle its parent has, not just the three it is given,
/// and the standard handles a process gets are inheritable. The daemon outlives the CLI, so it held
/// the write end of the pipe the CLI's output went to, and whatever read that pipe (`| jq`, an
/// agent's shell tool) waited for the daemon to exit. Rust's own spawns don't need the flag: they
/// duplicate the handles they pass.
#[cfg(windows)]
fn keep_std_handles_from_children() {
    use windows_sys::Win32::{
        Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation},
        System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
    };
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: only clears a flag on our own handles; a missing one (a GUI process) fails harmlessly.
        unsafe {
            let h = GetStdHandle(which);
            if !h.is_null() {
                SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

/// Why the daemon didn't come up, from its log: the error it printed on the way out (the CLI
/// prints errors as `{"error": ...}`), or else the log's last line.
fn startup_error(log: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(log).ok()?;
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let error = lines.clone().rev().find_map(|l| {
        serde_json::from_str::<Value>(l).ok()?.get("error")?.as_str().map(str::to_owned)
    });
    error.or_else(|| lines.next_back().map(str::to_owned))
}

impl Client {
    /// Connects to the daemon for `udid` (or the only device), starting it if it isn't running.
    pub async fn connect(udid: Option<&str>, source: Source) -> Result<Self> {
        let udid = devicesteer_core::resolve_udid(udid).await?;
        let stream = match transport::connect(&udid).await {
            Ok(s) => s,
            Err(_) => {
                // No daemon yet: refuse a device it couldn't serve before starting one, so the
                // caller gets the reason rather than a daemon that fails every request.
                devicesteer_core::ensure_supported(Some(&udid)).await?;
                let mut child = spawn_daemon(&udid)?;
                let log = daemon_log(&udid);
                let began = Instant::now();
                loop {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    if let Ok(s) = transport::connect(&udid).await {
                        break s;
                    }
                    // It exited: say why instead of waiting out the timeout. (Another client may
                    // have started a daemon first, which makes ours exit; then connect again.)
                    if let Ok(Some(status)) = child.try_wait() {
                        if let Ok(s) = transport::connect(&udid).await {
                            break s;
                        }
                        let why = startup_error(&log).unwrap_or_else(|| format!("it exited with {status}"));
                        bail!("the daemon could not start: {why} (log: {})", log.display());
                    }
                    if began.elapsed() > Duration::from_secs(5) {
                        let last = startup_error(&log).map(|l| format!("; its log ends with: {l}")).unwrap_or_default();
                        bail!("the daemon did not come up within 5 s{last} (log: {})", log.display());
                    }
                }
            }
        };
        let (mut rd, mut wr) = tokio::io::split(stream);
        let pending: Pending = Arc::new(Mutex::new(Some(HashMap::new())));
        let (ev_tx, ev_rx) = mpsc::channel::<Message>(1024);
        let p = pending.clone();
        let reader = tokio::spawn(async move {
            while let Ok(Some(m)) = wire::read(&mut rd).await {
                let id = match &m {
                    Message::Json(v) if v.get("event").is_none() => v.get("id").and_then(Value::as_u64),
                    _ => None,
                };
                match id.and_then(|id| p.lock().unwrap().as_mut().and_then(|w| w.remove(&id))) {
                    Some(tx) => {
                        if let Message::Json(v) = m {
                            let _ = tx.send(v);
                        }
                    }
                    None => {
                        // Events: drop them when nobody reads fast enough.
                        let _ = ev_tx.try_send(m);
                    }
                }
            }
            // Wake every waiter, and refuse later requests: the connection is gone (the daemon
            // exited or was stopped). A viewer kept its client and its next request waited forever.
            p.lock().unwrap().take();
        });
        let (out_tx, mut out_rx) = mpsc::channel::<Message>(64);
        let writer = tokio::spawn(async move {
            while let Some(m) = out_rx.recv().await {
                if wire::write(&mut wr, &m).await.is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            udid,
            source,
            session: None,
            next_id: AtomicU64::new(1),
            pending,
            out: out_tx,
            events: Some(ev_rx),
            _reader: reader,
            _writer: writer,
        })
    }

    /// Sends one command and returns its result, or its error as an error.
    pub async fn request(&self, cmd: &str, args: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap()
            .as_mut()
            .ok_or_else(|| anyhow!("the daemon connection closed"))?
            .insert(id, tx);
        let msg = json!({ "id": id, "cmd": cmd, "args": args, "source": self.source, "session": self.session });
        self.out
            .send(Message::Json(msg))
            .await
            .map_err(|_| anyhow!("the daemon connection closed"))?;
        let reply = rx.await.map_err(|_| anyhow!("the daemon connection closed before it answered"))?;
        if reply.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(reply.get("result").cloned().unwrap_or(Value::Null))
        } else {
            bail!("{}", reply.get("error").and_then(Value::as_str).unwrap_or("the daemon returned an error"))
        }
    }

    /// Sends the following requests as part of agent session `id`.
    pub fn with_session(mut self, id: Option<String>) -> Self {
        self.session = id.filter(|s| !s.is_empty());
        self
    }

    /// The events (and frames, after `subscribe` with frames) the daemon pushes. Once only.
    pub fn take_events(&mut self) -> Option<mpsc::Receiver<Message>> {
        self.events.take()
    }
}

impl Drop for Client {
    /// Closes the connection. Dropping a task's handle doesn't end the task, and the reader holds
    /// the socket's read half: without this a dropped client stayed connected, and the daemon
    /// kept counting a viewer that was gone (PITFALLS #23).
    fn drop(&mut self) {
        self._reader.abort();
        self._writer.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::startup_error;

    #[test]
    fn startup_error_prefers_the_printed_error() {
        let dir = std::env::temp_dir().join(format!("devicesteer-startup-error-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("daemon.log");
        std::fs::write(&log, "2026-01-01 INFO starting\n{\"error\":\"no device with UDID x is connected\"}\n\n").unwrap();
        assert_eq!(startup_error(&log).as_deref(), Some("no device with UDID x is connected"));
        std::fs::write(&log, "INFO starting\nthread 'main' has overflowed its stack\n").unwrap();
        assert_eq!(startup_error(&log).as_deref(), Some("thread 'main' has overflowed its stack"));
        std::fs::write(&log, "").unwrap();
        assert_eq!(startup_error(&log), None);
        assert_eq!(startup_error(&dir.join("missing.log")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
