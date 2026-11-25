//! Screen-sharing media streams (`com.apple.coredevice.displayservice`).
//!
//! The device drops synthetic HID input unless one of these streams runs (PITFALLS #7), and the
//! video stream is the live screen. Each [`MediaSession`] starts its streams under its own client
//! session id and stops only those, by the `RemoteSSRC` the device names them by. It never sends
//! `stopAll`, which ends other sessions' streams and has wedged the media daemon (PITFALLS #1).
//!
//! Adapted from iphone-cli `src/device/media.rs` and the media part of `src/device/input.rs`
//! (MIT, see THIRD_PARTY_NOTICES.md), minus its `stopAll` fallback.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use idevice::{
    RsdService,
    core_device::{
        CallInfoBlob, build_screen_audio_offer, build_screen_video_offer,
        build_start_audio_parameters, build_start_video_parameters,
    },
    rsd::RsdHandshake,
    tcp::handle::{AdapterHandle, UdpSocketHandle},
    xpc::{Dictionary, XPCObject},
};
use serde::Serialize;
use tokio::sync::broadcast;

use crate::{
    coredevice::{error_code, xpc_link},
    paths, step,
};

xpc_link!(DisplayLink, "com.apple.coredevice.displayservice");

const FEATURE_START: &str = "com.apple.coredevice.feature.startmediastream";
const FEATURE_STOP: &str = "com.apple.coredevice.feature.stopmediastream";
const ACTION_STOP: &str = "com.apple.coredevice.action.mediastreamstop";
const FEATURE_STATUS: &str = "com.apple.coredevice.feature.getmediastreamserverstatus";
/// What the client says it supports. Xcode's device window sends 140.
const CLIENT_SUPPORTED_FEATURES: u64 = 140;
/// The device's ports the offers name; it sends from these.
const AUDIO_SENDER_PORT: u16 = 50000;
const VIDEO_SENDER_PORT: u16 = 50001;
/// "The device microphone or camera is currently in use" (a call, a screen recording): the device
/// refuses the audio stream, and the video stream alone has to set up the screen-sharing session.
const ERROR_MEDIA_IN_USE: i64 = 9022;
/// How long the device may go on listing streams it was told to stop.
const STOPPING: Duration = Duration::from_millis(1500);
/// Longest wait for one session's streams to stop.
const STOP_WITHIN: Duration = Duration::from_secs(5);
/// Longest wait for the first media packet after starting.
const FIRST_PACKET: Duration = Duration::from_millis(2500);

/// A packet received on one of the session's UDP sockets.
#[derive(Debug, Clone)]
pub struct MediaPacket {
    pub kind: StreamKind,
    pub data: Arc<Vec<u8>>,
    /// When it arrived on the host.
    pub at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamKind {
    Audio,
    Video,
}

/// The display service's control connection.
struct MediaControl {
    adapter: AdapterHandle,
    rsd: RsdHandshake,
    /// Taken while a request is on it. A request cut off by a timeout leaves its answer on the
    /// connection for the next request to misread, so such a connection is never put back.
    link: Option<DisplayLink>,
}

impl MediaControl {
    async fn connect(adapter: &AdapterHandle, rsd: &RsdHandshake) -> Result<Self> {
        let mut ctl = Self { adapter: adapter.clone(), rsd: rsd.clone(), link: None };
        let link = ctl.open_link().await?;
        ctl.link = Some(link);
        Ok(ctl)
    }

    async fn open_link(&mut self) -> Result<DisplayLink> {
        step(
            "display service connection",
            DisplayLink::connect_rsd(&mut self.adapter, &mut self.rsd),
        )
        .await
    }

    async fn invoke(&mut self, feature: &str, input: Dictionary, action: Option<&str>) -> Result<plist::Value> {
        let mut link = match self.link.take() {
            Some(l) => l,
            None => self.open_link().await?,
        };
        let out = link.invoke(feature, input, action).await;
        // A CoreDevice error is a whole answer; only a transport error leaves the link unusable.
        if out.is_ok() || error_code(out.as_ref().unwrap_err()).is_some() {
            self.link = Some(link);
        }
        out
    }

