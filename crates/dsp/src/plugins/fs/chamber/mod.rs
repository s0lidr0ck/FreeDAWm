//! `fs_chamber`: the owner's ChamberMade (a reverb chamber you build yourself), a faithful
//! translation of `ChamberMade/Source/ChamberMadeDSP.h` and `ChamberGeometry.h` (the sources the
//! CMake build compiles into `ChamberMade_Render`; `ChamberMade.cmajor` is the older original
//! they were ported from and is not built).
//!
//! Wrapper (`PluginProcessor.{h,cpp}`) audit, everything that affects the sound:
//! - The DSP's constructor runs `prepare (48000)` with its default knobs. `prepareToPlay` only
//!   calls `dsp.prepare (sampleRate)` (with whatever knobs were last pushed: the defaults on a
//!   fresh instance) and raises `forcePush`. It does NOT push the parameters first, so on a fresh
//!   instance the room is built from the defaults and the set parameters arrive with the first
//!   block (the geometry then rebuilds on the 64th sample and the taps glide). [`Plugin::prepare`]
//!   does the same for a fresh instance: the first prepare allocates the buffers and runs the
//!   constructor's prepare, then `dsp.prepare (rate)` plus the forced push. A prepare after the
//!   instance has played is a reset (below).
//! - `processBlock`: `ScopedNoDenormals`, clears outputs beyond the inputs (never applies: in =
//!   out), pushes only what moved (`v != lastPushed[i]`, everything after a prepare) in
//!   parameter order, then `dsp.process`. Here `set_param` only stores the value; `process` does
//!   the wrapper's push before handing the block to the engine.
//! - `getTailLengthSeconds`: the room's longest RT60 band plus the pre-delay plus four tape
//!   times: [`Plugin::tail_samples`] reports exactly that.
//! - Latency: none (`latencySamples` is 0; the dry path is untouched).
//! - Bypass: the engine's own `bypass` parameter is a 0.002-per-sample crossfade to the input
//!   plus the still-ringing wet (translated in the engine). Kept in order but hidden (group ""):
//!   the UI uses the module's bypass.
//! - APVTS ranges: floats 0..1, choices whole indices, the bool 0/1. `ParamInfo::clamp` does the
//!   same (NaN becomes the default), so the engine never sees a non-finite value.
//! - Bus layout: mono or stereo, in = out. With one channel the engine reads it as both sides
//!   and writes `0.5 * (yL + yR)`. Extra channels are left untouched (the Rust host contract).
//! - `ScopedNoDenormals` (flush-to-zero): no safe Rust equivalent. No decision in this engine
//!   compares against anything near the denormal range (the smallest threshold is 1e-9), so FTZ
//!   only changes values below ~1e-38, far under the fidelity bar; `silence_tail_is_cheap`
//!   guards the CPU side.
//! - The preset dirty-tracking, the UI state (More panel, ft / m, inspector target), state save
//!   and the editor are UI-only. A preset carries the room and its voicing, not Mix, Level or
//!   Bypass (`getPresetParams`); the factory files hold exactly those keys.
//! - `reset` (the DAW's Play / Stop / Locate and bounce start), and a re-`prepare` of an
//!   instance that has played: Rust-only by controller ruling. The current parameters are pushed
//!   first, then the C++ `prepareToPlay` (`dsp.prepare (rate)` plus the forced push), then every
//!   control value is computed at the LFO's phase 0 and every glide seeded at its target
//!   ([`ChamberMadeDsp::seed_after_reset`]): every buffer, filter and position cleared, the room
//!   the current one, nothing flushed, muted or swooping, and the render the same whatever was
//!   played before. No allocation (the buffers are fixed-size and only zeroed). The first,
//!   fresh prepare stays the C++ path exactly.
//!
//! Station (`ModuleRegistry.cpp`): `fixed ("chambermade", "ChamberMade", "", 0xFFFF9A3D, ..,
//! list ("mix,decay,predelay,level"))`: no model parameter, key knobs Mix, Decay, Pre-Delay,
//! Level.

