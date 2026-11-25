//! `type`: text into the focused field, whatever the device's keyboard layout makes of HID keys.
//!
//! An HID key types what the device's input source puts on it, and the keyboard surface every
//! HID connection has uses whatever source a hardware keyboard last left behind, device-wide
//! (PITFALLS #35). So there are two methods:
//!
//! - `stream` (default): attach a hardware keyboard of our own (PITFALLS #36), whose source we can
//!   steer (LANG2 selects Latin, ctrl+space goes back to Korean), send all keys with the switches
//!   at the script boundaries, remove it, and read the field once (PITFALLS #38). A mismatch
//!   (autocorrection, an unknown source) is deleted and typed again with `tap`.
//! - `tap`: go run by run, sending each script's first letters by HID and reading the field; where
//!   they came out as the other script, delete them and tap that script on the on-screen keys
//!   (their frames come from the accessibility tree). Hangul composes only while the on-screen
//!   keyboard is Korean (old PITFALLS #58), so it is switched there first and back at the end.
//!
//! Without a readable value (a field that hides it, or no focused field) nothing can be checked:
//! the keys go out and the result says `verified: null`.

use std::{collections::HashMap, time::Duration};

use anyhow::{Result, bail};
use serde::Serialize;

use crate::{
    Controller,
    ax::{BuildOptions, Element, Screen},
    keyboard::{self, Script, Stroke, usage},
};

/// The keys redraw this long after a tap on the next-keyboard key or Shift.
const REDRAW: Duration = Duration::from_millis(250);
/// After typing, before reading the field: the last reports go out unanswered (old PITFALLS #16).
const SETTLE_BEFORE_READ: Duration = Duration::from_millis(250);
/// How long the input-source list that ctrl+space brings up takes the next space, Return or
/// LANG2 for itself: a space 1.0 s after was lost, 1.5 s after was typed (PITFALLS #38).
const SOURCE_LIST: Duration = Duration::from_millis(1700);
/// After removing the hardware keyboard, before the on-screen keyboard is read (PITFALLS #36).
const KEYBOARD_BACK: Duration = Duration::from_millis(500);
/// Readings taken again when the field looks unchanged after keys, before believing it.
const STALE_READS: usize = 2;
/// How long the on-screen keys may take to show a switch.
const KEYS_WAIT: Duration = Duration::from_secs(2);
/// The next-keyboard key steps through the device's keyboards; few devices have more than four.
const MAX_SWITCHES: usize = 4;

/// The Latin letter rows and the Dubeolsik jamo on the same keys, and the jamo with Shift.
const ROWS: [(&str, &str, &str); 3] = [
    ("qwertyuiop", "ㅂㅈㄷㄱㅅㅛㅕㅑㅐㅔ", "ㅃㅉㄸㄲㅆㅛㅕㅑㅒㅖ"),
    ("asdfghjkl", "ㅁㄴㅇㄹㅎㅗㅓㅏㅣ", "ㅁㄴㅇㄹㅎㅗㅓㅏㅣ"),
    ("zxcvbnm", "ㅋㅌㅊㅍㅠㅜㅡ", "ㅋㅌㅊㅍㅠㅜㅡ"),
];

/// The script the on-screen letter keys show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardLanguage {
    Latin,
    Korean,
}

impl KeyboardLanguage {
    fn of(script: Script) -> Option<Self> {
        match script {
            Script::Latin => Some(Self::Latin),
            Script::Hangul => Some(Self::Korean),
            Script::Common => None,
        }
    }
}

/// The on-screen keyboard as the accessibility tree lists it: elements with the `keyboard_key`
/// trait, labeled with what they type (`Q`, `ㅂ`) or what they do (`shift`, `delete`).
#[derive(Debug, Clone)]
struct OnScreenKeys {
    language: Option<KeyboardLanguage>,
    /// Letter key centers by their Latin position (`q` is also ㅂ).
    letters: HashMap<char, (f64, f64)>,
    shift: Option<(f64, f64)>,
    /// Shift is on: the next letter is a capital (a Latin keyboard at a sentence start) or a
    /// doubled jamo.
    shifted: bool,
    /// The next-keyboard (globe) key. Its label is localized ("Next keyboard", "다음 키보드");
    /// it is the one key with a value, the name of the keyboard it switches to.
    next: Option<(f64, f64)>,
    /// That name ("English (US)", "한국어").
    next_name: Option<String>,
}

