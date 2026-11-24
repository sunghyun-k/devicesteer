//! A raw dump turned into the element list `ui` prints.
//!
//! The walk (depth-first from each application in the device's child order, visible children
//! first) follows iphone-cli `src/ax/tree.rs`, MIT, see THIRD_PARTY_NOTICES.md.

use std::collections::HashSet;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppInfo {
    pub pid: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// One reading of the screen.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Screen {
    /// Screen size in points, from the application frames.
    pub width: f64,
    pub height: f64,
    pub apps: Vec<AppInfo>,
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
    let s = value_text(v)?;
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
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
        let mut screen = Screen { width: 0.0, height: 0.0, apps: Vec::new(), elements: Vec::new() };
        // The screen is as large as the largest application frame.
        for key in &dump.applications {
            if let Some(f) = dump.elements.get(key).and_then(frame_of) {
                screen.width = screen.width.max(f.x + f.w);
                screen.height = screen.height.max(f.y + f.h);
            }
        }
        let bounds = Rect { x: 0.0, y: 0.0, w: screen.width, h: screen.height };

        let mut seen: HashSet<ObjKey> = HashSet::new();
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
                screen.apps.push(AppInfo { pid: key.pid, name: label });
                continue;
            }
            let frame = frame_of(raw);
            let on_screen = match frame {
                Some(f) if bounds.w > 0.0 => f.w > 0.0 && f.h > 0.0 && f.intersects(&bounds),
                Some(f) => f.w > 0.0 && f.h > 0.0,
                None => false,
            };
            let bits = raw.attrs.get(&attrs::TRAITS).and_then(Ka::as_u64).unwrap_or(0);
            let value = value_text(raw.attrs.get(&attrs::VALUE)).filter(|v| !v.trim().is_empty());
            let identifier = text(raw.attrs.get(&attrs::IDENTIFIER));
            let has_text = label.is_some() || value.is_some();
            let role = attrs::role_name(raw.attrs.get(&attrs::ROLE).and_then(Ka::as_i64), bits, false, has_text);
            // A container with only an identifier stays: Calculator's display keeps its number
            // there (PITFALLS #10).
            if !opts.all && (!on_screen || (role == "other" && !has_text && identifier.is_none())) {
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
