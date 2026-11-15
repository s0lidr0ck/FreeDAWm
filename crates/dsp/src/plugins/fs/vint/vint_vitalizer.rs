//! VintVitalizer -- the SPL Vitalizer's process, after its patent and its manuals.
//!
//! Not a translation (the C++ VintEngine's Vitalizer is a bell cut and two soft clips), and NOT a
//! component-level model either: no drawing of a Vitalizer with its part values is public. What
//! is public, and what this is built from:
//!
//! - SPL's patent, US 7,352,872 B2 ("Filter circuit and method for processing an audio signal"),
//!   Fig. 2: the STRUCTURE. A reference path into an output summing amp; beside it a filter path
//!   through a drive stage, a bass stage that is "a frequency-dependent resistor to ground" at
//!   about 50 Hz, a state-variable ("universal") filter whose one control sets where the mids are
//!   lowered and the top raised, a control that sets how much of all that reaches the summing
//!   amp, and a separate treble / harmonics filter into the same amp. The patent prints no part
//!   value (only "1 kΩ to 100 kΩ, for example") and draws the treble filter as an empty box.
//! - SPL's manuals for the Vitalizer Mk3-T (hardware and plug-in): the CONTROLS and their ranges.
//!   Drive -20 .. +6 dB into the filter network; Bass Sound, nothing at its centre, Soft to the
//!   left and Tight to the right; Bass Comp, a soft-knee compressor in the bass path alone whose
//!   one knob raises the ratio and lowers the threshold; Mid-Hi Tune 1.1 .. 22 kHz, where a broad
//!   shelf starts; Process, how much of the bass and mid-hi processing joins the original; LC-EQ,
//!   a passive coil-and-capacitor stage, about 2 .. 20 kHz, with its own Intensity; a Stereo
//!   Expander through a valve.
//!
//! So: the three filters are circuits, solved on `vint_mna` as the engine's others are, laid out
//! as the patent's blocks are -- but EVERY PART VALUE IS OURS, chosen to land on the frequencies
//! the patent and the manuals state. The amounts (how many dB each control reaches), the
//! compressor's law and times, the coil's saturation and the expander are assumptions too: they
//! are gathered below, each marked. Nothing here has been checked against a Vitalizer.

use super::vint_mna::{Kind, Netlist};

// ---------------------------------------------------------------- the controls' ranges (the manuals')
/// The Drive knob's printed scale on the Mk3-T, in dB, at eleven even steps of its travel.
pub const DRIVE_SCALE_DB: [f32; 11] = [-20.0, -13.0, -8.0, -5.0, -2.0, 0.0, 1.5, 3.0, 4.0, 5.0, 6.0];
/// The Mid-Hi Tune knob's printed scale, in kHz, likewise.
pub const TUNE_SCALE_KHZ: [f32; 11] = [22.0, 14.0, 8.5, 6.0, 4.0, 2.8, 2.2, 1.8, 1.5, 1.3, 1.1];

/// A knob's position (0..1) on a scale printed at even steps: (the step below, how far past it).
fn on_scale(v: f32) -> (usize, f32) {
    let p = v.clamp(0.0, 1.0) * 10.0;
    let i = (p as usize).min(9);
    (i, p - i as f32)
}
/// Drive, as the knob's scale reads: -20 dB at minimum, 0 dB at the centre, +6 dB at maximum.
pub fn drive_db(v: f32) -> f32 {
    let (i, t) = on_scale(v);
    DRIVE_SCALE_DB[i] + t * (DRIVE_SCALE_DB[i + 1] - DRIVE_SCALE_DB[i])
}
/// Mid-Hi Tune, as the knob's scale reads: where the shelf starts, 22 kHz at the knob's left,
/// 2.8 kHz at its centre, 1.1 kHz hard right.
pub fn tune_hz(v: f32) -> f32 {
    let (i, t) = on_scale(v);
    1000.0 * TUNE_SCALE_KHZ[i] * (TUNE_SCALE_KHZ[i + 1] / TUNE_SCALE_KHZ[i]).powf(t)
}
/// LC-EQ: about 2 kHz hard left to about 20 kHz hard right.
pub fn lc_hz(v: f32) -> f32 {
    2_000.0 * 10.0f32.powf(v.clamp(0.0, 1.0))
}
/// Bass Sound: (amount 0..1, tight). Nothing at the centre; Soft to the left, Tight to the right.
pub fn bass_sound(v: f32) -> (f64, bool) {
    let d = f64::from(v.clamp(0.0, 1.0)) - 0.5;
    ((2.0 * d.abs()).min(1.0), d > 0.0)
}

