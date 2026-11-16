//! Heft's factory presets: FreeDAWm's own, written for Heft (starting points, by the job).
//!
//! Each preset lists only what it changes from Heft's defaults; the JSON a preset menu recalls
//! (`{"name", "group", "desc", "values"}`, as the FireStation engines' presets) carries every
//! parameter.

use super::HEFT_INFO;
use std::sync::OnceLock;

/// One factory preset: what it is for, and the parameters it sets away from their defaults.
pub struct HeftPreset {
    pub name: &'static str,
    pub group: &'static str,
    pub desc: &'static str,
    pub set: &'static [(&'static str, f32)],
}

const fn preset(group: &'static str, name: &'static str, desc: &'static str, set: &'static [(&'static str, f32)]) -> HeftPreset {
    HeftPreset { name, group, desc, set }
}

/// The presets, in menu order.
pub static HEFT_PRESETS: [HeftPreset; 7] = [
    preset(
        "Start Here",
        "Gentle Lift",
        "A little of the second harmonic over 100 Hz: the bass line reads on a laptop, the mix hardly changes.",
        &[("harmonics", 30.0)],
    ),
    preset(
        "Start Here",
        "Phone Speaker",
        "Enough harmonics that a bass line is heard where nothing below 150 Hz is played.",
        &[("freq", 150.0), ("harmonics", 70.0), ("character", 50.0)],
    ),
    preset(
        "Bass",
        "Finger Bass",
        "Warm, even harmonics over 90 Hz for a round electric bass.",
        &[("freq", 90.0), ("harmonics", 45.0), ("character", 20.0)],
    ),
    preset(
        "Bass",
        "Synth Sub",
        "A sine sub made audible: odd harmonics over 80 Hz, the sub itself a little lower.",
        &[("freq", 80.0), ("harmonics", 65.0), ("character", 75.0), ("original", 80.0)],
    ),
    preset(
        "Drums",
        "Kick Weight",
        "The kick's boom heard as punch on small speakers; the harmonics follow each hit.",
        &[("freq", 70.0), ("harmonics", 55.0), ("character", 35.0)],
    ),
    preset(
        "Space Saving",
        "Harmonics Instead",
        "The lows mostly replaced by their harmonics: room for a mix with too much low end.",
        &[("freq", 120.0), ("harmonics", 80.0), ("original", 35.0)],
    ),
    preset(
        "Space Saving",
        "Harmonics Only",
        "No fundamental at all, only what it implies: a test of how a bass line reads without its bottom.",
        &[("freq", 120.0), ("harmonics", 100.0), ("original", 0.0)],
    ),
];

fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn json_of(p: &HeftPreset) -> String {
    let values: Vec<String> = HEFT_INFO
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
    BUILT.get_or_init(|| HEFT_PRESETS.iter().map(|p| (p.name, &*Box::leak(json_of(p).into_boxed_str()))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::fs::FsPreset;

    #[test]
    fn every_preset_is_well_formed_and_parses() {
        let mut names = std::collections::HashSet::new();
        for p in &HEFT_PRESETS {
            assert!(names.insert(p.name.to_lowercase()), "{}", p.name);
            assert!(!p.group.is_empty() && !p.desc.is_empty());
            for (id, v) in p.set {
                let info = HEFT_INFO.param(id).unwrap_or_else(|| panic!("{}: {id}", p.name));
                assert_eq!(info.clamp(*v), *v, "{}: {id}", p.name);
            }
        }
        for (name, json) in factory_presets() {
            let p = FsPreset::parse(json, &HEFT_INFO).unwrap();
            assert_eq!(&p.name, name);
            assert_eq!(p.values.len(), HEFT_INFO.params.len());
        }
        assert_eq!(crate::plugins::fs::factory_presets("heft").len(), HEFT_PRESETS.len());
    }
}
