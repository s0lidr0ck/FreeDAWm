//! A note moved in the Vocal Editor is what plays, measured on the output's pitch.
//!
//! A clip given to an ARA plug-in plays the plug-in's audio in place of its own, so a tuning made
//! here on such a clip would show moved notes that sound as sung: the note commands refuse it
//! with a plain message, and the tuning plays once the plug-in is bypassed. A harmony made from
//! such a take plays its own tuned audio. A harmony note moved further (well past the harmony's
//! own interval) sounds moved by the whole amount.

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::vocal::{self, NoteRef};
use soundcraft_model::{AraRender, AraRenders, ChannelFormat, Clip, ClipAra, ClipId, Session, TrackId, TrackKind, ara_fingerprint};
use soundcraft_time::Range;
use std::collections::BTreeMap;
use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

const RATE: u32 = 48_000;

/// A sung vowel (harmonics through three formants) at `f0` for `secs`.
fn vowel(f0: f64, secs: f64) -> Vec<f32> {
    let n = (secs * f64::from(RATE)) as usize;
    let formants = [(700.0, 100.0, 1.0), (1200.0, 110.0, 0.6), (2600.0, 160.0, 0.3)];
    let ks: Vec<(f64, f64)> = (1..=40)
        .map(|k| f64::from(k) * f0)
        .filter(|f| *f < f64::from(RATE) / 2.0)
        .map(|f| (f, formants.iter().map(|(c, b, a)| a / (1.0 + ((f - c) / b).powi(2))).sum::<f64>() + 0.01))
        .collect();
    let peak = 0.3 / ks.iter().map(|(_, a)| a).sum::<f64>().max(1e-9);
    (0..n).map(|i| (ks.iter().map(|(f, a)| a * (TAU * f * i as f64 / f64::from(RATE)).sin()).sum::<f64>() * peak) as f32).collect()
}

fn hz(midi: f64) -> f64 {
    440.0 * 2f64.powf((midi - 69.0) / 12.0)
}

/// A vocal track in F# major singing A#3, C#4, D#4, F#4 (0.8 s each, 0.15 s apart), analysed.
fn session() -> (Engine, TrackId, ClipId, i64) {
    let mut s = Session::default();
    s.band.key = 6;
    let mut take = Vec::new();
    for m in [58.0, 61.0, 63.0, 66.0] {
        take.extend(vowel(hz(m), 0.8));
        take.extend(std::iter::repeat_n(0.0, (0.15 * f64::from(RATE)) as usize));
    }
    let len = take.len() as i64;
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vocal"));
    let src = soundcraft_engine::io::add_source(&mut s, "Vocal", AudioBuffer { sample_rate: RATE, channels: vec![take] }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "Vocal", src, 0, 0, len));
    if let Some(tr) = s.track_mut(t) {
        tr.vocal = Some(true);
    }
    let mut e = Engine::new(s);
    vocal::run_until_idle(&mut e, &[id]).unwrap();
    (e, t, id, len)
}

/// What `track` plays over the take (an ARA clip's render in place of its file, as heard).
fn heard(e: &Engine, track: TrackId, len: i64) -> Vec<f32> {
    soundcraft_mix::render_clips_as_heard(e.session(), track, Range::new(0, len)).into_iter().next().unwrap()
}

/// The centre (cents) of the note of `audio` overlapping frames `[a, b)` most, re-tracked.
fn sung(audio: &[f32], a: u32, b: u32) -> f32 {
    let an = soundcraft_vocal::analyse(audio, RATE, &AtomicBool::new(false)).unwrap();
    an.notes.iter().max_by_key(|n| n.end.min(b).saturating_sub(n.start.max(a))).map(|n| n.centre_cents).unwrap()
}

/// The frames of `r`'s note.
fn frames(e: &Engine, r: NoteRef) -> (u32, u32) {
    let c = e.session().find_clip(r.clip).unwrap().1.clone();
    let a = vocal::analysis_for(e.session(), &c).unwrap();
    a.notes.iter().find(|n| n.id == r.note).map(|n| (n.start, n.end)).unwrap()
}