impl OnScreenKeys {
    fn read(screen: &Screen) -> Option<Self> {
        let keys: Vec<&Element> = screen.elements.iter().filter(|e| e.traits.iter().any(|t| t == "keyboard_key")).collect();
        if keys.is_empty() {
            return None;
        }
        let center = |e: &Element| e.frame.map(|f| f.center());
        let (mut latin, mut jamo) = (0, 0);
        let mut letters = HashMap::new();
        let mut shift = None;
        let mut shifted = false;
        let mut next = None;
        let mut next_name = None;
        for e in &keys {
            let Some(label) = e.label.as_deref() else { continue };
            let Some(at) = center(e) else { continue };
            if label.eq_ignore_ascii_case("shift") {
                shift = Some(at);
                shifted = e.selected || e.traits.iter().any(|t| t == "selected");
                continue;
            }
            if e.value.is_some() && label.chars().count() > 1 {
                next = Some(at);
                next_name = e.value.clone();
                continue;
            }
            let mut chars = label.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else { continue };
            if c.is_ascii_alphabetic() {
                latin += 1;
                letters.insert(c.to_ascii_lowercase(), at);
            } else if let Some(p) = jamo_position(c) {
                jamo += 1;
                letters.insert(p, at);
            }
        }
        let language = if latin >= 20 && latin > jamo {
            Some(KeyboardLanguage::Latin)
        } else if jamo >= 20 && jamo > latin {
            Some(KeyboardLanguage::Korean)
        } else {
            None
        };
        Some(Self { language, letters, shift, shifted, next, next_name })
    }
}

/// What a field held before typing, given its reading then (`shown`) and after (`after`). An
/// empty field reads as its placeholder (Settings' search field reads "Search"), which typing
/// replaces rather than extends: when `after` no longer contains `shown`, the field was empty.
fn baseline<'a>(shown: &'a str, after: &str) -> &'a str {
    if after.contains(shown) { shown } else { "" }
}

/// The Latin position of a jamo key label, plain or shifted.
fn jamo_position(c: char) -> Option<char> {
    for (latin, plain, shifted) in ROWS {
        for (i, (p, s)) in plain.chars().zip(shifted.chars()).enumerate() {
            if c == p || c == s {
                return latin.chars().nth(i);
            }
        }
    }
    None
}

/// The element that has the keyboard.
fn focused(screen: &Screen) -> Option<&Element> {
    screen.elements.iter().find(|e| e.focused)
}

/// What `type` did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Typed {
    /// How the keys went: `stream` (all keys through our hardware keyboard, switching its input
    /// source between scripts) or `tap` (HID keys checked script by script, and on-screen keys
    /// tapped where the HID keys typed the other script). `stream+tap` when the streamed text
    /// came out wrong and was typed again the slow way.
    pub method: &'static str,
    /// Characters typed.
    pub chars: usize,
    /// Keys sent by HID, and on-screen keys tapped.
    pub hid_keys: usize,
    pub tapped_keys: usize,
    /// Scripts typed by tapping the on-screen keys, because the HID keys typed the other script.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tapped: Vec<KeyboardLanguage>,
    /// The on-screen keyboard was switched for the typing, and back afterwards.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub switched_keyboard: bool,
    /// Input source switches between scripts while streaming.
    #[serde(skip_serializing_if = "is_zero")]
    pub source_switches: usize,
    /// Whether the focused field's value shows the text afterwards; null when it can't be read.
    pub verified: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    pub ms: u64,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// How `type` sends the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TypeMethod {
    /// Stream the keys through a temporary hardware keyboard (PITFALLS #38), check once at the
    /// end, and fall back to `Tap` when the field doesn't show the text.
    #[default]
    Stream,
    /// HID keys checked run by run, on-screen keys tapped where they type the other script.
    Tap,
}

impl TypeMethod {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "stream" => Self::Stream,
            "tap" => Self::Tap,
            other => bail!("unknown type method `{other}`: stream or tap"),
        })
    }
}

/// How one script is typed in this call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Way {
    Hid,
    Tap,
}

const NO_FOCUS: &str = "no text field has the keyboard: the on-screen keyboard is not up and no element has focus. \
    Tap the field first and check with `ui` that the keyboard is up, then type again. Keys typed without a focused \
    field become app shortcuts (a space can start playback). If a hardware keyboard hides the on-screen keyboard, \
    pass --force";

impl Controller {
    /// Reads the screen for typing: a fresh reading, since typing changes it.
    async fn read_for_typing(&mut self) -> Result<Screen> {
        // Boxed, like the other steps of `type`: their futures nest deep enough to overflow the
        // daemon thread's stack in a debug build.
        let began = std::time::Instant::now();
        let r = Box::pin(self.ui(BuildOptions::default(), false)).await;
        tracing::debug!("type: screen read in {} ms", began.elapsed().as_millis());
        r
    }

    /// The focused field's value now (empty when it has none), or None without a focused field.
    ///
    /// The first reading after input can come back without the field, or without anything
    /// (PITFALLS #12), so a reading with no focused element is taken again.
    async fn field_value(&mut self) -> Result<Option<String>> {
        tokio::time::sleep(SETTLE_BEFORE_READ).await;
        for _ in 0..STALE_READS {
            let screen = self.read_for_typing().await?;
            if let Some(e) = focused(&screen) {
                return Ok(Some(e.value.clone().unwrap_or_default()));
            }
        }
        let screen = self.read_for_typing().await?;
        Ok(focused(&screen).map(|e| e.value.clone().unwrap_or_default()))
    }

