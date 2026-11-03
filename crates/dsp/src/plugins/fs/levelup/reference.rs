//! A LevelUp reference track: the processor's `loadReference` job, done by the host where it
//! makes the asset (`mix.module_reference`, off the audio thread), so the plugin only reads it.
//!
//! The C++ job decodes the file, resamples it to the session rate (JUCE's `LagrangeInterpolator`),
//! keeps at most `maxRefSeconds` (8 minutes), measures it with a fresh `LevelUpDSP` meter as it
//! goes (on the float samples, before the 16-bit copy), and fails a file that is too short (under
//! 1000 samples) or too quiet to measure (integrated <= -69 LUFS). Here the host decodes and
//! resamples (its own resampler, the one every SoundCraft asset uses: a departure in the
//! resampled values only, not in what the plugin does with them), and [`ReferenceAudio::new`]
//! does the rest as the C++ does. The samples are kept as the host's floats (a host asset is a
//! [`SampleSource`]); the plugin plays each one through `toFloat (fromFloat (x))`, the value the
//! C++'s 16-bit copy holds.

use super::level_up_dsp::{LevelUpDsp, RefView};
use crate::SampleSource;

/// `maxRefSeconds`: the longest stretch of a file kept.
pub const MAX_REF_SECONDS: f64 = 8.0 * 60.0;
/// A reference shorter than this (`outLen < 1000`) is refused.
pub const MIN_REF_FRAMES: usize = 1000;

/// See the module docs.
#[derive(Debug, Clone)]
pub struct ReferenceAudio {
    rate: f32,
    left: Vec<f32>,
    /// `None`: a mono file, read as both sides (the C++ copies channel 0 to channel 1).
    right: Option<Vec<f32>>,
    lufs: f32,
    true_peak_dbtp: f32,
    truncated: bool,
}

impl ReferenceAudio {
    /// The reference made from decoded audio at the session rate `rate` (planar; channels past
    /// the second are dropped, as the C++ reads two). Errors as the C++ job's `fail`.
    pub fn new(rate: f32, mut channels: Vec<Vec<f32>>) -> Result<Self, String> {
        if !(rate.is_finite() && rate > 0.0) {
            return Err("no usable sample rate".into());
        }
        if channels.is_empty() {
            return Err("not an audio file this can read".into());
        }
        channels.truncate(2);
        let max = (MAX_REF_SECONDS * f64::from(rate)).floor() as usize;
        let frames = channels.iter().map(Vec::len).min().unwrap_or(0);
        let truncated = frames > max;
        let keep = frames.min(max);
        for c in &mut channels {
            c.truncate(keep);
            c.shrink_to_fit();
        }
        if keep < MIN_REF_FRAMES {
            return Err("too short".into());
        }
        let right = if channels.len() > 1 { channels.pop() } else { None };
        let left = channels.pop().unwrap_or_default();
        let (lufs, true_peak_dbtp) = LevelUpDsp::measure_integrated(&left, right.as_deref().unwrap_or(&left), f64::from(rate));
        if lufs <= -69.0 {
            return Err("silent, or too quiet to measure".into());
        }
        Ok(ReferenceAudio { rate, left, right, lufs, true_peak_dbtp, truncated })
    }

    /// Its integrated loudness, measured on load.
    pub fn lufs(&self) -> f32 {
        self.lufs
    }
    /// Its true peak, dBTP.
    pub fn true_peak_dbtp(&self) -> f32 {
        self.true_peak_dbtp
    }
    /// Only the first 8 minutes were kept.
    pub fn truncated(&self) -> bool {
        self.truncated
    }
    /// Its length in seconds.
    pub fn seconds(&self) -> f32 {
        self.left.len() as f32 / self.rate
    }
    /// What the engine reads.
    pub fn view(&self) -> RefView<'_> {
        RefView { left: &self.left, right: self.right.as_deref().unwrap_or(&self.left), lufs: self.lufs }
    }
}

impl SampleSource for ReferenceAudio {
    fn sample_rate(&self) -> f32 {
        self.rate
    }
    fn num_channels(&self) -> usize {
        if self.right.is_some() { 2 } else { 1 }
    }
    fn channel(&self, ch: usize) -> Option<&[f32]> {
        match ch {
            0 => Some(&self.left),
            1 => self.right.as_deref(),
            _ => None,
        }
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// A host asset as a LevelUp reference (`None`: another kind of asset).
pub fn as_reference(a: &dyn SampleSource) -> Option<&ReferenceAudio> {
    a.as_any()?.downcast_ref::<ReferenceAudio>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(len: usize, amp: f32) -> Vec<f32> {
        (0..len).map(|i| amp * (i as f32 * 0.13).sin()).collect()
    }

    #[test]
    fn measures_refuses_and_truncates_as_the_cpp_job() {
        let r = ReferenceAudio::new(48_000.0, vec![tone(96_000, 0.5), tone(96_000, 0.5)]).unwrap();
        assert!(r.lufs() > -20.0 && r.lufs() < 0.0, "{}", r.lufs());
        assert_eq!((r.num_channels(), r.frames(), r.truncated()), (2, 96_000, false));
        // mono reads as both sides: the same loudness as the stereo copy
        let m = ReferenceAudio::new(48_000.0, vec![tone(96_000, 0.5)]).unwrap();
        assert_eq!((m.num_channels(), m.lufs()), (1, r.lufs()));
        assert_eq!(m.view().right.len(), 96_000);
        // more than two channels: the first two
        assert_eq!(ReferenceAudio::new(48_000.0, vec![tone(24_000, 0.5); 6]).unwrap().num_channels(), 2);
        // too short, silent, nothing, a bad rate
        assert_eq!(ReferenceAudio::new(48_000.0, vec![tone(999, 0.5)]).unwrap_err(), "too short");
        assert!(ReferenceAudio::new(48_000.0, vec![vec![0.0; 48_000]]).unwrap_err().contains("silent"));
        assert!(ReferenceAudio::new(48_000.0, vec![]).is_err());
        assert!(ReferenceAudio::new(f32::NAN, vec![tone(4000, 0.5)]).is_err());
        // at most 8 minutes (at a low rate, to keep the test small)
        let t = ReferenceAudio::new(4_000.0, vec![tone(2_000_000, 0.5)]).unwrap();
        assert_eq!((t.frames(), t.truncated()), (1_920_000, true));
        assert!((t.seconds() - 480.0).abs() < 1e-3);
        let any: std::sync::Arc<dyn SampleSource> = std::sync::Arc::new(r);
        assert!(as_reference(any.as_ref()).is_some());
        let plain = crate::SampleBuffer { sample_rate: 48_000.0, channels: vec![tone(4000, 0.5)] };
        assert!(as_reference(&plain).is_none());
    }
}
