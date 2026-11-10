//! Generate MIDI from Text (`ml.text_midi`): bad params and a missing model are refused with
//! nothing changed; a result lands as one instrument track per General MIDI instrument (named
//! after it, a program change on each clip, drums on channel 10 with the Drum Synth) at the edit
//! cursor, as one undo step; with MIDI-LLM installed, a short real piece does the same.
//!
//! The last part needs `midi_llm` (`SOUNDCRAFT_MODELS_DIR` or the default model folder); without
//! it that part prints `SKIPPED`.

use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_engine::ml::{self, MlResult, textmidi};
use soundcraft_midi::CtrlKind;
use soundcraft_ml::{JobOutput, NoteEvent, Transcription};
use soundcraft_model::{ClipContent, Session, TrackKind};
use std::path::PathBuf;

fn session() -> Engine {
    let mut s = Session { sample_rate: soundcraft_time::SampleRate::HZ_48000, ..Session::default() };
    s.tempo.set_tempo(0, 120.0).unwrap();
    s.add_track(TrackKind::Audio, soundcraft_model::ChannelFormat::Stereo, Some("Vox"));
    Engine::new(s)
}

fn names(e: &Engine) -> Vec<String> {
    e.session().tracks.iter().map(|t| t.name.clone()).collect()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("freedawm-textmidi-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn note(start: f64, end: f64, pitch: u8) -> NoteEvent {
    NoteEvent { start, end, pitch, velocity: textmidi::VELOCITY, bends: Vec::new() }
}

#[test]
fn bad_params_and_a_missing_model_are_refused() {
    let mut e = session();
    let none = scratch("none");
    let dir = none.to_string_lossy().into_owned();
    let err = e.execute("ml.text_midi", &json!({"prompt": "jazz", "models_dir": dir})).unwrap_err().to_string();
    assert!(err.contains("not installed"), "{err}");
    for p in [
        json!({}),
        json!({"prompt": "   "}),
        json!({"prompt": 5}),
        json!({"prompt": "jazz", "seconds": 0}),
        json!({"prompt": "jazz", "seconds": 500}),
        json!({"prompt": "jazz", "temperature": "hot"}),
        json!({"prompt": "jazz", "top_p": 0}),
        json!({"prompt": "jazz", "seed": -3}),
        json!({"prompt": "jazz", "max_tokens": 100000}),
    ] {
        assert!(e.execute("ml.text_midi", &p).is_err(), "{p}");
    }
    assert_eq!(names(&e), ["Vox"]);
    assert!(e.undo_label().is_none());
    let _ = std::fs::remove_dir_all(&none);
}

#[test]
fn a_result_lands_as_one_track_per_instrument_in_one_undo_step() {
    let mut e = session();
    // Start at 2 s; a fixed seed is kept.
    let job =
        ml::prepare(&e, "ml.text_midi", &json!({"prompt": "piano, bass and drums", "seconds": 4, "seed": 42, "start": {"seconds": 2.0}})).unwrap();
    assert_eq!(job.title(), "Generate MIDI from Text — piano, bass and drums");
    let parts = vec![
        textmidi::Part { instrument: 0, midi: Transcription { notes: vec![note(0.0, 0.5, 60), note(0.5, 1.0, 64)], ..Transcription::default() } },
        textmidi::Part { instrument: 33, midi: Transcription { notes: vec![note(0.0, 2.0, 36)], ..Transcription::default() } },
        textmidi::Part {
            instrument: textmidi::DRUMS,
            midi: Transcription { notes: vec![note(0.0, 0.1, 36), note(0.5, 0.6, 38)], drums: true, ..Transcription::default() },
        },
    ];
    let out = JobOutput { generated: Some(parts), notes: vec!["CPU".into()], ..JobOutput::default() };
    let v = ml::apply(&mut e, &job, MlResult { parts: vec![out] }).unwrap();
    assert_eq!(v["seed"], "42");
    let channels: Vec<u64> = v["made"].as_array().unwrap().iter().map(|m| m["channel"].as_u64().unwrap()).collect();
    assert_eq!(channels, [1, 2, 10], "each instrument its own channel, drums on 10");
    assert_eq!(names(&e), ["Vox", "Acoustic Grand Piano", "Electric Bass (finger)", "Drums"]);
    assert_eq!(e.undo_label(), Some("Generate MIDI from Text"));
    let s = e.session();
    for (name, plugin, program, channel, notes) in [
        ("Acoustic Grand Piano", "subtractive_synth", Some(0), 0, 2),
        ("Electric Bass (finger)", "subtractive_synth", Some(33), 1, 1),
        ("Drums", "drum_synth", None, 9, 2),
    ] {
        let t = s.tracks.iter().find(|t| t.name == name).unwrap();
        assert_eq!(t.kind, TrackKind::Instrument);
        assert_eq!(t.instrument.as_ref().unwrap().plugin, plugin);
        let clips = t.clips();
        assert_eq!(clips.len(), 1);
        let c = &clips[0];
        assert_eq!((c.start, c.length), (96_000, 192_000), "{name}: at 2 s, 4 s long");
        let ClipContent::Midi { sequence: seq } = &c.content else { panic!("{name}: not MIDI") };
        assert_eq!(seq.notes.len(), notes);
        assert!(seq.notes.iter().all(|n| n.channel == channel), "{name}");
        let programs: Vec<i32> = seq.ctrls.iter().filter(|c| c.kind == CtrlKind::Program && c.channel == channel).map(|c| c.value).collect();
        assert_eq!(programs, program.map(|p| vec![p]).unwrap_or_default(), "{name}");
    }
    // The second piano note: 0.5 s at 120 bpm = 960 ticks after the clip start.
    let piano = s.tracks.iter().find(|t| t.name == "Acoustic Grand Piano").unwrap();
    let ClipContent::Midi { sequence: seq } = &piano.clips()[0].content else { unreachable!() };
    assert_eq!(seq.notes[1].start, 960);
    assert_eq!(piano.instrument.as_ref().unwrap().preset, "Keys");
    assert!(e.undo());
    assert_eq!(names(&e), ["Vox"]);

    // No notes: refused, nothing changed.
    let empty = JobOutput { generated: Some(Vec::new()), ..JobOutput::default() };
    assert!(ml::apply(&mut e, &job, MlResult { parts: vec![empty] }).is_err());
    assert_eq!(names(&e), ["Vox"]);
}

#[test]
fn without_a_seed_one_is_chosen_and_reported() {
    let e = session();
    let a = ml::prepare(&e, "ml.text_midi", &json!({"prompt": "jazz"})).unwrap();
    let soundcraft_ml::JobKind::TextMidi(o) = a.kind else { panic!() };
    assert_ne!(o.seed, 0);
    let b = ml::prepare(&e, "ml.text_midi", &json!({"prompt": "jazz", "seed": "123"})).unwrap();
    let soundcraft_ml::JobKind::TextMidi(o) = b.kind else { panic!() };
    assert_eq!(o.seed, 123);
}

#[test]
fn a_real_piece_with_midi_llm() {
    if soundcraft_ml::models::load(&ml::models_dir(), textmidi::ID).is_err() {
        println!("SKIPPED: `{}` is not in {}", textmidi::ID, ml::models_dir().display());
        return;
    }
    let mut e = session();
    let v = e.execute("ml.text_midi", &json!({"prompt": "A cheerful rock song with bright electric guitars and drums", "seconds": 6, "max_tokens": 120, "seed": 3, "device": "cpu"})).unwrap();
    let made = v["made"].as_array().unwrap();
    assert!(!made.is_empty(), "{v}");
    assert_eq!(e.undo_label(), Some("Generate MIDI from Text"));
    assert_eq!(names(&e).len(), 1 + made.len());
    println!("{}", serde_json::to_string(&made).unwrap());
}
