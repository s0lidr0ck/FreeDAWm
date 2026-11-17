//! The hearing test behind the monitor stage's ear correction: what it plays and how its answers
//! become a correction.
//!
//! It needs no calibrated equipment because it never asks how quiet a sound can be heard. It asks
//! two things at each of [`TEST_HZ`], with third-octave bands of noise at a normal listening level:
//! - **Balance:** the band plays in both ears; the listener moves it until it sits in the middle.
//!   That gives the difference between the ears directly.
//! - **Loudness:** the band alternates with the 1 kHz band; the listener makes them equally loud.
//!   The bands start out at the levels a listener with normal hearing finds equally loud (the
//!   ISO 226:2003 contour at [`PHON`]), so what the listener adds is what their hearing lacks there.
//!
//! The second answer also takes in the headphones' own response, which the test cannot tell from
//! the listener's hearing: across frequencies the result is an estimate, between the ears it is
//! exact.

use crate::biquad::{Biquad, FilterType};

/// The frequencies tested (and the Ear Profile window's sliders), Hz.
pub const TEST_HZ: [f32; 10] = [125.0, 250.0, 500.0, 1000.0, 2000.0, 3000.0, 4000.0, 6000.0, 8000.0, 12_000.0];
/// Which of [`TEST_HZ`] is the loudness reference (1 kHz).
pub const REFERENCE: usize = 3;
/// The loudness level the bands are presented at, phon.
pub const PHON: f32 = 60.0;
/// The reference band's level, dBFS RMS.
pub const REFERENCE_DBFS: f32 = -36.0;
/// No band is ever played above this, dBFS RMS (noise peaks sit some 12 dB higher).
pub const MAX_DBFS: f32 = -15.0;
/// The balance answer's range: dB the right ear is raised over the left.
pub const BALANCE_DB: std::ops::RangeInclusive<f32> = -20.0..=20.0;
/// The loudness answer's range: dB added to the band.
pub const LOUDNESS_DB: std::ops::RangeInclusive<f32> = -15.0..=25.0;

// ISO 226:2003, Table 1: frequency, exponent of loudness perception, the linear transfer
// function's magnitude normalised at 1 kHz (dB), threshold of hearing (dB).
const ISO_HZ: [f32; 29] = [
    20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0, 315.0, 400.0, 500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0,
    2500.0, 3150.0, 4000.0, 5000.0, 6300.0, 8000.0, 10_000.0, 12_500.0,
];
const ISO_AF: [f32; 29] = [
    0.532, 0.506, 0.480, 0.455, 0.432, 0.409, 0.387, 0.367, 0.349, 0.330, 0.315, 0.301, 0.288, 0.276, 0.267, 0.259, 0.253, 0.250, 0.246, 0.244,
    0.243, 0.243, 0.243, 0.242, 0.242, 0.245, 0.254, 0.271, 0.301,
];
const ISO_LU: [f32; 29] = [
    -31.6, -27.2, -23.0, -19.1, -15.9, -13.0, -10.3, -8.1, -6.2, -4.5, -3.1, -2.0, -1.1, -0.4, 0.0, 0.3, 0.5, 0.0, -2.7, -4.1, -1.0, 1.7, 2.5, 1.2,
    -2.1, -7.1, -11.2, -10.7, -3.1,
];
const ISO_TF: [f32; 29] = [
    78.5, 68.7, 59.5, 51.1, 44.0, 37.5, 31.5, 26.5, 22.1, 17.9, 14.4, 11.4, 8.6, 6.2, 4.4, 3.0, 2.2, 2.4, 3.5, 1.7, -1.3, -4.2, -6.0, -5.4, -1.5,
    6.0, 12.6, 13.9, 12.3,
];

/// The sound pressure level of a pure tone at ISO frequency `i` that is `phon` loud, dB.
fn iso_spl(i: usize, phon: f32) -> f32 {
    let (Some(af), Some(lu), Some(tf)) = (ISO_AF.get(i), ISO_LU.get(i), ISO_TF.get(i)) else { return phon };
    let a = 0.00447 * (10f32.powf(0.025 * phon) - 1.15) + (0.4 * 10f32.powf((tf + lu) / 10.0 - 9.0)).powf(*af);
    10.0 / af * a.max(1e-20).log10() - lu + 94.0
}

/// How much more level than at 1 kHz a sound at `hz` needs to be as loud to normal hearing at
/// [`PHON`], dB (ISO 226:2003, read between its frequencies on a log axis, held past its ends).
pub fn contour_db(hz: f32) -> f32 {
    let phon = PHON;
    let at = |i: usize| iso_spl(i, phon) - phon;
    let hz = if hz.is_finite() { hz.clamp(20.0, 12_500.0) } else { 1000.0 };
    for (i, w) in ISO_HZ.windows(2).enumerate() {
        if let [a, b] = w
            && hz >= *a
            && hz <= *b
        {
            let t = (hz / a).ln() / (b / a).ln();
            return at(i) + (at(i + 1) - at(i)) * t;
        }
    }
    0.0
}

/// The level band `i` of [`TEST_HZ`] is played at when the listener has added `added_db`, dBFS
/// RMS: never above [`MAX_DBFS`].
pub fn band_dbfs(i: usize, added_db: f32) -> f32 {
    let hz = TEST_HZ.get(i).copied().unwrap_or(1000.0);
    let added = if added_db.is_finite() { added_db } else { 0.0 };
    (REFERENCE_DBFS + contour_db(hz) + added).min(MAX_DBFS)
}

