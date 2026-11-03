//! `fs_levelup`: the owner's LevelUp (a LUFS gain-matching leveler for honest A/B, with IN/OUT
//! pairing, four reference tracks, BS.1770-4 delivery metering, platform preview and dither), a
//! faithful translation of `LevelUp/Source/LevelUpDSP.h` ([`level_up_dsp`]) and
//! `LevelUpLink.{h,cpp}` ([`level_up_link`]), with the processor's reference loading in
//! [`reference`].
//!
//! Wrapper (`PluginProcessor.{h,cpp}`) audit, everything that affects the sound:
//! - `prepareToPlay` calls `dsp.prepare (sampleRate)` and raises `forcePush`, so the first
//!   `processBlock` pushes every parameter (in parameter order) before processing. On a rate change
//!   it decodes the references again at the new rate: here the host remakes module assets on a
//!   rate change (B4), and a reference at another rate than the plugin's is not played meanwhile.
//! - `processBlock` pushes only what moved (`v != lastPushed[i]`), in parameter order. That
//!   matters: Normalize and Learn fire on their rising edge in the engine's setter. Here
//!   `set_param` only stores the value; `process` does the wrapper's push.
//! - The host position: `hostPos` is the play head's time in samples while the host is playing,
//!   else -1 (the reference then runs on from where it was). Here [`Plugin::set_transport`] takes
//!   `Transport::samples` when `playing`, else -1.
//! - The reference: the slot is `jlimit (0, 3, roundToInt (refSlot))`; the C++ reads it under a
//!   try-lock (one block of the mix while the loader swaps one in). Here the host hands the
//!   references over as assets (`ref0`..`ref3`, [`crate::REFERENCE_KEYS`]) made off the audio
//!   thread ([`reference::ReferenceAudio`], `mix.module_reference`), and the plugin only swaps
//!   them in; one it lets go of goes back to the host through `take_released_asset` (Amp's
//!   pattern), never dropped here. An asset that is not a LevelUp reference, or at another rate
//!   than the plugin's, plays as none.
//! - The pairing timer (10 Hz, message thread): Learn's result goes back onto the Target knob
//!   (`setParamValue (pTarget, learned)`), then the instance publishes `(role, group,
//!   shortTermIn)` and an OUT that finds a live IN of its pair follows its loudness
//!   (`setExternalTarget`). Here, per block: the OUT follows before the engine runs, the instance
//!   publishes after it, on the host's link group slab (see [`level_up_link`]; the host renders
//!   the IN before the OUT). Learn: the plugin puts the learned value on its own Target (the next
//!   push sends it to the engine, as the C++'s next push does) and shows it in `readouts[6]` until
//!   the host sets Target, so the UI can write it back to the session.
//! - Role (`link`) is the link role the host orders by ([`crate::link::LinkSpec`]): IN produces,
//!   OUT consumes, Off takes no part. Like every link role parameter it is not offered for
//!   automation. Pair (`linkGroup`) stays the C++'s pair within the group (A-D).
//! - Latency: none. Tail: `getTailLengthSeconds` is 0; the engine keeps no audio of its input
//!   (its filters only feed the meters), so `tail_samples` is 0.
//! - Bypass: the engine's own `bypass` parameter returns before touching the audio (the meters
//!   stop), translated in the engine. It is kept in order but hidden (group ""): the UI uses the
//!   module's bypass.
//! - APVTS ranges: floats are 0..1, choices are whole indices, bools 0/1. `ParamInfo::clamp` does
//!   the same (NaN becomes the default), so the engine never sees a non-finite value.
//! - Bus layout: mono or stereo, in = out; with one channel the engine reads the left as both
//!   sides and writes only the left. Extra channels are left untouched (the Rust host contract).
//! - `ScopedNoDenormals`: no safe Rust equivalent; `silence_tail_is_cheap` guards the CPU side.
//! - Presets, state, the dirty flag and the editor are UI-only. The preset system leaves Bypass,
//!   Pair, Reference, Ref Slot, Preview, Normalize and Learn alone (`getPresetParams`): the UI's
//!   preset apply does the same ([`PRESET_SKIPS`]).
//! - Reset (the DAW's, on every Play, Stop and Locate): the C++ `prepareToPlay` on the live
//!   instance (`dsp.prepare` at the same rate, then the forced push), with the controller's
//!   rulings for a DAW (`LevelUpDsp::reset_live`): the leveler's ride and its short-term window
//!   are kept (no level step, no 3 s pump after a Locate); the reference keeps its gain and the
//!   short-term loudness it is matched to; Preview holds its gain until the meter has measured
//!   again; **the integrated loudness and the true-peak hold restart** (`resetDeliveryNow`, as the
//!   C++ `prepare`). No allocation: the engine is plain data in its box.
//! - `resetDelivery` (the Deliver tab's Reset): [`Plugin::action`] `"reset_delivery"`, reached by
//!   the `mix.module_action` command; it raises the engine's request flag, which the next block
//!   acts on, as the C++ does.
//!
//! Station: LevelUp is not one of the Engine family (it keeps its Amorph look, one cyan accent,
//! 0xFF3DD6FF); no Station entry, so the key knobs are the editor's Level row (Target, Attack,
//! Release, Gate).

