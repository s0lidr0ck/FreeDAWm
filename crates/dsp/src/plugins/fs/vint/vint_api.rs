//! VintApi -- the API 550A, as a circuit.
//! Line-by-line translation of `Engine 11 - Vint/Source/VintApi.h`.
//!
//! From API's drawing of the 550A-1: three bands of five frequencies, gain in 2 / 4 / 6 / 9 / 12 dB
//! steps, shelf switches on the outer bands, and the 50 Hz - 15 kHz filter; op-amps ideal.
//! `in -> [filter] -> A1: an inverting summing amp, 10k in, 10k0 || 100p feedback -> out`.
//! Each band is a Sallen-Key band-pass of gain 2; the GAIN switch is the reciprocal network
//! (boost from the input, cut from A1's output) with a ladder of 3k32 / 5k52 / 10k22 / 17k02 /
//! 39k02 into A1's virtual earth. Inferred where the fax is unreadable (as the C++ says): A1's
//! 10k input, the HF's last 2.7n pair, and the shelf switches' wiring.

// The C++'s literals (pi to 20 digits, 0.7071, 1.4142) and index loops are kept verbatim:
// a line-by-line translation, not a rewrite.
#![allow(clippy::excessive_precision, clippy::approx_constant, clippy::needless_range_loop, clippy::manual_clamp)]

use super::vint_mna::{Kind, Netlist};

pub const B_LOW: usize = 0;
pub const B_MID: usize = 1;
pub const B_HIGH: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Controls {
    /// switch position 0..4 (the plugin's order: low to high)
    pub freq: [i32; 3],
    /// 0..10: -12 -9 -6 -4 -2 0 +2 +4 +6 +9 +12
    pub gain: [i32; 3],
    pub low_shelf: bool,
    pub high_shelf: bool,
    pub filter: bool,
}

impl Default for Controls {
    fn default() -> Self {
        Controls { freq: [2, 2, 2], gain: [5, 5, 5], low_shelf: false, high_shelf: false, filter: false }
    }
}

