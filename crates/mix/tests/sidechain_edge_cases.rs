//! Module sidechain inputs (`Insert::sidechain`): the key reaches the plugin after its source
//! renders, whatever the track order; a missing, muted, deleted, inactive or cyclic source gives a
//! silent key and never a crash or a hang; channel counts are adapted; and the key lines up with
//! the module's main input when either side has latency.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_dsp::{Category, ParamInfo, Plugin, PluginInfo, Taper, Unit};
use soundcraft_mix::{MixEngine, PreparedInstance, SidechainProblem, SlotKey, sidechain_routes};
use soundcraft_model::{
    BusId, ChannelFormat, Clip, Insert, ModuleId, Route, SendSlot, Session, SidechainSource, SourceAudio, SourceId, TrackId, TrackKind,
};
use std::sync::Arc;

const SR: usize = 48_000;
const BLOCK: usize = 512;
const PROBE_ID: &str = "clap:test.key_probe";
const STUB_ID: &str = "clap:test.latency_stub";

static PROBE_INFO: PluginInfo = PluginInfo {
    id: PROBE_ID,
    name: "Key Probe",
    short_name: "Probe",
    category: Category::Other,
    params: &[],
    is_instrument: false,
    audiosuite: false,
    sidechain: true,
    link: None,
    references: 0,
    midi_out: false,
};

/// Replaces its input with the key; without a key (`None`) it inverts its input instead, so a
/// test can tell "no key" from "silent key".
#[derive(Default)]
struct KeyProbe {
    keyed: usize,
}

impl Plugin for KeyProbe {
    fn info(&self) -> &'static PluginInfo {
        &PROBE_INFO
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
        for c in io.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = -*x);
        }
    }
    fn process_with_sidechain(&mut self, io: &mut [Vec<f32>], frames: usize, key: Option<&[Vec<f32>]>) {
        let Some(key) = key else { return self.process(io, frames) };
        assert_eq!(key.len(), io.len(), "the key has the module's channel count");
        self.keyed += 1;
        for (c, k) in io.iter_mut().zip(key) {
            c[..frames].copy_from_slice(&k[..frames]);
        }
    }
}

static STUB_PARAMS: [ParamInfo; 1] = [ParamInfo {
    id: "n",
    name: "Samples",
    min: 0.0,
    max: 4096.0,
    default: 0.0,
    unit: Unit::None,
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

/// A pure delay of `n` samples that reports it as latency.
struct LatencyStub {
    n: usize,
    rings: Vec<Vec<f32>>,
    w: usize,
}

impl Plugin for LatencyStub {
    fn info(&self) -> &'static PluginInfo {
        &STUB_INFO
    }
    fn prepare(&mut self, _sr: f32, _mb: usize, ch: usize) {
        self.rings = vec![vec![0.0; 4097]; ch];
    }
    fn reset(&mut self) {
        self.rings.iter_mut().for_each(|r| r.fill(0.0));
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        if id == "n" {
            self.n = STUB_PARAMS[0].clamp(v) as usize;
        }
        id == "n"
    }
    fn param(&self, id: &str) -> Option<f32> {
        (id == "n").then_some(self.n as f32)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let mut w = self.w;
        for (c, ch) in io.iter_mut().enumerate() {
            let ring = &mut self.rings[c];
            let len = ring.len();
            w = self.w;
            for x in ch.iter_mut().take(frames) {
                ring[w] = *x;
                *x = ring[(w + len - self.n) % len];
                w = (w + 1) % len;
            }
        }
        self.w = w;
    }
    fn latency(&self) -> usize {
        self.n
    }
}

fn probe(track: TrackId, module: ModuleId, ch: usize) -> PreparedInstance {
    PreparedInstance { track, slot: SlotKey::Module(module), id: PROBE_ID.into(), ch, plugin: Box::new(KeyProbe::default()) }
}

fn stub(track: TrackId, module: ModuleId, ch: usize) -> PreparedInstance {
    let mut p = LatencyStub { n: 0, rings: Vec::new(), w: 0 };
    p.prepare(SR as f32, BLOCK, ch);
    PreparedInstance { track, slot: SlotKey::Module(module), id: STUB_ID.into(), ch, plugin: Box::new(p) }
}

/// A session with delay compensation on and one pooled source per signal.
fn session() -> Session {
    let mut s = Session::default();
    s.edit.delay_compensation = true;
    s
}

/// An audio track playing `signal` (on every channel of `fmt`) into `output`, hard-panned.
fn track(s: &mut Session, name: &str, fmt: ChannelFormat, signal: &[f32], output: Route) -> TrackId {
    let src = SourceId(1000 + s.tracks.len() as u64);
    let n = fmt.channels();
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR as u32, channels: vec![signal.to_vec(); n] })));
    let t = s.add_track(TrackKind::Audio, fmt, Some(name));
    let clip = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    if !signal.is_empty() {
        tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, name, src, 0, 0, signal.len() as i64));
    }
    tr.mixer.pan = if n == 1 { vec![-1.0] } else { vec![-1.0, 1.0] };
    tr.mixer.output = output;
    t
}