// 0.7071 is the C++ default verbatim (not 1/sqrt 2).
#![allow(clippy::approx_constant)]

pub mod chamber_geometry;
pub mod chamber_made_dsp;
mod presets;

pub use chamber_made_dsp::{ChamberMadeDsp, NUM_PARAMS, build_chamber_for};
pub use presets::PRESETS;

use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, ParamInfo, Plugin, PluginInfo, gain_to_db};
use chamber_made_dsp as d;

/// Fifteen surfaces, shared by Walls, Floor and Ceiling (`kMaterialChoices`).
pub const MATERIAL_NAMES: [&str; 15] = [
    "Painted Concrete",
    "Hard Plaster",
    "Glazed Tile",
    "Bare Brick",
    "Rough Stone",
    "Steel Plate",
    "Glass",
    "Hardwood",
    "Wood Panel",
    "Drywall",
    "Packed Earth",
    "Carpet",
    "Heavy Drape",
    "Acoustic Foam",
    "Painted Cinder Block",
];
/// Nine object types including None (`kObjectChoices`).
pub const OBJECT_NAMES: [&str; 9] =
    ["None", "Concrete Pillar", "Ceramic Pot", "Wood Diffuser", "Heavy Drape", "Bass Trap", "Steel Barrel", "Sofa", "Glass Gobo"];
/// `kShapeChoices`.
pub const SHAPE_NAMES: [&str; 6] = ["Rectangle", "Trapezoid", "Pentagon", "Hexagon", "Skewed", "Irregular"];
/// `kSpeakerChoices`.
pub const SPEAKER_NAMES: [&str; 5] = ["Studio Monitor", "Horn Duplex", "Dual Concentric", "Small Cone", "PA Column"];
/// `kMicPatternChoices`.
pub const MIC_PATTERN_NAMES: [&str; 4] = ["Omni", "Cardioid", "Hyper", "Figure-8"];
/// `kMicChoices`.
pub const MIC_NAMES: [&str; 4] = ["Condenser", "Tube Condenser", "Ribbon", "Dynamic"];

// ------------------------------------------------------------------ printed values
// `formatParam` in Parameters.h, through the engine's own knob maps. Where the C++ prints a value
// that needs the built room (a position confined into the walls, the ceiling's two ends, the
// RT60 next to the decay multiplier), a formatter that only sees the value cannot, and prints
// what the value itself says instead (noted on each).

/// `juce::roundToInt`: round half up.
fn round_to_int(x: f32) -> i32 {
    (x + 0.5).floor() as i32
}

fn pct(v: f32) -> String {
    format!("{} %", round_to_int(v * 100.0))
}

/// `fmtDb()`: "off" at the bottom, "0.0 dB" at unity, a sign when above.
fn decibels(g: f32) -> String {
    let db = if g > 1.0e-6 { 20.0 * g.log10() } else { -120.0 };
    if db <= -90.0 {
        return "off".to_string();
    }
    if db.abs() < 0.05 {
        return "0.0 dB".to_string();
    }
    format!("{}{db:.1} dB", if db > 0.0 { "+" } else { "" })
}

/// `lengthString` in feet (the editor's default unit): one decimal under 100, none above.
pub fn feet_string(metres: f32) -> String {
    let x = metres * 3.280_84;
    if x < 100.0 { format!("{x:.1} ft") } else { format!("{x:.0} ft") }
}

fn degrees(v: f32) -> String {
    format!("{}\u{b0}", round_to_int(v * 360.0) % 360)
}

fn width_text(v: f32) -> String {
    format!("{} %", round_to_int(v * 200.0))
}

fn repeats_text(v: f32) -> String {
    format!("{} %", round_to_int(v * 90.0))
}

