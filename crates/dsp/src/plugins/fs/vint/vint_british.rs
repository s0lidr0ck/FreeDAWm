//! VintBritish -- the SSL 4000 E channel EQ, as circuits.
//! Line-by-line translation of `Engine 11 - Vint/Source/VintBritish.h` (its knob tables are
//! `tables/vint_british_tables.rs`, generated from `VintBritishTables.h`).
//!
//! SSL's own drawings of the two E-series EQ cards, 82E02 (brown) and 82E242 (black), with the
//! NE5534s as ideal op-amps, coupling capacitors as shorts, the J112 FETs as switches:
//! `in -> T3 (HF + HMF) -> T8 (LMF + LF) -> out`, two inverting summing amps, 4k75 in/out.
//! Each piece is its own netlist driven by the op-amp before it: the filters, T3's section, T8's.

// The C++'s literals (pi to 20 digits, 0.7071, 1.4142) and index loops are kept verbatim:
// a line-by-line translation, not a rewrite.
#![allow(clippy::excessive_precision, clippy::approx_constant, clippy::needless_range_loop, clippy::manual_clamp)]

use super::tables::vint_british_tables::{K_GAIN_DB, K_Q, K_TABLE_POINTS};
use super::vint_mna::{Kind, Netlist};

pub const K_PI: f64 = 3.14159265358979323846;

pub const B_LF: usize = 0;
pub const B_LMF: usize = 1;
pub const B_HMF: usize = 2;
pub const B_HF: usize = 3;

/// The controls as the circuit sees them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Controls {
    /// 82E242 (black) or 82E02 (brown)
    pub black: bool,
    /// the filters' pot resistance per section; < 0: filter out
    pub hpf_r: f64,
    pub lpf_r: f64,
    /// each BOOST/CUT pot: -1 full cut .. 0 centre .. +1 full boost
    pub boost: [f64; 4],
    /// each FREQ pot's resistance per section (0 .. 50k)
    pub freq_r: [f64; 4],
    /// LMF, HMF Q pots' rotation 0 (broad) .. 1 (narrow)
    pub q_rot: [f64; 2],
    pub lf_bell: bool,
    pub hf_bell: bool,
    /// the 242's SET HF Q and SET LF Q trims: bell as loud as shelf
    pub rv1: f64,
    pub rv4: f64,
}

impl Default for Controls {
    fn default() -> Self {
        Controls {
            black: false,
            hpf_r: -1.0,
            lpf_r: -1.0,
            boost: [0.0; 4],
            freq_r: [0.0; 4],
            q_rot: [0.5, 0.5],
            lf_bell: false,
            hf_bell: false,
            rv1: 3.75e3,
            rv4: 1.0e3,
        }
    }
}

/// The band filters' parts, per card.
#[derive(Debug, Clone, Copy)]
pub struct MidParts {
    pub ri: f64,
    pub rf: f64,
    pub c: f64,
    pub r20: f64,
    pub r21: f64,
    pub r22: f64,
}

pub fn mid_parts(black: bool, band: usize) -> MidParts {
    if band == B_LMF {
        return if black {
            MidParts { ri: 3.0e3, rf: 3.6e3, c: 22e-9, r20: 470.0, r21: 1.8e3, r22: 910.0 + 250.0 }
        } else {
            MidParts { ri: 4.3e3, rf: 5.1e3, c: 15e-9, r20: 470.0, r21: 1.8e3, r22: 910.0 + 250.0 }
        };
    }
    if black {
        MidParts { ri: 3.0e3, rf: 3.6e3, c: 4.7e-9, r20: 470.0, r21: 1.6e3, r22: 910.0 + 250.0 }
    } else {
        MidParts { ri: 4.3e3, rf: 5.1e3, c: 4.7e-9, r20: 470.0, r21: 1.8e3, r22: 910.0 + 250.0 }
    }
}

/// Each band-pass's two resistors and two capacitors (bell), for the FREQ pot's calibration.
#[derive(Debug, Clone, Copy)]
pub struct Rc {
    pub ri: f64,
    pub rf: f64,
    pub ci: f64,
    pub cf: f64,
}

