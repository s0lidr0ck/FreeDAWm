//! PitchConfig -- the UI's tables: what each style is called, what it says about itself, which
//! knobs it shows and what they are called there, and how every value prints.
//! Translation of `Engine 3 - Pitch/Source/PitchConfig.h` (ported there from the PEDAL object and
//! the unit helpers in `PitchEngineUI.js`).
//!
//! The knob -> unit maps are not repeated: they are the engine's own ([`super::pitch_engine_dsp`]),
//! so a readout cannot drift away from what the DSP does.
//!
//! Translation notes: `juce::String (x, n)` is `format!("{x:.n$}")`; `juce::roundToInt` is
//! `f32::round` then the cast. They agree here: every value rounded is a finite, non-negative
//! knob map (Hz, ms, percent), where only an exact half could differ (`roundToInt` rounds it to
//! even, `round` away from zero), a display one unit off at worst.

use super::pitch_engine_dsp::{self as p, NUM_STYLES};

// ---------------------------------------------------------------- style table
pub const STYLES: [&str; 5] = ["POG", "Micro POG", "OC-2", "H910", "Whammy"];

/// One line under the style row, saying what you just picked.
pub fn style_blurb(style: i32) -> &'static str {
    const TEXT: [&str; 5] = [
        "Polyphonic octaves: sub, -1, +1, +2 and a 5th. Organ, 12-string, cello. Chords are fine.",
        "Sub and octave up only, on a short window: tight, fast, the one you leave on.",
        "Analog flip-flop divider: monophonic, gritty, glitches on chords on purpose. Bass and single notes.",
        "Doubler: two copies detuned a few cents each way, spread by Width. A second guitar player.",
        "One voice glides from unison to the Interval under the Pedal knob. Dry down = Whammy, Dry up = Harmony.",
    ];
    if (0..NUM_STYLES).contains(&style) { TEXT[style as usize] } else { "" }
}

// ---------------------------------------------------------------- what each style shows
/// The main knob row, per style, in order -- PEDAL.mainRow. A style shows only the controls it
/// uses; the rest are hidden, not greyed, which is the family's rule.
pub fn main_row_for(style: i32) -> &'static [usize] {
    match style {
        p::STYLE_POG => &[p::P_SUB, p::P_OCT_DOWN, p::P_DRY, p::P_OCT_UP, p::P_OCT2_UP, p::P_FIFTH],
        p::STYLE_MICRO_POG => &[p::P_SUB, p::P_DRY, p::P_OCT_UP],
        p::STYLE_OC2 => &[p::P_SUB, p::P_OCT_DOWN, p::P_DRY],
        p::STYLE_H910 => &[p::P_DRY, p::P_VOICE, p::P_DETUNE],
        _ => &[p::P_PEDAL, p::P_VOICE, p::P_DRY, p::P_GLIDE],
    }
}

/// Some knobs are called something else depending on the style -- PEDAL.labels. `None` when a
/// style does not rename it (the C++ then returns the parameter's own name).
pub fn knob_label(style: i32, param_index: usize) -> Option<&'static str> {
    if style == p::STYLE_POG || style == p::STYLE_MICRO_POG {
        if param_index == p::P_SUB {
            return Some("Sub");
        }
        if param_index == p::P_VOICE {
            return Some("Voice");
        }
    } else if style == p::STYLE_OC2 {
        if param_index == p::P_SUB {
            return Some("-2 Oct");
        }
        if param_index == p::P_OCT_DOWN {
            return Some("-1 Oct");
        }
        if param_index == p::P_DRY {
            return Some("Direct");
        }
        if param_index == p::P_VOICE {
            return Some("Voice");
        }
    } else if style == p::STYLE_H910 {
        if param_index == p::P_VOICE {
            return Some("Doubles");
        }
        if param_index == p::P_DETUNE {
            return Some("Spread");
        }
    } else if style == p::STYLE_WHAMMY {
        if param_index == p::P_VOICE {
            return Some("Whammy");
        }
        if param_index == p::P_DRY {
            return Some("Dry");
        }
    }
    None
}

/// Controls outside the main row that only some styles use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applicable {
    /// POG only, and only when the 5th is actually up.
    pub fifth_mode: bool,
    /// Whammy.
    pub interval: bool,
    pub pedal: bool,
    pub glide: bool,
    /// OC-2.
    pub track: bool,
    /// Every shifting style; OC-2 does not shift.
    pub grain: bool,
    /// POG (octave sharpening) and H910 (spread).
    pub detune: bool,
    pub attack: bool,
    pub width: bool,
}

