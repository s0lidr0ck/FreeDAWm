//! `AmpNetwork.h`: the passive parts of an amp (tone stacks, volume pots with their bright caps,
//! coupling caps, the Twin's reverb mix), solved as the circuit they are: modified nodal analysis
//! with trapezoidal companion models. With the knobs still the conductance matrix is fixed and is
//! inverted once; a knob that moves re-values a resistor and the matrix is rebuilt.
//!
//! Translated line by line. Rust addition: the rebuild's scratch matrix lives in the network
//! (sized when the netlist is prepared), where the C++ allocates a fresh `std::vector` on every
//! rebuild; a knob turned on the audio thread re-inverts in place, with no allocation.

use super::amp_tube::{cclamp, cmax};
use rustfft::num_complex::Complex;

#[derive(Debug, Clone, Copy)]
struct Res {
    a: usize,
    b: usize,
    r: f64,
}

#[derive(Debug, Clone, Copy)]
struct Cap {
    a: usize,
    b: usize,
    c: f64,
    hist: f64,
}

#[derive(Debug, Clone, Copy)]
struct Src {
    node: usize,
    r: f64,
    v: f64,
}

/// A linear RC network solved each sample.
#[derive(Debug, Clone)]
pub struct Network {
    n_nodes: usize,
    fs: f64,
    dirty: bool,
    res: Vec<Res>,
    caps: Vec<Cap>,
    srcs: Vec<Src>,
    inv: Vec<f64>,
    rhs: Vec<f64>,
    v: Vec<f64>,
    /// The rebuild's working copy of G (the C++ allocates it per rebuild).
    g: Vec<f64>,
}

impl Default for Network {
    fn default() -> Self {
        Network::new(1)
    }
}

/// (Rust, tests) Pushes a Vec's address and capacity (see the `heap_blocks` methods).
#[doc(hidden)]
#[allow(clippy::ptr_arg)]
pub fn heap_vec<T>(out: &mut Vec<usize>, v: &Vec<T>) {
    out.push(v.as_ptr() as usize);
    out.push(v.capacity());
}

impl Network {
    /// (Rust, tests) Address and capacity of every heap buffer this owns, to check that
    /// processing and resets never reallocate.
    #[doc(hidden)]
    pub fn heap_blocks(&self, out: &mut Vec<usize>) {
        heap_vec(out, &self.res);
        heap_vec(out, &self.caps);
        heap_vec(out, &self.srcs);
        heap_vec(out, &self.inv);
        heap_vec(out, &self.rhs);
        heap_vec(out, &self.v);
        heap_vec(out, &self.g);
    }

    pub const GROUND: usize = 0;

    pub fn new(num_nodes_including_ground: usize) -> Self {
        Network {
            n_nodes: num_nodes_including_ground.max(1),
            fs: 48_000.0,
            dirty: true,
            res: Vec::new(),
            caps: Vec::new(),
            srcs: Vec::new(),
            inv: Vec::new(),
            rhs: Vec::new(),
            v: vec![0.0],
            g: Vec::new(),
        }
    }

    pub fn add_node(&mut self) -> usize {
        self.dirty = true;
        let n = self.n_nodes;
        self.n_nodes += 1;
        n
    }

    /// Returns the element's index, for changing its value later.
    pub fn add_resistor(&mut self, a: usize, b: usize, ohms: f64) -> usize {
        self.res.push(Res { a, b, r: cmax(1.0e-3, ohms) });
        self.dirty = true;
        self.res.len() - 1
    }

    pub fn add_capacitor(&mut self, a: usize, b: usize, farads: f64) -> usize {
        self.caps.push(Cap { a, b, c: farads, hist: 0.0 });
        self.dirty = true;
        self.caps.len() - 1
    }

    /// An input: a voltage behind a series resistance, driving `node`. Returns its index.
    pub fn add_source(&mut self, node: usize, series_ohms: f64) -> usize {
        self.srcs.push(Src { node, r: cmax(1.0e-3, series_ohms), v: 0.0 });
        self.dirty = true;
        self.srcs.len() - 1
    }

    pub fn set_resistor(&mut self, idx: usize, ohms: f64) {
        let ohms = cmax(1.0e-3, ohms);
        if let Some(r) = self.res.get_mut(idx)
            && r.r != ohms
        {
            r.r = ohms;
            self.dirty = true;
        }
    }

