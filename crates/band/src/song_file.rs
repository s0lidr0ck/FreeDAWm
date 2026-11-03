//! The JSON half of `SongFile.cpp` (reading a saved song) and of `SongDoc.cpp` (`songFromVar`,
//! `songFromVarLenient`), plus the five factory songs.
//!
//! A saved song: the song, Key / Scale / Take, optionally its whole band, and copies of the players and
//! targets that band uses (Phase 5a).
//!   { "format": 1, "kind": "song", "app": "MIDIStudio", "name", "tags", "template", "factory",
//!     "song": { <songToVar>, "key", "scale", "take" },
//!     "band": { "name", "params": { m1_..m6_* }, "musicians": [ { "player", "target" } x6 ],
//!               "feel": { ... }, "live": { ... } },            (absent = no band)
//!     "players": [ <persona JSON> ], "targets": [ <target JSON> ] }
//!
//! `juce::var` is read through the small [`var`] helpers below, which keep JUCE's conversions (a string
//! reads as its leading integer, a double truncates, null is void, an absent property gives the default).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

use crate::meter::{Meter, meter_name};
use crate::song::{
    FrozenEvent, FrozenPart, MAX_ARRANGEMENT, MAX_CHORDS, MAX_FROZEN_EVENTS, MAX_SECTIONS, Song, SongChord, SongSection, legal_length,
};
use crate::song_feel::{FEEL_DENSITY, FEEL_DIAL_COUNT, dial_index, dial_title};

/// `juce::var` conversions over `serde_json::Value`. `None` is a property that isn't there.
mod var {
    use serde_json::Value;

    /// `obj.getProperty (name, default)`: None when `v` isn't an object or hasn't the property
    /// (the caller applies the default); a JSON null is `Some(Null)` (JUCE's void).
    pub fn prop<'a>(v: &'a Value, name: &str) -> Option<&'a Value> {
        v.as_object().and_then(|o| o.get(name))
    }
    pub fn is_void(v: Option<&Value>) -> bool {
        matches!(v, None | Some(Value::Null))
    }
    /// `String::getIntValue`: optional whitespace and a '-', then digits ("+5" reads 0, as JUCE) (wrapping like the C++ accumulation).
    fn string_int(s: &str) -> i64 {
        let t = s.trim_start();
        let (neg, digits) = match t.as_bytes().first() {
            Some(b'-') => (true, &t[1..]),
            _ => (false, t),
        };
        let mut n: i64 = 0;
        for b in digits.bytes() {
            if !b.is_ascii_digit() {
                break;
            }
            n = n.wrapping_mul(10).wrapping_add(i64::from(b - b'0'));
        }
        if neg { n.wrapping_neg() } else { n }
    }
    /// `String::getDoubleValue`: the longest leading number (0 when there is none).
    fn string_double(s: &str) -> f64 {
        let t = s.trim_start().as_bytes();
        let digits = |mut i: usize| {
            while t.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            i
        };
        let mut end = usize::from(matches!(t.first(), Some(b'-' | b'+')));
        end = digits(end);
        if t.get(end) == Some(&b'.') {
            end = digits(end + 1);
        }
        if matches!(t.get(end), Some(b'e' | b'E')) {
            let sign = usize::from(matches!(t.get(end + 1), Some(b'-' | b'+')));
            let exp_end = digits(end + 1 + sign);
            if exp_end > end + 1 + sign {
                end = exp_end;
            }
        }
        t.get(..end).and_then(|p| std::str::from_utf8(p).ok()).and_then(|p| p.parse::<f64>().ok()).unwrap_or(0.0)
    }
    /// `(int) var`; `dflt` when the property is absent.
    pub fn int(v: Option<&Value>, dflt: i32) -> i32 {
        match v {
            None => dflt,
            Some(Value::Null) | Some(Value::Array(_)) | Some(Value::Object(_)) => 0,
            Some(Value::Bool(b)) => i32::from(*b),
            Some(Value::Number(n)) => {
                if let Some(i) = n.as_i64() {
                    i as i32
                } else if let Some(u) = n.as_u64() {
                    u as i32
                } else {
                    n.as_f64().map_or(0, |f| f as i32)
                }
            }
            Some(Value::String(s)) => string_int(s) as i32,
        }
    }
    /// `(double) var`; `dflt` when absent.
    pub fn double(v: Option<&Value>, dflt: f64) -> f64 {
        match v {
            None => dflt,
            Some(Value::Null) | Some(Value::Array(_)) | Some(Value::Object(_)) => 0.0,
            Some(Value::Bool(b)) => {
                if *b {
                    1.0
                } else {
                    0.0
                }
            }
            Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
            Some(Value::String(s)) => string_double(s),
        }
    }
    /// `(bool) var`; `dflt` when absent.
    pub fn boolean(v: Option<&Value>, dflt: bool) -> bool {
        match v {
            None => dflt,
            Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
            Some(Value::String(s)) => string_int(s) != 0 || s.trim().eq_ignore_ascii_case("true") || s.trim().eq_ignore_ascii_case("yes"),
            Some(Value::Array(_)) | Some(Value::Object(_)) => true,
        }
    }
    /// `var::toString`; `dflt` when absent.
    pub fn string(v: Option<&Value>, dflt: &str) -> String {
        match v {
            None => dflt.to_string(),
            Some(Value::Null) => String::new(),
            Some(Value::Bool(b)) => if *b { "1" } else { "0" }.to_string(),
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(_)) => "[Array]".to_string(),
            Some(Value::Object(_)) => "Object".to_string(),
        }
    }
    /// `isInt() || isInt64() || isDouble()`.
    pub fn is_number(v: Option<&Value>) -> bool {
        matches!(v, Some(Value::Number(_)))
    }
    /// SongFile.cpp's isInt: an integer, or a double with no fraction.
    pub fn is_int(v: Option<&Value>) -> bool {
        match v {
            Some(Value::Number(n)) => n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|f| f == f.floor()),
            _ => false,
        }
    }
    pub fn array(v: Option<&Value>) -> Option<&Vec<Value>> {
        v.and_then(Value::as_array)
    }
    /// The single character of a one-character string, as `id.length() == 1` then `id[0]`.
    pub fn one_char(s: &str) -> Option<char> {
        let mut it = s.chars();
        match (it.next(), it.next()) {
            (Some(c), None) => Some(c),
            _ => None,
        }
    }
}