/// The C++ prints "x1.32 . 0.7 s" (the multiplier and the room's mid RT60); the RT60 needs the
/// built room, so the multiplier alone here (the strip's room plan shows the RT60s).
fn decay_text(v: f32) -> String {
    format!("\u{d7}{:.2}", d::knob_to_decay_mul(v))
}

fn pre_delay_text(v: f32) -> String {
    format!("{} ms", round_to_int(d::knob_to_pre_delay_ms(v)))
}

fn early_text(v: f32) -> String {
    decibels(d::knob_to_early_gain(v))
}

fn level_text(v: f32) -> String {
    decibels(d::knob_to_level_gain(v))
}

fn low_cut_text(v: f32) -> String {
    format!("{} Hz", round_to_int(d::knob_to_low_cut_hz(v)))
}

fn high_cut_text(v: f32) -> String {
    let f = d::knob_to_high_cut_hz(v);
    if f >= 10000.0 { format!("{:.1} k", f / 1000.0) } else { format!("{:.2} k", f / 1000.0) }
}

/// Room Width and Length: `ch.room.widthM` is exactly `knobToRoomW` of the value.
fn room_wl_text(v: f32) -> String {
    feet_string(d::knob_to_room_w(v))
}

fn room_h_text(v: f32) -> String {
    feet_string(d::knob_to_room_h(v))
}

/// The C++ prints the ceiling's two ends ("17.5 -> 13.6 ft"), which needs the room height; the
/// ends are the mean height plus and minus a quarter of the value, so that share is printed.
fn ceil_slope_text(v: f32) -> String {
    if v < 0.01 { "flat".to_string() } else { format!("\u{b1}{} %", round_to_int(v * 25.0)) }
}

fn tape_time_text(v: f32) -> String {
    format!("{} ms", round_to_int(d::knob_to_tape_ms(v)))
}

fn tape_tone_text(v: f32) -> String {
    format!("{:.1} k", d::knob_to_tape_tone_hz(v) / 1000.0)
}

/// Object Rotate: a half turn.
fn rotate_text(v: f32) -> String {
    format!("{}\u{b0}", round_to_int(v * 180.0) % 180)
}

/// Positions (speaker, mics, objects): the C++ prints the confined distance into the room, which
/// needs the built room; the percentage of the room is what the value is (the C++'s own text for
/// an object slot that is off).
fn position_text(v: f32) -> String {
    pct(v)
}

const BLEND: &str = "Blend";
const TONE: &str = "Tone";
const ROOM: &str = "Room";
const SPEAKER: &str = "Speaker";
const MICS: &str = "Mics";
const OBJECTS: &str = "Objects";
const TAPE: &str = "Tape";

const fn rotate(id: &'static str, name: &'static str) -> ParamInfo {
    knob01(id, name, 0.0, OBJECTS, rotate_text)
}

