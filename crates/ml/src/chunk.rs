//! Chunked overlap-add: run a model over long audio in fixed-size segments and cross-fade the
//! results back together. Two schemes, matching the two families of reference code:
//!
//! - [`demucs_split`]: Demucs' `apply_model` (segments of `segment` frames every
//!   `segment * (1 - overlap)`, each padded around its centre from the neighbouring audio,
//!   triangular weights raised to `transition_power`).
//! - [`msst_split`]: ZFTurbo's MSST `demix` (reflect-padded borders, `chunk / num_overlap` step,
//!   linear fades of `chunk / 10` except at the outer ends, normalised by the summed window).
//!
//! The model closure maps one chunk `[channel][frame]` to its stems `[stem][channel][frame]`.
//!
//! [`demucs_shifts`] is Demucs' `shifts` averaging around [`demucs_split`]: the input is offset
//! by a random amount up to half a second, separated, shifted back and averaged over the passes,
//! which smooths the chunk-edge and phase artefacts one pass leaves ("Quality: High").

use crate::job::Ctx;
use crate::{MlError, Result};
use serde::{Deserialize, Serialize};

/// How carefully a separation runs (Separate Stems, Make Karaoke, Split Drums' first step, and
/// Convert to MIDI's isolation): one pass, or shift averaging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    /// One pass at the model card's overlap (Demucs' `shifts=0`).
    #[default]
    Fast,
    /// [`HIGH_SHIFTS`] random shifts up to half a second, averaged, at [`HIGH_OVERLAP`]: about
    /// `HIGH_SHIFTS` times the work.
    High,
}

/// Quality High's number of shifted passes.
pub const HIGH_SHIFTS: usize = 2;
/// Quality High's segment overlap (Fast keeps the card's, 0.25 for HTDemucs).
pub const HIGH_OVERLAP: f32 = 0.5;
/// The most a pass is shifted, seconds (Demucs' `max_shift = 0.5 * samplerate`).
pub const MAX_SHIFT_SECONDS: f64 = 0.5;
/// The seed of the shifts: a given input separates the same way twice.
pub const SHIFT_SEED: u64 = 0x5EED_D3AC_0C55;

impl Quality {
    pub const ALL: [Quality; 2] = [Quality::Fast, Quality::High];

    pub fn parse(s: &str) -> Option<Quality> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fast" | "" => Some(Quality::Fast),
            "high" | "best" => Some(Quality::High),
            _ => None,
        }
    }

    /// The command / CLI name.
    pub fn id(self) -> &'static str {
        match self {
            Quality::Fast => "fast",
            Quality::High => "high",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::Fast => "Fast (one pass)",
            Quality::High => "High (shift averaging, about 2× slower)",
        }
    }

    /// How many separation passes it runs (what the progress and time estimates scale by).
    pub fn passes(self) -> usize {
        match self {
            Quality::Fast => 1,
            Quality::High => HIGH_SHIFTS,
        }
    }

    /// The segment overlap to use, given the card's.
    pub fn overlap(self, card_overlap: f32) -> f32 {
        match self {
            Quality::Fast => card_overlap,
            Quality::High => card_overlap.max(HIGH_OVERLAP),
        }
    }
}

/// A small deterministic generator for the shift offsets (SplitMix64: no dependency, the same
/// sequence on every machine).
pub struct ShiftRng(u64);

