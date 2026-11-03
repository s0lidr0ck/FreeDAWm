//! `fs_amp`: the owner's AmpEngine (FireStation Engine 12), a faithful translation of
//! `Engine 12 - Amp/Source/{AmpTube,AmpNetwork,AmpPower,AmpExtras,AmpModels,AmpEngineDSP}.h`:
//! seven tube amps (Twin Reverb '77, JTM45 '66, Deluxe Reverb '65, Super Reverb '65, Princeton
//! Reverb '65, AC30 Top Boost '64, OR120 Graphic '72), each built from its schematic and solved
//! tube by tube at 2x or 4x, with a built-in cabinet, an impulse-response cabinet or none, Auto
//! Level and the Output trim.
//!
//! Wrapper (`PluginProcessor.cpp`, `Parameters.h`, `Presets.h`, the editor) audit, everything that
//! affects the sound:
//! - `prepareToPlay` prepares the engine at the wanted oversampling (Oversampling 2x/4x) and every
//!   amp's convolution (re-made from that amp's latest IR), then flags a full push: the first block
//!   pushes every parameter in order, later blocks only those that moved. Most pushes re-level Auto
//!   Level (`relevel`). Here `set_param` stores and `process` pushes the same way
//!   ([`Amp::push_params`]). A `prepare` before any audio has run is the fresh instance (what the
//!   render tool does once); every later `prepare` and `reset` (Play, Stop, Locate, a render's
//!   start) is `prepareToPlay` on the live instance: the amps back at their idle, the cabinet and
//!   convolution history cleared, the knobs and each amp's learned Auto Level kept (the C++
//!   `prepare` does not touch Auto Level; the forced push only re-primes it), the latest IR in
//!   place with no crossfade. No allocation: everything is made in `prepare`. Ruling: Auto Level
//!   is kept on purpose (clearing it would glide the level up from unity on every Play), so with
//!   it on a reset is bit-identical only for the same learned level; the determinism test runs
//!   with Auto Level off, and a separate test pins that a reset leaves Auto Level unchanged.
//! - Power-up: a fresh amp fed silence settles from its solved idle, a faint thump (up to 1.3e-3
//!   in the first 100 ms, by amp) that decays; the C++ does the same, measured and pinned in
//!   `power_up_from_silence_matches_the_cpp`.
//! - Oversampling: a change re-prepares the engine asynchronously (`handleAsyncUpdate`) and flags
//!   a full push. Here both factors are prepared up front and the push switches to the other one,
//!   back at its idle ([`AmpEngineDsp::reset_fresh`]), with no allocation on the audio thread.
//! - Latency: fixed, the oversampling filters' 32 samples at either factor (`setLatencySamples`).
//!   Tail: `getTailLengthSeconds` is 3 s (the spring tank); this reports the larger of that and
//!   the IR in use.
//! - The amp averages the input channels and writes its mono result to every output channel; a
//!   mono track hands it one.
//! - Cabinet IR (`convolutions`, one per amp): JUCE `dsp::Convolution`, zero latency, the file's
//!   first channel, trimmed at -80 dB, resampled to the session rate, normalised; its output x2.
//!   A new IR crossfades in over 50 ms. "IR" with none loaded plays the built-in cabinet. Here the
//!   IR comes from the host as an [`ir::ImpulseAsset`] through [`Plugin::set_asset`], keyed by
//!   the C++ state keys (`irPathTwin`, `irPathJtm`, `irPathDlx`, `irPathSup`, `irPathPrn`,
//!   `irPathAC30`, `irPathOrange`); see [`convolution`] for the port of the convolution itself.
//!   The C++ feeds a still-loading convolution silence until its first IR arrives (a background
//!   thread); an asset arrives ready, so that path never runs.
//! - The IR files remembered per computer (`AmpEngine.settings`) are the host's business: the
//!   module keeps its IR choice in the session (the engine's `mix.module_ir`).
//! - Engine bypass: the dry mono input delayed by the latency, no Auto Level; kept, hidden (the
//!   module's bypass is used).
//! - `ScopedNoDenormals`: the engine's state is double precision and settles to its idle, not to
//!   denormals; `silence_tail_is_cheap` guards the CPU side.
//! - Presets leave the Input trims, Auto Level, Oversampling and Bypass alone (`getPresetParams`):
//!   the factory files hold none of them.
//! - The preset browser's dirty flag, the meters' atomics and the editor are UI-only.
//!
//! Rust additions:
//! - `readouts`: `[0]` input peak dBFS (after the Input trim), `[1]` output peak dBFS, `[2]` watts
//!   into the speaker load, `[3]` sag (V below idle B+), `[4]` Auto Level's gain dB, `[5]` the
//!   cabinet in use (0 built-in, 1 IR, 2 off), `[6]` the amp, `[7]` 1 when the amp has an IR.
//! - [`tone_curve`]: the selected amp's tone stack, solved by the engine's own network maths in
//!   the frequency domain, for the tile's graph (UI thread / worker; allocates).
//! - Never-crash: a non-finite output sample is written as 0.