/// Adds a module of `plugin` to `t`, keyed from `source` when given.
fn module(s: &mut Session, t: TrackId, plugin: &str, source: Option<(TrackId, bool)>) -> ModuleId {
    let mut ins = Insert::new(plugin);
    ins.sidechain = source.map(|(track, pre_fader)| SidechainSource { track, pre_fader });
    s.track_mut(t).unwrap().mixer.add_module(ins, None).unwrap()
}

fn engine(s: &Session, instances: Vec<PreparedInstance>) -> MixEngine {
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    eng.set_external_instances(true);
    eng.adopt(instances);
    eng.ensure_synced(s);
    eng
}

/// Renders `blocks` blocks from 0 and returns the main mix (planar).
fn render(eng: &mut MixEngine, s: &Session, blocks: usize) -> Vec<Vec<f32>> {
    let mut out = vec![Vec::new(), Vec::new()];
    let mut buf = vec![vec![0.0f32; BLOCK]; 2];
    for b in 0..blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut buf);
        for (o, c) in out.iter_mut().zip(&buf) {
            o.extend_from_slice(c);
        }
    }
    out
}

fn dc(level: f32) -> Vec<f32> {
    vec![level; 8 * BLOCK]
}

fn last(x: &[f32]) -> f32 {
    *x.last().unwrap()
}

/// A consumer (stereo, silent, hard-panned) listed *before* its mono DC source, which goes
/// nowhere: the main mix is the key alone.
fn keyed_session(pre_fader: bool) -> (Session, TrackId, TrackId, ModuleId) {
    let mut s = session();
    let c = track(&mut s, "Consumer", ChannelFormat::Stereo, &[], Route::Main);
    let src = track(&mut s, "Kick", ChannelFormat::Mono, &dc(0.5), Route::None);
    s.track_mut(src).unwrap().mixer.volume_db = -6.0;
    let m = module(&mut s, c, PROBE_ID, Some((src, pre_fader)));
    (s, c, src, m)
}

#[test]
fn the_key_is_the_source_after_its_fader_or_before_it() {
    for (pre, want) in [(false, 0.5 * 10f32.powf(-6.0 / 20.0)), (true, 0.5)] {
        let (s, c, _, m) = keyed_session(pre);
        assert_eq!(sidechain_routes(&s)[0].result, Ok(()));
        let mut eng = engine(&s, vec![probe(c, m, 2)]);
        let out = render(&mut eng, &s, 4);
        // A mono source keys both channels of the stereo module.
        for ch in &out {
            assert!((last(ch) - want).abs() < 1e-5, "pre {pre}: {} vs {want}", last(ch));
        }
    }
}

#[test]
fn a_module_without_a_source_gets_no_key() {
    let mut s = session();
    let c = track(&mut s, "Consumer", ChannelFormat::Stereo, &dc(0.25), Route::Main);
    let m = module(&mut s, c, PROBE_ID, None);
    let mut eng = engine(&s, vec![probe(c, m, 2)]);
    let out = render(&mut eng, &s, 2);
    assert!((last(&out[0]) + 0.25).abs() < 1e-6, "keyed from its own input: {}", last(&out[0]));
}

