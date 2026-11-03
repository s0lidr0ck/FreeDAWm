//! `AmpTube.h`: vacuum tubes as circuit elements. Koren's triode law with grid current, and a
//! common-cathode stage solved every sample (plate, cathode and bypass cap together, Newton on the
//! cathode voltage, from a precomputed plate-current table plus one exact refinement).
//!
//! Translated line by line. `std::max`/`std::min`/`std::clamp` keep their C++ meaning (including
//! with NaN) through [`cmax`], [`cmin`] and [`cclamp`].

use std::sync::{Arc, Mutex, PoisonError};

/// `std::max(a, b)`: `(a < b) ? b : a`.
#[inline]
pub fn cmax(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

/// `std::min(a, b)`: `(b < a) ? b : a`.
#[inline]
pub fn cmin(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

/// `std::clamp(v, lo, hi)`: `(v < lo) ? lo : (hi < v) ? hi : v` (NaN passes through, as in C++).
#[inline]
pub fn cclamp(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo {
        lo
    } else if hi < v {
        hi
    } else {
        v
    }
}

/// Tables the parts alone determine (a stage's plate-current grid, the phase inverter's
/// transfer, a cathode follower's curve) are computed once per set of parts and shared by every
/// instance: exactly the values the C++ computes in each `prepare`, without recomputing them for
/// every instance, both oversampling factors and every re-prepare. `prepare` only (allocates).
pub(crate) fn shared_table<T: Send + Sync + 'static>(key: &[u64], build: impl FnOnce() -> T) -> Arc<T> {
    type Entry = (Vec<u64>, Arc<dyn std::any::Any + Send + Sync>);
    static CACHE: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
    {
        let cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(found) = cache.iter().find(|(k, _)| k.as_slice() == key).and_then(|(_, v)| Arc::clone(v).downcast::<T>().ok()) {
            return found;
        }
    }
    // Built outside the lock (it can take a while); a racing builder makes the same values.
    let made = Arc::new(build());
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    cache.push((key.to_vec(), Arc::clone(&made) as Arc<dyn std::any::Any + Send + Sync>));
    made
}

/// Koren's triode parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriodeParams {
    pub mu: f64,
    pub ex: f64,
    pub kg1: f64,
    pub kp: f64,
    pub kvb: f64,
    /// Grid-to-cathode resistance once the grid conducts.
    pub rgk: f64,
    /// Where grid conduction starts (V).
    pub vg_on: f64,
}

impl TriodeParams {
    pub(crate) fn key(&self) -> [u64; 7] {
        [self.mu, self.ex, self.kg1, self.kp, self.kvb, self.rgk, self.vg_on].map(f64::to_bits)
    }
}

/// Koren's published fits. 12AX7 (also 7025, ECC83).
pub const K12AX7: TriodeParams = TriodeParams { mu: 100.0, ex: 1.4, kg1: 1060.0, kp: 600.0, kvb: 300.0, rgk: 2000.0, vg_on: 0.2 };
/// 12AT7 (ECC81): kg1 scaled 460 -> 558 to match Fender's 135 W chart (4.41 mA at the Twin's reverb
/// driver operating point).
pub const K12AT7: TriodeParams = TriodeParams { mu: 60.0, ex: 1.35, kg1: 558.0, kp: 300.0, kvb: 300.0, rgk: 2000.0, vg_on: 0.2 };

/// Plate current (A) for grid-cathode and plate-cathode voltages.
#[inline]
pub fn triode_ip(t: &TriodeParams, vgk: f64, vpk: f64) -> f64 {
    if vpk <= 0.0 {
        return 0.0;
    }
    let inner = t.kp * (1.0 / t.mu + vgk / (t.kvb + vpk * vpk).sqrt());
    // ln(1 + e^x), computed so a large x cannot overflow
    let soft = if inner > 30.0 { inner } else { inner.exp().ln_1p() };
    let e1 = vpk / t.kp * soft;
    if e1 <= 0.0 {
        return 0.0;
    }
    2.0 * e1.powf(t.ex) / t.kg1
}

/// Grid current (A) for a grid-cathode voltage: a soft diode.
#[inline]
pub fn grid_current(t: &TriodeParams, vgk: f64) -> f64 {
    if vgk > t.vg_on { (vgk - t.vg_on) / t.rgk } else { 0.0 }
}

/// A common-cathode stage's parts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StageParts {
    pub tube: TriodeParams,
    /// What the plate resistor is fed from.
    pub supply: f64,
    /// Plate resistor.
    pub rp: f64,
    /// Cathode resistor.
    pub rk: f64,
    /// Cathode bypass cap; 0 = unbypassed.
    pub ck: f64,
    /// Grid stopper / source resistance into the grid.
    pub rg_stop: f64,
    /// >1 when two triodes share one cathode resistor (the Marshall's V1).
    pub cathode_share: f64,
}

