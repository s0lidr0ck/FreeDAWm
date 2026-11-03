//! `AmpExtras.h`: the cathode follower (the Marshall's V2B and the AC30's Top Boost follower),
//! the Twin's spring tank, its optical "vibrato", and the Marshall's tremolo. Translated line by
//! line.

use super::amp_tube::{K12AX7, TriodeParams, cclamp, cmax, cmin, shared_table, triode_ip};
use std::sync::Arc;

// ------------------------------------------------------------------ the cathode follower
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FollowerParts {
    pub tube: TriodeParams,
    /// Plate supply.
    pub supply: f64,
    /// Cathode resistor to ground.
    pub rk: f64,
    /// What drives its grid: the stage in front's plate resistor.
    pub r_source: f64,
}

impl Default for FollowerParts {
    fn default() -> Self {
        FollowerParts { tube: K12AX7, supply: 310.0, rk: 100e3, r_source: 100e3 }
    }
}

const CF_N: usize = 4096;
const CF_LO: f64 = 0.0;
const CF_HI: f64 = 400.0;

#[derive(Debug, Clone, Default)]
pub struct CathodeFollower {
    parts: FollowerParts,
    table: Arc<Vec<f64>>,
}

impl CathodeFollower {
    pub fn prepare(&mut self, p: FollowerParts) {
        self.parts = p;
        // vk as a function of vg, solved once: ip = law(vg - vk, supply - vk), vk = ip rk
        let mut key = p.tube.key().to_vec();
        key.extend([p.supply.to_bits(), p.rk.to_bits(), 0x4346]);
        self.table = shared_table(&key, || {
            let mut table = vec![0.0; CF_N];
            for (i, slot) in table.iter_mut().enumerate() {
                let vg = CF_LO + (CF_HI - CF_LO) * i as f64 / (CF_N - 1) as f64;
                let mut a = 0.0;
                let mut b = vg + 5.0; // vk sits a volt or two above... bracket it
                b = cmax(b, 1.0);
                for _ in 0..50 {
                    let vk = 0.5 * (a + b);
                    let ip = triode_ip(&p.tube, vg - vk, p.supply - vk);
                    if ip * p.rk > vk {
                        a = vk;
                    } else {
                        b = vk;
                    }
                }
                *slot = 0.5 * (a + b);
            }
            table
        });
    }

    /// Grid voltage (DC included) in, cathode voltage (DC included) out.
    #[inline]
    pub fn process(&self, vg: f64) -> f64 {
        let mut vk = self.look(vg);
        // grid conduction: the one-sided clip
        if vg - vk > self.parts.tube.vg_on {
            let a = self.parts.tube.rgk / (self.parts.tube.rgk + self.parts.r_source);
            let vg_eff = vk + self.parts.tube.vg_on + (vg - vk - self.parts.tube.vg_on) * a;
            vk = self.look(vg_eff);
        }
        vk
    }

    /// The output impedance it drives the tone stack from, ~1/gm.
    pub fn output_resistance(&self) -> f64 {
        1.0 / 0.0016
    }

    #[inline]
    fn look(&self, vg: f64) -> f64 {
        let x = cclamp((vg - CF_LO) / (CF_HI - CF_LO) * (CF_N - 1) as f64, 0.0, CF_N as f64 - 1.001);
        let i = x as usize;
        let t = &self.table;
        let (Some(&t0), Some(&t1)) = (t.get(i), t.get(i + 1)) else { return 0.0 };
        t0 + (t1 - t0) * (x - i as f64)
    }
}

// ------------------------------------------------------------------ the spring tank
#[derive(Debug, Clone, Default)]
struct Ap {
    buf: Vec<f64>,
    idx: usize,
}

#[derive(Debug, Clone)]
struct Spring {
    buf: Vec<f64>,
    len: usize,
    pos: usize,
    stretch: usize,
    g: f64,
    lp: f64,
    hp: f64,
    ap: [Ap; 14],
}

impl Default for Spring {
    fn default() -> Self {
        Spring { buf: Vec::new(), len: 1, pos: 0, stretch: 1, g: 0.5, lp: 0.0, hp: 0.0, ap: Default::default() }
    }
}

/// The Twin's reverb pan: two springs as delay loops with dispersive allpass chains.
#[derive(Debug, Clone)]
pub struct SpringTank {
    springs: [Spring; 2],
    fs: f64,
    lp_a: f64,
    hp_a: f64,
}

impl Default for SpringTank {
    fn default() -> Self {
        SpringTank { springs: Default::default(), fs: 48_000.0, lp_a: 0.0, hp_a: 0.0 }
    }
}

impl SpringTank {
    /// (Rust, tests) Address and capacity of every heap buffer this owns, to check that
    /// processing and resets never reallocate.
    #[doc(hidden)]
    pub fn heap_blocks(&self, out: &mut Vec<usize>) {
        for s in &self.springs {
            super::amp_network::heap_vec(out, &s.buf);
            for a in &s.ap {
                super::amp_network::heap_vec(out, &a.buf);
            }
        }
    }

    pub fn prepare(&mut self, sample_rate: f64, decay_seconds: f64) {
        self.fs = sample_rate;
        // two springs of slightly different length, as in a 4AB3C1B
        let lens_ms = [33.0, 41.5];
        for (sp, ms) in self.springs.iter_mut().zip(lens_ms) {
            sp.len = ((ms * 0.001 * self.fs).round() as usize).max(1);
            sp.buf = vec![0.0; sp.len + 8];
            sp.pos = 0;
            // the loop's filters take their own share of each pass
            sp.g = 10f64.powf(-3.0 * (ms * 0.001) / (decay_seconds * 1.55));
            for ap in &mut sp.ap {
                *ap = Ap::default();
            }
            sp.lp = 0.0;
            sp.hp = 0.0;
            // the dispersion allpasses: a stretch that keeps the chirp's timing at any rate
            sp.stretch = ((self.fs / 48000.0).round() as usize).max(1);
            for ap in &mut sp.ap {
                ap.buf = vec![0.0; sp.stretch];
            }
        }
        self.lp_a = (-2.0 * 3.14159265358979 * 4300.0 / self.fs).exp(); // the tank's top end
        self.hp_a = (-2.0 * 3.14159265358979 * 110.0 / self.fs).exp(); // and its bottom
    }