// The C++ literals (3.14159265358979, 0.708, ...) are kept as written.
#![allow(clippy::approx_constant, clippy::excessive_precision)]

pub mod amp_engine_dsp;
pub mod amp_extras;
pub mod amp_models;
pub mod amp_network;
pub mod amp_power;
pub mod amp_tube;
pub mod convolution;
pub mod ir;
mod presets;

#[cfg(test)]
mod tests;

pub use amp_engine_dsp::{AmpEngineDsp, NUM_AMPS, NUM_PARAMS};
pub use ir::ImpulseAsset;
pub use presets::PRESETS;

use crate::analysis::Analysis;
use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, ParamInfo, Plugin, PluginInfo, SampleSource};
use amp_engine_dsp as d;
use convolution::{Convolver, as_impulse};
use std::sync::Arc;

/// The amps' choice labels (`Parameters.h`).
pub const AMPS: [&str; NUM_AMPS] =
    ["Twin Reverb '77", "JTM45 '66", "Deluxe Reverb '65", "Super Reverb '65", "Princeton Reverb '65", "AC30 Top Boost '64", "OR120 Graphic '72"];
/// Short amp names: the drawer's tab per amp, the tile's model name.
pub const AMP_NAMES: [&str; NUM_AMPS] = ["Twin", "JTM45", "Deluxe", "Super", "Princeton", "AC30", "Orange"];
const CABS: [&str; 3] = ["Built-in", "IR", "Off"];
const CHANNELS: [&str; 2] = ["Vibrato", "Normal"];

/// The state keys of each amp's IR (`irKey`), in amp order: the keys [`Plugin::set_asset`] takes.
pub const IR_KEYS: [&str; NUM_AMPS] = ["irPathTwin", "irPathJtm", "irPathDlx", "irPathSup", "irPathPrn", "irPathAC30", "irPathOrange"];

/// [`Analysis::live`] indices: the C++ editor's meters. `inPeak`, `outPeak` (linear) and `watts`
/// are running maxima it takes with `exchange (0)` (raised after each engine block here); `bPlus`
/// and `bPlusIdle` (volts) are plain values it loads.
pub const LIVE_IN_PEAK: usize = 0;
pub const LIVE_OUT_PEAK: usize = 1;
pub const LIVE_WATTS: usize = 2;
pub const LIVE_B_PLUS: usize = 3;
pub const LIVE_B_PLUS_IDLE: usize = 4;
pub const LIVE_LEN: usize = 5;

/// Station's key knobs per amp (`ModuleRegistry.cpp`, `kAmpModels`).
pub const KEY_KNOBS: [[&str; 4]; NUM_AMPS] = [
    ["volume", "treble", "middle", "bass"],
    ["jtmVolume", "jtmTreble", "jtmMiddle", "jtmBass"],
    ["dlxVolume", "dlxTreble", "dlxBass", "dlxReverb"],
    ["supVolume", "supTreble", "supMiddle", "supBass"],
    ["prnVolume", "prnTreble", "prnBass", "prnReverb"],
    ["acBrilliant", "acTreble", "acBass", "acCut"],
    ["orGain", "orBass", "orTreble", "orHfDrive"],
];