    pub fn set_source_resistance(&mut self, idx: usize, ohms: f64) {
        let ohms = cmax(1.0e-3, ohms);
        if let Some(s) = self.srcs.get_mut(idx)
            && s.r != ohms
        {
            s.r = ohms;
            self.dirty = true;
        }
    }

    pub fn prepare(&mut self, sample_rate: f64) {
        self.fs = sample_rate;
        let n = self.n_nodes - 1;
        // (Rust: the working storage, once)
        self.g = vec![0.0; n * n];
        self.inv = vec![0.0; n * n];
        self.rhs = vec![0.0; n];
        self.v.resize(self.n_nodes, 0.0);
        self.dirty = true;
        self.rebuild();
        self.reset();
    }

    pub fn reset(&mut self) {
        for c in &mut self.caps {
            c.hist = 0.0;
        }
        self.v.iter_mut().for_each(|x| *x = 0.0);
    }

    pub fn set_input(&mut self, src_idx: usize, volts: f64) {
        if let Some(s) = self.srcs.get_mut(src_idx) {
            s.v = volts;
        }
    }

    /// One sample; read node voltages with `voltage`.
    pub fn step(&mut self) {
        if self.dirty {
            self.rebuild();
        }
        let n = self.n_nodes - 1; // unknowns: nodes 1..n
        if self.rhs.len() < n || self.inv.len() < n * n || self.v.len() < n + 1 {
            return; // (not prepared)
        }
        self.rhs.iter_mut().for_each(|x| *x = 0.0);

        for s in &self.srcs {
            if s.node > 0 {
                self.rhs[s.node - 1] += s.v / s.r;
            }
        }

        for c in &self.caps {
            // trapezoidal companion: i = gC (va - vb) - hist, flowing a -> b
            if c.a > 0 {
                self.rhs[c.a - 1] += c.hist;
            }
            if c.b > 0 {
                self.rhs[c.b - 1] -= c.hist;
            }
        }

        for i in 0..n {
            let mut acc = 0.0;
            let row = &self.inv[i * n..i * n + n];
            for (j, r) in row.iter().enumerate() {
                acc += r * self.rhs[j];
            }
            self.v[i + 1] = acc;
        }

        let fs = self.fs;
        for c in &mut self.caps {
            let vab = self.v[c.a] - self.v[c.b];
            let g = 2.0 * c.c * fs;
            let i = g * vab - c.hist;
            c.hist = g * vab + i;
        }
    }

    pub fn voltage(&self, node: usize) -> f64 {
        self.v.get(node).copied().unwrap_or(0.0)
    }

    /// The steady-state transfer from source `src_idx` to `node` at `hz`, all other sources
    /// silent: the same netlist, solved in the frequency domain. Allocates (UI / tests).
    pub fn ac_response(&self, src_idx: usize, node: usize, hz: f64) -> Complex<f64> {
        type C = Complex<f64>;
        let n = self.n_nodes - 1;
        let mut a = vec![C::new(0.0, 0.0); n * n];
        let mut b = vec![C::new(0.0, 0.0); n];
        let mut add = |a_: usize, bb: usize, y: C| {
            if a_ > 0 {
                a[(a_ - 1) * n + (a_ - 1)] += y;
            }
            if bb > 0 {
                a[(bb - 1) * n + (bb - 1)] += y;
            }
            if a_ > 0 && bb > 0 {
                a[(a_ - 1) * n + (bb - 1)] -= y;
                a[(bb - 1) * n + (a_ - 1)] -= y;
            }
        };
        let jw = C::new(0.0, 2.0 * std::f64::consts::PI * hz);
        for r in &self.res {
            add(r.a, r.b, C::new(1.0 / r.r, 0.0));
        }
        for c in &self.caps {
            add(c.a, c.b, jw * c.c);
        }
        for (k, s) in self.srcs.iter().enumerate() {
            add(s.node, Self::GROUND, C::new(1.0 / s.r, 0.0));
            if k == src_idx && s.node > 0 {
                b[s.node - 1] += 1.0 / s.r;
            }
        }
        solve_complex(&mut a, &mut b, n);
        if node > 0 { b.get(node - 1).copied().unwrap_or_default() } else { C::new(0.0, 0.0) }
    }

