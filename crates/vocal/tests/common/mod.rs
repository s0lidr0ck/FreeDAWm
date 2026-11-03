//! Shared helpers for the vocal tests: synthetic signals, small statistics and the local-only
//! real test audio.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// A harmonic tone whose fundamental follows `f0(t)` (Hz at time `t` seconds), with `harmonics`
/// partials at amplitude `1/k` (partials above Nyquist are skipped), normalised to a 0.5 peak.
pub fn harmonic_tone(sr: u32, secs: f32, f0: impl Fn(f32) -> f32, harmonics: usize) -> Vec<f32> {
    harmonic_tone_with(sr, secs, f0, harmonics, |k| 1.0 / k as f64)
}

/// [`harmonic_tone`] with each partial's amplitude given by `amp(k)` (k = 1 is the fundamental).
pub fn harmonic_tone_with(sr: u32, secs: f32, f0: impl Fn(f32) -> f32, harmonics: usize, amp: impl Fn(usize) -> f64) -> Vec<f32> {
    let n = (secs * sr as f32).round() as usize;
    let mut phase = 0.0f64;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr as f32;
        let f = f0(t) as f64;
        let mut s = 0.0f64;
        for k in 1..=harmonics {
            if f * k as f64 >= sr as f64 / 2.0 {
                break;
            }
            s += amp(k) * (phase * k as f64).sin();
        }
        out.push(s);
        phase += std::f64::consts::TAU * f / sr as f64;
        if phase > std::f64::consts::TAU * 1e6 {
            phase %= std::f64::consts::TAU;
        }
    }
    let peak = out.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let g = if peak > 0.0 { 0.5 / peak } else { 0.0 };
    out.into_iter().map(|v| (v * g) as f32).collect()
}

/// Deterministic white noise in [-0.5, 0.5) (xorshift64).
pub fn noise(sr: u32, secs: f32, seed: u64) -> Vec<f32> {
    let n = (secs * sr as f32).round() as usize;
    let mut s = seed.max(1);
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 40) as f32 / (1u64 << 24) as f32) - 0.5
        })
        .collect()
}

pub fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

pub fn median(xs: impl Iterator<Item = f32>) -> f32 {
    let mut v: Vec<f32> = xs.collect();
    assert!(!v.is_empty(), "median of nothing");
    v.sort_by(f32::total_cmp);
    v[v.len() / 2]
}

/// MIDI cents (A4 = 6900) of a frequency in Hz.
pub fn hz_to_cents(hz: f32) -> f32 {
    6900.0 + 1200.0 * (hz / 440.0).log2()
}

/// The local-only test take `name` (git-ignored, never bundled): `$FREEDAWM_VOCAL_TEST_DIR/<name>` (or the older `$SOUNDCRAFT_VOCAL_TEST_DIR`)
/// when that variable is set, else the first `cleanq-ceiling-renders/<name>` found walking up from
/// `CARGO_MANIFEST_DIR` (read at run time). When it is missing this prints `SKIPPED <name>: test
/// audio missing` and returns `None`, or fails the test under `FIDELITY_STRICT=1`.
pub fn test_audio(name: &str) -> Option<PathBuf> {
    let found = match std::env::var_os("FREEDAWM_VOCAL_TEST_DIR").or_else(|| std::env::var_os("SOUNDCRAFT_VOCAL_TEST_DIR")) {
        Some(dir) => Some(Path::new(&dir).join(name)).filter(|p| p.is_file()),
        None => std::env::var_os("CARGO_MANIFEST_DIR")
            .and_then(|m| Path::new(&m).ancestors().map(|d| d.join("cleanq-ceiling-renders").join(name)).find(|p| p.is_file())),
    };
    if found.is_none() {
        let msg = format!("{name}: test audio missing (set FREEDAWM_VOCAL_TEST_DIR, or FIDELITY_STRICT=1 to fail instead of skipping)");
        if std::env::var("FIDELITY_STRICT").is_ok_and(|v| v == "1") {
            panic!("{msg}");
        }
        eprintln!("SKIPPED {msg}");
    }
    found
}

/// Channel 0 of an audio file and its sample rate.
pub fn load_mono(path: &Path) -> (Vec<f32>, u32) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let ext = path.extension().and_then(|e| e.to_str());
    let (info, buf) = soundcraft_audio_io::decode(&bytes, ext).unwrap_or_else(|e| panic!("decode {}: {e}", path.display()));
    let ch0 = buf.channels.into_iter().next().unwrap_or_default();
    (ch0, info.sample_rate)
}