// ---- SongDoc.cpp: the song as JSON -----------------------------------------------------------------------------

/// "meter": absent / void -> -1, "4/4" | "6/8" | "3/4" -> the Meter, anything else -> -2.
fn meter_index(v: Option<&Value>) -> i32 {
    if var::is_void(v) {
        return -1;
    }
    let n = var::string(v, "");
    if n == meter_name(Meter::M44) {
        return Meter::M44 as i32;
    }
    if n == meter_name(Meter::M68) {
        return Meter::M68 as i32;
    }
    if n == meter_name(Meter::M34) {
        return Meter::M34 as i32;
    }
    -2
}

/// Reads one dial: absent -> 'absent', a known name -> its position, else -2 (strict: an error; lenient: absent).
fn read_dial(o: &Value, d: i32, absent: i32) -> i32 {
    let v = var::prop(o, &dial_title(d).to_lowercase());
    if var::is_void(v) {
        return absent;
    }
    let i = dial_index(d, &var::string(v, ""));
    if i >= 0 { i } else { -2 }
}

/// One kept take; an error when anything is off. Returns the musician and the part.
#[allow(clippy::neg_cmp_op_on_partial_ord)] // `! (len > 0.0)`: a NaN length is refused
fn frozen_part_from(o: &Value) -> Result<(i32, Arc<FrozenPart>), String> {
    let m = var::prop(o, "m");
    if !var::is_number(m) || var::int(m, 0) < 0 || var::int(m, 0) > 5 {
        return Err("a kept take for a musician that doesn't exist".into());
    }
    let musician = var::int(m, 0);
    let mut part = FrozenPart { length_ppq: var::double(var::prop(o, "len"), 0.0), ..FrozenPart::default() };
    if !(part.length_ppq > 0.0) || part.length_ppq > 4096.0 {
        return Err("a kept take with no length".into());
    }
    let pm = meter_index(var::prop(o, "meter"));
    if pm == -2 {
        return Err("a kept take in a time signature MIDI Studio doesn't play".into());
    }
    part.meter = pm.max(0) as u8;
    let Some(ev) = var::array(var::prop(o, "ev")) else { return Err("a kept take with no notes".into()) };
    if ev.len() > MAX_FROZEN_EVENTS as usize {
        return Err("a kept take with more than 4,000 events".into());
    }
    part.events.reserve(ev.len());
    for t in ev {
        let a = match t.as_array() {
            Some(a) if a.len() == 5 => a,
            _ => return Err("a damaged kept take".into()),
        };
        let ppq = var::double(a.first(), 0.0);
        let st = var::int(a.get(1), 0);
        let d1 = var::int(a.get(2), 0);
        let d2 = var::int(a.get(3), 0);
        if !(0x80..=0xEF).contains(&st)
            || (st & 0x0F) > 2
            || !(0..=127).contains(&d1)
            || !(0..=127).contains(&d2)
            || ppq < -1.0
            || ppq > part.length_ppq + 16.0
        {
            return Err("a damaged kept take".into());
        }
        part.events.push(FrozenEvent { ppq, status: st as u8, d1: d1 as u8, d2: d2 as u8, keyswitch: var::int(a.get(4), 0) != 0 });
    }
    Ok((musician, Arc::new(part)))
}

