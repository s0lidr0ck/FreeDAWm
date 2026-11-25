//! Plumb's factory presets: FreeDAWm's own starting points, by how firmly they correct.
//!
//! Each preset lists only what it changes from Plumb's defaults (Gentle is the defaults);
//! the JSON a preset menu recalls (`{"name", "group", "desc", "values"}`, as the other built-ins'
//! presets) carries every parameter. The editor keeps the key, the scale, the note switches, the
//! mode and the voice range out of presets, so recalling one never changes the key or the mode.

use super::{AUTOPITCH_INFO, STYLE_CUSTOM, STYLE_HARD};
use std::sync::OnceLock;

/// One factory preset: what it is for, and the parameters it sets away from their defaults.
pub struct AutoPitchPreset {
    pub name: &'static str,
    pub group: &'static str,
    pub desc: &'static str,
    pub set: &'static [(&'static str, f32)],
}

/// The presets, in menu order.
pub static AUTOPITCH_PRESETS: [AutoPitchPreset; 3] = [
    AutoPitchPreset {
        name: "Gentle",
        group: "Start Here",
        desc: "Natural: held notes drift onto the key over about a tenth of a second, long notes more gently still; the vibrato is kept.",
        set: &[],
    },
    AutoPitchPreset {
        name: "Medium",
        group: "Start Here",
        desc: "Firmer: notes land within about 50 ms and stay there; the vibrato is kept.",
        set: &[("style", STYLE_CUSTOM as f32), ("retune", 25.0), ("humanize", 20.0), ("vibrato", 1.0), ("hold", 40.0)],
    },
    AutoPitchPreset {
        name: "Hard",
        group: "Start Here",
        desc: "Hard tune: every note snaps at once and the vibrato is flattened, the stepped sound of an obvious correction.",
        set: &[("style", STYLE_HARD as f32), ("retune", 0.0), ("humanize", 0.0), ("vibrato", 0.0), ("hold", 45.0)],
    },
];

fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn json_of(p: &AutoPitchPreset) -> String {
    let values: Vec<String> = AUTOPITCH_INFO
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
    BUILT.get_or_init(|| AUTOPITCH_PRESETS.iter().map(|p| (p.name, &*Box::leak(json_of(p).into_boxed_str()))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::fs::FsPreset;

    #[test]
    fn every_preset_is_well_formed_and_parses() {
        let mut names = std::collections::HashSet::new();
        for p in &AUTOPITCH_PRESETS {
            assert!(names.insert(p.name.to_lowercase()), "{}", p.name);
            assert!(!p.group.is_empty() && !p.desc.is_empty());
            for (id, v) in p.set {
                let info = AUTOPITCH_INFO.param(id).unwrap_or_else(|| panic!("{}: {id}", p.name));
                assert_eq!(info.clamp(*v), *v, "{}: {id}", p.name);
            }
        }
        for (name, json) in factory_presets() {
            let p = FsPreset::parse(json, &AUTOPITCH_INFO).unwrap();
            assert_eq!(&p.name, name);
            assert_eq!(p.values.len(), AUTOPITCH_INFO.params.len());
        }
        assert_eq!(crate::plugins::fs::factory_presets("autopitch").len(), AUTOPITCH_PRESETS.len());
        // Gentle is the defaults; Hard is the Hard tune style's own values.
        assert!(AUTOPITCH_PRESETS[0].set.is_empty());
        let hard = super::super::STYLE_VALUES[1];
        assert!(AUTOPITCH_PRESETS[2].set.contains(&("hold", hard.3)) && AUTOPITCH_PRESETS[2].set.contains(&("retune", hard.0)));
    }
}
