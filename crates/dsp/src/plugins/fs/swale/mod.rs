//! `fs_swale`: the owner's Swale engine (automatic volume swells: Pick / One-Shot / Rhythmic,
//! Clean or Slow Gear VCA, Bloom, Alternate), a faithful translation of
//! `Swale/Source/SwaleDSP.h`, with the editor's tables from `SwaleConfig.h` in [`swale_config`].
//!
//! Wrapper (`PluginProcessor.{h,cpp}`) audit, everything that affects the sound:
//! - `prepareToPlay` calls `dsp.prepare (sampleRate)` and raises `forcePush`, so the first
//!   `processBlock` pushes every parameter (in parameter order) before processing. `prepare`
//!   keeps the parameter state but restarts Level, Sens, Retrig's re-arm ratio and the detector
//!   low cut at their defaults (the forced push puts them back) and starts the Level glide at the
//!   default Level.
//! - `processBlock` first hands the engine the host transport, `setHostTransport (playing, bpm,
//!   ppq, ppqValid)` (bpm 0 when the host gives none), which also pulls the Rhythmic clock onto
//!   the host's grid (`lockToHost`, while playing on Host tempo). Here [`Plugin::set_transport`]
//!   does it with SoundCraft's `Transport` (`playing`, `tempo`, `quarters`), which the mix engine
//!   gives before every `process`, so the order is the C++'s: transport (locking with the Rate
//!   and Tempo Source pushed so far), then the parameter push, then the audio. A non-finite tempo
//!   counts as none, a non-finite position as `ppqValid == false`.
//! - Then it pushes only what moved, in parameter order (`v != lastPushed[i]`). That matters:
//!   Mode's setter resets the state machine, so pushing it every block would stop a swell ever
//!   completing. Here `set_param` only stores the value; `process` does the wrapper's push (all
//!   values after `prepare`/`reset`, else the changed ones).
//! - Latency: none (no lookahead). Tail: `getTailLengthSeconds` is 0; the VCA has no memory of
//!   its own, only Bloom's two one-pole stages (at 300 Hz at the lowest, gone within a few ms), so
//!   `tail_samples` is a computed 20 ms, which is at least the C++ value.
//! - Bypass: the engine's own `bypass` parameter passes the input straight through (the detector
//!   and clocks keep running), translated in the engine. It is kept in order but hidden (group
//!   ""): the UI uses the module's bypass.
//! - APVTS ranges: floats are 0..1, choices are whole indices, bools 0/1. `ParamInfo::clamp` does
//!   the same (NaN becomes the default), so the engine never sees a non-finite value.
//! - Bus layout: mono or stereo, in = out. `dsp.process` gets the channel count (at most 2);
//!   with one channel the engine reads the left as both sides, writes only the left and skips
//!   Alternate, as the C++. Extra channels are left untouched (the Rust host contract).
//! - `ScopedNoDenormals` (flush-to-zero): no safe Rust equivalent. It only changes values below
//!   ~1e-38, far under the fidelity bar, and no decision of the engine can see one (see
//!   [`swale_dsp`]); `silence_tail_is_cheap` guards the CPU side.
//! - The preset dirty-tracking (`parameterChanged`), state save and the editor are UI-only.
//!   `getEffectiveBpm` (the tempo the editor prints) is the engine's own `bpmOut`, readout `[5]`.
//! - Reset (the DAW's, on every Play, Stop and Locate): the C++ `prepareToPlay` on the live
//!   instance (`dsp.prepare` at the same rate, then the forced push, which clears every swell,
//!   follower, filter and the clock but keeps the parameters), with one departure by controller
//!   ruling: once the push has run, every glide (Input, Level, Mix, Floor) starts at its target,
//!   so a reset never re-glides Level up from its default. No allocation: the engine is plain
//!   data in its box.
//!
//! DAW departure (controller ruling, as for Mod and Filter): the BPM knob and the Tempo Source
//! switch are hidden in the drawer. The session always gives a tempo, and the engine's Host path
//! follows it exactly as the C++ follows a host's; the parameters themselves are unchanged.
//!
//! Station (`ModuleRegistry.cpp`): `fixed ("swale", "Swale", "mode", 0xFFF2B84B, ..,
//! list ("attack,hold,release,mix"))`: model param Mode, key knobs Attack, Hold, Release, Mix.

mod presets;
pub mod swale_config;
pub mod swale_dsp;

pub use presets::PRESETS;
pub use swale_dsp::{NUM_PARAMS, SwaleDsp};

