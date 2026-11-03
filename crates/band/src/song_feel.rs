//! Port of `SongFeel.h`, plus the pure (file-free) half of `FeelPresets.h`.
//!
//! SongFeel: the song's feel - five dials, the factory presets, and (task 2) how a dial shifts a musician.
//! A dial at 0 is neutral (it changes nothing); a feel that is off changes nothing.

pub const FEEL_TIME: i32 = 0;
pub const FEEL_DENSITY: i32 = 1;
pub const FEEL_POCKET: i32 = 2;
pub const FEEL_GROOVE: i32 = 3;
pub const FEEL_TOUCH: i32 = 4;
pub const FEEL_DIAL_COUNT: i32 = 5;

/// The song's dials. Time is song-level only (a Speed change restarts a musician's step clock).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SongFeel {
    pub on: bool,
    /// time 0 normal 1 half 2 double
    pub time: i8,
    /// density 0 medium 1 sparse 2 light 3 busy
    pub density: i8,
    /// pocket 0 on the beat 1 laid back 2 pushing
    pub pocket: i8,
    /// groove 0 straight 1 swing 2 shuffle 3 triplet
    pub groove: i8,
    /// touch 0 natural 1 soft 2 hard
    pub touch: i8,
    /// the preset it came from ("" = none)
    pub preset: String,
    /// a dial moved since the preset
    pub edited: bool,
}

impl SongFeel {
    pub fn dial(&self, d: i32) -> i8 {
        if d == FEEL_TIME {
            self.time
        } else if d == FEEL_DENSITY {
            self.density
        } else if d == FEEL_POCKET {
            self.pocket
        } else if d == FEEL_GROOVE {
            self.groove
        } else {
            self.touch
        }
    }
    pub fn set_dial(&mut self, d: i32, v: i8) {
        let slot = if d == FEEL_TIME {
            &mut self.time
        } else if d == FEEL_DENSITY {
            &mut self.density
        } else if d == FEEL_POCKET {
            &mut self.pocket
        } else if d == FEEL_GROOVE {
            &mut self.groove
        } else {
            &mut self.touch
        };
        *slot = v;
    }
}

/// A section's own dials (-1 = the song's). Time has none (song-level).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeelOverride {
    pub density: i8,
    pub pocket: i8,
    pub groove: i8,
    pub touch: i8,
}

impl Default for FeelOverride {
    fn default() -> Self {
        Self { density: -1, pocket: -1, groove: -1, touch: -1 }
    }
}

impl FeelOverride {
    pub fn any(&self) -> bool {
        self.density >= 0 || self.pocket >= 0 || self.groove >= 0 || self.touch >= 0
    }
    pub fn dial(&self, d: i32) -> i8 {
        if d == FEEL_DENSITY {
            self.density
        } else if d == FEEL_POCKET {
            self.pocket
        } else if d == FEEL_GROOVE {
            self.groove
        } else if d == FEEL_TOUCH {
            self.touch
        } else {
            -1
        }
    }
    pub fn set_dial(&mut self, d: i32, v: i8) {
        if d == FEEL_DENSITY {
            self.density = v;
        } else if d == FEEL_POCKET {
            self.pocket = v;
        } else if d == FEEL_GROOVE {
            self.groove = v;
        } else if d == FEEL_TOUCH {
            self.touch = v;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResolvedFeel {
    pub on: bool,
    pub time: i32,
    pub density: i32,
    pub pocket: i32,
    pub groove: i32,
    pub touch: i32,
}

/// The feel a section plays: its own dials where set, else the song's. Off when the song's feel is off.
pub fn resolve(f: &SongFeel, o: &FeelOverride) -> ResolvedFeel {
    if !f.on {
        return ResolvedFeel::default();
    }
    let pick = |own: i8, song: i8| i32::from(if own >= 0 { own } else { song });
    ResolvedFeel {
        on: true,
        time: i32::from(f.time),
        density: pick(o.density, f.density),
        pocket: pick(o.pocket, f.pocket),
        groove: pick(o.groove, f.groove),
        touch: pick(o.touch, f.touch),
    }
}

/// How many positions a dial has.
pub fn dial_positions(d: i32) -> i32 {
    if d == FEEL_TIME {
        3
    } else if d == FEEL_DENSITY {
        4
    } else if d == FEEL_POCKET {
        3
    } else if d == FEEL_GROOVE {
        4
    } else {
        3
    }
}

const DIAL_NAMES: [[&str; 4]; 5] = [
    ["normal", "half", "double", ""],
    ["medium", "sparse", "light", "busy"],
    ["on", "laidback", "pushing", ""],
    ["straight", "swing", "shuffle", "triplet"],
    ["natural", "soft", "hard", ""],
];
const DIAL_LABELS: [[&str; 4]; 5] = [
    ["Normal", "Half-time", "Double-time", ""],
    ["Medium", "Sparse", "Light", "Busy"],
    ["On the beat", "Laid back", "Pushing", ""],
    ["Straight", "Swing", "Shuffle", "Triplet"],
    ["Natural", "Soft", "Hard", ""],
];

fn table(t: &'static [[&'static str; 4]; 5], d: i32, pos: i32) -> &'static str {
    if (0..FEEL_DIAL_COUNT).contains(&d) && (0..dial_positions(d)).contains(&pos) {
        t.get(d as usize).and_then(|r| r.get(pos as usize)).copied().unwrap_or("")
    } else {
        ""
    }
}