pub fn band_rc(black: bool, band: usize) -> Rc {
    match band {
        B_LF => {
            if black {
                Rc { ri: 4.3e3, rf: 4.7e3, ci: 80e-9, cf: 80e-9 }
            } else {
                Rc { ri: 4.3e3, rf: 5.1e3, ci: 80e-9, cf: 80e-9 }
            }
        }
        B_HF => {
            if black {
                Rc { ri: 3.0e3, rf: 3.6e3, ci: 2.2e-9, cf: 2.42e-9 }
            } else {
                Rc { ri: 4.3e3, rf: 5.1e3, ci: 2.2e-9, cf: 2.42e-9 }
            }
        }
        _ => {
            let m = mid_parts(black, band);
            Rc { ri: m.ri, rf: m.rf, ci: m.c, cf: m.c }
        }
    }
}

/// The FREQ pot's resistance that puts the band-pass's centre at f. Held to the pot's 0 .. 50k.
pub fn freq_pot_for(black: bool, band: usize, f: f64) -> f64 {
    let b = band_rc(black, band);
    let k = 1.0 / ((2.0 * K_PI * f).powf(2.0) * b.ci * b.cf);
    let s = b.ri + b.rf;
    let r = 0.5 * (-s + (0.0f64.max(s * s - 4.0 * (b.ri * b.rf - k))).sqrt());
    r.clamp(0.0, 50.0e3)
}

pub fn hpf_pot_for(black: bool, f: f64) -> f64 {
    let c = if black { 220e-9 } else { (220e-9f64 * 330e-9).sqrt() };
    (1.0 / (2.0 * K_PI * f * c) - 1.8e3).clamp(0.0, if black { 47.0e3 } else { 50.0e3 })
}

pub fn lpf_pot_for(black: bool, f: f64) -> f64 {
    (1.0 / (2.0 * K_PI * f * (6.8e-9f64 * 3.3e-9).sqrt()) - if black { 680.0 } else { 470.0 }).clamp(0.0, 10.0e3)
}

/// A log ("A") taper: the fraction of the track at rotation x, 10 % at the middle.
pub fn log_taper(x: f64) -> f64 {
    (81.0f64.powf(x.clamp(0.0, 1.0)) - 1.0) / 80.0
}

