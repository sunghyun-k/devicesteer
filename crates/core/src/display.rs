//! The display in use, from CoreDevice's `getdisplayinfo`: its size in points, how it is turned,
//! and the id the screen-sharing service streams it by.
//!
//! Touch reports are normalized across the panel in its native orientation (PITFALLS #6), so a
//! point on the screen as the user sees it has to be turned onto the panel first.
//!
//! The answer's shape and the rotation mapping are adapted from iphone-cli
//! `src/device/display.rs`, MIT, see THIRD_PARTY_NOTICES.md.

use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use idevice::{RsdService, rsd::RsdHandshake, tcp::handle::AdapterHandle};
use serde::Serialize;

use crate::{coredevice::xpc_link, step};

xpc_link!(DeviceInfoLink, "com.apple.coredevice.deviceinfo");

const FEATURE_DISPLAY_INFO: &str = "com.apple.coredevice.feature.getdisplayinfo";

/// How the display is turned from its panel's native orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Rotation {
    #[serde(rename = "rot0")]
    R0,
    #[serde(rename = "rot90")]
    R90,
    #[serde(rename = "rot180")]
    R180,
    #[serde(rename = "rot270")]
    R270,
}

impl Rotation {
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "rot0" => Self::R0,
            "rot90" => Self::R90,
            "rot180" => Self::R180,
            "rot270" => Self::R270,
            other => bail!("unknown display orientation `{other}`"),
        })
    }

    /// Where a point of the turned screen, as shares of its width and height, lies on the panel.
    pub(crate) fn turn_share(self, (u, v): (f64, f64)) -> (f64, f64) {
        match self {
            Self::R0 => (u, v),
            Self::R90 => (v, 1.0 - u),
            Self::R180 => (1.0 - u, 1.0 - v),
            Self::R270 => (1.0 - v, u),
        }
    }
}

/// The display in use.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Display {
    /// Also part of its touchscreen's HID product name on phones with two panels.
    pub unique_id: String,
    /// The id the screen-sharing service streams this display by.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_id: Option<i64>,
    /// The panel's size in points (native orientation).
    pub panel: (f64, f64),
    /// Pixels per point.
    pub scale: f64,
    pub rotation: Rotation,
}

impl Display {
    /// `portrait` or `landscape`: how the screen the user sees is turned, from its size. The
    /// rotation alone doesn't say it, since it is relative to the panel's native orientation.
    pub fn orientation(&self) -> &'static str {
        let (w, h) = self.size();
        if w > h { "landscape" } else { "portrait" }
    }

    /// The screen's size in points as the user sees it (width and height swap at a quarter turn).
    pub fn size(&self) -> (f64, f64) {
        let (w, h) = self.panel;
        match self.rotation {
            Rotation::R90 | Rotation::R270 => (h, w),
            _ => (w, h),
        }
    }

    /// Whether `other` is the same panel: a foldable switches panels when it is folded or
    /// unfolded (PITFALLS #47), and the video and the touchscreen are bound to one.
    pub fn same_panel(&self, other: &Display) -> bool {
        self.unique_id == other.unique_id && self.display_id == other.display_id
    }

    /// Touch-report coordinates (0..=65535 across the panel) for a point on the screen as the user
    /// sees it. Out-of-range points are clamped: the device drops them silently (PITFALLS #6).
    pub fn report_point(&self, (x, y): (f64, f64)) -> (u16, u16) {
        let (w, h) = self.size();
        let (u, v) = self.rotation.turn_share((x / w, y / h));
        let unit = |s: f64| (s * 65535.0).round().clamp(0.0, 65535.0) as u16;
        (unit(u), unit(v))
    }
}

/// What stands in for the `uniqueId` a phone with one panel lacks (PITFALLS #9): its touchscreen
/// is `CoreDevice touchscreen(nil)`.
pub(crate) const NO_UNIQUE_ID: &str = "nil";

