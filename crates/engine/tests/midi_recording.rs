//! Recording MIDI (parity section 6): what a track listens to and what goes in and out of files.

use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_midi::{Note, Sequence};
use soundcraft_model::{Clip, MidiIn, Route, TrackId};

fn new_track(e: &mut Engine, kind: &str) -> TrackId {
    let id = e.execute("track.new", &json!({"kind": kind})).unwrap()["tracks"][0].as_u64().unwrap();
    TrackId(id)
}

fn note(pitch: u8, start: i64, length: i64) -> Note {
    Note { probability: 100, pitch, velocity: 100, muted: false, release_velocity: 64, channel: 0, start, length }
}

/// Bug 20: a new MIDI track, and the tracks a MIDI file import makes, listen to every MIDI input,
/// as a new instrument track does (they used to record nothing until an input was chosen).
#[test]
fn new_and_imported_midi_tracks_listen_to_every_input() {
    let mut e = Engine::default();
    for kind in ["midi", "instrument"] {
        let t = new_track(&mut e, kind);
        assert_eq!(e.session().track(t).unwrap().mixer.input, Route::Midi(MidiIn::all()), "{kind}");
    }
    let t = new_track(&mut e, "instrument");
    let mut seq = Sequence::default();
    seq.notes.push(note(60, 0, 960));
    let id = e.session_mut().new_clip_id();
    e.session_mut().track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(id, "m", 0, 48_000, seq));
    let bytes = soundcraft_engine::io::export_midi(&e, Some(&[t])).unwrap();
    let mut fresh = Engine::default();
    let made = soundcraft_engine::io::import_midi_bytes(&mut fresh, &bytes, 0, false).unwrap();
    assert!(!made.is_empty());
    for t in made {
        assert_eq!(fresh.session().track(t).unwrap().mixer.input, Route::Midi(MidiIn::all()));
    }
}

/// Bug 21: MIDI export writes what plays: a clip's trimmed window (a note running past its end
/// ends there) and no muted clip.
#[test]
fn midi_export_keeps_trims_and_leaves_muted_clips_out() {
    let mut e = Engine::default();
    let t = new_track(&mut e, "instrument");
    let mut seq = Sequence::default();
    // A quarter is 960 ticks, 24,000 samples at 120 bpm.
    seq.notes.extend([note(60, 0, 960), note(62, 960, 1920), note(64, 2880, 960)]);
    let id = e.session_mut().new_clip_id();
    // Two quarters long: the second note runs past its end, the third is outside it.
    e.session_mut().track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(id, "m", 0, 48_000, seq.clone()));
    let id2 = e.session_mut().new_clip_id();
    let mut muted = Clip::midi(id2, "muted", 96_000, 96_000, seq);
    muted.muted = true;
    e.session_mut().track_mut(t).unwrap().playlist_mut().unwrap().clips.push(muted);
    let bytes = soundcraft_engine::io::export_midi(&e, Some(&[t])).unwrap();
    let smf = soundcraft_midi::read_smf(&bytes).unwrap();
    let notes: Vec<(u8, i64, i64)> = smf.tracks.iter().flat_map(|tr| tr.sequence.notes.iter().map(|n| (n.pitch, n.start, n.length))).collect();
    let q = i64::from(soundcraft_midi::PPQ);
    assert_eq!(notes, vec![(60, 0, q), (62, q, q)]);
}

/// Bug 19: MIDI Real-Time Properties changes are an undo step.
#[test]
fn midi_real_time_properties_can_be_undone() {
    let mut e = Engine::default();
    let t = new_track(&mut e, "instrument");
    e.execute("event.midi_rtp", &json!({"tracks": [t.0], "transpose": 5})).unwrap();
    assert_eq!(e.session().edit.value(&format!("rtp.{}.transpose", t.0), 0.0), 5.0);
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(e.session().edit.value(&format!("rtp.{}.transpose", t.0), 0.0), 0.0);
}

/// Bug 13: module MIDI recorded in a loop prints over the loop, from the punch point when Record
/// was pressed inside it, not up to wherever the playhead was at Stop.
#[test]
fn module_midi_in_a_loop_prints_over_the_loop() {
    use soundcraft_engine::io::Recording;
    use soundcraft_engine::module_record::print_range;
    use soundcraft_time::Range;
    let plan =
        |start, pass, loop_from, punch| Recording { start, pass, punch, rate: 0, takes: Vec::new(), latency: 0, loop_from, punches: Vec::new() };
    // Loop record over 0..4000, stopped at 500 in a later pass.
    assert_eq!(print_range(&plan(0, Some(4_000), Some(0), None), 500), Some(Range::new(0, 4_000)));
    // Punched in at 1000 inside the playing loop.
    assert_eq!(print_range(&plan(1_000, Some(4_000), Some(0), None), 500), Some(Range::new(1_000, 4_000)));
    // No loop: where recording started to where it stopped, cut to the punch.
    assert_eq!(print_range(&plan(0, None, None, Some(Range::new(100, 300))), 500), Some(Range::new(100, 300)));
    assert_eq!(print_range(&plan(600, None, None, None), 500), None);
}
