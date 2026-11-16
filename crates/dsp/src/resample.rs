//! Reading recorded audio at a fractional position while it is played faster or slower (the
//! sampler and Juke): band-limited when it is played faster.
//!
//! At or below its own speed (a step of at most one source frame per output frame) a sample is
//! read with the 4-point cubic the instruments always used: nothing above the output's Nyquist
//! can be made, so there is nothing to filter. Faster than that (pitched up, or a sample at a
//! higher rate than the session's) the cubic folds everything between the output's Nyquist and
//! the sample's back down as aliasing. There [`read`] uses a Kaiser-windowed sinc (8 zero
//! crossings each side) stretched by the step, so it low-passes at 92 % of the output's Nyquist
//! as it reads. Its cost grows with the step: about 17 taps per octave up per side, capped at
//! [`MAX_STEP`] (four octaves; past that the filter stops getting narrower).

use std::sync::OnceLock;

/// Zero crossings of the sinc each side.
const ZEROS: usize = 8;
/// Table points per zero crossing (linearly interpolated between).
const PHASES: usize = 256;
/// The Kaiser window's beta (sidelobes about 80 dB down).
const BETA: f64 = 8.0;
/// The cutoff, as a fraction of the output's Nyquist.
const CUTOFF: f64 = 0.92;
/// The largest step the filter follows.
pub const MAX_STEP: f64 = 16.0;

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let y = x * x / 4.0;
    for k in 1..50 {
        term *= y / (k * k) as f64;
        sum += term;
        if term < sum * 1e-12 {
            break;
        }
    }
    sum
}

/// The windowed sinc from 0 to `ZEROS` zero crossings (one past, as 0, for the interpolation).
fn table() -> &'static [f32] {
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    T.get_or_init(|| {
        let norm = bessel_i0(BETA);
        (0..=ZEROS * PHASES + 1)
            .map(|i| {
                let x = i as f64 / PHASES as f64;
                if x >= ZEROS as f64 {
                    return 0.0;
                }
                let px = std::f64::consts::PI * x;
                let sinc = if i == 0 { 1.0 } else { px.sin() / px };
                let r = x / ZEROS as f64;
                (sinc * bessel_i0(BETA * (1.0 - r * r).max(0.0).sqrt()) / norm) as f32
            })
            .collect()
    })
}

/// Makes the filter's table now: call it off the audio thread (an instrument's constructor), so
/// the first pitched-up note does not.
pub(crate) fn warm() {
    let _ = table();
}

/// 4-point cubic (Catmull-Rom) interpolation of `x` at fractional frame `pos` (>= 0). Reads
/// outside the slice are silence.
#[inline]
pub(crate) fn hermite(x: &[f32], pos: f64) -> f32 {
    let i = pos as usize;
    let t = (pos - i as f64) as f32;
    let at = |k: usize| x.get(k).copied().unwrap_or(0.0);
    let x0 = at(i);
    if t == 0.0 {
        return if x0.is_finite() { x0 } else { 0.0 };
    }
    let xm1 = at(i.saturating_sub(1));
    let x1 = at(i.saturating_add(1));
    let x2 = at(i.saturating_add(2));
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    let y = ((c3 * t + c2) * t + c1) * t + x0;
    // A float file can hold NaN or infinities; they play as silence.
    if y.is_finite() { y } else { 0.0 }
}

/// `x` at fractional frame `pos` (>= 0, or a mirrored read for a negative one is silence), read
/// for a playback step of `step` source frames per output frame (see the module docs).
#[inline]
pub(crate) fn read(x: &[f32], pos: f64, step: f64) -> f32 {
    let step = step.abs();
    if step.is_nan() || step <= 1.0 || !pos.is_finite() {
        return hermite(x, pos.max(0.0));
    }
    let t = table();
    let scale = (CUTOFF / step.min(MAX_STEP)).min(1.0);
    let reach = ZEROS as f64 / scale;
    let lo = (pos - reach).ceil().max(0.0);
    let hi = (pos + reach).floor().min(x.len() as f64 - 1.0);
    if hi < lo {
        return 0.0;
    }
    let k = (PHASES as f64 * scale) as f32;
    let mut acc = 0.0f32;
    let first = lo as usize;
    let mut d = (lo - pos) as f32 * k;
    let inc = k;
    for v in x.get(first..=hi as usize).unwrap_or_default() {
        let a = d.abs();
        let i = a as usize;
        if let (Some(&w0), Some(&w1)) = (t.get(i), t.get(i + 1)) {
            acc += v * (w0 + (w1 - w0) * (a - i as f32));
        }
        d += inc;
    }
    let y = acc * scale as f32;
    if y.is_finite() { y } else { 0.0 }
}

/// [`read`] of audio reached through `get` (sample `i`, `None` past the end), for audio that is
/// not one slice: a clip's file streamed from disk. `None` when `pos` is outside the audio (as a
/// plain read past the end is silence). Never allocates.
pub fn read_with(mut get: impl FnMut(usize) -> Option<f32>, pos: f64, step: f64) -> Option<f32> {
    if !pos.is_finite() || pos < 0.0 {
        return None;
    }
    let i0 = pos as usize;
    let x0 = get(i0)?;
    let mut at = |k: i64| usize::try_from(k).ok().and_then(&mut get).filter(|v| v.is_finite()).unwrap_or(0.0);
    let step = step.abs();
    let y = if step.is_nan() || step <= 1.0 {
        let t = (pos - i0 as f64) as f32;
        if t == 0.0 {
            x0
        } else {
            let i = i0 as i64;
            let (xm1, x1, x2) = (at(i - 1), at(i + 1), at(i + 2));
            let c1 = 0.5 * (x1 - xm1);
            let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
            let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
            ((c3 * t + c2) * t + c1) * t + x0
        }
    } else {
        let t = table();
        let scale = (CUTOFF / step.min(MAX_STEP)).min(1.0);
        let reach = ZEROS as f64 / scale;
        let (lo, hi) = ((pos - reach).ceil().max(0.0) as i64, (pos + reach).floor() as i64);
        let k = (PHASES as f64 * scale) as f32;
        let mut acc = 0.0f32;
        let mut d = (lo as f64 - pos) as f32 * k;
        for j in lo..=hi {
            let a = d.abs();
            let i = a as usize;
            if let (Some(&w0), Some(&w1)) = (t.get(i), t.get(i + 1)) {
                acc += at(j) * (w0 + (w1 - w0) * (a - i as f32));
            }
            d += k;
        }
        acc * scale as f32
    };
    Some(if y.is_finite() { y } else { 0.0 })
}

