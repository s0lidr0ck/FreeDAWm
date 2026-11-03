//! `fs_filter`: the owner's Filter engine (Q-Tron / Mu-Tron / Cry Baby / Vox / Moog / MS-20 / SEM /
//! TB-303 / Wasp, with Manual / Envelope / LFO / Steps modulation), a faithful translation of
//! `Engine 5 - Filter/Source/FilterEngineDSP.h`, with the editor's tables from `FilterConfig.h`
//! in [`filter_config`].
//!
//! Wrapper (`PluginProcessor.{h,cpp}`) audit, everything that affects the sound:
//! - `prepareToPlay` calls `dsp.prepare (sampleRate)` and raises `forcePush`, so the first
//!   `processBlock` pushes every parameter (in parameter order) before processing.
//! - `processBlock` first hands the engine the host transport, `setHostTransport (playing, bpm,
//!   ppq)` (bpm 0 and ppq -1 when the host gives none), which also pulls the LFO and the step
//!   sequencer onto the host's grid (`lockToHost`). Here [`Plugin::set_transport`] does it with
//!   SoundCraft's `Transport` (`playing`, `tempo`, `quarters`), which the mix engine gives
//!   before every `process`, so the order is the C++'s: transport, then the parameter push, then
//!   the audio. A non-finite tempo or position counts as none (the JUCE host's "no value").
//! - Then it pushes only what moved, in parameter order (`v != lastPushed[i]`). That matters:
//!   Circuit's setter re-derives the model and clears every solver state, so pushing it every
//!   block would stop the filter ever running. Here `set_param` only stores the value; `process`
//!   does the wrapper's push (all values after `prepare`/`reset`, else the changed ones).
//! - Latency: none (IIR only). Tail: `getTailLengthSeconds` is 1 s (a self-oscillating filter
//!   keeps ringing), so `tail_samples` is one second.
//! - Bypass: the engine's own `bypass` parameter passes the input straight through (the
//!   modulation keeps running), translated in the engine. It is kept in order but hidden
//!   (group ""): the UI uses the module's bypass.
//! - APVTS ranges: floats are 0..1, choices are whole indices (Steps is a 15-way choice of the
//!   counts, read by the DSP as `count - 2`), bools 0/1. `ParamInfo::clamp` does the same (NaN
//!   becomes the default), so the engine never sees a non-finite value.
//! - Bus layout: mono or stereo, in = out. `dsp.process` gets the channel count (at most 2);
//!   with one channel the engine runs its second channel on a copy of the first and writes only
//!   the first, as the C++. Extra channels are left untouched (the Rust host contract).
//! - `ScopedNoDenormals` (flush-to-zero): no safe Rust equivalent. It only changes values below
//!   ~1e-38, far under the fidelity bar; `silence_tail_is_cheap` guards the CPU side.
//! - The preset dirty-tracking (`parameterChanged`), the diagnostics log, state save and the
//!   editor are UI-only. `getEffectiveBpm` (the BPM the editor prints) is the engine's own
//!   `bpmOut`, readout `[4]`.
//! - Reset: the host calls `reset` on every Play / Stop / Locate and at bounce start. Until audio
//!   has flowed, `reset` (and `prepare`) is the fresh-instance path: the engine as `new` made it
//!   (the member defaults, then the constructor's `prepare (48000.0)`), prepared at the rate,
//!   with a forced push, which the fidelity cases measure. After that it is `prepareToPlay` on
//!   the LIVE instance: what moved is pushed, the audio state cleared, `dsp.prepare` run, and
//!   the C++'s forced push done at once (its `prepare` writes the step values and envelope times
//!   back to defaults that only the push restores); then [`FilterEngineDsp::seat_live`] seats
//!   the glides at their targets and starts the modulation from rest, so nothing sweeps and two
//!   histories render the same. Both paths keep the host transport the engine was given, so
//!   the tempo lock is not dropped; the next `setHostTransport` re-locks the grid. No allocation.
//!
//! DAW departure (controller ruling, as for Mod): the BPM knob and the Tempo Source switch are
//! hidden in the drawer. The session always gives a tempo, and the engine's Host path follows it
//! exactly as the C++ follows a host's; the parameters themselves are unchanged.
//!
//! Station (`ModuleRegistry.cpp`): `fixed ("filter", "Filter", "circuit", 0xFF35E0C2, ..,
//! list ("cutoff,reso,drive,mix"))`: model param Circuit, key knobs Cutoff, Reso, Drive, Mix.