// ---------------------------------------------------------------- ASSUMED: amounts and laws
/// ASSUMED: the bass stage's resonance. The patent says "e.g. 50 Hz"; Soft sits under it and
/// broad, Tight over it and narrower.
pub const K_SOFT_HZ: f64 = 45.0;
pub const K_SOFT_Q: f64 = 0.6;
pub const K_TIGHT_HZ: f64 = 70.0;
pub const K_TIGHT_Q: f64 = 1.4;
/// ASSUMED: how much of the bass stage joins the sum at full Bass Sound and full Process
/// (+9.5 dB and +8 dB at the resonance).
pub const K_SOFT_GAIN: f64 = 2.0;
pub const K_TIGHT_GAIN: f64 = 1.5;
/// ASSUMED: at full Process the mids between the fixed corner and Mid-Hi Tune come down 4 dB and
/// the shelf above Mid-Hi Tune comes up 9 dB.
pub const K_MID_CUT: f64 = 0.37;
pub const K_SHELF: f64 = 1.8;
/// The lowered range starts here (the patent: "approximately 150 to 700 Hz").
pub const K_MID_FROM_HZ: f64 = 150.0;
/// ASSUMED: the state-variable filter's Q (the patent: "greatest bandwidth, smallest Q").
pub const K_FILTER_Q: f64 = 0.6;
/// ASSUMED: the LC stage's Q, and its level at full Intensity (+10 dB at its frequency).
pub const K_LC_Q: f64 = 1.0;
pub const K_LC_GAIN: f64 = 2.2;
/// ASSUMED: the bass compressor. The manuals give only its shape: soft knee, fixed times, one
/// knob for ratio (up) and threshold (down).
pub const K_COMP_ATTACK_S: f64 = 0.010;
pub const K_COMP_RELEASE_S: f64 = 0.150;
pub const K_COMP_KNEE_DB: f64 = 6.0;
/// ASSUMED: the Stereo Expander adds up to this much of the valve-coloured difference signal.
pub const K_WIDTH: f64 = 0.5;

// ---------------------------------------------------------------- the bass stage
/// The patent's block 6 as what it acts as: a resonant impedance to ground (its two op-amps are a
/// simulated inductor, here the inductor itself) fed through a resistor. A band-pass of gain 1.
pub fn bass_netlist(tight: bool) -> Netlist {
    const C: f64 = 1.0e-6;
    let (f0, q) = if tight { (K_TIGHT_HZ, K_TIGHT_Q) } else { (K_SOFT_HZ, K_SOFT_Q) };
    let w = 2.0 * std::f64::consts::PI * f0;
    let l = 1.0 / (w * w * C);
    let mut n = Netlist::default();
    n.add(Kind::R, 1, 2, q * (l / C).sqrt());
    n.add(Kind::C, 2, 0, C);
    n.add(Kind::L, 2, 0, l);
    n.out = 2;
    n
}

// ---------------------------------------------------------------- the universal filter
/// The filter's high-pass output (inverted: -in s^2 t^2 / D).
pub const N_HP: i32 = 4;
/// Its low-pass output through the fixed corner (inverted: the lowered mids).
pub const N_MID: i32 = 9;

