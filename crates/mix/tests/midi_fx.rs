//! MIDI effects ahead of an instrument, through the mix: an instrument track's clip played
//! through a chord, a MIDI track's clip arpeggiated and passed to another track's instrument
//! (MIDI to Track), the list changed while a note sounds, and a track without effects untouched.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use soundcraft_dsp::{Plugin, PluginInfo, Transport};
use soundcraft_midi::{Note, Sequence};
use soundcraft_mix::{MixEngine, PreparedInstance, SlotKey, track_midi};
use soundcraft_model::{ChannelFormat, Clip, Insert, MidiFx, MidiFxKind, Route, Session, TrackId, TrackKind};
use soundcraft_time::Samples;

const SR: usize = 48_000;
const BLOCK: usize = 512;
const TAPE: &str = "clap:test.fx_tape";

type Log = Arc<Mutex<Vec<(i64, [u8; 3])>>>;

/// An instrument that logs every note and message at its absolute sample.
struct Tape {
    inner: Box<dyn Plugin>,
    pos: Samples,
    log: Log,
}

impl Plugin for Tape {
    fn info(&self) -> &'static PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
    }
    fn set_transport(&mut self, t: &Transport) {
        self.pos = t.samples;
    }
    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.log.lock().unwrap().push((self.pos + offset as i64, [0x90, note, velocity]));
    }
    fn note_off(&mut self, offset: usize, note: u8) {
        self.log.lock().unwrap().push((self.pos + offset as i64, [0x80, note, 0]));
    }
    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        self.log.lock().unwrap().push((self.pos + offset as i64, msg));
    }
}

fn tape(t: TrackId) -> (PreparedInstance, Log) {
    let log: Log = Arc::default();
    let plugin = Tape { inner: soundcraft_dsp::create("gain").unwrap(), pos: 0, log: Arc::clone(&log) };
    (PreparedInstance { track: t, slot: SlotKey::Instrument, id: TAPE.into(), ch: 2, plugin: Box::new(plugin) }, log)
}

/// A one-second clip with one note of `pitch` from tick 0 for `len` ticks.
fn clip(s: &mut Session, t: TrackId, pitch: u8, len: i64) {
    let id = s.new_clip_id();
    let seq = Sequence {
        notes: vec![Note { probability: 100, start: 0, length: len, pitch, velocity: 100, release_velocity: 0, channel: 0, muted: false }],
        ctrls: Vec::new(),
        sysex: Vec::new(),
    };
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(id, "notes", 0, SR as i64, seq));
}

fn render(eng: &mut MixEngine, s: &Session, blocks: std::ops::Range<usize>) {
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    for b in blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut out);
    }
}

fn engine(instances: Vec<PreparedInstance>) -> MixEngine {
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    eng.set_external_instances(true);
    eng.adopt(instances);
    eng
}

fn ons(log: &Log) -> Vec<(i64, u8)> {
    log.lock().unwrap().iter().filter(|(_, m)| m[0] & 0xF0 == 0x90 && m[2] > 0).map(|(t, m)| (*t, m[1])).collect()
}

fn synth(s: &mut Session) -> TrackId {
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Synth"));
    s.track_mut(t).unwrap().instrument = Some(Insert::new(TAPE));
    s.track_mut(t).unwrap().mixer.output = Route::Main;
    t
}

#[test]
fn a_clip_plays_through_the_tracks_chord_and_a_change_lets_go() {
    let mut s = Session::default();
    let t = synth(&mut s);
    clip(&mut s, t, 60, 1_920);
    // Without effects: the note as written.
    let (inst, log) = tape(t);
    let mut eng = engine(vec![inst]);
    render(&mut eng, &s, 0..4);
    assert_eq!(ons(&log), vec![(0, 60)]);
    // With a minor chord on the track: the chord, at the note's sample.
    let mut s2 = s.clone();
    s2.track_mut(t).unwrap().midi_fx = vec![MidiFx { chord: 1, ..MidiFx::new(MidiFxKind::Chord) }];
    let (inst, log) = tape(t);
    let mut eng = engine(vec![inst]);
    render(&mut eng, &s2, 0..4);
    assert_eq!(ons(&log), vec![(0, 60), (0, 63), (0, 67)]);
    // Taken off while the note sounds: the chord lets go, the clip's note goes on alone.
    log.lock().unwrap().clear();
    render(&mut eng, &s, 4..5);
    let got = log.lock().unwrap().clone();
    let offs: Vec<u8> = got.iter().filter(|(_, m)| m[0] & 0xF0 == 0x80).map(|(_, m)| m[1]).collect();
    assert_eq!(offs.len(), 3, "{got:?}");
    assert_eq!(ons(&log).iter().map(|x| x.1).collect::<Vec<_>>(), vec![60], "{got:?}");
}

#[test]
fn a_midi_tracks_arpeggio_plays_the_instrument_it_is_routed_to() {
    let mut s = Session::default();
    let target = synth(&mut s);
    let a = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, Some("A"));
    clip(&mut s, a, 48, 3_840);
    let mut ins = Insert::new(track_midi::ID);
    ins.params.insert("channel".into(), 2.0);
    ins.set_state_bytes(target.0.to_string().as_bytes());
    s.track_mut(a).unwrap().instrument = Some(ins);
    // 1/8 steps over two octaves while the transport plays: 48, 60, 48, 60 … on the grid.
    s.track_mut(a).unwrap().midi_fx = vec![MidiFx { rate: 1, octaves: 2, ..MidiFx::new(MidiFxKind::Arpeggiator) }];
    let (inst, log) = tape(target);
    let mut eng = engine(vec![inst]);
    render(&mut eng, &s, 0..90);
    let got = log.lock().unwrap().clone();
    let on: Vec<(i64, [u8; 3])> = got.iter().copied().filter(|(_, m)| m[0] & 0xF0 == 0x90).collect();
    let eighth = s.tempo.tick_to_samples(480, s.sample_rate);
    assert!(on.len() >= 4, "{got:?}");
    for (k, (at, m)) in on.iter().take(4).enumerate() {
        assert_eq!(m[0], 0x91, "on the route's channel 2");
        assert_eq!(m[1], if k % 2 == 0 { 48 } else { 60 });
        assert!((at - k as i64 * eighth).abs() <= 1, "step {k} at {at}, not {}", k as i64 * eighth);
    }
}

/// A clip that holds its effects' output (recorded with Record Effects' Output) plays past them;
/// the same clip not marked so plays through them.
#[test]
fn a_printed_clip_plays_past_the_effects() {
    let mut s = Session::default();
    let t = synth(&mut s);
    s.track_mut(t).unwrap().midi_fx = vec![MidiFx::new(MidiFxKind::Chord)];
    clip(&mut s, t, 60, 1_920);
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips[0].midi_fx_printed = true;
    let (inst, log) = tape(t);
    let mut eng = engine(vec![inst]);
    render(&mut eng, &s, 0..4);
    assert_eq!(ons(&log), vec![(0, 60)], "the printed note alone, not its chord");
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips[0].midi_fx_printed = false;
    let (inst, log) = tape(t);
    let mut eng = engine(vec![inst]);
    render(&mut eng, &s, 0..4);
    assert_eq!(ons(&log), vec![(0, 60), (0, 64), (0, 67)]);
}
