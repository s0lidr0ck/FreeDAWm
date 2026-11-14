//! VintMaximizer -- the BBE 882 Sonic Maximizer's process, as a circuit.
//!
//! Not a translation: the C++ VintEngine's Maximizer is three filter bands and two delay lines.
//! This one is FreeDAWm's own, from BBE Sound's drawing of the 882 (rev 1.1, 12-25-98, in its
//! user guide): one NJM2153 ("BBE-PRO") per channel and the discrete parts around it. The
//! drawing's channel A designators are kept. What is inside the 2153 (which op-amp is on which
//! pin, where the VCA and the detector sit) is from Tom Farrand's circuit analysis of the 482i
//! (Radio-Flier Electronics, 2004), whose parts around the IC are the 882's, value for value.
//!
//! ```text
//!   in -> OA1, x2 (R6 / R11) --+-> state-variable filter: OA2 sums, OA3 and OA4 integrate
//!                              |      HP (pin 1) -> VCA (pin 3) -> R1 + PROCESS (VR1) ----+
//!                              |      BP (pin 17) -> R10 ---------------------------------+-> OA5 -> out
//!                              |      LP (pin 15) -> R14 + LO CONTOUR (VR2) --------------+   (R13 || C8)
//!                              +-> peak detector (R3, C4, C2) -> the VCA's gain
//! ```
//!
//! The filter's two integrators are a decade apart (C6 3n3, C5 33n, both through 21k5), and
//! R7 / R8 feed back half the band-pass, so it is heavily damped (Q 0.21): its poles sit at
//! 157 Hz and 3.2 kHz, and those are the crossovers. OA5 adds the three bands back with the
//! band-pass in the opposite sense to what would rebuild the input, so with both pots at minimum
//! the unit is flat within half a dB but its phase turns through a full circle from the lows to
//! the highs: the lows leave about 2 ms after the highs. The pots then raise the lows and the
//! highs by up to 15.6 dB.
//!
//! As circuits (the `vint_mna` solver, op-amps ideal, as the engine's other circuits):
//! OA1 to OA4 with every resistor and capacitor between them, and OA5 with R13 and C8.
//! The factory's calibration figures (5 kHz and 500 Hz, in the user guide) are met within
//! 0.4 dB: see the tests.
//!
//! Left out: the 882's balanced input and output amplifiers (5532s: flat, and 1.2 dB of loss
//! between them from jack to jack) and the coupling capacitors (C14, C10 and the jacks': they
//! turn over below 7 Hz against the resistors on the drawing). At 50 Hz the factory's figures
//! are 1.1 to 1.4 dB under this model, at both ends of LO CONTOUR: something turns over near
//! 30 Hz that the drawing does not show (C14 against the 2153's input bias resistor would,
//! if that were about 2k5). It is not modelled, because nothing documents it.
//!
//! NOT a circuit: the VCA and the peak detector. They are inside the NJM2153 and no drawing
//! shows their parts, so they are modelled by what is documented:
//! - the detector listens to OA1's output, the whole signal, not the high band (Farrand);
//! - its attack is R3 C4 (0.73 ms); C2 holds the level and lets it go;
//! - NJR: "the internal VCA suppresses the boost gain in high band to reduce noise when faint
//!   signals are inputted": the VCA is wide open on programme and closes on faint signals;
//! - BBE's specification: "Sensitivity: -45 dBu for maximum process": where it is wide open;
//! - the calibration figures at 5 kHz, taken at -10 dBu, are those of a VCA at a gain of 1.
//!
//! Two numbers of that model are documented nowhere and are assumptions, marked below: the
//! resistance that discharges C2, and how far the VCA closes. Above -45 dBu at the 882's input
//! (-63 dBFS here) the model is the circuit as drawn.

use super::vint_mna::{Kind, Netlist};

