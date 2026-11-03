//! `fs_cleanq`: the owner's CleanQ (24-band dynamic EQ), a faithful translation of
//! `CleanQ/Source/CleanQDSP.h` with `CleanQDynCeiling.h` and `CleanQCeiling.h`.
//! (`CleanQAmorphEq.h` is JSON I/O only; its one table, the slopes, is `slope_db_per_oct`.)
//!
//! Parameters: the host layout of `Parameters.h`, ids, order, ranges and defaults verbatim -- 7
//! globals, 24 bands x 10 unpacked fields, then the dynamic ceiling's 7 (254 in all). The engine
//! speaks the packed 127-value Cmajor layout; `pack_for_dsp` (the `packForDsp` of `Parameters.h`)
//! folds the host values into it, exactly as the processor does.
//!
//! Wrapper (`PluginProcessor.cpp`) audit, everything that affects the sound:
//! - `prepareToPlay`: `dsp.prepare(sr)`, `dynCeil.prepare(sr)`, `dynWasOn` from `ceilOn`, latency
//!   set from it, the ceiling pushed, and `forcePush` so the first block re-sends every value.
//!   Ported in `prepare`.
//! - `processBlock`: packs the host's values with `packForDsp` and pushes only those that moved
//!   since the last push (all of them when forced), BEFORE `dsp.process`. Ported in `process`
//!   (`push_params`): `set_param` only stores the value, as the APVTS does.
//! - Solo: host index 0 = None, 1..24 = slot; the DSP gets `index / 24`. Ported (`pack_for_dsp`).
//! - The dynamic ceiling runs AFTER the EQ, only while `ceilOn` is on. Switching it on resets it;
//!   its settings come from the `ceil*` parameters each block (`ceilAmount` / 100), and its
//!   `bypass` from the engine's own `bypass` parameter. Ported in `process`.
//! - Latency: `dynCeil.latencySamples()` (the frame length, 2048/4096/8192 by rate) while the
//!   dynamic ceiling is on, else 0. The C++ tells the host asynchronously after the block that
//!   switched it; here `latency()` reflects it right after that block. Ported.
//! - The ceiling shape is not a parameter: it lives in the plugin state, `uiState/ceiling`
//!   ([`clean_q_ceiling_state::CeilingState`], the same element here: the module state,
//!   `Insert::state`), and `pushCeilingToAudio` hands its 256-point grid to the audio thread on
//!   every change. Here the host works the grid out off the audio thread
//!   ([`crate::prepare_state`] / [`prepared_ceiling`]) and hands it over between blocks
//!   ([`Plugin::set_prepared_state`] -> [`CleanQ::set_ceiling`], a copy into fixed storage: the
//!   C++ copies under a try-lock into a reserved vector). `load_state` / `save_state` read and
//!   write the same element. With none drawn the dynamic ceiling is a pure delay (nothing to hold
//!   to), which is what the render tool hears. The shape survives `prepare` and `reset`, as the
//!   C++ ceiling survives `prepareToPlay` (it is pushed again there).
//! - Unused output channels are cleared and only mono/stereo layouts are accepted: here the
//!   engine processes at most 2 channels and leaves the rest untouched (the Rust host contract).
//! - `ScopedNoDenormals` (flush-to-zero): no safe Rust equivalent; it only changes values below
//!   ~1e-38, far under the fidelity bar; `silence_tail_is_cheap` guards the CPU side.
//! - No tail (`getTailLengthSeconds` is 0). Preset dirty-tracking, guides, the capture and the
//!   editor are UI-only.

pub mod clean_q_capture;
pub mod clean_q_ceiling;
pub mod clean_q_ceiling_state;
pub mod clean_q_dsp;
pub mod clean_q_dyn_ceiling;
pub mod clean_q_guide_templates;
pub mod clean_q_guides;
mod presets;

use std::sync::Arc;

pub use presets::PRESETS;

use crate::analysis::Analysis;
use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Taper, Unit};
use clean_q_ceiling_state::CeilingState;
use clean_q_dsp as q;
use clean_q_dsp::CleanQDsp;
use clean_q_dyn_ceiling::DynCeiling;

/// Key knobs on the module tile: Station's (`ModuleRegistry.cpp`), the output trim and the dynamic
/// ceiling's threshold and amount (the curve is the main control).
/// [`Analysis::live`] indices CleanQ publishes after each block (what `CleanQGraph.h`'s
/// `dynOffsetDb` per band and `setDynCut` read from the processor on the editor's timer):
/// each band's live dynamic offset in dB, by slot.
pub const LIVE_DYN: std::ops::Range<usize> = 0..24;
/// The dynamic ceiling's cut right now in dB (0 or below) on the `GRID_N` ceiling grid; 0 while
/// the dynamic ceiling is off.
pub const LIVE_CUT: std::ops::Range<usize> = 24..280;
/// 1.0 while the dynamic ceiling is on and cutting (`refreshDynamic`: some cut below -0.1 dB).
pub const LIVE_CUTTING: usize = 280;
/// How many live values CleanQ publishes.
pub const LIVE_LEN: usize = 281;

pub const KEY_KNOBS: [&str; 3] = ["outGain", "ceilThr", "ceilAmount"];

// ---------------------------------------------------------------- host layout (Parameters.h)
pub const NUM_GLOBAL_PARAMS: usize = 7;
pub const G_OUT_GAIN: usize = 0;
pub const G_BYPASS: usize = 1;
pub const G_INVERT: usize = 2;
pub const G_SOLO: usize = 3;
pub const G_AUTO_GAIN: usize = 4;
pub const G_DYN_ATTACK: usize = 5;
pub const G_DYN_RELEASE: usize = 6;

/// Fields the host sees for each band, in order.
pub const F_ON: usize = 0;
pub const F_TYPE: usize = 1;
pub const F_PLACE: usize = 2;
pub const F_SLOPE: usize = 3;
pub const F_DYN: usize = 4;
pub const F_FREQ: usize = 5;
pub const F_GAIN: usize = 6;
pub const F_Q: usize = 7;
pub const F_THRESHOLD: usize = 8;
pub const F_RANGE: usize = 9;
pub const NUM_BAND_FIELDS: usize = 10;

/// The dynamic ceiling's controls, appended after the bands.
pub const FIRST_CEIL_PARAM: usize = NUM_GLOBAL_PARAMS + q::NUM_BANDS * NUM_BAND_FIELDS; // 247
pub const C_ON: usize = FIRST_CEIL_PARAM;
pub const C_THRESHOLD: usize = FIRST_CEIL_PARAM + 1;
pub const C_AMOUNT: usize = FIRST_CEIL_PARAM + 2;
pub const C_MAX_CUT: usize = FIRST_CEIL_PARAM + 3;
pub const C_ATTACK: usize = FIRST_CEIL_PARAM + 4;
pub const C_RELEASE: usize = FIRST_CEIL_PARAM + 5;
pub const C_DELTA: usize = FIRST_CEIL_PARAM + 6;
pub const NUM_HOST_PARAMS: usize = FIRST_CEIL_PARAM + 7; // 254

/// Host parameter index for one band field.
pub const fn band_param(slot: usize, field: usize) -> usize {
    NUM_GLOBAL_PARAMS + slot * NUM_BAND_FIELDS + field
}