/// Refuses (with a message) anything outside the limits: 1..32 chords, length 1/2/4, bass -1..11,
/// slot 0..111, 1..8 sections with unique ids A..H, an arrangement of known ids (up to 64).
pub fn song_from_var(j: &Value) -> Result<Song, String> {
    let mut s = Song::default();
    let secs = match var::array(var::prop(j, "sections")) {
        Some(a) if !a.is_empty() => a,
        _ => return Err("the song has no sections".into()),
    };
    if secs.len() > MAX_SECTIONS as usize {
        return Err("more than 8 sections".into());
    }
    for sv in secs {
        let mut sec = SongSection::default();
        let id = var::string(var::prop(sv, "id"), "");
        let Some(c) = var::one_char(&id).filter(|c| ('A'..='H').contains(c)) else {
            return Err(format!("section id \"{id}\" must be A..H"));
        };
        sec.id = c;
        if s.find(sec.id).is_some() {
            return Err(format!("two sections are called {id}"));
        }
        sec.name = var::string(var::prop(sv, "name"), "");
        sec.energy_shift = var::int(var::prop(sv, "energyShift"), 0);
        if sec.energy_shift < -3 || sec.energy_shift > 3 {
            return Err(format!("section {id}: energy shift must be -3..+3"));
        }
        sec.follow_idea = var::boolean(var::prop(sv, "followIdea"), false);
        sec.take = var::int(var::prop(sv, "take"), 0);
        if sec.take < 0 || sec.take > 16 {
            return Err(format!("section {id}: take must be 0..16"));
        }
        let role = var::string(var::prop(sv, "role"), "");
        let shape = var::string(var::prop(sv, "shape"), "");
        if !role.is_empty() && role != "intro" && role != "ending" {
            return Err(format!("section {id}: role must be intro or ending"));
        }
        if !shape.is_empty() && shape != "build" && shape != "drop" {
            return Err(format!("section {id}: shape must be build or drop"));
        }
        sec.role = if role == "intro" {
            1
        } else if role == "ending" {
            2
        } else {
            0
        };
        sec.shape = if shape == "build" {
            1
        } else if shape == "drop" {
            2
        } else {
            0
        };
        if sec.role == 2 && s.sections.iter().any(|o| o.role == 2) {
            return Err("only one section can be the Ending".into());
        }
        if let Some(fv) = var::prop(sv, "feel").filter(|f| f.is_object()) {
            for d in FEEL_DENSITY..FEEL_DIAL_COUNT {
                let v = read_dial(fv, d, -1);
                if v == -2 {
                    return Err(format!("section {id}: unknown feel {}", dial_title(d).to_lowercase()));
                }
                sec.feel_override.set_dial(d, v as i8);
            }
        }
        if let Some(ms) = var::array(var::prop(sv, "musicians")) {
            for (k, mv) in ms.iter().take(6).enumerate() {
                sec.musicians[k].plays = var::boolean(var::prop(mv, "plays"), true);
                let e = var::prop(mv, "energy");
                let ev = if var::is_void(e) { 0 } else { var::int(e, 0) };
                if !(0..=4).contains(&ev) {
                    return Err(format!("section {id}: musician energy must be 1..4 or follow"));
                }
                sec.musicians[k].energy = ev;
            }
        }
        let cs = match var::array(var::prop(sv, "chords")) {
            Some(a) if !a.is_empty() => a,
            _ => return Err(format!("section {id} has no chords")),
        };
        if cs.len() > MAX_CHORDS as usize {
            return Err(format!("section {id} has more than 32 chords"));
        }
        for cv in cs {
            let c = SongChord {
                slot: var::int(var::prop(cv, "slot"), 0),
                bass: var::int(var::prop(cv, "bass"), -1),
                length: var::int(var::prop(cv, "length"), 2),
            };
            if c.slot < 0 || c.slot > 111 {
                return Err(format!("section {id}: chord value out of range"));
            }
            if c.bass < -1 || c.bass > 11 {
                return Err(format!("section {id}: bass note must be root or 0..11"));
            }
            if c.length != 1 && c.length != 2 && c.length != 4 {
                return Err(format!("section {id}: chord length must be half, 1 or 2 bars"));
            }
            sec.chords.push(c);
        }
        if let Some(fr) = var::array(var::prop(sv, "frozen")) {
            for fv in fr {
                let (k, part) = frozen_part_from(fv).map_err(|e| format!("section {id}: {e}"))?;
                let Some(slot) = sec.frozen.get_mut(k as usize) else { continue };
                if slot.is_some() {
                    return Err(format!("section {id}: two kept takes for one musician"));
                }
                *slot = Some(part);
            }
        }
        s.sections.push(sec);
    }
    if let Some(arr) = var::array(var::prop(j, "arrangement")) {
        if arr.len() > MAX_ARRANGEMENT as usize {
            return Err("the arrangement is longer than 64".into());
        }
        for a in arr {
            let id = var::string(Some(a), "");
            let Some(c) = var::one_char(&id).filter(|c| s.find(*c).is_some()) else {
                return Err(format!("the arrangement names a section \"{id}\" that doesn't exist"));
            };
            s.arrangement.push(c);
        }
    }
    if let Some(en) = var::array(var::prop(j, "arrangementEnergy")) {
        if en.len() != s.arrangement.len() {
            return Err("arrangementEnergy must have one value per arrangement entry".into());
        }
        for v in en {
            let e = var::int(Some(v), 0);
            if !(-3..=3).contains(&e) {
                return Err("arrangement energy must be -3..+3".into());
            }
            s.arrangement_energy.push(e);
        }
    }
    {
        let m = meter_index(var::prop(j, "meter"));
        if m == -2 {
            return Err("the song's time signature must be 4/4, 6/8 or 3/4".into());
        }
        s.meter_override = m;
    }
    if let Some(fv) = var::prop(j, "feel").filter(|f| f.is_object()) {
        s.feel.on = var::boolean(var::prop(fv, "on"), true);
        s.feel.preset = var::string(var::prop(fv, "preset"), "");
        s.feel.edited = var::boolean(var::prop(fv, "edited"), false);
        for d in 0..FEEL_DIAL_COUNT {
            let v = read_dial(fv, d, 0);
            if v == -2 {
                return Err(format!("unknown song feel {}", dial_title(d).to_lowercase()));
            }
            s.feel.set_dial(d, v as i8);
        }
        if let Some(ig) = var::array(var::prop(fv, "ignore")) {
            for v in ig {
                let k = if var::is_number(Some(v)) { var::int(Some(v), 0) } else { -1 };
                if !(0..=5).contains(&k) {
                    return Err("song feel: ignore names a musician 0..5".into());
                }
                s.feel_ignore |= 1u8 << k;
            }
        }
    }
    if let Some(tk) = var::array(var::prop(j, "arrangementTake")) {
        if tk.len() != s.arrangement.len() {
            return Err("arrangementTake must have one value per arrangement entry".into());
        }
        for v in tk {
            let t = var::int(Some(v), 0);
            if !(0..=16).contains(&t) {
                return Err("an arrangement Take must be 0..16".into());
            }
            s.arrangement_take.push(t);
        }
    }
    Ok(s)
}

