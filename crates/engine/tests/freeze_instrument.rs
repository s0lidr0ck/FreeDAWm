//! Track › Freeze on instrument tracks: the instrument's output for the track's MIDI, through the
//! chain (down to and including the freeze point), plays in place of the instrument, the MIDI
//! clips and the frozen modules. The modules after the point stay live; the MIDI and the
//! instrument are read-only while frozen; unfreeze plays live again.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{Session, TrackId, TrackKind};
use soundcraft_time::Range;

fn id(e: &Engine, name: &str) -> TrackId {
    e.session().tracks.iter().find(|t| t.name == name).unwrap().id
}

/// The demo with a gain module (+4 dB) and a second gain module (-2 dB) on the Keys (an
/// instrument track playing the subtractive synth). The Verb aux is made inactive: its plate
/// reverb's modulation runs free, which is not what these tests measure.
fn engine() -> Engine {
    let mut e = soundcraft_engine::demo::demo_engine();
    e.execute("mix.insert", &json!({"track": "Keys", "plugin": "gain", "params": {"gain": 4.0}})).unwrap();
    e.execute("mix.insert", &json!({"track": "Keys", "plugin": "gain", "params": {"gain": -2.0}})).unwrap();
    let verb = id(&e, "Verb");
    e.session_mut().track_mut(verb).unwrap().inactive = true;
    e
}

fn modules(e: &Engine, track: &str) -> Vec<u64> {
    e.session().track(id(e, track)).unwrap().mixer.inserts.iter().map(|i| u64::from(i.id.0)).collect()
}

/// The main mix from the start to 4 s past where the Keys come in.
fn mix(s: &Session) -> Vec<Vec<f32>> {
    let keys = s.tracks.iter().find(|t| t.name == "Keys").unwrap();
    let from = keys.clips().iter().map(|c| c.start).min().unwrap();
    soundcraft_mix::render_range(s, Range::new(0, from + s.sample_rate.samples(4.0)), 1024)
}

fn thawed(s: &Session) -> Session {
    let mut s = s.clone();
    for t in &mut s.tracks {
        t.frozen = false;
        t.freeze = None;
    }
    s
}

fn worst(a: &[Vec<f32>], b: &[Vec<f32>]) -> f32 {
    assert_eq!(a.len(), b.len(), "channels");
    a.iter().zip(b).fold(0.0f32, |w, (x, y)| {
        assert_eq!(x.len(), y.len(), "length");
        x.iter().zip(y).fold(w, |w, (p, q)| w.max((p - q).abs()))
    })
}

/// 1e-4 is about -60 dB under the demo's signals (0.07-0.5): what an f32 render and automation
/// ramped at block boundaries differ by, not an audible change. Loosening it would hide one.
fn assert_close(a: &[Vec<f32>], b: &[Vec<f32>], what: &str) {
    let d = worst(a, b);
    assert!(d < 1e-4, "{what}: differs by {d}");
    assert!(a.iter().flatten().any(|v| v.abs() > 1e-3), "{what}: silent");
}

/// The Keys are in the mix: muting them changes it.
fn assert_keys_heard(s: &Session) {
    let mut muted = s.clone();
    muted.tracks.iter_mut().find(|t| t.name == "Keys").unwrap().mixer.mute = true;
    assert!(worst(&mix(s), &mix(&muted)) > 1e-3, "the Keys are heard");
}

#[test]
fn a_frozen_instrument_track_sounds_as_it_did_live() {
    let mut e = engine();
    assert_keys_heard(e.session());
    let live = mix(e.session());
    let r = e.execute("track.freeze", &json!({"tracks": ["Keys"]})).unwrap();
    assert_eq!(r["frozen"].as_array().map(Vec::len), Some(1), "{r}");
    let tr = e.session().track(id(&e, "Keys")).unwrap();
    let fz = tr.frozen_audio().copied().expect("frozen");
    assert_eq!(fz.upto, None, "the whole chain");
    let src = e.session().sources.iter().find(|x| x.id == fz.source).expect("the render is a session source");
    assert!(src.path.starts_with("Audio Files/"), "{}", src.path);
    assert_close(&mix(e.session()), &live, "frozen vs live");
    // The fader stays live.
    e.execute("mix.volume", &json!({"track": "Keys", "db": -20.0})).unwrap();
    assert_close(&mix(e.session()), &mix(&thawed(e.session())), "fader move after freeze");
}

