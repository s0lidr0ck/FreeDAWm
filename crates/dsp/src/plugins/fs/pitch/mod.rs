//! `fs_pitch`: the owner's Pitch engine (POG / Micro POG / OC-2 / H910 / Whammy), a faithful
//! translation of `Engine 3 - Pitch/Source/PitchEngineDSP.h`, with the editor's tables from
//! `PitchConfig.h` in [`pitch_config`].
//!
//! Wrapper (`PluginProcessor.{h,cpp}`) audit, everything that affects the sound:
//! - `prepareToPlay` calls `dsp.prepare (sampleRate)` and raises `forcePush`, so the first
//!   `processBlock` pushes every parameter (in parameter order) before processing.
//! - `processBlock` pushes only what moved, in parameter order, at the start of the block
//!   (`v != lastPushed[i]`). That matters here: Style's setter re-seats every read head, so
//!   pushing it every block would restart every voice every block; and the order matters
//!   (Style re-derives the windows from the Grain knob as it stands). Here `set_param` only stores
//!   the value; `process` does the wrapper's push (all values after `prepare`/`reset`, else the
//!   changed ones) before handing the block to the engine.
//! - Latency: none reported. The dry path is untouched; `getVoiceLatencyMs` (how late the shifted
//!   voices are) is shown by the editor, "not reported to the host as latency". Here it is
//!   readout `[3]`, and `latency()` stays 0.
//! - Bypass: the engine's own `bypass` parameter is a 0.002-per-sample crossfade to the dry
//!   input (translated in `process`). It is kept in order but hidden (group ""): the UI uses the
//!   module's bypass.
//! - APVTS ranges: floats are 0..1, choices are whole indices, the bool 0/1. `ParamInfo::clamp`
//!   does the same (NaN becomes the default), so the engine never sees a non-finite value.
//! - Bus layout: mono or stereo, in = out. `dsp.process` gets the channel count (at most 2);
//!   with one channel the engine runs its second channel on a copy of the first and writes only
//!   the first, as the C++. Extra channels are left untouched (the Rust host contract). Unused
//!   output channels are cleared by the C++ wrapper (there are none: in and out match).
//! - `ScopedNoDenormals` (flush-to-zero): no safe Rust equivalent. It only changes values below
//!   ~1e-38, far under the fidelity bar; `silence_tail_is_cheap` guards the CPU side.
//! - Tail: `getTailLengthSeconds` is 0, but the shifted voices read up to a whole buffer
//!   (`BUF` samples) behind the input, so `tail_samples` reports that (max of the two). The OC-2
//!   lock lamp (`trackLamp`) is readout `[2]`. The preset dirty-tracking (`parameterChanged`),
//!   state save and editor are UI-only.
//! - Reset: the host calls `reset` on every Play / Stop / Locate and at bounce start. Before any
//!   audio has been processed it is the fresh-instance path (`new`'s engine, `prepareToPlay`,
//!   a full push), which the fidelity cases measure. After that it is `prepareToPlay` on the
//!   LIVE instance: what moved is pushed first (the C++ `prepare` builds its smoothers from the
//!   knobs), then [`PitchEngineDsp::reset_live`] clears the audio and seats every glide at its
//!   target, and the C++'s forced push follows at the next block (idempotent on a just-prepared
//!   engine). No allocation; the result does not depend on what played before.
//!
//! Station (`ModuleRegistry.cpp`): `fixed ("pitch", "Pitch", "style", 0xFF7BD4FF, ..,
//! list ("dry,octDown,octUp,level"))`: model param Style, key knobs Dry, -1 Oct, +1 Oct, Level.

// 0.7071 is the C++ default (unity on the audio taper), not 1/sqrt(2).
#![allow(clippy::approx_constant)]

pub mod pitch_config;
pub mod pitch_engine_dsp;
mod presets;

pub use pitch_engine_dsp::{NUM_PARAMS, PitchEngineDsp};
pub use presets::PRESETS;

use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, ParamInfo, Plugin, PluginInfo, gain_to_db};
use pitch_config::format_value;
use pitch_engine_dsp as d;

/// The Whammy's intervals (`kIntervalChoices`).
pub const INTERVALS: [&str; 10] = ["+2 Oct", "+1 Oct", "+5th", "+4th", "+2nd", "-2nd", "-4th", "-5th", "-1 Oct", "-2 Oct"];

macro_rules! display_fns {
    ($($name:ident => $idx:expr),* $(,)?) => {$(
        fn $name(v: f32) -> String {
            format_value($idx, v)
        }
    )*};
}
display_fns!(
    dry_text => d::P_DRY,
    sub_text => d::P_SUB,
    oct_down_text => d::P_OCT_DOWN,
    oct_up_text => d::P_OCT_UP,
    oct2_up_text => d::P_OCT2_UP,
    fifth_text => d::P_FIFTH,
    detune_text => d::P_DETUNE,
    attack_text => d::P_ATTACK,
    lpf_text => d::P_LPF,
    reso_text => d::P_RESO,
    level_text => d::P_LEVEL,
    trim_text => d::P_TRIM,
    voice_text => d::P_VOICE,
    pedal_text => d::P_PEDAL,
    glide_text => d::P_GLIDE,
    width_text => d::P_WIDTH,
    grain_text => d::P_GRAIN,
    track_text => d::P_TRACK,
);

/// The drawer's two groups, as the editor's two rows: the voices (and what picks and tunes
/// them) and the shaping row.
const VOICES: &str = "Voices";
const SHAPE: &str = "Shape";