/// For loading a saved session: never refuses a song that has at least one usable section; values out
/// of limits are clamped, extra chords / sections / arrangement entries dropped, unknown ids skipped.
pub fn song_from_var_lenient(j: &Value) -> Option<Song> {
    let mut s = Song::default();
    let secs = var::array(var::prop(j, "sections"))?;
    for sv in secs {
        if s.sections.len() as i32 >= MAX_SECTIONS {
            break;
        }
        let id = var::string(var::prop(sv, "id"), "");
        let Some(c) = var::one_char(&id).filter(|c| ('A'..='H').contains(c) && s.find(*c).is_none()) else { continue };
        let mut sec = SongSection {
            id: c,
            name: var::string(var::prop(sv, "name"), ""),
            energy_shift: var::int(var::prop(sv, "energyShift"), 0).clamp(-3, 3),
            follow_idea: var::boolean(var::prop(sv, "followIdea"), false),
            take: var::int(var::prop(sv, "take"), 0).clamp(0, 16),
            ..SongSection::default()
        };
        let role = var::string(var::prop(sv, "role"), "");
        let shape = var::string(var::prop(sv, "shape"), "");
        sec.role = if role == "intro" {
            1
        } else if role == "ending" {
            2
        } else {
            0
        };
        if sec.role == 2 && s.sections.iter().any(|o| o.role == 2) {
            sec.role = 0; // only one Ending: the first wins
        }
        sec.shape = if shape == "build" {
            1
        } else if shape == "drop" {
            2
        } else {
            0
        };
        if let Some(fv) = var::prop(sv, "feel").filter(|f| f.is_object()) {
            for d in FEEL_DENSITY..FEEL_DIAL_COUNT {
                sec.feel_override.set_dial(d, read_dial(fv, d, -1).max(-1) as i8); // unknown -> the song's
            }
        }
        if let Some(ms) = var::array(var::prop(sv, "musicians")) {
            for (k, mv) in ms.iter().take(6).enumerate() {
                sec.musicians[k].plays = var::boolean(var::prop(mv, "plays"), true);
                let e = var::prop(mv, "energy");
                sec.musicians[k].energy = if var::is_void(e) { 0 } else { var::int(e, 0).clamp(0, 4) };
            }
        }
        if let Some(cs) = var::array(var::prop(sv, "chords")) {
            for cv in cs {
                if sec.chords.len() as i32 >= MAX_CHORDS {
                    break;
                }
                let b = var::int(var::prop(cv, "bass"), -1);
                sec.chords.push(SongChord {
                    slot: var::int(var::prop(cv, "slot"), 0).clamp(0, 111),
                    bass: if b < 0 { -1 } else { b.clamp(0, 11) },
                    length: legal_length(var::int(var::prop(cv, "length"), 2)),
                });
            }
        }
        if sec.chords.is_empty() {
            sec.chords.push(SongChord::default());
        }
        if let Some(fr) = var::array(var::prop(sv, "frozen")) {
            for fv in fr {
                if let Ok((k, part)) = frozen_part_from(fv)
                    && let Some(slot) = sec.frozen.get_mut(k as usize)
                    && slot.is_none()
                {
                    *slot = Some(part);
                }
            }
        }
        s.sections.push(sec);
    }
    if s.sections.is_empty() {
        return None;
    }
    s.meter_override = meter_index(var::prop(j, "meter")).max(-1); // unknown -> follow the host
    if let Some(fv) = var::prop(j, "feel").filter(|f| f.is_object()) {
        // unknown dial -> neutral
        s.feel.on = var::boolean(var::prop(fv, "on"), true);
        s.feel.preset = var::string(var::prop(fv, "preset"), "");
        s.feel.edited = var::boolean(var::prop(fv, "edited"), false);
        for d in 0..FEEL_DIAL_COUNT {
            s.feel.set_dial(d, read_dial(fv, d, 0).max(0) as i8);
        }
        if let Some(ig) = var::array(var::prop(fv, "ignore")) {
            for v in ig {
                let k = var::int(Some(v), 0);
                if (0..=5).contains(&k) {
                    s.feel_ignore |= 1u8 << k;
                }
            }
        }
    }
    let en = var::array(var::prop(j, "arrangementEnergy"));
    let tk = var::array(var::prop(j, "arrangementTake"));
    if let Some(arr) = var::array(var::prop(j, "arrangement")) {
        for (k, a) in arr.iter().enumerate() {
            if s.arrangement.len() as i32 >= MAX_ARRANGEMENT {
                break;
            }
            let id = var::string(Some(a), "");
            // a skipped entry takes its energy with it
            let Some(c) = var::one_char(&id).filter(|c| s.find(*c).is_some()) else { continue };
            s.arrangement.push(c);
            s.arrangement_energy.push(en.and_then(|e| e.get(k)).map_or(0, |v| var::int(Some(v), 0).clamp(-3, 3)));
            s.arrangement_take.push(tk.and_then(|t| t.get(k)).map_or(0, |v| var::int(Some(v), 0).clamp(0, 16)));
        }
    }
    s.arrangement_energy.resize(s.arrangement.len(), 0);
    if s.arrangement_energy.iter().all(|e| *e == 0) {
        s.arrangement_energy.clear();
    }
    s.arrangement_take.resize(s.arrangement.len(), 0);
    if s.arrangement_take.iter().all(|t| *t == 0) {
        s.arrangement_take.clear();
    }
    Some(s)
}

