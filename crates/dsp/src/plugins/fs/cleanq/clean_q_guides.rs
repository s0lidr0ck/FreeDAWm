//! CleanQGuides -- the annotations a preset can carry, and that you can write.
//! Translation of `CleanQ/Source/CleanQGuides.h`.
//!
//! A guide is a note pinned to a region of the spectrum: "there is a resonance around here, it
//! probably wants about this much out of it". It is not a filter and it changes no audio --
//! clicking one is what turns it into a band. They are deliberately NOT parameters.
//!
//! Where they live:
//! - in the plugin's own state, under `uiState/guides` (`writeTo` / `readFrom`): here the module
//!   state ([`super::clean_q_ceiling_state::CeilingState`]) carries the same `<guides>` element,
//!   one `<guide freq lo hi gain dyn done note/>` child each, after the `<ceiling>` element;
//! - in a preset file under a `"guides"` key (`toVar` / `fromVar`): [`Guides::to_json`] and
//!   [`Guides::from_json`].
//!
//! Translation notes:
//! - `readFrom` / `fromVar` sanitise every guide and stop at [`MAX_GUIDES`], as the C++ does.
//! - Never-crash guards, not in the C++: a guide read from state whose `freq`, `lo` or `hi` is not
//!   a finite number inside 10 Hz..30 kHz, or whose `lo` is not below its `hi`, is dropped rather
//!   than sanitised into somewhere nobody put it (hostile or corrupt state; the C++ only ever
//!   writes sanitised guides, so a real state never trips it). Notes are capped at 80 characters
//!   by `sanitise` as in the C++.
//! - Not in the C++: a guide may also carry an `area` (a short name for the problem area, "Mud /
//!   box") and a `hint` (the usual move, "cut 2-3 dB"), which the guide templates
//!   ([`super::clean_q_guide_templates`]) fill. Both are optional: an empty one is not written
//!   (no `area` / `hint` attribute, no JSON key), so a state or preset without them reads and
//!   writes back exactly as before, and the C++ plugin, which does not know them, ignores them.
//!   `sanitise` caps them at [`MAX_AREA`] and [`MAX_HINT`] characters, trimmed.

use super::clean_q_ceiling_state::{attribute, find_element, juce_bool, juce_double};
use serde_json::{Map, Value, json};

/// `freqMin`, `freqMax` (`CleanQConfig.h`).
pub const FREQ_MIN: f64 = 10.0;
pub const FREQ_MAX: f64 = 30_000.0;
/// `GuideList::maxGuides`.
pub const MAX_GUIDES: usize = 24;
/// The longest `area` (characters, after trimming); a template's are far shorter.
pub const MAX_AREA: usize = 40;
/// The longest `hint` (characters, after trimming), as long as a note.
pub const MAX_HINT: usize = 80;

/// `cq::Guide`.
#[derive(Debug, Clone, PartialEq)]
pub struct Guide {
    pub freq_hz: f64,
    pub lo_hz: f64,
    pub hi_hz: f64,
    pub gain_db: f64,
    pub dynamic: bool,
    pub note: String,
    /// Set once you have acted on it: drawn dimmed, kept with its note.
    pub done: bool,
    /// The problem area's short name ("Mud / box"); empty for a guide you wrote yourself.
    pub area: String,
    /// The usual move there ("cut 2-3 dB"), shown as a hint and never applied; may be empty.
    pub hint: String,
}

impl Default for Guide {
    fn default() -> Self {
        Guide {
            freq_hz: 1000.0,
            lo_hz: 800.0,
            hi_hz: 1250.0,
            gain_db: -3.0,
            dynamic: false,
            note: String::new(),
            done: false,
            area: String::new(),
            hint: String::new(),
        }
    }
}

/// `juce::jlimit (lo, hi, v)` (NaN passes through, as in JUCE; callers guard it).
fn jlimit(lo: f64, hi: f64, v: f64) -> f64 {
    if v < lo {
        lo
    } else if hi < v {
        hi
    } else {
        v
    }
}