/// The Q tables' grid: most of the Q pot's range is in the last few degrees of its travel.
pub fn q_rot_at(u: f64) -> f64 {
    let v = 1.0 - u.clamp(0.0, 1.0);
    1.0 - v * v
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
    fn wire(&mut self, a: i32, b: i32) {
        self.n.add(Kind::R, a, b, 1.0e-3);
    }

    /// A BOOST/CUT pot: 25k, centre-tapped to ground through 4k7, 1k0 to each end. Returns the
    /// wiper.
    fn boost_pot(&mut self, in_node: i32, out_node: i32, boost: f64) -> i32 {
        let e1 = self.node();
        let e2 = self.node();
        let w = self.node();
        let tap = self.node();
        self.r(in_node, e1, 1.0e3);
        self.r(e2, out_node, 1.0e3);
        self.r(tap, 0, 4.7e3);
        let t = 0.5 * (1.0 - boost.clamp(-1.0, 1.0)); // along the track from the input end
        if t <= 0.5 {
            self.r(e1, w, t * 25.0e3);
            self.r(w, tap, (0.5 - t) * 25.0e3);
            self.r(tap, e2, 12.5e3);
        } else {
            self.r(e1, tap, 12.5e3);
            self.r(tap, w, (t - 0.5) * 25.0e3);
            self.r(w, e2, (1.0 - t) * 25.0e3);
        }
        self.r(w, 0, 1.8e3); // R3 / R17 / R26 / R41
        w
    }

    /// A mid band (LMF or HMF): returns the gain stage's output.
    fn mid_band(&mut self, black: bool, band: usize, wiper: i32, freq_r: f64, q_rot: f64) -> i32 {
        let p = mid_parts(black, band);
        let a = self.node();
        let b = self.node();
        let m = self.node();
        let d = self.node();
        let o = self.node();
        let pl = self.node();
        self.r(wiper, a, freq_r);
        self.r(a, b, p.ri);
        self.c(b, m, p.c);
        self.r(m, d, p.rf);
        self.r(d, o, freq_r);
        self.c(m, o, p.c);
        // the Q pot's first section taps the (R21 || 10k) string; its wiper is the + input
        let n19 = self.node();
        let n18 = self.node();
        let ta = log_taper(q_rot);
        self.r(o, n19, p.r22);
        self.r(n19, n18, p.r21);
        self.r(n19, pl, (1.0 - ta) * 10.0e3);
        self.r(pl, n18, ta * 10.0e3);
        self.r(n18, 0, p.r20);
        self.r(pl, 0, 100.0e3);
        self.n.add_op_amp(pl, m, o);
        // the gain stage: 750 in, 750 + the Q pot's second section back (ganged against the first)
        let m2 = self.node();
        let e = self.node();
        let o2 = self.node();
        self.r(o, m2, 750.0);
        self.r(m2, e, 750.0);
        self.r(e, o2, (1.0 - log_taper(q_rot)) * 10.0e3);
        self.n.add_op_amp(0, m2, o2);
        o2
    }

    fn hf_band(&mut self, black: bool, wiper: i32, freq_r: f64, bell: bool, rv1: f64) -> i32 {
        let b = band_rc(black, B_HF);
        let n9 = self.node();
        let a = self.node();
        let m = self.node();
        let d = self.node();
        let o = self.node();
        let x = self.node();
        let pl = self.node();
        self.r(wiper, n9, freq_r);
        self.c(wiper, n9, 220e-12);
        self.r(n9, a, b.ri);
        self.c(a, m, 2.2e-9);
        self.r(m, d, b.rf);
        self.r(d, o, freq_r);
        self.c(m, o, 220e-12);
        self.c(m, x, 2.2e-9);
        if !black {
            self.c(x, o, 330e-12); // C6
            self.r(pl, 0, 1.0e3); // R6, R7 (Q link fitted)
            self.r(pl, x, 1.8e3);
            if bell {
                self.wire(x, o);
            }
        } else {
            self.r(pl, 0, 5.1e3); // R6, R7 + RV1 (SET HF Q)
            self.r(pl, x, 2.4e3 + rv1);
            self.r(o, x, 100.0e3); // R79
            if bell {
                self.wire(x, o); // J112 on
            }
        }
        self.n.add_op_amp(pl, m, o);
        let m2 = self.node();
        let o2 = self.node();
        if !black {
            self.r(o, m2, 750.0);
            self.r(m2, o2, 7.5e3);
        } else {
            self.r(o, m2, 3.0e3);
            self.r(m2, o2, 43.0e3);
            self.r(m2, o2, if bell { 33.0e3 } else { 133.0e3 });
        }
        self.n.add_op_amp(0, m2, o2);
        o2
    }

    fn lf_band(&mut self, black: bool, wiper: i32, freq_r: f64, bell: bool, rv4: f64) -> i32 {
        let b = band_rc(black, B_LF);
        let n34 = self.node();
        let q = self.node();
        let m = self.node();
        let d = self.node();
        let o = self.node();
        let pl = self.node();
        self.r(wiper, n34, freq_r);
        if bell {
            self.c(n34, q, 33e-9 + 47e-9); // C16 || C17, one state
        } else {
            self.wire(n34, q);
        }
        self.r(q, m, b.ri);
        self.r(m, d, b.rf);
        self.r(d, o, freq_r);
        self.c(m, o, 33e-9 + 47e-9); // C18 || C19
        if !black {
            let n38 = self.node();
            self.r(pl, 0, 1.0e3); // R44, R45 (Q link fitted)
            self.r(pl, n38, 1.0e3);
            if bell {
                self.wire(n38, o); // R46 shorted for bell
            } else {
                self.r(o, n38, 2.2e3);
            }
        } else {
            self.r(pl, 0, 5.1e3);
            let x = self.node();
            self.r(pl, x, 2.4e3 + rv4); // R45 + RV4 (SET LF Q)
            if bell {
                self.wire(x, o); // J112 on / R85
            } else {
                self.r(x, o, 100.0e3);
            }
        }
        self.n.add_op_amp(pl, m, o);
        let m2 = self.node();
        let o2 = self.node();
        if !black {
            self.r(o, m2, 750.0);
            self.r(m2, o2, 3.3e3);
        } else {
            self.r(o, m2, 3.0e3);
            self.r(m2, o2, 43.0e3);
            self.r(m2, o2, if bell { 7.5e3 } else { 107.5e3 });
        }
        self.n.add_op_amp(0, m2, o2);
        o2
    }
}

