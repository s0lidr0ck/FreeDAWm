//! Convert to MIDI end to end: known notes rendered as audio land as the right notes at the right
//! ticks across a tempo change, on a new instrument track under the source, as one undo step; a
//! cancelled job changes nothing; the session saves and reloads; the clip exports to a .mid that
//! reads back the same.
//!
//! Needs the transcription models (`SOUNDCRAFT_MODELS_DIR` or the default model folder); without
//! them the model parts print `SKIPPED`.

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::ml::{self, Cancel, Ctx, MlError};
use soundcraft_midi::{CtrlKind, Sequence};
use soundcraft_model::{ChannelFormat, Clip, ClipContent, Session, TrackKind};
use std::path::PathBuf;

const RATE: u32 = 48_000;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soundcraft-mlmidi-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn installed(id: &str) -> bool {
    let ok = soundcraft_ml::models::load(&ml::models_dir(), id).is_ok();
    if !ok {
        println!("SKIPPED the parts that need `{id}` (not in {})", ml::models_dir().display());
    }
    ok
}

/// A plucked harmonic tone: `pitch` from `at` s for `dur` s into `out`.
fn tone(out: &mut [f32], pitch: u8, at: f64, dur: f64) {
    let f = 440.0 * 2f64.powf((f64::from(pitch) - 69.0) / 12.0);
    let a = (at * f64::from(RATE)) as usize;
    let n = (dur * f64::from(RATE)) as usize;
    for i in 0..n {
        let t = i as f64 / f64::from(RATE);
        let env = (t / 0.005).min(1.0) * (-t * 2.0).exp() * ((dur - t) / 0.02).min(1.0);
        let s: f64 = (1..=4).map(|k| (2.0 * std::f64::consts::PI * f * k as f64 * t).sin() / k as f64).sum();
        if let Some(o) = out.get_mut(a + i) {
            *o += (0.25 * env * s) as f32;
        }
    }
}

/// The expected notes `(clip-relative seconds, pitch)` of the two clips.
const CLIP1: [(f64, u8); 4] = [(0.5, 60), (1.5, 64), (3.5, 67), (5.0, 72)];
const CLIP2: [(f64, u8); 1] = [(0.4, 62)];
const CLIP1_AT: f64 = 1.0;
const CLIP2_AT: f64 = 9.0;

/// A session at 48 kHz with 120 bpm, then 90 bpm from bar 3 (4.0 s), and a mono track "Keys"
/// with two clips of rendered notes (at 1 s and 9 s).
fn session() -> Engine {
    let mut s = Session { sample_rate: soundcraft_time::SampleRate::HZ_48000, ..Session::default() };
    s.tempo.set_tempo(0, 120.0).unwrap();
    s.tempo.set_tempo(2 * 4 * 960, 90.0).unwrap();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Keys"));
    for (at, notes, secs) in [(CLIP1_AT, &CLIP1[..], 6.5), (CLIP2_AT, &CLIP2[..], 1.5)] {
        let mut a = vec![0.0f32; (secs * f64::from(RATE)) as usize];
        for (t0, p) in notes {
            tone(&mut a, *p, *t0, 0.6);
        }
        let len = a.len() as i64;
        let src = soundcraft_engine::io::add_source(&mut s, "Keys", AudioBuffer { sample_rate: RATE, channels: vec![a] }, None, FileFormat::Wav);
        let id = s.new_clip_id();
        soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "Keys", src, 0, (at * f64::from(RATE)) as i64, len));
    }
    Engine::new(s)
}

/// Seconds → ticks on the test's tempo map, worked out by hand: 120 bpm (1920 ticks/s) to 4 s,
/// then 90 bpm (1440 ticks/s).
fn expected_tick(abs_s: f64) -> f64 {
    if abs_s <= 4.0 { abs_s * 1920.0 } else { 7680.0 + (abs_s - 4.0) * 1440.0 }
}

fn names(e: &Engine) -> Vec<String> {
    e.session().tracks.iter().map(|t| t.name.clone()).collect()
}

fn midi_clips(e: &Engine, track: &str) -> Vec<(Clip, Sequence)> {
    e.session()
        .track_by_name(track)
        .unwrap()
        .clips()
        .iter()
        .map(|c| match &c.content {
            ClipContent::Midi { sequence } => (c.clone(), sequence.clone()),
            _ => panic!("not a MIDI clip"),
        })
        .collect()
}

