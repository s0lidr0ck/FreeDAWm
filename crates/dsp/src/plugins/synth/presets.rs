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

pub(crate) const fn preset(group: &'static str, name: &'static str, desc: &'static str, set: &'static [(&'static str, f32)]) -> SynthPreset {
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
    json_for(&SYNTH_INFO, p)
}

/// A preset's JSON for `info`'s instrument (every parameter, the preset's values over the
/// defaults): shared by the synths' preset lists.
pub(crate) fn json_for(info: &crate::PluginInfo, p: &SynthPreset) -> String {
    let values: Vec<String> = info
        .params
        .iter()
        .map(|info| {
            let v = p.set.iter().find(|(id, _)| *id == info.id).map_or(info.default, |(_, v)| info.clamp(*v));
            format!("{}:{v}", quoted(info.id))
        })
        .collect();
    format!("{{\"name\":{},\"group\":{},\"desc\":{},\"values\":{{{}}}}}", quoted(p.name), quoted(p.group), quoted(p.desc), values.join(","))
}

/// The factory presets as `(name, json)` pairs, in menu order (built once): the plain ones, then
/// those that use the modulation matrix and the macros.
pub fn factory_presets() -> &'static [(&'static str, &'static str)] {
    static BUILT: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    BUILT.get_or_init(|| {
        let mut v: Vec<(&'static str, &'static str)> = SYNTH_PRESETS.iter().map(|p| (p.name, &*Box::leak(json_of(p).into_boxed_str()))).collect();
        v.extend(patched_pairs_for(&SYNTH_INFO, &PATCHED_PRESETS));
        v
    })
}

/// A factory preset that patches the modulation matrix: its parameters, its slots
/// (source, destination, amount) and its macros (name, targets of destination, from, to; the
/// macros' values are parameters, `macro1`..`macro4`, in `preset.set`).
pub struct PatchedPreset {
    pub preset: SynthPreset,
    pub slots: &'static [(&'static str, &'static str, f32)],
    pub macros: &'static [MacroSpec],
}

/// A macro of a [`PatchedPreset`]: its name and its targets (destination, from, to).
pub type MacroSpec = (&'static str, &'static [(&'static str, f32, f32)]);

pub(crate) const fn patched(
    group: &'static str,
    name: &'static str,
    desc: &'static str,
    set: &'static [(&'static str, f32)],
    slots: &'static [(&'static str, &'static str, f32)],
    macros: &'static [MacroSpec],
) -> PatchedPreset {
    PatchedPreset { preset: SynthPreset { name, group, desc, set }, slots, macros }
}

impl PatchedPreset {
    /// The patch as the session stores it (`{"slots": [...], "macros": [...]}`).
    pub fn modulation_json(&self) -> String {
        let slots: Vec<String> =
            self.slots.iter().map(|(s, d, a)| format!("{{\"source\":{},\"dest\":{},\"amount\":{a}}}", quoted(s), quoted(d))).collect();
        let macros: Vec<String> = self
            .macros
            .iter()
            .map(|(name, targets)| {
                let t: Vec<String> = targets.iter().map(|(d, f, to)| format!("{{\"dest\":{},\"from\":{f},\"to\":{to}}}", quoted(d))).collect();
                format!("{{\"name\":{},\"targets\":[{}]}}", quoted(name), t.join(","))
            })
            .collect();
        format!("{{\"slots\":[{}],\"macros\":[{}]}}", slots.join(","), macros.join(","))
    }

    /// The numbers the synth takes for this patch (as the engine compiles a session's).
    pub fn compiled(&self, plugin: &str) -> Option<Vec<f64>> {
        use crate::plugins::synth_mod::{SlotSpec, TargetSpec, compile};
        let slots: Vec<SlotSpec<'_>> = self.slots.iter().map(|(s, d, a)| SlotSpec { source: s, dest: d, amount: *a }).collect();
        let targets: Vec<TargetSpec<'_>> = self
            .macros
            .iter()
            .enumerate()
            .flat_map(|(i, (_, t))| t.iter().map(move |(d, f, to)| TargetSpec { macro_index: i, dest: d, from: *f, to: *to }))
            .collect();
        compile(plugin, &slots, &targets)
    }
}

/// `list` as `(name, json)` pairs for `info`, each JSON carrying its `"modulation"` beside its
/// values.
pub(crate) fn patched_pairs_for(info: &crate::PluginInfo, list: &[PatchedPreset]) -> Vec<(&'static str, &'static str)> {
    list.iter()
        .map(|p| {
            let base = json_for(info, &p.preset);
            let json = format!("{},\"modulation\":{}}}", base.strip_suffix('}').unwrap_or(&base), p.modulation_json());
            (p.preset.name, &*Box::leak(json.into_boxed_str()))
        })
        .collect()
}

/// The presets that use the matrix and the macros (after [`SYNTH_PRESETS`] in the menu):
/// starting points written from what each slot does, not listened to; their levels are tested.
pub static PATCHED_PRESETS: [PatchedPreset; 4] = [
    patched(
        "Pad",
        "Breathing Pad",
        "Three detuned saws under a dark filter that the LFO opens and closes slowly, each note a hair off pitch. Open turns up the filter, Wobble the LFO's sweep, Grit the resonance and a growl from the sub, Space the unison's width.",
        &[
            ("osc2_detune", 14.0),
            ("unison", 2.0),
            ("unison_spread", 30.0),
            ("cutoff", 700.0),
            ("resonance", 15.0),
            ("filter_env_amount", 15.0),
            ("amp_attack", 600.0),
            ("amp_sustain", 85.0),
            ("amp_release", 1500.0),
            ("lfo_rate", 0.2),
            ("velocity", 30.0),
            ("level", -10.0),
            ("xmod_source", 1.0),
            ("macro1", 30.0),
        ],
        &[("lfo", "cutoff", 0.1), ("random", "pitch", 0.002), ("random", "osc_mix", 0.1)],
        &[
            ("Open", &[("cutoff", 0.0, 0.35)]),
            ("Wobble", &[("lfo_cutoff", 0.0, 0.5)]),
            ("Grit", &[("resonance", 0.0, 0.4), ("xmod_cutoff", 0.0, 0.25)]),
            ("Space", &[("unison_spread", 0.0, 0.6)]),
        ],
    ),
    patched(
        "Keys",
        "Velocity Pluck",
        "A short saw-and-square pluck whose filter opens with how hard the key is struck and a little more up the keyboard. Bright opens it for every note.",
        &[
            ("osc2_wave", 1.0),
            ("osc2_semitones", 12.0),
            ("osc_mix", 35.0),
            ("cutoff", 500.0),
            ("resonance", 20.0),
            ("filter_env_amount", 40.0),
            ("filter_decay", 250.0),
            ("filter_sustain", 0.0),
            ("amp_decay", 600.0),
            ("amp_sustain", 0.0),
            ("amp_release", 250.0),
        ],
        &[("velocity", "cutoff", 0.3), ("velocity", "filter_env_amount", 0.15), ("key", "cutoff", 0.12)],
        &[("Bright", &[("cutoff", 0.0, 0.3)])],
    ),
    patched(
        "Bass",
        "Growl Bass",
        "A saw an octave down, its pitch shaken by the sub at audio rate for a gritty edge, the filter wobbling with the mod wheel. Growl deepens the cross-modulation.",
        &[
            ("osc2_semitones", -12.0),
            ("osc2_detune", 0.0),
            ("osc_mix", 40.0),
            ("sub_level", 40.0),
            ("xmod_source", 1.0),
            ("xmod_pitch", 12.0),
            ("cutoff", 450.0),
            ("resonance", 30.0),
            ("filter_env_amount", 40.0),
            ("filter_decay", 220.0),
            ("filter_sustain", 15.0),
            ("amp_sustain", 90.0),
            ("amp_release", 90.0),
            ("lfo_rate", 4.0),
            ("level", -8.0),
        ],
        &[("mod_wheel", "lfo_cutoff", 0.5), ("env2", "xmod_cutoff", 0.2)],
        &[("Growl", &[("xmod_pitch", 0.0, 0.3)])],
    ),
    patched(
        "Lead",
        "Drift Lead",
        "Two saws whose detune breathes with the amp envelope and wanders a little per note; the mod wheel brings in vibrato. Bite raises the resonance and the filter envelope.",
        &[("osc2_detune", 8.0), ("cutoff", 2800.0), ("resonance", 20.0), ("filter_env_amount", 20.0), ("amp_release", 200.0), ("glide", 20.0)],
        &[("amp_env", "osc2_detune", -0.05), ("random", "osc2_detune", 0.04), ("mod_wheel", "lfo_pitch", 0.03)],
        &[("Bite", &[("resonance", 0.0, 0.4), ("filter_env_amount", 0.0, 0.2)])],
    ),
];

/// `list` as `(name, json)` pairs for `info` (each JSON made once, kept for the program's life).
pub(crate) fn pairs_for(info: &crate::PluginInfo, list: &[SynthPreset]) -> Vec<(&'static str, &'static str)> {
    list.iter().map(|p| (p.name, &*Box::leak(json_for(info, p).into_boxed_str()))).collect()
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
        assert_eq!(crate::plugins::fs::factory_presets(SYNTH_INFO.id).len(), SYNTH_PRESETS.len() + PATCHED_PRESETS.len());
    }
}