// ---------------------------------------------------------------- the parts, as drawn (882, channel A)
/// Input buffer: OA1's feedback and its leg to ground.
pub const R6: f64 = 21.5e3;
pub const R11: f64 = 21.5e3;
/// OA2, the filter's summing amp: the input, its own feedback, the low-pass back.
pub const R2: f64 = 21.5e3;
pub const R4: f64 = 21.5e3;
pub const R96: f64 = 21.5e3;
/// The band-pass back to OA2's + input, halved.
pub const R7: f64 = 21.5e3;
pub const R8: f64 = 21.5e3;
/// OA3, the first integrator.
pub const R5: f64 = 21.5e3;
pub const C6: f64 = 3.3e-9;
/// OA4, the second integrator.
pub const R9: f64 = 21.5e3;
pub const C5: f64 = 33.0e-9;
/// Into OA5's virtual earth: the low-pass and LO CONTOUR, the band-pass, the VCA and PROCESS.
pub const R14: f64 = 10.0e3;
pub const VR2: f64 = 50.0e3;
pub const R10: f64 = 39.2e3;
pub const R1: f64 = 10.0e3;
pub const VR1: f64 = 50.0e3;
/// OA5's feedback.
pub const R13: f64 = 28.2e3;
pub const C8: f64 = 100.0e-12;
/// The peak detector's parts outside the IC.
pub const R3: f64 = 3.32e3;
pub const C4: f64 = 0.22e-6;
pub const C2: f64 = 2.2e-6;

// ---------------------------------------------------------------- the VCA: documented, and assumed
/// BBE's specification for the 882: "Sensitivity: -45 dBu for maximum process" (0 dBu: 0.775 V).
pub const K_SENSITIVITY_DBU: f64 = -45.0;
/// The 882's balanced input amplifier ahead of the 2153: 10k of feedback (R31) over 1k + 10k
/// (R26 + R27).
pub const K_INPUT_AMP_GAIN: f64 = 10.0 / 11.0;
/// ASSUMED: the resistance inside the 2153 that discharges C2. With 100k the VCA takes 220 ms
/// to let go.
pub const K_ASSUMED_RELEASE_R: f64 = 100.0e3;
/// ASSUMED: how far the VCA closes. R1 / (R1 + VR1) takes away exactly the boost PROCESS can
/// add ("suppresses the boost gain"), leaving the highs where the pot's minimum has them.
pub const K_ASSUMED_VCA_FLOOR: f64 = R1 / (R1 + VR1);

/// The peak level at OA1's output, in volts, at the specified sensitivity: under it the VCA
/// starts to close. 11 mV: -63 dBFS at the engine's 8.7 V full scale.
pub fn vca_knee_volts() -> f64 {
    0.775 * 10.0f64.powf(K_SENSITIVITY_DBU / 20.0) * std::f64::consts::SQRT_2 * K_INPUT_AMP_GAIN * (1.0 + R6 / R11)
}

// ---------------------------------------------------------------- the filter's nodes
pub const N_IN: i32 = 1;
/// OA1's output (pin 11): the filter's input, and what the detector listens to.
pub const N_BUFFER: i32 = 3;
/// OA2's output (pin 1).
pub const N_HP: i32 = 6;
/// OA3's output (pin 17).
pub const N_BP: i32 = 8;
/// OA4's output (pin 15).
pub const N_LP: i32 = 10;

/// OA1 to OA4. `cap_scale` multiplies both integrator capacitors (1: the 882's parts).
/// The netlist's output is the low-pass; the other two bands are read at their nodes.
pub fn filter_netlist(cap_scale: f64) -> Netlist {
    const N_OA1_MINUS: i32 = 2;
    const N_OA2_MINUS: i32 = 4;
    const N_OA2_PLUS: i32 = 5;
    const N_OA3_MINUS: i32 = 7;
    const N_OA4_MINUS: i32 = 9;
    let mut n = Netlist::default();
    // input buffer
    n.add(Kind::R, N_OA1_MINUS, 0, R11);
    n.add(Kind::R, N_OA1_MINUS, N_BUFFER, R6);
    n.add_op_amp(N_IN, N_OA1_MINUS, N_BUFFER);
    // OA2
    n.add(Kind::R, N_BUFFER, N_OA2_MINUS, R2);
    n.add(Kind::R, N_OA2_MINUS, N_HP, R4);
    n.add(Kind::R, N_OA2_MINUS, N_LP, R96);
    n.add(Kind::R, N_OA2_PLUS, N_BP, R7);
    n.add(Kind::R, N_OA2_PLUS, 0, R8);
    n.add_op_amp(N_OA2_PLUS, N_OA2_MINUS, N_HP);
    // OA3
    n.add(Kind::R, N_HP, N_OA3_MINUS, R5);
    n.add(Kind::C, N_OA3_MINUS, N_BP, C6 * cap_scale);
    n.add_op_amp(0, N_OA3_MINUS, N_BP);
    // OA4
    n.add(Kind::R, N_BP, N_OA4_MINUS, R9);
    n.add(Kind::C, N_OA4_MINUS, N_LP, C5 * cap_scale);
    n.add_op_amp(0, N_OA4_MINUS, N_LP);
    n.out = N_LP;
    n
}