#[test]
fn known_notes_land_at_the_right_ticks_across_a_tempo_change() {
    if !installed("basicpitch") {
        return;
    }
    let mut e = session();
    let r = e.execute("ml.midi", &json!({"track": "Keys", "model": "general", "device": "cpu"})).unwrap();
    println!("ml.midi: {r}");
    assert_eq!(names(&e), ["Keys", "Keys MIDI"]);
    let t = e.session().track_by_name("Keys MIDI").unwrap();
    assert_eq!(t.kind, TrackKind::Instrument);
    assert_eq!(t.instrument.as_ref().map(|i| i.plugin.as_str()), Some("subtractive_synth"));
    let audio: Vec<Clip> = e.session().tracks[0].clips().to_vec();
    let clips = midi_clips(&e, "Keys MIDI");
    assert_eq!(clips.len(), 2, "one MIDI clip per source clip");
    let s = e.session();
    for ((c, seq), (src, (at, want))) in clips.iter().zip(audio.iter().zip([(CLIP1_AT, &CLIP1[..]), (CLIP2_AT, &CLIP2[..])])) {
        assert_eq!((c.start, c.length), (src.start, src.length), "same place, same length");
        let base = s.tempo.samples_to_ticks(c.start, s.sample_rate);
        println!("clip at {at} s: {:?}", seq.notes.iter().map(|n| (n.pitch, n.start, n.length, n.velocity)).collect::<Vec<_>>());
        for (t0, p) in want {
            let tick = expected_tick(at + t0) - base as f64;
            let n = seq.notes.iter().find(|n| n.pitch == *p && (n.start as f64 - tick).abs() < 0.04 * 1920.0);
            let n = n.unwrap_or_else(|| panic!("pitch {p} at {t0} s (tick {tick:.0}) not found"));
            // Its length follows the tempo too: about 0.6 s.
            let end = expected_tick(at + t0 + 0.6) - base as f64;
            assert!((n.end() as f64 - end).abs() < 0.15 * 1920.0, "pitch {p}: ends at {} not {end:.0}", n.end());
            assert!((1..=127).contains(&n.velocity));
            assert_eq!(n.channel, 0);
        }
        // Nothing far from the played notes (Basic Pitch may add an octave or a fifth over them).
        for n in &seq.notes {
            assert!(want.iter().any(|(_, p)| [0i32, 7, 12, 19, 24].contains(&(i32::from(n.pitch) - i32::from(*p)))), "stray note {n:?}");
        }
        assert!(seq.ctrls.is_empty(), "no bends by default");
    }
    assert_eq!(e.undo_label(), Some("Convert to MIDI"));
    assert!(e.undo());
    assert_eq!(names(&e), ["Keys"], "one undo step");
    assert!(e.redo());
    assert_eq!(names(&e), ["Keys", "Keys MIDI"]);

    // Save and reload: the MIDI clips come back as they were.
    let before = midi_clips(&e, "Keys MIDI");
    let out = scratch("save");
    let file = out.join("midi.scraft");
    soundcraft_engine::io::save_session(&mut e, &file.to_string_lossy()).unwrap();
    let mut again = Engine::new(Session::default());
    soundcraft_engine::io::open_session(&mut again, &file.to_string_lossy()).unwrap();
    assert_eq!(names(&again), ["Keys", "Keys MIDI"]);
    assert_eq!(midi_clips(&again, "Keys MIDI"), before);
    assert_eq!(again.session().tempo, e.session().tempo);

    // Export the MIDI track as a .mid and read it back: the same notes at the same ticks, and the
    // tempo change.
    let mid = out.join("keys.mid");
    e.execute("file.export_midi", &json!({"path": mid.to_string_lossy(), "tracks": ["Keys MIDI"]})).unwrap();
    let smf = soundcraft_midi::read_smf(&std::fs::read(&mid).unwrap()).unwrap();
    let tr = smf.tracks.iter().find(|t| !t.sequence.notes.is_empty()).unwrap();
    let s = e.session();
    let mut want: Vec<(u8, i64, i64, u8)> = Vec::new();
    for (c, seq) in &before {
        let base = s.tempo.samples_to_ticks(c.start, s.sample_rate);
        want.extend(seq.notes.iter().map(|n| (n.pitch, n.start + base, n.length, n.velocity)));
    }
    want.sort_by_key(|n| (n.1, n.0));
    let mut got: Vec<(u8, i64, i64, u8)> = tr.sequence.notes.iter().map(|n| (n.pitch, n.start, n.length, n.velocity)).collect();
    got.sort_by_key(|n| (n.1, n.0));
    assert_eq!(got, want, ".mid round trip");
    assert!(smf.tempos.iter().any(|(t, bpm)| *t == 7680 && (*bpm - 90.0).abs() < 1e-3), "{:?}", smf.tempos);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn pitch_bends_when_asked() {
    if !installed("basicpitch") {
        return;
    }
    let mut e = session();
    e.execute("ml.midi", &json!({"track": "Keys", "pitch_bends": true, "device": "cpu"})).unwrap();
    let clips = midi_clips(&e, "Keys MIDI");
    let bends: Vec<_> = clips.iter().flat_map(|(_, s)| s.ctrls.iter().filter(|c| c.kind == CtrlKind::PitchBend)).collect();
    println!("{} bend events", bends.len());
    assert!(!bends.is_empty());
    assert!(bends.iter().all(|b| (-8192..=8191).contains(&b.value)));
}

#[test]
fn cancel_changes_nothing() {
    if !installed("basicpitch") {
        return;
    }
    for at in [0.0f32, 0.5, 0.99] {
        let e = session();
        let before = (e.session().clone(), e.undo_history());
        let mut job = ml::prepare(&e, "ml.midi", &json!({"track": "Keys", "device": "cpu"})).unwrap();
        assert_eq!(job.parts.len(), 2);
        let cancel = Cancel::new();
        let c = cancel.clone();
        let r = job.run(&Ctx::new(cancel, move |f, _| {
            if f >= at {
                c.cancel();
            }
        }));
        assert!(matches!(r, Err(MlError::Cancelled)), "cancel at {at}: {r:?}");
        assert_eq!((e.session().clone(), e.undo_history()), before, "cancel at {at}");
    }
}

#[test]
fn the_ui_path_applies_one_undo_step_on_a_selected_clip() {
    if !installed("basicpitch") {
        return;
    }
    let mut e = session();
    let clip = e.session().tracks[0].clips()[1].id;
    let mut job = ml::prepare(&e, "ml.midi", &json!({"clip": clip.0, "device": "cpu"})).unwrap();
    assert_eq!(job.title(), "Convert to MIDI — Keys");
    let result = job.run(&Ctx::default()).unwrap();
    ml::apply(&mut e, &job, result).unwrap();
    assert_eq!(names(&e), ["Keys", "Keys MIDI"]);
    assert_eq!(midi_clips(&e, "Keys MIDI").len(), 1);
    assert_eq!(e.undo_label(), Some("Convert to MIDI"));
    assert!(e.undo());
    assert_eq!(names(&e), ["Keys"]);
}

/// A model's golden input (at its own rate) as the only clip of a track "Src" at 1 s.
fn golden_session(id: &str, rate: u32) -> Option<Engine> {
    golden_session_at(id, rate, None)
}

/// [`golden_session`] with the input brought to `level_db` (gated RMS) when given.
fn golden_session_at(id: &str, rate: u32, level_db: Option<f32>) -> Option<Engine> {
    let p = ml::models_dir().join(id).join("golden").join("input_5s.f32");
    let bytes = std::fs::read(&p).ok()?;
    let mut v: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    if let Some(db) = level_db {
        let g = 10f32.powf((db - soundcraft_ml::midi::gated_level_db(&v, rate)?) / 20.0);
        v.iter_mut().for_each(|x| *x *= g);
    }
    let a = soundcraft_dsp::offline::resample(&[v], rate, RATE);
    let mut s = Session { sample_rate: soundcraft_time::SampleRate::HZ_48000, ..Session::default() };
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Src"));
    let len = a[0].len() as i64;
    let src = soundcraft_engine::io::add_source(&mut s, "Src", AudioBuffer { sample_rate: RATE, channels: a }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "Src", src, 0, i64::from(RATE), len));
    Some(Engine::new(s))
}

