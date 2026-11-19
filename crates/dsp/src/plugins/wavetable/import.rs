//! Making a wavetable from audio: three ways of cutting a recording into single-cycle waves, as
//! pure functions over mono samples (the engine reads the file, mixes it to mono, tracks its pitch
//! and stores the result: `mix.wavetable_import`).
//!
//! - [`fixed`]: the file is a row of equal frames, each already one cycle (a wavetable file as
//!   other synths write them, 2048 samples a frame by convention). [`clm_frame_size`] reads the
//!   frame size a WAV names in its `clm ` chunk; [`guess_frame_size`] guesses it from the length.
//! - [`pitched`]: one cycle cut at each of up to 32 points through a pitched recording, at the
//!   pitch tracked there, resampled to the table length and turned so every wave's fundamental
//!   starts at the same phase (neighbouring waves morph without a jump).
//! - [`spectral`]: at up to 32 points a windowed spectrum, and one clean cycle made from its
//!   magnitudes at the harmonics of one reference pitch (the recording's own when it has one),
//!   every wave with the same fixed phases (so they morph smoothly). Works on anything: noise,
//!   speech, drums.
//!
//! Every function takes hostile input: empty, silent, constant or non-finite samples give silent
//! or empty results, never a panic or a non-finite sample.

use super::tables::{C, fft};
use super::user::{MAX_WAVES, WAVE_LEN, resample_cycle};

/// Frame sizes [`fixed`] accepts (powers of two).
pub const FRAME_SIZES: [usize; 6] = [256, 512, 1024, 2048, 4096, 8192];
/// The usual frame size of a wavetable file.
pub const DEFAULT_FRAME: usize = 2048;

/// Waves cut from a recording, and what the user should know about how.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sliced {
    /// Up to [`MAX_WAVES`] waves of [`WAVE_LEN`] samples.
    pub waves: Vec<Vec<f32>>,
    /// Plain sentences: what was guessed, spread, truncated or left out.
    pub notes: Vec<String>,
}