pub mod filter_config;
pub mod filter_engine_dsp;
mod presets;

pub use filter_engine_dsp::{FilterEngineDsp, NUM_PARAMS};
pub use presets::PRESETS;

use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Transport, gain_to_db};
use filter_config::{NOTES, STEP_COUNTS, format_value};
use filter_engine_dsp as d;

/// The tempo the value texts assume for the Note readout. The editor prints the length at the
/// tempo in force (`formatValue (index, v, bpm)`), which a parameter's own `fn(f32)` text cannot
/// know, so it assumes 120 BPM. Unseen in the drawer: Note is a choice, shown as its label in a
/// pill; [`filter_config::format_value`] takes the real tempo for any caller that has it.
const TEXT_BPM: f32 = 120.0;

macro_rules! display_fns {
    ($($name:ident => $idx:expr),* $(,)?) => {$(
        fn $name(v: f32) -> String {
            format_value($idx, v, TEXT_BPM)
        }
    )*};
}
display_fns!(
    cutoff_text => d::P_CUTOFF,
    reso_text => d::P_RESO,
    drive_text => d::P_DRIVE,
    mix_text => d::P_MIX,
    level_text => d::P_LEVEL,
    trim_text => d::P_TRIM,
    depth_text => d::P_DEPTH,
    env_sens_text => d::P_ENV_SENS,
    env_atk_text => d::P_ENV_ATK,
    env_rel_text => d::P_ENV_REL,
    lfo_rate_text => d::P_LFO_RATE,
    lfo_stereo_text => d::P_LFO_STEREO,
    glide_text => d::P_SEQ_GLIDE,
    bpm_text => d::P_BPM,
    step_text => d::P_STEP1,
);

/// The drawer's two groups, as the editor's sections: the circuit with its knobs, switches and
/// the setup row's Input and Level; and the modulation panel (with the step row, and the tempo
/// controls, which the drawer hides).
const FILTER: &str = "Filter";
const MOD: &str = "Mod";

const CIRCUITS: [&str; 9] = ["Q-Tron", "Mu-Tron", "Cry Baby", "Vox", "Moog", "MS-20", "SEM", "TB-303", "Wasp"];

