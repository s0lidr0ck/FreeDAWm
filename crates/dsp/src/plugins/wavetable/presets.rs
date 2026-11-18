//! The Wavetable Synth's factory presets: FreeDAWm's own parameter settings, starting points
//! written for this synth (nothing here was listened to: each is a set of values chosen by what
//! the controls do, checked by tests for level and range). No samples.

use super::WAVETABLE_INFO;
use crate::plugins::synth::presets::{SynthPreset, pairs_for, preset};
use std::sync::OnceLock;

// Tables: 0 Classic, 1 Harmonic Sweep, 2 Pulse Width, 3 Sync Saw, 4 Sync Square, 5 Vowels,
// 6 Odd to Even, 7 Drawbars, 8 Phase Bend, 9 Fold, 10 Steps, 11 Comb, 12 Grit.
// Filter types: 0 LP12, 1 LP24, 2 HP, 3 BP, 4 notch. LFO waves: 0 sine, 1 triangle, 2 square, 3 saw.

/// The presets, in menu order.
pub static WAVETABLE_PRESETS: [SynthPreset; 12] = [
    preset(
        "Bass",
        "Sync Growl",
        "The Sync Saw table swept up by the second envelope on each note, low-passed: a bass that opens and closes.",
        &[
            ("table", 3.0),
            ("position", 5.0),
            ("env2_position", 60.0),
            ("env2_decay", 350.0),
            ("sub_level", 50.0),
            ("filter_type", 1.0),
            ("cutoff", 1800.0),
            ("resonance", 25.0),
            ("amp_sustain", 90.0),
            ("amp_release", 120.0),
        ],
    ),
    preset(
        "Bass",
        "Fold Bass",
        "The Fold table, harder with velocity: soft notes round, hard ones growl.",
        &[
            ("table", 9.0),
            ("position", 10.0),
            ("velocity_position", 60.0),
            ("sub_level", 40.0),
            ("cutoff", 3000.0),
            ("amp_sustain", 85.0),
            ("amp_release", 100.0),
        ],
    ),
    preset(
        "Lead",
        "Pulse Lead",
        "Pulse Width moved by a slow triangle LFO, five unison copies a little apart, the mod wheel narrowing the pulse.",
        &[
            ("table", 2.0),
            ("position", 20.0),
            ("lfo_position", 40.0),
            ("lfo_wave", 1.0),
            ("lfo_rate", 0.7),
            ("wheel_position", 50.0),
            ("unison", 4.0),
            ("unison_detune", 12.0),
            ("unison_spread", 60.0),
            ("cutoff", 9000.0),
            ("glide", 40.0),
        ],
    ),
    preset(
        "Lead",
        "Vowel Lead",
        "The Vowels table moved from a to u by the mod wheel, with vibrato from the LFO.",
        &[
            ("table", 5.0),
            ("position", 0.0),
            ("wheel_position", 100.0),
            ("lfo_pitch", 15.0),
            ("lfo_rate", 5.5),
            ("amp_attack", 30.0),
            ("amp_release", 250.0),
        ],
    ),
    preset(
        "Keys",
        "Drawbar Keys",
        "Organ registrations from the Drawbars table, fuller with velocity, and a quick key click from noise.",
        &[
            ("table", 7.0),
            ("position", 40.0),
            ("velocity_position", 40.0),
            ("noise_level", 4.0),
            ("amp_attack", 2.0),
            ("amp_decay", 100.0),
            ("amp_sustain", 100.0),
            ("amp_release", 60.0),
            ("velocity", 30.0),
        ],
    ),
    preset(
        "Keys",
        "Glass Keys",
        "Phase Bend falling from bright to soft over each note: a bell-like attack that settles.",
        &[
            ("table", 8.0),
            ("position", 10.0),
            ("env2_position", 70.0),
            ("env2_decay", 900.0),
            ("amp_decay", 1500.0),
            ("amp_sustain", 30.0),
            ("amp_release", 700.0),
        ],
    ),
    preset(
        "Pad",
        "Harmonic Swell",
        "Harmonic Sweep opening slowly as the note is held, seven copies spread wide.",
        &[
            ("table", 1.0),
            ("position", 10.0),
            ("env2_position", 65.0),
            ("env2_attack", 2500.0),
            ("env2_decay", 3000.0),
            ("env2_sustain", 80.0),
            ("unison", 6.0),
            ("unison_detune", 18.0),
            ("unison_spread", 100.0),
            ("amp_attack", 900.0),
            ("amp_release", 2000.0),
            ("level", -10.0),
        ],
    ),
    preset(
        "Pad",
        "Comb Drift",
        "The Comb table drifting with a slow sine LFO, low-passed, wide.",
        &[
            ("table", 11.0),
            ("position", 50.0),
            ("lfo_position", 80.0),
            ("lfo_rate", 0.15),
            ("unison", 2.0),
            ("unison_detune", 10.0),
            ("unison_spread", 80.0),
            ("filter_type", 1.0),
            ("cutoff", 4000.0),
            ("amp_attack", 600.0),
            ("amp_release", 1500.0),
            ("level", -8.0),
        ],
    ),
    preset(
        "Pad",
        "Grit Texture",
        "Grit's fixed random spectra moving under a band-pass that follows the keys.",
        &[
            ("table", 12.0),
            ("position", 30.0),
            ("lfo_position", 60.0),
            ("lfo_rate", 0.3),
            ("lfo_wave", 1.0),
            ("filter_type", 3.0),
            ("cutoff", 1500.0),
            ("resonance", 30.0),
            ("key_track", 80.0),
            ("amp_attack", 400.0),
            ("amp_release", 1200.0),
        ],
    ),
    preset(
        "Pluck",
        "Step Pluck",
        "The Steps table falling from fine to coarse on each note, short and dry.",
        &[
            ("table", 10.0),
            ("position", 0.0),
            ("env2_position", 80.0),
            ("env2_decay", 200.0),
            ("amp_decay", 350.0),
            ("amp_sustain", 0.0),
            ("amp_release", 200.0),
            ("cutoff", 7000.0),
            ("env2_cutoff", 30.0),
        ],
    ),
    preset(
        "Pluck",
        "Sync Pluck",
        "Sync Square snapping down from its top on each note, with a filter pluck.",
        &[
            ("table", 4.0),
            ("position", 0.0),
            ("env2_position", 90.0),
            ("env2_decay", 150.0),
            ("filter_type", 1.0),
            ("cutoff", 900.0),
            ("env2_cutoff", 60.0),
            ("amp_decay", 400.0),
            ("amp_sustain", 0.0),
            ("amp_release", 250.0),
        ],
    ),
    preset(
        "Lead",
        "Morph Lead",
        "The Classic table from saw towards square as the key goes down harder, with a short glide.",
        &[
            ("table", 0.0),
            ("position", 66.0),
            ("velocity_position", 34.0),
            ("glide", 30.0),
            ("cutoff", 8000.0),
            ("resonance", 20.0),
            ("amp_release", 200.0),
        ],
    ),
];

/// The factory presets as `(name, json)` pairs, in menu order (built once).
pub fn factory_presets() -> &'static [(&'static str, &'static str)] {
    static BUILT: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    BUILT.get_or_init(|| pairs_for(&WAVETABLE_INFO, &WAVETABLE_PRESETS))
}
