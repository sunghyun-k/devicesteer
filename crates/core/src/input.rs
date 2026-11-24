//! Touch and hardware-button input over CoreDevice HID.
//!
//! Touches go to `hid.universalhidservice` as touchscreen reports, buttons to `hid.indigo`. The
//! device only acts on them while a screen-sharing media stream runs (PITFALLS #7), so callers
//! keep a [`crate::MediaSession`] open for as long as an [`Input`] is.
//!
//! The waits (devices listed, warm-up, the "released" first report, tap hold, gap between
//! touches, settle before closing) are adapted from iphone-cli `src/device/input.rs`, MIT, see
//! THIRD_PARTY_NOTICES.md. PITFALLS #8 has what each one prevents.

use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, bail};
use idevice::{
    ReadWrite, RsdService,
    core_device::hid::{
        ButtonState, DIGITIZER_SURFACE_MAIN_TOUCHSCREEN, HidSurface, IndigoHidClient, KeyboardUsage,
        MainKeyboardService, TOUCHSCREEN_STATE_CONTACT, TOUCHSCREEN_STATE_RELEASE, UniversalHidServiceClient,
        build_touchscreen_report,
    },
    rsd::RsdHandshake,
    tcp::handle::AdapterHandle,
};
use serde::{Deserialize, Serialize};

use crate::{
    display::Display,
    keyboard::{self, Stroke},
    step,
};

/// Longest wait for the device to list a new connection's HID devices.
const DEVICES_READY: Duration = Duration::from_secs(5);
/// After the devices are listed, the system still has to open them; a touch before that is held
/// and replayed all at once (PITFALLS #8).
const WARMUP: Duration = Duration::from_millis(300);
/// Keep the touch connection quiet this long after a release before closing it.
const SETTLE: Duration = Duration::from_millis(300);
/// After a home-button release, before closing (PITFALLS #6, old #17).
const BUTTON_SETTLE: Duration = Duration::from_millis(800);
/// Shortest pause between a release and the next touch, so two taps aren't a double tap.
const TOUCH_GAP: Duration = Duration::from_millis(150);
/// How long a tap holds the finger down.
pub const TAP_HOLD: Duration = Duration::from_millis(50);
/// Time between touch samples while dragging.
const DRAG_STEP: Duration = Duration::from_millis(16);
/// A live touch with no report for this long before its `up` was resting (PITFALLS #17).
const RESTING: Duration = Duration::from_millis(50);
/// Reports at the resting point sent before such an `up`, `DRAG_STEP` apart.
const STILL_SAMPLES: u32 = 4;
/// How far those reports wobble, in report units: about one pixel on a phone. Reports at exactly
/// the same point don't count as samples (PITFALLS #17).
const STILL_WOBBLE: u16 = 51;
/// How far (screen points) a live touch may stray from where it went down and still be a tap:
/// within it the device starts no scroll, so there is no speed to cancel before the `up`
/// (PITFALLS #17) and the still reports, 65 ms, are left out (PITFALLS #34).
const TAP_SLOP: f64 = 4.0;

/// A hardware button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Home,
    Lock,
    VolumeUp,
    VolumeDown,
    /// The Eject key: with a hardware keyboard attached, shows or hides the on-screen keyboard
    /// (PITFALLS #39).
    Eject,
}

impl Button {
    pub const NAMES: &str = "home, lock, volume_up, volume_down, eject";

    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name.trim().to_lowercase().replace('-', "_").as_str() {
            "home" => Self::Home,
            "lock" | "power" => Self::Lock,
            "volume_up" => Self::VolumeUp,
            "volume_down" => Self::VolumeDown,
            "eject" => Self::Eject,
            other => bail!("unknown button `{other}`; use one of: {}", Self::NAMES),
        })
    }

    /// HID consumer-page usage, and how long to hold it.
    fn usage(self) -> (u64, u64, Duration) {
        let ms = Duration::from_millis;
        match self {
            Self::Home => (0x0C, 0x40, ms(80)),
            Self::Lock => (0x0C, 0x30, ms(200)),
            Self::VolumeUp => (0x0C, 0xE9, ms(80)),
            Self::VolumeDown => (0x0C, 0xEA, ms(80)),
            Self::Eject => (0x0C, 0xB8, ms(80)),
        }
    }
}

/// How a drag ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Motion {
    /// Constant speed, lifted while moving: lists keep coasting.
    Flick,
    /// Slows to a stop before lifting: content moves by the finger's distance and stays.
    Place,
}

/// One step of a touch that a client drives live (the viewer's mouse): finger down, moved to a
/// point, or lifted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TouchPhase {
    Down,
    Move,
    Up,
}