/// Parameters in the C++ order (`Parameters.h`, 79 of them), ids, names and defaults verbatim.
pub static CHAMBER_PARAMS: [ParamInfo; NUM_PARAMS] = [
    // ---- the mix and the voicing
    knob01("mix", "Mix", 0.35, BLEND, pct),
    knob01("decay", "Decay", 0.5, BLEND, decay_text),
    knob01("predelay", "Pre-Delay", 0.0, BLEND, pre_delay_text),
    knob01("early", "Early", 0.7071, BLEND, early_text),
    knob01("tail", "Tail", 0.7071, BLEND, early_text),
    knob01("lowCut", "Low Cut", 0.1772, TONE, low_cut_text),
    knob01("highCut", "High Cut", 1.0, TONE, high_cut_text),
    knob01("width", "Width", 0.5, BLEND, width_text),
    knob01("level", "Level", 0.7071, BLEND, level_text),
    // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
    fs_toggle("bypass", "Bypass", 0.0, ""),
    knob01("drive", "Drive", 0.0, TONE, pct),
    knob01("drift", "Drift", 0.2, TONE, pct),
    // ---- the room
    fs_choice("shape", "Shape", 5.0, ROOM, &SHAPE_NAMES),
    knob01("roomW", "Room Width", 0.3059, ROOM, room_wl_text),
    knob01("roomL", "Room Length", 0.4683, ROOM, room_wl_text),
    knob01("roomH", "Room Height", 0.3153, ROOM, room_h_text),
    knob01("taper", "Shape Amount", 0.7, ROOM, pct),
    fs_choice("wallMat", "Walls", 0.0, ROOM, &MATERIAL_NAMES),
    fs_choice("floorMat", "Floor", 0.0, ROOM, &MATERIAL_NAMES),
    fs_choice("ceilMat", "Ceiling", 0.0, ROOM, &MATERIAL_NAMES),
    // ---- the speaker
    knob01("spkX", "Speaker X", 0.25, SPEAKER, position_text),
    knob01("spkY", "Speaker Y", 0.15, SPEAKER, position_text),
    knob01("spkZ", "Speaker Height", 0.3, SPEAKER, position_text),
    knob01("spkAim", "Speaker Aim", 0.625, SPEAKER, degrees),
    fs_choice("spkType", "Speaker", 1.0, SPEAKER, &SPEAKER_NAMES),
    // ---- the microphones
    knob01("micAX", "Mic A X", 0.3, MICS, position_text),
    knob01("micAY", "Mic A Y", 0.8, MICS, position_text),
    knob01("micBX", "Mic B X", 0.7, MICS, position_text),
    knob01("micBY", "Mic B Y", 0.8, MICS, position_text),
    knob01("micZ", "Mic Height", 0.45, MICS, position_text),
    knob01("micAAim", "Mic A Aim", 0.375, MICS, degrees),
    knob01("micBAim", "Mic B Aim", 0.125, MICS, degrees),
    fs_choice("micPat", "Mic Pattern", 0.0, MICS, &MIC_PATTERN_NAMES),
    fs_choice("micType", "Mic", 0.0, MICS, &MIC_NAMES),
    // ---- eight object slots, four parameters each
    fs_choice("o1t", "Obj 1 Type", 0.0, OBJECTS, &OBJECT_NAMES),
    knob01("o1x", "Obj 1 X", 0.5, OBJECTS, position_text),
    knob01("o1y", "Obj 1 Y", 0.5, OBJECTS, position_text),
    knob01("o1s", "Obj 1 Size", 0.5, OBJECTS, pct),
    fs_choice("o2t", "Obj 2 Type", 0.0, OBJECTS, &OBJECT_NAMES),
    knob01("o2x", "Obj 2 X", 0.5, OBJECTS, position_text),
    knob01("o2y", "Obj 2 Y", 0.5, OBJECTS, position_text),
    knob01("o2s", "Obj 2 Size", 0.5, OBJECTS, pct),
    fs_choice("o3t", "Obj 3 Type", 0.0, OBJECTS, &OBJECT_NAMES),
    knob01("o3x", "Obj 3 X", 0.5, OBJECTS, position_text),
    knob01("o3y", "Obj 3 Y", 0.5, OBJECTS, position_text),
    knob01("o3s", "Obj 3 Size", 0.5, OBJECTS, pct),
    fs_choice("o4t", "Obj 4 Type", 0.0, OBJECTS, &OBJECT_NAMES),
    knob01("o4x", "Obj 4 X", 0.5, OBJECTS, position_text),
    knob01("o4y", "Obj 4 Y", 0.5, OBJECTS, position_text),
    knob01("o4s", "Obj 4 Size", 0.5, OBJECTS, pct),
    fs_choice("o5t", "Obj 5 Type", 0.0, OBJECTS, &OBJECT_NAMES),
    knob01("o5x", "Obj 5 X", 0.5, OBJECTS, position_text),
    knob01("o5y", "Obj 5 Y", 0.5, OBJECTS, position_text),
    knob01("o5s", "Obj 5 Size", 0.5, OBJECTS, pct),
    fs_choice("o6t", "Obj 6 Type", 0.0, OBJECTS, &OBJECT_NAMES),
    knob01("o6x", "Obj 6 X", 0.5, OBJECTS, position_text),
    knob01("o6y", "Obj 6 Y", 0.5, OBJECTS, position_text),
    knob01("o6s", "Obj 6 Size", 0.5, OBJECTS, pct),
    fs_choice("o7t", "Obj 7 Type", 0.0, OBJECTS, &OBJECT_NAMES),
    knob01("o7x", "Obj 7 X", 0.5, OBJECTS, position_text),
    knob01("o7y", "Obj 7 Y", 0.5, OBJECTS, position_text),
    knob01("o7s", "Obj 7 Size", 0.5, OBJECTS, pct),
    fs_choice("o8t", "Obj 8 Type", 0.0, OBJECTS, &OBJECT_NAMES),
    knob01("o8x", "Obj 8 X", 0.5, OBJECTS, position_text),
    knob01("o8y", "Obj 8 Y", 0.5, OBJECTS, position_text),
    knob01("o8s", "Obj 8 Size", 0.5, OBJECTS, pct),
    // ---- added later, which is why they are here and not above
    knob01("ceilSlope", "Ceiling Slope", 0.4, ROOM, ceil_slope_text),
    knob01("tapeAmt", "Tape Send", 0.0, TAPE, pct),
    knob01("tapeTime", "Tape Time", 0.4927, TAPE, tape_time_text),
    knob01("tapeFb", "Tape Repeats", 0.3, TAPE, repeats_text),
    knob01("tapeTone", "Tape Tone", 0.6, TAPE, tape_tone_text),
    // ---- and later still
    rotate("o1r", "Obj 1 Rotate"),
    rotate("o2r", "Obj 2 Rotate"),
    rotate("o3r", "Obj 3 Rotate"),
    rotate("o4r", "Obj 4 Rotate"),
    rotate("o5r", "Obj 5 Rotate"),
    rotate("o6r", "Obj 6 Rotate"),
    rotate("o7r", "Obj 7 Rotate"),
    rotate("o8r", "Obj 8 Rotate"),
];

