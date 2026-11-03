//! The Band: MIDI Studio's six-musician state, the section and chord lanes.
//!
//! Pure data. The model can't depend on the band crate, so the Factory Band's slot values are
//! written here as literals (from `Library/Presets/Bands/Factory Band.json`). Loading is lenient
//! like Scout's: a field that doesn't parse keeps its default, numbers saturate and then clamp,
//! and the lists are capped, so a hostile or newer file can't crash or bloat a session.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Most section blocks a session keeps.
pub const MAX_BLOCKS: usize = 512;
/// Most chord events a session keeps.
pub const MAX_CHORDS: usize = 4096;
/// Largest tick position or length kept (keeps `start + length` far from overflow).
const MAX_TICK: i64 = 1 << 50;
const MAX_TEXT_CHARS: usize = 128;

/// One of the band's six musicians (`m1_*`..`m6_*` host parameters).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MusicianSlot {
    pub enabled: bool,
    pub mute: bool,
    /// Instrument choice, 0..=12.
    pub instrument: u8,
    /// Player name ("" = none).
    pub player: String,
    /// The player's persona seed, 0..=255 (0 = the slot path).
    pub player_seed: u16,
    /// "auto" or a target name.
    pub target: String,
    pub energy: u8,
    pub swing: u8,
    pub humanize: u8,
    pub dynamics: u8,
    pub fills: u8,
    pub respect: bool,
    pub octave: i8,
    pub length: u8,
    pub velocity: u8,
    /// 0 half, 1 normal, 2 double.
    pub speed: u8,
    pub voice: u8,
    pub swaminst: u8,
    /// `None` = the target's default.
    pub ksoctave: Option<i8>,
    pub art: [u8; 4],
    pub ample: u8,
    pub groove: u8,
    pub amplechords: u8,
    pub pluckchords: u8,
}

impl Default for MusicianSlot {
    fn default() -> Self {
        MusicianSlot {
            enabled: false,
            mute: false,
            instrument: 0,
            player: String::new(),
            player_seed: 0,
            target: "auto".into(),
            energy: 3,
            swing: 50,
            humanize: 50,
            dynamics: 50,
            fills: 50,
            respect: false,
            octave: 0,
            length: 100,
            velocity: 96,
            speed: 1,
            voice: 0,
            swaminst: 0,
            ksoctave: None,
            art: [0, 1, 2, 3],
            ample: 0,
            groove: 50,
            amplechords: 0,
            pluckchords: 0,
        }
    }
}

impl MusicianSlot {
    fn clamp(&mut self) {
        self.instrument = self.instrument.min(12);
        self.player.truncate(char_boundary(&self.player, MAX_TEXT_CHARS));
        self.player_seed = self.player_seed.min(255);
        self.target.truncate(char_boundary(&self.target, MAX_TEXT_CHARS));
        if self.target.is_empty() {
            self.target = "auto".into();
        }
        self.energy = self.energy.clamp(1, 4);
        self.swing = self.swing.min(100);
        self.humanize = self.humanize.min(100);
        self.dynamics = self.dynamics.min(100);
        self.fills = self.fills.min(100);
        self.octave = self.octave.clamp(-2, 2);
        self.length = self.length.clamp(10, 100);
        self.velocity = self.velocity.clamp(1, 127);
        self.speed = self.speed.min(2);
        self.voice = self.voice.min(4);
        self.swaminst = self.swaminst.min(12);
        self.ksoctave = self.ksoctave.map(|k| k.clamp(-2, 2));
        for a in &mut self.art {
            *a = (*a).min(15);
        }
        self.ample = self.ample.min(2);
        self.groove = self.groove.min(100);
        self.amplechords = self.amplechords.min(1);
        self.pluckchords = self.pluckchords.min(1);
    }
}

/// Byte index of the end of the first `max` chars of `s`.
fn char_boundary(s: &str, max: usize) -> usize {
    s.char_indices().nth(max).map_or(s.len(), |(i, _)| i)
}

/// The Performer's feel switches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BandFeel {
    pub natural: bool,
    pub tension: bool,
    pub bass_lock: bool,
    pub fills_yield: bool,
    pub leave_space: bool,
    pub dynamics: bool,
}