    /// Types `text` into the focused field (see the module docs). `force` sends the keys without
    /// looking at the screen. `enter` presses Return at the end.
    pub async fn type_text(&mut self, text: &str, enter: bool, force: bool, method: TypeMethod) -> Result<Typed> {
        let bad = keyboard::untypable(text);
        if !bad.is_empty() {
            let list = bad.iter().map(|c| format!("{c:?}")).collect::<Vec<_>>().join(", ");
            bail!("the keyboard can't type {list} (only ASCII and Hangul); use `paste` for text with them");
        }
        match method {
            TypeMethod::Stream => Box::pin(self.type_streamed(text, enter, force)).await,
            TypeMethod::Tap => Box::pin(self.type_tapping(text, enter, force)).await,
        }
    }

    /// The `tap` method. With our hardware keyboard attached the on-screen keys are hidden, so it
    /// is removed for the typing and attached again afterwards (PITFALLS #36).
    async fn type_tapping(&mut self, text: &str, enter: bool, force: bool) -> Result<Typed> {
        let (_, attached) = self.keyboard_state();
        if !attached || force {
            return Box::pin(self.type_on_screen_keys(text, enter, force)).await;
        }
        self.set_keyboard(false).await?;
        tokio::time::sleep(KEYBOARD_BACK).await;
        let typed = Box::pin(self.type_on_screen_keys(text, enter, force)).await;
        let back = self.set_keyboard(true).await;
        let mut typed = typed?;
        back?;
        typed.notes.push("the hardware keyboard was removed for the typing and attached again".into());
        Ok(typed)
    }

    /// The `stream` method (PITFALLS #38). A hardware keyboard types what the device's input
    /// source puts on its keys, and that source can be steered: attached, it starts on the
    /// language the on-screen keyboard shows; LANG2 selects the Latin source; ctrl+space goes back
    /// to the source before. So the keys go out in one stream with the switches at the script
    /// boundaries, and one reading at the end checks the field.
    async fn type_streamed(&mut self, text: &str, enter: bool, force: bool) -> Result<Typed> {
        let began = std::time::Instant::now();
        let mut out = Typed { method: "stream", chars: text.chars().count(), ..Default::default() };
        let (_, attached) = self.keyboard_state();
        let (keys, before) = if force {
            (None, None)
        } else {
            let screen = self.read_for_typing().await?;
            let keys = OnScreenKeys::read(&screen);
            let before = focused(&screen).map(|e| e.value.clone().unwrap_or_default());
            if keys.is_none() && before.is_none() {
                bail!(NO_FOCUS);
            }
            (keys, before)
        };
        let original = keys.as_ref().and_then(|k| k.language);
        let hangul = text.chars().any(keyboard::is_hangul);
        // The source it starts on is the on-screen keyboard's language: with Hangul to type, make
        // that Korean, so ctrl+space after LANG2 comes back to it.
        let mut start = original;
        if !attached && hangul && original != Some(KeyboardLanguage::Korean) && let Some(k) = keys.clone() {
            Box::pin(self.show_keyboard(KeyboardLanguage::Korean, Some(k), &mut out)).await?;
            start = Some(KeyboardLanguage::Korean);
        }
        if !attached {
            self.attach_for_typing(true).await?;
        }
        let mut current = if attached { None } else { start };
        let streamed = Box::pin(self.stream_keys(text, &mut current, &mut out)).await;
        if !attached {
            // Even after a failure: an attached keyboard hides the on-screen one.
            self.attach_for_typing(false).await?;
        }
        streamed?;

        let Some(before) = before else {
            if enter {
                out.hid_keys += 1;
                self.press_keys(&[Stroke::plain(usage::RETURN)]).await?;
            }
            out.ms = began.elapsed().as_millis() as u64;
            return Ok(out);
        };
        // One reading: the field's value, and the on-screen keyboard, back after the removal.
        let (after, keys_after) = Box::pin(self.read_field_and_keys()).await?;
        let typed_ok = after.as_deref().is_some_and(|a| {
            a.contains(text) && a.chars().count() == baseline(&before, a).chars().count() + out.chars
        });
        if typed_ok {
            out.verified = Some(true);
        } else if let Some(after) = after {
            tracing::debug!(
                "type: streamed text didn't check out ({} chars before, {} after, {} typed); typing it again by tapping",
                before.chars().count(),
                after.chars().count(),
                out.chars
            );
            Box::pin(self.undo_to(&before, &after)).await?;
            let again = Box::pin(self.type_tapping(text, false, false)).await?;
            out.method = "stream+tap";
            out.hid_keys += again.hid_keys;
            out.tapped_keys += again.tapped_keys;
            out.tapped = again.tapped;
            out.verified = again.verified;
            out.notes.extend(again.notes);
            out.notes.push("the streamed keys didn't come out as given (autocorrection, or an input source this doesn't know); the text was deleted and typed again by tapping".into());
        }
        // Put the on-screen keyboard back on the language it showed.
        if let (Some(lang), Some(k)) = (original, keys_after)
            && k.language != Some(lang)
            && out.method == "stream"
        {
            Box::pin(self.show_keyboard(lang, Some(k), &mut out)).await?;
        }
        if enter {
            out.hid_keys += 1;
            self.press_keys(&[Stroke::plain(usage::RETURN)]).await?;
        }
        out.ms = began.elapsed().as_millis() as u64;
        Ok(out)
    }

