//! VintNeve -- the Neve 1073's EQ, as circuits.
//! Line-by-line translation of `Engine 11 - Vint/Source/VintNeve.h`.
//!
//! From Neve's own drawings: B182 (high-pass), B205 (high and low EQ, a Baxandall in the feedback
//! of an inverting amp), B211 (presence), BA284 (the EQ amplifiers, ideal op-amps here).
//! Inductors from Sowter's data for their replacements. The high-pass's load is taken as 3k9;
//! the 10k across B182's first two switch capacitors is left out; the pots are linear, and the
//! knobs are calibrated to the circuit's own dB (`pot_for`).

// The C++'s literals (pi to 20 digits, 0.7071, 1.4142) and index loops are kept verbatim:
// a line-by-line translation, not a rewrite.
#![allow(clippy::excessive_precision, clippy::approx_constant, clippy::needless_range_loop, clippy::manual_clamp)]

use std::sync::OnceLock;

use super::vint_mna::{Kind, Netlist, ac};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Controls {
    /// 0 off, 1..4: 50 / 80 / 160 / 300 Hz
    pub hpf: i32,
    /// 0 off, 1..4: 35 / 60 / 110 / 220 Hz
    pub low: i32,
    /// RV2 wiper, 0 (cut end, the output side) .. 1 (boost end, the input side)
    pub low_pot: f64,
    /// 0..5: 360, 700, 1.6k, 3.2k, 4.8k, 7.2k (the presence switch)
    pub mid: i32,
    pub mid_on: bool,
    /// RV3
    pub mid_pot: f64,
    /// RV1
    pub high_pot: f64,
    pub hpf_load: f64,
    pub hpf_source: f64,
}

impl Default for Controls {
    fn default() -> Self {
        Controls { hpf: 0, low: 0, low_pot: 0.5, mid: 0, mid_on: true, mid_pot: 0.5, high_pot: 0.5, hpf_load: 3.9e3, hpf_source: 1.2e3 }
    }
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
        self.n.add(Kind::R, a, b, v.max(1.0e-3));
    }
    fn c(&mut self, a: i32, b: i32, v: f64) {
        self.n.add(Kind::C, a, b, v);
    }
    /// A pot between a (position 1) and b (position 0): returns the wiper.
    fn pot(&mut self, a: i32, b: i32, total: f64, pos: f64) -> i32 {
        let w = self.node();
        let pos = pos.clamp(0.0, 1.0);
        self.r(a, w, (1.0 - pos) * total);
        self.r(w, b, pos * total);
        w
    }
}

/// B205 around an inverting amp; in at node 1, out (in phase) returned.
pub fn low_high_section(k: &Controls) -> Netlist {
    let mut b = Builder::new();
    let ve = b.node();
    let y = b.node();
    let z = b.node();
    let out = b.node();
    b.n.add_op_amp(0, ve, y);
    b.r(z, ve, 12.0e3);
    b.c(z, ve, 10e-9);
    // HF
    let a1 = b.node();
    let p = b.node();
    let nn = b.node();
    let a2 = b.node();
    b.r(1, a1, 620.0);
    b.c(a1, p, 22e-9);
    b.r(y, a2, 620.0);
    b.c(a2, nn, 22e-9);
    let w1 = b.pot(p, nn, 10.0e3, k.high_pot);
    b.c(w1, z, 15e-9);
    // LF
    let aa = b.node();
    let v = b.node();
    b.r(1, aa, 6.8e3);
    b.r(y, v, 6.8e3);
    if k.low <= 0 {
        b.r(aa, ve, 1.0e-3);
        b.r(v, ve, 1.0e-3);
    } else {
        const CAP: [f64; 5] = [0.0, 200e-9, 100e-9, 47e-9, 15e-9];
        let w2 = b.pot(aa, v, 50.0e3, k.low_pot);
        b.r(w2, ve, 1.0e-3);
        let cap = CAP[k.low.clamp(1, 4) as usize];
        b.c(aa, z, cap);
        b.c(v, z, cap);
    }
    b.n.add_vcvs(0, y, out, 1.0);
    b.n.out = out;
    b.n
}

/// B211 around the other inverting amp.
pub fn presence_section(k: &Controls) -> Netlist {
    let mut b = Builder::new();
    let ve = b.node();
    let y = b.node();
    let x = b.node();
    let out = b.node();
    b.n.add_op_amp(0, ve, y);
    b.r(1, x, 6.8e3);
    b.r(x, y, 6.8e3);
    b.r(x, ve, 8.2e3);
    if k.mid_on {
        // { L, Rw, C }
        const LC: [(f64, f64, f64); 6] =
            [(2.0, 85.0, 100e-9), (1.1, 59.0, 47e-9), (0.45, 35.0, 22e-9), (0.2, 32.0, 12.2e-9), (0.2, 32.0, 4.7e-9), (0.2, 32.0, 2.42e-9)];
        let (l, rw, c) = LC[k.mid.clamp(0, 5) as usize];
        let w = b.pot(1, y, 10.0e3, k.mid_pot);
        let m1 = b.node();
        let m2 = b.node();
        let yn = b.node();
        b.c(w, m1, c);
        b.n.add(Kind::L, m1, m2, l);
        b.r(m2, yn, rw);
        b.r(yn, ve, 2.7e3);
    }
    b.n.add_vcvs(0, y, out, 1.0);
    b.n.out = out;
    b.n
}

