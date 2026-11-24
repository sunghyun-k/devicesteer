//! What keys to press for a piece of text or a key name, and the HID keyboard report.
//!
//! An HID keyboard sends key positions, not characters: the device's layout decides what a key
//! types (PITFALLS #35). Hangul is therefore sent as the Dubeolsik keys that compose it, and the
//! device composes it when its keyboard is Korean. The tables here follow the Swift
//! implementation's `HangulKeys`, `TextCommand.usage` and `KeyCommand.named`.

use anyhow::{Result, bail};

/// HID usages (Keyboard/Keypad page 0x07) of the modifier keys.
pub mod usage {
    pub const RETURN: u8 = 0x28;
    pub const ESCAPE: u8 = 0x29;
    pub const BACKSPACE: u8 = 0x2A;
    pub const TAB: u8 = 0x2B;
    pub const SPACE: u8 = 0x2C;
    pub const LEFT_CTRL: u8 = 0xE0;
    pub const LEFT_SHIFT: u8 = 0xE1;
    pub const LEFT_ALT: u8 = 0xE2;
    pub const LEFT_CMD: u8 = 0xE3;
    pub const RIGHT_CTRL: u8 = 0xE4;
    pub const RIGHT_SHIFT: u8 = 0xE5;
    pub const RIGHT_ALT: u8 = 0xE6;
    pub const RIGHT_CMD: u8 = 0xE7;
    /// LANG2 (英数 on a Japanese keyboard): selects the Latin input source (PITFALLS #38).
    pub const LANG2: u8 = 0x91;
}

/// The highest usage the keyboard report has a bit for.
pub const MAX_USAGE: u8 = 0xE7;

/// Whether a usage is one of the eight modifier keys.
pub fn is_modifier(u: u8) -> bool {
    (usage::LEFT_CTRL..=usage::RIGHT_CMD).contains(&u)
}

/// The 39-byte report for the device's keyboard surface: report ID 1, one bit per usage below
/// 0xE8 (usage `u` is bit `u % 8` of byte `1 + u / 8`), a constant byte, a 48-bit timestamp and
/// two reserved bytes. It always carries **every key that is down**, not a change: an empty set
/// releases them all.
pub fn report(down: impl IntoIterator<Item = u8>) -> Vec<u8> {
    let mut r = vec![0u8; 39];
    r[0] = 0x01;
    for u in down {
        if (1..=MAX_USAGE).contains(&u) {
            r[1 + usize::from(u / 8)] |= 1 << (u % 8);
        }
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        & ((1 << 48) - 1);
    r[31..37].copy_from_slice(&nanos.to_le_bytes()[..6]);
    r
}

/// One key press: the key and the modifiers held with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stroke {
    pub usage: u8,
    pub modifiers: Vec<u8>,
}

impl Stroke {
    pub fn plain(usage: u8) -> Self {
        Self { usage, modifiers: vec![] }
    }

    fn shifted(usage: u8, shift: bool) -> Self {
        Self { usage, modifiers: if shift { vec![usage::LEFT_SHIFT] } else { vec![] } }
    }
}

/// The US-layout key for an ASCII character, and whether it needs Shift. Digits and symbols sit
/// on the same keys in the Korean layout, so they type the same on either.
pub fn ascii_key(c: char) -> Option<(u8, bool)> {
    let b = u8::try_from(u32::from(c)).ok()?;
    Some(match c {
        'a'..='z' => (b - b'a' + 0x04, false),
        'A'..='Z' => (b - b'A' + 0x04, true),
        '1'..='9' => (b - b'1' + 0x1E, false),
        '0' => (0x27, false),
        '\n' => (usage::RETURN, false),
        '\t' => (usage::TAB, false),
        ' ' => (usage::SPACE, false),
        '-' => (0x2D, false),
        '=' => (0x2E, false),
        '[' => (0x2F, false),
        ']' => (0x30, false),
        '\\' => (0x31, false),
        ';' => (0x33, false),
        '\'' => (0x34, false),
        '`' => (0x35, false),
        ',' => (0x36, false),
        '.' => (0x37, false),
        '/' => (0x38, false),
        '!' => (0x1E, true),
        '@' => (0x1F, true),
        '#' => (0x20, true),
        '$' => (0x21, true),
        '%' => (0x22, true),
        '^' => (0x23, true),
        '&' => (0x24, true),
        '*' => (0x25, true),
        '(' => (0x26, true),
        ')' => (0x27, true),
        '_' => (0x2D, true),
        '+' => (0x2E, true),
        '{' => (0x2F, true),
        '}' => (0x30, true),
        '|' => (0x31, true),
        ':' => (0x33, true),
        '"' => (0x34, true),
        '~' => (0x35, true),
        '<' => (0x36, true),
        '>' => (0x37, true),
        '?' => (0x38, true),
        _ => return None,
    })
}

