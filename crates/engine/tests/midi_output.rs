//! MIDI output: a track whose instrument is External MIDI sends its clips' notes to its output
//! port at the time the block they are in will be heard, and a stop lets go of them.

use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_midi::{Note, Sequence};
use soundcraft_mix::MixEngine;
use soundcraft_mix::external_midi::{OutMsg, hub};
use soundcraft_model::{Clip, TrackId};

const BLOCK: usize = 480;

#[test]
fn an_external_midi_track_plays_its_clip_out_of_its_port() {
    let mut e = Engine::default();
    let t = TrackId(e.execute("track.new", &json!({"kind": "midi"})).unwrap()["tracks"][0].as_u64().unwrap());
    e.execute("mix.midi_output", &json!({"track": t.0, "port": "Render Test Synth", "channel": 2})).unwrap();
    // A quarter note at 120 bpm is 24,000 samples; the note starts on beat 2.
    let mut seq = Sequence::default();
    seq.notes.push(Note { probability: 100, pitch: 48, velocity: 100, muted: false, release_velocity: 64, channel: 0, start: 960, length: 960 });
    // A SysEx on the note's tick leaves before it; one hidden by nothing at tick 0 goes first.
    let reset = [0xF0, 0x7E, 0x7F, 0x09, 0x01, 0xF7];
    let patch = [0xF0, 0x43, 0x10, 0x4C, 0x02, 0x01, 0x00, 0x01, 0xF7];
    seq.sysex = vec![soundcraft_midi::SysEx::new(0, &reset).unwrap(), soundcraft_midi::SysEx::new(960, &patch).unwrap()];
    let id = e.session_mut().new_clip_id();
    soundcraft_engine::edit::place_clip(e.session_mut(), t, Clip::midi(id, "Bass", 0, 96_000, seq));
    let h = hub();
    let slot = h.open("Render Test Synth").unwrap();
    let mut eng = Box::new(MixEngine::new(48_000.0, BLOCK));
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    let s = e.session_arc();
    let mut got = Vec::new();
    for b in 0..60 {
        h.begin_block(1_000_000 + b as u64 * 10_000);
        eng.render(&s, (b * BLOCK) as i64, BLOCK, &mut out);
        while let Some(m) = h.pop(slot) {
            got.push(m);
        }
    }
    // Block 50 starts at sample 24,000: the note-on, on channel 2, at that block's heard time.
    let notes: Vec<_> = got.iter().filter(|m| matches!(m.1, OutMsg::Short(s) if s[0] & 0xE0 == 0x80)).collect();
    assert_eq!(notes, vec![&(1_500_000, OutMsg::Short([0x91, 48, 100]))], "{got:?}");
    // The SysEx whole, at its ticks' heard times, the one on the note's tick before it.
    let sysex: Vec<_> = got.iter().filter(|m| matches!(m.1, OutMsg::SysEx(_))).collect();
    assert_eq!(sysex, vec![&(1_000_000, OutMsg::SysEx(reset.to_vec())), &(1_500_000, OutMsg::SysEx(patch.to_vec()))], "{got:?}");
    let at = |m: &OutMsg| got.iter().position(|g| g.1 == *m);
    assert!(at(&OutMsg::SysEx(patch.to_vec())) < at(&OutMsg::Short([0x91, 48, 100])));
    assert!(out.iter().all(|c| c.iter().all(|x| *x == 0.0)), "External MIDI makes no sound itself");
    // A locate (a stop and play elsewhere) lets go of it.
    let mut after = Vec::new();
    eng.render(&s, 200_000, BLOCK, &mut out);
    while let Some((_, OutMsg::Short(m))) = h.pop(slot) {
        after.push(m);
    }
    assert_eq!(after.first(), Some(&[0x81, 48, 0]), "{after:?}");
    assert!(after.contains(&[0xB1, 123, 0]));
    h.close(slot);
}