pub mod level_up_dsp;
pub mod level_up_link;
mod presets;
pub mod reference;

pub use level_up_dsp::{LevelUpDsp, NUM_PARAMS};
pub use presets::PRESETS;
pub use reference::{ReferenceAudio, as_reference};

use crate::analysis::Analysis;
use crate::link::{LinkRole, LinkSlab, LinkSpec};
use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, ParamInfo, Plugin, PluginInfo, REFERENCE_KEYS, SampleSource, Transport};
use level_up_dsp as d;
use level_up_link::LevelUpLink;
use std::sync::Arc;

/// `formatParam` for the five knobs, through the engine's own maps.
fn attack_text(v: f32) -> String {
    format!("{:.2} s", d::knob_to_attack(v))
}
fn release_text(v: f32) -> String {
    format!("{:.2} s", d::knob_to_release(v))
}
fn gate_text(v: f32) -> String {
    format!("-{} dB", d::knob_to_gate(v) as i32)
}
fn max_gain_text(v: f32) -> String {
    format!("{:.1} dB", d::knob_to_max_gain(v))
}
fn target_text(v: f32) -> String {
    format!("{} LUFS", d::knob_to_target(v) as i32)
}

/// The editor's three tabs.
pub const LEVEL: &str = "Level";
pub const REFERENCE: &str = "Reference";
pub const DELIVER: &str = "Deliver";

pub const MODES: [&str; 2] = ["Auto", "Static"];
pub const ROLES: [&str; 3] = ["Off", "IN", "OUT"];
pub const PAIRS: [&str; 4] = ["A", "B", "C", "D"];
pub const REF_SLOTS: [&str; 4] = ["1", "2", "3", "4"];
pub const PLATFORM_NAMES: [&str; 7] = ["None", "Spotify", "Apple Music", "YouTube", "TIDAL", "Amazon Music", "Deezer"];
pub const DITHERS: [&str; 3] = ["Off", "24-bit", "16-bit"];

/// Parameters in the C++ order (`Parameters.h`), ids, names and defaults verbatim.
pub static LEVELUP_PARAMS: [ParamInfo; NUM_PARAMS] = [
    knob01("attack", "Attack", 0.18367, LEVEL, attack_text),
    knob01("release", "Release", 0.17949, LEVEL, release_text),
    knob01("gate", "Gate", 0.24324, LEVEL, gate_text),
    knob01("maxGain", "Max Gain", 0.5, LEVEL, max_gain_text),
    knob01("target", "Target", 0.72727, LEVEL, target_text),
    fs_choice("mode", "Mode", 0.0, LEVEL, &MODES),
    fs_toggle("normalize", "Normalize", 0.0, LEVEL),
    fs_toggle("freeze", "Freeze", 0.0, LEVEL),
    fs_toggle("ceiling", "Ceiling", 1.0, LEVEL),
    fs_toggle("learn", "Learn", 0.0, LEVEL),
    fs_choice("link", "Role", 0.0, LEVEL, &ROLES),
    fs_choice("linkGroup", "Pair", 0.0, LEVEL, &PAIRS),
    fs_toggle("reference", "Reference", 0.0, REFERENCE),
    fs_choice("refSlot", "Ref Slot", 0.0, REFERENCE, &REF_SLOTS),
    fs_choice("platform", "Platform", 0.0, DELIVER, &PLATFORM_NAMES),
    fs_toggle("preview", "Preview", 0.0, DELIVER),
    fs_choice("dither", "Dither", 0.0, DELIVER, &DITHERS),
    fs_toggle("shaping", "Shaped", 0.0, DELIVER),
    // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
    fs_toggle("bypass", "Bypass", 0.0, ""),
];