#[test]
fn the_render_keeps_the_release_tail_and_stops_once_it_is_silent() {
    let mut e = engine();
    // A long release: the tail rings well past the clip's end.
    e.execute("mix.instrument_param", &json!({"track": "Keys", "param": "amp_release", "value": 3000.0})).unwrap();
    e.execute("track.freeze", &json!({"tracks": ["Keys"]})).unwrap();
    let s = e.session();
    let tr = s.track(id(&e, "Keys")).unwrap();
    let end = tr.clips().iter().map(|c| c.end()).max().unwrap();
    let audio = &s.pool.get(tr.freeze.unwrap().source).unwrap().buffer();
    let frames = audio.frames() as i64;
    assert!(frames > end + s.sample_rate.samples(0.5), "the tail is kept: {frames} frames, content to {end}");
    assert!(frames <= end + s.sample_rate.samples(10.0) + 1024, "at most 10 s past the last note-off: {frames}");
    // It stops once the output is below -90 dBFS, not at the 10 s cap.
    assert!(frames < end + s.sample_rate.samples(9.0), "stopped at silence: {frames}");
    let floor = 10f32.powf(-90.0 / 20.0);
    let last: f32 = audio.channels.iter().map(|c| c.last().copied().unwrap_or(0.0).abs()).fold(0.0, f32::max);
    assert!(last < floor * 4.0, "the render ends within 12 dB of the floor: {last}");
}

#[test]
fn freeze_up_to_here_on_an_instrument_track_leaves_later_modules_live() {
    let mut e = engine();
    let m = modules(&e, "Keys");
    e.execute("track.freeze", &json!({"tracks": ["Keys"], "upto": m[0]})).unwrap();
    let tr = e.session().track(id(&e, "Keys")).unwrap();
    assert_eq!(tr.frozen_modules(), 1);
    assert_close(&mix(e.session()), &mix(&thawed(e.session())), "frozen up to the first module");
    // The module after the point is live: editable, and heard.
    e.execute("mix.insert_param", &json!({"track": "Keys", "slot": 1, "param": "gain", "value": -12.0})).unwrap();
    assert_close(&mix(e.session()), &mix(&thawed(e.session())), "the live module's edit is heard");
    // The frozen module, the instrument and the MIDI are not.
    let clip = e.session().track(id(&e, "Keys")).unwrap().clips()[0].id.0;
    for (cmd, p) in [
        ("mix.insert_param", json!({"track": "Keys", "slot": 0, "param": "gain", "value": 9.0})),
        ("mix.instrument_param", json!({"track": "Keys", "param": "cutoff", "value": 300})),
        ("mix.instrument", json!({"track": "Keys", "plugin": "drum_synth"})),
        ("midi.note_add", json!({"clip": clip, "pitch": 60, "start_ticks": 0})),
        ("midi.note_delete", json!({"clip": clip, "indices": [0]})),
    ] {
        let err = e.execute(cmd, &p).expect_err(cmd).to_string();
        assert!(err.contains("Unfreeze the track to edit it"), "{cmd}: {err}");
    }
}

#[test]
fn unfreezing_an_instrument_track_restores_it() {
    let mut e = engine();
    let live = mix(e.session());
    let before = e.session().track(id(&e, "Keys")).unwrap().clone();
    e.execute("track.freeze", &json!({"tracks": ["Keys"]})).unwrap();
    // Played frozen for a while, then thawed: the same engine sounds as live (a fresh render, but
    // this checks the session).
    e.execute("track.unfreeze", &json!({"tracks": ["Keys"]})).unwrap();
    let tr = e.session().track(id(&e, "Keys")).unwrap();
    assert_eq!(tr, &before, "the track is as it was");
    assert!(e.session().sources.iter().all(|s| !s.name.contains("Keys Freeze")), "the render left the session");
    assert_close(&mix(e.session()), &live, "thawed");
    e.execute("mix.instrument_param", &json!({"track": "Keys", "param": "cutoff", "value": 300})).unwrap();
    // One undo step each.
    e.execute("edit.undo", &json!({})).unwrap();
    e.execute("edit.undo", &json!({})).unwrap();
    assert!(e.session().track(id(&e, "Keys")).unwrap().frozen_audio().is_some(), "undoing the unfreeze freezes again");
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(e.session().track(id(&e, "Keys")).unwrap(), &before);
}