/// OA5 with its feedback, driven through one R13: an inverting amp of gain 1 that rolls off as
/// R13 || C8 does (56 kHz). The three bands' currents into its virtual earth add, so driving
/// it with `R13 * (their sum)` is the summing amp exactly.
pub fn summing_netlist() -> Netlist {
    const N_MINUS: i32 = 2;
    const N_OUT: i32 = 3;
    let mut n = Netlist::default();
    n.add(Kind::R, N_IN, N_MINUS, R13);
    n.add(Kind::R, N_MINUS, N_OUT, R13);
    n.add(Kind::C, N_MINUS, N_OUT, C8);
    n.add_op_amp(0, N_MINUS, N_OUT);
    n.out = N_OUT;
    n
}

/// A 50KB pot wired as the drawing has both: a linear track, the wiper tied to one end, all of
/// it in circuit at the knob's minimum and none at its maximum.
pub fn pot_ohms(track: f64, rotation: f64) -> f64 {
    track * (1.0 - rotation.clamp(0.0, 1.0))
}

/// What OA5 is driven with (see `summing_netlist`): the three bands, the VCA at `vca`.
#[inline]
pub fn summing_drive(lp: f64, bp: f64, hp: f64, vca: f64, lo_contour: f64, process: f64) -> f64 {
    R13 * (lp / (R14 + pot_ohms(VR2, lo_contour)) + bp / R10 + vca * hp / (R1 + pot_ohms(VR1, process)))
}

/// The Tune knob (0..1) as a scale on both integrator capacitors: 1 at the centre, the 882's
/// parts; the crossovers move a factor of 3.16 either way. The hardware has no such control.
pub fn cap_scale(tune: f32) -> f64 {
    10.0f64.powf(0.5 - f64::from(tune.clamp(0.0, 1.0)))
}

/// The detector's attack and release as one-pole steps at `fs`.
pub fn detector_steps(fs: f64) -> (f64, f64) {
    let step = |tau: f64| 1.0 - (-1.0 / (tau * fs)).exp();
    (step(R3 * C4), step(K_ASSUMED_RELEASE_R * C2))
}

/// The detector's held level (volts), one sample on: `buffer_volts` is OA1's output.
#[inline]
pub fn detect(env: f64, buffer_volts: f64, attack: f64, release: f64) -> f64 {
    let v = buffer_volts.abs();
    env + (if v > env { attack } else { release }) * (v - env)
}

/// The VCA's gain for the detector's level: 1 on programme, closing in proportion under the
/// specified sensitivity.
#[inline]
pub fn vca_gain(env_volts: f64) -> f64 {
    (env_volts / vca_knee_volts()).clamp(K_ASSUMED_VCA_FLOOR, 1.0)
}

#[cfg(test)]
mod tests {
    use super::super::vint_mna::{Scratch, Solver, ac};
    use super::*;
    use rustfft::num_complex::Complex;

    /// The whole unit's analog response with the VCA open: the three bands of the filter netlist,
    /// summed as OA5 sums them.
    fn response(f: f64, lo_contour: f64, process: f64) -> Complex<f64> {
        let band = |node: i32| {
            let mut n = filter_netlist(1.0);
            n.out = node;
            ac(&n, f)
        };
        let drive = (band(N_LP) / (R14 + pot_ohms(VR2, lo_contour)) + band(N_BP) / R10 + band(N_HP) / (R1 + pot_ohms(VR1, process))) * R13;
        drive * ac(&summing_netlist(), f)
    }

