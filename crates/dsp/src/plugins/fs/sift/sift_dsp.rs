//! SiftDsp: a de-esser that also catches plosives, for vocals. Rust-native: no C++ original.
//!
//! It sorts each moment of a voice into three parts and lets each be treated on its own: the
//! VOICE itself, the SIBILANCE (the hiss of s, sh, t, ch) and the PLOSIVES (the thump of p and
//! b). Nothing is judged by how loud it is, only by its shape against the voice's own level, so a
//! quiet take is treated like a loud one.
//!
//! Per frame (an STFT about 8 ms long, a new frame every eighth of that, Hann in and out; one
//! complex FFT carries both channels and both get the same gain):
//!
//! 1. Three levels: the BODY (200 Hz to 3 kHz, where vowels live), the SIBILANT band (between
//!    the Sibilance limits) and the LOW band (under the Plosive ceiling).
//! 2. Is this a sibilant? A vowel's top is far under its body; a sibilant's is level with it or
//!    over. The sibilant band against the body, in dB, gives a likelihood from 0 to 1.
//! 3. How much to take off: the sibilant band is held to a level set against the VOICE
//!    REFERENCE (the body's level as it has recently been, so an s that follows a loud vowel is
//!    measured against that vowel). What is over the mark comes down, up to Range.
//! 4. Is this a plosive? The low band jumps well over the body and well over where the low band
//!    has recently been (its own slow reference, so a low voice's steady fundamental and a
//!    steady rumble are left alone). The jump comes down towards the reference, up to Range.
//! 5. Both cuts rise within a frame and fall back with their own release, and are applied only
//!    to their own band (Split), or the sibilant one to everything (Wide).
//!
//! Listen plays one part alone: the sibilance or the plosives as found, or everything removed.
//!
//! Latency: the frame length, always. `prepare` allocates every buffer; `process`, `reset` and
//! `set` never allocate.

use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::sync::Arc;

/// The frame's length, seconds: short enough for consonants and for singing through it.
const FRAME_S: f64 = 0.008;
/// The body: where a voice's vowels are.
const BODY_LO_HZ: f32 = 200.0;
const BODY_HI_HZ: f32 = 3_000.0;
/// A frame quieter than this (dBFS, a full-scale sine reads 0) is silence: nothing is judged
/// and nothing is learned from it.
const GATE_DB: f32 = -70.0;
/// The sibilant band against the body: under `SIB_OFF_DB` it is a vowel, over `SIB_ON_DB` a
/// sibilant, a blend between.
const SIB_OFF_DB: f32 = -14.0;
const SIB_ON_DB: f32 = -2.0;
/// Where the sibilant band is held, against the voice reference, at Amount 0 and at 100 %.
const SIB_MARK_0_DB: f32 = 6.0;
const SIB_MARK_100_DB: f32 = -26.0;
/// dB taken off per dB over the mark.
const SIB_RATIO: f32 = 0.8;
/// The low band against the body: under the "off" mark it is a voice, over the "on" mark a
/// thump. Amount moves both, from the first pair to the second.
const PLO_OFF_DB: (f32, f32) = (14.0, 2.0);
const PLO_ON_DB: (f32, f32) = (24.0, 10.0);
/// The low band over its own reference before it counts as a jump, and how far over is fully one.
const PLO_JUMP_DB: f32 = 3.0;
const PLO_JUMP_FULL_DB: f32 = 9.0;
const PLO_RATIO: f32 = 0.9;
/// The voice reference follows the body: quickly up, slowly down.
const VOICE_UP_MS: f32 = 15.0;
const VOICE_DOWN_MS: f32 = 500.0;
/// The low reference follows the low band: up fast enough to sit on the peaks of a low voice
/// (whose level swings within each cycle of its pitch, a frame being shorter than a cycle), down
/// slowly. While something looks like a thump it rises much more slowly: a thump is over before
/// it is learned, and something that goes on (a rumble, a low hum) is learned all the same.
const LOW_UP_MS: f32 = 60.0;
const LOW_UP_THUMP_MS: f32 = 400.0;
const LOW_DOWN_MS: f32 = 800.0;
/// The low band and the body fall back this slowly before a plosive is judged, for the same
/// swing.
const PLO_SMOOTH_MS: f32 = 5.0;
/// The cuts fall back this fast.
const SIB_RELEASE_MS: f32 = 35.0;
const PLO_RELEASE_MS: f32 = 90.0;
/// Until a vowel has been heard there is no voice to measure a sibilant against: the voice is
/// taken to be this far over the sibilant itself, dB.
const UNKNOWN_VOICE_OVER_DB: f32 = 6.0;
/// Where the references start, dB.
const REF_START_DB: f32 = -60.0;
/// Power below this is silence.
const TINY: f32 = 1e-20;
const LN10_OVER_20: f32 = std::f32::consts::LN_10 / 20.0;

