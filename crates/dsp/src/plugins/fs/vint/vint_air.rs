//! VintAir -- the Dolby A-type module (Cat. No. 22), as far as the Air trick uses it.
//!
//! Not a translation: the C++ VintEngine's Air is four filter bands and a lift that is linear in
//! dB. This one is FreeDAWm's own, from Dolby's drawing of the Cat. No. 22 Noise Reduction Module
//! (AOC 441/3, issue 6) and the Model 361's manual (its section 10, the module's circuit
//! description, and section 12, its test figures). The drawing's designators are kept.
//!
//! The trick: the module in its record mode adds a noise reduction signal to the main signal,
//! the sum of four band compressors that pass quiet signals 18.6 dB up and hold loud ones back.
//! Used alone, with no decoder after it, that is "air": quiet detail raised, loud signal left.
//!
//! ```text
//!   in (the filter driver) -+-> band 1: 80 Hz low-pass ----------------> compressor 1 -R109-+
//!                           +-> band 2: wideband - (band 1 + band 3) --> compressor 2 -R209-+-> NR
//!                           +-> band 3: 3 kHz high-pass ---------------> compressor 3 -R309-+  signal
//!                           +-> band 4: 9 kHz high-pass ---------------> compressor 4 -R409-+
//! ```
//!
//! As circuits (the `vint_mna` solver; each filter's transistors are followers, taken as ideal):
//! the filters of bands 1, 3 and 4 (two capacitors and two resistors round a follower each:
//! 80.8 Hz, 2.98 kHz, 9.58 kHz). Band 2 is the drawing's subtraction (R280 over R282, of R287 and
//! R288's mean). The four mixing resistors weigh the bands as the passive mixer they are.
//!
//! Left out: the input amplifier's 34 kHz low-pass (Q502, Q503). It is ahead of the main signal
//! and the side chain alike, so it changes nothing of what the module adds, and it is there for
//! tape bias, which a digital input does not carry.
//!
//! The compressors are NOT solved as circuits (ten transistors each, two FETs as variable
//! resistors, diode rectifiers). They are modelled on what the drawing and the manual give:
//! - the gain, 8.5: the factory's GAIN test, 2 mV in, 16 to 18 mV out;
//! - the attenuator as drawn: R106 into Q101 through R107, then R108 into Q102, each FET a
//!   conductance that rises with the control voltage once past its threshold;
//! - the control: the compressor's output and, through R126 against R125, some of its input
//!   ("the downturning curve"), rectified, through the pre-integrator (R136, R137, C110, let go
//!   by R135) and the final integrator (R140, C106), both passing large changes fast through
//!   their diodes; bands 3 and 4 have half the time constants of bands 1 and 2;
//! - the overshoot limiter D100 / D101.
//!
//! FITTED, not on the drawing: the FETs' conductance slope and two thresholds, chosen so the
//! compressor meets the factory's LAW tests (10 mV in, 55 to 65 mV out; 50 mV in, 67 to 77 mV out)
//! with the control voltages the manual's table gives for them (3.1, 3.5, 3.7 V). ASSUMED: the
//! diodes' conduction as fixed drops, the fast paths' time constants, and the limiter's level.
//! The resistors marked AOT on the drawing are trims set on test: the fit stands in for them.
//!
//! Levels: Dolby Level is 300 mV at the filter driver, and a sine at the Dolby Level knob's dBFS
//! is that.

use super::vint_mna::{Kind, Netlist};