/// Folds the host's 247 unpacked band and global values into the 127 the DSP speaks (the dynamic
/// ceiling's seven are not the DSP's). `get` reads a host parameter by index, `push` hands a
/// packed value to the DSP. Verbatim `packForDsp`.
pub fn pack_for_dsp(get: impl Fn(usize) -> f32, mut push: impl FnMut(usize, f32)) {
    push(q::P_OUT_GAIN, get(G_OUT_GAIN));
    push(q::P_BYPASS, get(G_BYPASS));
    push(q::P_INVERT, get(G_INVERT));
    // Solo: host index 0 = None, 1..24 = the slot. The DSP wants slot/24 with 0 = none.
    push(q::P_SOLO, get(G_SOLO) / 24.0f32);
    push(q::P_AUTO_GAIN, get(G_AUTO_GAIN));
    push(q::P_DYN_ATTACK, get(G_DYN_ATTACK));
    push(q::P_DYN_RELEASE, get(G_DYN_RELEASE));

    for s in 0..q::NUM_BANDS {
        let m = q::ModeFields {
            on: get(band_param(s, F_ON)) as i32,
            type_: get(band_param(s, F_TYPE)) as i32,
            place: get(band_param(s, F_PLACE)) as i32,
            slope: get(band_param(s, F_SLOPE)) as i32,
            dyn_: get(band_param(s, F_DYN)) > 0.5f32,
        };

        let d = q::DynFields { threshold_db: f64::from(get(band_param(s, F_THRESHOLD))), range_db: f64::from(get(band_param(s, F_RANGE))) };

        push(q::band_param(s, q::K_MODE), q::pack_mode(&m));
        push(q::band_param(s, q::K_FREQ), get(band_param(s, F_FREQ)));
        push(q::band_param(s, q::K_GAIN), get(band_param(s, F_GAIN)));
        push(q::band_param(s, q::K_Q), get(band_param(s, F_Q)));
        push(q::band_param(s, q::K_DYN), q::pack_dyn(&d));
    }
}

// ---------------------------------------------------------------- display
fn out_text(v: f32) -> String {
    format!("{:+.1} dB", q::knob_to_out_db(v))
}
fn ms_text(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:.2} s", ms / 1000.0)
    } else if ms >= 10.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{ms:.1} ms")
    }
}
fn attack_text(v: f32) -> String {
    ms_text(q::knob_to_attack_ms(v))
}
fn release_text(v: f32) -> String {
    ms_text(q::knob_to_release_ms(v))
}
fn freq_text(v: f32) -> String {
    let f = q::knob_to_freq(v);
    if f >= 1000.0 { format!("{:.2} kHz", f / 1000.0) } else { format!("{f:.0} Hz") }
}
fn gain_text(v: f32) -> String {
    format!("{:+.1} dB", q::knob_to_gain_db(v))
}
fn q_text(v: f32) -> String {
    format!("{:.2}", q::knob_to_q(v))
}

/// A float parameter carrying its real range (`ParamDef::rangeLo/Hi`): the value is in units.
const fn ranged(id: &'static str, name: &'static str, min: f32, max: f32, default: f32, unit: Unit, group: &'static str) -> ParamInfo {
    ParamInfo { id, name, min, max, default, unit, taper: Taper::Linear, choices: &[], display: None, group }
}

const ON_CHOICES: [&str; 3] = ["Off", "Bypassed", "Active"];
const TYPE_CHOICES: [&str; 7] = ["Bell", "Low Shelf", "High Shelf", "Low Cut", "High Cut", "Notch", "Band Pass"];
const PLACE_CHOICES: [&str; 5] = ["Stereo", "Left", "Right", "Mid", "Side"];
const SLOPE_CHOICES: [&str; 8] = ["6 dB/oct", "12 dB/oct", "18 dB/oct", "24 dB/oct", "36 dB/oct", "48 dB/oct", "72 dB/oct", "96 dB/oct"];
const SOLO_CHOICES: [&str; 25] = [
    "None", "Band 1", "Band 2", "Band 3", "Band 4", "Band 5", "Band 6", "Band 7", "Band 8", "Band 9", "Band 10", "Band 11", "Band 12", "Band 13",
    "Band 14", "Band 15", "Band 16", "Band 17", "Band 18", "Band 19", "Band 20", "Band 21", "Band 22", "Band 23", "Band 24",
];

/// The table, built in declaration order like `getParamDefs()`: the seven globals, band 1..24 x ten
/// fields, then the dynamic ceiling's seven.
macro_rules! cleanq_params {
    ($( ($tag:literal, $num:literal) ),* $(,)?) => {
        [
            knob01("outGain", "Output", 0.5, "Global", out_text),
            // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
            fs_toggle("bypass", "Bypass", 0.0, ""),
            fs_toggle("invert", "Phase Invert", 0.0, "Global"),
            fs_choice("solo", "Solo Band", 0.0, "Global", &SOLO_CHOICES),
            fs_toggle("autoGain", "Auto Gain", 0.0, "Global"),
            knob01("dynAttack", "Dyn Attack", 0.540725, "Global", attack_text),
            knob01("dynRelease", "Dyn Release", 0.433677, "Global", release_text),
            $(
                fs_choice(concat!("b", $tag, "_on"), concat!("B", $num, " On"), 0.0, concat!("Band ", $num), &ON_CHOICES),
                fs_choice(concat!("b", $tag, "_type"), concat!("B", $num, " Type"), 0.0, concat!("Band ", $num), &TYPE_CHOICES),
                fs_choice(concat!("b", $tag, "_place"), concat!("B", $num, " Place"), 0.0, concat!("Band ", $num), &PLACE_CHOICES),
                fs_choice(concat!("b", $tag, "_slope"), concat!("B", $num, " Slope"), 0.0, concat!("Band ", $num), &SLOPE_CHOICES),
                fs_toggle(concat!("b", $tag, "_dyn"), concat!("B", $num, " Dynamic"), 0.0, concat!("Band ", $num)),
                knob01(concat!("b", $tag, "_freq"), concat!("B", $num, " Freq"), 0.5, concat!("Band ", $num), freq_text),
                knob01(concat!("b", $tag, "_gain"), concat!("B", $num, " Gain"), 0.5, concat!("Band ", $num), gain_text),
                knob01(concat!("b", $tag, "_q"), concat!("B", $num, " Q"), 0.5, concat!("Band ", $num), q_text),
                // The dyn default is rounded to -20 dB / 0 dB, as Parameters.h explains.
                ranged(concat!("b", $tag, "_thr"), concat!("B", $num, " Threshold"), -60.0, 0.0, -20.0, Unit::Db, concat!("Band ", $num)),
                ranged(concat!("b", $tag, "_range"), concat!("B", $num, " Range"), -30.0, 30.0, 0.0, Unit::Db, concat!("Band ", $num)),
            )*
            fs_toggle("ceilOn", "Ceiling Dynamic", 0.0, "Ceiling"),
            ranged("ceilThr", "Ceiling Threshold", 0.0, 12.0, 3.0, Unit::Db, "Ceiling"),
            ranged("ceilAmount", "Ceiling Amount", 0.0, 100.0, 70.0, Unit::Percent, "Ceiling"),
            ranged("ceilMaxCut", "Ceiling Max Cut", 0.0, 24.0, 9.0, Unit::Db, "Ceiling"),
            ranged("ceilAttack", "Ceiling Attack", 1.0, 100.0, 5.0, Unit::Ms, "Ceiling"),
            ranged("ceilRelease", "Ceiling Release", 20.0, 1000.0, 120.0, Unit::Ms, "Ceiling"),
            fs_toggle("ceilDelta", "Ceiling Delta", 0.0, "Ceiling"),
        ]
    };
}

/// Parameters in the C++ order (`Parameters.h`), ids, ranges and defaults verbatim.
pub static CLEANQ_PARAMS: [ParamInfo; NUM_HOST_PARAMS] = cleanq_params!(
    ("01", 1),
    ("02", 2),
    ("03", 3),
    ("04", 4),
    ("05", 5),
    ("06", 6),
    ("07", 7),
    ("08", 8),
    ("09", 9),
    ("10", 10),
    ("11", 11),
    ("12", 12),
    ("13", 13),
    ("14", 14),
    ("15", 15),
    ("16", 16),
    ("17", 17),
    ("18", 18),
    ("19", 19),
    ("20", 20),
    ("21", 21),
    ("22", 22),
    ("23", 23),
    ("24", 24),
);

