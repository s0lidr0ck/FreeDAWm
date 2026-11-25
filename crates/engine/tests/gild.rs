#![allow(clippy::needless_range_loop, clippy::field_reassign_with_default)]
//! Gild (docs/gild.md): effects on chosen words, measured on the output. A synthetic singer with
//! known word times: a delay throw echoes its word only, at the note value and tempo; a reverb
//! rings past the span and the clip; Dry off gives only what Gild adds; the swell rises into the
//! word and stops; the made-ahead effects change only their span; the stacking order; spans
//! following edits; copy and paste; undo; save and load; bounce, stems, freeze and duplicates;
//! the realtime path making nothing; and the cost of twenty gildings.

use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_mix::{BuiltinPrep, MixEngine};
use soundcraft_model::{ChannelFormat, Clip, ClipId, LyricLine, LyricWord, Session, TrackId, TrackKind};
use soundcraft_time::{Range, Samples, TICKS_PER_QUARTER, TempoMap};
use std::f64::consts::TAU;

const RATE: u32 = 48_000;
const R: f64 = RATE as f64;
/// Where the clip starts on the timeline (s).
const AT: f64 = 1.0;
/// The words (text, start, end in the clip's audio, s; MIDI pitch).
const WORDS: [(&str, f64, f64, f64); 3] = [("Amen", 0.3, 0.8, 64.0), ("Lord", 1.1, 1.6, 67.0), ("Amen", 1.9, 2.4, 62.0)];
const CLIP_SECS: f64 = 3.0;

fn secs(x: f64) -> Samples {
    (x * R).round() as Samples
}

/// A sung vowel on `midi` for `[a, b)` seconds into `out`: harmonics under two formants, faded.
fn sing(out: &mut [f32], a: f64, b: f64, midi: f64) {
    let f0 = 440.0 * 2f64.powf((midi - 69.0) / 12.0);
    let (i0, i1) = (secs(a) as usize, (secs(b) as usize).min(out.len()));
    let mut ph = 0.0f64;
    for i in i0..i1 {
        let t = (i - i0) as f64 / R;
        let env = (t / 0.015).min(1.0) * ((b - a - t) / 0.02).clamp(0.0, 1.0);
        ph += TAU * f0 * (1.0 + 0.004 * (TAU * 5.0 * t).sin()) / R;
        let x: f64 = (1..=20)
            .map(|k| {
                let fk = f0 * k as f64;
                let res = |c: f64, w: f64| 1.0 / (1.0 + ((fk - c) / w).powi(2));
                (res(600.0, 120.0) + 0.5 * res(1700.0, 200.0) + 0.02) * (ph * k as f64).sin()
            })
            .sum();
        out[i] = (0.1 * env * x) as f32;
    }
}

/// The vocal's audio; with `silent`, the words at those indices left out.
fn vocal(silent: &[usize]) -> Vec<f32> {
    let mut out = vec![0.0f32; secs(CLIP_SECS) as usize];
    for (i, (_, a, b, m)) in WORDS.iter().enumerate() {
        if !silent.contains(&i) {
            sing(&mut out, *a, *b, *m);
        }
    }
    out
}

