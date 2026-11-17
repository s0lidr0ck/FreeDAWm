//! Simulated outputs from captures: the capture test signal, and the analysis that turns a
//! recording of it played through a device (or a model of one) into a [`DeviceProfile`] for the
//! monitor stage.
//!
//! The signal is 95.5 s of stereo at 48 kHz. The analysis reads three of its 6 s exponential
//! sweeps (20 Hz to 22 kHz): the one on the left only (how much of it comes out on the right
//! gives the stereo width) and the one on both sides at -24 dBFS (its level per third-octave band,
//! against the signal's, gives the frequency response). That is a linear picture: what a device
//! does to loud passages (limiting, distortion) is not in the profile.
//!
//! [`test_signal`] writes the sync click, every sweep and the tone bursts where the full signal
//! has them, and silence where it has its multitone and noise segments, which the analysis does
//! not read.

use soundcraft_dsp::monitor::{BANDS, band_hz};
use soundcraft_model::monitor::DeviceProfile;

/// The sample rate of the signal, and the only one a capture is read at.
pub const RATE: u32 = 48_000;
const SR: usize = RATE as usize;
/// Frames of the whole signal.
pub const FRAMES: usize = 4_582_144;
const SWEEP: usize = 6 * SR;
const F1: f64 = 20.0;
const F2: f64 = 22_000.0;
const FADE: usize = SR / 50;
/// Where the sweeps the analysis reads start: left only, both sides at -24 dBFS.
const LEFT_AT: usize = 4 * SR;
const BOTH_AT: usize = 28 * SR;
/// The furthest a capture may sit from the signal, either way, in frames.
const MAX_OFFSET: i64 = 2400;
/// The lowest a band of a profile's curve is written, dB under its loudest band.
const FLOOR_DB: f32 = -40.0;

fn sweep_k() -> f64 {
    SWEEP as f64 / f64::from(RATE) / (F2 / F1).ln()
}

/// One sweep at full scale, with its 20 ms fades.
fn sweep() -> Vec<f32> {
    let k = sweep_k();
    (0..SWEEP)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let fade = (i.min(SWEEP - 1 - i) as f64 / FADE as f64).min(1.0);
            ((std::f64::consts::TAU * F1 * k * ((t / k).exp() - 1.0)).sin() * fade) as f32
        })
        .collect()
}

/// The frames of a sweep in which it passes through the third-octave band at `hz`, inside the
/// fades; `None` for a band the sweep does not cross.
fn band_frames(hz: f32) -> Option<(usize, usize)> {
    let k = sweep_k();
    let at = |f: f64| ((f / F1).max(1e-9).ln() * k * f64::from(RATE)).round();
    let edge = 2f64.powf(1.0 / 6.0);
    let lo = at(f64::from(hz) / edge).max(FADE as f64) as usize;
    let hi = at(f64::from(hz) * edge).min((SWEEP - FADE) as f64).max(0.0) as usize;
    (hi > lo + SR / 100).then_some((lo, hi))
}

/// The capture test signal: left and right, [`FRAMES`] long at [`RATE`] (see the module docs).
pub fn test_signal() -> [Vec<f32>; 2] {
    let mut l = vec![0.0f32; FRAMES];
    let mut r = vec![0.0f32; FRAMES];
    let db = |d: f32| 10f32.powf(d / 20.0);
    let sw = sweep();
    let put = |dst: &mut Vec<f32>, at: usize, gain: f32| {
        for (d, s) in dst.iter_mut().skip(at).zip(&sw) {
            *d = s * gain;
        }
    };
    // The sync click.
    for side in [&mut l, &mut r] {
        if let Some(x) = side.get_mut(SR / 2) {
            *x = db(-6.0);
        }
    }
    put(&mut l, LEFT_AT, db(-12.0));
    put(&mut r, 12 * SR, db(-12.0));
    // Both sides at -36, -24, -12 and -3 dBFS, and the -12 one again at the end.
    for (at, level) in [(20 * SR, -36.0), (BOTH_AT, -24.0), (36 * SR, -12.0), (44 * SR, -3.0), (4_198_144, -12.0)] {
        put(&mut l, at, db(level));
        put(&mut r, at, db(level));
    }
    // Tone bursts: -30 dBFS with half a second at -3 dBFS.
    let fade = SR / 100;
    for (at, hz) in [(52 * SR, 100.0f32), (56 * SR, 1000.0), (60 * SR, 5000.0)] {
        for i in 0..3 * SR {
            let level = if (SR..SR * 3 / 2).contains(&i) { db(-3.0) } else { db(-30.0) };
            let edge = (i.min(3 * SR - 1 - i) as f32 / fade as f32).min(1.0);
            let v = (std::f32::consts::TAU * hz * i as f32 / SR as f32).sin() * level * edge;
            for side in [&mut l, &mut r] {
                if let Some(x) = side.get_mut(at + i) {
                    *x = v;
                }
            }
        }
    }
    [l, r]
}