#[test]
fn a_thawed_instrument_starts_clean_in_a_running_mix() {
    // A realtime mix engine plays the Keys live, then frozen, then thawed: after the thaw the
    // instrument holds nothing from before the freeze, so it sounds as a fresh engine does.
    let mut e = engine();
    let keys = id(&e, "Keys");
    for t in &mut e.session_mut().tracks {
        if t.id != keys && t.kind != TrackKind::Master {
            t.inactive = true;
        }
    }
    let from = e.session().track(keys).unwrap().clips()[0].start;
    let sr = e.session().sample_rate.as_f64() as f32;
    let block = |eng: &mut soundcraft_mix::MixEngine, s: &Session, pos: i64| {
        let mut out = vec![vec![0.0f32; 1024]; 2];
        eng.render(s, pos, 1024, &mut out);
        out
    };
    let live = e.session().clone();
    let mut eng = soundcraft_mix::MixEngine::new(sr, 1024);
    let mut pos = from;
    for _ in 0..40 {
        block(&mut eng, &live, pos);
        pos += 1024;
    }
    e.execute("track.freeze", &json!({"tracks": ["Keys"]})).unwrap();
    let frozen = e.session().clone();
    for _ in 0..8 {
        block(&mut eng, &frozen, pos);
        pos += 1024;
    }
    // Thawed and located back to the Keys' first note.
    let mut fresh = soundcraft_mix::MixEngine::new(sr, 1024);
    let mut heard = false;
    for k in 0..8 {
        let at = from + k * 1024;
        let a = block(&mut eng, &live, at);
        let b = block(&mut fresh, &live, at);
        heard |= b.iter().flatten().any(|x| x.abs() > 1e-3);
        // A fresh instance ramps its parameters in over the first blocks; after that the two
        // agree (a voice held over from before the freeze would ring on at about 0.07).
        if k >= 4 {
            assert!(worst(&a, &b) < 1e-4, "block {k} after the thaw differs by {}", worst(&a, &b));
        }
    }
    assert!(heard, "the Keys play");
}

#[test]
fn tracks_without_an_instrument_are_refused_clearly() {
    let mut e = engine();
    for (name, why) in [("Verb", "no instrument"), ("Master", "is a Master Fader")] {
        if e.session().tracks.iter().all(|t| t.name != name) {
            continue;
        }
        let err = e.execute("track.freeze", &json!({"tracks": [name]})).unwrap_err().to_string();
        assert!(err.contains("Freeze works on audio tracks and tracks with an instrument"), "{name}: {err}");
        assert!(err.contains(why), "{name}: {err}");
    }
    e.execute("track.new", &json!({"name": "Bare", "kind": "instrument"})).unwrap();
    let bare = id(&e, "Bare");
    assert_eq!(e.session().track(bare).unwrap().kind, TrackKind::Instrument);
    e.session_mut().track_mut(bare).unwrap().instrument = None;
    let err = e.execute("track.freeze", &json!({"tracks": ["Bare"]})).unwrap_err().to_string();
    assert!(err.contains("has no instrument"), "{err}");
    assert!(e.session().tracks.iter().all(|t| !t.frozen), "a refused freeze freezes nothing");
}

#[test]
fn a_tempo_change_is_refused_while_an_instrument_track_is_frozen() {
    let mut e = engine();
    e.execute("track.freeze", &json!({"tracks": ["Keys"]})).unwrap();
    let tempo = e.session().tempo.clone();
    let err = e.execute("event.tempo", &json!({"bpm": 90.0})).unwrap_err().to_string();
    assert!(err.contains("`Keys` is frozen") && err.contains("Unfreeze it to change the tempo"), "{err}");
    assert_eq!(e.session().tempo, tempo);
    e.execute("track.unfreeze", &json!({"tracks": ["Keys"]})).unwrap();
    e.execute("event.tempo", &json!({"bpm": 90.0})).unwrap();
}

#[test]
fn midi_real_time_properties_and_track_offsets_are_refused_while_frozen() {
    let mut e = engine();
    let keys = id(&e, "Keys");
    e.execute("track.freeze", &json!({"tracks": ["Keys"]})).unwrap();
    let values = e.session().edit.values.clone();
    let err = e.execute("event.midi_rtp", &json!({"tracks": [keys.0], "transpose": 12})).unwrap_err().to_string();
    assert!(err.contains("`Keys` is frozen") && err.contains("Unfreeze it to change them"), "{err}");
    let err = e.execute("event.midi_track_offsets", &json!({"track": keys.0, "offset": 480})).unwrap_err().to_string();
    assert!(err.contains("`Keys` is frozen") && err.contains("Unfreeze it to change it"), "{err}");
    assert_eq!(e.session().edit.values, values, "nothing changed");
    // Another track's settings are not blocked.
    let lead = id(&e, "Lead");
    e.execute("event.midi_rtp", &json!({"tracks": [lead.0], "transpose": 12})).unwrap();
    e.execute("event.midi_track_offsets", &json!({"track": lead.0, "offset": 480})).unwrap();
    e.execute("track.unfreeze", &json!({"tracks": ["Keys"]})).unwrap();
    e.execute("event.midi_rtp", &json!({"tracks": [keys.0], "transpose": 12})).unwrap();
    e.execute("event.midi_track_offsets", &json!({"track": keys.0, "offset": 480})).unwrap();
}

