//! The pitch tracker on the real take (`00_original.wav`, local only) against the spike's
//! octave-repaired WORLD Harvest track (`tests/data/00_original_harvest.csv`: derived numbers, no
//! audio; one row per 5 ms, `f0_hz = 0` when unvoiced).

mod common;

use common::{hz_to_cents, load_mono, no_cancel, test_audio};
use soundcraft_vocal::track_pitch;
use std::path::Path;

/// `(t_s, f0_hz)` rows of the reference track.
fn read_harvest_csv() -> Vec<(f64, f64)> {
    let dir = std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    let path = Path::new(&dir).join("tests/data/00_original_harvest.csv");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    text.lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (t, f) = l.split_once(',').unwrap_or_else(|| panic!("bad row {l:?}"));
            (t.trim().parse().expect("t_s"), f.trim().parse().expect("f0_hz"))
        })
        .collect()
}

/// The value at quantile `q` (0..=1) of `v` (sorted in place; nearest rank).
fn quantile(v: &mut [f32], q: f32) -> f32 {
    v.sort_by(f32::total_cmp);
    let i = ((v.len() - 1) as f32 * q).round() as usize;
    v[i]
}

#[test]
fn agrees_with_the_spike_harvest_track() {
    let Some(p) = test_audio("00_original.wav") else { return };
    let (x, sr) = load_mono(&p);
    let t = track_pitch(&x, sr, &no_cancel()).unwrap();
    let r = read_harvest_csv();
    assert!(r.len() > 2000, "reference has {} rows", r.len());
    let dt = r[1].0 - r[0].0;

    let mut deltas: Vec<f32> = Vec::new();
    let (mut compared, mut agree) = (0usize, 0usize);
    for (i, f) in t.frames.iter().enumerate() {
        // Compare frame i with the reference row nearest its centre time.
        let secs = t.sample_of_frame(i) as f64 / f64::from(t.sample_rate);
        let j = ((secs - r[0].0) / dt).round();
        if j < 0.0 {
            continue;
        }
        let Some(&(rt, rf)) = r.get(j as usize) else { continue };
        if (rt - secs).abs() > dt / 2.0 + 1e-6 {
            continue;
        }
        compared += 1;
        let ref_voiced = rf > 0.0;
        if ref_voiced == f.cents.is_some() {
            agree += 1;
        }
        if let (Some(c), true) = (f.cents, ref_voiced) {
            deltas.push((c - hz_to_cents(rf as f32)).abs());
        }
    }
    assert!(compared > 2000, "only {compared} frames lined up with the reference");
    assert!(!deltas.is_empty(), "no frame voiced in both tracks");

    let both = deltas.len();
    let octave = deltas.iter().filter(|&&d| d > 600.0).count() as f32 / both as f32;
    let median = quantile(&mut deltas, 0.5);
    let p90 = quantile(&mut deltas, 0.9);
    let voicing = agree as f32 / compared as f32;
    println!(
        "median Δ {median:.2} c, p90 {p90:.2} c, octave {:.2} % ({both} both-voiced frames), voicing {:.2} % ({compared} frames)",
        octave * 100.0,
        voicing * 100.0
    );
    assert!(median <= 5.0, "median |Δ| {median:.2} c > 5 c");
    assert!(p90 <= 20.0, "p90 |Δ| {p90:.2} c > 20 c");
    assert!(octave < 0.01, "octave errors {:.2} % >= 1 %", octave * 100.0);
    assert!(voicing >= 0.90, "voicing agreement {:.2} % < 90 %", voicing * 100.0);
}