    /// The focused field's value and the on-screen keys, from one reading (taken again when the
    /// field is missing, PITFALLS #12).
    async fn read_field_and_keys(&mut self) -> Result<(Option<String>, Option<OnScreenKeys>)> {
        tokio::time::sleep(SETTLE_BEFORE_READ).await;
        let mut r = (None, None);
        for _ in 0..=STALE_READS {
            let screen = self.read_for_typing().await?;
            r = (focused(&screen).map(|e| e.value.clone().unwrap_or_default()), OnScreenKeys::read(&screen));
            if r.0.is_some() {
                break;
            }
        }
        Ok(r)
    }

    /// Sends `text` through the attached hardware keyboard, switching its input source at the
    /// script boundaries, and leaves it on the Latin source (PITFALLS #38).
    async fn stream_keys(&mut self, text: &str, current: &mut Option<KeyboardLanguage>, out: &mut Typed) -> Result<()> {
        let lang2 = Stroke::plain(usage::LANG2);
        let ctrl_space = Stroke { usage: usage::SPACE, modifiers: vec![usage::LEFT_CTRL] };
        // While the list ctrl+space brings up shows, letters type but a space, Return or LANG2
        // is taken by the list.
        let mut list_until: Option<std::time::Instant> = None;
        async fn wait_list(until: &mut Option<std::time::Instant>) {
            if let Some(t) = until.take() {
                let now = std::time::Instant::now();
                if t > now {
                    tokio::time::sleep(t - now).await;
                }
            }
        }
        let runs = keyboard::runs(text);
        let letter_lang = |i: usize| runs[i..].iter().find_map(|(s, _)| KeyboardLanguage::of(*s));
        if current.is_none() {
            // An input source nobody can read (the user's hardware keyboard): start from Latin.
            self.press_keys(std::slice::from_ref(&lang2)).await?;
            *current = Some(KeyboardLanguage::Latin);
        }
        for (i, (script, run)) in runs.iter().enumerate() {
            let strokes = keyboard::strokes(run);
            match KeyboardLanguage::of(*script) {
                None => {
                    if run.starts_with(char::is_whitespace) {
                        wait_list(&mut list_until).await;
                    }
                }
                Some(lang) if *current != Some(lang) => {
                    out.source_switches += 1;
                    match lang {
                        KeyboardLanguage::Latin => {
                            wait_list(&mut list_until).await;
                            self.press_keys(std::slice::from_ref(&lang2)).await?;
                        }
                        KeyboardLanguage::Korean => {
                            self.press_keys(std::slice::from_ref(&ctrl_space)).await?;
                            list_until = Some(std::time::Instant::now() + SOURCE_LIST);
                        }
                    }
                    *current = Some(lang);
                }
                Some(_) => {}
            }
            out.hid_keys += strokes.len();
            self.press_keys(&strokes).await?;
            // Leave Korean right after its letters, before the spaces that follow: a space after
            // a composing syllable was lost when the switch came after it (PITFALLS #38).
            if *script == Script::Hangul && letter_lang(i + 1) != Some(KeyboardLanguage::Korean) {
                wait_list(&mut list_until).await;
                out.source_switches += 1;
                self.press_keys(std::slice::from_ref(&lang2)).await?;
                *current = Some(KeyboardLanguage::Latin);
            }
        }
        wait_list(&mut list_until).await;
        Ok(())
    }

