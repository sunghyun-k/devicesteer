//! The per-device daemon: owns the tunnel, the media session, HID and the accessibility
//! subscription for one device, and serves the CLI and the viewer over a local socket.
//!
//! Requests are JSON `{"id", "cmd", "args", "source"}`; answers `{"id", "ok", "result"|"error"}`.
//! An agent's request may name its session (`"session": "<id>"`, see `control`). A client that
//! sends `subscribe` also gets events: `control` (the agent session and who drives), `input`
//! (every gesture with its points and label, so the viewer can draw it), `config` (decoder setup),
//! `orientation` (the screen turned), `recording` (a recording started or stopped, from any
//! client), `settings` and video frames.

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
use devicesteer_core::{
    Button, Clipboard, Controller, TypeMethod, DeviceSettings, Display, DisplayPoller, Frame, FrameStream, MediaMode, Motion, RecordSummary,
    Recorder, Resampler, Session, Setting, TAP_HOLD, Toggle, TouchPhase,
    ax::{BuildOptions, Screen},
    keyboard,
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
/// How often the display is asked for while the device session is open: the interface orientation
/// is only known by asking (PITFALLS #27). A call on the kept connection takes ~10 ms.
const DISPLAY_POLL: Duration = Duration::from_millis(500);
/// Polled this often for a while after input: a home press or a launch turns the screen ~1.4 s
/// later, and at 0.5 s the viewer showed the turned picture sideways for up to 0.4 s (PITFALLS #27).
const DISPLAY_POLL_AFTER_INPUT: Duration = Duration::from_millis(100);
const FAST_POLL_FOR: Duration = Duration::from_secs(3);
/// How long agent requests wait for a human's touch to end before input is refused.
const AGENT_WAITS_FOR_TOUCH: Duration = Duration::from_secs(3);
/// The longest `label` an input takes: it is shown next to the touch in the viewer, so it is a
/// few words, not a sentence.
const MAX_LABEL: usize = 60;

/// Held keys are released after this long without a key step from their client. A held key's
/// keydown repeats about every 30 to 80 ms in a browser, and the viewer sends those on; a key
/// left down on the device repeats its character.
const KEYS_TIMEOUT: Duration = Duration::from_secs(2);
/// The same with only modifiers down: those don't repeat, and a person may hold one a while.
const MODIFIERS_TIMEOUT: Duration = Duration::from_secs(30);
/// Commands that put input on the device.
const INPUT_COMMANDS: &[&str] = &["tap", "swipe", "button", "launch", "touch", "settings_set", "key", "type", "paste", "clipboard_set", "keyboard", "keyboard_connect", "keyboard_disconnect"];
/// How long a reading of the VoiceOver switch is trusted before an input reads it again.
const VOICEOVER_CHECK: Duration = Duration::from_secs(5);
/// Added to the result of input sent while VoiceOver is on.
const VOICEOVER_WARNING: &str = "VoiceOver is on: a tap selects an element and a double tap activates it, so \
    taps and swipes don't do what they would. Turn it off with `settings set voiceover off`";

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

/// A task that ends when this is dropped.
struct TaskGuard(tokio::task::JoinHandle<()>);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Default)]
struct Device {
    controller: Option<Controller>,
    /// Polls the controller's display while it is open.
    display_watch: Option<TaskGuard>,
    /// The frame stream of the current media session, by that session's id.
    frames: Option<(String, FrameStream, tokio::task::JoinHandle<()>)>,
    recording: Option<Recording>,
    last_input: Option<Instant>,
    touch: Option<Touch>,
    /// The location this daemon simulated (the device has no way to read it back).
    location: Option<(f64, f64)>,
    /// The VoiceOver switch as last read, and when.
    voiceover: Option<(bool, Instant)>,
    /// Keys a client holds down (the viewer's keyboard).
    keys: Option<HeldKeys>,
    /// The client whose hardware keyboard is attached only while it stays connected
    /// (`keyboard_connect` with `while_connected`: the viewer's "Type from Mac Keyboard"), and
    /// whether it is the human or an agent.
    keyboard_holder: Option<(u64, Source)>,
    /// The clipboard's contents from before a paste the device asked to allow, until
    /// `paste --restore`. Never sent to a client.
    clipboard: Option<Clipboard>,
}

/// Keys a client holds down on the device.
struct HeldKeys {
    conn: u64,
    source: Source,
    /// When that client last sent a key step (a held key repeats its keydown).
    last: Instant,
    /// Only modifiers are down.
    modifiers_only: bool,
}