/// The active display from a `getdisplayinfo` output.
///
/// The device also lists wireless displays of no size. A foldable marks its panels with `active`
/// and names them by `uniqueId`; a phone with one panel has neither key (PITFALLS #9), so there the
/// sized `primary` display is the one, and its touchscreen is `touchscreen(nil)`.
fn parse(output: &plist::Value) -> Result<Display> {
    let displays = output
        .as_dictionary()
        .and_then(|o| o.get("displays"))
        .and_then(|d| d.as_array())
        .ok_or_else(|| anyhow!("getdisplayinfo lists no displays"))?;
    let number = |v: &plist::Value| v.as_real().or_else(|| v.as_signed_integer().map(|i| i as f64));
    let size = |d: &plist::Dictionary| -> Option<(f64, f64)> {
        let corner = d.get("bounds")?.as_array()?.get(1)?.as_array()?;
        Some((number(corner.first()?)?, number(corner.get(1)?)?))
    };
    let flag = |d: &plist::Dictionary, key: &str| d.get(key).and_then(|a| a.as_boolean());
    let sized: Vec<&plist::Dictionary> = displays
        .iter()
        .filter_map(|d| d.as_dictionary())
        .filter(|d| size(d).is_some_and(|(w, h)| w > 0.0 && h > 0.0))
        .collect();
    let marks_active = sized.iter().any(|d| flag(d, "active").is_some());
    let candidates: Vec<&plist::Dictionary> = sized
        .into_iter()
        .filter(|d| {
            if marks_active {
                flag(d, "active") == Some(true)
            } else {
                flag(d, "primary") != Some(false)
            }
        })
        .collect();
    let [display] = candidates[..] else {
        bail!("{} displays are active, expected one", candidates.len());
    };
    let text = |key: &str| {
        display
            .get(key)
            .and_then(|v| v.as_string())
            .ok_or_else(|| anyhow!("the active display has no {key}"))
    };
    let scale = display
        .get("pointScale")
        .and_then(number)
        .filter(|s| *s > 0.0)
        .ok_or_else(|| anyhow!("the active display has no pointScale"))?;
    let (w, h) = size(display).ok_or_else(|| anyhow!("the active display has no bounds"))?;
    Ok(Display {
        unique_id: display.get("uniqueId").and_then(|v| v.as_string()).unwrap_or(NO_UNIQUE_ID).to_string(),
        display_id: display.get("displayId").and_then(|v| v.as_signed_integer()),
        panel: (w / scale, h / scale),
        scale,
        rotation: Rotation::parse(text("currentOrientation")?)?,
    })
}

/// How long one `getdisplayinfo` may take. It answers in 10 to 50 ms, but now and then not at
/// all: a call left waiting held the device for the 12 s connection timeout (PITFALLS #27).
const ASK_TIMEOUT: Duration = Duration::from_millis(1500);
/// Tries before giving up, each after a silent one on a new connection.
const ASK_TRIES: u32 = 3;

/// Asks the device for the display in use, on a connection of its own that it keeps between
/// calls. The interface orientation is only known by asking (PITFALLS #27), so a caller that
/// follows it polls this.
pub struct DisplayPoller {
    adapter: AdapterHandle,
    rsd: RsdHandshake,
    link: Option<DeviceInfoLink>,
}

impl DisplayPoller {
    /// Takes the tunnel handles by value, like [`crate::Input::open`], so the daemon can spawn
    /// the polling future.
    pub fn new(adapter: AdapterHandle, rsd: RsdHandshake) -> Self {
        Self { adapter, rsd, link: None }
    }

    /// The display in use now. A call that gets no answer is dropped with its connection and
    /// tried again on a new one. The final error doesn't say "connection" when the tunnel is fine,
    /// so the daemon keeps its session.
    pub async fn ask(&mut self) -> Result<Display> {
        parse(&self.ask_raw().await?)
    }