/// A finger that is down between live touch calls.
#[derive(Debug, Clone, Copy)]
struct Held {
    at: (u16, u16),
    since: Instant,
    /// When the last report went out.
    reported: Instant,
    /// Where it went down (screen points), and whether it has left [`TAP_SLOP`] around there.
    origin: (f64, f64),
    moved: bool,
}

/// An open HID connection, ready for touches.
pub struct Input {
    hid: UniversalHidServiceClient<Box<dyn ReadWrite>>,
    indigo: Option<IndigoHidClient<Box<dyn ReadWrite>>>,
    adapter: AdapterHandle,
    rsd: RsdHandshake,
    /// The touchscreen reports go to.
    touchscreen: u64,
    last_release: Option<Instant>,
    settle: Duration,
    /// The finger of a live touch, while it is down.
    held: Option<Held>,
    /// The keyboard surface key reports go to.
    keyboard: u64,
    /// Keys a client holds down between live key calls (the viewer's keyboard).
    keys_down: BTreeSet<u8>,
    /// The hardware keyboard we attached, while it is (see [`Input::connect_keyboard`]).
    hardware: Option<MainKeyboardService>,
}

/// The keyboard surface every universalhidservice connection gets (old PITFALLS #11).
const KEYBOARD_SURFACE: u64 = 512;
/// Time between attaching the hardware keyboard and its first key.
const KEYBOARD_ATTACH: Duration = Duration::from_millis(700);
/// Time between the reports of one key press. Dev measured 20 ms per step as reliable over Wi-Fi.
const KEY_STEP: Duration = Duration::from_millis(20);

/// The touchscreens among a connection's HID surfaces: `CoreDevice touchscreen(<display>)`.
fn touchscreens(surfaces: &[HidSurface]) -> Vec<(u64, String)> {
    surfaces
        .iter()
        .filter_map(|s| {
            let id = s.product.as_deref()?.strip_prefix("CoreDevice touchscreen(")?.strip_suffix(')')?;
            Some((s.service_id, id.to_string()))
        })
        .collect()
}

async fn devices_ready(hid: &mut UniversalHidServiceClient<Box<dyn ReadWrite>>) -> Option<Vec<HidSurface>> {
    match tokio::time::timeout(DEVICES_READY, hid.list_connected_services()).await {
        Ok(Ok(list)) => Some(list),
        other => {
            tracing::debug!("input: HID devices not listed: {other:?}");
            None
        }
    }
}

impl Input {
    /// Opens the touch connection and waits until the device can take touches on `display`.
    ///
    /// Takes the tunnel handles by value: a future holding references to them across awaits is
    /// not `Send` for every lifetime, and the daemon spawns these futures.
    pub async fn open(mut adapter: AdapterHandle, mut rsd: RsdHandshake, display: Display) -> Result<Self> {
        let mut hid = step(
            "HID service connection",
            UniversalHidServiceClient::connect_rsd(&mut adapter, &mut rsd),
        )
        .await?;
        let listed = devices_ready(&mut hid).await;
        let screens = listed.as_deref().map(touchscreens).unwrap_or_default();
        // One panel lists `touchscreen(nil)`; a foldable lists one per panel, named by display.
        let touchscreen = match screens.len() {
            0 | 1 => screens.first().map_or(DIGITIZER_SURFACE_MAIN_TOUCHSCREEN, |s| s.0),
            _ => screens
                .iter()
                .find(|(_, id)| *id == display.unique_id)
                .map(|s| s.0)
                .ok_or_else(|| anyhow!("no touchscreen belongs to the active display {}", display.unique_id))?,
        };
        let keyboard = listed
            .as_deref()
            .and_then(|l| l.iter().find(|s| s.product.as_deref() == Some("CoreDevice keyboard")))
            .map_or(KEYBOARD_SURFACE, |s| s.service_id);
        let mut input = Self {
            hid,
            indigo: None,
            adapter,
            rsd,
            touchscreen,
            last_release: None,
            settle: SETTLE,
            held: None,
            keyboard,
            keys_down: BTreeSet::new(),
            hardware: None,
        };
        // A "finger up" with no finger down does nothing, but the first report on a new
        // connection is sometimes lost and takes a gesture with it (PITFALLS #8).
        input.report(TOUCHSCREEN_STATE_RELEASE, 65535, 65535).await?;
        tokio::time::sleep(WARMUP).await;
        input.report(TOUCHSCREEN_STATE_RELEASE, 65535, 65535).await?;
        // The same for the keyboard: on a new connection the first `type` lost every key once
        // (PITFALLS #37). A report with no key down does nothing.
        input.key_report(&[]).await?;
        tokio::time::sleep(Duration::from_millis(60)).await;
        Ok(input)
    }

