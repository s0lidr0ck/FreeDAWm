//! Freeze, Commit and Bounce render through the host's live plugin instances (Part 3 of the
//! plugin stability spec): a hosted instrument whose preset was chosen in its own editor (so its
//! stored params are stale) freezes and bounces at the level it plays, through the very
//! instance, with no new instance made, and the instance's state is as before afterwards.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use soundcraft_audio_io::{BitDepth, EncodeOptions, FileFormat};
use soundcraft_dsp::Plugin;
use soundcraft_engine::Engine;
use soundcraft_engine::io::Bounce;
use soundcraft_engine::render_job::RenderJob;
use soundcraft_mix::{LiveInstances, MixEngine, PreparedInstance, SlotKey};
use soundcraft_model::{ChannelFormat, Clip, Insert, Session, TrackId, TrackKind};
use soundcraft_time::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// A hosted instrument standing in for a VST3/CLAP piano: a signal generator behind a
/// third-party id (no copy of it can be made: the id names no installed plugin), with the level
/// as its own state and a preset the user picks in its editor (`choose`), which the session's
/// stored params never see. Counts its `prepare` calls: a render through the live instance
/// prepares it again twice (offline, then realtime), a copy would be a new instance of its own
/// (which cannot be made: the id names no installed plugin).
struct HostedKeys {
    inner: Box<dyn Plugin>,
    choose: Arc<Mutex<Option<f32>>>,
    level: Arc<Mutex<f32>>,
    prepares: Arc<AtomicUsize>,
}

impl HostedKeys {
    fn new() -> (Self, Arc<Mutex<Option<f32>>>, Arc<AtomicUsize>) {
        let inner = soundcraft_dsp::create("signal_generator").unwrap();
        let level = Arc::new(Mutex::new(inner.param("level").unwrap_or(0.0)));
        let choose = Arc::new(Mutex::new(None));
        let prepares = Arc::new(AtomicUsize::new(0));
        (HostedKeys { inner, choose: Arc::clone(&choose), level, prepares: Arc::clone(&prepares) }, choose, prepares)
    }
    fn set_level(&mut self, v: f32) {
        self.inner.set_param("level", v);
        *self.level.lock().unwrap() = v;
    }
}

impl Plugin for HostedKeys {
    fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.prepares.fetch_add(1, Ordering::Relaxed);
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        if id == "level" {
            self.set_level(v);
            return true;
        }
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let chosen = self.choose.lock().unwrap().take();
        if let Some(level) = chosen {
            self.set_level(level);
        }
        self.inner.process(io, frames);
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        let Ok(bytes) = <[u8; 4]>::try_from(data) else { return false };
        self.set_level(f32::from_le_bytes(bytes));
        true
    }
    fn state_reader(&mut self) -> Option<Box<dyn soundcraft_dsp::PluginStateReader>> {
        let level = Arc::clone(&self.level);
        Some(Box::new(move || Some(level.lock().unwrap().to_le_bytes().to_vec())))
    }
}

const BLOCK: usize = 480;
const SECS: f64 = 2.0;

/// An instrument track playing the hosted keys over a MIDI clip of `SECS` seconds, the stored
/// params the scan's defaults.
fn session() -> (Session, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let mut ins = Insert::new("clap:test.keys");
    for p in soundcraft_dsp::plugin_info("signal_generator").unwrap().params {
        ins.params.insert(p.id.to_string(), p.default);
    }
    let cid = s.new_clip_id();
    let len = s.sample_rate.samples(SECS);
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "keys", 0, len, soundcraft_midi::Sequence::default()));
    tr.instrument = Some(ins);
    (s, t)
}

struct Live {
    eng: Box<MixEngine>,
    reader: Box<dyn soundcraft_dsp::PluginStateReader>,
    prepares: Arc<AtomicUsize>,
    /// What the player hears with the preset, dBFS RMS.
    heard: f32,
}