fn clean(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// `count` (at least 1) indices spread evenly over `0..n` (both ends included when there is room).
fn spread(n: usize, count: usize) -> Vec<usize> {
    let count = count.clamp(1, n.max(1));
    if count == 1 {
        return vec![n.saturating_sub(1) / 2];
    }
    (0..count).map(|i| ((i as f64 * (n - 1) as f64 / (count - 1) as f64).round() as usize).min(n.saturating_sub(1))).collect()
}

/// The frame size a WAV's `clm ` chunk names (`<!>2048 …`, written by wavetable synths), when
/// the bytes are a RIFF/WAVE file that has one and it is one of [`FRAME_SIZES`].
pub fn clm_frame_size(bytes: &[u8]) -> Option<usize> {
    if bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut at = 12usize;
    while let Some(head) = bytes.get(at..at.checked_add(8)?) {
        let len = u32::from_le_bytes(head.get(4..8)?.try_into().ok()?) as usize;
        let body = at + 8;
        if head.get(..4)? == b"clm " {
            let text = bytes.get(body..body.checked_add(len.min(64))?.min(bytes.len()))?;
            let text = std::str::from_utf8(text).ok()?;
            let digits: String = text.trim_start_matches("<!>").chars().take_while(char::is_ascii_digit).collect();
            return digits.parse().ok().filter(|n| FRAME_SIZES.contains(n));
        }
        at = body.checked_add(len)?.checked_add(len & 1)?;
    }
    None
}

/// The frame size for a fixed-size import of `len` samples: the `clm ` chunk's when there is
/// one, else the first of 2048, 1024, 4096, 512, 256, 8192 that divides the length into 1 to 256
/// whole frames, else 2048. The flag says it was a guess (no chunk named it).
pub fn guess_frame_size(len: usize, clm: Option<usize>) -> (usize, bool) {
    if let Some(n) = clm.filter(|n| FRAME_SIZES.contains(n)) {
        return (n, false);
    }
    let fits = |n: usize| len >= n && len.is_multiple_of(n) && len / n <= 256;
    let size = [2048, 1024, 4096, 512, 256, 8192].into_iter().find(|n| fits(*n)).unwrap_or(DEFAULT_FRAME);
    (size, true)
}

/// Fixed-size frames: `samples` cut into frames of `frame` samples (one of [`FRAME_SIZES`]), each
/// one cycle. More than 32 frames: 32 spread evenly through them. A file shorter than one frame
/// is taken as a single cycle of its own length; a part-frame left at the end is left out.
pub fn fixed(samples: &[f32], frame: usize) -> Sliced {
    let mut out = Sliced::default();
    let frame = if FRAME_SIZES.contains(&frame) { frame } else { DEFAULT_FRAME };
    if samples.is_empty() {
        return out;
    }
    let n = samples.len() / frame;
    if n == 0 {
        out.notes.push(format!("The file is shorter than one frame of {frame} samples: it is taken as one cycle of {} samples.", samples.len()));
        out.waves.push(to_wave(samples));
        return out;
    }
    if !samples.len().is_multiple_of(frame) {
        out.notes.push(format!("The last {} samples (less than a frame) were left out.", samples.len() % frame));
    }
    if n > MAX_WAVES {
        out.notes.push(format!("The file has {n} frames: {MAX_WAVES} spread evenly through them were taken."));
    }
    for i in spread(n, MAX_WAVES.min(n)) {
        out.waves.push(to_wave(samples.get(i * frame..(i + 1) * frame).unwrap_or(&[])));
    }
    out
}

/// One cycle of any length as a wave of [`WAVE_LEN`]: through its spectrum (harmonics up to what
/// both lengths hold), so a longer frame is not aliased on the way down.
fn to_wave(cycle: &[f32]) -> Vec<f32> {
    let m = cycle.len();
    if m == WAVE_LEN {
        return cycle.iter().map(|v| clean(*v)).collect();
    }
    if m < 4 {
        return resample_cycle(cycle, WAVE_LEN);
    }
    let h = (m / 2).saturating_sub(1).min(WAVE_LEN / 2 - 1);
    let coeffs = dft(cycle, h);
    synth_wave(&coeffs)
}

/// The Fourier coefficients c_1..c_h of one period (index 0 unused): exact for any length (a
/// transform when it is a power of two, else a direct sum).
fn dft(cycle: &[f32], h: usize) -> Vec<C> {
    let m = cycle.len().max(1);
    let mut c = vec![(0.0, 0.0); h + 1];
    if m.is_power_of_two() && m >= 2 {
        let mut x: Vec<C> = cycle.iter().map(|v| (f64::from(clean(*v)), 0.0)).collect();
        fft(&mut x, false);
        for (k, ck) in c.iter_mut().enumerate().skip(1) {
            let v = x.get(k).copied().unwrap_or((0.0, 0.0));
            *ck = (v.0 / m as f64, v.1 / m as f64);
        }
        return c;
    }
    for (k, ck) in c.iter_mut().enumerate().skip(1) {
        let (mut re, mut im) = (0.0, 0.0);
        let w = std::f64::consts::TAU * k as f64 / m as f64;
        for (i, v) in cycle.iter().enumerate() {
            let a = w * i as f64;
            re += f64::from(clean(*v)) * a.cos();
            im -= f64::from(clean(*v)) * a.sin();
        }
        *ck = (re / m as f64, im / m as f64);
    }
    c
}

/// A wave of [`WAVE_LEN`] from coefficients c_1.. (index 0 unused; beyond the table's harmonics
/// left out).
fn synth_wave(c: &[C]) -> Vec<f32> {
    let n = WAVE_LEN;
    let mut buf: Vec<C> = vec![(0.0, 0.0); n];
    for (k, ck) in c.iter().enumerate().skip(1).take(n / 2 - 1) {
        if let Some(x) = buf.get_mut(k) {
            *x = *ck;
        }
        if let Some(x) = buf.get_mut(n - k) {
            *x = (ck.0, -ck.1);
        }
    }
    fft(&mut buf, true);
    buf.iter().map(|x| if x.0.is_finite() { x.0 as f32 } else { 0.0 }).collect()
}

/// `x` read at fractional position `t` (cubic through four neighbours; 0 outside).
fn at(x: &[f32], t: f64) -> f32 {
    if !t.is_finite() || t < 0.0 {
        return 0.0;
    }
    let i = t.floor() as usize;
    let f = (t - i as f64) as f32;
    let s = |k: isize| usize::try_from(i as isize + k).ok().and_then(|k| x.get(k)).copied().map_or(0.0, clean);
    let (y0, y1, y2, y3) = (s(-1), s(0), s(1), s(2));
    let a = -0.5 * y0 + 1.5 * y1 - 1.5 * y2 + 0.5 * y3;
    let b = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c = -0.5 * y0 + 0.5 * y2;
    ((a * f + b) * f + c) * f + y1
}

/// How alike one period of `x` from `t` is to the next at period `p` (normalised correlation).
fn period_match(x: &[f32], t: f64, p: f64) -> f64 {
    let n = 64usize;
    let (mut ab, mut aa, mut bb) = (0.0f64, 0.0f64, 0.0f64);
    for k in 0..n {
        let s = t + p * k as f64 / n as f64;
        let (a, b) = (f64::from(at(x, s)), f64::from(at(x, s + p)));
        ab += a * b;
        aa += a * a;
        bb += b * b;
    }
    if aa <= 1e-20 || bb <= 1e-20 { 0.0 } else { ab / (aa * bb).sqrt() }
}

/// Pitch-tracked: at up to `count` points spread over the pitched part of `samples` (rate `sr`),
/// one cycle cut at the pitch `pitch` gives there (`pitch[i]`: Hz around sample `i * hop`,
/// `None` where unpitched), the period refined against the audio, the cycle's ends joined,
/// resampled to [`WAVE_LEN`], and turned so its fundamental starts at sine phase zero. Nothing
/// pitched: no waves, and a note saying so.
pub fn pitched(samples: &[f32], sr: f32, pitch: &[Option<f32>], hop: usize, count: usize) -> Sliced {
    let mut out = Sliced::default();
    let hop = hop.max(1);
    let voiced: Vec<(usize, f64)> = pitch
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.filter(|f| f.is_finite() && *f > 1.0 && *f < sr / 2.0).map(|f| (i * hop, f64::from(f))))
        .filter(|(at, f)| *at >= 4 && (*at as f64 + 2.5 * f64::from(sr) / f) < samples.len() as f64)
        .collect();
    if voiced.is_empty() || !(sr.is_finite() && sr > 0.0) {
        out.notes.push("No pitch was found in the audio: try Spectral (it works on anything) or Fixed.".into());
        return out;
    }
    let count = count.clamp(1, MAX_WAVES);
    if voiced.len() < count {
        out.notes.push(format!("Only {} pitched moments were found: {} waves were made.", voiced.len(), voiced.len()));
    }
    for i in spread(voiced.len(), count) {
        let Some(&(t, f0)) = voiced.get(i) else { continue };
        let guess = f64::from(sr) / f0;
        // The period that best repeats here, within 3% of the tracked one.
        let mut best = (guess, f64::MIN);
        for k in -30..=30 {
            let p = guess * (1.0 + f64::from(k) * 0.001);
            let m = period_match(samples, t as f64, p);
            if m > best.1 {
                best = (p, m);
            }
        }
        let p = best.0;
        // One period from t, its ends joined (the drift from the start to one period on is taken
        // out as a ramp), then turned to a common phase.
        let start = at(samples, t as f64);
        let drift = at(samples, t as f64 + p) - start;
        let cycle: Vec<f32> = (0..WAVE_LEN)
            .map(|k| {
                let frac = k as f64 / WAVE_LEN as f64;
                at(samples, t as f64 + p * frac) - drift * frac as f32
            })
            .collect();
        out.waves.push(align_phase(&cycle));
    }
    out
}

