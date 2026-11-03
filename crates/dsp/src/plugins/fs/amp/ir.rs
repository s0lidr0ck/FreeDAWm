//! The cabinet impulse response, made ready off the audio thread: what JUCE's
//! `dsp::Convolution::loadImpulseResponse (file, Stereo::no, Trim::yes, 0, Normalise::yes)` does
//! to a file before its engine runs (`juce_Convolution.cpp`: `loadStreamToBuffer`,
//! `fixNumChannels`, `trimImpulseResponse`, `resampleImpulseResponse` through
//! `ResamplingAudioSource`, `normaliseImpulseResponse`), then the engine's partitioning: the
//! response cut into blocks and each block's spectrum taken once (the `ConvolutionEngine`
//! constructor). Translated from the JUCE 8 sources the C++ is built with.
//!
//! The result is an [`ImpulseAsset`]: immutable, shared (`Arc`), handed to the plugin with
//! [`crate::Plugin::set_asset`]. The audio thread only swaps it in; the partition spectra are
//! never computed there.

use crate::SampleSource;
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

/// The convolution's partition (`ConvolutionEngine::blockSize`). JUCE uses the next power of two
/// of the host's block; the render tool's is 512. Any partition gives the same convolution (to
/// rounding), so one fixed size lets the spectra be made before the asset reaches a plugin.
pub const CONV_BLOCK: usize = 512;
/// `fftSize`: twice the block (blocks above 128).
pub const FFT_SIZE: usize = 2 * CONV_BLOCK;
/// Spectrum bins kept per segment (a real signal's spectrum is symmetric).
pub const BINS: usize = FFT_SIZE / 2 + 1;
/// The longest impulse response accepted, in seconds at the session rate (Rust: JUCE takes any
/// length; a cabinet's is well under a second, the owner's longest 2.9 s once trimmed). The
/// plugin's convolution state is sized for it when it is prepared.
pub const MAX_IR_SECONDS: f64 = 4.0;

/// An impulse response ready for the amp's convolution.
#[derive(Debug, Clone)]
pub struct ImpulseAsset {
    rate: f32,
    /// The response as the engine convolves with it: first channel, trimmed, at `rate`,
    /// normalised.
    samples: Vec<f32>,
    /// `num_segments` spectra of `BINS` bins each.
    spectra: Vec<Complex<f32>>,
    num_segments: usize,
}