fn db(x: &[f32]) -> f32 {
    let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt();
    20.0 * rms.max(1e-9).log10()
}

/// The realtime host's engine: the keys adopted (prepared once, as `create_instance` does), the
/// defaults pushed, then the preset chosen in the editor (-6 dB instead of -20 dB).
fn live(s: &Session, t: TrackId) -> Live {
    let (mut keys, choose, prepares) = HostedKeys::new();
    keys.prepare(48_000.0, BLOCK, 2);
    let reader = keys.state_reader().unwrap();
    let mut eng = Box::new(MixEngine::new(48_000.0, BLOCK));
    eng.set_external_instances(true);
    eng.metronome = true;
    eng.adopt(vec![PreparedInstance { track: t, slot: SlotKey::Instrument, id: "clap:test.keys".into(), ch: 2, plugin: Box::new(keys) }]);
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    for b in 0..8 {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut out);
    }
    *choose.lock().unwrap() = Some(-6.0);
    for b in 8..16 {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut out);
    }
    let heard = db(&out[0]);
    assert!((heard + 9.0).abs() < 0.5, "the preset plays live: {heard}");
    Live { eng, reader, prepares, heard }
}

fn lend(live: Box<MixEngine>, s: &Session) -> (LiveInstances, Arc<Mutex<Option<Box<MixEngine>>>>) {
    let back = Arc::new(Mutex::new(None));
    let returned = Arc::clone(&back);
    (LiveInstances::new(live, Arc::new(s.clone()), move |e| *returned.lock().unwrap() = Some(e)), back)
}

#[test]
fn a_freeze_renders_through_the_live_instance_at_its_playback_level() {
    let (s, t) = session();
    let mut e = Engine::new(s.clone());
    let Live { eng, mut reader, prepares, heard } = live(&s, t);
    let state = reader.save_state().unwrap();
    let made = prepares.load(Ordering::Relaxed);
    let (lent, back) = lend(eng, &s);
    let mut job = RenderJob::new(&e, "track.freeze", &json!({"tracks": [t.0]})).unwrap().expect("something to freeze").through(lent);
    assert!(job.is_live());
    assert_eq!(job.label(), "Freezing Keys");
    let mut steps = 0;
    while !job.step(|| false) {
        steps += 1;
        assert!(job.progress() <= 1.0);
    }
    assert!(steps > 10, "a block per step: {steps}");
    assert_eq!(job.progress(), 1.0);
    let (id, params) = job.command();
    let (id, params) = (id, params.clone());
    job.finish(&mut e);
    assert!(back.lock().unwrap().is_some(), "the engine is back before the command runs");
    let v = e.execute(id, &params).unwrap();
    assert_eq!(v["frozen"], json!([t.0]));
    // The frozen audio is the live instance's (no copy of a `clap:test.keys` can be made: a
    // render of its own would be silent).
    let tr = e.session().track(t).unwrap();
    let frozen = e.session().pool.get(tr.frozen_audio().unwrap().source).unwrap();
    let at = BLOCK * 8;
    let level = db(&frozen.buffer().channels[0][at..at + BLOCK]);
    assert!((level - heard).abs() < 0.5, "frozen {level:.1} dBFS vs live {heard:.1} dBFS");
    // The one instance, prepared again twice: offline for the render, realtime after it.
    assert_eq!(prepares.load(Ordering::Relaxed), made + 2, "no new instance");
    assert_eq!(reader.save_state().unwrap(), state, "the instance's state is as it was");
    assert_eq!(back.lock().unwrap().as_ref().unwrap().hosted_instances(), 1);
}