pub static CLEANQ_INFO: PluginInfo = PluginInfo {
    id: "fs_cleanq",
    name: "CleanQ",
    short_name: "CleanQ",
    category: Category::FireStation,
    params: &CLEANQ_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &CLEANQ_INFO,
    make: || Box::new(CleanQ::new()),
    presets: PRESETS,
    order: 50,
    fidelity: Some(super::Fidelity { name: "cleanq", source_dir: "CleanQ", render_tool: "CleanQ", tables: &[] }),
    prepare_state: Some(prepared_ceiling),
    actions: &[],
};

// ---------------------------------------------------------------- the curve, for the UI
/// A value from a `CLEANQ_PARAMS`-ordered slice, clamped like the APVTS (missing: the default).
fn value_at(values: &[f32], idx: usize) -> f32 {
    match CLEANQ_PARAMS.get(idx) {
        Some(info) => info.clamp(values.get(idx).copied().unwrap_or(info.default)),
        None => 0.0,
    }
}

/// One band as the engine decodes it from the host values (`packForDsp`, then `mapParams`):
/// `(used, active, design)`.
fn band_design(values: &[f32], slot: usize, fs: f64) -> (bool, bool, q::Design) {
    let get = |field: usize| value_at(values, band_param(slot, field));
    let m = q::ModeFields {
        on: get(F_ON) as i32,
        type_: get(F_TYPE) as i32,
        place: get(F_PLACE) as i32,
        slope: get(F_SLOPE) as i32,
        dyn_: get(F_DYN) > 0.5f32,
    };
    let mode = q::unpack_mode(q::pack_mode(&m));
    let order = q::slope_to_order(mode.slope);
    // As the editor's curve (`curveMagnitudeDb`): knob maps straight into `designFilter`, unclamped.
    let d = q::design_filter(mode.type_, order, q::knob_to_freq(get(F_FREQ)), q::knob_to_gain_db(get(F_GAIN)), q::knob_to_q(get(F_Q)), fs);
    (mode.on >= q::ON_BYPASSED, mode.on == q::ON_ACTIVE, d)
}

/// The sample rate `band_magnitude_db` designs at: the engine's own default (`CleanQDSP()`
/// prepares at 48 kHz). Use [`band_magnitude_db_at`] for the session rate.
pub const CURVE_DEFAULT_SR: f64 = 48_000.0;

/// |H| in dB of one band at angular frequency `w` (radians per sample at 48 kHz), designed from
/// the knob values (`values` in `CLEANQ_PARAMS` order), translated from `bandMagnitudeDb`. The
/// band's on state is not consulted; [`response_db`] sums only the active ones. Dynamic offsets
/// are not included (the static curve). 0 for a slot out of range.
pub fn band_magnitude_db(values: &[f32], slot: usize, w: f64) -> f64 {
    band_magnitude_db_at(values, slot, w, CURVE_DEFAULT_SR)
}

/// [`band_magnitude_db`] at a given sample rate.
pub fn band_magnitude_db_at(values: &[f32], slot: usize, w: f64, sample_rate: f64) -> f64 {
    if slot >= q::NUM_BANDS {
        return 0.0;
    }
    let fs = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { CURVE_DEFAULT_SR };
    let (_, _, d) = band_design(values, slot, fs);
    q::magnitude_db(&d, w)
}

/// The whole EQ's static response in dB at `freq`: the sum over the active bands (as
/// `curveMeanDb` sums `used && active`), for the UI curve.
pub fn response_db(values: &[f32], sample_rate: f32, freq: f32) -> f32 {
    let fs = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { CURVE_DEFAULT_SR };
    let w = 2.0 * q::PI * f64::from(freq) / fs;
    let mut g = 0.0;
    for slot in 0..q::NUM_BANDS {
        let (used, active, d) = band_design(values, slot, fs);
        if used && active {
            g += q::magnitude_db(&d, w);
        }
    }
    g as f32
}

// ---------------------------------------------------------------- the plugin
/// Never-crash guard, not in the C++: a sample the engine pushed past the float range (24 bands
/// of +30 dB shelves stacked is +720 dB, which the C++ casts to inf) becomes 0, as the C++ render
/// tool zeroes it. Only samples the C++ makes non-finite are touched, so it cannot move a
/// fidelity result, and the dynamic ceiling never sees an inf.
///
/// Side effect, a deliberate divergence: after a non-finite event the C++ DynCeiling's state
/// (`specPow`, the overlap-add buffers) goes NaN for good and it stops cutting; ours never sees
/// the inf, so it keeps cutting.
fn flush_non_finite(io: &mut [Vec<f32>], nc: usize, frames: usize) {
    for ch in io.iter_mut().take(nc) {
        for v in ch.iter_mut().take(frames) {
            if !v.is_finite() {
                *v = 0.0;
            }
        }
    }
}

/// The CleanQ port.
///
/// `readouts`: `[0]` the auto-gain compensation in dB (as the engine publishes it), `[1]` the
/// dynamic offset in dB of the band moving the most (signed: the largest magnitude across bands).
pub struct CleanQ {
    p: Params,
    dsp: Box<CleanQDsp>,
    dyn_ceil: Box<DynCeiling>,
    analysis: Arc<Analysis>,
    last_pushed: [f32; q::NUM_PARAMS],
    force_push: bool,
    dyn_was_on: bool,
    channels: usize,
    sample_rate: f64,
    /// It has processed audio: `prepare` and `reset` are then live resets ([`CleanQ::live`]);
    /// before that they are the fresh `prepareToPlay` the render tool measures.
    processed: bool,
    /// The state `load_state` read (what `save_state` writes back).
    ceiling: CeilingState,
}

/// The ceiling grid a stored state works out to (`pushCeilingToAudio`): the shape on the 256-point
/// grid in dBFS, empty when there is no shape. A state that cannot be read at all (not text, too
/// big) is no ceiling, as a state without a `ceiling` element is to the C++. Allocates: off the
/// audio thread.
pub fn prepared_ceiling(state: &[u8]) -> Vec<f64> {
    CeilingState::read(state).map(|s| s.shape.to_grid()).unwrap_or_default()
}

impl Default for CleanQ {
    fn default() -> Self {
        Self::new()
    }
}

impl CleanQ {
    pub fn new() -> Self {
        let analysis = Arc::new(Analysis::with_live(LIVE_LEN));
        let mut s = CleanQ {
            p: Params::new(&CLEANQ_PARAMS),
            dsp: Box::new(CleanQDsp::new(Arc::clone(&analysis))),
            dyn_ceil: Box::new(DynCeiling::new()),
            analysis,
            last_pushed: [0.0; q::NUM_PARAMS],
            force_push: true,
            dyn_was_on: false,
            channels: 2,
            sample_rate: 48_000.0,
            processed: false,
            ceiling: CeilingState::default(),
        };
        s.prepare(48_000.0, 512, 2);
        s
    }

    /// `packForDsp` with the processor's push: only what moved since the last push, or all of it
    /// right after `prepare` (`forcePush`).
    fn push_params(&mut self) {
        let force = std::mem::replace(&mut self.force_push, false);
        let p = &self.p;
        let dsp = &mut self.dsp;
        let last = &mut self.last_pushed;
        pack_for_dsp(
            |i| p.v(i),
            |i, v| {
                if let Some(slot) = last.get_mut(i)
                    && (force || v != *slot)
                {
                    *slot = v;
                    dsp.set_parameter(i, v);
                }
            },
        );
    }

