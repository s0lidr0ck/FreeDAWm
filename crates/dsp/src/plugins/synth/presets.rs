//! The Subtractive Synth's factory presets: FreeDAWm's own parameter settings (starting points,
//! by the job), written for this synth; no samples.
//!
//! Each preset lists only what it changes from the synth's defaults; the JSON a preset menu
//! recalls (`{"name", "group", "desc", "values"}`, as the FireStation engines' presets) carries
//! every parameter.

use super::SYNTH_INFO;
use std::sync::OnceLock;

/// One factory preset: what it is for, and the parameters it sets away from their defaults.
pub struct SynthPreset {
    pub name: &'static str,
    pub group: &'static str,
    pub desc: &'static str,
    pub set: &'static [(&'static str, f32)],
}

const fn preset(group: &'static str, name: &'static str, desc: &'static str, set: &'static [(&'static str, f32)]) -> SynthPreset {
    SynthPreset { name, group, desc, set }
}

// Waves: 0 saw, 1 square, 2 sine, 3 triangle.

/// The presets, in menu order.
pub static SYNTH_PRESETS: [SynthPreset; 15] = [
    preset(
        "Bass",
        "Round Bass",
        "Saw and square an octave down, filtered low with a short pluck: a bass line that sits under the mix.",
        &[
            ("osc2_wave", 1.0),
            ("osc2_semitones", -12.0),
            ("osc2_detune", 0.0),
            ("cutoff", 420.0),
            ("resonance", 15.0),
            ("filter_env_amount", 45.0),
            ("filter_decay", 180.0),
            ("filter_sustain", 10.0),
            ("amp_sustain", 85.0),
            ("amp_release", 90.0),
        ],
    ),
    preset(
        "Bass",
        "Sub Sine",
        "A sine with a touch of triangle: low end with no edge, for small speakers add Heft after it.",
        &[
            ("osc1_wave", 2.0),
            ("osc2_wave", 3.0),
            ("osc2_detune", 0.0),
            ("osc_mix", 20.0),
            ("cutoff", 1200.0),
            ("resonance", 0.0),
            ("filter_env_amount", 0.0),
            ("amp_sustain", 100.0),
            ("amp_release", 60.0),
        ],
    ),
    preset(
        "Bass",
        "Acid Line",
        "One saw, a resonant filter and a fast decaying sweep, with a little glide between notes.",
        &[
            ("osc_mix", 0.0),
            ("cutoff", 300.0),
            ("resonance", 75.0),
            ("filter_env_amount", 70.0),
            ("filter_decay", 160.0),
            ("filter_sustain", 0.0),
            ("amp_decay", 300.0),
            ("amp_sustain", 60.0),
            ("amp_release", 40.0),
            ("glide", 60.0),
        ],
    ),
    preset(
        "Lead",
        "Detuned Lead",
        "Two saws a few cents apart, bright, with a quick attack and the mod wheel bringing in vibrato.",
        &[
            ("osc2_detune", 12.0),
            ("cutoff", 5200.0),
            ("resonance", 25.0),
            ("filter_env_amount", 20.0),
            ("amp_release", 180.0),
            ("glide", 25.0),
            ("mod_wheel", 30.0),
        ],
    ),
    preset(
        "Lead",
        "Square Lead",
        "Two squares an octave apart: hollow and reedy, cutting through without much top.",
        &[
            ("osc1_wave", 1.0),
            ("osc2_wave", 1.0),
            ("osc2_semitones", 12.0),
            ("osc2_detune", 3.0),
            ("osc_mix", 35.0),
            ("cutoff", 3200.0),
            ("resonance", 10.0),
            ("filter_env_amount", 10.0),
            ("mod_wheel", 25.0),
        ],
    ),
    preset(
        "Pad",
        "Warm Pad",
        "Slow swell and long release, two saws detuned and filtered dark.",
        &[
            ("osc2_detune", 18.0),
            ("cutoff", 900.0),
            ("resonance", 10.0),
            ("filter_env_amount", 25.0),
            ("filter_attack", 900.0),
            ("filter_decay", 1500.0),
            ("filter_sustain", 50.0),
            ("amp_attack", 700.0),
            ("amp_decay", 1500.0),
            ("amp_sustain", 85.0),
            ("amp_release", 1800.0),
            ("velocity", 30.0),
            ("level", -10.0),
        ],
    ),
    preset(
        "Pad",
        "Glass Pad",
        "Triangle and sine a fifth apart: a soft, clear bed for chords.",
        &[
            ("osc1_wave", 3.0),
            ("osc2_wave", 2.0),
            ("osc2_semitones", 7.0),
            ("osc2_detune", 4.0),
            ("osc_mix", 40.0),
            ("cutoff", 6000.0),
            ("resonance", 5.0),
            ("filter_env_amount", 0.0),
            ("amp_attack", 400.0),
            ("amp_sustain", 90.0),
            ("amp_release", 1500.0),
            ("velocity", 30.0),
            ("level", -9.0),
        ],
    ),
    preset(
        "Keys",
        "Soft Keys",
        "A plucky triangle with a decaying filter: an electric-piano-like part for chords.",
        &[
            ("osc1_wave", 3.0),
            ("osc2_wave", 2.0),
            ("osc2_semitones", 12.0),
            ("osc2_detune", 0.0),
            ("osc_mix", 25.0),
            ("cutoff", 2400.0),
            ("resonance", 5.0),
            ("filter_env_amount", 35.0),
            ("filter_decay", 600.0),
            ("filter_sustain", 20.0),
            ("amp_attack", 2.0),
            ("amp_decay", 1400.0),
            ("amp_sustain", 25.0),
            ("amp_release", 400.0),
            ("velocity", 85.0),
        ],
    ),
    preset(
        "Keys",
        "Organ-ish",
        "Sine and square an octave apart, no envelope movement: a steady, simple organ tone.",
        &[
            ("osc1_wave", 2.0),
            ("osc2_wave", 1.0),
            ("osc2_semitones", 12.0),
            ("osc2_detune", 0.0),
            ("osc_mix", 30.0),
            ("cutoff", 3500.0),
            ("resonance", 0.0),
            ("filter_env_amount", 0.0),
            ("amp_attack", 4.0),
            ("amp_sustain", 100.0),
            ("amp_release", 40.0),
            ("velocity", 10.0),
            ("mod_wheel", 15.0),
        ],
    ),
    preset(
        "Pluck",
        "Pluck",
        "A short, bright pluck that dies away: arpeggios and fast patterns.",
        &[
            ("osc2_detune", 6.0),
            ("cutoff", 700.0),
            ("resonance", 30.0),
            ("filter_env_amount", 80.0),
            ("filter_attack", 1.0),
            ("filter_decay", 220.0),
            ("filter_sustain", 0.0),
            ("amp_attack", 1.0),
            ("amp_decay", 350.0),
            ("amp_sustain", 0.0),
            ("amp_release", 250.0),
            ("velocity", 80.0),
        ],
    ),
    preset(
        "Pluck",
        "Bell Pluck",
        "Sine and a square two octaves up, decaying: a small bell or mallet.",
        &[
            ("osc1_wave", 2.0),
            ("osc2_wave", 1.0),
            ("osc2_semitones", 24.0),
            ("osc2_detune", 0.0),
            ("osc_mix", 25.0),
            ("cutoff", 8000.0),
            ("resonance", 0.0),
            ("filter_env_amount", -30.0),
            ("filter_decay", 800.0),
            ("amp_attack", 1.0),
            ("amp_decay", 1200.0),
            ("amp_sustain", 0.0),
            ("amp_release", 900.0),
        ],
    ),
    preset(
        "Effects",
        "Riser",
        "A slow filter opening over a long attack: hold a note into a drop.",
        &[
            ("osc2_detune", 25.0),
            ("cutoff", 150.0),
            ("resonance", 55.0),
            ("filter_env_amount", 100.0),
            ("filter_attack", 4000.0),
            ("filter_sustain", 100.0),
            ("amp_attack", 2500.0),
            ("amp_sustain", 100.0),
            ("amp_release", 600.0),
            ("velocity", 0.0),
            ("level", -10.0),
        ],
    ),
    preset(
        "Lead",
        "Wide Stack Lead",
        "Seven detuned saws spread across the sides, a bright filter and a little glide: a big trance-style lead.",
        &[
            ("unison", 6.0),
            ("unison_detune", 28.0),
            ("unison_spread", 85.0),
            ("osc2_detune", 0.0),
            ("cutoff", 5200.0),
            ("resonance", 15.0),
            ("filter_env_amount", 20.0),
            ("amp_sustain", 90.0),
            ("amp_release", 350.0),
            ("glide", 30.0),
            ("level", -10.0),
        ],
    ),
    preset(
        "Lead",
        "Sync Lead",
        "Oscillator 2 hard-synced a fifth and an octave up, its tone swept by the filter envelope: a biting lead.",
        &[
            ("osc_sync", 1.0),
            ("osc_mix", 80.0),
            ("osc2_semitones", 19.0),
            ("osc2_detune", 0.0),
            ("cutoff", 1800.0),
            ("resonance", 25.0),
            ("filter_env_amount", 55.0),
            ("filter_decay", 600.0),
            ("filter_sustain", 35.0),
            ("amp_sustain", 85.0),
            ("mod_wheel", 25.0),
            ("level", -8.0),
        ],
    ),
    preset(
        "Pad",
        "Wide Unison Pad",
        "Four gently detuned copies of saw and triangle across the whole stereo field, slow to open and close.",
        &[
            ("unison", 3.0),
            ("unison_detune", 14.0),
            ("unison_spread", 100.0),
            ("osc2_wave", 3.0),
            ("osc2_semitones", 12.0),
            ("cutoff", 1400.0),
            ("filter_env_amount", 25.0),
            ("filter_attack", 1200.0),
            ("amp_attack", 900.0),
            ("amp_sustain", 90.0),
            ("amp_release", 1800.0),
            ("level", -10.0),
        ],
    ),
];

fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn json_of(p: &SynthPreset) -> String {
    let values: Vec<String> = SYNTH_INFO
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
    BUILT.get_or_init(|| SYNTH_PRESETS.iter().map(|p| (p.name, &*Box::leak(json_of(p).into_boxed_str()))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::fs::FsPreset;

    /// Parity section 7: the synth's factory presets name real parameters with values in range,
    /// and each parses into a full set of values.
    #[test]
    fn every_synth_preset_is_well_formed_and_parses() {
        let mut names = std::collections::HashSet::new();
        for p in &SYNTH_PRESETS {
            assert!(names.insert(p.name.to_lowercase()), "{}", p.name);
            assert!(!p.group.is_empty() && !p.desc.is_empty());
            for (id, v) in p.set {
                let info = SYNTH_INFO.param(id).unwrap_or_else(|| panic!("{}: {id}", p.name));
                assert_eq!(info.clamp(*v), *v, "{}: {id}", p.name);
            }
        }
        for (name, json) in factory_presets() {
            let p = FsPreset::parse(json, &SYNTH_INFO).unwrap();
            assert_eq!(&p.name, name);
            assert_eq!(p.values.len(), SYNTH_INFO.params.len());
        }
        assert_eq!(crate::plugins::fs::factory_presets(SYNTH_INFO.id).len(), SYNTH_PRESETS.len());
    }
}
