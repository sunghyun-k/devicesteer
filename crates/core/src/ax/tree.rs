//! A raw dump turned into the element list `ui` prints.
//!
//! The walk (depth-first from each application in the device's child order, visible children
//! first) follows iphone-cli `src/ax/tree.rs`, MIT, see THIRD_PARTY_NOTICES.md.

use std::collections::{HashMap, HashSet};

use crate::{
    apps::{ProcName, SPRINGBOARD, draws_in_springboard},
    display::Rotation,
};

use serde::{Deserialize, Serialize};

use super::{
    archive::Ka,
    attrs::{self, traits},
    dump::{Dump, ObjKey, RawElement},
    window,
};

/// A rectangle in screen points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    fn area(&self) -> f64 {
        self.w * self.h
    }

    fn clip(&self, o: &Rect) -> Option<Rect> {
        let (x0, y0) = (self.x.max(o.x), self.y.max(o.y));
        let (x1, y1) = ((self.x + self.w).min(o.x + o.w), (self.y + self.h).min(o.y + o.h));
        (x1 > x0 && y1 > y0).then_some(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }

    /// The rectangle on a `screen`-sized screen for one in the space `turn` makes of it (see
    /// [`Screen::build`]).
    fn unturn(&self, turn: Rotation, (sw, sh): (f64, f64)) -> Rect {
        if turn == Rotation::R0 || sw <= 0.0 || sh <= 0.0 {
            return *self;
        }
        let (tw, th) = if turn.is_quarter() { (sh, sw) } else { (sw, sh) };
        let corner = |x: f64, y: f64| {
            let (u, v) = turn.unturn_share((x / tw, y / th));
            (u * sw, v * sh)
        };
        let (a, b) = (corner(self.x, self.y), corner(self.x + self.w, self.y + self.h));
        let r = |v: f64| (v * 10.0).round() / 10.0;
        Rect { x: r(a.0.min(b.0)), y: r(a.1.min(b.1)), w: r((a.0 - b.0).abs()), h: r((a.1 - b.1).abs()) }
    }

    fn intersects(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }

    /// Parses `{{x, y}, {w, h}}`, rounded to a tenth of a point.
    pub fn parse(s: &str) -> Option<Rect> {
        let nums: Vec<f64> = s
            .split(|c: char| c == '{' || c == '}' || c == ',' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .map(str::parse::<f64>)
            .collect::<Result<_, _>>()
            .ok()?;
        let r = |v: f64| (v * 10.0).round() / 10.0;
        match nums[..] {
            [x, y, w, h] if nums.iter().all(|n| n.is_finite()) => Some(Rect { x: r(x), y: r(y), w: r(w), h: r(h) }),
            _ => None,
        }
    }
}

/// How far a tap keeps from elements drawn over the one it means (see [`Screen::tap_point`]).
const COVER_MARGIN: f64 = 24.0;

/// Half the width of a UISwitch (51 points).
const SWITCH_HALF_WIDTH: f64 = 26.0;

/// One element as `ui` prints it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Element {
    /// `@eN`, numbered in reading order. Valid until the next reading.
    #[serde(rename = "ref")]
    pub reference: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<Rect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<String>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub selected: bool,
    /// The text field that has the keyboard.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub focused: bool,
    /// Process the element belongs to.
    pub pid: i64,
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

/// A process with elements on screen.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppInfo {
    pub pid: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    /// SpringBoard: the home screen, Control Center, the lock screen and notifications, the app
    /// switcher, system alerts.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub system: bool,
    /// Not an app the screen shows but a process drawing inside one: the keyboard, web content,
    /// a share sheet, the widgets on the home and lock screens (listed as applications, PITFALLS
    /// #60). Some of these report frames relative to their own view, not the screen
    /// (PITFALLS #21); a widget's elements are listed without a frame (PITFALLS #62).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hosted: bool,
    /// Where an app made for a smaller screen is drawn, scaled, on a foldable's inner panel
    /// (compatibility mode). Its elements' frames are already placed in it (PITFALLS #70).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Rect>,
}

/// One reading of the screen.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Screen {
    /// Screen size in points as the user sees it, from the display. Application frames don't
    /// always give it: SpringBoard's on a foldable's inner panel is the cover panel's size, and an
    /// app in compatibility mode has its own (PITFALLS #70).
    pub width: f64,
    pub height: f64,
    pub apps: Vec<AppInfo>,
    /// Which system screen SpringBoard shows, when it is the only app: `home_screen`,
    /// `control_center`, `cover_sheet` (lock screen, notifications), `app_switcher`,
    /// `permission_alert`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    pub elements: Vec<Element>,
    /// Why the reading has no elements, when the device listed only an app with nothing in it
    /// (PITFALLS #71).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// An element on screen offers the home screen's edit mode (`_accessibilityToggleJigglyMode`),
    /// as the home screen's icons do (PITFALLS #62).
    #[serde(skip)]
    pub(crate) home_icons: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BuildOptions {
    /// Keep containers with nothing to read, and elements outside the screen.
    pub all: bool,
}