pub static CHAMBER_INFO: PluginInfo = PluginInfo {
    id: "fs_chamber",
    name: "ChamberMade",
    short_name: "Chamber",
    category: Category::FireStation,
    params: &CHAMBER_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &CHAMBER_INFO,
    make: || Box::new(ChamberMade::new()),
    presets: PRESETS,
    order: 150,
    fidelity: Some(super::Fidelity { name: "chamber", source_dir: "ChamberMade", render_tool: "ChamberMade", tables: &[] }),
    prepare_state: None,
    actions: &[],
};

/// The ChamberMade port.
///
/// `readouts` (the C++ editor's polled outputs, from the room the DSP is in): `[0]` `[1]` `[2]`
/// the RT60 per band in seconds (`rtLowOut`, `rtMidOut`, `rtHighOut`), `[3]` the volume in m^3,
/// `[4]` how much the parallel walls ring (`flutterOut`, the editor says "some" above 0.12,
/// "strong" above 0.35), `[5]` the wet peak of the last block in dBFS (`wetOut`), `[6]` and
/// `[7]` the reflections that survived the validity tests per microphone (`tapsAOut`,
/// `tapsBOut`).
#[derive(Clone)]
pub struct ChamberMade {
    p: Params,
    dsp: Box<ChamberMadeDsp>,
    /// The wrapper's `lastPushed` and `forcePush`.
    last_pushed: [f32; NUM_PARAMS],
    force_push: bool,
    channels: usize,
    /// Whether this instance has processed audio since it was made: its first, fresh prepare
    /// follows the C++ exactly; a later one is a reset at that rate.
    played: bool,
}

