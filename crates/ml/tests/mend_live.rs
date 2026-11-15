//! Mend with its real model: a synthetic sung line with noise through the insert (rendered
//! offline, so every answer is waited for) against Clean Vocal's whole-file pass of the same
//! model. The two agree in level and closely in sound, between the notes too.
//!
//! Needs `srs_small` in the model folder (`SOUNDCRAFT_MODELS_DIR`); prints `SKIPPED` without it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_dsp::Plugin;
use soundcraft_dsp::plugins::mend::Mend;
use soundcraft_ml::{Ctx, Device};

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

fn rms(a: &[f32]) -> f64 {
    (a.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / a.len().max(1) as f64).sqrt()
}

#[test]
fn mend_agrees_with_the_whole_file_model() {
    let Some(dir) = installed() else { return };
    let x = sung_with_noise();

    // The whole file, as Clean Vocal: the model, then its make-up gain.
    let model = soundcraft_ml::models::load(&dir, soundcraft_ml::srs::ID).unwrap();
    let (stems, _) = soundcraft_ml::srs::run(&model, std::slice::from_ref(&x), Device::Cpu, &Ctx::default()).unwrap();
    let mut whole = stems.into_iter().next().unwrap().1;
    let makeup = soundcraft_ml::clean::makeup_gain(std::slice::from_ref(&x), &whole, RATE);
    soundcraft_ml::clean::apply_gain(&mut whole, makeup);
    let whole = whole.into_iter().next().unwrap();

    // Mend, offline.
    soundcraft_ml::live::start();
    let mut mend = Mend::new();
    mend.set_offline(true);
    mend.prepare(RATE as f32, 512, 2);
    let lat = mend.latency();
    let padded: Vec<f32> = x.iter().copied().chain(std::iter::repeat_n(0.0, lat)).collect();
    let mut out = Vec::with_capacity(padded.len());
    for block in padded.chunks(512) {
        let mut io = vec![block.to_vec(), block.to_vec()];
        mend.process(&mut io, block.len());
        assert_eq!(io[0], io[1], "the same sound on both sides stays the same");
        out.extend_from_slice(&io[0]);
    }
    let mut r = [f32::NAN; 8];
    mend.readouts(&mut r);
    assert_eq!((r[0], r[4]), (2.0, 0.0), "the model answered, never late: {r:?}");
    let mended = &out[lat..];

    // After the first block (whose past is silence): the same level, and close in sound.
    let from = RATE as usize * 2;
    let (w, m) = (&whole[from..], &mended[from..]);
    let level_db = 20.0 * (rms(m) / rms(w)).log10();
    let diff: Vec<f32> = w.iter().zip(m).map(|(a, b)| a - b).collect();
    let agreement_db = 20.0 * (rms(w) / rms(&diff)).log10();
    println!("level {level_db:+.2} dB, agreement {agreement_db:.1} dB");
    assert!(level_db.abs() < 1.5, "Mend is {level_db:+.2} dB from the whole-file result");
    assert!(agreement_db > 10.0, "Mend is only {agreement_db:.1} dB from the whole-file result");

    // Between the notes (3.7 to 3.95 s) there is only noise: Mend leaves what the whole-file
    // pass leaves there, no more.
    let gap = (RATE as f32 * 3.7) as usize..(RATE as f32 * 3.95) as usize;
    let (noise_in, noise_whole, noise_mend) = (rms(&x[gap.clone()]), rms(&whole[gap.clone()]), rms(&mended[gap]));
    let db = |v: f64| 20.0 * (v / noise_in).log10();
    println!("the gap against the input: whole file {:+.1} dB, Mend {:+.1} dB", db(noise_whole), db(noise_mend));
    assert!(noise_mend < 1.5 * noise_whole, "noise in the gap: {noise_whole} whole-file, {noise_mend} Mend");
}