/// The parameters a LevelUp preset leaves alone (`getPresetParams`): this session's business,
/// not the setup's.
pub const PRESET_SKIPS: [&str; 7] = ["bypass", "linkGroup", "reference", "refSlot", "preview", "normalize", "learn"];

/// The actions the plugin takes ([`Plugin::action`]): the Deliver tab's Reset.
pub const ACTIONS: &[&str] = &["reset_delivery"];

pub static LEVELUP_INFO: PluginInfo = PluginInfo {
    id: "fs_levelup",
    name: "LevelUp",
    short_name: "LevelUp",
    category: Category::FireStation,
    params: &LEVELUP_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: Some(LinkSpec {
        enable_param: None,
        role_param: Some("link"),
        roles: &[None, Some(LinkRole::Producer), Some(LinkRole::Consumer)],
        layout: level_up_link::LAYOUT,
        words: level_up_link::WORDS,
    }),
    references: 4,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &LEVELUP_INFO,
    make: || Box::new(LevelUp::new()),
    presets: PRESETS,
    order: 180,
    fidelity: Some(super::Fidelity { name: "levelup", source_dir: "LevelUp", render_tool: "LevelUp", tables: &[] }),
    prepare_state: None,
    actions: ACTIONS,
};

/// [`Analysis::live`] index: the gain Preview plays the master at, dB (`previewGainOut`, the
/// Deliver tab's "Previewing ..." line; the eight readouts are taken).
pub const LIVE_PREVIEW_GAIN: usize = 0;
pub const LIVE_LEN: usize = 1;

/// The LevelUp port.
///
/// `readouts`: `[0]` short-term loudness in, LUFS (`shortTermIn`, -100 before a reading), `[1]`
/// the gain correction, dB (`gainOut`), `[2]` short-term loudness out (`shortTermOut`), `[3]`
/// integrated loudness out (`integratedOut`, -100 before a reading), `[4]` true peak out since
/// the reset, dBTP (`truePeakOut`), `[5]` the gain the reference plays at, dB (`refGainOut`), NaN
/// while no reference plays, `[6]` the Target knob value Learn just took (`learnedKnob`), NaN once
/// the host has set Target since (nothing to write back), `[7]` the loudness of the IN an OUT is
/// following (the processor's `inLufs`), NaN while it follows none.
pub struct LevelUp {
    p: Params,
    dsp: Box<LevelUpDsp>,
    /// The wrapper's `lastPushed` and `forcePush`.
    last_pushed: [f32; NUM_PARAMS],
    force_push: bool,
    channels: usize,
    /// `hostPos` for the next block.
    host_pos: i64,
    refs: [Option<Arc<dyn SampleSource>>; 4],
    /// References let go of, for the host to drop off the audio thread.
    released: [Option<Arc<dyn SampleSource>>; 8],
    link: LevelUpLink,
    /// The IN's loudness this OUT follows (`inLufs` while `followingIn`).
    following: Option<f32>,
    /// Learn's value, until the host sets Target.
    learned: Option<f32>,
    ref_playing: bool,
    /// The editor's value beyond the readouts ([`LIVE_PREVIEW_GAIN`]).
    analysis: Arc<Analysis>,
}

impl Default for LevelUp {
    fn default() -> Self {
        Self::new()
    }
}

