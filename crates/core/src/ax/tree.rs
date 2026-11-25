//! A raw dump turned into the element list `ui` prints.
//!
//! The walk (depth-first from each application in the device's child order, visible children
//! first) follows iphone-cli `src/ax/tree.rs`, MIT, see THIRD_PARTY_NOTICES.md.

use std::collections::HashSet;

use crate::apps::{ProcName, SPRINGBOARD, draws_in_springboard};

use serde::{Deserialize, Serialize};

use super::{
    archive::Ka,
    attrs::{self, traits},
    dump::{Dump, ObjKey, RawElement},
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
    /// (PITFALLS #21).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hosted: bool,
}

/// One reading of the screen.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Screen {
    /// Screen size in points, from the application frames.
    pub width: f64,
    pub height: f64,
    pub apps: Vec<AppInfo>,
    /// Which system screen SpringBoard shows, when it is the only app: `home_screen`,
    /// `control_center`, `cover_sheet` (lock screen, notifications), `app_switcher`,
    /// `permission_alert`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    pub elements: Vec<Element>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BuildOptions {
    /// Keep containers with nothing to read, and elements outside the screen.
    pub all: bool,
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
    pub fn build(dump: &Dump, opts: BuildOptions) -> Screen {
        let mut screen = Screen { width: 0.0, height: 0.0, apps: Vec::new(), surface: None, elements: Vec::new() };
        // The screen is as large as the largest application frame.
        for key in &dump.applications {
            if let Some(f) = dump.elements.get(key).and_then(frame_of) {
                screen.width = screen.width.max(f.x + f.w);
                screen.height = screen.height.max(f.y + f.h);
            }
        }
        let bounds = Rect { x: 0.0, y: 0.0, w: screen.width, h: screen.height };

        let mut seen: HashSet<ObjKey> = HashSet::new();
        // SpringBoard listed the battery in Control Center twice, same label, value and frame.
        let mut printed: HashSet<String> = HashSet::new();
        let mut stack: Vec<ObjKey> = dump.applications.iter().rev().copied().collect();
        while let Some(key) = stack.pop() {
            if !seen.insert(key) {
                continue;
            }
            let Some(raw) = dump.elements.get(&key) else { continue };
            for child in children_of(raw, opts.all).into_iter().rev() {
                stack.push(child);
            }
            let label = text(raw.attrs.get(&attrs::LABEL)).or_else(|| text(raw.attrs.get(&attrs::LABEL_PLAIN)));
            if raw.is_application {
                screen.apps.push(AppInfo { pid: key.pid, name: label, bundle_id: None, system: false, hosted: false });
                continue;
            }
            let frame = frame_of(raw);
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
            self.apps.push(AppInfo { pid, name: p.name, bundle_id: p.bundle_id, system: false, hosted: true });
        }
        for app in &mut self.apps {
            app.system = app.bundle_id.as_deref() == Some(SPRINGBOARD);
            // A widget renderer is listed as an application, but it only draws widgets into
            // SpringBoard's screens; counting it as an app hid the home screen (PITFALLS #60).
            app.hosted |= draws_in_springboard(app.bundle_id.as_deref());
        }
        self.surface = self.system_surface().map(str::to_string);
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
        } else if any(&|i| i == "spotlight-pill") {
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
        Screen { width: 428.0, height: 926.0, apps: vec![], surface: None, elements }
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
        s.apps = vec![AppInfo { pid: 37, name: None, bundle_id: None, system: false, hosted: false }];
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
        s.apps = vec![AppInfo { pid: 805, name: Some("Settings".into()), bundle_id: None, system: false, hosted: false }];
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
        let mut s = Screen::build(&dump, BuildOptions::default());
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
            apps: vec![],
            surface: None,
            elements: vec![el(1, "button", "General"), el(2, "button", "General Info"), el(3, "text", "Wi-Fi"), el(4, "text", "Wi-Fi Calling")],
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
