//! The display in use, from CoreDevice's `getdisplayinfo`: its size in points, how it is turned,
//! and the id the screen-sharing service streams it by.
//!
//! Touch reports are normalized across the panel in its native orientation (PITFALLS #6), so a
//! point on the screen as the user sees it has to be turned onto the panel first.
//!
//! The answer's shape and the rotation mapping are adapted from iphone-cli
//! `src/device/display.rs`, MIT, see THIRD_PARTY_NOTICES.md.

use anyhow::{Result, anyhow, bail};
use serde::Serialize;

use crate::coredevice::xpc_link;

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
    fn turn(self, (u, v): (f64, f64)) -> (f64, f64) {
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
    /// The screen's size in points as the user sees it (width and height swap at a quarter turn).
    pub fn size(&self) -> (f64, f64) {
        let (w, h) = self.panel;
        match self.rotation {
            Rotation::R90 | Rotation::R270 => (h, w),
            _ => (w, h),
        }
    }

    /// Touch-report coordinates (0..=65535 across the panel) for a point on the screen as the user
    /// sees it. Out-of-range points are clamped: the device drops them silently (PITFALLS #6).
    pub fn report_point(&self, (x, y): (f64, f64)) -> (u16, u16) {
        let (w, h) = self.size();
        let (u, v) = self.rotation.turn((x / w, y / h));
        let unit = |s: f64| (s * 65535.0).round().clamp(0.0, 65535.0) as u16;
        (unit(u), unit(v))
    }
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
        unique_id: display.get("uniqueId").and_then(|v| v.as_string()).unwrap_or("nil").to_string(),
        display_id: display.get("displayId").and_then(|v| v.as_signed_integer()),
        panel: (w / scale, h / scale),
        scale,
        rotation: Rotation::parse(text("currentOrientation")?)?,
    })
}

impl DeviceInfoLink {
    /// The display in use now.
    pub(crate) async fn active_display(&mut self) -> Result<Display> {
        let out = self
            .invoke(FEATURE_DISPLAY_INFO, idevice::xpc::Dictionary::new(), None)
            .await?;
        parse(&out)
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
}