/// The patent's block 7: three op-amps, a summer and two integrators, the band-pass fed back to
/// the summer's + input through a divider (its Q), both integrators' resistors on the Mid-Hi
/// Tune control. The low-pass output then passes a fixed C-R corner: what is left is the range
/// between that corner and Mid-Hi Tune.
pub fn filter_netlist(tune_hz: f64) -> Netlist {
    const R: f64 = 10.0e3;
    const C: f64 = 4.7e-9;
    const C_MID: f64 = 100.0e-9;
    let (minus, plus, hp, i1, bp, i2, lp, mid) = (2, 3, N_HP, 5, 6, 7, 8, N_MID);
    let r_tune = 1.0 / (2.0 * std::f64::consts::PI * tune_hz.max(20.0) * C);
    // D = s^2 t^2 + k s t + 1 with k = 3 d, d the divider: Q = 1 / k.
    let d = 1.0 / (3.0 * K_FILTER_Q);
    let mut n = Netlist::default();
    n.add(Kind::R, 1, minus, R);
    n.add(Kind::R, minus, hp, R);
    n.add(Kind::R, minus, lp, R);
    n.add(Kind::R, plus, bp, R * (1.0 - d) / d);
    n.add(Kind::R, plus, 0, R);
    n.add_op_amp(plus, minus, hp);
    n.add(Kind::R, hp, i1, r_tune);
    n.add(Kind::C, i1, bp, C);
    n.add_op_amp(0, i1, bp);
    n.add(Kind::R, bp, i2, r_tune);
    n.add(Kind::C, i2, lp, C);
    n.add_op_amp(0, i2, lp);
    n.add(Kind::C, lp, mid, C_MID);
    n.add(Kind::R, mid, 0, 1.0 / (2.0 * std::f64::consts::PI * K_MID_FROM_HZ * C_MID));
    n.out = mid;
    n
}

/// What the filter adds to the sum at full Process: the top raised, the mids lowered.
#[inline]
pub fn mid_hi(hp_node: f64, mid_node: f64) -> f64 {
    -K_SHELF * hp_node + K_MID_CUT * mid_node
}

// ---------------------------------------------------------------- the LC stage
/// A coil and a capacitor in parallel to ground, fed through a resistor: a band-pass of gain 1 at
/// the LC-EQ frequency. The control moves both (the Q stays).
pub fn lc_netlist(hz: f64) -> Netlist {
    const Z0: f64 = 1.0e3;
    let w = 2.0 * std::f64::consts::PI * hz.max(20.0);
    let mut n = Netlist::default();
    n.add(Kind::R, 1, 2, K_LC_Q * Z0);
    n.add(Kind::L, 2, 0, Z0 / w);
    n.add(Kind::C, 2, 0, 1.0 / (w * Z0));
    n.out = 2;
    n
}

/// ASSUMED: the coil saturating. Loud highs are held back, so that soft ones are not masked by
/// them (the manual, of Intensity); unity for small signals.
#[inline]
pub fn coil(v: f64) -> f64 {
    (1.5 * v).tanh() / 1.5
}

// ---------------------------------------------------------------- the bass compressor
/// The compressor's attack and release as one-pole steps at `fs`.
pub fn comp_steps(fs: f64) -> (f64, f64) {
    let step = |tau: f64| 1.0 - (-1.0 / (tau * fs)).exp();
    (step(K_COMP_ATTACK_S), step(K_COMP_RELEASE_S))
}

/// The compressor's gain for the bass path's level `env` (full scale 1) at knob `comp` (0..1):
/// ASSUMED threshold -12 .. -36 dBFS and ratio 1 .. 6, soft knee. 1 with the knob at minimum.
pub fn comp_gain(env: f64, comp: f64) -> f64 {
    let comp = comp.clamp(0.0, 1.0);
    if comp <= 0.0 || env <= 1.0e-9 {
        return 1.0;
    }
    let (threshold, ratio) = (-12.0 - 24.0 * comp, 1.0 + 5.0 * comp);
    let over = 20.0 * env.log10() - threshold;
    let half = K_COMP_KNEE_DB / 2.0;
    let slope = 1.0 / ratio - 1.0;
    let reduce = if over <= -half {
        0.0
    } else if over >= half {
        slope * over
    } else {
        slope * (over + half) * (over + half) / (2.0 * K_COMP_KNEE_DB)
    };
    10.0f64.powf(reduce / 20.0)
}