#[test]
fn piano_drums_and_vocal_models_in_the_session() {
    if installed("piano_kong") {
        let mut e = golden_session("piano_kong", 16_000).unwrap();
        // HTDemucs' other, as when these bounds were set: the default (BS-RoFormer SW's piano stem,
        // when installed) keeps 46 of this solo excerpt's notes.
        let r = e.execute("ml.midi", &json!({"track": "Src", "model": "piano", "isolate": "other", "device": "cpu"})).unwrap();
        println!("piano: {r}");
        let t = e.session().track_by_name("Src MIDI").unwrap();
        let ins = t.instrument.as_ref().unwrap();
        assert_eq!((ins.plugin.as_str(), ins.preset.as_str()), ("subtractive_synth", "Keys"));
        let (_, seq) = &midi_clips(&e, "Src MIDI")[0];
        // The golden excerpt has 62 notes and sustain pedal (resampled here, and brought to the
        // transcription level, so about the same).
        assert!((50..=85).contains(&seq.notes.len()), "{}", seq.notes.len());
        let cc64: Vec<_> = seq.ctrls.iter().filter(|c| c.kind == CtrlKind::Cc(64)).collect();
        assert!(!cc64.is_empty() && cc64.len() % 2 == 0);
        assert!(cc64.iter().all(|c| c.value == 127 || c.value == 0));
        // Pedal off: no CC64.
        assert!(e.undo());
        e.execute("ml.midi", &json!({"track": "Src", "model": "piano", "isolate": "other", "pedal": false, "device": "cpu"})).unwrap();
        assert!(midi_clips(&e, "Src MIDI")[0].1.ctrls.is_empty());
    }
    if installed("adtof_drums") {
        let mut e = golden_session("adtof_drums", 44_100).unwrap();
        e.execute("ml.midi", &json!({"track": "Src", "model": "drums", "device": "cpu"})).unwrap();
        let t = e.session().track_by_name("Src MIDI").unwrap();
        assert_eq!(t.instrument.as_ref().map(|i| i.plugin.as_str()), Some("drum_synth"));
        let (_, seq) = &midi_clips(&e, "Src MIDI")[0];
        println!("drums: {} hits", seq.notes.len());
        assert!((18..=32).contains(&seq.notes.len()), "golden has 25: {}", seq.notes.len());
        assert!(seq.notes.iter().all(|n| n.channel == 9 && [35, 38, 42, 47, 49].contains(&n.pitch) && (1..=127).contains(&n.velocity)));
        // Velocities follow each hit's level in the source: never one constant.
        assert!(seq.notes.iter().any(|n| n.velocity != seq.notes[0].velocity), "{:?}", seq.notes.iter().map(|n| n.velocity).collect::<Vec<_>>());
        // A higher threshold hears fewer hits.
        assert!(e.undo());
        e.execute("ml.midi", &json!({"track": "Src", "model": "drums", "onset_threshold": 0.6, "device": "cpu"})).unwrap();
        assert!(midi_clips(&e, "Src MIDI")[0].1.notes.len() < seq.notes.len());
        // The same drums at -29 dB and at -9 dB: the same hits (the input is normalised first),
        // louder velocities nowhere (they are relative to the take's loudest hit).
        let hits = |db: f32| {
            let mut e = golden_session_at("adtof_drums", 44_100, Some(db)).unwrap();
            e.execute("ml.midi", &json!({"track": "Src", "model": "drums", "isolate": "none", "device": "cpu"})).unwrap();
            midi_clips(&e, "Src MIDI")[0].1.notes.iter().map(|n| (n.pitch, n.velocity)).collect::<Vec<_>>()
        };
        let (quiet, loud) = (hits(-29.0), hits(-9.0));
        assert_eq!(quiet.len(), loud.len());
        assert_eq!(quiet, loud);
    }
    if installed("game_vocal") {
        let mut e = golden_session("game_vocal", 44_100).unwrap();
        e.execute("ml.midi", &json!({"track": "Src", "model": "vocal", "pitch_bends": true, "device": "cpu"})).unwrap();
        let (_, seq) = &midi_clips(&e, "Src MIDI")[0];
        println!("vocal: {:?}", seq.notes.iter().map(|n| n.pitch).collect::<Vec<_>>());
        assert!((5..=11).contains(&seq.notes.len()), "golden has 8: {}", seq.notes.len());
        // One note at a time.
        assert!(seq.notes.windows(2).all(|w| w[1].start >= w[0].end()));
        assert!(seq.ctrls.iter().any(|c| c.kind == CtrlKind::PitchBend));
    }
}