impl HeldKeys {
    fn stale(&self) -> bool {
        self.last.elapsed() > if self.modifiers_only { MODIFIERS_TIMEOUT } else { KEYS_TIMEOUT }
    }
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
    /// The display as last read: its rotation turns the video and the touch points.
    display: StdMutex<Option<Display>>,
    /// Until when the display is polled fast, after input.
    fast_poll_until: StdMutex<Instant>,
    /// The running recording's path and start, for clients that subscribe while it runs.
    recording: StdMutex<Option<(PathBuf, Instant)>>,
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
    /// The agent session it belongs to (`session start`), if any.
    #[serde(default)]
    session: Option<String>,
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

/// How the screen is turned, for events: `rotation` (from the panel's native orientation: the
/// video picture and the touch reports are in that), `orientation` and `screen`, the size in
/// points as the user sees it, which input points and `ui` frames use.
fn orientation_json(d: &Display) -> Value {
    let (w, h) = d.size();
    json!({ "rotation": d.rotation, "orientation": d.orientation(), "screen": [w, h] })
}

/// The `keyboard` event: whether our hardware keyboard is attached (the on-screen keyboard is
/// then hidden), and whether it is meant to be (it is attached again with the next input after
/// the HID connection closed while idle).
fn keyboard_event(c: &Controller) -> Value {
    let (wanted, attached) = c.keyboard_state();
    json!({ "event": "keyboard", "hardware": wanted, "attached": attached })
}

/// An input's optional `label`: a few words saying what it does, shown next to it in the viewer.
fn label(a: &Value) -> Result<Option<String>> {
    let Some(l) = a.get("label").filter(|v| !v.is_null()) else { return Ok(None) };
    let l = l.as_str().context("label is a string")?.split_whitespace().collect::<Vec<_>>().join(" ");
    if l.chars().count() > MAX_LABEL {
        bail!("the label is longer than {MAX_LABEL} characters; say what the input does in 3 to 6 words");
    }
    Ok(Some(l).filter(|l| !l.is_empty()))
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
            display: StdMutex::new(None),
            recording: StdMutex::new(None),
            fast_poll_until: StdMutex::new(Instant::now()),
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

    /// The `recording` event for the recording running now, if any.
    fn recording_event(&self) -> Option<Value> {
        let r = self.recording.lock().unwrap();
        let (path, started) = r.as_ref()?;
        Some(json!({ "event": "recording", "recording": true, "path": path, "seconds": started.elapsed().as_secs_f64() }))
    }

    fn emit_control(&self) {
        let ev = self.control.lock().unwrap().event();
        self.emit(ev);
    }

    fn want_video(&self, dev: &Device) -> bool {
        self.frame_subscribers.load(Ordering::Relaxed) > 0 || dev.recording.is_some()
    }

    async fn controller<'a>(self: &Arc<Self>, dev: &'a mut Device) -> Result<&'a mut Controller> {
        if dev.controller.is_none() {
            let session = Session::open(Some(&self.udid)).await?;
            let mode = if self.want_video(dev) { MediaMode::Video } else { MediaMode::InputOnly };
            let poller = DisplayPoller::new(session.adapter().clone(), session.rsd().clone());
            dev.display_watch = Some(TaskGuard(tokio::task::spawn_local(self.clone().watch_display(poller))));
            dev.controller = Some(Controller::new(session, mode));
        }
        Ok(dev.controller.as_mut().expect("opened above"))
    }

    /// Follows the interface orientation while the device session is open, also between the CLI's
    /// commands (each its own connection: a `tap` right after `button home` would otherwise be
    /// placed on the screen as it was before the home screen came up). The video and the touch
    /// reports are in the panel's orientation and the device doesn't announce a turn (PITFALLS
    /// #27), so the display is asked for again and again, on a connection of its own and without
    /// the device lock: a live touch or a reading isn't held up by it.
    async fn watch_display(self: Arc<Self>, mut poller: DisplayPoller) {
        loop {
            let fast = Instant::now() < *self.fast_poll_until.lock().unwrap();
            tokio::time::sleep(if fast { DISPLAY_POLL_AFTER_INPUT } else { DISPLAY_POLL }).await;
            match poller.ask().await {
                Ok(d) => self.display_seen(d).await,
                Err(e) => tracing::debug!("display poll: {e:#}"),
            }
        }
    }

    /// Takes a display reading: on a change, the controller places touches on the new screen, the
    /// cached `config` says how the video is turned, and subscribers get an `orientation` event.
    /// On another panel (a foldable folded or unfolded, PITFALLS #47) the controller moves its
    /// video and touches to it, and the frame stream follows.
    async fn display_seen(self: &Arc<Self>, d: Display) {
        if !self.note_display(&d) {
            return;
        }
        let mut dev = self.device.lock().await;
        let Some(c) = dev.controller.as_mut() else { return };
        match c.set_display(d).await {
            Ok(true) => self.follow_panel(&mut dev).await,
            Ok(false) => {}
            Err(e) => tracing::warn!("moving to the new panel: {e:#}"),
        }
    }

    /// After the controller moved to another panel: its HID connection closed with any live touch,
    /// and the frame stream goes on from the restarted media session (a new `config`).
    async fn follow_panel(self: &Arc<Self>, dev: &mut Device) {
        if dev.touch.is_some() && !dev.controller.as_ref().is_some_and(Controller::touching) {
            dev.touch = None;
            self.control.lock().unwrap().human_touching = false;
            self.emit_control();
        }
        if let Err(e) = self.sync_video(dev).await {
            tracing::warn!("video on the new panel: {e:#}");
        }
    }

    /// The part of [`Self::display_seen`] that needs no device lock. Returns whether it changed.
    fn note_display(&self, d: &Display) -> bool {
        let panel = {
            let mut cur = self.display.lock().unwrap();
            if cur.as_ref() == Some(d) {
                return false;
            }
            let panel = cur.as_ref().is_some_and(|c| !c.same_panel(d));
            *cur = Some(d.clone());
            panel
        };
        tracing::info!("display {}: {:?}, screen {:?}", d.unique_id, d.rotation, d.size());
        let mut ev = orientation_json(d);
        if panel {
            // The cached config is the old panel's stream: its picture size and decoder setup. The
            // stream restarted on the new panel sends its own; until then a viewer that joins
            // waits for it rather than sizing the old picture to the new screen.
            *self.config.lock().unwrap() = None;
            ev["event"] = json!("panel");
            ev["display_id"] = json!(d.display_id);
            ev["unique_id"] = json!(d.unique_id);
            self.emit(ev);
            return true;
        }
        if let Some(config) = self.config.lock().unwrap().as_mut()
            && let (Some(c), Some(o)) = (config.as_object_mut(), ev.as_object())
        {
            c.extend(o.clone());
            c.insert("event".into(), json!("config"));
        }
        ev["event"] = json!("orientation");
        self.emit(ev);
        true
    }

    /// Drops the cached session after a connection error.
    async fn forget(&self, dev: &mut Device, why: &anyhow::Error) {
        tracing::warn!("dropping the device session: {why:#}");
        dev.frames = None;
        dev.display_watch = None;
        if let Some(mut c) = dev.controller.take() {
            let had_keyboard = c.keyboard_state().0;
            let _ = tokio::time::timeout(Duration::from_secs(3), c.close()).await;
            // The next controller starts without one: say so, so the viewer asks again.
            dev.keyboard_holder = None;
            if had_keyboard {
                self.emit(json!({ "event": "keyboard", "hardware": false, "attached": false }));
            }
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
        self.display.lock().unwrap().get_or_insert_with(|| display.clone());
        let media = c.ensure_media().await?;
        let id = media.session_id();
        if existing.as_deref() == Some(id.as_str()) {
            return Ok(());
        }
        let stream = FrameStream::start(media);
        let mut rx = stream.subscribe();
        let me = self.clone();
        let display_px = ((display.panel.0 * display.scale).round(), (display.panel.1 * display.scale).round());
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
                    let mut ev = json!({
                        "event": "config",
                        "codec": p.codec(),
                        "description": base64::engine::general_purpose::STANDARD.encode(p.hvcc()),
                        "coded": [p.width, p.height],
                        // The panel is the top-left part of the coded picture (PITFALLS #11), in
                        // its native orientation whichever way the interface is turned (#27).
                        "display_px": [display_px.0, display_px.1],
                    });
                    // How to turn the picture, and the screen it then shows (`screen`, points).
                    let turned = orientation_json(&me.display.lock().unwrap().clone().unwrap_or_else(|| display.clone()));
                    if let (Some(e), Some(t)) = (ev.as_object_mut(), turned.as_object()) {
                        e.extend(t.clone());
                    }
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

    /// Checks an agent's request against its session (see `control`); Ok carries a note for the
    /// result. Human requests always pass.
    fn check_agent(&self, req: &Request) -> Result<Option<&'static str>> {
        if req.source != Source::Agent {
            return Ok(None);
        }
        let input = INPUT_COMMANDS.contains(&req.cmd.as_str());
        let mut control = self.control.lock().unwrap();
        let expired = control.session().map(|s| s.id.clone());
        let r = control.check_agent(req.session.as_deref(), input);
        let ended = expired.is_some() && control.session().is_none();
        drop(control);
        if ended {
            self.emit_control();
        }
        r.map_err(|e| anyhow!(e))
    }

    fn input_event(&self, source: Source, kind: &str, points: &[(f64, f64)], screen: (f64, f64), extra: Value) {
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
    }

    /// Releases the keys a client holds, if `which` picks it: before other input, when its
    /// client goes away or stops sending (a key left down repeats on the device).
    async fn release_keys(&self, dev: &mut Device, which: impl Fn(&HeldKeys) -> bool, why: &str) {
        if !dev.keys.as_ref().is_some_and(&which) {
            return;
        }
        dev.keys = None;
        if let Some(c) = dev.controller.as_mut()
            && let Err(e) = c.release_keys().await
        {
            tracing::warn!("could not release the held keys ({why}): {e:#}");
        }
    }

    /// Removes the hardware keyboard a client attached `while_connected`, if `which` picks that
    /// client: when it goes away, or when the agent gets the device from the human.
    async fn drop_held_keyboard(&self, dev: &mut Device, which: impl Fn(u64, Source) -> bool, why: &str) {
        if !dev.keyboard_holder.is_some_and(|(conn, source)| which(conn, source)) {
            return;
        }
        dev.keyboard_holder = None;
        self.release_keys(dev, |_| true, why).await;
        let Some(c) = dev.controller.as_mut() else { return };
        tracing::info!("removing the hardware keyboard: {why}");
        if let Err(e) = c.set_keyboard(false).await {
            tracing::warn!("could not remove the hardware keyboard ({why}): {e:#}");
        }
        let ev = keyboard_event(c);
        self.emit(ev);
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
        devicesteer_core::trace::line("req", format_args!("{} {}", req.conn, a.get("phase").and_then(Value::as_str).unwrap_or("?")));
        let phase: TouchPhase = serde_json::from_value(a.get("phase").cloned().unwrap_or_default())
            .map_err(|_| anyhow!("touch needs a phase: down, move or up"))?;
        let at = a.get("point").map(point).transpose()?;
        let touch_label = label(a)?;
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
            self.input_event(req.source, "touch", &[p], screen, json!({ "phase": phase, "label": touch_label }));
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
                    if now >= t.path.done_at() || t.path.settled() {
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
        self.input_event(req.source, "touch", &[p], sent.screen, json!({ "phase": phase, "label": touch_label }));
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
        if INPUT_COMMANDS.contains(&req.cmd.as_str()) {
            *self.fast_poll_until.lock().unwrap() = Instant::now() + FAST_POLL_FOR;
        }
        let mut dev = self.device.lock().await;
        // After the lock: the human may have touched or taken over while this waited for it.
        let note = self.check_agent(req)?;
        let mut r = self.device_op_locked(&mut dev, req).await;
        if let (Ok(v), Some(note)) = (&mut r, note)
            && v.is_object()
        {
            v["note"] = json!(note);
        }
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
                // Sent as the finger goes down, not before opening HID and the gap after the last
                // touch: the viewer ends its indicator `duration_ms` after it comes, as the finger
                // lifts.
                let extra = json!({ "label": label(a)?, "duration_ms": TAP_HOLD.as_millis() as u64 });
                let sent = c.tap(p, || self.input_event(req.source, "tap", &[p], screen, extra)).await?;
                dev.last_input = Some(Instant::now());
                let r = json!({ "tapped": [p.0, p.1], "element": element, "screen": sent.screen, "ms": sent.ms });
                Ok(self.warn_voiceover(dev, r).await)
            }
            "swipe" => {
                let from = point(a.get("from").context("swipe needs from")?)?;
                let to = point(a.get("to").context("swipe needs to")?)?;
                let ms = a.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(300);
                let motion = if a.get("place").and_then(|v| v.as_bool()).unwrap_or(false) { Motion::Place } else { Motion::Flick };
                let sync = self.sync_video(dev).await;
                let c = self.controller(dev).await?;
                sync?;
                let screen = c.display().await?.size();
                self.input_event(req.source, "swipe", &[from, to], screen, json!({ "duration_ms": ms, "label": label(a)? }));
                let sent = c.drag(from, to, Duration::from_millis(ms), motion).await?;
                dev.last_input = Some(Instant::now());
                let r = json!({ "from": [from.0, from.1], "to": [to.0, to.1], "screen": sent.screen, "ms": sent.ms });
                Ok(self.warn_voiceover(dev, r).await)
            }
            "button" => {
                let b = Button::parse(a.get("name").and_then(|v| v.as_str()).context("button needs a name")?)?;
                let sync = self.sync_video(dev).await;
                let c = self.controller(dev).await?;
                sync?;
                let screen = c.display().await?.size();
                self.input_event(req.source, "button", &[], screen, json!({ "button": b, "label": label(a)? }));
                c.button(b).await?;
                dev.last_input = Some(Instant::now());
                Ok(self.warn_voiceover(dev, json!({ "pressed": b })).await)
            }
            "touch" => {
                self.touch(dev, req).await
            }
            "key" => {
                let name = a.get("name").and_then(|v| v.as_str()).context("key needs a name")?;
                let stroke = keyboard::parse_key(name)?;
                let sync = self.sync_video(dev).await;
                self.release_keys(dev, |_| true, "a key press").await;
                let c = self.controller(dev).await?;
                sync?;
                let screen = c.display().await?.size();
                self.input_event(req.source, "key", &[], screen, json!({ "key": name, "label": label(a)? }));
                let sent = c.press_keys(std::slice::from_ref(&stroke)).await?;
                dev.last_input = Some(Instant::now());
                Ok(self.warn_voiceover(dev, json!({ "pressed": name, "ms": sent.ms })).await)
            }
            "type" => {
                let text = a.get("text").and_then(|v| v.as_str()).context("type needs text")?;
                let enter = a.get("enter").and_then(|v| v.as_bool()).unwrap_or(false);
                let force = a.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
                let method = match a.get("method").and_then(|v| v.as_str()) {
                    Some(m) => TypeMethod::parse(m)?,
                    None => TypeMethod::default(),
                };
                let sync = self.sync_video(dev).await;
                self.release_keys(dev, |_| true, "typing").await;
                let c = self.controller(dev).await?;
                sync?;
                let screen = c.display().await?.size();
                self.input_event(req.source, "type", &[], screen, json!({ "chars": text.chars().count(), "label": label(a)? }));
                let typed = Box::pin(c.type_text(text, enter, force, method)).await?;
                dev.last_input = Some(Instant::now());
                Ok(self.warn_voiceover(dev, serde_json::to_value(typed)?).await)
            }
            "paste" => {
                let restore = a.get("restore").and_then(|v| v.as_bool()).unwrap_or(false);
                let force = a.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
                let sync = self.sync_video(dev).await;
                self.release_keys(dev, |_| true, "pasting").await;
                let mut backup = dev.clipboard.take();
                let c = self.controller(dev).await?;
                sync?;
                let r = if restore {
                    c.restore_backup(&mut backup).await.map(|restored| {
                        json!({ "clipboard": if restored { "restored" } else { "nothing to restore" } })
                    })
                } else {
                    let text = a.get("text").and_then(|v| v.as_str()).context("paste needs text")?;
                    let screen = c.display().await?.size();
                    self.input_event(req.source, "paste", &[], screen, json!({ "chars": text.chars().count(), "label": label(a)? }));
                    let r = Box::pin(c.paste_text(text, force, &mut backup)).await;
                    r.and_then(|p| Ok(serde_json::to_value(p)?))
                };
                dev.clipboard = backup;
                dev.last_input = Some(Instant::now());
                Ok(self.warn_voiceover(dev, r?).await)
            }
            // The clipboard on its own, with no paste: the viewer's "Send Clipboard", "Get
            // Clipboard" and shared clipboard, and `devicesteer clipboard`. The contents go only to
            // the client that asked; nothing here logs them.
            "clipboard_set" => {
                let text = a.get("text").and_then(|v| v.as_str()).context("clipboard_set needs text")?;
                let c = self.controller(dev).await?;
                c.put_clipboard_text(text).await?;
                // The user's contents a paste kept for `paste --restore` would now replace what
                // was just set on purpose.
                let dropped = dev.clipboard.take().is_some();
                dev.last_input = Some(Instant::now());
                let mut r = json!({ "clipboard": "set", "chars": text.chars().count() });
                if dropped {
                    r["note"] = json!("the clipboard contents an earlier paste kept for `paste --restore` were dropped");
                }
                Ok(r)
            }
            "clipboard_get" => {
                let pending = dev.clipboard.is_some();
                let c = self.controller(dev).await?;
                let mut r = match c.session.clipboard().await? {
                    Clipboard::Text(t) => json!({ "kind": "text", "chars": t.chars().count(), "text": t }),
                    other => json!({ "kind": other.kind() }),
                };
                // The clipboard holds a paste's text, not the owner's: the viewer's shared
                // clipboard leaves it alone.
                if pending {
                    r["paste_pending"] = json!(true);
                }
                Ok(r)
            }
            "keyboard_connect" | "keyboard_disconnect" | "keyboard_status" => {
                if req.cmd != "keyboard_status" {
                    let sync = self.sync_video(dev).await;
                    self.release_keys(dev, |_| true, "the hardware keyboard changed").await;
                    let c = self.controller(dev).await?;
                    sync?;
                    let connect = req.cmd == "keyboard_connect";
                    c.set_keyboard(connect).await?;
                    dev.last_input = Some(Instant::now());
                    // A keyboard left attached keeps the on-screen keyboard hidden for whoever
                    // uses the device next (PITFALLS #36): one asked for `while_connected` goes
                    // with its client. Any other connect or disconnect makes it nobody's.
                    let held = connect && a.get("while_connected").and_then(Value::as_bool).unwrap_or(false);
                    dev.keyboard_holder = held.then_some((req.conn, req.source));
                }
                let ev = keyboard_event(self.controller(dev).await?);
                self.emit(ev.clone());
                Ok(ev)
            }
            "keyboard" => {
                let down = a.get("down").and_then(|v| v.as_bool()).context("keyboard needs down: true or false")?;
                let usage = match (a.get("code").and_then(|v| v.as_str()), a.get("usage").and_then(|v| v.as_u64())) {
                    (Some(code), _) => match keyboard::dom_code_usage(code) {
                        Some(u) => u,
                        // A key the device has no usage for (Fn, media keys): nothing to send.
                        None => return Ok(json!({ "ignored": code })),
                    },
                    (None, Some(u)) if u >= 1 && u <= u64::from(keyboard::MAX_USAGE) => u as u8,
                    _ => bail!("keyboard needs a DOM `code` (KeyA) or a HID `usage` (1 to 231)"),
                };
                if !down && dev.keys.is_none() {
                    return Ok(json!({ "usage": usage, "down": false }));
                }
                let sync = self.sync_video(dev).await;
                let c = self.controller(dev).await?;
                sync?;
                c.key(usage, down).await?;
                let down_now = c.keys_down();
                let modifiers_only = down_now.iter().all(|u| keyboard::is_modifier(*u));
                dev.keys = (!down_now.is_empty())
                    .then(|| HeldKeys { conn: req.conn, source: req.source, last: Instant::now(), modifiers_only });
                dev.last_input = Some(Instant::now());
                Ok(json!({ "usage": usage, "down": down }))
            }
            "ui" => {
                let opts = BuildOptions { all: a.get("all").and_then(|v| v.as_bool()).unwrap_or(false) };
                let full = a.get("full").and_then(|v| v.as_bool()).unwrap_or(false);
                let c = self.controller(dev).await?;
                let began = Instant::now();
                let screen: Screen = c.ui(opts, full).await?;
                let orientation = c.display().await?.orientation();
                let mut v = serde_json::to_value(&screen)?;
                v["orientation"] = json!(orientation);
                v["ms"] = json!(began.elapsed().as_millis() as u64);
                Ok(v)
            }
            "launch" => {
                let bundle = a.get("bundle_id").and_then(|v| v.as_str()).context("launch needs bundle_id")?;
                let relaunch = a.get("relaunch").and_then(|v| v.as_bool()).unwrap_or(false);
                let c = self.controller(dev).await?;
                let pid = c.launch(bundle, relaunch).await?;
                Ok(json!({ "launched": bundle, "pid": pid }))
            }
            "settings" => {
                let c = self.controller(dev).await?;
                let s = c.session.settings().await?;
                Ok(self.settings_event(dev, &s))
            }
            "settings_set" => {
                let name = a.get("name").and_then(|v| v.as_str()).context("settings_set needs a name")?;
                let values: Vec<String> = a
                    .get("value")
                    .and_then(|v| v.as_array())
                    .context("settings_set needs a value list")?
                    .iter()
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect();
                let setting = Setting::parse(name, &values)?;
                let c = self.controller(dev).await?;
                c.session.apply_setting(&setting).await?;
                if matches!(setting, Setting::Toggle(Toggle::VoiceOver, _)) {
                    c.forget_accessibility();
                }
                match setting {
                    Setting::Location(lat, lon) => dev.location = Some((lat, lon)),
                    Setting::ClearLocation => dev.location = None,
                    _ => {}
                }
                let c = self.controller(dev).await?;
                let s = c.session.settings().await?;
                let settings = self.settings_event(dev, &s);
                let mut r = json!({ "set": setting.name(), "settings": settings });
                if s.voiceover == Some(true) {
                    r["warning"] = json!(VOICEOVER_WARNING);
                }
                Ok(r)
            }
            "display" => {
                let c = self.controller(dev).await?;
                let before = c.display().await?;
                let d = c.refresh_display().await?;
                let r = json!({ "display": d, "screen": d.size(), "orientation": d.orientation() });
                // The controller has it already; this is called with the device lock held.
                self.note_display(&d);
                if !before.same_panel(&d) {
                    self.follow_panel(dev).await;
                }
                Ok(r)
            }
            "record_start" => {
                if dev.recording.is_some() {
                    bail!("a recording already runs; end it with `record stop` first");
                }
                let path = PathBuf::from(a.get("path").and_then(|v| v.as_str()).context("record_start needs a path")?);
                let mut recorder = Recorder::create(&path)?;
                // Asked now, not up to a poll old: the MP4 keeps this for the whole recording.
                let d = self.controller(dev).await?.refresh_display().await?;
                self.note_display(&d);
                recorder.set_rotation(d.rotation);
                let mut rx = self.frames.subscribe();
                let (stop_tx, mut stop_rx) = oneshot::channel();
                let (done_tx, done_rx) = oneshot::channel();
                // A stream that already runs (a viewer watches) sent its only keyframe long ago and
                // ignores requests for another (PITFALLS #11, #13, #28): restart it, or the recording
                // has nothing to start from. Viewers take the new stream's config and keyframe.
                let running = dev.frames.is_some();
                dev.recording = Some(Recording { path: path.clone(), stop: stop_tx, done: done_rx, started: Instant::now() });
                let started = async {
                    if running && let Some(c) = dev.controller.as_mut() {
                        c.restart_media().await?;
                    }
                    self.sync_video(dev).await
                };
                if let Err(e) = started.await {
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
                *self.recording.lock().unwrap() = Some((path.clone(), Instant::now()));
                // Every client shows it, whoever started it (the CLI, an agent or the viewer).
                if let Some(ev) = self.recording_event() {
                    self.emit(ev);
                }
                Ok(json!({ "recording": path }))
            }
            "record_stop" => {
                let rec = dev.recording.take().context("no recording runs; start one with `record start`")?;
                *self.recording.lock().unwrap() = None;
                let _ = rec.stop.send(());
                let summary = rec
                    .done
                    .await
                    .map_err(|_| anyhow!("the recorder task ended without a summary"))
                    .and_then(|r| r);
                let mut ev = json!({ "event": "recording", "recording": false, "path": rec.path });
                match &summary {
                    Ok(s) => ev["summary"] = serde_json::to_value(s)?,
                    Err(e) => ev["error"] = json!(format!("{e:#}")),
                }
                self.emit(ev);
                let summary = summary?;
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
                    "display": self.display.lock().unwrap().as_ref().map(orientation_json),
                }))
            }
            other => bail!("unknown command `{other}`"),
        }
    }

    /// Settings as the clients see them (with the location this daemon set), sent to subscribers
    /// as a `settings` event too, so a viewer follows changes the agent makes.
    fn settings_event(&self, dev: &mut Device, s: &DeviceSettings) -> Value {
        if let Some(on) = s.voiceover {
            dev.voiceover = Some((on, Instant::now()));
        }
        let mut v = serde_json::to_value(s).unwrap_or_default();
        v["location"] = json!(dev.location.map(|(lat, lon)| [lat, lon]));
        let mut ev = v.clone();
        ev["event"] = json!("settings");
        self.emit(ev);
        v
    }

    /// Adds a warning to an input's result while VoiceOver is on: taps then select instead of
    /// activating, and an agent that doesn't know would read the screen as unchanged. The switch
    /// is read again when the last reading is older than [`VOICEOVER_CHECK`]; a failed read
    /// leaves the result alone.
    async fn warn_voiceover(&self, dev: &mut Device, mut result: Value) -> Value {
        let fresh = dev.voiceover.filter(|(_, at)| at.elapsed() < VOICEOVER_CHECK).map(|(on, _)| on);
        let on = match fresh {
            Some(on) => on,
            None => {
                let Some(c) = dev.controller.as_mut() else { return result };
                match c.session.setting_toggle(Toggle::VoiceOver).await {
                    Ok(Some(on)) => {
                        let changed = dev.voiceover.map(|(was, _)| was) != Some(on);
                        dev.voiceover = Some((on, Instant::now()));
                        if changed {
                            self.emit(json!({ "event": "settings", "voiceover": on }));
                        }
                        on
                    }
                    _ => return result,
                }
            }
        };
        if on {
            result["warning"] = json!(VOICEOVER_WARNING);
        }
        result
    }

    /// Periodic work: session expiry, stale video, idle media and idle exit.
    async fn housekeeping(self: Arc<Self>, shutdown: mpsc::Sender<()>) {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        let mut n = 0u64;
        loop {
            tick.tick().await;
            n += 1;
            if self.control.lock().unwrap().expire() {
                self.emit_control();
            }
            if let Ok(mut dev) = self.device.try_lock() {
                self.lift_touch(&mut dev, |t| t.last.elapsed() > TOUCH_TIMEOUT, "no step for 2 s").await;
                self.release_keys(&mut dev, HeldKeys::stale, "no key step").await;
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

    /// `session` with `action`: start (`name`), finish (`force`), handoff (`reason`), resume,
    /// status. The session id comes in the request's `session`.
    async fn session_cmd(&self, req: &Request) -> Result<Value> {
        let a = &req.args;
        let action = a.get("action").and_then(Value::as_str).unwrap_or("status");
        let id = req.session.as_deref();
        let need_id = || id.context("say which session with --session (or DEVICESTEER_SESSION); `session start` prints its id");
        let text = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let r = {
            let mut c = self.control.lock().unwrap();
            match action {
                "start" => c.start(&text("name")),
                "finish" => c.finish(id, a.get("force").and_then(Value::as_bool).unwrap_or(false)),
                "handoff" => c.handoff(need_id()?, &text("reason")),
                "resume" => c.resume(need_id()?),
                "status" => c.status(id),
                other => Err(format!("unknown session action `{other}`: start, finish, handoff, resume or status")),
            }
        };
        let r = r.map_err(|e| anyhow!(e))?;
        if action != "status" {
            if action == "handoff" {
                // The human may want to touch right away; an agent's finger left down would fight them.
                let mut dev = self.device.lock().await;
                self.lift_touch(&mut dev, |t| t.source == Source::Agent, "the agent handed off").await;
                self.release_keys(&mut dev, |k| k.source == Source::Agent, "the agent handed off").await;
            }
            self.emit_control();
        }
        if matches!(action, "start" | "resume") {
            // The agent has the device now: the human's keyboard would hide the on-screen keys
            // it reads and taps, and its `type` attaches one of its own. After the control event,
            // so the viewer doesn't ask for it again.
            let mut dev = self.device.lock().await;
            self.drop_held_keyboard(&mut dev, |_, s| s == Source::Human, "an agent session took the device").await;
        }
        Ok(r)
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
                devicesteer_core::trace::line("recv", format_args!("{conn}"));
            }
            let result = match req.cmd.as_str() {
                "ping" => Ok(json!({ "udid": self.udid, "pid": std::process::id() })),
                "subscribe" => {
                    let frames = req.args.get("frames").and_then(|v| v.as_bool()).unwrap_or(false);
                    if let Some((task, had_frames)) = subscription.take() {
                        task.abort();
                        // A viewer that stops watching the screen but stays attached (for settings
                        // and the session panel) was the last one: the stream stops, as it would on
                        // disconnect, instead of running for nobody.
                        if had_frames && self.frame_subscribers.fetch_sub(1, Ordering::Relaxed) == 1 && !frames {
                            let mut dev = self.device.lock().await;
                            let _ = self.sync_video(&mut dev).await;
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
                    if let Some(ev) = self.recording_event() {
                        let _ = out_tx.send(Arc::new(Message::Json(ev))).await;
                    }
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
                "control" => match serde_json::from_value::<ControlAction>(req.args.get("action").cloned().unwrap_or_default()) {
                    Ok(action) => {
                        let applied = self.control.lock().unwrap().apply(action);
                        match applied {
                            Ok(losing) => {
                                // Whoever lost the screen lifts their finger: the human on handing
                                // back, the agent when the human takes over or stops it.
                                if let Some(losing) = losing {
                                    let mut dev = self.device.lock().await;
                                    self.lift_touch(&mut dev, |t| t.source == losing, "control changed").await;
                                    self.release_keys(&mut dev, |k| k.source == losing, "control changed").await;
                                }
                                self.emit_control();
                                // After the control event: a viewer that saw its keyboard go
                                // while it still had the device would ask for it again.
                                if let Some(losing) = losing {
                                    let mut dev = self.device.lock().await;
                                    self.drop_held_keyboard(&mut dev, |_, s| s == losing, "control changed").await;
                                }
                                Ok(self.control.lock().unwrap().event())
                            }
                            Err(e) => Err(anyhow!(e)),
                        }
                    }
                    Err(_) => Err(anyhow!("control action is one of take_over, hand_back, done, stop")),
                },
                "session" => self.session_cmd(&req).await,
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
            self.release_keys(&mut dev, |k| k.conn == conn, "its client disconnected").await;
            self.drop_held_keyboard(&mut dev, |c, _| c == conn, "its client disconnected").await;
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
        dev.display_watch = None;
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
