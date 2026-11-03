//! Offline (Process menu) whole-buffer processing on planar `f32` audio.
//!
//! These functions may allocate (they are not for the audio thread) but never panic: lengths,
//! rates and ratios are sanitized and output sizes are capped at [`MAX_OFFLINE_SAMPLES`] per
//! channel.

use std::f64::consts::PI;

use rustfft::{FftPlanner, num_complex::Complex};

use crate::osc::Osc;
pub use crate::osc::Waveform;
use crate::util::sane_sr;
use crate::{Plugin, db_to_gain, gain_to_db};

/// Largest number of samples per channel any offline function will produce.
pub const MAX_OFFLINE_SAMPLES: usize = 1 << 28;

/// Fade curves. [`FadeShape::gain`] is the fade-in curve; a fade-out uses `gain(1 - t)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum FadeShape {
    Linear,
    /// Constant power (sin/cos), for crossfades between uncorrelated material.
    #[default]
    EqualPower,
    /// Raised cosine.
    SCurve,
    /// Slow start, fast finish.
    Exponential,
    /// Fast start, slow finish.
    Logarithmic,
}

impl FadeShape {
    pub const ALL: [FadeShape; 5] = [FadeShape::Linear, FadeShape::EqualPower, FadeShape::SCurve, FadeShape::Exponential, FadeShape::Logarithmic];

    /// Fade-in gain at position `t` in 0..1 (0 → silent, 1 → unity). NaN is treated as 0.
    pub fn gain(self, t: f32) -> f32 {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        const K: f32 = 4.0;
        match self {
            FadeShape::Linear => t,
            FadeShape::EqualPower => (t * std::f32::consts::FRAC_PI_2).sin(),
            FadeShape::SCurve => 0.5 - 0.5 * (t * std::f32::consts::PI).cos(),
            FadeShape::Exponential => ((K * t).exp() - 1.0) / (K.exp() - 1.0),
            FadeShape::Logarithmic => (1.0 + (K.exp() - 1.0) * t).ln() / K,
        }
    }
}

fn max_len(ch: &[Vec<f32>]) -> usize {
    ch.iter().map(Vec::len).max().unwrap_or(0)
}

/// The largest finite sample magnitude in any channel.
pub fn peak(ch: &[Vec<f32>]) -> f32 {
    ch.iter().flatten().filter(|v| v.is_finite()).fold(0.0f32, |m, v| m.max(v.abs()))
}

/// Peak-normalizes to `target_db` dBFS, either all channels together or each on its own.
/// Silent channels are left alone.
pub fn normalize(ch: &mut [Vec<f32>], target_db: f32, per_channel: bool) {
    let target = db_to_gain(if target_db.is_nan() { 0.0 } else { target_db.clamp(-144.0, 24.0) });
    if per_channel {
        for c in ch.iter_mut() {
            let p = peak(std::slice::from_ref(c));
            if p > 0.0 {
                let g = target / p;
                c.iter_mut().for_each(|s| *s *= g);
            }
        }
    } else {
        let p = peak(ch);
        if p > 0.0 {
            let g = target / p;
            ch.iter_mut().flatten().for_each(|s| *s *= g);
        }
    }
}

/// Applies a gain change in dB.
pub fn gain(ch: &mut [Vec<f32>], db: f32) {
    let g = db_to_gain(if db.is_nan() { 0.0 } else { db.min(96.0) });
    ch.iter_mut().flatten().for_each(|s| *s *= g);
}

/// Flips polarity.
pub fn invert(ch: &mut [Vec<f32>]) {
    ch.iter_mut().flatten().for_each(|s| *s = -*s);
}

/// Reverses each channel in time.
pub fn reverse(ch: &mut [Vec<f32>]) {
    ch.iter_mut().for_each(|c| c.reverse());
}

/// Removes each channel's DC offset (subtracts its mean).
pub fn remove_dc(ch: &mut [Vec<f32>]) {
    for c in ch.iter_mut() {
        if c.is_empty() {
            continue;
        }
        let mean = c.iter().filter(|v| v.is_finite()).map(|&v| f64::from(v)).sum::<f64>() / c.len() as f64;
        let m = mean as f32;
        c.iter_mut().for_each(|s| *s -= m);
    }
}