/// Parameters in the C++ order (`Parameters.h`), ids, names and defaults verbatim.
pub static PITCH_PARAMS: [ParamInfo; NUM_PARAMS] = [
    fs_choice("style", "Style", 0.0, VOICES, &["POG", "Micro POG", "OC-2", "H910", "Whammy"]),
    knob01("dry", "Dry", 0.7071, VOICES, dry_text),
    knob01("sub", "Sub", 0.0, VOICES, sub_text),
    knob01("octDown", "-1 Oct", 0.6, VOICES, oct_down_text),
    knob01("octUp", "+1 Oct", 0.6, VOICES, oct_up_text),
    knob01("oct2Up", "+2 Oct", 0.0, VOICES, oct2_up_text),
    knob01("fifth", "5th", 0.0, VOICES, fifth_text),
    fs_choice("fifthMode", "5th Mode", 1.0, VOICES, &["+5th", "+Oct 5th", "-5th"]),
    knob01("detune", "Detune", 0.25, VOICES, detune_text),
    knob01("attack", "Attack", 0.0, SHAPE, attack_text),
    knob01("lpf", "Filter", 1.0, SHAPE, lpf_text),
    knob01("reso", "Q", 0.0, SHAPE, reso_text),
    knob01("level", "Level", 0.7071, SHAPE, level_text),
    // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
    fs_toggle("bypass", "Bypass", 0.0, ""),
    knob01("trim", "Input", 0.3333, SHAPE, trim_text),
    knob01("voice", "Voice", 0.7, VOICES, voice_text),
    fs_choice("interval", "Interval", 1.0, VOICES, &INTERVALS),
    knob01("pedal", "Pedal", 0.0, VOICES, pedal_text),
    knob01("glide", "Glide", 0.2, VOICES, glide_text),
    knob01("width", "Width", 0.35, SHAPE, width_text),
    knob01("grain", "Grain", 0.5, SHAPE, grain_text),
    knob01("track", "Track", 0.5, SHAPE, track_text),
];

pub static PITCH_INFO: PluginInfo = PluginInfo {
    id: "fs_pitch",
    name: "Pitch",
    short_name: "Pitch",
    category: Category::FireStation,
    params: &PITCH_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &PITCH_INFO,
    make: || Box::new(Pitch::new()),
    presets: PRESETS,
    order: 70,
    fidelity: Some(super::Fidelity { name: "pitch", source_dir: "Engine 3 - Pitch", render_tool: "PitchEngine", tables: &[] }),
    prepare_state: None,
    actions: &[],
};

/// The Pitch port.
///
/// `readouts`: `[0]` input peak dBFS and `[1]` output peak dBFS over the last block (Rust-side
/// meters; the C++ has none), `[2]` the OC-2 lock lamp (0 or 1, `trackLamp`), `[3]` how late the
/// shifted voices are in ms (`getVoiceLatencyMs`, from the parameters), `[4]` the Whammy voice's
/// current interval in semitones (the engine's slewed `whamSemi`).
pub struct Pitch {
    p: Params,
    dsp: Box<PitchEngineDsp>,
    /// The wrapper's `lastPushed` and `forcePush`.
    last_pushed: [f32; NUM_PARAMS],
    force_push: bool,
    channels: usize,
    in_peak_db: f32,
    out_peak_db: f32,
    /// Audio has gone through since `new`: `reset` / `prepare` take the live path.
    processed: bool,
}

impl Default for Pitch {
    fn default() -> Self {
        Self::new()
    }
}

impl Pitch {
    pub fn new() -> Self {
        Pitch {
            p: Params::new(&PITCH_PARAMS),
            dsp: Box::new(PitchEngineDsp::new()),
            last_pushed: [0.0; NUM_PARAMS],
            force_push: true,
            channels: 2,
            in_peak_db: f32::NAN,
            out_peak_db: f32::NAN,
            processed: false,
        }
    }

    /// `prepareToPlay` at `sr`: on a module that has not played yet, the fresh path (the engine
    /// as `new` made it, prepared, a full push at the next block); on a live one, what moved is
    /// pushed, then the engine prepares on its own state with every glide at its target.
    fn prepare_at(&mut self, sr: f64) {
        if self.processed {
            self.push_params();
            self.dsp.reset_live(sr);
        } else {
            self.dsp.restart();
            self.dsp.prepare(sr);
        }
        self.force_push = true;
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
    pub(crate) fn engine(&self) -> &PitchEngineDsp {
        &self.dsp
    }
}

fn peak(io: &[Vec<f32>], nc: usize, frames: usize) -> f32 {
    io.iter().take(nc).flat_map(|c| c.iter().take(frames)).fold(0.0f32, |m, &x| if x.abs() > m { x.abs() } else { m })
}

impl Plugin for Pitch {
    fn info(&self) -> &'static PluginInfo {
        &PITCH_INFO
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.p.set(id, value).is_some()
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        // `prepareToPlay`: prepare the engine (nothing to size: its buffers are fixed and made
        // in `new`), then force the push of every value before the next block.
        self.channels = channels.min(2);
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        self.prepare_at(sr);
        self.in_peak_db = f32::NAN;
        self.out_peak_db = f32::NAN;
    }

    fn reset(&mut self) {
        let sr = self.dsp.sample_rate();
        self.prepare_at(sr);
    }

    fn tail_samples(&self) -> usize {
        // max(`getTailLengthSeconds` = 0, the deepest a shifted voice reads behind the input).
        d::BUF as usize
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
        out[2] = self.dsp.track_lamp();
        out[3] = d::latency_ms(self.p.choice(d::P_STYLE) as i32, self.p.v(d::P_GRAIN));
        out[4] = self.dsp.whammy_semis();
    }
}

#[cfg(test)]
mod tests;
