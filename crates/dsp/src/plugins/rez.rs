//! Rez: a dynamic resonance suppressor.
//!
//! Rez listens for frequencies that stick out of the sound's own spectrum and turns down only
//! those, only while they stick out. Nothing is cut by a fixed curve: the reference is the signal
//! itself, so it behaves the same on quiet and loud material.
//!
//! Per frame (an STFT: 2048 points up to 50 kHz, a new frame every N/8 samples, root-Hann windows
//! on the way in and out):
//!
//! 1. The power spectrum, summed over the channels (or taken from the sidechain key), followed in
//!    time with Attack.
//! 2. FINE: that power averaged over a narrow band around each bin (Sharpness sets the width), in
//!    dB.
//! 3. ENVELOPE: FINE averaged, in dB, over a wide band (Selectivity sets the width). This is what
//!    the sound roughly looks like without its resonances.
//! 4. EXCESS = FINE - ENVELOPE. Where it is above the threshold (Selectivity), the part above is
//!    a cut request, scaled by Depth, weighted by the Low/High range and limited to [`MAX_CUT_DB`].
//! 5. Each bin's cut rises at once and falls back with Release.
//! 6. The bins are scaled and the frame is resynthesised by overlap-add.
//!
//! Every channel gets the same cut, so the stereo image does not move.
//!
//! Latency: one frame (N samples), always reported. Delta plays what is being removed. Every
//! buffer is allocated in `prepare`; `process` and `set_param` never allocate.

