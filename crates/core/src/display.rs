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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum Rotation {
    #[serde(rename = "rot0")]
    #[default]
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

    /// The inverse of [`Self::turn_share`]: where a point of the turned space lies on the screen.
    pub(crate) fn unturn_share(self, (p, q): (f64, f64)) -> (f64, f64) {
        match self {
            Self::R0 => (p, q),
            Self::R90 => (1.0 - q, p),
            Self::R180 => (1.0 - p, 1.0 - q),
            Self::R270 => (q, 1.0 - p),
        }
    }

    /// Whether the turn swaps width and height.
    pub(crate) fn is_quarter(self) -> bool {
        matches!(self, Self::R90 | Self::R270)
    }

    fn quarters(self) -> u8 {
        match self {
            Self::R0 => 0,
            Self::R90 => 1,
            Self::R180 => 2,
            Self::R270 => 3,
        }
    }

    /// This turn followed by `other`. Quarter turns of a square's shares commute, so the order
    /// doesn't matter.
    pub(crate) fn then(self, other: Rotation) -> Rotation {
        match (self.quarters() + other.quarters()) % 4 {
            0 => Self::R0,
            1 => Self::R90,
            2 => Self::R180,
            _ => Self::R270,
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
    /// How the touchscreen is turned from the panel (`nativeOrientation`): `rot270` on an iPad,
    /// whose touch reports stay portrait while its panel is landscape (PITFALLS #58).
    pub touch_rotation: Rotation,
    /// One of a foldable's panels (the device marks them `active`, PITFALLS #47).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub foldable: bool,
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

    /// The turn of the space SpringBoard's accessibility frames come in, from the screen: the
    /// touchscreen's (PITFALLS #61), and on a foldable not turned with the interface either: there
    /// SpringBoard's frames were in the panel's own orientation (PITFALLS #70).
    pub fn system_frame_turn(&self) -> Rotation {
        if self.foldable { self.rotation.then(self.touch_rotation) } else { self.touch_rotation }
    }

    /// The screen's size in pixels, native orientation, as screenshots have it.
    pub fn panel_px(&self) -> (f64, f64) {
        ((self.panel.0 * self.scale).round(), (self.panel.1 * self.scale).round())
    }

    /// The screen's size in a video picture of `coded` pixels: the top-left part of it, in the
    /// panel's native orientation (PITFALLS #11, #27). A panel larger than the stream's picture
    /// arrives scaled down to fit it, with the encoder's padding after it: a foldable's inner
    /// panel, 2007x2853, in a 1408x2016 picture (PITFALLS #48). Drawing the whole panel size
    /// from such a picture took in the padding, a black band at an edge.
    pub fn picture_px(&self, coded: (u32, u32)) -> (f64, f64) {
        let (pw, ph) = self.panel_px();
        let (cw, ch) = (f64::from(coded.0), f64::from(coded.1));
        if pw <= 0.0 || ph <= 0.0 || cw <= 0.0 || ch <= 0.0 {
            return (pw, ph);
        }
        let k = (cw / pw).min(ch / ph).min(1.0);
        ((pw * k).round().min(cw), (ph * k).round().min(ch))
    }

    /// The `displayUniqueID` a screenshot of this panel asks for; `None` on a phone with one
    /// panel, which has no id (PITFALLS #9) and is the panel the device captures without one.
    pub fn capture_id(&self) -> Option<&str> {
        Some(self.unique_id.as_str()).filter(|id| *id != NO_UNIQUE_ID)
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
        let (u, v) = self.touch_rotation.turn_share(self.rotation.turn_share((x / w, y / h)));
        let unit = |s: f64| (s * 65535.0).round().clamp(0.0, 65535.0) as u16;
        (unit(u), unit(v))
    }
}

/// What stands in for the `uniqueId` a phone with one panel lacks (PITFALLS #9): its touchscreen
/// is `CoreDevice touchscreen(nil)`.
pub(crate) const NO_UNIQUE_ID: &str = "nil";

/// The turn from the device's orientation, `getdisplayinfo`'s top-level `orientation`: the last
/// one held upright (`currentDeviceNonFlatOrientation`), so a phone laid flat keeps the screen as it
/// was. Both panels of a foldable have a native orientation of `rot0`, so this is the rotation from
/// the panel too. The device's orientation is not the interface's: an app that stays in portrait
/// would be turned while the phone is held sideways (PITFALLS #48).
fn device_rotation(output: &plist::Value) -> Option<Rotation> {
    let o = output.as_dictionary()?.get("orientation")?.as_dictionary()?;
    let name = ["currentDeviceNonFlatOrientation", "currentDeviceOrientation"]
        .iter()
        .filter_map(|k| o.get(k).and_then(|v| v.as_string()))
        .find(|n| !matches!(*n, "unknown" | "faceUp" | "faceDown"))?;
    Some(match name {
        "portrait" => Rotation::R0,
        "portraitUpsideDown" => Rotation::R180,
        // Seen: in landscapeLeft the interface's top lay along the picture's right edge, which
        // is rot270 (as on a phone with one panel, PITFALLS #27); landscapeRight the opposite.
        "landscapeLeft" => Rotation::R270,
        "landscapeRight" => Rotation::R90,
        _ => return None,
    })
}

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
        rotation: {
            let panel = Rotation::parse(text("currentOrientation")?)?;
            // A foldable's panels keep the `currentOrientation` they had (the inner one `rot90`,
            // the cover `rot0`) through every turn, while the interface turns inside the picture;
            // only the device's orientation follows (PITFALLS #48).
            if marks_active { device_rotation(output).unwrap_or(panel) } else { panel }
        },
        // Phones and both panels of a foldable say `rot0`, or nothing on older systems.
        touch_rotation: display
            .get("nativeOrientation")
            .and_then(|v| v.as_string())
            .and_then(|r| Rotation::parse(r).ok())
            .unwrap_or(Rotation::R0),
        foldable: marks_active,
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
            touch_rotation: Rotation::R0,
            foldable: false,
        }
    }

    #[test]
    fn turns_add_up() {
        assert_eq!(Rotation::R90.then(Rotation::R90), Rotation::R180);
        assert_eq!(Rotation::R270.then(Rotation::R180), Rotation::R90);
        assert_eq!(Rotation::R0.then(Rotation::R270), Rotation::R270);
        for a in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
            for b in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
                let p = (0.25, 0.625);
                assert_eq!(a.then(b).turn_share(p), b.turn_share(a.turn_share(p)), "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn springboard_frames_on_a_foldable_are_not_turned_with_the_interface() {
        // Seen on a foldable's inner panel held upside down (iOS 27.3): rot180, and SpringBoard's
        // frames in the panel's own orientation (PITFALLS #70). A phone with one panel: only the
        // touchscreen's turn (PITFALLS #61).
        let folded = Display { foldable: true, ..display(Rotation::R180) };
        assert_eq!(folded.system_frame_turn(), Rotation::R180);
        assert_eq!(display(Rotation::R180).system_frame_turn(), Rotation::R0);
        let ipad = Display { touch_rotation: Rotation::R270, ..display(Rotation::R0) };
        assert_eq!(ipad.system_frame_turn(), Rotation::R270);
    }

    #[test]
    fn an_ipad_touchscreen_is_turned_from_its_panel() {
        // Seen on an iPad Pro 13-inch held in landscape: the panel 1376x1032 points, rot0,
        // nativeOrientation rot270; points sent unturned swapped every drag's axis (PITFALLS #58).
        // The old implementation's rot270 rule, checked there by tapping an icon: (u, v) lands on
        // (1 - v, u).
        let d = Display { panel: (1376.0, 1032.0), touch_rotation: Rotation::R270, ..display(Rotation::R0) };
        assert_eq!(d.report_point((0.0, 0.0)), (65535, 0));
        assert_eq!(d.report_point((1376.0, 1032.0)), (0, 65535));
        // A drag along the screen's width moves along the touchscreen's height.
        let (a, b) = (d.report_point((100.0, 516.0)), d.report_point((1200.0, 516.0)));
        assert_eq!(a.0, b.0);
        assert!(b.1 > a.1);
    }

    #[test]
    fn unturning_undoes_every_turn() {
        for r in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
            for p in [(0.0, 0.0), (0.25, 0.75), (1.0, 0.5)] {
                assert_eq!(r.unturn_share(r.turn_share(p)), p, "{r:?}");
            }
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
        foldable_held(cover_active, None)
    }

    /// [`foldable`] with the top-level `orientation` the device reports, if any.
    fn foldable_held(cover_active: bool, held: Option<&str>) -> plist::Value {
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
        if let Some(held) = held {
            let mut o = plist::Dictionary::new();
            o.insert("currentDeviceOrientation".into(), plist::Value::String(held.into()));
            o.insert("currentDeviceNonFlatOrientation".into(), plist::Value::String(held.into()));
            o.insert("currentDeviceOrientationLocked".into(), plist::Value::Boolean(false));
            out.insert("orientation".into(), plist::Value::Dictionary(o));
        }
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
    fn a_foldable_turns_with_the_device_not_its_panels_stale_orientation() {
        // Seen on iOS 27.3: the inner panel says rot90 and the cover rot0 whichever way the phone
        // is held; the interface follows the device's orientation (PITFALLS #48).
        for (held, rotation) in [
            ("portrait", Rotation::R0),
            ("landscapeLeft", Rotation::R270),
            ("landscapeRight", Rotation::R90),
            ("portraitUpsideDown", Rotation::R180),
        ] {
            assert_eq!(parse(&foldable_held(false, Some(held))).unwrap().rotation, rotation, "inner, {held}");
            assert_eq!(parse(&foldable_held(true, Some(held))).unwrap().rotation, rotation, "cover, {held}");
        }
        let inner = parse(&foldable_held(false, Some("portrait"))).unwrap();
        assert_eq!((inner.size(), inner.orientation()), ((669.0, 951.0), "portrait"));
        // Laid flat: the last orientation held upright.
        let mut flat = foldable_held(false, Some("landscapeLeft"));
        let o = flat.as_dictionary_mut().unwrap().get_mut("orientation").unwrap().as_dictionary_mut().unwrap();
        o.insert("currentDeviceOrientation".into(), plist::Value::String("faceUp".into()));
        assert_eq!(parse(&flat).unwrap().rotation, Rotation::R270);
        // No device orientation: the panel's own.
        assert_eq!(parse(&foldable(false)).unwrap().rotation, Rotation::R90);
    }

    #[test]
    fn a_phone_with_one_panel_keeps_its_panels_orientation() {
        // There `currentOrientation` is the interface orientation, which also covers apps that
        // stay in portrait while the phone is sideways (PITFALLS #27).
        let mut out = parse_input_one_panel("rot0");
        let mut o = plist::Dictionary::new();
        o.insert("currentDeviceNonFlatOrientation".into(), plist::Value::String("landscapeLeft".into()));
        out.as_dictionary_mut().unwrap().insert("orientation".into(), plist::Value::Dictionary(o));
        assert_eq!(parse(&out).unwrap().rotation, Rotation::R0);
    }

    fn parse_input_one_panel(rot: &str) -> plist::Value {
        let arr = |v: Vec<plist::Value>| plist::Value::Array(v);
        let real = |f: f64| plist::Value::Real(f);
        let mut d = plist::Dictionary::new();
        d.insert("primary".into(), plist::Value::Boolean(true));
        d.insert("displayId".into(), plist::Value::Integer(1.into()));
        d.insert("pointScale".into(), real(3.0));
        d.insert("currentOrientation".into(), plist::Value::String(rot.into()));
        d.insert("bounds".into(), arr(vec![arr(vec![real(0.0), real(0.0)]), arr(vec![real(1284.0), real(2778.0)])]));
        let mut out = plist::Dictionary::new();
        out.insert("displays".into(), arr(vec![plist::Value::Dictionary(d)]));
        plist::Value::Dictionary(out)
    }

    #[test]
    fn the_screen_fills_a_picture_it_fits_in_and_is_scaled_into_a_smaller_one() {
        let px = |w: f64, h: f64| Display { panel: (w / 3.0, h / 3.0), ..display(Rotation::R0) };
        // A phone: the picture is the panel plus padding (PITFALLS #11).
        assert_eq!(px(1284.0, 2778.0).picture_px((1296, 2816)), (1284.0, 2778.0));
        // A foldable's cover panel: the same.
        assert_eq!(px(1398.0, 2034.0).picture_px((1408, 2048)), (1398.0, 2034.0));
        // Its inner panel arrives scaled to the picture's width, padding below (PITFALLS #48).
        assert_eq!(px(2007.0, 2853.0).picture_px((1408, 2016)), (1408.0, 2002.0));
        // The turn doesn't matter: the picture is always the panel in its native orientation.
        let turned = Display { rotation: Rotation::R90, ..px(2007.0, 2853.0) };
        assert_eq!(turned.picture_px((1408, 2016)), (1408.0, 2002.0));
        // No picture size yet: the panel's.
        assert_eq!(px(2007.0, 2853.0).picture_px((0, 0)), (2007.0, 2853.0));
    }

    #[test]
    fn a_turn_is_the_same_panel() {
        let upright = display(Rotation::R0);
        let turned = display(Rotation::R90);
        assert_ne!(upright, turned);
        assert!(upright.same_panel(&turned));
    }
}