    fn db(v: Complex<f64>) -> f64 {
        20.0 * v.norm().log10()
    }

    #[test]
    fn the_filter_is_the_drawings() {
        // HP = -A s^2 t1 t2 / D, BP = A s t2 / D, LP = -A / D, D = s^2 t1 t2 + 1.5 s t2 + 1,
        // A = 2 in, t1 = R5 C6, t2 = R9 C5.
        let (t1, t2) = (R5 * C6, R9 * C5);
        for f in [20.0, 157.0, 709.0, 3208.0, 15000.0] {
            let s = Complex::new(0.0, 2.0 * std::f64::consts::PI * f);
            let d = s * s * t1 * t2 + s * 1.5 * t2 + 1.0;
            let want = [(N_LP, Complex::new(-2.0, 0.0) / d), (N_BP, s * 2.0 * t2 / d), (N_HP, -s * s * 2.0 * t1 * t2 / d)];
            for (node, w) in want {
                let mut n = filter_netlist(1.0);
                n.out = node;
                let got = ac(&n, f);
                assert!((got - w).norm() < 1e-9 * (1.0 + w.norm()), "node {node} at {f} Hz: {got} vs {w}");
            }
        }
        // the poles: 157 Hz and 3.2 kHz
        let disc = (2.25 / (t1 * t1) - 4.0 / (t1 * t2)).sqrt();
        let poles = [(1.5 / t1 - disc) / 2.0, (1.5 / t1 + disc) / 2.0].map(|w| w / (2.0 * std::f64::consts::PI));
        assert!((poles[0] - 156.9).abs() < 1.0 && (poles[1] - 3208.0).abs() < 5.0, "{poles:?}");
    }

    #[test]
    fn pots_at_minimum_are_flat_and_turn_the_phase() {
        for f in [20.0, 100.0, 700.0, 3000.0] {
            let g = db(response(f, 0.0, 0.0));
            assert!(g.abs() < 0.6, "{f} Hz: {g:.2} dB");
        }
        // R13 || C8 turns over at 56 kHz: another 0.3 dB down at 15 kHz
        let g = db(response(15000.0, 0.0, 0.0));
        assert!(g < -0.6 && g > -1.0, "15 kHz: {g:.2} dB");
        // in phase at the bottom, inverted at the filter's centre, back in phase at the top
        assert!(response(5.0, 0.0, 0.0).re > 0.9);
        assert!(response(709.4, 0.0, 0.0).re < -0.9);
        assert!(response(100_000.0, 0.0, 0.0).re > 0.0);
    }

    #[test]
    fn each_pot_has_fifteen_db() {
        let range = 20.0 * ((R14 + VR2) / R14).log10();
        assert!((range - 15.56).abs() < 0.01);
        let lows = db(response(10.0, 1.0, 0.0)) - db(response(10.0, 0.0, 0.0));
        let highs = db(response(18000.0, 0.0, 1.0)) - db(response(18000.0, 0.0, 0.0));
        assert!((lows - range).abs() < 0.2, "{lows}");
        assert!((highs - range).abs() < 0.6, "{highs}");
        // each pot leaves the other end alone
        assert!((db(response(18000.0, 1.0, 0.0)) - db(response(18000.0, 0.0, 0.0))).abs() < 0.3);
        assert!((db(response(10.0, 0.0, 1.0)) - db(response(10.0, 0.0, 0.0))).abs() < 0.1);
    }