/// Station's accent per amp (`kAmpModels`, ARGB).
pub const ACCENTS: [u32; NUM_AMPS] = [0xFF8F_C9D6, 0xFFD9_B45A, 0xFFE4_DDCF, 0xFFA9_C2DE, 0xFFD9_C9A8, 0xFFD7_925A, 0xFFF2_994A];

/// `formatParam` for the trims: `+x.x dB`.
fn trim_text(v: f32) -> String {
    let db = d::knob_to_trim_db(v);
    format!("{}{db:.1} dB", if db > 0.05 { "+" } else { "" })
}

/// `formatParam` for the other knobs: the panel's 0..10.
fn dial_text(v: f32) -> String {
    format!("{:.1}", d::knob_to_amp_dial(v))
}

const GLOBAL: &str = "Global";
const TWIN: &str = "Twin";
const JTM: &str = "JTM45";
const DLX: &str = "Deluxe";
const SUP: &str = "Super";
const PRN: &str = "Princeton";
const AC: &str = "AC30";
const OR: &str = "Orange";

/// Parameters in the C++ order (`Parameters.h`), ids, names and defaults verbatim; grouped by the
/// amp they belong to.
pub static AMP_PARAMS: [ParamInfo; NUM_PARAMS] = [
    fs_choice("amp", "Amp", 0.0, GLOBAL, &AMPS),
    knob01("input", "Input (Twin)", 0.5, TWIN, trim_text),
    knob01("volume", "Volume (Twin)", 0.35, TWIN, dial_text),
    fs_toggle("bright", "Bright (Twin)", 0.0, TWIN),
    knob01("treble", "Treble (Twin)", 0.6, TWIN, dial_text),
    knob01("middle", "Middle (Twin)", 0.5, TWIN, dial_text),
    knob01("bass", "Bass (Twin)", 0.4, TWIN, dial_text),
    knob01("reverb", "Reverb (Twin)", 0.25, TWIN, dial_text),
    knob01("speed", "Speed (Twin)", 0.4, TWIN, dial_text),
    knob01("intensity", "Intensity (Twin)", 0.0, TWIN, dial_text),
    knob01("master", "Master (Twin)", 0.8, TWIN, dial_text),
    fs_toggle("pullBoost", "Pull Boost (Twin)", 0.0, TWIN),
    knob01("volume2", "Volume II (JTM45)", 0.0, JTM, dial_text),
    knob01("presence", "Presence (JTM45)", 0.4, JTM, dial_text),
    fs_choice("cab", "Cabinet (Twin)", 0.0, TWIN, &CABS),
    knob01("output", "Output (Twin)", 0.5, TWIN, trim_text),
    fs_choice("quality", "Oversampling", 0.0, GLOBAL, &["2x", "4x"]),
    // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
    fs_toggle("bypass", "Bypass", 0.0, ""),
    fs_choice("channel", "Channel (Twin)", 0.0, TWIN, &CHANNELS),
    knob01("jtmVolume", "Volume I (JTM45)", 0.35, JTM, dial_text),
    knob01("jtmTreble", "Treble (JTM45)", 0.6, JTM, dial_text),
    knob01("jtmMiddle", "Middle (JTM45)", 0.5, JTM, dial_text),
    knob01("jtmBass", "Bass (JTM45)", 0.4, JTM, dial_text),
    knob01("jtmSpeed", "Speed (JTM45)", 0.4, JTM, dial_text),
    knob01("jtmIntensity", "Intensity (JTM45)", 0.0, JTM, dial_text),
    knob01("jtmInput", "Input (JTM45)", 0.5, JTM, trim_text),
    knob01("jtmOutput", "Output (JTM45)", 0.5, JTM, trim_text),
    fs_choice("jtmCab", "Cabinet (JTM45)", 0.0, JTM, &CABS),
    fs_toggle("autoLevel", "Auto Level (Twin)", 1.0, TWIN),
    fs_toggle("jtmAutoLevel", "Auto Level (JTM45)", 1.0, JTM),
    knob01("dlxVolume", "Volume (Deluxe)", 0.35, DLX, dial_text),
    knob01("dlxTreble", "Treble (Deluxe)", 0.55, DLX, dial_text),
    knob01("dlxBass", "Bass (Deluxe)", 0.5, DLX, dial_text),
    knob01("dlxReverb", "Reverb (Deluxe)", 0.25, DLX, dial_text),
    knob01("dlxSpeed", "Speed (Deluxe)", 0.4, DLX, dial_text),
    knob01("dlxIntensity", "Intensity (Deluxe)", 0.0, DLX, dial_text),
    knob01("dlxInput", "Input (Deluxe)", 0.5, DLX, trim_text),
    knob01("dlxOutput", "Output (Deluxe)", 0.5, DLX, trim_text),
    fs_choice("dlxCab", "Cabinet (Deluxe)", 0.0, DLX, &CABS),
    fs_toggle("dlxAutoLevel", "Auto Level (Deluxe)", 1.0, DLX),
    fs_choice("dlxChannel", "Channel (Deluxe)", 0.0, DLX, &CHANNELS),
    knob01("supVolume", "Volume (Super)", 0.35, SUP, dial_text),
    knob01("supTreble", "Treble (Super)", 0.55, SUP, dial_text),
    knob01("supMiddle", "Middle (Super)", 0.5, SUP, dial_text),
    knob01("supBass", "Bass (Super)", 0.45, SUP, dial_text),
    knob01("supReverb", "Reverb (Super)", 0.25, SUP, dial_text),
    knob01("supSpeed", "Speed (Super)", 0.4, SUP, dial_text),
    knob01("supIntensity", "Intensity (Super)", 0.0, SUP, dial_text),
    knob01("supInput", "Input (Super)", 0.5, SUP, trim_text),
    knob01("supOutput", "Output (Super)", 0.5, SUP, trim_text),
    fs_choice("supCab", "Cabinet (Super)", 0.0, SUP, &CABS),
    fs_toggle("supAutoLevel", "Auto Level (Super)", 1.0, SUP),
    fs_choice("supChannel", "Channel (Super)", 0.0, SUP, &CHANNELS),
    fs_toggle("supBright", "Bright (Super)", 0.0, SUP),
    knob01("prnVolume", "Volume (Princeton)", 0.35, PRN, dial_text),
    knob01("prnTreble", "Treble (Princeton)", 0.55, PRN, dial_text),
    knob01("prnBass", "Bass (Princeton)", 0.5, PRN, dial_text),
    knob01("prnReverb", "Reverb (Princeton)", 0.25, PRN, dial_text),
    knob01("prnSpeed", "Speed (Princeton)", 0.4, PRN, dial_text),
    knob01("prnIntensity", "Intensity (Princeton)", 0.0, PRN, dial_text),
    knob01("prnInput", "Input (Princeton)", 0.5, PRN, trim_text),
    knob01("prnOutput", "Output (Princeton)", 0.5, PRN, trim_text),
    fs_choice("prnCab", "Cabinet (Princeton)", 0.0, PRN, &CABS),
    fs_toggle("prnAutoLevel", "Auto Level (Princeton)", 1.0, PRN),
    fs_choice("acChannel", "Input (AC30)", 0.0, AC, &["Top Boost", "Normal", "Both"]),
    knob01("acBrilliant", "Brilliant (AC30)", 0.3, AC, dial_text),
    knob01("acNormal", "Normal (AC30)", 0.3, AC, dial_text),
    knob01("acTreble", "Treble (AC30)", 0.5, AC, dial_text),
    knob01("acBass", "Bass (AC30)", 0.5, AC, dial_text),
    knob01("acCut", "Cut (AC30)", 0.3, AC, dial_text),
    knob01("acInput", "Input trim (AC30)", 0.5, AC, trim_text),
    knob01("acOutput", "Output (AC30)", 0.5, AC, trim_text),
    fs_choice("acCab", "Cabinet (AC30)", 0.0, AC, &CABS),
    fs_toggle("acAutoLevel", "Auto Level (AC30)", 1.0, AC),
    knob01("orGain", "Gain (Orange)", 0.3, OR, dial_text),
    knob01("orBass", "Bass (Orange)", 0.5, OR, dial_text),
    knob01("orTreble", "Treble (Orange)", 0.5, OR, dial_text),
    fs_choice("orFac", "FAC (Orange)", 0.0, OR, &["1", "2", "3", "4", "5", "6"]),
    knob01("orHfDrive", "HF Drive (Orange)", 0.3, OR, dial_text),
    knob01("orInput", "Input (Orange)", 0.5, OR, trim_text),
    knob01("orOutput", "Output (Orange)", 0.5, OR, trim_text),
    fs_choice("orCab", "Cabinet (Orange)", 0.0, OR, &CABS),
    fs_toggle("orAutoLevel", "Auto Level (Orange)", 1.0, OR),
];