/// What Listen plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Listen {
    /// The treated voice.
    #[default]
    Off,
    /// Only what was found to be sibilance (before any cut).
    Sibilance,
    /// Only what was found to be a plosive (before any cut).
    Plosives,
    /// Only what is being removed.
    Removed,
}

/// What the knobs say, in the engine's units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// 0..1: how firmly sibilance is held down.
    pub sib_amount: f32,
    /// The most taken off a sibilant, dB.
    pub sib_range_db: f32,
    /// The sibilant band, Hz.
    pub sib_lo_hz: f32,
    pub sib_hi_hz: f32,
    /// The cut reaches the whole signal, not only the sibilant band.
    pub wide: bool,
    /// 0..1: how readily a thump counts as a plosive.
    pub plo_amount: f32,
    /// The most taken off a plosive, dB.
    pub plo_range_db: f32,
    /// The top of the low band, Hz.
    pub plo_hi_hz: f32,
    pub listen: Listen,
    /// Pass the input through untouched (still a frame late).
    pub bypass: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            sib_amount: 0.0,
            sib_range_db: 12.0,
            sib_lo_hz: 4_500.0,
            sib_hi_hz: 14_000.0,
            wide: false,
            plo_amount: 0.0,
            plo_range_db: 18.0,
            plo_hi_hz: 180.0,
            listen: Listen::Off,
            bypass: false,
        }
    }
}

/// The frame length for a sample rate: about 8 ms, a multiple of 8 samples.
pub fn frame_len(sample_rate: f64) -> usize {
    let n = (sample_rate * FRAME_S / 8.0).round() as usize * 8;
    n.clamp(64, 4096)
}

fn pow_db(p: f32) -> f32 {
    10.0 * p.max(TINY).log10()
}

/// 0 at or under `lo`, 1 at or over `hi`, smooth between.
fn smoothstep(x: f32, lo: f32, hi: f32) -> f32 {
    if hi <= lo {
        return if x >= hi { 1.0 } else { 0.0 };
    }
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A one-pole step's coefficient for a time constant, at the frame rate.
fn frame_coef(ms: f32, frames_per_sec: f32) -> f32 {
    if ms <= 0.0 || frames_per_sec <= 0.0 { 0.0 } else { (-1000.0 / (ms * frames_per_sec)).exp() }
}

/// What the engine found and did in the frames since [`SiftDsp::take_meters`] was last called
/// (the largest of each).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Meters {
    /// dB taken off the sibilant band and off the low band (non-negative).
    pub sib_cut_db: f32,
    pub plo_cut_db: f32,
    /// How sure it was of a sibilant and of a plosive, 0..1.
    pub sibilant: f32,
    pub plosive: f32,
}

/// The engine.
pub struct SiftDsp {
    set: Settings,
    sr: f64,
    n: usize,
    hop: usize,
    channels: usize,
    fft: Option<Arc<dyn Fft<f32>>>,
    ifft: Option<Arc<dyn Fft<f32>>>,
    scratch: Vec<Complex<f32>>,
    buf: Vec<Complex<f32>>,
    window: Vec<f32>,
    /// The last N input samples per channel and the output being overlap-added, circular on `pos`.
    inp: [Vec<f32>; 2],
    ola: [Vec<f32>; 2],
    pos: usize,
    count: usize,
    /// Per bin 0..=N/2: how much it belongs to the body, the sibilant band and the low band.
    w_body: Vec<f32>,
    w_sib: Vec<f32>,
    w_plo: Vec<f32>,
    /// The body's level as it has recently been, and the low band's, dB.
    voice_ref: f32,
    low_ref: f32,
    /// A vowel has been heard since the engine was last at rest, so `voice_ref` means something.
    voice_known: bool,
    /// The cuts in force, dB, and the likelihoods of the last frame.
    sib_cut: f32,
    plo_cut: f32,
    sibilant: f32,
    plosive: f32,
    /// The low band's and the body's power, smoothed for the plosive judgement.
    low_smooth: f32,
    body_smooth: f32,
    plo_smooth: f32,
    voice_up: f32,
    voice_down: f32,
    low_up: f32,
    low_up_thump: f32,
    low_down: f32,
    sib_release: f32,
    plo_release: f32,
    meters: Meters,
}