use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Transport, gain_to_db};
use swale_config::{CURVES, HOLD_NOTES, MODES, NOTES, PHRASES, TEMPO_SOURCES, VCAS, format_value};
use swale_dsp as d;

macro_rules! display_fns {
    ($($name:ident => $idx:expr),* $(,)?) => {$(
        fn $name(v: f32) -> String {
            format_value($idx, v)
        }
    )*};
}
display_fns!(
    attack_text => d::P_ATTACK,
    hold_text => d::P_HOLD,
    release_text => d::P_RELEASE,
    sens_text => d::P_SENS,
    mix_text => d::P_MIX,
    level_text => d::P_LEVEL,
    trim_text => d::P_TRIM,
    retrig_text => d::P_RETRIG,
    det_hp_text => d::P_DET_HP,
    floor_text => d::P_FLOOR,
    bloom_text => d::P_BLOOM,
    alternate_text => d::P_ALTERNATE,
    bpm_text => d::P_BPM,
);

/// The drawer's two groups, as the editor's panels: the swell (the mode, the three stages with
/// their sync, Sens, Rate, Bloom, Mix, the curve row and the tempo foot, which the drawer hides);
/// and the More panel (Retrig, Low Cut, Floor, Alternate, Input, Level, the VCA).
const SWELL: &str = "Swell";
const MORE: &str = "More";

/// Parameters in the C++ order (`Parameters.h`), ids, names and defaults verbatim.
pub static SWALE_PARAMS: [ParamInfo; NUM_PARAMS] = [
    knob01("attack", "Attack", 0.55, SWELL, attack_text),
    knob01("hold", "Hold", 0.0, SWELL, hold_text),
    knob01("release", "Release", 0.5, SWELL, release_text),
    knob01("sens", "Sens", 0.5, SWELL, sens_text),
    knob01("mix", "Mix", 1.0, SWELL, mix_text),
    knob01("level", "Level", 0.6667, MORE, level_text),
    fs_choice("mode", "Mode", 0.0, SWELL, &MODES),
    // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
    fs_toggle("bypass", "Bypass", 0.0, ""),
    knob01("trim", "Input", 0.3333, MORE, trim_text),
    fs_choice("curve", "Curve", 1.0, SWELL, &CURVES),
    knob01("retrig", "Retrigger", 0.7, MORE, retrig_text),
    knob01("detHp", "Det Low Cut", 0.3, MORE, det_hp_text),
    knob01("floorAmt", "Floor", 0.0, MORE, floor_text),
    knob01("bloom", "Bloom", 0.0, SWELL, bloom_text),
    fs_choice("vca", "VCA", 0.0, MORE, &VCAS),
    knob01("alternate", "Alternate", 0.0, MORE, alternate_text),
    fs_toggle("atkSync", "Attack Sync", 0.0, SWELL),
    fs_choice("atkNote", "Attack Note", 9.0, SWELL, &NOTES),
    fs_toggle("holdSync", "Hold Sync", 0.0, SWELL),
    fs_choice("holdNote", "Hold Note", 18.0, SWELL, &HOLD_NOTES),
    fs_toggle("relSync", "Release Sync", 0.0, SWELL),
    fs_choice("relNote", "Release Note", 12.0, SWELL, &NOTES),
    fs_choice("rate", "Rate", 6.0, SWELL, &NOTES),
    fs_toggle("peakBeat", "Peak on Beat", 1.0, SWELL),
    fs_choice("tempoSrc", "Tempo Source", 0.0, SWELL, &TEMPO_SOURCES),
    knob01("bpm", "BPM", 0.4, SWELL, bpm_text),
    fs_choice("phrase", "Swell", 1.0, SWELL, &PHRASES),
];

pub static SWALE_INFO: PluginInfo = PluginInfo {
    id: "fs_swale",
    name: "Swale",
    short_name: "Swale",
    category: Category::FireStation,
    params: &SWALE_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &SWALE_INFO,
    make: || Box::new(Swale::new()),
    presets: PRESETS,
    order: 140,
    fidelity: Some(super::Fidelity { name: "swale", source_dir: "Swale", render_tool: "Swale", tables: &[] }),
    prepare_state: None,
    actions: &[],
};

/// `readouts[4]` keeps the trigger count to 24 bits, so every count is exact as an `f32`.
pub const TRIG_MASK: i32 = 0x00FF_FFFF;

