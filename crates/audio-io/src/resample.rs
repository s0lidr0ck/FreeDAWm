//! Streaming sample-rate conversion for the lossy encoders, whose codecs only take some rates
//! (MP3 up to 48 kHz, Opus at 48 kHz).
//!
//! A Kaiser-windowed sinc read from a finely sampled table (the design `soundcraft-dsp`'s offline
//! resampler uses), fed block by block so a bounce of any length never has to fit in memory.
//! Output sample `n` is read at input time `n * from / to`: the filter is centred, so the output
//! has no delay; the input it still needs is held back until the next block or [`Resampler::finish`].

use std::f64::consts::PI;

/// Zero crossings of the sinc on each side, at the cutoff.
const ZERO_CROSSINGS: f64 = 32.0;
/// Table points per input sample.
const TABLE_RES: usize = 512;
/// Kaiser window shape.
const BETA: f64 = 9.0;

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, half) = (1.0, 1.0, x / 2.0);
    for k in 1..64 {
        let r = half / f64::from(k);
        term *= r * r;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// Converts planar audio from one rate to another, a block at a time.
pub(crate) struct Resampler {
    /// Output rate / input rate, reduced: output `n` reads input time `n * down / up`.
    up: u64,
    down: u64,
    /// One side of the kernel, `TABLE_RES` points per input sample.
    table: Vec<f32>,
    /// Half the kernel's width, in input samples.
    half_width: f64,
    /// Per channel: input samples from `hist_start` on.
    hist: Vec<Vec<f32>>,
    hist_start: u64,
    /// Input frames pushed so far.
    in_total: u64,
    /// The next output frame to compute.
    out_next: u64,
}

impl Resampler {
    /// A converter from `from` Hz to `to` Hz for `channels` channels (both rates non-zero).
    pub(crate) fn new(from: u32, to: u32, channels: usize) -> Resampler {
        let (from, to) = (u64::from(from.max(1)), u64::from(to.max(1)));
        let g = gcd(from, to);
        let ratio = to as f64 / from as f64;
        // Anti-alias below the lower of the two Nyquists, with a little transition room.
        let cutoff = (if ratio < 1.0 { ratio * 0.94 } else { 0.97 }).clamp(0.01, 1.0);
        let half_width = ZERO_CROSSINGS / cutoff;
        let n = (half_width * TABLE_RES as f64).ceil() as usize + 2;
        let i0b = bessel_i0(BETA);
        let table = (0..n)
            .map(|i| {
                let t = i as f64 / TABLE_RES as f64;
                let x = t / half_width;
                if x >= 1.0 {
                    return 0.0;
                }
                let arg = PI * cutoff * t;
                let sinc = if arg.abs() < 1e-12 { 1.0 } else { arg.sin() / arg };
                (cutoff * sinc * bessel_i0(BETA * (1.0 - x * x).max(0.0).sqrt()) / i0b) as f32
            })
            .collect();
        Resampler { up: to / g, down: from / g, table, half_width, hist: vec![Vec::new(); channels], hist_start: 0, in_total: 0, out_next: 0 }
    }

    fn kernel(&self, t: f64) -> f32 {
        let pos = t.abs() * TABLE_RES as f64;
        let i = pos as usize;
        let f = (pos - i as f64) as f32;
        match (self.table.get(i), self.table.get(i + 1)) {
            (Some(a), Some(b)) => a + (b - a) * f,
            (Some(a), None) => *a,
            _ => 0.0,
        }
    }

    /// Input time of output frame `n`.
    fn time_of(&self, n: u64) -> f64 {
        // `n * down` overflows only past ~2^44 output frames at the largest reduced ratio
        // (768 kHz to 8 kHz): far beyond any bounce. Saturating keeps it finite regardless.
        let num = n.saturating_mul(self.down);
        (num / self.up) as f64 + (num % self.up) as f64 / self.up as f64
    }

    /// Output frames for the input pushed so far, once all of it has gone through.
    fn out_len(&self) -> u64 {
        let num = u128::from(self.in_total) * u128::from(self.up);
        let den = u128::from(self.down);
        u64::try_from((num + den / 2) / den).unwrap_or(u64::MAX)
    }

    /// Feeds the first `frames` frames of `block` (missing samples are silence) and appends every
    /// output frame they complete to `out` (one `Vec` per channel).
    pub(crate) fn process(&mut self, block: &[Vec<f32>], frames: usize, out: &mut [Vec<f32>]) {
        for (c, h) in self.hist.iter_mut().enumerate() {
            let src = block.get(c).map_or(&[][..], |ch| ch.get(..frames.min(ch.len())).unwrap_or_default());
            h.extend_from_slice(src);
            h.resize(h.len() + (frames - src.len()), 0.0);
        }
        self.in_total += frames as u64;
        self.run(false, out);
    }

    /// Flushes the held-back input (the input ends in silence) into `out`.
    pub(crate) fn finish(&mut self, out: &mut [Vec<f32>]) {
        self.run(true, out);
    }

    fn run(&mut self, flush: bool, out: &mut [Vec<f32>]) {
        let end = if flush { self.out_len() } else { u64::MAX };
        while self.out_next < end {
            let t = self.time_of(self.out_next);
            let hi = (t + self.half_width).floor();
            if !flush && hi >= self.in_total as f64 {
                break;
            }
            let lo = (t - self.half_width).ceil().max(0.0) as u64;
            let hi = (hi.max(0.0) as u64).min(self.in_total.saturating_sub(1));
            for (h, o) in self.hist.iter().zip(out.iter_mut()) {
                let mut acc = 0.0f32;
                for j in lo.max(self.hist_start)..=hi {
                    if let Some(&v) = usize::try_from(j - self.hist_start).ok().and_then(|i| h.get(i)) {
                        acc += v * self.kernel(t - j as f64);
                    }
                }
                o.push(acc);
            }
            self.out_next += 1;
        }
        // Drop the input no later output reads.
        let keep_from = (self.time_of(self.out_next) - self.half_width).ceil().max(0.0) as u64;
        if keep_from > self.hist_start {
            let drop = usize::try_from(keep_from - self.hist_start).unwrap_or(usize::MAX);
            for h in &mut self.hist {
                h.drain(..drop.min(h.len()));
            }
            self.hist_start = keep_from;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(from: u32, to: u32, x: &[f32], block: usize) -> Vec<f32> {
        let mut r = Resampler::new(from, to, 1);
        let mut out = vec![Vec::new()];
        for chunk in x.chunks(block.max(1)) {
            r.process(&[chunk.to_vec()], chunk.len(), &mut out);
        }
        r.finish(&mut out);
        out.remove(0)
    }

    fn tone(sr: u32, hz: f64, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * PI * hz * i as f64 / f64::from(sr)).sin() as f32 * 0.5).collect()
    }

    #[test]
    fn length_and_tone_survive_conversion() {
        for (from, to) in [(96_000, 48_000), (44_100, 48_000), (192_000, 48_000), (88_200, 44_100), (22_050, 48_000), (44_101, 48_000)] {
            let n = from as usize / 2;
            let y = run(from, to, &tone(from, 1000.0, n), 1000);
            let want = (n as f64 * f64::from(to) / f64::from(from)).round() as usize;
            assert_eq!(y.len(), want, "{from}->{to}");
            let reference = tone(to, 1000.0, y.len());
            // Away from the edges (where the input starts and stops abruptly) the tone matches.
            let mid = &y[y.len() / 4..3 * y.len() / 4];
            let err = mid.iter().zip(&reference[y.len() / 4..]).map(|(a, b)| (a - b).powi(2)).sum::<f32>() / mid.len() as f32;
            assert!(err < 1e-7, "{from}->{to}: mean squared error {err}");
        }
    }

    #[test]
    fn block_size_does_not_change_the_output() {
        let x = tone(96_000, 3000.0, 20_000);
        let whole = run(96_000, 48_000, &x, x.len());
        for block in [1, 7, 512, 4096] {
            assert_eq!(run(96_000, 48_000, &x, block), whole, "block {block}");
        }
    }

    #[test]
    fn downsampling_removes_what_the_new_rate_cannot_hold() {
        // 30 kHz is above 24 kHz, the Nyquist of 48 kHz: it must not alias back down.
        let y = run(96_000, 48_000, &tone(96_000, 30_000.0, 96_000), 4096);
        let mid = &y[y.len() / 4..3 * y.len() / 4];
        let rms = (mid.iter().map(|v| v * v).sum::<f32>() / mid.len() as f32).sqrt();
        assert!(rms < 1e-3, "aliased energy {rms}");
    }

    #[test]
    fn empty_input_gives_empty_output() {
        assert!(run(96_000, 48_000, &[], 64).is_empty());
    }
}