    async fn type_on_screen_keys(&mut self, text: &str, enter: bool, force: bool) -> Result<Typed> {
        let began = std::time::Instant::now();
        let bad = keyboard::untypable(text);
        if !bad.is_empty() {
            let list = bad.iter().map(|c| format!("{c:?}")).collect::<Vec<_>>().join(", ");
            bail!("the keyboard can't type {list} (only ASCII and Hangul); use `paste` for text with them");
        }
        let mut out = Typed { chars: text.chars().count(), ..Default::default() };
        if force {
            let mut strokes = keyboard::strokes(text);
            if enter {
                strokes.push(Stroke::plain(usage::RETURN));
            }
            out.hid_keys = strokes.len();
            self.press_keys(&strokes).await?;
            out.ms = began.elapsed().as_millis() as u64;
            return Ok(out);
        }

        let screen = self.read_for_typing().await?;
        let keys = OnScreenKeys::read(&screen);
        // The keys as last read, while no input came since; saves a reading before a switch.
        let mut known = keys.clone();
        let field = focused(&screen).map(|e| e.value.clone().unwrap_or_default());
        if keys.is_none() && field.is_none() {
            bail!(NO_FOCUS);
        }
        let original = keys.as_ref().and_then(|k| k.language);
        let app = focused(&screen).map(|e| e.pid);
        let has_keys = keys.is_some();
        // The field's value as typing left it; None when there is no field to read.
        let mut value = field;
        // What an earlier call learned in this app; only for a field whose value can be read, so
        // the check at the end can catch a stale entry.
        let mut ways: HashMap<KeyboardLanguage, Way> = match (app, &value) {
            (Some(pid), Some(_)) => {
                self.typing_ways.iter().filter(|((p, _), _)| *p == pid).map(|((_, l), w)| (*l, *w)).collect()
            }
            _ => HashMap::new(),
        };

        for (script, run) in keyboard::runs(text) {
            let Some(lang) = KeyboardLanguage::of(script) else {
                let strokes = keyboard::strokes(&run);
                out.hid_keys += strokes.len();
                self.press_keys(&strokes).await?;
                known = None;
                value = None; // read again when it is needed
                continue;
            };
            // Hangul composes only with Korean keys showing, by HID or by tapping.
            if lang == KeyboardLanguage::Korean && has_keys {
                let (k, v) = Box::pin(self.show_keyboard(lang, known.take(), &mut out)).await?;
                known = Some(k);
                if v.is_some() {
                    value = v;
                }
            }
            let way = match ways.get(&lang) {
                Some(w) => *w,
                None => {
                    let w = Box::pin(self.probe(lang, &run, has_keys, &mut value, &mut out)).await?;
                    tracing::debug!("type: {lang:?} goes by {w:?}");
                    ways.insert(lang, w);
                    // The probe typed (and maybe deleted) keys: Shift may have changed.
                    known = None;
                    if w == Way::Hid {
                        // The probe typed the whole run (Latin) or its first syllable (Hangul).
                        let rest: String = match lang {
                            KeyboardLanguage::Latin => String::new(),
                            KeyboardLanguage::Korean => run.chars().skip(1).collect(),
                        };
                        let strokes = keyboard::strokes(&rest);
                        out.hid_keys += strokes.len();
                        self.press_keys(&strokes).await?;
                        value = None;
                        continue;
                    }
                    Way::Tap
                }
            };
            match way {
                Way::Hid => {
                    let strokes = keyboard::strokes(&run);
                    out.hid_keys += strokes.len();
                    self.press_keys(&strokes).await?;
                }
                Way::Tap => {
                    let (k, _) = Box::pin(self.show_keyboard(lang, known.take(), &mut out)).await?;
                    Box::pin(self.tap_run(&run, lang, k, &mut out)).await?;
                    if !out.tapped.contains(&lang) {
                        out.tapped.push(lang);
                    }
                }
            }
            known = None;
            value = None;
        }

        // Back before Return: a Return that sends or searches can take the keyboard away.
        // The reading that confirms the switch back also has the field's value to check.
        let mut last_value = None;
        if out.switched_keyboard
            && let Some(lang) = original
        {
            let (_, v) = Box::pin(self.show_keyboard(lang, None, &mut out)).await?;
            last_value = v;
        }
        if enter {
            out.hid_keys += 1;
            self.press_keys(&[Stroke::plain(usage::RETURN)]).await?;
        } else {
            let v = match last_value.filter(|v| v.contains(text)) {
                Some(v) => Some(v),
                None => Box::pin(self.field_value()).await?,
            };
            out.verified = v.map(|v| v.contains(text));
            if out.verified == Some(false) {
                out.notes.push(
                    "the field's value doesn't show the text as given: look at the screen (autocorrection, a field that \
                     hides or reformats its text, or keys typed in another script) before going on"
                        .into(),
                );
            }
        }
        if let Some(pid) = app {
            if out.verified == Some(false) {
                self.typing_ways.retain(|(p, _), _| *p != pid);
            } else if out.verified == Some(true) {
                for (l, w) in &ways {
                    self.typing_ways.insert((pid, *l), *w);
                }
            }
        }
        if out.tapped.contains(&KeyboardLanguage::Latin) {
            out.notes.push("Latin letters were tapped on the on-screen keys; autocorrection can change a tapped word".into());
        }
        out.ms = began.elapsed().as_millis() as u64;
        Ok(out)
    }