pub fn what_applies(style: i32, fifth_knob: f32) -> Applicable {
    Applicable {
        fifth_mode: style == p::STYLE_POG && fifth_knob > 0.001,
        interval: style == p::STYLE_WHAMMY,
        pedal: style == p::STYLE_WHAMMY,
        glide: style == p::STYLE_WHAMMY,
        track: style == p::STYLE_OC2,
        grain: style != p::STYLE_OC2,
        detune: style == p::STYLE_POG || style == p::STYLE_H910,
        attack: true,
        width: style != p::STYLE_OC2, // the divider is mono by nature
    }
}

/// Whether the editor shows a parameter for this style (`refreshAll`): the main row's knobs, the
/// shaping row's (Attack, Filter, Q, Input and Level always; Width, Grain and Track where they
/// apply), Detune where it applies, and the 5th Mode and Interval switch rows. Style is always
/// shown; the engine's own Bypass is the header pill.
pub fn shown_for(style: i32, fifth_knob: f32, param_index: usize) -> bool {
    let a = what_applies(style, fifth_knob);
    if main_row_for(style).contains(&param_index) {
        return true;
    }
    match param_index {
        p::P_STYLE | p::P_BYPASS => true,
        p::P_LPF | p::P_RESO | p::P_TRIM | p::P_LEVEL => true,
        p::P_ATTACK => a.attack,
        p::P_WIDTH => a.width,
        p::P_GRAIN => a.grain,
        p::P_TRACK => a.track,
        p::P_DETUNE => a.detune,
        p::P_FIFTH_MODE => a.fifth_mode,
        p::P_INTERVAL => a.interval,
        _ => false,
    }
}

// ---------------------------------------------------------------- value readouts
pub fn format_hz(hz: f64) -> String {
    if hz >= 10000.0 {
        return format!("{:.1} k", hz / 1000.0);
    }
    if hz >= 1000.0 {
        return format!("{:.2} k", hz / 1000.0);
    }
    format!("{} Hz", hz.round() as i64)
}

/// Voice knobs are an audio taper with 0.7071 at unity -- dry, level, and every voice.
pub fn format_voice_db(v: f32) -> String {
    let g = p::knob_to_gain(v);
    if g <= 0.0 {
        return "Off".to_string();
    }
    let db = 20.0 * g.log10();
    if db < -59.0 {
        return "Off".to_string();
    }
    if (-0.05..=0.05).contains(&db) { "0.0 dB".to_string() } else { format!("{}{db:.1} dB", if db > 0.0 { "+" } else { "" }) }
}

fn signed_db(db: f32) -> String {
    if db.abs() < 0.05 {
        return "+0.0 dB".to_string();
    }
    format!("{}{db:.1} dB", if db > 0.0 { "+" } else { "" })
}

/// How a parameter prints under its knob.
pub fn format_value(index: usize, v: f32) -> String {
    match index {
        p::P_DRY | p::P_LEVEL | p::P_SUB | p::P_OCT_DOWN | p::P_OCT_UP | p::P_OCT2_UP | p::P_FIFTH | p::P_VOICE => format_voice_db(v),
        p::P_TRIM => signed_db(p::knob_to_trim_db(v)),
        p::P_DETUNE => format!("{:.1} \u{a2}", p::knob_to_cents(v)),
        p::P_ATTACK => {
            let ms = p::knob_to_attack_ms(v);
            if ms <= 0.0 {
                return "Off".to_string();
            }
            if ms >= 1000.0 { format!("{:.2} s", ms / 1000.0) } else { format!("{} ms", ms.round() as i64) }
        }
        p::P_LPF => {
            if v >= 0.995 {
                "Open".to_string()
            } else {
                format_hz(f64::from(p::knob_to_filter_hz(v)))
            }
        }
        p::P_RESO => format!("{:.2}", p::knob_to_q(v)),
        p::P_GLIDE => {
            let ms = p::knob_to_glide_ms(v);
            if ms < 1.0 { "Off".to_string() } else { format!("{} ms", ms.round() as i64) }
        }
        p::P_WIDTH => format!("{} %", (v * 100.0).round() as i64),
        p::P_GRAIN => format!("x {:.2}", p::knob_to_grain(v)),
        p::P_TRACK => format_hz(f64::from(p::knob_to_track_hz(v))),
        p::P_PEDAL => format!("{} %", (v * 100.0).round() as i64),
        _ => format!("{}", (v * 10.0).round() as i64),
    }
}

/// What the Whammy is actually doing right now: heel to toe, in semitones.
pub fn whammy_readout(interval_idx: i32, pedal: f32) -> String {
    let semis = p::interval_semis(interval_idx) * pedal;
    let name = super::INTERVALS[interval_idx.clamp(0, 9) as usize];
    if pedal < 0.005 {
        return format!("Unison  \u{2192}  {name}");
    }
    format!("{semis:.1} semi  of  {name}")
}