    /// (Rust) As `prepare` leaves it, in place: empty springs, the read heads at the start.
    pub fn reset_fresh(&mut self) {
        for sp in &mut self.springs {
            sp.buf.iter_mut().for_each(|x| *x = 0.0);
            sp.pos = 0;
            sp.lp = 0.0;
            sp.hp = 0.0;
            for ap in &mut sp.ap {
                ap.buf.iter_mut().for_each(|x| *x = 0.0);
                ap.idx = 0;
            }
        }
    }

    pub fn process(&mut self, x: f64) -> f64 {
        let mut out = 0.0;
        let (lp_a, hp_a) = (self.lp_a, self.hp_a);
        for sp in &mut self.springs {
            // read the loop, disperse it, filter it, feed it back with the new input
            let mut y = sp.buf.get(sp.pos).copied().unwrap_or(0.0);
            for ap in &mut sp.ap {
                // stretched first-order allpass: H(z) = (a + z^-K) / (1 + a z^-K)
                let a = 0.62;
                let k = ap.idx;
                let delayed = ap.buf.get(k).copied().unwrap_or(0.0);
                let v = y - a * delayed;
                let o = a * v + delayed;
                if let Some(slot) = ap.buf.get_mut(k) {
                    *slot = v;
                }
                ap.idx = (ap.idx + 1) % ap.buf.len().max(1);
                y = o;
            }
            sp.lp = y + (sp.lp - y) * lp_a;
            sp.hp = sp.lp + (sp.hp - sp.lp) * hp_a;
            let filtered = sp.lp - sp.hp;
            if let Some(slot) = sp.buf.get_mut(sp.pos) {
                *slot = x + filtered * sp.g;
            }
            sp.pos = (sp.pos + 1) % sp.len;
            out += filtered;
        }
        0.5 * out
    }
}

// ------------------------------------------------------------------ the Twin's vibrato
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpticalVibrato {
    fs: f64,
    phase: f64,
    light: f64,
    k_attack: f64,
    k_decay: f64,
}

impl Default for OpticalVibrato {
    fn default() -> Self {
        OpticalVibrato { fs: 48_000.0, phase: 0.0, light: 0.0, k_attack: 0.0, k_decay: 0.0 }
    }
}

impl OpticalVibrato {
    pub fn prepare(&mut self, sample_rate: f64) {
        self.fs = sample_rate;
        self.phase = 0.0;
        self.light = 0.0;
        self.k_attack = 1.0 - (-1.0 / (0.004 * self.fs)).exp();
        self.k_decay = 1.0 - (-1.0 / (0.045 * self.fs)).exp();
    }

    pub fn reset_fresh(&mut self) {
        self.phase = 0.0;
        self.light = 0.0;
    }

    /// speed 0..1 (~2 .. 10 Hz), intensity 0..1. Returns the gain for this sample. `r_source` is
    /// the resistance feeding the node the LDR shunts (the C++ default argument is 100k).
    pub fn gain(&mut self, speed: f64, intensity: f64, r_source: f64) -> f64 {
        let hz = 2.0 * 5f64.powf(cclamp(speed, 0.0, 1.0));
        self.phase += hz / self.fs;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let osc = (2.0 * 3.14159265358979 * self.phase).sin();

        // the neon strikes only when the oscillator's swing clears its striking voltage
        let drive = cclamp(intensity, 0.0, 1.0) * (0.5 + 0.5 * osc);
        let neon = cmax(0.0, (drive - 0.35) / 0.65);

        // the LDR: quick to respond to light, slow to recover in the dark
        self.light += (neon - self.light) * if neon > self.light { self.k_attack } else { self.k_decay };

        // lit, a few kilohms; dark, megohms
        let r_ldr = cmin(10e6, 8e3 * cmax(self.light, 1e-4).powf(-1.2));
        r_ldr / (r_source + r_ldr)
    }

    pub fn rate_hz(&self, speed: f64) -> f64 {
        2.0 * 5f64.powf(cclamp(speed, 0.0, 1.0))
    }
}

// ------------------------------------------------------------------ the Marshall's tremolo
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiasTremolo {
    fs: f64,
    phase: f64,
}

impl Default for BiasTremolo {
    fn default() -> Self {
        BiasTremolo { fs: 48_000.0, phase: 0.0 }
    }
}

impl BiasTremolo {
    pub fn prepare(&mut self, sample_rate: f64) {
        self.fs = sample_rate;
        self.phase = 0.0;
    }

    pub fn reset_fresh(&mut self) {
        self.phase = 0.0;
    }

    /// speed 0..1 (~2 .. 9 Hz), intensity 0..1. Returns the gain for this sample.
    pub fn gain(&mut self, speed: f64, intensity: f64) -> f64 {
        let hz = 2.0 * 4.5f64.powf(cclamp(speed, 0.0, 1.0));
        self.phase += hz / self.fs;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        // a phase-shift oscillator's output is close to a sine, slightly flattened
        let osc = (1.6 * (2.0 * 3.14159265358979 * self.phase).sin()).tanh() / 1.6f64.tanh();
        let depth = 0.85 * cclamp(intensity, 0.0, 1.0);
        1.0 - depth * (0.5 + 0.5 * osc)
    }
}