#[test]
fn muted_deleted_and_inactive_sources_give_a_silent_key() {
    for pre in [false, true] {
        // Muted (also when soloed out).
        let (mut s, c, src, m) = keyed_session(pre);
        s.track_mut(src).unwrap().mixer.mute = true;
        let mut eng = engine(&s, vec![probe(c, m, 2)]);
        assert!(render(&mut eng, &s, 3).iter().flatten().all(|x| *x == 0.0), "muted, pre {pre}");
        s.track_mut(src).unwrap().mixer.mute = false;
        s.track_mut(c).unwrap().mixer.solo_safe = true;
        let other = track(&mut s, "Other", ChannelFormat::Mono, &[], Route::None);
        s.track_mut(other).unwrap().mixer.solo = true;
        let mut eng = engine(&s, vec![probe(c, m, 2)]);
        let out = render(&mut eng, &s, 3);
        assert!(out[0].iter().all(|x| *x == 0.0), "soloed out, pre {pre}");

        // Inactive.
        let (mut s, c, src, m) = keyed_session(pre);
        s.track_mut(src).unwrap().inactive = true;
        assert_eq!(sidechain_routes(&s)[0].result, Err(SidechainProblem::Inactive));
        let mut eng = engine(&s, vec![probe(c, m, 2)]);
        assert!(render(&mut eng, &s, 3).iter().flatten().all(|x| *x == 0.0), "inactive, pre {pre}");

        // Deleted while playing: the strip goes, the key falls silent.
        let (mut s, c, src, m) = keyed_session(pre);
        let mut eng = engine(&s, vec![probe(c, m, 2)]);
        assert!(last(&render(&mut eng, &s, 2)[0]) > 0.2);
        s.tracks.retain(|t| t.id != src);
        assert_eq!(sidechain_routes(&s)[0].result, Err(SidechainProblem::Missing));
        let mut buf = vec![vec![0.0f32; BLOCK]; 2];
        for b in 2..5 {
            eng.render(&s, (b * BLOCK) as i64, BLOCK, &mut buf);
        }
        assert!(buf.iter().flatten().all(|x| *x == 0.0), "deleted, pre {pre}");
    }
}

#[test]
fn cycles_are_refused_and_silent_and_never_hang() {
    // A ← B ← A: the second sidechain in session order closes the loop.
    let mut s = session();
    let a = track(&mut s, "A", ChannelFormat::Mono, &dc(0.5), Route::Main);
    let b = track(&mut s, "B", ChannelFormat::Mono, &dc(0.25), Route::None);
    let ma = module(&mut s, a, PROBE_ID, Some((b, false)));
    let mb = module(&mut s, b, PROBE_ID, Some((a, false)));
    let routes = sidechain_routes(&s);
    assert_eq!(routes[0].result, Ok(()));
    assert_eq!(routes[1].result, Err(SidechainProblem::Cycle));
    let mut eng = engine(&s, vec![probe(a, ma, 1), probe(b, mb, 1)]);
    let out = render(&mut eng, &s, 4);
    // B's key is silent, so B puts out silence, which keys A.
    assert!(out[0].iter().all(|x| *x == 0.0));

    // A track keyed from itself.
    let mut s = session();
    let a = track(&mut s, "A", ChannelFormat::Mono, &dc(0.5), Route::Main);
    let m = module(&mut s, a, PROBE_ID, Some((a, true)));
    assert_eq!(sidechain_routes(&s)[0].result, Err(SidechainProblem::Cycle));
    let mut eng = engine(&s, vec![probe(a, m, 1)]);
    assert!(render(&mut eng, &s, 2)[0].iter().all(|x| *x == 0.0));

    // A longer loop: A ← B ← C ← A.
    let mut s = session();
    let a = track(&mut s, "A", ChannelFormat::Mono, &dc(0.5), Route::Main);
    let b = track(&mut s, "B", ChannelFormat::Mono, &dc(0.5), Route::Main);
    let c = track(&mut s, "C", ChannelFormat::Mono, &dc(0.5), Route::Main);
    let ma = module(&mut s, a, PROBE_ID, Some((b, false)));
    let mb = module(&mut s, b, PROBE_ID, Some((c, false)));
    let mc = module(&mut s, c, PROBE_ID, Some((a, false)));
    let r: Vec<_> = sidechain_routes(&s).into_iter().map(|r| r.result).collect();
    assert_eq!(r, [Ok(()), Ok(()), Err(SidechainProblem::Cycle)]);
    let mut eng = engine(&s, vec![probe(a, ma, 1), probe(b, mb, 1), probe(c, mc, 1)]);
    assert!(render(&mut eng, &s, 3)[0].iter().all(|x| x.is_finite()));

    // Through busses: Aux X → bus 2 → Aux Y; X keyed from Y would wait for itself.
    let mut s = session();
    let (b1, b2) = (BusId(901), BusId(902));
    let a = track(&mut s, "A", ChannelFormat::Mono, &dc(0.5), Route::Bus(b1));
    let x = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("X"));
    let y = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("Y"));
    for (t, i, o) in [(x, b1, Route::Bus(b2)), (y, b2, Route::Main)] {
        let m = &mut s.track_mut(t).unwrap().mixer;
        m.input = Route::Bus(i);
        m.output = o;
    }
    s.busses.push(soundcraft_model::Bus { id: b1, name: "1".into(), format: ChannelFormat::Mono });
    s.busses.push(soundcraft_model::Bus { id: b2, name: "2".into(), format: ChannelFormat::Mono });
    let mx = module(&mut s, x, PROBE_ID, Some((y, false)));
    let ma = module(&mut s, a, PROBE_ID, Some((x, false)));
    let r: Vec<_> = sidechain_routes(&s).into_iter().map(|r| r.result).collect();
    // A feeds X through bus 1; X feeds Y through bus 2.
    assert_eq!(r, [Err(SidechainProblem::Cycle), Err(SidechainProblem::Cycle)]);
    let mut eng = engine(&s, vec![probe(x, mx, 1), probe(a, ma, 1)]);
    assert!(render(&mut eng, &s, 3).iter().flatten().all(|x| *x == 0.0));
}

