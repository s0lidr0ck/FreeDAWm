//! Lyrics live on clips (in the clip's source time) and follow every edit of the audio: move,
//! nudge, trim (hidden, not lost), separate, delete (and undo), insert silence, duplicate; old
//! sessions with track-level lyrics migrate; .mid export and import keep them.

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::lyrics::{TimedWord, place_lyrics};
use soundcraft_model::{ChannelFormat, Clip, Session, TrackKind, track_lines};

const RATE: i64 = 48_000;

/// "Vox": one clip at 2 s, 10 s long, playing from 1 s into its source; words "a" "b" "c" at
/// 3 s, 5 s and 8 s on the timeline, one line each.
fn engine() -> Engine {
    let mut s = Session { sample_rate: soundcraft_time::SampleRate::HZ_48000, ..Session::default() };
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let audio: Vec<f32> = (0..RATE * 12).map(|i| 0.1 * ((i as f32) * 0.01).sin()).collect();
    let src = soundcraft_engine::io::add_source(&mut s, "Vox", AudioBuffer { sample_rate: 48_000, channels: vec![audio] }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "Vox", src, RATE, 2 * RATE, 10 * RATE));
    let w = |at: f64, x: &str| vec![TimedWord { start: at, end: at + 0.5, text: x.into() }];
    // Seconds from the clip's start (2 s).
    place_lyrics(&mut s, t, 2 * RATE, 10 * RATE, &[w(1.0, "a"), w(3.0, "b"), w(6.0, "c")]);
    Engine::new(s)
}

/// Visible words of `track` as (seconds on the timeline, text).
fn words(e: &Engine, track: &str) -> Vec<(f64, String)> {
    let t = e.session().track_by_name(track).unwrap();
    track_lines(t).iter().flat_map(|l| l.words.iter()).map(|w| (w.start as f64 / RATE as f64, w.text.clone())).collect()
}

fn at(v: &[(f64, &str)]) -> Vec<(f64, String)> {
    v.iter().map(|(a, b)| (*a, b.to_string())).collect()
}

fn clip_id(e: &Engine) -> u64 {
    e.session().track_by_name("Vox").unwrap().clips()[0].id.0
}

#[test]
fn lyrics_are_stored_in_clip_time() {
    let e = engine();
    let c = &e.session().track_by_name("Vox").unwrap().clips()[0];
    // "a" at 3 s on the timeline = 1 s into the clip = 2 s into the source.
    assert_eq!(c.lyrics[0].words[0].start, 2 * RATE);
    assert_eq!(words(&e, "Vox"), at(&[(3.0, "a"), (5.0, "b"), (8.0, "c")]));
}

#[test]
fn move_and_nudge_carry_the_words() {
    let mut e = engine();
    let id = clip_id(&e);
    e.execute("edit.move_clips", &json!({"clips": [id], "by": RATE})).unwrap();
    assert_eq!(words(&e, "Vox"), at(&[(4.0, "a"), (6.0, "b"), (9.0, "c")]));
    assert!(e.undo());
    assert_eq!(words(&e, "Vox"), at(&[(3.0, "a"), (5.0, "b"), (8.0, "c")]));
    e.execute("edit.select", &json!({"clips": [id]})).unwrap();
    let before = e.session().track_by_name("Vox").unwrap().clips()[0].start;
    e.execute("edit.nudge", &json!({"direction": 1})).unwrap();
    let d = (e.session().track_by_name("Vox").unwrap().clips()[0].start - before) as f64 / RATE as f64;
    assert!(d > 0.0);
    assert_eq!(words(&e, "Vox"), at(&[(3.0 + d, "a"), (5.0 + d, "b"), (8.0 + d, "c")]));
}

