//! Dumps a vocal take's analysis as JSON: the pitch track and the notes found by each note
//! detection mode, for plotting and comparing segmentations outside the app.
//!
//! `cargo run --release -p soundcraft-vocal --example vocal_notes -- <in.wav> <out.json>`
//!
//! Only channel 0 of the input is used. The JSON holds `sample_rate`, `hop`, `frames`
//! (`[cents | null, level_db, voiced_prob]` per 5 ms frame) and `classic` / `musical` note lists
//! (`{start, end, scoop, core, centre}` in frames, centre in MIDI cents).

#[cfg(target_arch = "wasm32")]
fn main() {
    println!("vocal_notes: desktop only");
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    match desktop::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("vocal_notes: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use serde_json::{Value, json};
    use soundcraft_vocal::musical::core_range;
    use soundcraft_vocal::{Note, NoteMode, PitchTrack, segment_with, track_pitch};
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

    fn notes_json(track: &PitchTrack, notes: &[Note], mode: NoteMode) -> Value {
        Value::Array(
            notes
                .iter()
                .map(|n| json!({"start": n.start, "end": n.end, "scoop": n.scoop, "core": (mode == NoteMode::Musical).then(|| core_range(track, n)), "centre": n.centre_cents, "level": n.level_db}))
                .collect(),
        )
    }

    pub fn run() -> Result<(), String> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let (Some(input), Some(output)) = (args.first(), args.get(1)) else {
            return Err("usage: vocal_notes <in.wav> <out.json>".into());
        };
        let bytes = std::fs::read(input).map_err(|e| format!("read {input}: {e}"))?;
        let ext = Path::new(input).extension().and_then(|e| e.to_str());
        let (info, buf) = soundcraft_audio_io::decode(&bytes, ext).map_err(|e| format!("decode {input}: {e}"))?;
        let x = buf.channels.into_iter().next().ok_or("no audio channels")?;
        let track = track_pitch(&x, info.sample_rate, &AtomicBool::new(false)).map_err(|e| e.to_string())?;
        let (classic, _) = segment_with(&track, NoteMode::Classic);
        let (musical, _) = segment_with(&track, NoteMode::Musical);
        let frames: Vec<Value> = track.frames.iter().map(|f| json!([f.cents, f.level_db, f.voiced_prob])).collect();
        let doc = json!({
            "sample_rate": track.sample_rate,
            "hop": track.hop_samples,
            "frames": frames,
            "classic": notes_json(&track, &classic, NoteMode::Classic),
            "musical": notes_json(&track, &musical, NoteMode::Musical),
        });
        std::fs::write(output, doc.to_string()).map_err(|e| format!("write {output}: {e}"))?;
        println!("{input}: {} frames, {} classic notes, {} musical notes", track.frames.len(), classic.len(), musical.len());
        Ok(())
    }
}
