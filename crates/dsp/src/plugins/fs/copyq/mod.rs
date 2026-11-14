//! `fs_copyq`: CopyQ, a match equaliser. Profile any sound, then make another sound's tone meet
//! it. Rust-native: like Manners it has no C++ original, so there is no fidelity test; the engine
//! is [`copyq_dsp`], a profile is [`profile::Profile`], measuring is [`measure`].
//!
//! How it is used:
//! - **Capture.** Put it on the sound to copy, switch Capture on and play: every frame that is
//!   not silence or bleed is averaged, the audio passes untouched, and the editor saves what was
//!   measured as a named profile (the live values carry all fifteen curves, [`LIVE_MEASURED`]).
//!   A file or a clip is measured without a module at all ([`measure::measure_audio`]).
//! - **Match.** On the sound to change, the module's state ([`profile::CopyQState`]) names up to
//!   four profiles and a weight for each; the host makes that ready off the audio thread
//!   ([`profile::prepare_state`]) and hands it over between blocks ([`Plugin::set_prepared_state`]).
//!   The module measures its own input as it plays (Follow) and applies the difference; with
//!   Follow off it holds what it has, or uses the source frozen into its state.
//! - **Link.** In a link group, Send publishes its measurement and passes its audio untouched,
//!   Match takes the group's target instead of its state's, and Group does both: every Group
//!   member comes to the group's average, or to the Send members when there are any ([`link`]).
//!
//! Wrapper notes:
//! - Parameters are in their own units (percent, dB, Hz, seconds).
//! - Latency: the STFT frame in linear phase (4096 samples at 44.1/48 kHz, 8192 up to 96 kHz,
//!   16384 above), none in minimum phase; the host re-aligns when Phase changes.
//! - Mono runs the same engine on one channel; mid/side and left/right then mean one curve.
//!   Extra channels are left untouched (the host contract).
//! - `reset` (every Play, Stop and Locate) drops the audio in flight and nothing else: the
//!   measurement and the curve are kept. The `reset_measure` action forgets the measurement.
//! - `bypass` is the engine's own, hidden (group "") like every engine's.
//! - A preset never sets Capture, Link or Bypass ([`PRESET_SKIPS`]).

pub mod bells;
pub mod copyq_dsp;
pub mod link;
pub mod measure;
mod presets;
pub mod profile;
#[cfg(test)]
mod tests;

pub use copyq_dsp::{BANDS, ChannelMode, CopyQDsp, Phase, Role, Settings};
pub use presets::PRESETS;
pub use profile::{CURVES, CopyQState, GRID_N, Profile, Shape};

use crate::analysis::Analysis;
use crate::link::{LinkRole, LinkSlab, LinkSpec};
use crate::params::Params;
use crate::{Category, ParamInfo, Plugin, PluginInfo, Taper, Unit};
use link::CopyQLink;
use std::sync::Arc;

const fn knob(id: &'static str, name: &'static str, min: f32, max: f32, default: f32, unit: Unit, taper: Taper, group: &'static str) -> ParamInfo {
    ParamInfo { id, name, min, max, default, unit, taper, choices: &[], display: None, group }
}
const fn switch(id: &'static str, name: &'static str, default: f32, group: &'static str) -> ParamInfo {
    knob(id, name, 0.0, 1.0, default, Unit::Toggle, Taper::Linear, group)
}
const fn pick(id: &'static str, name: &'static str, choices: &'static [&'static str], group: &'static str) -> ParamInfo {
    ParamInfo { id, name, min: 0.0, max: (choices.len() - 1) as f32, default: 0.0, unit: Unit::Choice, taper: Taper::Linear, choices, display: None, group }
}

pub const CHANNEL_CHOICES: [&str; 3] = ["Stereo", "Mid/Side", "Left/Right"];
pub const PHASE_CHOICES: [&str; 2] = ["Linear Phase", "Minimum Phase"];
pub const LINK_CHOICES: [&str; 4] = ["Off", "Send", "Match", "Group"];

pub const NUM_PARAMS: usize = 24;
const P_AMOUNT: usize = 0;
const P_DETAIL: usize = 1;
const P_LIMIT: usize = 2;
const P_DYNAMICS: usize = 3;
const P_FOLLOW: usize = 4;
const P_WINDOW: usize = 5;
const P_GATE: usize = 6;
const P_CAPTURE: usize = 7;
const P_CHANNELS: usize = 8;
const P_PHASE: usize = 9;
const P_AUTO: usize = 10;
const P_OUTPUT: usize = 11;
const P_LO: usize = 12;
const P_HI: usize = 13;
const P_SPLIT: usize = 14;
const P_RANGE_AMOUNT: usize = 17;
const P_LINK: usize = 21;
const P_DELTA: usize = 22;
const P_BYPASS: usize = 23;

