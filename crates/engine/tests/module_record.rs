//! Recording through a module MIDI input: Shale on a kick track, an instrument track armed with
//! its input from it, a record pass over the kick → a MIDI clip with the hits.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use serde_json::json;
use soundcraft_audio_io::AudioBuffer;
use soundcraft_engine::Engine;
use soundcraft_engine::module_record::{record_module_inputs, render_module_midi};
use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackId, TrackKind};
use soundcraft_time::Range;

const SR: usize = 48_000;
const PERIOD: usize = 24_000;
const FIRST: usize = 3_000;

fn hits(len: usize) -> Vec<f32> {
    (0..len)
        .map(|i| {
            if i < FIRST {
                return 0.0;
            }
            let t = ((i - FIRST) % PERIOD) as f32 / SR as f32;
            0.9 * (-t / 0.08).exp() * (std::f32::consts::TAU * 60.0 * t).sin()
        })
        .collect()
}

/// "Kick" (audio, two seconds of hits, a Shale with MIDI Out on) and "Drums" (instrument, armed,
/// input from the Shale).
fn rig() -> (Engine, TrackId, TrackId) {
    let mut e = Engine::default();
    let signal = hits(SR * 2);
    let s = e.session_mut();
    let src = SourceId(1000);
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR as u32, channels: vec![signal.clone(); 2] })));
    let kick = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Kick"));
    let clip = s.new_clip_id();
    s.track_mut(kick).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "Kick", src, 0, 0, signal.len() as i64));
    let drums = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Drums"));
    e.execute("mix.module_add", &json!({"track": kick.0, "plugin": "fs_shale"})).unwrap();
    let m = e.session().track(kick).unwrap().mixer.inserts[0].id;
    e.execute("mix.module_param", &json!({"track": kick.0, "module": m.0, "param": "midiOut", "value": 1.0})).unwrap();
    e.execute("mix.module_param", &json!({"track": kick.0, "module": m.0, "param": "instrument", "value": 0.0})).unwrap();
    e.execute("mix.instrument", &json!({"track": drums.0, "plugin": "subtractive_synth"})).unwrap();
    e.execute("track.input", &json!({"tracks": [drums.0], "input": "module:Kick/Shale"})).unwrap();
    e.execute("mix.record_arm", &json!({"tracks": [drums.0], "value": true})).unwrap();
    (e, kick, drums)
}

#[test]
fn a_record_pass_through_a_module_input_makes_a_midi_clip_with_the_hits() {
    let (mut e, _kick, drums) = rig();
    let range = Range::new(0, (SR * 2) as i64);
    let events = render_module_midi(e.session(), range, &[drums]);
    let ons: Vec<i64> = events[0].1.iter().filter(|(_, m)| m[0] == 0x99).map(|(at, _)| *at).collect();
    assert_eq!(ons.len(), 4, "{ons:?}");
    for (k, on) in ons.iter().enumerate() {
        let hit = (FIRST + k * PERIOD) as i64;
        assert!(*on >= hit && *on < hit + 512, "note-on {on} for the hit at {hit}");
    }
    let undo = e.undo_history().len();
    let ids = record_module_inputs(&mut e, range, None);
    assert_eq!(ids.len(), 1);
    let t = e.session().track(drums).unwrap();
    let clip = t.clips().iter().find(|c| c.id == ids[0]).unwrap();
    assert_eq!((clip.start, clip.length), (0, range.len()));
    assert!(clip.midi_channels, "channels kept");
    let seq = clip.midi_playing().unwrap();
    assert_eq!(seq.notes.len(), 4, "{:?}", seq.notes);
    let s = e.session();
    for (k, n) in seq.notes.iter().enumerate() {
        let at = s.tempo.tick_to_samples(n.start, s.sample_rate);
        let hit = (FIRST + k * PERIOD) as i64;
        assert!(at >= hit - 10 && at < hit + 512, "note {k} at {at}, hit at {hit}");
        assert_eq!(n.channel, 9, "channel 10");
        assert!(n.velocity > 0 && n.length > 0);
    }
    assert_eq!(e.undo_history().len(), undo + 1, "one undo step");
    assert_eq!(e.undo_label(), Some("Record"));
    e.undo();
    assert!(e.session().track(drums).unwrap().clips().is_empty());
}

#[test]
fn print_input_command_and_its_refusals() {
    let (mut e, kick, drums) = rig();
    let out = e.execute("track.print_input", &json!({"tracks": [drums.0], "range": {"start": 0, "end": SR}})).unwrap();
    assert_eq!(out["clips"][0]["notes"], json!(2), "{out}");
    // A silent range, a track with another input, an audio track.
    let err = e.execute("track.print_input", &json!({"tracks": [drums.0], "range": {"start": SR * 3, "end": SR * 4}})).unwrap_err().to_string();
    assert!(err.contains("Nothing to print"), "{err}");
    let err = e.execute("track.print_input", &json!({"tracks": [kick.0], "range": {"start": 0, "end": SR}})).unwrap_err().to_string();
    assert!(err.contains("not a module's MIDI"), "{err}");
    // Not armed: Record's own pass skips it, the command does not.
    e.execute("mix.record_arm", &json!({"tracks": [drums.0], "value": false})).unwrap();
    assert!(record_module_inputs(&mut e, Range::new(0, SR as i64), None).is_empty());
    assert_eq!(record_module_inputs(&mut e, Range::new(0, SR as i64), Some(&[drums])).len(), 1);
}