pub static AMP_INFO: PluginInfo = PluginInfo {
    id: "fs_amp",
    name: "Amp",
    short_name: "Amp",
    category: Category::FireStation,
    params: &AMP_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &AMP_INFO,
    make: || Box::new(Amp::new()),
    presets: PRESETS,
    order: 160,
    fidelity: Some(super::Fidelity { name: "amp", source_dir: "Engine 12 - Amp", render_tool: "AmpEngine", tables: &[] }),
    prepare_state: None,
    actions: &[],
};

/// The amp a parameter belongs to (its drawer tab), `None` for the global ones (Amp,
/// Oversampling, the engine bypass).
pub fn param_amp(id: &str) -> Option<usize> {
    let p = AMP_PARAMS.iter().find(|p| p.id == id)?;
    AMP_NAMES.iter().position(|n| *n == p.group)
}

/// A parameter's name without its amp: "Volume (Twin)" is "Volume", "Input trim (AC30)" is
/// "Input trim".
pub fn short_name(name: &str) -> &str {
    match name.rsplit_once(" (") {
        Some((head, tail)) if tail.ends_with(')') => head,
        _ => name,
    }
}

/// The selected amp's tone stack (the circuit between its first stage and the next, with the
/// knobs where `values` has them), in dB at each of `freqs`, solved by the engine's own network
/// maths in the frequency domain. The Fenders' is their Vibrato channel's stack, Volume and Bright
/// cap; the JTM45's the stack behind its cathode follower; the AC30's the Top Boost stack (on
/// Normal, the Normal side into the mixer and the Cut; on Both, the two sides summed there); the
/// Orange's the Baxandall. Allocates: UI / worker thread.
pub fn tone_curve(values: &[f32], sample_rate: f32, freqs: &[f32], out: &mut [f32]) {
    use amp_models::{Ac30, FrontHalf, K_DELUXE_CONFIG, K_PRINCETON_CONFIG, K_SUPER_CONFIG, K_TWIN_CONFIG, Marshall, Orange};
    let amp = values.get(d::P_AMP).map_or(0, |v| if v.is_finite() { v.round().clamp(0.0, 6.0) as usize } else { 0 });
    let fs = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 } * 2.0;
    let c = d::controls_for(amp, values);
    let mut response = |f: &dyn Fn(f64) -> f64| {
        for (o, &hz) in out.iter_mut().zip(freqs) {
            let db = 20.0 * f(f64::from(hz)).max(1e-9).log10();
            *o = if db.is_finite() { db as f32 } else { -180.0 };
        }
    };
    match amp {
        d::AMP_MARSHALL => {
            let mut m = Marshall::default();
            m.prepare_tone(fs, &c);
            let (net, src, node) = m.tone_network();
            response(&|hz| net.ac_response(src, node, hz).norm());
        }
        d::AMP_AC30 => {
            let mut a = Ac30::default();
            a.prepare_tone(fs, &c);
            match c.ac_input {
                // Normal skips the Top Boost stack: its own coupling cap and Volume into the mixer,
                // then the Cut (its only tone control). Both: the two sides summed at the mixer.
                1 | 2 => {
                    let (net, cf, normal, node) = a.mix_network();
                    let both = c.ac_input == 2;
                    response(&|hz| {
                        let mut h = net.ac_response(normal, node, hz);
                        if both {
                            h += net.ac_response(cf, node, hz);
                        }
                        (h * amp_power::PowerAmp::cut_response(&amp_power::K_AC30_POWER, c.cut, hz)).norm()
                    });
                }
                _ => {
                    let (net, src) = a.tone_network();
                    let node = a.node_tone_out;
                    response(&|hz| net.ac_response(src, node, hz).norm());
                }
            }
        }
        d::AMP_ORANGE => {
            let mut o = Orange::default();
            o.prepare_tone(fs, &c);
            let (net, src) = o.tone_network();
            let node = o.node_tone_out;
            response(&|hz| net.ac_response(src, node, hz).norm());
        }
        _ => {
            let cfg = match amp {
                d::AMP_DELUXE => K_DELUXE_CONFIG,
                d::AMP_SUPER => K_SUPER_CONFIG,
                d::AMP_PRINCETON => K_PRINCETON_CONFIG,
                _ => K_TWIN_CONFIG,
            };
            let mut f = FrontHalf::default();
            f.build(fs, &cfg);
            f.set_knobs(&c);
            let (src, node) = (f.source(), f.vol_top);
            response(&|hz| f.net.ac_response(src, node, hz).norm());
        }
    }
}