#[test]
fn a_commit_renders_through_the_live_instance() {
    let (s, t) = session();
    let mut e = Engine::new(s.clone());
    let Live { eng, prepares, heard, .. } = live(&s, t);
    let made = prepares.load(Ordering::Relaxed);
    let (lent, _back) = lend(eng, &s);
    let job = RenderJob::new(&e, "track.bounce", &json!({"tracks": [t.0]})).unwrap().expect("something to commit").through(lent);
    assert_eq!(job.label(), "Committing Keys");
    let v = job.run(&mut e).unwrap();
    let nt = TrackId(v["tracks"][0].as_u64().unwrap());
    let tr = e.session().track(nt).unwrap();
    let clip = tr.clips()[0].clone();
    let src = match clip.content {
        soundcraft_model::ClipContent::Audio { source, .. } => source,
        _ => panic!("an audio clip"),
    };
    let audio = e.session().pool.get(src).unwrap();
    let at = BLOCK * 8;
    let level = db(&audio.buffer().channels[0][at..at + BLOCK]);
    assert!((level - heard).abs() < 0.5, "committed {level:.1} dBFS vs live {heard:.1} dBFS");
    assert_eq!(prepares.load(Ordering::Relaxed), made + 2, "no new instance (prepared again for the mode change only)");
}

#[test]
fn a_render_made_ahead_is_only_good_for_the_session_it_came_from() {
    let (s, t) = session();
    let mut e = Engine::new(s.clone());
    let Live { eng, .. } = live(&s, t);
    let (lent, _back) = lend(eng, &s);
    let mut job = RenderJob::new(&e, "track.freeze", &json!({"tracks": [t.0]})).unwrap().unwrap().through(lent);
    while !job.step(|| true) {}
    job.finish(&mut e);
    // The session changes before the command runs: the render is stale and the command renders
    // again on its own (silent here: the plugin cannot be made).
    e.execute("track.rename", &json!({"track": t.0, "name": "Piano"})).unwrap();
    e.execute("track.freeze", &json!({"tracks": [t.0]})).unwrap();
    let tr = e.session().track(t).unwrap();
    let frozen = e.session().pool.get(tr.frozen_audio().unwrap().source).unwrap();
    assert!(frozen.buffer().channels[0].iter().all(|x| x.abs() < 1e-6), "a stale render is not used");
}

#[test]
fn a_bounce_renders_through_the_live_instance_at_its_playback_level() {
    let (s, t) = session();
    let e = Engine::new(s.clone());
    let Live { eng, mut reader, prepares, heard } = live(&s, t);
    let state = reader.save_state().unwrap();
    let made = prepares.load(Ordering::Relaxed);
    let (lent, back) = lend(eng, &s);
    let dir = std::env::temp_dir().join(format!("soundcraft-render-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("keys.wav");
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None, bitrate_kbps: 0 };
    let range = Range::new(0, s.sample_rate.samples(SECS));
    // Normalizing: two passes through the same instances.
    let bounce = Bounce::new(e.session_arc(), &path, range, &opts, None, true, false).unwrap().through(lent);
    assert!(bounce.is_live());
    let done = bounce.run().unwrap();
    assert!(done.peak_db > -0.2, "normalized: {}", done.peak_db);
    let (_, file) = soundcraft_audio_io::decode(&std::fs::read(&path).unwrap(), None).unwrap();
    // The normalizing gain is the same for the whole file: the peak (the sine's level, 3 dB over
    // its RMS) is brought to -0.1 dBFS.
    let at = BLOCK * 8;
    let level = db(&file.channels[0][at..at + BLOCK]);
    let gain = -0.1 - (heard + 3.0);
    assert!((level - (heard + gain)).abs() < 0.5, "bounced {level:.1} dBFS vs live {heard:.1} dBFS + {gain:.1} dB normalizing");
    // Two passes through the one instance: prepared again once for the render (offline) and
    // once when it went back (realtime), never between the passes.
    assert_eq!(prepares.load(Ordering::Relaxed), made + 2, "no new instance");
    assert!(back.lock().unwrap().is_some(), "the engine is back once the bounce is done");
    assert_eq!(reader.save_state().unwrap(), state, "the instance's state is as it was");
    let _ = std::fs::remove_dir_all(&dir);
}