const INITIALS: [&str; 19] = ["r", "R", "s", "e", "E", "f", "a", "q", "Q", "t", "T", "d", "w", "W", "c", "z", "x", "v", "g"];
const MEDIALS: [&str; 21] = [
    "k", "o", "i", "O", "j", "p", "u", "P", "h", "hk", "ho", "hl", "y", "n", "nj", "np", "nl", "b", "m", "ml", "l",
];
const FINALS: [&str; 28] = [
    "", "r", "R", "rt", "s", "sw", "sg", "e", "f", "fr", "fa", "fq", "ft", "fx", "fv", "fg", "a", "q", "qt", "t", "T",
    "d", "w", "c", "z", "x", "v", "g",
];
/// Compatibility jamo U+3131 (ㄱ) to U+3163 (ㅣ), in order.
const JAMO: [&str; 51] = [
    "r", "R", "rt", "s", "sw", "sg", "e", "E", "f", "fr", "fa", "fq", "ft", "fx", "fv", "fg", "a", "q", "Q", "qt", "t",
    "T", "d", "w", "W", "c", "z", "x", "v", "g", "k", "o", "i", "O", "j", "p", "u", "P", "h", "hk", "ho", "hl", "y",
    "n", "nj", "np", "nl", "b", "m", "ml", "l",
];

/// The Dubeolsik (2-set Korean) keys, as Latin letters, that type one Hangul syllable or
/// compatibility jamo: "안" is `dks`. Capitals are the shifted keys (ㅃ is `Q`). Sent one
/// syllable after another, the device's Korean keyboard splits them right by itself: `rksk` is
/// "가나", because a final consonant moves to the next syllable when a vowel follows.
pub fn hangul_keys(c: char) -> Option<String> {
    let u = u32::from(c);
    match u {
        0xAC00..=0xD7A3 => {
            let i = (u - 0xAC00) as usize;
            Some(format!("{}{}{}", INITIALS[i / (21 * 28)], MEDIALS[(i % (21 * 28)) / 28], FINALS[i % 28]))
        }
        0x3131..=0x3163 => Some(JAMO[(u - 0x3131) as usize].to_string()),
        _ => None,
    }
}

/// Whether `c` is a compatibility jamo (a lone consonant or vowel, ㄱ to ㅣ).
pub fn is_jamo(c: char) -> bool {
    (0x3131..=0x3163).contains(&u32::from(c))
}

pub fn is_hangul(c: char) -> bool {
    hangul_keys(c).is_some()
}

/// How a character can be typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    /// A Latin letter: typed as letters only where the device's layout is Latin.
    Latin,
    /// Hangul: typed as Dubeolsik keys, which compose only where the layout is Korean.
    Hangul,
    /// Digits, symbols, spaces and newlines: the same key in both layouts.
    Common,
}

pub fn script(c: char) -> Option<Script> {
    if c.is_ascii_alphabetic() {
        Some(Script::Latin)
    } else if is_hangul(c) {
        Some(Script::Hangul)
    } else if ascii_key(c).is_some() {
        Some(Script::Common)
    } else {
        None
    }
}