// ---------------------------------------------------------------- the filters, as drawn
/// Band 1.
pub const R180: f64 = 301.0e3;
pub const R181: f64 = 274.0e3;
pub const C180: f64 = 0.01e-6;
pub const C181: f64 = 4700.0e-12;
/// Band 3.
pub const C380: f64 = 6800.0e-12;
pub const R380: f64 = 11.0e3;
pub const R382: f64 = 5.62e3;
/// Band 4.
pub const C480: f64 = 2200.0e-12;
pub const R480: f64 = 12.0e3;
pub const R482: f64 = 4.75e3;
/// Band 2's subtractor: bands 1 and 3 meet in R287 and R288 (equal: their mean), are inverted
/// with a gain of R280 / R282 and added to the wideband signal.
pub const R280: f64 = 2.21e3;
pub const R282: f64 = 1.07e3;
/// The mixing resistors into the addition stage, bands 1 to 4.
pub const MIX_R: [f64; 4] = [11.0e3, 11.3e3, 6.87e3, 9.53e3];

// ---------------------------------------------------------------- the compressor, as drawn
/// The attenuator: R106, Q101 through R107; R108, Q102.
pub const R106: f64 = 10.0e3;
pub const R107: f64 = 3.3e3;
pub const R108: f64 = 47.0e3;
/// Into the control amplifier: the compressor's output through R125, its input through R126.
pub const R125: f64 = 4.3e3;
pub const R126: f64 = 10.0e3;
/// The integrators, bands 1 and 2 / bands 3 and 4: the pre-integrator's two resistors (each),
/// the resistor that lets it go, its capacitor; the final integrator's resistor, its capacitor,
/// and the resistor its follower pulls down with.
pub const R136: [f64; 2] = [20.0e3, 10.0e3];
pub const R135: [f64; 2] = [560.0e3, 270.0e3];
pub const C110: f64 = 0.1e-6;
pub const R140: f64 = 47.0e3;
pub const C106: [f64; 2] = [2.2e-6, 1.0e-6];
pub const R138: f64 = 47.0e3;

// ---------------------------------------------------------------- the factory's figures (section 12)
/// Dolby Level at the filter driver, mV.
pub const K_DOLBY_LEVEL_MV: f64 = 300.0;
/// The GAIN test: 2 mV in, 16 to 18 mV out.
pub const K_GAIN: f64 = 8.5;
/// The module in record against the module with its noise reduction off, 3 mV in at the tone's
/// 850 Hz: 9.5 to 10.5 dB.
pub const K_LOW_LEVEL_BOOST_DB: f64 = 10.0;
/// The control voltage (the final integrator's, test points 75, 48, 29, 11) in the GAIN, LAW 1 and
/// LAW 2 tests, against the level its rectifier sees there (mV RMS): 17 mV out and 2 mV in; 60 and
/// 10; 72 and 50.
const K_CONTROL: [(f64, f64); 3] = [(17.0 + 2.0 * R125 / R126, 3.1), (60.0 + 10.0 * R125 / R126, 3.5), (72.0 + 50.0 * R125 / R126, 3.7)];

// ---------------------------------------------------------------- fitted, and assumed
/// FITTED to the LAW tests: the FETs' conductance per volt of control past their thresholds
/// (siemens), Q101's threshold and Q102's ("first Q101 and then Q102 conducts"), volts.
pub const K_FIT_FET_S_PER_V: f64 = 230.0e-6;
pub const K_FIT_Q101_V: f64 = 3.29;
pub const K_FIT_Q102_V: f64 = 3.50;
/// ASSUMED: a FET's least resistance.
const K_FET_ON_OHMS: f64 = 150.0;
/// ASSUMED: the diodes as fixed drops, in volts of control: D104 + D105 round the pre-integrator's
/// R136, D106 + D107 round the final integrator's R140 (rises), D108 (falls).
const K_PRE_FAST_V: f64 = 1.2;
const K_FINAL_RISE_FAST_V: f64 = 0.9;
const K_FINAL_FALL_FAST_V: f64 = 0.6;
/// ASSUMED: the final integrator's rise through its diodes (the follower's own resistance).
const K_FINAL_FAST_S: f64 = 0.2e-3;
/// ASSUMED: where the overshoot limiter D100 / D101 starts and where it stops the compressor's
/// output, mV.
const K_LIMIT_KNEE_MV: f64 = 400.0;
const K_LIMIT_MV: f64 = 600.0;
/// Where the pre-integrator settles on a steady sine, as a fraction of its peak: it charges
/// through 2 R136 while the rectified signal is over it and lets go through 2 R136 + R135, a
/// fifteenth as fast, while it is under (see the tests).
pub const K_DETECTOR_SETTLES: f64 = 0.90;

