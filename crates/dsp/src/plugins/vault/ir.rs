//! A reverb impulse response made ready for Vault's convolution, off the audio thread: the file's
//! channels resampled to the session rate, trimmed, levelled and cut into partitions whose
//! spectra are taken once. The result is a [`SpaceIr`]: immutable, shared (`Arc`), handed to the
//! plugin with [`crate::Plugin::set_asset`]. The audio thread only swaps it in.
//!
//! The response is partitioned twice ([`super::conv`]): its first [`HEAD_LEN`] samples in blocks
//! of [`HEAD_BLOCK`] (run with no latency) and the rest in blocks of [`TAIL_BLOCK`] (cheap for a
//! long tail). Each partition keeps two spectra: the partition's, and the partition's times a
//! ramp from 0 to 1. A weighted sum of the two is the spectrum of the partition under any
//! straight-line gain, which is how Decay shortens a response without making it again.

use crate::SampleSource;
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

/// The head's partition and FFT sizes, and the bins kept (a real signal's spectrum is symmetric).
pub const HEAD_BLOCK: usize = 512;
pub const HEAD_FFT: usize = 2 * HEAD_BLOCK;
pub const HEAD_BINS: usize = HEAD_FFT / 2 + 1;
/// The tail's.
pub const TAIL_BLOCK: usize = 4096;
pub const TAIL_FFT: usize = 2 * TAIL_BLOCK;
pub const TAIL_BINS: usize = TAIL_FFT / 2 + 1;
/// Samples of the response the head runs: two tail blocks, the time the tail has to get a
/// block's work done in.
pub const HEAD_LEN: usize = 2 * TAIL_BLOCK;
/// The head's partitions at most.
pub const HEAD_SEGS: usize = HEAD_LEN / HEAD_BLOCK;
/// The longest response kept, in seconds at the session rate; a longer file is cut there and
/// faded.
pub const MAX_IR_SECONDS: f64 = 10.0;
/// The longest file read, in seconds at its own rate.
const MAX_FILE_SECONDS: f64 = 60.0;
/// The level a response is brought to: the root of the energy reaching one output.
const LEVEL: f32 = 0.5;

/// One channel's partitions of one stage.
#[derive(Debug, Clone, Default)]
pub(crate) struct Stage {
    pub(crate) segs: usize,
    bins: usize,
    /// `segs` spectra of `bins` bins: the partitions'.
    h: Vec<Complex<f32>>,
    /// The same partitions times a ramp from 0 to 1 over the block.
    r: Vec<Complex<f32>>,
}

impl Stage {
    fn new(samples: &[f32], block: usize) -> Stage {
        let (fft_len, bins) = (2 * block, block + 1);
        let segs = samples.len().div_ceil(block);
        let fft = FftPlanner::<f32>::new().plan_fft_forward(fft_len);
        let mut stage = Stage { segs, bins, h: Vec::with_capacity(segs * bins), r: Vec::with_capacity(segs * bins) };
        let mut buf = vec![Complex::new(0.0f32, 0.0); fft_len];
        for part in samples.chunks(block) {
            for ramp in [false, true] {
                buf.iter_mut().for_each(|c| *c = Complex::new(0.0, 0.0));
                for (n, (c, s)) in buf.iter_mut().zip(part).enumerate() {
                    c.re = if ramp { *s * n as f32 / block as f32 } else { *s };
                }
                fft.process(&mut buf);
                let into = if ramp { &mut stage.r } else { &mut stage.h };
                into.extend_from_slice(buf.get(..bins).unwrap_or(&[]));
            }
        }
        stage
    }

    /// Partition `k`'s spectrum and its ramped one (empty past the last).
    pub(crate) fn seg(&self, k: usize) -> (&[Complex<f32>], &[Complex<f32>]) {
        let at = k * self.bins..(k + 1) * self.bins;
        (self.h.get(at.clone()).unwrap_or(&[]), self.r.get(at).unwrap_or(&[]))
    }
}

/// A reverb impulse response ready for Vault.
#[derive(Debug, Clone)]
pub struct SpaceIr {
    rate: f32,
    /// The response as it is convolved with: 1 channel (mono), 2 (a stereo pair) or 4 (true
    /// stereo: left in to left and right out, then right in to left and right out).
    samples: Vec<Vec<f32>>,
    head: Vec<Stage>,
    tail: Vec<Stage>,
}