impl ShiftRng {
    pub fn new(seed: u64) -> Self {
        ShiftRng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `0..=max` (Python's `random.randint(0, max_shift)`).
    pub fn below_or_equal(&mut self, max: usize) -> usize {
        if max == 0 {
            return 0;
        }
        usize::try_from(self.next_u64() % (max as u64 + 1)).unwrap_or(0)
    }
}

/// Demucs' `apply_model` with `shifts > 0`: `shifts` passes of [`demucs_split`], each on the
/// input preceded by `max_shift - offset` zeros (a random `offset` in `0..=max_shift`), the
/// results shifted back and averaged. `shifts == 0` is one plain pass. The offsets come from
/// `seed`, so the same input gives the same output every time. Progress covers all passes.
#[allow(clippy::too_many_arguments)]
pub fn demucs_shifts(
    mix: &[Vec<f32>],
    shifts: usize,
    max_shift: usize,
    seed: u64,
    segment: usize,
    overlap: f32,
    transition_power: f32,
    ctx: &Ctx,
    mut model: impl FnMut(&[Vec<f32>]) -> Result<Stems>,
) -> Result<Stems> {
    if shifts == 0 || max_shift == 0 {
        return demucs_split(mix, segment, overlap, transition_power, ctx, model);
    }
    let length = crate::frames(mix);
    if length == 0 {
        return Ok(Vec::new());
    }
    let mut rng = ShiftRng::new(seed);
    let mut acc: Stems = Vec::new();
    for pass in 0..shifts {
        ctx.check()?;
        let offset = rng.below_or_equal(max_shift);
        let lead = max_shift - offset;
        let shifted: Vec<Vec<f32>> = mix.iter().map(|c| std::iter::repeat_n(0.0f32, lead).chain(c.iter().copied()).collect()).collect();
        let stage = ctx.stage(pass as f32 / shifts as f32, (pass + 1) as f32 / shifts as f32);
        let out = demucs_split(&shifted, segment, overlap, transition_power, &stage, &mut model)?;
        drop(shifted);
        if acc.is_empty() {
            acc = out.into_iter().map(|s| s.into_iter().map(|c| c.into_iter().skip(lead).take(length).collect()).collect()).collect();
        } else {
            if out.len() != acc.len() || out.iter().zip(&acc).any(|(a, b)| a.len() != b.len()) {
                return Err(MlError::Runtime("the model returned a different number of stems or channels for one pass".into()));
            }
            for (a, o) in acc.iter_mut().zip(out) {
                for (ac, oc) in a.iter_mut().zip(o) {
                    for (v, x) in ac.iter_mut().zip(oc.into_iter().skip(lead)) {
                        *v += x;
                    }
                }
            }
        }
    }
    let g = 1.0 / shifts as f32;
    for s in acc.iter_mut() {
        for c in s.iter_mut() {
            c.iter_mut().for_each(|v| *v *= g);
        }
    }
    Ok(acc)
}

/// Stems Ã— channels Ã— frames.
pub type Stems = Vec<Vec<Vec<f32>>>;

/// Demucs' triangular overlap weight for a segment of `n` frames.
pub fn triangle(n: usize, power: f32) -> Vec<f32> {
    let half = n / 2;
    let w: Vec<f32> = (1..=half).map(|i| i as f32).chain((1..=(n - half)).rev().map(|i| i as f32)).collect();
    let max = w.iter().copied().fold(0.0f32, f32::max).max(1.0);
    w.into_iter().map(|v| (v / max).powf(power)).collect()
}

/// Add `chunk` (`[stem][channel][frame]`), weighted by `w`, into `out`/`sum` at `offset`.
fn accumulate(out: &mut Stems, sum: &mut [f32], chunk: &Stems, w: &[f32], offset: usize, len: usize) -> Result<()> {
    if out.is_empty() {
        let channels = chunk.first().map_or(0, Vec::len);
        *out = vec![vec![vec![0.0; sum.len()]; channels]; chunk.len()];
    }
    if chunk.len() != out.len() || chunk.iter().any(|s| s.len() != out.first().map_or(0, Vec::len)) {
        return Err(MlError::Runtime("the model returned a different number of stems or channels for one chunk".into()));
    }
    for (os, cs) in out.iter_mut().zip(chunk) {
        for (oc, cc) in os.iter_mut().zip(cs) {
            for k in 0..len {
                if let (Some(o), Some(v), Some(wk)) = (oc.get_mut(offset + k), cc.get(k), w.get(k)) {
                    *o += v * wk;
                }
            }
        }
    }
    for k in 0..len {
        if let (Some(s), Some(wk)) = (sum.get_mut(offset + k), w.get(k)) {
            *s += wk;
        }
    }
    Ok(())
}

fn normalize(out: &mut Stems, sum: &[f32]) {
    for s in out.iter_mut() {
        for c in s.iter_mut() {
            for (v, w) in c.iter_mut().zip(sum) {
                *v = if *w > 0.0 { *v / w } else { 0.0 };
            }
        }
    }
}

/// Demucs' `apply_model` split (`split=True`, no shifts): see the module docs. `segment` and
/// the closure's chunk length are equal; the closure gets `[channel][segment]` and returns
/// `[stem][channel][segment]`.
pub fn demucs_split(
    mix: &[Vec<f32>],
    segment: usize,
    overlap: f32,
    transition_power: f32,
    ctx: &Ctx,
    mut model: impl FnMut(&[Vec<f32>]) -> Result<Stems>,
) -> Result<Stems> {
    let length = crate::frames(mix);
    if segment == 0 {
        return Err(MlError::BadInput("segment length is zero".into()));
    }
    let overlap = if overlap.is_finite() { overlap.clamp(0.0, 0.95) } else { 0.25 };
    let stride = ((1.0 - overlap) * segment as f32) as usize;
    let stride = stride.max(1);
    let weight = triangle(segment, if transition_power.is_finite() { transition_power } else { 1.0 });
    let mut out: Stems = Vec::new();
    let mut sum = vec![0.0f32; length];
    let offsets: Vec<usize> = (0..length).step_by(stride).collect();
    let total = offsets.len().max(1);
    for (i, &offset) in offsets.iter().enumerate() {
        ctx.check()?;
        let chunk_len = segment.min(length - offset);
        // TensorChunk.padded(segment): centred on the chunk, from the neighbouring audio, zeros past the ends.
        let delta = segment - chunk_len;
        let start = offset as isize - (delta / 2) as isize;
        let input: Vec<Vec<f32>> = mix
            .iter()
            .map(|c| (0..segment).map(|k| usize::try_from(start + k as isize).ok().and_then(|j| c.get(j)).copied().unwrap_or(0.0)).collect())
            .collect();
        let res = model(&input)?;
        // center_trim to the chunk length.
        let trim = (segment - chunk_len) / 2;
        let res: Stems = res.into_iter().map(|s| s.into_iter().map(|c| c.into_iter().skip(trim).take(chunk_len).collect()).collect()).collect();
        accumulate(&mut out, &mut sum, &res, &weight, offset, chunk_len)?;
        ctx.report((i + 1) as f32 / total as f32);
    }
    normalize(&mut out, &sum);
    Ok(out)
}

/// The MSST `demix` window: linear fades of `fade` frames; the first chunk has no fade-in, else
/// the last chunk has no fade-out (`if i == 0 … elif last …`).
pub fn msst_window(chunk: usize, fade: usize, first: bool, last: bool) -> Vec<f32> {
    let mut w = vec![1.0f32; chunk];
    let fade = fade.min(chunk);
    if fade > 1 {
        let ramp = |k: usize, up: bool| -> f32 {
            // np.linspace(0, 1, fade, dtype=float32) / np.linspace(1, 0, fade, dtype=float32)
            let v = (k as f64 / (fade - 1) as f64) as f32;
            if up { v } else { (1.0 - k as f64 / (fade - 1) as f64) as f32 }
        };
        for k in 0..fade {
            if let Some(v) = w.get_mut(chunk - fade + k) {
                *v = ramp(k, false);
            }
        }
        for k in 0..fade {
            if let Some(v) = w.get_mut(k) {
                *v = ramp(k, true);
            }
        }
        if first {
            w.iter_mut().take(fade).for_each(|v| *v = 1.0);
        } else if last {
            w.iter_mut().skip(chunk - fade).for_each(|v| *v = 1.0);
        }
    }
    w
}

/// Reflect-pad `x` by `left`/`right` frames (`F.pad(mode='reflect')`); constant zeros when the
/// signal is too short to reflect.
fn pad_reflect(x: &[f32], left: usize, right: usize) -> Vec<f32> {
    let n = x.len();
    if n <= left.max(right) {
        return std::iter::repeat_n(0.0, left).chain(x.iter().copied()).chain(std::iter::repeat_n(0.0, right)).collect();
    }
    let at = |i: isize| -> f32 {
        let j = if i < 0 {
            (-i) as usize
        } else if i as usize >= n {
            2 * (n - 1) - i as usize
        } else {
            i as usize
        };
        x.get(j).copied().unwrap_or(0.0)
    };
    (-(left as isize)..(n + right) as isize).map(at).collect()
}

/// ZFTurbo MSST `demix` (generic mode): see the module docs. The closure gets `[channel][chunk]`
/// and returns `[stem][channel][chunk]`.
pub fn msst_split(
    mix: &[Vec<f32>],
    chunk: usize,
    num_overlap: usize,
    ctx: &Ctx,
    mut model: impl FnMut(&[Vec<f32>]) -> Result<Stems>,
) -> Result<Stems> {
    if chunk == 0 {
        return Err(MlError::BadInput("chunk length is zero".into()));
    }
    let length_init = crate::frames(mix);
    let step = (chunk / num_overlap.max(1)).max(1);
    let fade = chunk / 10;
    let border = chunk - step;
    let padded: Vec<Vec<f32>> =
        if length_init > 2 * border && border > 0 { mix.iter().map(|c| pad_reflect(c, border, border)).collect() } else { mix.to_vec() };
    let length = crate::frames(&padded);
    let mut out: Stems = Vec::new();
    let mut sum = vec![0.0f32; length];
    let starts: Vec<usize> = (0..length).step_by(step).collect();
    let total = starts.len().max(1);
    for (n, &i) in starts.iter().enumerate() {
        ctx.check()?;
        let part_len = chunk.min(length - i);
        let part: Vec<Vec<f32>> = padded
            .iter()
            .map(|c| {
                let p: Vec<f32> = c.iter().skip(i).take(part_len).copied().collect();
                if part_len < chunk {
                    if part_len > chunk / 2 {
                        let mut r = pad_reflect(&p, 0, chunk - part_len);
                        r.truncate(chunk);
                        r
                    } else {
                        let mut r = p;
                        r.resize(chunk, 0.0);
                        r
                    }
                } else {
                    p
                }
            })
            .collect();
        let res = model(&part)?;
        let w = msst_window(chunk, fade, i == 0, i + step >= length);
        accumulate(&mut out, &mut sum, &res, &w, i, part_len)?;
        ctx.report((n + 1) as f32 / total as f32);
    }
    normalize(&mut out, &sum);
    if length_init > 2 * border && border > 0 {
        for s in out.iter_mut() {
            for c in s.iter_mut() {
                *c = c.iter().skip(border).take(length_init).copied().collect();
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(n: usize) -> Vec<Vec<f32>> {
        vec![(0..n).map(|i| (i as f32 * 0.01).sin()).collect(), (0..n).map(|i| (i as f32 * 0.003).cos()).collect()]
    }

    #[test]
    fn identity_models_reconstruct_the_input() {
        let x = signal(10_000);
        let id = |c: &[Vec<f32>]| Ok(vec![c.to_vec(), c.iter().map(|ch| ch.iter().map(|v| v * 0.5).collect()).collect()]);
        for seg in [1000usize, 4096, 20_000] {
            let y = demucs_split(&x, seg, 0.25, 1.0, &Ctx::default(), id).unwrap();
            assert_eq!(y.len(), 2);
            for (a, b) in x.iter().zip(&y[0]) {
                assert_eq!(a.len(), b.len());
                let err = a.iter().zip(b).map(|(p, q)| (p - q).abs()).fold(0.0f32, f32::max);
                assert!(err < 1e-5, "demucs seg {seg}: {err}");
            }
            let y = msst_split(&x, seg, 4, &Ctx::default(), id).unwrap();
            for (a, b) in x.iter().zip(&y[1]) {
                assert_eq!(a.len(), b.len());
                let err = a.iter().zip(b).map(|(p, q)| (p * 0.5 - q).abs()).fold(0.0f32, f32::max);
                assert!(err < 1e-5, "msst seg {seg}: {err}");
            }
        }
    }

    #[test]
    fn triangle_matches_demucs() {
        // th.cat([arange(1, 3), arange(3, 0, -1)]) / 3 for segment 5.
        let w = triangle(5, 1.0);
        let want = [1.0 / 3.0, 2.0 / 3.0, 1.0, 2.0 / 3.0, 1.0 / 3.0];
        assert!(w.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-6), "{w:?}");
    }

    #[test]
    fn cancel_and_bad_models_are_errors() {
        let x = signal(5000);
        let ctx = Ctx::default();
        ctx.cancel_flag().cancel();
        assert_eq!(demucs_split(&x, 1000, 0.25, 1.0, &ctx, |c| Ok(vec![c.to_vec()])), Err(MlError::Cancelled));
        let mut n = 0;
        let flaky = |c: &[Vec<f32>]| {
            n += 1;
            Ok(if n == 1 { vec![c.to_vec()] } else { vec![c.to_vec(), c.to_vec()] })
        };
        assert!(msst_split(&x, 1000, 2, &Ctx::default(), flaky).is_err());
        assert!(demucs_split(&[], 1000, 0.25, 1.0, &Ctx::default(), |c| Ok(vec![c.to_vec()])).unwrap().is_empty());
    }

    /// A stand-in separator: a brick-wall split of each chunk into a low band (bass), a mid band
    /// (pad) and a high band (drums) with a rectangular-window FFT, plus a little noise seeded by
    /// the chunk's content (what a network's chunk-dependent errors look like). Each pass sees
    /// different chunks, so averaging passes thins the noise; one pass keeps all of it.
    fn band_split(chunk: &[Vec<f32>]) -> Result<Stems> {
        use rustfft::num_complex::Complex32;
        let n = crate::frames(chunk);
        let fft = rustfft::FftPlanner::<f32>::new().plan_fft_forward(n);
        let ifft = rustfft::FftPlanner::<f32>::new().plan_fft_inverse(n);
        let bin = |hz: f32| (hz / 8000.0 * n as f32) as usize;
        let edges = [(0, bin(150.0)), (bin(150.0), bin(1500.0)), (bin(1500.0), n / 2 + 1)];
        let mut out: Stems = vec![vec![Vec::new(); chunk.len()]; 3];
        for (ci, c) in chunk.iter().enumerate() {
            let mut spec: Vec<Complex32> = c.iter().map(|v| Complex32::new(*v, 0.0)).collect();
            fft.process(&mut spec);
            let mut h = ShiftRng::new(c.iter().fold(17u64, |a, v| a.wrapping_mul(31).wrapping_add(u64::from(v.to_bits()))));
            for (si, (lo, hi)) in edges.iter().enumerate() {
                let mut band: Vec<Complex32> = spec
                    .iter()
                    .enumerate()
                    .map(|(k, v)| {
                        let kk = if k > n / 2 { n - k } else { k };
                        if kk >= *lo && kk < *hi { *v } else { Complex32::new(0.0, 0.0) }
                    })
                    .collect();
                ifft.process(&mut band);
                let g = 1.0 / n as f32;
                out[si][ci] = band.iter().map(|v| v.re * g + 0.01 * ((h.below_or_equal(2000) as f32 / 1000.0) - 1.0)).collect();
            }
        }
        Ok(out)
    }

    fn sdr(reference: &[f32], x: &[f32]) -> f64 {
        let sig: f64 = reference.iter().map(|v| f64::from(*v).powi(2)).sum();
        let err: f64 = reference.iter().zip(x).map(|(a, b)| f64::from(a - b).powi(2)).sum();
        10.0 * (sig / err.max(1e-12)).log10()
    }

    #[test]
    fn shift_averaging_is_no_worse_than_one_pass_and_repeats_exactly() {
        // 8 kHz "session": a sine bass, a chord pad, noise-burst drums; six seconds.
        let sr = 8000usize;
        let n = 6 * sr;
        let t = |i: usize| i as f32 / sr as f32;
        let bass: Vec<f32> = (0..n).map(|i| 0.4 * (2.0 * std::f32::consts::PI * 55.0 * t(i)).sin()).collect();
        let pad: Vec<f32> =
            (0..n).map(|i| [440.0f32, 554.4, 659.3].iter().map(|f| 0.1 * (2.0 * std::f32::consts::PI * f * t(i)).sin()).sum::<f32>()).collect();
        let mut h = ShiftRng::new(99);
        let drums: Vec<f32> = (0..n)
            .map(|i| {
                let beat = (i % (sr / 2)) as f32 / sr as f32;
                let env = if beat < 0.08 { 1.0 - beat / 0.08 } else { 0.0 };
                // High-passed by differencing two noise values.
                0.5 * env * ((h.below_or_equal(2000) as f32 - h.below_or_equal(2000) as f32) / 2000.0)
            })
            .collect();
        let mix: Vec<f32> = (0..n).map(|i| bass[i] + pad[i] + drums[i]).collect();
        let mix = vec![mix.clone(), mix];
        let ctx = Ctx::default();
        let (segment, max_shift) = (sr * 2, sr / 2);
        let fast = demucs_split(&mix, segment, 0.25, 1.0, &ctx, band_split).unwrap();
        let high = demucs_shifts(&mix, HIGH_SHIFTS, max_shift, SHIFT_SEED, segment, HIGH_OVERLAP, 1.0, &ctx, band_split).unwrap();
        let again = demucs_shifts(&mix, HIGH_SHIFTS, max_shift, SHIFT_SEED, segment, HIGH_OVERLAP, 1.0, &ctx, band_split).unwrap();
        assert_eq!(high, again, "a fixed seed separates the same way twice");
        assert_eq!(high.len(), 3);
        assert!(high.iter().all(|s| s.len() == 2 && s[0].len() == n));
        for (i, src) in [&bass, &pad, &drums].into_iter().enumerate() {
            let f = sdr(src, &fast[i][0]);
            let g = sdr(src, &high[i][0]);
            assert!(g >= f - 1e-6, "stem {i}: fast {f:.2} dB, high {g:.2} dB");
        }
        // Another seed gives another (equally valid) answer; zero shifts is one plain pass.
        let other = demucs_shifts(&mix, HIGH_SHIFTS, max_shift, 7, segment, HIGH_OVERLAP, 1.0, &ctx, band_split).unwrap();
        assert_ne!(other, high);
        let plain = demucs_shifts(&mix, 0, max_shift, 1, segment, 0.25, 1.0, &ctx, band_split).unwrap();
        assert_eq!(plain, fast);
    }

    #[test]
    fn quality_names_and_passes() {
        for q in Quality::ALL {
            assert_eq!(Quality::parse(q.id()), Some(q));
        }
        assert_eq!(Quality::parse("medium"), None);
        assert_eq!((Quality::Fast.passes(), Quality::High.passes()), (1, HIGH_SHIFTS));
        assert_eq!((Quality::Fast.overlap(0.25), Quality::High.overlap(0.25), Quality::High.overlap(0.75)), (0.25, 0.5, 0.75));
        assert_eq!(ShiftRng::new(1).below_or_equal(0), 0);
    }
}