#[test]
fn midi_cleanup_is_refused_and_its_preview_kept_while_frozen() {
    let mut e = engine();
    let keys = id(&e, "Keys");
    let clip = e.session().track(keys).unwrap().clips()[0].id;
    e.execute("band.chord.add", &json!({"at": 0, "chord": "C"})).unwrap();
    e.execute("midi.cleanup", &json!({"clip": clip.0})).unwrap();
    let cleanup = |e: &Engine| e.session().find_clip(clip).unwrap().1.midi_cleanup.clone().unwrap();
    let preview = |e: &Engine| cleanup(e).preview.unwrap();
    assert!(!cleanup(&e).original, "on Cleaned");
    e.execute("track.freeze", &json!({"tracks": ["Keys"]})).unwrap();
    let kept = preview(&e);
    let before = e.session().clone();
    for (cmd, p) in [
        ("midi.cleanup", json!({"clip": clip.0, "toggle": true})),
        ("midi.cleanup", json!({"clip": clip.0, "wrong": 95})),
        ("midi.cleanup_remove", json!({"clip": clip.0})),
        ("midi.cleanup_commit", json!({"clip": clip.0})),
    ] {
        let err = e.execute(cmd, &p).expect_err(cmd).to_string();
        assert!(err.contains("Unfreeze the track to edit it"), "{cmd}: {err}");
    }
    assert!(*e.session() == before, "nothing changed");
    // The cleaned notes follow the chords under the clip: a chord edit there is refused too (the
    // preview is not saved, so it could not be kept past a reopen), one elsewhere is not.
    let (start, end) = {
        let c = &e.session().track(keys).unwrap().clips()[0];
        (c.start, c.end())
    };
    let err = e.execute("band.chord.add", &json!({"at": start, "chord": "F#m7"})).unwrap_err().to_string();
    assert!(err.contains("`Keys` is frozen, and its MIDI Cleanup follows the chords. Unfreeze it to change them"), "{err}");
    assert!(*e.session() == before, "the chords are as they were");
    e.execute("band.chord.add", &json!({"at": end + 1, "chord": "F#m7"})).unwrap();
    assert!(std::sync::Arc::ptr_eq(&preview(&e), &kept), "a chord after the clip leaves its cleaned notes alone");
    // Saved and reopened, the preview is derived again from the same chords: the render matches.
    let dir = std::env::temp_dir().join(format!("soundcraft-freeze-cleanup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("song.scraft");
    soundcraft_engine::io::save_session(&mut e, path.to_str().unwrap()).unwrap();
    let mut back = Engine::default();
    let missing = soundcraft_engine::io::open_session(&mut back, path.to_str().unwrap()).unwrap();
    assert!(missing.is_empty(), "{missing:?}");
    assert!(back.session().track(keys).unwrap().frozen_audio().is_some(), "still frozen");
    assert!(back.session().find_clip(clip).unwrap().1.midi_cleanup.as_ref().is_some_and(|m| m.preview.is_some()), "the preview is derived on open");
    assert_close(&mix(back.session()), &mix(&thawed(back.session())), "reopened: the render matches the cleaned notes");
    let _ = std::fs::remove_dir_all(&dir);
    // Thawed, the chords under the clip can change again, and the preview follows.
    e.execute("track.unfreeze", &json!({"tracks": ["Keys"]})).unwrap();
    e.execute("band.chord.add", &json!({"at": start, "chord": "F#m7"})).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&preview(&e), &kept), "thawed, the preview is derived from the new chords");
    e.execute("midi.cleanup", &json!({"clip": clip.0, "toggle": true})).unwrap();
}