/// B182: fed through 1k2, into the next stage; its passband loss made up. Off: a wire.
pub fn hpf_section(k: &Controls) -> Netlist {
    let mut b = Builder::new();
    if k.hpf <= 0 {
        b.n.out = 1;
        return b.n;
    }
    // { c, L, Rw }
    const POS: [(f64, f64, f64); 5] = [(0.0, 0.0, 0.0), (1.0e-6, 10.0, 1000.0), (0.47e-6, 7.0, 800.0), (0.22e-6, 3.0, 490.0), (0.122e-6, 1.3, 310.0)];
    let (c, l, rw) = POS[k.hpf.clamp(1, 4) as usize];
    let a = b.node();
    let t = b.node();
    let l1 = b.node();
    let o = b.node();
    let out = b.node();
    b.r(1, a, k.hpf_source);
    b.c(a, t, c);
    b.n.add(Kind::L, t, l1, l);
    b.r(l1, 0, rw);
    b.c(t, o, c);
    b.r(o, 0, k.hpf_load);
    b.n.add_vcvs(o, 0, out, (k.hpf_source + k.hpf_load) / k.hpf_load);
    b.n.out = out;
    b.n
}

// ---------------------------------------------------------------- the knobs' calibration
/// The knobs are labelled in dB: the LF shelf's full depth, the HF shelf's full height, the
/// presence peak. Each pot is turned to where the circuit gives that, measured once.
/// `[boost, cut][position 0.5 +- i / 64]`
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PotTable {
    pub db: [[f64; 33]; 2],
}

pub fn measure_pot(which: i32) -> PotTable {
    let mut t = PotTable { db: [[0.0; 33]; 2] };
    for s in 0..2 {
        for i in 0..33 {
            let mut k = Controls::default();
            let pos = 0.5 + (if s == 0 { 1.0 } else { -1.0 }) * f64::from(i) / 64.0;
            let h = if which == 0 {
                k.low = 2;
                k.low_pot = pos;
                ac(&low_high_section(&k), 0.5).norm()
            } else if which == 1 {
                k.low = 2;
                k.high_pot = pos;
                ac(&low_high_section(&k), 400000.0).norm() // LF at its default, 60 Hz
            } else {
                k.mid = 2;
                k.mid_pot = pos;
                ac(&presence_section(&k), 1600.0).norm()
            };
            t.db[s][i as usize] = 20.0 * h.log10();
        }
    }
    t
}

/// The three tables, measured on first use (the C++'s function static). The engine forces this
/// in its constructor, as the C++ does, so the audio thread never measures.
pub fn pot_table(which: i32) -> &'static PotTable {
    static T: OnceLock<[PotTable; 3]> = OnceLock::new();
    let t = T.get_or_init(|| [measure_pot(0), measure_pot(1), measure_pot(2)]);
    &t[which.clamp(0, 2) as usize]
}

/// The pot position (0 .. 1) giving db: which 0 LF, 1 HF, 2 presence. Past the circuit's range,
/// the pot's end.
pub fn pot_for(which: i32, db: f64) -> f64 {
    let t = pot_table(which);
    let s = if db >= 0.0 { 0 } else { 1 };
    let a = db.abs();
    let sign = if s == 0 { 1.0 } else { -1.0 };
    for i in 1..33 {
        let lo = t.db[s][i - 1].abs();
        let hi = t.db[s][i].abs();
        if a <= hi {
            return 0.5 + sign * ((i - 1) as f64 + (a - lo) / (hi - lo).max(1.0e-12)) / 64.0;
        }
    }
    if s == 0 { 1.0 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_is_flat_and_the_knobs_land_on_their_db() {
        let k = Controls { low: 2, ..Controls::default() };
        for f in [30.0, 1000.0, 10000.0] {
            assert!((20.0 * ac(&low_high_section(&k), f).norm().log10()).abs() < 0.05, "{f}");
        }
        let k = Controls { mid: 2, mid_pot: pot_for(2, 12.0), ..Controls::default() };
        assert!((20.0 * ac(&presence_section(&k), 1600.0).norm().log10() - 12.0).abs() < 0.01);
        assert_eq!(hpf_section(&Controls::default()).out, 1);
        let k = Controls { hpf: 2, ..Controls::default() };
        assert!(20.0 * ac(&hpf_section(&k), 20.0).norm().log10() < -20.0);
    }
}
