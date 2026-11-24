//! Touch and hardware-button input over CoreDevice HID.
//!
//! Touches go to `hid.universalhidservice` as touchscreen reports, buttons to `hid.indigo`. The
//! device only acts on them while a screen-sharing media stream runs (PITFALLS #7), so callers
//! keep a [`crate::MediaSession`] open for as long as an [`Input`] is.
//!
//! The waits (devices listed, warm-up, the "released" first report, tap hold, gap between
//! touches, settle before closing) are adapted from iphone-cli `src/device/input.rs`, MIT, see
//! THIRD_PARTY_NOTICES.md. PITFALLS #8 has what each one prevents.

use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use idevice::{
    ReadWrite, RsdService,
    core_device::hid::{
        ButtonState, DIGITIZER_SURFACE_MAIN_TOUCHSCREEN, HidSurface, IndigoHidClient,
        TOUCHSCREEN_STATE_CONTACT, TOUCHSCREEN_STATE_RELEASE, UniversalHidServiceClient,
        build_touchscreen_report,
    },
    rsd::RsdHandshake,
    tcp::handle::AdapterHandle,
};
use serde::{Deserialize, Serialize};

use crate::{display::Display, step};

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

/// A hardware button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    Home,
    Lock,
    VolumeUp,
    VolumeDown,
}

impl Button {
    pub const NAMES: &str = "home, lock, volume_up, volume_down";

    pub fn parse(name: &str) -> Result<Self> {
        Ok(match name.trim().to_lowercase().replace('-', "_").as_str() {
            "home" => Self::Home,
            "lock" | "power" => Self::Lock,
            "volume_up" => Self::VolumeUp,
            "volume_down" => Self::VolumeDown,
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
}

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
        let mut input = Self {
            hid,
            indigo: None,
            adapter,
            rsd,
            touchscreen,
            last_release: None,
            settle: SETTLE,
            held: None,
        };
        // A "finger up" with no finger down does nothing, but the first report on a new
        // connection is sometimes lost and takes a gesture with it (PITFALLS #8).
        input.report(TOUCHSCREEN_STATE_RELEASE, 65535, 65535).await?;
        tokio::time::sleep(WARMUP).await;
        input.report(TOUCHSCREEN_STATE_RELEASE, 65535, 65535).await?;
        tokio::time::sleep(Duration::from_millis(60)).await;
        Ok(input)
    }

    async fn report(&mut self, state: u8, x: u16, y: u16) -> Result<()> {
        let bytes = build_touchscreen_report(state, x, y, None);
        self.hid
            .send_report(self.touchscreen, bytes)
            .await
            .map_err(|e| anyhow!("touch report failed (connection error: {e:?})"))
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
        let at = at.map(|p| display.report_point(p));
        match (phase, self.held) {
            (TouchPhase::Down | TouchPhase::Move, held) => {
                let at = at.ok_or_else(|| anyhow!("a touch down or move needs a point"))?;
                // A finger that stays put sends nothing: the device keeps it down (a long press).
                if held.is_some_and(|h| h.at == at) {
                    return Ok(());
                }
                self.report(TOUCHSCREEN_STATE_CONTACT, at.0, at.1).await?;
                let since = held.map_or_else(Instant::now, |h| h.since);
                self.held = Some(Held { at, since, reported: Instant::now() });
                Ok(())
            }
            (TouchPhase::Up, None) => Ok(()),
            (TouchPhase::Up, Some(h)) => {
                let at = at.unwrap_or(h.at);
                if at != h.at {
                    self.report(TOUCHSCREEN_STATE_CONTACT, at.0, at.1).await?;
                } else if h.reported.elapsed() >= RESTING {
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
    pub async fn tap(&mut self, display: &Display, at: (f64, f64), hold: Duration) -> Result<()> {
        self.lift().await?;
        let (x, y) = display.report_point(at);
        self.gap().await;
        self.report(TOUCHSCREEN_STATE_CONTACT, x, y).await?;
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

    /// Lifts a finger left down, lets the last release sink in, then closes the connections.
    pub async fn close(mut self) {
        if let Err(e) = self.lift().await {
            tracing::warn!("could not lift the finger before closing HID: {e:#}");
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
