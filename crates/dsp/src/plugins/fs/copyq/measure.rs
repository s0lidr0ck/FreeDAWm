//! Measuring a sound for CopyQ: from FFT frames to a [`Profile`].
//!
//! - [`BandMap`] gathers a frame's power per FFT bin into the grid's twelfth-octave bands. A bin
//!   stands for the power between its neighbours' halfway points, and a band takes the share of
//!   each bin that lies inside its edges, so a band narrower than a bin (the low end) gets its
//!   share of that bin and the result does not depend on the frame length or the sample rate.
//! - [`Meter`] averages the frames it accepts, three ways at once: all of them, the loud ones
//!   ([`LEVEL_MARGIN_DB`] or more over the mean level so far) and the quiet ones (as far under
//!   it); a frame in between is neither, so a sound that holds one level has no loud or quiet
//!   version. It averages either everything since the last reset (a capture) or over a window
//!   (following a track).
//! - Silence and bleed are left out: a frame is skipped when it is under [`ABS_GATE_DB`], or more
//!   than the gate under the programme level (the frame level, which rises at once and falls
//!   [`PROGRAMME_FALL_DB_PER_S`] a second, so a tail or the spill between hits is not measured
//!   but a passage that really is quieter is, a few seconds in).
//! - [`measure_audio`] runs all of it over a recording (a file, a clip), off the audio thread.

use super::profile::{ALL, CH, CURVES, Curve, FLOOR_DB, GRID_N, LEFT, LOUD, LV, MID, PER_OCTAVE, Profile, QUIET, RIGHT, SIDE, SUM, curve_index, grid_hz};
use rustfft::{FftPlanner, num_complex::Complex};

/// A frame under this level is never measured, dB.
pub const ABS_GATE_DB: f32 = -70.0;
/// A frame is loud this far over the mean level, quiet this far under it, and neither in
/// between, dB.
pub const LEVEL_MARGIN_DB: f32 = 3.0;
/// How fast the programme level falls, dB per second.
pub const PROGRAMME_FALL_DB_PER_S: f32 = 4.0;
const TINY: f32 = 1.0e-20;

/// The STFT frame length CopyQ works in at a sample rate.
pub fn frame_len(sample_rate: f64) -> usize {
    if sample_rate > 100_000.0 {
        16_384
    } else if sample_rate > 50_000.0 {
        8_192
    } else {
        4_096
    }
}

/// Power to dB, floored.
pub fn pow_db(p: f32) -> f32 {
    if p.is_finite() && p > TINY { (10.0 * p.log10()).max(FLOOR_DB) } else { FLOOR_DB }
}

/// Which FFT bins make up each grid band. See the module docs.
#[derive(Debug, Clone, Default)]
pub struct BandMap {
    first: Vec<u32>,
    last: Vec<u32>,
    w_first: Vec<f32>,
    w_last: Vec<f32>,
    /// Whether the band lies under the Nyquist frequency.
    valid: Vec<bool>,
    bins: usize,
}

impl BandMap {
    /// The map for frames of `n` samples at `sample_rate`. Allocates.
    pub fn new(n: usize, sample_rate: f64) -> Self {
        let n = n.max(8);
        let half = n / 2;
        let df = (sample_rate.max(1.0) / n as f64) as f32;
        let edge = 2f32.powf(0.5 / PER_OCTAVE as f32);
        let mut m = BandMap { bins: half, ..BandMap::default() };
        for i in 0..GRID_N {
            let f = grid_hz(i);
            let (lo, hi) = (f / edge, f * edge);
            let top = (half.saturating_sub(1)).max(1) as f32;
            let k0 = (lo / df + 0.5).floor().clamp(1.0, top);
            let k1 = (hi / df + 0.5).floor().clamp(1.0, top);
            let (w0, w1) = if k0 >= k1 { ((hi - lo) / df, 0.0) } else { (((k0 + 0.5) * df - lo) / df, (hi - (k1 - 0.5) * df) / df) };
            m.first.push(k0 as u32);
            m.last.push(k1 as u32);
            m.w_first.push(w0.clamp(0.0, 1.0));
            m.w_last.push(w1.clamp(0.0, 1.0));
            m.valid.push(f64::from(hi) < sample_rate * 0.5);
        }
        m
    }