impl Default for BandFeel {
    fn default() -> Self {
        BandFeel { natural: true, tension: true, bass_lock: false, fills_yield: false, leave_space: false, dynamics: false }
    }
}

/// The song's five feel dials. Positions: time 0..=2, density 0..=3, pocket 0..=2, groove 0..=3, touch 0..=2.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SongFeelDials {
    pub on: bool,
    pub time: i8,
    pub density: i8,
    pub pocket: i8,
    pub groove: i8,
    pub touch: i8,
}

/// Highest position of each dial, in the order time, density, pocket, groove, touch.
const DIAL_MAX: [i8; 5] = [2, 3, 2, 3, 2];

impl SongFeelDials {
    fn clamp(&mut self) {
        self.time = self.time.clamp(0, DIAL_MAX[0]);
        self.density = self.density.clamp(0, DIAL_MAX[1]);
        self.pocket = self.pocket.clamp(0, DIAL_MAX[2]);
        self.groove = self.groove.clamp(0, DIAL_MAX[3]);
        self.touch = self.touch.clamp(0, DIAL_MAX[4]);
    }
}

/// Who plays in a section, and at what fixed energy (0 = follow the shift, 1..=4 fixed).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SectionMusician {
    pub plays: bool,
    pub energy: u8,
}

impl Default for SectionMusician {
    fn default() -> Self {
        SectionMusician { plays: true, energy: 0 }
    }
}

/// A section definition, A..=H.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SectionDef {
    pub id: char,
    pub name: String,
    /// -3..=3.
    pub energy_shift: i8,
    /// 0 none, 1 build, 2 drop.
    pub shape: u8,
    /// 0 normal, 1 intro, 2 ending.
    pub role: u8,
    /// 0 = the band's Take, 1..=16 its own.
    pub take: u8,
    pub follow_idea: bool,
    pub musicians: [SectionMusician; 6],
    /// The section's own dials in the order time, density, pocket, groove, touch; -1 = the song's.
    pub feel: [i8; 5],
}

impl Default for SectionDef {
    fn default() -> Self {
        SectionDef {
            id: 'A',
            name: "Verse".into(),
            energy_shift: 0,
            shape: 0,
            role: 0,
            take: 0,
            follow_idea: false,
            musicians: [SectionMusician::default(); 6],
            feel: [-1; 5],
        }
    }
}

impl SectionDef {
    fn clamp(&mut self) {
        self.name.truncate(char_boundary(&self.name, MAX_TEXT_CHARS));
        self.energy_shift = self.energy_shift.clamp(-3, 3);
        self.shape = self.shape.min(2);
        self.role = self.role.min(2);
        self.take = self.take.min(16);
        for m in &mut self.musicians {
            m.energy = m.energy.min(4);
        }
        for (f, max) in self.feel.iter_mut().zip(DIAL_MAX) {
            *f = (*f).clamp(-1, max);
        }
    }
}

/// A section placed on the song lane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SectionBlock {
    pub id: u32,
    pub start_tick: i64,
    pub length_ticks: i64,
    pub section: char,
    /// The arrangement chip's energy, -3..=3.
    pub energy: i8,
    /// 0 = the section's own Take.
    pub take: u8,
}

impl Default for SectionBlock {
    fn default() -> Self {
        SectionBlock { id: 0, start_tick: 0, length_ticks: 0, section: 'A', energy: 0, take: 0 }
    }
}

/// A chord on the chord lane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChordEvent {
    pub id: u32,
    pub tick: i64,
    pub length_ticks: i64,
    /// Chord slot, 0..=111.
    pub slot: u8,
    /// -1 = root, else 0..=11 key-relative.
    pub bass: i8,
}

impl Default for ChordEvent {
    fn default() -> Self {
        ChordEvent { id: 0, tick: 0, length_ticks: 0, slot: 0, bass: -1 }
    }
}

