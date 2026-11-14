//! Sprawl: a four-band stereo imager.
//!
//! Sprawl works on the difference between the two channels. The signal is taken apart into its
//! mid `(L + R) / 2` and its side `(L - R) / 2`, both are split into four bands, and each band's
//! side is turned up or down by that band's Width (times the overall Width): 0 % is mono, 100 %
//! leaves the band as it came, 200 % doubles its side. The mid is never scaled, so the mono sum
//! of the output is the mono sum of the input whatever the widths are (and Stereoize, below):
//! what Sprawl adds disappears in mono instead of colouring it.
//!
//! - The bands are split by fourth-order Linkwitz-Riley crossovers (two Butterworth sections
//!   each way) in a tree: the middle split first, then the low half at the low split and the
//!   high half at the high split, each half first passed through the other half's split as an
//!   all-pass so the four bands add back up to the input with a flat magnitude. With every width
//!   at 100 % the output is the input through those all-passes: level and image unchanged, phase
//!   rotated around the splits. No latency.
//! - Stereoize makes a side signal out of the mid, for sources that have none: the mid above
//!   [`STEREOIZE_HP_HZ`] through a chain of all-pass delays, added to the side before the bands
//!   (so each band's Width acts on it too, and the low end stays in the middle). Left and right
//!   get complementary combs that cancel in mono.
//! - Mono Safe holds a widened band back while its side is louder than its mid, which is when
//!   the band's left and right start to cancel in mono (a correlation below zero). It never turns
//!   a band below the width it came in with.
//!
//! The first two channels are the stereo pair; further channels and a mono channel pass through
//! (Trim still applies to the pair or the mono channel only). Every buffer is allocated in
//! `prepare`; `process` and `set_param` never allocate.
//!
//! For its editor Sprawl publishes ([`Analysis::live`]) the correlation of its output, each
//! band's correlation, side share and side gain in force, and a ring of recent output samples
//! for a vectorscope; the analysis rings carry the mono sum of the input and of the output.
//! `readouts`: `[0]` the output's correlation (-1..1), `[1]` its side share (0..1).

pub mod presets;

use crate::analysis::Analysis;
use crate::biquad::{Biquad, FilterType};
use crate::params::{Params, lin, log, toggle};
use crate::util::{DelayLine, Smoother, flush, frames_in, ms_to_samples, sane_channels, sane_sr, time_coef};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};
use std::f32::consts::FRAC_1_SQRT_2;
use std::sync::Arc;

/// The bands, and the splits between them.
pub const BANDS: usize = 4;
pub const SPLITS: usize = BANDS - 1;
/// The lowest and highest frequency a split can sit at, in Hz.
pub const SPLIT_HZ: (f32, f32) = (40.0, 16_000.0);
/// Neighbouring splits are kept at least this far apart (a ratio of their frequencies).
pub const MIN_SPLIT_RATIO: f32 = 1.5;
/// The widest a Width goes, in percent (100 leaves the image as it is).
pub const MAX_WIDTH: f32 = 200.0;
/// The side Stereoize makes at 100 %, against the mid it is made from.
const STEREOIZE_GAIN: f32 = 0.6;
/// Stereoize leaves the mid below this alone, in Hz.
pub const STEREOIZE_HP_HZ: f32 = 180.0;
/// The all-pass delays of Stereoize's chain, in ms, and their feedback.
const STAGE_MS: [f32; 4] = [1.37, 2.89, 4.73, 7.19];
const STAGE_G: f32 = 0.6;
/// How quickly the widths, Stereoize and Trim glide to a new value, in ms.
const GLIDE_MS: f32 = 20.0;
/// Mono Safe: the time over which a band's mid and side levels are compared, and how quickly
/// the side gain follows, in ms.
const SAFE_WINDOW_MS: f32 = 60.0;
const SAFE_GLIDE_MS: f32 = 8.0;
/// The time the correlation readings are averaged over, in ms.
const METER_MS: f32 = 250.0;
/// Power below this is silence: no correlation reading.
const POWER_FLOOR: f32 = 1.0e-12;
/// The vectorscope shows about this much sound, in ms.
const SCOPE_MS: f32 = 60.0;

/// The points of the vectorscope ring.
pub const SCOPE_POINTS: usize = 256;
/// [`Analysis::live`]: the output's correlation (-1..1; 0 in silence).
pub const LIVE_CORRELATION: usize = 0;
/// Each band's output correlation (-1..1; 0 in silence), from the lowest band up.
pub const LIVE_BAND_CORRELATION: usize = 1;
/// Each band's side share of its output power (0 mono, 0.5 unrelated sides, 1 out of phase).
pub const LIVE_BAND_SIDE: usize = LIVE_BAND_CORRELATION + BANDS;
/// Each band's side gain in force (1 = as it came; below the setting while Mono Safe holds it).
pub const LIVE_BAND_GAIN: usize = LIVE_BAND_SIDE + BANDS;
/// The vectorscope: [`SCOPE_POINTS`] recent output samples of the left channel, then the same
/// of the right (a ring: which point is the newest does not matter to a scope).
pub const LIVE_SCOPE_L: usize = LIVE_BAND_GAIN + BANDS;
pub const LIVE_SCOPE_R: usize = LIVE_SCOPE_L + SCOPE_POINTS;
/// How many live values Sprawl publishes.
pub const LIVE_LEN: usize = LIVE_SCOPE_R + SCOPE_POINTS;

