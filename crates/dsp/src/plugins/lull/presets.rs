//! Lull's factory presets: FreeDAWm's own, written for Lull (starting points, by the job at hand).
//!
//! Each preset lists only what it changes from Lull's defaults; the JSON a preset menu recalls
//! (`{"name", "group", "desc", "values"}`, as the FireStation engines' presets) carries every
//! parameter, so recalling one never leaves a setting of the previous preset behind. The
//! threshold is always a starting point: set it to the recording.

use super::LULL_INFO;
use std::sync::OnceLock;

/// One factory preset: what it is for, and the parameters it sets away from their defaults.
pub struct LullPreset {
    pub name: &'static str,
    pub group: &'static str,
    pub desc: &'static str,
    pub set: &'static [(&'static str, f32)],
}

const fn preset(group: &'static str, name: &'static str, desc: &'static str, set: &'static [(&'static str, f32)]) -> LullPreset {
    LullPreset { name, group, desc, set }
}

const GATE: f32 = 1.0;
const TRIGGER: f32 = 1.0;

/// The presets, in menu order.
pub static LULL_PRESETS: [LullPreset; 26] = [
    // ---------------------------------------------------------------- Start here
    preset(
        "Start Here",
        "Gentle Expander",
        "Two to one under the threshold, 12 dB at most. Set the threshold just under the quietest thing you want to keep.",
        &[("range", 12.0)],
    ),
    preset(
        "Start Here",
        "Plain Gate",
        "On or off, 40 dB down when closed, with a little hold. Set the threshold between the source and the noise.",
        &[("mode", GATE), ("range", 40.0), ("hold", 40.0), ("release", 150.0)],
    ),
    preset(
        "Start Here",
        "Hear The Detector",
        "Listen is on: you hear what the detector hears. Set the two filters until it is mostly the source, then switch Listen off.",
        &[("listen", 1.0), ("sc_hp", 150.0), ("sc_lp", 6000.0)],
    ),
    // ---------------------------------------------------------------- Vocals
    preset(
        "Vocals",
        "Between The Lines",
        "Room and headphone bleed pushed back between phrases, slowly enough that breaths still sound like breaths.",
        &[
            ("threshold", -42.0),
            ("range", 10.0),
            ("ratio", 1.8),
            ("knee", 12.0),
            ("attack", 2.0),
            ("hold", 60.0),
            ("release", 350.0),
            ("sc_hp", 120.0),
        ],
    ),
    preset(
        "Vocals",
        "Stage Vocal",
        "A singer's microphone on a loud stage: the band falls away whenever they are not singing. The detector ignores drums' lows and cymbals' highs.",
        &[
            ("threshold", -30.0),
            ("range", 14.0),
            ("ratio", 2.5),
            ("knee", 9.0),
            ("attack", 1.0),
            ("hold", 80.0),
            ("release", 250.0),
            ("sc_hp", 180.0),
            ("sc_lp", 5000.0),
        ],
    ),
    preset(
        "Vocals",
        "Stage Vocal, Guarded",
        "As Stage Vocal, with a key: pick the loudest thing near the singer (the guitar amp, the snare) and its bleed cannot open the microphone.",
        &[
            ("threshold", -30.0),
            ("range", 14.0),
            ("ratio", 2.5),
            ("knee", 9.0),
            ("hold", 80.0),
            ("release", 250.0),
            ("sc_hp", 180.0),
            ("sc_lp", 5000.0),
            ("guard", 12.0),
            ("guard_delay", 6.0),
        ],
    ),
    preset(
        "Vocals",
        "Backing Singers",
        "Microphones that are open all night and sung into now and then: firm, but slow to close.",
        &[
            ("threshold", -34.0),
            ("range", 18.0),
            ("ratio", 3.0),
            ("knee", 8.0),
            ("hold", 150.0),
            ("release", 400.0),
            ("sc_hp", 200.0),
            ("sc_lp", 6000.0),
        ],
    ),
    preset(
        "Vocals",
        "Breath Tamer",
        "Only a few dB, only on the breaths between words.",
        &[
            ("threshold", -36.0),
            ("range", 6.0),
            ("ratio", 1.6),
            ("knee", 14.0),
            ("attack", 5.0),
            ("hold", 30.0),
            ("release", 180.0),
            ("sc_hp", 300.0),
        ],
    ),
    preset(
        "Vocals",
        "Spoken Word Hush",
        "Speech over a quiet room: the room is taken down between sentences without the pumping a gate would bring.",
        &[
            ("threshold", -46.0),
            ("range", 12.0),
            ("ratio", 2.0),
            ("knee", 12.0),
            ("attack", 3.0),
            ("hold", 120.0),
            ("release", 450.0),
            ("sc_hp", 100.0),
        ],
    ),
    // ---------------------------------------------------------------- Drums
    preset(
        "Drums",
        "Kick, Tight",
        "Opens on the beater, closes before the next hit. The detector hears only the low end, so the snare cannot open it.",
        &[
            ("mode", GATE),
            ("threshold", -24.0),
            ("range", 40.0),
            ("knee", 2.0),
            ("hysteresis", 4.0),
            ("attack", 0.1),
            ("hold", 60.0),
            ("release", 120.0),
            ("lookahead", 2.0),
            ("sc_lp", 500.0),
        ],
    ),
    preset(
        "Drums",
        "Snare, Clean",
        "The snare and nothing between: the detector skips the kick below and the hat above.",
        &[
            ("mode", GATE),
            ("threshold", -22.0),
            ("range", 30.0),
            ("knee", 2.0),
            ("hysteresis", 5.0),
            ("attack", 0.1),
            ("hold", 70.0),
            ("release", 160.0),
            ("lookahead", 2.0),
            ("sc_hp", 180.0),
            ("sc_lp", 4000.0),
        ],
    ),
    preset(
        "Drums",
        "Snare, Natural",
        "An expander instead of a gate: the bleed goes down 12 dB and the ghost notes survive.",
        &[
            ("threshold", -26.0),
            ("range", 12.0),
            ("ratio", 3.0),
            ("knee", 6.0),
            ("attack", 0.2),
            ("hold", 50.0),
            ("release", 200.0),
            ("sc_hp", 180.0),
            ("sc_lp", 4000.0),
        ],
    ),
    preset(
        "Drums",
        "Toms",
        "Closed most of the song, then open for the whole ring of the drum.",
        &[
            ("mode", GATE),
            ("threshold", -20.0),
            ("range", 50.0),
            ("knee", 3.0),
            ("hysteresis", 6.0),
            ("attack", 0.2),
            ("hold", 180.0),
            ("release", 450.0),
            ("lookahead", 3.0),
            ("sc_hp", 70.0),
            ("sc_lp", 1500.0),
        ],
    ),
    preset(
        "Drums",
        "Hi-Hat Mic",
        "The hat microphone without the snare in it: the detector hears only the top.",
        &[("threshold", -30.0), ("range", 15.0), ("ratio", 2.5), ("attack", 0.5), ("hold", 30.0), ("release", 120.0), ("sc_hp", 1800.0)],
    ),
    preset(
        "Drums",
        "Room, Chopped",
        "A room microphone gated short for the old trick: all crack, no tail.",
        &[
            ("mode", GATE),
            ("threshold", -18.0),
            ("range", 60.0),
            ("knee", 0.0),
            ("attack", 0.1),
            ("hold", 90.0),
            ("release", 40.0),
            ("lookahead", 1.0),
        ],
    ),
    preset(
        "Drums",
        "Kick Opens The Room",
        "Trigger key: pick the kick as the key and this track (a room, a sub tone) opens only with it.",
        &[
            ("mode", GATE),
            ("key", TRIGGER),
            ("threshold", -24.0),
            ("range", 60.0),
            ("attack", 0.5),
            ("hold", 80.0),
            ("release", 180.0),
            ("sc_lp", 400.0),
        ],
    ),
    // ---------------------------------------------------------------- Guitars & Bass
    preset(
        "Guitars & Bass",
        "Amp Hiss",
        "A high-gain amp's hiss and hum between riffs, closed fast enough to keep stops tight.",
        &[
            ("mode", GATE),
            ("threshold", -44.0),
            ("range", 50.0),
            ("knee", 4.0),
            ("hysteresis", 5.0),
            ("attack", 0.3),
            ("hold", 40.0),
            ("release", 110.0),
        ],
    ),
    preset(
        "Guitars & Bass",
        "Let It Ring",
        "The same amp noise, but released slowly so held chords die away on their own.",
        &[("threshold", -48.0), ("range", 24.0), ("ratio", 3.0), ("knee", 10.0), ("attack", 1.0), ("hold", 120.0), ("release", 700.0)],
    ),
    preset(
        "Guitars & Bass",
        "Staccato Stops",
        "A gate keyed tight to the playing for chopped rhythm parts.",
        &[
            ("mode", GATE),
            ("threshold", -34.0),
            ("range", 60.0),
            ("knee", 1.0),
            ("hysteresis", 3.0),
            ("attack", 0.1),
            ("hold", 15.0),
            ("release", 35.0),
            ("lookahead", 1.0),
        ],
    ),
    preset(
        "Guitars & Bass",
        "Acoustic, Quiet Room",
        "A few dB off the room and the chair creaks between phrases.",
        &[
            ("threshold", -48.0),
            ("range", 8.0),
            ("ratio", 1.7),
            ("knee", 14.0),
            ("attack", 3.0),
            ("hold", 100.0),
            ("release", 500.0),
            ("sc_hp", 80.0),
        ],
    ),
    preset(
        "Guitars & Bass",
        "Bass Finger Noise",
        "The slides and clicks between notes, taken down without shortening the notes.",
        &[
            ("threshold", -38.0),
            ("range", 12.0),
            ("ratio", 2.5),
            ("knee", 8.0),
            ("attack", 1.0),
            ("hold", 90.0),
            ("release", 260.0),
            ("sc_lp", 2500.0),
        ],
    ),
    // ---------------------------------------------------------------- Stage & Post
    preset(
        "Stage & Post",
        "Horn Section Mic",
        "A horn player's microphone between entries, guarded from the drums behind them when a key is picked.",
        &[
            ("threshold", -32.0),
            ("range", 16.0),
            ("ratio", 2.5),
            ("knee", 8.0),
            ("hold", 120.0),
            ("release", 300.0),
            ("sc_hp", 250.0),
            ("guard", 9.0),
            ("guard_delay", 9.0),
        ],
    ),
    preset(
        "Stage & Post",
        "Panel Discussion",
        "One microphone of several at a table: down, not off, when its speaker is quiet.",
        &[
            ("threshold", -40.0),
            ("range", 15.0),
            ("ratio", 2.2),
            ("knee", 10.0),
            ("attack", 2.0),
            ("hold", 250.0),
            ("release", 500.0),
            ("sc_hp", 150.0),
            ("sc_lp", 5000.0),
        ],
    ),
    preset(
        "Stage & Post",
        "Dialogue Noise Floor",
        "Location dialogue: the background eased down between lines, never to silence.",
        &[
            ("threshold", -44.0),
            ("range", 9.0),
            ("ratio", 1.8),
            ("knee", 14.0),
            ("attack", 3.0),
            ("hold", 150.0),
            ("release", 600.0),
            ("sc_hp", 120.0),
        ],
    ),
    preset(
        "Stage & Post",
        "Two Mics, One Voice",
        "Unlinked, on a stereo pair of spot microphones: each side closes on its own.",
        &[("link", 0.0), ("threshold", -36.0), ("range", 14.0), ("ratio", 2.5), ("knee", 8.0), ("hold", 100.0), ("release", 300.0)],
    ),
    preset(
        "Stage & Post",
        "Half Gate",
        "A gate blended half and half with the dry signal: the grip of a gate, the tail of the room.",
        &[("mode", GATE), ("threshold", -30.0), ("range", 60.0), ("hold", 60.0), ("release", 200.0), ("mix", 50.0)],
    ),
];

/// A JSON string for `text` (quotes and backslashes escaped; the presets' own text has no
/// control characters).
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// One preset as the JSON a preset menu recalls: every parameter, the preset's own values over
/// the defaults.
fn json_of(p: &LullPreset) -> String {
    let values: Vec<String> = LULL_INFO
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
    BUILT.get_or_init(|| LULL_PRESETS.iter().map(|p| (p.name, &*Box::leak(json_of(p).into_boxed_str()))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Plugin;
    use crate::plugins::fs::FsPreset;
    use crate::plugins::lull::Lull;

    #[test]
    fn every_preset_is_well_formed() {
        let mut names = std::collections::HashSet::new();
        for p in &LULL_PRESETS {
            assert!(names.insert(p.name.to_lowercase()), "{} is listed twice", p.name);
            assert!(!p.group.is_empty() && !p.desc.is_empty(), "{}", p.name);
            let mut ids = std::collections::HashSet::new();
            for (id, v) in p.set {
                let info = LULL_INFO.param(id).unwrap_or_else(|| panic!("{}: no parameter {id}", p.name));
                assert!(ids.insert(*id), "{}: {id} set twice", p.name);
                assert_eq!(info.clamp(*v), *v, "{}: {id} = {v} is out of range", p.name);
            }
            let get = |id: &str| p.set.iter().find(|(k, _)| *k == id).map_or(LULL_INFO.param(id).unwrap().default, |(_, v)| *v);
            assert!(get("sc_hp") < get("sc_lp"), "{}: the detector's filters cross", p.name);
        }
    }

    #[test]
    fn the_json_carries_every_parameter_and_parses_back() {
        let list = factory_presets();
        assert_eq!(list.len(), LULL_PRESETS.len());
        for ((name, json), p) in list.iter().zip(&LULL_PRESETS) {
            let parsed = FsPreset::parse(json, &LULL_INFO).unwrap_or_else(|| panic!("{name} does not parse"));
            assert_eq!((parsed.name.as_str(), parsed.group.as_str()), (p.name, p.group));
            assert_eq!(parsed.values.len(), LULL_INFO.params.len(), "{name}");
            for (id, v) in p.set {
                assert_eq!(parsed.values.get(*id), Some(v), "{name}: {id}");
            }
        }
        assert!(std::ptr::eq(list, factory_presets()));
    }

    #[test]
    fn every_preset_processes_sound() {
        for p in &LULL_PRESETS {
            let mut l = Lull::new();
            l.prepare(48_000.0, 512, 2);
            for (id, v) in p.set {
                assert!(l.set_param(id, *v));
            }
            // A loud second, then a quiet one.
            let x: Vec<f32> =
                (0..96_000).map(|i| (if i < 48_000 { 0.5 } else { 0.001 }) * (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin()).collect();
            let mut out = Vec::with_capacity(x.len());
            for block in x.chunks(512) {
                let mut io = vec![block.to_vec(), block.to_vec()];
                l.process(&mut io, block.len());
                out.extend_from_slice(&io[0]);
            }
            assert!(out.iter().all(|v| v.is_finite()), "{}", p.name);
            let peak = |s: &[f32]| s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            // (Listen plays the filtered detector signal, whose filters may ring a little over.)
            assert!(peak(&out) <= 0.5 * 1.15, "{} made it louder: {}", p.name, peak(&out));
            // A Trigger preset stays closed without its key; every other one passes the loud second.
            let triggered = p.set.iter().any(|(id, v)| *id == "key" && *v == TRIGGER);
            if !triggered {
                assert!(peak(&out[24_000..48_000]) > 0.2, "{} does not pass a loud signal: {}", p.name, peak(&out[24_000..48_000]));
            }
        }
    }
}