/// `wave` ([`WAVE_LEN`]) turned in time so its fundamental is a sine starting at phase zero (a
/// silent fundamental: left as it is).
fn align_phase(wave: &[f32]) -> Vec<f32> {
    let h = WAVE_LEN / 2 - 1;
    let mut c = dft(wave, h);
    let Some(&c1) = c.get(1) else { return wave.to_vec() };
    if c1.0.hypot(c1.1) < 1e-9 {
        return wave.to_vec();
    }
    // c1 = (a/2) e^{i(φ − π/2)} for a sin(ωt + φ): turn every harmonic k by −kφ.
    let phi = c1.1.atan2(c1.0) + std::f64::consts::FRAC_PI_2;
    for (k, ck) in c.iter_mut().enumerate().skip(1) {
        let (s, co) = (-(k as f64) * phi).sin_cos();
        *ck = (ck.0 * co - ck.1 * s, ck.0 * s + ck.1 * co);
    }
    synth_wave(&c)
}

/// Spectral: at up to `count` points spread through `samples` (rate `sr`), a Hann-windowed
/// spectrum, and one cycle made from its magnitudes at the harmonics of `f_ref` (the recording's
/// pitch, else none: each harmonic is then one bin of a 2048-point window, about 23 Hz apart at
/// 48 kHz), every harmonic at a fixed phase shared by all waves (a low-crest choice) so the waves
/// morph smoothly. Harmonics above half the sample rate stay silent.
pub fn spectral(samples: &[f32], sr: f32, count: usize, f_ref: Option<f32>) -> Sliced {
    let mut out = Sliced::default();
    if samples.is_empty() || !(sr.is_finite() && sr > 0.0) {
        return out;
    }
    let sr = f64::from(sr);
    let f_ref = f_ref.map(f64::from).filter(|f| f.is_finite() && *f > 1.0 && *f < sr / 4.0);
    // The window: four periods of the reference at least, 2048 samples at least.
    let n = match f_ref {
        Some(f) => (4.0 * sr / f).ceil().clamp(2048.0, 65_536.0) as usize,
        None => 2048,
    }
    .next_power_of_two();
    let f_ref = f_ref.unwrap_or(sr / n as f64);
    let count = count.clamp(1, MAX_WAVES);
    let len = samples.len();
    let span = len.saturating_sub(n);
    let h = WAVE_LEN / 2 - 1;
    let mut buf: Vec<C> = vec![(0.0, 0.0); n];
    for i in 0..count {
        let start = if count == 1 { span / 2 } else { (i as f64 * span as f64 / (count - 1) as f64).round() as usize };
        // The window's own mean (a constant offset) taken out first: the window would smear it
        // into the lowest harmonics.
        let seg = samples.get(start..(start + n).min(len)).unwrap_or(&[]);
        let mean = seg.iter().map(|v| f64::from(clean(*v))).sum::<f64>() / seg.len().max(1) as f64;
        for (k, b) in buf.iter_mut().enumerate() {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * k as f64 / n as f64).cos();
            let x = seg.get(k).map_or(0.0, |v| f64::from(clean(*v)) - mean);
            *b = (x * w, 0.0);
        }
        fft(&mut buf, false);
        let mag = |bin: usize| buf.get(bin).map_or(0.0, |v| v.0.hypot(v.1)) * 4.0 / n as f64;
        let mut c = vec![(0.0, 0.0); h + 1];
        for (k, ck) in c.iter_mut().enumerate().skip(1) {
            let hz = f_ref * k as f64;
            if hz >= sr / 2.0 {
                break;
            }
            let b = hz * n as f64 / sr;
            let lo = (b.floor() as usize).saturating_sub(1).max(1);
            let hi = (b.ceil() as usize + 1).min(n / 2);
            // The loudest bin near the harmonic, its true peak found by a parabola through the
            // log magnitudes of it and its neighbours (the window's scalloping taken out).
            let m = (lo..=hi).fold(lo, |best, x| if mag(x) > mag(best) { x } else { best });
            let ln = |bin: usize| mag(bin).max(1e-30).ln();
            let (al, be, ga) = (ln(m.saturating_sub(1)), ln(m), ln(m + 1));
            let den = al - 2.0 * be + ga;
            let a = if m >= 1 && den < -1e-12 {
                let p = (0.5 * (al - ga) / den).clamp(-0.5, 0.5);
                (be - 0.25 * (al - ga) * p).exp()
            } else {
                mag(m)
            };
            let a = if a.is_finite() && a > 1e-12 { a } else { 0.0 };
            // A fixed phase per harmonic (quadratic, Newman's low-crest choice), the same for
            // every wave.
            let ph = std::f64::consts::PI * (k * k) as f64 / h as f64;
            *ck = (a / 2.0 * ph.cos(), a / 2.0 * ph.sin());
        }
        out.waves.push(synth_wave(&c));
    }
    if len < n {
        out.notes.push(format!("The audio is shorter than one analysis window ({n} samples): every wave comes from the whole of it."));
    }
    out
}

/// The amplitude of each harmonic 1..=`h` of `wave` (a sine of peak 1 at harmonic k reads 1).
pub fn harmonic_amplitudes(wave: &[f32], h: usize) -> Vec<f32> {
    dft(wave, h).iter().skip(1).map(|c| (2.0 * c.0.hypot(c.1)) as f32).collect()
}

/// A brightness measure of a wave: its amplitude-weighted mean harmonic number.
pub fn brightness(wave: &[f32]) -> f32 {
    let a = harmonic_amplitudes(wave, WAVE_LEN / 2 - 1);
    let (num, den) = a.iter().enumerate().fold((0.0f64, 0.0f64), |(n, d), (k, v)| (n + (k + 1) as f64 * f64::from(*v), d + f64::from(*v)));
    if den <= 1e-12 { 0.0 } else { (num / den) as f32 }
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