/// What a capture's analysis found.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    /// The simulated output: the curve (its loudest band at 0 dB) and the width.
    pub profile: DeviceProfile,
    /// How far the capture sits behind the signal, frames.
    pub offset: i64,
    /// What the curve was moved by so its loudest band is at 0 dB: the device's own level there
    /// is this much the other way.
    pub trim_db: f32,
    /// The capture is the signal itself (flat and fully stereo): whatever was meant to shape it
    /// was not in the path.
    pub unprocessed: bool,
}

fn energy(x: &[f32], from: usize, to: usize) -> f64 {
    x.get(from..to.min(x.len())).unwrap_or_default().iter().map(|v| f64::from(*v) * f64::from(*v)).sum()
}

/// Where the capture's left side sits against the signal: the lag at which the 1 to 2 kHz stretch
/// of the both-sides sweep lines up best, and how well (0..1).
fn align(left: &[f32], reference: &[f32]) -> (i64, f64) {
    let k = sweep_k();
    let at = |f: f64| ((f / F1).ln() * k * f64::from(RATE)) as usize;
    let (from, to) = (at(1000.0), at(2000.0));
    let chunk = reference.get(from..to).unwrap_or_default();
    let ref_energy: f64 = chunk.iter().map(|v| f64::from(*v) * f64::from(*v)).sum();
    let mut best = (0i64, 0.0f64);
    for lag in -MAX_OFFSET..=MAX_OFFSET {
        let Some(start) = (BOTH_AT + from).checked_add_signed(lag as isize) else { continue };
        let Some(got) = left.get(start..start + chunk.len()) else { continue };
        let dot: f64 = got.iter().zip(chunk).map(|(a, b)| f64::from(*a) * f64::from(*b)).sum();
        if dot.abs() > best.1.abs() {
            best = (lag, dot);
        }
    }
    let start = (BOTH_AT + from).saturating_add_signed(best.0 as isize);
    let got_energy = energy(left, start, start + chunk.len());
    let quality = if ref_energy > 0.0 && got_energy > 0.0 { best.1.abs() / (ref_energy * got_energy).sqrt() } else { 0.0 };
    (best.0, quality)
}

