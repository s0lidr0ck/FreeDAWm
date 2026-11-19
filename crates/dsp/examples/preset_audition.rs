//! Renders factory presets of the built-in synths to WAV files for listening.
//!
//! `cargo run --release -p soundcraft-dsp --example preset_audition -- --list`
//! `cargo run --release -p soundcraft-dsp --example preset_audition -- <out dir> <synth id> <preset name>…`
//!
//! Each file plays a short phrase suited to the preset's group three times: with the macros as
//! saved, with Macro 1 at 0 %, and with Macro 1 at 100 %, a short gap between.
//!
//! A preset of the showcase set (the "Patched" group, whose point is what velocity and the mod
//! wheel do) plays its phrase five times instead: soft (velocity 40), hard (velocity 120), with
//! the mod wheel swept up and back down while it plays (velocity 90), then with Macro 1 at 0 %
//! and at 100 %. Its phrase follows the job its description names (a bass, a lead, a pad...).

use soundcraft_dsp::plugins::preset_lab::{Change, Macros, Note, Preset, RATE, SYNTHS, State, WHEEL, presets, render, render_changing, secs};
use std::io::Write;
use std::path::Path;

/// The phrase a preset is heard on, and how long a pass lasts (seconds).
fn phrase(p: &Preset) -> (Vec<Note>, f32) {
    let n = |at: f32, len: f32, key: u8, vel: u8| Note { at: secs(at), len: secs(len), key, vel };
    let g = if showcase(p) { job(p) } else { p.group.to_lowercase() };
    if g.contains("bass") {
        // A riff in C: C2 C2 Eb2 F2 G2 C3 Bb2 G2.
        let keys = [36u8, 36, 39, 41, 43, 48, 46, 43];
        (keys.iter().enumerate().map(|(i, &k)| n(i as f32 * 0.375, 0.3, k, if i % 2 == 0 { 110 } else { 84 })).collect(), 3.6)
    } else if g.contains("lead") {
        let keys = [60u8, 63, 65, 67, 70, 67, 72, 70, 67, 65, 63, 60];
        (keys.iter().enumerate().map(|(i, &k)| n(i as f32 * 0.3, 0.27, k, 100)).collect(), 4.4)
    } else if g.contains("pad") || g.contains("texture") || g.contains("string") {
        let mut v: Vec<Note> = [48u8, 55, 60, 64].iter().map(|&k| n(0.0, 2.6, k, 90)).collect();
        v.extend([45u8, 52, 60, 64].iter().map(|&k| n(2.7, 2.6, k, 90)));
        (v, 7.2)
    } else if g.contains("effect") || g.contains("perc") {
        (vec![n(0.0, 1.5, 48, 110), n(2.0, 0.2, 60, 120), n(2.6, 0.2, 60, 70)], 4.2)
    } else {
        // Keys, plucks, bells, mallets, brass, basics: a broken chord, then the chord.
        let keys = [48u8, 55, 60, 64, 67, 72, 67, 64];
        let mut v: Vec<Note> = keys.iter().enumerate().map(|(i, &k)| n(i as f32 * 0.25, 0.4, k, if i % 3 == 0 { 115 } else { 80 })).collect();
        v.extend([48u8, 55, 60, 64].iter().map(|&k| n(2.2, 1.6, k, 95)));
        (v, 4.8)
    }
}

/// Whether `p` is in the showcase set (the "Patched" group).
fn showcase(p: &Preset) -> bool {
    p.group == "Patched"
}