#[test]
fn an_aux_takes_a_key_from_a_track_and_from_an_earlier_aux() {
    let mut s = session();
    let b1 = BusId(911);
    s.busses.push(soundcraft_model::Bus { id: b1, name: "1".into(), format: ChannelFormat::Mono });
    // Listed first, keyed from an aux listed after it and from a track.
    let x = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("X"));
    let y = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("Y"));
    let kick = track(&mut s, "Kick", ChannelFormat::Mono, &dc(0.5), Route::Bus(b1));
    {
        let m = &mut s.track_mut(x).unwrap().mixer;
        m.output = Route::Main;
        m.pan = vec![-1.0];
    }
    {
        let m = &mut s.track_mut(y).unwrap().mixer;
        m.input = Route::Bus(b1);
        m.output = Route::Main;
        m.pan = vec![1.0];
    }
    let mx = module(&mut s, x, PROBE_ID, Some((y, false)));
    assert_eq!(sidechain_routes(&s)[0].result, Ok(()));
    let mut eng = engine(&s, vec![probe(x, mx, 1)]);
    // X (left) puts out its key: Y's output (right).
    let out = render(&mut eng, &s, 3);
    let (l, r) = (last(&out[0]), last(&out[1]));
    assert!(l > 0.1 && (l - r).abs() < 1e-6, "{l} vs {r}");
    // And straight from the track.
    s.track_mut(x).unwrap().mixer.inserts[0].sidechain = Some(SidechainSource { track: kick, pre_fader: false });
    let mut eng = engine(&s, vec![probe(x, mx, 1)]);
    assert!((last(&render(&mut eng, &s, 3)[0]) - 0.5).abs() < 1e-5);
    // A track can't wait for an aux (auxes render after it): unsupported, silent.
    let a = track(&mut s, "A", ChannelFormat::Mono, &dc(0.5), Route::Main);
    let ma = module(&mut s, a, PROBE_ID, Some((x, false)));
    let r = sidechain_routes(&s);
    assert!(matches!(r.last().unwrap().result, Err(SidechainProblem::Unsupported(_))), "{r:?}");
    let mut eng = engine(&s, vec![probe(x, mx, 1), probe(a, ma, 1)]);
    let out = render(&mut eng, &s, 3);
    assert!((last(&out[0]) - 0.5).abs() < 1e-5, "only X's key on the left: {}", last(&out[0]));
}

#[test]
fn a_stereo_source_keys_a_mono_module_with_its_average() {
    let mut s = session();
    let c = track(&mut s, "Consumer", ChannelFormat::Mono, &[], Route::Main);
    let src = track(&mut s, "Pad", ChannelFormat::Stereo, &[], Route::None);
    // Left 0.6, right 0.2 (a stereo clip of different channels).
    let id = SourceId(77);
    s.pool.insert(id, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR as u32, channels: vec![vec![0.6; 8 * BLOCK], vec![0.2; 8 * BLOCK]] })));
    let clip = s.new_clip_id();
    s.track_mut(src).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "pad", id, 0, 0, 8 * BLOCK as i64));
    let m = module(&mut s, c, PROBE_ID, Some((src, true)));
    let mut eng = engine(&s, vec![probe(c, m, 1)]);
    let out = render(&mut eng, &s, 3);
    assert!((last(&out[0]) - 0.4).abs() < 1e-5, "{}", last(&out[0]));
}