#[test]
fn trimming_hides_words_and_untrimming_brings_them_back() {
    let mut e = engine();
    let id = clip_id(&e);
    e.execute("edit.trim_clip", &json!({"clip": id, "start": 4 * RATE})).unwrap();
    assert_eq!(words(&e, "Vox"), at(&[(5.0, "b"), (8.0, "c")]));
    e.execute("edit.trim_clip", &json!({"clip": id, "end": 7 * RATE})).unwrap();
    assert_eq!(words(&e, "Vox"), at(&[(5.0, "b")]));
    e.execute("edit.trim_clip", &json!({"clip": id, "start": 2 * RATE, "end": 12 * RATE})).unwrap();
    assert_eq!(words(&e, "Vox"), at(&[(3.0, "a"), (5.0, "b"), (8.0, "c")]), "hidden, not lost");
}

#[test]
fn separating_splits_the_words_between_the_halves_once() {
    let mut e = engine();
    e.execute("edit.separate", &json!({"tracks": ["Vox"], "at": 6 * RATE})).unwrap();
    let t = e.session().track_by_name("Vox").unwrap();
    assert_eq!(t.clips().len(), 2);
    assert_eq!(words(&e, "Vox"), at(&[(3.0, "a"), (5.0, "b"), (8.0, "c")]), "each word once");
    let lrc = soundcraft_engine::lyrics::lrc(e.session(), t.id);
    assert_eq!(lrc.lines().filter(|l| l.starts_with("[00:")).count(), 3, "{lrc}");
    // Moving the second half moves only its word.
    let second = t.clips()[1].id.0;
    e.execute("edit.move_clips", &json!({"clips": [second], "by": RATE})).unwrap();
    assert_eq!(words(&e, "Vox"), at(&[(3.0, "a"), (5.0, "b"), (9.0, "c")]));
}

#[test]
fn deleting_the_clip_takes_its_words_and_undo_brings_them_back() {
    let mut e = engine();
    e.execute("edit.clear", &json!({"tracks": ["Vox"], "start": 0, "end": 20 * RATE})).unwrap();
    assert!(words(&e, "Vox").is_empty());
    assert!(e.execute("file.export_lyrics", &json!({"path": std::env::temp_dir().join("sc-none.lrc").to_string_lossy()})).is_err());
    assert!(e.undo());
    assert_eq!(words(&e, "Vox"), at(&[(3.0, "a"), (5.0, "b"), (8.0, "c")]));
}

#[test]
fn inserting_time_shifts_the_words_with_the_clip() {
    // Shuffle mode: inserting time pushes the clip later.
    let mut e = engine();
    let mut s = e.session().clone();
    s.edit.edit_mode = soundcraft_model::EditMode::Shuffle;
    let mut e2 = Engine::new(s);
    std::mem::swap(&mut e, &mut e2);
    e.execute("edit.insert_silence", &json!({"tracks": ["Vox"], "start": RATE, "end": 3 * RATE})).unwrap();
    assert_eq!(words(&e, "Vox"), at(&[(5.0, "a"), (7.0, "b"), (10.0, "c")]));
}

