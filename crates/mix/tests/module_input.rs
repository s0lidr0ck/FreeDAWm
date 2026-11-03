//! Chain modules' MIDI output as a track input (`Route::Modules`): Shale on an audio track fed a
//! kick-like signal, an instrument track playing its hits. The source renders before the track
//! that plays it whatever the track order; the notes land in the block of the hit; moving the
//! input off the module sends all notes off; a cycle or an aux source plays nothing. (The rings
//! are fixed arrays inside the slots and strips: rendering has nothing to allocate for them.)
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use soundcraft_audio_io::AudioBuffer;
use soundcraft_dsp::{Plugin, PluginInfo, Transport};
use soundcraft_mix::{MixEngine, PreparedInstance, SidechainProblem, SlotKey, module_input_refusal, module_input_routes};
use soundcraft_model::{
    ChannelFormat, Clip, Insert, ModuleId, ModuleRef, Route, Session, SidechainSource, SourceAudio, SourceId, TrackId, TrackKind,
};
use soundcraft_time::Samples;

const SR: usize = 48_000;
const BLOCK: usize = 512;
const TAPE: &str = "clap:test.module_tape";

type Log = Arc<Mutex<Vec<(i64, [u8; 3])>>>;

/// An instrument that logs every `midi_raw` at its absolute sample and counts `all_notes_off`.
struct Tape {
    inner: Box<dyn Plugin>,
    pos: Samples,
    log: Log,
    resets: Arc<Mutex<Vec<i64>>>,
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
    fn all_notes_off(&mut self) {
        self.resets.lock().unwrap().push(self.pos);
    }
}

fn tape(t: TrackId) -> (PreparedInstance, Log, Arc<Mutex<Vec<i64>>>) {
    let log: Log = Arc::default();
    let resets: Arc<Mutex<Vec<i64>>> = Arc::default();
    let plugin = Tape { inner: soundcraft_dsp::create("gain").unwrap(), pos: 0, log: Arc::clone(&log), resets: Arc::clone(&resets) };
    (PreparedInstance { track: t, slot: SlotKey::Instrument, id: TAPE.into(), ch: 2, plugin: Box::new(plugin) }, log, resets)
}

/// A kick-ish hit every `period` samples from `first` (a 60 Hz body decaying in ~80 ms).
fn hits(len: usize, period: usize, first: usize, amp: f32) -> Vec<f32> {
    (0..len)
        .map(|i| {
            if i < first {
                return 0.0;
            }
            let t = ((i - first) % period) as f32 / SR as f32;
            amp * (-t / 0.08).exp() * (std::f32::consts::TAU * 60.0 * t).sin()
        })
        .collect()
}

const PERIOD: usize = 24_000;
const FIRST: usize = 3_000;
const LEN: usize = SR * 2;

/// An audio track playing `signal`.
fn audio_track(s: &mut Session, name: &str, signal: &[f32]) -> TrackId {
    let src = SourceId(1000 + s.tracks.len() as u64);
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR as u32, channels: vec![signal.to_vec(); 2] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some(name));
    let clip = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, name, src, 0, 0, signal.len() as i64));
    tr.mixer.output = Route::Main;
    t
}

/// A Shale set up as a kick mic with MIDI Out on (the default note, C1).
fn shale(s: &mut Session, t: TrackId) -> ModuleId {
    shale_note(s, t, 0.1667)
}

/// [`shale`] with its MIDI Note knob at `note`.
fn shale_note(s: &mut Session, t: TrackId, note: f32) -> ModuleId {
    let mut ins = Insert::new("fs_shale");
    for (id, v) in [("instrument", 0.0), ("midiOut", 1.0), ("midiNote", note)] {
        ins.params.insert(id.into(), v);
    }
    s.track_mut(t).unwrap().mixer.add_module(ins, None).unwrap()
}

fn instrument_track(s: &mut Session, name: &str) -> TrackId {
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(name));
    s.track_mut(t).unwrap().instrument = Some(Insert::new(TAPE));
    t
}

