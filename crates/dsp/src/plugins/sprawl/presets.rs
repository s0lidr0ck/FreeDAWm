//! Sprawl's factory presets: FreeDAWm's own, written for Sprawl (starting points, by the job at
//! hand).
//!
//! Each preset lists only what it changes from Sprawl's defaults; the JSON a preset menu recalls
//! (`{"name", "group", "desc", "values"}`, as the FireStation engines' presets) carries every
//! parameter, so recalling one never leaves a setting of the previous preset behind.

use super::SPRAWL_INFO;
use std::sync::OnceLock;

/// One factory preset: what it is for, and the parameters it sets away from their defaults.
pub struct SprawlPreset {
    pub name: &'static str,
    pub group: &'static str,
    pub desc: &'static str,
    pub set: &'static [(&'static str, f32)],
}

const fn preset(group: &'static str, name: &'static str, desc: &'static str, set: &'static [(&'static str, f32)]) -> SprawlPreset {
    SprawlPreset { name, group, desc, set }
}

const ON: f32 = 1.0;

/// The presets, in menu order.
pub static SPRAWL_PRESETS: [SprawlPreset; 16] = [
    // ---------------------------------------------------------------- Start Here
    preset(
        "Start Here",
        "A Little Wider",
        "Everything a quarter wider, with Mono Safe watching. Raise Width until you notice it, then back off.",
        &[("width", 125.0), ("safe", ON)],
    ),
    preset("Start Here", "Solid Low End", "Nothing changes but the lows: under 120 Hz is mono, where it translates best.", &[("b1_width", 0.0)]),
    preset(
        "Start Here",
        "Wide Top, Firm Bottom",
        "The classic shape: mono lows, the low mids as they came, more and more width toward the top.",
        &[("b1_width", 0.0), ("b3_width", 125.0), ("b4_width", 150.0), ("safe", ON)],
    ),
    preset(
        "Start Here",
        "Check In Mono",
        "Width at zero: what a single speaker will play. Step back to the previous preset to compare.",
        &[("width", 0.0)],
    ),
    // ---------------------------------------------------------------- Mix Bus
    preset(
        "Mix Bus",
        "Master Polish",
        "A master that is nearly there: mono below 100 Hz and a touch of air at the sides.",
        &[("split1", 100.0), ("split3", 7000.0), ("b1_width", 0.0), ("b4_width", 120.0), ("safe", ON)],
    ),
    preset(
        "Mix Bus",
        "Open The Mids",
        "For a mix that feels crowded in the centre: the mids spread, the lows and the very top left alone.",
        &[("b2_width", 115.0), ("b3_width", 135.0), ("safe", ON)],
    ),
    preset(
        "Mix Bus",
        "Rein It In",
        "A mix that is too wide to hold together: everything a third narrower, the lows mono.",
        &[("width", 70.0), ("b1_width", 0.0)],
    ),
    preset(
        "Mix Bus",
        "Vinyl Safe",
        "Mono up to 250 Hz and a narrower low mid, as a cutting lathe likes it.",
        &[("split1", 250.0), ("b1_width", 0.0), ("b2_width", 80.0)],
    ),
    // ---------------------------------------------------------------- Instruments
    preset(
        "Instruments",
        "Wide Pad",
        "A stereo pad or string layer spread to the edges, its low end kept out of the way.",
        &[("width", 150.0), ("b1_width", 40.0), ("safe", ON)],
    ),
    preset(
        "Instruments",
        "Overheads",
        "Drum overheads: the kick and snare body centred, the cymbals wide.",
        &[("split1", 200.0), ("split2", 1500.0), ("b1_width", 0.0), ("b2_width", 80.0), ("b4_width", 140.0), ("safe", ON)],
    ),
    preset(
        "Instruments",
        "Piano In The Room",
        "A close stereo piano pulled back from ear to ear to something a listener would hear.",
        &[("width", 75.0), ("b1_width", 60.0)],
    ),
    preset(
        "Instruments",
        "Backing Vocal Stack",
        "Stacked harmonies pushed out to leave the centre for the lead.",
        &[("split1", 180.0), ("b1_width", 50.0), ("b3_width", 150.0), ("b4_width", 160.0), ("safe", ON)],
    ),
    // ---------------------------------------------------------------- Mono Sources
    preset("Mono Sources", "Mono To Stereo", "Stereoize at half: a mono source gets sides that vanish again in mono.", &[("stereoize", 50.0)]),
    preset(
        "Mono Sources",
        "Mono Synth, Big",
        "A mono synth made wide, more so toward the top; its bass stays where it was.",
        &[("stereoize", 80.0), ("b1_width", 0.0), ("b4_width", 140.0)],
    ),
    preset(
        "Mono Sources",
        "Guitar, Off The Centre",
        "A single-mic guitar with just enough side to stop it sitting on the vocal.",
        &[("stereoize", 35.0), ("split1", 200.0), ("b1_width", 0.0)],
    ),
    preset(
        "Mono Sources",
        "Air Only",
        "Stereoize kept to the top band: the body stays mono, the air opens up.",
        &[("stereoize", 70.0), ("b1_width", 0.0), ("b2_width", 0.0), ("b3_width", 0.0), ("b4_width", 150.0)],
    ),
];

/// A JSON string for `text` (quotes and backslashes escaped; the presets' own text has no
/// control characters).
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// One preset as the JSON a preset menu recalls: every parameter, the preset's own values over
/// the defaults.
fn json_of(p: &SprawlPreset) -> String {
    let values: Vec<String> = SPRAWL_INFO
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
    BUILT.get_or_init(|| SPRAWL_PRESETS.iter().map(|p| (p.name, &*Box::leak(json_of(p).into_boxed_str()))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Plugin;
    use crate::plugins::fs::FsPreset;
    use crate::plugins::sprawl::{SPLIT_IDS, Sprawl, splits};

    #[test]
    fn every_preset_is_well_formed() {
        let mut names = std::collections::HashSet::new();
        for p in &SPRAWL_PRESETS {
            assert!(names.insert(p.name.to_lowercase()), "{} is listed twice", p.name);
            assert!(!p.group.is_empty() && !p.desc.is_empty(), "{}", p.name);
            let mut ids = std::collections::HashSet::new();
            for (id, v) in p.set {
                let info = SPRAWL_INFO.param(id).unwrap_or_else(|| panic!("{}: no parameter {id}", p.name));
                assert!(ids.insert(*id), "{}: {id} set twice", p.name);
                assert_eq!(info.clamp(*v), *v, "{}: {id} = {v} is out of range", p.name);
            }
            // The splits are in order and apart as written: what the preset says is what plays.
            let get = |id: &str| p.set.iter().find(|(k, _)| *k == id).map_or(SPRAWL_INFO.param(id).unwrap().default, |(_, v)| *v);
            let set = SPLIT_IDS.map(get);
            assert_eq!(splits(set, 44_100.0), set, "{}", p.name);
        }
    }

    #[test]
    fn the_json_carries_every_parameter_and_parses_back() {
        let list = factory_presets();
        assert_eq!(list.len(), SPRAWL_PRESETS.len());
        for ((name, json), p) in list.iter().zip(&SPRAWL_PRESETS) {
            let parsed = FsPreset::parse(json, &SPRAWL_INFO).unwrap_or_else(|| panic!("{name} does not parse"));
            assert_eq!((parsed.name.as_str(), parsed.group.as_str()), (p.name, p.group));
            assert_eq!(parsed.values.len(), SPRAWL_INFO.params.len(), "{name}");
            for (id, v) in p.set {
                assert_eq!(parsed.values.get(*id), Some(v), "{name}: {id}");
            }
            let json: serde_json::Value = serde_json::from_str(json).unwrap();
            assert_eq!(json["desc"], p.desc);
        }
        assert!(std::ptr::eq(list, factory_presets()));
    }

    #[test]
    fn every_preset_processes_sound_and_keeps_the_mono_sum() {
        let noise = |mut seed: u32| -> Vec<f32> {
            (0..24_000)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.4
                })
                .collect()
        };
        let run = |set: &[(&str, f32)]| {
            let mut s = Sprawl::new();
            for (id, v) in set {
                assert!(s.set_param(id, *v));
            }
            s.prepare(48_000.0, 512, 2);
            let mut io = vec![noise(1), noise(2)];
            let n = io[0].len();
            s.process(&mut io, n);
            io
        };
        for p in &SPRAWL_PRESETS {
            // The splits move the all-passes, so compare with the same splits and nothing else.
            let only_splits: Vec<(&str, f32)> = p.set.iter().copied().filter(|(id, _)| SPLIT_IDS.contains(id)).collect();
            let (plain, out) = (run(&only_splits), run(p.set));
            assert!(out.iter().flatten().all(|v| v.is_finite()), "{}", p.name);
            let worst = (0..plain[0].len()).fold(0.0f32, |m, i| m.max(((plain[0][i] + plain[1][i]) - (out[0][i] + out[1][i])).abs()));
            assert!(worst < 1.0e-4, "{}: the mono sum moved by {worst}", p.name);
        }
    }
}