/// The first sample at or above half of the peak.
fn onset(x: &[f32]) -> usize {
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    x.iter().position(|v| v.abs() >= 0.5 * peak && peak > 0.0).unwrap_or(usize::MAX)
}

/// An impulse at `at` (a step, so it survives sample-exact comparisons).
fn step(at: usize) -> Vec<f32> {
    (0..8 * BLOCK).map(|n| if n >= at { 0.5 } else { 0.0 }).collect()
}

#[test]
fn the_key_lines_up_with_the_main_input_when_either_has_latency() {
    const D: usize = 300;
    const AT: usize = 1000;
    // (latency on the source, latency on the consumer before the module, key before the fader)
    for (src_lat, own_lat, pre) in [(D, 0, false), (0, D, false), (D, D / 2, false), (D, 0, true), (D / 2, D, true)] {
        let mut s = session();
        // The consumer (mono, hard left), a reference track playing the same step (hard right),
        // and the source, which goes nowhere.
        let c = track(&mut s, "Consumer", ChannelFormat::Mono, &[], Route::Main);
        let r = track(&mut s, "Reference", ChannelFormat::Mono, &step(AT), Route::Main);
        s.track_mut(r).unwrap().mixer.pan = vec![1.0];
        let src = track(&mut s, "Source", ChannelFormat::Mono, &step(AT), Route::None);
        let mut inst = Vec::new();
        stub_on(&mut s, src, src_lat, &mut inst);
        stub_on(&mut s, c, own_lat, &mut inst);
        let m = module(&mut s, c, PROBE_ID, Some((src, pre)));
        inst.push(probe(c, m, 1));
        let mut eng = engine(&s, inst);
        let out = render(&mut eng, &s, 8);
        let (l, rr) = (onset(&out[0]), onset(&out[1]));
        assert_eq!(l, rr, "source {src_lat}, own {own_lat}, pre {pre}: key at {l}, reference at {rr}");
        assert_eq!(eng.latency, src_lat.max(own_lat), "the later side sets the strip's latency");
    }
}

/// Adds a LatencyStub of `n` samples to (mono) track `t`.
fn stub_on(s: &mut Session, t: TrackId, n: usize, inst: &mut Vec<PreparedInstance>) -> ModuleId {
    let mut ins = Insert::new(STUB_ID);
    ins.params.insert("n".into(), n as f32);
    let m = s.track_mut(t).unwrap().mixer.add_module(ins, None).unwrap();
    inst.push(stub(t, m, 1));
    m
}

/// A mono aux into the main mix, hard left, reading `input`.
fn aux(s: &mut Session, name: &str, input: Route) -> TrackId {
    let x = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some(name));
    let m = &mut s.track_mut(x).unwrap().mixer;
    m.input = input;
    m.output = Route::Main;
    m.pan = vec![-1.0];
    x
}

/// A reference track playing the step hard right, straight into the main mix.
fn reference(s: &mut Session, at: usize) -> TrackId {
    let r = track(s, "Reference", ChannelFormat::Mono, &step(at), Route::Main);
    s.track_mut(r).unwrap().mixer.pan = vec![1.0];
    r
}

#[test]
fn an_aux_consumer_lines_its_key_up_with_the_mix() {
    const D: usize = 300;
    const AT: usize = 1000;
    for (with_input, pre) in [(false, false), (true, false), (true, true)] {
        let mut s = session();
        reference(&mut s, AT);
        let src = track(&mut s, "Source", ChannelFormat::Mono, &step(AT), Route::None);
        let mut inst = Vec::new();
        stub_on(&mut s, src, D, &mut inst);
        // The aux reads a bus fed by a silent track (or has no input at all).
        let input = if with_input {
            let b = BusId(931);
            s.busses.push(soundcraft_model::Bus { id: b, name: "1".into(), format: ChannelFormat::Mono });
            track(&mut s, "Feed", ChannelFormat::Mono, &[], Route::Bus(b));
            Route::Bus(b)
        } else {
            Route::None
        };
        let x = aux(&mut s, "X", input);
        let m = module(&mut s, x, PROBE_ID, Some((src, pre)));
        inst.push(probe(x, m, 1));
        let mut eng = engine(&s, inst);
        let out = render(&mut eng, &s, 8);
        let (l, r) = (onset(&out[0]), onset(&out[1]));
        assert_eq!(l, r, "input {with_input}, pre {pre}: key at {l}, reference at {r}");
    }
}

