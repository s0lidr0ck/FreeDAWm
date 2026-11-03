//! Delay compensation follows a module whose latency changes while playing: every strip is
//! re-aligned with a short crossfade between the old and the new delay tap (no click, no
//! allocation), and constant latency is compensated exactly as before.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_dsp::{Category, ParamInfo, Plugin, PluginInfo, Taper, Unit};
use soundcraft_mix::{MixEngine, PreparedInstance, SlotKey};
use soundcraft_model::{ChannelFormat, Clip, Insert, ModuleId, Route, Session, SourceAudio, SourceId, TrackId, TrackKind};
use std::sync::Arc;

const SR: usize = 48_000;
const BLOCK: usize = 512;
/// One second, rounded up to whole blocks.
const SECOND_BLOCKS: usize = SR.div_ceil(BLOCK);
const LEN: usize = 2 * SECOND_BLOCKS * BLOCK;
/// `CROSSFADE_MS = 10.0` at 48 kHz.
const XF: usize = 480;
const STUB_ID: &str = "clap:test.latency_stub";

static STUB_PARAMS: [ParamInfo; 1] = [ParamInfo {
    id: "ms",
    name: "Delay",
    min: 0.0,
    max: 20.0,
    default: 0.0,
    unit: Unit::Ms,
    taper: Taper::Linear,
    choices: &[],
    display: None,
    group: "",
}];

static STUB_INFO: PluginInfo = PluginInfo {
    id: STUB_ID,
    name: "Latency Stub",
    short_name: "LatStub",
    category: Category::Other,
    params: &STUB_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// A pure delay whose latency is the param `ms` (0–20 ms). It switches delay instantly.
#[derive(Default)]
struct LatencyStub {
    ms: f32,
    sr: f32,
    rings: Vec<Vec<f32>>,
    w: usize,
}

impl LatencyStub {
    fn delay(&self) -> usize {
        (self.ms * self.sr / 1000.0).round() as usize
    }
}

impl Plugin for LatencyStub {
    fn info(&self) -> &'static PluginInfo {
        &STUB_INFO
    }
    fn prepare(&mut self, sr: f32, _mb: usize, ch: usize) {
        self.sr = sr;
        self.rings = vec![vec![0.0; (sr * 0.02) as usize + 1]; ch];
        self.w = 0;
    }
    fn reset(&mut self) {
        self.rings.iter_mut().for_each(|r| r.fill(0.0));
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        if id != "ms" {
            return false;
        }
        self.ms = STUB_PARAMS[0].clamp(v);
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        (id == "ms").then_some(self.ms)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let d = self.delay();
        let mut w = self.w;
        for (c, ch) in io.iter_mut().enumerate() {
            let ring = &mut self.rings[c];
            let n = ring.len();
            w = self.w;
            for x in ch.iter_mut().take(frames) {
                ring[w] = *x;
                *x = ring[(w + n - d) % n];
                w = (w + 1) % n;
            }
        }
        self.w = w;
    }
    fn latency(&self) -> usize {
        self.delay()
    }
}

fn stub(track: TrackId, module: ModuleId) -> Vec<PreparedInstance> {
    let mut p = LatencyStub::default();
    p.prepare(SR as f32, BLOCK, 2);
    vec![PreparedInstance { track, slot: SlotKey::Module(module), id: STUB_ID.into(), ch: 2, plugin: Box::new(p) }]
}

/// Two stereo tracks (hard-panned) playing `signal` into the main mix: A with a LatencyStub at
/// `ms`, B with no modules (phase-inverted when `invert`).
fn session(signal: &[f32], ms: f32, invert: bool) -> (Session, TrackId, TrackId, ModuleId) {
    let mut s = Session::default();
    s.edit.delay_compensation = true;
    s.pool.insert(SourceId(31), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR as u32, channels: vec![signal.to_vec(), signal.to_vec()] })));
    let mut add = |name: &str| {
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some(name));
        let clip = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, name, SourceId(31), 0, 0, signal.len() as i64));
        tr.mixer.pan = vec![-1.0, 1.0];
        tr.mixer.output = Route::Main;
        t
    };
    let a = add("A");
    let b = add("B");
    let mut ins = Insert::new(STUB_ID);
    ins.params.insert("ms".into(), ms);
    let m = s.track_mut(a).unwrap().mixer.add_module(ins, None).unwrap();
    s.track_mut(b).unwrap().mixer.phase_invert = invert;
    (s, a, b, m)
}

fn set_ms(s: &mut Session, a: TrackId, ms: f32) {
    s.track_mut(a).unwrap().mixer.inserts[0].params.insert("ms".into(), ms);
}