/// A "Lead Vox" mono track, the clip at `AT` holding the words, 120 BPM.
fn session_of(audio: Vec<f32>) -> (Engine, TrackId, ClipId) {
    let mut s = Session::default();
    s.tempo = TempoMap::new(120.0, 4, 4).unwrap();
    s.edit.click = false;
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead Vox"));
    let len = audio.len() as Samples;
    let src = soundcraft_engine::io::add_source(&mut s, "Vox", AudioBuffer { sample_rate: RATE, channels: vec![audio] }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    let mut c = Clip::audio(id, "Vox", src, 0, secs(AT), len);
    let words: Vec<LyricWord> = WORDS.iter().map(|(w, a, b, _)| LyricWord::new(secs(*a), secs(*b), *w)).collect();
    c.lyrics = vec![LyricLine { start: secs(0.3), end: secs(2.4), words, sheet_line: None }];
    soundcraft_engine::edit::place_clip(&mut s, t, c);
    (Engine::new(s), t, id)
}

fn session() -> (Engine, TrackId, ClipId) {
    session_of(vocal(&[]))
}

fn word(clip: ClipId, i: usize) -> Value {
    json!({"clip": clip.0, "line": 0, "word": i})
}

/// The main output's left channel over `[0, secs)`.
fn render(e: &Engine, until: f64) -> Vec<f32> {
    soundcraft_mix::render_range(e.session(), Range::new(0, secs(until)), 512).swap_remove(0)
}

fn rms(x: &[f32], a: f64, b: f64) -> f64 {
    let (i, j) = (secs(a).max(0) as usize, (secs(b) as usize).min(x.len()));
    if j <= i {
        return 0.0;
    }
    (x[i..j].iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / (j - i) as f64).sqrt()
}

fn peak(x: &[f32], a: f64, b: f64) -> f32 {
    let (i, j) = (secs(a).max(0) as usize, (secs(b) as usize).min(x.len()));
    x[i..j].iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

/// The lag (s, within `[lo, hi]`) at which `y` best matches `x`'s `[a, b)` (normalised).
fn best_lag(x: &[f32], y: &[f32], a: f64, b: f64, lo: f64, hi: f64) -> f64 {
    let (i0, i1) = (secs(a) as usize, secs(b) as usize);
    let mut best = (f64::MIN, 0);
    for lag in (secs(lo) as usize..=secs(hi) as usize).step_by(4) {
        let mut c = 0.0;
        for i in (i0..i1).step_by(2) {
            if let (Some(p), Some(q)) = (x.get(i), y.get(i + lag)) {
                c += f64::from(*p) * f64::from(*q);
            }
        }
        if c > best.0 {
            best = (c, lag);
        }
    }
    best.1 as f64 / R
}

fn add(e: &mut Engine, words: Value, fx: Value) -> Value {
    e.execute("gild.add", &json!({"words": words, "fx": fx})).unwrap()
}

fn list(e: &mut Engine, t: TrackId) -> Vec<Value> {
    e.execute("gild.list", &json!({"track": t.0})).unwrap()["gildings"].as_array().unwrap().clone()
}

#[test]
fn a_delay_throw_echoes_its_word_only_at_the_note_value() {
    let (mut e, t, c) = session();
    add(&mut e, json!([word(c, 0)]), json!({"delay": {"note": "1/4", "repeats": 0.0, "level_db": 0.0, "low_cut": 40, "high_cut": 18000}}));
    e.execute("gild.track", &json!({"track": t.0, "dry": false})).unwrap();
    let out = render(&e, 5.0);
    let (a, b) = (AT + 0.3, AT + 0.8);
    // Dry off: nothing where the word is sung, its echo half a second (a quarter at 120) later.
    if let Some(i) = out.iter().position(|x| x.abs() > 1e-5).filter(|_| std::env::var("GILD_DEBUG").is_ok()) {
        eprintln!("first sound at {:.4} s", i as f64 / R);
    }
    assert!(peak(&out, 0.0, a + 0.45) < 1e-5, "only the echo is heard: {}", peak(&out, 0.0, a + 0.45));
    assert!(rms(&out, a + 0.52, b + 0.48) > 0.01, "the echo: {}", rms(&out, a + 0.52, b + 0.48));
    let dry = vocal(&[]);
    let mut dry_tl = vec![0.0f32; secs(AT) as usize];
    dry_tl.extend(&dry);
    let lag = best_lag(&dry_tl, &out, a, b, 0.3, 0.8);
    assert!((lag - 0.5).abs() < 0.004, "echo spacing {lag} s, want 0.5 (a quarter at 120 BPM)");
    // The other words never feed it: the same output with them silent.
    let (mut e2, t2, c2) = session_of(vocal(&[1, 2]));
    add(&mut e2, json!([word(c2, 0)]), json!({"delay": {"note": "1/4", "repeats": 0.0, "level_db": 0.0, "low_cut": 40, "high_cut": 18000}}));
    e2.execute("gild.track", &json!({"track": t2.0, "dry": false})).unwrap();
    assert_eq!(render(&e2, 5.0), out, "only the gilded word feeds the throw");
    // A tempo change before the word: the echoes follow the new tempo (100 BPM: 0.6 s).
    let mut s = e.session().clone();
    s.tempo.set_tempo(2 * TICKS_PER_QUARTER, 100.0).unwrap();
    let e3 = Engine::new(s);
    let out3 = render(&e3, 5.0);
    let lag = best_lag(&dry_tl, &out3, a, b, 0.3, 0.9);
    assert!((lag - 0.6).abs() < 0.004, "after the tempo change: {lag} s, want 0.6");
    // Repeats: a second echo a beat after the first.
    let (mut e4, t4, c4) = session();
    add(&mut e4, json!([word(c4, 0)]), json!({"delay": {"note": "1/8", "repeats": 0.5, "level_db": 0.0}}));
    e4.execute("gild.track", &json!({"track": t4.0, "dry": false})).unwrap();
    let out4 = render(&e4, 5.0);
    assert!(rms(&out4, a + 0.52, b + 0.25) > 0.004 && rms(&out4, b + 0.55, b + 0.7) > 0.001, "repeats ring on");
}

#[test]
fn a_reverb_throw_rings_past_the_word_and_the_clips_end() {
    let (mut e, t, c) = session();
    // The clip ends just after "Lord".
    {
        let mut s = e.session().clone();
        s.find_clip_mut(c).unwrap().length = secs(1.7);
        e = Engine::new(s);
    }
    let plain = render(&e, 6.0);
    add(&mut e, json!([word(c, 1)]), json!({"reverb": {"space": "hall", "decay_s": 3.0, "level_db": 0.0}}));
    let out = render(&e, 6.0);
    let end = AT + 1.7;
    assert_eq!(peak(&plain, end + 0.05, 6.0), 0.0);
    assert!(rms(&out, end + 0.1, end + 1.0) > 1e-3, "the tail after the clip: {}", rms(&out, end + 0.1, end + 1.0));
    // Before the word everything is as it was (no latency, no colour).
    let a = secs(AT + 1.1) as usize;
    assert_eq!(out[..a], plain[..a], "the track is untouched before the gilded word");
    let _ = t;
}

#[test]
fn dry_off_gives_only_what_gild_adds_and_dry_on_is_unchanged_before() {
    let (mut e, t, c) = session();
    let plain = render(&e, 5.0);
    add(&mut e, json!([word(c, 1)]), json!({"delay": {"note": "1/8"}}));
    let on = render(&e, 5.0);
    let a = secs(AT + 1.1) as usize;
    assert_eq!(on[..a], plain[..a], "Dry on: the same up to the gilded word (latency unchanged)");
    assert!(on.iter().zip(&plain).skip(a).any(|(x, y)| x != y));
    e.execute("gild.track", &json!({"track": t.0, "dry": false})).unwrap();
    let off = render(&e, 5.0);
    assert!(peak(&off, 0.0, AT + 1.1) < 1e-6, "Dry off: silence before the gilded word");
    assert!(rms(&off, AT + 1.4, AT + 2.2) > 1e-3, "and the echoes");
    // The level trim scales what Gild adds.
    e.execute("gild.track", &json!({"track": t.0, "level_db": -6.0})).unwrap();
    let quiet = render(&e, 5.0);
    let r = rms(&quiet, AT + 1.4, AT + 2.2) / rms(&off, AT + 1.4, AT + 2.2);
    assert!((r - 0.501).abs() < 0.01, "-6 dB: {r}");
}

#[test]
fn the_swell_rises_into_the_word_and_stops_there() {
    let (mut e, t, c) = session();
    add(&mut e, json!([word(c, 1)]), json!({"swell": {"length_s": 1.0, "level_db": 0.0}}));
    e.execute("gild.track", &json!({"track": t.0, "dry": false})).unwrap();
    let missing = render(&e, 5.0);
    assert!(peak(&missing, 0.0, 5.0) < 1e-6, "not made yet: nothing");
    assert_eq!(e.execute("gild.render", &json!({})).unwrap()["made"], 1);
    assert_eq!(list(&mut e, t)[0]["render"], "ready");
    let out = render(&e, 5.0);
    let start = AT + 1.1;
    let late = rms(&out, start - 0.15, start);
    let early = rms(&out, start - 0.95, start - 0.75);
    assert!(late > 1e-3 && late > 2.0 * early, "rises: early {early}, just before {late}");
    assert!(peak(&out, start + 0.001, 5.0) < 1e-6, "silent once the word starts");
    // Ending earlier with the offset.
    e.execute("gild.set", &json!({"clip": c.0, "id": 1, "fx": {"swell": {"offset_ms": -200.0}}})).unwrap();
    e.execute("gild.render", &json!({})).unwrap();
    let out = render(&e, 5.0);
    assert!(peak(&out, start - 0.199, 5.0) < 1e-6);
}

#[test]
fn made_ahead_effects_change_only_their_span() {
    for (fx, tail) in [
        (json!({"double": {"cents": 9.0, "ms": 22.0}}), 0.023),
        (json!({"shift": {"semitones": 5.0, "layer": false}}), 0.0),
        (json!({"grit": {"drive": 0.8}}), 0.0),
        (json!({"shift": {"semitones": -12.0, "layer": true}}), 0.0),
        (json!({"harmony": {"voices": [{"steps": 2, "level_db": -6.0}, {"steps": -3, "level_db": -8.0}]}}), 0.0),
    ] {
        let (mut e, _t, c) = session();
        let plain = render(&e, 4.0);
        add(&mut e, json!([word(c, 1)]), fx.clone());
        e.execute("gild.render", &json!({})).unwrap();
        let out = render(&e, 4.0);
        let (a, b) = (secs(AT + 1.1) as usize, secs(AT + 1.6 + tail) as usize + 1);
        assert_eq!(out[..a], plain[..a], "{fx}: the same before the span");
        assert_eq!(out[b..], plain[b..], "{fx}: the same after it");
        let d = out[a..b].iter().zip(&plain[a..b]).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
        assert!(d > 1e-3, "{fx}: the span changed ({d})");
    }
}

#[test]
fn changed_words_are_thrown_as_changed() {
    // Grit then a delay: the echo is the gritty word, not the clean one.
    let echo_of = |fx: Value| {
        let (mut e, t, c) = session();
        add(&mut e, json!([word(c, 0)]), fx);
        e.execute("gild.track", &json!({"track": t.0, "dry": false})).unwrap();
        e.execute("gild.render", &json!({})).unwrap();
        let out = render(&e, 4.0);
        out[secs(AT + 0.8) as usize..secs(AT + 1.3) as usize].to_vec()
    };
    let clean = echo_of(json!({"delay": {"repeats": 0.0, "level_db": 0.0}}));
    let gritty = echo_of(json!({"grit": {"drive": 1.0, "blend": 1.0, "flavour": "hard"}, "delay": {"repeats": 0.0, "level_db": 0.0}}));
    // Distortion adds harmonics: more sign changes per second in the echo.
    let crossings = |x: &[f32]| x.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count();
    assert!(clean.iter().any(|x| x.abs() > 1e-3));
    assert_ne!(clean, gritty);
    assert!(crossings(&gritty) > crossings(&clean), "gritty {} vs clean {}", crossings(&gritty), crossings(&clean));
}

#[test]
fn spans_follow_their_words_clips_and_edits() {
    let (mut e, t, c) = session();
    add(&mut e, json!([word(c, 1), word(c, 2)]), json!({"reverb": true}));
    let g = &list(&mut e, t)[0];
    assert_eq!((g["start"].as_i64().unwrap(), g["end"].as_i64().unwrap()), (secs(AT + 1.1), secs(AT + 2.4)), "adjacent words: one span");
    assert_eq!(g["text"], "Lord Amen");
    // A word retimed: the span follows.
    e.execute("lyrics.word_set", &json!({"clip": c.0, "line": 0, "word": 2, "end": secs(AT + 2.5)})).unwrap();
    assert_eq!(list(&mut e, t)[0]["end"].as_i64(), Some(secs(AT + 2.5)));
    // A handle moved by hand keeps its time.
    e.execute("gild.set", &json!({"clip": c.0, "id": 1, "end": secs(AT + 2.7)})).unwrap();
    e.execute("lyrics.word_set", &json!({"clip": c.0, "line": 0, "word": 2, "end": secs(AT + 2.45)})).unwrap();
    let g = &list(&mut e, t)[0];
    assert_eq!((g["end"].as_i64(), g["held"].as_bool()), (Some(secs(AT + 2.7)), Some(true)));
    e.execute("gild.set", &json!({"clip": c.0, "id": 1, "held": false})).unwrap();
    assert_eq!(list(&mut e, t)[0]["end"].as_i64(), Some(secs(AT + 2.45)), "following its words again");
    // Moved with its clip.
    e.execute("edit.move_clips", &json!({"clips": [c.0], "by": secs(0.5)})).unwrap();
    assert_eq!(list(&mut e, t)[0]["start"].as_i64(), Some(secs(AT + 1.6)));
    e.execute("edit.undo", &json!({})).unwrap();
    // Split between the words: each piece keeps the gilding, heard where its part plays.
    e.execute("edit.separate", &json!({"tracks": [t.0], "at": secs(AT + 1.8)})).unwrap();
    let gs = list(&mut e, t);
    assert_eq!(gs.len(), 2, "{gs:?}");
    assert_eq!(gs[0]["end"].as_i64(), Some(secs(AT + 1.8)));
    assert_eq!(gs[1]["start"].as_i64(), Some(secs(AT + 1.8)));
    // A word split in two: the gilding names both parts.
    let (mut e, t, c) = session();
    add(&mut e, json!([word(c, 0)]), json!({"delay": true}));
    e.execute("lyrics.word_split", &json!({"clip": c.0, "line": 0, "word": 0})).unwrap();
    e.execute("lyrics.word_set", &json!({"clip": c.0, "line": 0, "word": 1, "end": secs(0.85) + secs(AT)})).unwrap();
    assert_eq!(list(&mut e, t)[0]["end"].as_i64(), Some(secs(AT + 0.85)));
}

#[test]
fn copy_paste_every_occurrence_and_undo() {
    let (mut e, t, c) = session();
    let picked = e.execute("lyrics.select_words", &json!({"track": t.0, "which": "same_word", "text": "amen"})).unwrap();
    let refs: Vec<Value> =
        picked["words"].as_array().unwrap().iter().map(|w| json!({"clip": w["clip"], "line": w["line"], "word": w["word"]})).collect();
    assert_eq!(refs.len(), 2);
    let made = add(&mut e, json!(refs), json!({"delay": {"note": "1/8d", "bounce": true}, "reverb": {"space": "plate"}}));
    assert_eq!(made["gildings"].as_array().unwrap().len(), 2, "separate words: separate gildings");
    let gs = list(&mut e, t);
    assert_eq!(gs[0]["fx"], gs[1]["fx"], "sharing the settings");
    assert_eq!(gs[0]["kinds"], json!(["delay", "reverb"]));
    // Already gilded: refused.
    assert!(e.execute("gild.add", &json!({"words": [word(c, 0)], "fx": {"delay": true}})).is_err());
    // Copy one, paste onto "Lord".
    e.execute("gild.copy", &json!({"clip": c.0, "id": gs[0]["id"]})).unwrap();
    e.execute("gild.paste", &json!({"words": [word(c, 1)]})).unwrap();
    let gs = list(&mut e, t);
    assert_eq!(gs.len(), 3);
    assert_eq!(gs[1]["text"], "Lord");
    assert_eq!(gs[1]["fx"], gs[0]["fx"]);
    // A new gilding starts from the last settings used.
    e.execute("gild.remove", &json!({"words": [word(c, 1)]})).unwrap();
    e.execute("gild.add", &json!({"words": [word(c, 1)]})).unwrap();
    assert_eq!(list(&mut e, t)[1]["fx"], gs[0]["fx"]);
    // Undo and redo, one step each.
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(list(&mut e, t).len(), 2);
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(list(&mut e, t).len(), 3);
    e.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(list(&mut e, t).len(), 2);
    // Remove all on the track.
    e.execute("gild.clear_track", &json!({"track": t.0})).unwrap();
    assert!(list(&mut e, t).is_empty());
    assert!(e.execute("gild.clear_track", &json!({"track": t.0})).is_err());
}

#[test]
fn untimed_tracks_and_ara_clips_are_refused_plainly() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let src = soundcraft_engine::io::add_source(&mut s, "Vox", AudioBuffer { sample_rate: RATE, channels: vec![vocal(&[])] }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "Vox", src, 0, 0, secs(3.0)));
    let mut e = Engine::new(s);
    let err = e.execute("gild.add", &json!({"track": t.0, "words": [word(id, 0)]})).unwrap_err().to_string();
    assert!(err.contains("time the words first"), "{err}");
    let (mut e, _t, c) = session();
    {
        let mut s = e.session().clone();
        s.find_clip_mut(c).unwrap().ara = Some(soundcraft_model::ClipAra { plugin: "vst3:x".into(), name: "Tuner".into(), ..Default::default() });
        e = Engine::new(s);
    }
    let err = e.execute("gild.add", &json!({"words": [word(c, 0)], "fx": {"shift": true}})).unwrap_err().to_string();
    assert!(err.contains("Tuner (ARA)"), "{err}");
    e.execute("gild.add", &json!({"words": [word(c, 0)], "fx": {"delay": true, "swell": true}})).unwrap();
}