/// The AmpEngine port. See the module docs for the wrapper audit and the readouts.
pub struct Amp {
    p: Params,
    dsp: Box<AmpEngineDsp>,
    conv: Option<Box<Convolver>>,
    /// Each amp's IR, as the host handed it over.
    assets: [Option<Arc<dyn SampleSource>>; NUM_AMPS],
    /// The amp whose IR the convolution runs.
    conv_amp: usize,
    /// Assets let go of before a convolution existed (for the host to collect).
    released: [Option<Arc<dyn SampleSource>>; 8],
    /// The wrapper's `lastPushed` and `forcePush`.
    last_pushed: [f32; NUM_PARAMS],
    force_push: bool,
    channels: usize,
    sr: f64,
    max_block: usize,
    /// Whether `prepare` has built the engine (at `sr`, `max_block`).
    built: bool,
    /// Whether audio has run since the instance was made fresh.
    processed: bool,
    /// The editor's meters ([`LIVE_IN_PEAK`] ..).
    analysis: Arc<Analysis>,
}

impl Default for Amp {
    fn default() -> Self {
        Self::new()
    }
}

impl Amp {
    pub fn new() -> Self {
        Amp {
            p: Params::new(&AMP_PARAMS),
            dsp: Box::new(AmpEngineDsp::new()),
            conv: None,
            assets: Default::default(),
            conv_amp: 0,
            released: Default::default(),
            last_pushed: [0.0; NUM_PARAMS],
            force_push: true,
            channels: 2,
            sr: 48_000.0,
            max_block: 1024,
            built: false,
            processed: false,
            analysis: Arc::new(Analysis::with_live(LIVE_LEN)),
        }
    }

