//! Per-module meters: one entry per chain module, in chain order, filled by the render loop.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::MixEngine;
use soundcraft_model::{ChannelFormat, Clip, Insert, ModuleId, Route, Session, SourceAudio, SourceId, TrackId, TrackKind};
use std::sync::Arc;

const BLOCK: usize = 512;
const LEN: usize = BLOCK * 200;

/// A stereo track playing a 1 kHz sine of 0 dBFS peak into the main mix, with `chain` inserted.
fn session(chain: Vec<Insert>) -> (Session, TrackId, Vec<ModuleId>) {
    let mut s = Session::default();
    let sine: Vec<f32> = (0..LEN).map(|n| (std::f32::consts::TAU * 1000.0 * n as f32 / 48_000.0).sin()).collect();
    s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![sine.clone(), sine] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Sine"));
    let clip = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, "sine", SourceId(5), 0, 0, LEN as i64));
    tr.mixer.output = Route::Main;
    let ids = chain.into_iter().map(|i| tr.mixer.add_module(i, None).unwrap()).collect();
    (s, t, ids)
}

fn render(eng: &mut MixEngine, s: &Session, blocks: std::ops::Range<usize>) {
    let mut buf = vec![vec![0.0f32; BLOCK]; 2];
    for b in blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut buf);
    }
}

#[test]
fn module_meters_follow_the_chain() {
    let mut gain = Insert::new("gain");
    gain.params.insert("gain".into(), -12.0);
    let mut comp = Insert::new("compressor");
    comp.params.insert("threshold".into(), -40.0);
    comp.params.insert("ratio".into(), 8.0);
    let (s, t, ids) = session(vec![gain, comp]);
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    render(&mut eng, &s, 0..20);
    let m = &eng.meters[&t].modules;
    assert_eq!(m.len(), 2);
    assert_eq!((m[0].module, m[1].module), (ids[0], ids[1]));
    assert!((m[0].out_peak - 0.251).abs() < 0.01, "gain out {}", m[0].out_peak);
    assert_eq!(m[1].in_peak, m[0].out_peak);
    assert!(m[1].gr_db > 3.0, "gr {}", m[1].gr_db);
    assert!((m[1].readouts[0] + 12.0).abs() < 1.0, "detector {}", m[1].readouts[0]);
    assert!(!m[0].failed && !m[1].failed);
}

#[test]
fn a_missing_plugin_passes_the_signal_through_its_meter() {
    let mut gain = Insert::new("gain");
    gain.params.insert("gain".into(), -6.0);
    let (s, t, ids) = session(vec![gain, Insert::new("no_such_plugin")]);
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    render(&mut eng, &s, 0..4);
    let m = &eng.meters[&t].modules;
    assert_eq!(m.len(), 2);
    assert_eq!(m[1].module, ids[1]);
    assert!(m[1].in_peak > 0.4);
    assert_eq!(m[1].in_peak, m[1].out_peak);
    assert_eq!((m[1].gr_db, m[1].failed), (0.0, false));
    assert!(m[1].readouts.iter().all(|r| r.is_nan()), "no readings: {:?}", m[1].readouts);
}

#[test]
fn bypassed_modules_report_the_passing_signal() {
    let mut gain = Insert::new("gain");
    gain.params.insert("gain".into(), -12.0);
    gain.bypass = true;
    let (s, t, _) = session(vec![gain]);
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    render(&mut eng, &s, 0..4);
    let m = &eng.meters[&t].modules[0];
    assert!(m.in_peak > 0.9);
    assert_eq!(m.in_peak, m.out_peak);
    assert_eq!(m.gr_db, 0.0);
}

#[test]
fn readouts_a_module_did_not_write_are_nan() {
    let mut off = Insert::new("compressor");
    off.bypass = true;
    let (s, t, _) = session(vec![Insert::new("compressor"), off, Insert::new("channel_strip"), Insert::new("gain")]);
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    render(&mut eng, &s, 0..4);
    let m = &eng.meters[&t].modules;
    assert!(m[0].readouts[0].is_finite(), "the running compressor's detector level: {:?}", m[0].readouts);
    assert!(m[0].readouts[1..].iter().all(|r| r.is_nan()), "slots it doesn't use: {:?}", m[0].readouts);
    for (i, what) in [(1, "bypassed"), (2, "a plugin with no readouts"), (3, "a plugin with no readouts")] {
        assert!(m[i].readouts.iter().all(|r| r.is_nan()), "{what}: {:?}", m[i].readouts);
    }
    assert!(soundcraft_mix::ModuleMeter::default().readouts.iter().all(|r| r.is_nan()), "before the first block");
}

