//! The accessibility tree over DTX (`com.apple.accessibility.axAuditDaemon.remoteAXService`).
//!
//! `clientNeedsAccessibility:` asks for a dump of the screen; it arrives as
//! `processDataFromRemoteDevice:` in two passes (labels and frames first, identifiers later).
//! Timing rules (PITFALLS #10) come from iphone-cli `src/device/ax.rs`, MIT, see
//! THIRD_PARTY_NOTICES.md.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail};
use idevice::{
    ReadWrite,
    dvt::{message::AuxValue, remote_server::RemoteServerClient},
    rsd::RsdHandshake,
    tcp::handle::AdapterHandle,
};
use tokio::sync::{mpsc, watch};

use super::dump::{Dump, DumpKind, Payload};

const SERVICE: &str = "com.apple.accessibility.axAuditDaemon.remoteAXService";
const SUBSCRIBE: &str = "clientNeedsAccessibility:";
const PUSH: &str = "processDataFromRemoteDevice:";
/// The device never answers a request sooner than this; a dump that comes earlier answers an
/// older request or was sent unasked.
const ANSWER_AT_LEAST: Duration = Duration::from_millis(700);
/// Every request restarts the device's quiet period, so ask again only after this long. Answers
/// came 1.2 to 1.6 s after the request they answered (PITFALLS #12).
const REASK_AFTER: Duration = Duration::from_millis(2500);
/// After input, a second request this long after the first (PITFALLS #12).
const SECOND_ASK: Duration = Duration::from_millis(800);

#[derive(Default)]
struct State {
    seq: u64,
    initial: Option<Arc<Dump>>,
    initial_at: Option<Instant>,
    full_seq: u64,
    full: Option<Arc<Dump>>,
    unparsed: u64,
    closed: bool,
}

/// One subscription to the accessibility tree.
pub struct AxClient {
    ask: mpsc::UnboundedSender<()>,
    state: Arc<Mutex<State>>,
    changed: watch::Receiver<u64>,
    task: tokio::task::JoinHandle<()>,
    asked_at: Instant,
}

impl Drop for AxClient {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn subscribe(dtx: &mut RemoteServerClient<Box<dyn ReadWrite>>) -> Result<()> {
    dtx.call_method(0, Some(SUBSCRIBE), Some(vec![AuxValue::archived_value(true)]), false)
        .await
        .map_err(|e| anyhow!("{SUBSCRIBE} failed (connection error: {e:?})"))
}

impl AxClient {
    /// Connects and sends the first request.
    pub async fn open(adapter: &AdapterHandle, rsd: &RsdHandshake) -> Result<Self> {
        let port = rsd
            .services
            .get(SERVICE)
            .ok_or_else(|| {
                anyhow!("the device does not advertise {SERVICE}; the Developer Disk Image is probably not mounted")
            })?
            .port;
        let mut adapter = adapter.clone();
        let stream = crate::step("accessibility service connection", adapter.connect(port)).await?;
        let mut dtx = RemoteServerClient::new(stream);
        subscribe(&mut dtx).await?;
        let asked_at = Instant::now();

        let state = Arc::new(Mutex::new(State::default()));
        let (ask_tx, mut ask_rx) = mpsc::unbounded_channel::<()>();
        let (changed_tx, changed_rx) = watch::channel(0u64);
        let st = state.clone();
        let task = tokio::spawn(async move {
            let mut ticks = 0u64;
            loop {
                tokio::select! {
                    ask = ask_rx.recv() => {
                        tracing::debug!("ax: request");
                        if ask.is_none() || subscribe(&mut dtx).await.is_err() {
                            break;
                        }
                    }
                    msg = dtx.read_message(0) => {
                        let Ok(msg) = msg else { break };
                        if !matches!(&msg.data, Some(plist::Value::String(s)) if s == PUSH) {
                            continue;
                        }
                        let Some(AuxValue::Array(bytes)) = msg.aux.and_then(|a| a.values.into_iter().next()) else {
                            continue;
                        };
                        let arrived = Instant::now();
                        // Off the async threads: a large payload must not stall the tunnel.
                        let parsed = tokio::task::spawn_blocking(move || Payload::parse(&bytes)).await;
                        match parsed {
                            Ok(Ok(Payload::Dump(dump))) => {
                                tracing::debug!("ax: dump {:?}, {} elements, {} apps", dump.kind, dump.elements.len(), dump.applications.len());
                                if accept(&st, dump, arrived) {
                                    ticks += 1;
                                    let _ = changed_tx.send(ticks);
                                }
                            }
                            Ok(Ok(_)) => {}
                            Ok(Err(e)) => {
                                tracing::debug!("accessibility payload not understood: {e:#}");
                                st.lock().unwrap().unparsed += 1;
                            }
                            Err(_) => {}
                        }
                    }
                }
            }
            st.lock().unwrap().closed = true;
            let _ = changed_tx.send(ticks + 1);
        });
        Ok(Self { ask: ask_tx, state, changed: changed_rx, task, asked_at })
    }