    /// `getdisplayinfo`'s whole answer, every panel in it, for probes.
    pub async fn ask_raw(&mut self) -> Result<plist::Value> {
        for _ in 0..ASK_TRIES {
            let mut link = match self.link.take() {
                Some(l) => l,
                None => step("display info connection", DeviceInfoLink::connect_rsd(&mut self.adapter, &mut self.rsd)).await?,
            };
            match tokio::time::timeout(ASK_TIMEOUT, link.invoke(FEATURE_DISPLAY_INFO, idevice::xpc::Dictionary::new(), None)).await {
                Ok(Ok(d)) => {
                    self.link = Some(link);
                    return Ok(d);
                }
                Ok(Err(e)) => return Err(e),
                Err(_) => tracing::debug!("display info: no answer within {ASK_TIMEOUT:?}; asking on a new link"),
            }
        }
        bail!("the device did not say which display is in use ({ASK_TRIES} tries); retry")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(rotation: Rotation) -> Display {
        Display {
            unique_id: "1".into(),
            display_id: Some(1),
            panel: (400.0, 800.0),
            scale: 3.0,
            rotation,
        }
    }

    #[test]
    fn upright_points_scale_across_the_panel() {
        let d = display(Rotation::R0);
        assert_eq!(d.size(), (400.0, 800.0));
        assert_eq!(d.report_point((200.0, 400.0)), (32768, 32768));
        assert_eq!(d.report_point((0.0, 0.0)), (0, 0));
        assert_eq!(d.report_point((-5.0, 900.0)), (0, 65535));
    }

    #[test]
    fn a_quarter_turn_swaps_the_screen_and_turns_points_onto_the_panel() {
        let d = display(Rotation::R270);
        assert_eq!(d.size(), (800.0, 400.0));
        // rot270: (u, v) lands on (1 - v, u).
        assert_eq!(d.report_point((0.0, 0.0)), (65535, 0));
        let d = display(Rotation::R90);
        // rot90: (u, v) lands on (v, 1 - u).
        assert_eq!(d.report_point((0.0, 0.0)), (0, 65535));
    }

    #[test]
    fn orientation_follows_the_turned_size() {
        assert_eq!(display(Rotation::R0).orientation(), "portrait");
        assert_eq!(display(Rotation::R180).orientation(), "portrait");
        assert_eq!(display(Rotation::R90).orientation(), "landscape");
        assert_eq!(display(Rotation::R270).orientation(), "landscape");
    }

    #[test]
    fn a_landscape_corner_lands_on_the_panel_corner_the_video_shows_it_at() {
        // Seen on an iPhone in landscape (rot270): the screen's top-left corner was at the top
        // right of the portrait video picture, which is the panel (PITFALLS #27).
        let d = display(Rotation::R270);
        assert_eq!(d.report_point((0.0, 0.0)), (65535, 0));
        assert_eq!(d.report_point((800.0, 400.0)), (0, 65535));
    }

    #[test]
    fn parse_picks_the_one_active_display_with_a_size() {
        let arr = |v: Vec<plist::Value>| plist::Value::Array(v);
        let real = |f: f64| plist::Value::Real(f);
        let mut on = plist::Dictionary::new();
        on.insert("active".into(), plist::Value::Boolean(true));
        on.insert("uniqueId".into(), plist::Value::String("42".into()));
        on.insert("displayId".into(), plist::Value::Integer(1.into()));
        on.insert("pointScale".into(), real(3.0));
        on.insert("currentOrientation".into(), plist::Value::String("rot0".into()));
        on.insert(
            "bounds".into(),
            arr(vec![arr(vec![real(0.0), real(0.0)]), arr(vec![real(1284.0), real(2778.0)])]),
        );
        let mut empty = plist::Dictionary::new();
        empty.insert("active".into(), plist::Value::Boolean(true));
        let mut out = plist::Dictionary::new();
        out.insert(
            "displays".into(),
            arr(vec![plist::Value::Dictionary(empty), plist::Value::Dictionary(on)]),
        );
        let d = parse(&plist::Value::Dictionary(out.clone())).unwrap();
        assert_eq!(d.panel, (428.0, 926.0));
        assert_eq!(d.unique_id, "42");
        assert_eq!(d.rotation, Rotation::R0);
    }

    #[test]
    fn a_phone_with_one_panel_has_no_active_flag_and_no_unique_id() {
        // Shape seen on an iPhone with iOS 27.2.1: a primary panel and sizeless wireless displays.
        let arr = |v: Vec<plist::Value>| plist::Value::Array(v);
        let real = |f: f64| plist::Value::Real(f);
        let display = |primary: bool, w: f64, h: f64| {
            let mut d = plist::Dictionary::new();
            d.insert("primary".into(), plist::Value::Boolean(primary));
            d.insert("displayId".into(), plist::Value::Integer(if primary { 1 } else { 2 }.into()));
            d.insert("pointScale".into(), plist::Value::Integer(if primary { 3 } else { 1 }.into()));
            d.insert("currentOrientation".into(), plist::Value::String("rot0".into()));
            d.insert(
                "bounds".into(),
                arr(vec![arr(vec![real(0.0), real(0.0)]), arr(vec![real(w), real(h)])]),
            );
            plist::Value::Dictionary(d)
        };
        let mut out = plist::Dictionary::new();
        out.insert("displays".into(), arr(vec![display(true, 1284.0, 2778.0), display(false, 0.0, 0.0)]));
        let d = parse(&plist::Value::Dictionary(out)).unwrap();
        assert_eq!(d.panel, (428.0, 926.0));
        assert_eq!(d.unique_id, "nil");
        assert_eq!(d.display_id, Some(1));
    }

    /// A foldable's answer (shape seen on iOS 27.3): two sized panels with `active`, and a sized
    /// wireless display while a stream runs (it carries the scaled-down video, PITFALLS #47).
    fn foldable(cover_active: bool) -> plist::Value {
        let arr = |v: Vec<plist::Value>| plist::Value::Array(v);
        let real = |f: f64| plist::Value::Real(f);
        let panel = |id: &str, display_id: i64, active: bool, primary: bool, w: f64, h: f64, rot: &str| {
            let mut d = plist::Dictionary::new();
            d.insert("active".into(), plist::Value::Boolean(active));
            d.insert("primary".into(), plist::Value::Boolean(primary));
            d.insert("uniqueId".into(), plist::Value::String(id.into()));
            d.insert("displayId".into(), plist::Value::Integer(display_id.into()));
            d.insert("pointScale".into(), real(3.0));
            d.insert("currentOrientation".into(), plist::Value::String(rot.into()));
            d.insert("bounds".into(), arr(vec![arr(vec![real(0.0), real(0.0)]), arr(vec![real(w), real(h)])]));
            plist::Value::Dictionary(d)
        };
        let mut wireless = plist::Dictionary::new();
        wireless.insert("active".into(), plist::Value::Boolean(false));
        wireless.insert("displayId".into(), plist::Value::Integer(4.into()));
        wireless.insert("pointScale".into(), real(1.0));
        wireless.insert("currentOrientation".into(), plist::Value::String("rot0".into()));
        wireless.insert("bounds".into(), arr(vec![arr(vec![real(0.0), real(0.0)]), arr(vec![real(1408.0), real(2048.0)])]));
        let mut out = plist::Dictionary::new();
        out.insert(
            "displays".into(),
            arr(vec![
                panel("cover", 1, cover_active, true, 1398.0, 2034.0, "rot0"),
                // The panel not in use keeps reporting a stale orientation.
                panel("inner", 2, !cover_active, false, 2007.0, 2853.0, "rot90"),
                plist::Value::Dictionary(wireless),
            ]),
        );
        plist::Value::Dictionary(out)
    }

    #[test]
    fn folding_moves_the_active_display_to_the_other_panel() {
        let open = parse(&foldable(false)).unwrap();
        let folded = parse(&foldable(true)).unwrap();
        assert_eq!((open.unique_id.as_str(), open.display_id, open.panel), ("inner", Some(2), (669.0, 951.0)));
        assert_eq!((folded.unique_id.as_str(), folded.display_id, folded.panel), ("cover", Some(1), (466.0, 678.0)));
        assert!(!open.same_panel(&folded));
    }

    #[test]
    fn a_turn_is_the_same_panel() {
        let upright = display(Rotation::R0);
        let turned = display(Rotation::R90);
        assert_ne!(upright, turned);
        assert!(upright.same_panel(&turned));
    }
}