/// Parameters in the C++ order (`Parameters.h`), ids, names and defaults verbatim.
pub static FILTER_PARAMS: [ParamInfo; NUM_PARAMS] = [
    knob01("cutoff", "Cutoff", 0.5, FILTER, cutoff_text),
    knob01("reso", "Reso", 0.5, FILTER, reso_text),
    knob01("drive", "Drive", 0.0, FILTER, drive_text),
    knob01("mix", "Mix", 1.0, FILTER, mix_text),
    knob01("level", "Level", 0.6667, FILTER, level_text),
    fs_choice("circuit", "Circuit", 0.0, FILTER, &CIRCUITS),
    // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
    fs_toggle("bypass", "Bypass", 0.0, ""),
    knob01("trim", "Input", 0.3333, FILTER, trim_text),
    fs_choice("modSrc", "Mod Source", 1.0, MOD, &["Manual", "Envelope", "LFO", "Steps"]),
    knob01("depth", "Mod Depth", 0.85, MOD, depth_text),
    knob01("envSens", "Env Sens", 0.4, MOD, env_sens_text),
    knob01("envAtk", "Env Attack", 0.2, MOD, env_atk_text),
    knob01("envRel", "Env Release", 0.4, MOD, env_rel_text),
    fs_choice("lfoShape", "LFO Shape", 0.0, MOD, &["Sine", "Triangle", "Saw Up", "Saw Down", "Square", "S&H", "Drift"]),
    fs_toggle("lfoSync", "LFO Sync", 1.0, MOD),
    knob01("lfoRate", "LFO Rate", 0.4, MOD, lfo_rate_text),
    fs_choice("note", "Note", 9.0, MOD, &NOTES),
    knob01("lfoStereo", "LFO Stereo", 0.0, MOD, lfo_stereo_text),
    fs_choice("seqSteps", "Steps", 6.0, MOD, &STEP_COUNTS),
    knob01("seqGlide", "Glide", 0.1, MOD, glide_text),
    fs_choice("tempoSrc", "Tempo Source", 0.0, MOD, &["Host", "Manual"]),
    knob01("bpm", "BPM", 0.4, MOD, bpm_text),
    knob01("step1", "Step 1", 0.20, MOD, step_text),
    knob01("step2", "Step 2", 0.80, MOD, step_text),
    knob01("step3", "Step 3", 0.45, MOD, step_text),
    knob01("step4", "Step 4", 0.65, MOD, step_text),
    knob01("step5", "Step 5", 0.10, MOD, step_text),
    knob01("step6", "Step 6", 0.90, MOD, step_text),
    knob01("step7", "Step 7", 0.35, MOD, step_text),
    knob01("step8", "Step 8", 0.55, MOD, step_text),
    knob01("step9", "Step 9", 0.20, MOD, step_text),
    knob01("step10", "Step 10", 0.80, MOD, step_text),
    knob01("step11", "Step 11", 0.45, MOD, step_text),
    knob01("step12", "Step 12", 0.65, MOD, step_text),
    knob01("step13", "Step 13", 0.10, MOD, step_text),
    knob01("step14", "Step 14", 0.90, MOD, step_text),
    knob01("step15", "Step 15", 0.35, MOD, step_text),
    knob01("step16", "Step 16", 0.55, MOD, step_text),
    // per-circuit switches
    fs_choice("qtRange", "QT Range", 1.0, FILTER, &["Lo", "Hi"]),
    fs_choice("qtMode", "QT Mode", 0.0, FILTER, &["LP", "BP", "HP", "Mix"]),
    fs_choice("muRange", "Mu Range", 1.0, FILTER, &["Lo", "Hi"]),
    fs_choice("muMode", "Mu Mode", 0.0, FILTER, &["LP", "BP", "HP"]),
    fs_choice("cbInductor", "CB Inductor", 0.0, FILTER, &["Red Fasel", "Yellow Fasel", "Halo", "TDK"]),
    fs_choice("cbVoice", "CB Sweep", 0.0, FILTER, &["Stock", "Deep", "Vocal"]),
    fs_choice("voxVoice", "Vox Model", 0.0, FILTER, &["V846", "Clyde McCoy"]),
    fs_choice("moogSlope", "Moog Slope", 0.0, FILTER, &["24 dB", "18 dB", "12 dB", "6 dB"]),
    fs_choice("moogComp", "Moog Bass", 0.0, FILTER, &["Stock", "Comp"]),
    fs_choice("msMode", "MS Mode", 0.0, FILTER, &["LP", "HP", "HP+LP"]),
    fs_choice("semMode", "SEM Mode", 0.0, FILTER, &["LP", "BP", "HP", "Notch"]),
    fs_choice("tbMod", "303 Mod", 0.0, FILTER, &["Stock", "Devil Fish"]),
    fs_choice("waspMode", "Wasp Mode", 0.0, FILTER, &["LP", "BP", "HP", "Notch"]),
    fs_choice("envSrc", "Env Detect", 0.0, MOD, &["Full", "Low Cut"]),
];

