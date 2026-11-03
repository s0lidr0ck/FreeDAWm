//! The UI's tables, translated from `Swale/Source/SwaleConfig.h` (and the choice lists of
//! `Parameters.h`): the mode names and blurbs, how every value prints, which controls apply in
//! the current mode, and the stage lengths exactly as the DSP derives them. The unit maps are the
//! engine's own ([`super::swale_dsp`]), so a readout cannot drift from what the DSP does.

use super::swale_dsp as d;

/// The mode menu.
pub const MODES: [&str; 3] = ["Pick", "One-Shot", "Rhythmic"];
/// The note menu, long to short (Attack, Release and Rate).
pub const NOTES: [&str; 18] = [
    "4 Bars", "2 Bars", "1/1 D", "1/1", "1/2 D", "1/1 T", "1/2", "1/4 D", "1/2 T", "1/4", "1/8 D", "1/4 T", "1/8", "1/16 D", "1/8 T", "1/16",
    "1/16 T", "1/32",
];
/// Hold's note menu: the same plus "None", the hold stage switched off.
pub const HOLD_NOTES: [&str; 19] = [
    "4 Bars", "2 Bars", "1/1 D", "1/1", "1/2 D", "1/1 T", "1/2", "1/4 D", "1/2 T", "1/4", "1/8 D", "1/4 T", "1/8", "1/16 D", "1/8 T", "1/16",
    "1/16 T", "1/32", "None",
];
pub const CURVES: [&str; 4] = ["Linear", "Exp", "Log", "S-Curve"];
pub const VCAS: [&str; 2] = ["Clean", "Slow Gear"];
pub const TEMPO_SOURCES: [&str; 2] = ["Host", "Manual"];
pub const PHRASES: [&str; 2] = ["Per Note", "Phrase"];

/// `modeBlurb`: what the current mode does.
pub fn mode_blurb(mode: i32) -> &'static str {
    match mode {
        0 => {
            "Rests closed. A picked note opens it over Attack, it stays open while the note sustains, \
             Hold is how long it hangs on after the note drops away, then Release closes it."
        }
        1 => "Every pick fires one fixed Attack / Hold / Release shape, however long the note rings.",
        2 => {
            "No trigger. The shape repeats once per Rate, locked to the host's bar position. \
             Peak on Beat puts the top of the swell on the beat instead of the start."
        }
        _ => "",
    }
}

/// `phraseBlurb`: Pick mode's two characters.
pub fn phrase_blurb(phrase: i32) -> &'static str {
    match phrase {
        0 => "Per Note \u{2014} every new pick shuts it and swells again. The Slow Gear / violin effect: play separated notes.",
        1 => {
            "Phrase \u{2014} the first note swells in, then it stays open while you keep playing. \
             It lets go once your playing drops under the line, after Hang."
        }
        _ => "",
    }
}

/// `curveBlurb`: what the current curve does (the editor's curve row).
pub fn curve_blurb(curve: i32) -> &'static str {
    match curve {
        0 => "Linear \u{2014} an even fade. Sounds faster than it is, because the ear hears most of the rise early.",
        1 => "Exp \u{2014} even in dB. Starts slow and arrives late: the bowed-note swell.",
        2 => "Log \u{2014} jumps up and eases into the top. Takes the pick off without sounding like a fade.",
        3 => "S-Curve \u{2014} eases out of silence and into the top. The volume-pedal sweep.",
        _ => "",
    }
}

/// `vcaBlurb`: what the VCA choice does (the editor's More panel).
pub fn vca_blurb(vca: i32) -> &'static str {
    match vca {
        0 => "Clean \u{2014} a plain gain change. Closes to silence (or to Floor).",
        1 => {
            "Slow Gear \u{2014} JFET shunt attenuator, solved from the FET equations. Closes to about \
             \u{2212}44 dB, and loud notes pick up a little grit as it comes through pinch-off."
        }
        _ => "",
    }
}