/// A dial's position as it is written in a song file ("half", "sparse" ...).
pub fn dial_name(d: i32, pos: i32) -> &'static str {
    table(&DIAL_NAMES, d, pos)
}
/// As the UI shows it.
pub fn dial_label(d: i32, pos: i32) -> &'static str {
    table(&DIAL_LABELS, d, pos)
}
pub fn dial_title(d: i32) -> &'static str {
    const T: [&str; 5] = ["Time", "Density", "Pocket", "Groove", "Touch"];
    if (0..FEEL_DIAL_COUNT).contains(&d) { T.get(d as usize).copied().unwrap_or("") } else { "" }
}
/// -1 for an unknown name.
pub fn dial_index(d: i32, name: &str) -> i32 {
    for p in 0..dial_positions(d) {
        if name == dial_name(d, p) {
            return p;
        }
    }
    -1
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeelPreset {
    pub name: &'static str,
    pub time: i8,
    pub density: i8,
    pub pocket: i8,
    pub groove: i8,
    pub touch: i8,
    /// -1 none, 1 = 6/8, 2 = 3/4
    pub suggest_meter: i32,
}

const fn fp(name: &'static str, time: i8, density: i8, pocket: i8, groove: i8, touch: i8, suggest_meter: i32) -> FeelPreset {
    FeelPreset { name, time, density, pocket, groove, touch, suggest_meter }
}

const FACTORY_FEEL_PRESETS: [FeelPreset; 22] = [
    fp("Ballad", 1, 1, 1, 0, 1, -1),
    fp("Worship anthem", 0, 2, 0, 0, 0, -1),
    fp("Hymn / traditional", 1, 2, 0, 0, 0, -1),
    fp("Gospel", 0, 3, 2, 1, 2, -1),
    fp("Country train", 2, 0, 2, 0, 0, -1),
    fp("Folk / acoustic", 0, 2, 1, 0, 1, -1),
    fp("Motown / soul", 0, 0, 0, 0, 0, -1),
    fp("Blues shuffle", 0, 0, 1, 2, 0, -1),
    fp("Rock, driving", 0, 3, 2, 0, 2, -1),
    fp("Funk", 0, 3, 0, 0, 2, -1),
    fp("Bossa / Latin", 0, 2, 1, 0, 1, -1),
    fp("Cinematic / ambient", 1, 1, 1, 0, 1, -1),
    fp("Acoustic worship", 0, 2, 0, 0, 1, -1),
    fp("Worship ballad (3/4 \u{b7} 6/8)", 0, 2, 1, 0, 1, 2),
    fp("Reggae", 1, 2, 1, 0, 1, -1),
    fp("Bluegrass", 0, 3, 2, 0, 2, -1),
    fp("Southern rock", 0, 0, 1, 0, 2, -1),
    fp("Country two-step", 0, 0, 0, 0, 0, -1),
    fp("Country waltz", 0, 2, 1, 0, 1, 2),
    fp("Celtic jig", 0, 3, 2, 3, 0, 1),
    fp("Pop", 0, 0, 0, 0, 0, -1),
    fp("Neo-soul / R&B", 0, 0, 1, 1, 1, -1),
];

/// The factory presets (spec §3), in the order the menu lists them.
pub fn factory_feel_presets() -> &'static [FeelPreset] {
    &FACTORY_FEEL_PRESETS
}

/// The feel a preset gives (on, its dials, its name).
pub fn feel_from_preset(p: &FeelPreset) -> SongFeel {
    SongFeel {
        on: true,
        time: p.time,
        density: p.density,
        pocket: p.pocket,
        groove: p.groove,
        touch: p.touch,
        preset: p.name.to_string(),
        edited: false,
    }
}

/// What a resolved feel does to one musician (spec §2): multipliers and offsets on its own knobs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeelShift {
    /// -1 one step slower (Half), +1 faster: rhythm roles in 4/4 only
    pub speed_step: i32,
    pub fills_mul: f32,
    pub vel_mul: f32,
    pub dyn_mul: f32,
    pub len_mul: f32,
    pub push_mul: f32,
    pub pickups_mul: f32,
    pub energy_add: i32,
    pub space_add: f32,
    /// pocketMs and pushMul: Natural feel only
    pub pocket_ms: f32,
    /// -1 = the player's own swing base
    pub swing_centre: f32,
    /// anything at all (else the musician plays exactly as without a feel)
    pub any: bool,
}