#[test]
fn an_aux_keyed_from_a_latent_aux_lines_up() {
    const D: usize = 300;
    const AT: usize = 1000;
    for pre in [false, true] {
        let mut s = session();
        reference(&mut s, AT);
        let b = BusId(941);
        s.busses.push(soundcraft_model::Bus { id: b, name: "1".into(), format: ChannelFormat::Mono });
        track(&mut s, "Source", ChannelFormat::Mono, &step(AT), Route::Bus(b));
        // Listed first: X keyed from Y, an aux with latency that reads the source's bus.
        let x = aux(&mut s, "X", Route::None);
        let y = aux(&mut s, "Y", Route::Bus(b));
        s.track_mut(y).unwrap().mixer.output = Route::None;
        let mut inst = Vec::new();
        stub_on(&mut s, y, D, &mut inst);
        let m = module(&mut s, x, PROBE_ID, Some((y, pre)));
        inst.push(probe(x, m, 1));
        let mut eng = engine(&s, inst);
        let out = render(&mut eng, &s, 8);
        let (l, r) = (onset(&out[0]), onset(&out[1]));
        assert_eq!(l, r, "pre {pre}: key at {l}, reference at {r}");
    }
}

#[test]
fn keys_from_a_latent_source_keep_their_buffers() {
    let mut s = session();
    let c = track(&mut s, "Consumer", ChannelFormat::Mono, &dc(0.25), Route::Main);
    let src = track(&mut s, "Source", ChannelFormat::Mono, &dc(0.5), Route::None);
    let mut inst = Vec::new();
    let st = stub_on(&mut s, src, 400, &mut inst);
    let m = module(&mut s, c, PROBE_ID, Some((src, false)));
    inst.push(probe(c, m, 1));
    let mut eng = engine(&s, inst);
    render(&mut eng, &s, 2);
    let before = (eng.key_buffers(c), eng.delay_buffers(c), eng.delay_buffers(src));
    assert!(!before.0.is_empty());
    // The source's latency changes while playing (within the reserved headroom).
    for (i, n) in [100usize, 900, 0, 650].into_iter().enumerate() {
        s.track_mut(src).unwrap().mixer.module_mut(st).unwrap().params.insert("n".into(), n as f32);
        let mut buf = vec![vec![0.0f32; BLOCK]; 2];
        for b in 0..50 {
            eng.render(&s, ((2 + i * 50 + b) * BLOCK) as i64, BLOCK, &mut buf);
        }
        assert!(buf.iter().flatten().all(|x| x.is_finite()));
    }
    eng.reset();
    render(&mut eng, &s, 4);
    assert_eq!((eng.key_buffers(c), eng.delay_buffers(c), eng.delay_buffers(src)), before, "no reallocation");
}

#[test]
fn rendering_with_keys_keeps_its_buffers() {
    let (s, c, _, m) = keyed_session(false);
    let mut eng = engine(&s, vec![probe(c, m, 2)]);
    render(&mut eng, &s, 2);
    let before = eng.key_buffers(c);
    assert!(!before.is_empty());
    render(&mut eng, &s, 200);
    eng.reset();
    render(&mut eng, &s, 20);
    assert_eq!(eng.key_buffers(c), before, "no reallocation while rendering or resetting");
}

#[test]
fn sends_still_order_auxes_with_keys_present() {
    // A pre-fader key source that also has a send keeps both working.
    let (mut s, c, src, m) = keyed_session(true);
    let b = BusId(921);
    s.busses.push(soundcraft_model::Bus { id: b, name: "1".into(), format: ChannelFormat::Mono });
    let mut send = SendSlot::new(Route::Bus(b));
    send.level_db = 0.0;
    send.pre_fader = true;
    s.track_mut(src).unwrap().mixer.sends = vec![Some(send)];
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("Aux"));
    {
        let mx = &mut s.track_mut(aux).unwrap().mixer;
        mx.input = Route::Bus(b);
        mx.output = Route::Main;
        mx.pan = vec![1.0];
    }
    let mut eng = engine(&s, vec![probe(c, m, 2)]);
    let out = render(&mut eng, &s, 4);
    assert!((last(&out[0]) - 0.5).abs() < 1e-5, "key: {}", last(&out[0]));
    assert!(last(&out[1]) - 0.5 > 0.3, "key + the pre-fader send through the aux: {}", last(&out[1]));
}
