//! The real-time stretcher: a pitch-preserving time stretch read straight from a file, heard the
//! moment a stretch changes (a tempo edit, a warp-marker drag, Elastic Properties) until the
//! offline render (WSOLA, [`crate::offline`]) replaces it.
//!
//! Output sample `r` (the position in the stretched clip) is two grains half a grain apart. Grain
//! `j` of a stream covers `grain` output samples; it is centred on output position `c`, starts
//! reading the file at the position the stretch map gives `c`, and reads at the file's own speed
//! (times `pitch`), so the pitch holds whatever the stretch. Each grain is faded sin² over its
//! length, and the two streams' fades sum to one. Every sample depends only on `r`: a locate, a
//! loop wrap, a block edge or a new session snapshot plays exactly what playing through would,
//! with no state to keep and nothing allocated, so it runs on the audio thread in the clip read.
//! Nothing is read ahead of the nominal position by more than half a grain, and the output is not
//! delayed: no latency to report.
//!
//! Each grain plays the file at its own pitch (the tests hold it within 1 %, measured in the
//! middle of grains). Where two grains overlap they are at different phases, so a steady tone
//! beats at the grain rate and its long-term spectrum can sit up to half the grain rate (25 Hz
//! with 40 ms grains) off: plain grains are phasier than WSOLA on sustained material, which is
//! why the offline render replaces this as soon as it is ready.

/// The grain the clip read uses, in seconds.
pub const GRAIN_SECS: f64 = 0.04;

/// Output sample `r` of a stretch whose map takes an output position to a file position
/// (`map`, monotonic, fractional), reading the file with `read(file position, step)` (a
/// band-limited reader; `step` the file samples read per output sample) and grains of `grain`
/// output samples, transposed by `pitch` (1: none). `None` where neither grain has audio.
/// Never allocates.
pub fn stretch_at(mut read: impl FnMut(f64, f64) -> Option<f32>, map: impl Fn(f64) -> f64, r: f64, grain: f64, pitch: f64) -> Option<f32> {
    let grain = if grain.is_finite() && grain >= 2.0 { grain } else { 2.0 };
    let pitch = if pitch.is_finite() && pitch > 0.0 { pitch } else { 1.0 };
    let mut out: Option<f32> = None;
    for k in [0.0, 0.5] {
        let x = r / grain + k;
        let j = x.floor();
        let phase = x - j;
        let centre = (j + 0.5 - k) * grain;
        let g = (std::f64::consts::PI * phase).sin().powi(2) as f32;
        if g == 0.0 {
            continue;
        }
        let at = map(centre) + (r - centre) * pitch;
        if let Some(v) = read(at, pitch) {
            out = Some(out.unwrap_or(0.0) + g * v);
        }
    }
    out
}