/// Each band's capacitor per switch position (both of the pair), in the plugin's order.
pub fn band_c(band: usize, pos: i32) -> f64 {
    const LO: [f64; 5] = [935e-9, 465e-9, 223e-9, 141e-9, 102e-9]; // 50 .. 400 Hz
    const MID: [f64; 5] = [136.62e-9, 68.62e-9, 35.62e-9, 17.62e-9, 10.82e-9]; // 400 .. 5k
    const HI: [f64; 5] = [9.88e-9, 7.18e-9, 4.98e-9, 3.98e-9, 3.3e-9]; // 5k .. 15k
    let pos = pos.clamp(0, 4) as usize;
    if band == B_LOW {
        LO[pos]
    } else if band == B_MID {
        MID[pos]
    } else {
        HI[pos]
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BandParts {
    pub r_in: f64,
    pub r_back: f64,
    pub r_gnd: f64,
    pub c_amp: f64,
}

pub fn band_parts(band: usize) -> BandParts {
    if band == B_LOW {
        return BandParts { r_in: 6650.0, r_back: 3650.0, r_gnd: 4990.0, c_amp: 6.8e-9 }; // C13 across the amp's 4k7
    }
    if band == B_MID {
        return BandParts { r_in: 5900.0, r_back: 3240.0, r_gnd: 4120.0, c_amp: 1.0e-9 }; // C17
    }
    BandParts { r_in: 6340.0, r_back: 3480.0, r_gnd: 2750.0 + 2000.0, c_amp: 100e-12 } // C16
}

/// The ladder from the band's output into A1's virtual earth for |step| 1..5 (2 .. 12 dB).
pub fn ladder(step: i32) -> f64 {
    const R: [f64; 6] = [0.0, 39.02e3, 17.02e3, 10.22e3, 5.52e3, 3.32e3];
    R[step.clamp(1, 5) as usize]
}

struct Builder {
    n: Netlist,
    next: i32,
}

impl Builder {
    fn new() -> Self {
        Builder { n: Netlist::default(), next: 2 }
    }
    fn node(&mut self) -> i32 {
        let v = self.next;
        self.next += 1;
        v
    }
    fn r(&mut self, a: i32, b: i32, v: f64) {
        self.n.add(Kind::R, a, b, v);
    }
    fn c(&mut self, a: i32, b: i32, v: f64) {
        self.n.add(Kind::C, a, b, v);
    }

    /// A band: input from `src`, output returned into the virtual earth `ve`.
    #[allow(clippy::too_many_arguments)]
    fn band(&mut self, which: usize, src: i32, ve: i32, step: i32, cap: f64, shelf: bool) {
        let p = band_parts(which);
        let n1 = self.node();
        let pl = self.node();
        let m = self.node();
        let o = self.node();
        self.r(src, n1, p.r_in);
        if !shelf {
            self.c(n1, 0, cap);
            self.c(n1, pl, cap);
            self.r(pl, 0, p.r_gnd);
            self.r(o, n1, p.r_back);
        } else if which == B_LOW {
            self.c(n1, 0, cap);
            self.r(n1, pl, 1.0e-3); // series capacitor shorted
            self.r(pl, 0, p.r_gnd);
        } else {
            self.c(n1, pl, cap); // capacitor to ground out
            self.r(pl, 0, p.r_gnd);
        }
        self.n.add_op_amp(pl, m, o);
        self.r(o, m, 4.7e3);
        self.c(o, m, p.c_amp);
        self.r(m, 0, 4.7e3);
        self.r(o, ve, ladder(step));
    }
}

/// The EQ: in at node 1, out (in phase) at the returned netlist's out.
pub fn eq_section(k: &Controls) -> Netlist {
    let mut b = Builder::new();
    let ve = b.node();
    let y = b.node();
    let out = b.node();
    b.r(1, ve, 10.0e3);
    b.r(ve, y, 10.0e3);
    b.c(ve, y, 100e-12);
    b.n.add_op_amp(0, ve, y);
    for band in 0..3 {
        let g = k.gain[band].clamp(0, 10) - 5; // -5 .. +5 steps
        if g == 0 {
            continue; // switch at 0: out of circuit
        }
        let shelf = (band == B_LOW && k.low_shelf) || (band == B_HIGH && k.high_shelf);
        b.band(band, if g > 0 { 1 } else { y }, ve, g.abs(), band_c(band, k.freq[band]), shelf);
    }
    b.n.add_vcvs(0, y, out, 1.0); // the output transformer's phase
    b.n.out = out;
    b.n
}

/// The 50 Hz - 15 kHz filter (S1), in at node 1. Off: a wire.
pub fn filter_section(k: &Controls) -> Netlist {
    let mut b = Builder::new();
    if !k.filter {
        b.n.out = 1;
        return b.n;
    }
    // C5 .1 - R9 2k4 - node a - C6 .1 - R10 2k7 - node p (+ in; C7 3n3 and R11 43k to ground);
    // gain 1 + 1k6 / 10k; R12 16k || C8 6n8 from the output back to node a
    let a = b.node();
    let s = b.node();
    let t = b.node();
    let p = b.node();
    let m = b.node();
    let o = b.node();
    b.c(1, s, 0.1e-6);
    b.r(s, a, 2.4e3);
    b.c(a, t, 0.1e-6);
    b.r(t, p, 2.7e3);
    b.c(p, 0, 3.3e-9);
    b.r(p, 0, 43.0e3);
    b.n.add_op_amp(p, m, o);
    b.r(o, m, 1.6e3);
    b.r(m, 0, 10.0e3);
    b.r(o, a, 16.0e3);
    b.c(o, a, 6.8e-9);
    b.n.out = o;
    b.n
}

#[cfg(test)]
mod tests {
    use super::super::vint_mna::ac;
    use super::*;

    #[test]
    fn gain_steps_land_on_their_db() {
        let db = |k: &Controls, f: f64| 20.0 * ac(&eq_section(k), f).norm().log10();
        assert!(db(&Controls::default(), 1000.0).abs() < 1e-3);
        for (g, want) in [(6, 2.0), (7, 4.0), (8, 6.0), (9, 9.0), (10, 12.0), (0, -12.0)] {
            let mut k = Controls::default();
            k.gain[1] = g;
            k.freq[1] = 2; // 1.5k
            let got = (0..200)
                .map(|i| db(&k, 1000.0 + 10.0 * f64::from(i)))
                .fold(if want > 0.0 { f64::MIN } else { f64::MAX }, |a, b| if want > 0.0 { a.max(b) } else { a.min(b) });
            assert!((got - want).abs() < 0.25, "step {g}: {got}");
        }
        assert_eq!(filter_section(&Controls::default()).out, 1);
    }
}
