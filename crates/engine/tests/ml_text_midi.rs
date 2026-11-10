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

/// The session plus a MIDI clip "Riff" on an instrument track "Keys": C4 E4 G4, a beat each, at 4 s.
fn with_riff(program: Option<i32>, channel: u8) -> (Engine, u64) {
    let mut e = session();
    let s = e.session_mut();
    let t = s.add_track(TrackKind::Instrument, soundcraft_model::ChannelFormat::Stereo, Some("Keys"));
    let mut seq = soundcraft_midi::Sequence::default();
    for (i, pitch) in [60u8, 64, 67].into_iter().enumerate() {
        seq.notes.push(soundcraft_midi::Note { pitch, velocity: 90, release_velocity: 64, channel, start: i as i64 * 960, length: 960 });
    }
    if let Some(p) = program {
        seq.ctrls.push(soundcraft_midi::CtrlEvent { tick: 0, channel, kind: CtrlKind::Program, value: p });
    }
    let cid = s.new_clip_id();
    soundcraft_engine::edit::place_clip(s, t, soundcraft_model::Clip::midi(cid, "Riff", 192_000, 3 * 24_000, seq));
    (e, cid.0)
}

#[test]
fn a_midi_clip_can_start_the_piece() {
    let (e, clip) = with_riff(Some(24), 0);
    let job = ml::prepare(&e, "ml.text_midi", &json!({"prompt": "folk", "continue_from": clip, "seed": 1})).unwrap();
    let soundcraft_ml::JobKind::TextMidi(o) = job.kind else { panic!() };
    // Its three notes, half a second apart at 120 bpm, on its program (nylon guitar).
    assert_eq!(o.prime.notes(), 3);
    let parts = textmidi::decode(o.prime.tokens(), 100.0);
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].instrument, 24);
    let starts: Vec<f64> = parts[0].midi.notes.iter().map(|n| n.start).collect();
    assert_eq!(starts, [0.0, 0.5, 1.0]);
    // The new tracks start where the clip does.
    let soundcraft_engine::ml::Target::Generate { start, .. } = &job.parts[0].target else { panic!() };
    assert_eq!(*start, 192_000);
    // Drums: by channel 10, or asked for; a bad instrument or clip is refused.
    let (e, clip) = with_riff(None, 9);
    let job = ml::prepare(&e, "ml.text_midi", &json!({"prompt": "rock", "continue_from": clip})).unwrap();
    let soundcraft_ml::JobKind::TextMidi(o) = job.kind else { panic!() };
    assert_eq!(textmidi::decode(o.prime.tokens(), 100.0)[0].instrument, textmidi::DRUMS);
    let (e, clip) = with_riff(None, 0);
    let job = ml::prepare(&e, "ml.text_midi", &json!({"prompt": "rock", "continue_from": clip, "instrument": 33})).unwrap();
    let soundcraft_ml::JobKind::TextMidi(o) = job.kind else { panic!() };
    assert_eq!(textmidi::decode(o.prime.tokens(), 100.0)[0].instrument, 33);
    for p in [
        json!({"prompt": "x", "continue_from": clip, "instrument": 200}),
        json!({"prompt": "x", "continue_from": 9999}),
        json!({"prompt": "x", "continue_from": "a"}),
    ] {
        assert!(ml::prepare(&e, "ml.text_midi", &p).is_err(), "{p}");
    }
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
    // Continuing a riff: the new tracks start at the clip, and the riff itself is not copied.
    let (mut e, clip) = with_riff(Some(0), 0);
    let v = e
        .execute(
            "ml.text_midi",
            &json!({"prompt": "A gentle piano piece in C major", "continue_from": clip, "seconds": 6, "max_tokens": 60, "seed": 3, "device": "cpu"}),
        )
        .unwrap();
    for m in v["made"].as_array().unwrap() {
        let c = e.session().find_clip(soundcraft_model::ClipId(m["clip"].as_u64().unwrap())).unwrap().1.clone();
        assert_eq!(c.start, 192_000);
    }
    println!("continued: {}", serde_json::to_string(&v["made"]).unwrap());
}