impl Default for FeelShift {
    fn default() -> Self {
        Self {
            speed_step: 0,
            fills_mul: 1.0,
            vel_mul: 1.0,
            dyn_mul: 1.0,
            len_mul: 1.0,
            push_mul: 1.0,
            pickups_mul: 1.0,
            energy_add: 0,
            space_add: 0.0,
            pocket_ms: 0.0,
            swing_centre: -1.0,
            any: false,
        }
    }
}

/// role: a kRoleNames index (0 keys, 1 bass, 2 drums, 3 guitar, 4 ensemble, 5 melodic), -1 Session. Rhythm = keys, bass,
/// drums, guitar, Session: they take Time. meter12: 3/4 or 6/8 (no half / double time there).
#[allow(clippy::float_cmp)]
pub fn shift_for(f: &ResolvedFeel, role: i32, drums: bool, meter12: bool, natural: bool) -> FeelShift {
    let mut s = FeelShift::default();
    if !f.on {
        return s;
    }
    let rhythm = role <= 3 || drums;
    if rhythm && !meter12 {
        s.speed_step = if f.time == 1 {
            -1
        } else if f.time == 2 {
            1
        } else {
            0
        };
    }
    if f.density == 1 {
        s.fills_mul = 0.3;
        s.energy_add = -1;
        s.space_add = 0.25;
        s.pickups_mul = 0.5;
    } else if f.density == 2 {
        s.fills_mul = 0.6;
        s.space_add = 0.1;
    } else if f.density == 3 {
        s.fills_mul = 1.5;
        s.energy_add = 1;
        s.pickups_mul = 1.5;
    }
    if natural && f.pocket == 1 {
        s.pocket_ms = if drums || role == 2 {
            6.0
        } else if role == 1 {
            8.0
        } else {
            12.0
        };
        s.push_mul = 0.5;
    } else if natural && f.pocket == 2 {
        s.pocket_ms = if drums || role == 2 { -4.0 } else { -6.0 };
        s.push_mul = 1.6;
    }
    if f.groove == 1 {
        s.swing_centre = 58.0;
    } else if f.groove >= 2 {
        s.swing_centre = 66.0;
    }
    if f.touch == 1 {
        s.vel_mul = 0.82;
        s.dyn_mul = 0.7;
        s.len_mul = 1.1;
    } else if f.touch == 2 {
        s.vel_mul = 1.12;
        s.dyn_mul = 1.3;
        s.len_mul = 0.9;
    }
    s.any = s.speed_step != 0
        || s.fills_mul != 1.0
        || s.energy_add != 0
        || s.space_add != 0.0
        || s.pickups_mul != 1.0
        || s.pocket_ms != 0.0
        || s.push_mul != 1.0
        || s.swing_centre >= 0.0
        || s.vel_mul != 1.0
        || s.dyn_mul != 1.0
        || s.len_mul != 1.0;
    s
}

/// Speed (0 Half, 1 Normal, 2 Double) moved by a step, never past Half or Double.
pub fn effective_speed(knob: i32, step: i32) -> i32 {
    let v = knob.wrapping_add(step);
    v.clamp(0, 2)
}
/// Fills (0..100) scaled: 0 stays 0 ("never"), capped at 100.
#[allow(clippy::manual_clamp)] // the C++ chain (a NaN passes through)
pub fn effective_fills(knob: f32, mul: f32) -> f32 {
    let v = knob * mul;
    if v > 100.0 {
        100.0
    } else if v < 0.0 {
        0.0
    } else {
        v
    }
}
/// A player's phrasing space / pickups with the Density shift: unchanged without one (a player's own value stands),
/// capped only when the shift raises it.
pub fn effective_space(base: f32, add: f32) -> f32 {
    if add > 0.0 { if base + add > 0.9 { 0.9 } else { base + add } } else { base }
}
pub fn effective_pickups(base: f32, mul: f32) -> f32 {
    if mul > 1.0 { if base * mul > 1.0 { 1.0 } else { base * mul } } else { base * mul }
}
/// Energy (1..4) moved, clamped.
pub fn effective_energy(e: i32, add: i32) -> i32 {
    let v = e.wrapping_add(add);
    v.clamp(1, 4)
}

// ---- FeelPresets.h: the user's own song-feel presets ("Save as preset...") ------------------------------------
// Kept in the Library folder's FeelPresets.json: { "presets": [ { "name": "...", "time": "half", "density": "sparse",
// ... }, ... ] }. Never inside a song. An unreadable file reads as none. Only the text half is here: the caller owns
// the file (there is no user folder on wasm).

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserFeelPreset {
    pub name: String,
    pub dials: SongFeel,
}