// ---------------------------------------------------------------- the filters
/// Band 1: 80 Hz low-pass (R180, R181, C180, C181 round Q180 .. Q182).
pub fn band1_netlist() -> Netlist {
    let mut n = Netlist::default();
    n.add(Kind::R, 1, 2, R180);
    n.add(Kind::R, 2, 3, R181);
    n.add(Kind::C, 3, 0, C181);
    n.add(Kind::C, 2, 4, C180);
    n.add_op_amp(3, 4, 4);
    n.out = 4;
    n
}

/// Bands 3 and 4: a high-pass, two equal capacitors, `to_bias` from the follower's input to the
/// 10 V line and `feedback` from its output back between the capacitors.
fn high_pass_netlist(c: f64, to_bias: f64, feedback: f64) -> Netlist {
    let mut n = Netlist::default();
    n.add(Kind::C, 1, 2, c);
    n.add(Kind::C, 2, 3, c);
    n.add(Kind::R, 3, 0, to_bias);
    n.add(Kind::R, 2, 4, feedback);
    n.add_op_amp(3, 4, 4);
    n.out = 4;
    n
}
/// Band 3: 3 kHz high-pass (C380, C381, R380, R382 round Q380, Q381).
pub fn band3_netlist() -> Netlist {
    high_pass_netlist(C380, R380, R382)
}
/// Band 4: 9 kHz high-pass (C480, C481, R480, R482 round Q480, Q481).
pub fn band4_netlist() -> Netlist {
    high_pass_netlist(C480, R480, R482)
}

/// Band 2 from the wideband signal and bands 1 and 3.
#[inline]
pub fn band2(wide: f64, band1: f64, band3: f64) -> f64 {
    wide - (R280 / R282) * 0.5 * (band1 + band3)
}

/// How much of each compressor's output is in the noise reduction signal as it joins the main
/// signal: the mixing resistors' shares, scaled so that band 2's compressor, at its low-level
/// gain, makes the factory's 10 dB.
pub fn mix_weights() -> [f64; 4] {
    let added = 10.0f64.powf(K_LOW_LEVEL_BOOST_DB / 20.0) - 1.0;
    MIX_R.map(|r| added / K_GAIN * MIX_R[1] / r)
}

/// A sample's millivolts per unit of full scale, with Dolby Level a sine at `dolby_level_dbfs`.
pub fn mv_per_unit(dolby_level_dbfs: f64) -> f64 {
    K_DOLBY_LEVEL_MV * std::f64::consts::SQRT_2 / 10.0f64.powf(dolby_level_dbfs / 20.0)
}

// ---------------------------------------------------------------- the compressor
/// The control voltage for the level the rectifier sees (mV RMS of a sine): the manual's three
/// figures, straight between them and on past the last.
pub fn control_volts(level_mv: f64) -> f64 {
    let [a, b, c] = K_CONTROL;
    let (p, q) = if level_mv <= b.0 { (a, b) } else { (b, c) };
    p.1 + (q.1 - p.1) * (level_mv - p.0) / (q.0 - p.0)
}

/// The attenuator's gain at a control voltage: the drawn ladder, the FETs at their fitted
/// conductances.
pub fn attenuation(control: f64) -> f64 {
    let fet = |threshold: f64| (K_FIT_FET_S_PER_V * (control - threshold)).clamp(0.0, 1.0 / K_FET_ON_OHMS);
    let (g1, g2) = (fet(K_FIT_Q101_V), fet(K_FIT_Q102_V));
    if g1 <= 0.0 {
        return 1.0;
    }
    // R106, then to ground R107 + Q101 beside R108 + Q102; the output across Q102
    let first = 1.0 / (R107 + 1.0 / g1);
    if g2 <= 0.0 {
        return 1.0 / (1.0 + R106 * first);
    }
    let q102 = 1.0 / g2;
    let second = 1.0 / (R108 + q102);
    (1.0 / (1.0 + R106 * (first + second))) * q102 * second
}