#[cfg(test)]
#[allow(clippy::approx_constant)] // 0.7071: the C++ unity knob value
mod tests {
    use super::*;

    #[test]
    fn formats_like_the_editor() {
        assert_eq!(format_value(p::P_DRY, 0.7071), "0.0 dB");
        assert_eq!(format_value(p::P_SUB, 0.0), "Off");
        assert_eq!(format_value(p::P_OCT_UP, 1.0), "+6.0 dB");
        assert_eq!(format_value(p::P_OCT_UP, 0.5), "-6.0 dB");
        assert_eq!(format_value(p::P_TRIM, 0.3333), "+0.0 dB");
        assert_eq!(format_value(p::P_TRIM, 0.0), "-12.0 dB");
        assert_eq!(format_value(p::P_DETUNE, 0.25), "12.5 \u{a2}");
        assert_eq!(format_value(p::P_ATTACK, 0.0), "Off");
        assert_eq!(format_value(p::P_ATTACK, 1.0), "1.50 s");
        assert_eq!(format_value(p::P_LPF, 1.0), "Open");
        assert_eq!(format_value(p::P_LPF, 0.0), "300 Hz");
        assert_eq!(format_value(p::P_RESO, 0.0), "0.50");
        assert_eq!(format_value(p::P_GLIDE, 0.0), "Off");
        assert_eq!(format_value(p::P_GLIDE, 0.2), "20 ms");
        assert_eq!(format_value(p::P_WIDTH, 0.35), "35 %");
        assert_eq!(format_value(p::P_GRAIN, 0.5), "x 1.00");
        assert_eq!(format_value(p::P_TRACK, 1.0), "2.00 k");
        assert_eq!(format_value(p::P_PEDAL, 0.5), "50 %");
        assert_eq!(format_hz(12_345.0), "12.3 k");
        assert_eq!(whammy_readout(1, 0.0), "Unison  \u{2192}  +1 Oct");
        assert_eq!(whammy_readout(1, 0.5), "6.0 semi  of  +1 Oct");
    }

    #[test]
    fn styles_show_what_the_editor_shows() {
        // POG: the main row, Detune, the shaping row, no Whammy/OC-2 controls; 5th Mode only with
        // the 5th up.
        assert!(shown_for(p::STYLE_POG, 0.0, p::P_OCT2_UP));
        assert!(shown_for(p::STYLE_POG, 0.0, p::P_DETUNE));
        assert!(!shown_for(p::STYLE_POG, 0.0, p::P_FIFTH_MODE));
        assert!(shown_for(p::STYLE_POG, 0.5, p::P_FIFTH_MODE));
        assert!(!shown_for(p::STYLE_POG, 0.5, p::P_VOICE));
        assert!(!shown_for(p::STYLE_POG, 0.5, p::P_TRACK));
        // Micro POG: sub, dry, octave up.
        assert!(!shown_for(p::STYLE_MICRO_POG, 0.5, p::P_OCT_DOWN));
        assert!(!shown_for(p::STYLE_MICRO_POG, 0.5, p::P_DETUNE));
        // OC-2: no grain, no width, a track knob.
        assert!(shown_for(p::STYLE_OC2, 0.0, p::P_TRACK));
        assert!(!shown_for(p::STYLE_OC2, 0.0, p::P_GRAIN));
        assert!(!shown_for(p::STYLE_OC2, 0.0, p::P_WIDTH));
        assert_eq!(knob_label(p::STYLE_OC2, p::P_DRY), Some("Direct"));
        // H910: voice as Doubles, detune as Spread.
        assert!(shown_for(p::STYLE_H910, 0.0, p::P_VOICE));
        assert_eq!(knob_label(p::STYLE_H910, p::P_DETUNE), Some("Spread"));
        // Whammy: pedal, glide, interval.
        for i in [p::P_PEDAL, p::P_GLIDE, p::P_INTERVAL, p::P_VOICE] {
            assert!(shown_for(p::STYLE_WHAMMY, 0.0, i));
        }
        assert!(!shown_for(p::STYLE_WHAMMY, 0.0, p::P_SUB));
        // always
        for s in 0..NUM_STYLES {
            for i in [p::P_STYLE, p::P_LPF, p::P_RESO, p::P_TRIM, p::P_LEVEL, p::P_DRY, p::P_ATTACK] {
                assert!(shown_for(s, 0.0, i), "style {s} param {i}");
            }
        }
        assert_eq!(style_blurb(9), "");
    }
}
