//! The owner's A/B tool for the vocal tuner: analyses a vocal take, applies a factory preset to
//! every note, renders it and prints the selection summary and the harmonic flux ("wobble") of
//! the input and the render.
//!
//! `cargo run --release -p soundcraft-vocal --example vocal_ab -- <in.wav> <out.wav> ["preset name"]`
//!
//! The preset defaults to "Like Melodyne 70 %". Only channel 0 of the input is used; the output is
//! a mono 24-bit WAV.

#[cfg(target_arch = "wasm32")]
fn main() {
    println!("vocal_ab: desktop only");
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    match desktop::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("vocal_ab: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use soundcraft_vocal::metrics::{harmonic_flux_db, steady_frames};
    use soundcraft_vocal::resynth::render_all;
    use soundcraft_vocal::{NoteEdit, NoteId, PitchClassSet, analyse, build_curves, factory_presets, selection_summary, typical_level_db};
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    const DEFAULT_PRESET: &str = "Like Melodyne 70 %";

    pub fn run() -> Result<(), String> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let (Some(input), Some(output)) = (args.first(), args.get(1)) else {
            return Err("usage: vocal_ab <in.wav> <out.wav> [preset name]".into());
        };
        let presets = factory_presets();
        let name = args.get(2).map_or(DEFAULT_PRESET, String::as_str);
        let Some(preset) = presets.iter().find(|p| p.name == name) else {
            let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
            return Err(format!("unknown preset {name:?}; one of: {}", names.join(", ")));
        };

        let bytes = std::fs::read(input).map_err(|e| format!("read {input}: {e}"))?;
        let ext = Path::new(input).extension().and_then(|e| e.to_str());
        let (info, buf) = soundcraft_audio_io::decode(&bytes, ext).map_err(|e| format!("decode {input}: {e}"))?;
        let x = buf.channels.into_iter().next().ok_or("no audio channels")?;
        let sr = info.sample_rate;
        let cancel = AtomicBool::new(false);

        let t0 = Instant::now();
        let a = analyse(&x, sr, &cancel).map_err(|e| e.to_string())?;
        let t_analyse = t0.elapsed();
        let typical = typical_level_db(&a.notes.iter().map(|n| n.level_db).collect::<Vec<_>>());
        let edits: BTreeMap<NoteId, NoteEdit> = a
            .notes
            .iter()
            .map(|n| {
                let e = NoteEdit {
                    centre: preset.centre,
                    drift: preset.drift,
                    vibrato: preset.vibrato,
                    raise: preset.raise,
                    lower: preset.lower,
                    typical_db: Some(typical),
                    ..Default::default()
                };
                (n.id, e)
            })
            .collect();
        let scale: Option<PitchClassSet> = None;
        let t1 = Instant::now();
        let curves = build_curves(&a, &edits, &scale);
        let y = render_all(&x, &a.track, &curves, &cancel).map_err(|e| e.to_string())?;
        let t_render = t1.elapsed();

        let out = soundcraft_audio_io::AudioBuffer { sample_rate: sr, channels: vec![y.clone()] };
        let enc = soundcraft_audio_io::encode(&out, &soundcraft_audio_io::EncodeOptions::default()).map_err(|e| format!("encode: {e}"))?;
        std::fs::write(output, enc).map_err(|e| format!("write {output}: {e}"))?;

        println!(
            "{input}: {:.2} s at {sr} Hz, {} notes, {} unvoiced regions (analysis {:.0} ms)",
            x.len() as f64 / f64::from(sr),
            a.notes.len(),
            a.unvoiced.len(),
            t_analyse.as_secs_f64() * 1e3
        );
        println!(
            "preset {:?}: centre {:.2}, drift {:.2}, vibrato {:+.2}, raise {:.2}, lower {:.2}; typical level {typical:.1} dB",
            preset.name, preset.centre, preset.drift, preset.vibrato, preset.raise, preset.lower
        );
        let ids: Vec<NoteId> = a.notes.iter().map(|n| n.id).collect();
        if let Some(s) = selection_summary(&a, &edits, &ids, &scale) {
            println!(
                "summary: {} notes; off pitch {:.1} c -> {:.1} c; drift {:.1} c -> {:.1} c; level spread {:.1} dB",
                s.count, s.avg_off_before, s.avg_off_after, s.drift_before, s.drift_after, s.level_spread_db
            );
        }
        let shifted = curves.shift_cents.iter().filter(|&&v| v != 0.0).count();
        let max_shift = curves.shift_cents.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        println!("curves: {shifted}/{} frames shifted, max |shift| {max_shift:.1} c", curves.shift_cents.len());
        let steady = steady_frames(&a.track);
        let hop = a.track.hop_samples;
        println!(
            "harmonic flux (steady frames from our tracker, {}): input {:.3} dB, render {:.3} dB",
            steady.iter().filter(|&&s| s).count(),
            harmonic_flux_db(&x, sr, &steady, hop),
            harmonic_flux_db(&y, sr, &steady, hop)
        );
        println!("render {:.0} ms -> {output}", t_render.as_secs_f64() * 1e3);
        Ok(())
    }
}