pub static COPYQ_PARAMS: [ParamInfo; NUM_PARAMS] = [
    knob("amount", "Amount", -100.0, 200.0, 100.0, Unit::Percent, Taper::Linear, "Match"),
    knob("detail", "Detail", 0.0, 100.0, 40.0, Unit::Percent, Taper::Linear, "Match"),
    knob("limit", "Max Gain", 1.0, 24.0, 12.0, Unit::Db, Taper::Linear, "Match"),
    knob("dynamics", "Dynamics", 0.0, 100.0, 0.0, Unit::Percent, Taper::Linear, "Match"),
    switch("follow", "Follow", 1.0, "Source"),
    knob("window", "Window", 1.0, 60.0, 10.0, Unit::Seconds, Taper::Log, "Source"),
    knob("gate", "Gate", 6.0, 60.0, 24.0, Unit::Db, Taper::Linear, "Source"),
    switch("capture", "Capture", 0.0, "Source"),
    pick("channels", "Channels", &CHANNEL_CHOICES, "Output"),
    pick("phase", "Phase", &PHASE_CHOICES, "Output"),
    switch("autoGain", "Match Level", 1.0, "Output"),
    knob("output", "Output", -24.0, 24.0, 0.0, Unit::Db, Taper::Linear, "Output"),
    knob("rangeLo", "Range Low", 20.0, 20_000.0, 20.0, Unit::Hz, Taper::Log, "Range"),
    knob("rangeHi", "Range High", 20.0, 20_000.0, 20_000.0, Unit::Hz, Taper::Log, "Range"),
    knob("split1", "Low Split", 20.0, 20_000.0, 150.0, Unit::Hz, Taper::Log, "Ranges"),
    knob("split2", "Mid Split", 20.0, 20_000.0, 1_000.0, Unit::Hz, Taper::Log, "Ranges"),
    knob("split3", "High Split", 20.0, 20_000.0, 6_000.0, Unit::Hz, Taper::Log, "Ranges"),
    knob("amtLow", "Low Amount", 0.0, 200.0, 100.0, Unit::Percent, Taper::Linear, "Ranges"),
    knob("amtLowMid", "Low Mid Amount", 0.0, 200.0, 100.0, Unit::Percent, Taper::Linear, "Ranges"),
    knob("amtHighMid", "High Mid Amount", 0.0, 200.0, 100.0, Unit::Percent, Taper::Linear, "Ranges"),
    knob("amtHigh", "High Amount", 0.0, 200.0, 100.0, Unit::Percent, Taper::Linear, "Ranges"),
    pick("link", "Link", &LINK_CHOICES, "Link"),
    switch("delta", "Delta", 0.0, "Listen"),
    // The engine's own bypass, hidden; the UI uses the module's bypass instead.
    switch("bypass", "Bypass", 0.0, ""),
];

/// Parameters a preset leaves alone.
pub const PRESET_SKIPS: [&str; 3] = ["bypass", "capture", "link"];

/// The knobs on the module tile.
pub const KEY_KNOBS: [&str; 3] = ["amount", "detail", "limit"];

/// The actions CopyQ takes ([`Plugin::action`]).
pub const ACTIONS: &[&str] = &["reset_measure"];

pub static COPYQ_INFO: PluginInfo = PluginInfo {
    id: "fs_copyq",
    name: "CopyQ",
    short_name: "CopyQ",
    category: Category::FireStation,
    params: &COPYQ_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: Some(LinkSpec {
        enable_param: None,
        role_param: Some("link"),
        roles: &[None, Some(LinkRole::Producer), Some(LinkRole::Consumer), Some(LinkRole::Peer)],
        layout: link::LAYOUT,
        words: link::WORDS,
    }),
    references: 0,
    midi_out: false,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &COPYQ_INFO,
    make: || Box::new(CopyQ::new()),
    presets: PRESETS,
    order: 55,
    fidelity: None,
    prepare_state: Some(profile::prepare_state),
    actions: ACTIONS,
};

/// [`Analysis::live`], as of the last frame. Scalars first:
pub const LIVE_REV: usize = 0; // counts up with every frame published
pub const LIVE_SECONDS: usize = 1; // ..4: seconds measured (all, loud, quiet)
pub const LIVE_LEVEL: usize = 4; // ..7: the measurement's mean level per level, dB
pub const LIVE_STEREO: usize = 7; // 1: measured from two channels
pub const LIVE_MATCHING: usize = 8; // 1: a curve is being applied
pub const LIVE_SPLIT: usize = 9; // 1: the two curves differ (mid/side or left/right in force)
pub const LIVE_COMP: usize = 10; // the level given back, dB
pub const LIVE_POSITION: usize = 11; // where the level sits between quiet (0) and loud (1)
pub const LIVE_SENDERS: usize = 12; // Send members publishing in the link group
pub const LIVE_GROUP: usize = 13; // Group members publishing
pub const LIVE_LINKED: usize = 14; // 1: the target comes from the link group
pub const LIVE_HAS_TARGET: usize = 15; // 1: the state names a target
pub const LIVE_HAS_SOURCE: usize = 16; // 1: the state holds a frozen source
pub const LIVE_SCALARS: usize = 20;
/// Then the measurement, every curve in dB ([`profile::curve_index`] order, [`GRID_N`] each):
pub const LIVE_MEASURED: usize = LIVE_SCALARS;
/// then, for curve A and curve B in turn, the source, the target (at the source's level) and the
/// response applied, [`GRID_N`] each.
pub const LIVE_SHOWN: usize = LIVE_MEASURED + CURVES * GRID_N;
pub const LIVE_LEN: usize = LIVE_SHOWN + 6 * GRID_N;

