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

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

pub struct Client {
    pub udid: String,
    source: Source,
    next_id: AtomicU64,
    pending: Pending,
    out: mpsc::Sender<Message>,
    events: Option<mpsc::Receiver<Message>>,
    _reader: tokio::task::JoinHandle<()>,
    _writer: tokio::task::JoinHandle<()>,
}

/// The `iphone-use` binary that runs daemons: `IPHONE_USE_BIN`, this executable when it is the
/// CLI, the CLI next to this executable (the viewer ships with it), or `iphone-use` on PATH.
fn cli_binary() -> PathBuf {
    if let Some(p) = std::env::var_os("IPHONE_USE_BIN") {
        return PathBuf::from(p);
    }
    let exe = std::env::current_exe().ok();
    let name = if cfg!(windows) { "iphone-use.exe" } else { "iphone-use" };
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

fn spawn_daemon(udid: &str) -> Result<()> {
    let log = iphone_use_core::paths::state_dir().join(format!("{udid}.daemon.log"));
    let mut cmd = std::process::Command::new(cli_binary());
    cmd.args(["daemon", "--udid", udid])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(&log)?);
    // Its own process group, so closing the terminal that started it does not end it.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    cmd.spawn().with_context(|| format!("could not start the daemon ({})", cli_binary().display()))?;
    Ok(())
}

impl Client {
    /// Connects to the daemon for `udid` (or the only device), starting it if it isn't running.
    pub async fn connect(udid: Option<&str>, source: Source) -> Result<Self> {
        let udid = iphone_use_core::resolve_udid(udid).await?;
        let stream = match transport::connect(&udid).await {
            Ok(s) => s,
            Err(_) => {
                spawn_daemon(&udid)?;
                let began = Instant::now();
                loop {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    if let Ok(s) = transport::connect(&udid).await {
                        break s;
                    }
                    if began.elapsed() > Duration::from_secs(5) {
                        let log = iphone_use_core::paths::state_dir().join(format!("{udid}.daemon.log"));
                        bail!("the daemon did not come up within 5 s; see {}", log.display());
                    }
                }
            }
        };
        let (mut rd, mut wr) = tokio::io::split(stream);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (ev_tx, ev_rx) = mpsc::channel::<Message>(1024);
        let p = pending.clone();
        let reader = tokio::spawn(async move {
            while let Ok(Some(m)) = wire::read(&mut rd).await {
                let id = match &m {
                    Message::Json(v) if v.get("event").is_none() => v.get("id").and_then(Value::as_u64),
                    _ => None,
                };
                match id.and_then(|id| p.lock().unwrap().remove(&id)) {
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
            // Wake every waiter: the connection is gone.
            p.lock().unwrap().clear();
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
        self.pending.lock().unwrap().insert(id, tx);
        let msg = json!({ "id": id, "cmd": cmd, "args": args, "source": self.source });
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

    /// The events (and frames, after `subscribe` with frames) the daemon pushes. Once only.
    pub fn take_events(&mut self) -> Option<mpsc::Receiver<Message>> {
        self.events.take()
    }
}
