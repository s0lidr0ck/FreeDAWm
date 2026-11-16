//! Band-limited (PolyBLEP) oscillators shared by the generator and the synth.

use std::f32::consts::TAU;

use crate::util::{PinkNoise, Rng};

/// Test-signal / oscillator waveforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Waveform {
    Sine,
    Square,
    Saw,
    Triangle,
    WhiteNoise,
    PinkNoise,
}

impl Waveform {
    pub const ALL: [Waveform; 6] = [Waveform::Sine, Waveform::Square, Waveform::Saw, Waveform::Triangle, Waveform::WhiteNoise, Waveform::PinkNoise];

    /// Waveform for a choice index (out of range → Sine).
    pub fn from_index(i: usize) -> Waveform {
        Self::ALL.get(i).copied().unwrap_or(Waveform::Sine)
    }
}

/// The PolyBLEP residual of a unit step at phase 0, for phase `t` (0..1) and increment `dt`.
#[inline]
pub(crate) fn poly_blep(t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        0.0
    } else if t < dt {
        let t = t / dt;
        2.0 * t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + 2.0 * t + 1.0
    } else {
        0.0
    }
}

/// The PolyBLAMP residual of a unit change of slope (per sample) at phase 0, for phase `t`
/// (0..1) and increment `dt`: what a band-limited corner adds to a naive one.
#[inline]
fn poly_blamp(t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        0.0
    } else if t < dt {
        let x = t / dt - 1.0;
        -x * x * x / 3.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt + 1.0;
        x * x * x / 3.0
    } else {
        0.0
    }
}

/// Phase-accumulator oscillator. Output peaks at ±1 (noise is uniform ±1 / pink ≈ ±1).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Osc {
    pub(crate) phase: f32,
    rng: Rng,
    pink: PinkNoise,
}

impl Osc {
    pub(crate) fn new(seed: u32) -> Self {
        Self { phase: 0.0, rng: Rng::new(seed), pink: PinkNoise::new() }
    }

    pub(crate) fn reset(&mut self) {
        self.phase = 0.0;
        self.pink = PinkNoise::new();
    }

    /// Produces one sample; `inc` is frequency / sample rate.
    #[inline]
    pub(crate) fn next(&mut self, wave: Waveform, inc: f32) -> f32 {
        let dt = if inc.is_finite() { inc.clamp(0.0, 0.49) } else { 0.0 };
        let t = self.phase;
        let out = match wave {
            Waveform::Sine => (t * TAU).sin(),
            Waveform::Saw => 2.0 * t - 1.0 - poly_blep(t, dt),
            Waveform::Square => {
                let naive = if t < 0.5 { 1.0 } else { -1.0 };
                let t2 = if t + 0.5 >= 1.0 { t - 0.5 } else { t + 0.5 };
                naive + poly_blep(t, dt) - poly_blep(t2, dt)
            }
            Waveform::Triangle => {
                // Starts at 0 rising, like a sine; its two corners rounded by PolyBLAMP (the
                // slope turns at the top, t = 1/4, and back at the bottom, 3/4).
                let u = t + 0.25;
                let u = if u >= 1.0 { u - 1.0 } else { u };
                let wrap = |x: f32| if x < 0.0 { x + 1.0 } else { x };
                let top = wrap(t - 0.25);
                let bottom = wrap(t - 0.75);
                1.0 - 4.0 * (u - 0.5).abs() + 4.0 * dt * (poly_blamp(bottom, dt) - poly_blamp(top, dt))
            }
            Waveform::WhiteNoise => self.rng.bipolar(),
            Waveform::PinkNoise => {
                let w = self.rng.bipolar();
                self.pink.next(w).clamp(-1.0, 1.0)
            }
        };
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bug 27 of the MIDI writing parity list: the triangle was naive, so its corners aliased.
    /// The harmonics above Nyquist fold back to frequencies the triangle does not have: in the
    /// lower half of the band the rounded corners leave them at least 20 dB quieter, at every
    /// rate (close under Nyquist the gain is smaller, as with any two-sample correction).
    #[test]
    fn the_triangle_is_band_limited() {
        for sr in [44_100usize, 48_000, 96_000] {
            let f = 3_100usize;
            let dt = f as f32 / sr as f32;
            let mut o = Osc::new(1);
            let ours: Vec<f32> = (0..sr).map(|_| o.next(Waveform::Triangle, dt)).collect();
            let mut t = 0.0f32;
            let naive: Vec<f32> = (0..sr)
                .map(|_| {
                    let u = (t + 0.25).fract();
                    t = (t + dt).fract();
                    1.0 - 4.0 * (u - 0.5).abs()
                })
                .collect();
            let mag = |x: &[f32], hz: usize| {
                let (mut re, mut im) = (0.0f64, 0.0f64);
                for (i, v) in x.iter().enumerate() {
                    let ph = std::f64::consts::TAU * ((hz * i) % sr) as f64 / sr as f64;
                    re += f64::from(*v) * ph.cos();
                    im += f64::from(*v) * ph.sin();
                }
                (re * re + im * im).sqrt() / sr as f64
            };
            // where the 7th to 31st harmonics fold to
            let aliases: Vec<usize> = (3..16)
                .map(|k| (2 * k + 1) * f)
                .filter(|h| *h > sr / 2)
                .map(|h| (h % sr).min(sr - h % sr))
                .filter(|a| a % f != 0 && *a < sr / 4)
                .collect();
            assert!(aliases.len() >= 2, "{sr}: {aliases:?}");
            let sum = |x: &[f32]| aliases.iter().map(|a| mag(x, *a)).sum::<f64>();
            assert!(sum(&ours) < 0.1 * sum(&naive), "{sr}: {} against {}", sum(&ours), sum(&naive));
            // the tone itself is all but unchanged (PolyBLAMP's own droop near the top: under 0.5 dB)
            assert!((mag(&ours, f) - mag(&naive, f)).abs() < 0.06 * mag(&naive, f));
        }
    }
}