/// The integrators' steps at a sample rate, for bands 1 and 2 (`[0]`) and bands 3 and 4 (`[1]`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Steps {
    /// the pre-integrator: charging through both its resistors, through one (its diodes
    /// conducting), and let go through R135
    pre_rise: [f64; 2],
    pre_fast: [f64; 2],
    pre_fall: [f64; 2],
    /// the final integrator: through R140, through its diodes; falling through R140 and R138, and
    /// through D108 and R138
    final_rise: [f64; 2],
    final_fast: f64,
    final_fall: [f64; 2],
    final_fall_fast: [f64; 2],
    /// each band's share of the noise reduction signal ([`mix_weights`])
    pub weights: [f64; 4],
}

impl Steps {
    pub fn for_rate(fs: f64) -> Steps {
        let step = |tau: f64| 1.0 - (-1.0 / (tau * fs)).exp();
        let each = |f: &dyn Fn(usize) -> f64| [step(f(0)), step(f(1))];
        Steps {
            pre_rise: each(&|i| 2.0 * R136[i] * C110),
            pre_fast: each(&|i| R136[i] * C110),
            pre_fall: each(&|i| (R135[i] + 2.0 * R136[i]) * C110),
            final_rise: each(&|i| R140 * C106[i]),
            final_fast: step(K_FINAL_FAST_S),
            final_fall: each(&|i| (R140 + R138) * C106[i]),
            final_fall_fast: each(&|i| R138 * C106[i]),
            weights: mix_weights(),
        }
    }
}

/// One band's compressor: its two integrators' levels, as the rectified signal's (mV, peak).
#[derive(Debug, Clone, Copy, Default)]
pub struct Compressor {
    pre: f64,
    held: f64,
}

/// Volts of control per mV (peak) at the integrators, for the diodes' drops there.
fn mv_per_control_volt() -> f64 {
    let [a, b, _] = K_CONTROL;
    (b.0 - a.0) / (b.1 - a.1) * std::f64::consts::SQRT_2 * K_DETECTOR_SETTLES
}

impl Compressor {
    /// One sample: `input` is the band's signal (mV), `group` 0 for bands 1 and 2, 1 for bands 3
    /// and 4. Returns the compressor's output (mV).
    #[inline]
    pub fn run(&mut self, input: f64, group: usize, steps: &Steps) -> f64 {
        let level = self.held / (std::f64::consts::SQRT_2 * K_DETECTOR_SETTLES);
        let mut out = K_GAIN * attenuation(control_volts(level)) * input;
        // D100 / D101: transient overshoots held
        let size = out.abs();
        if size > K_LIMIT_KNEE_MV {
            let span = K_LIMIT_MV - K_LIMIT_KNEE_MV;
            out = out.signum() * (K_LIMIT_KNEE_MV + span * ((size - K_LIMIT_KNEE_MV) / span).tanh());
        }
        // the control: the output, and some of the input round the attenuator, rectified
        let seen = (out + R125 / R126 * input).abs();
        let per_volt = mv_per_control_volt();
        let d = seen - self.pre;
        let step = if d <= 0.0 {
            steps.pre_fall[group]
        } else if d > K_PRE_FAST_V * per_volt {
            steps.pre_fast[group]
        } else {
            steps.pre_rise[group]
        };
        self.pre += step * d;
        let d = self.pre - self.held;
        let step = if d >= 0.0 {
            if d > K_FINAL_RISE_FAST_V * per_volt { steps.final_fast } else { steps.final_rise[group] }
        } else if -d > K_FINAL_FALL_FAST_V * per_volt {
            steps.final_fall_fast[group]
        } else {
            steps.final_fall[group]
        };
        self.held += step * d;
        out
    }