impl Guide {
    /// `sanitise`: inside the ranges the plugin can express, `lo <= freq <= hi`, the note at most
    /// 80 characters, trimmed. Never-crash guard: a non-finite value is first taken as 1 kHz
    /// (`freq`, `lo`, `hi`) or -3 dB (`gain`).
    pub fn sanitise(&mut self) {
        let finite = |v: f64, d: f64| if v.is_finite() { v } else { d };
        self.lo_hz = jlimit(FREQ_MIN, FREQ_MAX, finite(self.lo_hz, 1000.0));
        self.hi_hz = jlimit(FREQ_MIN, FREQ_MAX, finite(self.hi_hz, 1000.0));
        if self.hi_hz < self.lo_hz {
            std::mem::swap(&mut self.lo_hz, &mut self.hi_hz);
        }
        self.freq_hz = jlimit(self.lo_hz, self.hi_hz, jlimit(FREQ_MIN, FREQ_MAX, finite(self.freq_hz, 1000.0)));
        self.gain_db = jlimit(-30.0, 30.0, finite(self.gain_db, -3.0));
        // `note.substring (0, 80).trim()`
        self.note = clip_trim(&self.note, 80);
        self.area = clip_trim(&self.area, MAX_AREA);
        self.hint = clip_trim(&self.hint, MAX_HINT);
    }

    /// The guide's label on the graph: its area, else its note, else nothing.
    pub fn label(&self) -> &str {
        if self.area.is_empty() { &self.note } else { &self.area }
    }

    /// `Guide::around`: a bare frequency gets about a third of an octave either side.
    pub fn around(hz: f64, gain: f64) -> Guide {
        let mut g = Guide { freq_hz: hz, lo_hz: hz / 1.26, hi_hz: hz * 1.26, gain_db: gain, ..Guide::default() };
        g.sanitise();
        g
    }

    /// The never-crash check on a guide read from state (see the module notes).
    fn plausible(&self) -> bool {
        let inside = |v: f64| v.is_finite() && (FREQ_MIN..=FREQ_MAX).contains(&v);
        inside(self.freq_hz) && inside(self.lo_hz) && inside(self.hi_hz) && self.lo_hz < self.hi_hz && self.gain_db.is_finite()
    }
}

/// `s.substring (0, n).trim()`.
fn clip_trim(s: &str, n: usize) -> String {
    s.chars().take(n).collect::<String>().trim().to_string()
}

/// `GuideList`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Guides {
    pub list: Vec<Guide>,
}

/// `GuideList::colourFor`: `MCOLORS`, as 0xRRGGBB.
pub const GUIDE_COLOURS: [u32; 8] = [0xFFD166, 0xFF8A5C, 0x7BE0B0, 0xC9A2FF, 0xFF7AB6, 0x7FD8FF, 0xFFB347, 0xB8FF7A];

impl Guides {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// `add`: the index it landed at, or `None` when the list is full.
    pub fn add(&mut self, mut g: Guide) -> Option<usize> {
        if self.list.len() >= MAX_GUIDES {
            return None;
        }
        g.sanitise();
        self.list.push(g);
        Some(self.list.len() - 1)
    }

    /// `replace`.
    pub fn replace(&mut self, i: usize, mut g: Guide) {
        if let Some(slot) = self.list.get_mut(i) {
            g.sanitise();
            *slot = g;
        }
    }

    /// `remove`.
    pub fn remove(&mut self, i: usize) {
        if i < self.list.len() {
            self.list.remove(i);
        }
    }

    /// `numOutstanding`: what the button's badge counts.
    pub fn num_outstanding(&self) -> usize {
        self.list.iter().filter(|g| !g.done).count()
    }

    /// `colourFor (index)` as 0xRRGGBB.
    pub fn colour_for(index: usize) -> u32 {
        GUIDE_COLOURS[index % 8]
    }