/// Gives `clip` to an ARA plug-in whose render (what playback plays) is the take as sung, as a
/// pitch editor with no edits of its own plays it.
fn give_to_ara(e: &mut Engine, clip: ClipId, track: TrackId, len: i64) {
    let s = e.session_mut();
    let c = s.find_clip_mut(clip).unwrap();
    c.ara = Some(ClipAra {
        plugin: "vst3:0123456789ABCDEF0123456789ABCDEF".into(),
        name: "Pitch Tool".into(),
        origin: Some(clip.0),
        ..ClipAra::default()
    });
    let c = c.clone();
    let mut original = e.session().clone();
    if let Some(x) = original.find_clip_mut(clip) {
        x.ara = None;
        x.tune = None;
    }
    let audio = soundcraft_mix::render_clips(&original, track, Range::new(0, len));
    let mut map = BTreeMap::new();
    map.insert(clip, Arc::new(AraRender { fingerprint: ara_fingerprint(&c).unwrap(), audio: Arc::new(audio) }));
    let mut all = (*e.session().ara_renders.0).clone();
    all.extend(map);
    e.set_ara_renders(AraRenders(Arc::new(all)));
}

#[test]
fn a_clip_played_through_an_ara_plugin_refuses_moves_and_plays_them_once_bypassed() {
    let (mut e, t, c, len) = session();
    let notes = vocal::track_notes(e.session(), t, None);
    let r = notes[1];
    let (a, b) = frames(&e, r);
    let before = sung(&heard(&e, t, len), a, b);
    // A move made before the clip went to the plug-in: shown moved, and (the bug) not heard.
    e.execute("vocal.edit", &json!({"notes": [r], "offset": 300})).unwrap();
    vocal::run_until_idle(&mut e, &[c]).unwrap();
    assert!((sung(&heard(&e, t, len), a, b) - before - 300.0).abs() < 10.0, "heard moved without a plug-in");
    give_to_ara(&mut e, c, t, len);
    let through = sung(&heard(&e, t, len), a, b);
    assert!((through - before).abs() < 10.0, "through the plug-in the note sounds as sung ({} c)", through - before);
    // So the editor's commands refuse to move it there, saying why and what to do.
    let err = e.execute("vocal.edit", &json!({"notes": [r], "offset": 500})).unwrap_err().to_string();
    assert!(err.contains("Pitch Tool") && err.contains("ARA") && err.contains("bypass"), "{err}");
    assert!(e.execute("vocal.edit", &json!({"notes": [r], "gain_db": -6})).is_err(), "level too");
    assert!(e.execute("vocal.edit", &json!({"notes": [r], "formant": 2})).is_err(), "formant too");
    assert!(e.execute("vocal.edit", &json!({"notes": [r], "drift": 100, "vibrato": -100})).is_err(), "drift and vibrato too");
    assert_eq!(e.session().find_clip(c).unwrap().1.tune_live().unwrap().edits[&r.note].offset_cents, 300.0, "nothing changed");
    // Bypassed, the clip plays its own audio with the tuning: the move is heard.
    e.execute("clip.ara_bypass", &json!({"clips": [c.0], "on": true})).unwrap();
    vocal::run_until_idle(&mut e, &[c]).unwrap();
    assert!((sung(&heard(&e, t, len), a, b) - before - 300.0).abs() < 10.0, "bypassed: heard moved");
    e.execute("vocal.edit", &json!({"notes": [r], "offset": 500})).unwrap();
    vocal::run_until_idle(&mut e, &[c]).unwrap();
    assert!((sung(&heard(&e, t, len), a, b) - before - 500.0).abs() < 10.0, "and moved again");
}