/// A burst of noise a third of an octave wide around `hz`: `frames` at `rate`, an RMS of 1, with
/// 20 ms fades. The same `seed` gives the same burst.
pub fn band_noise(hz: f32, rate: f32, frames: usize, seed: u32) -> Vec<f32> {
    let rate = if rate.is_finite() && rate >= 8000.0 { rate } else { 48_000.0 };
    let hz = if hz.is_finite() { hz.clamp(20.0, 0.45 * rate) } else { 1000.0 };
    let mut filters = [Biquad::design(FilterType::BandPass, hz, 4.32, 0.0, rate); 2];
    let mut state = seed.wrapping_mul(2_654_435_761).max(1);
    let lead = (rate * 0.05) as usize;
    let mut out = Vec::with_capacity(frames);
    for i in 0..frames + lead {
        // xorshift32: white noise in -1..1.
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let mut v = state as f32 / u32::MAX as f32 * 2.0 - 1.0;
        for f in &mut filters {
            v = f.process_sample(v);
        }
        // The filters settle before the burst starts.
        if i >= lead {
            out.push(v);
        }
    }
    let rms = (out.iter().map(|v| v * v).sum::<f32>() / out.len().max(1) as f32).sqrt();
    let fade = ((rate * 0.02) as usize).min(frames / 2).max(1);
    let n = out.len();
    for (i, v) in out.iter_mut().enumerate() {
        let edge = (i.min(n - 1 - i) as f32 / fade as f32).min(1.0);
        *v = if rms > 0.0 { *v / rms * edge } else { 0.0 };
    }
    out
}

/// A listener's answers.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Answers {
    /// Per band: the dB the right ear was raised over the left for the band to sit in the middle.
    pub balance: [f32; 10],
    /// Per band: the dB added for it to be as loud as the 1 kHz band (0 at 1 kHz itself).
    pub loudness: [f32; 10],
}

/// The correction the answers ask for: per ear and band, the gain that ear needs, dB. The loudness
/// answer is shared by both ears; the balance answer is split between them.
pub fn correction(a: &Answers) -> ([f32; 10], [f32; 10]) {
    let clean = |v: f32, r: &std::ops::RangeInclusive<f32>| if v.is_finite() { v.clamp(*r.start(), *r.end()) } else { 0.0 };
    let mut left = [0.0; 10];
    let mut right = [0.0; 10];
    for i in 0..10 {
        let loud = if i == REFERENCE { 0.0 } else { clean(a.loudness[i], &LOUDNESS_DB) };
        let bal = clean(a.balance[i], &BALANCE_DB);
        left[i] = loud - bal / 2.0;
        right[i] = loud + bal / 2.0;
    }
    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_contour_is_the_standards_at_sixty_phon() {
        // ISO 226:2003 at 60 phon: 1 kHz is 60 dB by definition; the lows and the top need more,
        // the ear's own resonance near 3 to 4 kHz needs less.
        assert!(contour_db(1000.0).abs() < 0.05);
        assert!((contour_db(125.0) - 15.6).abs() < 0.1, "{}", contour_db(125.0));
        assert!((contour_db(250.0) - 7.5).abs() < 0.1 && (contour_db(8000.0) - 11.7).abs() < 0.1);
        assert!((contour_db(3150.0) + 3.6).abs() < 0.1, "{}", contour_db(3150.0));
        assert_eq!(contour_db(5.0), contour_db(20.0));
        assert!(contour_db(f32::NAN).abs() < 0.05);
    }

    #[test]
    fn no_band_is_ever_played_above_the_ceiling() {
        for i in 0..10 {
            assert!(band_dbfs(i, *LOUDNESS_DB.end()) <= MAX_DBFS);
            assert!(band_dbfs(i, f32::INFINITY) <= MAX_DBFS);
        }
        assert_eq!(band_dbfs(REFERENCE, 0.0), REFERENCE_DBFS + contour_db(1000.0));
    }

    #[test]
    fn a_band_of_noise_sits_in_its_band_at_unit_level_and_repeats() {
        let x = band_noise(2000.0, 48_000.0, 24_000, 7);
        assert_eq!(x.len(), 24_000);
        assert_eq!(x, band_noise(2000.0, 48_000.0, 24_000, 7));
        assert_ne!(x, band_noise(2000.0, 48_000.0, 24_000, 8));
        let rms = |v: &[f32]| (v.iter().map(|s| s * s).sum::<f32>() / v.len() as f32).sqrt();
        assert!((rms(&x) - 1.0).abs() < 0.1, "{}", rms(&x));
        assert!(x[0].abs() < 1e-3 && x[23_999].abs() < 1e-3);
        // Little of it is left an octave and a half away.
        let mut far = Biquad::design(FilterType::BandPass, 6000.0, 4.32, 0.0, 48_000.0);
        let mut y = x.clone();
        far.process_block(&mut y);
        assert!(rms(&y) < 0.12, "{}", rms(&y));
        assert!(band_noise(f32::NAN, f32::NAN, 100, 0).iter().all(|v| v.is_finite()));
        assert!(band_noise(1000.0, 48_000.0, 0, 1).is_empty());
    }

    #[test]
    fn the_correction_shares_loudness_and_splits_balance() {
        let mut a = Answers::default();
        a.loudness[6] = 12.0;
        a.balance[6] = 6.0;
        a.loudness[REFERENCE] = 9.0;
        a.balance[0] = f32::NAN;
        a.loudness[9] = 900.0;
        let (l, r) = correction(&a);
        assert_eq!((l[6], r[6]), (9.0, 15.0));
        assert_eq!((l[REFERENCE], r[REFERENCE]), (0.0, 0.0));
        assert_eq!((l[0], r[0]), (0.0, 0.0));
        assert_eq!(l[9], *LOUDNESS_DB.end());
    }
}