const P_WIDTH: usize = 0;
const P_STEREOIZE: usize = 1;
const P_SAFE: usize = 2;
const P_TRIM: usize = 3;
const P_SPLITS: usize = 4;
const P_BANDS: usize = P_SPLITS + SPLITS;

/// The splits' defaults, in Hz.
pub const SPLIT_DEFAULTS: [f32; SPLITS] = [120.0, 1000.0, 6000.0];

static SPRAWL_PARAMS: [ParamInfo; P_BANDS + BANDS] = [
    lin("width", "Width", 0.0, MAX_WIDTH, 100.0, Unit::Percent),
    lin("stereoize", "Stereoize", 0.0, 100.0, 0.0, Unit::Percent),
    toggle("safe", "Mono Safe", false),
    lin("trim", "Trim", -12.0, 12.0, 0.0, Unit::Db),
    log("split1", "Split 1", SPLIT_HZ.0, SPLIT_HZ.1, SPLIT_DEFAULTS[0], Unit::Hz),
    log("split2", "Split 2", SPLIT_HZ.0, SPLIT_HZ.1, SPLIT_DEFAULTS[1], Unit::Hz),
    log("split3", "Split 3", SPLIT_HZ.0, SPLIT_HZ.1, SPLIT_DEFAULTS[2], Unit::Hz),
    lin("b1_width", "Band 1 Width", 0.0, MAX_WIDTH, 100.0, Unit::Percent),
    lin("b2_width", "Band 2 Width", 0.0, MAX_WIDTH, 100.0, Unit::Percent),
    lin("b3_width", "Band 3 Width", 0.0, MAX_WIDTH, 100.0, Unit::Percent),
    lin("b4_width", "Band 4 Width", 0.0, MAX_WIDTH, 100.0, Unit::Percent),
];

/// The parameter ids of the splits, from the lowest up, and of the bands' widths.
pub const SPLIT_IDS: [&str; SPLITS] = ["split1", "split2", "split3"];
pub const BAND_IDS: [&str; BANDS] = ["b1_width", "b2_width", "b3_width", "b4_width"];
/// What the bands are called.
pub const BAND_NAMES: [&str; BANDS] = ["Low", "Low Mid", "High Mid", "High"];