impl SampleSource for SpaceIr {
    fn sample_rate(&self) -> f32 {
        self.rate
    }
    fn num_channels(&self) -> usize {
        self.samples.len()
    }
    fn channel(&self, ch: usize) -> Option<&[f32]> {
        self.samples.get(ch).map(Vec::as_slice)
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// The usable response behind a shared asset (`None`: not one made by [`SpaceIr::new`]).
pub fn as_space(s: &dyn SampleSource) -> Option<&SpaceIr> {
    s.as_any()?.downcast_ref::<SpaceIr>()
}

impl SpaceIr {
    /// Makes the response from a file's channels (at `file_rate`) for a session at `rate`. One
    /// channel is a mono response, four are true stereo, anything else is read as a stereo pair
    /// (its first two channels). Errors name what is wrong; nothing is half-made.
    pub fn new(channels: &[Vec<f32>], file_rate: f64, rate: f64) -> Result<SpaceIr, String> {
        if !(file_rate.is_finite() && (1000.0..=768_000.0).contains(&file_rate)) {
            return Err("the file has no usable sample rate".into());
        }
        if !(rate.is_finite() && (1000.0..=768_000.0).contains(&rate)) {
            return Err(format!("unusable session rate {rate}"));
        }
        let take = match channels.len() {
            0 => return Err("no audio".into()),
            1 => 1,
            4 => 4,
            _ => 2,
        };
        let used = channels.get(..take).unwrap_or(&[]);
        let frames = used.iter().map(Vec::len).min().unwrap_or(0);
        if frames == 0 {
            return Err("no audio".into());
        }
        if frames as f64 > MAX_FILE_SECONDS * file_rate {
            return Err(format!("too long for an impulse response: {:.0} s (at most {MAX_FILE_SECONDS:.0} s)", frames as f64 / file_rate));
        }
        if used.iter().flat_map(|c| c.iter().take(frames)).any(|x| !x.is_finite()) {
            return Err("the file holds non-finite samples".into());
        }
        let cut: Vec<Vec<f32>> = used.iter().map(|c| c.get(..frames).unwrap_or(&[]).to_vec()).collect();
        let mut samples = crate::offline::resample(&cut, file_rate.round() as u32, rate.round() as u32);
        // Trim what is 80 dB under the peak at both ends (every channel by the same amount).
        let peak = samples.iter().flatten().fold(0.0f32, |m, x| m.max(x.abs()));
        if !(peak.is_finite() && peak > 1.0e-9) {
            return Err("the file is silent".into());
        }
        let len = samples.iter().map(Vec::len).min().unwrap_or(0);
        let loud = |i: usize| samples.iter().any(|c| c.get(i).is_some_and(|x| x.abs() >= peak * 1.0e-4));
        let start = (0..len).find(|&i| loud(i)).unwrap_or(0);
        let end = (start..len).rev().find(|&i| loud(i)).map_or(start + 1, |i| i + 1);
        let limit = (MAX_IR_SECONDS * rate).floor() as usize;
        let keep = (end - start).min(limit).max(1);
        let fade = if end - start > limit { (0.05 * rate) as usize } else { 0 };
        for c in &mut samples {
            *c = c.get(start..start + keep).unwrap_or(&[]).to_vec();
            let n = c.len();
            for (i, x) in c.iter_mut().enumerate().skip(n.saturating_sub(fade)) {
                *x *= (n - i) as f32 / (fade + 1) as f32;
            }
        }
        // One level for every response: the energy reaching the louder output.
        let energy = |c: &[f32]| c.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>();
        let e: Vec<f64> = samples.iter().map(|c| energy(c)).collect();
        let reaching = if take == 4 { (e[0] + e[2]).max(e[1] + e[3]) } else { e.iter().copied().fold(0.0, f64::max) };
        if !(reaching.is_finite() && reaching > 0.0) {
            return Err("the file is silent".into());
        }
        let gain = LEVEL / reaching.sqrt() as f32;
        samples.iter_mut().flatten().for_each(|x| *x *= gain);
        let head = samples.iter().map(|c| Stage::new(c.get(..c.len().min(HEAD_LEN)).unwrap_or(&[]), HEAD_BLOCK)).collect();
        let tail = samples.iter().map(|c| Stage::new(c.get(HEAD_LEN..).unwrap_or(&[]), TAIL_BLOCK)).collect();
        Ok(SpaceIr { rate: rate as f32, samples, head, tail })
    }

    /// The space Vault plays until a response is loaded: a made-up hall, generated here (two
    /// unrelated noise decays, the top of the spectrum dying away twice as fast as the bottom,
    /// after a few milliseconds of build-up). Nothing is recorded or taken from anywhere.
    pub fn built_in(rate: f64) -> Result<SpaceIr, String> {
        let rate = if rate.is_finite() && rate >= 1000.0 { rate.min(768_000.0) } else { 48_000.0 };
        let len = (1.8 * rate) as usize;
        let lp = (-std::f64::consts::TAU * 2200.0 / rate).exp() as f32;
        let channels: Vec<Vec<f32>> = [0x2545_F491u32, 0x9E37_79B9]
            .iter()
            .map(|seed| {
                let mut state = *seed;
                let mut low = 0.0f32;
                (0..len)
                    .map(|n| {
                        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        let noise = (state >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
                        low = noise + (low - noise) * lp;
                        let t = n as f32 / rate as f32;
                        let build = (t / 0.004).min(1.0);
                        // 60 dB down after 1.7 s (lows) and 0.8 s (highs)
                        build * (low * (-6.91 * t / 1.7).exp() + (noise - low) * 0.6 * (-6.91 * t / 0.8).exp())
                    })
                    .collect()
            })
            .collect();
        SpaceIr::new(&channels, rate, rate)
    }

    /// Frames per channel.
    pub fn len(&self) -> usize {
        self.samples.first().map_or(0, Vec::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn head(&self, ch: usize) -> Option<&Stage> {
        self.head.get(ch)
    }

    pub(crate) fn tail(&self, ch: usize) -> Option<&Stage> {
        self.tail.get(ch)
    }

    /// Partitions of the head and of the tail.
    pub(crate) fn segs(&self) -> (usize, usize) {
        (self.head.first().map_or(0, |s| s.segs), self.tail.first().map_or(0, |s| s.segs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decay(len: usize, seed: f32) -> Vec<f32> {
        (0..len).map(|i| ((i as f32) * seed).sin() * (-(i as f32) / (len as f32 / 6.0)).exp()).collect()
    }

    #[test]
    fn channel_counts_map_to_mono_stereo_and_true_stereo() {
        let c = decay(20_000, 0.37);
        for (given, kept) in [(1, 1), (2, 2), (3, 2), (4, 4), (6, 2)] {
            let ir = SpaceIr::new(&vec![c.clone(); given], 48_000.0, 48_000.0).unwrap();
            assert_eq!(ir.num_channels(), kept, "{given} channels");
        }
        assert!(SpaceIr::new(&[], 48_000.0, 48_000.0).is_err());
    }

    #[test]
    fn the_response_is_trimmed_levelled_and_partitioned() {
        let mut c = vec![0.0f32; 1000];
        c.extend(decay(30_000, 0.37));
        c.extend(vec![0.0f32; 5000]);
        let ir = SpaceIr::new(&[c.clone(), c.iter().map(|x| x * 0.5).collect()], 48_000.0, 48_000.0).unwrap();
        assert!(ir.len() < 30_000 && ir.len() > 20_000, "{}", ir.len());
        let e: f32 = ir.channel(0).unwrap().iter().map(|x| x * x).sum();
        assert!((e.sqrt() - LEVEL).abs() < 1.0e-3, "the louder channel at the level: {}", e.sqrt());
        let e1: f32 = ir.channel(1).unwrap().iter().map(|x| x * x).sum();
        assert!((e1.sqrt() - LEVEL / 2.0).abs() < 1.0e-3, "the balance is kept");
        assert_eq!(ir.segs(), (HEAD_SEGS, (ir.len() - HEAD_LEN).div_ceil(TAIL_BLOCK)));
        // a short response has no tail
        let short = SpaceIr::new(&[decay(700, 0.2)], 48_000.0, 48_000.0).unwrap();
        assert_eq!(short.segs(), (short.len().div_ceil(HEAD_BLOCK), 0));
    }

    #[test]
    fn other_rates_resample_and_a_long_file_is_cut_at_ten_seconds() {
        let c = decay(44_100, 0.2);
        let ir = SpaceIr::new(&[c], 44_100.0, 96_000.0).unwrap();
        assert_eq!(ir.sample_rate(), 96_000.0);
        assert!((ir.len() as f64 - 96_000.0).abs() < 20_000.0, "{}", ir.len());
        let long = vec![0.25f32; 8000 * 12];
        let ir = SpaceIr::new(&[long], 8000.0, 8000.0).unwrap();
        assert_eq!(ir.len(), 80_000);
        assert!(ir.channel(0).unwrap()[79_999].abs() < 1.0e-3, "faded out");
    }

    #[test]
    fn bad_inputs_are_errors() {
        assert!(SpaceIr::new(&[vec![0.5]], 0.0, 48_000.0).is_err());
        assert!(SpaceIr::new(&[vec![0.5]], 48_000.0, f64::NAN).is_err());
        assert!(SpaceIr::new(&[vec![f32::NAN; 10]], 48_000.0, 48_000.0).is_err());
        assert!(SpaceIr::new(&[vec![0.0; 100]], 48_000.0, 48_000.0).unwrap_err().contains("silent"));
        assert!(SpaceIr::new(&[vec![]], 48_000.0, 48_000.0).is_err());
        assert!(SpaceIr::new(&[vec![0.1; 8000 * 61]], 8000.0, 48_000.0).unwrap_err().contains("too long"));
    }

    #[test]
    fn the_built_in_space_is_a_stereo_decay_at_any_rate() {
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            let ir = SpaceIr::built_in(rate).unwrap();
            assert_eq!(ir.num_channels(), 2);
            let secs = ir.len() as f64 / rate;
            assert!((1.0..=1.81).contains(&secs), "{secs}");
            let (l, r) = (ir.channel(0).unwrap(), ir.channel(1).unwrap());
            let dot: f32 = l.iter().zip(r).map(|(a, b)| a * b).sum();
            assert!(dot.abs() < 0.05 * LEVEL * LEVEL, "the two sides are unrelated: {dot}");
            let first: f32 = l[..l.len() / 4].iter().map(|x| x * x).sum();
            let last: f32 = l[3 * l.len() / 4..].iter().map(|x| x * x).sum();
            assert!(last < first * 1.0e-3, "it decays");
        }
    }
}