// ---- SongFile.cpp: the song file -------------------------------------------------------------------------------

const FEEL_NAMES: [&str; 5] = ["modWheelEnergy", "bassLock", "fillsYield", "leaveSpace", "dynamicsTogether"];
const FORMAT: i32 = 1;

/// The file's top-level `"feel"`: Natural or Classic. Absent (None) reads as Classic, as files saved before Natural.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeelFlagsFile {
    Natural,
    Classic,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SongFileBand {
    /// the band label in the band bar
    pub name: String,
    /// every m1..m6_* host parameter
    pub params: BTreeMap<String, f32>,
    /// player, target names
    pub musicians: [(String, String); 6],
    /// modWheelEnergy, bassLock, fillsYield, leaveSpace, dynamicsTogether
    pub feel: [bool; 5],
    pub live: bool,
    pub live_on_beat: bool,
    pub fill_every: i32,
    pub perf_key_base: i32,
}

impl Default for SongFileBand {
    fn default() -> Self {
        Self {
            name: String::new(),
            params: BTreeMap::new(),
            musicians: Default::default(),
            feel: [false; 5],
            live: false,
            live_on_beat: false,
            fill_every: 4,
            perf_key_base: 24,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SongFileData {
    pub name: String,
    pub tags: Vec<String>,
    pub is_template: bool,
    pub factory: bool,
    pub song: Song,
    pub key: i32,
    pub scale: i32,
    pub take: i32,
    pub band: Option<SongFileBand>,
    /// Natural feel: "natural" / "classic" / absent (saved before Natural existed -> Classic).
    pub feel: Option<FeelFlagsFile>,
    /// Tension and release: true / false / absent (reads off).
    pub tension: Option<bool>,
    /// canonical JSON text of each copy
    pub players: Vec<String>,
    pub targets: Vec<String>,
}

impl Default for SongFileData {
    fn default() -> Self {
        Self {
            name: String::new(),
            tags: Vec::new(),
            is_template: false,
            factory: false,
            song: Song::default(),
            key: 0,
            scale: 0,
            take: 1,
            band: None,
            feel: None,
            tension: None,
            players: Vec::new(),
            targets: Vec::new(),
        }
    }
}

impl SongFileData {
    /// MS's `natural`: the file said "natural".
    pub fn natural(&self) -> bool {
        self.feel == Some(FeelFlagsFile::Natural)
    }
    /// MS's `tension`: the file said true.
    pub fn tension_on(&self) -> bool {
        self.tension == Some(true)
    }
}

/// A band setting's range by host id (`Parameters.h` `findHostParam`): `Some((min, max))` for a known one.
pub type HostParamRange = fn(&str) -> Option<(f32, f32)>;

fn is_band_param(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() > 3 && b[0] == b'm' && (b'1'..=b'6').contains(&b[1]) && b[2] == b'_'
}

/// Parse once and print compactly, so the same JSON always gives the same text ("" when it isn't JSON).
pub fn canonical_json(text: &str) -> String {
    serde_json::from_str::<Value>(text).map(|v| v.to_string()).unwrap_or_default()
}

/// Reads the band; strict fails on anything off, lenient keeps defaults for what's off and notes it.
fn read_band(j: &Value, strict: bool, notes: &mut Vec<String>, host: Option<HostParamRange>) -> Result<SongFileBand, String> {
    if !j.is_object() {
        return Err("the band isn't an object".into());
    }
    let mut b = SongFileBand { name: var::string(var::prop(j, "name"), ""), ..SongFileBand::default() };
    let Some(params) = var::prop(j, "params").and_then(Value::as_object) else { return Err("the band has no params".into()) };
    for (name, value) in params {
        // Without the host table (Parameters.h, ported with the Band) every m1..m6_* id is known and unbounded.
        let hp = match host {
            Some(f) => f(name),
            None => Some((f32::NEG_INFINITY, f32::INFINITY)),
        };
        let numeric = matches!(value, Value::Number(_) | Value::Bool(_));
        let Some((lo, hi)) = hp.filter(|_| is_band_param(name) && numeric) else {
            if strict {
                return Err(format!("unknown band setting \"{name}\""));
            }
            notes.push(format!("band setting \"{name}\""));
            continue;
        };
        let v = var::double(Some(value), 0.0) as f32;
        if v < lo || v > hi {
            if strict {
                return Err(format!("band setting \"{name}\" out of range"));
            }
            notes.push(format!("band setting \"{name}\" (out of range, clamped)"));
        }
        b.params.insert(name.clone(), v.max(lo).min(hi));
    }
    let ms = var::array(var::prop(j, "musicians"));
    match ms {
        None => return Err("the band must have six musicians".into()),
        Some(m) if m.len() != 6 => {
            if strict {
                return Err("the band must have six musicians".into());
            }
            notes.push("musicians beyond six".into());
        }
        Some(_) => {}
    }
    for (k, m) in ms.into_iter().flatten().take(6).enumerate() {
        b.musicians[k] = (var::string(var::prop(m, "player"), ""), var::string(var::prop(m, "target"), ""));
    }
    let feel = var::prop(j, "feel");
    for (i, n) in FEEL_NAMES.iter().enumerate() {
        b.feel[i] = feel.is_some_and(|f| var::boolean(var::prop(f, n), false));
    }
    let live = var::prop(j, "live");
    let lp = |n: &str| live.and_then(|l| var::prop(l, n));
    b.live = var::boolean(lp("live"), false);
    b.live_on_beat = var::boolean(lp("liveOnBeat"), false);
    b.fill_every = if var::int(lp("fillEvery"), 4) == 8 { 8 } else { 4 };
    b.perf_key_base = var::int(lp("perfKeyBase"), 24).clamp(-1, 115);
    Ok(b)
}

fn read_file(text: &str, strict: bool, notes: &mut Vec<String>, host: Option<HostParamRange>) -> Result<SongFileData, String> {
    let j: Value = match serde_json::from_str(text) {
        Ok(v @ Value::Object(_)) => v,
        _ => return Err("This isn't a song file (not valid JSON).".into()),
    };
    if var::string(var::prop(&j, "kind"), "") != "song" {
        return Err("This isn't a MIDI Studio song file.".into());
    }
    let format = var::prop(&j, "format");
    if !var::is_int(format) || var::int(format, 0) < 1 {
        return Err("This song file has no format number.".into());
    }
    if var::int(format, 0) > FORMAT {
        return Err("This song was saved by a newer MIDI Studio.".into());
    }

    let mut d = SongFileData { name: var::string(var::prop(&j, "name"), "").trim().to_string(), ..SongFileData::default() };
    if let Some(t) = var::array(var::prop(&j, "tags")) {
        for s in t {
            let tag = var::string(Some(s), "").trim().to_lowercase();
            if !tag.is_empty() && !d.tags.contains(&tag) {
                d.tags.push(tag);
            }
        }
    }
    d.is_template = var::boolean(var::prop(&j, "template"), false);
    d.factory = var::boolean(var::prop(&j, "factory"), false);

    let Some(song) = var::prop(&j, "song").filter(|s| s.is_object()) else { return Err("This song file has no song.".into()) };
    if strict {
        d.song = song_from_var(song).map_err(|e| format!("The song is damaged: {e}"))?;
    } else {
        let Some(s) = song_from_var_lenient(song) else { return Err("The song in this file can't be read.".into()) };
        if let Err(e) = song_from_var(song) {
            notes.push(format!("parts of the song ({e})"));
        }
        d.song = s;
    }
    let mut field = |name: &str, lo: i32, hi: i32, dflt: i32| -> Result<i32, String> {
        let v = var::prop(song, name);
        let present = v.is_some();
        let ok = if present { var::is_int(v) } else { true };
        let n = if present { var::int(v, dflt) } else { dflt };
        if !ok || n < lo || n > hi {
            if strict {
                return Err(format!("The song's {name} is out of range."));
            }
            notes.push(format!("the song's {name}"));
            return Ok(dflt);
        }
        Ok(n)
    };
    d.key = field("key", 0, 11, 0)?;
    d.scale = field("scale", 0, 1, 0)?;
    d.take = field("take", 1, 16, 1)?;

    // Feel: "natural", "classic" or absent (saved before Natural existed -> Classic); Tension: true / false or absent.
    {
        let fv = var::prop(&j, "feel");
        let f = var::string(fv, "");
        if !var::is_void(fv) {
            d.feel = match fv {
                Some(Value::String(s)) if s == "natural" => Some(FeelFlagsFile::Natural),
                Some(Value::String(s)) if s == "classic" => Some(FeelFlagsFile::Classic),
                _ => {
                    if strict {
                        return Err(format!("The song's feel \"{f}\" isn't natural or classic."));
                    }
                    notes.push(format!("the song's feel (\"{f}\" - opened as Classic)"));
                    None
                }
            };
        }
        let tv = var::prop(&j, "tension");
        if !var::is_void(tv) {
            d.tension = match tv {
                Some(Value::Bool(b)) => Some(*b),
                _ => {
                    if strict {
                        return Err("The song's tension setting must be true or false.".into());
                    }
                    notes.push("the song's tension setting (opened off)".into());
                    None
                }
            };
        }
    }
    let band = var::prop(&j, "band");
    if !var::is_void(band) {
        match band.map(|b| read_band(b, strict, notes, host)) {
            Some(Ok(b)) => d.band = Some(b),
            Some(Err(e)) => {
                if strict {
                    return Err(format!("The band is damaged: {e}"));
                }
                notes.push(format!("the band ({e})"));
            }
            None => {}
        }
    }
    let copies = |name: &str, out: &mut Vec<String>, notes: &mut Vec<String>| -> Result<(), String> {
        let v = var::prop(&j, name);
        if var::is_void(v) {
            return Ok(());
        }
        let Some(a) = var::array(v) else {
            if strict {
                return Err(format!("The {name} list is damaged."));
            }
            notes.push(format!("the {name}"));
            return Ok(());
        };
        for x in a {
            if !x.is_object() || var::string(var::prop(x, "name"), "").is_empty() {
                if strict {
                    return Err(format!("A copy in {name} is damaged."));
                }
                notes.push(format!("a copy in {name}"));
                continue;
            }
            out.push(x.to_string());
        }
        Ok(())
    };
    let mut players = Vec::new();
    let mut targets = Vec::new();
    copies("players", &mut players, notes)?;
    copies("targets", &mut targets, notes)?;
    d.players = players;
    d.targets = targets;
    Ok(d)
}

/// Strict: anything wrong is an error; a newer format is refused. Band settings are checked for their `m1_..m6_`
/// form only (see [`song_file_from_json_checked`]).
pub fn song_file_from_json(text: &str) -> Result<SongFileData, String> {
    let mut notes = Vec::new();
    read_file(text, true, &mut notes, None)
}

/// Strict, with band settings checked against the host parameter table as MS does.
pub fn song_file_from_json_checked(text: &str, host: HostParamRange) -> Result<SongFileData, String> {
    let mut notes = Vec::new();
    read_file(text, true, &mut notes, Some(host))
}

/// Lenient: keeps whatever is usable and lists what was skipped. Still refuses a newer format or no song.
pub fn song_file_from_json_lenient(text: &str, host: Option<HostParamRange>) -> Result<(SongFileData, Vec<String>), String> {
    let mut skipped = Vec::new();
    let d = read_file(text, false, &mut skipped, host)?;
    Ok((d, skipped))
}

/// The five factory songs (`Songs/Factory/*.song.json`, embedded), in file-name order. Each reads strictly, else
/// leniently; one that reads neither way is left out.
pub fn factory_songs() -> Vec<SongFileData> {
    let mut out = Vec::new();
    for (path, bytes) in crate::library::embedded::files() {
        if !(path.starts_with("Songs/Factory/") && path.ends_with(".song.json")) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(bytes) else { continue };
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        match song_file_from_json(text) {
            Ok(d) => out.push(d),
            Err(_) => {
                if let Ok((d, _)) = song_file_from_json_lenient(text, None) {
                    out.push(d);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(song: &str) -> String {
        format!(r#"{{"kind":"song","format":1,"song":{song}}}"#)
    }

    #[test]
    fn juce_conversions() {
        assert_eq!(var::int(Some(&Value::String(" -12abc".into())), 0), -12);
        assert_eq!(var::int(Some(&serde_json::json!(2.9)), 0), 2);
        assert_eq!(var::int(Some(&Value::String("+5".into())), 0), 0);
        assert_eq!(var::int(Some(&Value::Null), 7), 0);
        assert_eq!(var::int(None, 7), 7);
        assert!(var::boolean(Some(&Value::String(" Yes ".into())), false));
        assert_eq!(var::double(Some(&Value::String("1.5e1x".into())), 0.0), 15.0);
        assert_eq!(var::string(Some(&Value::Bool(true)), ""), "1");
    }

    #[test]
    fn strict_and_lenient_songs() {
        let text = wrap(
            r#"{"sections":[{"id":"A","chords":[{"slot":0,"length":2}],"feel":{"groove":"swing"},
                 "frozen":[{"m":2,"len":8,"ev":[[0.5,144,60,100,0],[1,128,60,0,0]]}]},
               {"id":"B","role":"ending","chords":[{"slot":48,"bass":0,"length":4}]}],
               "arrangement":["A","B"],"arrangementEnergy":[1,-1],"meter":"6/8",
               "feel":{"preset":"Pop","density":"busy","ignore":[2,5]}}"#,
        );
        let d = song_file_from_json(&text).unwrap_or_else(|e| panic!("{e}"));
        let s = &d.song;
        assert_eq!(s.sections[0].feel_override.groove, 1);
        assert_eq!(s.sections[0].frozen[2].as_ref().map(|f| f.events.len()), Some(2));
        assert_eq!(s.sections[1].role, 2);
        assert_eq!((s.meter_override, s.feel.on, s.feel.density, s.feel_ignore), (1, true, 3, 0b100100));
        assert_eq!(s.arrangement_energy, [1, -1]);

        let bad = wrap(r#"{"sections":[{"id":"A","chords":[{"slot":200}]},{"id":"Z","chords":[]}],"arrangement":["A","Q"],"arrangementEnergy":[9]}"#);
        assert_eq!(song_file_from_json(&bad).err().as_deref(), Some("The song is damaged: section A: chord value out of range"));
        let (d, notes) = song_file_from_json_lenient(&bad, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(d.song.sections.len(), 1);
        assert_eq!(d.song.sections[0].chords[0].slot, 111);
        assert_eq!(d.song.arrangement, ['A']);
        assert_eq!(d.song.arrangement_energy, [3]);
        assert_eq!(notes, ["parts of the song (section A: chord value out of range)"]);
    }

    #[test]
    fn file_fields_and_band() {
        let text = r#"{"kind":"song","format":1,"name":" X ","tags":["Rock"," rock","pop"],
            "song":{"sections":[{"id":"A","chords":[{"slot":0}]}],"key":3.0,"scale":1,"take":16},
            "feel":"classic","tension":false,
            "band":{"name":"Trio","params":{"m1_energy":3,"m2_x":true},"musicians":[{"player":"Clay","target":"T"},{},{},{},{},{}],
                    "feel":{"bassLock":true},"live":{"fillEvery":8,"perfKeyBase":200}},
            "players":[{"name":"Clay","seed":3}],"targets":[]}"#;
        let d = song_file_from_json(text).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((d.name.as_str(), d.tags.clone()), ("X", vec!["rock".to_string(), "pop".to_string()]));
        assert_eq!((d.key, d.scale, d.take), (3, 1, 16));
        assert_eq!((d.feel, d.tension, d.natural(), d.tension_on()), (Some(FeelFlagsFile::Classic), Some(false), false, false));
        let b = d.band.unwrap_or_default();
        assert_eq!((b.name.as_str(), b.params.len(), b.feel[1], b.fill_every, b.perf_key_base), ("Trio", 2, true, 8, 115));
        assert_eq!(b.musicians[0], ("Clay".to_string(), "T".to_string()));
        assert_eq!(d.players.len(), 1);

        fn only_energy(id: &str) -> Option<(f32, f32)> {
            (id == "m1_energy").then_some((1.0, 4.0))
        }
        assert_eq!(song_file_from_json_checked(text, only_energy).err().as_deref(), Some("The band is damaged: unknown band setting \"m2_x\""));
        let (d, notes) = song_file_from_json_lenient(text, Some(only_energy)).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(d.band.map(|b| b.params.len()), Some(1));
        assert_eq!(notes, ["band setting \"m2_x\""]);

        let out = r#"{"kind":"song","format":1,"song":{"sections":[{"id":"A","chords":[{"slot":0}]}],"key":12},"feel":"x"}"#;
        assert_eq!(song_file_from_json(out).err().as_deref(), Some("The song's key is out of range."));
        let (d, notes) = song_file_from_json_lenient(out, None).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((d.key, d.feel), (0, None));
        assert_eq!(notes, ["the song's key", "the song's feel (\"x\" - opened as Classic)"]);

        let nulls = r#"{"kind":"song","format":1,"song":{"sections":[{"id":"A","chords":[{"slot":0}]}]},"feel":null,"tension":null,"band":null}"#;
        let d = song_file_from_json(nulls).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((d.feel, d.tension, d.band), (None, None, None));
    }
}