#[test]
fn module_meters_do_not_allocate_in_steady_state() {
    let (s, t, _) = session(vec![Insert::new("gain"), Insert::new("compressor")]);
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    render(&mut eng, &s, 0..1);
    let before = (eng.meters[&t].modules.capacity(), eng.meters[&t].modules.as_ptr());
    render(&mut eng, &s, 1..101);
    let after = (eng.meters[&t].modules.capacity(), eng.meters[&t].modules.as_ptr());
    assert_eq!(before, after);
}

/// Stands in for a third-party plugin that puts out NaN.
struct Nan(Box<dyn soundcraft_dsp::Plugin>);

impl soundcraft_dsp::Plugin for Nan {
    fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
        self.0.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.0.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.0.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.0.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.0.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        io.iter_mut().for_each(|c| c.iter_mut().take(frames).for_each(|x| *x = f32::NAN));
    }
    fn latency(&self) -> usize {
        0
    }
}

fn nan_instance(track: TrackId, module: ModuleId) -> Vec<soundcraft_mix::PreparedInstance> {
    vec![soundcraft_mix::PreparedInstance {
        track,
        slot: soundcraft_mix::SlotKey::Module(module),
        id: "clap:test.nan".into(),
        ch: 2,
        plugin: Box::new(Nan(soundcraft_dsp::create("gain").unwrap())),
    }]
}

/// Levels finite; readouts finite or NaN (no reading), never infinite.
fn all_finite(m: &[soundcraft_mix::ModuleMeter]) -> bool {
    m.iter().all(|m| [m.in_peak, m.out_peak, m.gr_db].iter().all(|v| v.is_finite()) && m.readouts.iter().all(|v| !v.is_infinite()))
}

#[test]
fn a_module_that_puts_out_nan_is_flagged_and_everything_stays_finite() {
    let (s, t, ids) = session(vec![Insert::new("gain"), Insert::new("clap:test.nan"), Insert::new("gain")]);
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    eng.set_external_instances(true);
    render(&mut eng, &s, 0..1);
    eng.adopt(nan_instance(t, ids[1]));
    render(&mut eng, &s, 1..2);
    let m = &eng.meters[&t].modules;
    assert_eq!(m.len(), 3);
    assert!(all_finite(m), "{m:?}");
    assert!(!m[0].failed && m[1].failed && !m[2].failed);
    assert!(m[1].in_peak > 0.9, "the NaN block's input was measured before the module: {}", m[1].in_peak);
    assert_eq!(m[1].out_peak, 0.0, "the tripping block is silenced");
    assert_eq!(m[2].in_peak, 0.0);
    // Later blocks: the failed instance is out of the path, passing the signal through.
    render(&mut eng, &s, 2..4);
    let m = &eng.meters[&t].modules;
    assert!(all_finite(m) && m[1].failed);
    assert!(m[1].in_peak > 0.9);
    assert_eq!(m[1].in_peak, m[1].out_peak);
    assert_eq!(m[2].in_peak, m[1].out_peak);
}

#[test]
fn a_master_chain_module_gets_a_module_meter() {
    let (mut s, _t, _) = session(vec![]);
    let m = s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
    let mut gain = Insert::new("gain");
    gain.params.insert("gain".into(), -12.0);
    let id = s.track_mut(m).unwrap().mixer.add_module(gain, None).unwrap();
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    render(&mut eng, &s, 0..20);
    let mm = &eng.meters[&m].modules;
    assert_eq!(mm.len(), 1);
    assert_eq!(mm[0].module, id);
    assert!(mm[0].in_peak > 0.4, "the main mix reaches the master chain: {}", mm[0].in_peak);
    assert!((mm[0].out_peak / mm[0].in_peak - 0.251).abs() < 0.01, "{} -> {}", mm[0].in_peak, mm[0].out_peak);
    assert!(!mm[0].failed);
}

/// A pass-through plugin that exposes analysis streams.
struct Tap(Box<dyn soundcraft_dsp::Plugin>, Arc<soundcraft_dsp::analysis::Analysis>);

impl soundcraft_dsp::Plugin for Tap {
    fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
        self.0.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.0.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.0.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.0.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.0.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let mono = |io: &[Vec<f32>], n: usize| io.iter().map(|c| c.get(n).copied().unwrap_or(0.0)).sum::<f32>() / io.len().max(1) as f32;
        for n in 0..frames {
            self.1.pre.push(mono(io, n));
        }
        self.0.process(io, frames);
        for n in 0..frames {
            self.1.post.push(mono(io, n));
        }
    }
    fn latency(&self) -> usize {
        0
    }
    fn analysis(&self) -> Option<Arc<soundcraft_dsp::analysis::Analysis>> {
        Some(self.1.clone())
    }
}