    /// Types the start of a run by HID and reads the field to see whether it came out as given.
    /// Latin sends the whole run, Hangul its first syllable. If it came out wrong it is deleted
    /// and the script is to be tapped. Without a readable field, Latin goes by HID and Hangul
    /// by tapping (when there are on-screen keys).
    async fn probe(
        &mut self,
        lang: KeyboardLanguage,
        run: &str,
        has_keys: bool,
        value: &mut Option<String>,
        out: &mut Typed,
    ) -> Result<Way> {
        let before = match value.clone() {
            Some(v) => Some(v),
            None => Box::pin(self.field_value()).await?,
        };
        let Some(before) = before else {
            return Ok(if lang == KeyboardLanguage::Korean && has_keys { Way::Tap } else { Way::Hid });
        };
        let probe: String = match lang {
            KeyboardLanguage::Latin => run.to_string(),
            KeyboardLanguage::Korean => run.chars().take(1).collect(),
        };
        let strokes = keyboard::strokes(&probe);
        out.hid_keys += strokes.len();
        self.press_keys(&strokes).await?;
        let mut after = Box::pin(self.field_value()).await?.unwrap_or_default();
        // A reading can come from before the keys (PITFALLS #37): read again before judging.
        for _ in 0..STALE_READS {
            if after != before {
                break;
            }
            after = Box::pin(self.field_value()).await?.unwrap_or_default();
        }
        tracing::debug!(
            "type: probe {lang:?}: {} chars before, {} after, {} typed",
            before.chars().count(),
            after.chars().count(),
            probe.chars().count()
        );
        if after == before {
            out.notes.push(format!("the field's value didn't change after typing {lang:?} text; check the field"));
            return Ok(Way::Hid);
        }
        let grew = after.chars().count() == baseline(&before, &after).chars().count() + probe.chars().count();
        if grew && after.contains(&probe) {
            return Ok(Way::Hid);
        }
        if !has_keys {
            // Nothing to tap instead: leave it, and say so.
            out.notes.push(format!(
                "the keys for {lang:?} text came out differently here and there is no on-screen keyboard to tap; \
                 check the field"
            ));
            return Ok(Way::Hid);
        }
        Box::pin(self.undo_to(&before, &after)).await?;
        *value = Some(before);
        Ok(Way::Tap)
    }

    /// Presses Backspace until the field reads `before` again: once per character the keys
    /// added, then one at a time while the field still has more (a Hangul syllable that is
    /// still composing loses one jamo per Backspace, not the syllable). A reading that hasn't
    /// changed since the last press may be stale (PITFALLS #37), so it is read again first.
    async fn undo_to(&mut self, shown: &str, after: &str) -> Result<()> {
        // An empty field reads as its placeholder, and reads as it again once emptied.
        let before = baseline(shown, after);
        let extra = after.chars().count().saturating_sub(before.chars().count()).max(1);
        let backspace = Stroke::plain(usage::BACKSPACE);
        self.press_keys(&vec![backspace.clone(); extra]).await?;
        let mut prev = after.to_string();
        let mut same = 0;
        for _ in 0..extra * 3 + 6 {
            let now = Box::pin(self.field_value()).await?.unwrap_or_default();
            tracing::debug!("type: undo: {} chars, want {}", now.chars().count(), before.chars().count());
            if now == before || (before.is_empty() && now == shown) {
                return Ok(());
            }
            if now.chars().count() < before.chars().count() {
                break;
            }
            if now == prev && same < STALE_READS {
                same += 1;
                continue;
            }
            same = 0;
            prev = now;
            self.press_keys(std::slice::from_ref(&backspace)).await?;
        }
        bail!(
            "typed keys came out in the wrong script and could not be removed again; look at the field with `ui` and \
             delete what doesn't belong (key backspace)"
        )
    }

    /// Switches the on-screen keyboard to `lang` with its next-keyboard key, and returns it.
    /// `known` is a reading of the keys taken since the last input, if there is one. Also returns
    /// the focused field's value from the last reading.
    async fn show_keyboard(
        &mut self,
        lang: KeyboardLanguage,
        known: Option<OnScreenKeys>,
        out: &mut Typed,
    ) -> Result<(OnScreenKeys, Option<String>)> {
        let (mut keys, mut value) = match known {
            Some(k) => (k, None),
            None => Box::pin(self.read_keys(|_| true, None)).await?,
        };
        for _ in 0..MAX_SWITCHES {
            if keys.language == Some(lang) {
                return Ok((keys, value));
            }
            let Some(next) = keys.next else { break };
            let was = keys.language;
            let pick = keys.next_name.clone();
            self.tap(next, || {}).await?;
            out.switched_keyboard = true;
            // A reading right after the tap can still list the old keys (PITFALLS #37).
            (keys, value) = Box::pin(self.read_keys(move |k| k.language != was, pick.as_deref())).await?;
        }
        if keys.language == Some(lang) {
            return Ok((keys, value));
        }
        match lang {
            KeyboardLanguage::Korean => bail!(
                "the on-screen keyboard has no Korean layout (the next-keyboard key didn't reach one), and Hangul only \
                 composes on it. Ask the user to add a Korean keyboard (Settings > General > Keyboard > Keyboards), or \
                 use `paste`"
            ),
            KeyboardLanguage::Latin => bail!(
                "the on-screen keyboard has no Latin layout to tap the letters on; ask the user to add an English \
                 keyboard, or use `paste`"
            ),
        }
    }