impl Default for StageParts {
    fn default() -> Self {
        StageParts { tube: K12AX7, supply: 370.0, rp: 100e3, rk: 1500.0, ck: 25e-6, rg_stop: 68e3, cathode_share: 1.0 }
    }
}

// ---- the table: plate current over grid-cathode volts x cathode volts
const N_G: usize = 768;
const N_K: usize = 48;
const G_LO: f64 = -90.0;
const G_HI: f64 = 6.0;

/// A common-cathode triode stage: plate resistor to a supply, cathode resistor with an optional
/// bypass capacitor, a grid stopper in front of the grid. `process` takes the grid-side input
/// voltage (AC, around 0 V) and returns the plate voltage (DC included).
#[derive(Debug, Clone)]
pub struct TriodeStage {
    /// Off: every sample solves the plate exactly. On (the default): the table plus one exact
    /// Newton refinement.
    pub use_table: bool,
    parts: StageParts,
    fs: f64,
    vk_prev: f64,
    i_hist: f64,
    vp_dc: f64,
    vk_dc: f64,
    ip_dc: f64,
    k_hi: f64,
    table: Arc<Vec<f32>>,
}

impl Default for TriodeStage {
    fn default() -> Self {
        TriodeStage {
            use_table: true,
            parts: StageParts::default(),
            fs: 48_000.0,
            vk_prev: 0.0,
            i_hist: 0.0,
            vp_dc: 0.0,
            vk_dc: 0.0,
            ip_dc: 0.0,
            k_hi: 20.0,
            table: Arc::new(Vec::new()),
        }
    }
}

impl TriodeStage {
    pub fn prepare(&mut self, p: StageParts, sample_rate: f64) {
        self.parts = p;
        self.fs = sample_rate;
        self.build_table();
        // settle at the DC operating point, with the grid at 0 V
        self.solve_dc();
    }

    /// The DC operating point (no signal): plate and cathode volts.
    pub fn dc_plate(&self) -> f64 {
        self.vp_dc
    }
    pub fn dc_cathode(&self) -> f64 {
        self.vk_dc
    }
    pub fn dc_current(&self) -> f64 {
        self.ip_dc
    }

    /// One sample: the grid input voltage in, the plate voltage out.
    pub fn process(&mut self, vin: f64) -> f64 {
        let p = self.parts;
        // the resistor itself: cathode_kcl already multiplies this triode's current by the share
        let rk_eff = p.rk;
        // cathode node: resistor to ground, and (if bypassed) a capacitor, trapezoidal
        let g_c = if p.ck > 0.0 { 2.0 * p.ck * self.fs } else { 0.0 };

        // Newton on vk, from last sample's value.
        let mut vk = self.vk_prev;
        for _ in 0..8 {
            let f = self.cathode_kcl(vin, vk, g_c, rk_eff);
            let h = 1.0e-4;
            let df = (self.cathode_kcl(vin, vk + h, g_c, rk_eff) - f) / h;
            if df.abs() < 1e-12 {
                break;
            }
            let step = f / df;
            vk -= cclamp(step, -5.0, 5.0);
            if step.abs() < 1e-5 {
                break; // ten microvolts at the cathode
            }
        }

        let (_vg, _ig, _ip, vp) = self.operating_at(vin, vk, true);

        // the bypass cap's history for the next sample (trapezoidal)
        if g_c > 0.0 {
            let i_cap = g_c * vk - self.i_hist;
            self.i_hist = g_c * vk + i_cap;
        }
        self.vk_prev = vk;
        vp
    }

    /// `reset()`: back to the DC operating point (what `prepare` leaves).
    pub fn reset(&mut self) {
        self.vk_prev = self.vk_dc;
        self.i_hist = (if self.parts.ck > 0.0 { 2.0 * self.parts.ck * self.fs } else { 0.0 }) * self.vk_dc;
    }

