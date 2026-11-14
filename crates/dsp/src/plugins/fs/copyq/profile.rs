//! A CopyQ profile: what a sound's tone was measured as, and the module state built from profiles.
//!
//! A profile is fifteen curves on one grid ([`GRID_N`] points a twelfth of an octave apart from
//! 20 Hz): five views of the sound ([`SUM`], [`LEFT`], [`RIGHT`], [`MID`], [`SIDE`]) at three
//! levels ([`ALL`] of it, its [`LOUD`] passages and its [`QUIET`] ones). A value is the power in that
//! twelfth of an octave, in dB (a full-scale square wave's whole power is 0 dB), so pink noise is
//! a flat line. Every curve of a profile shares one reference: the side really is that far under
//! the mid, the quiet passages that far under the loud ones.
//!
//! As text a profile is JSON (`"format": "copyq-profile"`) with the curves packed: little-endian
//! `i16` hundredths of a dB, base64. The module's state (`Insert::state`, [`CopyQState`]) is JSON
//! too: up to [`MAX_TARGETS`] profiles with a weight each (blended into the one target the engine
//! matches to), and the module's own frozen source profile, if it has one. All of it is hostile
//! input: nothing here panics on any bytes, sizes are capped, non-finite numbers are dropped.

use serde_json::{Value, json};

/// Points on the grid: ten octaves at twelve to the octave, both ends included.
pub const GRID_N: usize = 121;
/// The first grid point, Hz.
pub const GRID_LO_HZ: f32 = 20.0;
/// Grid points per octave.
pub const PER_OCTAVE: usize = 12;

/// The views of the sound (a curve's channel).
pub const SUM: usize = 0;
pub const LEFT: usize = 1;
pub const RIGHT: usize = 2;
pub const MID: usize = 3;
pub const SIDE: usize = 4;
pub const CH: usize = 5;
/// The levels of the sound.
pub const ALL: usize = 0;
pub const LOUD: usize = 1;
pub const QUIET: usize = 2;
pub const LV: usize = 3;
/// Curves in a profile: `level * CH + channel`.
pub const CURVES: usize = CH * LV;

/// "Nothing there", dB.
pub const FLOOR_DB: f32 = -160.0;
/// A level (loud, quiet) measured for less than this, in seconds, is not used.
pub const MIN_LEVEL_SECONDS: f32 = 0.5;
/// The largest profile or state text read.
pub const MAX_TEXT_BYTES: usize = 512 * 1024;
/// Most profiles a state blends.
pub const MAX_TARGETS: usize = 4;
/// The longest profile name kept, in characters.
pub const MAX_NAME_CHARS: usize = 80;

const FORMAT: &str = "copyq-profile";
const STATE_FORMAT: &str = "copyq-state";

pub type Curve = [f32; GRID_N];

/// The frequency of grid point `i`, Hz.
pub fn grid_hz(i: usize) -> f32 {
    GRID_LO_HZ * 2f32.powf(i as f32 / PER_OCTAVE as f32)
}

/// Where `hz` falls on the grid, in points (0 at 20 Hz; may be outside `0..GRID_N`).
pub fn grid_pos(hz: f32) -> f32 {
    if hz.is_finite() && hz > 0.0 { (hz / GRID_LO_HZ).log2() * PER_OCTAVE as f32 } else { 0.0 }
}

/// The index of a curve.
pub const fn curve_index(level: usize, channel: usize) -> usize {
    level * CH + channel
}

/// One measured sound. See the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    /// The curves, dB, by [`curve_index`].
    pub curves: Box<[Curve; CURVES]>,
    /// The mean level of the frames each level was measured from, dB.
    pub level: [f32; LV],
    /// How much sound each level was measured from, seconds.
    pub seconds: [f32; LV],
    /// Measured from two channels (a mono profile's side is the floor and its other views are
    /// all the same curve).
    pub stereo: bool,
}

impl Default for Profile {
    fn default() -> Self {
        Profile::silent()
    }
}

impl Profile {
    /// A profile of nothing.
    pub fn silent() -> Self {
        Profile { name: String::new(), curves: Box::new([[FLOOR_DB; GRID_N]; CURVES]), level: [FLOOR_DB; LV], seconds: [0.0; LV], stereo: false }
    }