impl Default for SiftDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl SiftDsp {
    /// An engine at rest. Unprepared: call [`SiftDsp::prepare`].
    pub fn new() -> Self {
        SiftDsp {
            set: Settings::default(),
            sr: 48_000.0,
            n: 0,
            hop: 0,
            channels: 2,
            fft: None,
            ifft: None,
            scratch: Vec::new(),
            buf: Vec::new(),
            window: Vec::new(),
            inp: [Vec::new(), Vec::new()],
            ola: [Vec::new(), Vec::new()],
            pos: 0,
            count: 0,
            w_body: Vec::new(),
            w_sib: Vec::new(),
            w_plo: Vec::new(),
            voice_ref: REF_START_DB,
            low_ref: REF_START_DB,
            voice_known: false,
            sib_cut: 0.0,
            plo_cut: 0.0,
            sibilant: 0.0,
            plosive: 0.0,
            low_smooth: 0.0,
            body_smooth: 0.0,
            plo_smooth: 0.0,
            voice_up: 0.0,
            voice_down: 0.0,
            low_up: 0.0,
            low_up_thump: 0.0,
            low_down: 0.0,
            sib_release: 0.0,
            plo_release: 0.0,
            meters: Meters::default(),
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.sr
    }

    /// The latency, samples: the frame length (0 before `prepare`).
    pub fn latency_samples(&self) -> usize {
        self.n
    }

    pub fn settings(&self) -> Settings {
        self.set
    }

    /// The largest cuts and likelihoods since the last call, then forgotten.
    pub fn take_meters(&mut self) -> Meters {
        std::mem::take(&mut self.meters)
    }

    /// Sizes everything for a rate and channel count (1 or 2) and goes to rest. Allocates.
    pub fn prepare(&mut self, sample_rate: f64, channels: usize) {
        let sr = if sample_rate.is_finite() { sample_rate.clamp(8_000.0, 384_000.0) } else { 48_000.0 };
        self.sr = sr;
        self.channels = channels.clamp(1, 2);
        let n = frame_len(sr);
        self.n = n;
        self.hop = n / 8;
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);
        let ifft = planner.plan_fft_inverse(n);
        let scratch = fft.get_inplace_scratch_len().max(ifft.get_inplace_scratch_len());
        self.scratch = vec![Complex::new(0.0, 0.0); scratch];
        self.fft = Some(fft);
        self.ifft = Some(ifft);
        self.buf = vec![Complex::new(0.0, 0.0); n];
        self.window = (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect();
        for v in self.inp.iter_mut().chain(self.ola.iter_mut()) {
            *v = vec![0.0; n];
        }
        let bins = n / 2 + 1;
        self.w_body = vec![0.0; bins];
        self.w_sib = vec![0.0; bins];
        self.w_plo = vec![0.0; bins];
        let fps = (sr / self.hop.max(1) as f64) as f32;
        self.voice_up = frame_coef(VOICE_UP_MS, fps);
        self.voice_down = frame_coef(VOICE_DOWN_MS, fps);
        self.plo_smooth = frame_coef(PLO_SMOOTH_MS, fps);
        self.low_up = frame_coef(LOW_UP_MS, fps);
        self.low_up_thump = frame_coef(LOW_UP_THUMP_MS, fps);
        self.low_down = frame_coef(LOW_DOWN_MS, fps);
        self.sib_release = frame_coef(SIB_RELEASE_MS, fps);
        self.plo_release = frame_coef(PLO_RELEASE_MS, fps);
        self.update_bands();
        self.reset();
    }

    /// Back to rest: no audio in flight, no cut, nothing learned. Keeps the settings.
    pub fn reset(&mut self) {
        for v in self.inp.iter_mut().chain(self.ola.iter_mut()) {
            v.fill(0.0);
        }
        self.pos = 0;
        self.count = 0;
        self.voice_ref = REF_START_DB;
        self.low_ref = REF_START_DB;
        self.voice_known = false;
        self.low_smooth = 0.0;
        self.body_smooth = 0.0;
        self.sib_cut = 0.0;
        self.plo_cut = 0.0;
        self.sibilant = 0.0;
        self.plosive = 0.0;
        self.meters = Meters::default();
    }

    /// Takes the knobs (clamped; a non-finite value keeps its default).
    pub fn set(&mut self, s: Settings) {
        let d = Settings::default();
        let f = |v: f32, lo: f32, hi: f32, def: f32| if v.is_finite() { v.clamp(lo, hi) } else { def };
        let new = Settings {
            sib_amount: f(s.sib_amount, 0.0, 1.0, d.sib_amount),
            sib_range_db: f(s.sib_range_db, 0.0, 30.0, d.sib_range_db),
            sib_lo_hz: f(s.sib_lo_hz, 1_000.0, 20_000.0, d.sib_lo_hz),
            sib_hi_hz: f(s.sib_hi_hz, 1_000.0, 20_000.0, d.sib_hi_hz),
            wide: s.wide,
            plo_amount: f(s.plo_amount, 0.0, 1.0, d.plo_amount),
            plo_range_db: f(s.plo_range_db, 0.0, 36.0, d.plo_range_db),
            plo_hi_hz: f(s.plo_hi_hz, 40.0, 500.0, d.plo_hi_hz),
            listen: s.listen,
            bypass: s.bypass,
        };
        let bands = (new.sib_lo_hz, new.sib_hi_hz, new.plo_hi_hz) != (self.set.sib_lo_hz, self.set.sib_hi_hz, self.set.plo_hi_hz);
        self.set = new;
        if bands {
            self.update_bands();
        }
    }

    /// Each bin's share of the three bands, from the settings.
    fn update_bands(&mut self) {
        let n = self.n.max(2);
        let bin_hz = (self.sr / n as f64) as f32;
        let s = self.set;
        let (lo, hi) = if s.sib_lo_hz <= s.sib_hi_hz { (s.sib_lo_hz, s.sib_hi_hz) } else { (s.sib_hi_hz, s.sib_lo_hz) };
        // The band never closes: at least a third of an octave.
        let hi = hi.max(lo * 1.26);
        // An edge fades over a third of an octave, centred on the limit.
        let edge = |oct: f32| (0.5 + 1.5 * oct).clamp(0.0, 1.0);
        for k in 0..self.w_body.len() {
            let hz = (k as f32 * bin_hz).max(1.0);
            let body = if (BODY_LO_HZ..=BODY_HI_HZ).contains(&hz) { 1.0 } else { 0.0 };
            let sib = edge((hz / lo).log2()) * edge((hi / hz).log2());
            // The low band: everything under the ceiling, gone half an octave over it.
            let plo = if k == 0 { 1.0 } else { (1.0 - 2.0 * (hz / s.plo_hi_hz).log2()).clamp(0.0, 1.0) };
            if let (Some(b), Some(si), Some(p)) = (self.w_body.get_mut(k), self.w_sib.get_mut(k), self.w_plo.get_mut(k)) {
                *b = body;
                *si = sib;
                *p = plo;
            }
        }
    }

    /// Processes `frames` samples of `io` in place (at most the prepared channels, at most each
    /// buffer's length). The output is the input N samples late with the cuts applied.
    pub fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let n = self.n;
        if n == 0 || self.hop == 0 {
            return;
        }
        let nc = self.channels.min(io.len()).min(2);
        if nc == 0 {
            return;
        }
        let frames = io.iter().take(nc).map(Vec::len).min().unwrap_or(0).min(frames);
        for i in 0..frames {
            for c in 0..nc {
                let x = io.get(c).and_then(|v| v.get(i)).copied().unwrap_or(0.0);
                let x = if x.is_finite() { x } else { 0.0 };
                let mut y = 0.0;
                if let Some(slot) = self.inp.get_mut(c).and_then(|v| v.get_mut(self.pos)) {
                    *slot = x;
                }
                if let Some(slot) = self.ola.get_mut(c).and_then(|v| v.get_mut(self.pos)) {
                    y = *slot;
                    *slot = 0.0;
                }
                if let Some(o) = io.get_mut(c).and_then(|v| v.get_mut(i)) {
                    *o = y;
                }
            }
            self.pos += 1;
            if self.pos >= n {
                self.pos = 0;
            }
            self.count += 1;
            if self.count >= self.hop {
                self.count = 0;
                self.frame(nc);
            }
        }
    }