    fn request(&mut self) -> Result<()> {
        self.ask
            .send(())
            .map_err(|_| anyhow!("accessibility connection closed"))?;
        self.asked_at = Instant::now();
        Ok(())
    }

    /// A fresh reading: the first dump that answers a request made at or after now (or the one
    /// still due from [`Self::open`]). With `full`, also waits up to `full_wait` for the second
    /// pass (identifiers) and merges it.
    ///
    /// `changed`: input went to the device since the last reading, so the screen probably changed.
    pub async fn read(&mut self, timeout: Duration, full: bool, changed: bool) -> Result<Arc<Dump>> {
        // An answer to the request `open` sent is as fresh as a new one, and a new request would
        // only restart the device's quiet period.
        if self.asked_at.elapsed() > ANSWER_AT_LEAST {
            self.request()?;
            if changed {
                // After the screen changed, the device ignores the first request (PITFALLS #12).
                tokio::time::sleep(SECOND_ASK).await;
                self.request()?;
            }
        }
        let deadline = Instant::now() + timeout;
        let seq = loop {
            let earliest = self.asked_at + ANSWER_AT_LEAST;
            {
                let s = self.state.lock().unwrap();
                if s.closed {
                    bail!("accessibility connection closed");
                }
                if s.initial_at.is_some_and(|at| at >= earliest) {
                    break s.seq;
                }
            }
            let now = Instant::now();
            if now >= deadline {
                let unparsed = self.state.lock().unwrap().unparsed;
                bail!(
                    "no accessibility dump within {} s{}. The screen may be animating or locked; \
                     check it with `screenshot` and retry",
                    timeout.as_secs(),
                    if unparsed > 0 { format!(" ({unparsed} payloads not understood)") } else { String::new() }
                );
            }
            if now >= self.asked_at + REASK_AFTER {
                self.request()?;
            }
            let wait = deadline.min(self.asked_at + REASK_AFTER).saturating_duration_since(now);
            let _ = tokio::time::timeout(wait.max(Duration::from_millis(10)), self.changed.changed()).await;
        };
        if full {
            let until = Instant::now() + Duration::from_millis(2500);
            loop {
                {
                    let s = self.state.lock().unwrap();
                    if s.full_seq == seq && let Some(d) = s.full.clone() {
                        return Ok(d);
                    }
                    if s.seq > seq || s.closed {
                        break;
                    }
                }
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() || tokio::time::timeout(left, self.changed.changed()).await.is_err() {
                    break;
                }
            }
        }
        let s = self.state.lock().unwrap();
        Ok(s.initial.clone().expect("a dump arrived above"))
    }
}

/// Stores a dump. Returns false for dumps that carry nothing.
fn accept(state: &Mutex<State>, dump: Dump, arrived: Instant) -> bool {
    if dump.elements.is_empty() || dump.applications.is_empty() {
        return false;
    }
    match dump.kind {
        DumpKind::Initial => {
            let mut s = state.lock().unwrap();
            s.seq += 1;
            s.initial = Some(Arc::new(dump));
            s.initial_at = Some(arrived);
            true
        }
        DumpKind::Additional => {
            let mut s = state.lock().unwrap();
            let Some(initial) = s.initial.clone() else { return false };
            // The second pass belongs to the newest first pass only if it has the same elements.
            if !initial.same_elements(&dump) {
                return false;
            }
            let mut merged = (*initial).clone();
            merged.absorb(&dump);
            s.full = Some(Arc::new(merged));
            s.full_seq = s.seq;
            true
        }
        DumpKind::Other(_) => false,
    }
}
