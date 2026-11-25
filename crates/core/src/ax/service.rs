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
    RsdService,
    core_device::ScreenCaptureServiceClient,
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
/// Some screens answer only 3 to 4.2 s after a request, and re-asking every 2.5 s restarted their
/// quiet period forever (PITFALLS #19). So each further re-ask waits twice as long as the last,
/// and the first one waits at least this much longer than the last answer took. Re-asks go out on
/// a new connection (PITFALLS #25).
const REASK_GROWTH: f64 = 2.0;
const REASK_MARGIN: Duration = Duration::from_millis(1000);
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
    /// Any message came on this connection. A working service greets a new connection at once
    /// (`_notifyOfPublishedCapabilities:`, `hostAPIVersion`); a stuck one sends nothing
    /// (PITFALLS #25).
    heard: bool,
    closed: bool,
}

/// The device's accessibility daemon took a new connection and sent nothing back at all, not
/// even its greeting (PITFALLS #25). [`crate::Controller`] restarts it.
#[derive(Debug)]
pub struct AxUnresponsive;

impl std::fmt::Display for AxUnresponsive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not worded as a connection failure: the tunnel is fine, and the daemon would drop it.
        f.write_str("the device's accessibility service (axauditd) answers nothing")
    }
}

impl std::error::Error for AxUnresponsive {}

/// A working service greets a new connection within milliseconds (PITFALLS #25).
const GREETING_WITHIN: Duration = Duration::from_secs(3);

/// One DTX connection to the service, with the task that reads it.
struct Conn {
    ask: mpsc::UnboundedSender<()>,
    state: Arc<Mutex<State>>,
    changed: watch::Receiver<u64>,
    task: tokio::task::JoinHandle<()>,
    opened_at: Instant,
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// One subscription to the accessibility tree.
pub struct AxClient {
    adapter: AdapterHandle,
    port: u16,
    conn: Conn,
    asked_at: Instant,
    /// How long the last answer took after the request before it (PITFALLS #19).
    latency: Duration,
}

async fn subscribe(dtx: &mut RemoteServerClient<Box<dyn ReadWrite>>) -> Result<()> {
    dtx.call_method(0, Some(SUBSCRIBE), Some(vec![AuxValue::archived_value(true)]), false)
        .await
        .map_err(|e| anyhow!("{SUBSCRIBE} failed (connection error: {e:?})"))
}

impl Conn {
    /// Connects and sends the first request.
    async fn open(adapter: &AdapterHandle, port: u16) -> Result<Self> {
        let mut adapter = adapter.clone();
        let stream = crate::step("accessibility service connection", adapter.connect(port)).await?;
        let mut dtx = RemoteServerClient::new(stream);
        subscribe(&mut dtx).await?;
        let opened_at = Instant::now();

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
                        st.lock().unwrap().heard = true;
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
        Ok(Self { ask: ask_tx, state, changed: changed_rx, task, opened_at })
    }
}

/// Why [`SERVICE`] isn't there. It comes with the Developer Disk Image, but not every OS has it:
/// iPadOS 17 with the image mounted had only `axAuditDaemon.remoteserver.shim.remote`
/// (PITFALLS #65), so blaming the image there sent the user to a step that was already done.
fn missing_service(rsd: &RsdHandshake) -> anyhow::Error {
    // The screen capture service comes with the same image.
    let mounted = rsd.services.contains_key(ScreenCaptureServiceClient::rsd_service_name().as_ref());
    if mounted {
        anyhow!(
            "the device does not advertise {SERVICE} although the Developer Disk Image is mounted: DeviceSteer \
             reads the screen's elements only on iOS 27 or later. Use `screenshot` for this screen; if the \
             device runs iOS 27 or later, restart it, run `devicesteer setup` and retry"
        )
    } else {
        anyhow!(
            "the device does not advertise {SERVICE}: the Developer Disk Image is not mounted (a restart can \
             unmount it). Run `devicesteer setup`, then retry"
        )
    }
}

impl AxClient {
    /// Connects and sends the first request.
    pub async fn open(adapter: &AdapterHandle, rsd: &RsdHandshake) -> Result<Self> {
        let port = rsd.services.get(SERVICE).ok_or_else(|| missing_service(rsd))?.port;
        let conn = Conn::open(adapter, port).await?;
        let asked_at = conn.opened_at;
        Ok(Self { adapter: adapter.clone(), port, conn, asked_at, latency: Duration::ZERO })
    }

    /// Asks again on a new connection. A subscription can go silent for good while its
    /// connection stays open (VoiceOver switched on or off, PITFALLS #25), and a request on a new
    /// connection costs the same as one on the old.
    async fn reconnect(&mut self) -> Result<()> {
        tracing::debug!("ax: request on a new connection");
        self.conn = Conn::open(&self.adapter, self.port).await?;
        self.asked_at = self.conn.opened_at;
        Ok(())
    }

    fn request(&mut self) -> Result<()> {
        self.conn
            .ask
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
        let mut reask = REASK_AFTER.max(self.latency + REASK_MARGIN);
        let seq = loop {
            let earliest = self.asked_at + ANSWER_AT_LEAST;
            let greeting_due = {
                let s = self.conn.state.lock().unwrap();
                if s.closed {
                    bail!("accessibility connection closed");
                }
                if let Some(at) = s.initial_at.filter(|at| *at >= earliest) {
                    self.latency = at.saturating_duration_since(self.asked_at);
                    break s.seq;
                }
                (!s.heard).then_some(self.conn.opened_at + GREETING_WITHIN)
            };
            let now = Instant::now();
            if greeting_due.is_some_and(|due| now >= due) {
                return Err(AxUnresponsive.into());
            }
            if now >= deadline {
                let unparsed = self.conn.state.lock().unwrap().unparsed;
                bail!(
                    "no accessibility dump within {} s{}. The screen may be animating or locked; \
                     check it with `screenshot` and retry",
                    timeout.as_secs(),
                    if unparsed > 0 { format!(" ({unparsed} payloads not understood)") } else { String::new() }
                );
            }
            if now >= self.asked_at + reask {
                self.reconnect().await?;
                reask = reask.mul_f64(REASK_GROWTH);
                continue;
            }
            let mut until = deadline.min(self.asked_at + reask);
            if let Some(due) = greeting_due {
                until = until.min(due);
            }
            let wait = until.saturating_duration_since(now);
            let _ = tokio::time::timeout(wait.max(Duration::from_millis(10)), self.conn.changed.changed()).await;
        };
        if full && let Some(d) = self.second_pass(seq).await {
            return Ok(d);
        }
        let s = self.conn.state.lock().unwrap();
        Ok(s.initial.clone().expect("a dump arrived above"))
    }

    /// The newest reading with its second pass (identifiers) merged in, if that pass comes.
    /// Sends no request.
    pub async fn complete(&mut self) -> Option<Arc<Dump>> {
        let seq = self.conn.state.lock().unwrap().seq;
        self.second_pass(seq).await
    }

    async fn second_pass(&mut self, seq: u64) -> Option<Arc<Dump>> {
        // The second pass came 0.5 s after the first on most screens, 3.2 s on a slow one.
        let until = Instant::now() + Duration::from_millis(2500).max(self.latency + REASK_MARGIN);
        loop {
            {
                let s = self.conn.state.lock().unwrap();
                if s.full_seq == seq && let Some(d) = s.full.clone() {
                    return Some(d);
                }
                if s.seq > seq || s.closed {
                    return None;
                }
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() || tokio::time::timeout(left, self.conn.changed.changed()).await.is_err() {
                return None;
            }
        }
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