    /// The compressor settled on a steady sine of `input_mv` RMS: its output, mV RMS (for the
    /// factory's static tests; the running compressor gets there by itself).
    pub fn settled(input_mv: f64) -> f64 {
        let (mut lo, mut hi) = (0.0, K_GAIN * input_mv);
        for _ in 0..80 {
            let out = 0.5 * (lo + hi);
            if K_GAIN * attenuation(control_volts(out + R125 / R126 * input_mv)) * input_mv > out {
                lo = out;
            } else {
                hi = out;
            }
        }
        0.5 * (lo + hi)
    }
}

#[cfg(test)]
mod tests {
    use super::super::vint_mna::{Scratch, Solver, ac};
    use super::*;
    use rustfft::num_complex::Complex;

    fn db(v: f64) -> f64 {
        20.0 * v.log10()
    }

    /// The four bands at f, as the filters and the subtractor make them from the wideband signal.
    fn bands(f: f64) -> [Complex<f64>; 4] {
        let (b1, b3, b4) = (ac(&band1_netlist(), f), ac(&band3_netlist(), f), ac(&band4_netlist(), f));
        [b1, Complex::new(1.0, 0.0) - (b1 + b3) * (R280 / R282 * 0.5), b3, b4]
    }

    /// The record characteristic at low level: the main signal plus the bands from `first` up.
    fn low_level_boost_db(f: f64, first: usize) -> f64 {
        let w = mix_weights();
        let nr: Complex<f64> = bands(f).iter().zip(w).skip(first).map(|(b, w)| b * (w * K_GAIN)).sum();
        db((Complex::new(1.0, 0.0) + nr).norm())
    }

    #[test]
    fn the_filters_are_dolbys_bands() {
        // 80 Hz, 3 kHz and 9 kHz: each filter 3 dB down within a few percent of there
        let corner = |n: &Netlist, lo: f64, hi: f64| {
            let (mut a, mut b) = (lo, hi);
            let rising = ac(n, hi).norm() > ac(n, lo).norm();
            for _ in 0..60 {
                let m = (a * b).sqrt();
                if (ac(n, m).norm() > std::f64::consts::FRAC_1_SQRT_2) == rising { b = m } else { a = m }
            }
            a
        };
        let (f1, f3, f4) =
            (corner(&band1_netlist(), 20.0, 400.0), corner(&band3_netlist(), 500.0, 9000.0), corner(&band4_netlist(), 2000.0, 30000.0));
        assert!((f1 / 80.0 - 1.0).abs() < 0.06 && (f3 / 3000.0 - 1.0).abs() < 0.06 && (f4 / 9000.0 - 1.0).abs() < 0.09, "{f1} {f3} {f4}");
        // band 2 is what bands 1 and 3 leave ("complementary to that of bands 1 and 3"): it is the
        // middle, and the three together are the wideband signal again, within R280 / R282's 3%
        let b = bands(850.0);
        assert!((b[1].norm() - 1.0).abs() < 0.1 && b[0].norm() < 0.02 && b[2].norm() < 0.1);
        for f in [20.0, 80.0, 500.0, 3000.0, 20_000.0] {
            let b = bands(f);
            assert!((b[0] + b[1] + b[2] - Complex::new(1.0, 0.0)).norm() < 0.05, "{f} Hz");
        }
    }