/// Everything the Band keeps in a session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BandState {
    pub slots: [MusicianSlot; 6],
    /// 0..=11.
    pub key: u8,
    /// 0 major, 1 minor.
    pub scale: u8,
    /// 1..=16.
    pub take: u8,
    pub feel: BandFeel,
    pub song_feel: SongFeelDials,
    /// Bit k: musician k doesn't follow the song's feel.
    pub feel_ignore: u8,
    pub defs: Vec<SectionDef>,
    pub sections: Vec<SectionBlock>,
    pub chords: Vec<ChordEvent>,
    pub band_name: String,
    /// Next block / chord id (always above every id in use).
    pub next_id: u32,
}

/// The Factory Band's slot `k` (`Library/Presets/Bands/Factory Band.json`).
fn factory_slot(k: usize) -> MusicianSlot {
    // (enabled, instrument, player, seed, energy, respect, octave, speed, ksoctave)
    type Row = (bool, u8, &'static str, u16, u8, bool, i8, u8, i8);
    const ROWS: [Row; 6] = [
        (true, 0, "Clay", 1, 1, true, -1, 0, 0),
        (true, 5, "Benny", 8, 1, true, -2, 0, 0),
        (true, 2, "Cole", 41, 1, true, 0, 0, 0),
        (true, 7, "Aria", 14, 4, true, 2, 1, -1),
        (true, 9, "Duke", 16, 4, true, 0, 0, 0),
        (false, 0, "Session", 0, 3, false, 0, 1, 0),
    ];
    let Some(&(enabled, instrument, player, player_seed, energy, respect, octave, speed, ks)) = ROWS.get(k) else {
        return MusicianSlot::default();
    };
    // The file names each musician's target; a session starts on "auto" and the band crate picks
    // the target from the instrument.
    MusicianSlot {
        enabled,
        instrument,
        player: player.into(),
        player_seed,
        energy,
        respect,
        octave,
        speed,
        ksoctave: Some(ks),
        ..MusicianSlot::default()
    }
}

impl Default for BandState {
    fn default() -> Self {
        BandState {
            slots: std::array::from_fn(factory_slot),
            key: 0,
            scale: 0,
            take: 1,
            feel: BandFeel::default(),
            song_feel: SongFeelDials::default(),
            feel_ignore: 0,
            defs: vec![SectionDef::default()],
            sections: Vec::new(),
            chords: Vec::new(),
            band_name: "Factory Band".into(),
            next_id: 1,
        }
    }
}

impl BandState {
    /// True while nothing differs from the Factory Band (then the session file omits `band`).
    pub fn is_untouched(&self) -> bool {
        *self == BandState::default()
    }

    /// The section definition with this id.
    pub fn def(&self, id: char) -> Option<&SectionDef> {
        self.defs.iter().find(|d| d.id == id)
    }

    /// Clamp every field to its range, drop unusable blocks and chords, cap the lists, keep the
    /// definitions unique A..=H and sort the lanes by start.
    pub fn sanitize(&mut self) {
        for s in &mut self.slots {
            s.clamp();
        }
        self.key = self.key.min(11);
        self.scale = self.scale.min(1);
        self.take = self.take.clamp(1, 16);
        self.song_feel.clamp();
        self.feel_ignore &= 0x3F;
        self.band_name.truncate(char_boundary(&self.band_name, MAX_TEXT_CHARS));

        let mut seen = [false; 8];
        self.defs.retain_mut(|d| {
            let Some(i) = ('A'..='H').position(|c| c == d.id) else { return false };
            if seen.get(i).copied().unwrap_or(true) {
                return false;
            }
            if let Some(s) = seen.get_mut(i) {
                *s = true;
            }
            d.clamp();
            true
        });
        self.defs.sort_by_key(|d| d.id);
        if self.defs.is_empty() {
            self.defs.push(SectionDef::default());
        }

        self.sections.retain_mut(|b| {
            if b.length_ticks <= 0 || !('A'..='H').contains(&b.section) {
                return false;
            }
            b.start_tick = b.start_tick.clamp(0, MAX_TICK);
            b.length_ticks = b.length_ticks.min(MAX_TICK);
            b.energy = b.energy.clamp(-3, 3);
            b.take = b.take.min(16);
            true
        });
        self.sections.truncate(MAX_BLOCKS);
        self.sections.sort_by_key(|b| (b.start_tick, b.id));

        self.chords.retain_mut(|c| {
            if c.length_ticks <= 0 {
                return false;
            }
            c.tick = c.tick.clamp(0, MAX_TICK);
            c.length_ticks = c.length_ticks.min(MAX_TICK);
            c.slot = c.slot.min(111);
            if !(0..=11).contains(&c.bass) {
                c.bass = -1;
            }
            true
        });
        self.chords.truncate(MAX_CHORDS);
        self.chords.sort_by_key(|c| (c.tick, c.id));

        let top = self.sections.iter().map(|b| b.id).chain(self.chords.iter().map(|c| c.id)).max().unwrap_or(0);
        self.next_id = self.next_id.max(top.saturating_add(1)).max(1);
    }
}

/// Retries for an integer that doesn't fit its field's type: the number clamped to a type's range.
fn saturated(v: &Value) -> Vec<Value> {
    let Value::Number(n) = v else { return Vec::new() };
    let Some(n) = n.as_i64().or_else(|| n.as_u64().map(|u| i64::try_from(u).unwrap_or(i64::MAX))) else { return Vec::new() };
    if n < 0 {
        vec![n.clamp(-128, 127).into(), n.clamp(0, 255).into(), n.clamp(0, 65535).into()]
    } else {
        vec![n.clamp(0, 255).into(), n.clamp(-128, 127).into(), n.clamp(0, 65535).into()]
    }
}

/// `default` with every key of `input` that fits applied. A key that doesn't parse (a string for a
/// number, `null`, a wrong-sized array) keeps the default; out-of-range integers saturate.
fn merge_lenient<T: Serialize + serde::de::DeserializeOwned + Clone>(default: &T, input: &Value, skip: &[&str]) -> T {
    let Ok(Value::Object(mut base)) = serde_json::to_value(default) else { return default.clone() };
    if let Value::Object(map) = input {
        for (k, v) in map {
            if skip.contains(&k.as_str()) {
                continue;
            }
            let mut tries = vec![v.clone()];
            tries.extend(saturated(v));
            for t in tries {
                let mut trial: Map<String, Value> = base.clone();
                trial.insert(k.clone(), t);
                if serde_json::from_value::<T>(Value::Object(trial.clone())).is_ok() {
                    base = trial;
                    break;
                }
            }
        }
    }
    serde_json::from_value::<T>(Value::Object(base)).unwrap_or_else(|_| default.clone())
}

/// The object entries of a JSON list, each merged over `default`; at most `4 * cap` are read.
fn lenient_list<T: Serialize + serde::de::DeserializeOwned + Clone>(input: &Value, default: &T, cap: usize) -> Vec<T> {
    let Value::Array(items) = input else { return Vec::new() };
    items.iter().filter(|v| v.is_object()).take(cap.saturating_mul(4)).map(|v| merge_lenient(default, v, &[])).collect()
}

/// Last resort: any problem keeps the default for that field; sizes are capped and ranges clamped.
pub(crate) fn lenient_state<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BandState, D::Error> {
    let v = Value::deserialize(d)?;
    let mut st = merge_lenient(&BandState::default(), &v, &["slots", "defs", "sections", "chords"]);
    if let Value::Object(map) = &v {
        if let Some(Value::Array(slots)) = map.get("slots") {
            for (i, sv) in slots.iter().take(6).enumerate() {
                if let Some(slot) = st.slots.get_mut(i) {
                    *slot = merge_lenient(&factory_slot(i), sv, &[]);
                }
            }
        }
        if let Some(defs) = map.get("defs").filter(|d| d.is_array()) {
            st.defs = lenient_list(defs, &SectionDef::default(), 8);
        }
        if let Some(sections) = map.get("sections").filter(|d| d.is_array()) {
            st.sections = lenient_list(sections, &SectionBlock::default(), MAX_BLOCKS);
        }
        if let Some(chords) = map.get("chords").filter(|d| d.is_array()) {
            st.chords = lenient_list(chords, &ChordEvent::default(), MAX_CHORDS);
        }
    }
    st.sanitize();
    Ok(st)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Mixer, Route, Session};
    use serde_json::json;

    fn band_from(v: Value) -> BandState {
        let mut s = serde_json::to_value(Session::default()).unwrap();
        s["band"] = v;
        serde_json::from_value::<Session>(s).unwrap().band
    }

    #[test]
    fn old_sessions_load_with_the_default_band() {
        let mut s = serde_json::to_value(Session::default()).unwrap();
        assert!(s.as_object_mut().unwrap().remove("band").is_none());
        let loaded: Session = serde_json::from_value(s).unwrap();
        assert_eq!(loaded.band, BandState::default());
        assert_eq!(loaded.band.slots[0].player, "Clay");
        assert_eq!(loaded.band.slots[3].ksoctave, Some(-1));
        assert!(!loaded.band.slots[5].enabled);
        assert_eq!(loaded.band.defs.len(), 1);
        assert_eq!(loaded.band.defs[0].name, "Verse");
    }

    #[test]
    fn untouched_band_is_not_written() {
        let s = Session::default();
        assert!(s.band.is_untouched());
        assert!(serde_json::to_value(&s).unwrap().get("band").is_none());
        let mut t = Session::default();
        t.band.key = 7;
        assert!(serde_json::to_value(&t).unwrap().get("band").is_some());
    }

    #[test]
    fn band_round_trips() {
        let mut s = Session::default();
        s.band.key = 9;
        s.band.scale = 1;
        s.band.take = 5;
        s.band.slots[2].mute = true;
        s.band.slots[1].ksoctave = None;
        s.band.song_feel = SongFeelDials { on: true, time: 1, density: 2, pocket: 1, groove: 3, touch: 2 };
        s.band.defs.push(SectionDef { id: 'B', name: "Chorus".into(), energy_shift: 2, ..SectionDef::default() });
        s.band.sections.push(SectionBlock { id: 1, start_tick: 0, length_ticks: 7680, section: 'A', energy: -1, take: 3 });
        s.band.chords.push(ChordEvent { id: 2, tick: 0, length_ticks: 1920, slot: 14, bass: 4 });
        s.band.next_id = 3;
        let back: Session = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.band, s.band);
    }

    #[test]
    fn hostile_band_state_loads_clamped() {
        let many: Vec<Value> = (0..100_000).map(|i| json!({"id": i, "tick": i, "length_ticks": 10, "slot": 3})).collect();
        let slots: Vec<Value> = (0..9).map(|_| json!({"energy": 99})).collect();
        let b = band_from(json!({
            "slots": slots,
            "key": 200,
            "take": 0,
            "defs": [{"id": "A"}, {"id": "Z"}, {"id": "A", "name": "dup"}, {"id": "C", "energy_shift": 90}],
            "sections": [{"id": 1, "start_tick": 0, "length_ticks": -5, "section": "A"}, {"id": 2, "start_tick": 0, "length_ticks": 9, "section": "A"}],
            "chords": [{"id": 1, "tick": 0, "length_ticks": 10, "slot": 500, "bass": 40}],
        }));
        assert_eq!(b.slots.len(), 6);
        assert!(b.slots.iter().all(|s| s.energy == 4));
        assert_eq!(b.key, 11);
        assert_eq!(b.take, 1);
        assert_eq!(b.defs.iter().map(|d| d.id).collect::<String>(), "AC");
        assert_eq!(b.defs[0].name, "Verse");
        assert_eq!(b.defs[1].energy_shift, 3);
        assert_eq!(b.sections.len(), 1);
        assert_eq!(b.chords[0].slot, 111);
        assert_eq!(b.chords[0].bass, -1);

        let b = band_from(json!({ "chords": many }));
        assert_eq!(b.chords.len(), MAX_CHORDS);
        assert!(b.next_id > 4095);

        // A short slots list: the rest keep the factory slots.
        let b = band_from(json!({ "slots": [{"player": "X"}, {}, {}] }));
        assert_eq!(b.slots[0].player, "X");
        assert_eq!(b.slots[4].player, "Duke");

        // A field of the wrong type (what NaN becomes) keeps its default.
        let b = band_from(json!({ "slots": [{"swing": "NaN", "velocity": null, "octave": -500}], "key": "NaN" }));
        assert_eq!(b.slots[0].swing, 50);
        assert_eq!(b.slots[0].velocity, 96);
        assert_eq!(b.slots[0].octave, -2);
        assert_eq!(b.key, 0);
        // A document that isn't an object is the default band.
        assert_eq!(band_from(json!([1, 2, 3])), BandState::default());
        // A literal NaN is not JSON at all.
        assert!(serde_json::from_str::<Value>("{\"swing\": NaN}").is_err());
    }

    #[test]
    fn sanitize_sorts_and_caps_lanes() {
        let mut b = BandState::default();
        for i in 0..600 {
            b.sections.push(SectionBlock { id: i, start_tick: i64::from(600 - i), length_ticks: 5, ..SectionBlock::default() });
        }
        b.sanitize();
        assert_eq!(b.sections.len(), MAX_BLOCKS);
        assert!(b.sections.windows(2).all(|w| w[0].start_tick <= w[1].start_tick));
        assert!(b.def('A').is_some() && b.def('B').is_none());
    }

    #[test]
    fn musician_output_is_sanitized_to_none() {
        let mut m = Mixer { output: Route::Musician(1), input: Route::Musician(2), ..Mixer::new(crate::ChannelFormat::Stereo) };
        m.sanitize();
        assert_eq!(m.output, Route::None);
        assert_eq!(m.input, Route::Musician(2));
    }

    #[test]
    fn route_modules_round_trips_and_is_input_only() {
        let one = crate::ModuleRef { track: crate::TrackId(7), module: crate::ModuleId(3) };
        let r = Route::Modules(vec![one]);
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, "{\"Modules\":[{\"track\":7,\"module\":3}]}");
        assert_eq!(serde_json::from_str::<Route>(&json).unwrap(), r);
        assert_eq!(r.module_sources(), &[one]);
        assert!(Route::Musician(1).module_sources().is_empty());
        // The set keeps its order and drops duplicates; empty is `None`.
        let two = crate::ModuleRef { track: crate::TrackId(8), module: crate::ModuleId(1) };
        assert_eq!(Route::modules([two, one, two]), Route::Modules(vec![two, one]));
        assert_eq!(Route::modules([]), Route::None);
        let mut m = Mixer { output: r.clone(), input: r.clone(), ..Mixer::new(crate::ChannelFormat::Stereo) };
        m.sanitize();
        assert_eq!(m.output, Route::None);
        assert_eq!(m.input, r);
        // Old sessions (no such route) load; a session with one on an audio track, or feeding a
        // track its own module, loses it.
        assert_eq!(serde_json::from_str::<Route>("\"None\"").unwrap(), Route::None);
        let mut s = crate::Session::new("x", soundcraft_time::SampleRate::HZ_48000);
        let audio = s.add_track(crate::TrackKind::Audio, crate::ChannelFormat::Mono, Some("A"));
        let inst = s.add_track(crate::TrackKind::Instrument, crate::ChannelFormat::Stereo, Some("I"));
        let own = crate::ModuleRef { track: inst, module: crate::ModuleId(1) };
        let other = crate::ModuleRef { track: audio, module: crate::ModuleId(1) };
        s.track_mut(audio).unwrap().mixer.input = Route::Modules(vec![other]);
        s.track_mut(inst).unwrap().mixer.input = Route::Modules(vec![own]);
        s.sanitize();
        assert_eq!(s.track(audio).unwrap().mixer.input, Route::None);
        assert_eq!(s.track(inst).unwrap().mixer.input, Route::None);
        // Its own module drops out of a set; the rest stays (each once).
        s.track_mut(inst).unwrap().mixer.input = Route::Modules(vec![other, own, other]);
        s.sanitize();
        assert_eq!(s.track(inst).unwrap().mixer.input, Route::Modules(vec![other]));
    }

    #[test]
    fn route_musician_serializes_as_tagged() {
        assert_eq!(serde_json::to_string(&Route::Musician(2)).unwrap(), "{\"Musician\":2}");
        assert_eq!(serde_json::from_str::<Route>("{\"Musician\":2}").unwrap(), Route::Musician(2));
    }
}