#[test]
fn saved_and_loaded_and_older_files_still_load() {
    let (mut e, t, c) = session();
    add(&mut e, json!([word(c, 0)]), json!({"delay": {"note": "1/2"}, "double": true}));
    e.execute("gild.track", &json!({"track": t.0, "dry": false, "level_db": -3.0})).unwrap();
    let text = serde_json::to_string(e.session()).unwrap();
    let mut back: Session = serde_json::from_str(&text).unwrap();
    back.sanitize();
    let tr = back.track(t).unwrap();
    assert_eq!(tr.clips()[0].gild, e.session().track(t).unwrap().clips()[0].gild);
    assert_eq!((tr.gild.dry, tr.gild.level_db), (false, -3.0));
    // An older file (no Gild) loads with none, and a file's Gild an older build ignores.
    let mut v: Value = serde_json::from_str(&text).unwrap();
    let clip = &mut v["tracks"][0]["playlists"][0]["clips"][0];
    assert!(clip.get("gild").is_some());
    clip.as_object_mut().unwrap().remove("gild");
    let old: Session = serde_json::from_value(v).unwrap();
    assert!(old.track(t).unwrap().clips()[0].gild.is_empty());
}

fn wav(path: &std::path::Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap();
    soundcraft_audio_io::decode(&bytes, Some("wav")).unwrap().1.channels.swap_remove(0)
}

