//! A frozen track plays its frozen render in place of its clips and module chain: the chain is
//! not processed (a counting stub sees no calls), while the fader, pan and mute stay live.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_dsp::{Category, Plugin, PluginInfo};
use soundcraft_mix::{MixEngine, PreparedInstance, SlotKey};
use soundcraft_model::{ChannelFormat, Clip, FrozenAudio, Insert, ModuleId, Route, Session, SourceAudio, SourceId, TrackId, TrackKind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const SR: usize = 48_000;
const BLOCK: usize = 512;
const LEN: usize = 16 * BLOCK;
const STUB_ID: &str = "clap:test.counting_half";
const CLIP: SourceId = SourceId(31);
const FROZEN: SourceId = SourceId(32);

static STUB_INFO: PluginInfo = PluginInfo {
    id: STUB_ID,
    name: "Counting Half",
    short_name: "Count",
    category: Category::Other,
    params: &[],
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// Halves its input and counts its `process` calls.
struct CountingHalf(Arc<AtomicUsize>);

impl Plugin for CountingHalf {
    fn info(&self) -> &'static PluginInfo {
        &STUB_INFO
    }
    fn prepare(&mut self, _sr: f32, _mb: usize, _ch: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _id: &str, _v: f32) -> bool {
        false
    }
    fn param(&self, _id: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.0.fetch_add(1, Ordering::Relaxed);
        for c in io.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x *= 0.5);
        }
    }
}

/// A stereo audio track (hard-panned) playing 0.4 through the counting stub, with a frozen render
/// of 0.2 (what the chain makes of it) in the pool.
fn session() -> (Session, TrackId, ModuleId) {
    let mut s = Session::default();
    let flat = |v: f32| Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR as u32, channels: vec![vec![v; LEN], vec![v; LEN]] }));
    s.pool.insert(CLIP, flat(0.4));
    s.pool.insert(FROZEN, flat(0.2));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Gtr"));
    let clip = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, "Gtr", CLIP, 0, 0, LEN as i64));
    tr.mixer.pan = vec![-1.0, 1.0];
    tr.mixer.output = Route::Main;
    let m = tr.mixer.add_module(Insert::new(STUB_ID), None).unwrap();
    (s, t, m)
}

fn engine(s: &Session, t: TrackId, m: ModuleId, calls: &Arc<AtomicUsize>) -> MixEngine {
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    eng.set_external_instances(true);
    let plugin = Box::new(CountingHalf(Arc::clone(calls)));
    eng.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Module(m), id: STUB_ID.into(), ch: 2, plugin }]);
    eng.ensure_synced(s);
    eng
}

/// The left channel of `blocks` blocks from `from`.
fn render(eng: &mut MixEngine, s: &Session, from: usize, blocks: usize) -> Vec<f32> {
    let mut buf = vec![vec![0.0f32; BLOCK]; 2];
    let mut out = Vec::new();
    for b in from..from + blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut buf);
        out.extend_from_slice(&buf[0]);
    }
    out
}

fn freeze(s: &mut Session, t: TrackId) {
    let tr = s.track_mut(t).unwrap();
    tr.frozen = true;
    tr.freeze = Some(FrozenAudio { source: FROZEN, start: 0, upto: None });
}

#[test]
fn a_frozen_track_plays_its_render_and_skips_its_chain() {
    let (mut s, t, m) = session();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut eng = engine(&s, t, m, &calls);
    let live = render(&mut eng, &s, 0, 4);
    assert!(calls.load(Ordering::Relaxed) > 0, "live, the chain runs");
    assert!(live[BLOCK..].iter().all(|x| (x - 0.2).abs() < 1e-5), "live: 0.4 halved");

    freeze(&mut s, t);
    let before = calls.load(Ordering::Relaxed);
    let frozen = render(&mut eng, &s, 4, 4);
    assert_eq!(calls.load(Ordering::Relaxed), before, "frozen, the chain is not processed");
    assert!(frozen.iter().all(|x| (x - 0.2).abs() < 1e-5), "frozen: the render, not the clip");
}