    pub fn curve(&self, level: usize, channel: usize) -> &Curve {
        static NOTHING: Curve = [FLOOR_DB; GRID_N];
        self.curves.get(curve_index(level, channel)).unwrap_or(&NOTHING)
    }

    /// Whether `level` was measured from enough sound to be used ([`ALL`]: from any at all).
    pub fn has_level(&self, level: usize) -> bool {
        let s = self.seconds.get(level).copied().unwrap_or(0.0);
        if level == ALL { s > 0.0 } else { s >= MIN_LEVEL_SECONDS }
    }

    /// Whether anything was measured.
    pub fn is_measured(&self) -> bool {
        self.has_level(ALL)
    }

    /// A made-up profile: a straight line of `db_per_octave` in the twelfth-octave power (0 is
    /// pink noise, +3.01 white, -3.01 brown), -30 dB at 1 kHz, the same at every level, mono.
    pub fn tilt(name: &str, db_per_octave: f32) -> Self {
        let mut p = Profile::silent();
        p.name = clean_name(name);
        let at_1k = grid_pos(1_000.0);
        let mut line = [0.0f32; GRID_N];
        for (i, v) in line.iter_mut().enumerate() {
            *v = -30.0 + db_per_octave * (i as f32 - at_1k) / PER_OCTAVE as f32;
        }
        for lv in 0..LV {
            for ch in [SUM, LEFT, RIGHT, MID] {
                if let Some(c) = p.curves.get_mut(curve_index(lv, ch)) {
                    *c = line;
                }
            }
        }
        p.level = [-20.0, -17.0, -23.0];
        p.seconds = [10.0, 0.0, 0.0];
        p
    }

    /// Several profiles as one: the curves and levels averaged in dB by weight (negative and
    /// non-finite weights count as 0). A profile that lacks a level stands in with its [`ALL`]
    /// curves, and the result has the level if any of them had it. `None` when no measured
    /// profile has a weight.
    pub fn blend(parts: &[(f32, &Profile)]) -> Option<Profile> {
        let parts: Vec<(f32, &Profile)> =
            parts.iter().filter(|(w, p)| w.is_finite() && *w > 0.0 && p.is_measured()).map(|(w, p)| (*w, *p)).take(MAX_TARGETS * 4).collect();
        let total: f32 = parts.iter().map(|(w, _)| *w).sum();
        if parts.is_empty() || !(total.is_finite() && total > 0.0) {
            return None;
        }
        let mut out = Profile::silent();
        out.name = match parts.as_slice() {
            [(_, one)] => one.name.clone(),
            many => clean_name(&many.iter().map(|(_, p)| p.name.as_str()).collect::<Vec<_>>().join(" + ")),
        };
        for lv in 0..LV {
            let mut level = 0.0f32;
            let mut seconds = 0.0f32;
            for ch in 0..CH {
                let mut acc = [0.0f32; GRID_N];
                for (w, p) in &parts {
                    let from = if p.has_level(lv) { lv } else { ALL };
                    for (a, v) in acc.iter_mut().zip(p.curve(from, ch)) {
                        *a += v * w / total;
                    }
                }
                if let Some(c) = out.curves.get_mut(curve_index(lv, ch)) {
                    *c = acc;
                }
            }
            for (w, p) in &parts {
                let from = if p.has_level(lv) { lv } else { ALL };
                level += p.level.get(from).copied().unwrap_or(FLOOR_DB) * w / total;
                if p.has_level(lv) {
                    seconds = seconds.max(p.seconds.get(lv).copied().unwrap_or(0.0));
                }
            }
            if let (Some(l), Some(s)) = (out.level.get_mut(lv), out.seconds.get_mut(lv)) {
                *l = level;
                *s = seconds;
            }
        }
        out.stereo = parts.iter().any(|(_, p)| p.stereo);
        Some(out)
    }