/// Renders `blocks` into `out` (planar, appended).
fn render(eng: &mut MixEngine, s: &Session, blocks: std::ops::Range<usize>, out: &mut [Vec<f32>]) {
    let mut buf = vec![vec![0.0f32; BLOCK]; 2];
    for b in blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut buf);
        for (o, c) in out.iter_mut().zip(&buf) {
            o.extend_from_slice(c);
        }
    }
}

fn engine(s: &Session, a: TrackId, m: ModuleId) -> MixEngine {
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    eng.set_external_instances(true);
    eng.adopt(stub(a, m));
    eng.ensure_synced(s);
    eng
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn max_jump(x: &[f32]) -> f32 {
    x.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
}

fn noise(len: usize) -> Vec<f32> {
    let mut seed = 0x1234_5678u32;
    (0..len)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * 0.25
        })
        .collect()
}

#[test]
fn latency_change_realigns_without_click() {
    let sine: Vec<f32> = (0..LEN).map(|n| (std::f32::consts::TAU * 200.0 * n as f32 / SR as f32).sin()).collect();
    let (mut s, a, b, m) = session(&sine, 0.0, true);
    let mut eng = engine(&s, a, m);
    let mut out = vec![Vec::new(), Vec::new()];
    render(&mut eng, &s, 0..SECOND_BLOCKS, &mut out);
    let before = eng.delay_buffers(b);
    assert!(!before.is_empty(), "sync reserved the alignment lines");
    set_ms(&mut s, a, 5.0);
    render(&mut eng, &s, SECOND_BLOCKS..2 * SECOND_BLOCKS, &mut out);
    assert_eq!(eng.latency, 240, "5 ms at 48 kHz");
    assert_eq!(eng.delay_buffers(b), before, "no reallocation: capacity and pointers unchanged");
    assert_eq!(eng.delay_buffers(a).len(), before.len());
    let switch = SECOND_BLOCKS * BLOCK;
    for c in &out {
        assert!(c.iter().all(|x| x.is_finite()));
        let tail = rms(&c[switch + XF..]);
        assert!(tail < 1e-3, "the strips null again after the crossfade: rms {tail}");
        let jump = max_jump(c);
        assert!(jump < 0.2, "no click: largest jump {jump}");
    }
}

#[test]
fn latency_change_realigns_a_non_periodic_signal() {
    // The sine above repeats every 5 ms, so it nulls at any multiple of 5 ms; noise does not.
    let sig = noise(LEN);
    let (mut s, a, b, m) = session(&sig, 0.0, true);
    let mut eng = engine(&s, a, m);
    let mut out = vec![Vec::new(), Vec::new()];
    render(&mut eng, &s, 0..SECOND_BLOCKS, &mut out);
    let before = eng.delay_buffers(b);
    set_ms(&mut s, a, 5.0);
    render(&mut eng, &s, SECOND_BLOCKS..2 * SECOND_BLOCKS - 20, &mut out);
    // And back down, mid-play.
    set_ms(&mut s, a, 2.0);
    render(&mut eng, &s, 2 * SECOND_BLOCKS - 20..2 * SECOND_BLOCKS, &mut out);
    assert_eq!(eng.latency, 96);
    assert_eq!(eng.delay_buffers(b), before);
    let first = SECOND_BLOCKS * BLOCK;
    let second = (2 * SECOND_BLOCKS - 20) * BLOCK;
    for c in &out {
        assert!(c.iter().all(|x| x.is_finite()));
        assert!(rms(&c[..first]) < 1e-6, "nulls before the change");
        let mid = rms(&c[first + XF..second]);
        assert!(mid < 1e-3, "re-aligned after growing: rms {mid}");
        let tail = rms(&c[second + XF..]);
        assert!(tail < 1e-3, "re-aligned after shrinking: rms {tail}");
    }
}

#[test]
fn constant_latency_output_is_unchanged() {
    let sig = noise(LEN);
    let (s, a, _, m) = session(&sig, 5.0, false);
    let mut eng = engine(&s, a, m);
    let mut out = vec![Vec::new(), Vec::new()];
    render(&mut eng, &s, 0..2 * SECOND_BLOCKS, &mut out);
    assert_eq!(eng.latency, 240);
    for c in &out {
        for (n, y) in c.iter().enumerate() {
            let want = if n < 240 { 0.0 } else { 2.0 * sig[n - 240] };
            assert!((y - want).abs() <= 1e-6, "sample {n}: {y} vs {want}");
        }
    }
}