/// How a dump's frames map onto the screen as the user sees it.
#[derive(Debug, Clone, Default)]
pub struct Geometry {
    /// The screen's size in points ([`Display::size`]).
    ///
    /// [`Display::size`]: crate::Display::size
    pub screen: (f64, f64),
    /// The turn of the space apps' frames come in: the touchscreen's (PITFALLS #61).
    pub turn: Rotation,
    /// The turn of SpringBoard's frames ([`Display::system_frame_turn`]), for the processes in
    /// `system`.
    ///
    /// [`Display::system_frame_turn`]: crate::Display::system_frame_turn
    pub system_turn: Rotation,
    /// SpringBoard and the processes that draw in it, by pid.
    pub system: HashSet<i64>,
    /// Apps drawn in a window of their own, by pid: where the window is on the screen
    /// ([`window::find`]). Their frames are in their own points (PITFALLS #70).
    pub windows: HashMap<i64, Rect>,
}

/// How one application's frames, and those of the elements under it, reach the screen.
#[derive(Debug, Clone, Copy)]
enum Placement {
    /// Turned by this, over the whole screen.
    Turned(Rotation),
    /// Scaled from an app of this size into a window.
    Window { app: (f64, f64), window: Rect },
}

impl Placement {
    fn apply(&self, f: &Rect, screen: (f64, f64)) -> Rect {
        match self {
            Self::Turned(t) => f.unturn(*t, screen),
            Self::Window { app, window } => window::place(f, *app, window),
        }
    }

    /// The part of the screen the application's elements show in.
    fn bounds(&self, (w, h): (f64, f64)) -> Rect {
        match self {
            Self::Turned(_) => Rect { x: 0.0, y: 0.0, w, h },
            Self::Window { window, .. } => *window,
        }
    }
}

pub(crate) fn value_text(v: Option<&Ka>) -> Option<String> {
    Some(match v? {
        Ka::Str(s) => s.clone(),
        other @ Ka::Obj { .. } => other.text()?,
        Ka::Int(i) => i.to_string(),
        Ka::UInt(u) => u.to_string(),
        Ka::Real(r) => r.to_string(),
        _ => return None,
    })
}

fn text(v: Option<&Ka>) -> Option<String> {
    let t = clean(&value_text(v)?);
    (!t.is_empty()).then_some(t)
}