    /// The ceiling's shape on the grid (`clean_q_ceiling::Ceiling::to_grid`), in dBFS, for the
    /// dynamic ceiling (`pushCeilingToAudio`). Empty or wrong-sized: no ceiling. Never allocates.
    /// Never-crash guard, not in the C++ (whose grid comes from limited, finite points): a grid
    /// with a non-finite value is no ceiling.
    pub fn set_ceiling(&mut self, grid_db: &[f64]) {
        if grid_db.iter().all(|v| v.is_finite()) {
            self.dyn_ceil.set_ceiling(grid_db);
        } else {
            self.dyn_ceil.set_ceiling(&[]);
        }
    }

    /// The live reset (rules: `reset` on Play/Stop/Locate reproduces `prepareToPlay` on the live
    /// state): every value pushed now, then the engine's audio state cleared with each glide
    /// seated at its target (`CleanQDsp::reset_live`) and the dynamic ceiling's frames cleared.
    /// Settings and the ceiling shape are kept. No allocation.
    fn live(&mut self) {
        self.force_push = true;
        self.push_params();
        self.dsp.reset_live(self.sample_rate);
        self.dyn_ceil.reset();
        self.dyn_was_on = self.p.v(C_ON) > 0.5f32;
    }

    /// Publishes the live values ([`LIVE_DYN`], [`LIVE_CUT`], [`LIVE_CUTTING`]) for the editor.
    /// Atomic stores only: no allocation, safe on the audio thread.
    fn publish_live(&self) {
        let live = &self.analysis.live;
        for (i, v) in self.dsp.dyn_offset_db.iter().enumerate() {
            live.set(LIVE_DYN.start + i, if v.is_finite() { *v } else { 0.0 });
        }
        let mut cutting = false;
        for i in 0..LIVE_CUT.len() {
            let c = if self.dyn_was_on { self.dyn_ceil.cut_at(i) } else { 0.0 };
            let c = if c.is_finite() { c } else { 0.0 };
            cutting |= c < -0.1;
            live.set(LIVE_CUT.start + i, c);
        }
        live.set(LIVE_CUTTING, if cutting { 1.0 } else { 0.0 });
    }

    /// Whether a ceiling shape is set (the dynamic ceiling has something to hold to).
    pub fn has_ceiling(&self) -> bool {
        self.dyn_ceil.has_ceiling()
    }

    /// The dynamic ceiling's cut right now at a grid point (dB, 0 or below).
    pub fn dyn_cut_at(&self, grid_index: usize) -> f32 {
        self.dyn_ceil.cut_at(grid_index)
    }

    /// Whether the dynamic ceiling is switched on (as of the last block).
    pub fn is_dyn_ceiling_on(&self) -> bool {
        self.dyn_was_on
    }

    /// Per-band dynamic offset in dB (the moving node in the graph).
    pub fn dyn_offset_db(&self, slot: usize) -> f32 {
        self.dsp.dyn_offset_db.get(slot).copied().unwrap_or(0.0)
    }

    /// |H| in dB of one band at `w` from the engine's live (smoothed) coefficients.
    pub fn live_band_magnitude_db(&self, slot: usize, w: f64) -> f64 {
        self.dsp.band_magnitude_db(slot, w)
    }

    /// The session sample rate (`getSampleRateForDisplay`).
    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }
}

impl CleanQ {
    /// `processBlock`: the engine, then the dynamic ceiling after everything else.
    fn process_block(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let nc = self.channels.min(io.len());
        if nc == 0 {
            return;
        }
        self.processed = true;

        self.push_params();
        self.dsp.process(io, nc, frames);
        flush_non_finite(io, nc, frames);

        // ---- the dynamic ceiling, after everything else
        let on = self.p.v(C_ON) > 0.5f32;
        if on != self.dyn_was_on {
            self.dyn_was_on = on;
            if on {
                self.dyn_ceil.reset();
            }
        }

        if on {
            let s = clean_q_dyn_ceiling::Settings {
                on: true,
                threshold_db: f64::from(self.p.v(C_THRESHOLD)),
                amount: f64::from(self.p.v(C_AMOUNT)) * 0.01,
                max_cut_db: f64::from(self.p.v(C_MAX_CUT)),
                attack_ms: f64::from(self.p.v(C_ATTACK)),
                release_ms: f64::from(self.p.v(C_RELEASE)),
                delta: self.p.v(C_DELTA) > 0.5f32,
                bypass: self.p.v(G_BYPASS) > 0.5f32,
            };
            self.dyn_ceil.set_settings(&s);

            let Some((first, rest)) = io.split_first_mut() else { return };
            let r = if nc > 1 { rest.first_mut().map(Vec::as_mut_slice) } else { None };
            self.dyn_ceil.process(first.as_mut_slice(), r, frames);
            flush_non_finite(io, nc, frames);
        }
    }
}

