//! MIDI tracks playing an instrument on another track (MIDI to Track): two MIDI tracks on
//! channels 1 and 2 play one multi-timbral instrument track, merged with its own clip, at the
//! samples their notes are on (the sources render first whatever the track order); a muted source
//! lets go of its note and starts none; a route taken off lets go of what it held; a loop is
//! refused and plays nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use soundcraft_dsp::{Plugin, PluginInfo, Transport};
use soundcraft_midi::{Note, Sequence};
use soundcraft_mix::{INSTRUMENT_OUT, MixEngine, PreparedInstance, SidechainProblem, SlotKey, module_input_routes, track_midi};
use soundcraft_model::{ChannelFormat, Clip, Insert, Route, Session, TrackId, TrackKind};
use soundcraft_time::Samples;

const SR: usize = 48_000;
const BLOCK: usize = 512;
const TAPE: &str = "clap:test.routed_tape";

type Log = Arc<Mutex<Vec<(i64, [u8; 3])>>>;

/// An instrument that logs every `midi_raw` at its absolute sample.
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
    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        self.log.lock().unwrap().push((self.pos + offset as i64, msg));
    }
}

fn tape(t: TrackId) -> (PreparedInstance, Log) {
    let log: Log = Arc::default();
    let plugin = Tape { inner: soundcraft_dsp::create("gain").unwrap(), pos: 0, log: Arc::clone(&log) };
    (PreparedInstance { track: t, slot: SlotKey::Instrument, id: TAPE.into(), ch: 2, plugin: Box::new(plugin) }, log)
}

/// A clip over the first second with one note of `pitch` from `tick` for `len` ticks.
fn clip_with(s: &mut Session, t: TrackId, pitch: u8, tick: i64, len: i64, channels: bool) {
    let id = s.new_clip_id();
    let seq = Sequence {
        notes: vec![Note { start: tick, length: len, pitch, velocity: 100, release_velocity: 0, channel: 0, muted: false }],
        ctrls: Vec::new(),
    };
    let mut c = Clip::midi(id, "notes", 0, SR as i64, seq);
    c.midi_channels = channels;
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(c);
}

fn to_track(target: TrackId, channel: f32) -> Insert {
    let mut ins = Insert::new(track_midi::ID);
    ins.params.insert("channel".into(), channel);
    ins.set_state_bytes(target.0.to_string().as_bytes());
    ins
}

/// The synth first (session order alone would render it before its sources), then MIDI tracks A
/// (pitch 60, channel 1) and B (pitch 64, channel 2) routed to it; the synth's own clip plays 67
/// on channel 3.
fn rig() -> (Session, TrackId, TrackId, TrackId) {
    let mut s = Session::default();
    let synth = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Synth"));
    s.track_mut(synth).unwrap().instrument = Some(Insert::new(TAPE));
    s.track_mut(synth).unwrap().mixer.output = Route::Main;
    clip_with(&mut s, synth, 67, 0, 960, false);
    if let Some(c) = s.track_mut(synth).unwrap().playlist_mut().unwrap().clips.first_mut()
        && let soundcraft_model::ClipContent::Midi { sequence } = &mut c.content
    {
        sequence.notes[0].channel = 2;
        c.midi_channels = true;
    }
    let a = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, Some("A"));
    let b = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, Some("B"));
    clip_with(&mut s, a, 60, 40, 960, false);
    clip_with(&mut s, b, 64, 80, 4_000, false);
    s.track_mut(a).unwrap().instrument = Some(to_track(synth, 1.0));
    s.track_mut(b).unwrap().instrument = Some(to_track(synth, 2.0));
    (s, synth, a, b)
}

fn engine(instances: Vec<PreparedInstance>) -> MixEngine {
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    eng.set_external_instances(true);
    eng.adopt(instances);
    eng
}

fn render(eng: &mut MixEngine, s: &Session, blocks: std::ops::Range<usize>) {
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    for b in blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut out);
        assert!(out.iter().flatten().all(|x| x.is_finite()));
    }
}

fn at(s: &Session, tick: i64) -> i64 {
    s.tempo.tick_to_samples(tick, s.sample_rate)
}

#[test]
fn two_midi_tracks_play_one_instrument_each_on_its_channel() {
    let (s, synth, a, b) = rig();
    let routes: Vec<_> = module_input_routes(&s).into_iter().filter(|r| r.module == INSTRUMENT_OUT).collect();
    assert_eq!(routes.len(), 2);
    assert!(routes.iter().all(|r| r.track == synth && r.result.is_ok()), "{routes:?}");
    let (inst, log) = tape(synth);
    let mut eng = engine(vec![inst]);
    render(&mut eng, &s, 0..60);
    let ons: Vec<(i64, [u8; 3])> = log.lock().unwrap().iter().copied().filter(|(_, m)| m[0] & 0xF0 == 0x90).collect();
    // In the block of each note, at its sample: the sources rendered before the synth.
    assert!(ons.contains(&(0, [0x92, 67, 100])), "its own clip: {ons:?}");
    assert!(ons.contains(&(at(&s, 40), [0x90, 60, 100])), "A on channel 1: {ons:?}");
    assert!(ons.contains(&(at(&s, 80), [0x91, 64, 100])), "B on channel 2: {ons:?}");
    let offs = log.lock().unwrap().iter().filter(|(_, m)| m[0] == 0x80 && m[1] == 60).count();
    assert_eq!(offs, 1, "A's note ends");
    let _ = (a, b);
}

#[test]
fn a_muted_source_lets_go_and_a_route_taken_off_too() {
    let (s, synth, a, b) = rig();
    let (inst, log) = tape(synth);
    let mut eng = engine(vec![inst]);
    // B's long note is sounding by block 10.
    render(&mut eng, &s, 0..10);
    assert!(log.lock().unwrap().iter().any(|(_, m)| *m == [0x91, 64, 100]));
    let mut muted = s.clone();
    muted.track_mut(b).unwrap().mixer.mute = true;
    render(&mut eng, &muted, 10..12);
    assert!(log.lock().unwrap().iter().any(|(_, m)| *m == [0x81, 64, 0]), "B's note lets go when B is muted");
    // A rendered again from the start, its route then taken off while its note sounds.
    log.lock().unwrap().clear();
    render(&mut eng, &s, 0..3);
    assert!(log.lock().unwrap().iter().any(|(_, m)| *m == [0x90, 60, 100]));
    let mut off = s.clone();
    off.track_mut(a).unwrap().instrument = None;
    off.track_mut(b).unwrap().mixer.mute = true;
    render(&mut eng, &off, 3..4);
    assert!(log.lock().unwrap().iter().any(|(_, m)| *m == [0x80, 60, 0]), "A's note lets go: {:?}", log.lock().unwrap());
}

#[test]
fn a_loop_is_refused_and_plays_nothing() {
    let (mut s, synth, a, _) = rig();
    // The synth's own output played back into A: a loop.
    s.track_mut(synth).unwrap().instrument = Some(to_track(a, 0.0));
    let routes = module_input_routes(&s);
    assert!(routes.iter().any(|r| r.result == Err(SidechainProblem::Cycle)), "{routes:?}");
    let mut eng = engine(Vec::new());
    render(&mut eng, &s, 0..20);
}
