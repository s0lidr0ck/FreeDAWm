//! Shale's parameter text and choice lists, from `Shale/Source/Parameters.h` (`formatParam`,
//! `kInstrumentChoices`, `kLookaheadChoices` and the choice strings of `getParamDefs`).
//!
//! `formatParam` reads the DSP for four knobs: Length and Hold print through the selected mic's
//! table row, Crossover and the detector band through `getXoverHz` / `getDetLowHz` /
//! `getDetHighHz`, which follow the mic while Auto Band is on. [`format_param`] takes those from
//! the module's own values (`values`, in parameter order), so the drawer prints exactly what the
//! C++ panel prints; [`format_alone`] is the context-free text a bare `ParamInfo` gives (the
//! default Snare Top, each knob through its own map).

use super::shale_align::knob_to_offset_ms;
use super::shale_dsp as d;

/// `kInstrumentChoices`.
pub const INSTRUMENTS: [&str; 11] =
    ["Kick In", "Kick Out", "Sub Kick", "Snare Top", "Snare Btm", "Rack Tom", "Floor Tom", "Hi-Hat", "Ride", "Overheads", "Room"];
/// `kLookaheadChoices`.
pub const LOOKAHEADS: [&str; 5] = ["Off", "1 ms", "2 ms", "3 ms", "5 ms"];
pub const CLOSE_MODES: [&str; 2] = ["Decay Match", "Fixed"];
pub const DEBLEED_MODES: [&str; 2] = ["Shelf", "Learned"];
pub const SC_SOURCES: [&str; 2] = ["Internal", "Sidechain"];
pub const MONITORS: [&str; 3] = ["Output", "Bleed", "Detector"];
pub const LINK_GROUPS: [&str; 4] = ["A", "B", "C", "D"];
pub const LINK_ROLES: [&str; 3] = ["Lead", "Follow", "Pair"];
pub const ALIGN_TOS: [&str; 12] =
    ["Group Ref", "Kick In", "Kick Out", "Sub Kick", "Snare Top", "Snare Btm", "Rack Tom", "Floor Tom", "Hi-Hat", "Ride", "Overheads", "Room"];

/// `juce::roundToInt` (round half to even, the FPU's way).
pub fn round_to_int(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// The `hz` lambda: "750 Hz", "1.80 k", "12.0 k".
pub fn hz(f: f32) -> String {
    if f >= 1000.0 {
        if f < 10000.0 { format!("{:.2} k", f / 1000.0) } else { format!("{:.1} k", f / 1000.0) }
    } else {
        format!("{} Hz", round_to_int(f64::from(f)))
    }
}

/// What the DSP getters `formatParam` reads would return for these values (in parameter order;
/// a missing one at its default).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Context {
    pub instrument: i32,
    pub auto_band: bool,
    pub det_low_knob: f32,
    pub det_high_knob: f32,
    pub xover_knob: f32,
}

impl Default for Context {
    fn default() -> Self {
        Context { instrument: d::I_SNARE_TOP, auto_band: true, det_low_knob: 0.4, det_high_knob: 0.6, xover_knob: 0.5 }
    }
}

impl Context {
    /// From values in parameter order (clamped by the caller).
    pub fn from_values(values: &[f32]) -> Self {
        let dflt = Context::default();
        let get = |i: usize| values.get(i).copied().filter(|v| v.is_finite());
        Context {
            instrument: get(d::P_INSTRUMENT).map_or(dflt.instrument, |v| (v.round() as i32).clamp(0, d::NUM_INSTRUMENTS - 1)),
            auto_band: get(d::P_AUTO_BAND).map_or(dflt.auto_band, |v| v > 0.5),
            det_low_knob: get(d::P_DET_LOW).map_or(dflt.det_low_knob, |v| v.clamp(0.0, 1.0)),
            det_high_knob: get(d::P_DET_HIGH).map_or(dflt.det_high_knob, |v| v.clamp(0.0, 1.0)),
            xover_knob: get(d::P_XOVER).map_or(dflt.xover_knob, |v| v.clamp(0.0, 1.0)),
        }
    }

    /// `getDetLowHz`.
    pub fn det_low_hz(&self) -> f32 {
        if self.auto_band { d::mode(self.instrument).det_low_hz } else { d::knob_to_det_hz(self.det_low_knob) }
    }
    /// `getDetHighHz`.
    pub fn det_high_hz(&self) -> f32 {
        if self.auto_band { d::mode(self.instrument).det_high_hz } else { d::knob_to_det_hz(self.det_high_knob) }
    }
    /// `getXoverHz`.
    pub fn xover_hz(&self) -> f32 {
        if self.auto_band { d::mode(self.instrument).xover_hz } else { d::knob_to_xover_hz(self.xover_knob) }
    }
}

fn pct(v: f32) -> String {
    format!("{} %", round_to_int(f64::from(v * 100.0)))
}