/// The file position output position `q` reads through a stretch map of `(file position, output
/// position)` points in output order: linear between neighbours, extended past the ends at the
/// slope of the nearest segment (or one to one with a single point). Never allocates.
pub fn map_at(points: &[(f64, f64)], q: f64) -> f64 {
    let n = points.len();
    let Some(&(f0, q0)) = points.first() else { return q };
    if n == 1 {
        return f0 + (q - q0);
    }
    let i = points.partition_point(|p| p.1 <= q).clamp(1, n - 1);
    let (Some(&(fa, qa)), Some(&(fb, qb))) = (points.get(i - 1), points.get(i)) else { return f0 + (q - q0) };
    let span = qb - qa;
    if span.abs() < 1e-12 {
        return fa;
    }
    fa + (q - qa) * (fb - fa) / span
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    /// A linear-interpolating reader of `v` (enough for a sine well under Nyquist).
    fn reader(v: &[f32]) -> impl FnMut(f64, f64) -> Option<f32> + '_ {
        move |f: f64, _step: f64| {
            if f < 0.0 {
                return None;
            }
            let i = f.floor() as usize;
            let t = (f - f.floor()) as f32;
            let a = *v.get(i)?;
            let b = v.get(i + 1).copied().unwrap_or(0.0);
            Some(a + (b - a) * t)
        }
    }

    fn sine(hz: f64, n: usize) -> Vec<f32> {
        (0..n).map(|i| (std::f64::consts::TAU * hz * i as f64 / SR).sin() as f32 * 0.5).collect()
    }

    fn render(v: &[f32], ratio: f64, n: usize, pitch: f64) -> Vec<f32> {
        let grain = SR * GRAIN_SECS;
        let mut read = reader(v);
        (0..n).map(|r| stretch_at(&mut read, |q| q / ratio, r as f64, grain, pitch).unwrap_or(0.0)).collect()
    }

    /// The frequency the grains play at, over output samples `[from, to)` of `v` (positions
    /// from the clip start): measured in the middle fifth of each grain, where one grain is all
    /// but alone (the other's fade is under a tenth), from the first to the last rising zero
    /// crossing, averaged over the grains. This is the pitch the stretcher holds; between grains
    /// two copies at different phases overlap, which a whole-span count or spectrum would mix
    /// into the measure (the phasiness the offline render removes).
    fn hz_of(v: &[f32], from: usize, to: usize) -> f64 {
        let grain = SR * GRAIN_SECS;
        let mut sum = 0.0;
        let mut n = 0.0;
        let mut j = (from as f64 / grain).ceil();
        while (j + 1.0) * grain < to as f64 {
            for k in [0.0, 0.5] {
                let centre = (j + 0.5 - k) * grain;
                let (a, b) = ((centre - 0.1 * grain) as usize, (centre + 0.1 * grain) as usize);
                let Some(w) = v.get(a..b) else { continue };
                let ups: Vec<f64> = w
                    .windows(2)
                    .enumerate()
                    .filter(|(_, x)| x[0] < 0.0 && x[1] >= 0.0)
                    .map(|(i, x)| i as f64 + f64::from(-x[0] / (x[1] - x[0])))
                    .collect();
                if let (Some(f), Some(l)) = (ups.first(), ups.last())
                    && ups.len() >= 3
                {
                    sum += (ups.len() - 1) as f64 * SR / (l - f);
                    n += 1.0;
                }
            }
            j += 1.0;
        }
        if n > 0.0 { sum / n } else { 0.0 }
    }

    fn rms(v: &[f32]) -> f64 {
        (v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / v.len().max(1) as f64).sqrt()
    }

    /// The stretched output is as long as asked, with the material all the way to its end; the
    /// pitch each grain plays holds within 1 % at half to double length; a transpose moves it by
    /// its ratio.
    #[test]
    fn a_stretch_keeps_the_pitch_and_fills_its_length() {
        let src = sine(440.0, 48_000);
        for ratio in [0.5, 0.8, 1.5, 2.0] {
            let n = (48_000.0 * ratio) as usize;
            let out = render(&src, ratio, n, 1.0);
            assert_eq!(out.len(), n);
            let hz = hz_of(&out, n / 10, n * 9 / 10);
            assert!((hz / 440.0 - 1.0).abs() < 0.01, "ratio {ratio}: {hz} Hz");
            // Level held through to the last tenth (the file is used up exactly at the end).
            let tail = &out[n * 8 / 10..n * 9 / 10];
            assert!(rms(tail) > 0.25, "ratio {ratio}: {}", rms(tail));
        }
        let out = render(&src, 1.0, 48_000, 2f64.powf(3.0 / 12.0));
        let hz = hz_of(&out, 4_800, 43_200);
        assert!((hz / (440.0 * 2f64.powf(0.25)) - 1.0).abs() < 0.01, "{hz}");
    }

    /// No clicks: block edges change nothing (each sample stands alone), and no step between
    /// neighbouring samples is much larger than the sine's own steepest.
    #[test]
    fn no_clicks_at_block_edges_or_between_grains() {
        let src = sine(220.0, 96_000);
        let whole = render(&src, 1.37, 100_000, 1.0);
        let grain = SR * GRAIN_SECS;
        let mut read = reader(&src);
        let mut blocks = Vec::new();
        for b in (0..100_000).step_by(61) {
            for r in b..(b + 61).min(100_000) {
                blocks.push(stretch_at(&mut read, |q| q / 1.37, r as f64, grain, 1.0).unwrap_or(0.0));
            }
        }
        assert_eq!(whole, blocks, "rendered in odd blocks: the very same samples");
        let steepest = 0.5 * std::f64::consts::TAU * 220.0 / SR;
        let worst = whole[2_000..95_000].windows(2).map(|w| f64::from((w[1] - w[0]).abs())).fold(0.0, f64::max);
        assert!(worst < steepest * 1.6, "{worst} vs {steepest}");
    }

    /// A loop wrap is a jump back in output position: what plays after it is what played the
    /// first time through, sample for sample.
    #[test]
    fn a_loop_wrap_plays_what_playing_through_did() {
        let src = sine(330.0, 48_000);
        let grain = SR * GRAIN_SECS;
        let mut read = reader(&src);
        let mut at = |r: usize| stretch_at(&mut read, |q| q / 1.25, r as f64, grain, 1.0).unwrap_or(0.0);
        let first: Vec<f32> = (10_000..12_000).map(&mut at).collect();
        let _ = (20_000..30_000).map(&mut at).count();
        let again: Vec<f32> = (10_000..12_000).map(&mut at).collect();
        assert_eq!(first, again);
    }

    #[test]
    fn the_map_is_linear_between_points_and_extends_past_them() {
        let pts = [(1_000.0, 0.0), (2_000.0, 2_000.0), (2_500.0, 3_000.0)];
        assert_eq!(map_at(&pts, 0.0), 1_000.0);
        assert_eq!(map_at(&pts, 1_000.0), 1_500.0);
        assert_eq!(map_at(&pts, 2_500.0), 2_250.0);
        assert_eq!(map_at(&pts, 4_000.0), 3_000.0, "past the end at the last slope");
        assert_eq!(map_at(&pts, -200.0), 900.0);
        assert_eq!(map_at(&[], 5.0), 5.0);
        assert_eq!(map_at(&[(10.0, 0.0)], 5.0), 15.0);
    }
}