    /// The profile as a JSON value.
    pub fn to_value(&self) -> Value {
        let mut bytes = Vec::with_capacity(CURVES * GRID_N * 2);
        for c in self.curves.iter() {
            for v in c {
                let q = (sane_db(*v) * 100.0).round().clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
                bytes.extend_from_slice(&q.to_le_bytes());
            }
        }
        let round = |v: f32| (f64::from(if v.is_finite() { v } else { 0.0 }) * 100.0).round() / 100.0;
        json!({
            "format": FORMAT,
            "version": 1,
            "name": self.name,
            "stereo": self.stereo,
            "grid": {"points": GRID_N, "lo_hz": GRID_LO_HZ, "per_octave": PER_OCTAVE},
            "level": self.level.iter().map(|v| round(sane_db(*v))).collect::<Vec<f64>>(),
            "seconds": self.seconds.iter().map(|v| round(v.max(0.0))).collect::<Vec<f64>>(),
            "curves": b64_encode(&bytes),
        })
    }

    /// The profile as JSON text.
    pub fn to_json(&self) -> String {
        self.to_value().to_string()
    }

    /// Reads [`Profile::to_value`]'s JSON. `None` for anything that is not a profile on this grid.
    pub fn from_value(v: &Value) -> Option<Profile> {
        let o = v.as_object()?;
        if o.get("format").and_then(Value::as_str) != Some(FORMAT) {
            return None;
        }
        let grid = o.get("grid").and_then(Value::as_object)?;
        if grid.get("points").and_then(Value::as_u64) != Some(GRID_N as u64) || grid.get("per_octave").and_then(Value::as_u64) != Some(PER_OCTAVE as u64) {
            return None;
        }
        let bytes = b64_decode(o.get("curves").and_then(Value::as_str)?)?;
        if bytes.len() != CURVES * GRID_N * 2 {
            return None;
        }
        let mut p = Profile::silent();
        let mut values = bytes.chunks_exact(2).map(|b| f32::from(i16::from_le_bytes([b.first().copied().unwrap_or(0), b.get(1).copied().unwrap_or(0)])) / 100.0);
        for c in p.curves.iter_mut() {
            for slot in c.iter_mut() {
                *slot = sane_db(values.next().unwrap_or(FLOOR_DB));
            }
        }
        let three = |key: &str, lo: f32, hi: f32, def: f32| {
            let mut out = [def; LV];
            if let Some(a) = o.get(key).and_then(Value::as_array) {
                for (slot, v) in out.iter_mut().zip(a) {
                    *slot = v.as_f64().map(|x| x as f32).filter(|x| x.is_finite()).map_or(def, |x| x.clamp(lo, hi));
                }
            }
            out
        };
        p.level = three("level", FLOOR_DB, 40.0, FLOOR_DB);
        p.seconds = three("seconds", 0.0, 1.0e7, 0.0);
        p.stereo = o.get("stereo").and_then(Value::as_bool).unwrap_or(false);
        p.name = clean_name(o.get("name").and_then(Value::as_str).unwrap_or(""));
        Some(p)
    }

    /// Reads a profile's JSON text.
    pub fn from_json(text: &str) -> Option<Profile> {
        if text.len() > MAX_TEXT_BYTES {
            return None;
        }
        Profile::from_value(&serde_json::from_str::<Value>(text).ok()?)
    }
}

/// A dB value a curve may hold: finite, between the floor and +40.
fn sane_db(v: f32) -> f32 {
    if v.is_finite() { v.clamp(FLOOR_DB, 40.0) } else { FLOOR_DB }
}

/// A name as kept: no control characters, trimmed, at most [`MAX_NAME_CHARS`].
pub fn clean_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(MAX_NAME_CHARS).collect::<String>().trim().to_string()
}

/// The made-up profiles every install has: straight spectral slopes, no recording behind them.
pub const FACTORY_TILTS: [(&str, f32); 5] =
    [("Flat (white noise)", 3.01), ("Bright (-1.5 dB/oct)", 1.5), ("Pink (-3 dB/oct)", 0.0), ("Warm (-4.5 dB/oct)", -1.5), ("Dark (-6 dB/oct)", -3.01)];