/// A new instrument track, alone in the mix (the demo's tracks are made inactive), whose one
/// clip has a single short note at its very end, with the instrument's release short, so the
/// render has nothing to hear between the clip's end and a delayed note.
fn solo_note(e: &mut Engine) -> (TrackId, i64) {
    e.execute("track.new", &json!({"name": "Solo", "kind": "instrument"})).unwrap();
    let t = id(e, "Solo");
    for tr in &mut e.session_mut().tracks {
        if tr.id != t && tr.kind != TrackKind::Master {
            tr.inactive = true;
        }
    }
    let sr = e.session().sample_rate;
    let end = sr.samples(2.0);
    let clip = e.execute("midi.new_clip", &json!({"track": t.0, "start": 0, "end": end})).unwrap()["clip"].as_u64().unwrap();
    let ticks = e.session().tempo.samples_to_ticks(end, sr);
    e.execute("midi.note_add", &json!({"clip": clip, "pitch": 72, "start_ticks": ticks - 48, "length_ticks": 48, "velocity": 110})).unwrap();
    e.execute("mix.instrument_param", &json!({"track": t.0, "param": "amp_release", "value": 10.0})).unwrap();
    (t, end)
}

fn render_of(e: &Engine, t: TrackId) -> (Vec<Vec<f32>>, i64) {
    let s = e.session();
    let audio = &s.pool.get(s.track(t).unwrap().freeze.unwrap().source).unwrap().buffer();
    (audio.channels.clone(), audio.frames() as i64)
}

fn heard_after(audio: &[Vec<f32>], at: i64) -> bool {
    let at = usize::try_from(at).unwrap();
    audio.iter().any(|c| c.get(at..).is_some_and(|x| x.iter().any(|v| v.abs() > 1e-3)))
}

#[test]
fn a_note_delayed_past_the_clip_end_by_a_track_offset_is_in_the_render() {
    let mut e = engine();
    let (t, end) = solo_note(&mut e);
    let sr = e.session().sample_rate;
    let offset = sr.samples(0.3);
    e.execute("event.midi_track_offsets", &json!({"track": t.0, "offset": offset})).unwrap();
    // Live, the note sounds 300 ms past the clip's end.
    let past = Range::new(end, end + sr.samples(0.5));
    let live = soundcraft_mix::render_range(e.session(), past, 1024);
    assert!(live.iter().flatten().any(|v| v.abs() > 1e-3), "the delayed note is heard live");
    e.execute("track.freeze", &json!({"tracks": [t.0]})).unwrap();
    let (audio, frames) = render_of(&e, t);
    assert!(frames > end + offset, "the render runs past the delayed note: {frames} frames, clip to {end}, offset {offset}");
    assert!(heard_after(&audio, end + sr.samples(0.2)), "the delayed note is in the render");
    assert_close(&soundcraft_mix::render_range(e.session(), past, 1024), &live, "frozen vs live past the clip's end");
}

/// A Real-Time Properties delay moves notes within their clip: the mix ends every note at its
/// clip's end (one delayed past it does not play), so the render's tail still starts at the clip's
/// end, and the render matches live either way.
#[test]
fn a_note_delayed_by_real_time_properties_is_in_the_render() {
    let mut e = engine();
    let (t, end) = solo_note(&mut e);
    let sr = e.session().sample_rate;
    // The note (48 ticks before the clip's end) delayed by 24 ticks: on inside the clip, off at
    // its end.
    e.execute("event.midi_rtp", &json!({"tracks": [t.0], "delay": 24})).unwrap();
    let past = Range::new(end - sr.samples(0.1), end + sr.samples(0.5));
    let live = soundcraft_mix::render_range(e.session(), past, 1024);
    assert!(live.iter().flatten().any(|v| v.abs() > 1e-3), "the delayed note is heard live");
    e.execute("track.freeze", &json!({"tracks": [t.0]})).unwrap();
    let (audio, frames) = render_of(&e, t);
    assert!(frames > end, "the render keeps the note's release: {frames}");
    assert!(heard_after(&audio, end - sr.samples(0.05)), "the delayed note is in the render");
    assert_close(&soundcraft_mix::render_range(e.session(), past, 1024), &live, "frozen vs live around the clip's end");
    // Delayed past the clip's end, the note plays neither live nor frozen.
    e.execute("track.unfreeze", &json!({"tracks": [t.0]})).unwrap();
    e.execute("event.midi_rtp", &json!({"tracks": [t.0], "delay": 480})).unwrap();
    assert!(soundcraft_mix::render_range(e.session(), past, 1024).iter().flatten().all(|v| v.abs() < 1e-4), "silent live");
    e.execute("track.freeze", &json!({"tracks": [t.0]})).unwrap();
    assert!(soundcraft_mix::render_range(e.session(), past, 1024).iter().flatten().all(|v| v.abs() < 1e-4), "silent frozen");
}