/// The characters of `text` the HID keyboard can't type at all (emoji, accents, other scripts),
/// each once.
pub fn untypable(text: &str) -> Vec<char> {
    let mut out: Vec<char> = Vec::new();
    for c in text.chars() {
        if script(c).is_none() && !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// The key strokes that type `text`, Hangul as Dubeolsik keys. Characters the keyboard can't type
/// are left out (check [`untypable`] first).
pub fn strokes(text: &str) -> Vec<Stroke> {
    let mut out = Vec::new();
    for c in text.chars() {
        match hangul_keys(c) {
            Some(keys) => out.extend(keys.chars().filter_map(ascii_key).map(|(u, s)| Stroke::shifted(u, s))),
            None => out.extend(ascii_key(c).map(|(u, s)| Stroke::shifted(u, s))),
        }
    }
    out
}

/// Splits `text` into runs of one script. A run of [`Script::Common`] between two runs of the same
/// script stays its own run, so a space between two Hangul words is typed by HID either way.
pub fn runs(text: &str) -> Vec<(Script, String)> {
    let mut out: Vec<(Script, String)> = Vec::new();
    for c in text.chars() {
        let Some(s) = script(c) else { continue };
        match out.last_mut() {
            Some((last, run)) if *last == s => run.push(c),
            _ => out.push((s, c.to_string())),
        }
    }
    out
}

/// Named keys, for `key <name>` and the viewer: (names, usage).
const NAMED: &[(&[&str], u8)] = &[
    (&["return", "enter"], usage::RETURN),
    (&["escape", "esc"], usage::ESCAPE),
    (&["backspace", "delete"], usage::BACKSPACE),
    (&["forward_delete", "forwarddelete", "del"], 0x4C),
    (&["tab"], usage::TAB),
    (&["space"], usage::SPACE),
    (&["right"], 0x4F),
    (&["left"], 0x50),
    (&["down"], 0x51),
    (&["up"], 0x52),
    (&["home"], 0x4A),
    (&["end"], 0x4D),
    (&["pageup", "page_up"], 0x4B),
    (&["pagedown", "page_down"], 0x4E),
    (&["capslock", "caps_lock"], 0x39),
    // LANG1/LANG2 (Hangul/English toggle and Hanja on Korean keyboards).
    (&["lang1"], 0x90),
    (&["lang2"], 0x91),
    (&["f1"], 0x3A),
    (&["f2"], 0x3B),
    (&["f3"], 0x3C),
    (&["f4"], 0x3D),
    (&["f5"], 0x3E),
    (&["f6"], 0x3F),
    (&["f7"], 0x40),
    (&["f8"], 0x41),
    (&["f9"], 0x42),
    (&["f10"], 0x43),
    (&["f11"], 0x44),
    (&["f12"], 0x45),
];

const MODIFIERS: &[(&[&str], u8)] = &[
    (&["cmd", "command", "meta", "super"], usage::LEFT_CMD),
    (&["ctrl", "control"], usage::LEFT_CTRL),
    (&["alt", "opt", "option"], usage::LEFT_ALT),
    (&["shift"], usage::LEFT_SHIFT),
];

/// The names `key` takes, for help and errors.
pub fn key_names() -> String {
    NAMED.iter().map(|(n, _)| n[0]).collect::<Vec<_>>().join(", ")
}

/// Parses a key name with optional modifiers joined by `+`: `return`, `cmd+a`, `shift+tab`,
/// `ctrl+space`, or a single printable ASCII character (`a`, `?`).
pub fn parse_key(name: &str) -> Result<Stroke> {
    let name = name.trim();
    // "cmd++" ends in the "+" key.
    let (mods, key) = match name.strip_suffix("++") {
        Some(m) => (m, "+"),
        None => match name.rsplit_once('+') {
            Some((m, k)) if !k.is_empty() => (m, k),
            _ => ("", name),
        },
    };
    let mut modifiers = Vec::new();
    for m in mods.split('+').filter(|m| !m.is_empty()) {
        let m = m.trim().to_lowercase();
        let Some((_, u)) = MODIFIERS.iter().find(|(names, _)| names.contains(&m.as_str())) else {
            bail!("unknown modifier `{m}`; use cmd, ctrl, alt or shift, joined with + (cmd+shift+z)");
        };
        if !modifiers.contains(u) {
            modifiers.push(*u);
        }
    }
    let lower = key.to_lowercase().replace('-', "_");
    let usage = if let Some((_, u)) = NAMED.iter().find(|(names, _)| names.contains(&lower.as_str())) {
        *u
    } else if let Some((_, u)) = MODIFIERS.iter().find(|(names, _)| names.contains(&lower.as_str())) {
        *u
    } else {
        let mut chars = key.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => {
                let Some((u, shift)) = ascii_key(c) else {
                    bail!("`{c}` has no key on the HID keyboard; use `type` or `paste` for it");
                };
                // A capital with a modifier means the letter key (cmd+A is cmd+a); alone it is
                // shift+letter.
                let capital_with_modifier = c.is_ascii_uppercase() && !modifiers.is_empty();
                if shift && !capital_with_modifier && !modifiers.contains(&usage::LEFT_SHIFT) {
                    modifiers.push(usage::LEFT_SHIFT);
                }
                u
            }
            _ => bail!("unknown key `{key}`; use a single character or one of: {}", key_names()),
        }
    };
    Ok(Stroke { usage, modifiers })
}

/// The HID usage for a DOM `KeyboardEvent.code` (the physical key, whatever the layout or IME
/// makes of it), for forwarding the viewer's keyboard.
pub fn dom_code_usage(code: &str) -> Option<u8> {
    if let Some(l) = code.strip_prefix("Key") {
        let mut c = l.chars();
        if let (Some(ch @ 'A'..='Z'), None) = (c.next(), c.next()) {
            return Some(ch as u8 - b'A' + 0x04);
        }
    }
    if let Some(d) = code.strip_prefix("Digit") {
        return match d {
            "0" => Some(0x27),
            "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => Some(d.as_bytes()[0] - b'1' + 0x1E),
            _ => None,
        };
    }
    if let Some(n) = code.strip_prefix('F')
        && let Ok(n) = n.parse::<u8>()
        && (1..=12).contains(&n)
    {
        return Some(0x3A + n - 1);
    }
    Some(match code {
        "Enter" | "NumpadEnter" => usage::RETURN,
        "Escape" => usage::ESCAPE,
        "Backspace" => usage::BACKSPACE,
        "Tab" => usage::TAB,
        "Space" => usage::SPACE,
        "Minus" => 0x2D,
        "Equal" => 0x2E,
        "BracketLeft" => 0x2F,
        "BracketRight" => 0x30,
        "Backslash" => 0x31,
        "Semicolon" => 0x33,
        "Quote" => 0x34,
        "Backquote" => 0x35,
        "Comma" => 0x36,
        "Period" => 0x37,
        "Slash" => 0x38,
        "CapsLock" => 0x39,
        "Insert" => 0x49,
        "Home" => 0x4A,
        "PageUp" => 0x4B,
        "Delete" => 0x4C,
        "End" => 0x4D,
        "PageDown" => 0x4E,
        "ArrowRight" => 0x4F,
        "ArrowLeft" => 0x50,
        "ArrowDown" => 0x51,
        "ArrowUp" => 0x52,
        "IntlBackslash" => 0x64,
        // The Mac's 한/영 key reports Lang1, 한자 Lang2.
        "Lang1" => 0x90,
        "Lang2" => 0x91,
        "ControlLeft" => usage::LEFT_CTRL,
        "ShiftLeft" => usage::LEFT_SHIFT,
        "AltLeft" => usage::LEFT_ALT,
        "MetaLeft" => usage::LEFT_CMD,
        "ControlRight" => usage::RIGHT_CTRL,
        "ShiftRight" => usage::RIGHT_SHIFT,
        "AltRight" => usage::RIGHT_ALT,
        "MetaRight" => usage::RIGHT_CMD,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hangul_to_dubeolsik() {
        let keys = |s: &str| s.chars().map(|c| hangul_keys(c).unwrap()).collect::<String>();
        assert_eq!(keys("안녕"), "dkssud");
        assert_eq!(keys("빨간"), "Qkfrks");
        assert_eq!(keys("값"), "rkqt");
        assert_eq!(keys("ㅃㅘ"), "Qhk");
        assert_eq!(hangul_keys('a'), None);
    }

    #[test]
    fn strokes_shift_capitals_and_symbols() {
        let s = strokes("Hi!");
        assert_eq!(s[0], Stroke { usage: 0x0B, modifiers: vec![usage::LEFT_SHIFT] });
        assert_eq!(s[1], Stroke::plain(0x0C));
        assert_eq!(s[2], Stroke { usage: 0x1E, modifiers: vec![usage::LEFT_SHIFT] });
        // 빠 = Q k: shifted q, then k.
        assert_eq!(strokes("빠"), vec![Stroke { usage: 0x14, modifiers: vec![usage::LEFT_SHIFT] }, Stroke::plain(0x0E)]);
    }

    #[test]
    fn runs_split_by_script() {
        let r = runs("안녕 Seoul 1호선!");
        let kinds: Vec<Script> = r.iter().map(|(s, _)| *s).collect();
        assert_eq!(
            kinds,
            [Script::Hangul, Script::Common, Script::Latin, Script::Common, Script::Hangul, Script::Common]
        );
        assert_eq!(r[4].1, "호선");
        assert_eq!(untypable("a😀é😀"), vec!['😀', 'é']);
    }

    #[test]
    fn key_names_parse() {
        assert_eq!(parse_key("return").unwrap(), Stroke::plain(usage::RETURN));
        assert_eq!(parse_key("Enter").unwrap(), Stroke::plain(usage::RETURN));
        assert_eq!(parse_key("cmd+a").unwrap(), Stroke { usage: 0x04, modifiers: vec![usage::LEFT_CMD] });
        assert_eq!(parse_key("cmd+A").unwrap(), Stroke { usage: 0x04, modifiers: vec![usage::LEFT_CMD] });
        assert_eq!(
            parse_key("cmd+shift+z").unwrap(),
            Stroke { usage: 0x1D, modifiers: vec![usage::LEFT_CMD, usage::LEFT_SHIFT] }
        );
        assert_eq!(parse_key("ctrl+space").unwrap(), Stroke { usage: usage::SPACE, modifiers: vec![usage::LEFT_CTRL] });
        assert_eq!(parse_key("?").unwrap(), Stroke { usage: 0x38, modifiers: vec![usage::LEFT_SHIFT] });
        assert_eq!(parse_key("cmd++").unwrap(), Stroke { usage: 0x2E, modifiers: vec![usage::LEFT_CMD, usage::LEFT_SHIFT] });
        assert_eq!(parse_key("left").unwrap(), Stroke::plain(0x50));
        assert!(parse_key("hyper+a").is_err());
        assert!(parse_key("banana").is_err());
    }

    #[test]
    fn report_layout() {
        let r = report([0x04, usage::LEFT_SHIFT]);
        assert_eq!(r.len(), 39);
        assert_eq!(r[0], 1);
        assert_eq!(r[1], 0b1_0000);
        assert_eq!(r[1 + 0xE1 / 8], 1 << (0xE1 % 8));
        assert_eq!(&r[37..], &[0, 0]);
    }

    #[test]
    fn dom_codes() {
        assert_eq!(dom_code_usage("KeyA"), Some(0x04));
        assert_eq!(dom_code_usage("Digit0"), Some(0x27));
        assert_eq!(dom_code_usage("Digit1"), Some(0x1E));
        assert_eq!(dom_code_usage("F12"), Some(0x45));
        assert_eq!(dom_code_usage("MetaLeft"), Some(usage::LEFT_CMD));
        assert_eq!(dom_code_usage("Fn"), None);
    }
}