    /// `toVar`: what goes into a preset under `"guides"` (values rounded to 0.1).
    pub fn to_json(&self) -> Value {
        let r = |v: f64| (v * 10.0).round() / 10.0;
        Value::Array(
            self.list
                .iter()
                .map(|g| {
                    let mut o = Map::new();
                    o.insert("freq".into(), json!(r(g.freq_hz)));
                    o.insert("lo".into(), json!(r(g.lo_hz)));
                    o.insert("hi".into(), json!(r(g.hi_hz)));
                    o.insert("gain".into(), json!(r(g.gain_db)));
                    if g.dynamic {
                        o.insert("dynamic".into(), json!(true));
                    }
                    if g.done {
                        o.insert("done".into(), json!(true));
                    }
                    if !g.note.is_empty() {
                        o.insert("note".into(), json!(g.note));
                    }
                    if !g.area.is_empty() {
                        o.insert("area".into(), json!(g.area));
                    }
                    if !g.hint.is_empty() {
                        o.insert("hint".into(), json!(g.hint));
                    }
                    Value::Object(o)
                })
                .collect(),
        )
    }

    /// `fromVar`: anything unusable is skipped; `label` and `reason` are spellings of `note`.
    pub fn from_json(v: &Value) -> Guides {
        let mut out = Guides::default();
        let Some(arr) = v.as_array() else { return out };
        // `(double) var`: a number, a numeric string, a bool; anything else 0.
        let num = |x: Option<&Value>| -> Option<f64> {
            x.map(|x| match x {
                Value::Number(n) => n.as_f64().unwrap_or(0.0),
                Value::String(s) => juce_double(s),
                Value::Bool(b) => f64::from(u8::from(*b)),
                _ => 0.0,
            })
        };
        // An optional text: a string as it is, anything else none.
        let text = |x: Option<&Value>| x.and_then(Value::as_str).map(|s| s.chars().take(4 * MAX_HINT).collect()).unwrap_or_default();
        let truthy = |x: Option<&Value>| match x {
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
            Some(Value::String(s)) => juce_bool(s),
            _ => false,
        };
        for item in arr {
            let Some(o) = item.as_object() else { continue };
            let f = num(o.get("freq")).unwrap_or(0.0);
            if f.is_nan() || f <= 0.0 || !f.is_finite() {
                continue; // a guide with no frequency says nothing
            }
            let mut g = Guide {
                freq_hz: f,
                lo_hz: num(o.get("lo")).unwrap_or(f / 1.26),
                hi_hz: num(o.get("hi")).unwrap_or(f * 1.26),
                gain_db: num(o.get("gain")).unwrap_or(-3.0),
                dynamic: truthy(o.get("dynamic")),
                done: truthy(o.get("done")),
                note: String::new(),
                area: text(o.get("area")),
                hint: text(o.get("hint")),
            };
            for key in ["note", "label", "reason"] {
                if let Some(n) = o.get(key) {
                    g.note = match n {
                        Value::String(s) => s.clone(),
                        Value::Null => String::new(),
                        other => other.to_string(),
                    };
                    break;
                }
            }
            g.sanitise();
            out.list.push(g);
            if out.list.len() >= MAX_GUIDES {
                break;
            }
        }
        out
    }

    /// `writeTo`: the `<guides>` element (nothing when there are none, so a state without guides
    /// is byte-identical to one written before guides were carried).
    pub fn write_xml(&self, out: &mut String) {
        if self.list.is_empty() {
            return;
        }
        out.push_str("<guides>");
        for g in &self.list {
            // The optional texts only when there are any (a guide without them is written as before).
            let mut extra = String::new();
            if !g.area.is_empty() {
                extra.push_str(&format!(" area=\"{}\"", xml_escape(&g.area)));
            }
            if !g.hint.is_empty() {
                extra.push_str(&format!(" hint=\"{}\"", xml_escape(&g.hint)));
            }
            out.push_str(&format!(
                "<guide freq=\"{}\" lo=\"{}\" hi=\"{}\" gain=\"{}\" dyn=\"{}\" done=\"{}\" note=\"{}\"{extra}/>",
                var_double(g.freq_hz),
                var_double(g.lo_hz),
                var_double(g.hi_hz),
                var_double(g.gain_db),
                u8::from(g.dynamic),
                u8::from(g.done),
                xml_escape(&g.note)
            ));
        }
        out.push_str("</guides>");
    }

