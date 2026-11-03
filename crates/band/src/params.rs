//! Port of `Parameters.h`: the host parameter list (the contract) and `packInto`.
//!
//! Three layers, as in MIDI Studio:
//!   1. [`host_params`]: what the DAW sees and automates, ids stable for ever;
//!   2. [`pack_into`]: the one place the table folds into Amorph's Band endpoints ([`packed_endpoints`]);
//!   3. `Band` / `Musician`: still speaking the Cmajor endpoint layout.
//!
//! Folded here: mK_on is a plain Enabled switch; the packed 16-bit mK_artmap is four articulation-key choices;
//! mK_render is gone. Player has 16 slots and Target 32, because both come from the Library.
//!
//! The tables are built once ([`host_params`] / [`packed_endpoints`] are `OnceLock`s); the runtime builds them
//! before any audio, so [`pack_into`] never allocates.

use std::sync::OnceLock;

/// `hostParams().size()`.
pub const HOST_PARAMS: usize = 159;
/// `packedEndpoints().size()`.
pub const PACKED: usize = 141;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostParamKind {
    Int,
    Choice,
    Bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HostParam {
    pub id: String,
    pub name: String,
    pub kind: HostParamKind,
    pub min: i32,
    pub max: i32,
    pub def: i32,
    /// Choice only
    pub choices: Vec<String>,
}

/// The engine's art map holds 4 bits per slot: 16 keys from C0. Offer exactly those.
pub const ART_KEY_NAMES: [&str; 16] = ["C0", "C#0", "D0", "D#0", "E0", "F0", "F#0", "G0", "G#0", "A0", "A#0", "B0", "C1", "C#1", "D1", "D#1"];

pub const INSTRUMENT_CHOICES: [&str; 13] =
    ["Piano", "Synth", "Bass", "Electric Keys", "Harp & Mallets", "Drums", "Guitar", "Strings", "Winds", "Brass", "Ethnic", "Beat", "Folk"];

fn choice(s: &str) -> Vec<String> {
    s.split('|').map(str::to_string).collect()
}

fn strings(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_string()).collect()
}

/// Shared, then m1..m6, in a fixed order. Defaults are the Amorph graph's init: values.
pub fn host_params() -> &'static [HostParam] {
    static LIST: OnceLock<Vec<HostParam>> = OnceLock::new();
    LIST.get_or_init(build_host_params)
}

