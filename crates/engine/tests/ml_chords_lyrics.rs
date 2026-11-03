//! Detect Chords and Transcribe Lyrics in a session: chords land on the Band's chord lane at the
//! right ticks across a tempo change, snapped to half bars with repeats merged ("replace" and
//! "fill"); lyrics land on the vocal track at the clip's position (source offset included); each
//! job is one undo step; a cancelled job changes nothing; the session saves and reloads; the
//! `.lrc`, text and MIDI exports hold the words.
//!
//! The parts that run the models use the installed `btc_chords` and `whisper_lyrics` (with their
//! golden inputs) and print SKIPPED without them.

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::cmd::chord_lane::{Grid, KeyChoice, LaneOptions, TimedChord, place_chords};
use soundcraft_engine::ml::{self, Cancel, Ctx, MlError};
use soundcraft_model::{ChannelFormat, Clip, Session, TrackKind};
use std::path::PathBuf;

const RATE: u32 = 48_000;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soundcraft-mlcl-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn golden(id: &str, file: &str) -> Option<Vec<f32>> {
    let dir = ml::models_dir();
    let ok = soundcraft_ml::models::load(&dir, id).is_ok();
    let path = dir.join(id).join("golden").join(file);
    if !ok || !path.is_file() {
        println!("SKIPPED the parts that need `{id}` (not in {})", dir.display());
        return None;
    }
    let bytes = std::fs::read(path).unwrap();
    Some(bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}

/// 120 bpm, then 90 bpm from bar 3 (tick 7680 = 4 s).
fn tempo_session() -> Session {
    let mut s = Session { sample_rate: soundcraft_time::SampleRate::HZ_48000, ..Session::default() };
    s.tempo.set_tempo(0, 120.0).unwrap();
    s.tempo.set_tempo(2 * 4 * 960, 90.0).unwrap();
    s
}

/// A track `name` with one clip of `audio` (at `rate`, resampled to the session's) placed at `at`
/// seconds, playing from `offset` seconds into its source.
fn add_clip_track(s: &mut Session, name: &str, audio: Vec<f32>, rate: u32, at: f64, offset: f64) -> soundcraft_model::TrackId {
    let audio = if rate == RATE { vec![audio] } else { soundcraft_dsp::offline::resample(&[audio], rate, RATE) };
    let total = audio[0].len() as i64;
    let off = (offset * f64::from(RATE)) as i64;
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
    let src = soundcraft_engine::io::add_source(s, name, AudioBuffer { sample_rate: RATE, channels: audio }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(s, t, Clip::audio(id, name, src, off, (at * f64::from(RATE)) as i64, total - off));
    t
}

/// The lane as `(tick, length, slot)`.
fn lane(s: &Session) -> Vec<(i64, i64, u8)> {
    s.band.chords.iter().map(|c| (c.tick, c.length_ticks, c.slot)).collect()
}

/// The golden BTC segments of `input_12s` (Em, C, D, then no chord), seconds.
fn golden_chords() -> Vec<TimedChord> {
    let unit = 10.0 / 108.0;
    [(0, 48, "Em"), (48, 85, "C"), (85, 127, "D")]
        .iter()
        .map(|&(a, b, n)| TimedChord { start: a as f64 * unit, end: b as f64 * unit, name: n.into() })
        .collect()
}

/// Where the golden chords land from a source at 1 s on the test's tempo map, worked out by hand:
/// Em 1-5.44 s = ticks 1920-9760, C to 8.87 s = 14693, D to 12.76 s = 20293; source end 13 s =
/// 20640. Half-bar cells (1920 ticks) take the chord heard longest: Em 1920-9600, C 9600-15360,
/// D 15360-21120 (the last cell is 1440 ticks inside the source, D covers 1093 of them).
const EXPECTED: [(i64, i64); 3] = [(1920, 9600 - 1920), (9600, 15360 - 9600), (15360, 21120 - 15360)];

#[test]
fn chords_snap_to_half_bars_across_a_tempo_change() {
    let mut s = tempo_session();
    let opts = LaneOptions { grid: Grid::HalfBar, replace: true, key: KeyChoice::Set(4, 1) };
    let v = place_chords(&mut s, i64::from(RATE), i64::from(RATE) * 12, &golden_chords(), &opts).unwrap();
    let got: Vec<(i64, i64)> = lane(&s).iter().map(|c| (c.0, c.1)).collect();
    assert_eq!(got, EXPECTED);
    assert!(v["chords"].as_array().unwrap().iter().all(|c| c["exact"] == true), "{v}");
    // E minor: Em is i, C is VI, D is VII.
    assert_eq!((s.band.key, s.band.scale), (4, 1));
    let names: Vec<&str> = v["chords"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Em", "C", "D"]);
    // C starts at 4 s + 1920/1440 s.
    assert_eq!(v["chords"][1]["at"], json!(RATE as i64 * 4 + RATE as i64 * 4 / 3));

    // Bars: two cells per chord boundary decision.
    let mut b = tempo_session();
    place_chords(&mut b, i64::from(RATE), i64::from(RATE) * 12, &golden_chords(), &LaneOptions { grid: Grid::Bar, ..opts }).unwrap();
    let ticks: Vec<i64> = lane(&b).iter().map(|c| c.0).collect();
    assert!(ticks.iter().all(|t| t % 3840 == 0), "{ticks:?}");

    // Again with "replace": the same lane, nothing doubled.
    place_chords(&mut s, i64::from(RATE), i64::from(RATE) * 12, &golden_chords(), &opts).unwrap();
    assert_eq!(lane(&s).iter().map(|c| (c.0, c.1)).collect::<Vec<_>>(), EXPECTED);

    // "Fill": a chord already at 9600-13440 stays; C fills only the rest of its span.
    let mut f = tempo_session();
    f.band.chords.push(soundcraft_model::ChordEvent { id: 1, tick: 9600, length_ticks: 3840, slot: 0, bass: -1 });
    f.band.next_id = 2;
    place_chords(&mut f, i64::from(RATE), i64::from(RATE) * 12, &golden_chords(), &LaneOptions { replace: false, key: KeyChoice::Keep, ..opts })
        .unwrap();
    let got: Vec<(i64, i64)> = lane(&f).iter().map(|c| (c.0, c.1)).collect();
    assert_eq!(got, [(1920, 7680), (9600, 3840), (13440, 1920), (15360, 5760)]);
    assert_eq!(f.band.key, 0, "keep leaves the band's key");
}

#[test]
fn auto_key_is_set_only_on_an_empty_lane() {
    let mut s = tempo_session();
    let opts = LaneOptions::default();
    let chords = vec![
        TimedChord { start: 0.0, end: 2.0, name: "Am".into() },
        TimedChord { start: 2.0, end: 4.0, name: "F".into() },
        TimedChord { start: 4.0, end: 6.0, name: "C".into() },
        TimedChord { start: 6.0, end: 8.0, name: "G".into() },
    ];
    let v = place_chords(&mut s, 0, i64::from(RATE) * 8, &chords, &opts).unwrap();
    assert_eq!(v["key_set"], true);
    let key = (s.band.key, s.band.scale);
    assert!(key == (0, 0) || key == (9, 1), "{key:?}");
    // A chord elsewhere on the lane: the key stays.
    s.band.key = 2;
    s.band.chords.push(soundcraft_model::ChordEvent { id: 99, tick: 960 * 4 * 40, length_ticks: 1920, slot: 0, bass: -1 });
    let v = place_chords(&mut s, 0, i64::from(RATE) * 8, &chords, &opts).unwrap();
    assert_eq!((v["key_set"].clone(), s.band.key), (json!(false), 2));
}

#[test]
fn detect_chords_in_a_session() {
    let Some(input) = golden("btc_chords", "input_12s.f32") else { return };
    let mut s = tempo_session();
    // A mono track named after an instrument: the bass guess defaults off.
    add_clip_track(&mut s, "Piano", input, 22_050, 1.0, 0.0);
    let mut e = Engine::new(s);
    let before = (e.session().clone(), e.undo_history());

    // Cancel at any point changes nothing.
    for at in [0.0f32, 0.5] {
        let mut job = ml::prepare(&e, "ml.chords", &json!({"track": "Piano", "device": "cpu"})).unwrap();
        assert!(matches!(job.kind, ml::JobKind::Chords(o) if !o.bass));
        let cancel = Cancel::new();
        let c = cancel.clone();
        let r = job.run(&Ctx::new(cancel, move |f, _| {
            if f >= at {
                c.cancel();
            }
        }));
        assert!(matches!(r, Err(MlError::Cancelled)), "{r:?}");
        assert_eq!((e.session().clone(), e.undo_history()), before);
    }

    let v = e.execute("ml.chords", &json!({"track": "Piano", "device": "cpu", "key": "E", "scale": "minor"})).unwrap();
    println!("{v}");
    assert_eq!(lane(e.session()).iter().map(|c| (c.0, c.1)).collect::<Vec<_>>(), EXPECTED);
    assert!(e.session().edit.rulers.iter().any(|r| r == "band_chords"), "the chord ruler shows");
    assert_eq!(e.undo_label(), Some("Detect Chords"));
    let after = lane(e.session());
    assert!(e.undo());
    assert!(e.session().band.chords.is_empty(), "one undo step");
    assert!(e.redo());
    assert_eq!(lane(e.session()), after);

    // Save and reload.
    let dir = scratch("chords");
    let file = dir.join("Chords.scraft");
    soundcraft_engine::io::save_session(&mut e, &file.to_string_lossy()).unwrap();
    let mut again = Engine::new(Session::default());
    soundcraft_engine::io::open_session(&mut again, &file.to_string_lossy()).unwrap();
    assert_eq!(lane(again.session()), after);
    assert_eq!((again.session().band.key, again.session().band.scale), (4, 1));
    let _ = std::fs::remove_dir_all(&dir);

    // Bad params are refused.
    for p in [json!({"track": "Piano", "grid": "beat"}), json!({"track": "Piano", "mode": "merge"}), json!({"track": "Piano", "key": "H"})] {
        assert!(e.execute("ml.chords", &p).is_err(), "{p}");
    }
}

/// Lines of words: `(seconds from the timeline start, text)`.
fn words(s: &Session, track: &str) -> Vec<(f64, String)> {
    let t = s.track_by_name(track).unwrap();
    soundcraft_model::track_lines(t).iter().flat_map(|l| l.words.iter()).map(|w| (w.start as f64 / f64::from(RATE), w.text.clone())).collect()
}

#[test]
fn lyrics_exports_without_a_model() {
    let mut s = tempo_session();
    // A 20 s clip at 1 s holds the words.
    let t = add_clip_track(&mut s, "Vox", vec![0.05; RATE as usize * 20], RATE, 1.0, 0.0);
    let w = |a: f64, b: f64, x: &str| soundcraft_engine::lyrics::TimedWord { start: a, end: b, text: x.into() };
    soundcraft_engine::lyrics::place_lyrics(
        &mut s,
        t,
        i64::from(RATE) * 5,
        i64::from(RATE) * 10,
        &[vec![w(0.0, 0.5, "Show"), w(0.5, 1.0, "me")], vec![w(2.0, 3.0, "Lord")]],
    );
    let mut e = Engine::new(s);
    let dir = scratch("export");
    let lrc = dir.join("Vox.lrc");
    let txt = dir.join("Vox.txt");
    let mid = dir.join("Vox.mid");
    e.execute("file.export_lyrics", &json!({"path": lrc.to_string_lossy()})).unwrap();
    e.execute("file.export_lyrics_text", &json!({"path": txt.to_string_lossy(), "track": "Vox"})).unwrap();
    assert_eq!(std::fs::read_to_string(&lrc).unwrap(), "[ti:Vox]\n[re:FreeDAWm]\n[00:05.00]<00:05.00>Show <00:05.50>me\n[00:07.00]<00:07.00>Lord\n");
    assert_eq!(std::fs::read_to_string(&txt).unwrap(), "Show me\nLord\n");
    // MIDI: a lyrics track with one event per word, at the words' ticks on the tempo map (5 s =
    // 7680 + 1440, 7 s = 7680 + 3 × 1440).
    e.execute("file.export_midi", &json!({"path": mid.to_string_lossy()})).unwrap();
    let smf = soundcraft_midi::read_smf(&std::fs::read(&mid).unwrap()).unwrap();
    assert_eq!(smf.tracks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Vox Lyrics"]);
    assert_eq!(smf.lyrics, vec![(0, 9120, "Show".to_string()), (0, 9120 + 720, "me".to_string()), (0, 7680 + 3 * 1440, "Lord".to_string())]);
    // Listing and clearing (undoable).
    let l = e.execute("lyrics.list", &json!({"track": "Vox"})).unwrap();
    assert_eq!(l["lines"][1]["text"], "Lord");
    e.execute("lyrics.clear", &json!({"track": "Vox"})).unwrap();
    assert!(e.execute("file.export_lyrics", &json!({"path": lrc.to_string_lossy()})).is_err());
    assert!(e.undo());
    assert_eq!(words(e.session(), "Vox").len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn transcribe_lyrics_in_a_session() {
    let Some(input) = golden("whisper_lyrics", "input_10s.f32") else { return };
    // 2 s of silence before the vocal; the clip plays from 2 s into its source and sits at 3 s,
    // so the vocal starts at 3 s on the timeline.
    let mut audio = vec![0.0f32; 2 * 16_000];
    audio.extend(input);
    let mut s = tempo_session();
    add_clip_track(&mut s, "Vox", audio, 16_000, 3.0, 2.0);
    let mut e = Engine::new(s);
    let before = (e.session().clone(), e.undo_history());

    // Cancelled as soon as it starts: nothing changes.
    let clip = e.session().tracks[0].clips()[0].id;
    let mut job = ml::prepare(&e, "ml.lyrics", &json!({"clip": clip.0})).unwrap();
    let cancel = Cancel::new();
    let c = cancel.clone();
    let r = job.run(&Ctx::new(cancel, move |_, what| {
        if what.starts_with("Transcribing") {
            c.cancel();
        }
    }));
    assert!(matches!(r, Err(MlError::Cancelled)), "{r:?}");
    assert_eq!((e.session().clone(), e.undo_history()), before);

    // The UI path on the clip: one undo step.
    // Best (f16) without skipping: the golden transcript's settings.
    let mut job = ml::prepare(&e, "ml.lyrics", &json!({"clip": clip.0, "language": "en", "quality": "best", "skip_silence": false})).unwrap();
    assert_eq!(job.title(), "Transcribe Lyrics — Vox");
    let result = job.run(&Ctx::default()).unwrap();
    let v = ml::apply(&mut e, &job, result).unwrap();
    println!("{v}");
    let got = words(e.session(), "Vox");
    let text: Vec<&str> = got.iter().map(|w| w.1.as_str()).collect();
    println!("{got:?}");
    assert!(text.join(" ").contains("called me to boldly"), "{text:?}");
    // The golden times (seconds of the vocal) + 3 s, within 0.1 s.
    let at = |word: &str| got.iter().find(|w| w.1 == word).map(|w| w.0).unwrap_or(f64::NAN);
    for (word, secs) in [("And", 5.24), ("called", 6.5), ("boldly", 8.28)] {
        assert!((at(word) - (3.0 + secs)).abs() < 0.1, "{word}: {} vs {}", at(word), 3.0 + secs);
    }
    assert_eq!(soundcraft_model::track_lines(&e.session().tracks[0]).len(), 2, "two lines");
    assert_eq!(e.undo_label(), Some("Transcribe Lyrics"));
    assert!(e.undo());
    assert!(!soundcraft_model::track_has_lyrics(&e.session().tracks[0]));
    assert!(e.redo());

    // Save and reload.
    let dir = scratch("lyrics");
    let file = dir.join("Lyrics.scraft");
    soundcraft_engine::io::save_session(&mut e, &file.to_string_lossy()).unwrap();
    let mut again = Engine::new(Session::default());
    soundcraft_engine::io::open_session(&mut again, &file.to_string_lossy()).unwrap();
    assert_eq!(soundcraft_model::track_lines(&again.session().tracks[0]), soundcraft_model::track_lines(&e.session().tracks[0]));
    let lrc = dir.join("Vox.lrc");
    again.execute("file.export_lyrics", &json!({"path": lrc.to_string_lossy()})).unwrap();
    let body = std::fs::read_to_string(&lrc).unwrap();
    assert!(body.contains("[00:08.2") && body.contains("And"), "{body}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Absolute pitch classes and bass of each chord on the lane (what the band plays).
fn sounding(s: &Session) -> Vec<(i64, [bool; 12], i32)> {
    let (k, sc) = (i32::from(s.band.key), i32::from(s.band.scale));
    s.band
        .chords
        .iter()
        .map(|c| {
            let pcs = soundcraft_band::chord_match::slot_pitch_classes(i32::from(c.slot), k, sc);
            let root = soundcraft_band::chord_theory::shape_for(i32::from(c.slot), k, sc, -1).root_pc;
            (c.tick, pcs, if c.bass >= 0 { (i32::from(c.bass) + k).rem_euclid(12) } else { root })
        })
        .collect()
}

#[test]
fn a_new_key_keeps_the_chords_already_on_the_lane() {
    // Verse chords entered in C major (bars 1-4); Detect Chords then writes the chorus (bar 9 on) in G.
    let mut e = Engine::new(tempo_session());
    for (bar, name) in [(0, "C"), (1, "F"), (2, "G/B"), (3, "Am")] {
        let at = e.session().tempo.tick_to_samples(bar * 3840, e.session().sample_rate);
        e.execute("band.chord.add", &json!({"at": at, "chord": name})).unwrap();
    }
    let verse = sounding(e.session());
    let mut s = e.session().clone();
    let chorus = vec![TimedChord { start: 0.0, end: 2.0, name: "G".into() }, TimedChord { start: 2.0, end: 4.0, name: "D".into() }];
    let at = s.tempo.tick_to_samples(8 * 3840, s.sample_rate);
    place_chords(&mut s, at, i64::from(RATE) * 4, &chorus, &LaneOptions { key: KeyChoice::Set(7, 0), ..LaneOptions::default() }).unwrap();
    assert_eq!((s.band.key, s.band.scale), (7, 0));
    let after = sounding(&s);
    assert_eq!(after[..4], verse[..], "the verse sounds the same in the new key");
    assert_eq!(after.len(), 4 + 2);

    // A chord the new key can't hold exactly refuses the change and leaves the lane alone.
    let mut refused = false;
    'search: for name in ["C#dim7", "Bbaug", "F#m7b5", "Ebsus2", "Abm6", "Dbmaj7", "Gbm", "E7"] {
        for key in 0..12u8 {
            for scale in 0..2u8 {
                let mut t = Engine::new(tempo_session());
                t.execute("band.chord.add", &json!({"at": 0, "chord": name})).unwrap();
                let mut s = t.session().clone();
                let before = s.band.clone();
                if let Err(m) =
                    place_chords(&mut s, at, i64::from(RATE) * 4, &chorus, &LaneOptions { key: KeyChoice::Set(key, scale), ..LaneOptions::default() })
                {
                    assert!(m.contains("can't be written"), "{m}");
                    assert_eq!(s.band, before);
                    refused = true;
                    break 'search;
                }
            }
        }
    }
    println!("refusal case found: {refused}");
}
