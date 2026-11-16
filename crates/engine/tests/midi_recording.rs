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
    Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start, length }
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

/// Bug 13: module MIDI recorded in a loop prints over the loop, from the punch point when Record
/// was pressed inside it, not up to wherever the playhead was at Stop.
#[test]
fn module_midi_in_a_loop_prints_over_the_loop() {
    use soundcraft_engine::io::Recording;
    use soundcraft_engine::module_record::print_range;
    use soundcraft_time::Range;
    let plan = |start, pass, loop_from, punch| Recording { start, pass, punch, rate: 0, takes: Vec::new(), latency: 0, loop_from };
    // Loop record over 0..4000, stopped at 500 in a later pass.
    assert_eq!(print_range(&plan(0, Some(4_000), Some(0), None), 500), Some(Range::new(0, 4_000)));
    // Punched in at 1000 inside the playing loop.
    assert_eq!(print_range(&plan(1_000, Some(4_000), Some(0), None), 500), Some(Range::new(1_000, 4_000)));
    // No loop: where recording started to where it stopped, cut to the punch.
    assert_eq!(print_range(&plan(0, None, None, Some(Range::new(100, 300))), 500), Some(Range::new(100, 300)));
    assert_eq!(print_range(&plan(600, None, None, None), 500), None);
}