fn build_host_params() -> Vec<HostParam> {
    use HostParamKind::{Bool, Choice, Int};
    let p = |id: String, name: String, kind: HostParamKind, min: i32, max: i32, def: i32, choices: Vec<String>| HostParam {
        id,
        name,
        kind,
        min,
        max,
        def,
        choices,
    };
    let mut v = Vec::with_capacity(HOST_PARAMS);
    v.push(p("key".into(), "Key".into(), Choice, 0, 11, 0, choice("C|C#|D|D#|E|F|F#|G|G#|A|A#|B")));
    v.push(p("scale".into(), "Scale".into(), Choice, 0, 1, 0, choice("Major|Minor")));
    // The song (sections, chords, arrangement) is not automation: it lives in the plugin state (Song.h).
    v.push(p("take".into(), "Take".into(), Int, 1, 16, 1, Vec::new()));

    let targets: Vec<String> = (1..=32).map(|i| format!("Target {i}")).collect(); // index = Library slot; the UI shows names

    // Per musician, from the Band graph's mK_* init: values (DSP.txt 2527-2670), except the Feel sliders:
    // every musician starts as its player is written (Swing / Humanize / Dynamics / Fills 50%, Note length
    // 100%, Velocity 96 = the written velocities). The engine's own defaults (Band.h) stay Amorph's.
    // instrument, player, idea, energy, swing, humanize, dynamics, fills, respect, octave, length,
    // velocity, channel, target, ksoctave, speed, on
    const D: [[i32; 17]; 6] = [
        [0, 1, 3, 1, 50, 50, 50, 50, 1, -1, 100, 96, 1, 4, 0, 0, 1],
        [5, 1, 2, 1, 50, 50, 50, 50, 1, -2, 100, 96, 2, 1, 0, 0, 1],
        [2, 7, 2, 1, 50, 50, 50, 50, 1, 0, 100, 96, 3, 2, 0, 0, 1],
        [7, 1, 0, 4, 50, 50, 50, 50, 1, 2, 100, 96, 4, 10, -1, 1, 1],
        [9, 1, 2, 4, 50, 50, 50, 50, 1, 0, 100, 96, 5, 10, 0, 0, 1],
        [0, 0, 0, 3, 50, 50, 50, 50, 0, 0, 100, 96, 6, 0, 0, 1, 0],
    ];
    for (k, m) in D.iter().enumerate() {
        let pre = format!("m{}_", k + 1);
        let n = format!("M{} ", k + 1);
        let [
            instrument,
            _player,
            _idea,
            energy,
            swing,
            humanize,
            dynamics,
            fills,
            respect,
            octave,
            length,
            velocity,
            channel,
            target,
            ksoctave,
            speed,
            on,
        ] = *m;
        let id = |f: &str| format!("{pre}{f}");
        let nm = |f: &str| format!("{n}{f}");
        v.push(p(id("instrument"), nm("Instrument"), Choice, 0, INSTRUMENT_CHOICES.len() as i32 - 1, instrument, strings(&INSTRUMENT_CHOICES)));
        v.push(p(id("energy"), nm("Energy"), Int, 1, 4, energy, Vec::new()));
        v.push(p(id("swing"), nm("Swing"), Int, 0, 100, swing, Vec::new()));
        v.push(p(id("humanize"), nm("Humanize"), Int, 0, 100, humanize, Vec::new()));
        v.push(p(id("dynamics"), nm("Dynamics"), Int, 0, 100, dynamics, Vec::new()));
        v.push(p(id("fills"), nm("Fills"), Int, 0, 100, fills, Vec::new()));
        v.push(p(id("respect"), nm("Respect Chords"), Bool, 0, 1, respect, Vec::new()));
        v.push(p(id("octave"), nm("Octave Shift"), Int, -2, 2, octave, Vec::new()));
        v.push(p(id("length"), nm("Length"), Int, 10, 100, length, Vec::new()));
        v.push(p(id("velocity"), nm("Velocity"), Int, 1, 127, velocity, Vec::new()));
        v.push(p(id("channel"), nm("MIDI Channel"), Int, 1, 16, channel, Vec::new()));
        v.push(p(id("target"), nm("Target"), Choice, 0, 31, target, targets.clone()));
        v.push(p(id("ksoctave"), nm("Keyswitch Octave"), Int, -2, 2, ksoctave, Vec::new()));
        v.push(p(
            id("swaminst"),
            nm("SWAM Instrument"),
            Choice,
            0,
            12,
            0,
            choice("Violin|Viola|Cello|Double Bass|Trumpet|Trombone|Horn|Tuba|Flute|Clarinet|Oboe|Bassoon|Alto Sax"),
        ));
        v.push(p(id("voice"), nm("Voice"), Choice, 0, 4, 0, choice("Auto|Top|2nd|3rd|Bass")));
        v.push(p(id("speed"), nm("Speed"), Choice, 0, 2, speed, choice("Half|Normal|Double")));
        v.push(p(id("enabled"), nm("Enabled"), Bool, 0, 1, on, Vec::new()));
        v.push(p(id("mute"), nm("Mute"), Bool, 0, 1, 0, Vec::new()));
        v.push(p(id("art_long"), nm("Art Long"), Choice, 0, 15, 0, strings(&ART_KEY_NAMES)));
        v.push(p(id("art_short"), nm("Art Short"), Choice, 0, 15, 1, strings(&ART_KEY_NAMES)));
        v.push(p(id("art_pluck"), nm("Art Pluck"), Choice, 0, 15, 2, strings(&ART_KEY_NAMES)));
        v.push(p(id("art_trem"), nm("Art Tremolo"), Choice, 0, 15, 3, strings(&ART_KEY_NAMES)));
    }
    // Added after the original list so every earlier parameter keeps its index.
    for k in 1..=6 {
        v.push(p(format!("m{k}_ample"), format!("M{k} Ample"), Choice, 0, 2, 0, choice("Player's choice|Strummer|Notes")));
    }
    for k in 1..=6 {
        // Natural feel: how much of the player's groove (50 = as written)
        v.push(p(format!("m{k}_groove"), format!("M{k} Groove"), Int, 0, 100, 50, Vec::new()));
    }
    for k in 1..=6 {
        // Ample guitar: Chord Detect, or Ample's 12 chord slots (the Strummer chord sheet)
        v.push(p(format!("m{k}_amplechords"), format!("M{k} Ample Chords"), Choice, 0, 1, 0, choice("Detect|Slots")));
    }
    for k in 1..=6 {
        // Indiginus Mandolin / Ukulele / Resonator: own strums, or the library's one-key chord strums
        v.push(p(format!("m{k}_pluckchords"), format!("M{k} Plucked Chords"), Choice, 0, 1, 0, choice("Own strums|Library chord keys")));
    }
    v
}

/// `findHostParam (id)`, as an index into [`host_params`].
pub fn host_param_index(id: &str) -> Option<usize> {
    host_params().iter().position(|p| p.id == id)
}