    /// Whether grid band `i` is under the Nyquist frequency.
    pub fn valid(&self, i: usize) -> bool {
        self.valid.get(i).copied().unwrap_or(false)
    }

    /// The power in each band of a frame whose power per bin is `bins` (index 0 is DC, unused).
    pub fn gather(&self, bins: &[f32], out: &mut Curve) {
        for (i, o) in out.iter_mut().enumerate() {
            if !self.valid(i) {
                *o = 0.0;
                continue;
            }
            let k0 = self.first.get(i).copied().unwrap_or(1) as usize;
            let k1 = self.last.get(i).copied().unwrap_or(1) as usize;
            let mut sum = bins.get(k0).copied().unwrap_or(0.0) * self.w_first.get(i).copied().unwrap_or(0.0);
            if k1 > k0 {
                sum += bins.get(k0 + 1..k1).map_or(0.0, |s| s.iter().sum::<f32>());
                sum += bins.get(k1).copied().unwrap_or(0.0) * self.w_last.get(i).copied().unwrap_or(0.0);
            }
            *o = if sum.is_finite() { sum.max(0.0) } else { 0.0 };
        }
    }

    /// Bins the map was made for (half the frame).
    pub fn bins(&self) -> usize {
        self.bins
    }
}

/// How a [`Meter`] averages.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeterCfg {
    /// The averaging window in frames; 0 averages everything since the last reset.
    pub window_frames: f32,
    /// Frames this far under the programme level are skipped, dB.
    pub gate_db: f32,
    /// Frames per second (for the programme level's fall and the seconds counted).
    pub frames_per_sec: f32,
}

/// The running measurement. Plain data: making one allocates its curves, nothing else does.
#[derive(Debug, Clone)]
pub struct Meter {
    /// Mean power per band, by [`curve_index`].
    acc: Box<[Curve; CURVES]>,
    /// Frames averaged per level.
    count: [f32; LV],
    /// Mean frame level per level, dB.
    level: [f32; LV],
    /// Seconds of sound per level.
    seconds: [f32; LV],
    programme: f32,
    /// The level of the last frame given, accepted or not, dB.
    last_level: f32,
}

impl Default for Meter {
    fn default() -> Self {
        Self::new()
    }
}

impl Meter {
    pub fn new() -> Self {
        Meter { acc: Box::new([[0.0; GRID_N]; CURVES]), count: [0.0; LV], level: [FLOOR_DB; LV], seconds: [0.0; LV], programme: FLOOR_DB, last_level: FLOOR_DB }
    }

    /// Forgets everything measured.
    pub fn reset(&mut self) {
        for c in self.acc.iter_mut() {
            c.fill(0.0);
        }
        self.count = [0.0; LV];
        self.level = [FLOOR_DB; LV];
        self.seconds = [0.0; LV];
        self.programme = FLOOR_DB;
        self.last_level = FLOOR_DB;
    }