/// The Swale port.
///
/// `readouts`: `[0]` input peak dBFS and `[1]` output peak dBFS over the last block (Rust-side
/// meters; the C++ has none), then the engine's own UI outputs as the editor polls them: `[2]`
/// the VCA opening 0..1 after Curve (`envOut`), `[3]` the detector level, linear (`detOut`),
/// `[4]` the trigger count (`trigSeq`, bumped on every swell; masked by [`TRIG_MASK`]), `[5]` the
/// tempo in use (`bpmOut`).
pub struct Swale {
    p: Params,
    dsp: Box<SwaleDsp>,
    /// The wrapper's `lastPushed` and `forcePush`.
    last_pushed: [f32; NUM_PARAMS],
    force_push: bool,
    /// A reset is waiting for its push: start the glides at their targets once it has run.
    seed_glides: bool,
    channels: usize,
    in_peak_db: f32,
    out_peak_db: f32,
}

impl Default for Swale {
    fn default() -> Self {
        Self::new()
    }
}

impl Swale {
    pub fn new() -> Self {
        Swale {
            p: Params::new(&SWALE_PARAMS),
            dsp: Box::new(SwaleDsp::new()),
            last_pushed: [0.0; NUM_PARAMS],
            force_push: true,
            seed_glides: false,
            channels: 2,
            in_peak_db: f32::NAN,
            out_peak_db: f32::NAN,
        }
    }

    /// The top of `processBlock`: push only what moved (everything after a prepare), in
    /// parameter order.
    fn push_params(&mut self) {
        let force = std::mem::take(&mut self.force_push);
        for i in 0..NUM_PARAMS {
            let v = self.p.v(i);
            if force || v != self.last_pushed[i] {
                self.last_pushed[i] = v;
                self.dsp.set_parameter(i, v);
            }
        }
    }

    /// The engine (tests).
    #[cfg(test)]
    pub(crate) fn engine(&self) -> &SwaleDsp {
        &self.dsp
    }
}

fn peak(io: &[Vec<f32>], nc: usize, frames: usize) -> f32 {
    io.iter().take(nc).flat_map(|c| c.iter().take(frames)).fold(0.0f32, |m, &x| if x.abs() > m { x.abs() } else { m })
}

impl Plugin for Swale {
    fn info(&self) -> &'static PluginInfo {
        &SWALE_INFO
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.p.set(id, value).is_some()
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        // `prepareToPlay`: prepare the engine (nothing to size: it is plain data, made in
        // `new`), then force the push of every value before the next block.
        self.channels = channels.min(2);
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        self.dsp.prepare(sr);
        self.force_push = true;
        self.seed_glides = false;
        self.in_peak_db = f32::NAN;
        self.out_peak_db = f32::NAN;
    }

    fn reset(&mut self) {
        // `prepareToPlay` on the live instance (see the module docs), the glides seeded at their
        // targets once the forced push has run.
        let sr = self.dsp.sample_rate();
        self.dsp.clear_bloom_coef();
        self.dsp.prepare(sr);
        self.force_push = true;
        self.seed_glides = true;
        self.in_peak_db = f32::NAN;
        self.out_peak_db = f32::NAN;
    }

    fn set_transport(&mut self, transport: &Transport) {
        // `pos->getBpm()` (0 when absent) and `pos->getPpqPosition()` (`ppqOk`).
        let bpm = if transport.tempo.is_finite() { transport.tempo as f32 } else { 0.0 };
        let ppq_ok = transport.quarters.is_finite();
        let ppq = if ppq_ok { transport.quarters } else { 0.0 };
        self.dsp.set_host_transport(transport.playing, bpm, ppq, ppq_ok);
    }

    fn tail_samples(&self) -> usize {
        // max (getTailLengthSeconds = 0, Bloom's few milliseconds): 20 ms.
        (0.02 * self.dsp.sample_rate()).max(0.0) as usize
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.push_params();
        if std::mem::take(&mut self.seed_glides) {
            self.dsp.seed_glides();
        }
        let nc = self.channels.min(io.len());
        self.in_peak_db = gain_to_db(peak(io, nc, frames));
        if nc > 0 {
            self.dsp.process(io, nc, frames);
        }
        self.out_peak_db = gain_to_db(peak(io, nc, frames));
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        out[0] = self.in_peak_db;
        out[1] = self.out_peak_db;
        out[2] = self.dsp.env_out();
        out[3] = self.dsp.det_out();
        out[4] = (self.dsp.trig_seq() & TRIG_MASK) as f32;
        out[5] = self.dsp.bpm_out();
    }
}

#[cfg(test)]
mod tests;