    /// Audio taper (log) pot: fraction of resistance at a knob position, 10 % at half-way.
    pub fn audio_taper(pos: f64) -> f64 {
        let pos = cclamp(pos, 0.0, 1.0);
        (10f64.powf(2.0 * pos) - 1.0) / 99.0 // 0 -> 0, 0.5 -> 0.091, 1 -> 1
    }

    fn rebuild(&mut self) {
        let n = self.n_nodes - 1;
        if self.g.len() != n * n || self.inv.len() != n * n {
            return; // (not prepared: `prepare` sizes the storage)
        }
        let g = &mut self.g;
        g.iter_mut().for_each(|x| *x = 0.0);
        let mut add = |a: usize, b: usize, y: f64| {
            if a > 0 {
                g[(a - 1) * n + (a - 1)] += y;
            }
            if b > 0 {
                g[(b - 1) * n + (b - 1)] += y;
            }
            if a > 0 && b > 0 {
                g[(a - 1) * n + (b - 1)] -= y;
                g[(b - 1) * n + (a - 1)] -= y;
            }
        };
        for r in &self.res {
            add(r.a, r.b, 1.0 / r.r);
        }
        for c in &self.caps {
            add(c.a, c.b, 2.0 * c.c * self.fs);
        }
        for s in &self.srcs {
            add(s.node, Self::GROUND, 1.0 / s.r);
        }
        // a whisper of conductance to ground on every node, so the matrix cannot go singular
        for i in 0..n {
            g[i * n + i] += 1.0e-12;
        }

        let inv = &mut self.inv;
        inv.iter_mut().for_each(|x| *x = 0.0);
        for i in 0..n {
            inv[i * n + i] = 1.0;
        }
        // Gauss-Jordan with partial pivoting
        for col in 0..n {
            let mut piv = col;
            for r in col + 1..n {
                if g[r * n + col].abs() > g[piv * n + col].abs() {
                    piv = r;
                }
            }
            if piv != col {
                for k in 0..n {
                    g.swap(col * n + k, piv * n + k);
                    inv.swap(col * n + k, piv * n + k);
                }
            }
            let d = g[col * n + col];
            for k in 0..n {
                g[col * n + k] /= d;
                inv[col * n + k] /= d;
            }
            for r in 0..n {
                if r == col {
                    continue;
                }
                let f = g[r * n + col];
                if f == 0.0 {
                    continue;
                }
                for k in 0..n {
                    g[r * n + k] -= f * g[col * n + k];
                    inv[r * n + k] -= f * inv[col * n + k];
                }
            }
        }
        self.rhs.iter_mut().for_each(|x| *x = 0.0);
        self.dirty = false;
    }
}

fn solve_complex(a: &mut [Complex<f64>], b: &mut [Complex<f64>], n: usize) {
    for col in 0..n {
        let mut piv = col;
        for r in col + 1..n {
            if a[r * n + col].norm() > a[piv * n + col].norm() {
                piv = r;
            }
        }
        if piv != col {
            for k in 0..n {
                a.swap(col * n + k, piv * n + k);
            }
            b.swap(col, piv);
        }
        for r in col + 1..n {
            let f = a[r * n + col] / a[col * n + col];
            for k in col..n {
                let t = a[col * n + k];
                a[r * n + k] -= f * t;
            }
            let t = b[col];
            b[r] -= f * t;
        }
    }
    for r in (0..n).rev() {
        let mut s = b[r];
        for k in r + 1..n {
            s -= a[r * n + k] * b[k];
        }
        b[r] = s / a[r * n + r];
    }
}

// ------------------------------------------------------------------ the tone stack
/// The Fender / Marshall "TMB" tone stack. Part values per amp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneStackParts {
    pub c_treble: f64,
    pub c_bass: f64,
    pub c_mid: f64,
    pub r_slope: f64,
    pub pot_treble: f64,
    pub pot_bass: f64,
    pub pot_mid: f64,
    /// Fender's Treble is a 250K AUDIO pot; Marshall's tone pots are linear.
    pub treble_audio: bool,
}

