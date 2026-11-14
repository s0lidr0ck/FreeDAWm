//! The built-in plugins and their registry.

pub mod drums;
pub mod dynamics;
pub mod eq;
pub mod fs;
pub mod harmonic;
pub mod juke;
pub mod modulation;
pub mod pitch;
pub mod reverb;
pub mod rez;
pub mod sampler;
pub mod sprawl;
pub mod strip;
pub mod synth;
#[cfg(feature = "test-plugins")]
pub mod test_link;
pub mod utility;

use crate::{Plugin, PluginInfo};

/// The built-in plugins other than the FireStation engines, in menu order.
const BUILTINS: [&PluginInfo; 30] = [
    &eq::EQ1_INFO,
    &eq::EQ7_INFO,
    &dynamics::COMP_INFO,
    &dynamics::GATE_INFO,
    &dynamics::DEESS_INFO,
    &dynamics::MAX_INFO,
    &rez::REZ_INFO,
    &strip::STRIP_INFO,
    &reverb::ROOM_INFO,
    &reverb::PLATE_INFO,
    &modulation::DELAY_INFO,
    &modulation::CHORUS_INFO,
    &modulation::FLANGER_INFO,
    &modulation::PHASER_INFO,
    &harmonic::SAT_INFO,
    &harmonic::LOFI_INFO,
    &harmonic::RECT_INFO,
    &pitch::PITCH_INFO,
    &sprawl::SPRAWL_INFO,
    &utility::SHIFT_INFO,
    &utility::GAIN_INFO,
    &utility::TRIM_INFO,
    &utility::INVERT_INFO,
    &utility::DC_INFO,
    &utility::GEN_INFO,
    &utility::DITHER_INFO,
    &synth::SYNTH_INFO,
    &drums::DRUM_INFO,
    &sampler::SAMPLER_INFO,
    &juke::JUKE_INFO,
];

/// Every built-in plugin, in menu order: [`BUILTINS`], then the FireStation engines by their
/// `order` (collected by `build.rs`, see [`fs::EngineReg`]).
pub(crate) static REGISTRY: [&PluginInfo; BUILTINS.len() + fs::ENGINE_COUNT] = registry(&BUILTINS, &fs::BY_ORDER);

/// `builtins` followed by the engines' infos, at compile time (a wrong `N` fails the build).
const fn registry<const N: usize>(builtins: &[&'static PluginInfo], engines: &[&'static fs::EngineReg]) -> [&'static PluginInfo; N] {
    let mut out = [builtins[0]; N];
    let mut i = 0;
    while i < builtins.len() {
        out[i] = builtins[i];
        i += 1;
    }
    let mut j = 0;
    while j < engines.len() {
        out[i + j] = engines[j].info;
        j += 1;
    }
    assert!(i + j == N, "the registry's length is the built-ins plus the engines");
    out
}

/// Plugins for tests only (feature `test-plugins`): found by id, never listed.
#[cfg(feature = "test-plugins")]
pub(crate) static TEST_REGISTRY: &[&PluginInfo] = &[&test_link::TEST_LINK_INFO, &test_link::TEST_LINK_BIG_INFO];
#[cfg(not(feature = "test-plugins"))]
pub(crate) static TEST_REGISTRY: &[&PluginInfo] = &[];

/// Constructs an (unprepared) plugin by id.
pub(crate) fn instantiate(id: &str) -> Option<Box<dyn Plugin>> {
    Some(match id {
        "eq_1band" => Box::new(eq::Eq1::new()),
        "eq_7band" => Box::new(eq::Eq7::new()),
        "compressor" => Box::new(dynamics::Compressor::new()),
        "expander_gate" => Box::new(dynamics::ExpanderGate::new()),
        "de_esser" => Box::new(dynamics::DeEsser::new()),
        "maximizer" => Box::new(dynamics::Maximizer::new()),
        "rez" => Box::new(rez::Rez::new()),
        "channel_strip" => Box::new(strip::ChannelStrip::new()),
        "room_reverb" => Box::new(reverb::Reverb::room()),
        "plate_reverb" => Box::new(reverb::Reverb::plate()),
        "mod_delay" => Box::new(modulation::ModDelay::new()),
        "chorus" => Box::new(modulation::ModFx::chorus()),
        "flanger" => Box::new(modulation::ModFx::flanger()),
        "phaser" => Box::new(modulation::Phaser::new()),
        "saturator" => Box::new(harmonic::Saturator::new()),
        "lofi" => Box::new(harmonic::LoFi::new()),
        "rectifier" => Box::new(harmonic::Rectifier::new()),
        "pitch_shifter" => Box::new(pitch::PitchShifter::new()),
        "sprawl" => Box::new(sprawl::Sprawl::new()),
        "time_shift" => Box::new(utility::TimeShift::new()),
        "gain" => Box::new(utility::Gain::new()),
        "trim" => Box::new(utility::Trim::new()),
        "invert" => Box::new(utility::Invert::default()),
        "dc_offset_removal" => Box::new(utility::DcRemoval::new()),
        "signal_generator" => Box::new(utility::SignalGenerator::new()),
        "dither" => Box::new(utility::Dither::new()),
        "subtractive_synth" => Box::new(synth::SubtractiveSynth::new()),
        "drum_synth" => Box::new(drums::DrumSynth::new()),
        "sampler" => Box::new(sampler::Sampler::new()),
        "juke" => Box::new(juke::Juke::new()),
        #[cfg(feature = "test-plugins")]
        "test_link" => Box::new(test_link::TestLink::new()),
        #[cfg(feature = "test-plugins")]
        "test_link_big" => Box::new(test_link::TestLink::big()),
        _ => return fs::engine(id).map(|e| (e.make)()),
    })
}

#[cfg(test)]
mod tests {
    /// The registry's ids in menu order, pinned (snapshot taken before the FireStation engines
    /// were collected by `build.rs`): a new plugin is added here on purpose.
    const PINNED: &[&str] = &[
        "eq_1band",
        "eq_7band",
        "compressor",
        "expander_gate",
        "de_esser",
        "maximizer",
        "rez",
        "channel_strip",
        "room_reverb",
        "plate_reverb",
        "mod_delay",
        "chorus",
        "flanger",
        "phaser",
        "saturator",
        "lofi",
        "rectifier",
        "pitch_shifter",
        "sprawl",
        "time_shift",
        "gain",
        "trim",
        "invert",
        "dc_offset_removal",
        "signal_generator",
        "dither",
        "subtractive_synth",
        "drum_synth",
        "sampler",
        "juke",
        "fs_sat",
        "fs_comp",
        "fs_tape",
        "fs_twine",
        "fs_manners",
        "fs_cleanq",
        "fs_vint",
        "fs_pitch",
        "fs_mod",
        "fs_dist",
        "fs_transient",
        "fs_filter",
        "fs_delay",
        "fs_reverb",
        "fs_swale",
        "fs_chamber",
        "fs_amp",
        "fs_shale",
        "fs_levelup",
    ];

    #[test]
    fn registry_lists_the_same_ids_in_the_same_order() {
        let ids: Vec<&str> = super::REGISTRY.iter().map(|i| i.id).collect();
        assert_eq!(ids, PINNED);
    }

    #[test]
    fn every_registered_plugin_instantiates_as_itself() {
        for info in super::REGISTRY.iter() {
            let p = super::instantiate(info.id).unwrap_or_else(|| panic!("{} does not instantiate", info.id));
            assert_eq!(p.info().id, info.id);
        }
        assert!(super::instantiate("no_such_plugin").is_none());
    }
}
