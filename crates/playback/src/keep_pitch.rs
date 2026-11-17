//! Varispeed that keeps the pitch: the output of a play at speed `s` (read through the
//! resampler, so its pitch moved by `s`) goes through a pitch shifter by `1 / s`.
//!
//! The shifter is the two-tap delay-line kind: each output channel writes into a ring of
//! [`WINDOW`] device frames, and two read taps half a window apart slide through it at
//! `1 − 1/s` frames a frame, each faded in and out (sin², so the two always sum to one) as it
//! wraps. The material comes out at its own pitch while the play still runs at `s`. It adds
//! about half a window of delay (21 ms at 48 kHz). Nothing allocates after it is made (on the
//! UI thread, with the rest of the audio state); at speed 1 or with the option off it is not run.

use crate::outmap::MAX_DEVICE_CHANNELS;

/// Ring length in device frames (power of two).
pub(crate) const WINDOW: usize = 2048;

pub(crate) struct KeepPitch {
    ring: Vec<[f32; WINDOW]>,
    write: usize,
    /// Tap A's place in its sweep, 0–1 (tap B is half a sweep on).
    phase: f64,
}

impl Default for KeepPitch {
    fn default() -> KeepPitch {
        KeepPitch { ring: vec![[0.0; WINDOW]; MAX_DEVICE_CHANNELS], write: 0, phase: 0.0 }
    }
}

impl KeepPitch {
    /// Forget what it holds (a new play starts from silence).
    pub(crate) fn clear(&mut self) {
        self.ring.iter_mut().for_each(|r| r.iter_mut().for_each(|x| *x = 0.0));
        (self.write, self.phase) = (0, 0.0);
    }

    /// Shifts interleaved `out` (`channels` wide) in place by `pitch` (output pitch / input pitch).
    pub(crate) fn process(&mut self, out: &mut [f32], channels: usize, pitch: f64) {
        let channels = channels.clamp(1, MAX_DEVICE_CHANNELS);
        let pitch = if pitch.is_finite() { pitch.clamp(0.1, 10.0) } else { 1.0 };
        let w = (WINDOW - 4) as f64;
        // The delay shrinks by `pitch − 1` frames a frame: the phase moves the other way.
        let rate = (1.0 - pitch) / w;
        for frame in out.chunks_mut(channels) {
            let (pa, pb) = (self.phase, (self.phase + 0.5).fract());
            let (da, db) = (2.0 + pa * w, 2.0 + pb * w);
            let ga = (std::f64::consts::PI * pa).sin().powi(2) as f32;
            let gb = 1.0 - ga;
            for (c, x) in frame.iter_mut().enumerate() {
                let Some(r) = self.ring.get_mut(c) else { continue };
                if let Some(slot) = r.get_mut(self.write) {
                    *slot = if x.is_finite() { *x } else { 0.0 };
                }
                let tap = |d: f64| -> f32 {
                    let at = self.write as f64 - d;
                    let i = at.floor();
                    let f = (at - i) as f32;
                    let i = (i as i64).rem_euclid(WINDOW as i64) as usize;
                    let a = r.get(i).copied().unwrap_or(0.0);
                    let b = r.get((i + 1) % WINDOW).copied().unwrap_or(0.0);
                    a + (b - a) * f
                };
                *x = ga * tap(da) + gb * tap(db);
            }
            self.write = (self.write + 1) % WINDOW;
            self.phase = (self.phase + rate).rem_euclid(1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The strongest frequency in `y` (by a slow DFT scan), in cycles per sample.
    fn peak_freq(y: &[f32]) -> f64 {
        let mut best = (0.0, 0.0);
        for k in 1..400 {
            let f = k as f64 * 0.0005;
            let (mut re, mut im) = (0.0, 0.0);
            for (n, v) in y.iter().enumerate() {
                let a = std::f64::consts::TAU * f * n as f64;
                re += f64::from(*v) * a.cos();
                im += f64::from(*v) * a.sin();
            }
            let m = re * re + im * im;
            if m > best.1 {
                best = (f, m);
            }
        }
        best.0
    }

    #[test]
    fn a_tone_comes_out_at_the_pitch_asked_for() {
        for pitch in [2.0, 0.5, 1.25] {
            let mut k = KeepPitch::default();
            let f0 = 0.02;
            let mut y: Vec<f32> = (0..16_384).map(|n| (std::f64::consts::TAU * f0 * n as f64).sin() as f32 * 0.5).collect();
            k.process(&mut y, 1, pitch);
            let got = peak_freq(&y[4096..12_288]);
            assert!((got - f0 * pitch).abs() < 0.0015, "{pitch}: {got} for {}", f0 * pitch);
            assert!(y.iter().all(|v| v.is_finite() && v.abs() < 1.0));
        }
    }
}