    /// One STFT frame: the last N samples in, their treated version overlap-added out.
    fn frame(&mut self, nc: usize) {
        let n = self.n;
        let (Some(fft), Some(ifft)) = (self.fft.clone(), self.ifft.clone()) else { return };
        // Left in the real part, right in the imaginary one (mono: imaginary zero).
        for j in 0..n {
            let idx = (self.pos + j) % n;
            let w = self.window.get(j).copied().unwrap_or(0.0);
            let l = self.inp.first().and_then(|v| v.get(idx)).copied().unwrap_or(0.0);
            let r = if nc > 1 { self.inp.get(1).and_then(|v| v.get(idx)).copied().unwrap_or(0.0) } else { 0.0 };
            if let Some(b) = self.buf.get_mut(j) {
                *b = Complex::new(l * w, r * w);
            }
        }
        fft.process_with_scratch(&mut self.buf, &mut self.scratch);
        self.judge(nc);
        // The same real gain on a bin and its mirror keeps left real and right imaginary.
        let half = n / 2;
        let s = self.set;
        let (sib_cut, plo_cut) = (self.sib_cut, self.plo_cut);
        let (sibilant, plosive) = (self.sibilant, self.plosive);
        for k in 0..=half {
            let ws = self.w_sib.get(k).copied().unwrap_or(0.0);
            let wp = self.w_plo.get(k).copied().unwrap_or(0.0);
            let reach = if s.wide { 1.0 } else { ws };
            let treated = (-(sib_cut * reach + plo_cut * wp) * LN10_OVER_20).exp();
            let g = if s.bypass {
                1.0
            } else {
                match s.listen {
                    Listen::Off => treated,
                    Listen::Sibilance => ws * sibilant,
                    Listen::Plosives => wp * plosive,
                    Listen::Removed => 1.0 - treated,
                }
            };
            if let Some(b) = self.buf.get_mut(k) {
                *b *= g;
            }
            if k > 0
                && k < half
                && let Some(b) = self.buf.get_mut(n - k)
            {
                *b *= g;
            }
        }
        ifft.process_with_scratch(&mut self.buf, &mut self.scratch);
        // Hann twice at a hop of N/8 sums to 3; the inverse FFT is unnormalised.
        let norm = 1.0 / (3.0 * n as f32);
        for j in 0..n {
            let idx = (self.pos + j) % n;
            let w = self.window.get(j).copied().unwrap_or(0.0) * norm;
            let v = self.buf.get(j).copied().unwrap_or_default();
            if let Some(o) = self.ola.first_mut().and_then(|o| o.get_mut(idx)) {
                *o += v.re * w;
            }
            if nc > 1
                && let Some(o) = self.ola.get_mut(1).and_then(|o| o.get_mut(idx))
            {
                *o += v.im * w;
            }
        }
    }