#[test]
fn bad_params_and_missing_models_are_refused() {
    let mut e = session();
    let none = scratch("none");
    let dir = none.to_string_lossy().into_owned();
    let err = e.execute("ml.midi", &json!({"track": "Keys", "models_dir": dir})).unwrap_err().to_string();
    assert!(err.contains("not installed"), "{err}");
    for p in [
        json!({"track": "Keys", "model": "kazoo"}),
        json!({"track": "Keys", "onset_threshold": 2.0}),
        json!({"track": "Keys", "min_note_ms": -1}),
        json!({"track": "Keys", "frame_threshold": "high"}),
    ] {
        assert!(e.execute("ml.midi", &p).is_err(), "{p}");
    }
    assert_eq!(names(&e), ["Keys"]);
    let _ = std::fs::remove_dir_all(&none);
}

#[test]
fn seconds_become_ticks_on_the_tempo_map() {
    let e = session();
    let s = e.session();
    let start = i64::from(RATE); // 1 s
    let t = soundcraft_ml::Transcription {
        notes: vec![
            soundcraft_ml::NoteEvent { start: 1.0, end: 2.0, pitch: 60, velocity: 0, bends: Vec::new() },
            soundcraft_ml::NoteEvent { start: 3.5, end: 9.0, pitch: 62, velocity: 200, bends: Vec::new() },
            soundcraft_ml::NoteEvent { start: 6.0, end: 6.5, pitch: 64, velocity: 90, bends: Vec::new() },
        ],
        pedals: vec![(0.5, 4.0)],
        bends: vec![(1.0, 300), (1.5, 0), (3.5, 900)],
        drums: false,
    };
    let len = i64::from(RATE) * 5; // the clip ends at 6 s
    let seq = ml::midi_sequence(s, start, len, &t);
    let base = 1920i64;
    // 2 s → 3840; 3 s → 5760; 4.5 s → 7680 + 720; the clip end 6 s → 7680 + 2880.
    assert_eq!(seq.notes.len(), 2, "the note at the clip's end is dropped");
    assert_eq!((seq.notes[0].start, seq.notes[0].length, seq.notes[0].velocity), (3840 - base, 1920, 1));
    assert_eq!((seq.notes[1].start, seq.notes[1].end(), seq.notes[1].velocity), (8400 - base, 10_560 - base, 127));
    let cc: Vec<(i64, i32)> = seq.ctrls.iter().filter(|c| c.kind == CtrlKind::Cc(64)).map(|c| (c.tick, c.value)).collect();
    // Pedal down at 1.5 s, up at 5 s.
    assert_eq!(cc, vec![(2880 - base, 127), (7680 + 1440 - base, 0)]);
    let pb: Vec<(i64, i32)> = seq.ctrls.iter().filter(|c| c.kind == CtrlKind::PitchBend).map(|c| (c.tick, c.value)).collect();
    // The wheel is bent from 4.5 s: centred again at the clip's end.
    assert_eq!(pb, vec![(3840 - base, 300), (4800 - base, 0), (8400 - base, 900), (7680 + 2880 - 1 - base, 0)]);
}

