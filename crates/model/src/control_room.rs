//! The control room (Window › Transport & Session › Control Room): what the engineer hears, apart from what the mix
//! is. Monitor sets (the speakers the main mix, or the listen bus, plays on, with level, dim,
//! mono and mute), cue mixes (hardware outputs fed by sends, with a level each) and a talkback
//! input routed to the cues. None of it reaches a bounce, the main meters or the loudness meter:
//! the engine applies it after them, and only when it plays live.
//!
//! Absent from a session (`Session::control_room` is `None`) everything is as before: the main
//! mix goes to the main output at unity.

/// Most monitor sets and cue mixes a control room keeps.
pub const MAX_MONITOR_SETS: usize = 4;
pub const MAX_CUES: usize = 4;

/// A pair (or more) of speakers the monitor path can play on.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MonitorSet {
    pub name: String,
    /// The hardware output (an `OutputPath` name) it plays on; empty: the main output.
    #[serde(default)]
    pub output: String,
    /// Its level (the control room's big knob), dB.
    #[serde(default)]
    pub level_db: f32,
    /// A fixed calibration trim for these speakers, dB.
    #[serde(default)]
    pub trim_db: f32,
}

/// A headphone mix: a hardware output the tracks' cue sends feed.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CueMix {
    pub name: String,
    /// The hardware output (an `OutputPath` name) the cue plays on.
    pub output: String,
    #[serde(default)]
    pub level_db: f32,
    #[serde(default)]
    pub mute: bool,
    /// The talkback microphone is heard in this cue while talkback is on.
    #[serde(default = "yes")]
    pub talkback: bool,
}

/// The talkback microphone.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Talkback {
    /// The hardware input (an `InputPath` name, or a legacy "In N") it is read from; empty: none.
    #[serde(default)]
    pub input: String,
    #[serde(default)]
    pub level_db: f32,
    /// Talkback dims the monitor speakers while it is on (so the microphone does not feed back).
    #[serde(default = "yes")]
    pub dims: bool,
    /// Talkback is on now (held, or latched). Never saved: a session opens with it off.
    #[serde(skip)]
    pub on: bool,
}

impl Default for Talkback {
    fn default() -> Self {
        Talkback { input: String::new(), level_db: 0.0, dims: true, on: false }
    }
}

/// The control room of a session.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ControlRoom {
    /// At least one set (`ControlRoom::default` has the main output).
    pub sets: Vec<MonitorSet>,
    #[serde(default)]
    pub active_set: usize,
    /// The listen bus's level (AFL/PFL solo), dB, on top of the set's.
    #[serde(default)]
    pub listen_db: f32,
    #[serde(default)]
    pub cues: Vec<CueMix>,
    #[serde(default)]
    pub talkback: Talkback,
    /// How far Dim turns the monitor down, dB (negative).
    #[serde(default = "default_dim")]
    pub dim_db: f32,
    #[serde(default)]
    pub dim: bool,
    #[serde(default)]
    pub mono: bool,
    #[serde(default)]
    pub mute: bool,
}

fn yes() -> bool {
    true
}

fn default_dim() -> f32 {
    -20.0
}

impl Default for ControlRoom {
    fn default() -> Self {
        ControlRoom {
            sets: vec![MonitorSet { name: "Main".into(), output: String::new(), level_db: 0.0, trim_db: 0.0 }],
            active_set: 0,
            listen_db: 0.0,
            cues: Vec::new(),
            talkback: Talkback::default(),
            dim_db: default_dim(),
            dim: false,
            mono: false,
            mute: false,
        }
    }
}

impl ControlRoom {
    /// The set the monitor path plays on.
    pub fn set(&self) -> Option<&MonitorSet> {
        self.sets.get(self.active_set).or(self.sets.first())
    }

    /// The monitor path's gain in dB before mute: the set's level and trim, and the dim (by Dim,
    /// or by talkback while it is on and dims).
    pub fn monitor_db(&self) -> f32 {
        let set = self.set().map_or(0.0, |s| s.level_db + s.trim_db);
        let dimmed = self.dim || (self.talkback.on && self.talkback.dims);
        set + if dimmed { self.dim_db } else { 0.0 }
    }
}
