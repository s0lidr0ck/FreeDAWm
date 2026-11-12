//! Rez's factory presets: FreeDAWm's own, written for Rez (starting points, by the job at hand).
//!
//! Each preset lists only what it changes from Rez's defaults; the JSON a preset menu recalls
//! (`{"name", "group", "desc", "values"}`, as the FireStation engines' presets) carries every
//! parameter, so recalling one never leaves a setting of the previous preset behind.

use super::REZ_INFO;
use std::sync::OnceLock;

/// One factory preset: what it is for, and the parameters it sets away from their defaults.
pub struct RezPreset {
    pub name: &'static str,
    pub group: &'static str,
    pub desc: &'static str,
    pub set: &'static [(&'static str, f32)],
}

const fn preset(group: &'static str, name: &'static str, desc: &'static str, set: &'static [(&'static str, f32)]) -> RezPreset {
    RezPreset { name, group, desc, set }
}

const HARD: f32 = 1.0;
const MID_SIDE: f32 = 1.0;

/// The presets, in menu order.
pub static REZ_PRESETS: [RezPreset; 30] = [
    // ---------------------------------------------------------------- Start here
    preset("Start Here", "Even Hand", "A light touch across the whole range. Raise Depth until you hear it, then back off.", &[("depth", 55.0)]),
    preset(
        "Start Here",
        "Find It With Delta",
        "Delta is on: you hear only what Rez removes. Shape the bands until that is all problem, then switch Delta off.",
        &[("depth", 65.0), ("delta", 1.0)],
    ),
    preset(
        "Start Here",
        "Firm Grip",
        "Hard mode for a source that really rings. It reaches into the low mids too; Mix keeps some of the original.",
        &[("mode", HARD), ("depth", 55.0), ("mix", 75.0)],
    ),
    // ---------------------------------------------------------------- Vocals
    preset(
        "Vocals",
        "Soften The Edge",
        "The 2.5 to 5 kHz bite of a pushed vocal, eased only on the notes that have it.",
        &[("depth", 62.0), ("low", 1200.0), ("high", 9000.0), ("b3_freq", 3400.0), ("b3_gain", 5.0), ("b3_q", 0.9), ("release", 30.0)],
    ),
    preset(
        "Vocals",
        "Ess Catcher",
        "Sibilance only: fast, narrow to the top, the body of the voice left alone.",
        &[
            ("depth", 66.0),
            ("sharpness", 55.0),
            ("low", 2000.0),
            ("b4_freq", 7200.0),
            ("b4_gain", 7.0),
            ("b4_q", 1.2),
            ("b3_on", 0.0),
            ("attack", 3.0),
            ("release", 45.0),
        ],
    ),
    preset(
        "Vocals",
        "Out Of The Nose",
        "Honk and boxiness around 600 Hz to 1.2 kHz. Hard mode, because Rez goes easy down there otherwise.",
        &[
            ("mode", HARD),
            ("depth", 52.0),
            ("low", 300.0),
            ("high", 2500.0),
            ("b2_freq", 850.0),
            ("b2_gain", 6.0),
            ("b2_q", 0.8),
            ("mix", 80.0),
            ("release", 60.0),
        ],
    ),
    preset(
        "Vocals",
        "Close Mic Chest",
        "The low-mid bloom of a voice worked right on the microphone.",
        &[
            ("mode", HARD),
            ("depth", 48.0),
            ("low", 120.0),
            ("high", 1000.0),
            ("b1_freq", 240.0),
            ("b1_gain", 9.0),
            ("b1_q", 0.7),
            ("attack", 30.0),
            ("release", 120.0),
            ("mix", 70.0),
        ],
    ),
    preset(
        "Vocals",
        "Stack Polish",
        "Backing vocals as one: broad, gentle, slow enough not to chatter.",
        &[("depth", 58.0), ("sharpness", 32.0), ("selectivity", 20.0), ("low", 400.0), ("attack", 30.0), ("release", 140.0)],
    ),
    // ---------------------------------------------------------------- Drums
    preset(
        "Drums",
        "Cymbal Wash",
        "Overheads that hiss and pile up: the top smoothed, the kit under it untouched.",
        &[("depth", 64.0), ("sharpness", 58.0), ("low", 2000.0), ("b4_freq", 9000.0), ("b4_gain", 4.0), ("release", 70.0)],
    ),
    preset(
        "Drums",
        "Snare Ring",
        "One singing overtone and nothing else: narrow, picky, firm.",
        &[
            ("mode", HARD),
            ("depth", 53.0),
            ("sharpness", 88.0),
            ("selectivity", 72.0),
            ("low", 250.0),
            ("high", 4000.0),
            ("release", 90.0),
            ("mix", 85.0),
        ],
    ),
    preset(
        "Drums",
        "Kick Cardboard",
        "The boxy middle of a kick, so the click and the weight meet.",
        &[
            ("mode", HARD),
            ("depth", 52.0),
            ("low", 180.0),
            ("high", 1500.0),
            ("b1_freq", 380.0),
            ("b1_gain", 8.0),
            ("b1_q", 1.0),
            ("attack", 8.0),
            ("release", 80.0),
        ],
    ),
    preset(
        "Drums",
        "Room Glue",
        "A room microphone's hard reflections taken down a size: wide cuts, slow release.",
        &[("depth", 60.0), ("sharpness", 26.0), ("selectivity", 18.0), ("attack", 25.0), ("release", 220.0), ("mix", 80.0)],
    ),
    preset(
        "Drums",
        "Hat Bleed In The Snare",
        "The top of a snare microphone calmed between hits, with Mix keeping the crack.",
        &[("depth", 68.0), ("low", 2000.0), ("sharpness", 50.0), ("attack", 15.0), ("release", 35.0), ("mix", 65.0)],
    ),
    // ---------------------------------------------------------------- Guitars
    preset(
        "Guitars",
        "Amp Fizz",
        "The grainy top of a distorted guitar, 4 to 8 kHz, without dulling the pick.",
        &[
            ("depth", 64.0),
            ("sharpness", 52.0),
            ("low", 2000.0),
            ("high", 12_000.0),
            ("b4_freq", 5500.0),
            ("b4_gain", 5.0),
            ("b4_q", 0.9),
            ("release", 50.0),
        ],
    ),
    preset(
        "Guitars",
        "Acoustic Boom",
        "The body note that jumps out of a strummed acoustic. Follows the chords instead of thinning them all.",
        &[
            ("mode", HARD),
            ("depth", 47.0),
            ("low", 90.0),
            ("high", 1000.0),
            ("b1_freq", 190.0),
            ("b1_gain", 10.0),
            ("b1_q", 0.9),
            ("attack", 20.0),
            ("release", 150.0),
            ("mix", 75.0),
        ],
    ),
    preset(
        "Guitars",
        "Pick And Squeak",
        "String noise and a hard pick on a close-miked acoustic.",
        &[("depth", 62.0), ("sharpness", 60.0), ("low", 1500.0), ("b3_freq", 3000.0), ("b3_gain", 4.0), ("attack", 5.0), ("release", 40.0)],
    ),
    preset(
        "Guitars",
        "Wall Of Rhythm",
        "A guitar bus that has grown a hard midrange: both sides treated as one.",
        &[
            ("depth", 58.0),
            ("sharpness", 38.0),
            ("selectivity", 28.0),
            ("low", 700.0),
            ("high", 9000.0),
            ("b3_freq", 2600.0),
            ("b3_gain", 3.0),
            ("attack", 25.0),
            ("release", 110.0),
        ],
    ),
    // ---------------------------------------------------------------- Bass
    preset(
        "Bass",
        "Even Notes",
        "The one or two notes that bloom on every bass. Slow and Hard, so it rides the note, not the pluck.",
        &[
            ("mode", HARD),
            ("depth", 46.0),
            ("sharpness", 60.0),
            ("low", 40.0),
            ("high", 1000.0),
            ("b1_freq", 110.0),
            ("b1_gain", 12.0),
            ("b1_q", 0.6),
            ("b2_freq", 260.0),
            ("b2_gain", 8.0),
            ("attack", 40.0),
            ("release", 180.0),
            ("mix", 70.0),
        ],
    ),
    preset(
        "Bass",
        "Fret Clank",
        "Finger and fret noise up top, the low end out of range and untouched.",
        &[("depth", 64.0), ("low", 900.0), ("high", 8000.0), ("b3_freq", 2400.0), ("b3_gain", 5.0), ("attack", 4.0), ("release", 45.0)],
    ),
    // ---------------------------------------------------------------- Keys
    preset(
        "Keys",
        "Hammer Softener",
        "A bright piano's hard hammer strike, with the ring after it left in.",
        &[("depth", 60.0), ("sharpness", 50.0), ("low", 900.0), ("b3_freq", 2800.0), ("b3_gain", 4.0), ("attack", 3.0), ("release", 35.0)],
    ),
    preset(
        "Keys",
        "Electric Piano Bark",
        "The midrange bark of an electric piano played hard, eased so chords stay round.",
        &[
            ("mode", HARD),
            ("depth", 45.0),
            ("low", 350.0),
            ("high", 3500.0),
            ("b2_freq", 1100.0),
            ("b2_gain", 5.0),
            ("b2_q", 0.8),
            ("mix", 70.0),
            ("release", 90.0),
        ],
    ),
    // ---------------------------------------------------------------- Strings & Brass
    preset(
        "Strings & Brass",
        "Bow Edge",
        "Rosin and edge on close strings: slow to act, slow to let go.",
        &[
            ("depth", 60.0),
            ("sharpness", 42.0),
            ("low", 1500.0),
            ("high", 10_000.0),
            ("b3_freq", 3600.0),
            ("b3_gain", 4.0),
            ("attack", 35.0),
            ("release", 160.0),
        ],
    ),
    preset(
        "Strings & Brass",
        "Brass Blare",
        "A horn section at full volume, 1.5 to 4 kHz, held back only on the loud phrases.",
        &[
            ("depth", 54.0),
            ("sharpness", 40.0),
            ("selectivity", 30.0),
            ("low", 800.0),
            ("high", 7000.0),
            ("b3_freq", 2300.0),
            ("b3_gain", 5.0),
            ("b3_q", 0.8),
            ("attack", 15.0),
            ("release", 100.0),
        ],
    ),
    // ---------------------------------------------------------------- Synths
    preset(
        "Synths",
        "Filter Sweep Tamer",
        "A resonant filter that screams at the top of its sweep. Narrow and quick enough to follow it.",
        &[("depth", 68.0), ("sharpness", 72.0), ("selectivity", 60.0), ("attack", 5.0), ("release", 40.0)],
    ),
    preset(
        "Synths",
        "Pad Smoother",
        "A thick pad made easier to sit things on: broad, slow, never obvious.",
        &[("depth", 60.0), ("sharpness", 28.0), ("selectivity", 20.0), ("attack", 50.0), ("release", 260.0), ("mix", 80.0)],
    ),
    // ---------------------------------------------------------------- Mix Bus
    preset(
        "Mix Bus",
        "Quiet Polish",
        "A finished mix, a couple of dB at most where it turns hard. Linked, slow, mostly out of the way.",
        &[("depth", 48.0), ("sharpness", 40.0), ("selectivity", 30.0), ("low", 500.0), ("attack", 40.0), ("release", 150.0), ("mix", 60.0)],
    ),
    preset(
        "Mix Bus",
        "Sides Only",
        "Mid/side with the balance all the way to the sides: wide cymbals and guitars smoothed, the centre untouched.",
        &[("stereo", MID_SIDE), ("link", 0.0), ("balance", 100.0), ("depth", 60.0), ("low", 1500.0), ("attack", 25.0), ("release", 110.0)],
    ),
    preset(
        "Mix Bus",
        "Centre Only",
        "Mid/side with the balance all the way to the middle: the vocal and snare eased, the width left as it is.",
        &[
            ("stereo", MID_SIDE),
            ("link", 0.0),
            ("balance", -100.0),
            ("depth", 58.0),
            ("low", 1000.0),
            ("high", 9000.0),
            ("attack", 20.0),
            ("release", 90.0),
        ],
    ),
    // ---------------------------------------------------------------- With A Key
    preset(
        "With A Key",
        "Make Room For The Key",
        "Pick a sidechain key in the Mix window (a vocal, say): this track ducks only where the key is strong.",
        &[("depth", 70.0), ("sharpness", 26.0), ("selectivity", 12.0), ("low", 250.0), ("attack", 10.0), ("release", 120.0)],
    ),
    preset(
        "With A Key",
        "Kick Through The Bass",
        "On the bass, keyed from the kick: the notes give way where the kick lands. Hard, so it reaches the low end.",
        &[
            ("mode", HARD),
            ("depth", 55.0),
            ("sharpness", 30.0),
            ("selectivity", 10.0),
            ("low", 30.0),
            ("high", 1000.0),
            ("b1_freq", 80.0),
            ("b1_gain", 12.0),
            ("b1_q", 0.6),
            ("attack", 2.0),
            ("release", 90.0),
        ],
    ),
];

/// A JSON string for `text` (quotes and backslashes escaped; the presets' own text has no
/// control characters).
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// One preset as the JSON a preset menu recalls: every parameter, the preset's own values over
/// the defaults.
fn json_of(p: &RezPreset) -> String {
    let values: Vec<String> = REZ_INFO
        .params
        .iter()
        .map(|info| {
            let v = p.set.iter().find(|(id, _)| *id == info.id).map_or(info.default, |(_, v)| info.clamp(*v));
            format!("{}:{v}", quoted(info.id))
        })
        .collect();
    format!("{{\"name\":{},\"group\":{},\"desc\":{},\"values\":{{{}}}}}", quoted(p.name), quoted(p.group), quoted(p.desc), values.join(","))
}

/// The factory presets as `(name, json)` pairs, in menu order (built once).
pub fn factory_presets() -> &'static [(&'static str, &'static str)] {
    static BUILT: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    BUILT.get_or_init(|| REZ_PRESETS.iter().map(|p| (p.name, &*Box::leak(json_of(p).into_boxed_str()))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Plugin;
    use crate::plugins::fs::FsPreset;
    use crate::plugins::rez::Rez;

    #[test]
    fn every_preset_is_well_formed() {
        let mut names = std::collections::HashSet::new();
        for p in &REZ_PRESETS {
            assert!(names.insert(p.name.to_lowercase()), "{} is listed twice", p.name);
            assert!(!p.group.is_empty() && !p.desc.is_empty(), "{}", p.name);
            let mut ids = std::collections::HashSet::new();
            for (id, v) in p.set {
                let info = REZ_INFO.param(id).unwrap_or_else(|| panic!("{}: no parameter {id}", p.name));
                assert!(ids.insert(*id), "{}: {id} set twice", p.name);
                assert_eq!(info.clamp(*v), *v, "{}: {id} = {v} is out of range", p.name);
            }
            // The range stays the right way round.
            let get = |id: &str| p.set.iter().find(|(k, _)| *k == id).map_or(REZ_INFO.param(id).unwrap().default, |(_, v)| *v);
            assert!(get("low") < get("high"), "{}: Low is not below High", p.name);
        }
    }

    #[test]
    fn the_json_carries_every_parameter_and_parses_back() {
        let list = factory_presets();
        assert_eq!(list.len(), REZ_PRESETS.len());
        for ((name, json), p) in list.iter().zip(&REZ_PRESETS) {
            let parsed = FsPreset::parse(json, &REZ_INFO).unwrap_or_else(|| panic!("{name} does not parse"));
            assert_eq!((parsed.name.as_str(), parsed.group.as_str()), (p.name, p.group));
            assert_eq!(parsed.values.len(), REZ_INFO.params.len(), "{name}");
            for (id, v) in p.set {
                assert_eq!(parsed.values.get(*id), Some(v), "{name}: {id}");
            }
            let json: serde_json::Value = serde_json::from_str(json).unwrap();
            assert_eq!(json["desc"], p.desc);
        }
        // Built once: the same strings every time.
        assert!(std::ptr::eq(list, factory_presets()));
    }

    #[test]
    fn every_preset_processes_sound_and_only_the_delta_one_inverts_it() {
        for p in &REZ_PRESETS {
            let mut r = Rez::new();
            r.prepare(48_000.0, 512, 2);
            for (id, v) in p.set {
                assert!(r.set_param(id, *v));
            }
            let mut seed = 0x9E37_79B9u32;
            let mut io: Vec<Vec<f32>> = (0..2)
                .map(|_| {
                    (0..48_000)
                        .map(|i| {
                            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                            let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                            noise * 0.2 + 0.2 * (std::f32::consts::TAU * 3000.0 * i as f32 / 48_000.0).sin()
                        })
                        .collect()
                })
                .collect();
            let input_peak = io[0].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            for start in (0..48_000).step_by(512) {
                let mut block: Vec<Vec<f32>> = io.iter().map(|c| c[start..(start + 512).min(48_000)].to_vec()).collect();
                let n = block[0].len();
                r.process(&mut block, n);
                for (c, b) in io.iter_mut().zip(&block) {
                    c[start..start + n].copy_from_slice(b);
                }
            }
            let out_peak = io[0][24_000..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(io.iter().flatten().all(|v| v.is_finite()), "{}", p.name);
            assert!(out_peak < input_peak * 1.5, "{} made it louder: {out_peak} from {input_peak}", p.name);
            assert!(out_peak > 0.0, "{} is silent", p.name);
        }
    }
}