    /// Reads the on-screen keys until `done` says they are what is awaited, for up to
    /// [`KEYS_WAIT`]; returns the last reading that had keys. A reading taken while the keys redraw
    /// can have none, or the old ones (PITFALLS #37).
    ///
    /// `pick` is the keyboard a tap on the next-keyboard key was for. If the device took the tap
    /// for a long press, it lists the keyboards over the keys instead; that one is picked from
    /// the list (PITFALLS #37).
    async fn read_keys(
        &mut self,
        done: impl Fn(&OnScreenKeys) -> bool,
        pick: Option<&str>,
    ) -> Result<(OnScreenKeys, Option<String>)> {
        let until = std::time::Instant::now() + KEYS_WAIT;
        let mut last = None;
        let mut picked = false;
        loop {
            tokio::time::sleep(REDRAW).await;
            let screen = self.read_for_typing().await?;
            let keys = OnScreenKeys::read(&screen);
            if let Some(name) = pick.filter(|_| !picked)
                && let Some(item) = screen.elements.iter().find(|e| {
                    e.label.as_deref() == Some(name) && !e.traits.iter().any(|t| t == "keyboard_key")
                })
                && let Some(at) = item.frame.map(|f| f.center())
            {
                tracing::debug!("type: the keyboard list is open; picking from it");
                self.tap(at, || {}).await?;
                picked = true;
                continue;
            }
            if let Some(k) = keys {
                let value = focused(&screen).map(|e| e.value.clone().unwrap_or_default());
                if done(&k) {
                    return Ok((k, value));
                }
                last = Some((k, value));
            }
            if std::time::Instant::now() >= until {
                return last.ok_or_else(|| {
                    anyhow::anyhow!("the on-screen keyboard went away while typing; tap the field again and check what was typed")
                });
            }
        }
    }

    /// Taps `run` on the on-screen keys, which show `lang`.
    async fn tap_run(&mut self, run: &str, lang: KeyboardLanguage, keys: OnScreenKeys, out: &mut Typed) -> Result<()> {
        let mut shifted = keys.shifted;
        let positions: Vec<(char, bool)> = match lang {
            KeyboardLanguage::Latin => run.chars().map(|c| (c.to_ascii_lowercase(), c.is_ascii_uppercase())).collect(),
            KeyboardLanguage::Korean => run
                .chars()
                .filter_map(keyboard::hangul_keys)
                .flat_map(|k| k.chars().collect::<Vec<_>>())
                .map(|c| (c.to_ascii_lowercase(), c.is_ascii_uppercase()))
                .collect(),
        };
        tracing::debug!("type: tapping {} keys on {lang:?}, shift {shifted}", positions.len());
        for (letter, want_shift) in positions {
            let Some(&at) = keys.letters.get(&letter) else {
                bail!("the on-screen keyboard has no key for `{letter}` (a layout this doesn't know); use `paste`");
            };
            if want_shift != shifted {
                let Some(shift) = keys.shift else {
                    bail!("the on-screen keyboard has no Shift key to tap; use `paste`");
                };
                self.tap(shift, || {}).await?;
                out.tapped_keys += 1;
            }
            self.tap(at, || {}).await?;
            out.tapped_keys += 1;
            // Shift goes off by itself after one letter.
            shifted = false;
        }
        Ok(())
    }
}

/// What `paste` did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Pasted {
    pub chars: usize,
    /// Whether the focused field shows the text afterwards; null when it can't be read.
    pub verified: Option<bool>,
    /// The device asks the user to allow the paste (old PITFALLS #30). It must not be approved for
    /// them; the clipboard keeps the text until `paste --restore`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub prompt: bool,
    /// What happened to the clipboard's earlier contents: `restored`, or `kept` until
    /// `paste --restore` (never their contents).
    pub clipboard: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    pub ms: u64,
}

/// Labels of the paste permission prompt's buttons, compared with what the device shows.
const PASTE_PROMPT: &[&str] = &["Allow Paste", "Don’t Allow Paste", "Don't Allow Paste", "붙여넣기 허용", "붙여넣기 허용 안 함"];
/// After ⌘V, before reading the field: a paste takes longer than a key to show.
const PASTE_SETTLE: Duration = Duration::from_millis(600);

impl Controller {
    /// Pastes `text` into the focused field through the clipboard: the clipboard's contents are
    /// saved in `backup` (unless it already holds an earlier paste's), replaced with `text`, ⌘V
    /// is pressed, and the contents are put back once the field shows the text. If the device
    /// asks to allow the paste, they stay in `backup`: put back then, the user's allowing would
    /// paste them instead.
    pub async fn paste_text(&mut self, text: &str, force: bool, backup: &mut Option<crate::Clipboard>) -> Result<Pasted> {
        let began = std::time::Instant::now();
        let mut out = Pasted { chars: text.chars().count(), clipboard: "restored", ..Default::default() };
        let screen = self.read_for_typing().await?;
        let before = focused(&screen).map(|e| e.value.clone().unwrap_or_default());
        if !force && before.is_none() && OnScreenKeys::read(&screen).is_none() {
            bail!(NO_FOCUS);
        }
        if backup.is_none() {
            *backup = Some(self.session.clipboard().await?);
        }
        if let Err(e) = self.put_clipboard_text(text).await {
            self.restore_backup(backup).await?;
            return Err(e.context("retry, or use `type`"));
        }
        self.press_keys(&[Stroke { usage: 0x19, modifiers: vec![usage::LEFT_CMD] }]).await?;
        tokio::time::sleep(PASTE_SETTLE).await;
        let screen = self.read_for_typing().await?;
        let after = focused(&screen).map(|e| e.value.clone().unwrap_or_default());
        out.prompt = screen.elements.iter().any(|e| e.label.as_deref().is_some_and(|l| PASTE_PROMPT.contains(&l)));
        out.verified = match (&before, &after) {
            (Some(b), Some(a)) => Some(a.contains(text) && a.chars().count() >= b.chars().count() + out.chars),
            _ => None,
        };
        if out.prompt && out.verified != Some(true) {
            out.clipboard = "kept";
            out.notes.push(
                "the device asks to allow the paste: don't allow it for the user. Decline it, or hand the device to them \
                 (`session handoff --reason \"allow or deny the paste\"`); once the prompt is gone, run \
                 `paste --restore` to put the clipboard back"
                    .into(),
            );
        } else {
            self.restore_backup(backup).await?;
            if out.verified == Some(false) {
                out.notes.push("the field doesn't show the pasted text; look at the screen with `ui`".into());
            }
        }
        out.ms = began.elapsed().as_millis() as u64;
        Ok(out)
    }