/// `formatParam (index, v, dsp)` for the knobs it prints in units; `None` for a choice or a
/// switch (their labels are the parameter's own).
pub fn format_param(index: usize, v: f32, ctx: &Context) -> Option<String> {
    Some(match index {
        d::P_THRESHOLD => format!("{:.1} dB", d::knob_to_thresh_db(v)),
        d::P_SENS => format!("{:.1} dB", d::knob_to_flux_db(v)),
        d::P_REDUCTION => {
            if d::is_full_mute(v) {
                "Mute".to_string()
            } else {
                format!("{:.1} dB", d::knob_to_red_db(v))
            }
        }
        // Length is not a time: how far below THIS hit's own peak the gate follows it down.
        d::P_LENGTH => format!("{:.1} dB", -d::follow_db(ctx.instrument, v)),
        d::P_HOLD => format!("{:.0} ms", d::mode(ctx.instrument).min_hold_ms * d::knob_to_hold_scale(v)),
        d::P_RETRIG => format!("{:.0} ms", d::knob_to_retrig_ms(v)),
        d::P_DEBLEED => {
            if v <= 0.0005 {
                "Off".to_string()
            } else {
                pct(v)
            }
        }
        // `getXoverHz` reads the knob being printed (the DSP holds this same value).
        d::P_XOVER => hz(Context { xover_knob: v, ..*ctx }.xover_hz()),
        d::P_VEL_TRACK | d::P_MIX => pct(v),
        d::P_DET_LOW => hz(Context { det_low_knob: v, ..*ctx }.det_low_hz()),
        d::P_DET_HIGH => hz(Context { det_high_knob: v, ..*ctx }.det_high_hz()),
        d::P_TRIM => format!("{:.1} dB", d::knob_to_trim_db(v)),
        d::P_OUTPUT => format!("{:.1} dB", d::knob_to_out_db(v)),
        d::P_ALIGN_OFFSET => {
            let ms = knob_to_offset_ms(f64::from(v));
            format!("{}{ms:.2} ms", if ms > 0.0005 { "+" } else { "" })
        }
        d::P_REJECT => {
            if v <= 0.0005 {
                "Off".to_string()
            } else {
                format!("{:.0} dB", 20.0 * (1.0 - v))
            }
        }
        d::P_MIDI_NOTE => {
            const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
            let n = d::knob_to_midi_note(v);
            format!("{}{} ({n})", NAMES[n.rem_euclid(12) as usize], n.div_euclid(12) - 1)
        }
        _ => return None,
    })
}

/// The text of a knob with nothing else known: the default mic (Snare Top), and Crossover and
/// the detector band through their own knob maps (as with Auto Band off).
pub fn format_alone(index: usize, v: f32) -> String {
    let ctx = Context { auto_band: false, ..Context::default() };
    format_param(index, v, &ctx).unwrap_or_else(|| format!("{v:.2}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_match_format_param() {
        let c = Context::default();
        let f = |i, v| format_param(i, v, &c).unwrap();
        assert_eq!(f(d::P_THRESHOLD, 0.55), "-29.7 dB");
        assert_eq!(f(d::P_SENS, 0.5), "5.5 dB");
        assert_eq!(f(d::P_REDUCTION, 1.0), "Mute");
        assert_eq!(f(d::P_REDUCTION, 0.0), "-3.0 dB");
        // Snare Top follows 6..45 dB: 0.4 is 21.6 dB below the hit's peak.
        assert_eq!(f(d::P_LENGTH, 0.4), "-21.6 dB");
        // 22 ms x (0.35 + 1.9 x 0.35).
        assert_eq!(f(d::P_HOLD, 0.35), "22 ms");
        assert_eq!(f(d::P_RETRIG, 0.2), "15 ms");
        assert_eq!(f(d::P_DEBLEED, 0.0), "Off");
        assert_eq!(f(d::P_DEBLEED, 0.4), "40 %");
        // Auto Band: the mic's numbers, whatever the knobs say.
        assert_eq!(f(d::P_XOVER, 0.0), "750 Hz");
        assert_eq!(f(d::P_DET_LOW, 0.9), "170 Hz");
        let manual = Context { auto_band: false, ..c };
        assert_eq!(format_param(d::P_XOVER, 1.0, &manual).unwrap(), "15.0 k");
        assert_eq!(format_param(d::P_DET_LOW, 0.0, &manual).unwrap(), "25 Hz");
        let hat = Context { instrument: d::I_HAT, ..c };
        assert_eq!(format_param(d::P_DET_HIGH, 0.0, &hat).unwrap(), "9.00 k");
        assert_eq!(f(d::P_TRIM, 1.0 / 3.0), "0.0 dB");
        assert_eq!(f(d::P_OUTPUT, 1.0), "12.0 dB");
        assert_eq!(f(d::P_ALIGN_OFFSET, 0.5), "0.00 ms");
        assert_eq!(f(d::P_ALIGN_OFFSET, 0.75), "+5.00 ms");
        assert_eq!(f(d::P_ALIGN_OFFSET, 0.25), "-5.00 ms");
        assert_eq!(f(d::P_REJECT, 0.5), "10 dB");
        // (The C++ comment calls note 38 "D1"; its code prints n / 12 - 1 = 2.)
        assert_eq!(f(d::P_MIDI_NOTE, 0.1667), "D2 (38)");
        assert!(format_param(d::P_INSTRUMENT, 3.0, &c).is_none());
    }

    #[test]
    fn context_follows_the_values() {
        let mut v = [0.0f32; d::NUM_PARAMS];
        v[d::P_INSTRUMENT] = 99.0;
        v[d::P_AUTO_BAND] = 0.0;
        v[d::P_XOVER] = f32::NAN;
        let c = Context::from_values(&v);
        assert_eq!(c.instrument, d::I_ROOM);
        assert!(!c.auto_band);
        assert_eq!(c.xover_knob, 0.5);
        assert_eq!(c.det_low_hz(), 25.0);
    }
}