/// False when the text is there but isn't a presets file we can read: then it is never written over (the user's
/// presets in it would be lost). A missing file (`None`) is fine.
pub fn feel_presets_writable(text: Option<&str>) -> bool {
    let Some(text) = text else { return true };
    serde_json::from_str::<serde_json::Value>(text).ok().and_then(|j| j.get("presets").map(serde_json::Value::is_array)).unwrap_or(false)
}

/// `juce::var::toString` of a JSON value, as far as a dial name or preset name needs it.
fn var_text(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Bool(b)) => if *b { "1" } else { "0" }.to_string(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(serde_json::Value::Array(_)) => "[Array]".to_string(),
        Some(serde_json::Value::Object(_)) => "Object".to_string(),
        _ => String::new(),
    }
}

pub fn load_feel_presets(text: &str) -> Vec<UserFeelPreset> {
    let mut out = Vec::new();
    let Ok(j) = serde_json::from_str::<serde_json::Value>(text) else { return out };
    if let Some(a) = j.get("presets").and_then(serde_json::Value::as_array) {
        for v in a {
            let mut p = UserFeelPreset { name: var_text(v.get("name")), dials: SongFeel::default() };
            if p.name.is_empty() {
                continue;
            }
            p.dials.on = true;
            p.dials.preset = p.name.clone();
            for d in 0..FEEL_DIAL_COUNT {
                let idx = dial_index(d, &var_text(v.get(dial_title(d).to_lowercase().as_str())));
                p.dials.set_dial(d, idx.max(0) as i8);
            }
            out.push(p);
        }
    }
    out
}

/// The file's text for these presets.
pub fn feel_presets_to_json(all: &[UserFeelPreset]) -> String {
    let arr: Vec<serde_json::Value> = all
        .iter()
        .map(|p| {
            let mut o = serde_json::Map::new();
            o.insert("name".into(), serde_json::Value::String(p.name.clone()));
            for d in 0..FEEL_DIAL_COUNT {
                o.insert(dial_title(d).to_lowercase(), serde_json::Value::String(dial_name(d, i32::from(p.dials.dial(d))).to_string()));
            }
            serde_json::Value::Object(o)
        })
        .collect();
    let mut root = serde_json::Map::new();
    root.insert("presets".into(), serde_json::Value::Array(arr));
    serde_json::Value::Object(root).to_string()
}

/// Adds the preset, or replaces the one with that name (keeping its place). False when the name is empty.
pub fn save_feel_preset(all: &mut Vec<UserFeelPreset>, name: &str, dials: &SongFeel) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut p = UserFeelPreset { name: name.to_string(), dials: dials.clone() };
    p.dials.on = true;
    p.dials.preset = name.to_string();
    p.dials.edited = false;
    let mut replaced = false;
    for q in all.iter_mut() {
        if q.name == name {
            *q = p.clone();
            replaced = true;
        }
    }
    if !replaced {
        all.push(p);
    }
    true
}

/// Removes every preset with that name; false when none was there.
pub fn delete_feel_preset(all: &mut Vec<UserFeelPreset>, name: &str) -> bool {
    let before = all.len();
    all.retain(|p| p.name != name);
    all.len() != before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dials_and_presets() {
        assert_eq!(factory_feel_presets().len(), 22);
        assert_eq!(dial_index(FEEL_GROOVE, "shuffle"), 2);
        assert_eq!(dial_index(FEEL_TIME, ""), -1);
        assert_eq!(dial_name(FEEL_POCKET, 3), "");
        let f = feel_from_preset(&factory_feel_presets()[7]);
        let r = resolve(&f, &FeelOverride { groove: 1, ..FeelOverride::default() });
        assert_eq!((r.pocket, r.groove), (1, 1));
        let s = shift_for(&r, 1, false, false, true);
        assert_eq!((s.pocket_ms, s.swing_centre, s.any), (8.0, 58.0, true));
        assert!(!shift_for(&resolve(&feel_from_preset(&factory_feel_presets()[6]), &FeelOverride::default()), 0, false, false, true).any);
    }

    #[test]
    fn user_presets_round_trip() {
        let mut all = Vec::new();
        let dials = SongFeel { time: 1, groove: 3, ..SongFeel::default() };
        assert!(save_feel_preset(&mut all, "Mine", &dials));
        assert!(save_feel_preset(&mut all, "Mine", &dials));
        assert_eq!(all.len(), 1);
        let text = feel_presets_to_json(&all);
        assert!(feel_presets_writable(Some(&text)) && feel_presets_writable(None) && !feel_presets_writable(Some("{")));
        assert_eq!(load_feel_presets(&text), all);
        assert!(delete_feel_preset(&mut all, "Mine") && !delete_feel_preset(&mut all, "Mine"));
    }
}
