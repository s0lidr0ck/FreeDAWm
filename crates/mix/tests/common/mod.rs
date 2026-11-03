//! A latent test plugin shared by the integration tests: a pure delay whose latency is its param
//! `n` (samples, 0–4096), adopted through the external-instance path (a `clap:` id).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_dsp::{Category, ParamInfo, Plugin, PluginInfo, Taper, Unit};
use soundcraft_mix::{MixEngine, PreparedInstance};
use soundcraft_model::{Insert, ModuleId, Session, TrackId};

pub const SR: f32 = 48_000.0;
pub const DELAY_ID: &str = "clap:test.delay_stub";
const RING: usize = 4097;

static PARAMS: [ParamInfo; 1] = [ParamInfo {
    id: "n",
    name: "Delay",
    min: 0.0,
    max: 4096.0,
    default: 0.0,
    unit: Unit::None,
    taper: Taper::Linear,
    choices: &[],
    display: None,
    group: "",
}];

static INFO: PluginInfo = PluginInfo {
    id: DELAY_ID,
    name: "Delay Stub",
    short_name: "DlyStub",
    category: Category::Other,
    params: &PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// A pure delay of `n` samples that reports `n` as its latency.
#[derive(Default)]
pub struct DelayStub {
    n: usize,
    rings: Vec<Vec<f32>>,
    w: usize,
}

impl Plugin for DelayStub {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, _sr: f32, _mb: usize, ch: usize) {
        self.rings = vec![vec![0.0; RING]; ch];
        self.w = 0;
    }
    fn reset(&mut self) {
        self.rings.iter_mut().for_each(|r| r.fill(0.0));
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        if id != "n" {
            return false;
        }
        self.n = PARAMS[0].clamp(v).round() as usize;
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        (id == "n").then_some(self.n as f32)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let d = self.n;
        let mut w = self.w;
        for (c, ch) in io.iter_mut().enumerate() {
            let Some(ring) = self.rings.get_mut(c) else { continue };
            w = self.w;
            for x in ch.iter_mut().take(frames) {
                ring[w] = *x;
                *x = ring[(w + RING - d) % RING];
                w = (w + 1) % RING;
            }
        }
        self.w = w;
    }
    fn latency(&self) -> usize {
        self.n
    }
}

/// A delay-stub module of `n` samples, added to `track`'s chain.
pub fn add_delay(s: &mut Session, track: TrackId, n: usize) -> ModuleId {
    let mut ins = Insert::new(DELAY_ID);
    ins.params.insert("n".into(), n as f32);
    s.track_mut(track).unwrap().mixer.add_module(ins, None).unwrap()
}

/// An engine for `s` with a fresh delay stub adopted for every delay-stub module.
pub fn engine(s: &Session, block: usize) -> MixEngine {
    engine_at(s, block, SR)
}

/// [`engine`] at sample rate `sr`.
pub fn engine_at(s: &Session, block: usize, sr: f32) -> MixEngine {
    let mut eng = MixEngine::new(sr, block);
    eng.set_external_instances(true);
    adopt_stubs(&mut eng, s);
    eng
}

/// Hands `eng` a fresh delay stub for every delay-stub module of `s`.
pub fn adopt_stubs(eng: &mut MixEngine, s: &Session) {
    let mut v = Vec::new();
    for spec in soundcraft_mix::instance_specs(s) {
        if spec.id != DELAY_ID {
            continue;
        }
        let mut p = DelayStub::default();
        p.prepare(SR, eng.max_block(), spec.ch); // the stub ignores the rate
        v.push(PreparedInstance { track: spec.track, slot: spec.slot, id: spec.id, ch: spec.ch, plugin: Box::new(p) });
    }
    eng.adopt(v);
}