/// `findHostParam (id)`'s range, for reading song files (`song_file::HostParamRange`).
pub fn host_param_range(id: &str) -> Option<(f32, f32)> {
    let p = host_params().get(host_param_index(id)?)?;
    Some((p.min as f32, p.max as f32))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackedKind {
    Direct,
    EnabledToOn,
    Mute,
    Artmap,
}

/// One engine endpoint packInto produces, and how it is made from the host values (indices into host_params()).
#[derive(Clone, Debug, PartialEq)]
pub struct PackedEndpoint {
    pub name: String,
    pub kind: PackedKind,
    pub src: [usize; 4],
}

/// The engine endpoints packInto produces, in order, and how each is made from the host values. Built once;
/// [`pack_into`] then allocates nothing.
pub fn packed_endpoints() -> &'static [PackedEndpoint] {
    static TABLE: OnceLock<Vec<PackedEndpoint>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let defs = host_params();
        let index_of = |id: &str| defs.iter().position(|d| d.id == id).unwrap_or(0);
        let mut t = Vec::with_capacity(PACKED);
        for (i, d) in defs.iter().enumerate() {
            let id = d.id.as_str();
            let b = id.as_bytes();
            if b.len() > 3 && b[0] == b'm' && b[2] == b'_' {
                let (pre, field) = id.split_at(3);
                if field == "enabled" {
                    t.push(PackedEndpoint { name: format!("{pre}on"), kind: PackedKind::EnabledToOn, src: [i, 0, 0, 0] });
                    continue;
                }
                if field == "art_long" {
                    let src = [i, index_of(&format!("{pre}art_short")), index_of(&format!("{pre}art_pluck")), index_of(&format!("{pre}art_trem"))];
                    t.push(PackedEndpoint { name: format!("{pre}artmap"), kind: PackedKind::Artmap, src });
                    continue;
                }
                if field == "art_short" || field == "art_pluck" || field == "art_trem" {
                    continue;
                }
                if field == "mute" {
                    t.push(PackedEndpoint { name: id.to_string(), kind: PackedKind::Mute, src: [i, 0, 0, 0] });
                    continue;
                }
            }
            t.push(PackedEndpoint { name: id.to_string(), kind: PackedKind::Direct, src: [i, 0, 0, 0] });
        }
        t
    })
}

/// `juce::roundToInt` on a float: round half to even (the 6755399441055744.0 trick).
pub(crate) fn round_to_int(v: f32) -> i32 {
    f64::from(v).round_ties_even() as i32
}

/// host[i] = real value of host_params()[i]; out[j] = value of packed_endpoints()[j]. No allocation.
pub fn pack_into(host: &[f32; HOST_PARAMS], out: &mut [f32; PACKED]) {
    let get = |i: usize| host.get(i).copied().unwrap_or(0.0);
    for (o, e) in out.iter_mut().zip(packed_endpoints()) {
        *o = match e.kind {
            PackedKind::Direct => get(e.src[0]),
            PackedKind::EnabledToOn | PackedKind::Mute => {
                if get(e.src[0]) >= 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            PackedKind::Artmap => {
                let mut map = 0i32;
                for (s, &src) in e.src.iter().enumerate() {
                    map |= round_to_int(get(src)).clamp(0, 15) << (4 * s);
                }
                map as f32
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_order_match_ms() {
        assert_eq!(host_params().len(), HOST_PARAMS);
        assert_eq!(packed_endpoints().len(), PACKED);
        // Parameters.h's own check (feeltest): the Groove parameters come after the Ample switches.
        assert_eq!(host_params()[3 + 6 * 24 - 1].id, "m6_groove");
        assert_eq!(host_params()[3 + 6 * 23].id, "m1_groove");
        assert_eq!(host_param_index("m2_target"), Some(3 + 22 + 11));
        assert_eq!(host_params()[3 + 22 + 11].def, 1);
        let names: Vec<&str> = packed_endpoints().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(&names[..4], &["key", "scale", "take", "m1_instrument"]);
        assert!(names.contains(&"m1_on") && names.contains(&"m6_artmap") && !names.contains(&"m1_enabled"));
        assert_eq!(names.last(), Some(&"m6_pluckchords"));
    }

    #[test]
    fn pack_folds_enabled_mute_and_artmap() {
        let mut host = [0.0f32; HOST_PARAMS];
        for (i, p) in host_params().iter().enumerate() {
            host[i] = p.def as f32;
        }
        let mut out = [0.0f32; PACKED];
        pack_into(&host, &mut out);
        let at = |n: &str| packed_endpoints().iter().position(|e| e.name == n).map(|i| out[i]);
        assert_eq!(at("m1_on"), Some(1.0));
        assert_eq!(at("m6_on"), Some(0.0));
        assert_eq!(at("m1_artmap"), Some((1 << 4 | 2 << 8 | 3 << 12) as f32)); // 12816, the engine default
        assert_eq!(at("m3_instrument"), Some(2.0));
        assert_eq!(host_param_range("m1_length"), Some((10.0, 100.0)));
        assert_eq!(host_param_range("nope"), None);
    }

    #[test]
    fn factory_songs_read_strictly_with_the_host_ranges() {
        // song_file_from_json_checked fed from this table (MS SongFile reads a band's params against findHostParam).
        let mut n = 0;
        for (path, bytes) in crate::library::embedded::files() {
            if path.starts_with("Songs/Factory/") && path.ends_with(".song.json") {
                let text = String::from_utf8_lossy(bytes);
                let r = crate::song_file::song_file_from_json_checked(text.trim_start_matches('\u{feff}'), host_param_range);
                assert!(r.is_ok(), "{path}: {r:?}");
                n += 1;
            }
        }
        assert_eq!(n, 5);
    }
}