/// Reads a capture of the test signal (`channels` at `rate`: stereo, 48 kHz, from the signal's
/// first frame or within 50 ms of it) into a simulated output named `name`.
pub fn analyze(name: &str, channels: &[Vec<f32>], rate: u32) -> Result<Analysis, String> {
    if rate != RATE {
        return Err(format!("the capture is at {rate} Hz: bounce it at {RATE} Hz, as the test file is"));
    }
    let [left, right] = channels else {
        return Err(format!("the capture has {} channel(s): it must be stereo", channels.len()));
    };
    if left.len().min(right.len()) < BOTH_AT + SWEEP + SR {
        return Err("the capture is too short: bounce the whole test file, from its very start".into());
    }
    let reference = sweep();
    let (offset, quality) = align(left, &reference);
    if quality < 0.2 {
        return Err("this does not look like a capture of the test file (its sweeps are not where they should be)".into());
    }
    let shift = |at: usize| at.saturating_add_signed(offset as isize);
    let centres: Vec<f32> = (0..BANDS).map(band_hz).collect();
    // The response: both sides of the -24 dBFS sweep against the signal's, per band.
    let level = 10f64.powf(-24.0 / 20.0);
    let mut curve: Vec<[f32; 2]> = Vec::new();
    for hz in &centres {
        let Some((lo, hi)) = band_frames(*hz) else { continue };
        let want = 2.0 * energy(&reference, lo, hi) * level * level;
        let got = energy(left, shift(BOTH_AT + lo), shift(BOTH_AT + hi)) + energy(right, shift(BOTH_AT + lo), shift(BOTH_AT + hi));
        if want > 0.0 {
            curve.push([*hz, (10.0 * (got / want).max(1e-12).log10()) as f32]);
        }
    }
    if curve.is_empty() {
        return Err("the capture holds no sweep to measure".into());
    }
    // The width: how much of the left-only sweep comes out on the right, 200 Hz to 5 kHz.
    let (mut sum, mut n) = (0.0f64, 0usize);
    for hz in centres.iter().filter(|hz| (200.0..=5000.0).contains(*hz)) {
        let Some((lo, hi)) = band_frames(*hz) else { continue };
        let own = energy(left, shift(LEFT_AT + lo), shift(LEFT_AT + hi));
        let other = energy(right, shift(LEFT_AT + lo), shift(LEFT_AT + hi));
        if own > 0.0 {
            sum += 10.0 * (other / own).max(1e-14).log10();
            n += 1;
        }
    }
    let leak = if n > 0 { 10f64.powf(sum / n as f64 / 20.0) } else { 0.0 };
    let width = ((1.0 - leak) / (1.0 + leak)).clamp(0.0, 1.0);
    let width = ((width * 20.0).round() / 20.0) as f32;
    let unprocessed = curve.iter().all(|p| p[1].abs() < 0.3) && leak < 0.01;
    let top = curve.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
    for p in &mut curve {
        p[0] = (p[0] * 10.0).round() / 10.0;
        p[1] = (((p[1] - top).max(FLOOR_DB)) * 10.0).round() / 10.0;
    }
    let mut profile = DeviceProfile {
        name: name.to_string(),
        curve,
        width,
        notes: "Measured with FreeDAWm's capture test file: frequency response and stereo width (no distortion or limiting).".into(),
        ..DeviceProfile::default()
    };
    profile.sanitize();
    if profile.name.is_empty() {
        return Err("the profile needs a name".into());
    }
    Ok(Analysis { profile, offset, trim_db: -top, unprocessed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_dsp::monitor::State;

    /// The signal as `device` plays it, `late` frames late.
    fn captured(device: &DeviceProfile, late: usize) -> Vec<Vec<f32>> {
        let [mut l, mut r] = test_signal();
        let mut st = State::default();
        st.load(&crate::monitor_profiles::plan(RATE as f32, None, false, Some(device), true));
        st.process(&mut l, &mut r);
        for side in [&mut l, &mut r] {
            side.splice(0..0, std::iter::repeat_n(0.0, late));
        }
        vec![l, r]
    }

    #[test]
    fn the_signal_is_the_length_and_shape_the_analysis_expects() {
        let [l, r] = test_signal();
        assert_eq!((l.len(), r.len()), (FRAMES, FRAMES));
        assert!(l.iter().chain(&r).all(|v| v.abs() < 0.72));
        // Left only, then right only, then both.
        assert!(energy(&l, LEFT_AT, LEFT_AT + SWEEP) > 1000.0 && energy(&r, LEFT_AT, LEFT_AT + SWEEP) == 0.0);
        assert!(energy(&r, 12 * SR, 18 * SR) > 1000.0 && energy(&l, 12 * SR, 18 * SR) == 0.0);
        assert_eq!(energy(&l, BOTH_AT, BOTH_AT + SWEEP), energy(&r, BOTH_AT, BOTH_AT + SWEEP));
    }

    #[test]
    fn a_capture_gives_back_the_device_it_was_played_through() {
        let device = DeviceProfile {
            name: "Box".into(),
            curve: vec![[100.0, -18.0], [300.0, -4.0], [1000.0, 0.0], [3000.0, -6.0], [9000.0, -2.0], [16_000.0, -14.0]],
            width: 0.3,
            ..DeviceProfile::default()
        };
        let a = analyze("Box", &captured(&device, 5), RATE).unwrap();
        // (the device's own filters delay it a frame or so more)
        assert!((5..=7).contains(&a.offset) && !a.unprocessed, "{}", a.offset);
        assert!((a.profile.width - 0.3).abs() < 0.06, "{}", a.profile.width);
        let want = soundcraft_dsp::monitor::clean_points(&device.curve);
        for p in a.profile.curve.iter().filter(|p| (50.0..=14_000.0).contains(&p[0])) {
            let w = soundcraft_dsp::monitor::curve_db(&want, p[0]);
            assert!((p[1] - w).abs() < 1.0, "{} Hz: measured {} dB, the device has {} dB", p[0], p[1], w);
        }
        assert!(a.trim_db.abs() < 1.0);
    }

    #[test]
    fn the_signal_itself_is_reported_as_unprocessed_and_other_audio_is_refused() {
        let [l, r] = test_signal();
        let a = analyze("None", &[l.clone(), r.clone()], RATE).unwrap();
        assert!(a.unprocessed && a.offset == 0 && a.profile.width == 1.0);
        assert!(a.profile.curve.iter().all(|p| p[1].abs() <= 0.3));
        assert!(analyze("x", &[l.clone(), r.clone()], 44_100).unwrap_err().contains("48000"));
        assert!(analyze("x", std::slice::from_ref(&l), RATE).unwrap_err().contains("stereo"));
        assert!(analyze("x", &[l[..SR].to_vec(), r[..SR].to_vec()], RATE).unwrap_err().contains("too short"));
        let noise: Vec<f32> = (0..FRAMES).map(|i| ((i * 7919) % 1000) as f32 / 5000.0 - 0.1).collect();
        assert!(analyze("x", &[noise.clone(), noise], RATE).unwrap_err().contains("does not look like"));
        assert!(analyze("  ", &[l, r], RATE).is_err());
    }
}