    async fn report(&mut self, state: u8, x: u16, y: u16) -> Result<()> {
        let bytes = build_touchscreen_report(state, x, y, None);
        let began = Instant::now();
        let r = self
            .hid
            .send_report(self.touchscreen, bytes)
            .await
            .map_err(|e| anyhow!("touch report failed (connection error: {e:?})"));
        crate::trace::line("hid", format_args!("{state} {x} {y} {}", began.elapsed().as_micros()));
        r
    }

    async fn gap(&self) {
        if let Some(t) = self.last_release {
            let since = t.elapsed();
            if since < TOUCH_GAP {
                tokio::time::sleep(TOUCH_GAP - since).await;
            }
        }
    }

    async fn release(&mut self, x: u16, y: u16) -> Result<()> {
        let r = self.report(TOUCHSCREEN_STATE_RELEASE, x, y).await;
        self.last_release = Some(Instant::now());
        r
    }

    /// Whether a live touch has a finger down.
    pub fn touching(&self) -> bool {
        self.held.is_some()
    }

    /// One step of a live touch, at `at` in screen points. `Up` may leave out the point: the finger
    /// lifts where it is.
    ///
    /// The reports go out as the calls come, so the path is the caller's. There is no gap before
    /// a contact (PITFALLS #8) because a person's two quick clicks are meant as a double tap. A
    /// `Move` with no finger down puts one down; an `Up` with none does nothing.
    pub async fn touch(&mut self, display: &Display, phase: TouchPhase, at: Option<(f64, f64)>) -> Result<()> {
        let point = at;
        let at = at.map(|p| display.report_point(p));
        match (phase, self.held) {
            (TouchPhase::Down | TouchPhase::Move, held) => {
                let (Some(at), Some(point)) = (at, point) else { bail!("a touch down or move needs a point") };
                // A finger that stays put sends nothing: the device keeps it down (a long press).
                if held.is_some_and(|h| h.at == at) {
                    return Ok(());
                }
                self.report(TOUCHSCREEN_STATE_CONTACT, at.0, at.1).await?;
                let (since, origin, moved) = held.map_or((Instant::now(), point, false), |h| (h.since, h.origin, h.moved));
                let moved = moved || (point.0 - origin.0).hypot(point.1 - origin.1) > TAP_SLOP;
                self.held = Some(Held { at, since, reported: Instant::now(), origin, moved });
                Ok(())
            }
            (TouchPhase::Up, None) => Ok(()),
            (TouchPhase::Up, Some(h)) => {
                let at = at.unwrap_or(h.at);
                if at != h.at {
                    self.report(TOUCHSCREEN_STATE_CONTACT, at.0, at.1).await?;
                } else if h.moved && h.reported.elapsed() >= RESTING {
                    // The device takes the speed of the last reports that moved, however long
                    // ago: a finger that rested and then lifted would fling the list. Show it
                    // still, with samples that move by a pixel (PITFALLS #17).
                    for i in 0..STILL_SAMPLES {
                        let x = if i % 2 == 0 { at.0.saturating_add(STILL_WOBBLE) } else { at.0 };
                        self.report(TOUCHSCREEN_STATE_CONTACT, x, at.1).await?;
                        tokio::time::sleep(DRAG_STEP).await;
                    }
                }
                // A trackpad tap-to-click sends down and up within a millisecond or two; hold
                // for as long as a tap does, or the device may miss the touch.
                if let Some(wait) = TAP_HOLD.checked_sub(h.since.elapsed()) {
                    tokio::time::sleep(wait).await;
                }
                self.held = None;
                self.release(at.0, at.1).await
            }
        }
    }

    /// Lifts a live touch's finger, if one is down, before a gesture of our own.
    async fn lift(&mut self) -> Result<()> {
        match self.held.take() {
            Some(h) => self.release(h.at.0, h.at.1).await,
            None => Ok(()),
        }
    }

    /// Finger down at `at` (screen points as the user sees them), held for `hold`, then lifted.
    /// `on_contact` runs as the finger goes down, after any wait before it.
    pub async fn tap(&mut self, display: &Display, at: (f64, f64), hold: Duration, on_contact: impl FnOnce()) -> Result<()> {
        self.lift().await?;
        let (x, y) = display.report_point(at);
        self.gap().await;
        self.report(TOUCHSCREEN_STATE_CONTACT, x, y).await?;
        on_contact();
        tokio::time::sleep(hold).await;
        self.release(x, y).await
    }