    /// Given the input and the cathode voltage, everything else in the stage: (vg, ig, ip, vp).
    /// `refine`: the final evaluation of a sample adds an exact Newton step to the table's answer.
    fn operating_at(&self, vin: f64, vk: f64, refine: bool) -> (f64, f64, f64, f64) {
        let p = &self.parts;
        // grid: if the grid conducts it pulls the grid voltage down toward the cathode
        let vgk_open = vin - vk;
        let vg = if vgk_open > p.tube.vg_on {
            let a = p.tube.rgk / (p.tube.rgk + p.rg_stop);
            vk + p.tube.vg_on + (vgk_open - p.tube.vg_on) * a
        } else {
            vin
        };
        let ig = grid_current(&p.tube, vg - vk);

        // plate: vp = supply - ip rp, and ip depends on vp -- a small inner solve
        let ip = if self.use_table && !self.table.is_empty() && !refine {
            self.lookup(vg - vk, vk)
        } else if self.use_table && !self.table.is_empty() {
            // the table's answer, then one exact Newton step on the plate equation
            let vgk = vg - vk;
            let mut ip = self.lookup(vgk, vk);
            let h = 1.0e-9;
            let f = ip - triode_ip(&p.tube, vgk, p.supply - ip * p.rp - vk);
            let f2 = (ip + h) - triode_ip(&p.tube, vgk, p.supply - (ip + h) * p.rp - vk);
            let df = (f2 - f) / h;
            if df > 1.0e-12 {
                ip = cmax(0.0, ip - f / df);
            }
            ip
        } else {
            self.plate_exact(vg - vk, vk)
        };
        let vp = p.supply - ip * p.rp;
        (vg, ig, ip, vp)
    }

    fn plate_exact(&self, vgk: f64, vk: f64) -> f64 {
        plate_exact(&self.parts, vgk, vk)
    }

    fn build_table(&mut self) {
        // the cathode can sit anywhere from 0 to a few times its idle
        self.k_hi = cmax(8.0, self.parts.supply * 0.06);
        let parts = self.parts;
        let k_hi = self.k_hi;
        let mut key = parts.tube.key().to_vec();
        key.extend([parts.supply.to_bits(), parts.rp.to_bits(), 0x0054_4142_4c45]);
        self.table = shared_table(&key, || {
            let mut table = vec![0.0f32; N_G * N_K];
            for k in 0..N_K {
                for g in 0..N_G {
                    let vgk = G_LO + (G_HI - G_LO) * g as f64 / (N_G - 1) as f64;
                    let vk = k_hi * k as f64 / (N_K - 1) as f64;
                    table[k * N_G + g] = plate_exact(&parts, vgk, vk) as f32;
                }
            }
            table
        });
    }

    #[inline]
    fn lookup(&self, vgk: f64, vk: f64) -> f64 {
        let gx = cclamp((vgk - G_LO) / (G_HI - G_LO) * (N_G - 1) as f64, 0.0, N_G as f64 - 1.001);
        let kx = cclamp(vk / self.k_hi * (N_K - 1) as f64, 0.0, N_K as f64 - 1.001);
        // (a NaN lands on cell 0 here, where the C++ cast is undefined)
        let g0 = gx as usize;
        let k0 = kx as usize;
        let fg = gx - g0 as f64;
        let fk = kx - k0 as f64;
        let i0 = k0 * N_G + g0;
        let t = &self.table;
        let (Some(&r00), Some(&r01), Some(&r10), Some(&r11)) = (t.get(i0), t.get(i0 + 1), t.get(i0 + N_G), t.get(i0 + N_G + 1)) else {
            return 0.0;
        };
        let a = f64::from(r00) + f64::from(r01 - r00) * fg;
        let b = f64::from(r10) + f64::from(r11 - r10) * fg;
        a + (b - a) * fk
    }

    fn cathode_kcl(&self, vin: f64, vk: f64, g_c: f64, rk_eff: f64) -> f64 {
        let (_vg, ig, ip, _vp) = self.operating_at(vin, vk, false);
        // the current into the cathode node must leave through the resistor and the capacitor
        let i_in = (ip + ig) * self.parts.cathode_share;
        let i_out = vk / rk_eff + if g_c > 0.0 { g_c * vk - self.i_hist } else { 0.0 };
        i_in - i_out
    }

    fn solve_dc(&mut self) {
        // at DC the bypass cap carries nothing: vk = ip * rk * share
        let mut vk = 1.0;
        for _ in 0..200 {
            let (_vg, _ig, ip, _vp) = self.operating_at(0.0, vk, false);
            let next = ip * self.parts.cathode_share * self.parts.rk;
            if (next - vk).abs() < 1e-9 {
                vk = next;
                break;
            }
            vk = 0.5 * (vk + next);
        }
        let (_vg, _ig, ip, vp) = self.operating_at(0.0, vk, false);
        self.ip_dc = ip;
        self.vp_dc = vp;
        self.vk_dc = vk;
        self.vk_prev = vk;
        self.i_hist = (if self.parts.ck > 0.0 { 2.0 * self.parts.ck * self.fs } else { 0.0 }) * vk;
    }
}

/// `plateExact`: 40-step bisection of ip = law(vgk, supply - ip rp - vk).
fn plate_exact(p: &StageParts, vgk: f64, vk: f64) -> f64 {
    let mut lo = 0.0;
    let mut hi = p.supply / p.rp; // bracketing ip
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        let vpm = p.supply - mid * p.rp;
        let law = triode_ip(&p.tube, vgk, vpm - vk);
        if law > mid {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}
