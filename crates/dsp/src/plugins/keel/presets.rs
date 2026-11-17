//! Keel's factory presets: FreeDAWm's own, written for Keel (starting points, by the job).
//!
//! Each preset lists only what it changes from Keel's defaults; the JSON a preset menu recalls
//! (`{"name", "group", "desc", "values"}`, as the FireStation engines' presets) carries every
//! parameter, so recalling one never leaves a setting of the previous preset behind.

use super::KEEL_INFO;
use std::sync::OnceLock;

/// One factory preset: what it is for, and the parameters it sets away from their defaults.
pub struct KeelPreset {
    pub name: &'static str,
    pub group: &'static str,
    pub desc: &'static str,
    pub set: &'static [(&'static str, f32)],
}

const fn preset(group: &'static str, name: &'static str, desc: &'static str, set: &'static [(&'static str, f32)]) -> KeelPreset {
    KeelPreset { name, group, desc, set }
}

const TUBE: f32 = 0.0;
const SOLID: f32 = 1.0;
const GRIND: f32 = 2.0;
const C410: f32 = 0.0;
const C115: f32 = 1.0;
const C810: f32 = 2.0;
const OFF: f32 = 3.0;

/// The presets, in menu order.
pub static KEEL_PRESETS: [KeelPreset; 12] = [
    // ---------------------------------------------------------------- Clean
    preset(
        "Clean",
        "Warm Valve",
        "A valve amp just starting to round the peaks, on a 4x10, with some DI under it.",
        &[("voicing", TUBE), ("drive", 4.0)],
    ),
    preset(
        "Clean",
        "Round And Deep",
        "Bass up, treble down, through the 1x15: a soft, old-school bottom for fingerstyle.",
        &[("voicing", TUBE), ("drive", 3.0), ("bass", 7.0), ("treble", 3.5), ("cab", C115), ("blend", 80.0)],
    ),
    preset(
        "Clean",
        "Hi-Fi Studio",
        "The Solid State voicing barely driven, Bright on, half DI: clear for slap and busy lines.",
        &[("voicing", SOLID), ("drive", 2.5), ("treble", 6.0), ("bright", 1.0), ("blend", 50.0)],
    ),
    preset(
        "Clean",
        "Mostly DI",
        "Just enough amp under the DI to give it a cabinet's weight. Safe on any mix.",
        &[("voicing", SOLID), ("drive", 3.0), ("blend", 25.0), ("squeeze", 3.0)],
    ),
    // ---------------------------------------------------------------- Rock
    preset(
        "Rock",
        "Classic Rock",
        "The valve pushed into a gentle growl on the 8x10, mids forward to sit under guitars.",
        &[("voicing", TUBE), ("drive", 6.5), ("mid", 6.5), ("cab", C810), ("blend", 75.0)],
    ),
    preset(
        "Rock",
        "Pick Attack",
        "Played with a pick: mids and Bright to bring the click out, Squeeze to even the notes.",
        &[("voicing", SOLID), ("drive", 5.5), ("mid", 6.0), ("treble", 6.5), ("bright", 1.0), ("squeeze", 4.0)],
    ),
    preset(
        "Rock",
        "Grind Blend",
        "The Grind voicing for the edge, half DI to keep the fundamental solid underneath.",
        &[("voicing", GRIND), ("drive", 6.0), ("cab", C410), ("blend", 50.0)],
    ),
    preset(
        "Rock",
        "Fuzz Wall",
        "Grind wide open on the 8x10. Pull Blend down if the low notes lose their pitch.",
        &[("voicing", GRIND), ("drive", 9.0), ("bass", 6.0), ("mid", 6.5), ("cab", C810), ("blend", 70.0), ("output", -6.0)],
    ),
    // ---------------------------------------------------------------- Tight
    preset(
        "Tight",
        "Even Notes",
        "Squeeze holding every note at the same level, for a line that has to sit still.",
        &[("voicing", TUBE), ("drive", 4.5), ("squeeze", 6.0)],
    ),
    preset(
        "Tight",
        "Scooped Slap",
        "Mids down, lows and highs up, Bright on: the slap sound, with Squeeze on the pops.",
        &[("voicing", SOLID), ("drive", 3.0), ("bass", 7.0), ("mid", 2.5), ("treble", 7.5), ("bright", 1.0), ("squeeze", 5.0), ("blend", 60.0)],
    ),
    preset(
        "Tight",
        "Synth Bass Grit",
        "For a synth bass: a little Grind with no cabinet, so the low end stays full.",
        &[("voicing", GRIND), ("drive", 5.0), ("cab", OFF), ("blend", 60.0)],
    ),
    preset(
        "Tight",
        "Small Speakers",
        "Mids pushed and a little drive, so the bass line is heard on a phone as well as on monitors.",
        &[("voicing", TUBE), ("drive", 6.0), ("mid", 7.5), ("bass", 4.0), ("cab", C410), ("squeeze", 3.0)],
    ),
];

/// A JSON string for `text` (quotes and backslashes escaped; the presets' own text has no
/// control characters).
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// One preset as the JSON a preset menu recalls: every parameter, the preset's own values over
/// the defaults.
fn json_of(p: &KeelPreset) -> String {
    let values: Vec<String> = KEEL_INFO
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
    BUILT.get_or_init(|| KEEL_PRESETS.iter().map(|p| (p.name, &*Box::leak(json_of(p).into_boxed_str()))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Plugin;
    use crate::plugins::fs::FsPreset;
    use crate::plugins::keel::Keel;

    #[test]
    fn every_preset_is_well_formed_and_parses() {
        let mut names = std::collections::HashSet::new();
        for p in &KEEL_PRESETS {
            assert!(names.insert(p.name.to_lowercase()), "{} is listed twice", p.name);
            assert!(!p.group.is_empty() && !p.desc.is_empty(), "{}", p.name);
            let mut ids = std::collections::HashSet::new();
            for (id, v) in p.set {
                let info = KEEL_INFO.param(id).unwrap_or_else(|| panic!("{}: no parameter {id}", p.name));
                assert!(ids.insert(*id), "{}: {id} set twice", p.name);
                assert_eq!(info.clamp(*v), *v, "{}: {id} = {v} is out of range", p.name);
            }
        }
        let built = factory_presets();
        assert_eq!(built.len(), KEEL_PRESETS.len());
        for (name, json) in built {
            let p = FsPreset::parse(json, &KEEL_INFO).unwrap();
            assert_eq!(&p.name, name);
            assert_eq!(p.values.len(), KEEL_INFO.params.len(), "{name}: every parameter");
        }
        assert_eq!(crate::plugins::fs::factory_presets("keel").len(), KEEL_PRESETS.len());
    }

    #[test]
    fn every_preset_plays_finite_and_not_silent() {
        for p in &KEEL_PRESETS {
            let mut k = Keel::new();
            for (id, v) in p.set {
                assert!(k.set_param(id, *v));
            }
            k.prepare(48_000.0, 512, 2);
            let mut io: Vec<Vec<f32>> = (0..2).map(|_| (0..9600).map(|i| 0.5 * (i as f32 * 0.0144).sin()).collect()).collect();
            k.process(&mut io, 9600);
            assert!(io.iter().flatten().all(|v| v.is_finite()), "{}", p.name);
            let peak = io[0][4800..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak > 0.05 && peak < 4.0, "{}: peak {peak}", p.name);
        }
    }
}