pub static SPRAWL_INFO: PluginInfo = PluginInfo {
    id: "sprawl",
    name: "Sprawl",
    short_name: "Sprawl",
    category: Category::Other,
    params: &SPRAWL_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// The splits Sprawl works with for the settings `set` (Hz, from the lowest up) at sample rate
/// `sr`: in order, [`MIN_SPLIT_RATIO`] apart at least, inside [`SPLIT_HZ`] and below Nyquist. A
/// split set past its neighbour is held at the nearest place it can be; a NaN is its default.
pub fn splits(set: [f32; SPLITS], sr: f32) -> [f32; SPLITS] {
    let [a, b, c] = set;
    let clean = |v: f32, default: f32| if v.is_finite() { v } else { default };
    let r = MIN_SPLIT_RATIO;
    let top = SPLIT_HZ.1.min(sane_sr(sr) * 0.45);
    let a = clean(a, SPLIT_DEFAULTS[0]).min(top / (r * r)).max(SPLIT_HZ.0);
    let b = clean(b, SPLIT_DEFAULTS[1]).min(top / r).max(a * r);
    let c = clean(c, SPLIT_DEFAULTS[2]).min(top).max(b * r);
    [a, b, c]
}

/// The band (0 the lowest) that `hz` falls in, with the splits `at`.
pub fn band_of(at: &[f32; SPLITS], hz: f32) -> usize {
    at.iter().filter(|s| hz >= **s).count()
}

/// The side gain of a band: its Width times the overall Width, both in percent.
pub fn side_gain(width: f32, band_width: f32) -> f32 {
    let g = (width / 100.0) * (band_width / 100.0);
    if g.is_finite() { g.clamp(0.0, (MAX_WIDTH / 100.0) * (MAX_WIDTH / 100.0)) } else { 1.0 }
}

/// The correlation of left and right from the averaged powers of the mid, the side and their
/// product: 1 the same signal, 0 unrelated (or silence), -1 the same signal inverted.
fn correlation(mm: f32, ss: f32, ms: f32) -> f32 {
    let ll = (mm + ss + 2.0 * ms).max(0.0);
    let rr = (mm + ss - 2.0 * ms).max(0.0);
    let den = (ll * rr).sqrt();
    if den > POWER_FLOOR && den.is_finite() { ((mm - ss) / den).clamp(-1.0, 1.0) } else { 0.0 }
}

/// The side's share of the power: 0 mono, 1 nothing but side (0 in silence).
fn side_share(mm: f32, ss: f32) -> f32 {
    let all = mm + ss;
    if all > POWER_FLOOR && all.is_finite() { (ss / all).clamp(0.0, 1.0) } else { 0.0 }
}

/// A fourth-order Linkwitz-Riley split: two Butterworth sections each way.
#[derive(Debug, Clone, Copy, Default)]
struct Split {
    lp: [Biquad; 2],
    hp: [Biquad; 2],
}

impl Split {
    fn set(&mut self, hz: f32, sr: f32) {
        for b in &mut self.lp {
            b.set(FilterType::LowPass, hz, FRAC_1_SQRT_2, 0.0, sr);
        }
        for b in &mut self.hp {
            b.set(FilterType::HighPass, hz, FRAC_1_SQRT_2, 0.0, sr);
        }
    }

    fn reset(&mut self) {
        self.lp.iter_mut().chain(&mut self.hp).for_each(Biquad::reset);
    }

    /// (below the split, above it).
    #[inline]
    fn run(&mut self, x: f32) -> (f32, f32) {
        let [l1, l2] = &mut self.lp;
        let [h1, h2] = &mut self.hp;
        (l2.process_sample(l1.process_sample(x)), h2.process_sample(h1.process_sample(x)))
    }
}

/// One signal into four bands that add back up to it (through all-passes).
#[derive(Debug, Clone, Copy, Default)]
struct Crossover {
    middle: Split,
    low: Split,
    high: Split,
    /// The high split's phase, for the low half; the low split's, for the high half.
    low_phase: Biquad,
    high_phase: Biquad,
}

impl Crossover {
    fn set(&mut self, at: &[f32; SPLITS], sr: f32) {
        let [a, b, c] = *at;
        self.low.set(a, sr);
        self.middle.set(b, sr);
        self.high.set(c, sr);
        self.low_phase.set(FilterType::AllPass, c, FRAC_1_SQRT_2, 0.0, sr);
        self.high_phase.set(FilterType::AllPass, a, FRAC_1_SQRT_2, 0.0, sr);
    }

    fn reset(&mut self) {
        self.middle.reset();
        self.low.reset();
        self.high.reset();
        self.low_phase.reset();
        self.high_phase.reset();
    }

    #[inline]
    fn run(&mut self, x: f32) -> [f32; BANDS] {
        let (lo, hi) = self.middle.run(x);
        let (b1, b2) = self.low.run(self.low_phase.process_sample(lo));
        let (b3, b4) = self.high.run(self.high_phase.process_sample(hi));
        [b1, b2, b3, b4]
    }
}

/// Stereoize's side maker: a high-pass, then a chain of all-pass delays.
#[derive(Debug, Clone, Default)]
struct Decorrelator {
    hp: Biquad,
    lines: [DelayLine; 4],
    delays: [usize; 4],
}

impl Decorrelator {
    fn prepare(&mut self, sr: f32) {
        self.hp.set(FilterType::HighPass, STEREOIZE_HP_HZ, FRAC_1_SQRT_2, 0.0, sr);
        for ((line, delay), ms) in self.lines.iter_mut().zip(&mut self.delays).zip(STAGE_MS) {
            *delay = (ms_to_samples(ms, sr).round() as usize).max(1);
            line.allocate(*delay + 1);
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.hp.reset();
        self.lines.iter_mut().for_each(DelayLine::clear);
    }

    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let mut v = self.hp.process_sample(x);
        for (line, delay) in self.lines.iter_mut().zip(self.delays) {
            let delayed = line.tap(delay);
            let w = flush(v + STAGE_G * delayed);
            line.push(w);
            v = delayed - STAGE_G * w;
        }
        v
    }
}

/// The averaged powers of a mid and a side and of their product.
#[derive(Debug, Clone, Copy, Default)]
struct Powers {
    mm: f32,
    ss: f32,
    ms: f32,
}

impl Powers {
    #[inline]
    fn add(&mut self, m: f32, s: f32, keep: f32) {
        self.mm = flush(m * m + (self.mm - m * m) * keep);
        self.ss = flush(s * s + (self.ss - s * s) * keep);
        self.ms = flush(m * s + (self.ms - m * s) * keep);
    }
}

/// What `prepare` builds: the filters, the meters and the gliding values of the stereo pair.
#[derive(Debug, Clone)]
struct Engine {
    mid: Crossover,
    side: Crossover,
    decor: Decorrelator,
    /// Each band's side gain as set, gliding, and as applied (Mono Safe may hold it lower).
    gains: [Smoother; BANDS],
    applied: [f32; BANDS],
    stereoize: Smoother,
    trim: Smoother,
    /// Mono Safe's view of each band as it came in.
    safe: [Powers; BANDS],
    /// The meters: each band's output, and the whole output.
    bands: [Powers; BANDS],
    out: Powers,
    safe_keep: f32,
    safe_glide: f32,
    meter_keep: f32,
    /// The vectorscope ring: where the next point goes, and the samples until it is taken.
    scope_at: usize,
    scope_step: usize,
    scope_wait: usize,
}

impl Engine {
    fn new(sr: f32) -> Self {
        let glide = |v: f32| {
            let mut s = Smoother::new(v);
            s.set_time(GLIDE_MS, sr);
            s
        };
        let mut decor = Decorrelator::default();
        decor.prepare(sr);
        Engine {
            mid: Crossover::default(),
            side: Crossover::default(),
            decor,
            gains: [glide(1.0); BANDS],
            applied: [1.0; BANDS],
            stereoize: glide(0.0),
            trim: glide(1.0),
            safe: [Powers::default(); BANDS],
            bands: [Powers::default(); BANDS],
            out: Powers::default(),
            safe_keep: time_coef(SAFE_WINDOW_MS, sr),
            safe_glide: time_coef(SAFE_GLIDE_MS, sr),
            meter_keep: time_coef(METER_MS, sr),
            scope_at: 0,
            scope_step: ((ms_to_samples(SCOPE_MS, sr) / SCOPE_POINTS as f32).round() as usize).max(1),
            scope_wait: 0,
        }
    }

    /// Forgets the sound so far; the settings arrive where they are going.
    fn reset(&mut self) {
        self.mid.reset();
        self.side.reset();
        self.decor.reset();
        self.gains.iter_mut().for_each(Smoother::snap);
        for (applied, gain) in self.applied.iter_mut().zip(&self.gains) {
            *applied = gain.value();
        }
        self.stereoize.snap();
        self.trim.snap();
        self.safe = [Powers::default(); BANDS];
        self.bands = [Powers::default(); BANDS];
        self.out = Powers::default();
        self.scope_at = 0;
        self.scope_wait = 0;
    }

    fn publish(&self, live: &crate::analysis::LiveValues) {
        live.set(LIVE_CORRELATION, correlation(self.out.mm, self.out.ss, self.out.ms));
        for (i, (p, g)) in self.bands.iter().zip(&self.applied).enumerate() {
            live.set(LIVE_BAND_CORRELATION + i, correlation(p.mm, p.ss, p.ms));
            live.set(LIVE_BAND_SIDE + i, side_share(p.mm, p.ss));
            live.set(LIVE_BAND_GAIN + i, *g);
        }
    }
}

/// The stereo imager.
pub struct Sprawl {
    p: Params,
    sr: f32,
    ch: usize,
    engine: Option<Box<Engine>>,
    /// The splits in force (see [`splits`]).
    at: [f32; SPLITS],
    safe: bool,
    /// Trim for a mono channel (the stereo pair's glides in the engine).
    mono_trim: f32,
    correlation: f32,
    side: f32,
    analysis: Arc<Analysis>,
}

impl Default for Sprawl {
    fn default() -> Self {
        Self::new()
    }
}

impl Sprawl {
    pub fn new() -> Self {
        let mut i = Sprawl {
            p: Params::new(&SPRAWL_PARAMS),
            sr: 48_000.0,
            ch: 0,
            engine: None,
            at: SPLIT_DEFAULTS,
            safe: false,
            mono_trim: 1.0,
            correlation: 0.0,
            side: 0.0,
            analysis: Arc::new(Analysis::with_live(LIVE_LEN)),
        };
        i.update();
        i
    }

    /// The splits in force, in Hz from the lowest up.
    pub fn splits(&self) -> [f32; SPLITS] {
        self.at
    }

    /// Works the parameters out into the engine's targets. No allocation.
    fn update(&mut self) {
        let at = splits([self.p.v(P_SPLITS), self.p.v(P_SPLITS + 1), self.p.v(P_SPLITS + 2)], self.sr);
        let moved = at != self.at;
        self.at = at;
        self.safe = self.p.on(P_SAFE);
        let trim = db_to_gain(self.p.v(P_TRIM));
        self.mono_trim = trim;
        let Some(e) = self.engine.as_mut() else { return };
        if moved {
            e.mid.set(&at, self.sr);
            e.side.set(&at, self.sr);
        }
        let width = self.p.v(P_WIDTH);
        for (i, g) in e.gains.iter_mut().enumerate() {
            g.set(side_gain(width, self.p.v(P_BANDS + i)));
        }
        e.stereoize.set(self.p.v(P_STEREOIZE) / 100.0 * STEREOIZE_GAIN);
        e.trim.set(trim);
    }

    fn run_mono(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let frames = frames_in(io, frames, 1);
        let Some(buf) = io.first_mut() else { return };
        for x in buf.iter_mut().take(frames) {
            let dry = if x.is_finite() { *x } else { 0.0 };
            *x = dry * self.mono_trim;
            self.analysis.pre.push(dry);
            self.analysis.post.push(*x);
        }
    }

    fn run_stereo(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let safe = self.safe;
        let Some(e) = self.engine.as_mut() else { return };
        let frames = frames_in(io, frames, 2);
        let [left, right, ..] = io else { return };
        for (l, r) in left.iter_mut().zip(right.iter_mut()).take(frames) {
            let (xl, xr) = (if l.is_finite() { *l } else { 0.0 }, if r.is_finite() { *r } else { 0.0 });
            let m = 0.5 * (xl + xr);
            let s = 0.5 * (xl - xr) + e.stereoize.next() * e.decor.run(m);
            let mid = e.mid.run(m);
            let side = e.side.run(s);
            let (mut mo, mut so) = (0.0f32, 0.0f32);
            for b in 0..BANDS {
                let (Some(mb), Some(sb), Some(gain), Some(applied), Some(seen), Some(meter)) =
                    (mid.get(b), side.get(b), e.gains.get_mut(b), e.applied.get_mut(b), e.safe.get_mut(b), e.bands.get_mut(b))
                else {
                    continue;
                };
                let mut g = gain.next();
                if safe {
                    seen.add(*mb, *sb, e.safe_keep);
                    // Widened past the point where the side is as loud as the mid, the band's
                    // left and right start to cancel: hold it there, never below as it came.
                    if g > 1.0 && seen.ss * g * g > seen.mm {
                        g = (seen.mm / seen.ss.max(POWER_FLOOR)).sqrt().clamp(1.0, g);
                    }
                    *applied = g + (*applied - g) * e.safe_glide;
                } else {
                    *applied = g;
                }
                let sb = *sb * *applied;
                meter.add(*mb, sb, e.meter_keep);
                mo += *mb;
                so += sb;
            }
            let trim = e.trim.next();
            let (yl, yr) = ((mo + so) * trim, (mo - so) * trim);
            *l = yl;
            *r = yr;
            e.out.add(mo, so, e.meter_keep);
            self.analysis.pre.push(m);
            self.analysis.post.push(mo * trim);
            if e.scope_wait == 0 {
                e.scope_wait = e.scope_step;
                self.analysis.live.set(LIVE_SCOPE_L + e.scope_at, yl);
                self.analysis.live.set(LIVE_SCOPE_R + e.scope_at, yr);
                e.scope_at = if e.scope_at + 1 >= SCOPE_POINTS { 0 } else { e.scope_at + 1 };
            }
            e.scope_wait -= 1;
        }
        self.correlation = correlation(e.out.mm, e.out.ss, e.out.ms);
        self.side = side_share(e.out.mm, e.out.ss);
        e.publish(&self.analysis.live);
    }
}

impl Plugin for Sprawl {
    crate::params::param_plumbing!(SPRAWL_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        let mut e = Box::new(Engine::new(self.sr));
        self.at = splits([self.p.v(P_SPLITS), self.p.v(P_SPLITS + 1), self.p.v(P_SPLITS + 2)], self.sr);
        e.mid.set(&self.at, self.sr);
        e.side.set(&self.at, self.sr);
        self.engine = Some(e);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.correlation = 0.0;
        self.side = 0.0;
        if let Some(e) = self.engine.as_mut() {
            e.reset();
            e.publish(&self.analysis.live);
        }
        for i in 0..2 * SCOPE_POINTS {
            self.analysis.live.set(LIVE_SCOPE_L + i, 0.0);
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        match self.ch.min(io.len()) {
            0 => {}
            1 => self.run_mono(io, frames),
            _ => self.run_stereo(io, frames),
        }
    }

    fn tail_samples(&self) -> usize {
        // Stereoize's all-pass chain rings on for a moment.
        if self.p.v(P_STEREOIZE) > 0.0 { ms_to_samples(150.0, self.sr) as usize } else { 0 }
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        out[0] = self.correlation;
        out[1] = self.side;
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(self.analysis.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: f32 = 48_000.0;
    const LEN: usize = 48_000;

    fn noise(len: usize, mut seed: u32) -> Vec<f32> {
        (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.5
            })
            .collect()
    }

    fn sine(hz: f32, amp: f32, len: usize) -> Vec<f32> {
        (0..len).map(|i| amp * (TAU * hz * i as f32 / SR).sin()).collect()
    }

    fn prepared(set: &[(&str, f32)]) -> Sprawl {
        let mut im = Sprawl::new();
        for (id, v) in set {
            assert!(im.set_param(id, *v), "{id}");
        }
        im.prepare(SR, 512, 2);
        im
    }

    fn run(im: &mut Sprawl, l: &[f32], r: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let (mut ol, mut or) = (Vec::new(), Vec::new());
        for (a, b) in l.chunks(512).zip(r.chunks(512)) {
            let mut io = vec![a.to_vec(), b.to_vec()];
            im.process(&mut io, a.len());
            ol.extend_from_slice(&io[0]);
            or.extend_from_slice(&io[1]);
        }
        (ol, or)
    }

    /// RMS of the second half (the filters have settled).
    fn rms(x: &[f32]) -> f32 {
        let x = &x[x.len() / 2..];
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    fn db(a: f32, b: f32) -> f32 {
        20.0 * (a / b).log10()
    }

    fn mid(l: &[f32], r: &[f32]) -> Vec<f32> {
        l.iter().zip(r).map(|(a, b)| 0.5 * (a + b)).collect()
    }

    fn side(l: &[f32], r: &[f32]) -> Vec<f32> {
        l.iter().zip(r).map(|(a, b)| 0.5 * (a - b)).collect()
    }

    fn corr(l: &[f32], r: &[f32]) -> f32 {
        let (l, r) = (&l[l.len() / 2..], &r[r.len() / 2..]);
        let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        dot(l, r) / (dot(l, l) * dot(r, r)).sqrt()
    }

    #[test]
    fn at_its_defaults_the_level_and_the_image_are_unchanged_at_every_frequency() {
        // On the splits, between them and far from them: a tone on the left only keeps its level
        // and stays on the left.
        for hz in [30.0, 60.0, 120.0, 350.0, 1000.0, 2500.0, 6000.0, 9000.0, 15_000.0] {
            let mut im = prepared(&[]);
            let l = sine(hz, 0.5, LEN);
            let (ol, or) = run(&mut im, &l, &vec![0.0; LEN]);
            assert!(db(rms(&ol), rms(&l)).abs() < 0.02, "{hz} Hz: {} dB", db(rms(&ol), rms(&l)));
            assert!(rms(&or) < rms(&l) * 1.0e-4, "{hz} Hz leaked to the right: {}", rms(&or));
        }
        // Two unrelated sides keep their levels and stay unrelated.
        let mut im = prepared(&[]);
        let (l, r) = (noise(LEN, 1), noise(LEN, 2));
        let (ol, or) = run(&mut im, &l, &r);
        assert!(db(rms(&ol), rms(&l)).abs() < 0.05 && db(rms(&or), rms(&r)).abs() < 0.05);
        assert!(corr(&ol, &or).abs() < 0.05);
        assert_eq!(im.latency(), 0);
    }

    #[test]
    fn width_zero_is_mono_and_width_200_doubles_the_side() {
        let (l, r) = (noise(LEN, 3), noise(LEN, 4));
        let mut im = prepared(&[("width", 0.0)]);
        let (ol, or) = run(&mut im, &l, &r);
        assert!(rms(&side(&ol, &or)) < 1.0e-6, "mono: {}", rms(&side(&ol, &or)));
        assert!(db(rms(&ol), rms(&mid(&l, &r))).abs() < 0.05, "the mid at its own level");

        let mut im = prepared(&[("width", 200.0)]);
        let (ol, or) = run(&mut im, &l, &r);
        let gain = db(rms(&side(&ol, &or)), rms(&side(&l, &r)));
        assert!((gain - 6.02).abs() < 0.05, "the side is up {gain} dB");
    }

    #[test]
    fn the_mono_sum_is_the_same_whatever_the_widths_and_stereoize_are() {
        let (l, r) = (noise(LEN, 5), noise(LEN, 6));
        let mut plain = prepared(&[]);
        let (pl, pr) = run(&mut plain, &l, &r);
        let want = mid(&pl, &pr);
        let set = [("width", 170.0), ("b1_width", 0.0), ("b2_width", 60.0), ("b4_width", 200.0), ("stereoize", 100.0), ("safe", 1.0)];
        let mut im = prepared(&set);
        let (ol, or) = run(&mut im, &l, &r);
        let got = mid(&ol, &or);
        let worst = want.iter().zip(&got).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(worst < 1.0e-5, "the mono sum moved by {worst}");
        let moved: Vec<f32> = side(&ol, &or).iter().zip(side(&pl, &pr)).map(|(a, b)| a - b).collect();
        assert!(rms(&moved) > 0.02, "and the image did change: {}", rms(&moved));
    }

    #[test]
    fn a_bands_width_acts_on_its_band_only() {
        // Out-of-phase tones (all side) at 60 Hz and 5 kHz, the low band made mono.
        let tone = |hz: f32| sine(hz, 0.3, LEN);
        let l: Vec<f32> = tone(60.0).iter().zip(tone(5000.0)).map(|(a, b)| a + b).collect();
        let r: Vec<f32> = l.iter().map(|v| -v).collect();
        let level = |x: &[f32], hz: f32| {
            let x = &x[x.len() / 2..];
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (i, v) in x.iter().enumerate() {
                let ph = TAU * hz * i as f32 / SR;
                re += v * ph.cos();
                im += v * ph.sin();
            }
            2.0 * (re * re + im * im).sqrt() / x.len() as f32
        };
        let mut im = prepared(&[("split1", 300.0), ("b1_width", 0.0)]);
        let (ol, or) = run(&mut im, &l, &r);
        let s = side(&ol, &or);
        assert!(db(level(&s, 60.0), 0.3) < -50.0, "60 Hz is mono: {} dB", db(level(&s, 60.0), 0.3));
        assert!(db(level(&s, 5000.0), 0.3).abs() < 0.05, "5 kHz is as it came: {} dB", db(level(&s, 5000.0), 0.3));

        let mut im = prepared(&[("split3", 16_000.0), ("b3_width", 200.0)]);
        let (ol, or) = run(&mut im, &l, &r);
        let s = side(&ol, &or);
        assert!((db(level(&s, 5000.0), 0.3) - 6.02).abs() < 0.1 && db(level(&s, 60.0), 0.3).abs() < 0.05);
    }

    #[test]
    fn stereoize_gives_a_mono_source_a_side_and_leaves_the_low_end_in_the_middle() {
        let x = noise(LEN, 7);
        let mut im = prepared(&[("stereoize", 100.0)]);
        let (ol, or) = run(&mut im, &x, &x);
        let ratio = rms(&side(&ol, &or)) / rms(&mid(&ol, &or));
        assert!(ratio > 0.4 && ratio < 0.7, "side against mid: {ratio}");
        assert!(corr(&ol, &or) < 0.75, "no longer one signal: {}", corr(&ol, &or));
        assert!(im.tail_samples() > 0);

        let bass = sine(50.0, 0.5, LEN);
        let mut im = prepared(&[("stereoize", 100.0)]);
        let (ol, or) = run(&mut im, &bass, &bass);
        assert!(db(rms(&side(&ol, &or)), rms(&bass)) < -18.0, "50 Hz stays in the middle");

        // Off, a mono source stays mono.
        let mut im = prepared(&[("width", 200.0)]);
        let (ol, or) = run(&mut im, &x, &x);
        assert_eq!(rms(&side(&ol, &or)), 0.0);
        assert_eq!(im.tail_samples(), 0);
    }

    #[test]
    fn mono_safe_stops_a_widened_band_before_it_cancels_in_mono() {
        // A side nearly as loud as the mid, doubled: left and right would be mostly opposed.
        let (m, s) = (noise(LEN, 8), noise(LEN, 9));
        let l: Vec<f32> = m.iter().zip(&s).map(|(m, s)| m + 0.8 * s).collect();
        let r: Vec<f32> = m.iter().zip(&s).map(|(m, s)| m - 0.8 * s).collect();
        let mut im = prepared(&[("width", 200.0)]);
        let (ol, or) = run(&mut im, &l, &r);
        assert!(corr(&ol, &or) < -0.3, "without it: {}", corr(&ol, &or));

        let mut im = prepared(&[("width", 200.0), ("safe", 1.0)]);
        let (ol, or) = run(&mut im, &l, &r);
        assert!(corr(&ol, &or) > -0.08, "with it: {}", corr(&ol, &or));
        assert!(rms(&side(&ol, &or)) >= rms(&side(&l, &r)) * 0.99, "never narrower than it came");
        let live = &im.analysis().unwrap().live;
        for b in 0..BANDS {
            let g = live.get(LIVE_BAND_GAIN + b);
            assert!((1.0..1.6).contains(&g), "band {b} is held at {g}");
        }

        // A source with little side is widened in full.
        let l: Vec<f32> = m.iter().zip(&s).map(|(m, s)| m + 0.2 * s).collect();
        let r: Vec<f32> = m.iter().zip(&s).map(|(m, s)| m - 0.2 * s).collect();
        let mut im = prepared(&[("width", 200.0), ("safe", 1.0)]);
        let (ol, or) = run(&mut im, &l, &r);
        assert!((db(rms(&side(&ol, &or)), rms(&side(&l, &r))) - 6.02).abs() < 0.1);
    }

    #[test]
    fn the_splits_stay_in_order_and_apart() {
        assert_eq!(splits(SPLIT_DEFAULTS, SR), SPLIT_DEFAULTS);
        for set in [[5000.0, 100.0, 50.0], [16_000.0; 3], [40.0; 3], [f32::NAN, f32::INFINITY, -1.0], [1000.0, 1001.0, 1002.0]] {
            for sr in [1000.0, 8000.0, 44_100.0, 192_000.0] {
                let [a, b, c] = splits(set, sr);
                let ok = a >= SPLIT_HZ.0 && c <= SPLIT_HZ.1.min(sr * 0.45) * 1.0001;
                assert!(ok && b >= a * MIN_SPLIT_RATIO * 0.9999 && c >= b * MIN_SPLIT_RATIO * 0.9999, "{set:?} at {sr}: {a} {b} {c}");
            }
        }
        // A split set past its neighbour is held next to it.
        let mut im = prepared(&[("split1", 3000.0)]);
        assert_eq!(im.splits(), [3000.0, 4500.0, 6750.0]);
        assert!(im.set_param("split1", 120.0));
        assert_eq!(im.splits(), SPLIT_DEFAULTS);
        assert_eq!(band_of(&SPLIT_DEFAULTS, 50.0), 0);
        assert_eq!(band_of(&SPLIT_DEFAULTS, 1000.0), 2);
        assert_eq!(band_of(&SPLIT_DEFAULTS, 19_000.0), 3);
    }

    #[test]
    fn the_bands_still_add_up_with_the_splits_anywhere() {
        for set in [[40.0, 60.0, 90.0], [4000.0, 9000.0, 16_000.0], [200.0, 300.0, 12_000.0]] {
            for hz in [45.0, 75.0, 250.0, 3000.0, 10_000.0] {
                let mut im = prepared(&[("split1", set[0]), ("split2", set[1]), ("split3", set[2])]);
                let l = sine(hz, 0.5, LEN);
                let (ol, _) = run(&mut im, &l, &vec![0.0; LEN]);
                assert!(db(rms(&ol), rms(&l)).abs() < 0.03, "{set:?} at {hz} Hz: {} dB", db(rms(&ol), rms(&l)));
            }
        }
    }

    #[test]
    fn trim_scales_the_output_and_changes_glide() {
        let (l, r) = (noise(LEN, 10), noise(LEN, 11));
        let mut im = prepared(&[("trim", -6.0)]);
        let (ol, _) = run(&mut im, &l, &r);
        assert!((db(rms(&ol), rms(&l)) + 6.0).abs() < 0.05);
        // A width change mid-stream moves no sample by a jump.
        let mut im = prepared(&[]);
        let x = sine(1000.0, 0.5, 4800);
        let y: Vec<f32> = x.iter().map(|v| -v).collect();
        let (a, _) = run(&mut im, &x, &y);
        assert!(im.set_param("width", 0.0));
        let (b, _) = run(&mut im, &x, &y);
        let step = (b[0] - a[a.len() - 1]).abs();
        assert!(step < 0.1, "a jump of {step}");
        let long = sine(1000.0, 0.5, LEN);
        let inverted: Vec<f32> = long.iter().map(|v| -v).collect();
        let (c, _) = run(&mut im, &long, &inverted);
        assert!(b[b.len() - 1].abs() < 0.05 && rms(&c) < 1.0e-5, "and it gets there: {}", rms(&c));
    }

    #[test]
    fn the_editor_gets_the_correlation_the_bands_and_a_scope() {
        let x = noise(LEN, 12);
        let inv: Vec<f32> = x.iter().map(|v| -v).collect();
        let mut im = prepared(&[]);
        let _ = run(&mut im, &x, &x);
        let a = im.analysis().unwrap();
        assert_eq!(a.live.len(), LIVE_LEN);
        assert!(a.live.get(LIVE_CORRELATION) > 0.99);
        let mut ro = [f32::NAN; 8];
        im.readouts(&mut ro);
        assert!(ro[0] > 0.99 && ro[1] < 0.01 && ro[2].is_nan());
        for b in 0..BANDS {
            assert!(a.live.get(LIVE_BAND_CORRELATION + b) > 0.99, "band {b}");
            assert!(a.live.get(LIVE_BAND_SIDE + b) < 0.01);
            assert_eq!(a.live.get(LIVE_BAND_GAIN + b), 1.0);
        }
        // The scope holds the output: left and right the same here, and not silent.
        let scope: Vec<(f32, f32)> = (0..SCOPE_POINTS).map(|i| (a.live.get(LIVE_SCOPE_L + i), a.live.get(LIVE_SCOPE_R + i))).collect();
        assert!(scope.iter().all(|(l, r)| l == r) && scope.iter().filter(|(l, _)| *l != 0.0).count() > SCOPE_POINTS / 2);
        assert!(a.pre.written() >= LEN && a.post.written() >= LEN);

        im.reset();
        assert_eq!(a.live.get(LIVE_CORRELATION), 0.0);
        assert!((0..2 * SCOPE_POINTS).all(|i| a.live.get(LIVE_SCOPE_L + i) == 0.0));
        let _ = run(&mut im, &x, &inv);
        assert!(a.live.get(LIVE_CORRELATION) < -0.99);
        assert!(a.live.get(LIVE_BAND_SIDE + 2) > 0.99);
    }

    #[test]
    fn mono_more_channels_hostile_input_and_no_prepare_are_safe() {
        // Unprepared: nothing happens.
        let mut im = Sprawl::new();
        let mut io = vec![vec![0.5; 64]; 2];
        im.process(&mut io, 64);
        im.reset();
        assert_eq!(io[0][0], 0.5);

        // Mono: the channel passes (Trim applies).
        let mut im = Sprawl::new();
        assert!(im.set_param("trim", -6.0) && im.set_param("width", 0.0));
        im.prepare(SR, 64, 1);
        let mut io = vec![vec![0.5; 64]];
        im.process(&mut io, 64);
        assert!((io[0][0] - 0.5 * db_to_gain(-6.0)).abs() < 1.0e-6);

        // Six channels: the first two are the pair, the rest are untouched.
        let mut im = prepared(&[]);
        im.prepare(SR, 64, 6);
        assert!(im.set_param("width", 0.0));
        let mut io: Vec<Vec<f32>> = (0..6).map(|c| vec![0.1 * (c + 1) as f32; 64]).collect();
        im.process(&mut io, 64);
        assert_eq!(io[2], vec![0.3; 64]);
        assert_eq!(io[5], vec![0.6; 64]);

        // Hostile: NaN and infinite samples, more frames than there are, short and missing buffers.
        let mut im = prepared(&[("stereoize", 100.0), ("safe", 1.0), ("width", 200.0)]);
        for id in SPRAWL_INFO.params.iter().map(|p| p.id) {
            assert!(im.set_param(id, f32::NAN) && im.set_param(id, f32::INFINITY) && im.set_param(id, f32::NEG_INFINITY));
        }
        assert!(!im.set_param("no_such", 1.0));
        let mut io = vec![vec![f32::NAN, f32::INFINITY, 1.0e30, -1.0e30, 0.5], vec![f32::NEG_INFINITY, 0.25, f32::NAN]];
        im.process(&mut io, 1000);
        im.process(&mut [], 16);
        im.process(&mut [vec![0.0; 4]], 16);
        let mut io = vec![vec![0.25; 512]; 2];
        for _ in 0..40 {
            im.process(&mut io, 512);
        }
        assert!(io.iter().flatten().all(|v| v.is_finite()), "it recovers");
        for sr in [f32::NAN, 0.0, 1000.0, 1.0e9] {
            im.prepare(sr, 0, usize::MAX);
            let mut io = vec![vec![0.25; 32]; 2];
            im.process(&mut io, 32);
            assert!(io.iter().flatten().all(|v| v.is_finite()), "at {sr}");
        }
    }
}