/// The factory profiles ([`FACTORY_TILTS`]), made on the spot.
pub fn factory_profiles() -> Vec<Profile> {
    FACTORY_TILTS.iter().map(|(name, slope)| Profile::tilt(name, *slope)).collect()
}

/// What a CopyQ module keeps beside its parameters (`Insert::state`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CopyQState {
    /// The profiles matched to, each with its weight in the blend (0..1).
    pub targets: Vec<(f32, Profile)>,
    /// This module's own input, frozen (Follow off): what the targets are compared with.
    pub source: Option<Profile>,
}

impl CopyQState {
    /// Reads a state. `None` for bytes that are not one (not text, too big, another format).
    /// Entries that are not profiles are dropped; at most [`MAX_TARGETS`] targets are kept.
    pub fn read(bytes: &[u8]) -> Option<CopyQState> {
        if bytes.len() > MAX_TEXT_BYTES {
            return None;
        }
        let v: Value = serde_json::from_str(std::str::from_utf8(bytes).ok()?).ok()?;
        let o = v.as_object()?;
        if o.get("format").and_then(Value::as_str) != Some(STATE_FORMAT) {
            return None;
        }
        let mut st = CopyQState::default();
        if let Some(list) = o.get("targets").and_then(Value::as_array) {
            for t in list.iter().take(MAX_TARGETS * 4) {
                let Some(p) = t.get("profile").and_then(Profile::from_value) else { continue };
                let w = t.get("weight").and_then(Value::as_f64).map(|w| w as f32).filter(|w| w.is_finite()).map_or(1.0, |w| w.clamp(0.0, 1.0));
                if st.targets.len() < MAX_TARGETS {
                    st.targets.push((w, p));
                }
            }
        }
        st.source = o.get("source").and_then(Profile::from_value);
        Some(st)
    }

    /// The state as bytes ([`CopyQState::read`] reads them back).
    pub fn write(&self) -> Vec<u8> {
        let targets: Vec<Value> = self
            .targets
            .iter()
            .take(MAX_TARGETS)
            .map(|(w, p)| json!({"weight": (f64::from(if w.is_finite() { w.clamp(0.0, 1.0) } else { 1.0 }) * 1000.0).round() / 1000.0, "profile": p.to_value()}))
            .collect();
        json!({
            "format": STATE_FORMAT,
            "version": 1,
            "targets": targets,
            "source": self.source.as_ref().map(Profile::to_value),
        })
        .to_string()
        .into_bytes()
    }

    /// The one profile the targets blend into (`None`: no target).
    pub fn target(&self) -> Option<Profile> {
        let parts: Vec<(f32, &Profile)> = self.targets.iter().map(|(w, p)| (*w, p)).collect();
        Profile::blend(&parts)
    }

    /// The state as the numbers the plugin takes between blocks ([`PREPARED_LEN`] of them; see
    /// [`Shape::from_prepared`]).
    pub fn prepared(&self) -> Vec<f64> {
        let mut out = Vec::with_capacity(PREPARED_LEN);
        out.push(PREPARED_MAGIC);
        for p in [self.target(), self.source.clone().filter(Profile::is_measured)] {
            let shape = p.as_ref().map(Shape::from_profile).unwrap_or_default();
            shape.push_prepared(&mut out);
        }
        out
    }
}

/// [`crate::plugins::fs::EngineReg::prepare_state`] for CopyQ: a stored state made ready off the
/// audio thread. A state that does not read gives no target and no source.
pub fn prepare_state(bytes: &[u8]) -> Vec<f64> {
    CopyQState::read(bytes).unwrap_or_default().prepared()
}

const PREPARED_MAGIC: f64 = 1_129_337_169.0;
/// Numbers per shape in a prepared state: present, stereo, three "has level" flags, three levels,
/// the curves.
const SHAPE_PREPARED: usize = 2 + LV + LV + CURVES * GRID_N;
/// The length of [`CopyQState::prepared`].
pub const PREPARED_LEN: usize = 1 + 2 * SHAPE_PREPARED;