impl SampleSource for ImpulseAsset {
    fn sample_rate(&self) -> f32 {
        self.rate
    }
    fn num_channels(&self) -> usize {
        1
    }
    fn channel(&self, ch: usize) -> Option<&[f32]> {
        (ch == 0).then_some(self.samples.as_slice())
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// Segments needed for `len` samples (`numSamples / (fftSize - blockSize) + 1`).
pub fn segments_for(len: usize) -> usize {
    len / (FFT_SIZE - CONV_BLOCK) + 1
}

/// The most segments an asset may have at `rate` (what a plugin prepared at `rate` sizes for).
pub fn max_segments(rate: f64) -> usize {
    let rate = if rate.is_finite() && rate > 0.0 { rate.min(768_000.0) } else { 48_000.0 };
    segments_for((MAX_IR_SECONDS * rate).ceil() as usize)
}

impl ImpulseAsset {
    /// Makes the response the convolution runs from a file's first channel (`ch0`, at
    /// `file_rate`) for a session at `rate`. Errors name what is wrong; nothing is half-made.
    pub fn new(ch0: &[f32], file_rate: f64, rate: f64) -> Result<ImpulseAsset, String> {
        if !(file_rate.is_finite() && file_rate > 0.0) {
            return Err("the file has no sample rate".into());
        }
        if !(rate.is_finite() && rate > 0.0 && rate <= 768_000.0) {
            return Err(format!("unusable session rate {rate}"));
        }
        if ch0.iter().any(|x| !x.is_finite()) {
            return Err("the file holds non-finite samples".into());
        }
        // fixNumChannels: no samples at all is a unit impulse
        let fixed: &[f32] = if ch0.is_empty() { &[1.0] } else { ch0 };
        let trimmed = trim_impulse_response(fixed);
        let out_len = if approximately_equal(file_rate, rate) { trimmed.len() } else { resampled_len(trimmed.len(), file_rate / rate) };
        let limit = (MAX_IR_SECONDS * rate).ceil() as usize;
        if out_len > limit {
            return Err(format!(
                "too long for a cabinet impulse response: {:.1} s once trimmed (at most {MAX_IR_SECONDS:.0} s)",
                out_len as f64 / rate
            ));
        }
        let mut samples = resample_impulse_response(&trimmed, file_rate, rate);
        normalise_impulse_response(&mut samples);
        let (spectra, num_segments) = partition(&samples);
        Ok(ImpulseAsset { rate: rate as f32, samples, spectra, num_segments })
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn num_segments(&self) -> usize {
        self.num_segments
    }

    /// Segment `i`'s spectrum (`BINS` bins).
    pub(crate) fn segment(&self, i: usize) -> &[Complex<f32>] {
        self.spectra.get(i * BINS..(i + 1) * BINS).unwrap_or(&[])
    }
}

/// `approximatelyEqual` for doubles (JUCE 8: relative to the larger, with an absolute floor).
fn approximately_equal(a: f64, b: f64) -> bool {
    let diff = (a - b).abs();
    diff <= f64::MIN_POSITIVE || diff <= f64::EPSILON * a.abs().max(b.abs())
}

/// `trimImpulseResponse`: drops what is below -80 dB at both ends.
fn trim_impulse_response(buf: &[f32]) -> Vec<f32> {
    let threshold = 10f32.powf(-80.0 / 20.0); // Decibels::decibelsToGain (-80.0f)
    let n = buf.len();
    let begin = buf.iter().position(|s| s.abs() >= threshold).unwrap_or(n);
    let end = buf.iter().rev().position(|s| s.abs() >= threshold).unwrap_or(n);
    if begin == n {
        return vec![0.0];
    }
    let new_len = n.saturating_sub(begin + end).max(1);
    buf.get(begin..begin + new_len).map(<[f32]>::to_vec).unwrap_or_else(|| vec![0.0])
}

/// `calculateNormalisationFactor` and `normaliseImpulseResponse`: float sums, as JUCE.
fn normalise_impulse_response(buf: &mut [f32]) {
    let sum = buf.iter().fold(0.0f32, |acc, s| acc + s * s);
    let factor = if sum < 1e-8 { 1.0 } else { 0.125f32 / sum.sqrt() };
    for s in buf.iter_mut() {
        *s *= factor;
    }
}

/// `roundToInt (jmax (1.0, numSamples / factorReading))`.
fn resampled_len(n: usize, factor_reading: f64) -> usize {
    (n as f64 / factor_reading).max(1.0).round() as usize
}

/// `resampleImpulseResponse`: a `ResamplingAudioSource` over a `MemoryAudioSource`, one block of
/// the final size.
fn resample_impulse_response(buf: &[f32], src_rate: f64, dest_rate: f64) -> Vec<f32> {
    if approximately_equal(src_rate, dest_rate) {
        return buf.to_vec();
    }
    let ratio = src_rate / dest_rate; // samplesInPerOutputSample
    let final_size = resampled_len(buf.len(), ratio);
    let mut r = Resampler::new(ratio);
    r.render(buf, final_size)
}

/// JUCE's `ResamplingAudioSource` for one channel and one `getNextAudioBlock` call (all the IR
/// path makes): linear interpolation, with a 2-pole low-pass before (down-sampling) or after
/// (up-sampling).
struct Resampler {
    ratio: f64,
    coefficients: [f64; 6],
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Resampler {
    fn new(ratio: f64) -> Self {
        let mut r = Resampler { ratio: ratio.max(0.0), coefficients: [0.0; 6], x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 };
        r.create_low_pass(r.ratio);
        r
    }

    fn create_low_pass(&mut self, frequency_ratio: f64) {
        let proportional_rate = if frequency_ratio > 1.0 { 0.5 / frequency_ratio } else { 0.5 * frequency_ratio };
        let n = 1.0 / (std::f64::consts::PI * proportional_rate.max(0.001)).tan();
        let n_squared = n * n;
        let c1 = 1.0 / (1.0 + std::f64::consts::SQRT_2 * n + n_squared);
        // (c1 * 2.0f: a float literal times a double, still a double)
        self.set_filter_coefficients(c1, c1 * 2.0, c1, 1.0, c1 * 2.0 * (1.0 - n_squared), c1 * (1.0 - std::f64::consts::SQRT_2 * n + n_squared));
    }

    fn set_filter_coefficients(&mut self, c1: f64, c2: f64, c3: f64, c4: f64, c5: f64, c6: f64) {
        let a = 1.0 / c4;
        self.coefficients = [c1 * a, c2 * a, c3 * a, c4, c5 * a, c6 * a];
    }

    fn apply_filter(&mut self, samples: &mut [f32]) {
        let c = self.coefficients;
        for s in samples.iter_mut() {
            let input = f64::from(*s);
            let mut out = c[0] * input + c[1] * self.x1 + c[2] * self.x2 - c[4] * self.y1 - c[5] * self.y2;
            // JUCE_INTEL: flush tiny values
            if (-1.0e-8..=1.0e-8).contains(&out) {
                out = 0.0;
            }
            self.x2 = self.x1;
            self.x1 = input;
            self.y2 = self.y1;
            self.y1 = out;
            *s = out as f32;
        }
    }

    /// One `getNextAudioBlock` of `num_samples` from a fresh source reading `input` (zeros past
    /// its end).
    fn render(&mut self, input: &[f32], num_samples: usize) -> Vec<f32> {
        let local_ratio = self.ratio;
        let samps_needed = (num_samples as f64 * local_ratio).round() as usize + 3;
        // the source reads everything it needs in one go (its buffer is big enough)
        let mut buffer = vec![0.0f32; samps_needed];
        let take = samps_needed.min(input.len());
        buffer[..take].copy_from_slice(&input[..take]);
        if local_ratio > 1.0001 {
            // for down-sampling, pre-apply the filter
            self.apply_filter(&mut buffer);
        }
        let mut out = vec![0.0f32; num_samples];
        let mut buffer_pos = 0usize;
        let mut sub_sample_offset = 0.0f64;
        for o in out.iter_mut() {
            let alpha = sub_sample_offset as f32;
            let a = buffer.get(buffer_pos).copied().unwrap_or(0.0);
            let b = buffer.get(buffer_pos + 1).copied().unwrap_or(0.0);
            *o = a + alpha * (b - a);
            sub_sample_offset += local_ratio;
            while sub_sample_offset >= 1.0 {
                buffer_pos += 1;
                sub_sample_offset -= 1.0;
            }
        }
        if local_ratio < 0.9999 {
            // for up-sampling, apply the filter after transposing
            self.apply_filter(&mut out);
        }
        out
    }
}

/// The `ConvolutionEngine` constructor's partitioning: segment `i` holds the spectrum of
/// `samples[i*512 .. i*512+512]`, zero-padded to the FFT size.
fn partition(samples: &[f32]) -> (Vec<Complex<f32>>, usize) {
    let num_segments = segments_for(samples.len());
    let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
    let mut spectra = Vec::with_capacity(num_segments * BINS);
    let mut buf = vec![Complex::new(0.0f32, 0.0); FFT_SIZE];
    let mut ptr = 0usize;
    for _ in 0..num_segments {
        buf.iter_mut().for_each(|c| *c = Complex::new(0.0, 0.0));
        let n = (FFT_SIZE - CONV_BLOCK).min(samples.len().saturating_sub(ptr));
        for (c, s) in buf.iter_mut().zip(&samples[ptr..ptr + n]) {
            c.re = *s;
        }
        fft.process(&mut buf);
        spectra.extend_from_slice(&buf[..BINS]);
        ptr += FFT_SIZE - CONV_BLOCK;
    }
    (spectra, num_segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_drops_quiet_ends_and_keeps_one_sample() {
        assert_eq!(trim_impulse_response(&[0.0, 0.0, 0.5, 0.25, 0.0]), vec![0.5, 0.25]);
        assert_eq!(trim_impulse_response(&[0.0, 0.0]), vec![0.0]);
    }

    #[test]
    fn normalising_sets_the_energy_juce_does() {
        let mut ir = vec![0.5f32, -0.25, 0.1];
        normalise_impulse_response(&mut ir);
        let e: f32 = ir.iter().map(|x| x * x).sum();
        assert!((e.sqrt() - 0.125).abs() < 1e-6, "{e}");
    }

    #[test]
    fn same_rate_is_untouched_and_other_rates_resample() {
        let ir: Vec<f32> = (1..=1000).map(|i| (i as f32 * 0.01).sin() * (-(i as f32) / 300.0).exp()).collect();
        let a = ImpulseAsset::new(&ir, 48_000.0, 48_000.0).unwrap();
        assert_eq!(a.len(), 1000);
        let b = ImpulseAsset::new(&ir, 48_000.0, 96_000.0).unwrap();
        assert_eq!(b.len(), 2000);
        let c = ImpulseAsset::new(&ir, 96_000.0, 44_100.0).unwrap();
        assert_eq!(c.len(), 459);
        for x in [a, b, c] {
            assert!(x.channel(0).unwrap().iter().all(|v| v.is_finite()));
            assert_eq!(x.num_segments(), segments_for(x.len()));
        }
    }

    #[test]
    fn bad_inputs_are_errors() {
        assert!(ImpulseAsset::new(&[0.5], 0.0, 48_000.0).is_err());
        assert!(ImpulseAsset::new(&[0.5], 48_000.0, f64::NAN).is_err());
        assert!(ImpulseAsset::new(&[f32::NAN], 48_000.0, 48_000.0).is_err());
        let long = vec![0.5f32; 48_000 * 5];
        assert!(ImpulseAsset::new(&long, 48_000.0, 48_000.0).unwrap_err().contains("too long"));
        // nothing at all is a unit impulse (JUCE's fixNumChannels)
        assert_eq!(ImpulseAsset::new(&[], 48_000.0, 48_000.0).unwrap().len(), 1);
    }
}