/// Fades in over the first `fade_in` samples and out over the last `fade_out` samples.
pub fn fade(ch: &mut [Vec<f32>], fade_in: usize, fade_out: usize, shape: FadeShape) {
    for c in ch.iter_mut() {
        let len = c.len();
        let fi = fade_in.min(len);
        for (i, s) in c.iter_mut().take(fi).enumerate() {
            *s *= shape.gain(i as f32 / fi as f32);
        }
        let fo = fade_out.min(len);
        for (j, s) in c.iter_mut().skip(len - fo).enumerate() {
            *s *= shape.gain((fo - 1 - j) as f32 / fo as f32);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Resampling (Kaiser-windowed sinc)

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let y = x * x / 4.0;
    for k in 1..64 {
        term *= y / (k as f64 * k as f64);
        sum += term;
        if term < sum * 1e-12 {
            break;
        }
    }
    sum
}

const SINC_ZC: f64 = 32.0;
const SINC_RES: usize = 512;

/// Precomputed one-sided windowed-sinc kernel for a given cutoff (fraction of input Nyquist).
struct SincTable {
    table: Vec<f32>,
    half_width: f64,
}

impl SincTable {
    fn new(cutoff: f64) -> Self {
        let cutoff = cutoff.clamp(0.01, 1.0);
        let half_width = SINC_ZC / cutoff;
        let n = (half_width * SINC_RES as f64).ceil() as usize + 2;
        let beta = 9.0;
        let i0b = bessel_i0(beta);
        let table = (0..n)
            .map(|i| {
                let t = i as f64 / SINC_RES as f64;
                let x = t / half_width;
                if x >= 1.0 {
                    return 0.0;
                }
                let arg = PI * cutoff * t;
                let sinc = if arg.abs() < 1e-12 { 1.0 } else { arg.sin() / arg };
                let w = bessel_i0(beta * (1.0 - x * x).max(0.0).sqrt()) / i0b;
                (cutoff * sinc * w) as f32
            })
            .collect();
        Self { table, half_width }
    }

    #[inline]
    fn at(&self, t: f64) -> f32 {
        let pos = t.abs() * SINC_RES as f64;
        let i = pos as usize;
        let f = (pos - i as f64) as f32;
        match (self.table.get(i), self.table.get(i + 1)) {
            (Some(a), Some(b)) => a + (b - a) * f,
            (Some(a), None) => *a,
            _ => 0.0,
        }
    }
}

/// Resamples one channel to exactly `out_len` samples, where output sample `n` is read at input
/// time `n / ratio`.
fn resample_channel(x: &[f32], ratio: f64, out_len: usize, table: &SincTable) -> Vec<f32> {
    let len = x.len() as i64;
    let hw = table.half_width;
    (0..out_len)
        .map(|n| {
            let t = n as f64 / ratio;
            let lo = (t - hw).ceil().max(0.0) as i64;
            let hi = ((t + hw).floor() as i64).min(len - 1);
            let mut acc = 0.0f32;
            let mut j = lo;
            while j <= hi {
                if let Some(&v) = x.get(j as usize) {
                    acc += v * table.at(t - j as f64);
                }
                j += 1;
            }
            acc
        })
        .collect()
}

fn resample_ratio(ch: &[Vec<f32>], ratio: f64, out_len: usize) -> Vec<Vec<f32>> {
    if !ratio.is_finite() || ratio <= 0.0 {
        return ch.iter().map(|_| vec![0.0; out_len]).collect();
    }
    // Anti-alias below the lower of the two Nyquists, with a little transition room.
    let cutoff = if ratio < 1.0 { ratio * 0.94 } else { 0.97 };
    let table = SincTable::new(cutoff);
    ch.iter().map(|c| resample_channel(c, ratio, out_len, &table)).collect()
}

/// High-quality windowed-sinc sample-rate conversion from `from` Hz to `to` Hz.
///
/// Rates in a small integer ratio (44.1 and 48 kHz are 147 : 160) use an exact polyphase filter
/// bank: the same Kaiser-windowed sinc as the general path, evaluated exactly per phase instead of
/// read from an interpolated table. Channels run on their own threads where threads exist.
pub fn resample(ch: &[Vec<f32>], from: u32, to: u32) -> Vec<Vec<f32>> {
    if from == 0 || to == 0 || from == to {
        return ch.to_vec();
    }
    let g = gcd(from, to);
    let (up, down) = ((to / g) as usize, (from / g) as usize);
    let ratio = f64::from(to) / f64::from(from);
    let bank = (up <= MAX_PHASES).then(|| PolyBank::new(up, down, ratio));
    let one = |c: &Vec<f32>| -> Vec<f32> {
        let out_len = ((c.len() as f64 * ratio).round() as usize).min(MAX_OFFLINE_SAMPLES);
        match &bank {
            Some(b) => b.run(c, out_len),
            None => resample_ratio(std::slice::from_ref(c), ratio, out_len).into_iter().next().unwrap_or_default(),
        }
    };
    per_channel(ch, &one)
}

/// `f` on each channel, on a thread per channel when there are threads (inline otherwise, or when
/// a thread can't be started).
fn per_channel(ch: &[Vec<f32>], f: &(dyn Fn(&Vec<f32>) -> Vec<f32> + Sync)) -> Vec<Vec<f32>> {
    #[cfg(not(target_arch = "wasm32"))]
    if ch.len() > 1 {
        let done: Vec<Option<Vec<f32>>> = std::thread::scope(|sc| {
            let handles: Vec<_> =
                ch.iter().map(|c| std::thread::Builder::new().name("resample".into()).spawn_scoped(sc, move || f(c)).ok()).collect();
            handles.into_iter().map(|h| h.and_then(|h| h.join().ok())).collect()
        });
        return done.into_iter().zip(ch).map(|(d, c)| d.unwrap_or_else(|| f(c))).collect();
    }
    ch.iter().map(f).collect()
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// Most polyphase phases (the reduced output factor) before the table path is used instead.
const MAX_PHASES: usize = 4096;

/// A polyphase filter bank: output sample `n` is read at input time `n * down / up`.
struct PolyBank {
    up: usize,
    down: usize,
    /// Per phase: the first tap's offset from `floor(t)`, and the taps.
    phases: Vec<(i64, Vec<f32>)>,
}

impl PolyBank {
    fn new(up: usize, down: usize, ratio: f64) -> Self {
        let cutoff = (if ratio < 1.0 { ratio * 0.94 } else { 0.97 }).clamp(0.01, 1.0);
        let hw = SINC_ZC / cutoff;
        let beta = 9.0;
        let i0b = bessel_i0(beta);
        let kernel = |t: f64| -> f64 {
            let x = t.abs() / hw;
            if x >= 1.0 {
                return 0.0;
            }
            let arg = PI * cutoff * t;
            let sinc = if arg.abs() < 1e-12 { 1.0 } else { arg.sin() / arg };
            cutoff * sinc * bessel_i0(beta * (1.0 - x * x).max(0.0).sqrt()) / i0b
        };
        let phases = (0..up)
            .map(|p| {
                let frac = p as f64 / up as f64;
                let k0 = (frac - hw).ceil() as i64;
                let k1 = (frac + hw).floor() as i64;
                (k0, (k0..=k1).map(|k| kernel(frac - k as f64) as f32).collect())
            })
            .collect();
        PolyBank { up, down, phases }
    }

    fn run(&self, x: &[f32], out_len: usize) -> Vec<f32> {
        let len = x.len() as i64;
        let mut out = Vec::with_capacity(out_len);
        let (mut base, mut p) = (0i64, 0usize);
        for _ in 0..out_len {
            let (k0, taps) = match self.phases.get(p) {
                Some(ph) => (ph.0, ph.1.as_slice()),
                None => (0, &[][..]),
            };
            let j0 = base + k0;
            let j1 = j0 + taps.len() as i64;
            let acc = if j0 >= 0 && j1 <= len {
                x.get(j0 as usize..j1 as usize).unwrap_or_default().iter().zip(taps).map(|(a, b)| a * b).sum::<f32>()
            } else {
                let mut acc = 0.0f32;
                for (i, w) in taps.iter().enumerate() {
                    let j = j0 + i as i64;
                    if j >= 0
                        && let Some(v) = x.get(j as usize)
                    {
                        acc += v * w;
                    }
                }
                acc
            };
            out.push(acc);
            // Advance the input time by down/up.
            p += self.down;
            base += (p / self.up) as i64;
            p %= self.up;
        }
        out
    }
}

/// The general table-interpolated resampler (what [`resample`] used for every ratio before the
/// polyphase path; kept for odd ratios and as the reference in tests).
pub fn resample_table(ch: &[Vec<f32>], from: u32, to: u32) -> Vec<Vec<f32>> {
    if from == 0 || to == 0 || from == to {
        return ch.to_vec();
    }
    let ratio = f64::from(to) / f64::from(from);
    ch.iter()
        .map(|c| {
            let out_len = ((c.len() as f64 * ratio).round() as usize).min(MAX_OFFLINE_SAMPLES);
            resample_ratio(std::slice::from_ref(c), ratio, out_len).into_iter().next().unwrap_or_default()
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Time stretch (WSOLA) and pitch shift

/// Pitch-preserving time stretch by WSOLA (waveform-similarity overlap-add).
/// `ratio` = output length / input length, clamped to 0.1 ..= 10. All channels share the same
/// segment choices so the stereo image stays coherent.
pub fn time_stretch(ch: &[Vec<f32>], ratio: f64, sample_rate: f32) -> Vec<Vec<f32>> {
    let len = max_len(ch);
    let ratio = if ratio.is_finite() { ratio.clamp(0.1, 10.0) } else { 1.0 };
    let out_len = ((len as f64 * ratio).round() as usize).min(MAX_OFFLINE_SAMPLES);
    if len == 0 || ch.is_empty() {
        return ch.iter().map(|_| vec![0.0; out_len]).collect();
    }
    let nch = ch.len() as f32;
    let guide: Vec<f32> = (0..len).map(|i| ch.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() / nch).collect();
    let plan = wsola_plan(&guide, ratio, sample_rate);
    wsola_apply(ch, &plan)
}

/// The splice choices of a WSOLA stretch: where each output frame reads its input. Made once from a
/// guide signal and applied to any number of signals of the guide's length ([`wsola_apply`]), so
/// they all get exactly the same stretch (phase-coherent multitrack: a drum kit's mics).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsolaPlan {
    /// Input start of each output frame (output frame `k` starts at `k * hop`).
    pub reads: Vec<i64>,
    pub frame: usize,
    pub hop: usize,
    pub out_len: usize,
    /// No stretch: the input is copied.
    pub identity: bool,
}

/// Plan a stretch of `guide` by `ratio` (output / input length, clamped to 0.1 ..= 10).
pub fn wsola_plan(guide: &[f32], ratio: f64, sample_rate: f32) -> WsolaPlan {
    let len = guide.len();
    let ratio = if ratio.is_finite() { ratio.clamp(0.1, 10.0) } else { 1.0 };
    let out_len = ((len as f64 * ratio).round() as usize).min(MAX_OFFLINE_SAMPLES);
    let sr = f64::from(sane_sr(sample_rate));
    let frame = (((sr * 0.04) as usize) & !1).clamp(64, 8192);
    let hop = frame / 2;
    if len == 0 || (ratio - 1.0).abs() < 1e-9 {
        return WsolaPlan { reads: Vec::new(), frame, hop, out_len, identity: true };
    }
    let tol = ((sr * 0.010) as usize).clamp(8, 2048) as i64;
    let g = |i: i64| -> f32 { if i < 0 { 0.0 } else { guide.get(i as usize).copied().filter(|v| v.is_finite()).unwrap_or(0.0) } };
    let mut reads = Vec::with_capacity(out_len / hop.max(1) + 1);
    let mut prev_in: i64 = 0;
    let mut k = 0usize;
    loop {
        let out_pos = k * hop;
        if out_pos >= out_len {
            break;
        }
        let in_pos = if k == 0 {
            0
        } else {
            let nominal = (out_pos as f64 / ratio).round() as i64;
            let natural = prev_in + hop as i64;
            // Similarity of candidate start `p` with the natural continuation (stride 2).
            let score = |p: i64| -> f32 {
                let (mut xy, mut yy) = (0.0f32, 1e-9f32);
                let mut i = 0i64;
                while i < hop as i64 {
                    let a = g(natural + i);
                    let b = g(p + i);
                    xy += a * b;
                    yy += b * b;
                    i += 2;
                }
                xy / yy.sqrt()
            };
            let lo = (nominal - tol).max(0);
            let hi = nominal + tol;
            let mut best = nominal.max(0);
            let mut best_s = f32::NEG_INFINITY;
            let mut p = lo;
            while p <= hi {
                let s = score(p);
                if s > best_s {
                    best_s = s;
                    best = p;
                }
                p += 4;
            }
            let (rlo, rhi) = ((best - 3).max(lo), (best + 3).min(hi));
            for p in rlo..=rhi {
                let s = score(p);
                if s > best_s {
                    best_s = s;
                    best = p;
                }
            }
            best
        };
        reads.push(in_pos);
        prev_in = in_pos;
        k += 1;
    }
    WsolaPlan { reads, frame, hop, out_len, identity: false }
}

/// Stretch `ch` by `plan` (made from a guide of the same length).
pub fn wsola_apply(ch: &[Vec<f32>], plan: &WsolaPlan) -> Vec<Vec<f32>> {
    let out_len = plan.out_len;
    if plan.identity {
        return ch.iter().map(|c| c.iter().copied().chain(std::iter::repeat(0.0)).take(out_len).collect()).collect();
    }
    let frame = plan.frame;
    let window: Vec<f32> = (0..frame).map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / frame as f64).cos()) as f32).collect();
    let buf_len = out_len + frame;
    let mut out: Vec<Vec<f32>> = ch.iter().map(|_| vec![0.0; buf_len]).collect();
    let mut wsum = vec![0.0f32; buf_len];
    for (k, in_pos) in plan.reads.iter().enumerate() {
        let out_pos = k * plan.hop;
        for (c, o) in ch.iter().zip(out.iter_mut()) {
            for (i, w) in window.iter().enumerate() {
                let src = usize::try_from(*in_pos).unwrap_or(0) + i;
                let v = c.get(src).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
                if let Some(d) = o.get_mut(out_pos + i) {
                    *d += v * w;
                }
            }
        }
        for (i, w) in window.iter().enumerate() {
            if let Some(d) = wsum.get_mut(out_pos + i) {
                *d += w;
            }
        }
    }
    // Every position a window covers is normalized, however little: a floor as high as 1e-3 left
    // the first ~20 samples (under the first window's foot) near silent, then stepped.
    for o in out.iter_mut() {
        for (s, w) in o.iter_mut().zip(&wsum) {
            if *w > 1e-12 {
                *s /= w;
            }
        }
        o.truncate(out_len);
    }
    out
}

/// Length-preserving pitch shift by `semitones` (clamped to ±48): WSOLA stretch followed by
/// windowed-sinc resampling back to the original length.
pub fn pitch_shift(ch: &[Vec<f32>], semitones: f32, sample_rate: f32) -> Vec<Vec<f32>> {
    let st = if semitones.is_finite() { semitones.clamp(-48.0, 48.0) } else { 0.0 };
    if st.abs() < 1e-4 {
        return ch.to_vec();
    }
    let factor = 2f64.powf(f64::from(st) / 12.0);
    // time_stretch clamps to 0.1..=10, short of the 16x (or 1/16) that ±48 needs: stretch twice by
    // the square root instead.
    let stretched = if (0.1..=10.0).contains(&factor) {
        time_stretch(ch, factor, sample_rate)
    } else {
        let half = factor.sqrt();
        time_stretch(&time_stretch(ch, half, sample_rate), half, sample_rate)
    };
    let len = max_len(ch);
    let slen = max_len(&stretched);
    if slen == 0 || len == 0 {
        return ch.iter().map(|c| vec![0.0; c.len()]).collect();
    }
    let ratio = len as f64 / slen as f64;
    let mut out = resample_ratio(&stretched, ratio, len);
    for (o, c) in out.iter_mut().zip(ch) {
        o.truncate(c.len());
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Generators and analysis

/// Generates a test signal. Periodic waveforms peak at `level_db`; noise is scaled so its peak
/// stays at or below `level_db`.
pub fn signal_generator(kind: Waveform, freq: f32, level_db: f32, sample_rate: f32, frames: usize, channels: usize) -> Vec<Vec<f32>> {
    let sr = sane_sr(sample_rate);
    let frames = frames.min(MAX_OFFLINE_SAMPLES);
    let channels = channels.min(crate::util::MAX_CHANNELS);
    let f = if freq.is_finite() { freq.clamp(0.0, sr * 0.49) } else { 1000.0 };
    let g = db_to_gain(if level_db.is_nan() { -20.0 } else { level_db.min(0.0) });
    let mut osc = Osc::new(0x1234_5678);
    let mono: Vec<f32> = (0..frames).map(|_| osc.next(kind, f / sr) * g).collect();
    (0..channels).map(|_| mono.clone()).collect()
}

/// Onset detection: how much a log-magnitude spectrum on 24 bands per octave rises against the frame
/// before, each band against the loudest of its neighbours there so vibrato doesn't count
/// (vibrato-suppressed spectral flux). A peak must stand out from the flux within 0.1 s by a fixed
/// amount, not relative to the loudest hit, so quiet hits count and the result doesn't depend on where
/// the analysed audio starts. Each is placed where its attack starts. `sensitivity` 0..1 (higher finds
/// more). Returns sample positions, ascending.
pub fn detect_transients(ch: &[Vec<f32>], sample_rate: f32, sensitivity: f32) -> Vec<usize> {
    let len = max_len(ch);
    let sr = sane_sr(sample_rate);
    let sens = if sensitivity.is_finite() { sensitivity.clamp(0.0, 1.0) } else { 0.5 };
    let n = ((sr * 0.012) as usize).clamp(64, 8192).next_power_of_two();
    let hop = n / 4;
    if len < n || ch.is_empty() {
        return Vec::new();
    }
    let ch: Vec<Vec<f32>> =
        ch.iter().take(crate::util::MAX_CHANNELS).map(|c| c.iter().map(|v| if v.is_finite() { *v } else { 0.0 }).collect()).collect();
    let nch = ch.len() as f32;
    let bands = log_bands(n, sr);
    let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
    let window: Vec<f32> = (0..n).map(|i| (0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()) as f32).collect();
    // Magnitudes relative to a full-scale sine; below about -80 dB a band's changes barely count.
    let gain = 1.0e4 / (n as f32 / 4.0);
    let frames = (len - n) / hop + 1;
    let mut buf = vec![Complex::new(0.0f32, 0.0); n];
    let mut power = vec![0.0f32; n / 2 + 1];
    let mut prev: Vec<f32> = Vec::new();
    let mut flux = Vec::with_capacity(frames);
    let mut energy: Vec<f32> = Vec::with_capacity(frames);
    for m in 0..frames {
        power.iter_mut().for_each(|p| *p = 0.0);
        for c in &ch {
            for (i, b) in buf.iter_mut().enumerate() {
                *b = Complex::new(c.get(m * hop + i).copied().unwrap_or(0.0) * window.get(i).copied().unwrap_or(0.0), 0.0);
            }
            fft.process(&mut buf);
            for (p, b) in power.iter_mut().zip(&buf) {
                *p += b.norm_sqr() / nch;
            }
        }
        energy.push(power.iter().sum());
        let cur: Vec<f32> = bands
            .iter()
            .map(|band| (1.0 + gain * band.iter().map(|&(k, w)| power.get(k).map_or(0.0, |p| p.sqrt()) * w).sum::<f32>()).log10())
            .collect();
        let rise = |b: usize, v: f32| {
            (v - prev.get(b.saturating_sub(1)..(b + 2).min(prev.len())).map_or(0.0, |s| s.iter().copied().fold(0.0, f32::max))).max(0.0)
        };
        flux.push(if prev.is_empty() { 0.0 } else { cur.iter().enumerate().map(|(b, v)| rise(b, *v)).sum() });
        prev = cur;
    }
    let frames_of = |secs: f32| ((secs * sr) as usize / hop).max(1);
    let (near, before, after, gap) = (frames_of(0.03), frames_of(0.1), frames_of(0.07), frames_of(0.03));
    let delta = 2.5 * 2f32.powf((0.5 - sens) * 4.0);
    let mut out: Vec<usize> = Vec::new();
    let mut last: Option<usize> = None;
    for (m, &cur) in flux.iter().enumerate() {
        let span = |a: usize, b: usize| flux.get(m.saturating_sub(a)..(m + b + 1).min(frames)).unwrap_or(&[]);
        let peak = span(near, near).iter().copied().fold(0.0, f32::max);
        let around = span(before, after);
        let mean = around.iter().sum::<f32>() / around.len().max(1) as f32;
        if cur <= 0.0 || cur < peak || cur < mean + delta || last.is_some_and(|l| m - l < gap) {
            continue;
        }
        let (at, contrast) = attack_start(&ch, m * hop, n, sr);
        // A fast release smears energy between a note's partials, which looks like a rise. Its
        // attack edge is weak and the energy doesn't grow, unlike a real onset's.
        let was = energy.get(m.saturating_sub(1)).copied().unwrap_or(0.0);
        let grows = energy.get(m..(m + 2).min(frames)).is_some_and(|e| e.iter().any(|v| *v > was));
        if contrast < std::f64::consts::LN_2 && !grows {
            continue;
        }
        last = Some(m);
        if out.last().is_none_or(|&p| at > p) {
            out.push(at);
        }
    }
    out
}

/// Triangular filters 24 to the octave from 30 Hz to 20 kHz over the bins of an `n`-point FFT, without
/// duplicates where bins are coarser than that; each filter's weights sum to 1.
fn log_bands(n: usize, sr: f32) -> Vec<Vec<(usize, f32)>> {
    let bin_hz = sr / n as f32;
    let top = 20_000.0f32.min(sr * 0.45);
    let mut centres: Vec<usize> = Vec::new();
    let mut f = 30.0f32;
    while f <= top {
        let k = (f / bin_hz).round() as usize;
        if centres.last().is_none_or(|&c| k > c) {
            centres.push(k);
        }
        f *= 2f32.powf(1.0 / 24.0);
    }
    centres
        .windows(3)
        .filter_map(|w| {
            let (&lo, &mid, &hi) = (w.first()?, w.get(1)?, w.get(2)?);
            let tri: Vec<(usize, f32)> = (lo..=hi)
                .map(|k| (k, if k <= mid { (k - lo) as f32 / (mid - lo).max(1) as f32 } else { (hi - k) as f32 / (hi - mid).max(1) as f32 }))
                .filter(|(_, v)| *v > 0.0)
                .collect();
            let sum: f32 = tri.iter().map(|(_, v)| v).sum();
            (sum > 0.0).then(|| tri.into_iter().map(|(k, v)| (k, v / sum)).collect())
        })
        .collect()
}

/// Where the attack found in the frame starting at `frame` begins. The log spectrum jumps as soon as the
/// window reaches an attack, so it can sit up to half a window past the frame. Find the step edge of the
/// high-passed energy, the sample where the next 3 ms gain most over the 6 ms before, then go back to
/// where the energy first crosses the midpoint (in dB) between the two.
fn attack_start(ch: &[Vec<f32>], frame: usize, n: usize, sr: f32) -> (usize, f64) {
    let (before, after, short) = (((sr * 0.006) as usize).max(1), ((sr * 0.003) as usize).max(1), ((sr * 0.0005) as usize).max(1));
    let (from, to) = (frame.saturating_sub(before), frame + n + n / 2 + after);
    let mut cum = vec![0.0f64; to - from + 1];
    for (j, i) in (from..to).enumerate() {
        let e: f32 = ch
            .iter()
            .map(|x| {
                let d = x.get(i).copied().unwrap_or(0.0) - i.checked_sub(1).and_then(|p| x.get(p)).copied().unwrap_or(0.0);
                d * d
            })
            .sum();
        let sum = cum.get(j).copied().unwrap_or(0.0) + f64::from(e);
        if let Some(c) = cum.get_mut(j + 1) {
            *c = sum;
        }
    }
    let at = |i: usize| cum.get(i.saturating_sub(from)).copied().unwrap_or(0.0);
    let mean = |a: usize, b: usize| (at(b) - at(a)) / b.saturating_sub(a).max(1) as f64;
    let contrast = |k: usize| ((mean(k, k + after) + 1e-12) / (mean(k.saturating_sub(before), k) + 1e-12)).ln();
    let k = (frame..frame + n + n / 2).max_by(|&a, &b| contrast(a).total_cmp(&contrast(b))).unwrap_or(frame);
    let level = (mean(k.saturating_sub(before), k) * mean(k, k + after)).sqrt();
    ((k.saturating_sub(after)..=k).find(|&i| mean(i, i + short) > level).unwrap_or(k), contrast(k))
}

/// Strip Silence: ranges `(start, end)` (end exclusive) of material whose peak across channels
/// exceeds `threshold_db`. Silent gaps shorter than `min_len` samples do not split a region.
/// Each region is padded by `pad_before`/`pad_after` (clamped to the buffer) and overlapping
/// regions are merged.
pub fn non_silent_ranges(ch: &[Vec<f32>], threshold_db: f32, min_len: usize, pad_before: usize, pad_after: usize) -> Vec<(usize, usize)> {
    let mut scan = SilenceScan::new(threshold_db, min_len as u64);
    scan.push(ch);
    let at = |x: u64| usize::try_from(x).unwrap_or(usize::MAX);
    scan.finish(pad_before as u64, pad_after as u64).into_iter().map(|(s, e)| (at(s), at(e))).collect()
}

/// [`non_silent_ranges`] over audio that arrives a block at a time, so material of any length can
/// be scanned without holding all of it.
pub struct SilenceScan {
    thr: f32,
    min_len: u64,
    len: u64,
    start: Option<u64>,
    last_loud: u64,
    raw: Vec<(u64, u64)>,
}

impl SilenceScan {
    pub fn new(threshold_db: f32, min_len: u64) -> Self {
        let thr = db_to_gain(if threshold_db.is_nan() { -48.0 } else { threshold_db });
        SilenceScan { thr, min_len, len: 0, start: None, last_loud: 0, raw: Vec::new() }
    }

    /// Scan the next block (planar), which follows the audio scanned so far.
    pub fn push(&mut self, ch: &[Vec<f32>]) {
        let n = max_len(ch);
        for k in 0..n {
            if !ch.iter().any(|c| c.get(k).is_some_and(|v| v.is_finite() && v.abs() > self.thr)) {
                continue;
            }
            let i = self.len.saturating_add(k as u64);
            match self.start {
                Some(s) if i - self.last_loud > self.min_len.max(1) => {
                    self.raw.push((s, self.last_loud.saturating_add(1)));
                    self.start = Some(i);
                }
                None => self.start = Some(i),
                _ => {}
            }
            self.last_loud = i;
        }
        self.len = self.len.saturating_add(n as u64);
    }

    /// Pass over `n` samples known to be silent.
    pub fn skip(&mut self, n: u64) {
        self.len = self.len.saturating_add(n);
    }

    /// The regions found, padded and merged (see [`non_silent_ranges`]).
    pub fn finish(mut self, pad_before: u64, pad_after: u64) -> Vec<(u64, u64)> {
        if let Some(s) = self.start {
            self.raw.push((s, self.last_loud.saturating_add(1)));
        }
        let mut out: Vec<(u64, u64)> = Vec::with_capacity(self.raw.len());
        for (s, e) in self.raw {
            let s = s.saturating_sub(pad_before);
            let e = e.saturating_add(pad_after).min(self.len);
            match out.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => out.push((s, e)),
            }
        }
        out
    }
}

/// Runs a plugin over whole buffers in blocks, compensating its latency so the output lines up
/// with the input. The buffer length is unchanged (any tail past the end is dropped; see
/// [`apply_plugin_with_tail`]).
pub fn apply_plugin(ch: &mut [Vec<f32>], plugin: &mut dyn Plugin, sample_rate: f32) {
    let len = max_len(ch);
    let out = render(ch, plugin, sample_rate, len);
    for (c, o) in ch.iter_mut().zip(out) {
        for (d, s) in c.iter_mut().zip(o) {
            *d = s;
        }
    }
}

/// Like [`apply_plugin`] but returns new buffers extended by the plugin's tail (capped at 60 s).
pub fn apply_plugin_with_tail(ch: &[Vec<f32>], plugin: &mut dyn Plugin, sample_rate: f32) -> Vec<Vec<f32>> {
    let len = max_len(ch);
    plugin.prepare(sane_sr(sample_rate), 1024, ch.len());
    let tail = plugin.tail_samples().min((sane_sr(sample_rate) * 60.0) as usize);
    render(ch, plugin, sample_rate, (len + tail).min(MAX_OFFLINE_SAMPLES))
}

fn render(ch: &[Vec<f32>], plugin: &mut dyn Plugin, sample_rate: f32, out_len: usize) -> Vec<Vec<f32>> {
    const BLOCK: usize = 1024;
    let sr = sane_sr(sample_rate);
    let channels = ch.len();
    plugin.prepare(sr, BLOCK, channels);
    plugin.reset();
    let lat = plugin.latency().min(MAX_OFFLINE_SAMPLES);
    let mut out: Vec<Vec<f32>> = (0..channels).map(|_| vec![0.0; out_len]).collect();
    let mut scratch: Vec<Vec<f32>> = (0..channels).map(|_| vec![0.0; BLOCK]).collect();
    let total = out_len + lat;
    let mut done = 0usize;
    while done < total {
        let n = BLOCK.min(total - done);
        for (s, c) in scratch.iter_mut().zip(ch) {
            for (i, v) in s.iter_mut().enumerate() {
                *v = if i < n { c.get(done + i).copied().unwrap_or(0.0) } else { 0.0 };
            }
        }
        plugin.process(&mut scratch, n);
        for (s, o) in scratch.iter().zip(out.iter_mut()) {
            for (i, v) in s.iter().take(n).enumerate() {
                let idx = done + i;
                if idx >= lat
                    && let Some(d) = o.get_mut(idx - lat)
                {
                    *d = if v.is_finite() { *v } else { 0.0 };
                }
            }
        }
        done += n;
    }
    out
}

/// Peak level of the buffers in dBFS (-144 for silence).
pub fn peak_db(ch: &[Vec<f32>]) -> f32 {
    gain_to_db(peak(ch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_shapes_endpoints() {
        for s in FadeShape::ALL {
            assert!(s.gain(0.0).abs() < 1e-6, "{s:?}");
            assert!((s.gain(1.0) - 1.0).abs() < 1e-5, "{s:?}");
            assert!(s.gain(f32::NAN) == 0.0);
        }
    }

    #[test]
    fn fade_hits_zero_at_edges() {
        let mut ch = vec![vec![1.0f32; 100]];
        fade(&mut ch, 10, 10, FadeShape::Linear);
        assert_eq!(ch[0][0], 0.0);
        assert_eq!(ch[0][99], 0.0);
        assert_eq!(ch[0][50], 1.0);
        fade(&mut ch, 1000, 1000, FadeShape::SCurve);
    }
}

#[cfg(test)]
mod resample_tests {
    use super::*;

    fn sine(f: f64, sr: u32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (0.5 * (2.0 * PI * f * i as f64 / f64::from(sr)).sin()) as f32).collect()
    }

    /// THD+N of a sine at `f`: power left after removing the best-fitting sine, relative to all, dB.
    fn thd_n_db(x: &[f32], f: f64, sr: u32) -> f64 {
        let (mut c, mut s, mut tot) = (0.0f64, 0.0f64, 0.0f64);
        for (i, v) in x.iter().enumerate() {
            let a = 2.0 * PI * f * i as f64 / f64::from(sr);
            c += f64::from(*v) * a.cos();
            s += f64::from(*v) * a.sin();
            tot += f64::from(*v) * f64::from(*v);
        }
        let n = x.len() as f64;
        let (c, s) = (2.0 * c / n, 2.0 * s / n);
        let mut res = 0.0f64;
        for (i, v) in x.iter().enumerate() {
            let a = 2.0 * PI * f * i as f64 / f64::from(sr);
            let e = f64::from(*v) - c * a.cos() - s * a.sin();
            res += e * e;
        }
        10.0 * (res / tot.max(1e-30)).log10()
    }

    pub(super) fn quality(f: fn(&[Vec<f32>], u32, u32) -> Vec<Vec<f32>>) -> (f64, f64, f64) {
        let mut thd = f64::MIN;
        for (from, to) in [(48_000u32, 44_100u32), (44_100, 48_000)] {
            let y = f(&[sine(997.0, from, from as usize * 2)], from, to);
            let mid = &y[0][4000..y[0].len() - 4000];
            thd = thd.max(thd_n_db(mid, 997.0, to));
        }
        // 23 kHz at 48 kHz has no place at 44.1 kHz: what is left of it (folded to 21.1 kHz).
        let y = f(&[sine(23_000.0, 48_000, 96_000)], 48_000, 44_100);
        let mid = &y[0][4000..y[0].len() - 4000];
        let alias = 10.0 * (mid.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / mid.len() as f64 / 0.125).log10();
        // 18 kHz passes: its level after a round trip 44.1 -> 48 -> 44.1, dB.
        let x = sine(18_000.0, 44_100, 88_200);
        let y = f(&f(&[x], 44_100, 48_000), 48_000, 44_100);
        let mid = &y[0][4000..y[0].len() - 4000];
        let pass = 10.0 * (mid.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / mid.len() as f64 / 0.125).log10();
        (thd, alias, pass)
    }

    #[test]
    fn polyphase_matches_the_table_path_and_is_clean() {
        for (from, to) in [(48_000u32, 44_100u32), (44_100, 48_000)] {
            let x = sine(997.0, from, from as usize * 2);
            let a = resample(&[x.clone(), x.clone()], from, to);
            let b = resample_table(&[x], from, to);
            assert_eq!(a[0].len(), b[0].len());
            assert_eq!(a[0], a[1]);
            let (mut s, mut e) = (0.0f64, 0.0f64);
            for (p, q) in a[0].iter().zip(&b[0]).skip(2000).take(80_000) {
                s += f64::from(*q) * f64::from(*q);
                e += f64::from(p - q) * f64::from(p - q);
            }
            let snr = 10.0 * (s / e.max(1e-30)).log10();
            println!("{from} -> {to}: polyphase vs table {snr:.1} dB");
            assert!(snr > 80.0, "{from}->{to}: {snr:.1} dB");
        }
        let (thd, alias, pass) = quality(resample);
        let (thd0, alias0, pass0) = quality(resample_table);
        println!("polyphase: THD+N {thd:.1} dB, 23 kHz alias {alias:.1} dB, 18 kHz round trip {pass:.2} dB");
        println!("table:     THD+N {thd0:.1} dB, 23 kHz alias {alias0:.1} dB, 18 kHz round trip {pass0:.2} dB");
        assert!(thd <= thd0 + 1.0 && thd < -80.0, "{thd} vs {thd0}");
        assert!(alias <= alias0 + 1.0 && alias < -80.0, "{alias} vs {alias0}");
        assert!((pass - pass0).abs() < 0.1, "{pass} vs {pass0}");
    }

    #[test]
    fn resample_edge_cases() {
        assert_eq!(resample(&[vec![]], 48_000, 44_100), vec![Vec::<f32>::new()]);
        assert_eq!(resample(&[vec![1.0]], 44_100, 48_000)[0].len(), 1);
        // An awkward ratio still works (table path).
        let y = resample(&[vec![0.1; 1000]], 44_100, 96_001);
        assert_eq!(y[0].len(), (1000.0f64 * 96_001.0 / 44_100.0).round() as usize);
    }

    #[test]
    #[ignore = "timing: cargo test --release -p soundcraft-dsp resample_speed -- --ignored --nocapture"]
    fn resample_speed() {
        let x = sine(997.0, 48_000, 48_000 * 240);
        for f in [resample as fn(&[Vec<f32>], u32, u32) -> Vec<Vec<f32>>, resample_table] {
            let t0 = std::time::Instant::now();
            let y = f(&[x.clone(), x.clone()], 48_000, 44_100);
            let t1 = t0.elapsed().as_secs_f64();
            let _ = f(&y, 44_100, 48_000);
            println!("4 min stereo: 48->44.1 {t1:.2} s, back {:.2} s", t0.elapsed().as_secs_f64() - t1);
        }
    }
}