#[test]
fn a_duplicated_ara_track_refuses_moves_too() {
    let (mut e, t, c, len) = session();
    give_to_ara(&mut e, c, t, len);
    let d = e.execute("track.duplicate", &json!({"tracks": [t.0]})).unwrap();
    let dt = TrackId(d["tracks"][0].as_u64().unwrap());
    let dc = e.session().track(dt).unwrap().clips()[0].id;
    vocal::run_until_idle(&mut e, &[dc]).unwrap();
    let notes = vocal::track_notes(e.session(), dt, None);
    assert!(!notes.is_empty());
    let err = e.execute("vocal.edit", &json!({"notes": [notes[0]], "offset_by": 300})).unwrap_err().to_string();
    assert!(err.contains("ARA"), "{err}");
}

#[test]
fn a_harmony_of_an_ara_take_plays_its_own_tuned_audio() {
    let (mut e, lead, c, len) = session();
    give_to_ara(&mut e, c, lead, len);
    let lead_heard = heard(&e, lead, len);
    let r = e.execute("vocal.harmony", &json!({"source": lead.0})).unwrap();
    assert_eq!(r["ara_left_out"], json!(["Pitch Tool"]));
    let h = TrackId(r["track"].as_u64().unwrap());
    let hc = ClipId(r["clips"][0].as_u64().unwrap());
    assert!(e.session().find_clip(hc).unwrap().1.ara.is_none(), "the copy is not given to the plug-in");
    vocal::run_until_idle(&mut e, &[hc]).unwrap();
    let notes = vocal::track_notes(e.session(), h, None);
    let n = notes[0];
    let (a, b) = frames(&e, n);
    let base = sung(&lead_heard, a, b);
    let third = e.session().find_clip(hc).unwrap().1.tune_live().unwrap().edits[&n.note].offset_cents;
    assert_eq!(third, 300.0, "A#3 up a diatonic 3rd in F# major is C#4");
    assert!((sung(&heard(&e, h, len), a, b) - base - third).abs() < 10.0, "the harmony sounds a third up");
    // Moved further, by hand: heard.
    e.execute("vocal.edit", &json!({"notes": [n], "offset": 748})).unwrap();
    vocal::run_until_idle(&mut e, &[hc]).unwrap();
    assert!((sung(&heard(&e, h, len), a, b) - base - 748.0).abs() < 10.0, "moved to +748 c");
}

#[test]
fn a_harmony_note_moved_far_sounds_moved_by_the_whole_amount() {
    let (mut e, lead, _, len) = session();
    let base_audio = heard(&e, lead, len);
    let r = e.execute("vocal.harmony", &json!({"source": lead.0})).unwrap();
    let h = TrackId(r["track"].as_u64().unwrap());
    let hc = ClipId(r["clips"][0].as_u64().unwrap());
    let n = vocal::track_notes(e.session(), h, None)[2];
    let (a, b) = frames(&e, n);
    let base = sung(&base_audio, a, b);
    for to in [748.0f32, 1900.0, -1300.0] {
        e.execute("vocal.edit", &json!({"notes": [n], "offset": to})).unwrap();
        vocal::run_until_idle(&mut e, &[hc]).unwrap();
        let got = sung(&heard(&e, h, len), a, b) - base;
        assert!((got - to).abs() < 10.0, "offset {to}: heard {got} c");
    }
    // Level and formant on that note apply too: louder by the gain, the move kept.
    let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt();
    let body = |x: &[f32]| x[(a as usize + 20) * 240..(b as usize - 20) * 240].to_vec();
    let quiet = rms(&body(&heard(&e, h, len)));
    e.execute("vocal.edit", &json!({"notes": [n], "gain_db": 6})).unwrap();
    vocal::run_until_idle(&mut e, &[hc]).unwrap();
    let loud = rms(&body(&heard(&e, h, len)));
    assert!((20.0 * (loud / quiet).log10() - 6.0).abs() < 1.5, "gain {} dB", 20.0 * (loud / quiet).log10());
    let before = heard(&e, h, len);
    e.execute("vocal.edit", &json!({"notes": [n], "formant": 3})).unwrap();
    vocal::run_until_idle(&mut e, &[hc]).unwrap();
    let after = heard(&e, h, len);
    assert_ne!(body(&before), body(&after), "the formant shift changes the note");
    assert!((sung(&after, a, b) - base + 1300.0).abs() < 10.0, "its pitch kept");
}