// ---------------------------------------------------------------- the stereo expander
/// ASSUMED: the valve the difference signal passes: a lopsided soft clip (even harmonics first),
/// unity for small signals.
#[inline]
fn valve(v: f64) -> f64 {
    const BIAS: f64 = 0.2;
    let k = 1.0 - BIAS.tanh() * BIAS.tanh();
    ((v + BIAS).tanh() - BIAS.tanh()) / k
}

/// The Stereo Expander on one sample pair: the difference of the channels, through the valve,
/// added to the left and taken from the right.
#[inline]
pub fn expand(left: f32, right: f32, width: f32) -> (f32, f32) {
    let w = f64::from(width.clamp(0.0, 1.0)) * K_WIDTH;
    if w <= 0.0 {
        return (left, right);
    }
    let side = valve(f64::from(left) - f64::from(right)) * w;
    ((f64::from(left) + side) as f32, (f64::from(right) - side) as f32)
}

#[cfg(test)]
mod tests {
    use super::super::vint_mna::ac;
    use super::*;
    use rustfft::num_complex::Complex;

    fn node(mut n: Netlist, out: i32, f: f64) -> Complex<f64> {
        n.out = out;
        ac(&n, f)
    }

    /// The whole process's response (drive 0 dB, no compression, the coil small-signal).
    fn response(f: f64, bass: f32, tune: f32, process: f64, lc: f32, intensity: f64) -> f64 {
        let (amount, tight) = bass_sound(bass);
        let b = ac(&bass_netlist(tight), f) * amount * if tight { K_TIGHT_GAIN } else { K_SOFT_GAIN };
        let fl = filter_netlist(f64::from(tune_hz(tune)));
        let mh = node(fl, N_HP, f) * -K_SHELF + node(fl, N_MID, f) * K_MID_CUT;
        let l = ac(&lc_netlist(f64::from(lc_hz(lc))), f) * K_LC_GAIN * intensity;
        20.0 * (Complex::new(1.0, 0.0) + (b + mh) * process + l).norm().log10()
    }

    #[test]
    fn the_controls_have_the_manuals_ranges() {
        assert_eq!((drive_db(0.0), drive_db(0.5), drive_db(1.0)), (-20.0, 0.0, 6.0));
        assert!((tune_hz(0.0) - 22_000.0).abs() < 1.0 && (tune_hz(1.0) - 1_100.0).abs() < 1.0);
        assert!((tune_hz(0.5) - 2_800.0).abs() < 1.0 && (tune_hz(0.2) - 8_500.0).abs() < 1.0);
        assert!((drive_db(0.1) - -13.0).abs() < 1e-4 && (drive_db(0.8) - 4.0).abs() < 1e-4 && (drive_db(0.55) - 0.75).abs() < 1e-4);
        assert!((lc_hz(0.0) - 2_000.0).abs() < 1.0 && (lc_hz(1.0) - 20_000.0).abs() < 1.0);
        assert_eq!(bass_sound(0.5), (0.0, false));
        assert_eq!(bass_sound(0.0), (1.0, false));
        assert_eq!(bass_sound(1.0), (1.0, true));
    }

    #[test]
    fn process_at_minimum_is_the_original() {
        for f in [30.0, 300.0, 3000.0, 15000.0] {
            assert!(response(f, 0.0, 0.5, 0.0, 0.5, 0.0).abs() < 1e-9);
        }
    }