#[test]
fn overlapping_same_pitch_notes_are_trimmed_to_the_next_start() {
    let e = session();
    let s = e.session();
    let n = |start: f64, end: f64, pitch: u8| soundcraft_ml::NoteEvent { start, end, pitch, velocity: 90, bends: Vec::new() };
    // At 120 bpm (before the tempo change), 1 s = 1920 ticks; the clip starts at 0.
    let t = soundcraft_ml::Transcription { notes: vec![n(0.0, 2.0, 60), n(1.0, 1.5, 60), n(0.5, 3.0, 64), n(1.0, 1.25, 60)], ..Default::default() };
    let seq = ml::midi_sequence(s, 0, i64::from(RATE) * 4, &t);
    let c4: Vec<(i64, i64)> = seq.notes.iter().filter(|x| x.pitch == 60).map(|x| (x.start, x.length)).collect();
    // The first C ends where the second starts; the duplicate at 1 s merges into one (the longer).
    assert_eq!(c4, vec![(0, 1920), (1920, 960)]);
    // Other pitches are untouched.
    let e4: Vec<(i64, i64)> = seq.notes.iter().filter(|x| x.pitch == 64).map(|x| (x.start, x.length)).collect();
    assert_eq!(e4, vec![(960, 4800)]);
    // Sorted by start.
    assert!(seq.notes.windows(2).all(|w| w[0].start <= w[1].start));
}