#[test]
fn the_fader_and_mute_stay_live_while_frozen() {
    let (mut s, t, m) = session();
    freeze(&mut s, t);
    let calls = Arc::new(AtomicUsize::new(0));
    let mut eng = engine(&s, t, m, &calls);
    s.track_mut(t).unwrap().mixer.volume_db = -6.0;
    let quieter = render(&mut eng, &s, 0, 4);
    let g = 10f32.powf(-6.0 / 20.0);
    assert!(quieter[BLOCK..].iter().all(|x| (x - 0.2 * g).abs() < 1e-4), "the fader applies: {}", quieter[BLOCK]);
    s.track_mut(t).unwrap().mixer.mute = true;
    let muted = render(&mut eng, &s, 4, 4);
    assert!(muted[BLOCK..].iter().all(|x| x.abs() < 1e-5), "mute applies");
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[test]
fn unfreezing_plays_the_chain_again_and_a_missing_render_is_silence() {
    let (mut s, t, m) = session();
    freeze(&mut s, t);
    // Not in the pool: silence, never a crash.
    s.track_mut(t).unwrap().freeze = Some(FrozenAudio { source: SourceId(999), start: 0, upto: None });
    let calls = Arc::new(AtomicUsize::new(0));
    let mut eng = engine(&s, t, m, &calls);
    assert!(render(&mut eng, &s, 0, 2).iter().all(|x| *x == 0.0));
    let tr = s.track_mut(t).unwrap();
    tr.frozen = false;
    tr.freeze = None;
    let live = render(&mut eng, &s, 2, 4);
    assert!(calls.load(Ordering::Relaxed) > 0);
    assert!(live[BLOCK..].iter().all(|x| (x - 0.2).abs() < 1e-5));
}

/// The track of [`session`] with a second counting stub after the first: chain Before, After.
fn two_modules() -> (Session, TrackId, ModuleId, ModuleId) {
    let (mut s, t, before) = session();
    let after = s.track_mut(t).unwrap().mixer.add_module(Insert::new(STUB_ID), None).unwrap();
    (s, t, before, after)
}

#[test]
fn a_partial_freeze_plays_the_render_into_the_modules_after_the_point() {
    let (mut s, t, before, after) = two_modules();
    let (cb, ca) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    eng.set_external_instances(true);
    eng.adopt(vec![
        PreparedInstance { track: t, slot: SlotKey::Module(before), id: STUB_ID.into(), ch: 2, plugin: Box::new(CountingHalf(Arc::clone(&cb))) },
        PreparedInstance { track: t, slot: SlotKey::Module(after), id: STUB_ID.into(), ch: 2, plugin: Box::new(CountingHalf(Arc::clone(&ca))) },
    ]);
    eng.ensure_synced(&s);
    let live = render(&mut eng, &s, 0, 4);
    assert!(live[BLOCK..].iter().all(|x| (x - 0.1).abs() < 1e-5), "live: 0.4 halved twice");

    // Frozen up to Before: the render (0.2, what Before makes of the clip) runs through After.
    let tr = s.track_mut(t).unwrap();
    tr.frozen = true;
    tr.freeze = Some(FrozenAudio { source: FROZEN, start: 0, upto: Some(before) });
    let (b0, a0) = (cb.load(Ordering::Relaxed), ca.load(Ordering::Relaxed));
    let partial = render(&mut eng, &s, 4, 4);
    assert_eq!(cb.load(Ordering::Relaxed), b0, "the module at the point is not processed");
    assert!(ca.load(Ordering::Relaxed) > a0, "the module after the point is");
    assert!(partial.iter().all(|x| (x - 0.1).abs() < 1e-5), "render through After: {}", partial[0]);

    // Frozen up to After (the last module): nothing runs; the render plays as it is.
    s.track_mut(t).unwrap().freeze = Some(FrozenAudio { source: FROZEN, start: 0, upto: Some(after) });
    let a1 = ca.load(Ordering::Relaxed);
    let whole = render(&mut eng, &s, 8, 4);
    assert_eq!(ca.load(Ordering::Relaxed), a1);
    assert!(whole.iter().all(|x| (x - 0.2).abs() < 1e-5));
}

#[test]
fn a_mono_tracks_render_is_at_unity() {
    // A mono track renders through a stereo main mix and folds back: the centre pan law (exactly
    // -3 dB) must be undone exactly, or the render plays about 0.01 dB hot.
    let mut s = Session::default();
    s.pool.insert(CLIP, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR as u32, channels: vec![vec![0.4; LEN]] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Bass"));
    let clip = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "Bass", CLIP, 0, 0, LEN as i64));
    let out = soundcraft_mix::render_track_frozen(&s, t, LEN as i64, None);
    assert_eq!(out.len(), 1);
    let worst = out[0][..LEN].iter().fold(0.0f32, |w, x| w.max((x - 0.4).abs()));
    assert!(worst < 1e-6, "off unity by {worst}");
}

static RESETS_INFO: PluginInfo = PluginInfo { id: STUB_ID, name: "Counting Resets", short_name: "Resets", ..STUB_INFO };

/// Passes audio through and counts its resets.
struct CountingResets(Arc<AtomicUsize>);

impl Plugin for CountingResets {
    fn info(&self) -> &'static PluginInfo {
        &RESETS_INFO
    }
    fn prepare(&mut self, _sr: f32, _mb: usize, _ch: usize) {}
    fn reset(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn set_param(&mut self, _id: &str, _v: f32) -> bool {
        false
    }
    fn param(&self, _id: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _io: &mut [Vec<f32>], _frames: usize) {}
}

#[test]
fn thawing_a_partial_freeze_resets_only_the_frozen_modules() {
    let (mut s, t, before, after) = two_modules();
    let (rb, ra) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    eng.set_external_instances(true);
    eng.adopt(vec![
        PreparedInstance { track: t, slot: SlotKey::Module(before), id: STUB_ID.into(), ch: 2, plugin: Box::new(CountingResets(Arc::clone(&rb))) },
        PreparedInstance { track: t, slot: SlotKey::Module(after), id: STUB_ID.into(), ch: 2, plugin: Box::new(CountingResets(Arc::clone(&ra))) },
    ]);
    eng.ensure_synced(&s);
    let tr = s.track_mut(t).unwrap();
    tr.frozen = true;
    tr.freeze = Some(FrozenAudio { source: FROZEN, start: 0, upto: Some(before) });
    render(&mut eng, &s, 0, 2);
    let (b0, a0) = (rb.load(Ordering::Relaxed), ra.load(Ordering::Relaxed));
    let tr = s.track_mut(t).unwrap();
    tr.frozen = false;
    tr.freeze = None;
    render(&mut eng, &s, 2, 2);
    assert!(rb.load(Ordering::Relaxed) > b0, "the frozen module starts clean");
    assert_eq!(ra.load(Ordering::Relaxed), a0, "the live module after the point keeps ringing");
}