    /// Replaces the clipboard with `text` and reads it back, without pasting: the next paste
    /// on the device (by the user, or `paste`) inserts it.
    pub async fn put_clipboard_text(&mut self, text: &str) -> Result<()> {
        for _ in 0..3 {
            self.session.set_clipboard_text(text).await?;
            if matches!(self.session.clipboard().await?, crate::Clipboard::Text(t) if t == text) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        bail!("the device clipboard did not take the text (read back differently)")
    }

    /// Puts back the clipboard contents a paste saved, if any.
    pub async fn restore_backup(&mut self, backup: &mut Option<crate::Clipboard>) -> Result<bool> {
        let Some(saved) = backup.take() else { return Ok(false) };
        if let Err(e) = self.session.restore_clipboard(&saved).await {
            *backup = Some(saved);
            return Err(e.context("could not put the clipboard back; retry `paste --restore`"));
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ax::Rect;

    #[test]
    fn placeholder_is_an_empty_baseline() {
        assert_eq!(baseline("Search", "Bluetooth"), "");
        assert_eq!(baseline("Hello", "Hello World"), "Hello");
        assert_eq!(baseline("", "abc"), "");
    }

    fn key(n: usize, label: &str) -> Element {
        Element {
            reference: format!("@e{n}"),
            role: "key".into(),
            label: Some(label.into()),
            value: None,
            identifier: None,
            frame: Some(Rect { x: 10.0 * n as f64, y: 600.0, w: 10.0, h: 50.0 }),
            traits: vec!["keyboard_key".into()],
            enabled: true,
            selected: false,
            focused: false,
            pid: 1,
        }
    }

    fn screen(labels: &str) -> Screen {
        let mut elements: Vec<Element> = labels.chars().enumerate().map(|(i, c)| key(i, &c.to_string())).collect();
        let mut shift = key(90, "shift");
        shift.selected = true;
        elements.push(shift);
        let mut next = key(91, "Next keyboard");
        next.value = Some("English (US)".into());
        elements.push(next);
        Screen { width: 428.0, height: 926.0, elements, ..Default::default() }
    }

    #[test]
    fn reads_a_korean_keyboard() {
        let k = OnScreenKeys::read(&screen("ㅂㅈㄷㄱㅅㅛㅕㅑㅐㅔㅁㄴㅇㄹㅎㅗㅓㅏㅣㅋㅌㅊㅍㅠㅜㅡ")).unwrap();
        assert_eq!(k.language, Some(KeyboardLanguage::Korean));
        assert_eq!(k.letters[&'q'], k.letters[&'q']);
        assert!(k.letters.contains_key(&'m'));
        assert!(k.shifted);
        assert!(k.next.is_some());
    }

    #[test]
    fn reads_a_shifted_korean_keyboard_by_position() {
        let k = OnScreenKeys::read(&screen("ㅃㅉㄸㄲㅆㅛㅕㅑㅒㅖㅁㄴㅇㄹㅎㅗㅓㅏㅣㅋㅌㅊㅍㅠㅜㅡ")).unwrap();
        assert_eq!(k.language, Some(KeyboardLanguage::Korean));
        assert_eq!(k.letters.get(&'q').map(|p| p.0), Some(5.0));
        assert_eq!(k.letters.get(&'p').map(|p| p.0), Some(95.0));
    }

    #[test]
    fn reads_a_latin_keyboard() {
        let k = OnScreenKeys::read(&screen("QWERTYUIOPASDFGHJKLZXCVBNM")).unwrap();
        assert_eq!(k.language, Some(KeyboardLanguage::Latin));
        assert_eq!(k.letters.len(), 26);
    }

    #[test]
    fn a_number_pad_has_no_language() {
        let k = OnScreenKeys::read(&screen("1234567890-/:;()")).unwrap();
        assert_eq!(k.language, None);
    }
}