/// The instrument track first (so session order alone would render it before its source), the
/// kick after it with a Shale, the instrument's input the Shale's MIDI.
fn rig() -> (Session, TrackId, TrackId, ModuleId) {
    let mut s = Session::default();
    let drums = instrument_track(&mut s, "Drums");
    let kick = audio_track(&mut s, "Kick", &hits(LEN, PERIOD, FIRST, 0.9));
    let m = shale(&mut s, kick);
    s.track_mut(drums).unwrap().mixer.input = Route::Modules(vec![ModuleRef { track: kick, module: m }]);
    (s, drums, kick, m)
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

fn note_ons(log: &Log) -> Vec<i64> {
    log.lock().unwrap().iter().filter(|(_, m)| m[0] == 0x99 && m[2] > 0).map(|(at, _)| *at).collect()
}

#[test]
fn the_instrument_plays_a_note_in_the_block_of_each_hit_whatever_the_track_order() {
    let (s, drums, kick, m) = rig();
    let routes = module_input_routes(&s);
    assert_eq!(routes.len(), 1);
    assert_eq!((routes[0].track, routes[0].source, routes[0].module, routes[0].result), (drums, kick, m, Ok(())));
    let (inst, log, _) = tape(drums);
    let mut eng = engine(vec![inst]);
    render(&mut eng, &s, 0..LEN / BLOCK);
    let ons = note_ons(&log);
    assert_eq!(ons.len(), 4, "{ons:?}");
    for (k, on) in ons.iter().enumerate() {
        let hit = (FIRST + k * PERIOD) as i64;
        // Within one block of the hit (the gate's lookahead and onset detection are inside it).
        assert!(*on >= hit && *on < hit + BLOCK as i64, "note-on {on} for the hit at {hit}");
    }
    let all = log.lock().unwrap().clone();
    assert!(all.iter().all(|(_, m)| m[0] & 0x0F == 9), "channel 10 kept: {all:?}");
    assert_eq!(all.iter().filter(|(_, m)| m[0] == 0x89).count(), 4, "a note-off per hit");
    // Rendering again from the start plays the same notes: nothing was left behind.
    render(&mut eng, &s, 0..LEN / BLOCK);
    assert_eq!(note_ons(&log).len(), 8);
}

#[test]
fn moving_the_input_off_the_module_closes_its_notes() {
    for next in [Route::None, Route::Musician(0)] {
        let (s, drums, _, _) = rig();
        let (inst, log, resets) = tape(drums);
        let mut eng = engine(vec![inst]);
        // Stop right after a hit, while its note sounds.
        let upto = (FIRST + 300) / BLOCK + 1;
        render(&mut eng, &s, 0..upto);
        assert_eq!(note_ons(&log).len(), 1);
        let before = resets.lock().unwrap().len();
        let mut moved = s.clone();
        moved.track_mut(drums).unwrap().mixer.input = next.clone();
        render(&mut eng, &moved, upto..upto + 1);
        assert_eq!(resets.lock().unwrap().len(), before + 1, "{next:?}: all notes off when the input moves");
        render(&mut eng, &moved, upto + 1..upto + 2);
        assert_eq!(resets.lock().unwrap().len(), before + 1, "staying put changes nothing");
    }
}

#[test]
fn midi_out_off_plays_nothing_and_a_missing_source_is_silent() {
    let (mut s, drums, kick, m) = rig();
    s.track_mut(kick).unwrap().mixer.module_mut(m).unwrap().params.insert("midiOut".into(), 0.0);
    let (inst, log, _) = tape(drums);
    let mut eng = engine(vec![inst]);
    render(&mut eng, &s, 0..LEN / BLOCK);
    assert!(log.lock().unwrap().is_empty(), "MIDI Out off: no events");
    // The module gone: the route stays, nothing plays, nothing crashes.
    s.track_mut(kick).unwrap().mixer.remove_module(m).unwrap();
    assert_eq!(module_input_routes(&s)[0].result, Err(SidechainProblem::Missing));
    render(&mut eng, &s, 0..8);
    assert!(log.lock().unwrap().is_empty());
    // The track gone too.
    s.tracks.retain(|t| t.id != kick);
    render(&mut eng, &s, 0..8);
    assert!(log.lock().unwrap().is_empty());
}

#[test]
fn cycles_and_aux_sources_are_refused_and_play_nothing() {
    let (mut s, drums, kick, m) = rig();
    // Kick's Shale keyed from Drums while Drums plays Kick's Shale: the later route (the module
    // input, resolved after the sidechains) is the one that loses.
    s.track_mut(kick).unwrap().mixer.module_mut(m).unwrap().sidechain = Some(SidechainSource { track: drums, pre_fader: false });
    assert_eq!(module_input_routes(&s)[0].result, Err(SidechainProblem::Cycle));
    let why = module_input_refusal(&s, drums, &Route::Modules(vec![ModuleRef { track: kick, module: m }])).unwrap();
    assert!(why.contains("feed a track into itself"), "{why}");
    let (inst, log, _) = tape(drums);
    let mut eng = engine(vec![inst]);
    render(&mut eng, &s, 0..LEN / BLOCK);
    assert!(log.lock().unwrap().is_empty(), "a cycle plays nothing");
    // A module on an aux can't feed a track (auxes render after it).
    let mut s = Session::default();
    let drums = instrument_track(&mut s, "Drums");
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Drum Bus"));
    let m = shale(&mut s, aux);
    let want = Route::Modules(vec![ModuleRef { track: aux, module: m }]);
    let why = module_input_refusal(&s, drums, &want).unwrap();
    assert!(why.contains("aux"), "{why}");
    // Feeding itself.
    let m2 = shale(&mut s, drums);
    let why = module_input_refusal(&s, drums, &Route::Modules(vec![ModuleRef { track: drums, module: m2 }])).unwrap();
    assert!(why.contains("feed a track into itself"), "{why}");
    // A plain source is fine, an inactive one too (it may come back).
    let kick = audio_track(&mut s, "Kick", &[0.0; 64]);
    let m3 = shale(&mut s, kick);
    assert_eq!(module_input_refusal(&s, drums, &Route::Modules(vec![ModuleRef { track: kick, module: m3 }])), None);
    s.track_mut(kick).unwrap().inactive = true;
    assert_eq!(module_input_refusal(&s, drums, &Route::Modules(vec![ModuleRef { track: kick, module: m3 }])), None);
}

#[test]
fn three_shales_feed_one_instrument_in_order_and_leave_one_at_a_time() {
    use soundcraft_dsp::plugins::fs::shale::shale_dsp::knob_to_midi_note;
    // Kick, snare and hats: the same hits (the snare 100 samples late), three notes, in a set
    // listed hats first, so source order (not track order) decides ties.
    let mut s = Session::default();
    let drums = instrument_track(&mut s, "Drums");
    let kick = audio_track(&mut s, "Kick", &hits(LEN, PERIOD, FIRST, 0.9));
    let snare = audio_track(&mut s, "Snare", &hits(LEN, PERIOD, FIRST + 100, 0.9));
    let hats = audio_track(&mut s, "Hats", &hits(LEN, PERIOD, FIRST, 0.9));
    let (km, sm, hm) = (shale_note(&mut s, kick, 0.1667), shale_note(&mut s, snare, 0.2), shale_note(&mut s, hats, 0.25));
    let notes = [knob_to_midi_note(0.25) as u8, knob_to_midi_note(0.1667) as u8, knob_to_midi_note(0.2) as u8];
    let set = vec![ModuleRef { track: hats, module: hm }, ModuleRef { track: kick, module: km }, ModuleRef { track: snare, module: sm }];
    s.track_mut(drums).unwrap().mixer.input = Route::Modules(set.clone());
    assert!(module_input_routes(&s).iter().all(|r| r.result.is_ok()), "{:?}", module_input_routes(&s));
    let (inst, log, resets) = tape(drums);
    let mut eng = engine(vec![inst]);
    let upto = (FIRST + 400) / BLOCK + 1;
    render(&mut eng, &s, 0..upto);
    let ons: Vec<(i64, u8)> = log.lock().unwrap().iter().filter(|(_, m)| m[0] == 0x99 && m[2] > 0).map(|(at, m)| (*at, m[1])).collect();
    assert_eq!(ons.len(), 3, "{ons:?}");
    assert_eq!(ons.iter().map(|(_, n)| *n).collect::<Vec<_>>(), notes, "hats and kick at the same offset in set order, the snare after");
    assert!(ons[0].0 == ons[1].0 && ons[1].0 < ons[2].0, "{ons:?}");
    assert!(ons.iter().all(|(at, _)| *at >= FIRST as i64 && *at < (FIRST + 100 + BLOCK) as i64), "{ons:?}");
    // The kick leaves the set while all three notes sound: only its note closes, no reset.
    let before = log.lock().unwrap().len();
    let mut moved = s.clone();
    moved.track_mut(drums).unwrap().mixer.input = Route::Modules(vec![set[0], set[2]]);
    render(&mut eng, &moved, upto..upto + 1);
    let new: Vec<[u8; 3]> = log.lock().unwrap().iter().skip(before).map(|(_, m)| *m).collect();
    assert_eq!(new, vec![[0x89, notes[1], 0]], "the kick's note-off alone");
    assert!(resets.lock().unwrap().is_empty(), "no all-notes-off for a set change");
    // Later hits: the kick is gone, the other two play on.
    render(&mut eng, &moved, upto + 1..LEN / BLOCK);
    let later: Vec<u8> = log.lock().unwrap().iter().skip(before + 1).filter(|(_, m)| m[0] == 0x99).map(|(_, m)| m[1]).collect();
    assert!(!later.is_empty() && later.iter().all(|n| *n != notes[1]), "{later:?}");
    // Off the modules altogether: all notes off.
    let mut off = moved.clone();
    off.track_mut(drums).unwrap().mixer.input = Route::None;
    render(&mut eng, &off, LEN / BLOCK..LEN / BLOCK + 1);
    assert_eq!(resets.lock().unwrap().len(), 1);
}
