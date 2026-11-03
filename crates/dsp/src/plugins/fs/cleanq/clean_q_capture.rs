//! The Ceiling panel's capture, for Learn: the C++ editor's `feedCapture` and the analyser
//! transform in front of it (`PluginEditor.cpp`), feeding the translated `SpectrumAverager`.
//!
//! Every analyser frame of the INPUT goes into the long-term average. The editor takes the last
//! `fftSize` samples of the engine's input (the plugin's `pre` analysis ring here), multiplies by
//! the Hann window (`fill_window`), transforms, scales each magnitude by `FFT_MAGNITUDE_SCALE`,
//! and adds them with `dt` the time since the previous frame (held to 5..250 ms, 1/30 s for the
//! first) and the state's `avgSeconds` as the memory. Silent frames are skipped, so stopping
//! playback does not slowly wipe what was captured.
//!
//! UI-side, never on the audio thread. The FFT is `rustfft` in `f32` where the editor uses
//! `juce::dsp::FFT::performFrequencyOnlyForwardTransform` (also `float`): the two differ in the
//! last bits of a bin, far below anything the 1/N-octave smoothing or a ceiling node can show.

use super::clean_q_ceiling::{SpectrumAverager, smooth_to_grid};
use super::clean_q_dsp::{FFT_MAGNITUDE_SCALE, FFT_SIZE, NUM_BINS, fill_window};
use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::sync::Arc;

/// The capture: the averager plus the transform's plan, window and buffers, made once.
#[derive(Clone)]
pub struct CeilingCapture {
    avg: SpectrumAverager,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    buf: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    mags: Vec<f32>,
    /// UI time of the previous frame (`lastCaptureMs`), seconds; `None` before the first.
    last: Option<f64>,
}

impl std::fmt::Debug for CeilingCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CeilingCapture").field("frames", &self.avg.frame_count()).finish_non_exhaustive()
    }
}

impl Default for CeilingCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl CeilingCapture {
    pub fn new() -> Self {
        let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        let mut window = vec![0.0f32; FFT_SIZE];
        fill_window(&mut window);
        CeilingCapture {
            avg: SpectrumAverager::default(),
            scratch: vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()],
            fft,
            window,
            buf: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            mags: vec![0.0; NUM_BINS],
            last: None,
        }
    }

    /// Reset Capture: forget everything averaged (as the editor's, the time of the last frame
    /// is kept).
    pub fn reset(&mut self) {
        self.avg.reset();
    }

    /// One analyser frame: `frame` is the newest [`FFT_SIZE`] samples of the input (fewer are
    /// zero-padded at the end, non-finite ones read 0), `now` the UI time in seconds, `tau` the
    /// state's `avgSeconds`. `false` when the frame was silent and skipped.
    pub fn feed(&mut self, frame: &[f32], now: f64, tau: f64) -> bool {
        let mut peak = 0.0f32;
        for (n, (b, w)) in self.buf.iter_mut().zip(&self.window).enumerate() {
            let x = frame.get(n).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
            peak = peak.max(x.abs());
            *b = Complex::new(x * w, 0.0);
        }
        self.fft.process_with_scratch(&mut self.buf, &mut self.scratch);

        let mut total = 0.0f64;
        for (m, c) in self.mags.iter_mut().zip(&self.buf) {
            *m = c.norm() * FFT_MAGNITUDE_SCALE;
            total += f64::from(*m) * f64::from(*m);
        }

        let dt = match self.last {
            Some(last) if now.is_finite() => (now - last).clamp(0.005, 0.25),
            _ => 1.0 / 30.0,
        };
        if now.is_finite() {
            self.last = Some(now);
        }

        if total < 1.0e-10 {
            return false;
        }
        let tau = if tau.is_finite() { tau } else { 0.0 };
        self.avg.add(&self.mags, dt, tau);
        // the frame's sample peak (before the window), for Conform's make-up headroom
        self.avg.note_peak(f64::from(peak));
        true
    }

    /// The averager (its bins, frames and seconds).
    pub fn averager(&self) -> &SpectrumAverager {
        &self.avg
    }

    /// Feeds precomputed magnitude bins straight into the averager (tests, and frames made
    /// elsewhere): what `feed` does after its transform.
    pub fn add_magnitudes(&mut self, mags: &[f32], dt: f64, tau: f64) {
        self.avg.add(mags, dt, tau);
    }

    /// The capture on the ceiling grid, in dB, smoothed to 1/`smoothing` octave, at the session
    /// `sample_rate` (`smoothToGrid`); empty while nothing has been captured.
    pub fn grid(&self, sample_rate: f64, smoothing: i32) -> Vec<f64> {
        if !self.avg.has_data() {
            return Vec::new();
        }
        smooth_to_grid(self.avg.bins(), sample_rate, FFT_SIZE, f64::from(smoothing))
    }
}

#[cfg(test)]
mod tests {
    use super::super::clean_q_ceiling::{GRID_N, grid_hz};
    use super::*;

    #[test]
    fn silence_is_skipped_and_a_tone_lands_on_its_grid_point() {
        let mut c = CeilingCapture::new();
        assert!(!c.feed(&[0.0; FFT_SIZE], 0.0, 0.0));
        assert!(c.grid(48_000.0, 3).is_empty());
        let tone: Vec<f32> = (0..FFT_SIZE).map(|n| 0.5 * (std::f32::consts::TAU * 1000.0 * n as f32 / 48_000.0).sin()).collect();
        for k in 0..10 {
            assert!(c.feed(&tone, 0.1 * k as f64, 0.0));
        }
        assert_eq!(c.averager().frame_count(), 10);
        // the silent frame at 0 s still set the clock: the first tone frame is 5 ms (the floor) after it
        assert!((c.averager().seconds_averaged() - (0.005 + 9.0 * 0.1)).abs() < 1e-9);
        let mut d = CeilingCapture::new();
        assert!(d.feed(&tone, 7.0, 0.0));
        assert!((d.averager().seconds_averaged() - 1.0 / 30.0).abs() < 1e-12, "the very first frame is 1/30 s");
        let g = c.grid(48_000.0, 3);
        assert_eq!(g.len(), GRID_N);
        let at = |hz: f64| g[(0..GRID_N).min_by(|&a, &b| (grid_hz(a) - hz).abs().total_cmp(&(grid_hz(b) - hz).abs())).unwrap_or(0)];
        assert!(at(1000.0) > at(250.0) + 30.0, "{} {}", at(1000.0), at(250.0));
        // garbage in the frame reads 0; a short frame is zero-padded
        let mut bad = tone.clone();
        bad[1000] = f32::NAN;
        bad[2000] = f32::INFINITY;
        bad.truncate(3000);
        assert!(c.feed(&bad, 2.0, f64::NAN));
        assert!(c.grid(48_000.0, 3).iter().all(|v| v.is_finite()));
        // the learned sample peak: the tone's 0.5 (-6 dBFS), not the garbage
        assert!((c.averager().peak_db() + 6.0206).abs() < 0.01, "{}", c.averager().peak_db());
        c.reset();
        assert!(c.grid(48_000.0, 3).is_empty());
        assert_eq!(c.averager().peak_db(), f64::NEG_INFINITY);
    }
}