    /// `readFrom`: every `guide` child of the first `guides` element, at most [`MAX_GUIDES`]. No
    /// element: none.
    pub fn read_xml(text: &str) -> Guides {
        let mut out = Guides::default();
        let Some(el) = find_element(text, "guides") else { return out };
        let mut rest = el.body;
        // A hostile state may hold thousands: stop scanning once enough have been looked at.
        let mut looked = 0usize;
        while out.list.len() < MAX_GUIDES && looked < 4 * MAX_GUIDES {
            let Some(node) = find_element(rest, "guide") else { break };
            rest = node.after;
            looked += 1;
            let a = |k: &str| attribute(node.attrs, k);
            let mut g = Guide {
                freq_hz: a("freq").map_or(1000.0, juce_double),
                lo_hz: a("lo").map_or(800.0, juce_double),
                hi_hz: a("hi").map_or(1250.0, juce_double),
                gain_db: a("gain").map_or(-3.0, juce_double),
                dynamic: a("dyn").is_some_and(juce_bool),
                done: a("done").is_some_and(juce_bool),
                note: a("note").map(|n| xml_unescape(clip_raw(n))).unwrap_or_default(),
                area: a("area").map(|n| xml_unescape(clip_raw(n))).unwrap_or_default(),
                hint: a("hint").map(|n| xml_unescape(clip_raw(n))).unwrap_or_default(),
            };
            if !g.plausible() {
                continue;
            }
            g.sanitise();
            out.list.push(g);
        }
        out
    }
}

/// A double as the JUCE `var` prints it into XML: whole numbers keep a `.0`.
fn var_double(v: f64) -> String {
    if v.is_finite() && v == v.trunc() && v.abs() < 1.0e15 { format!("{v:.1}") } else { format!("{v}") }
}

/// `XmlElement`'s attribute escaping: `& < > " '`, and control characters as `&#N;`.
fn xml_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 32 => o.push_str(&format!("&#{};", c as u32)),
            c => o.push(c),
        }
    }
    o
}

/// The most of a raw (escaped) note that is unescaped: 80 characters escape to at most ~800
/// bytes; anything longer is cut first (on a character boundary), as `sanitise` cuts the note.
const MAX_RAW_NOTE: usize = 1024;

fn clip_raw(s: &str) -> &str {
    if s.len() <= MAX_RAW_NOTE {
        return s;
    }
    let mut end = MAX_RAW_NOTE;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or_default()
}