    #[test]
    fn the_factorys_compressor_tests() {
        // Section 12.4, test 9: GAIN 2 mV, LAW 1 10 mV, LAW 2 50 mV into the compressor.
        let (gain, law1, law2) = (Compressor::settled(2.0), Compressor::settled(10.0), Compressor::settled(50.0));
        assert!((16.0..=18.0).contains(&gain), "GAIN: {gain}");
        assert!((55.0..=65.0).contains(&law1), "LAW 1: {law1}");
        assert!((67.0..=77.0).contains(&law2), "LAW 2: {law2}");
        assert!((law1 - 60.0).abs() < 1.0 && (law2 - 72.0).abs() < 1.0, "the fit is to the middle: {law1} {law2}");
        // and the control voltages the table gives for them
        assert!((control_volts(17.0 + 0.86) - 3.1).abs() < 1e-9 && (control_volts(72.0 + 21.5) - 3.7).abs() < 1e-9);
        // "first Q101 and then Q102 conducts"; nothing attenuated in the GAIN test
        assert!(K_FIT_Q101_V < K_FIT_Q102_V && attenuation(3.1) == 1.0);
        // the least Q101 alone can leave: R107 against R106
        let q101_alone = attenuation(K_FIT_Q102_V);
        assert!(q101_alone < 1.0 && q101_alone > R107 / (R106 + R107));
    }

    #[test]
    fn the_output_turns_down_at_high_levels() {
        // the feed-forward through R126: past Dolby Level the compressor's output falls again
        let at = |mv: f64| Compressor::settled(mv);
        assert!(at(300.0) > at(50.0) && at(3000.0) < at(300.0), "{} {} {}", at(50.0), at(300.0), at(3000.0));
        // so the noise reduction signal is a negligible part of a loud main signal
        let added = |mv: f64| db(1.0 + mix_weights()[1] * at(mv) / mv);
        assert!((added(3.0) - 10.0).abs() < 0.05, "{}", added(3.0));
        assert!(added(30.0) > 3.0 && added(30.0) < 5.0, "{}", added(30.0));
        assert!(added(300.0) < 0.8 && added(3000.0) < 0.1, "{} {}", added(300.0), added(3000.0));
    }

    #[test]
    fn the_record_characteristic_at_low_level() {
        // Section 12.4, tests 10 and 11: 3 mV at the tone's 850 Hz comes out 9.5 to 10.5 dB up.
        let at_tone = low_level_boost_db(850.0, 0);
        assert!((9.5..=10.5).contains(&at_tone), "{at_tone}");
        // Dolby's A-type characteristic: 10 dB up to about 5 kHz, rising to 15 dB at 15 kHz.
        for f in [20.0, 80.0, 400.0, 2000.0, 3000.0] {
            let b = low_level_boost_db(f, 0);
            assert!((b - 10.0).abs() < 1.1, "{f} Hz: {b}");
        }
        let (at_5k, at_15k) = (low_level_boost_db(5000.0, 0), low_level_boost_db(15_000.0, 0));
        assert!(at_5k > 10.0 && at_5k < 12.0 && at_15k > 14.5 && at_15k < 16.6, "{at_5k} {at_15k}");
        // the trick's other settings: bands 3 and 4 alone, band 4 alone
        assert!(low_level_boost_db(100.0, 2).abs() < 0.1 && low_level_boost_db(15_000.0, 2) > 15.0);
        assert!(low_level_boost_db(1000.0, 3).abs() < 0.5 && low_level_boost_db(15_000.0, 3) > 9.0);
    }

