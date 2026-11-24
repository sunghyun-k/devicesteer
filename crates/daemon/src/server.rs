//! The per-device daemon: owns the tunnel, the media session, HID and the accessibility
//! subscription for one device, and serves the CLI and the viewer over a local socket.
//!
//! Requests are JSON `{"id", "cmd", "args", "source"}`; answers `{"id", "ok", "result"|"error"}`.
//! A client that sends `subscribe` also gets events: `control` (who drives), `input` (every
//! gesture with its points, so the viewer can draw it), `config` (decoder setup) and video frames.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use iphone_use_core::{
    Button, Controller, Frame, FrameStream, MediaMode, Motion, RecordSummary, Recorder, Resampler, Session, TouchPhase,
    ax::{BuildOptions, Screen},
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};

use crate::{
    control::{ControlAction, ControlState, Source},
    transport,
    wire::{self, Message},
};

/// Exit after this long with no client and no recording.
const IDLE_EXIT: Duration = Duration::from_secs(600);
/// Stop the media session (and the device's screen-sharing indicator) after this long without
/// input, viewer or recording.
const MEDIA_IDLE: Duration = Duration::from_secs(30);
/// A video that sent nothing for this long is restarted while someone watches.
const VIDEO_STALE_MS: u64 = 3000;
/// How long a late viewer waits for a keyframe it asked for before the stream is restarted.
const KEYFRAME_WAIT: Duration = Duration::from_millis(800);
/// A live touch whose client sent nothing for this long is lifted, so a client that hangs or
/// loses an `up` doesn't leave a finger on the screen. A client holding still (a long press)
/// repeats its last `move` to keep it down.
const TOUCH_TIMEOUT: Duration = Duration::from_secs(2);
/// A live touch's path is written to the device on this tick, from where the pointer was a
/// little earlier (PITFALLS #18). Twice the phone's 60 Hz display, so each display frame gets a
/// new point.
const TOUCH_TICK: Duration = Duration::from_micros(8333);
/// The least the reports play behind the client's pointer (ms), so a point to interpolate
/// towards has arrived: the viewer's moves come ~10 to 17 ms apart. The resampler raises it for
/// clients that send less often.
const TOUCH_MIN_DELAY_MS: f64 = 20.0;
/// How long agent requests wait for a human's touch to end before input is refused.
const AGENT_WAITS_FOR_TOUCH: Duration = Duration::from_secs(3);

/// Commands that put input on the device.
const INPUT_COMMANDS: &[&str] = &["tap", "swipe", "button", "launch", "touch"];

struct Recording {
    path: PathBuf,
    stop: oneshot::Sender<()>,
    done: oneshot::Receiver<Result<RecordSummary>>,
    started: Instant,
}

/// A live touch with its finger down.
struct Touch {
    /// The client connection that drives it.
    conn: u64,
    source: Source,
    at: (f64, f64),
    /// When that client last sent a step.
    last: Instant,
    /// The pointer's path, played to the device on [`TOUCH_TICK`].
    path: Resampler,
}

#[derive(Default)]
struct Device {
    controller: Option<Controller>,
    /// The frame stream of the current media session, by that session's id.
    frames: Option<(String, FrameStream, tokio::task::JoinHandle<()>)>,
    recording: Option<Recording>,
    last_input: Option<Instant>,
    touch: Option<Touch>,
}

pub struct Daemon {
    udid: String,
    device: Mutex<Device>,
    control: StdMutex<ControlState>,
    /// Events and frames for subscribed clients.
    events: broadcast::Sender<Arc<Message>>,
    /// Frames for the recorder, across media restarts.
    frames: broadcast::Sender<Frame>,
    /// The last `config` event, for clients that subscribe later.
    config: StdMutex<Option<Value>>,
    frame_subscribers: AtomicUsize,
    clients: AtomicUsize,
    last_client: StdMutex<Instant>,
    next_conn: AtomicU64,
    /// The daemon's clock for touch paths (ms since this instant).
    epoch: Instant,
    /// A live touch may be down: the touch tick has work.
    touch_live: AtomicBool,
}

#[derive(Deserialize, Clone)]
struct Request {
    #[serde(default)]
    id: Value,
    cmd: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    source: Source,
    /// The client connection it came on (set by the daemon, not the client).
    #[serde(skip)]
    conn: u64,
}