    /// One finger from `from` to `to` over `duration`, then lifted.
    pub async fn drag(
        &mut self,
        display: &Display,
        from: (f64, f64),
        to: (f64, f64),
        duration: Duration,
        motion: Motion,
    ) -> Result<()> {
        self.lift().await?;
        let (from, to) = (display.report_point(from), display.report_point(to));
        self.gap().await;
        let steps = (duration.as_millis() / DRAG_STEP.as_millis()).clamp(4, 240) as u32;
        let start = Instant::now();
        let mut last = from;
        let mut sent = false;
        let moved: Result<()> = async {
            for i in 0..=steps {
                let t = f64::from(i) / f64::from(steps);
                let eased = match motion {
                    Motion::Flick => t,
                    // Cubic ease-out: fast at first, almost still at the end.
                    Motion::Place => 1.0 - (1.0 - t).powi(3),
                };
                let lerp = |a: u16, b: u16| (f64::from(a) + (f64::from(b) - f64::from(a)) * eased).round() as u16;
                last = (lerp(from.0, to.0), lerp(from.1, to.1));
                self.report(TOUCHSCREEN_STATE_CONTACT, last.0, last.1).await?;
                sent = true;
                let due = duration.mul_f64(t);
                if let Some(wait) = due.checked_sub(start.elapsed()) {
                    tokio::time::sleep(wait).await;
                }
            }
            if motion == Motion::Place {
                // The last samples must say "not moving" or the list flings on release.
                for _ in 0..4 {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    self.report(TOUCHSCREEN_STATE_CONTACT, to.0, to.1).await?;
                }
            }
            Ok(())
        }
        .await;
        // Always lift a finger that went down, also after a failed sample.
        if sent {
            let lifted = self.release(last.0, last.1).await;
            moved?;
            lifted
        } else {
            moved
        }
    }

    /// Presses and releases a hardware button.
    pub async fn button(&mut self, button: Button) -> Result<()> {
        if self.indigo.is_none() {
            let indigo = step(
                "HID button service connection",
                IndigoHidClient::connect_rsd(&mut self.adapter, &mut self.rsd),
            )
            .await?;
            self.indigo = Some(indigo);
            // Its devices come after those already asked for, so the touch connection's next
            // answer comes once they are made too.
            devices_ready(&mut self.hid).await;
            tokio::time::sleep(WARMUP).await;
        }
        let indigo = self.indigo.as_mut().expect("connected above");
        let (page, usage, hold) = button.usage();
        indigo
            .send_button(page, usage, ButtonState::Down)
            .await
            .map_err(|e| anyhow!("button down failed (connection error: {e:?})"))?;
        tokio::time::sleep(hold).await;
        indigo
            .send_button(page, usage, ButtonState::Up)
            .await
            .map_err(|e| anyhow!("button up failed (connection error: {e:?})"))?;
        self.last_release = Some(Instant::now());
        self.settle = self.settle.max(BUTTON_SETTLE);
        Ok(())
    }

    async fn key_report(&mut self, down: &[u8]) -> Result<()> {
        let began = Instant::now();
        let r = match self.hardware.as_mut() {
            Some(k) => {
                let usages = down.iter().filter_map(|u| KeyboardUsage::new(u16::from(*u)).ok());
                self.hid
                    .set_main_keyboard_usages(k, usages)
                    .await
                    .map_err(|e| anyhow!("key report failed (connection error: {e})"))
            }
            None => self
                .hid
                .send_report(self.keyboard, keyboard::report(down.iter().copied()))
                .await
                .map_err(|e| anyhow!("key report failed (connection error: {e:?})")),
        };
        crate::trace::line("key", format_args!("{down:02x?} {}", began.elapsed().as_micros()));
        r
    }

    /// Presses and releases one key with its modifiers, the way a person types: modifiers down
    /// first, then the key, then the key up, then the modifiers. A modifier sent in the same
    /// report as its key was sometimes missed (old PITFALLS #18: cmd+v typed a Hangul letter).
    pub async fn press(&mut self, stroke: &Stroke) -> Result<()> {
        self.release_keys().await?;
        let mods = stroke.modifiers.clone();
        if !mods.is_empty() {
            self.key_report(&mods).await?;
            tokio::time::sleep(KEY_STEP).await;
        }
        let mut all = mods.clone();
        all.push(stroke.usage);
        self.key_report(&all).await?;
        tokio::time::sleep(KEY_STEP).await;
        if !mods.is_empty() {
            self.key_report(&mods).await?;
            tokio::time::sleep(KEY_STEP).await;
        }
        self.key_report(&[]).await?;
        tokio::time::sleep(KEY_STEP).await;
        Ok(())
    }