/// Where curve `ab`'s shown `part` (0 source, 1 target, 2 applied) starts in the live values.
pub const fn live_shown(ab: usize, part: usize) -> usize {
    LIVE_SHOWN + (ab * 3 + part) * GRID_N
}

/// CopyQ.
///
/// `readouts` (as of the last frame): `[0]` the largest cut, dB; `[1]` the largest lift, dB;
/// `[2]` the level given back, dB; `[3]` seconds measured. `gain_reduction_db` is `[0]`.
/// `analysis()`: `pre` the mono input, `post` the mono output, and the live values above.
pub struct CopyQ {
    p: Params,
    dsp: Box<CopyQDsp>,
    analysis: Arc<Analysis>,
    channels: usize,
    link: CopyQLink,
    /// The link group's target as last read.
    link_shape: Box<Shape>,
    link_counts: (u32, u32),
    /// The stored state as last loaded ([`Plugin::load_state`]).
    state: Vec<u8>,
    rev: u32,
}

impl Default for CopyQ {
    fn default() -> Self {
        Self::new()
    }
}

impl CopyQ {
    pub fn new() -> Self {
        let mut s = CopyQ {
            p: Params::new(&COPYQ_PARAMS),
            dsp: Box::new(CopyQDsp::new()),
            analysis: Arc::new(Analysis::with_live(LIVE_LEN)),
            channels: 2,
            link: CopyQLink::default(),
            link_shape: Box::default(),
            link_counts: (0, 0),
            state: CopyQState::default().write(),
            rev: 0,
        };
        s.update();
        s
    }

    /// The engine (tests and tools).
    pub fn dsp(&self) -> &CopyQDsp {
        &self.dsp
    }

    /// Hands the knobs to the engine.
    fn update(&mut self) {
        let p = &self.p;
        self.dsp.set(Settings {
            amount: p.v(P_AMOUNT) / 100.0,
            detail: p.v(P_DETAIL) / 100.0,
            limit_db: p.v(P_LIMIT),
            dynamics: p.v(P_DYNAMICS) / 100.0,
            follow: p.on(P_FOLLOW),
            window_s: p.v(P_WINDOW),
            gate_db: p.v(P_GATE),
            channels: match p.choice(P_CHANNELS) {
                1 => ChannelMode::MidSide,
                2 => ChannelMode::LeftRight,
                _ => ChannelMode::Stereo,
            },
            phase: if p.choice(P_PHASE) == 1 { Phase::Minimum } else { Phase::Linear },
            auto_gain: p.on(P_AUTO),
            lo_hz: p.v(P_LO),
            hi_hz: p.v(P_HI),
            split_hz: [p.v(P_SPLIT), p.v(P_SPLIT + 1), p.v(P_SPLIT + 2)],
            range_amount: [p.v(P_RANGE_AMOUNT) / 100.0, p.v(P_RANGE_AMOUNT + 1) / 100.0, p.v(P_RANGE_AMOUNT + 2) / 100.0, p.v(P_RANGE_AMOUNT + 3) / 100.0],
            capture: p.on(P_CAPTURE),
            role: match p.choice(P_LINK) {
                1 => Role::Send,
                2 => Role::Match,
                3 => Role::Group,
                _ => Role::Off,
            },
            delta: p.on(P_DELTA),
            output_db: p.v(P_OUTPUT),
            bypass: p.on(P_BYPASS),
        });
    }

    /// The link group, both ways: this module's measurement out, the group's target in.
    fn sync_link(&mut self) {
        let role = self.dsp.settings().role;
        let seconds = self.dsp.measured_seconds().first().copied().unwrap_or(0.0);
        let publish = match role {
            Role::Send => link::ROLE_SEND,
            Role::Group => link::ROLE_GROUP,
            _ => link::ROLE_NONE,
        };
        self.link.publish(publish, self.dsp.own(), seconds);
        if matches!(role, Role::Match | Role::Group) && self.link.linked() {
            self.link_counts = self.link.gather(true, &mut self.link_shape);
            let shape = self.link_shape.present.then_some(&*self.link_shape);
            self.dsp.set_link_target(shape);
        } else {
            self.link_counts = (0, 0);
            self.dsp.set_link_target(None);
        }
    }