    /// `wantedOversampling`: 4x when Oversampling is on its second choice.
    fn wanted_oversampling(&self) -> usize {
        if self.p.choice(d::P_QUALITY) == 1 { 4 } else { 2 }
    }

    /// The top of `processBlock`: push only what moved (everything after a prepare), in parameter
    /// order. Oversampling's async re-prepare happens here, between blocks.
    fn push_params(&mut self) {
        let force = std::mem::take(&mut self.force_push);
        for i in 0..NUM_PARAMS {
            let v = self.p.v(i);
            if force || v != self.last_pushed[i] {
                self.last_pushed[i] = v;
                self.dsp.set_parameter(i, v);
            }
        }
        let want = self.wanted_oversampling();
        if self.dsp.get_oversampling() != want {
            // handleAsyncUpdate: the engine re-prepared at the new factor, then a full push
            self.dsp.reset_fresh(want);
            self.dsp.relevel();
        }
    }

    /// An asset the plugin can run at this rate (one made by [`ImpulseAsset::new`] for it).
    fn usable(&self, a: &Arc<dyn SampleSource>) -> bool {
        as_impulse(a).is_some() && (f64::from(a.sample_rate()) - self.sr).abs() < 0.5
    }

    /// Hands the amp in use its IR: at once on an amp change (or a first IR), through the
    /// crossfade when it replaces the one playing.
    /// One engine block: frames `at..at + n` of `io`, `n` at most `max_block`.
    fn process_chunk(&mut self, io: &mut [Vec<f32>], nc: usize, at: usize, n: usize) {
        self.push_params();
        let amp = self.dsp.current_amp();
        self.sync_convolver(amp);
        let loaded = self.assets.get(amp).and_then(Option::as_ref).is_some_and(|a| self.usable(a));
        let ir_arrived = loaded && self.conv.as_ref().is_some_and(|c| c.current().is_some());
        self.dsp.set_impulse_ready(ir_arrived);

        // the amp runs on the input channels (averaged) and writes its mono result to all of them
        self.dsp.process(io, nc, at, n);

        if !self.dsp.is_bypassed()
            && self.dsp.effective_cab() == d::CAB_IMPULSE
            && let Some(conv) = self.conv.as_mut()
        {
            let ch0 = &mut io[0][at..at + n];
            conv.process(ch0);
            // JUCE normalises every IR to an energy of 0.125; x2 has a dimed JTM45 peaking just
            // under 0 dBFS
            for s in ch0.iter_mut() {
                *s *= 2.0;
            }
        }

        if let Some((first, rest)) = io.split_first_mut() {
            for c in rest.iter_mut().take(nc - 1) {
                c[at..at + n].copy_from_slice(&first[at..at + n]);
            }
        }

        // Auto Level and the Output trim, after whichever cabinet
        self.dsp.finish(io, nc, at, n);
        for c in io.iter_mut().take(nc) {
            for s in &mut c[at..at + n] {
                if !s.is_finite() {
                    *s = 0.0;
                }
            }
        }
    }