    /// Takes one frame: the power per band of each view (`[SUM, LEFT, RIGHT, MID, SIDE]`).
    /// `true` when it was measured, `false` when the gate left it out.
    pub fn add_frame(&mut self, frame: &[Curve; CH], cfg: MeterCfg) -> bool {
        let total: f32 = frame.first().map_or(0.0, |c| c.iter().sum());
        let level = pow_db(total);
        self.last_level = level;
        let fps = if cfg.frames_per_sec.is_finite() { cfg.frames_per_sec.max(1.0) } else { 50.0 };
        self.programme = level.max(self.programme - PROGRAMME_FALL_DB_PER_S / fps);
        let gate = if cfg.gate_db.is_finite() { cfg.gate_db.clamp(1.0, 120.0) } else { 24.0 };
        if level < ABS_GATE_DB || level < self.programme - gate {
            return false;
        }
        let window = if cfg.window_frames.is_finite() { cfg.window_frames.max(0.0) } else { 0.0 };
        // Loud or quiet against the mean so far; the first frame, and any frame near the mean, is
        // neither.
        let mean = self.level.first().copied().unwrap_or(FLOOR_DB);
        let class = if self.count.first().copied().unwrap_or(0.0) < 1.0 {
            None
        } else if level >= mean + LEVEL_MARGIN_DB {
            Some(LOUD)
        } else if level <= mean - LEVEL_MARGIN_DB {
            Some(QUIET)
        } else {
            None
        };
        for lv in [Some(ALL), class].into_iter().flatten() {
            let Some(n) = self.count.get_mut(lv) else { continue };
            *n = if window > 0.0 { (*n + 1.0).min(window) } else { *n + 1.0 };
            let a = 1.0 / n.max(1.0);
            if let Some(l) = self.level.get_mut(lv) {
                *l = if *n <= 1.0 { level } else { *l + (level - *l) * a };
            }
            if let Some(s) = self.seconds.get_mut(lv) {
                *s = if window > 0.0 { (*s + 1.0 / fps).min(window / fps) } else { *s + 1.0 / fps };
            }
            for (ch, src) in frame.iter().enumerate() {
                if let Some(dst) = self.acc.get_mut(curve_index(lv, ch)) {
                    for (d, s) in dst.iter_mut().zip(src) {
                        *d += (*s - *d) * a;
                    }
                }
            }
        }
        true
    }

    /// The level of the last frame given, dB.
    pub fn last_level(&self) -> f32 {
        self.last_level
    }

    pub fn seconds(&self) -> [f32; LV] {
        self.seconds
    }

    pub fn level(&self) -> [f32; LV] {
        self.level
    }

    /// One curve in dB into `out`.
    pub fn curve_db(&self, index: usize, out: &mut Curve) {
        match self.acc.get(index) {
            Some(c) => {
                for (o, p) in out.iter_mut().zip(c) {
                    *o = pow_db(*p);
                }
            }
            None => out.fill(FLOOR_DB),
        }
    }

    /// The measurement as a profile (allocates: not for the audio thread).
    pub fn profile(&self, name: &str, stereo: bool) -> Profile {
        let mut p = Profile::silent();
        p.name = super::profile::clean_name(name);
        for (i, c) in p.curves.iter_mut().enumerate() {
            self.curve_db(i, c);
        }
        p.level = self.level;
        p.seconds = self.seconds;
        p.stereo = stereo;
        p
    }
}

/// The power per bin of the views of one frame, from the spectrum of `left + j·right` (`mono`:
/// the imaginary part was zero). `scale` turns a squared magnitude into power. Each output is
/// `half + 1` long at most; index 0 (DC) is left at 0.
pub fn bin_powers(spectrum: &[Complex<f32>], mono: bool, scale: f32, left: &mut [f32], right: &mut [f32], mid: &mut [f32], side: &mut [f32]) {
    let n = spectrum.len();
    let half = n / 2;
    for k in 1..half {
        let (Some(zk), Some(zm)) = (spectrum.get(k).copied(), spectrum.get(n - k).copied()) else { break };
        let (l, r) = split_lr(zk, zm);
        let (l, r) = if mono { (l, l) } else { (l, r) };
        let (m, s) = ((l + r) * 0.5, (l - r) * 0.5);
        for (dst, v) in [(&mut *left, l), (&mut *right, r), (&mut *mid, m), (&mut *side, s)] {
            if let Some(d) = dst.get_mut(k) {
                *d = v.norm_sqr() * scale;
            }
        }
    }
}

/// The left and right spectra at a bin, from `Z = L + jR` at that bin (`zk`) and at its mirror
/// (`zm`): `L = (Z[k] + conj Z[N-k]) / 2`, `R = (Z[k] - conj Z[N-k]) / 2j`.
#[inline]
pub fn split_lr(zk: Complex<f32>, zm: Complex<f32>) -> (Complex<f32>, Complex<f32>) {
    let c = zm.conj();
    let l = (zk + c) * 0.5;
    let d = (zk - c) * 0.5;
    (l, Complex::new(d.im, -d.re))
}