#[cfg(test)]
mod read_with_tests {
    use super::*;

    #[test]
    fn read_with_matches_the_slice_read() {
        let x: Vec<f32> = (0..4000).map(|i| ((i as f32 * 0.37).sin() + (i as f32 * 2.1).sin()) * 0.4).collect();
        for step in [0.5, 1.0, 1.7, 3.0] {
            for pos in [100.0, 1234.25, 2000.5, 3500.9] {
                let a = read(&x, pos, step);
                let b = read_with(|i| x.get(i).copied(), pos, step).unwrap();
                assert!((a - b).abs() < 1e-5, "{step} {pos}: {a} {b}");
            }
        }
        assert_eq!(read_with(|i| x.get(i).copied(), 4000.0, 1.0), None);
        assert_eq!(read_with(|i| x.get(i).copied(), -1.0, 1.0), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustfft::FftPlanner;
    use rustfft::num_complex::Complex;

    /// The level (dB, relative to `full`) of the strongest bin within 3 bins of `bin`.
    fn level_at(mag: &[f32], bin: usize, full: f32) -> f32 {
        let m = mag[bin.saturating_sub(3)..=(bin + 3).min(mag.len() - 1)].iter().cloned().fold(0.0, f32::max);
        20.0 * (m / full).max(1e-12).log10()
    }

    fn spectrum(y: &[f32]) -> Vec<f32> {
        let n = y.len();
        let mut buf: Vec<Complex<f32>> =
            y.iter().enumerate().map(|(i, v)| Complex::new(v * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()), 0.0)).collect();
        FftPlanner::new().plan_fft_forward(n).process(&mut buf);
        buf[..n / 2].iter().map(|c| c.norm()).collect()
    }

    /// A tone near the top of the sample played an octave up lands above the output's Nyquist:
    /// the cubic folds it back at nearly full level, the band-limited read removes it, and a tone
    /// low enough to stay below keeps its level.
    #[test]
    fn pitching_up_does_not_fold_the_top_back() {
        let n = 1 << 14;
        let src_len = 2 * n + 64;
        // tones on the output's FFT bins: about 0.35 of the sample's rate (an octave up it would
        // be at 0.7, folding to 0.3) and about 0.1 (an octave up, 0.2: it fits)
        let (fold_bin, low_bin) = (4915, 3277);
        let src_hz = |out_bin: usize| out_bin as f32 / (2.0 * n as f32);
        let high_hz = 0.5 - src_hz(fold_bin);
        let high: Vec<f32> = (0..src_len).map(|i| (std::f64::consts::TAU * f64::from(high_hz) * i as f64).sin() as f32).collect();
        let low: Vec<f32> = (0..src_len).map(|i| (std::f64::consts::TAU * f64::from(src_hz(low_bin)) * i as f64).sin() as f32).collect();
        let play = |x: &[f32], band: bool| -> Vec<f32> {
            (0..n).map(|i| if band { read(x, 2.0 * i as f64 + 16.0, 2.0) } else { hermite(x, 2.0 * i as f64 + 16.0) }).collect()
        };
        let full = n as f32 / 4.0; // a unit sine's peak bin under the Hann window
        let folded = fold_bin;
        let cubic = level_at(&spectrum(&play(&high, false)), folded, full);
        let sinc = level_at(&spectrum(&play(&high, true)), folded, full);
        assert!(cubic > -20.0, "the cubic folds it back: {cubic:.1} dB");
        assert!(sinc < -70.0, "band-limited: {sinc:.1} dB");
        let kept = level_at(&spectrum(&play(&low, true)), low_bin, full);
        assert!(kept.abs() < 0.2, "a tone that fits keeps its level: {kept:.2} dB");
    }

    #[test]
    fn at_or_below_its_speed_it_is_the_cubic_and_edges_are_safe() {
        let x: Vec<f32> = (0..100).map(|i| (i as f32 * 0.3).sin()).collect();
        for pos in [0.0, 3.25, 50.5, 98.9] {
            assert_eq!(read(&x, pos, 1.0), hermite(&x, pos));
            assert_eq!(read(&x, pos, 0.5), hermite(&x, pos));
        }
        for (pos, step) in [(0.0, 3.0), (99.0, 3.0), (500.0, 2.0), (-5.0, 2.0), (f64::NAN, 2.0), (10.0, f64::INFINITY), (10.0, 1e9)] {
            assert!(read(&x, pos, step).is_finite(), "{pos} {step}");
        }
        assert_eq!(read(&[], 3.0, 2.0), 0.0);
        // DC stays DC
        let dc = vec![0.5f32; 400];
        assert!((read(&dc, 200.3, 3.7) - 0.5).abs() < 0.005);
    }
}