/// 1977 Fender Twin Reverb, 135 W: 250 pF, .1 uF, .047 uF, 100k, 250k/250k/10k.
pub const K_TWIN_STACK: ToneStackParts = ToneStackParts {
    c_treble: 250e-12,
    c_bass: 0.1e-6,
    c_mid: 0.047e-6,
    r_slope: 100e3,
    pot_treble: 250e3,
    pot_bass: 250e3,
    pot_mid: 10e3,
    treble_audio: true,
};
/// 1965 Fender Deluxe Reverb (AB763): no Middle pot, a fixed 6.8k (a 6.8k "pot" always at full).
pub const K_DELUXE_STACK: ToneStackParts = ToneStackParts {
    c_treble: 250e-12,
    c_bass: 0.1e-6,
    c_mid: 0.047e-6,
    r_slope: 100e3,
    pot_treble: 250e3,
    pot_bass: 250e3,
    pot_mid: 6.8e3,
    treble_audio: true,
};
/// Marshall JTM45: 270 pF, .02 uF, .02 uF, 56k, 250k/1M/25k.
pub const K_MARSHALL_STACK: ToneStackParts = ToneStackParts {
    c_treble: 270e-12,
    c_bass: 0.02e-6,
    c_mid: 0.02e-6,
    r_slope: 56e3,
    pot_treble: 250e3,
    pot_bass: 1e6,
    pot_mid: 25e3,
    treble_audio: false,
};

/// The tone stack built into a network: driven at `n_in`, the treble wiper is the output.
#[derive(Debug, Clone, Copy)]
pub struct ToneStack {
    pub n_in: usize,
    pub n_top: usize,
    pub n_wiper: usize,
    pub n_treb_bot: usize,
    pub n_slope: usize,
    pub n_mid_top: usize,
    r_treb_up: usize,
    r_treb_down: usize,
    r_bass: usize,
    r_mid: usize,
    pub parts: ToneStackParts,
}

impl Default for ToneStack {
    fn default() -> Self {
        ToneStack {
            n_in: 0,
            n_top: 0,
            n_wiper: 0,
            n_treb_bot: 0,
            n_slope: 0,
            n_mid_top: 0,
            r_treb_up: usize::MAX,
            r_treb_down: usize::MAX,
            r_bass: usize::MAX,
            r_mid: usize::MAX,
            parts: K_TWIN_STACK,
        }
    }
}

impl ToneStack {
    pub fn build(&mut self, net: &mut Network, input: usize, p: &ToneStackParts) {
        self.parts = *p;
        self.n_in = input;
        self.n_top = net.add_node();
        self.n_wiper = net.add_node();
        self.n_treb_bot = net.add_node();
        self.n_slope = net.add_node();
        self.n_mid_top = net.add_node();
        net.add_capacitor(self.n_in, self.n_top, p.c_treble); // treble cap to the top of the treble pot
        self.r_treb_up = net.add_resistor(self.n_top, self.n_wiper, p.pot_treble * 0.5);
        self.r_treb_down = net.add_resistor(self.n_wiper, self.n_treb_bot, p.pot_treble * 0.5);
        net.add_resistor(self.n_in, self.n_slope, p.r_slope); // slope resistor
        net.add_capacitor(self.n_slope, self.n_treb_bot, p.c_bass); // bass cap
        net.add_capacitor(self.n_slope, self.n_mid_top, p.c_mid); // mid cap
        self.r_bass = net.add_resistor(self.n_treb_bot, self.n_mid_top, p.pot_bass * 0.5);
        self.r_mid = net.add_resistor(self.n_mid_top, Network::GROUND, p.pot_mid * 0.5);
    }

    pub fn set_knobs(&self, net: &mut Network, treble: f64, bass: f64, middle: f64) {
        let mut treble = cclamp(treble, 0.0, 1.0);
        if self.parts.treble_audio {
            treble = Network::audio_taper(treble);
        }
        // a real pot never quite reaches 0 ohms at its ends
        net.set_resistor(self.r_treb_up, cmax(50.0, self.parts.pot_treble * (1.0 - treble)));
        net.set_resistor(self.r_treb_down, cmax(50.0, self.parts.pot_treble * treble));
        net.set_resistor(self.r_bass, cmax(50.0, self.parts.pot_bass * Network::audio_taper(bass)));
        net.set_resistor(self.r_mid, cmax(50.0, self.parts.pot_mid * cclamp(middle, 0.0, 1.0)));
    }
}
