//! What each built-in plugin is, in a few words: the subtitle shown after its name in the
//! picker and under the drawer's title ("Swale - Volume Swells"), so a name that says nothing
//! by itself still tells a newcomer what it does. The insert slots keep the short name.

use soundcraft_dsp::PluginInfo;

/// The subtitle of plugin `id` ("" for one whose name already says it, or an unknown id).
pub fn subtitle(id: &str) -> &'static str {
    match id {
        "fs_amp" => "Guitar & Bass Amps",
        "fs_chamber" => "Build-a-Room Reverb",
        "fs_cleanq" => "Dynamic EQ",
        "fs_comp" => "Multi-Style Compressor",
        "fs_copyq" => "Match EQ",
        "fs_delay" => "Digital, Tape & Analog Echo",
        "fs_dist" => "Distortion Pedals",
        "fs_filter" => "Envelope & Auto Filter",
        "fs_levelup" => "Loudness Matching & Metering",
        "fs_manners" => "Automatic Balance EQ",
        "fs_mod" => "Chorus, Flanger, Phaser, Tremolo",
        "fs_pitch" => "Octaver & Harmonizer",
        "fs_reverb" => "Room, Hall, Plate & Spring",
        "fs_sat" => "Harmonic Saturation",
        "fs_shale" => "Drum Gate",
        "fs_sift" => "De-Esser",
        "fs_swale" => "Volume Swells",
        "fs_tape" => "Tape Machine Colour",
        "fs_transient" => "Transient Shaper",
        "fs_twine" => "Clipper & Limiter",
        "fs_vint" => "Vintage EQ & Enhancers",
        "rez" => "Resonance Suppressor",
        "lull" => "Gate & Expander",
        "sprawl" => "Stereo Imager",
        "keel" => "Bass Amp",
        "heft" => "Bass Enhancer",
        "vault" => "Convolution Reverb",
        "hw_insert" => "Outboard Gear in the Chain",
        "mend" => "Vocal Cleaner",
        "imprint" => "Impulse Response Capture",
        "subtractive_synth" => "Analog-Style Synth",
        "wavetable_synth" => "Morphing Tables",
        "fm_synth" => "Four-Operator Synth",
        "time_shift" => "Track Alignment",
        "trim" => "Input Level for Gain Staging",
        "gain" => "Clean Level Change",
        "dither" => "Bit-Depth Reduction for the Final Bounce",
        "channel_tool" => "Swap, Mono, Mid/Side & Polarity",
        "spectrum" => "Spectrum Analyser",
        "loudness" => "LUFS & True Peak",
        "phase_meter" => "Correlation & Goniometer",
        "tuner" => "Chromatic Tuner",
        "invert" => "Polarity Flip",
        "dc_offset_removal" => "",
        "signal_generator" => "Test Tones & Noise",
        "lofi" => "Bit & Sample-Rate Reducer",
        _ => "",
    }
}

/// A plugin's name with its subtitle: "Name - Subtitle". A name that carries one already
/// ("Lift - Sampler", "Juke — Drum Machine") or has none to add is given as it is.
pub fn titled(info: &PluginInfo) -> String {
    let sub = subtitle(info.id);
    if sub.is_empty() || info.name.contains(" - ") || info.name.contains(" — ") { info.name.to_string() } else { format!("{} - {sub}", info.name) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_plugin_the_lists_offer_says_what_it_is() {
        for p in soundcraft_dsp::listed_plugins() {
            let t = titled(p);
            // DC Offset Removal's name is its own description.
            assert!(t.contains(" - ") || t.contains(" — ") || p.id == "dc_offset_removal", "{} has no subtitle", p.id);
            assert!(t.starts_with(p.name), "{t}");
            assert!(!subtitle(p.id).eq_ignore_ascii_case(p.name), "{}: the subtitle only repeats the name", p.id);
        }
        assert_eq!(soundcraft_dsp::plugin_info("fs_swale").map(titled).as_deref(), Some("Swale - Volume Swells"));
        assert_eq!(soundcraft_dsp::plugin_info("sampler").map(titled).as_deref(), Some("Lift - Sampler"));
        assert_eq!(subtitle("clap:whatever"), "");
    }
}
