//! The synth playing a user table: what it plays, that it does not alias, that a swap while a
//! note sounds is smooth, and that going back to the built-in bank sounds as if no table had
//! ever been loaded.

use super::user::{BuiltTable, UserTable, WAVE_LEN};
use super::*;
use crate::SampleSource;
use std::sync::Arc;

const SR: f32 = 48_000.0;

fn synth() -> WavetableSynth {
    let mut s = WavetableSynth::new();
    s.prepare(SR, 256, 2);
    for (id, v) in [("cutoff", 20_000.0), ("amp_sustain", 100.0), ("amp_attack", 0.5)] {
        assert!(s.set_param(id, v));
    }
    s
}

fn wave(harmonics: &[(usize, f32)]) -> Vec<f32> {
    (0..WAVE_LEN)
        .map(|i| {
            let x = i as f32 / WAVE_LEN as f32;
            harmonics.iter().map(|(k, a)| a * (std::f32::consts::TAU * *k as f32 * x).sin()).sum()
        })
        .collect()
}

fn table(waves: &[Vec<f32>]) -> Arc<dyn SampleSource> {
    Arc::new(BuiltTable::new(&UserTable::new("t", waves).unwrap()))
}

/// Renders `blocks` blocks of 256 (left channel), `each` called before every block.
fn run(s: &mut WavetableSynth, blocks: usize, mut each: impl FnMut(&mut WavetableSynth, usize)) -> Vec<f32> {
    let mut io = vec![vec![0.0f32; 256]; 2];
    let mut out = Vec::new();
    for b in 0..blocks {
        each(s, b);
        s.process(&mut io, 256);
        out.extend_from_slice(&io[0]);
    }
    out
}

fn mag(x: &[f32], hz: f64) -> f64 {
    let n = x.len() as f64;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (i, v) in x.iter().enumerate() {
        let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n).cos();
        let a = std::f64::consts::TAU * hz * i as f64 / f64::from(SR);
        re += f64::from(*v) * w * a.cos();
        im += f64::from(*v) * w * a.sin();
    }
    (re * re + im * im).sqrt()
}

/// The built-in tables' aliasing measure (`tests::aliasing_db`): energy where harmonics up to the
/// 200th fold back to, more than 40 Hz from any harmonic, over the energy at the harmonics, dB.
fn aliasing_db(x: &[f32], f0: f64) -> f64 {
    let nyq = f64::from(SR) / 2.0;
    let near_harmonic = |f: f64| {
        let k = (f / f0).round();
        (f - k * f0).abs() < 40.0
    };
    let mut harm = 0.0;
    let mut k = 1.0;
    while k * f0 < nyq - 100.0 {
        harm += mag(x, k * f0).powi(2);
        k += 1.0;
    }
    let mut alias = 0.0;
    let mut seen: Vec<f64> = Vec::new();
    for h in 1..=200 {
        for j in 1..=2 {
            let f = (f64::from(SR) * j as f64 - h as f64 * f0).abs();
            if f > 40.0 && f < nyq - 40.0 && !near_harmonic(f) && !seen.iter().any(|s| (s - f).abs() < 5.0) {
                seen.push(f);
                alias += mag(x, f).powi(2);
            }
        }
    }
    10.0 * (alias.max(1e-30) / harm).log10()
}

#[test]
fn a_user_table_plays_its_own_harmonics_and_only_with_the_right_key() {
    let mut s = synth();
    assert!(!s.set_asset("irPathTwin", Some(table(&[wave(&[(1, 1.0)])]))), "another key is not taken");
    assert!(s.set_asset(crate::PREPARED_ASSET_KEY, Some(table(&[wave(&[(1, 1.0), (3, 0.5)])]))));
    let note = 57; // 220 Hz
    let out = run(&mut s, 64, |s, b| {
        if b == 0 {
            s.note_on(0, note, 127);
        }
    });
    let x = &out[4096..];
    let f0 = 220.0;
    let (h1, h2, h3) = (mag(x, f0), mag(x, 2.0 * f0), mag(x, 3.0 * f0));
    eprintln!("user table at 220 Hz: h1 {h1:.1}, h2 {h2:.3}, h3 {h3:.1}");
    assert!((h3 / h1 - 0.5).abs() < 0.03, "the third harmonic at half the first: {}", h3 / h1);
    assert!(h2 / h1 < 1e-3, "nothing at the second: {}", h2 / h1);
    while s.take_released_asset().is_some() {}
}