#[test]
fn bounce_stems_and_freeze_print_gild() {
    let dir = std::env::temp_dir().join(format!("gild-bounce-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (mut e, t, c) = session();
    add(&mut e, json!([word(c, 1)]), json!({"swell": {"length_s": 0.8}, "delay": {"note": "1/4"}}));
    e.execute("gild.track", &json!({"track": t.0, "dry": false})).unwrap();
    // Not rendered in the background yet: the bounce makes the swell itself.
    let path = dir.join("mix.wav");
    e.execute("file.bounce_mix", &json!({"path": path.to_str().unwrap(), "start": 0, "end": secs(5.0), "bit_depth": "32f"})).unwrap();
    let b = wav(&path);
    let before = AT + 1.1;
    assert!(rms(&b, before - 0.2, before) > 1e-3, "the swell is in the bounce");
    assert!(rms(&b, before + 0.55, before + 1.0) > 1e-3, "and the echo");
    e.execute("file.bounce_stems", &json!({"dir": dir.to_str().unwrap(), "tracks": [t.0], "start": 0, "end": secs(5.0), "bit_depth": "32f"}))
        .unwrap();
    let stem = std::fs::read_dir(&dir).unwrap().flatten().map(|d| d.path()).find(|p| p.to_string_lossy().contains("Lead Vox")).unwrap();
    let s = wav(&stem);
    assert!(rms(&s, before - 0.2, before) > 1e-3 && rms(&s, before + 0.55, before + 1.0) > 1e-3, "and in the track's stem");
    // Freeze prints it; the frozen track plays it without running it again.
    e.execute("gild.render", &json!({})).unwrap();
    let live = render(&e, 5.0);
    e.execute("track.freeze", &json!({"tracks": [t.0]})).unwrap();
    assert!(e.session().track(t).unwrap().frozen);
    let frozen = render(&e, 5.0);
    let d = live.iter().zip(&frozen).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
    assert!(d < 1e-4, "frozen plays as live: {d}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_duplicated_track_keeps_gild_and_the_realtime_path_makes_nothing() {
    let (mut e, t, c) = session();
    add(&mut e, json!([word(c, 0)]), json!({"delay": true, "reverb": true}));
    e.execute("track.duplicate", &json!({"tracks": [t.0]})).unwrap();
    let dup = e.session().tracks.iter().find(|x| x.id != t).unwrap();
    assert_eq!(dup.clips()[0].gild.len(), 1, "the duplicate has the same gildings");
    // Realtime: the modules come from the control thread; the render path makes none.
    let s = e.session().clone();
    let mut eng = MixEngine::new(48_000.0, 512);
    eng.set_external_instances(true);
    eng.set_prepared_builtins(true);
    let mut reference = MixEngine::new(48_000.0, 512);
    reference.set_external_instances(true);
    let mut prep = BuiltinPrep::default();
    eng.adopt_prepared(prep.prepare(&s, 512));
    let (mut out, mut want) = (vec![vec![0.0f32; 512]; 2], vec![vec![0.0f32; 512]; 2]);
    for b in 0..(secs(4.0) / 512) {
        eng.render(&s, b * 512, 512, &mut out);
        reference.render(&s, b * 512, 512, &mut want);
        assert_eq!(out, want, "block {b}");
    }
    assert_eq!(eng.made_in_sync(), 0, "no module was made on the render path");
}

#[test]
fn twenty_gildings_cost() {
    // Twenty words, each with a delay and a reverb throw.
    let n = 20;
    let mut audio = vec![0.0f32; secs(n as f64 * 0.8 + 1.0) as usize];
    for k in 0..n {
        sing(&mut audio, 0.3 + k as f64 * 0.8, 0.7 + k as f64 * 0.8, 60.0 + (k % 7) as f64);
    }
    let mut s = Session::default();
    s.edit.click = false;
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead Vox"));
    let len = audio.len() as Samples;
    let src = soundcraft_engine::io::add_source(&mut s, "Vox", AudioBuffer { sample_rate: RATE, channels: vec![audio] }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    let mut c = Clip::audio(id, "Vox", src, 0, 0, len);
    let words: Vec<LyricWord> = (0..n).map(|k| LyricWord::new(secs(0.3 + k as f64 * 0.8), secs(0.7 + k as f64 * 0.8), "la")).collect();
    c.lyrics = vec![LyricLine { start: 0, end: len, words, sheet_line: None }];
    soundcraft_engine::edit::place_clip(&mut s, t, c);
    let e = Engine::new(s);
    let r = Range::new(0, len);
    let time = |e: &Engine| {
        let t0 = std::time::Instant::now();
        let out = soundcraft_mix::render_range(e.session(), r, 512);
        (t0.elapsed().as_secs_f64(), out)
    };
    let (bare, _) = time(&e);
    let audio_secs = len as f64 / R;
    let base = e.session().clone();
    // The same throw on every word (shared runs), then every word a throw of its own.
    for (name, distinct) in [("the same settings", false), ("each its own settings", true)] {
        let mut e = Engine::new(base.clone());
        for k in 0..n {
            let repeats = if distinct { 0.3 + 0.01 * k as f64 } else { 0.4 };
            add(&mut e, json!([word(id, k)]), json!({"delay": {"repeats": repeats}, "reverb": {"decay_s": 2.8}}));
        }
        assert_eq!(list(&mut e, t).len(), n);
        let (gilded, _) = time(&e);
        eprintln!(
            "GILD CPU ({name}): {n} gildings (delay + reverb each) over {audio_secs:.1} s of audio: {gilded:.3} s to render vs {bare:.3} s bare \
             ({:.1}% of real time for Gild, one core)",
            100.0 * (gilded - bare).max(0.0) / audio_secs
        );
        assert!(gilded < audio_secs, "renders faster than real time");
    }
}

#[test]
fn harmony_follows_the_song_key() {
    let (mut e, t, c) = session();
    add(&mut e, json!([word(c, 1)]), json!({"harmony": true}));
    let key0 = list(&mut e, t)[0]["fx"]["harmony"]["key"].clone();
    assert!(key0.is_array(), "the key at the word is kept: {key0}");
    e.execute("gild.render", &json!({})).unwrap();
    assert_eq!(list(&mut e, t)[0]["render"], "ready");
    // A key change on the Key ruler before the word: the harmony is made again in it.
    let mut s = e.session().clone();
    s.key_signatures.push((0, "Eb minor".into()));
    let mut e = Engine::new(s);
    e.execute("gild.track", &json!({"track": t.0, "level_db": 0.0})).unwrap();
    let g = &list(&mut e, t)[0];
    assert_ne!(g["fx"]["harmony"]["key"], key0);
    assert_eq!(g["render"], "making");
}