    #[test]
    fn process_lowers_the_mids_and_raises_the_top() {
        // Mid-Hi Tune at its centre: 2.8 kHz. No bass, no LC.
        let at = |f: f64| response(f, 0.5, 0.5, 1.0, 0.5, 0.0);
        assert!(at(30.0).abs() < 0.5, "under the lowered range: {}", at(30.0));
        assert!(at(800.0) < -3.0 && at(800.0) > -5.0, "the lowered mids: {}", at(800.0));
        assert!(at(20_000.0) > 8.0 && at(20_000.0) < 9.5, "the shelf: {}", at(20_000.0));
        // Mid-Hi Tune moves where the top starts: hard right (1.1 kHz) it is up by 3 kHz already
        let low_tuned = response(3000.0, 0.5, 1.0, 1.0, 0.5, 0.0);
        let high_tuned = response(3000.0, 0.5, 0.0, 1.0, 0.5, 0.0);
        assert!(low_tuned > 4.0 && high_tuned < -2.0, "{low_tuned} {high_tuned}");
        // and Process is how much of it there is
        assert!((response(800.0, 0.5, 0.5, 0.5, 0.5, 0.0) - at(800.0) / 2.0).abs() < 0.6);
    }

    #[test]
    fn bass_sound_is_soft_one_way_and_tight_the_other() {
        let soft = |f: f64| response(f, 0.0, 0.0, 1.0, 0.5, 0.0);
        let tight = |f: f64| response(f, 1.0, 0.0, 1.0, 0.5, 0.0);
        assert!(soft(45.0) > 8.5 && soft(45.0) < 10.0, "{}", soft(45.0));
        assert!(tight(70.0) > 7.0 && tight(70.0) < 8.5, "{}", tight(70.0));
        // Soft is the broader: an octave and a half up it still adds more than Tight does
        assert!(soft(140.0) > tight(200.0), "{} {}", soft(140.0), tight(200.0));
        // the netlists are band-passes of gain 1 at their frequencies
        assert!((ac(&bass_netlist(false), K_SOFT_HZ).norm() - 1.0).abs() < 1e-9);
        assert!((ac(&bass_netlist(true), K_TIGHT_HZ).norm() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn the_lc_stage_peaks_where_it_is_set() {
        for v in [0.0f32, 0.5, 1.0] {
            let f0 = f64::from(lc_hz(v));
            let n = lc_netlist(f0);
            assert!((ac(&n, f0).norm() - 1.0).abs() < 1e-9);
            assert!((ac(&n, f0 * 2.0).norm() - ac(&n, f0 / 2.0).norm()).abs() < 1e-9, "a bell on a log axis");
            assert!(ac(&n, f0 * 4.0).norm() < 0.3);
        }
        // Intensity alone: +10 dB at the frequency, Process at minimum
        let up = response(f64::from(lc_hz(0.5)), 0.5, 0.5, 0.0, 0.5, 1.0);
        assert!((up - 10.1).abs() < 0.2, "{up}");
        assert!(coil(0.01) / 0.01 > 0.999 && coil(1.0) < 0.62);
    }

    #[test]
    fn the_compressor_is_out_at_minimum_and_soft_kneed() {
        assert_eq!(comp_gain(1.0, 0.0), 1.0);
        assert_eq!(comp_gain(0.0, 1.0), 1.0);
        // full: threshold -36 dBFS, 6:1. 24 dB over comes out 4 dB over.
        let g = 20.0 * comp_gain(10.0f64.powf(-12.0 / 20.0), 1.0).log10();
        assert!((g - -20.0).abs() < 0.01, "{g}");
        // under the knee nothing, and the knee is continuous
        assert_eq!(comp_gain(10.0f64.powf(-40.0 / 20.0), 1.0), 1.0);
        let (a, b) = (comp_gain(10.0f64.powf(-33.001 / 20.0), 1.0), comp_gain(10.0f64.powf(-32.999 / 20.0), 1.0));
        assert!((a - b).abs() < 1e-3);
        let (att, rel) = comp_steps(48_000.0);
        assert!(att > rel && rel > 0.0);
    }

    #[test]
    fn the_expander_widens_and_leaves_mono_alone() {
        assert_eq!(expand(0.3, 0.3, 1.0), (0.3, 0.3));
        assert_eq!(expand(0.3, -0.2, 0.0), (0.3, -0.2));
        let (l, r) = expand(0.01, -0.01, 1.0);
        assert!(l > 0.0195 && l < 0.0205 && (l + r).abs() < 1e-6, "{l} {r}");
    }
}