    /// One step of a key a client holds (the viewer's keyboard): `down` adds it to the keys that
    /// are down, `!down` takes it away, and the report carries the whole set. A key that is
    /// already in that state sends nothing.
    pub async fn key(&mut self, usage: u8, down: bool) -> Result<()> {
        let changed = if down { self.keys_down.insert(usage) } else { self.keys_down.remove(&usage) };
        if !changed {
            return Ok(());
        }
        let set: Vec<u8> = self.keys_down.iter().copied().collect();
        self.key_report(&set).await
    }

    /// Keys a client holds down.
    pub fn keys_down(&self) -> Vec<u8> {
        self.keys_down.iter().copied().collect()
    }

    /// Releases every key a client holds, before keys of our own or when the client goes away.
    pub async fn release_keys(&mut self) -> Result<()> {
        if self.keys_down.is_empty() {
            return Ok(());
        }
        self.keys_down.clear();
        self.key_report(&[]).await
    }

    /// Attaches a keyboard the device takes for a hardware one, and sends keys to it from now on.
    ///
    /// The keyboard surface every connection has is a "software keyboard": the on-screen
    /// keyboard stays up and its layout is fixed, whatever the input source (PITFALLS #35). A
    /// keyboard made with `createService` (idevice's `mainKeyboard`) is a hardware one: the
    /// on-screen keyboard goes away, the keys follow the input source (Hangul composes with the
    /// Korean one, ctrl+space switches), and it lasts until it is removed or this connection
    /// closes (PITFALLS #36).
    pub async fn connect_keyboard(&mut self) -> Result<()> {
        if self.hardware.is_some() {
            return Ok(());
        }
        self.release_keys().await?;
        let k = self
            .hid
            .create_main_keyboard()
            .await
            .map_err(|e| anyhow!("could not attach the hardware keyboard ({e}); retry, or type without it"))?;
        self.hardware = Some(k);
        // Keys sent 250 ms after registering were lost, 800 ms worked (iphone-cli
        // `src/device/input.rs`, MIT, see THIRD_PARTY_NOTICES.md). A report with nothing down
        // first, as on a new connection: the first key after attaching on a fresh connection
        // lost its Shift once (PITFALLS #38).
        tokio::time::sleep(KEYBOARD_ATTACH / 2).await;
        self.key_report(&[]).await?;
        tokio::time::sleep(KEYBOARD_ATTACH / 2).await;
        Ok(())
    }

    /// Removes the hardware keyboard, if one is attached; the on-screen keyboard comes back.
    pub async fn disconnect_keyboard(&mut self) -> Result<()> {
        let Some(mut k) = self.hardware.take() else { return Ok(()) };
        self.keys_down.clear();
        if let Err(e) = self.hid.remove_main_keyboard(&mut k).await {
            if k.is_active() {
                self.hardware = Some(k);
            }
            bail!("could not remove the hardware keyboard ({e}); retry `keyboard disconnect`");
        }
        Ok(())
    }

    pub fn keyboard_connected(&self) -> bool {
        self.hardware.is_some()
    }

    /// Lifts a finger left down, lets the last release sink in, then closes the connections.
    pub async fn close(mut self) {
        if let Err(e) = self.lift().await {
            tracing::warn!("could not lift the finger before closing HID: {e:#}");
        }
        if let Err(e) = self.release_keys().await {
            tracing::warn!("could not release the keys before closing HID: {e:#}");
        }
        // The device removes it with the connection too; removing it first says so for sure.
        if let Err(e) = self.disconnect_keyboard().await {
            tracing::warn!("{e:#}");
        }
        if let Some(t) = self.last_release {
            let since = t.elapsed();
            if since < self.settle {
                tokio::time::sleep(self.settle - since).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(service_id: u64, product: &str) -> HidSurface {
        HidSurface { service_id, product: Some(product.into()), primary_usage: None, primary_usage_page: None }
    }

    #[test]
    fn touchscreens_are_named_by_their_display() {
        let list = [
            surface(257, "CoreDevice touchscreen(nil)"),
            surface(512, "CoreDevice keyboard"),
            surface(1281, "CoreDevice touchscreenGesture"),
        ];
        assert_eq!(touchscreens(&list), [(257, "nil".to_string())]);
    }

    #[test]
    fn button_names() {
        assert_eq!(Button::parse("Home").unwrap(), Button::Home);
        assert_eq!(Button::parse("volume-up").unwrap(), Button::VolumeUp);
        assert_eq!(Button::parse("power").unwrap(), Button::Lock);
        assert!(Button::parse("menu").is_err());
    }
}