/// The job a showcase preset's description names first (its group says only "Patched"), as a
/// group name [`phrase`] knows.
fn job(p: &Preset) -> String {
    let d = p.desc.to_lowercase();
    let first = d.split(':').next().unwrap_or(&d).to_string();
    let jobs = [
        ("struck", "keys"),
        ("voice", "lead"),
        ("bass", "bass"),
        ("lead", "lead"),
        ("pad", "pad"),
        ("choir", "pad"),
        ("strings", "pad"),
        ("brass", "pad"),
        ("riser", "pad"),
        ("drone", "texture"),
        ("texture", "texture"),
        ("swarm", "texture"),
        ("field", "texture"),
        ("bells", "effects"),
        ("hail", "effects"),
    ];
    jobs.iter().find(|(w, _)| first.contains(w)).map_or_else(|| "keys".to_string(), |(_, g)| (*g).to_string())
}

/// `notes` with every velocity set to `vel`.
fn at_velocity(notes: &[Note], vel: u8) -> Vec<Note> {
    notes.iter().map(|n| Note { vel, ..*n }).collect()
}

fn wav(path: &Path, x: &[Vec<f32>; 2]) -> std::io::Result<()> {
    let n = x[0].len().min(x[1].len());
    let data = (n * 4) as u32;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&(RATE as u32).to_le_bytes())?;
    f.write_all(&((RATE as u32) * 4).to_le_bytes())?;
    f.write_all(&4u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data.to_le_bytes())?;
    for i in 0..n {
        for c in x {
            f.write_all(&((c[i].clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
        }
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--list") {
        for s in SYNTHS {
            for p in presets(s) {
                println!("{s}\t{}\t{}\t{}\t{}", p.tier, p.group, p.name, p.macros.iter().map(|m| m.0.as_str()).collect::<Vec<_>>().join(", "));
            }
        }
        return;
    }
    let (Some(dir), Some(synth)) = (args.first(), args.get(1)) else {
        eprintln!("usage: preset_audition <out dir> <synth id> <preset name>… | --list");
        std::process::exit(2);
    };
    let Some(synth) = SYNTHS.iter().copied().find(|s| s == synth) else {
        eprintln!("unknown synth `{synth}`");
        std::process::exit(2);
    };
    let _ = std::fs::create_dir_all(dir);
    let all = presets(synth);
    for name in &args[2..] {
        let Some(p) = all.iter().find(|p| &p.name == name) else {
            eprintln!("no preset `{name}`");
            continue;
        };
        let (notes, len) = phrase(p);
        let mut out = [Vec::new(), Vec::new()];
        let mut passes: Vec<Option<[Vec<f32>; 2]>> = Vec::new();
        if showcase(p) {
            // Soft, then hard, then the wheel swept up and back down over the phrase.
            passes.push(render(p, State::SAVED, &at_velocity(&notes, 40), secs(len)));
            passes.push(render(p, State::SAVED, &at_velocity(&notes, 120), secs(len)));
            let steps = 40;
            let sweep: Vec<Change> = (0..=steps)
                .map(|k| {
                    let t = k as f32 / steps as f32;
                    Change { at: secs(len * t), param: WHEEL, value: 1.0 - (2.0 * t - 1.0).abs() }
                })
                .collect();
            passes.push(render_changing(p, RATE, State::SAVED, &at_velocity(&notes, 90), secs(len), &sweep));
            for macros in [Macros::One(0, 0.0), Macros::One(0, 100.0)] {
                passes.push(render(p, State { macros, wheel: 0.0 }, &notes, secs(len)));
            }
        } else {
            for macros in [Macros::Saved, Macros::One(0, 0.0), Macros::One(0, 100.0)] {
                passes.push(render(p, State { macros, wheel: 0.0 }, &notes, secs(len)));
            }
        }
        for x in passes.into_iter().flatten() {
            for c in 0..2 {
                out[c].extend_from_slice(&x[c]);
                out[c].extend(std::iter::repeat_n(0.0, secs(0.5)));
            }
        }
        let file = format!("{}-{}.wav", synth.split('_').next().unwrap_or(synth), name.to_lowercase().replace(' ', "-"));
        match wav(&Path::new(dir).join(&file), &out) {
            Ok(()) => println!("wrote {file}"),
            Err(e) => eprintln!("{file}: {e}"),
        }
    }
}