    fn publish(&mut self) {
        let live = &self.analysis.live;
        let d = &self.dsp;
        self.rev = self.rev.wrapping_add(1) % 1_000_000;
        let flag = |b: bool| if b { 1.0 } else { 0.0 };
        let own = d.own();
        let seconds = d.measured_seconds();
        for lv in 0..profile::LV {
            live.set(LIVE_SECONDS + lv, seconds.get(lv).copied().unwrap_or(0.0));
            live.set(LIVE_LEVEL + lv, own.level.get(lv).copied().unwrap_or(profile::FLOOR_DB));
        }
        live.set(LIVE_STEREO, flag(own.stereo));
        live.set(LIVE_MATCHING, flag(d.matching()));
        live.set(LIVE_SPLIT, flag(d.split()));
        live.set(LIVE_COMP, d.level_comp_db());
        live.set(LIVE_POSITION, d.dynamics_position());
        live.set(LIVE_SENDERS, self.link_counts.0 as f32);
        live.set(LIVE_GROUP, self.link_counts.1 as f32);
        live.set(LIVE_LINKED, flag(self.link_shape.present && matches!(d.settings().role, Role::Match | Role::Group)));
        let (target, source) = d.state_shapes();
        live.set(LIVE_HAS_TARGET, flag(target.present));
        live.set(LIVE_HAS_SOURCE, flag(source.present));
        let mut at = LIVE_MEASURED;
        for c in own.curves.iter() {
            for v in c {
                live.set(at, if own.present { *v } else { profile::FLOOR_DB });
                at += 1;
            }
        }
        for ab in 0..2 {
            let (s, t, a) = d.shown(ab);
            for (part, c) in [s, t, a].into_iter().enumerate() {
                let base = live_shown(ab, part);
                for (i, v) in c.iter().enumerate() {
                    live.set(base + i, *v);
                }
            }
        }
        live.set(LIVE_REV, self.rev as f32);
    }
}

impl Plugin for CopyQ {
    crate::params::param_plumbing!(COPYQ_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.channels = channels.clamp(1, 2);
        self.dsp.prepare(f64::from(sample_rate), self.channels);
        self.update();
        self.publish();
    }

    fn reset(&mut self) {
        self.dsp.reset();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let nc = self.channels.min(io.len());
        if nc == 0 {
            return;
        }
        let frames = io.iter().take(nc).map(Vec::len).min().unwrap_or(0).min(frames);
        let mono = |io: &[Vec<f32>], i: usize| io.iter().take(nc).filter_map(|c| c.get(i)).sum::<f32>() / nc as f32;
        for i in 0..frames {
            self.analysis.pre.push(mono(io, i));
        }
        self.dsp.process(io, frames);
        for i in 0..frames {
            self.analysis.post.push(mono(io, i));
        }
        if self.dsp.take_fresh() {
            self.sync_link();
            self.publish();
        }
    }

    fn latency(&self) -> usize {
        self.dsp.latency_samples()
    }

    fn gain_reduction_db(&self) -> f32 {
        self.dsp.extremes_db().0
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        let (cut, lift) = self.dsp.extremes_db();
        out[0] = cut;
        out[1] = lift;
        out[2] = self.dsp.level_comp_db();
        out[3] = self.dsp.measured_seconds().first().copied().unwrap_or(0.0);
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(self.analysis.clone())
    }

    fn set_link(&mut self, link: Option<Arc<LinkSlab>>, member: usize) {
        self.link.set(link, member);
        self.link_shape.clear();
        self.link_counts = (0, 0);
        self.dsp.set_link_target(None);
    }

    fn action(&mut self, name: &str) -> bool {
        match name {
            "reset_measure" => {
                self.dsp.reset_measure();
                true
            }
            _ => false,
        }
    }

    fn set_prepared_state(&mut self, data: &[f64]) -> bool {
        self.dsp.set_prepared(data);
        true
    }

    /// The state last read by `load_state` (an empty one before any). As with CleanQ, the host
    /// never reads a live CopyQ's state: the session's `Insert::state` is the source of truth,
    /// edited by commands and handed over prepared.
    fn save_state(&mut self) -> Option<Vec<u8>> {
        Some(self.state.clone())
    }

    /// Reads a [`CopyQState`] and takes its target and source. `false`, keeping the previous
    /// state, for bytes that are not one. Allocates: not for the audio thread.
    fn load_state(&mut self, data: &[u8]) -> bool {
        let Some(st) = CopyQState::read(data) else { return false };
        self.dsp.set_prepared(&st.prepared());
        self.state = st.write();
        true
    }
}