/// `juce::roundToInt` (the FPU's round-half-to-even).
fn round_to_int(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// `formatSeconds`: "300 ms" / "1.25 s".
pub fn format_seconds(s: f64) -> String {
    if s <= 0.0 || s.is_nan() {
        return "0 ms".to_string();
    }
    if s < 1.0 {
        return format!("{} ms", round_to_int(s * 1000.0));
    }
    format!("{s:.2} s")
}

fn pct(x: f32) -> String {
    format!("{} %", round_to_int(f64::from(x) * 100.0))
}

/// Unity prints as +0.0 dB, never -0.0.
fn signed_db(db: f32) -> String {
    if db.abs() < 0.05 {
        return "+0.0 dB".to_string();
    }
    format!("{}{db:.1} dB", if db > 0.0 { "+" } else { "" })
}

/// `formatValue`: how a parameter prints under its knob.
pub fn format_value(index: usize, v: f32) -> String {
    let note = |list: &[&str]| list.get(round_to_int(f64::from(v)).clamp(0, list.len() as i64 - 1) as usize).copied().unwrap_or("").to_string();
    match index {
        d::P_ATTACK | d::P_RELEASE => format_seconds(d::knob_to_stage_sec(v)),
        d::P_HOLD => format_seconds(d::knob_to_hold_sec(v)),
        d::P_SENS => format!("{} dB", round_to_int(f64::from(d::knob_to_sens_db(v)))),
        d::P_MIX | d::P_BLOOM | d::P_ALTERNATE => pct(v),
        d::P_LEVEL => signed_db(d::knob_to_level_db(v)),
        d::P_TRIM => signed_db(d::knob_to_trim_db(v)),
        d::P_RETRIG => format!("{:.1}", v * 10.0),
        d::P_DET_HP => format!("{} Hz", round_to_int(f64::from(d::knob_to_det_hp_hz(v)))),
        d::P_FLOOR if v < 0.005 => "Silent".to_string(),
        d::P_FLOOR => format!("{} dB", round_to_int(f64::from(d::knob_to_floor_db(v)))),
        d::P_BPM => format!("{}", round_to_int(f64::from(d::knob_to_bpm(v)))),
        d::P_ATK_NOTE | d::P_REL_NOTE | d::P_RATE => note(&NOTES),
        d::P_HOLD_NOTE => note(&HOLD_NOTES),
        _ => format!("{v:.2}"),
    }
}

/// What applies right now (`Applicable`): controls that do not apply to the mode are hidden; the
/// two detector knobs are dimmed in place in Rhythmic, which has no trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applicable {
    pub attack_time: bool,
    pub attack_note: bool,
    pub hold_time: bool,
    pub hold_note: bool,
    pub release_time: bool,
    pub release_note: bool,
    pub sens: bool,
    pub rate: bool,
    pub peak_beat: bool,
    pub phrase: bool,
    pub bpm_knob: bool,
    pub dim_detector: bool,
    /// Pick calls Hold "Hang": it is how long it waits, not how long it holds.
    pub hold_label: &'static str,
}

/// `whatApplies`.
pub fn what_applies(mode: i32, atk_sync: bool, hold_sync: bool, rel_sync: bool, manual_tempo: bool, have_host_tempo: bool) -> Applicable {
    let rhythmic = mode == 2;
    Applicable {
        attack_time: !atk_sync,
        attack_note: atk_sync,
        hold_time: !hold_sync,
        hold_note: hold_sync,
        release_time: !rel_sync,
        release_note: rel_sync,
        sens: !rhythmic,
        rate: rhythmic,
        peak_beat: rhythmic,
        phrase: mode == 0,
        dim_detector: rhythmic,
        hold_label: if mode == 0 { "Hang" } else { "Hold" },
        // The BPM knob is shown when it is doing something: on Manual, or with no host tempo.
        bpm_knob: manual_tempo || !have_host_tempo,
    }
}

/// The three stage lengths and the Rhythmic cycle in seconds (`StageTimes`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StageTimes {
    pub attack: f64,
    pub hold: f64,
    pub release: f64,
    pub cycle: f64,
    /// The shape was longer than the Rate and had to be scaled.
    pub fitted: bool,
}

/// `stageTimes`: exactly as the DSP derives them, including the squeeze that fits an over-long
/// shape into one Rhythmic cycle.
#[allow(clippy::too_many_arguments)]
pub fn stage_times(
    mode: i32,
    bpm: f32,
    atk_sync: bool,
    attack_knob: f32,
    atk_note: i32,
    hold_sync: bool,
    hold_knob: f32,
    hold_note: i32,
    rel_sync: bool,
    release_knob: f32,
    rel_note: i32,
    rate: i32,
) -> StageTimes {
    // jmax (20.0f, bpm)
    let bpm = if bpm < 20.0 || bpm.is_nan() { 20.0 } else { bpm };
    let beat = 60.0 / f64::from(bpm);
    let mut t = StageTimes {
        attack: if atk_sync { f64::from(d::note_beats(atk_note)) * beat } else { d::knob_to_stage_sec(attack_knob) },
        release: if rel_sync { f64::from(d::note_beats(rel_note)) * beat } else { d::knob_to_stage_sec(release_knob) },
        hold: if hold_sync { f64::from(d::note_beats(hold_note)) * beat } else { d::knob_to_hold_sec(hold_knob) },
        cycle: f64::from(d::note_beats(rate)) * beat,
        fitted: false,
    };
    let total = t.attack + t.hold + t.release;
    if mode == 2 && total > t.cycle && total > 0.0 {
        let k = t.cycle / total;
        t.attack *= k;
        t.hold *= k;
        t.release *= k;
        t.fitted = true;
    }
    t
}