#[test]
fn duplicates_copy_the_words_once() {
    let mut e = engine();
    e.execute("track.duplicate", &json!({"tracks": ["Vox"]})).unwrap();
    let names: Vec<String> = e.session().tracks.iter().map(|t| t.name.clone()).collect();
    assert_eq!(names.len(), 2);
    for n in &names {
        assert_eq!(words(&e, n).len(), 3, "{n}");
    }
    // MIDI export: one lyrics track per audio track, each word once.
    let dir = std::env::temp_dir().join(format!("sc-lyrdup-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let mid = dir.join("d.mid");
    e.execute("file.export_midi", &json!({"path": mid.to_string_lossy()})).unwrap();
    let smf = soundcraft_midi::read_smf(&std::fs::read(&mid).unwrap()).unwrap();
    assert_eq!(smf.tracks.len(), 2);
    for i in 0..2 {
        assert_eq!(smf.lyrics.iter().filter(|l| l.0 == i).count(), 3);
    }
    let _ = std::fs::remove_dir_all(&dir);

    // Edit › Duplicate of the clip: its copy plays later, with its words once.
    let mut e = engine();
    e.execute("edit.duplicate", &json!({"tracks": ["Vox"], "start": 2 * RATE, "end": 12 * RATE})).unwrap();
    assert_eq!(words(&e, "Vox"), at(&[(3.0, "a"), (5.0, "b"), (8.0, "c"), (13.0, "a"), (15.0, "b"), (18.0, "c")]));
}

#[test]
fn saved_and_reloaded_and_old_track_lyrics_migrate() {
    let mut e = engine();
    let dir = std::env::temp_dir().join(format!("sc-lyrsave-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("L.scraft");
    soundcraft_engine::io::save_session(&mut e, &file.to_string_lossy()).unwrap();
    let mut again = Engine::new(Session::default());
    soundcraft_engine::io::open_session(&mut again, &file.to_string_lossy()).unwrap();
    assert_eq!(words(&again, "Vox"), words(&e, "Vox"));

    // The first build kept lyrics on the track in absolute samples: they move onto the clip.
    let mut v: serde_json::Value = serde_json::from_str(&e.session().to_json().unwrap()).unwrap();
    let clip = &mut v["tracks"][0]["playlists"][0]["clips"][0];
    clip.as_object_mut().unwrap().remove("lyrics");
    v["tracks"][0]["lyrics"] = json!([{"start": 5 * RATE, "end": 5 * RATE + 100, "words": [{"start": 5 * RATE, "end": 5 * RATE + 100, "text": "old"}]},
                                      {"start": 30 * RATE, "end": 30 * RATE + 1, "words": [{"start": 30 * RATE, "end": 30 * RATE + 1, "text": "nowhere"}]}]);
    let s = Session::from_json(&v.to_string()).unwrap();
    let t = &s.tracks[0];
    assert!(t.lyrics.is_empty());
    assert_eq!(t.clips()[0].lyrics.len(), 1, "the line over no clip is dropped");
    let shown: Vec<String> = track_lines(t).iter().flat_map(|l| l.words.iter().map(|w| format!("{}@{}", w.text, w.start))).collect();
    assert_eq!(shown, [format!("old@{}", 5 * RATE)]);
    assert!(!s.to_json().unwrap().contains("\"lyrics\": [\n        {\n          \"start\": 240000"), "track lyrics are never written");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn midi_export_and_import_keep_the_lyrics() {
    let e = engine();
    let dir = std::env::temp_dir().join(format!("sc-lyrmid-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let mid = dir.join("v.mid");
    let bytes = soundcraft_engine::io::export_midi(&e, None).unwrap();
    std::fs::write(&mid, &bytes).unwrap();
    // Into a fresh session at 120 bpm: a lyrics-only track still comes in, with its words.
    let mut f = Engine::new(Session { sample_rate: soundcraft_time::SampleRate::HZ_48000, ..Session::default() });
    let ids = soundcraft_engine::io::import_midi_bytes(&mut f, &bytes, 0, true).unwrap();
    assert_eq!(ids.len(), 1);
    let t = f.session().track(ids[0]).unwrap();
    let got: Vec<(f64, String)> =
        track_lines(t).iter().flat_map(|l| l.words.iter()).map(|w| (w.start as f64 / RATE as f64, w.text.clone())).collect();
    let near = |a: f64, b: f64| (a - b).abs() < 0.01;
    assert_eq!(got.iter().map(|w| w.1.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
    assert!(near(got[0].0, 3.0) && near(got[1].0, 5.0) && near(got[2].0, 8.0), "{got:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rendering_the_clip_keeps_its_words_in_place() {
    // Clip Gain › Render makes new audio from the clip's window (offset 0): the words stay put.
    let mut e = engine();
    let id = clip_id(&e);
    e.execute("clip.gain", &json!({"clips": [id], "db": -3.0})).ok();
    e.execute("clip.gain_render", &json!({"clips": [id]})).unwrap();
    let c = &e.session().track_by_name("Vox").unwrap().clips()[0];
    assert_eq!(c.source_offset(), 0, "re-sourced");
    assert_eq!(words(&e, "Vox"), at(&[(3.0, "a"), (5.0, "b"), (8.0, "c")]));
}