    fn sync_convolver(&mut self, amp: usize) {
        let want = self.assets.get(amp).and_then(Option::as_ref).filter(|a| self.usable(a)).cloned();
        let Some(conv) = self.conv.as_mut() else { return };
        if self.conv_amp != amp {
            self.conv_amp = amp;
            conv.install_now(want);
        } else if let Some(w) = want {
            conv.load(w);
        }
    }

    /// The IR length of the amp in use, in samples.
    fn ir_len(&self) -> usize {
        let amp = self.dsp.current_amp();
        self.assets.get(amp).and_then(Option::as_ref).and_then(as_impulse).map_or(0, ImpulseAsset::len)
    }

    fn keep_released(&mut self, a: Arc<dyn SampleSource>) {
        if let Some(slot) = self.released.iter_mut().find(|s| s.is_none()) {
            *slot = Some(a);
        }
        // (all full means the host never collects them: the reference drops here)
    }

    /// The engine (tests).
    #[cfg(test)]
    pub(crate) fn engine(&self) -> &AmpEngineDsp {
        &self.dsp
    }

    /// The convolution (tests).
    #[cfg(test)]
    pub(crate) fn convolver(&self) -> Option<&Convolver> {
        self.conv.as_deref()
    }
}

impl Plugin for Amp {
    fn info(&self) -> &'static PluginInfo {
        &AMP_INFO
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.p.set(id, value).is_some()
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.channels = channels.clamp(1, 2);
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate.min(768_000.0)) } else { 48_000.0 };
        let mb = max_block.clamp(1, 1 << 16);
        let want = self.wanted_oversampling();
        if !self.built || (sr - self.sr).abs() > 0.0 || mb != self.max_block {
            self.sr = sr;
            self.max_block = mb;
            self.dsp.prepare(sr, mb, want);
            let mut conv = Box::new(Convolver::new(sr, mb));
            // the old convolution's references go to the host with the rest
            if let Some(mut old) = self.conv.take() {
                while let Some(a) = old.take_released() {
                    self.keep_released(a);
                }
                if let Some(c) = old.current().cloned() {
                    self.keep_released(c);
                }
            }
            conv.install_now(None);
            self.conv = Some(conv);
            self.conv_amp = usize::MAX;
            self.built = true;
        } else {
            self.dsp.reset_fresh(want);
            if let Some(conv) = self.conv.as_mut() {
                conv.reset();
            }
        }
        if !self.processed {
            self.last_pushed = [0.0; NUM_PARAMS];
        }
        self.force_push = true;
    }

    fn reset(&mut self) {
        // `prepareToPlay` on the live instance (or, before any audio, the fresh one again)
        self.dsp.reset_fresh(self.wanted_oversampling());
        if let Some(conv) = self.conv.as_mut() {
            conv.reset();
        }
        self.force_push = true;
    }

    fn latency(&self) -> usize {
        self.dsp.latency_samples()
    }

    fn tail_samples(&self) -> usize {
        // `getTailLengthSeconds`: 3 s (the spring tank); a longer IR rings longer
        ((3.0 * self.sr) as usize).max(self.ir_len()) + self.dsp.latency_samples()
    }

    fn set_asset(&mut self, key: &str, asset: Option<Arc<dyn SampleSource>>) -> bool {
        let Some(a) = IR_KEYS.iter().position(|k| *k == key) else { return false };
        if let Some(old) = std::mem::replace(&mut self.assets[a], asset) {
            self.keep_released(old);
        }
        true
    }

    fn take_released_asset(&mut self) -> Option<Arc<dyn SampleSource>> {
        if let Some(a) = self.released.iter_mut().find_map(Option::take) {
            return Some(a);
        }
        self.conv.as_mut().and_then(|c| c.take_released())
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.processed = true;
        let nc = self.channels.min(io.len());
        let total = io.iter().take(nc).map(Vec::len).min().unwrap_or(0).min(frames);
        if nc == 0 || total == 0 || self.max_block == 0 {
            return;
        }
        // A host block longer than `prepare`'s runs in chunks of at most that (the C++ grows its
        // mono buffer instead; nothing here may allocate). Each chunk is one engine block.
        let mut at = 0;
        while at < total {
            let n = (total - at).min(self.max_block);
            self.process_chunk(io, nc, at, n);
            // `raiseMax` / `store` into what the editor reads
            let m = self.dsp.meters;
            let live = &self.analysis.live;
            live.raise_max(LIVE_IN_PEAK, m.in_peak);
            live.raise_max(LIVE_OUT_PEAK, m.out_peak);
            live.raise_max(LIVE_WATTS, m.watts);
            live.set(LIVE_B_PLUS, m.b_plus);
            live.set(LIVE_B_PLUS_IDLE, m.b_plus_idle);
            at += n;
        }
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(self.analysis.clone())
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        let m = self.dsp.meters;
        let db = |x: f32| if x > 0.0 { 20.0 * x.log10() } else { -120.0 };
        out[0] = db(m.in_peak);
        out[1] = db(m.out_peak);
        out[2] = m.watts;
        out[3] = m.b_plus_idle - m.b_plus;
        out[4] = m.auto_gain_db;
        out[5] = self.dsp.effective_cab() as f32;
        out[6] = self.dsp.current_amp() as f32;
        let amp = self.dsp.current_amp();
        out[7] = if self.assets.get(amp).and_then(Option::as_ref).is_some_and(|a| self.usable(a)) { 1.0 } else { 0.0 };
    }
}