/// T3's section: HF and HMF around one inverting summing amp. In at node 1.
pub fn high_section(k: &Controls) -> Netlist {
    let mut b = Builder::new();
    let v = b.node();
    let o = b.node();
    b.r(1, v, 4.75e3);
    b.r(v, o, 4.75e3);
    b.c(v, o, if k.black { 220e-12 } else { 330e-12 });
    b.n.add_op_amp(0, v, o);
    let w_hmf = b.boost_pot(1, o, k.boost[B_HMF]);
    let w_hf = b.boost_pot(1, o, k.boost[B_HF]);
    let o_hmf = b.mid_band(k.black, B_HMF, w_hmf, k.freq_r[B_HMF], k.q_rot[1]);
    let o_hf = b.hf_band(k.black, w_hf, k.freq_r[B_HF], k.hf_bell, k.rv1);
    b.r(o_hmf, v, 4.75e3); // R13, R14
    b.r(o_hf, v, 4.75e3);
    b.r(w_hmf, o_hf, 12.0e3); // R64 / R94, R63 / R95
    b.r(o_hmf, w_hf, 12.0e3);
    b.n.out = o;
    b.n
}

/// T8's section: LMF and LF. In at node 1 (T3's output).
pub fn low_section(k: &Controls) -> Netlist {
    let mut b = Builder::new();
    let v = b.node();
    let o = b.node();
    b.r(1, v, 4.75e3);
    b.r(v, o, 4.75e3);
    b.c(v, o, if k.black { 220e-12 } else { 150e-12 });
    b.n.add_op_amp(0, v, o);
    let w_lmf = b.boost_pot(1, o, k.boost[B_LMF]);
    let w_lf = b.boost_pot(1, o, k.boost[B_LF]);
    let o_lmf = b.mid_band(k.black, B_LMF, w_lmf, k.freq_r[B_LMF], k.q_rot[0]);
    let o_lf = b.lf_band(k.black, w_lf, k.freq_r[B_LF], k.lf_bell, k.rv4);
    b.r(o_lmf, v, 4.75e3); // R36, R37
    b.r(o_lf, v, 4.75e3);
    b.r(w_lmf, o_lf, 12.0e3); // R65 / R96, R66 / R97
    b.r(o_lmf, w_lf, 12.0e3);
    b.n.out = o;
    b.n
}

/// The filters, in front of the EQ. Both out: a wire.
pub fn filter_section(k: &Controls) -> Netlist {
    let mut b = Builder::new();
    let mut inp = 1;
    if k.hpf_r >= 0.0 {
        if !k.black {
            let a = b.node();
            let s = b.node();
            let p = b.node();
            let m = b.node();
            let o = b.node();
            b.r(inp, a, 300.0);
            b.r(a, 0, 510.0);
            b.c(a, s, 220e-9);
            b.r(s, o, 1.8e3 + k.hpf_r);
            b.c(s, p, 330e-9);
            b.r(p, 0, 1.8e3 + k.hpf_r);
            b.r(m, o, 3.0e3);
            b.r(m, 0, 5.1e3);
            b.n.add_op_amp(p, m, o);
            inp = o;
        } else {
            // first-order section, then the unity-gain-set pad (ideal: x 0.5) and a Sallen-Key
            // of gain 2: 18 dB/oct Butterworth
            let p1 = b.node();
            let o1 = b.node();
            let s = b.node();
            let p = b.node();
            let m = b.node();
            let o = b.node();
            b.c(inp, p1, 220e-9);
            b.r(p1, 0, 1.8e3 + k.hpf_r);
            b.n.add_vcvs(p1, 0, o1, 0.5);
            b.c(o1, s, 220e-9);
            b.r(s, o, 1.8e3 + k.hpf_r);
            b.c(s, p, 220e-9);
            b.r(p, 0, 1.8e3 + k.hpf_r);
            b.r(m, o, 7.5e3);
            b.r(m, 0, 7.5e3);
            b.n.add_op_amp(p, m, o);
            inp = o;
        }
    }
    if k.lpf_r >= 0.0 {
        let rs = if k.black { 680.0 } else { 470.0 };
        let a = b.node();
        let p = b.node();
        let o = b.node();
        b.r(inp, a, rs + k.lpf_r);
        b.c(a, o, 6.8e-9);
        b.r(a, p, rs + k.lpf_r);
        b.c(p, 0, 3.3e-9);
        b.n.add_op_amp(p, o, o);
        inp = o;
    }
    b.n.out = inp;
    b.n
}

