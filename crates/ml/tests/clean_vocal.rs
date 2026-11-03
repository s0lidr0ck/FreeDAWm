//! Clean Vocal's Vocal mode (Smule Renaissance alone, then the fixed make-up gain) on a synthetic
//! sung line with noise: the level holds steady through sustained notes (no jump over 3 dB between
//! 20 ms windows on the voiced part) and the result is as loud as the source (within 1 dB), its
//! peak at or under −1 dBFS.
//!
//! Needs `srs_small` in the model folder (`SOUNDCRAFT_MODELS_DIR`); prints `SKIPPED` without it.

use soundcraft_ml::clean::{CleanOptions, level_stats};
use soundcraft_ml::{Ctx, Device, Job, JobKind};

const RATE: u32 = 48_000;

/// Four sustained sung notes (harmonics with a vocal-ish tilt and formant bumps, 5.5 Hz vibrato,
/// soft attack and release) with 0.4 s gaps, plus white noise about 40 dB under the voice.
fn sung_with_noise() -> Vec<f32> {
    let notes = [220.0f32, 246.9, 261.6, 293.7];
    let (note_s, gap_s) = (1.6f32, 0.4f32);
    let len = ((note_s + gap_s) * notes.len() as f32 * RATE as f32) as usize;
    let mut out = vec![0.0f32; len];
    let mut phase = 0.0f32;
    for (i, v) in out.iter_mut().enumerate() {
        let t = i as f32 / RATE as f32;
        let slot = (t / (note_s + gap_s)) as usize;
        let tn = t - slot as f32 * (note_s + gap_s);
        let Some(&f0) = notes.get(slot) else { continue };
        if tn >= note_s {
            continue;
        }
        let env = (tn / 0.06).min(1.0) * ((note_s - tn) / 0.08).min(1.0);
        let f = f0 * (1.0 + 0.03 * (2.0 * std::f32::consts::PI * 5.5 * t).sin());
        phase += 2.0 * std::f32::consts::PI * f / RATE as f32;
        let mut s = 0.0;
        // Fixed scattered harmonic phases, as a voice has (aligned ones make a buzz-like crest).
        for k in 1..=16 {
            let ph = (k * k) as f32 * 1.7;
            let fk = f * k as f32;
            let formant = 1.0 + 2.0 * (-((fk - 700.0) / 250.0).powi(2)).exp() + 1.5 * (-((fk - 2500.0) / 400.0).powi(2)).exp();
            s += (k as f32 * phase + ph).sin() * formant / k as f32;
        }
        *v = 0.12 * env * s;
    }
    let mut seed = 11u32;
    for v in &mut out {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *v += ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.004;
    }
    out
}

fn installed() -> Option<std::path::PathBuf> {
    let dir = soundcraft_ml::models::models_dir();
    if soundcraft_ml::models::load(&dir, "srs_small").is_err() {
        println!("SKIPPED: srs_small is not in {}", dir.display());
        return None;
    }
    Some(dir)
}

fn vocal(x: &[Vec<f32>], dir: std::path::PathBuf) -> Vec<Vec<f32>> {
    let job = Job {
        kind: JobKind::Clean(CleanOptions::vocal(1.0)),
        audio: x.to_vec(),
        rate: RATE,
        device: Device::Cpu,
        models_dir: dir,
        quality: soundcraft_ml::Quality::Fast,
    };
    let out = soundcraft_ml::pipeline::run(&job, &Ctx::default()).unwrap();
    println!("{:?}", out.notes);
    let y = out.stems.into_iter().next().unwrap().audio;
    assert_eq!((y.len(), y[0].len()), (x.len(), x[0].len()));
    y
}

fn peak(a: &[f32]) -> f32 {
    a.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

/// The sustained (voiced) part: 20 ms windows within 12 dB of the loudest. (Note onsets and ends,
/// where a window is half silence, are left out.)
const VOICED_DB: f64 = 12.0;

/// The make-up's ceiling, -1 dBFS (and a hair for float rounding).
const CEILING: f32 = 0.891_251 * 1.000_01;

#[test]
fn vocal_mode_holds_the_level_on_sustained_sung_notes() {
    let Some(dir) = installed() else { return };
    let x = vec![sung_with_noise()];
    let y = vocal(&x, dir);
    let st = level_stats(&x, &y, RATE, VOICED_DB);
    println!("{st:?}");
    assert!(st.windows > 200, "{st:?}");
    assert_eq!(st.jumps, 0, "level jumps on the sustained notes: {st:?}");
    assert!(st.sd_db < 1.5, "{st:?}");
    // Loudness matched (Renaissance raises this voice's crest factor about 1.6 dB: the peak may
    // pass the source's; the only limit is -1 dBFS, far above here).
    assert!(st.loudness_diff_db.abs() < 1.0, "loudness off by {:.2} dB", st.loudness_diff_db);
    assert!(peak(&y[0]) <= CEILING, "peak {}", peak(&y[0]));
}

#[test]
fn vocal_mode_matches_the_loudness_of_a_real_sung_vocal_with_noise() {
    let Some(dir) = installed() else { return };
    let Ok(bytes) = std::fs::read(dir.join("srs_small").join("golden").join("input_12s.f32")) else {
        println!("SKIPPED: no golden input");
        return;
    };
    // A live vocal (12 s of the owner's song) plus white noise about 40 dB under it.
    let mut seed = 5u32;
    let x: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|b| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            f32::from_le_bytes([b[0], b[1], b[2], b[3]]) + ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.004
        })
        .collect();
    let x = vec![x];
    let y = vocal(&x, dir);
    let st = level_stats(&x, &y, RATE, VOICED_DB);
    println!("{st:?}");
    // Renaissance also de-clips, which raises a live vocal's peaks: loudness is still matched.
    assert!(st.loudness_diff_db.abs() < 1.0, "loudness off by {:.2} dB", st.loudness_diff_db);
    assert!(peak(&y[0]) <= CEILING, "peak {}", peak(&y[0]));
    assert!(st.sd_db < 3.0, "{st:?}");
}