/// The inverse of [`xml_escape`] (named and numeric entities; anything unknown left as it is).
fn xml_unescape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        o.push_str(rest.get(..at).unwrap_or_default());
        let tail = rest.get(at..).unwrap_or_default();
        // An entity is short: look for its ';' in the next few bytes only (a hostile run of '&'
        // must not make this quadratic).
        let Some(semi) = tail.as_bytes().iter().take(12).position(|b| *b == b';') else {
            o.push('&');
            rest = tail.get(1..).unwrap_or_default();
            continue;
        };
        let ent = tail.get(1..semi).unwrap_or_default();
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") || e.starts_with("#X") => e.get(2..).and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32),
            e if e.starts_with('#') => e.get(1..).and_then(|d| d.parse::<u32>().ok()).and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                o.push(c);
                rest = tail.get(semi + 1..).unwrap_or_default();
            }
            None => {
                o.push('&');
                rest = tail.get(1..).unwrap_or_default();
            }
        }
    }
    o.push_str(rest);
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn around_and_sanitise_as_the_cpp() {
        let g = Guide::around(1000.0, -4.0);
        assert!((g.lo_hz - 1000.0 / 1.26).abs() < 1e-9 && (g.hi_hz - 1260.0).abs() < 1e-9);
        let mut g = Guide { freq_hz: 5.0, lo_hz: 40_000.0, hi_hz: 3.0, gain_db: 99.0, note: format!("  {}  ", "x".repeat(100)), ..Guide::default() };
        g.sanitise();
        assert_eq!((g.lo_hz, g.hi_hz, g.freq_hz, g.gain_db), (FREQ_MIN, FREQ_MAX, FREQ_MIN, 30.0));
        assert_eq!(g.note.len(), 78, "80 characters, then trimmed");
        let mut n = Guide { freq_hz: f64::NAN, lo_hz: f64::INFINITY, hi_hz: f64::NAN, gain_db: f64::NAN, ..Guide::default() };
        n.sanitise();
        assert!(n.freq_hz.is_finite() && n.lo_hz.is_finite() && n.hi_hz.is_finite() && n.gain_db.is_finite());
    }

    #[test]
    fn json_round_trip_and_spellings() {
        let mut gs = Guides::default();
        gs.add(Guide { note: "boxy".into(), dynamic: true, ..Guide::around(300.0, -5.0) });
        gs.add(Guide { done: true, ..Guide::around(3000.0, 2.0) });
        let back = Guides::from_json(&gs.to_json());
        assert_eq!(back.list.len(), 2);
        assert_eq!(back.list[0].note, "boxy");
        assert!(back.list[0].dynamic && back.list[1].done);
        let amorph = json!([{"freq": 500, "label": "mud"}, {"freq": 0}, {"lo": 3}, "x", {"freq": 800, "reason": "honk", "gain": "2.5"}]);
        let g = Guides::from_json(&amorph);
        assert_eq!(g.list.len(), 2);
        assert_eq!((g.list[0].note.as_str(), g.list[0].gain_db), ("mud", -3.0));
        assert_eq!((g.list[1].note.as_str(), g.list[1].gain_db), ("honk", 2.5));
        let many = Value::Array((0..100).map(|i| json!({"freq": 100 + i})).collect());
        assert_eq!(Guides::from_json(&many).list.len(), MAX_GUIDES);
    }

    #[test]
    fn xml_round_trip_with_awkward_notes() {
        let mut gs = Guides::default();
        gs.add(Guide { note: "a \"b\" <c> & 'd'\tend".into(), ..Guide::around(1234.5, -2.25) });
        let mut s = String::new();
        gs.write_xml(&mut s);
        assert!(!s.contains('\t'));
        let back = Guides::read_xml(&s);
        assert_eq!(back, gs);
        let mut empty = String::new();
        Guides::default().write_xml(&mut empty);
        assert!(empty.is_empty());
    }

    #[test]
    fn area_and_hint_are_optional_and_round_trip() {
        // A guide from before they existed writes exactly as it did: no area, no hint.
        let mut old = Guides::default();
        old.add(Guide { note: "mud".into(), ..Guide::around(300.0, -3.0) });
        let mut s = String::new();
        old.write_xml(&mut s);
        assert!(s.ends_with("note=\"mud\"/></guides>") && !s.contains("area") && !s.contains("hint"), "{s}");
        let j = old.to_json();
        assert!(j[0].get("area").is_none() && j[0].get("hint").is_none());
        assert!(Guides::read_xml(&s).list[0].area.is_empty());
        // With them: through the state and a preset.
        let mut gs = Guides::default();
        gs.add(Guide { area: "  Mud / box ".into(), hint: "cut 2\u{2013}3 dB & \"see\"".into(), ..Guide::around(450.0, 0.0) });
        assert_eq!(gs.list[0].area, "Mud / box", "trimmed");
        assert_eq!(gs.list[0].label(), "Mud / box");
        let mut s = String::new();
        gs.write_xml(&mut s);
        assert_eq!(Guides::read_xml(&s), gs);
        let back = Guides::from_json(&gs.to_json());
        assert_eq!((back.list[0].area.as_str(), back.list[0].hint.as_str()), ("Mud / box", "cut 2\u{2013}3 dB & \"see\""));
        // Capped, and a non-string is none.
        let long = Guides::from_json(&json!([{"freq": 100, "area": "a".repeat(500), "hint": 7}]));
        assert_eq!((long.list[0].area.chars().count(), long.list[0].hint.as_str()), (MAX_AREA, ""));
    }
}