/// Drops characters that only steer layout: object replacement (an inline icon, as in
/// "\u{fffc} AirPlay"), zero-width and word joiners, and direction marks ("\u{200e}apple.com");
/// no-break spaces become spaces so a label typed with a plain space still matches.
fn clean(s: &str) -> String {
    let kept: String = s
        .chars()
        .filter(|c| !matches!(c, '\u{fffc}' | '\u{200b}' | '\u{2060}' | '\u{feff}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .map(|c| if c == '\u{a0}' { ' ' } else { c })
        .collect();
    kept.trim().to_string()
}

fn frame_of(raw: &RawElement) -> Option<Rect> {
    raw.attrs
        .get(&attrs::FRAME)
        .and_then(|v| v.get("NS.rectval"))
        .and_then(Ka::as_str)
        .and_then(Rect::parse)
}

/// Whether one of the element's custom actions calls `selector`.
fn has_action(raw: &RawElement, selector: &str) -> bool {
    let Some(actions) = raw.attrs.get(&attrs::CUSTOM_ACTIONS).and_then(Ka::as_array) else { return false };
    actions.iter().any(|a| match a {
        Ka::Dict(pairs) => pairs.iter().any(|(k, v)| {
            k.as_str() == Some("AXPCustomActionIdentifier")
                && v.as_str().is_some_and(|id| id.lines().any(|l| l.strip_prefix("Selector:") == Some(selector)))
        }),
        _ => false,
    })
}

fn children_of(raw: &RawElement, all: bool) -> Vec<ObjKey> {
    let pick = |n: i64| -> Vec<ObjKey> {
        raw.attrs
            .get(&n)
            .and_then(Ka::as_array)
            .map(|a| a.iter().filter_map(ObjKey::from_ka).collect())
            .unwrap_or_default()
    };
    let (first, second) = if all {
        (attrs::CHILDREN, attrs::VISIBLE_CHILDREN)
    } else {
        (attrs::VISIBLE_CHILDREN, attrs::CHILDREN)
    };
    let list = pick(first);
    if list.is_empty() { pick(second) } else { list }
}

impl Screen {
    /// The application frames in a dump, by pid: an app in compatibility mode has its own size
    /// there (PITFALLS #70).
    pub(crate) fn app_frames(dump: &Dump) -> Vec<(i64, Rect)> {
        dump.applications.iter().filter_map(|k| Some((k.pid, dump.elements.get(k).and_then(frame_of)?))).collect()
    }

    /// The elements of a dump, in screen points.
    ///
    /// Frames come in several spaces: apps' turned like the touchscreen (on an iPad in landscape
    /// they are portrait, PITFALLS #61), SpringBoard's on a foldable also not turned with the
    /// interface, and those of an app in compatibility mode in its own smaller screen
    /// (PITFALLS #70). Each element is placed as the application it was reached from.
    pub fn build(dump: &Dump, opts: BuildOptions, geometry: &Geometry) -> Screen {
        let mut screen = Screen { width: geometry.screen.0, height: geometry.screen.1, ..Screen::default() };
        let size = geometry.screen;
        let placement = |pid: i64| -> Placement {
            if let Some(window) = geometry.windows.get(&pid) {
                let app = dump
                    .applications
                    .iter()
                    .find(|k| k.pid == pid)
                    .and_then(|k| dump.elements.get(k))
                    .and_then(frame_of)
                    .map(|f| (f.w, f.h));
                if let Some(app) = app {
                    return Placement::Window { app, window: *window };
                }
            }
            Placement::Turned(if geometry.system.contains(&pid) { geometry.system_turn } else { geometry.turn })
        };

        let mut seen: HashSet<ObjKey> = HashSet::new();
        // SpringBoard listed the battery in Control Center twice, same label, value and frame.
        let mut printed: HashSet<String> = HashSet::new();
        let mut stack: Vec<(ObjKey, Placement)> =
            dump.applications.iter().rev().map(|k| (*k, placement(k.pid))).collect();
        while let Some((key, place)) = stack.pop() {
            if !seen.insert(key) {
                continue;
            }
            let Some(raw) = dump.elements.get(&key) else { continue };
            for child in children_of(raw, opts.all).into_iter().rev() {
                stack.push((child, place));
            }
            let label = text(raw.attrs.get(&attrs::LABEL)).or_else(|| text(raw.attrs.get(&attrs::LABEL_PLAIN)));
            if raw.is_application {
                let window = match place {
                    Placement::Window { window, .. } => Some(window),
                    Placement::Turned(_) => None,
                };
                screen.apps.push(AppInfo { pid: key.pid, name: label, bundle_id: None, system: false, hosted: false, window });
                continue;
            }
            let frame = frame_of(raw).map(|f| place.apply(&f, size));
            let bounds = place.bounds(size);
            let on_screen = match frame {
                Some(f) if bounds.w > 0.0 => f.w > 0.0 && f.h > 0.0 && f.intersects(&bounds),
                Some(f) => f.w > 0.0 && f.h > 0.0,
                None => false,
            };
            let bits = raw.attrs.get(&attrs::TRAITS).and_then(Ka::as_u64).unwrap_or(0);
            let value = text(raw.attrs.get(&attrs::VALUE));
            let identifier = text(raw.attrs.get(&attrs::IDENTIFIER));
            let has_text = label.is_some() || value.is_some();
            let role = attrs::role_name(raw.attrs.get(&attrs::ROLE).and_then(Ka::as_i64), bits, false, has_text);
            // A container with only an identifier stays: Calculator's display keeps its number
            // there (PITFALLS #10).
            if !opts.all && (!on_screen || (role == "other" && !has_text && identifier.is_none())) {
                continue;
            }
            if !printed.insert(format!("{}|{role}|{label:?}|{value:?}|{frame:?}", key.pid)) {
                continue;
            }
            screen.home_icons |= has_action(raw, "_accessibilityToggleJigglyMode");
            screen.elements.push(Element {
                reference: format!("@e{}", screen.elements.len() + 1),
                role: role.to_string(),
                label,
                value,
                identifier,
                frame,
                traits: attrs::trait_names(bits).into_iter().map(String::from).collect(),
                enabled: bits & traits::NOT_ENABLED == 0,
                selected: bits & traits::SELECTED != 0,
                focused: bits & traits::IS_EDITING != 0,
                pid: key.pid,
            });
        }
        screen
    }

    /// Names the processes on screen, adds those that only draw inside an app (`hosted`), and
    /// tells which system screen SpringBoard shows.
    pub fn name_processes(&mut self, describe: impl Fn(i64) -> ProcName) {
        for app in &mut self.apps {
            let p = describe(app.pid);
            // The application element's label is what the home screen shows; keep it.
            app.name = app.name.take().or(p.name);
            app.bundle_id = p.bundle_id;
        }
        let mut hosted: Vec<i64> = Vec::new();
        for e in &self.elements {
            if !self.apps.iter().any(|a| a.pid == e.pid) && !hosted.contains(&e.pid) {
                hosted.push(e.pid);
            }
        }
        for pid in hosted {
            let p = describe(pid);
            self.apps.push(AppInfo { pid, name: p.name, bundle_id: p.bundle_id, system: false, hosted: true, window: None });
        }
        for app in &mut self.apps {
            app.system = app.bundle_id.as_deref() == Some(SPRINGBOARD);
            // A widget renderer is listed as an application, but it only draws widgets into
            // SpringBoard's screens; counting it as an app hid the home screen (PITFALLS #60).
            app.hosted |= draws_in_springboard(app.bundle_id.as_deref());
        }
        // A widget's frames are relative to the widget, and the dump has nothing that says where
        // the widget is (PITFALLS #62): a tap at them would land near the top-left corner. They stay
        // listed for their text.
        let widgets: Vec<i64> =
            self.apps.iter().filter(|a| draws_in_springboard(a.bundle_id.as_deref())).map(|a| a.pid).collect();
        for e in self.elements.iter_mut().filter(|e| widgets.contains(&e.pid)) {
            e.frame = None;
        }
        self.surface = self.system_surface().map(str::to_string);
        self.note = self.empty_note();
    }

    /// What to tell the reader when the device listed apps but no element: right after a launch the
    /// tree can be empty for a moment (PITFALLS #10), and on a foldable's cover panel the device
    /// listed only Siri AI, empty, over the app the screen showed, reading after reading
    /// (PITFALLS #71). Set by [`Screen::name_processes`].
    fn empty_note(&self) -> Option<String> {
        let apps: Vec<&AppInfo> = self.apps.iter().filter(|a| !a.hosted).collect();
        if !self.elements.is_empty() || apps.is_empty() || apps.iter().any(|a| a.system) {
            return None;
        }
        let names: Vec<String> =
            apps.iter().map(|a| a.name.clone().unwrap_or_else(|| format!("pid {}", a.pid))).collect();
        Some(format!(
            "the device listed only {} and no element in it. Right after a launch, read again. If a screenshot \
             shows another app (seen: Siri AI over the app on a foldable's cover panel), its elements can't be \
             read: tap points from the screenshot",
            names.join(", ")
        ))
    }

    /// True when SpringBoard is the only app (not counting hosted processes).
    pub fn springboard_only(&self) -> bool {
        let mut apps = self.apps.iter().filter(|a| !a.hosted).peekable();
        apps.peek().is_some() && apps.all(|a| a.system)
    }

    /// The system screen, told by accessibility identifiers (which are not localized, unlike
    /// labels). They come only with the second pass, so `ui` waits for it on SpringBoard.
    fn system_surface(&self) -> Option<&'static str> {
        if !self.springboard_only() {
            return None;
        }
        let ids: Vec<&str> = self.elements.iter().filter_map(|e| e.identifier.as_deref()).collect();
        let any = |f: &dyn Fn(&str) -> bool| ids.iter().any(|i| f(i));
        if any(&|i| i.starts_with("kTCCService")) {
            Some("permission_alert")
        } else if any(&|i| i.starts_with("card:")) {
            Some("app_switcher")
        } else if any(&|i| i == "Page control" || i.starts_with("cc-") || i.starts_with("mru-router") || i == "airplane-button") {
            Some("control_center")
        } else if any(&|i| i == "flashlight-orb-button" || i == "camera-orb-button") {
            Some("cover_sheet")
        } else if self.home_icons || any(&|i| i == "spotlight-pill") {
            Some("home_screen")
        } else {
            None
        }
    }

    /// Where to tap an element: the middle of its visible part, moved off any smaller element
    /// drawn over it (a menu's full-screen "dismiss" button lies under the menu, PITFALLS #20).
    /// A switch row is tapped at its trailing end, where the switch is; tapping the row's label
    /// does not toggle it (PITFALLS #20).
    pub fn tap_point(&self, e: &Element) -> Option<(f64, f64)> {
        let frame = e.frame?;
        let bounds = Rect { x: 0.0, y: 0.0, w: self.width, h: self.height };
        let f = if self.width > 0.0 { frame.clip(&bounds).unwrap_or(frame) } else { frame };
        if e.role == "switch" && f.w > 3.0 * f.h {
            return Some((f.x + f.w - (f.w / 2.0).min(SWITCH_HALF_WIDTH), f.y + f.h / 2.0));
        }
        // A menu's background reaches past its items and is no element, so keep clear of them.
        let covered = |p: (f64, f64), margin: f64| {
            self.elements.iter().any(|o| {
                o.reference != e.reference
                    && o.frame.is_some_and(|g| {
                        g.area() < frame.area()
                            && Rect { x: g.x - margin, y: g.y - margin, w: g.w + 2.0 * margin, h: g.h + 2.0 * margin }.contains(p)
                    })
            })
        };
        let center = f.center();
        if !covered(center, 0.0) {
            return Some(center);
        }
        // The free point of a grid over the frame that is nearest its middle, as far from the
        // elements on top as the frame allows.
        const STEPS: usize = 16;
        for margin in [COVER_MARGIN, COVER_MARGIN / 3.0, 0.0] {
            let mut best: Option<((f64, f64), f64)> = None;
            for i in 0..STEPS {
                for j in 0..STEPS {
                    let p = (f.x + f.w * (i as f64 + 0.5) / STEPS as f64, f.y + f.h * (j as f64 + 0.5) / STEPS as f64);
                    if covered(p, margin) {
                        continue;
                    }
                    let d = (p.0 - center.0).powi(2) + (p.1 - center.1).powi(2);
                    if best.is_none_or(|(_, bd)| d < bd) {
                        best = Some((p, d));
                    }
                }
            }
            if let Some((p, _)) = best {
                return Some(p);
            }
        }
        Some(center)
    }

    /// The element a `tap` target names: `@eN`, or a label (exact first, then case-insensitive,
    /// then a unique substring). Errors say what to do when there are none or several.
    pub fn find(&self, target: &str) -> Result<&Element, String> {
        if target.starts_with('@') {
            return self
                .elements
                .iter()
                .find(|e| e.reference == target)
                .ok_or_else(|| format!("no element {target} on the last reading; run `ui` again and use a ref it prints"));
        }
        let label = |e: &Element| e.label.clone().unwrap_or_default().to_lowercase();
        let wanted = target.to_lowercase();
        // Exact, then case-insensitive, then substring.
        let matches = |tier: u8, e: &Element| match tier {
            0 => e.label.as_deref() == Some(target),
            1 => label(e) == wanted,
            _ => label(e).contains(&wanted),
        };
        for tier in 0..3 {
            let hits: Vec<&Element> =
                self.elements.iter().filter(|e| e.frame.is_some() && matches(tier, e)).collect();
            match hits[..] {
                [] => continue,
                [one] => return Ok(one),
                _ => {
                    let refs: Vec<String> = hits
                        .iter()
                        .map(|e| format!("{} ({}{})", e.reference, e.role, e.label.as_ref().map(|l| format!(" \"{l}\"")).unwrap_or_default()))
                        .collect();
                    return Err(format!("{} elements match \"{target}\": {}; tap one by its ref", hits.len(), refs.join(", ")));
                }
            }
        }
        Err(format!("no element labeled \"{target}\" on screen; run `ui` to see the labels"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(n: usize, role: &str, label: &str) -> Element {
        Element {
            reference: format!("@e{n}"),
            role: role.into(),
            label: Some(label.into()),
            value: None,
            identifier: None,
            frame: Some(Rect { x: 0.0, y: 10.0 * n as f64, w: 100.0, h: 10.0 }),
            traits: vec![],
            enabled: true,
            selected: false,
            focused: false,
            pid: 1,
        }
    }

    fn at(n: usize, role: &str, x: f64, y: f64, w: f64, h: f64) -> Element {
        Element { frame: Some(Rect { x, y, w, h }), ..el(n, role, "x") }
    }

    fn screen(elements: Vec<Element>) -> Screen {
        Screen { width: 428.0, height: 926.0, elements, ..Default::default() }
    }

    #[test]
    fn clean_drops_layout_characters() {
        assert_eq!(clean("\u{fffc} AirPlay"), "AirPlay");
        assert_eq!(clean("\u{200e}example.com"), "example.com");
        assert_eq!(clean("Apple\u{a0}Account"), "Apple Account");
        assert_eq!(clean("le\u{2060}arn\u{200b}"), "learn");
    }

    #[test]
    fn switch_rows_are_tapped_at_the_switch() {
        // A Settings row: the frame spans the label and the switch.
        let s = screen(vec![at(1, "switch", 40.0, 238.0, 348.0, 28.0), at(2, "switch", 60.7, 175.7, 58.0, 58.0)]);
        assert_eq!(s.tap_point(&s.elements[0]), Some((362.0, 252.0)));
        // A square Control Center tile: its middle.
        assert_eq!(s.tap_point(&s.elements[1]), Some(s.elements[1].frame.unwrap().center()));
    }

    #[test]
    fn taps_avoid_elements_drawn_on_top() {
        // A context menu over a full-screen "dismiss" button.
        let mut elements: Vec<Element> = (0..12).map(|i| at(i + 1, "button", 85.0, 382.0 + 42.0 * i as f64, 250.0, 42.0)).collect();
        elements.push(at(13, "button", 0.0, 0.0, 428.0, 926.0));
        let s = screen(elements);
        let p = s.tap_point(&s.elements[12]).unwrap();
        assert!(s.elements[..12].iter().all(|e| {
            let f = e.frame.unwrap();
            !Rect { x: f.x - COVER_MARGIN, y: f.y - COVER_MARGIN, w: f.w + 2.0 * COVER_MARGIN, h: f.h + 2.0 * COVER_MARGIN }.contains(p)
        }));
        // The menu items themselves are tapped in their middle.
        assert_eq!(s.tap_point(&s.elements[0]), Some(s.elements[0].frame.unwrap().center()));
    }

    #[test]
    fn names_processes_and_system_screens() {
        let mut s = screen(vec![
            Element { identifier: Some("Page control".into()), pid: 37, ..el(1, "adjustable", "x") },
            Element { pid: 163, ..el(2, "key", "q") },
        ]);
        s.apps = vec![AppInfo { pid: 37, name: None, bundle_id: None, system: false, hosted: false, window: None }];
        s.name_processes(|pid| match pid {
            37 => ProcName { name: Some("SpringBoard".into()), bundle_id: Some(SPRINGBOARD.into()) },
            _ => ProcName { name: Some("InputUI".into()), bundle_id: Some("com.apple.InputUI".into()) },
        });
        assert!(s.apps[0].system && !s.apps[0].hosted);
        assert_eq!(s.apps[1].pid, 163);
        assert!(s.apps[1].hosted);
        assert_eq!(s.surface.as_deref(), Some("control_center"));

        // An app on screen: no system surface.
        let mut s = screen(vec![Element { identifier: Some("Page control".into()), pid: 805, ..el(1, "adjustable", "x") }]);
        s.apps = vec![AppInfo { pid: 805, name: Some("Settings".into()), bundle_id: None, system: false, hosted: false, window: None }];
        s.name_processes(|_| ProcName { name: Some("Preferences".into()), bundle_id: Some("com.apple.Preferences".into()) });
        assert_eq!(s.apps[0].name.as_deref(), Some("Settings"));
        assert_eq!(s.surface, None);
    }

    /// A raw element as a dump carries it: frame, label, identifier and children.
    fn raw(app: bool, frame: &str, label: Option<&str>, id: Option<&str>, children: &[ObjKey]) -> RawElement {
        let mut attrs = std::collections::BTreeMap::new();
        let obj = |class: &str, fields: Vec<(&str, Ka)>| Ka::Obj {
            class: class.into(),
            fields: fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        };
        attrs.insert(attrs::FRAME, obj("NSValue", vec![("NS.rectval", Ka::Str(frame.into()))]));
        if let Some(l) = label {
            attrs.insert(attrs::LABEL, Ka::Str(l.into()));
        }
        if let Some(i) = id {
            attrs.insert(attrs::IDENTIFIER, Ka::Str(i.into()));
        }
        let kids = children
            .iter()
            .map(|k| obj("AXPTranslationObject", vec![("pid", Ka::Int(k.pid)), ("objectID", Ka::UInt(k.object_id))]))
            .collect();
        attrs.insert(attrs::VISIBLE_CHILDREN, Ka::Array(kids));
        attrs.insert(attrs::TRAITS, Ka::UInt(if app { 0 } else { traits::BUTTON }));
        RawElement { is_application: app, attrs, singles: Default::default() }
    }

    #[test]
    fn home_screen_with_widgets_is_the_home_screen() {
        // The shape `ui` reported on an iPad home screen with widgets (iPadOS 27.2.1): SpringBoard
        // and a widget renderer both come as applications, the widget being the renderer's
        // child. Labels and frames are made up.
        let (sb, wr) = (40, 50);
        let key = |pid: i64, object_id: u64| ObjKey { pid, object_id };
        let full = "{{0, 0}, {1032, 1376}}";
        let mut elements = std::collections::HashMap::new();
        elements.insert(key(sb, 1), raw(true, full, Some(" "), None, &[key(sb, 2), key(sb, 3), key(sb, 4)]));
        elements.insert(key(sb, 2), raw(false, "{{60, 420}, {76, 76}}", Some("App One"), Some("App One"), &[]));
        elements.insert(key(sb, 3), raw(false, "{{180, 420}, {76, 76}}", Some("App Two"), Some("App Two"), &[]));
        elements.insert(key(sb, 4), raw(false, "{{456, 1250}, {120, 36}}", Some("Search"), Some("spotlight-pill"), &[]));
        elements.insert(key(wr, 1), raw(true, full, Some("WidgetRenderer"), None, &[key(wr, 2)]));
        elements.insert(key(wr, 2), raw(false, "{{60, 80}, {340, 160}}", Some("Calendar widget"), None, &[]));
        let dump = Dump {
            kind: super::super::dump::DumpKind::Additional,
            applications: vec![key(sb, 1), key(wr, 1)],
            order: elements.keys().copied().collect(),
            elements,
        };
        let mut s = Screen::build(&dump, BuildOptions::default(), &geo((1032.0, 1376.0), Rotation::R0));
        s.name_processes(|pid| match pid {
            40 => ProcName { name: Some("SpringBoard".into()), bundle_id: Some(SPRINGBOARD.into()) },
            _ => ProcName {
                name: Some("WidgetRenderer_Default".into()),
                bundle_id: Some("com.apple.chrono.WidgetRenderer-Default".into()),
            },
        });
        let renderer = s.apps.iter().find(|a| a.pid == wr).unwrap();
        assert!(renderer.hosted && !renderer.system);
        assert!(s.springboard_only());
        assert_eq!(s.surface.as_deref(), Some("home_screen"));
        // The widget is still listed.
        assert!(s.elements.iter().any(|e| e.pid == wr && e.label.as_deref() == Some("Calendar widget")));
    }

    /// [`raw`] with the custom actions a home screen icon has (names made up, selectors as seen).
    fn icon(frame: &str, label: &str) -> RawElement {
        let mut r = raw(false, frame, Some(label), None, &[]);
        let action = |name: &str, selector: &str| {
            Ka::Dict(vec![
                (Ka::Str("AXPCustomActionName".into()), Ka::Str(name.into())),
                (Ka::Str("AXPCustomActionIdentifier".into()), Ka::Str(format!("Name:{name}\nTarget:0x1\nSelector:{selector}"))),
            ])
        };
        r.attrs.insert(
            attrs::CUSTOM_ACTIONS,
            Ka::Array(vec![action("Edit", "_accessibilityToggleJigglyMode"), action("Today", "_accessibilityShowTodayView")]),
        );
        r
    }

    #[test]
    fn ipad_landscape_home_screen_frames_are_turned_and_widgets_lose_theirs() {
        // The shape of an iPad Pro 13-inch home screen in landscape (iPadOS 27.2.1, panel rot0,
        // touchscreen rot270): the application frame is landscape, every element's frame is in the
        // touchscreen's portrait space, a widget's frames are its own, and there is no
        // spotlight-pill (page dots instead). Frames as seen; labels made up.
        let (sb, wr) = (38, 1377);
        let key = |pid: i64, object_id: u64| ObjKey { pid, object_id };
        let mut elements = std::collections::HashMap::new();
        let sb_children = [key(wr, 1), key(sb, 2), key(sb, 3), key(sb, 4), key(sb, 5)];
        elements.insert(key(sb, 1), raw(true, "{{0, 0}, {1376, 1032}}", Some(" "), None, &sb_children));
        elements.insert(key(wr, 1), raw(false, "{{0, 0}, {170, 170}}", Some("Clock"), None, &[]));
        elements.insert(key(sb, 2), icon("{{814.5, 747.5}, {95.5, 79}}", "App One"));
        // Past the landscape screen's height until it is turned.
        elements.insert(key(sb, 3), icon("{{814.5, 1146}, {95.5, 72}}", "App Two"));
        elements.insert(key(sb, 4), icon("{{27.5, 206.5}, {68, 68}}", "Dock App"));
        let mut dots = raw(false, "{{138, 655}, {41, 66}}", None, None, &[]);
        dots.attrs.insert(attrs::TRAITS, Ka::UInt(traits::ADJUSTABLE));
        dots.attrs.insert(attrs::VALUE, Ka::Str("page 1 of 2".into()));
        elements.insert(key(sb, 5), dots);
        let dump = Dump {
            kind: super::super::dump::DumpKind::Initial,
            applications: vec![key(sb, 1)],
            order: elements.keys().copied().collect(),
            elements,
        };
        let mut s = Screen::build(&dump, BuildOptions::default(), &geo((1376.0, 1032.0), Rotation::R270));
        s.name_processes(|pid| match pid {
            38 => ProcName { name: Some("SpringBoard".into()), bundle_id: Some(SPRINGBOARD.into()) },
            _ => ProcName {
                name: Some("WidgetRenderer_Default".into()),
                bundle_id: Some("com.apple.chrono.WidgetRenderer-Default".into()),
            },
        });
        assert_eq!((s.width, s.height), (1376.0, 1032.0));
        assert_eq!(s.surface.as_deref(), Some("home_screen"));
        let frame = |label: &str| s.elements.iter().find(|e| e.label.as_deref() == Some(label)).unwrap().frame;
        // Where the screenshot showed them: the first icon near the top, the dock at the bottom.
        assert_eq!(frame("App One"), Some(Rect { x: 747.5, y: 122.0, w: 79.0, h: 95.5 }));
        assert_eq!(frame("App Two"), Some(Rect { x: 1146.0, y: 122.0, w: 72.0, h: 95.5 }));
        assert_eq!(frame("Dock App"), Some(Rect { x: 206.5, y: 936.5, w: 68.0, h: 68.0 }));
        let dots = s.elements.iter().find(|e| e.role == "adjustable").unwrap();
        assert_eq!(dots.frame, Some(Rect { x: 655.0, y: 853.0, w: 66.0, h: 41.0 }));
        // The widget is read but not tapped.
        assert_eq!(frame("Clock"), None);
        assert!(s.find("Clock").is_err());

        // Without the icons' edit action (and no spotlight-pill) the screen isn't named.
        let mut plain = dump.clone();
        for k in [key(sb, 2), key(sb, 3), key(sb, 4)] {
            plain.elements.get_mut(&k).unwrap().attrs.remove(&attrs::CUSTOM_ACTIONS);
        }
        let mut s = Screen::build(&plain, BuildOptions::default(), &geo((1376.0, 1032.0), Rotation::R270));
        s.name_processes(|_| ProcName { name: None, bundle_id: Some(SPRINGBOARD.into()) });
        assert_eq!(s.surface, None);
    }

    fn geo(screen: (f64, f64), turn: Rotation) -> Geometry {
        Geometry { screen, turn, system_turn: turn, ..Default::default() }
    }

    #[test]
    fn foldable_springboard_frames_are_turned_and_the_screen_comes_from_the_display() {
        // A foldable's inner panel (669x951 points) held upside down, home screen (iOS 27.3):
        // SpringBoard's application frame was the cover panel's 466x678, and its elements' frames
        // were in the panel's own orientation, the dock's first icon at {{416.3, 35}, {64, 64}}
        // while the screenshot showed it at the bottom left (PITFALLS #70). Labels made up.
        let sb = 853;
        let key = |object_id: u64| ObjKey { pid: sb, object_id };
        let mut elements = std::collections::HashMap::new();
        elements.insert(key(1), raw(true, "{{0, 0}, {466, 678}}", Some(" "), None, &[key(2), key(3), key(4)]));
        elements.insert(key(2), raw(false, "{{416.33333333333337, 35}, {64, 64}}", Some("Phone"), None, &[]));
        elements.insert(key(3), raw(false, "{{188.33333333333337, 35}, {64, 64}}", Some("Music"), None, &[]));
        let mut dots = raw(false, "{{298, 136}, {73, 35.3}}", None, None, &[]);
        dots.attrs.insert(attrs::TRAITS, Ka::UInt(traits::ADJUSTABLE));
        dots.attrs.insert(attrs::VALUE, Ka::Str("page 1 of 3".into()));
        elements.insert(key(4), dots);
        let dump = Dump {
            kind: super::super::dump::DumpKind::Initial,
            applications: vec![key(1)],
            order: elements.keys().copied().collect(),
            elements,
        };
        let geometry = Geometry {
            screen: (669.0, 951.0),
            turn: Rotation::R0,
            system_turn: Rotation::R180,
            system: HashSet::from([sb]),
            windows: HashMap::new(),
        };
        let s = Screen::build(&dump, BuildOptions::default(), &geometry);
        assert_eq!((s.width, s.height), (669.0, 951.0));
        let frame = |label: &str| s.elements.iter().find(|e| e.label.as_deref() == Some(label)).unwrap().frame.unwrap();
        // Where the screenshot showed them: Phone at the dock's left end, about (220, 883).
        assert_eq!(frame("Phone"), Rect { x: 188.7, y: 852.0, w: 64.0, h: 64.0 });
        assert_eq!(frame("Music"), Rect { x: 416.7, y: 852.0, w: 64.0, h: 64.0 });
        let dots = s.elements.iter().find(|e| e.role == "adjustable").unwrap().frame.unwrap();
        assert_eq!(dots, Rect { x: 298.0, y: 779.7, w: 73.0, h: 35.3 });
    }

    #[test]
    fn a_compat_app_is_placed_in_its_window() {
        // An app made for a 375x667 screen on a foldable's inner panel (iOS 27.3), drawn from
        // x 46.3 at 1.426 times its size (PITFALLS #70). Frames as seen; labels made up.
        let app = 18695;
        let key = |object_id: u64| ObjKey { pid: app, object_id };
        let mut elements = std::collections::HashMap::new();
        elements.insert(key(1), raw(true, "{{0, 0}, {375, 667}}", Some("app"), None, &[key(2), key(3), key(4)]));
        elements.insert(key(2), raw(false, "{{20, 522.33335113525391}, {45, 24}}", Some("Likes"), None, &[]));
        elements.insert(key(3), raw(false, "{{20, 310.00000762939447}, {335, 40}}", Some("More"), None, &[]));
        // Scrolled off the app's screen: not on screen either.
        elements.insert(key(4), raw(false, "{{20, 700}, {335, 40}}", Some("Below"), None, &[]));
        let dump = Dump {
            kind: super::super::dump::DumpKind::Initial,
            applications: vec![key(1)],
            order: elements.keys().copied().collect(),
            elements,
        };
        let window = Rect { x: 46.33, y: 0.0, w: 534.67, h: 951.0 };
        let geometry = Geometry {
            screen: (669.0, 951.0),
            turn: Rotation::R0,
            system_turn: Rotation::R180,
            system: HashSet::new(),
            windows: HashMap::from([(app, window)]),
        };
        let s = Screen::build(&dump, BuildOptions::default(), &geometry);
        assert_eq!((s.width, s.height), (669.0, 951.0));
        assert_eq!(s.apps[0].window, Some(window));
        let frame = |label: &str| s.elements.iter().find(|e| e.label.as_deref() == Some(label)).map(|e| e.frame.unwrap());
        // The screenshot showed the button from about (75, 459) to (550, 498).
        assert_eq!(frame("More"), Some(Rect { x: 74.8, y: 442.0, w: 477.6, h: 57.0 }));
        assert_eq!(frame("Likes"), Some(Rect { x: 74.8, y: 744.7, w: 64.2, h: 34.2 }));
        assert_eq!(frame("Below"), None);
        let tap = s.tap_point(s.find("Likes").unwrap()).unwrap();
        assert!((tap.0 - 106.9).abs() < 0.1 && (tap.1 - 761.8).abs() < 0.1, "{tap:?}");
    }

    #[test]
    fn an_empty_app_alone_gets_a_note() {
        // Seen on a foldable's cover panel (iOS 27.3): only Siri AI, no element (PITFALLS #71).
        let mut s = screen(vec![]);
        s.apps = vec![AppInfo { pid: 11, name: Some("Siri AI".into()), bundle_id: None, system: false, hosted: false, window: None }];
        s.name_processes(|_| ProcName::default());
        assert!(s.note.as_deref().is_some_and(|n| n.contains("only Siri AI")), "{:?}", s.note);
        // A locked SpringBoard with no element is a system screen, not this (PITFALLS #60).
        let mut s = screen(vec![]);
        s.apps = vec![AppInfo { pid: 37, name: None, bundle_id: None, system: false, hosted: false, window: None }];
        s.name_processes(|_| ProcName { name: Some("SpringBoard".into()), bundle_id: Some(SPRINGBOARD.into()) });
        assert_eq!(s.note, None);
        // Elements: no note.
        let mut s = screen(vec![el(1, "button", "OK")]);
        s.apps = vec![AppInfo { pid: 1, name: Some("App".into()), bundle_id: None, system: false, hosted: false, window: None }];
        s.name_processes(|_| ProcName::default());
        assert_eq!(s.note, None);
    }

    #[test]
    fn rect_parses_ns_rect_strings() {
        assert_eq!(Rect::parse("{{16, 120.25}, {396, 52}}"), Some(Rect { x: 16.0, y: 120.3, w: 396.0, h: 52.0 }));
        assert_eq!(Rect::parse("{{1, 2}, {3}}"), None);
    }

    #[test]
    fn find_prefers_exact_labels_and_reports_ambiguity() {
        let s = Screen {
            width: 400.0,
            height: 800.0,
            elements: vec![el(1, "button", "General"), el(2, "button", "General Info"), el(3, "text", "Wi-Fi"), el(4, "text", "Wi-Fi Calling")],
            ..Default::default()
        };
        assert_eq!(s.find("General").unwrap().reference, "@e1");
        assert_eq!(s.find("wi-fi").unwrap().reference, "@e3");
        assert_eq!(s.find("@e4").unwrap().reference, "@e4");
        assert!(s.find("Info").is_ok());
        assert!(s.find("i").unwrap_err().contains("elements match"));
        assert!(s.find("Bluetooth").is_err());
        assert!(s.find("@e9").is_err());
    }
}