/// What turns a Hann-windowed frame's squared bin magnitude into power: the bins of one side
/// then add up to the signal's mean square.
pub fn power_scale(n: usize) -> f32 {
    // The Hann window's squares add up to 3N/8.
    let n = n.max(1) as f32;
    2.0 / (n * n * 0.375)
}

/// A Hann window of `n` points.
pub fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n.max(1) as f32).cos()).collect()
}

/// Gathers the four per-bin powers of a frame into the five views' band powers (the sum is the
/// mean of left and right).
pub fn gather_views(map: &BandMap, left: &[f32], right: &[f32], mid: &[f32], side: &[f32], out: &mut [Curve; CH]) {
    for (ch, bins) in [(LEFT, left), (RIGHT, right), (MID, mid), (SIDE, side)] {
        if let Some(c) = out.get_mut(ch) {
            map.gather(bins, c);
        }
    }
    let (l, r) = (out.get(LEFT).copied().unwrap_or([0.0; GRID_N]), out.get(RIGHT).copied().unwrap_or([0.0; GRID_N]));
    if let Some(sum) = out.get_mut(SUM) {
        for ((s, a), b) in sum.iter_mut().zip(&l).zip(&r) {
            *s = (a + b) * 0.5;
        }
    }
}

/// The longest recording [`measure_audio`] reads, in frames per channel (an hour at 192 kHz).
pub const MAX_MEASURE_FRAMES: usize = 192_000 * 3_600;

/// Measures a whole recording: `channels` planar (one: mono; two or more: the first two as left
/// and right), from frame `start` for `len` frames. `gate_db` as [`MeterCfg::gate_db`]. Every
/// accepted frame counts the same. Allocates; never for the audio thread. A recording too short
/// for one frame, or silent, gives a profile that is not measured ([`Profile::is_measured`]).
pub fn measure_audio(channels: &[&[f32]], sample_rate: f64, start: usize, len: usize, gate_db: f32, name: &str) -> Profile {
    let sr = if sample_rate.is_finite() { sample_rate.clamp(8_000.0, 384_000.0) } else { 48_000.0 };
    let Some(first) = channels.first() else { return Profile::silent() };
    let stereo = channels.len() > 1;
    let second = channels.get(1).copied().unwrap_or(first);
    let avail = first.len().min(second.len());
    let start = start.min(avail);
    let end = start.saturating_add(len.min(MAX_MEASURE_FRAMES)).min(avail);
    let n = frame_len(sr);
    let hop = n / 4;
    let map = BandMap::new(n, sr);
    let window = hann(n);
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let mut scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
    let mut buf = vec![Complex::new(0.0, 0.0); n];
    let mut pw = vec![vec![0.0f32; n / 2 + 1]; 4];
    let mut views = [[0.0f32; GRID_N]; CH];
    let mut meter = Meter::new();
    let cfg = MeterCfg { window_frames: 0.0, gate_db, frames_per_sec: (sr / hop as f64) as f32 };
    let scale = power_scale(n);
    let mut at = start;
    while at.saturating_add(n) <= end {
        for (j, b) in buf.iter_mut().enumerate() {
            let w = window.get(j).copied().unwrap_or(0.0);
            let sample = |c: &[f32]| c.get(at + j).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
            *b = Complex::new(sample(first) * w, if stereo { sample(second) * w } else { 0.0 });
        }
        fft.process_with_scratch(&mut buf, &mut scratch);
        if let [l, r, m, s] = pw.as_mut_slice() {
            bin_powers(&buf, !stereo, scale, l, r, m, s);
            gather_views(&map, l, r, m, s, &mut views);
        }
        meter.add_frame(&views, cfg);
        at += hop;
    }
    meter.profile(name, stereo)
}