/// Measured as the built-in tables' aliasing test measures: a bright user table (every harmonic
/// to 1000, as a saw) played at a high note leaves next to nothing between the harmonics.
#[test]
fn a_user_table_does_not_alias_at_a_high_note() {
    let saw: Vec<(usize, f32)> = (1..=1000).map(|k| (k, 1.0 / k as f32)).collect();
    let mut s = synth();
    assert!(s.set_asset(crate::PREPARED_ASSET_KEY, Some(table(&[wave(&saw)]))));
    let out = run(&mut s, 80, |s, b| {
        if b == 0 {
            s.note_on(0, 100, 127);
        }
    });
    let f0 = f64::from(crate::util::note_hz(100.0));
    let db = aliasing_db(&out[4096..], f0);
    eprintln!("user table aliasing at {f0:.0} Hz: {db:.1} dB");
    assert!(db < -60.0, "{db:.1} dB");
}

#[test]
fn a_swap_while_a_note_sounds_is_smooth() {
    let a = table(&[wave(&[(1, 1.0)])]);
    let b = table(&[wave(&[(1, 0.3), (2, 0.7), (5, 0.4)])]);
    let mut s = synth();
    assert!(s.set_asset(crate::PREPARED_ASSET_KEY, Some(Arc::clone(&a))));
    let out = run(&mut s, 40, |s, k| {
        if k == 0 {
            s.note_on(0, 45, 127);
        }
        if k == 20 {
            assert!(s.set_asset(crate::PREPARED_ASSET_KEY, Some(Arc::clone(&b))));
        }
    });
    let jump = |r: std::ops::Range<usize>| out[r].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    let steady_old = jump(2_000..5_120);
    let steady_new = jump(6_500..10_240);
    let at_swap = jump(5_100..6_200);
    eprintln!("largest step: old {steady_old:.4}, new {steady_new:.4}, across the swap {at_swap:.4}");
    assert!(at_swap <= steady_old.max(steady_new) * 1.05, "no click at the swap");
    // The old table came back to the host, which still holds it.
    let mut back = 0;
    while let Some(x) = s.take_released_asset() {
        assert!(Arc::strong_count(&x) >= 2);
        back += 1;
    }
    assert_eq!(back, 1);
}

#[test]
fn back_to_the_bank_sounds_as_if_no_table_was_ever_loaded() {
    let notes = |s: &mut WavetableSynth, b: usize| {
        if b == 0 {
            s.note_on(0, 60, 100);
        }
        if b == 10 {
            s.note_on(17, 67, 90);
        }
    };
    let mut p = synth();
    p.reset();
    let plain = run(&mut p, 40, notes);
    // A table that is cleared again before the first block, and one cleared after playing.
    let mut s = synth();
    assert!(s.set_asset(crate::PREPARED_ASSET_KEY, Some(table(&[wave(&[(2, 1.0)])]))));
    assert!(s.set_asset(crate::PREPARED_ASSET_KEY, None));
    s.reset();
    let cleared = run(&mut s, 40, notes);
    assert_eq!(plain, cleared, "bit for bit");
    // While it fades out the bank comes back, and after the fade it plays the bank alone.
    let mut s = synth();
    s.reset();
    assert!(s.set_asset(crate::PREPARED_ASSET_KEY, Some(table(&[wave(&[(2, 1.0)])]))));
    let mut both = run(&mut s, 40, |s, b| {
        notes(s, b);
        if b == 20 {
            assert!(s.set_asset(crate::PREPARED_ASSET_KEY, None));
        }
    });
    assert_ne!(both[..5_000], plain[..5_000]);
    // (The filters still ring with what the table played for a moment.)
    let after = both.split_off(20 * 256 + 2_000);
    let diff = after.iter().zip(&plain[20 * 256 + 2_000..]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(diff < 1e-5, "{diff}");
}