impl LevelUp {
    pub fn new() -> Self {
        LevelUp {
            p: Params::new(&LEVELUP_PARAMS),
            dsp: Box::new(LevelUpDsp::new()),
            last_pushed: [0.0; NUM_PARAMS],
            force_push: true,
            channels: 2,
            host_pos: -1,
            refs: Default::default(),
            released: Default::default(),
            link: LevelUpLink::default(),
            following: None,
            learned: None,
            ref_playing: false,
            analysis: Arc::new(Analysis::with_live(LIVE_LEN)),
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

    fn keep_released(&mut self, a: Arc<dyn SampleSource>) {
        if let Some(slot) = self.released.iter_mut().find(|s| s.is_none()) {
            *slot = Some(a);
        }
        // (all full means the host never collects them; it keeps its own reference to every asset
        // it hands over, so this is never the last one)
    }

    /// `roundToInt` of a choice parameter.
    fn index(&self, i: usize) -> i32 {
        let v = self.p.v(i);
        if v.is_finite() { v.round() as i32 } else { 0 }
    }

    /// The engine (tests).
    #[cfg(test)]
    pub(crate) fn engine(&self) -> &LevelUpDsp {
        &self.dsp
    }
}

impl Plugin for LevelUp {
    fn info(&self) -> &'static PluginInfo {
        &LEVELUP_INFO
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(i) = self.p.set(id, value) else { return false };
        if i == d::P_TARGET {
            // the host has its Target now: nothing of Learn's left to write back
            self.learned = None;
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        // `prepareToPlay` on a fresh engine (plain data, rebuilt in its box), then the forced push.
        self.channels = channels.min(2);
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        *self.dsp = LevelUpDsp::new();
        self.dsp.prepare(sr);
        self.force_push = true;
        self.following = None;
        self.ref_playing = false;
    }

    fn reset(&mut self) {
        // `prepareToPlay` on the live instance, with the DAW rulings (see the module docs).
        self.dsp.reset_live();
        self.force_push = true;
        self.ref_playing = false;
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.host_pos = if transport.playing { transport.samples } else { -1 };
    }

    fn set_asset(&mut self, key: &str, asset: Option<Arc<dyn SampleSource>>) -> bool {
        let Some(i) = REFERENCE_KEYS.iter().position(|k| *k == key) else { return false };
        if let Some(old) = std::mem::replace(&mut self.refs[i], asset) {
            self.keep_released(old);
        }
        true
    }

    fn take_released_asset(&mut self) -> Option<Arc<dyn SampleSource>> {
        self.released.iter_mut().find_map(Option::take)
    }

    fn set_link(&mut self, link: Option<Arc<LinkSlab>>, member: usize) {
        self.link.set(link, member);
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.push_params();

        // The timer's follow: an OUT aims at a live IN of its pair.
        let role = self.index(d::P_LINK);
        let group = self.index(d::P_LINK_GROUP);
        self.following = if role == d::LINK_OUT { self.link.find_in(group) } else { None };
        self.dsp.set_external_target(self.following.is_some(), self.following.unwrap_or(-100.0));

        let nc = self.channels.min(io.len());
        if nc > 0 {
            let slot = self.index(d::P_REF_SLOT).clamp(0, 3) as usize;
            let sr = self.dsp.sample_rate();
            let view =
                self.refs[slot].as_deref().filter(|a| (f64::from(a.sample_rate()) - sr).abs() < 0.5).and_then(as_reference).map(ReferenceAudio::view);
            self.ref_playing = !self.p.on(d::P_BYPASS) && self.p.on(d::P_REFERENCE) && view.is_some_and(|v| v.valid());
            self.dsp.process(io, nc, frames, view, self.host_pos);
        }

        // Learn's result onto the Target knob, then the timer's publish.
        if self.dsp.learned_knob >= 0.0 {
            let k = std::mem::replace(&mut self.dsp.learned_knob, -1.0);
            self.p.set("target", k);
            self.learned = Some(self.p.v(d::P_TARGET));
        }
        self.link.publish(role, group, self.dsp.short_term_in);
        self.analysis.live.set(LIVE_PREVIEW_GAIN, self.dsp.preview_gain_out);
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(self.analysis.clone())
    }

    fn action(&mut self, name: &str) -> bool {
        match name {
            // `resetDelivery`: a flag the next block acts on (no allocation)
            "reset_delivery" => {
                self.dsp.reset_delivery();
                true
            }
            _ => false,
        }
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        out[0] = self.dsp.short_term_in;
        out[1] = self.dsp.gain_out;
        out[2] = self.dsp.short_term_out;
        out[3] = self.dsp.integrated_out;
        out[4] = self.dsp.true_peak_out;
        out[5] = if self.ref_playing { self.dsp.ref_gain_out } else { f32::NAN };
        out[6] = self.learned.unwrap_or(f32::NAN);
        out[7] = self.following.unwrap_or(f32::NAN);
    }
}

#[cfg(test)]
mod tests;