    async fn status(&mut self) -> Result<plist::Value> {
        self.invoke(FEATURE_STATUS, Dictionary::new(), None).await
    }

    /// Stops the streams the device runs for client session `id`, and no other session's.
    async fn stop_session(&mut self, id: &str) -> Result<()> {
        let mut asked = false;
        let mut settling = Instant::now();
        loop {
            let status = self.status().await?;
            match next_stop(&status, id, asked) {
                Stop::Done => return Ok(()),
                _ if Instant::now() < settling => tokio::time::sleep(Duration::from_millis(100)).await,
                Stop::Named(ssrcs) => {
                    let mut input = Dictionary::new();
                    input.insert("stopAll".into(), XPCObject::Bool(false));
                    // The device decodes each as a UInt32 and refuses an Int64.
                    input.insert(
                        "identifiers".into(),
                        XPCObject::Array(ssrcs.iter().map(|s| XPCObject::UInt64(u64::from(*s))).collect()),
                    );
                    let stopped = self.invoke(FEATURE_STOP, input, Some(ACTION_STOP)).await;
                    tracing::debug!("media: stop {ssrcs:?} of session {id}: {:?}", stopped.as_ref().map(|_| ()));
                    asked = true;
                    settling = Instant::now() + if stopped.is_ok() { STOPPING } else { Duration::ZERO };
                }
                Stop::Kept(why) => bail!("the device kept the streams of media session {id}: {why}"),
            }
        }
    }
}

/// The next step in stopping one client session's streams, from what the device lists.
#[derive(Debug, PartialEq)]
enum Stop {
    Done,
    /// Stop these by `RemoteSSRC`.
    Named(Vec<u32>),
    /// They can't be stopped by name. `stopAll` would end other sessions' streams and has wedged
    /// the media daemon (PITFALLS #1), so this is an error, not a fallback.
    Kept(String),
}

fn next_stop(status: &plist::Value, id: &str, asked: bool) -> Stop {
    if sessions(status).is_none() {
        return Stop::Kept("the server status has no session list to find them in".into());
    }
    let ours = streams_of(status, id);
    if ours.is_empty() {
        return Stop::Done;
    }
    if asked {
        return Stop::Kept(format!("{} stream(s) still listed after stopping them by name", ours.len()));
    }
    match ours.iter().copied().collect::<Option<Vec<u32>>>() {
        Some(ssrcs) => Stop::Named(ssrcs),
        None => Stop::Kept("a stream has no RemoteSSRC to stop it by".into()),
    }
}

fn sessions(status: &plist::Value) -> Option<&Vec<plist::Value>> {
    status.as_dictionary()?.get("sessions")?.as_array()
}

/// The `RemoteSSRC` of a stream, the name the device stops it by.
fn remote_ssrc(stream: &plist::Value) -> Option<u32> {
    let config = stream
        .as_dictionary()?
        .get("connection")?
        .as_dictionary()?
        .get("streamConfig")?
        .as_dictionary()?;
    u32::try_from(config.get("RemoteSSRC")?.as_unsigned_integer()?).ok()
}

/// The names of the streams the device runs for client session `id`.
fn streams_of(status: &plist::Value, id: &str) -> Vec<Option<u32>> {
    let client = |s: &plist::Value| -> Option<String> {
        let options = s.as_dictionary()?.get("connection")?.as_dictionary()?.get("options")?.as_dictionary()?;
        let uuid = options.get("avcMediaStreamOptionClientSessionID")?.as_dictionary()?.get("uuid")?;
        uuid.as_string().map(str::to_lowercase)
    };
    sessions(status)
        .into_iter()
        .flatten()
        .filter(|s| client(s).as_deref() == Some(&id.to_lowercase()))
        .map(remote_ssrc)
        .collect()
}

/// How many streams the device runs, of any session.
fn stream_count(status: &plist::Value) -> usize {
    sessions(status).map_or(0, Vec::len)
}

/// What the client says it is, as Xcode's device window does.
fn call_info() -> CallInfoBlob {
    CallInfoBlob {
        call_id: 0,
        client_version: 1,
        device_type: "Mac17,7".into(),
        framework_version: "2205.3.1".into(),
        os_version: "25F71".into(),
        device_name: None,
        audio_device_uid: None,
    }
}

// Session ids whose streams may still run on a device, one per line as `<id> <pid>`. A process
// killed before it stopped its streams leaves them running for their RTCP timeout (an hour), with
// the device encoding into nowhere; the next session stops them. Only sessions whose process is
// gone count as leftovers: a recording in another process is alive, and stopping its streams
// ends it (PITFALLS #11).

fn marked(marker: &Path) -> Vec<(String, Option<u32>)> {
    std::fs::read_to_string(marker)
        .map(|s| {
            s.lines()
                .filter_map(|l| {
                    let mut parts = l.split_whitespace();
                    let id = parts.next()?.to_string();
                    Some((id, parts.next().and_then(|p| p.parse().ok())))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_marked(marker: &Path, entries: &[(String, Option<u32>)]) {
    if entries.is_empty() {
        let _ = std::fs::remove_file(marker);
        return;
    }
    let text: String = entries
        .iter()
        .map(|(id, pid)| match pid {
            Some(p) => format!("{id} {p}\n"),
            None => format!("{id}\n"),
        })
        .collect();
    let _ = std::fs::write(marker, text);
}

fn mark(marker: &Path, id: &str) {
    let mut entries = marked(marker);
    if !entries.iter().any(|(m, _)| m == id) {
        entries.push((id.to_string(), Some(std::process::id())));
    }
    write_marked(marker, &entries);
}

fn unmark(marker: &Path, id: &str) {
    let entries: Vec<_> = marked(marker).into_iter().filter(|(m, _)| m != id).collect();
    write_marked(marker, &entries);
}

/// Whether process `pid` still runs. Unknown counts as running, so its streams are left alone.
fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else { return true };
        // Signal 0 checks existence; EPERM means it exists under another user.
        // SAFETY: kill with signal 0 sends nothing.
        let r = unsafe { libc::kill(pid, 0) };
        r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, GetLastError},
            System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        };
        // What GetExitCodeProcess reports for a process that hasn't exited (STILL_ACTIVE). A
        // process that exited with this very code reads as running, which only leaves its
        // streams alone.
        const STILL_ACTIVE: u32 = 259;
        // SAFETY: plain Win32 calls; the handle is closed before returning.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                // No such process; any other failure (access denied: another user's) means it
                // exists.
                return GetLastError() != ERROR_INVALID_PARAMETER;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code) != 0;
            CloseHandle(h);
            !ok || code == STILL_ACTIVE
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

/// What a session asks the device to stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaMode {
    /// Audio only, the packets thrown away: enough for HID input, and light on the tunnel.
    InputOnly,
    /// Audio and video: the live screen.
    Video,
}

/// Packet counters per stream, for status and the first-packet wait.
#[derive(Debug, Default)]
struct Counters {
    first_ms: AtomicU64,
    last_ms: AtomicU64,
    packets: AtomicU64,
    bytes: AtomicU64,
}

/// One stream's numbers.
#[derive(Debug, Clone, Serialize)]
pub struct StreamStatus {
    pub kind: StreamKind,
    pub packets: u64,
    pub bytes: u64,
    /// Milliseconds since the last packet, if any arrived.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_ms: Option<u64>,
}

/// The state of a [`MediaSession`].
#[derive(Debug, Clone, Serialize)]
pub struct MediaStatus {
    pub session_id: String,
    pub streams: Vec<StreamStatus>,
    /// Streams the device runs for any client, ours included.
    pub device_streams: usize,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The streams one client session started, and the tasks reading them.
pub struct MediaSession {
    ctl: MediaControl,
    id: uuid::Uuid,
    marker: PathBuf,
    streams: Vec<(StreamKind, Arc<Counters>)>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    packets: broadcast::Sender<MediaPacket>,
    /// A receiver made before the streams started, so the first consumer also gets the stream's
    /// first packets (the only keyframe the device sends unasked, PITFALLS #11).
    early: Option<broadcast::Receiver<MediaPacket>>,
    /// Whether video runs (asked for, or forced by an audio refusal).
    pub video: bool,
    /// The video socket, to send RTCP back on, and the SSRC our offer named.
    video_rtcp: Option<(Arc<UdpSocketHandle>, u32)>,
    /// Incremented per keyframe request (RTCP FIR sequence number).
    fir_seq: u8,
    /// The display the video shows.
    pub display_id: i64,
    stopped: bool,
}

impl Drop for MediaSession {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
        if !self.stopped {
            tracing::warn!("media session {} dropped without stop; the next session stops its streams", self.id);
        }
    }
}

impl MediaSession {
    /// Starts the streams for `mode` and waits briefly for the first packets.
    ///
    /// `display_id` is the display the video shows (from `getdisplayinfo`).
    pub async fn start(
        udid: &str,
        adapter: &AdapterHandle,
        rsd: &RsdHandshake,
        mode: MediaMode,
        display_id: i64,
    ) -> Result<Self> {
        let mut ctl = MediaControl::connect(adapter, rsd).await?;
        let marker = paths::media_marker(udid);
        sweep_leftovers(&mut ctl, &marker).await;

        let id = uuid::Uuid::new_v4();
        mark(&marker, &id.to_string());
        // Big enough to hold the packets that arrive while `start` waits for the first one: a
        // keyframe is a few hundred packets.
        let (packets, early) = broadcast::channel(4096);
        let mut session = Self {
            ctl,
            id,
            marker,
            streams: Vec::new(),
            tasks: Vec::new(),
            packets,
            early: (mode == MediaMode::Video).then_some(early),
            video: mode == MediaMode::Video,
            video_rtcp: None,
            fir_seq: 0,
            display_id,
            stopped: false,
        };
        match session.start_streams(adapter).await {
            Ok(()) => Ok(session),
            Err(e) => {
                let _ = session.stop().await;
                Err(e)
            }
        }
    }

    async fn start_streams(&mut self, adapter: &AdapterHandle) -> Result<()> {
        let host = adapter.host_ip().to_string();
        let peer = adapter.peer_ip().to_string();

        // Xcode starts the audio stream first; it sets up the screen-sharing session.
        let udp = adapter.bind_udp(0).await.context("UDP bind on the tunnel failed (tunnel closed)")?;
        let call = uuid::Uuid::new_v4().to_string().to_uppercase();
        let offer = build_screen_audio_offer(&call, &call_info()).map_err(|e| anyhow!("audio offer: {e:?}"))?;
        let params = build_start_audio_parameters(
            &host, udp.local_port(), &peer, AUDIO_SENDER_PORT, offer, CLIENT_SUPPORTED_FEATURES, self.id,
        );
        match self.ctl.invoke(FEATURE_START, params, None).await {
            Ok(_) => {
                self.read(StreamKind::Audio, udp);
            }
            Err(e) if error_code(&e) == Some(ERROR_MEDIA_IN_USE) => {
                tracing::debug!("media: audio refused (9022), video sets up the session");
                self.video = true;
            }
            Err(e) => return Err(e.context("starting the audio stream failed")),
        }

        if self.video {
            let udp = adapter.bind_udp(0).await.context("UDP bind on the tunnel failed (tunnel closed)")?;
            let call = uuid::Uuid::new_v4().to_string().to_uppercase();
            let ssrc = uuid::Uuid::new_v4().as_u128() as u32;
            let offer = build_screen_video_offer(&call, &call_info(), ssrc).map_err(|e| anyhow!("video offer: {e:?}"))?;
            let params = build_start_video_parameters(
                &host, udp.local_port(), &peer, VIDEO_SENDER_PORT, offer, CLIENT_SUPPORTED_FEATURES,
                self.display_id, self.id,
            );
            self.ctl
                .invoke(FEATURE_START, params, None)
                .await
                .context("starting the video stream failed")?;
            let udp = self.read(StreamKind::Video, udp);
            self.video_rtcp = Some((udp, ssrc));
        }

        let began = Instant::now();
        while !self.flowing() && began.elapsed() < FIRST_PACKET {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        if !self.flowing() {
            bail!(
                "the device started the media streams but sent no packet within {} ms (connection stalled)",
                FIRST_PACKET.as_millis()
            );
        }
        Ok(())
    }

    fn read(&mut self, kind: StreamKind, udp: UdpSocketHandle) -> Arc<UdpSocketHandle> {
        let udp = Arc::new(udp);
        let socket = udp.clone();
        let counters = Arc::new(Counters::default());
        let (c, tx) = (counters.clone(), self.packets.clone());
        self.tasks.push(tokio::spawn(async move {
            while let Ok(dg) = udp.recv().await {
                let now = now_ms();
                let _ = c.first_ms.compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed);
                c.last_ms.store(now, Ordering::Relaxed);
                c.packets.fetch_add(1, Ordering::Relaxed);
                c.bytes.fetch_add(dg.data.len() as u64, Ordering::Relaxed);
                // No receiver is fine: input-only sessions throw the media away.
                let _ = tx.send(MediaPacket { kind, data: Arc::new(dg.data), at: Instant::now() });
            }
        }));
        self.streams.push((kind, counters));
        socket
    }

    /// Asks the device for a new keyframe (RTCP PLI + FIR on the video socket). `media_ssrc` is
    /// the SSRC of the video's RTP packets. The test device ignored these (PITFALLS #13); callers
    /// fall back to restarting the stream.
    pub async fn request_keyframe(&mut self, media_ssrc: u32) -> Result<()> {
        let Some((udp, ours)) = self.video_rtcp.clone() else {
            bail!("no video stream runs in this media session");
        };
        self.fir_seq = self.fir_seq.wrapping_add(1);
        let packet = idevice::core_device::build_keyframe_request(ours, "devicesteer", media_ssrc, &[], self.fir_seq);
        udp.send_to(VIDEO_SENDER_PORT, packet)
            .await
            .context("sending a keyframe request failed (tunnel closed)")
    }

    /// Whether every stream has delivered a packet.
    pub fn flowing(&self) -> bool {
        !self.streams.is_empty() && self.streams.iter().all(|(_, c)| c.first_ms.load(Ordering::Relaxed) > 0)
    }

    /// Milliseconds since the most recent packet of any stream, if any arrived.
    pub fn idle_ms(&self) -> Option<u64> {
        let last = self.streams.iter().map(|(_, c)| c.last_ms.load(Ordering::Relaxed)).max()?;
        (last > 0).then(|| now_ms().saturating_sub(last))
    }

    /// Media packets as they arrive. The first call also gets the packets from the start of the
    /// streams; later calls only new ones (ask for a keyframe then).
    pub fn subscribe(&mut self) -> broadcast::Receiver<MediaPacket> {
        self.early.take().unwrap_or_else(|| self.packets.subscribe())
    }

    pub fn session_id(&self) -> String {
        self.id.to_string()
    }

    /// Our streams' counters and the device's stream count.
    pub async fn status(&mut self) -> Result<MediaStatus> {
        let device = self.ctl.status().await?;
        let streams = self
            .streams
            .iter()
            .map(|(kind, c)| {
                let last = c.last_ms.load(Ordering::Relaxed);
                StreamStatus {
                    kind: *kind,
                    packets: c.packets.load(Ordering::Relaxed),
                    bytes: c.bytes.load(Ordering::Relaxed),
                    idle_ms: (last > 0).then(|| now_ms().saturating_sub(last)),
                }
            })
            .collect();
        Ok(MediaStatus { session_id: self.session_id(), streams, device_streams: stream_count(&device) })
    }

    /// Stops this session's streams (only ours, by name) and strikes it from the marker file.
    pub async fn stop(&mut self) -> Result<()> {
        for t in self.tasks.drain(..) {
            t.abort();
        }
        let id = self.id.to_string();
        let stopped = match tokio::time::timeout(STOP_WITHIN, self.ctl.stop_session(&id)).await {
            Ok(r) => r,
            Err(_) => Err(anyhow!("stopping media session {id} got no answer within {} s (connection stalled)", STOP_WITHIN.as_secs())),
        };
        if stopped.is_ok() {
            unmark(&self.marker, &id);
        }
        self.stopped = true;
        stopped
    }
}

/// Stops the streams of sessions an earlier process left running. Failures are logged, not
/// fatal: they belong to another session, and the new one can still start.
async fn sweep_leftovers(ctl: &mut MediaControl, marker: &Path) {
    let me = std::process::id();
    for (id, pid) in marked(marker) {
        if pid.is_some_and(|p| p == me || process_alive(p)) {
            continue;
        }
        match tokio::time::timeout(STOP_WITHIN, ctl.stop_session(&id)).await {
            Ok(Ok(())) => unmark(marker, &id),
            other => tracing::warn!("media: leftover session {id} not stopped: {other:?}"),
        }
    }
}

/// The device's media server status, without starting anything.
pub async fn server_status(adapter: &AdapterHandle, rsd: &RsdHandshake) -> Result<plist::Value> {
    MediaControl::connect(adapter, rsd).await?.status().await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(pairs: Vec<(&str, plist::Value)>) -> plist::Value {
        plist::Value::Dictionary(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    /// A stream as the media server lists it, started by client session `id`.
    fn stream(id: &str, ssrc: Option<u64>) -> plist::Value {
        let uuid = dict(vec![("uuid", plist::Value::String(id.into()))]);
        let options = dict(vec![("avcMediaStreamOptionClientSessionID", uuid)]);
        let mut conn = vec![("options", options)];
        if let Some(ssrc) = ssrc {
            conn.push(("streamConfig", dict(vec![("RemoteSSRC", plist::Value::Integer(ssrc.into()))])));
        }
        dict(vec![("connection", dict(conn))])
    }

    fn status(sessions: Vec<plist::Value>) -> plist::Value {
        dict(vec![("sessions", plist::Value::Array(sessions))])
    }

    const OURS: &str = "f608fea1-4ad9-4b5e-be7b-de38a2fbb0df";
    const THEIRS: &str = "0b7e9d3c-1111-4c2a-9e43-5d1f0a6b7c88";

    #[test]
    fn only_our_streams_are_named_for_stopping() {
        let all = status(vec![stream(OURS, Some(2_292_078_377)), stream(THEIRS, Some(11)), stream(&OURS.to_uppercase(), Some(5))]);
        assert_eq!(streams_of(&all, OURS), vec![Some(2_292_078_377), Some(5)]);
        assert_eq!(next_stop(&all, OURS, false), Stop::Named(vec![2_292_078_377, 5]));
        assert_eq!(next_stop(&status(vec![stream(THEIRS, Some(11))]), OURS, false), Stop::Done);
    }

    #[test]
    fn stop_all_is_never_the_answer() {
        // Still listed after stopping by name, even with nothing else running.
        assert!(matches!(next_stop(&status(vec![stream(OURS, Some(5))]), OURS, true), Stop::Kept(_)));
        // No name to stop by.
        assert!(matches!(next_stop(&status(vec![stream(OURS, None)]), OURS, false), Stop::Kept(_)));
        // No list to find them in.
        assert!(matches!(next_stop(&dict(vec![]), OURS, false), Stop::Kept(_)));
    }

    #[test]
    fn a_remote_ssrc_must_fit_a_u32() {
        assert_eq!(remote_ssrc(&stream("a", Some(u64::from(u32::MAX) + 1))), None);
        assert_eq!(remote_ssrc(&stream("a", Some(7))), Some(7));
    }

    #[test]
    fn the_marker_keeps_sessions_until_struck() {
        let dir = std::env::temp_dir().join(format!("devicesteer-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("x.media");
        mark(&marker, "a");
        mark(&marker, "b");
        mark(&marker, "a");
        let me = Some(std::process::id());
        assert_eq!(marked(&marker), vec![("a".to_string(), me), ("b".to_string(), me)]);
        assert!(process_alive(std::process::id()));
        unmark(&marker, "a");
        unmark(&marker, "b");
        assert!(!marker.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