/// A profile as the engine holds it: fixed storage, the levels already checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    /// Anything to match at all.
    pub present: bool,
    pub stereo: bool,
    /// Whether each level's curves are its own ([`ALL`] is whenever the shape is present).
    pub has: [bool; LV],
    pub level: [f32; LV],
    pub curves: Box<[Curve; CURVES]>,
}

impl Default for Shape {
    fn default() -> Self {
        Shape { present: false, stereo: false, has: [false; LV], level: [FLOOR_DB; LV], curves: Box::new([[FLOOR_DB; GRID_N]; CURVES]) }
    }
}

impl Shape {
    pub fn from_profile(p: &Profile) -> Shape {
        let mut s = Shape { present: p.is_measured(), stereo: p.stereo, ..Shape::default() };
        for lv in 0..LV {
            if let (Some(h), Some(l)) = (s.has.get_mut(lv), s.level.get_mut(lv)) {
                *h = p.has_level(lv);
                *l = sane_db(p.level.get(lv).copied().unwrap_or(FLOOR_DB));
            }
        }
        for (d, c) in s.curves.iter_mut().zip(p.curves.iter()) {
            for (a, b) in d.iter_mut().zip(c) {
                *a = sane_db(*b);
            }
        }
        s
    }

    pub fn curve(&self, level: usize, channel: usize) -> &Curve {
        static NOTHING: Curve = [FLOOR_DB; GRID_N];
        self.curves.get(curve_index(level, channel)).unwrap_or(&NOTHING)
    }

    /// Becomes a copy of `other` without allocating.
    pub fn copy_from(&mut self, other: &Shape) {
        self.present = other.present;
        self.stereo = other.stereo;
        self.has = other.has;
        self.level = other.level;
        for (d, s) in self.curves.iter_mut().zip(other.curves.iter()) {
            *d = *s;
        }
    }

    /// Nothing to match (no allocation).
    pub fn clear(&mut self) {
        self.present = false;
        self.stereo = false;
        self.has = [false; LV];
        self.level = [FLOOR_DB; LV];
    }

    fn push_prepared(&self, out: &mut Vec<f64>) {
        out.push(if self.present { 1.0 } else { 0.0 });
        out.push(if self.stereo { 1.0 } else { 0.0 });
        out.extend(self.has.iter().map(|h| if *h { 1.0 } else { 0.0 }));
        out.extend(self.level.iter().map(|l| f64::from(*l)));
        for c in self.curves.iter() {
            out.extend(c.iter().map(|v| f64::from(*v)));
        }
    }

    /// Reads shape `which` (0 the target, 1 the source) of a prepared state into `self`, without
    /// allocating. Anything that is not a prepared state (the wrong length, the wrong first
    /// number) clears the shape.
    pub fn from_prepared(&mut self, data: &[f64], which: usize) {
        self.clear();
        if data.len() != PREPARED_LEN || data.first().copied() != Some(PREPARED_MAGIC) || which > 1 {
            return;
        }
        let Some(d) = data.get(1 + which * SHAPE_PREPARED..1 + (which + 1) * SHAPE_PREPARED) else { return };
        let flag = |i: usize| d.get(i).is_some_and(|v| *v >= 0.5);
        self.present = flag(0);
        self.stereo = flag(1);
        for lv in 0..LV {
            if let (Some(h), Some(l)) = (self.has.get_mut(lv), self.level.get_mut(lv)) {
                *h = self.present && flag(2 + lv);
                *l = sane_db(d.get(2 + LV + lv).copied().unwrap_or(f64::from(FLOOR_DB)) as f32);
            }
        }
        let mut values = d.iter().skip(2 + LV + LV);
        for c in self.curves.iter_mut() {
            for slot in c.iter_mut() {
                *slot = sane_db(values.next().copied().unwrap_or(f64::from(FLOOR_DB)) as f32);
            }
        }
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(B64.get(((n >> shift) & 63) as usize).copied().map_or('A', char::from));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Reads standard base64 (padding optional, whitespace skipped). `None` for anything else.
pub fn b64_decode(text: &str) -> Option<Vec<u8>> {
    if text.len() > MAX_TEXT_BYTES {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        acc = ((acc << 6) | u32::from(v)) & 0x00FF_FFFF;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}