// ---------------------------------------------------------------- the knobs' calibration

/// The BOOST/CUT pot position (-1 .. 1) for db, for a slot of kGainDb (0 LF shelf, 1 LF bell,
/// 2 LMF, 3 HMF, 4 HF shelf, 5 HF bell). Past the circuit's range: the pot's end.
pub fn boost_for(black: bool, slot: usize, db: f64) -> f64 {
    let t = &K_GAIN_DB[usize::from(black)][slot.min(5)][if db >= 0.0 { 0 } else { 1 }];
    let a = db.abs();
    for i in 1..K_TABLE_POINTS {
        let lo = t[i - 1].abs();
        let hi = t[i].abs();
        if a <= hi {
            let pos = ((i - 1) as f64 + (a - lo) / (hi - lo).max(1.0e-12)) / (K_TABLE_POINTS as f64 - 1.0);
            return if db >= 0.0 { pos } else { -pos };
        }
    }
    if db >= 0.0 { 1.0 } else { -1.0 }
}

/// The mid Q pot's rotation for q (mid 0 LMF, 1 HMF), interpolated in log Q.
pub fn q_rot_for(black: bool, mid: usize, q: f64) -> f64 {
    let t = &K_Q[usize::from(black)][mid.min(1)];
    if q <= t[0] {
        return q_rot_at(0.0);
    }
    let lq = q.ln();
    for i in 1..K_TABLE_POINTS {
        if q <= t[i] {
            let lo = t[i - 1].ln();
            let hi = t[i].ln();
            return q_rot_at(((i - 1) as f64 + (lq - lo) / (hi - lo).max(1.0e-12)) / (K_TABLE_POINTS as f64 - 1.0));
        }
    }
    1.0
}

/// Two netlists the same, part for part (so a section need not be solved again).
pub fn same(a: &Netlist, b: &Netlist) -> bool {
    if a.count != b.count || a.nodes != b.nodes || a.out != b.out {
        return false;
    }
    a.used().iter().zip(b.used()).all(|(x, y)| x.kind == y.kind && x.a == y.a && x.b == y.b && x.c == y.c && x.value == y.value)
}

#[cfg(test)]
mod tests {
    use super::super::vint_mna::ac;
    use super::*;

    #[test]
    fn flat_card_is_flat_and_a_mid_lands_on_its_label() {
        for black in [false, true] {
            let mut k = Controls { black, ..Controls::default() };
            for (band, f) in [(B_LF, 100.0), (B_LMF, 800.0), (B_HMF, 3000.0), (B_HF, 8000.0)] {
                k.freq_r[band] = freq_pot_for(black, band, f);
            }
            let h = |k: &Controls, f: f64| 20.0 * (ac(&high_section(k), f) * ac(&low_section(k), f)).norm().log10();
            assert!(h(&k, 1000.0).abs() < 0.1, "flat {black}");
            k.boost[B_HMF] = boost_for(black, 3, 12.0);
            assert!(h(&k, 3000.0) > 10.0, "HMF {black}: {}", h(&k, 3000.0));
        }
        assert_eq!(filter_section(&Controls::default()).out, 1);
        assert!(same(&high_section(&Controls::default()), &high_section(&Controls::default())));
        assert!((q_rot_for(false, 0, 0.1) - q_rot_at(0.0)).abs() < 1e-15);
        assert!(hpf_pot_for(true, 100.0) > 0.0 && lpf_pot_for(false, 8000.0) > 0.0);
    }
}