    #[test]
    fn the_running_compressor_meets_the_static_tests() {
        // A 1 kHz sine at each test level through the compressor as it runs, at 96 kHz.
        let fs = 96_000.0;
        let steps = Steps::for_rate(fs);
        for (mv, group) in [(2.0, 0), (10.0, 0), (50.0, 0), (10.0, 1), (50.0, 1)] {
            let mut c = Compressor::default();
            let (mut sum, mut n) = (0.0, 0);
            for i in 0..(fs as usize * 2) {
                let x = mv * std::f64::consts::SQRT_2 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / fs).sin();
                let y = c.run(x, group, &steps);
                assert!(y.is_finite());
                if i >= fs as usize {
                    sum += y * y;
                    n += 1;
                }
            }
            let rms = (sum / n as f64).sqrt();
            let want = Compressor::settled(mv);
            assert!((rms / want - 1.0).abs() < 0.08, "{mv} mV, group {group}: {rms} against {want}");
        }
    }

    #[test]
    fn large_changes_pass_fast_and_small_ones_slowly() {
        let fs = 96_000.0;
        let steps = Steps::for_rate(fs);
        // bands 3 and 4 have half the time constants of bands 1 and 2
        assert!(steps.pre_rise[1] > 1.9 * steps.pre_rise[0] && steps.final_rise[1] > 2.0 * steps.final_rise[0]);
        assert!(steps.pre_fast[0] > steps.pre_rise[0] && steps.pre_rise[0] > 10.0 * steps.pre_fall[0]);
        assert!(steps.final_fast > 100.0 * steps.final_rise[0] && steps.final_fall_fast[0] > steps.final_fall[0]);
        // a tone that jumps from nothing to Dolby Level: the overshoot is held by the limiter and
        // over within a few milliseconds
        let mut c = Compressor::default();
        let tone = |i: usize| 300.0 * std::f64::consts::SQRT_2 * (2.0 * std::f64::consts::PI * 2000.0 * i as f64 / fs).sin();
        let out: Vec<f64> = (0..(fs as usize / 2)).map(|i| c.run(tone(i), 1, &steps)).collect();
        let peak = |from_ms: f64, to_ms: f64| out[(from_ms * 96.0) as usize..(to_ms * 96.0) as usize].iter().fold(0.0f64, |m, v| m.max(v.abs()));
        assert!(peak(0.0, 2.0) <= K_LIMIT_MV && peak(0.0, 2.0) > 300.0, "{}", peak(0.0, 2.0));
        assert!(peak(10.0, 20.0) < 0.6 * peak(0.0, 2.0), "{} {}", peak(10.0, 20.0), peak(0.0, 2.0));
        let settled = Compressor::settled(300.0) * std::f64::consts::SQRT_2;
        assert!((peak(400.0, 499.0) / settled - 1.0).abs() < 0.1, "{} against {settled}", peak(400.0, 499.0));
    }

    #[test]
    fn the_detector_settles_under_the_peak() {
        // charging while the rectified sine is over it, a fifteenth as fast letting go while under
        let ratio = 2.0 * R136[0] / (R135[0] + 2.0 * R136[0]);
        let n = 20_000;
        let rectified = |i: usize| (std::f64::consts::PI * (i as f64 + 0.5) / n as f64).sin();
        let charge = |r: f64| (0..n).map(|i| (rectified(i) - r).max(0.0)).sum::<f64>();
        let let_go = |r: f64| (0..n).map(|i| (r - rectified(i)).max(0.0)).sum::<f64>();
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..50 {
            let r = 0.5 * (lo + hi);
            if charge(r) > ratio * let_go(r) { lo = r } else { hi = r }
        }
        assert!((lo - K_DETECTOR_SETTLES).abs() < 0.01, "{lo}");
    }

    #[test]
    fn the_filters_run_as_they_measure() {
        // band 3 at 3 kHz through the solver at 96 kHz, against its analog response
        let fs = 96_000.0;
        let mut sc = Scratch::new_boxed();
        let mut s = Solver::default();
        s.set_running(&band3_netlist(), fs, &mut sc);
        let mut peak = 0.0f64;
        for i in 0..(fs as usize) {
            let y = s.run((2.0 * std::f64::consts::PI * 3000.0 * i as f64 / fs).sin());
            if i > fs as usize / 2 {
                peak = peak.max(y.abs());
            }
        }
        let want = ac(&band3_netlist(), 3000.0).norm();
        assert!((peak / want - 1.0).abs() < 0.02, "{peak} against {want}");
        assert!((mv_per_unit(-16.0) * 10.0f64.powf(-16.0 / 20.0) / std::f64::consts::SQRT_2 - K_DOLBY_LEVEL_MV).abs() < 1e-9);
    }
}