use crate::params::{Params, choice, lin, log, toggle};
use crate::util::{frames_in, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// The deepest cut any one frequency gets, in dB.
pub const MAX_CUT_DB: f32 = 30.0;
/// Frames per frame length (the overlap).
const OVERLAP: usize = 8;
/// The knee of the Soft mode, in dB.
const SOFT_KNEE_DB: f32 = 6.0;
/// How much harder the Hard mode cuts than Soft at the same Depth.
const HARD_SLOPE: f32 = 1.5;
/// The narrowest and widest FINE band (Sharpness 100 % and 0 %), in octaves.
const FINE_OCT: (f32, f32) = (1.0 / 24.0, 1.0 / 3.0);
/// The narrowest and widest ENVELOPE band (Selectivity 100 % and 0 %), in octaves.
const ENV_OCT: (f32, f32) = (0.75, 2.5);
/// The lowest and highest threshold (Selectivity 0 % and 100 %), in dB of excess.
const THRESHOLD_DB: (f32, f32) = (1.0, 7.0);
/// FINE never averages fewer than this many bins each side (the low end, where a fraction of an
/// octave is less than a bin).
const MIN_HALF_BINS: f32 = 1.5;
/// Power below this is silence (-200 dB): keeps the logarithm finite.
const POWER_FLOOR: f32 = 1.0e-20;

const P_DEPTH: usize = 0;
const P_SHARPNESS: usize = 1;
const P_SELECTIVITY: usize = 2;
const P_ATTACK: usize = 3;
const P_RELEASE: usize = 4;
const P_LOW: usize = 5;
const P_HIGH: usize = 6;
const P_MODE: usize = 7;
const P_MIX: usize = 8;
const P_DELTA: usize = 9;

static REZ_PARAMS: [ParamInfo; 10] = [
    lin("depth", "Depth", 0.0, 100.0, 50.0, Unit::Percent),
    lin("sharpness", "Sharpness", 0.0, 100.0, 50.0, Unit::Percent),
    lin("selectivity", "Selectivity", 0.0, 100.0, 50.0, Unit::Percent),
    log("attack", "Attack", 1.0, 200.0, 15.0, Unit::Ms),
    log("release", "Release", 5.0, 1000.0, 80.0, Unit::Ms),
    log("low", "Low", 20.0, 2000.0, 60.0, Unit::Hz),
    log("high", "High", 1000.0, 20000.0, 20000.0, Unit::Hz),
    choice("mode", "Mode", &["Soft", "Hard"], 0),
    lin("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
    toggle("delta", "Delta", false),
];

pub static REZ_INFO: PluginInfo = PluginInfo {
    id: "rez",
    name: "Rez",
    short_name: "Rez",
    category: Category::Dynamics,
    params: &REZ_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: true,
    link: None,
    references: 0,
    midi_out: false,
};

/// The frame length for a sample rate: 2048 up to 50 kHz, doubling with the rate.
pub fn frame_size_for(sr: f32) -> usize {
    let mut n = 2048usize;
    let mut limit = 50_000.0f32;
    while sr > limit && n < 16_384 {
        n *= 2;
        limit *= 2.0;
    }
    n
}

/// The cut for an excess of `over` dB above the threshold: `slope` dB per dB, eased in over
/// `knee` dB centred on the threshold.
#[inline]
fn cut_for(over: f32, slope: f32, knee: f32) -> f32 {
    if knee > 1.0e-3 && 2.0 * over.abs() <= knee {
        let t = over + knee * 0.5;
        slope * t * t / (2.0 * knee)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

/// `a + (b - a) * t` on a log scale (both positive).
#[inline]
fn log_lerp(a: f32, b: f32, t: f32) -> f32 {
    a * (b / a).powf(t)
}

/// The prefix sum `cum` (`cum[i]` = the sum of the first `i` values) read at a fractional
/// position, each value taken to fill one unit.
#[inline]
fn cum_at(cum: &[f64], x: f32) -> f64 {
    let last = cum.len().saturating_sub(1);
    let x = x.clamp(0.0, last as f32);
    let i = (x as usize).min(last);
    let frac = f64::from(x - i as f32);
    let a = cum.get(i).copied().unwrap_or(0.0);
    let b = cum.get(i + 1).copied().unwrap_or(a);
    a + (b - a) * frac
}

/// The mean of the values behind `cum` over the bins `lo..=hi` (fractional, each bin `k` taken
/// to span `k - 0.5 .. k + 0.5`).
#[inline]
fn band_mean(cum: &[f64], lo: f32, hi: f32) -> f32 {
    let a = lo + 0.5;
    let b = hi + 0.5;
    let width = f64::from((b - a).max(1.0e-3));
    ((cum_at(cum, b) - cum_at(cum, a)) / width) as f32
}

/// What `prepare` builds for one sample rate and channel count.
struct Engine {
    n: usize,
    hop: usize,
    /// Bins from DC to Nyquist: `n / 2 + 1`.
    bins: usize,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    /// One spectrum per channel.
    spec: Vec<Vec<Complex<f32>>>,
    /// The key's spectrum (one channel at a time).
    key_spec: Vec<Complex<f32>>,
    /// The root-Hann window, and the same scaled for the way out (the inverse transform's `n` and
    /// the overlap's sum folded in).
    window: Vec<f32>,
    window_out: Vec<f32>,
    /// The last `n` input samples per channel, the key's likewise, and the overlap-add
    /// accumulator: rings read and written at `pos`.
    input: Vec<Vec<f32>>,
    key: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    pos: usize,
    /// Samples since the last frame.
    since: usize,
    /// Whether the key ring holds a key (the last block had one).
    keyed: bool,
    // Per bin:
    power: Vec<f32>,
    /// `power` followed with Attack.
    detect: Vec<f32>,
    cum: Vec<f64>,
    fine_db: Vec<f32>,
    env_db: Vec<f32>,
    /// The cut in force, in dB (>= 0).
    cut: Vec<f32>,
    gain: Vec<f32>,
    /// The FINE and ENVELOPE bands' edges (in bins) and the Low/High weight.
    fine_lo: Vec<f32>,
    fine_hi: Vec<f32>,
    env_lo: Vec<f32>,
    env_hi: Vec<f32>,
    weight: Vec<f32>,
}

impl Engine {
    fn new(sr: f32, channels: usize) -> Self {
        let n = frame_size_for(sr);
        let hop = n / OVERLAP;
        let bins = n / 2 + 1;
        let mut planner = FftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(n);
        let inverse = planner.plan_fft_inverse(n);
        let scratch_len = forward.get_inplace_scratch_len().max(inverse.get_inplace_scratch_len());
        let zero = Complex::new(0.0f32, 0.0);
        let window: Vec<f32> = (0..n).map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos()).sqrt() as f32).collect();
        // window * window is a Hann, which sums to OVERLAP / 2 over the overlapping frames.
        let scale = 1.0 / (n as f32 * OVERLAP as f32 * 0.5);
        let window_out = window.iter().map(|w| w * scale).collect();
        Engine {
            n,
            hop,
            bins,
            forward,
            inverse,
            scratch: vec![zero; scratch_len],
            spec: vec![vec![zero; n]; channels],
            key_spec: vec![zero; n],
            window,
            window_out,
            input: vec![vec![0.0; n]; channels],
            key: vec![vec![0.0; n]; channels],
            output: vec![vec![0.0; n]; channels],
            pos: 0,
            since: 0,
            keyed: false,
            power: vec![0.0; bins],
            detect: vec![0.0; bins],
            cum: vec![0.0; bins + 1],
            fine_db: vec![0.0; bins],
            env_db: vec![0.0; bins],
            cut: vec![0.0; bins],
            gain: vec![1.0; bins],
            fine_lo: vec![0.0; bins],
            fine_hi: vec![0.0; bins],
            env_lo: vec![0.0; bins],
            env_hi: vec![0.0; bins],
            weight: vec![0.0; bins],
        }
    }

    fn reset(&mut self) {
        for ring in self.input.iter_mut().chain(self.key.iter_mut()).chain(self.output.iter_mut()) {
            ring.fill(0.0);
        }
        self.pos = 0;
        self.since = 0;
        self.keyed = false;
        self.detect.fill(0.0);
        self.cut.fill(0.0);
        self.gain.fill(1.0);
    }
}

/// What the parameters come to, per frame.
#[derive(Debug, Clone, Copy, Default)]
struct Settings {
    /// dB of cut per dB of excess above the threshold.
    slope: f32,
    knee: f32,
    threshold: f32,
    /// One-pole coefficients per frame.
    attack: f32,
    release: f32,
    mix: f32,
    delta: bool,
}

/// The resonance suppressor.
///
/// `gain_reduction_db` is the deepest cut at any frequency during the last block.
pub struct Rez {
    p: Params,
    sr: f32,
    ch: usize,
    engine: Option<Box<Engine>>,
    s: Settings,
    gr_meter: f32,
}

impl Default for Rez {
    fn default() -> Self {
        Self::new()
    }
}

impl Rez {
    pub fn new() -> Self {
        let mut r = Rez { p: Params::new(&REZ_PARAMS), sr: 48_000.0, ch: 0, engine: None, s: Settings::default(), gr_meter: 0.0 };
        r.update();
        r
    }

    /// The cut in force at each bin from DC to Nyquist, in dB (empty before `prepare`).
    pub fn cut_db(&self) -> &[f32] {
        self.engine.as_ref().map_or(&[], |e| e.cut.as_slice())
    }

    /// The frequency of bin `k` of [`Rez::cut_db`], in Hz.
    pub fn bin_hz(&self, k: usize) -> f32 {
        self.engine.as_ref().map_or(0.0, |e| k as f32 * self.sr / e.n as f32)
    }

    /// Works the parameters out into [`Settings`] and the per-bin tables. No allocation.
    fn update(&mut self) {
        let sharp = self.p.v(P_SHARPNESS) / 100.0;
        let select = self.p.v(P_SELECTIVITY) / 100.0;
        let hard = self.p.choice(P_MODE) == 1;
        self.s.slope = self.p.v(P_DEPTH) / 100.0 * if hard { HARD_SLOPE } else { 1.0 };
        self.s.knee = if hard { 0.0 } else { SOFT_KNEE_DB };
        self.s.threshold = THRESHOLD_DB.0 + (THRESHOLD_DB.1 - THRESHOLD_DB.0) * select;
        self.s.mix = self.p.v(P_MIX) / 100.0;
        self.s.delta = self.p.on(P_DELTA);
        let Some(e) = self.engine.as_mut() else { return };
        let frame_ms = 1000.0 * e.hop as f32 / self.sr;
        let coef = |ms: f32| (-frame_ms / ms.max(0.01)).exp();
        self.s.attack = coef(self.p.v(P_ATTACK));
        self.s.release = coef(self.p.v(P_RELEASE));

        // Half a band, as a frequency ratio.
        let fine = 2.0f32.powf(0.5 * log_lerp(FINE_OCT.1, FINE_OCT.0, sharp));
        let env = 2.0f32.powf(0.5 * log_lerp(ENV_OCT.1, ENV_OCT.0, select));
        let top = (e.bins - 1) as f32;
        let hz_per_bin = self.sr / e.n as f32;
        let low = self.p.v(P_LOW);
        let high = self.p.v(P_HIGH).max(low);
        let edges = e.fine_lo.iter_mut().zip(e.fine_hi.iter_mut()).zip(e.env_lo.iter_mut().zip(e.env_hi.iter_mut()));
        for (k, (((fine_lo, fine_hi), (env_lo, env_hi)), weight)) in edges.zip(e.weight.iter_mut()).enumerate() {
            let kf = k as f32;
            *fine_lo = (kf / fine).min(kf - MIN_HALF_BINS).max(-0.5);
            *fine_hi = (kf * fine).max(kf + MIN_HALF_BINS).min(top + 0.5);
            // The envelope is at least three times as wide as the fine band, and never reaches
            // below bin 1 (DC is not part of the sound).
            let half = (kf - *fine_lo).max(*fine_hi - kf) * 3.0;
            *env_lo = (kf / env).min(kf - half).max(0.5);
            *env_hi = (kf * env).max(kf + half).min(top + 0.5);
            // The range: full weight between Low and High, fading out over half an octave outside.
            let hz = kf * hz_per_bin;
            let fade = |octaves_outside: f32| {
                let t = (1.0 - octaves_outside * 2.0).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            };
            *weight = if k == 0 || hz <= 0.0 {
                0.0
            } else if hz < low {
                fade((low / hz).log2())
            } else if hz > high {
                fade((hz / high).log2())
            } else {
                1.0
            };
        }
    }

    /// One frame: analyse, work out the cuts, resynthesise into the overlap-add ring.
    fn frame(e: &mut Engine, s: &Settings) -> f32 {
        let (n, bins, pos) = (e.n, e.bins, e.pos);
        // Analysis: each channel's last `n` samples, oldest first, windowed.
        e.power.fill(0.0);
        for (spec, ring) in e.spec.iter_mut().zip(&e.input) {
            let (newer, older) = ring.split_at(pos.min(ring.len()));
            for ((bin, x), w) in spec.iter_mut().zip(older.iter().chain(newer)).zip(&e.window) {
                *bin = Complex::new(x * w, 0.0);
            }
            e.forward.process_with_scratch(spec, &mut e.scratch);
            if !e.keyed {
                for (p, bin) in e.power.iter_mut().zip(spec.iter()) {
                    *p += bin.norm_sqr();
                }
            }
        }
        if e.keyed {
            for ring in &e.key {
                let (newer, older) = ring.split_at(pos.min(ring.len()));
                for ((bin, x), w) in e.key_spec.iter_mut().zip(older.iter().chain(newer)).zip(&e.window) {
                    *bin = Complex::new(x * w, 0.0);
                }
                e.forward.process_with_scratch(&mut e.key_spec, &mut e.scratch);
                for (p, bin) in e.power.iter_mut().zip(e.key_spec.iter()) {
                    *p += bin.norm_sqr();
                }
            }
        }

        // 1. The detector follows the power with Attack.
        let mut sum = 0.0f64;
        if let Some(first) = e.cum.first_mut() {
            *first = 0.0;
        }
        for ((d, p), c) in e.detect.iter_mut().zip(&e.power).zip(e.cum.iter_mut().skip(1)) {
            let v = *p + (*d - *p) * s.attack;
            *d = if v.is_finite() && v > POWER_FLOOR { v } else { 0.0 };
            sum += f64::from(*d);
            *c = sum;
        }
        // 2. FINE, in dB.
        for ((f, lo), hi) in e.fine_db.iter_mut().zip(&e.fine_lo).zip(&e.fine_hi) {
            *f = 10.0 * (band_mean(&e.cum, *lo, *hi).max(0.0) + POWER_FLOOR).log10();
        }
        // 3. ENVELOPE: FINE averaged in dB.
        let mut sum = 0.0f64;
        for (f, c) in e.fine_db.iter().zip(e.cum.iter_mut().skip(1)) {
            sum += f64::from(*f);
            *c = sum;
        }
        for ((v, lo), hi) in e.env_db.iter_mut().zip(&e.env_lo).zip(&e.env_hi) {
            *v = band_mean(&e.cum, *lo, *hi);
        }
        // 4 and 5. The cut request, and the cut that follows it.
        let mut deepest = 0.0f32;
        for (((cut, f), v), w) in e.cut.iter_mut().zip(&e.fine_db).zip(&e.env_db).zip(&e.weight) {
            let want = (cut_for(f - v - s.threshold, s.slope, s.knee) * w).clamp(0.0, MAX_CUT_DB);
            let now = if want >= *cut { want } else { want + (*cut - want) * s.release };
            *cut = if now.is_finite() && now > 1.0e-4 { now } else { 0.0 };
            deepest = deepest.max(*cut);
        }
        // The gain per bin: the cut, lightly smoothed across neighbours.
        let last = bins.saturating_sub(1);
        for (k, g) in e.gain.iter_mut().enumerate() {
            let at = |i: usize| e.cut.get(i).copied().unwrap_or(0.0);
            let c = 0.25 * at(k.saturating_sub(1)) + 0.5 * at(k) + 0.25 * at((k + 1).min(last));
            *g = if c > 1.0e-4 { (-c * (std::f32::consts::LN_10 / 20.0)).exp() } else { 1.0 };
        }

        // 6. Scale the bins (and their mirror images) and overlap-add the frame.
        for (spec, out) in e.spec.iter_mut().zip(e.output.iter_mut()) {
            for (k, g) in e.gain.iter().enumerate() {
                if let Some(bin) = spec.get_mut(k) {
                    *bin *= *g;
                }
                if k > 0
                    && k < n - k
                    && let Some(bin) = spec.get_mut(n - k)
                {
                    *bin *= *g;
                }
            }
            e.inverse.process_with_scratch(spec, &mut e.scratch);
            let (newer, older) = out.split_at_mut(pos.min(n));
            for ((o, bin), w) in older.iter_mut().chain(newer.iter_mut()).zip(spec.iter()).zip(&e.window_out) {
                *o += bin.re * w;
            }
        }
        deepest
    }

    fn run(&mut self, io: &mut [Vec<f32>], frames: usize, key: Option<&[Vec<f32>]>) {
        let s = self.s;
        let Some(e) = self.engine.as_mut() else { return };
        let ch = self.ch.min(io.len());
        if ch == 0 {
            return;
        }
        let frames = frames_in(io, frames, ch);
        // A key that does not cover the block is no key.
        let key = key.filter(|k| !k.is_empty() && k.iter().take(ch).all(|b| b.len() >= frames));
        e.keyed = key.is_some();
        let mut deepest = 0.0f32;
        for i in 0..frames {
            let pos = e.pos;
            for (c, buf) in io.iter_mut().take(ch).enumerate() {
                let (Some(x), Some(inp), Some(out)) =
                    (buf.get_mut(i), e.input.get_mut(c).and_then(|r| r.get_mut(pos)), e.output.get_mut(c).and_then(|r| r.get_mut(pos)))
                else {
                    continue;
                };
                // The slot holds the input of `n` samples ago and its finished overlap-add sum.
                let dry = *inp;
                let wet = std::mem::take(out);
                *inp = if x.is_finite() { *x } else { 0.0 };
                *x = if s.delta { (dry - wet) * s.mix } else { dry + (wet - dry) * s.mix };
                let k = key.and_then(|k| k.get(c)).and_then(|b| b.get(i)).copied().unwrap_or(0.0);
                if let Some(slot) = e.key.get_mut(c).and_then(|r| r.get_mut(pos)) {
                    *slot = if k.is_finite() { k } else { 0.0 };
                }
            }
            e.pos = if pos + 1 >= e.n { 0 } else { pos + 1 };
            e.since += 1;
            if e.since >= e.hop {
                e.since = 0;
                deepest = deepest.max(Self::frame(e, &s));
            }
        }
        if frames > 0 {
            self.gr_meter = if deepest > 0.0 { deepest } else { e.cut.iter().fold(0.0f32, |m, c| m.max(*c)) };
        }
    }
}

impl Plugin for Rez {
    crate::params::param_plumbing!(REZ_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.engine = Some(Box::new(Engine::new(self.sr, self.ch)));
        self.gr_meter = 0.0;
        self.update();
    }

    fn reset(&mut self) {
        if let Some(e) = self.engine.as_mut() {
            e.reset();
        }
        self.gr_meter = 0.0;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.run(io, frames, None);
    }

    fn process_with_sidechain(&mut self, io: &mut [Vec<f32>], frames: usize, key: Option<&[Vec<f32>]>) {
        self.run(io, frames, key);
    }

    fn latency(&self) -> usize {
        self.engine.as_ref().map_or(0, |e| e.n)
    }

    fn gain_reduction_db(&self) -> f32 {
        self.gr_meter
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// A small deterministic generator (xorshift), uniform in -1..1.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 >> 40) as f32 / (1u64 << 23) as f32) - 1.0
        }
    }

    /// Roughly pink noise (Paul Kellet's filter), about -24 dBFS RMS.
    fn pink(len: usize, seed: u64) -> Vec<f32> {
        let mut rng = Rng(seed);
        let (mut b0, mut b1, mut b2) = (0.0f32, 0.0f32, 0.0f32);
        (0..len)
            .map(|_| {
                let w = rng.next();
                b0 = 0.99765 * b0 + w * 0.099_046;
                b1 = 0.96300 * b1 + w * 0.296_516_4;
                b2 = 0.57000 * b2 + w * 1.052_691_3;
                (b0 + b1 + b2 + w * 0.1848) * 0.05
            })
            .collect()
    }

    fn sine(hz: f32, amp: f32, len: usize) -> Vec<f32> {
        (0..len).map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / SR).sin()).collect()
    }

    fn prepared() -> Rez {
        let mut r = Rez::new();
        r.prepare(SR, 512, 2);
        r
    }

    fn run(r: &mut Rez, x: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(x.len());
        for block in x.chunks(512) {
            let mut io = vec![block.to_vec(), block.to_vec()];
            r.process(&mut io, block.len());
            out.extend_from_slice(&io[0]);
        }
        out
    }

    /// The level of `x` at `hz` in dB (a single-bin Fourier sum over the whole slice).
    fn level_at(x: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, v) in x.iter().enumerate() {
            let ph = std::f64::consts::TAU * f64::from(hz) * i as f64 / f64::from(SR);
            re += f64::from(*v) * ph.cos();
            im += f64::from(*v) * ph.sin();
        }
        (20.0 * ((re * re + im * im).sqrt() * 2.0 / x.len() as f64).max(1e-12).log10()) as f32
    }

    fn rms_db(x: &[f32]) -> f32 {
        let ms = x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / x.len().max(1) as f64;
        (10.0 * ms.max(1e-24).log10()) as f32
    }

    #[test]
    fn depth_zero_is_the_input_delayed_by_the_latency() {
        let mut r = prepared();
        r.set_param("depth", 0.0);
        let lat = r.latency();
        assert_eq!(lat, 2048);
        let x: Vec<f32> = pink(20_000, 7).iter().zip(sine(1000.0, 0.3, 20_000)).map(|(a, b)| a + b).collect();
        let y = run(&mut r, &x);
        let err = x.iter().zip(&y[lat..]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 2.0e-5, "reconstruction error {err}");
        assert_eq!(r.gain_reduction_db(), 0.0);
    }

    #[test]
    fn plain_noise_is_left_nearly_alone() {
        let mut r = prepared();
        let x = pink(96_000, 11);
        let y = run(&mut r, &x);
        let lat = r.latency();
        let change = rms_db(&y[lat + 24_000..]) - rms_db(&x[24_000..96_000 - lat]);
        assert!(change.abs() < 0.5, "plain pink noise changed by {change} dB");
    }

    #[test]
    fn a_tone_in_noise_is_cut_where_it_is_and_nowhere_else() {
        let mut r = prepared();
        let len = 96_000;
        let tone = sine(2000.0, 0.1, len);
        let probe = sine(300.0, 0.001, len);
        let x: Vec<f32> = pink(len, 3).iter().zip(&tone).zip(&probe).map(|((n, t), p)| n * 0.2 + t + p).collect();
        let y = run(&mut r, &x);
        let lat = r.latency();
        let (a, b) = (&x[24_000..len - lat], &y[lat + 24_000..]);
        let cut = level_at(a, 2000.0) - level_at(b, 2000.0);
        assert!(cut > 6.0, "the 2 kHz tone was cut by only {cut} dB");
        let elsewhere = level_at(a, 300.0) - level_at(b, 300.0);
        assert!(elsewhere.abs() < 1.0, "300 Hz moved by {elsewhere} dB");
        assert!(r.gain_reduction_db() > 6.0);
        // The deepest cut is at the tone.
        let k = r.cut_db().iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |m| m.0);
        assert!((r.bin_hz(k) - 2000.0).abs() < 120.0, "deepest cut at {} Hz", r.bin_hz(k));
    }

    #[test]
    fn the_cut_does_not_depend_on_the_input_level() {
        let cut_at = |gain: f32| {
            let mut r = prepared();
            let len = 72_000;
            let x: Vec<f32> = pink(len, 5).iter().zip(sine(3000.0, 0.1, len)).map(|(n, t)| (n * 0.2 + t) * gain).collect();
            let y = run(&mut r, &x);
            let lat = r.latency();
            level_at(&x[24_000..len - lat], 3000.0) - level_at(&y[lat + 24_000..], 3000.0)
        };
        let (loud, quiet) = (cut_at(1.0), cut_at(0.01));
        assert!((loud - quiet).abs() < 0.2, "cut {loud} dB loud, {quiet} dB quiet");
    }

    #[test]
    fn delta_plus_the_output_is_the_delayed_input() {
        let len = 48_000;
        let x: Vec<f32> = pink(len, 9).iter().zip(sine(1500.0, 0.2, len)).map(|(n, t)| n * 0.2 + t).collect();
        let mut wet = prepared();
        let mut delta = prepared();
        delta.set_param("delta", 1.0);
        let (w, d) = (run(&mut wet, &x), run(&mut delta, &x));
        let lat = wet.latency();
        assert!(rms_db(&d[lat + 12_000..]) > -60.0, "delta is silent");
        let err = x.iter().zip(w[lat..].iter().zip(&d[lat..])).map(|(x, (w, d))| (x - (w + d)).abs()).fold(0.0f32, f32::max);
        assert!(err < 2.0e-5, "output + delta differs from the input by {err}");
    }

    #[test]
    fn the_cut_recovers_after_the_resonance_stops() {
        let mut r = prepared();
        r.set_param("release", 50.0);
        let x: Vec<f32> = pink(48_000, 13).iter().zip(sine(2000.0, 0.1, 48_000)).map(|(n, t)| n * 0.2 + t).collect();
        let _ = run(&mut r, &x);
        assert!(r.gain_reduction_db() > 6.0);
        let _ = run(&mut r, &pink(48_000, 17).iter().map(|n| n * 0.2).collect::<Vec<_>>());
        let near = r.cut_db().iter().enumerate().filter(|(k, _)| (r.bin_hz(*k) - 2000.0).abs() < 100.0).fold(0.0f32, |m, (_, c)| m.max(*c));
        assert!(near < 1.5, "still cutting {near} dB at 2 kHz a second later");
    }

    #[test]
    fn the_range_keeps_the_cut_inside_low_and_high() {
        let mut r = prepared();
        r.set_param("low", 1000.0);
        let len = 72_000;
        let x: Vec<f32> = pink(len, 21).iter().zip(sine(200.0, 0.1, len)).map(|(n, t)| n * 0.2 + t).collect();
        let y = run(&mut r, &x);
        let lat = r.latency();
        let cut = level_at(&x[24_000..len - lat], 200.0) - level_at(&y[lat + 24_000..], 200.0);
        assert!(cut.abs() < 0.3, "200 Hz cut by {cut} dB with Low at 1 kHz");
    }

    #[test]
    fn a_key_decides_where_the_input_is_cut() {
        let mut r = prepared();
        let len = 72_000;
        let x: Vec<f32> = pink(len, 23).iter().zip(sine(1000.0, 0.01, len)).map(|(n, t)| n * 0.2 + t).collect();
        let key: Vec<f32> = pink(len, 29).iter().zip(sine(1000.0, 0.2, len)).map(|(n, t)| n * 0.2 + t).collect();
        let mut y = Vec::with_capacity(len);
        for (xb, kb) in x.chunks(512).zip(key.chunks(512)) {
            let mut io = vec![xb.to_vec(), xb.to_vec()];
            let k = vec![kb.to_vec(), kb.to_vec()];
            r.process_with_sidechain(&mut io, xb.len(), Some(&k));
            y.extend_from_slice(&io[0]);
        }
        let lat = r.latency();
        let cut = level_at(&x[24_000..len - lat], 1000.0) - level_at(&y[lat + 24_000..], 1000.0);
        assert!(cut > 6.0, "the key's 1 kHz resonance cut the input by only {cut} dB");
    }

    #[test]
    fn the_frame_grows_with_the_sample_rate() {
        assert_eq!(frame_size_for(44_100.0), 2048);
        assert_eq!(frame_size_for(48_000.0), 2048);
        assert_eq!(frame_size_for(96_000.0), 4096);
        assert_eq!(frame_size_for(192_000.0), 8192);
        assert_eq!(frame_size_for(1_536_000.0), 16_384);
    }

    #[test]
    fn unprepared_and_reset_are_safe() {
        let mut r = Rez::new();
        let mut io = vec![vec![0.5f32; 64]; 2];
        r.process(&mut io, 64);
        assert_eq!(io[0][0], 0.5);
        assert_eq!(r.latency(), 0);
        r.prepare(SR, 64, 2);
        let _ = run(&mut r, &sine(1000.0, 0.5, 8192));
        r.reset();
        let y = run(&mut r, &vec![0.0; 4096]);
        assert!(y.iter().all(|v| *v == 0.0), "reset left audio in the rings");
    }
}