impl Default for ChamberMade {
    fn default() -> Self {
        Self::new()
    }
}

impl ChamberMade {
    /// No buffers yet: they are allocated by the first `prepare` (`process` passes the input
    /// through untouched until then).
    pub fn new() -> Self {
        ChamberMade {
            p: Params::new(&CHAMBER_PARAMS),
            dsp: Box::new(ChamberMadeDsp::unallocated()),
            last_pushed: [0.0; NUM_PARAMS],
            force_push: true,
            channels: 2,
            played: false,
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

    /// `prepareToPlay`: `dsp.prepare (sampleRate)` with the knobs it holds, then force the next
    /// block's push. Allocation-free once the buffers exist.
    fn prepare_to_play(&mut self, sr: f64) {
        self.dsp.prepare(sr);
        self.force_push = true;
    }

    /// The host's reset (and a re-prepare of an instance that has played), Rust-only by
    /// controller ruling: the current parameters pushed FIRST (so the room is built from them and
    /// `tail_samples` reports it at once), then `prepareToPlay`, then every control value and
    /// glide seeded at its target ([`ChamberMadeDsp::seed_after_reset`]): deterministic whatever
    /// was played before, and nothing swoops. No allocation.
    fn reset_to(&mut self, sr: f64) {
        for i in 0..NUM_PARAMS {
            self.last_pushed[i] = self.p.v(i);
            self.dsp.set_parameter(i, self.last_pushed[i]);
        }
        self.prepare_to_play(sr);
        self.dsp.seed_after_reset();
    }

    /// The engine (tests and the UI's tests).
    pub fn engine(&self) -> &ChamberMadeDsp {
        &self.dsp
    }

    /// The C++ `getTailLengthSeconds`.
    fn tail_seconds(&self) -> f32 {
        let rt = self.dsp.rt_low_out.max(self.dsp.rt_mid_out.max(self.dsp.rt_high_out));
        let pre = 0.001 * d::knob_to_pre_delay_ms(self.p.v(d::P_PRE_DELAY));
        let tape = 0.001 * d::knob_to_tape_ms(self.p.v(d::P_TAPE_TIME));
        rt + pre + tape * 4.0
    }
}

impl Plugin for ChamberMade {
    fn info(&self) -> &'static PluginInfo {
        &CHAMBER_INFO
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
        if !self.dsp.is_allocated() {
            // The C++ constructor: the buffers, then `prepare (48000)` with the default knobs
            // (the `prepare` right after zeroes the buffers, so the constructor's need not).
            self.dsp.allocate();
            self.dsp.prepare_inner(48_000.0, false);
        }
        if self.played {
            self.reset_to(sr);
        } else {
            // The fresh instance: the C++ path exactly (the knobs it holds, the push with the
            // first block), which the fidelity references were rendered through.
            self.prepare_to_play(sr);
        }
    }

    fn reset(&mut self) {
        // No allocation: the buffers only get zeroed.
        if self.dsp.is_allocated() {
            let sr = f64::from(self.dsp.sample_rate());
            self.reset_to(sr);
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.push_params();
        self.played |= self.dsp.is_allocated();
        let nc = self.channels.min(io.len());
        self.dsp.process(io, nc, frames);
    }

    fn latency(&self) -> usize {
        0
    }

    fn tail_samples(&self) -> usize {
        let samples = (self.tail_seconds() * self.dsp.sample_rate()).ceil();
        if samples.is_finite() && samples > 0.0 { samples as usize } else { 0 }
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        let e = &self.dsp;
        out[0] = e.rt_low_out;
        out[1] = e.rt_mid_out;
        out[2] = e.rt_high_out;
        out[3] = e.volume_out;
        out[4] = e.flutter_out;
        out[5] = gain_to_db(e.wet_out);
        out[6] = e.taps_a_out as f32;
        out[7] = e.taps_b_out as f32;
    }
}

#[cfg(test)]
mod tests;
