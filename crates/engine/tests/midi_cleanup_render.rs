//! Playback and renders play the cleaned notes (or the original when the switch says so).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use soundcraft_model::{ChannelFormat, ChordEvent, ClipId, Insert, TrackId, TrackKind};
use soundcraft_time::Range;

const SR: i64 = 48_000;

fn note(pitch: u8, velocity: u8, start: i64, length: i64) -> Note {
    Note { pitch, velocity, release_velocity: 64, channel: 0, start, length }
}

/// An instrument track with a synth and one MIDI clip at 0 (4 s at the default 120 bpm, so a beat
/// is 0.5 s), key C major with the given chords `(tick, length)` on the C and F slots.
fn setup(seq: Sequence, chords: &[(i64, i64, u8)]) -> (Engine, TrackId, ClipId) {
    let mut e = Engine::default();
    let s = e.session_mut();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    s.track_mut(t).unwrap().instrument = Some(Insert::new("subtractive_synth"));
    let id = s.new_clip_id();
    let len = s.sample_rate.samples(4.0);
    soundcraft_engine::edit::place_clip(s, t, soundcraft_model::Clip::midi(id, "take", 0, len, seq));
    s.band.key = 0;
    s.band.scale = 0;
    for (k, (tick, length, slot)) in chords.iter().enumerate() {
        s.band.chords.push(ChordEvent { id: k as u32 + 1, tick: *tick, length_ticks: *length, slot: *slot, bass: -1 });
    }
    s.band.next_id = chords.len() as u32 + 1;
    (e, t, id)
}

/// The track rendered from the start, then cut to `from..to` s (a render starting inside a note
/// does not sound it, so windows are cut from a render that includes the note-on).
fn render(e: &Engine, t: TrackId, from: f64, to: f64) -> Vec<Vec<f32>> {
    let all = soundcraft_mix::render_track_pre_fader(&e.session_arc(), t, Range::new(0, (to * SR as f64) as i64));
    let a = (from * SR as f64) as usize;
    all.into_iter().map(|c| c[a.min(c.len())..].to_vec()).collect()
}

fn peak(b: &[Vec<f32>]) -> f32 {
    b.iter().flatten().fold(0.0, |m, x| m.max(x.abs()))
}

fn rms(b: &[Vec<f32>]) -> f32 {
    let n = b.iter().map(Vec::len).sum::<usize>().max(1);
    (b.iter().flatten().map(|x| x * x).sum::<f32>() / n as f32).sqrt()
}

#[test]
fn cleaned_clip_plays_cleaned_notes() {
    // A held C triad over beat 0..1 and a short quiet C#5 alone at beat 2 (tick 1920, 1.0 s).
    let seq = Sequence { notes: vec![note(60, 90, 0, 900), note(64, 90, 0, 900), note(67, 90, 0, 900), note(73, 20, 1920, 60)], ctrls: Vec::new() };
    let (mut e, t, id) = setup(seq, &[(0, 3840, 0)]);
    e.execute("midi.cleanup", &json!({"clip": id.0, "wrong": 100, "protect_melody": false})).unwrap();
    // 1.0 s to 1.5 s: only the C# is (or was) sounding.
    assert!(peak(&render(&e, t, 1.0, 1.5)) < 1e-4, "the wrong note is gone");
    e.execute("midi.cleanup", &json!({"clip": id.0, "original": true})).unwrap();
    assert!(peak(&render(&e, t, 1.0, 1.5)) > 1e-3, "the original still has it");
}

#[test]
fn pedal_edits_reach_the_instrument() {
    // C for the first bar (3840 ticks = 2 s), then F; E (a C-chord tone, foreign to F) is held
    // 0..4800 under the pedal across the change at 2 s.
    let seq = Sequence {
        notes: vec![note(64, 90, 0, 4800)],
        ctrls: vec![
            CtrlEvent { tick: 0, channel: 0, kind: CtrlKind::Cc(64), value: 127 },
            CtrlEvent { tick: 5000, channel: 0, kind: CtrlKind::Cc(64), value: 0 },
        ],
    };
    let (mut e, t, id) = setup(seq, &[(0, 3840, 0), (3840, 3840, 3)]);
    e.execute("midi.cleanup", &json!({"clip": id.0, "wrong": 0, "held": 100, "protect_melody": false})).unwrap();
    let cleaned = rms(&render(&e, t, 2.1, 2.4));
    e.execute("midi.cleanup", &json!({"clip": id.0, "original": true})).unwrap();
    let original = rms(&render(&e, t, 2.1, 2.4));
    assert!(original > 1e-3, "the held E rings in the original ({original})");
    assert!(cleaned < 0.5 * original, "cleaned {cleaned} vs original {original}");
}

#[test]
fn no_cleanup_renders_identically() {
    let seq = Sequence { notes: vec![note(60, 90, 0, 900), note(73, 20, 1920, 60)], ctrls: Vec::new() };
    let (mut e, t, id) = setup(seq, &[(0, 3840, 0)]);
    let plain = render(&e, t, 0.0, 2.0);
    e.execute("midi.cleanup", &json!({"clip": id.0, "wrong": 0, "held": 0, "legato": 0, "extend": 0})).unwrap();
    let zero = render(&e, t, 0.0, 2.0);
    assert_eq!(plain, zero);
    assert!(peak(&plain) > 1e-3);
}