pub static FILTER_INFO: PluginInfo = PluginInfo {
    id: "fs_filter",
    name: "Filter",
    short_name: "Filter",
    category: Category::FireStation,
    params: &FILTER_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &FILTER_INFO,
    make: || Box::new(Filter::new()),
    presets: PRESETS,
    order: 110,
    fidelity: Some(super::Fidelity { name: "filter", source_dir: "Engine 5 - Filter", render_tool: "FilterEngine", tables: &[] }),
    prepare_state: None,
    actions: &[],
};

/// The Filter port.
///
/// `readouts`: `[0]` input peak dBFS and `[1]` output peak dBFS over the last block (Rust-side
/// meters; the C++ has none), then the engine's own UI outputs as the editor polls them: `[2]`
/// the live cutoff of the left channel in Hz (`cutoffOut`), `[3]` the current mod value -1..1
/// (`modOut`), `[4]` the tempo in use, negative when the host gave none and the BPM knob runs it
/// (`bpmOut`), `[5]` the current step (0-based), -1 when Steps is not the source (`stepOut`).
pub struct Filter {
    p: Params,
    dsp: Box<FilterEngineDsp>,
    /// The wrapper's `lastPushed` and `forcePush`.
    last_pushed: [f32; NUM_PARAMS],
    force_push: bool,
    channels: usize,
    in_peak_db: f32,
    out_peak_db: f32,
    /// Audio has gone through since `new`: `prepare` / `reset` take the live path.
    processed: bool,
}

impl Default for Filter {
    fn default() -> Self {
        Self::new()
    }
}

impl Filter {
    pub fn new() -> Self {
        Filter {
            p: Params::new(&FILTER_PARAMS),
            dsp: Box::new(FilterEngineDsp::new()),
            last_pushed: [0.0; NUM_PARAMS],
            force_push: true,
            channels: 2,
            in_peak_db: f32::NAN,
            out_peak_db: f32::NAN,
            processed: false,
        }
    }

    /// `prepareToPlay` at `sr`, fresh before any audio, else on the live instance (see the
    /// module notes).
    fn prepare_at(&mut self, sr: f64) {
        if self.processed {
            self.push_params();
            self.dsp.clear_audio();
            self.dsp.prepare(sr);
            self.force_push = true;
            self.push_params();
            self.dsp.seat_live();
        } else {
            self.dsp.restore_initial();
            self.dsp.prepare(sr);
            self.force_push = true;
        }
        self.in_peak_db = f32::NAN;
        self.out_peak_db = f32::NAN;
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
    pub(crate) fn engine(&self) -> &FilterEngineDsp {
        &self.dsp
    }
}

fn peak(io: &[Vec<f32>], nc: usize, frames: usize) -> f32 {
    io.iter().take(nc).flat_map(|c| c.iter().take(frames)).fold(0.0f32, |m, &x| if x.abs() > m { x.abs() } else { m })
}

impl Plugin for Filter {
    fn info(&self) -> &'static PluginInfo {
        &FILTER_INFO
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
        self.prepare_at(sr);
    }

    fn reset(&mut self) {
        let sr = self.dsp.sample_rate();
        self.prepare_at(sr);
    }

    fn set_transport(&mut self, transport: &Transport) {
        // `pos->getBpm()` (0 when absent) and `pos->getPpqPosition()` (-1 when absent).
        let bpm = if transport.tempo.is_finite() { transport.tempo as f32 } else { 0.0 };
        let ppq = if transport.quarters.is_finite() { transport.quarters } else { -1.0 };
        self.dsp.set_host_transport(transport.playing, bpm, ppq);
    }

    fn tail_samples(&self) -> usize {
        // getTailLengthSeconds: 1 s.
        self.dsp.sample_rate().max(0.0) as usize
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.processed = true;
        self.push_params();
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
        out[2] = self.dsp.cutoff_out();
        out[3] = self.dsp.mod_out();
        out[4] = self.dsp.bpm_out();
        out[5] = self.dsp.step_out();
    }
}

#[cfg(test)]
mod tests;