/// Errors that mean the tunnel is gone: the cached session is dropped and the next request
/// reconnects (lib.rs `step` puts these words in connection failures).
fn is_connection_error(e: &anyhow::Error) -> bool {
    let t = format!("{e:#}");
    t.contains("connection") || t.contains("tunnel")
}

fn point(v: &Value) -> Result<(f64, f64)> {
    match v.as_array().map(Vec::as_slice) {
        Some([x, y]) => Ok((x.as_f64().context("x is not a number")?, y.as_f64().context("y is not a number")?)),
        _ => bail!("a point is [x, y] in screen points"),
    }
}

impl Daemon {
    fn new(udid: String) -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        let (frames, _) = broadcast::channel(512);
        Arc::new(Self {
            udid,
            device: Mutex::new(Device::default()),
            control: StdMutex::new(ControlState::default()),
            events,
            frames,
            config: StdMutex::new(None),
            frame_subscribers: AtomicUsize::new(0),
            clients: AtomicUsize::new(0),
            last_client: StdMutex::new(Instant::now()),
            next_conn: AtomicU64::new(1),
            epoch: Instant::now(),
            touch_live: AtomicBool::new(false),
        })
    }

    fn emit(&self, v: Value) {
        let _ = self.events.send(Arc::new(Message::Json(v)));
    }

    fn emit_control(&self) {
        let ev = self.control.lock().unwrap().event();
        self.emit(ev);
    }

    fn want_video(&self, dev: &Device) -> bool {
        self.frame_subscribers.load(Ordering::Relaxed) > 0 || dev.recording.is_some()
    }

    async fn controller<'a>(&self, dev: &'a mut Device) -> Result<&'a mut Controller> {
        if dev.controller.is_none() {
            let session = Session::open(Some(&self.udid)).await?;
            let mode = if self.want_video(dev) { MediaMode::Video } else { MediaMode::InputOnly };
            dev.controller = Some(Controller::new(session, mode));
        }
        Ok(dev.controller.as_mut().expect("opened above"))
    }

    /// Drops the cached session after a connection error.
    async fn forget(&self, dev: &mut Device, why: &anyhow::Error) {
        tracing::warn!("dropping the device session: {why:#}");
        dev.frames = None;
        if let Some(mut c) = dev.controller.take() {
            let _ = tokio::time::timeout(Duration::from_secs(3), c.close()).await;
        }
    }

    /// Puts the media session in the mode the subscribers and recording need, and keeps a frame
    /// stream on it while video is wanted.
    async fn sync_video(self: &Arc<Self>, dev: &mut Device) -> Result<()> {
        let want = self.want_video(dev);
        if !want {
            dev.frames = None;
            if let Some(c) = dev.controller.as_mut() {
                c.set_mode(MediaMode::InputOnly).await?;
            }
            return Ok(());
        }
        let existing = dev.frames.as_ref().map(|(fid, _, _)| fid.clone());
        let c = self.controller(dev).await?;
        c.set_mode(MediaMode::Video).await?;
        let display = c.display().await?;
        let media = c.ensure_media().await?;
        let id = media.session_id();
        if existing.as_deref() == Some(id.as_str()) {
            return Ok(());
        }
        let stream = FrameStream::start(media);
        let mut rx = stream.subscribe();
        let me = self.clone();
        let display_px = ((display.panel.0 * display.scale).round(), (display.panel.1 * display.scale).round());
        let screen = display.size();
        let forwarder = tokio::spawn(async move {
            let mut last_params = None;
            let mut seq = 0u64;
            loop {
                let f = match rx.recv().await {
                    Ok(f) => f,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("frame forwarder fell {n} frames behind");
                        continue;
                    }
                    Err(_) => break,
                };
                if let Some(p) = &f.params
                    && last_params.as_ref() != Some(p)
                {
                    last_params = Some(p.clone());
                    let ev = json!({
                        "event": "config",
                        "codec": p.codec(),
                        "description": base64::engine::general_purpose::STANDARD.encode(p.hvcc()),
                        "coded": [p.width, p.height],
                        // The screen is the top-left part of the coded picture (PITFALLS #11).
                        "display_px": [display_px.0, display_px.1],
                        "screen": [screen.0, screen.1],
                    });
                    *me.config.lock().unwrap() = Some(ev.clone());
                    me.emit(ev);
                }
                seq += 1;
                let header = json!({
                    "event": "frame",
                    "seq": seq,
                    "pts_us": f.pts_us,
                    "keyframe": f.keyframe,
                    "host_age_us": f.arrived.elapsed().as_micros() as u64,
                });
                let _ = me.events.send(Arc::new(Message::Frame { header, data: f.sample.to_vec() }));
                let _ = me.frames.send(f);
            }
        });
        dev.frames = Some((id, stream, forwarder));
        Ok(())
    }

    /// For a viewer that joins a running stream: ask for a keyframe, and restart the stream if
    /// none comes (PITFALLS #11).
    async fn fresh_keyframe(self: &Arc<Self>) -> Result<Value> {
        let mut rx = self.frames.subscribe();
        let asked = {
            let mut dev = self.device.lock().await;
            let ssrc = dev.frames.as_ref().and_then(|(_, s, _)| s.ssrc());
            match (ssrc, dev.controller.as_mut().and_then(|c| c.media_mut())) {
                (Some(ssrc), Some(m)) => m.request_keyframe(ssrc).await.is_ok(),
                _ => false,
            }
        };
        let began = Instant::now();
        if asked {
            while let Ok(Ok(f)) = tokio::time::timeout(KEYFRAME_WAIT.saturating_sub(began.elapsed()), rx.recv()).await {
                if f.keyframe {
                    return Ok(json!({ "keyframe": "requested", "ms": began.elapsed().as_millis() as u64 }));
                }
            }
        }
        let mut dev = self.device.lock().await;
        if let Some(c) = dev.controller.as_mut() {
            c.restart_media().await?;
        }
        self.sync_video(&mut dev).await?;
        Ok(json!({ "keyframe": "restarted", "ms": began.elapsed().as_millis() as u64 }))
    }

    fn check_agent(&self, source: Source) -> Result<()> {
        if source == Source::Agent
            && let Some(why) = self.control.lock().unwrap().refuse_agent()
        {
            bail!("{why}");
        }
        Ok(())
    }

    fn input_event(&self, source: Source, kind: &str, points: &[(f64, f64)], screen: (f64, f64), extra: Value) {
        if source == Source::Agent {
            self.control.lock().unwrap().agent_input();
        }
        let mut ev = json!({
            "event": "input",
            "source": source,
            "kind": kind,
            "points": points.iter().map(|p| [p.0, p.1]).collect::<Vec<_>>(),
            "screen": [screen.0, screen.1],
        });
        if let (Some(o), Some(e)) = (ev.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        self.emit(ev);
        if source == Source::Agent && self.control.lock().unwrap().active_changed() {
            self.emit_control();
        }
    }

    /// Lifts the live touch's finger, if one is down and `which` picks it. The `input` event
    /// says why, so the viewer clears its indicator.
    async fn lift_touch(&self, dev: &mut Device, which: impl Fn(&Touch) -> bool, why: &str) {
        if !dev.touch.as_ref().is_some_and(&which) {
            return;
        }
        let t = dev.touch.take().expect("checked above");
        self.control.lock().unwrap().human_touching = false;
        tracing::info!("lifting the {:?} touch: {why}", t.source);
        let mut screen = (0.0, 0.0);
        if let Some(c) = dev.controller.as_mut() {
            match c.touch(TouchPhase::Up, None).await {
                Ok(sent) => screen = sent.screen,
                Err(e) => tracing::warn!("could not lift the touch: {e:#}"),
            }
        }
        self.emit(json!({
            "event": "input",
            "source": t.source,
            "kind": "touch",
            "phase": "up",
            "points": [[t.at.0, t.at.1]],
            "screen": [screen.0, screen.1],
            "lifted": why,
        }));
    }

    /// One step of a live touch from client `req.conn`: args `phase` (down, move, up), `point`
    /// `[x, y]` in screen points, and optionally `t`, when the pointer was there (ms on the
    /// client's own clock), which spaces the replayed path as it was drawn (PITFALLS #18). A move's
    /// answer means it joined the path, not that it reached the device.
    async fn touch(self: &Arc<Self>, dev: &mut Device, req: &Request) -> Result<Value> {
        let a = &req.args;
        iphone_use_core::trace::line("req", format_args!("{} {}", req.conn, a.get("phase").and_then(Value::as_str).unwrap_or("?")));
        let phase: TouchPhase = serde_json::from_value(a.get("phase").cloned().unwrap_or_default())
            .map_err(|_| anyhow!("touch needs a phase: down, move or up"))?;
        let at = a.get("point").map(point).transpose()?;
        let mine = dev.touch.as_ref().map(|t| t.conn == req.conn);
        match (phase, mine) {
            // Another client's finger is down. A human's touch lifts it, as a second hand on the
            // glass would take over; an agent never gets here while a human touches (check_agent).
            (TouchPhase::Down, Some(false)) => {
                if req.source == Source::Agent {
                    bail!("another agent client is touching the screen; wait for it to lift and retry");
                }
                self.lift_touch(dev, |_| true, "a human touched the screen").await;
            }
            (TouchPhase::Move | TouchPhase::Up, Some(false)) => {
                bail!("another client is touching the screen; this client has no finger down")
            }
            (TouchPhase::Move, None) => {
                bail!("no finger is down (another touch, a control change or {} s without a step lifted it); send phase down to touch again", TOUCH_TIMEOUT.as_secs())
            }
            (TouchPhase::Up, None) => return Ok(json!({ "phase": "up", "lifted": false })),
            _ => {}
        }
        if phase != TouchPhase::Up && at.is_none() {
            bail!("a touch down or move needs a point [x, y] in screen points");
        }
        // The client's own time of the step (ms, any clock), to space the path as it was drawn.
        let client_t = a.get("t").and_then(Value::as_f64);
        let now = self.now_ms();
        // A move only extends the path; the touch tick writes it (PITFALLS #18).
        if phase == TouchPhase::Move
            && let (Some(t), Some(p)) = (dev.touch.as_mut(), at)
        {
            t.path.push(now, client_t, p);
            t.at = p;
            t.last = Instant::now();
            let screen = match dev.controller.as_mut() {
                Some(c) => c.display().await?.size(),
                None => (0.0, 0.0),
            };
            dev.last_input = Some(Instant::now());
            self.input_event(req.source, "touch", &[p], screen, json!({ "phase": phase }));
            return Ok(json!({ "phase": phase, "point": [p.0, p.1], "screen": screen }));
        }
        let sync = self.sync_video(dev).await;
        self.controller(dev).await?;
        sync?;
        let Device { controller, touch, .. } = &mut *dev;
        let c = controller.as_mut().expect("opened above");
        let sent = match (phase, touch.as_mut()) {
            (TouchPhase::Up, Some(t)) => {
                // Play the rest of the path before lifting, or the end of the gesture is cut off.
                if let Some(p) = at {
                    t.path.push(now, client_t, p);
                }
                let mut played = Ok(());
                loop {
                    let now = self.now_ms();
                    if let Err(e) = c.touch(TouchPhase::Move, Some(t.path.sample(now))).await {
                        played = Err(e);
                        break;
                    }
                    if now >= t.path.done_at() {
                        break;
                    }
                    tokio::time::sleep(TOUCH_TICK).await;
                }
                match played {
                    Ok(()) => c.touch(TouchPhase::Up, Some(t.path.last())).await,
                    Err(e) => Err(e),
                }
            }
            _ => c.touch(phase, at).await,
        };
        let p = at.or(dev.touch.as_ref().map(|t| t.at)).unwrap_or_default();
        let sent = match sent {
            Ok(s) => s,
            Err(e) => {
                // The finger's state on the device is unknown; don't keep refusing the agent.
                dev.touch = None;
                self.control.lock().unwrap().human_touching = false;
                return Err(e);
            }
        };
        dev.touch = match phase {
            TouchPhase::Up => None,
            _ => Some(Touch {
                conn: req.conn,
                source: req.source,
                at: p,
                last: Instant::now(),
                path: Resampler::new(now, client_t, p, TOUCH_MIN_DELAY_MS),
            }),
        };
        if dev.touch.is_some() {
            self.touch_live.store(true, Ordering::Relaxed);
        }
        if req.source == Source::Human {
            self.control.lock().unwrap().human_touching = dev.touch.is_some();
        }
        dev.last_input = Some(Instant::now());
        self.input_event(req.source, "touch", &[p], sent.screen, json!({ "phase": phase }));
        Ok(json!({ "phase": phase, "point": [p.0, p.1], "screen": sent.screen, "ms": sent.ms }))
    }

    fn now_ms(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64() * 1000.0
    }

    /// Writes live touches' paths to the device at a steady rate (PITFALLS #18).
    async fn touch_tick(self: Arc<Self>) {
        let mut tick = tokio::time::interval(TOUCH_TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if !self.touch_live.load(Ordering::Relaxed) {
                continue;
            }
            let mut dev = self.device.lock().await;
            let now = self.now_ms();
            let Some(p) = dev.touch.as_mut().map(|t| t.path.sample(now)) else {
                self.touch_live.store(false, Ordering::Relaxed);
                continue;
            };
            let Some(c) = dev.controller.as_mut().filter(|c| c.touching()) else { continue };
            // An unchanged point sends nothing (a resting finger stays down, PITFALLS #17).
            if let Err(e) = c.touch(TouchPhase::Move, Some(p)).await {
                tracing::warn!("live touch report failed: {e:#}");
                dev.touch = None;
                self.control.lock().unwrap().human_touching = false;
                if is_connection_error(&e) {
                    self.forget(&mut dev, &e).await;
                }
            }
        }
    }

    /// An agent's request waits while a human touches, for up to [`AGENT_WAITS_FOR_TOUCH`]: the
    /// agent should not move the screen under a person's finger, nor read it mid-gesture.
    async fn wait_for_human_touch(&self) {
        let began = Instant::now();
        while self.control.lock().unwrap().human_touching && began.elapsed() < AGENT_WAITS_FOR_TOUCH {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Runs one request against the device. Connection errors drop the cached session.
    async fn device_op(self: &Arc<Self>, req: &Request) -> Result<Value> {
        if req.source == Source::Agent {
            self.wait_for_human_touch().await;
        }
        let mut dev = self.device.lock().await;
        if req.source == Source::Agent && INPUT_COMMANDS.contains(&req.cmd.as_str()) {
            // The human may have touched again while this waited for the lock.
            self.check_agent(req.source)?;
        }
        let r = self.device_op_locked(&mut dev, req).await;
        // A tap or swipe lifts a live touch's finger first, and a media restart closes HID with
        // it; forget such a touch so its next `move` says the finger is gone.
        if dev.touch.is_some() && !dev.controller.as_ref().is_some_and(Controller::touching) {
            dev.touch = None;
            self.control.lock().unwrap().human_touching = false;
        }
        if let Err(e) = &r
            && is_connection_error(e)
        {
            dev.touch = None;
            self.control.lock().unwrap().human_touching = false;
            self.forget(&mut dev, e).await;
        }
        r
    }

    async fn device_op_locked(self: &Arc<Self>, dev: &mut Device, req: &Request) -> Result<Value> {
        let a = &req.args;
        match req.cmd.as_str() {
            "tap" => {
                self.check_agent(req.source)?;
                let at = if let Some(p) = a.get("point") {
                    (point(p)?, Value::Null)
                } else if let Some(target) = a.get("target").and_then(|v| v.as_str()) {
                    let c = self.controller(dev).await?;
                    if !target.starts_with('@') || c.last_screen.is_none() {
                        c.ui(BuildOptions::default(), false).await?;
                    }
                    let screen = c.last_screen.as_ref().expect("read above");
                    let e = screen.find(target).map_err(|e| anyhow!(e))?;
                    let point = screen.tap_point(e).context("that element has no frame to tap")?;
                    (point, serde_json::to_value(e)?)
                } else {
                    bail!("tap needs a point [x, y] or a target (@eN or a label)");
                };
                let (p, element) = at;
                let sync = self.sync_video(dev).await;
                let c = self.controller(dev).await?;
                sync?;
                let screen = c.display().await?.size();
                self.input_event(req.source, "tap", &[p], screen, json!({}));
                let sent = c.tap(p).await?;
                dev.last_input = Some(Instant::now());
                Ok(json!({ "tapped": [p.0, p.1], "element": element, "screen": sent.screen, "ms": sent.ms }))
            }
            "swipe" => {
                self.check_agent(req.source)?;
                let from = point(a.get("from").context("swipe needs from")?)?;
                let to = point(a.get("to").context("swipe needs to")?)?;
                let ms = a.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(300);
                let motion = if a.get("place").and_then(|v| v.as_bool()).unwrap_or(false) { Motion::Place } else { Motion::Flick };
                let sync = self.sync_video(dev).await;
                let c = self.controller(dev).await?;
                sync?;
                let screen = c.display().await?.size();
                self.input_event(req.source, "swipe", &[from, to], screen, json!({ "duration_ms": ms }));
                let sent = c.drag(from, to, Duration::from_millis(ms), motion).await?;
                dev.last_input = Some(Instant::now());
                Ok(json!({ "from": [from.0, from.1], "to": [to.0, to.1], "screen": sent.screen, "ms": sent.ms }))
            }
            "button" => {
                self.check_agent(req.source)?;
                let b = Button::parse(a.get("name").and_then(|v| v.as_str()).context("button needs a name")?)?;
                let sync = self.sync_video(dev).await;
                let c = self.controller(dev).await?;
                sync?;
                let screen = c.display().await?.size();
                self.input_event(req.source, "button", &[], screen, json!({ "button": b }));
                c.button(b).await?;
                dev.last_input = Some(Instant::now());
                Ok(json!({ "pressed": b }))
            }
            "touch" => {
                self.check_agent(req.source)?;
                self.touch(dev, req).await
            }
            "ui" => {
                let opts = BuildOptions { all: a.get("all").and_then(|v| v.as_bool()).unwrap_or(false) };
                let full = a.get("full").and_then(|v| v.as_bool()).unwrap_or(false);
                let c = self.controller(dev).await?;
                let began = Instant::now();
                let screen: Screen = c.ui(opts, full).await?;
                let mut v = serde_json::to_value(&screen)?;
                v["ms"] = json!(began.elapsed().as_millis() as u64);
                Ok(v)
            }
            "launch" => {
                self.check_agent(req.source)?;
                let bundle = a.get("bundle_id").and_then(|v| v.as_str()).context("launch needs bundle_id")?;
                let relaunch = a.get("relaunch").and_then(|v| v.as_bool()).unwrap_or(false);
                let c = self.controller(dev).await?;
                let pid = c.launch(bundle, relaunch).await?;
                Ok(json!({ "launched": bundle, "pid": pid }))
            }
            "display" => {
                let c = self.controller(dev).await?;
                let d = c.refresh_display().await?;
                Ok(json!({ "display": d, "screen": d.size() }))
            }
            "record_start" => {
                if dev.recording.is_some() {
                    bail!("a recording already runs; end it with `record stop` first");
                }
                let path = PathBuf::from(a.get("path").and_then(|v| v.as_str()).context("record_start needs a path")?);
                let mut recorder = Recorder::create(&path)?;
                let mut rx = self.frames.subscribe();
                let (stop_tx, mut stop_rx) = oneshot::channel();
                let (done_tx, done_rx) = oneshot::channel();
                dev.recording = Some(Recording { path: path.clone(), stop: stop_tx, done: done_rx, started: Instant::now() });
                if let Err(e) = self.sync_video(dev).await {
                    dev.recording = None;
                    return Err(e);
                }
                tokio::spawn(async move {
                    let mut last = Instant::now();
                    let r = loop {
                        tokio::select! {
                            f = rx.recv() => match f {
                                Ok(f) => {
                                    if let Err(e) = recorder.push(&f) {
                                        break Err(e);
                                    }
                                    last = Instant::now();
                                }
                                Err(broadcast::error::RecvError::Lagged(n)) => tracing::warn!("recorder fell {n} frames behind"),
                                Err(_) => break Ok(()),
                            },
                            _ = &mut stop_rx => break Ok(()),
                        }
                    };
                    let _ = done_tx.send(r.and_then(|()| recorder.finish(last.elapsed())));
                });
                Ok(json!({ "recording": path }))
            }
            "record_stop" => {
                let rec = dev.recording.take().context("no recording runs; start one with `record start`")?;
                let _ = rec.stop.send(());
                let summary = rec.done.await.map_err(|_| anyhow!("the recorder task ended without a summary"))??;
                self.sync_video(dev).await?;
                Ok(serde_json::to_value(summary)?)
            }
            "status" => {
                let media = match dev.controller.as_mut() {
                    Some(c) => c.media_status().await.ok().flatten(),
                    None => None,
                };
                let control = self.control.lock().unwrap().event();
                Ok(json!({
                    "udid": self.udid,
                    "pid": std::process::id(),
                    "connected": dev.controller.is_some(),
                    "media": media,
                    "control": control,
                    "recording": dev.recording.as_ref().map(|r| json!({ "path": r.path, "seconds": r.started.elapsed().as_secs_f64() })),
                    "clients": self.clients.load(Ordering::Relaxed),
                    "viewers": self.frame_subscribers.load(Ordering::Relaxed),
                    "touch": dev.touch.as_ref().map(|t| json!({ "source": t.source, "point": [t.at.0, t.at.1], "idle_ms": t.last.elapsed().as_millis() as u64 })),
                }))
            }
            other => bail!("unknown command `{other}`"),
        }
    }

    /// Periodic work: the agent-active indicator, stale video, idle media and idle exit.
    async fn housekeeping(self: Arc<Self>, shutdown: mpsc::Sender<()>) {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        let mut n = 0u64;
        loop {
            tick.tick().await;
            n += 1;
            if self.control.lock().unwrap().active_changed() {
                self.emit_control();
            }
            if let Ok(mut dev) = self.device.try_lock() {
                self.lift_touch(&mut dev, |t| t.last.elapsed() > TOUCH_TIMEOUT, "no step for 2 s").await;
            }
            if !n.is_multiple_of(4) {
                continue;
            }
            let Ok(mut dev) = self.device.try_lock() else { continue };
            let want = self.want_video(&dev);
            let idle_ms = dev.controller.as_ref().and_then(|c| c.media()).and_then(|m| m.idle_ms());
            if want && idle_ms.is_some_and(|ms| ms > VIDEO_STALE_MS) {
                tracing::warn!("video went quiet for {idle_ms:?} ms; restarting it");
                if let Some(c) = dev.controller.as_mut() {
                    let _ = c.restart_media().await;
                }
                let _ = self.sync_video(&mut dev).await;
            }
            let input_idle = dev.last_input.is_none_or(|t| t.elapsed() > MEDIA_IDLE);
            if !want
                && input_idle
                && let Some(c) = dev.controller.as_mut().filter(|c| c.media().is_some())
            {
                let _ = c.close().await;
            }
            let no_clients = self.clients.load(Ordering::Relaxed) == 0;
            if no_clients && dev.recording.is_none() && self.last_client.lock().unwrap().elapsed() > IDLE_EXIT {
                let _ = shutdown.send(()).await;
                return;
            }
        }
    }

    async fn serve_client<S>(self: Arc<Self>, stream: S, shutdown: mpsc::Sender<()>)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + 'static,
    {
        self.clients.fetch_add(1, Ordering::Relaxed);
        let conn = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let (mut rd, mut wr) = tokio::io::split(stream);
        let (out_tx, mut out_rx) = mpsc::channel::<Arc<Message>>(512);
        let writer = tokio::task::spawn_local(async move {
            while let Some(m) = out_rx.recv().await {
                if wire::write(&mut wr, &m).await.is_err() {
                    break;
                }
            }
        });
        let mut subscription: Option<(tokio::task::JoinHandle<()>, bool)> = None;
        while let Ok(Some(msg)) = wire::read(&mut rd).await {
            let Message::Json(v) = msg else { continue };
            let req: Request = match serde_json::from_value(v) {
                Ok(r) => Request { conn, ..r },
                Err(e) => {
                    let _ = out_tx.send(Arc::new(Message::Json(json!({ "ok": false, "error": format!("bad request: {e}") })))).await;
                    continue;
                }
            };
            if req.cmd == "touch" {
                iphone_use_core::trace::line("recv", format_args!("{conn}"));
            }
            let result = match req.cmd.as_str() {
                "ping" => Ok(json!({ "udid": self.udid, "pid": std::process::id() })),
                "subscribe" => {
                    let frames = req.args.get("frames").and_then(|v| v.as_bool()).unwrap_or(false);
                    if let Some((task, had_frames)) = subscription.take() {
                        task.abort();
                        if had_frames {
                            self.frame_subscribers.fetch_sub(1, Ordering::Relaxed);
                        }
                    }
                    let mut rx = self.events.subscribe();
                    let tx = out_tx.clone();
                    let task = tokio::spawn(async move {
                        loop {
                            match rx.recv().await {
                                Ok(m) => {
                                    if !frames && matches!(*m, Message::Frame { .. }) {
                                        continue;
                                    }
                                    // A slow client loses messages rather than holding up others.
                                    if tx.try_send(m).is_err() && tx.is_closed() {
                                        break;
                                    }
                                }
                                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                                Err(_) => break,
                            }
                        }
                    });
                    subscription = Some((task, frames));
                    let ev = self.control.lock().unwrap().event();
                    let _ = out_tx.send(Arc::new(Message::Json(ev))).await;
                    if frames {
                        let first = self.frame_subscribers.fetch_add(1, Ordering::Relaxed) == 0;
                        let mut dev = self.device.lock().await;
                        let had_stream = dev.frames.is_some();
                        let synced = self.sync_video(&mut dev).await;
                        drop(dev);
                        match synced {
                            Err(e) => Err(e),
                            Ok(()) if had_stream || !first => {
                                let config = self.config.lock().unwrap().clone();
                                if let Some(c) = config {
                                    let _ = out_tx.send(Arc::new(Message::Json(c))).await;
                                }
                                self.fresh_keyframe().await
                            }
                            Ok(()) => Ok(json!({ "keyframe": "stream start" })),
                        }
                    } else {
                        Ok(json!({ "subscribed": true }))
                    }
                }
                "control" => {
                    match serde_json::from_value::<ControlAction>(req.args.get("action").cloned().unwrap_or_default()) {
                        Ok(action) => {
                            self.control.lock().unwrap().apply(action);
                            // Whoever lost the screen lifts their finger: the human on handing
                            // back, the agent when the human takes over or stops it.
                            let losing = match action {
                                ControlAction::Release => Some(Source::Human),
                                ControlAction::TakeOver | ControlAction::StopAgent => Some(Source::Agent),
                                ControlAction::ResumeAgent => None,
                            };
                            if let Some(losing) = losing {
                                let mut dev = self.device.lock().await;
                                self.lift_touch(&mut dev, |t| t.source == losing, "control changed").await;
                            }
                            self.emit_control();
                            Ok(self.control.lock().unwrap().event())
                        }
                        Err(_) => Err(anyhow!("control action is one of take_over, release, stop_agent, resume_agent")),
                    }
                }
                "shutdown" => {
                    let _ = shutdown.send(()).await;
                    Ok(json!({ "stopping": true }))
                }
                _ => self.device_op(&req).await,
            };
            let reply = match result {
                Ok(r) => json!({ "id": req.id, "ok": true, "result": r }),
                Err(e) => json!({ "id": req.id, "ok": false, "error": format!("{e:#}") }),
            };
            if out_tx.send(Arc::new(Message::Json(reply))).await.is_err() {
                break;
            }
        }
        {
            let mut dev = self.device.lock().await;
            self.lift_touch(&mut dev, |t| t.conn == conn, "its client disconnected").await;
        }
        if let Some((task, frames)) = subscription.take() {
            task.abort();
            if frames && self.frame_subscribers.fetch_sub(1, Ordering::Relaxed) == 1 {
                let mut dev = self.device.lock().await;
                let _ = self.sync_video(&mut dev).await;
            }
        }
        drop(out_tx);
        let _ = writer.await;
        self.clients.fetch_sub(1, Ordering::Relaxed);
        *self.last_client.lock().unwrap() = Instant::now();
    }

    async fn close(&self) {
        let mut dev = self.device.lock().await;
        if let Some(rec) = dev.recording.take() {
            let _ = rec.stop.send(());
            let _ = rec.done.await;
        }
        dev.frames = None;
        if let Some(mut c) = dev.controller.take() {
            let _ = tokio::time::timeout(Duration::from_secs(6), c.close()).await;
        }
    }
}

/// Runs the daemon for `udid` until it is idle for long enough, told to stop, or interrupted.
///
/// It runs on a runtime of its own with one thread: device work is serialized anyway, and the
/// futures over the tunnel hold references across awaits that rustc can't prove `Send` for every
/// lifetime, so client tasks are spawned locally.
pub fn run_blocking(udid: &str) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, run(udid))
}

async fn run(udid: &str) -> Result<()> {
    let listener = transport::Listener::bind(udid).await?;
    let daemon = Daemon::new(udid.to_string());
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(4);
    tokio::task::spawn_local(daemon.clone().housekeeping(shutdown_tx.clone()));
    tokio::task::spawn_local(daemon.clone().touch_tick());
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    tracing::info!("daemon for {udid} listening");
    loop {
        tokio::select! {
            conn = listener.accept() => match conn {
                Ok(stream) => {
                    tokio::task::spawn_local(daemon.clone().serve_client(stream, shutdown_tx.clone()));
                }
                Err(e) => tracing::warn!("accept failed: {e:#}"),
            },
            _ = shutdown_rx.recv() => break,
            _ = &mut ctrl_c => break,
        }
    }
    daemon.close().await;
    listener.remove();
    Ok(())
}