impl Plugin for CleanQ {
    fn info(&self) -> &'static PluginInfo {
        &CLEANQ_INFO
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.p.set(id, value).is_some()
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.channels = channels.min(2);
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        self.sample_rate = sr;
        // Allocates the frame buffers for this rate (the largest frame is 8192, at > 100 kHz).
        self.dyn_ceil.prepare(sr);
        self.dyn_was_on = self.p.v(C_ON) > 0.5f32;
        if self.processed {
            self.live();
            self.publish_live();
            return;
        }
        self.dsp.prepare(sr);
        self.force_push = true;
        self.publish_live();
    }

    fn reset(&mut self) {
        // Not played yet: the fresh `prepareToPlay` (every value pushed again at the next block,
        // the glides from the engine's start, as the render tool hears it). Played: a live reset.
        // The ceiling shape is kept either way (the C++ pushes it again in `prepareToPlay`).
        if self.processed {
            self.live();
            self.publish_live();
            return;
        }
        self.dsp.prepare(self.sample_rate);
        self.dyn_ceil.reset();
        self.force_push = true;
        self.publish_live();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.process_block(io, frames);
        self.publish_live();
    }
    fn latency(&self) -> usize {
        if self.dyn_was_on { self.dyn_ceil.latency_samples() } else { 0 }
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        out[0] = self.dsp.auto_gain_db;
        out[1] = self.dsp.dyn_offset_db.iter().copied().fold(0.0f32, |m, v| if v.abs() > m.abs() { v } else { m });
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(Arc::clone(&self.analysis))
    }

    fn set_prepared_state(&mut self, data: &[f64]) -> bool {
        self.set_ceiling(data);
        true
    }

    /// The `uiState/ceiling` element ([`CeilingState::write`]) last read by `load_state`.
    ///
    /// The host never reads a live CleanQ's state: the session's `Insert::state` is the source
    /// of truth (edited by commands, handed over prepared through `set_prepared_state`), and
    /// CleanQ has no `state_reader`, so `capture_states` (session save, bounce, module copy)
    /// skips it. A live instance's copy may lag the session; nothing serialises it.
    fn save_state(&mut self) -> Option<Vec<u8>> {
        Some(self.ceiling.write())
    }

    /// Reads a `ceiling` element (`readFrom`) and pushes its shape. `false`, keeping the previous
    /// state, only for data that is not a state at all (not text, too big).
    fn load_state(&mut self, data: &[u8]) -> bool {
        let Some(st) = CeilingState::read(data) else { return false };
        self.set_ceiling(&st.shape.to_grid());
        self.ceiling = st;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::fs::{FsPreset, factory_presets};
    use std::time::Instant;

    fn sine(len: usize, amp: f32) -> Vec<f32> {
        (0..len).map(|i| amp * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin()).collect()
    }

    fn noise(len: usize, amp: f32, mut seed: u32) -> Vec<f32> {
        (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                amp * ((seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0)
            })
            .collect()
    }

    fn prepared() -> CleanQ {
        let mut s = CleanQ::new();
        s.prepare(48_000.0, 512, 2);
        s
    }

    fn run(s: &mut CleanQ, io: &mut [Vec<f32>], block: usize) {
        let len = io.iter().map(Vec::len).min().unwrap_or(0);
        let mut buf: Vec<Vec<f32>> = io.iter().map(|_| vec![0.0; block]).collect();
        let mut pos = 0;
        while pos < len {
            let n = block.min(len - pos);
            for (b, c) in buf.iter_mut().zip(io.iter()) {
                b[..n].copy_from_slice(&c[pos..pos + n]);
            }
            s.process(&mut buf, n);
            for (b, c) in buf.iter().zip(io.iter_mut()) {
                c[pos..pos + n].copy_from_slice(&b[..n]);
            }
            pos += n;
        }
    }

    fn all_finite(io: &[Vec<f32>]) -> bool {
        io.iter().flatten().all(|v| v.is_finite())
    }

    /// A busy setting: every type, place and slope somewhere, dynamics on the shelves and bells.
    fn busy(s: &mut CleanQ) {
        for b in 0..q::NUM_BANDS {
            let t = format!("b{:02}_", b + 1);
            s.set_param(&format!("{t}on"), 2.0);
            s.set_param(&format!("{t}type"), (b % 7) as f32);
            s.set_param(&format!("{t}place"), (b % 5) as f32);
            s.set_param(&format!("{t}slope"), (b % 8) as f32);
            s.set_param(&format!("{t}dyn"), (b % 2) as f32);
            s.set_param(&format!("{t}freq"), 0.1 + 0.035 * b as f32);
            s.set_param(&format!("{t}gain"), 0.3 + 0.017 * b as f32);
            s.set_param(&format!("{t}thr"), -40.0);
            s.set_param(&format!("{t}range"), if b % 3 == 0 { 9.0 } else { -9.0 });
        }
    }

    #[test]
    fn table_matches_the_cpp_order_and_defaults() {
        assert_eq!(CLEANQ_PARAMS.len(), 254);
        let ids: Vec<&str> = CLEANQ_PARAMS.iter().map(|p| p.id).collect();
        assert_eq!(ids[..7], ["outGain", "bypass", "invert", "solo", "autoGain", "dynAttack", "dynRelease"]);
        assert_eq!(ids[7..17], ["b01_on", "b01_type", "b01_place", "b01_slope", "b01_dyn", "b01_freq", "b01_gain", "b01_q", "b01_thr", "b01_range"]);
        assert_eq!(ids[band_param(23, F_RANGE)], "b24_range");
        assert_eq!(ids[247..], ["ceilOn", "ceilThr", "ceilAmount", "ceilMaxCut", "ceilAttack", "ceilRelease", "ceilDelta"]);
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), 254);
        assert_eq!(CLEANQ_PARAMS[G_DYN_ATTACK].default, 0.540725);
        assert_eq!(CLEANQ_PARAMS[G_DYN_RELEASE].default, 0.433677);
        assert_eq!(CLEANQ_PARAMS[band_param(4, F_THRESHOLD)].default, -20.0);
        assert_eq!((CLEANQ_PARAMS[C_RELEASE].min, CLEANQ_PARAMS[C_RELEASE].max, CLEANQ_PARAMS[C_RELEASE].default), (20.0, 1000.0, 120.0));
        assert_eq!(CLEANQ_PARAMS[G_SOLO].choices.len(), 25);
        assert_eq!(CLEANQ_PARAMS[G_OUT_GAIN].group, "Global");
        assert_eq!(CLEANQ_PARAMS[band_param(11, F_GAIN)].group, "Band 12");
        assert_eq!(CLEANQ_PARAMS[band_param(11, F_GAIN)].name, "B12 Gain");
        assert_eq!(CLEANQ_PARAMS[C_AMOUNT].group, "Ceiling");
        assert_eq!(CLEANQ_PARAMS[G_OUT_GAIN].format(0.5), "+0.0 dB");
        assert_eq!(CLEANQ_PARAMS[band_param(0, F_FREQ)].format(1.0), "20.00 kHz");
        assert_eq!(CLEANQ_PARAMS[band_param(0, F_GAIN)].format(0.6), "+6.0 dB");
        for k in KEY_KNOBS {
            assert!(CLEANQ_INFO.param(k).is_some(), "{k}");
        }
    }

    #[test]
    fn registered_and_latency_follows_the_dynamic_ceiling() {
        let mut p = crate::create("fs_cleanq").unwrap();
        assert_eq!(p.info().category, Category::FireStation);
        assert_eq!(p.latency(), 0, "zero latency while the dynamic ceiling is off");
        let mut io = vec![sine(512, 0.5), sine(512, 0.5)];
        p.set_param("ceilOn", 1.0);
        assert_eq!(p.latency(), 0, "as the C++: the switch is noticed by the next block");
        p.process(&mut io, 512);
        assert_eq!(p.latency(), 2048);
        p.set_param("ceilOn", 0.0);
        p.process(&mut io, 512);
        assert_eq!(p.latency(), 0);
        // At 96 kHz the frame (and latency) is 4096; prepare picks up a switched-on ceiling.
        p.set_param("ceilOn", 1.0);
        p.prepare(96_000.0, 512, 2);
        assert_eq!(p.latency(), 4096);
        p.prepare(192_000.0, 512, 2);
        assert_eq!(p.latency(), 8192);
    }

    #[test]
    fn dynamic_ceiling_delays_by_its_latency_and_cuts_against_a_ceiling() {
        let mut s = prepared();
        s.set_param("ceilOn", 1.0);
        let src = sine(48_000, 0.5);
        let mut io = vec![src.clone(), src.clone()];
        run(&mut s, &mut io, 512);
        for i in 10_000..48_000 {
            assert!((io[0][i] - src[i - 2048]).abs() < 1e-5, "{i}");
        }
        assert!(s.is_dyn_ceiling_on());

        let mut s = prepared();
        s.set_param("ceilOn", 1.0);
        s.set_ceiling(&clean_q_ceiling::flat_ceiling(-60.0).to_grid());
        let mut io = vec![noise(48_000, 0.05, 3), noise(48_000, 0.05, 4)];
        for (i, v) in io[0].iter_mut().enumerate() {
            *v += 0.5 * (std::f32::consts::TAU * 2000.0 * i as f32 / 48_000.0).sin();
        }
        run(&mut s, &mut io, 512);
        assert!(all_finite(&io));
        assert!((0..clean_q_ceiling::GRID_N).any(|i| s.dyn_cut_at(i) < -3.0));
    }

    /// The editor's live values: each dynamic band's offset, the dynamic ceiling's cut and its
    /// cutting flag, published after every block.
    #[test]
    fn cleanq_publishes_band_offsets_and_cut() {
        let mut s = prepared();
        let tone =
            |len: usize, amp: f32| -> Vec<f32> { (0..len).map(|i| amp * (std::f32::consts::TAU * 1000.0 * i as f32 / 48_000.0).sin()).collect() };
        s.set_param("b01_on", 2.0);
        s.set_param("b01_type", q::TYPE_BELL as f32);
        s.set_param("b01_dyn", 1.0);
        s.set_param("b01_freq", q::freq_to_knob(1000.0));
        s.set_param("b01_thr", -24.0);
        s.set_param("b01_range", -6.0);
        let a = s.analysis().unwrap();
        assert_eq!(a.live.len(), LIVE_LEN);
        // -6 dBFS: amplitude 0.5.
        let mut io = vec![tone(48_000, 0.5), tone(48_000, 0.5)];
        run(&mut s, &mut io, 512);
        let off = a.live.get(LIVE_DYN.start);
        assert!((-6.0..-1.0).contains(&off), "band 1's live offset {off}");
        assert_eq!(off, s.dyn_offset_db(0));
        assert_eq!(a.live.get(LIVE_CUTTING), 0.0, "the dynamic ceiling is off");

        // The dynamic ceiling on, the signal well over a flat -60 dBFS ceiling: it cuts.
        s.set_param("ceilOn", 1.0);
        s.set_ceiling(&clean_q_ceiling::flat_ceiling(-60.0).to_grid());
        let mut io = vec![tone(48_000, 0.5), tone(48_000, 0.5)];
        run(&mut s, &mut io, 512);
        assert_eq!(a.live.get(LIVE_CUTTING), 1.0);
        assert!(LIVE_CUT.clone().any(|i| a.live.get(i) < -0.5));
        assert_eq!(a.live.get(LIVE_CUT.start + 100), s.dyn_cut_at(100));
        assert!((0..LIVE_LEN).all(|i| a.live.get(i).is_finite()));
    }

    #[test]
    fn live_values_do_not_allocate() {
        // As `process_does_not_allocate`: the live block is allocated once, and its address and
        // length stay put across 1000 blocks and resets.
        let mut s = prepared();
        busy(&mut s);
        s.set_param("ceilOn", 1.0);
        let a = s.analysis().unwrap();
        let before = (a.live.buffer_address(), a.live.len());
        let mut io = vec![sine(256, 0.7), sine(256, 0.7)];
        for blk in 0..1000 {
            s.process(&mut io, 256);
            if blk % 100 == 0 {
                s.reset();
            }
        }
        let after = s.analysis().unwrap();
        assert!(Arc::ptr_eq(&a, &after));
        assert_eq!(before, (after.live.buffer_address(), after.live.len()));
        assert!((0..LIVE_LEN).all(|i| after.live.get(i).is_finite()));
    }

    #[test]
    fn the_host_never_reads_a_live_cleanq_state() {
        // `capture_states` reads only plugins with a state reader: CleanQ has none, so a stale
        // copy in a live instance can never overwrite the session's state.
        assert!(CleanQ::new().state_reader().is_none());
    }

    #[test]
    fn readouts_auto_gain_and_dyn_offset() {
        let mut s = prepared();
        s.set_param("autoGain", 1.0);
        s.set_param("b01_on", 2.0);
        s.set_param("b01_freq", q::freq_to_knob(1000.0));
        s.set_param("b01_gain", q::gain_to_knob(12.0));
        s.set_param("b02_on", 2.0);
        s.set_param("b02_dyn", 1.0);
        s.set_param("b02_freq", q::freq_to_knob(220.0));
        s.set_param("b02_thr", -40.0);
        s.set_param("b02_range", -9.0);
        let mut io = vec![sine(96_000, 0.5), sine(96_000, 0.5)];
        run(&mut s, &mut io, 512);
        let mut r = [f32::NAN; 8];
        s.readouts(&mut r);
        assert!(r[0] < -0.5 && r[0] > -12.0, "auto gain compensates a boost: {r:?}");
        assert!(r[1] < -8.0, "the dynamic band is pulled down by its range: {r:?}");
        assert!(r[2].is_nan());
        assert_eq!(s.dyn_offset_db(1), r[1]);
    }

    #[test]
    fn analysis_rings_take_the_engine_input_and_output() {
        let mut s = prepared();
        let a = s.analysis().unwrap();
        assert!(Arc::ptr_eq(&a, &s.analysis().unwrap()));
        s.set_param("outGain", 1.0); // +24 dB
        let src = sine(4096, 0.01);
        let mut io = vec![src.clone(), src.clone()];
        run(&mut s, &mut io, 512);
        assert_eq!(a.pre.written(), 4096);
        let mut pre = [0.0f32; 4];
        let mut post = [0.0f32; 4];
        a.pre.latest(&mut pre);
        a.post.latest(&mut post);
        assert_eq!(pre[..], src[4092..]);
        assert_eq!(post[..], io[0][4092..]);
    }

    #[test]
    fn response_db_single_bell_and_all_off() {
        let mut v: Vec<f32> = CLEANQ_PARAMS.iter().map(|p| p.default).collect();
        for f in [20.0, 100.0, 1000.0, 10_000.0, 20_000.0] {
            assert_eq!(response_db(&v, 48_000.0, f), 0.0, "all off is flat at {f} Hz");
        }
        v[band_param(0, F_ON)] = 2.0;
        v[band_param(0, F_TYPE)] = 0.0;
        v[band_param(0, F_FREQ)] = q::freq_to_knob(1000.0);
        v[band_param(0, F_GAIN)] = q::gain_to_knob(6.0);
        let at_1k = response_db(&v, 48_000.0, 1000.0);
        assert!((at_1k - 6.0).abs() < 0.1, "{at_1k}");
        assert!(response_db(&v, 48_000.0, 50.0).abs() < 0.2);
        // band_magnitude_db agrees at the engine's default 48 kHz.
        let w = 2.0 * q::PI * 1000.0 / 48_000.0;
        assert!((band_magnitude_db(&v, 0, w) - f64::from(at_1k)).abs() < 1e-6);
        // Bypassed bands do not count; out-of-range slots and garbage do not panic.
        v[band_param(0, F_ON)] = 1.0;
        assert_eq!(response_db(&v, 48_000.0, 1000.0), 0.0);
        assert_eq!(band_magnitude_db(&v, 99, w), 0.0);
        assert!(response_db(&[f32::NAN; 3], f32::NAN, f32::INFINITY).is_finite());
    }

    #[test]
    fn response_db_matches_the_engine_curve() {
        // The UI curve from the knob values equals the live coefficients once the glides settle.
        let mut s = prepared();
        busy(&mut s);
        let mut io = vec![vec![0.0f32; 48_000], vec![0.0f32; 48_000]];
        run(&mut s, &mut io, 512);
        let values: Vec<f32> = CLEANQ_PARAMS.iter().map(|p| s.param(p.id).unwrap_or(p.default)).collect();
        for slot in [0usize, 3, 4, 9, 13] {
            let w = 2.0 * q::PI * 700.0 / 48_000.0;
            if slot % 2 == 1 && slot % 7 <= 2 {
                continue; // dynamic bands move with the signal
            }
            let live = s.live_band_magnitude_db(slot, w);
            let ui = band_magnitude_db(&values, slot, w);
            assert!((live - ui).abs() < 0.01, "slot {slot}: {live} vs {ui}");
        }
    }

    #[test]
    fn every_factory_preset_parses() {
        let presets = factory_presets("fs_cleanq");
        assert_eq!(presets.len(), 22);
        for (name, json) in presets {
            let p = FsPreset::parse(json, &CLEANQ_INFO).unwrap();
            assert_eq!(&p.name, name);
            assert!(p.values.len() >= 245, "{name}: {}", p.values.len());
            assert_eq!(p.values["b01_on"].fract(), 0.0);
        }
        let p = FsPreset::parse(PRESETS[21].1, &CLEANQ_INFO).unwrap();
        assert_eq!(p.name, "Vocal cleanup");
        assert_eq!((p.values["b01_type"], p.values["b01_slope"], p.values["b03_thr"]), (3.0, 1.0, -24.0));
    }

    #[test]
    fn hostile_params_stay_finite() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9, 0.0, 1.0] {
            for ceil in [0.0, 1.0] {
                let mut s = prepared();
                busy(&mut s);
                // Bypass stays off: a hostile value that turned it on would hide the wet path.
                for info in CLEANQ_PARAMS.iter().filter(|p| p.id != "bypass") {
                    assert!(s.set_param(info.id, bad));
                }
                s.set_param("ceilOn", ceil);
                s.set_ceiling(&clean_q_ceiling::flat_ceiling(-50.0).to_grid());
                assert_eq!(s.param("bypass"), Some(0.0));
                let mut io = vec![sine(8192, 0.9), noise(8192, 0.9, 7)];
                run(&mut s, &mut io, 256);
                assert!(all_finite(&io), "value {bad}, ceiling {ceil}");
            }
        }
    }

    #[test]
    fn hostile_bypass_values_stay_finite() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0, 1e9, -1e9] {
            let mut s = prepared();
            busy(&mut s);
            s.set_param("ceilOn", 1.0);
            let src = [sine(8192, 0.9), noise(8192, 0.9, 9)];
            // Half the run with the wet path engaged, then the hostile bypass value mid-stream.
            let mut a = vec![src[0][..4096].to_vec(), src[1][..4096].to_vec()];
            run(&mut s, &mut a, 256);
            assert!(s.set_param("bypass", bad));
            let mut b = vec![src[0][4096..].to_vec(), src[1][4096..].to_vec()];
            run(&mut s, &mut b, 256);
            assert!(all_finite(&a) && all_finite(&b), "bypass {bad}");
        }
    }

    #[test]
    fn switching_quality_and_model_mid_stream_is_safe() {
        // CleanQ has no quality or model: the switches are band type, slope, placement, the
        // dynamic ceiling and solo, every one of them while audio plays.
        let mut s = prepared();
        let mut buf = vec![vec![0.0f32; 128], vec![0.0f32; 128]];
        let src = noise(128 * 400, 0.8, 11);
        for blk in 0..400 {
            for b in 0..4 {
                let t = format!("b{:02}_", b + 1);
                s.set_param(&format!("{t}on"), ((blk + b) % 3) as f32);
                s.set_param(&format!("{t}type"), ((blk + b) % 7) as f32);
                s.set_param(&format!("{t}slope"), ((blk / 2 + b) % 8) as f32);
                s.set_param(&format!("{t}place"), ((blk / 3 + b) % 5) as f32);
                s.set_param(&format!("{t}dyn"), ((blk / 5 + b) % 2) as f32);
            }
            s.set_param("ceilOn", ((blk / 7) % 2) as f32);
            s.set_param("solo", (blk % 6) as f32);
            for b in &mut buf {
                b.copy_from_slice(&src[blk * 128..(blk + 1) * 128]);
            }
            s.process(&mut buf, 128);
            assert!(all_finite(&buf), "block {blk}");
        }
    }

    #[test]
    fn mono_and_wide_buffers() {
        // Mono: one channel processed (the right is the left, as the C++ with no second channel).
        let mut s = CleanQ::new();
        s.prepare(48_000.0, 512, 1);
        s.set_param("b01_on", 2.0);
        s.set_param("b01_gain", 0.8);
        s.set_param("b01_freq", q::freq_to_knob(220.0));
        let mut mono = vec![sine(4096, 0.5)];
        let orig = mono.clone();
        run(&mut s, &mut mono, 512);
        assert!(all_finite(&mono));
        assert_ne!(mono, orig);

        // Six channels: the first two processed, the rest untouched (with the ceiling on too).
        let mut s = CleanQ::new();
        s.prepare(48_000.0, 512, 6);
        busy(&mut s);
        s.set_param("ceilOn", 1.0);
        let mut wide: Vec<Vec<f32>> = (0..6).map(|c| sine(8192, 0.1 * (c + 1) as f32)).collect();
        let orig = wide.clone();
        run(&mut s, &mut wide, 512);
        assert!(all_finite(&wide));
        assert_ne!(wide[0], orig[0]);
        assert_ne!(wide[1], orig[1]);
        assert_eq!(wide[2..], orig[2..]);

        // Fewer buffers than prepared, short buffers, zero frames: no panic.
        let mut s = prepared();
        s.set_param("ceilOn", 1.0);
        let mut one = vec![vec![0.3f32; 10]];
        s.process(&mut one, 512);
        s.process(&mut [], 64);
        let mut empty = vec![Vec::new(), Vec::new()];
        s.process(&mut empty, 64);
        let mut ragged = vec![vec![0.3f32; 10], vec![0.3f32; 3]];
        s.process(&mut ragged, 512);
    }

    #[test]
    fn silence_tail_is_cheap() {
        let sr = 48_000usize;
        let mut cases = Vec::new();
        for ceil in [0.0, 1.0] {
            let time = |signal_s: usize, silence_s: usize| {
                let mut s = prepared();
                busy(&mut s);
                s.set_param("ceilOn", ceil);
                let mut io = vec![noise(signal_s * sr, 0.9, 5), noise(signal_s * sr, 0.9, 6)];
                for c in &mut io {
                    c.resize((signal_s + silence_s) * sr, 0.0);
                }
                let t = Instant::now();
                run(&mut s, &mut io, 512);
                assert!(all_finite(&io));
                t.elapsed().as_secs_f64()
            };
            let tail = time(2, 10);
            let full = time(12, 0);
            cases.push((ceil, tail, full));
            assert!(tail < 4.0 * full, "ceiling {ceil}: silence tail {tail:.3}s vs signal {full:.3}s");
        }
        println!("{cases:?}");
    }

    #[test]
    fn process_does_not_allocate() {
        // No counting allocator in this crate. The engine is one boxed fixed-size struct; the
        // dynamic ceiling's buffers are allocated in prepare and only indexed after. Their
        // addresses and capacities, and the host buffers', must not move across 1000 blocks.
        let mut s = prepared();
        busy(&mut s);
        s.set_param("ceilOn", 1.0);
        let engine_ptr: *const CleanQDsp = &*s.dsp;
        let ceil_bufs = s.dyn_ceil.buffer_fingerprint();
        let mut io = vec![sine(256, 0.7), sine(256, 0.7)];
        let ptrs: Vec<(*const f32, usize)> = io.iter().map(|c| (c.as_ptr(), c.capacity())).collect();
        for blk in 0..1000 {
            s.set_param("b01_type", (blk % 7) as f32);
            s.set_param("ceilOn", ((blk / 50) % 2) as f32);
            s.process(&mut io, 256);
        }
        assert_eq!(engine_ptr, &*s.dsp as *const CleanQDsp);
        assert_eq!(ceil_bufs, s.dyn_ceil.buffer_fingerprint());
        assert_eq!(ptrs, io.iter().map(|c| (c.as_ptr(), c.capacity())).collect::<Vec<_>>());
        assert!(all_finite(&io));
    }

    /// A tone over noise, the dynamic ceiling's test signal.
    fn tone_over_noise(len: usize, seed: u32) -> Vec<Vec<f32>> {
        let mut io = vec![noise(len, 0.05, seed), noise(len, 0.05, seed + 1)];
        for c in &mut io {
            for (i, v) in c.iter_mut().enumerate() {
                *v += 0.5 * (std::f32::consts::TAU * 2000.0 * i as f32 / 48_000.0).sin();
            }
        }
        io
    }

    fn flat_state(db: f64) -> Vec<u8> {
        CeilingState { shape: clean_q_ceiling::flat_ceiling(db), ..CeilingState::default() }.write()
    }

    #[test]
    fn the_ceiling_state_loads_saves_and_is_handed_over_prepared() {
        let mut s = prepared();
        assert!(!s.has_ceiling());
        assert_eq!(s.save_state(), Some(CeilingState::default().write()));
        let state = flat_state(-60.0);
        assert!(s.load_state(&state));
        assert!(s.has_ceiling());
        assert_eq!(s.save_state(), Some(state.clone()));
        // The prepared grid is the shape on the grid; the plugin takes it without the state.
        let grid = crate::prepare_state("fs_cleanq", &state).unwrap();
        assert_eq!(grid, clean_q_ceiling::flat_ceiling(-60.0).to_grid());
        assert_eq!(crate::prepare_state("fs_comp", &state), None);
        let mut t = prepared();
        assert!(t.set_prepared_state(&grid));
        assert!(t.has_ceiling());
        t.set_param("ceilOn", 1.0);
        let mut io = tone_over_noise(48_000, 3);
        run(&mut t, &mut io, 512);
        assert!(all_finite(&io));
        assert!((0..clean_q_ceiling::GRID_N).any(|i| t.dyn_cut_at(i) < -3.0), "it cuts against the handed-over ceiling");
        // Clear: an empty grid is no ceiling, and the dynamic ceiling stops cutting.
        assert!(t.set_prepared_state(&[]));
        assert!(!t.has_ceiling());
    }

    #[test]
    fn bad_file_inputs() {
        // Corrupt state: garbage bytes, wrong lengths, NaN points, absurd counts. Nothing panics,
        // the module keeps working, and the fallback is no ceiling (or the previous state, for
        // data that is no state at all).
        let mut s = prepared();
        s.set_param("ceilOn", 1.0);
        assert!(s.load_state(&flat_state(-60.0)));
        assert!(!s.load_state(&[0xff, 0x00, 0xfe]), "not text: refused, the previous ceiling kept");
        assert!(s.has_ceiling());
        assert!(!s.load_state(&vec![b'x'; clean_q_ceiling_state::MAX_STATE_BYTES + 1]));
        let nan_points: String = (0..16).map(|k| format!("<point hz=\"{}\" db=\"NaN\"/>", 50 * (k + 1))).collect();
        let many = format!("<ceiling>{}</ceiling>", "<point hz=\"1e308\" db=\"-1e308\"/>".repeat(20_000));
        let cases: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"garbage".to_vec(),
            b"<ceiling".to_vec(),
            b"<ceiling><point hz=\"100\" db=\"-3\"/></ceiling>".to_vec(),
            format!("<ceiling>{nan_points}</ceiling>").into_bytes(),
            many.into_bytes(),
            b"<ceiling smooth=\"99999999999999999999\" avg=\"-inf\" maxBands=\"-7\"/>".to_vec(),
        ];
        for (i, c) in cases.iter().enumerate() {
            let mut s = prepared();
            s.set_param("ceilOn", 1.0);
            assert!(s.load_state(c), "case {i}: text reads as the C++ reads it");
            let grid = prepared_ceiling(c);
            assert!(grid.is_empty() || grid.len() == clean_q_ceiling::GRID_N, "case {i}");
            assert!(grid.iter().all(|v| v.is_finite()), "case {i}");
            let mut io = tone_over_noise(16_384, 5);
            run(&mut s, &mut io, 512);
            assert!(all_finite(&io), "case {i}");
        }
        assert!(prepared_ceiling(&[0xff; 7]).is_empty());
        // A host grid with NaN or the wrong length is no ceiling, not a NaN in the audio.
        let mut s = prepared();
        s.set_param("ceilOn", 1.0);
        let mut g = clean_q_ceiling::flat_ceiling(-60.0).to_grid();
        g[7] = f64::NAN;
        s.set_prepared_state(&g);
        assert!(!s.has_ceiling());
        s.set_prepared_state(&[1.0, 2.0]);
        assert!(!s.has_ceiling());
        let mut io = tone_over_noise(16_384, 9);
        run(&mut s, &mut io, 512);
        assert!(all_finite(&io));
    }

    /// Non-default settings with the dynamic ceiling on and a ceiling to hold to.
    fn live(s: &mut CleanQ) {
        busy(s);
        s.set_param("outGain", 0.6);
        s.set_param("ceilOn", 1.0);
        s.set_param("ceilThr", 1.5);
        s.set_param("ceilAmount", 90.0);
        assert!(s.load_state(&flat_state(-55.0)));
    }

    #[test]
    fn the_ceiling_survives_reset_and_two_resets_render_alike() {
        let mut s = prepared();
        live(&mut s);
        let mut warm = tone_over_noise(24_000, 21);
        run(&mut s, &mut warm, 512);
        s.reset();
        assert!(s.has_ceiling(), "reset keeps the shape");
        assert!(s.is_dyn_ceiling_on());
        let mut a = tone_over_noise(16_384, 31);
        run(&mut s, &mut a, 512);
        assert!((0..clean_q_ceiling::GRID_N).any(|i| s.dyn_cut_at(i) < -3.0), "still cutting against it after reset");

        // Another history, then reset: the same render from the same state.
        let mut other = tone_over_noise(30_000, 77);
        for c in &mut other {
            c.iter_mut().for_each(|v| *v *= 1.7);
        }
        run(&mut s, &mut other, 256);
        s.reset();
        let mut b = tone_over_noise(16_384, 31);
        run(&mut s, &mut b, 512);
        assert_eq!(a, b, "two resets from the same state render bit-identically");
        assert_eq!(s.latency(), 2048, "the dynamic ceiling's latency is unchanged by reset");
    }

    #[test]
    fn a_live_reset_does_not_reglide_and_equals_a_settled_instance() {
        // Settings far from the defaults: every band busy (dynamics too), output, invert, auto gain.
        let set = |s: &mut CleanQ| {
            busy(s);
            s.set_param("outGain", 0.8);
            s.set_param("invert", 1.0);
            s.set_param("autoGain", 1.0);
        };
        // A fresh instance that has settled on them over a second of silence.
        let mut settled = prepared();
        set(&mut settled);
        let mut quiet = vec![vec![0.0f32; 48_000], vec![0.0f32; 48_000]];
        run(&mut settled, &mut quiet, 512);
        // A played instance with another history, reset live.
        let mut live = prepared();
        set(&mut live);
        let mut history = vec![noise(30_000, 0.9, 41), noise(30_000, 0.9, 42)];
        run(&mut live, &mut history, 256);
        live.reset();
        let src = vec![noise(8192, 0.5, 51), sine(8192, 0.5)];
        let (mut a, mut b) = (src.clone(), src.clone());
        run(&mut settled, &mut a, 512);
        run(&mut live, &mut b, 512);
        let worst = a.iter().flatten().zip(b.iter().flatten()).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "no ramp from defaults after reset: worst difference {worst}");
        // From the first sample: +19 dB out-gain and the inversion are there at once.
        assert!(b[0][..64].iter().zip(&a[0][..64]).all(|(x, y)| (x - y).abs() < 1e-5));
    }

    #[test]
    fn reset_does_not_allocate() {
        // As `process_does_not_allocate`: the engine and the dynamic ceiling's buffers keep their
        // addresses and capacities across resets.
        let mut s = prepared();
        live(&mut s);
        let mut io = vec![sine(256, 0.7), sine(256, 0.7)];
        s.process(&mut io, 256);
        let engine_ptr: *const CleanQDsp = &*s.dsp;
        let ceil_bufs = s.dyn_ceil.buffer_fingerprint();
        for _ in 0..100 {
            s.reset();
            s.process(&mut io, 256);
        }
        assert_eq!(engine_ptr, &*s.dsp as *const CleanQDsp);
        assert_eq!(ceil_bufs, s.dyn_ceil.buffer_fingerprint());
        assert!(all_finite(&io));
    }

    #[test]
    fn engine_bypass_lands_exactly_on_the_input() {
        let mut s = prepared();
        busy(&mut s);
        s.set_param("bypass", 1.0);
        let src = sine(48_000, 0.6);
        let mut io = vec![src.clone(), src.clone()];
        run(&mut s, &mut io, 512);
        // The glide (0.25 a control tick) has long reached 1.0 in double; wet * 0 + dry.
        assert_eq!(io[0][24_000..], src[24_000..]);
    }
}