    /// Reads the frame's spectrum: what it is (sibilant, plosive, voice) and how much to take off.
    fn judge(&mut self, nc: usize) {
        let n = self.n;
        let half = n / 2;
        let s = self.set;
        // |Z[k]|² + |Z[N-k]|² is 2(|L|² + |R|²); a Hann-windowed sine of amplitude A peaks at A·N/4.
        let scale = 0.5 / nc as f32 / (n as f32 * 0.25).powi(2);
        let (mut body, mut sib, mut low, mut total) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for k in 1..half {
            let a = self.buf.get(k).map_or(0.0, |c| c.norm_sqr());
            let b = self.buf.get(n - k).map_or(0.0, |c| c.norm_sqr());
            let p = (a + b) * scale;
            if !p.is_finite() {
                continue;
            }
            total += p;
            body += p * self.w_body.get(k).copied().unwrap_or(0.0);
            sib += p * self.w_sib.get(k).copied().unwrap_or(0.0);
            low += p * self.w_plo.get(k).copied().unwrap_or(0.0);
        }
        let (body_db, sib_db) = (pow_db(body), pow_db(sib));
        let heard = pow_db(total) > GATE_DB;
        let (mut want_sib, mut want_plo) = (0.0f32, 0.0f32);
        self.sibilant = 0.0;
        self.plosive = 0.0;
        if heard {
            // The references learn from what is heard.
            // A sibilant: the top level with the body or over it.
            self.sibilant = smoothstep(sib_db - body_db, SIB_OFF_DB, SIB_ON_DB);
            if self.voice_known {
                let c = if body_db > self.voice_ref { self.voice_up } else { self.voice_down };
                self.voice_ref = body_db + (self.voice_ref - body_db) * c;
            } else if self.sibilant < 0.5 {
                // The first vowel: the reference starts there.
                self.voice_ref = body_db;
                self.voice_known = true;
            }
            let voice = if self.voice_known { self.voice_ref } else { sib_db + UNKNOWN_VOICE_OVER_DB };
            let mark = voice + SIB_MARK_0_DB + (SIB_MARK_100_DB - SIB_MARK_0_DB) * s.sib_amount;
            if s.sib_amount > 0.0 {
                want_sib = self.sibilant * (SIB_RATIO * (sib_db - mark).max(0.0)).min(s.sib_range_db);
            }
            // A plosive: the low band well over the body, and jumping over where it has been.
            // (Each follows its level up at once and down over a few milliseconds: the peaks of
            // a low voice's swing, with no delay for a thump.)
            self.low_smooth = if low >= self.low_smooth { low } else { low + (self.low_smooth - low) * self.plo_smooth };
            self.body_smooth = if body >= self.body_smooth { body } else { body + (self.body_smooth - body) * self.plo_smooth };
            let (low_db, body_db) = (pow_db(self.low_smooth), pow_db(self.body_smooth));
            let off = PLO_OFF_DB.0 + (PLO_OFF_DB.1 - PLO_OFF_DB.0) * s.plo_amount;
            let on = PLO_ON_DB.0 + (PLO_ON_DB.1 - PLO_ON_DB.0) * s.plo_amount;
            let over = low_db - self.low_ref;
            let thump = smoothstep(low_db - body_db, off, on);
            self.plosive = thump * smoothstep(over, PLO_JUMP_DB, PLO_JUMP_FULL_DB);
            if s.plo_amount > 0.0 {
                want_plo = self.plosive * (PLO_RATIO * (over - PLO_JUMP_DB).max(0.0)).min(s.plo_range_db);
            }
            if low_db < self.low_ref {
                self.low_ref = low_db + (self.low_ref - low_db) * self.low_down;
            } else {
                let c = if thump < 0.25 { self.low_up } else { self.low_up_thump };
                self.low_ref = low_db + (self.low_ref - low_db) * c;
            }
        }
        let (want_sib, want_plo) = (if want_sib.is_finite() { want_sib } else { 0.0 }, if want_plo.is_finite() { want_plo } else { 0.0 });
        // A cut rises within the frame and falls back with its release.
        self.sib_cut = if want_sib >= self.sib_cut { want_sib } else { want_sib + (self.sib_cut - want_sib) * self.sib_release };
        self.plo_cut = if want_plo >= self.plo_cut { want_plo } else { want_plo + (self.plo_cut - want_plo) * self.plo_release };
        if self.sib_cut < 1e-4 {
            self.sib_cut = 0.0;
        }
        if self.plo_cut < 1e-4 {
            self.plo_cut = 0.0;
        }
        let m = &mut self.meters;
        m.sib_cut_db = m.sib_cut_db.max(self.sib_cut);
        m.plo_cut_db = m.plo_cut_db.max(self.plo_cut);
        m.sibilant = m.sibilant.max(self.sibilant);
        m.plosive = m.plosive.max(self.plosive);
    }
}