    #[test]
    fn the_factorys_calibration_figures() {
        // "Calibration Procedure for the BBE 882": -10 dBu into the 1/4" jack, a voltmeter on the
        // 1/4" output, each reading +/- 1 dBu. Jack to jack there are also the input amplifier
        // (10/11) and one leg of the output amplifier (R17, R99, R98, R102: 0.9565).
        let io = 20.0 * (K_INPUT_AMP_GAIN * (1.0 + 39.2 / 46.4) * 42.2 / (39.2 + 42.2)).log10();
        let reads = |f: f64, lo: f64, pr: f64| -10.0 + io + db(response(f, lo, pr));
        // BBE PROCESS TEST, 5 kHz (LO CONTOUR at minimum): -12 dBu, then +2 dBu.
        assert!((reads(5000.0, 0.0, 0.0) - -12.0).abs() < 0.4, "{}", reads(5000.0, 0.0, 0.0));
        assert!((reads(5000.0, 0.0, 1.0) - 2.0).abs() < 0.4, "{}", reads(5000.0, 0.0, 1.0));
        // BYPASS TEST, 500 Hz, the pots where the tests before it left them (both at maximum): -9 dBu.
        assert!((reads(500.0, 1.0, 1.0) - -9.0).abs() < 0.4, "{}", reads(500.0, 1.0, 1.0));
        // LO CONTOUR TEST, 50 Hz (PROCESS left at maximum): -13 dBu, then +2 dBu. The model is
        // 1.1 and 1.4 dB over: see the module notes.
        assert!((reads(50.0, 0.0, 1.0) - -13.0 - 1.1).abs() < 0.2, "{}", reads(50.0, 0.0, 1.0));
        assert!((reads(50.0, 1.0, 1.0) - 2.0 - 1.4).abs() < 0.2, "{}", reads(50.0, 1.0, 1.0));
    }

    #[test]
    fn the_running_circuit_follows_the_analog_one() {
        // A 1 kHz sine through the solvers at 192 kHz, against the analog response.
        let fs = 192_000.0;
        let mut sc = Scratch::new_boxed();
        let (mut filter, mut sum) = (Solver::default(), Solver::default());
        filter.set(&filter_netlist(1.0), fs, &mut sc);
        sum.set_running(&summing_netlist(), fs, &mut sc);
        let (lo, pr) = (0.6, 0.8);
        let (mut peak, f) = (0.0f64, 1000.0);
        for i in 0..(fs as usize) {
            let x = (2.0 * std::f64::consts::PI * f * i as f64 / fs).sin();
            let lp = filter.open_circuit(x);
            let (bp, hp) = (filter.node_open(N_BP), filter.node_open(N_HP));
            filter.commit(0.0);
            let y = sum.run(summing_drive(lp, bp, hp, 1.0, lo, pr));
            assert!(y.is_finite());
            if i > fs as usize / 2 {
                peak = peak.max(y.abs());
            }
        }
        let want = response(f, lo, pr).norm();
        assert!((peak / want - 1.0).abs() < 0.01, "{peak} vs {want}");
    }

    #[test]
    fn the_vca_is_open_on_programme_and_closes_on_faint_signals() {
        assert_eq!(vca_gain(1.0), 1.0);
        assert!((vca_knee_volts() - 0.0112).abs() < 1e-4, "{}", vca_knee_volts());
        assert_eq!(vca_gain(vca_knee_volts()), 1.0);
        assert!((vca_gain(vca_knee_volts() / 2.0) - 0.5).abs() < 1e-12);
        assert_eq!(vca_gain(0.0), K_ASSUMED_VCA_FLOOR);
        // closed, PROCESS at maximum is PROCESS at minimum
        assert!((K_ASSUMED_VCA_FLOOR / R1 - 1.0 / (R1 + VR1)).abs() < 1e-15);
        // the detector rises in about R3 C4 and falls far more slowly
        let (a, r) = detector_steps(48_000.0);
        assert!(a > 100.0 * r);
        let mut e = 0.0;
        for _ in 0..(48_000.0 * 3.0 * R3 * C4) as usize {
            e = detect(e, 1.0, a, r);
        }
        assert!(e > 0.94 && e < 1.0, "{e}");
    }

    #[test]
    fn tune_centre_is_the_drawn_parts() {
        assert_eq!(cap_scale(0.5), 1.0);
        assert!(cap_scale(0.0) > 3.16 && cap_scale(1.0) < 0.317);
    }
}