fn tap_instance(track: TrackId, module: ModuleId, a: &Arc<soundcraft_dsp::analysis::Analysis>) -> Vec<soundcraft_mix::PreparedInstance> {
    vec![soundcraft_mix::PreparedInstance {
        track,
        slot: soundcraft_mix::SlotKey::Module(module),
        id: "clap:test.tap".into(),
        ch: 2,
        plugin: Box::new(Tap(soundcraft_dsp::create("gain").unwrap(), a.clone())),
    }]
}

#[test]
fn strip_meter_carries_analysis_of_modules_that_have_it() {
    let (mut s, t, ids) = session(vec![Insert::new("gain"), Insert::new("clap:test.tap")]);
    let a = Arc::new(soundcraft_dsp::analysis::Analysis::default());
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    eng.set_external_instances(true);
    render(&mut eng, &s, 0..1);
    assert!(eng.meters[&t].analysis.is_empty(), "no instance yet");
    eng.adopt(tap_instance(t, ids[1], &a));
    render(&mut eng, &s, 1..3);
    let m = &eng.meters[&t];
    assert_eq!(m.analysis.len(), 1);
    assert_eq!(m.analysis[0].0, ids[1]);
    assert!(Arc::ptr_eq(&m.analysis[0].1, &a));
    assert!(a.post.written() > 0 && a.pre.written() > 0);
    // Reordering keeps the instance, so the same handle follows the module.
    let tr = s.track_mut(t).unwrap();
    let first = tr.mixer.inserts.remove(0);
    tr.mixer.inserts.push(first);
    render(&mut eng, &s, 3..5);
    let m = &eng.meters[&t];
    assert_eq!(m.analysis.len(), 1);
    assert_eq!(m.analysis[0].0, ids[1]);
    assert!(Arc::ptr_eq(&m.analysis[0].1, &a));
}

#[test]
fn a_removed_modules_analysis_leaves_with_the_retired_instance() {
    let (mut s, t, ids) = session(vec![Insert::new("clap:test.tap")]);
    let a = Arc::new(soundcraft_dsp::analysis::Analysis::default());
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    eng.set_external_instances(true);
    eng.adopt(tap_instance(t, ids[0], &a));
    render(&mut eng, &s, 0..2);
    // A host's meter snapshot (the playback crate copies the engine's meters into one).
    let mut snapshot = soundcraft_mix::StripMeter::default();
    snapshot.copy_from(&eng.meters[&t]);
    let w = Arc::downgrade(&a);
    drop(a);
    assert_eq!(w.strong_count(), 4, "instance, strip, engine meter, snapshot");
    s.track_mut(t).unwrap().mixer.remove_module(ids[0]).unwrap();
    render(&mut eng, &s, 2..3);
    assert!(eng.meters[&t].analysis.is_empty());
    let plugins = eng.take_retired();
    let handles = eng.take_retired_analysis();
    assert_eq!((plugins.len(), handles.len()), (1, 1));
    assert_eq!(w.strong_count(), 3, "the engine holds no copy: instance, handed-over handle, snapshot");
    // The host refreshes its snapshot before handing the garbage over: the stale copy goes
    // while the handed-over handle still holds the analysis.
    snapshot.copy_from(&eng.meters[&t]);
    assert!(snapshot.analysis.is_empty());
    assert_eq!(w.strong_count(), 2);
    drop(plugins);
    assert_eq!(w.strong_count(), 1, "the last reference is the one handed over, dropped by the garbage side");
    drop(handles);
    assert!(w.upgrade().is_none());
}

#[test]
fn a_removed_tracks_analysis_leaves_with_its_instances() {
    let (mut s, t, ids) = session(vec![Insert::new("clap:test.tap")]);
    let a = Arc::new(soundcraft_dsp::analysis::Analysis::default());
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    eng.set_external_instances(true);
    eng.adopt(tap_instance(t, ids[0], &a));
    render(&mut eng, &s, 0..2);
    let w = Arc::downgrade(&a);
    drop(a);
    s.tracks.retain(|x| x.id != t);
    render(&mut eng, &s, 2..3);
    let plugins = eng.take_retired();
    let handles = eng.take_retired_analysis();
    assert_eq!(plugins.len(), 1);
    assert!(eng.meters.get(&t).is_none_or(|m| m.analysis.is_empty()), "the stale meter lets go");
    drop(plugins);
    assert_eq!(w.strong_count(), handles.len(), "only the handed-over handles hold it");
    drop(handles);
    assert!(w.upgrade().is_none());
}
