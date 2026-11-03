//! Library: personas (players) and targets. Port of `Library.h`.
//!
//! The engine reads persona and target data only through [`LibraryData`]; the loader ([`load`])
//! builds it. C++ pointers become indices: `by_seed` indexes `personas`, `targets` indexes
//! `target_store`.

use std::sync::Arc;

pub mod auto_target;
pub mod embedded;
pub(crate) mod json;
pub mod load;

pub use auto_target::auto_target;
pub use load::{K_KEYS, LibraryOverlay, LibrarySource, MAX_FILE_BYTES, embedded_library, factory_player_names, factory_players, load_library};

#[cfg(test)]
mod tests;

pub const K_INSTRUMENTS: usize = 13;
pub const K_PLAYER_SLOTS: usize = 16;
pub const K_TARGET_SLOTS: usize = 32;
pub const K_MAX_SEED: usize = 255;
/// energy x lane x step (2 bars) x field
pub const K_GRID_INTS: usize = 4 * 4 * 32 * 4;
/// The same for native 12-step bars (6/8, 3/4).
pub const K_GRID12_INTS: usize = 4 * 4 * 24 * 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    #[default]
    Keys = 0,
    Bass,
    Drums,
    Guitar,
    Ensemble,
    Melodic,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Driver {
    #[default]
    Generic,
    DrumMap,
    KeyswitchBass,
    PianoPedal,
    Swam,
    ArticulationKeys,
    UjamPhrase,
    AmpleGuitar,
    Fiddle,
    Organ,
    Evolution,
    IndiginusStrings,
    BrassSection,
    Keyswitch,
}

pub const INSTRUMENT_NAMES: [&str; K_INSTRUMENTS] =
    ["Piano", "Synth", "Bass", "Electric Keys", "Harp & Mallets", "Drums", "Guitar", "Strings", "Winds", "Brass", "Ethnic", "Beat", "Folk"];
pub const ROLE_NAMES: [&str; 6] = ["keys", "bass", "drums", "guitar", "ensemble", "melodic"];
/// feel_voicing codes 0..9, in order.
pub const STYLE_NAMES: [&str; 10] =
    ["none", "clay-close", "selah-wide", "deacon-rootless", "mara-drop2", "ensemble", "arp", "guitar-shape", "chank", "nash-sixth"];
pub const K_DRIVERS: usize = 14;
pub const DRIVER_NAMES: [&str; K_DRIVERS] = [
    "generic",
    "drumMap",
    "keyswitchBass",
    "pianoPedal",
    "swam",
    "articulationKeys",
    "ujamPhrase",
    "ampleGuitar",
    "fiddle",
    "organ",
    "evolution",
    "indiginusStrings",
    "brassSection",
    "keyswitch",
];
// evolution articulation names (JSON "articulations" keys), in TargetData::EvArt order.
pub const K_EV_ART_COUNT: usize = 22;
pub const EV_ART_NAMES: [&str; K_EV_ART_COUNT] = [
    "powerchord",
    "octaves",
    "releaseSlideDown",
    "slide16",
    "slide8",
    "slide4",
    "velocitySlide",
    "wsHammer",
    "wsSlideIn",
    "buzzTrill",
    "graceDown",
    "graceUp",
    "harmonics",
    "mute",
    "palmMute",
    "halfPalmMute",
    "snapStaccato",
    "snapSustain",
    "squeal",
    "tapping",
    "misfret",
    "buzzRelease",
];
// evolution performance-effect names (JSON "perfFx" keys); which effect sits on which key is read by the user.
pub const EV_PERF_FX_NAMES: [&str; 8] = ["body", "slap", "scrape", "knock", "fretNoise", "pickNoise", "stringHit", "release"];
// indiginusStrings articulation names, in TargetData::InArt order.
pub const K_IN_ART_COUNT: usize = 14;
pub const IN_ART_NAMES: [&str; K_IN_ART_COUNT] = [
    "hammer",
    "pullOff",
    "trillUp",
    "trillDown",
    "upSlide",
    "downSlide",
    "grace",
    "graceSlideUp",
    "graceSlideDown",
    "mutes",
    "tremolo",
    "bend1",
    "bend2",
    "sustain",
];
// enum FdArt
pub const FD_TREMOLO: usize = 0;
pub const FD_SCOOP: usize = 1;
pub const FD_STACCATO: usize = 2;
pub const FD_GRACE_UP: usize = 3;
pub const FD_GRACE_DOWN: usize = 4;
pub const FD_TRILL_UP: usize = 5;
pub const FD_GRACE_SLIDE_UP: usize = 6;
pub const FD_TRILL_DOWN: usize = 7;
pub const FD_GRACE_SLIDE_DOWN: usize = 8;

// TargetData::EvArt (indices into `ev_art`)
pub const EV_POWERCHORD: usize = 0;
pub const EV_OCTAVES: usize = 1;
pub const EV_RELEASE_SLIDE_DOWN: usize = 2;
pub const EV_SLIDE16: usize = 3;
pub const EV_SLIDE8: usize = 4;
pub const EV_SLIDE4: usize = 5;
pub const EV_VELOCITY_SLIDE: usize = 6;
pub const EV_WS_HAMMER: usize = 7;
pub const EV_WS_SLIDE_IN: usize = 8;
pub const EV_BUZZ_TRILL: usize = 9;
pub const EV_GRACE_DOWN: usize = 10;
pub const EV_GRACE_UP: usize = 11;
pub const EV_HARMONICS: usize = 12;
pub const EV_MUTE: usize = 13;
pub const EV_PALM_MUTE: usize = 14;
pub const EV_HALF_PALM_MUTE: usize = 15;
pub const EV_SNAP_STACCATO: usize = 16;
pub const EV_SNAP_SUSTAIN: usize = 17;
pub const EV_SQUEAL: usize = 18;
pub const EV_TAPPING: usize = 19;
pub const EV_MISFRET: usize = 20;
pub const EV_BUZZ_RELEASE: usize = 21;
pub const EV_ART_COUNT: usize = 22;

// TargetData::InArt (indices into `in_art`)
pub const IN_HAMMER: usize = 0;
pub const IN_PULL_OFF: usize = 1;
pub const IN_TRILL_UP: usize = 2;
pub const IN_TRILL_DOWN: usize = 3;
pub const IN_UP_SLIDE: usize = 4;
pub const IN_DOWN_SLIDE: usize = 5;
pub const IN_GRACE: usize = 6;
pub const IN_GRACE_SLIDE_UP: usize = 7;
pub const IN_GRACE_SLIDE_DOWN: usize = 8;
pub const IN_MUTES: usize = 9;
pub const IN_TREMOLO: usize = 10;
pub const IN_BEND1: usize = 11;
pub const IN_BEND2: usize = 12;
pub const IN_SUSTAIN: usize = 13;
pub const IN_ART_COUNT: usize = 14;

// TargetData::KsIntent
pub const KS_LONG: usize = 0;
pub const KS_SHORT: usize = 1;
pub const KS_PLUCK: usize = 2;
pub const KS_TREMOLO: usize = 3;
pub const KS_LEGATO_UP: usize = 4;
pub const KS_SLIDE_IN: usize = 5;
pub const KS_GHOST: usize = 6;
pub const KS_INTENT_COUNT: usize = 7;
// TargetData::KsMode
pub const KS_TAP: u8 = 0;
pub const KS_HOLD: u8 = 1;
pub const KS_LATCH: u8 = 2;
pub const KS_SAME: u8 = 3;

/// How a player drives an Ample Sound guitar (the player file's optional "ample" block). Amounts 0..1.
#[derive(Clone, Debug, PartialEq)]
pub struct AmpleStyle {
    /// -1 not given (guitarists: auto, others: notes), 0 Strummer, 1 notes, 2 auto
    pub mode: i32,
    /// auto: per Energy, do the player's patterns strum (from the grids)
    pub strum_at: [bool; 4],
    pub open_strings_first: bool,
    /// 0 chord (5-fret positions), 1 melody (3 octaves)
    pub position_mode: i32,
    // notes
    pub palm_mute: f32,
    pub pops: f32,
    pub harmonics: f32,
    pub hammer_on: f32,
    pub slides: f32,
    pub slide_in: f32,
    pub fx: f32,
    // Strummer
    pub muted: f32,
    pub strum_noise: f32,
    pub bass_strings: bool,
    /// notes mode: chord shapes fingered from a capo on this fret (0-7)
    pub capo: i32,
    /// Strummer, Natural: the chord detect sent in this inversion (0 root, 1 3rd, 2 5th lowest)
    pub strum_inversion: i32,
}
impl Default for AmpleStyle {
    fn default() -> Self {
        Self {
            mode: -1,
            strum_at: [true, true, true, true],
            open_strings_first: false,
            position_mode: 0,
            palm_mute: 0.0,
            pops: 0.0,
            harmonics: 0.0,
            hammer_on: 0.0,
            slides: 0.0,
            slide_in: 0.0,
            fx: 0.0,
            muted: 0.0,
            strum_noise: 0.0,
            bass_strings: true,
            capo: 0,
            strum_inversion: 0,
        }
    }
}

/// How a player ornaments on the Indiginus The Fiddle target (the player file's optional "fiddle" block). Amounts 0..1.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FiddleStyle {
    pub scoops: f32,
    pub graces: f32,
    pub grace_slides: f32,
    pub trills: f32,
    pub slides: f32,
    pub double_stops: f32,
    pub shuffle: f32,
    pub chop: f32,
    pub kickoff: f32,
    pub fill_licks: f32,
    pub turnaround: f32,
    pub tag: f32,
    /// 0 nashville 1 georgia 2 longbow
    pub shuffle_style: i32,
}

/// How a player plays MODO BASS 2 in Natural (the optional "bass" block). Amounts 0..1; position 0 low 1 mid 2 high,
/// pluck 0 neck 1 mid 2 bridge.
#[derive(Clone, Debug, PartialEq)]
pub struct BassStyle {
    pub vibrato: f32,
    pub let_ring: f32,
    pub falls: f32,
    pub position: i32,
    pub pluck: i32,
}
impl Default for BassStyle {
    fn default() -> Self {
        Self { vibrato: 0.0, let_ring: 0.0, falls: 0.0, position: 0, pluck: 1 }
    }
}

/// How a player plays MODO DRUM in Natural (the optional "drums" block). Amounts 0..1.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrumsStyle {
    pub cross_stick: f32,
    pub pedal_hat: f32,
    pub flams: f32,
    pub hat_bark: f32,
    pub crash_alternate: f32,
    pub aperture: f32,
    pub china: f32,
    pub ride_switch: f32,
    pub bell_cymbal: f32,
}

/// How a player bows on the Embertone Joshua Bell Violin target (the optional "violin" block). Amounts 0..1.
#[derive(Clone, Debug, PartialEq)]
pub struct ViolinStyle {
    pub portamento: f32,
    pub double_stops: f32,
    pub vibrato_depth: f32,
    pub rebow: f32,
}
impl Default for ViolinStyle {
    fn default() -> Self {
        Self { portamento: 0.0, double_stops: 0.0, vibrato_depth: 0.7, rebow: 0.7 }
    }
}

/// How a player plays the Hammond B-3X target (the optional "organ" block). Amounts 0..1;
/// chorus 0 off, 1-3 V1-V3, 4-6 C1-C3.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OrganStyle {
    pub percussion: f32,
    pub swell: f32,
    pub leslie_fast: f32,
    pub chorus: i32,
    pub build: f32,
    pub leslie_kick: f32,
    pub smear: f32,
    pub lower_hand: f32,
    /// 0 gospel, 1 jazz, 2 rock, 3 pad
    pub registration: i32,
}

/// How a player plays the plucked Kontakt targets (evolution, indiginusStrings; the optional "plucked" block).
/// Amounts 0..1. mode 0 auto, 1 strum, 2 notes; rolls 0 none, 1 forward, 2 backward, 3 alternating, 4 foggy.
#[derive(Clone, Debug, PartialEq)]
pub struct PluckedStyle {
    pub mode: i32,
    pub strum_ms: i32,
    pub up_weight: f32,
    pub up_strings: i32,
    pub chop: f32,
    pub tremolo: f32,
    pub rolls: i32,
    pub clawhammer: f32,
    pub drop_thumb: f32,
    pub hammer: f32,
    pub slides: f32,
    pub graces: f32,
    pub buzz: f32,
    pub palm_mute: f32,
    pub harmonics: f32,
    pub vibrato: f32,
    pub kickoff: f32,
    pub fill_licks: f32,
    pub turnaround: f32,
}
impl Default for PluckedStyle {
    fn default() -> Self {
        Self {
            mode: 0,
            strum_ms: 18,
            up_weight: 0.75,
            up_strings: 3,
            chop: 0.0,
            tremolo: 0.0,
            rolls: 0,
            clawhammer: 0.0,
            drop_thumb: 0.0,
            hammer: 0.0,
            slides: 0.0,
            graces: 0.0,
            buzz: 0.0,
            palm_mute: 0.0,
            harmonics: 0.0,
            vibrato: 0.5,
            kickoff: 0.0,
            fill_licks: 0.0,
            turnaround: 0.0,
        }
    }
}

/// How a player plays Blue Street Brass (brassSection; the optional "brass" block). Amounts 0..1.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrassStyle {
    pub stab: f32,
    pub swell: f32,
    pub voiced: f32,
    pub fall: f32,
    pub legato: f32,
    /// 0 none, 1 octave, 2 thirds
    pub doubling: i32,
}

/// How a player sits in time and phrases (the player file's optional "groove" block; Natural feel only).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Groove {
    /// ms per step of a native 12-step bar (6/8, 3/4), -30..30
    pub timing12: [f32; 12],
    /// ms per 16th of the bar, -30..30: where this player sits (consistent, not random)
    pub timing: [f32; 16],
    // 0..1: anticipations, section lead-ins, phrase variation, air
    pub push: f32,
    pub pickups: f32,
    pub variation: f32,
    pub space: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PersonaData {
    pub name: String,
    pub desc: String,
    pub file: String,
    pub seed: i32,
    // The feel row. Names and types match tables.js intFields / floatFields.
    pub heavy: i32,
    pub artic: i32,
    pub role: i32,
    pub voicing: i32,
    pub swing_steps: i32,
    pub pedal_mode: i32,
    pub fill_count: i32,
    pub rh_low: i32,
    pub rh_high: i32,
    pub rh_anchor: i32,
    pub lh_low: i32,
    pub swing_base: f32,
    pub timing_ms: f32,
    pub roll_ms: f32,
    pub jitter_ms: f32,
    pub vel_var: f32,
    pub arc: f32,
    pub fill_rate: f32,
    pub lane_ms: [f32; 4],
    pub accents: [f32; 16],
    /// accent weight per step of a native 12-step bar; 1 when the file gives none
    pub accents12: [f32; 12],
    pub slap: bool,
    pub ample: AmpleStyle,
    pub fiddle: FiddleStyle,
    pub bass: BassStyle,
    pub drums: DrumsStyle,
    pub violin: ViolinStyle,
    pub organ: OrganStyle,
    pub plucked: PluckedStyle,
    pub brass: BrassStyle,
    pub groove: Groove,
    /// (instrument, slot) as listed in the file; slot 0 = unnumbered
    pub slots: Vec<(i32, i32)>,
    /// lower-case: genre first, then style
    pub tags: Vec<String>,
    /// `[((((e-1)*4+lane)*32+step)*4)+field]`: prob, vel, len16, type
    pub grid: [i32; K_GRID_INTS],
    /// `[((((e-1)*4+lane)*24+bar*12+step)*4)+field]`, same fields; empty unless native12
    pub grids12: [i32; K_GRID12_INTS],
    /// bit 0 = 6/8, bit 1 = 3/4: the meters grids12 serves
    pub meters12: u8,
    /// the file has grids12
    pub native12: bool,
    /// ... and no 16-step grids (the player sits out 4/4 and its kin)
    pub only12: bool,
}
impl Default for PersonaData {
    fn default() -> Self {
        Self {
            name: String::new(),
            desc: String::new(),
            file: String::new(),
            seed: 0,
            heavy: 0,
            artic: 0,
            role: 0,
            voicing: 0,
            swing_steps: 4,
            pedal_mode: 0,
            fill_count: 0,
            rh_low: 0,
            rh_high: 0,
            rh_anchor: 0,
            lh_low: 0,
            swing_base: 50.0,
            timing_ms: 0.0,
            roll_ms: 0.0,
            jitter_ms: 0.0,
            vel_var: 0.0,
            arc: 0.0,
            fill_rate: 0.0,
            lane_ms: [0.0; 4],
            accents: [0.0; 16],
            accents12: [0.0; 12],
            slap: false,
            ample: AmpleStyle::default(),
            fiddle: FiddleStyle::default(),
            bass: BassStyle::default(),
            drums: DrumsStyle::default(),
            violin: ViolinStyle::default(),
            organ: OrganStyle::default(),
            plucked: PluckedStyle::default(),
            brass: BrassStyle::default(),
            groove: Groove::default(),
            slots: Vec::new(),
            tags: Vec::new(),
            grid: [0; K_GRID_INTS],
            grids12: [0; K_GRID12_INTS],
            meters12: 0,
            native12: false,
            only12: false,
        }
    }
}

// Velocity bounds apply only when the file gives them: the shaped velocity mapDrum sees can be
// below 1 or above 127 before it is clamped, and the original map (vel < 45 -> 29) covered that.
#[derive(Clone, Debug, PartialEq)]
pub struct DrumRule {
    pub note: i32,
    pub min_vel: i32,
    pub max_vel: i32,
    /// 0 any, 1 left, 2 right
    pub side: i32,
    pub out: i32,
    pub offset: i32,
}
impl Default for DrumRule {
    fn default() -> Self {
        Self { note: 0, min_vel: -100000, max_vel: 100000, side: 0, out: -1, offset: 0 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SwamInstrument {
    pub name: String,
    pub low: i32,
    pub high: i32,
}
impl Default for SwamInstrument {
    fn default() -> Self {
        Self { name: String::new(), low: 0, high: 127 }
    }
}

/// A solo library's own articulation key (Embertone): `key` -1 = not mapped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArtKey {
    pub key: i32,
    pub instant: bool,
}
impl Default for ArtKey {
    fn default() -> Self {
        Self { key: -1, instant: false }
    }
}

/// A named keyswitch articulation (key, mode `KS_TAP`..`KS_SAME`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KsArt {
    pub key: i32,
    pub mode: u8,
    pub min_len_ms: f32,
}
impl Default for KsArt {
    fn default() -> Self {
        Self { key: -1, mode: KS_TAP, min_len_ms: 0.0 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TargetData {
    pub name: String,
    pub help: String,
    pub file: String,
    pub slot: i32,
    pub driver: Driver,
    pub instruments: [bool; K_INSTRUMENTS],
    pub ks_octave_default: i32,
    // Velocity calibration (Natural): v -> velLo + (velHi - velLo) * (v/127)^velCurve; velLo < 0 = none.
    pub vel_lo: i32,
    pub vel_hi: i32,
    pub vel_curve: f32,
    // drumMap
    pub drum_rules: Vec<DrumRule>,
    // drumMap in Natural feel (-1 = not used): cymbal -> its choke note, hat shank closed / open, extra-open,
    // foot splash, ride bell.
    pub dr_chokes: Vec<(i32, i32)>,
    pub dr_shank_closed: i32,
    pub dr_shank_open: i32,
    pub dr_extra_open: i32,
    pub dr_foot_splash: i32,
    pub dr_ride_bell: i32,
    // ... cross-stick, pedal hat, open-hat bark, splash, aperture CC and its CC-following keys, the two crashes,
    // china, ride 2, the 10" bell cymbal.
    pub dr_cross_stick: i32,
    pub dr_pedal_hat: i32,
    pub dr_open_shank: i32,
    pub dr_splash: i32,
    pub dr_aperture_cc: i32,
    pub dr_cc_shank: i32,
    pub dr_cc_tip: i32,
    pub dr_crashes: [i32; 2],
    pub dr_china: i32,
    pub dr_ride2: i32,
    pub dr_bell_cymbal: i32,
    // keyswitchBass
    pub kb_finger: i32,
    pub kb_pick: i32,
    pub kb_slap: i32,
    pub kb_ghost: i32,
    pub kb_palm_cc: i32,
    pub kb_palm_on: i32,
    pub kb_pitch_shift: i32,
    pub kb_min_note: i32,
    // Natural feel (-1 = not used): hammer-on, stop-on-detach and down-stroke keys, the legato-slide CC, CC9's range.
    pub kb_hammer: i32,
    pub kb_stop: i32,
    pub kb_down: i32,
    pub kb_legato_cc: i32,
    pub kb_mute_lo: i32,
    pub kb_mute_hi: i32,
    // ... vibrato CC, let-ring CC, left-hand position keys (lo..hi = frets 0..), pluck-position CC, bend range (semitones).
    pub kb_vibrato_cc: i32,
    pub kb_let_ring_cc: i32,
    pub kb_pos_lo: i32,
    pub kb_pos_hi: i32,
    pub kb_pos_cc: i32,
    pub kb_pluck_cc: i32,
    pub kb_bend_range: i32,
    // pianoPedal
    pub half_pedal: [i32; 3],
    pub soft_pedal_cc: i32,
    pub soft_pedal_max_energy: i32,
    /// Natural: staccato stabs keep the resonance (-1 = none)
    pub harmonic_pedal_cc: i32,
    // swam
    /// 0 strings, 1 brass, 2 winds
    pub swam_family: i32,
    pub swam_instruments: Vec<SwamInstrument>,
    pub swam_expr_cc: i32,
    pub swam_vib_cc: i32,
    pub swam_ks_offset: i32,
    /// false: SWAM's own keyswitch layout is not sent
    pub swam_keyswitches: bool,
    // A solo library's own articulation keys (Embertone): sustain, staccato, pizzicato (latching),
    // tremolo, harmonic (instant: held for the note). key -1 = not mapped.
    pub swam_arts: [ArtKey; 5],
    pub swam_custom_arts: bool,
    /// dynamics carried by note velocity
    pub swam_velocity_dynamics: bool,
    pub swam_vel_lo: i32,
    pub swam_vel_hi: i32,
    // fiddle (Indiginus The Fiddle)
    pub fd_low: i32,
    pub fd_high: i32,
    pub fd_expr_cc: i32,
    pub fd_split_vib: i32,
    pub fd_split_port: i32,
    pub fd_split_slur: i32,
    /// tremolo, scoop, staccato, graceUp, graceDown, trillUp, graceSlideUp, trillDown, graceSlideDown
    pub fd_art: [i32; 9],
    pub fd_portamento_key: i32,
    /// 0 unconfirmed, 1 same, 2 key, 3 hold
    pub fd_return_mode: i32,
    pub fd_return_key: i32,
    pub fd_bow_seconds: f32,
    // organ (Hammond B-3X): expression / Leslie speed / percussion / chorus-vibrato CCs (-1 = never sent),
    // the fixed note velocity, the inverted drawbar-setting key per Energy, the chorus selector's CC values.
    pub og_expr_cc: i32,
    pub og_leslie_cc: i32,
    pub og_perc_cc: i32,
    pub og_chorus_cc: i32,
    pub og_note_vel: i32,
    pub og_drawbar_keys: [i32; 4],
    // Registration ladders by player style (gospel, jazz, rock, pad): a key per Energy, -1 = use ogDrawbarKeys.
    pub og_registrations: [[i32; 4]; 4],
    // Lower manual (0 = off, 1-16 = its channel) and its own inverted keys by Energy (used from Task 9).
    pub og_lower_channel: i32,
    pub og_lower_keys: [i32; 4],
    pub og_chorus_values: [i32; 7],
    // evolution (Orange Tree Evolution engine)
    /// -1 = absent; momentary keys (held over the note)
    pub ev_art: [i32; EV_ART_COUNT],
    pub ev_low: i32,
    pub ev_high: i32,
    /// -1 padded
    pub ev_string_keys: [i32; 6],
    pub ev_string_count: i32,
    pub ev_drone_key: i32,
    pub ev_drone_note: i32,
    pub ev_perf_fx_base: i32,
    /// names in `EV_PERF_FX_NAMES`
    pub ev_perf_fx: [i32; 8],
    pub ev_vibrato_cc: i32,
    pub ev_legato_st: i32,
    pub ev_slide_range: i32,
    pub ev_bypass_legato_key: i32,
    pub ev_strum_down: i32,
    pub ev_strum_up: i32,
    pub ev_strum_engine: bool,
    pub ev_trem_in: i32,
    pub ev_trem_on: i32,
    pub ev_trem_out: i32,
    pub ev_vel_lo: i32,
    pub ev_vel_hi: i32,
    /// Hollowbody / Strat: pitch-bend scoops (+-2 ST)
    pub ev_bend_blues: bool,
    /// open strings low -> high (-1: from evStringCount); the indiginusStrings view sets it
    pub ev_open: [i32; 6],
    // indiginusStrings (The Banjo / Mandolin / Ukulele / Resonator)
    /// -1 = absent
    pub in_art: [i32; IN_ART_COUNT],
    pub in_low: i32,
    pub in_high: i32,
    /// 0 renegade, 1 banjo
    pub in_layout: i32,
    /// -1 padded
    pub in_never_send: [i32; 4],
    /// -1 padded
    pub in_init_keys: [i32; 4],
    /// 0 unconfirmed, 1 hold, 2 latch, 3 same
    pub in_return_mode: i32,
    pub in_return_key: i32,
    pub in_expr_cc: i32,
    pub in_vibrato_cc: i32,
    pub in_master_key_cc: i32,
    pub in_mode_cc: i32,
    pub in_tremolo_speed_cc: i32,
    pub in_chord_down: i32,
    pub in_chord_up: i32,
    /// chord keys: root-select base
    pub in_root_select: i32,
    /// Major / Minor / 7th keys (-1: none)
    pub in_variation: [i32; 3],
    pub in_slots: bool,
    pub in_drone_note: i32,
    // The indiginusStrings targets also fill the evolution fields above (low / high, velocity 50-110, strings, open
    // strings, drone, vibrato CC, articulation keys mapped onto EvArt when the return mode is confirmed): the
    // plucked engine (Musician ev*) plays both drivers from them (LibraryLoader: indiginusPluckedView).
    pub in_string_count: i32,
    // brassSection (Blue Street Brass)
    pub br_low: i32,
    pub br_high: i32,
    pub br_dyn_cc: i32,
    pub br_note_vel: i32,
    /// {cc, value}, -1 padded
    pub br_setup_ccs: [(i32, i32); 4],
    pub br_legato_off: i32,
    pub br_legato_on: i32,
    pub br_instrument_cc: i32,
    /// index = patch 1..26
    pub br_instrument_value: [i32; 27],
    pub br_patch_ensemble: i32,
    pub br_patch_melodic: i32,
    pub br_patch_low: i32,
    // articulationKeys: key per slot (long, short, pluck, tremolo), -1 = the target has no such slot
    pub art_keys: [i32; 4],
    pub art_key_base: i32,
    /// custom: the musician's art map overrides artKeys
    pub art_custom_keys: bool,
    pub art_dyn_cc: i32,
    pub art_expr_cc: i32,
    pub art_tremolo: bool,
    pub art_pluck_cap_below_energy: i32,
    pub art_pluck_vel_cap: i32,
    // keyswitch (data-driven): named articulations (key, mode), musical intents -> articulation index (-1 none).
    // Intents come from the step flags as the articulationKeys driver reads them: 32 pluck, 64 tremolo, 1024 short, else long.
    pub ks_arts: [KsArt; 16],
    pub ks_art_count: i32,
    /// the file's articulation names (keyswitch test markers)
    pub ks_art_names: [String; 16],
    pub ks_use: [i32; KS_INTENT_COUNT],
    /// the articulation when no intent is mapped (-1: none)
    pub ks_default: i32,
    pub ks_key_base: i32,
    pub ks_dyn_cc: i32,
    pub ks_expr_cc: i32,
    /// CCs this far ahead of the step (today's articulationKeys leads)
    pub ks_dyn_lead_ms: f32,
    pub ks_expr_lead_ms: f32,
    /// hold / latch / same keys this far ahead of their note
    pub ks_key_lead_ms: f32,
    /// latch return: 0 none, 1 same (the latched key again), 2 key
    pub ks_return: i32,
    pub ks_return_key: i32,
    /// notes fold by octave into this range (never onto a key)
    pub ks_low: i32,
    pub ks_high: i32,
    /// velocity clamp
    pub ks_vel_lo: i32,
    pub ks_vel_hi: i32,
    /// CC vibrato over notes of a beat or more
    pub ks_vibrato_cc: i32,
    pub ks_vibrato_depth: f32,
    /// -1 padded
    pub ks_init: [i32; 8],
    /// -1 padded
    pub ks_never: [i32; 8],
    /// values sent on reset (-1: none); Crow Hill 64 / 127
    pub ks_reset_dyn: i32,
    pub ks_reset_expr: i32,
    pub ks_send_on_change_only: bool,
    /// the pluck intent's velocity cap below this Energy
    pub ks_cap_below_energy: i32,
    pub ks_cap_vel: i32,
    // ujamPhrase
    pub uj_phrase_keys: [i32; 6],
    pub uj_additions: [i32; 4],
    pub uj_energy_base: [i32; 4],
    /// stopKey / modCC -1 = none
    pub uj_stop_key: i32,
    pub uj_play_base: i32,
    pub uj_play_high: i32,
    pub uj_bass_base: i32,
    pub uj_mod_cc: i32,
    /// 0 always a bass note, 1 only for slash chords, 2 never
    pub uj_bass_mode: i32,
    /// a song's Ending plays one (by Energy); -1 = none
    pub uj_ending_keys: [i32; 5],
    // ampleGuitar (Ample Sound guitars; every key from the target file)
    pub am_note_low: i32,
    pub am_note_high: i32,
    /// picked notes' velocity range (quiet notes keep their body)
    pub am_note_vel_low: i32,
    pub am_note_vel_high: i32,
    /// sustain, harmonic, palmMute, slideIn, legatoSlide, hammerOn
    pub am_art: [i32; 6],
    /// 6th..1st, for harmonic positions
    pub am_open_strings: [i32; 6],
    /// velocity >= 64 = on / detect / chord / on
    pub am_strum_mode_key: i32,
    pub am_chord_mode_key: i32,
    pub am_position_key: i32,
    pub am_open_first_key: i32,
    pub am_on_vel: i32,
    pub am_off_vel: i32,
    /// Strummer chord detect: the chord in low..high, root at or below rootHigh
    pub am_chord_low: i32,
    pub am_chord_high: i32,
    pub am_root_high: i32,
    /// Strummer single strings, 6th..1st
    pub am_strings: [i32; 6],
    // Strummer strokes: down, up, downMuted, upMuted x { full, treble } keys. Velocity is the player's dynamics:
    // Ample plays more strings the harder the stroke, so it's mapped into amStrokeVel, never fixed.
    pub am_strokes: [[i32; 2]; 4],
    pub am_stroke_vel_low: i32,
    pub am_stroke_vel_high: i32,
    /// Natural: note-off velocities (spec 10.1)
    pub am_release_velocity: bool,
    /// notes go this much early (spec 10.2)
    pub am_onset_ms: i32,
    /// Natural: Strum Time velocities (high = quick sweep)
    pub am_strum_time_lo: i32,
    pub am_strum_time_hi: i32,
    /// Strum Time (higher velocity = quicker sweep); -1 = none
    pub am_strum_time_key: i32,
    pub am_mute_voices: [i32; 2],
    /// downstroke noise 1, 2, upstroke noise 1, 2
    pub am_noise: [i32; 4],
    pub am_hit_top: i32,
    pub am_hit_top_muted: i32,
    pub am_strum_mute: i32,
    /// Natural FX (spec 10.4): fx.slap / fx.scratch, optional
    pub am_slap: i32,
    pub am_scratch: i32,
}

impl Default for TargetData {
    fn default() -> Self {
        Self {
            name: String::new(),
            help: String::new(),
            file: String::new(),
            slot: 0,
            driver: Driver::Generic,
            instruments: [false; K_INSTRUMENTS],
            ks_octave_default: 0,
            vel_lo: -1,
            vel_hi: -1,
            vel_curve: 1.0,
            drum_rules: Vec::new(),
            dr_chokes: Vec::new(),
            dr_shank_closed: -1,
            dr_shank_open: -1,
            dr_extra_open: -1,
            dr_foot_splash: -1,
            dr_ride_bell: -1,
            dr_cross_stick: -1,
            dr_pedal_hat: -1,
            dr_open_shank: -1,
            dr_splash: -1,
            dr_aperture_cc: -1,
            dr_cc_shank: -1,
            dr_cc_tip: -1,
            dr_crashes: [-1, -1],
            dr_china: -1,
            dr_ride2: -1,
            dr_bell_cymbal: -1,
            kb_finger: 25,
            kb_pick: 27,
            kb_slap: 30,
            kb_ghost: 22,
            kb_palm_cc: 9,
            kb_palm_on: 96,
            kb_pitch_shift: 12,
            kb_min_note: 40,
            kb_hammer: -1,
            kb_stop: -1,
            kb_down: -1,
            kb_legato_cc: -1,
            kb_mute_lo: -1,
            kb_mute_hi: -1,
            kb_vibrato_cc: -1,
            kb_let_ring_cc: -1,
            kb_pos_lo: -1,
            kb_pos_hi: -1,
            kb_pos_cc: -1,
            kb_pluck_cc: -1,
            kb_bend_range: 4,
            half_pedal: [24, 70, 127],
            soft_pedal_cc: 67,
            soft_pedal_max_energy: 1,
            harmonic_pedal_cc: -1,
            swam_family: 0,
            swam_instruments: Vec::new(),
            swam_expr_cc: 11,
            swam_vib_cc: 1,
            swam_ks_offset: 24,
            swam_keyswitches: true,
            swam_arts: [ArtKey::default(); 5],
            swam_custom_arts: false,
            swam_velocity_dynamics: false,
            swam_vel_lo: 1,
            swam_vel_hi: 127,
            fd_low: 55,
            fd_high: 91,
            fd_expr_cc: 11,
            fd_split_vib: 50,
            fd_split_port: 16,
            fd_split_slur: 61,
            fd_art: [36, 37, 38, 41, 42, 43, 44, 45, 46],
            fd_portamento_key: 50,
            fd_return_mode: 0,
            fd_return_key: 39,
            fd_bow_seconds: 3.0,
            og_expr_cc: 11,
            og_leslie_cc: 1,
            og_perc_cc: -1,
            og_chorus_cc: -1,
            og_note_vel: 100,
            og_drawbar_keys: [24, 26, 29, 35],
            og_registrations: [[-1; 4]; 4],
            og_lower_channel: 0,
            og_lower_keys: [36, 38, 41, 43],
            og_chorus_values: [0, 20, 40, 60, 80, 100, 127],
            ev_art: [-1; EV_ART_COUNT],
            ev_low: 40,
            ev_high: 86,
            ev_string_keys: [-1; 6],
            ev_string_count: 6,
            ev_drone_key: -1,
            ev_drone_note: -1,
            ev_perf_fx_base: 96,
            ev_perf_fx: [-1; 8],
            ev_vibrato_cc: 1,
            ev_legato_st: 2,
            ev_slide_range: 2,
            ev_bypass_legato_key: -1,
            ev_strum_down: -1,
            ev_strum_up: -1,
            ev_strum_engine: false,
            ev_trem_in: -1,
            ev_trem_on: -1,
            ev_trem_out: -1,
            ev_vel_lo: 30,
            ev_vel_hi: 124,
            ev_bend_blues: false,
            ev_open: [-1; 6],
            in_art: [-1; IN_ART_COUNT],
            in_low: 55,
            in_high: 88,
            in_layout: 0,
            in_never_send: [-1; 4],
            in_init_keys: [-1; 4],
            in_return_mode: 0,
            in_return_key: -1,
            in_expr_cc: 11,
            in_vibrato_cc: -1,
            in_master_key_cc: -1,
            in_mode_cc: -1,
            in_tremolo_speed_cc: -1,
            in_chord_down: -1,
            in_chord_up: -1,
            in_root_select: -1,
            in_variation: [-1; 3],
            in_slots: false,
            in_drone_note: -1,
            in_string_count: 4,
            br_low: 40,
            br_high: 84,
            br_dyn_cc: 1,
            br_note_vel: 100,
            br_setup_ccs: [(-1, -1); 4],
            br_legato_off: 12,
            br_legato_on: 14,
            br_instrument_cc: 15,
            br_instrument_value: [-1; 27],
            br_patch_ensemble: 21,
            br_patch_melodic: 7,
            br_patch_low: 22,
            art_keys: [0, 1, 2, -1],
            art_key_base: 24,
            art_custom_keys: false,
            art_dyn_cc: -1,
            art_expr_cc: 11,
            art_tremolo: false,
            art_pluck_cap_below_energy: 0,
            art_pluck_vel_cap: 127,
            ks_arts: [KsArt::default(); 16],
            ks_art_count: 0,
            ks_art_names: std::array::from_fn(|_| String::new()),
            ks_use: [-1; KS_INTENT_COUNT],
            ks_default: -1,
            ks_key_base: 0,
            ks_dyn_cc: -1,
            ks_expr_cc: -1,
            ks_dyn_lead_ms: 15.0,
            ks_expr_lead_ms: 10.0,
            ks_key_lead_ms: 10.0,
            ks_return: 0,
            ks_return_key: -1,
            ks_low: 0,
            ks_high: 127,
            ks_vel_lo: 1,
            ks_vel_hi: 127,
            ks_vibrato_cc: -1,
            ks_vibrato_depth: 0.5,
            ks_init: [-1; 8],
            ks_never: [-1; 8],
            ks_reset_dyn: -1,
            ks_reset_expr: -1,
            ks_send_on_change_only: true,
            ks_cap_below_energy: 0,
            ks_cap_vel: 127,
            uj_phrase_keys: [24, 26, 28, 29, 31, 33],
            uj_additions: [25, 27, 30, 32],
            uj_energy_base: [0, 2, 4, 5],
            uj_stop_key: 35,
            uj_play_base: 60,
            uj_play_high: 83,
            uj_bass_base: 48,
            uj_mod_cc: 1,
            uj_bass_mode: 0,
            uj_ending_keys: [-1; 5],
            am_note_low: 40,
            am_note_high: 84,
            am_note_vel_low: 40,
            am_note_vel_high: 124,
            am_art: [24, 25, 26, 27, 28, 29],
            am_open_strings: [40, 45, 50, 55, 59, 64],
            am_strum_mode_key: 97,
            am_chord_mode_key: 86,
            am_position_key: 107,
            am_open_first_key: 104,
            am_on_vel: 100,
            am_off_vel: 20,
            am_chord_low: 36,
            am_chord_high: 59,
            am_root_high: 47,
            am_strings: [62, 64, 65, 67, 69, 71],
            am_strokes: [[72, 74], [76, 76], [79, 81], [83, 83]],
            am_stroke_vel_low: 45,
            am_stroke_vel_high: 127,
            am_release_velocity: false,
            am_onset_ms: 0,
            am_strum_time_lo: 118,
            am_strum_time_hi: 126,
            am_strum_time_key: 85,
            am_mute_voices: [77, 78],
            am_noise: [93, 95, 94, 96],
            am_hit_top: 101,
            am_hit_top_muted: 102,
            am_strum_mute: 92,
            am_slap: 90,
            am_scratch: 89,
        }
    }
}

impl TargetData {
    /// Velocity calibration: `v -> velLo + (velHi - velLo) * (v/127)^velCurve`; `velLo < 0` = none.
    pub fn calibrated(&self, v: i32) -> i32 {
        if self.vel_lo < 0 {
            return v;
        }
        let x = (f64::from(v.clamp(1, 127)) / 127.0).powf(f64::from(self.vel_curve));
        let r = (f64::from(self.vel_lo) + f64::from(self.vel_hi.wrapping_sub(self.vel_lo)) * x).round() as i32;
        r.clamp(1, 127)
    }
}

/// Strummer or notes on an Ample target: the musician's switch (1 Strummer, 2 Notes), else the player's
/// "ample" mode; auto (and a guitarist without one) strums at the Energy levels where its patterns strum.
/// energy 1-4; 0 = most of them.
pub fn ample_strums(p: &PersonaData, musician_switch: i32, energy: i32) -> bool {
    if musician_switch == 1 {
        return true;
    }
    if musician_switch == 2 {
        return false;
    }
    if p.ample.mode == 0 || p.ample.mode == 1 {
        return p.ample.mode == 0;
    }
    if p.ample.mode == -1 && p.role != 3 {
        return false;
    }
    if (1..=4).contains(&energy) {
        return p.ample.strum_at.get((energy - 1) as usize).copied().unwrap_or(false);
    }
    let n: i32 = p.ample.strum_at.iter().map(|&b| i32::from(b)).sum();
    n >= 2
}

/// Auto: strums at some Energy levels and picks at others.
pub fn ample_mixed(p: &PersonaData) -> bool {
    let auto_mode = p.ample.mode == 2 || (p.ample.mode == -1 && p.role == 3);
    let s = &p.ample.strum_at;
    auto_mode && !(s[0] == s[1] && s[1] == s[2] && s[2] == s[3])
}

/// Immutable once built. The engine holds an `Arc<LibraryData>`; reload swaps it between blocks.
#[derive(Clone, Debug)]
pub struct LibraryData {
    /// file order
    pub personas: Vec<PersonaData>,
    /// seed -> index into `personas` (None = none)
    pub by_seed: [Option<u16>; K_MAX_SEED + 1],
    /// seed per [instrument][slot-1], 0 = empty (the Amorph slot path)
    pub roster: [[i32; K_PLAYER_SLOTS]; K_INSTRUMENTS],
    /// every player per instrument, browser order: numbered slots, then A-Z
    pub players: [Vec<i32>; K_INSTRUMENTS],
    /// slot -> index into `target_store` (slot 0 never None)
    pub targets: [Option<u16>; K_TARGET_SLOTS],
    pub target_store: Vec<TargetData>,
    /// "Players/X.json: duplicate seed 6 (also Dex.json)"
    pub errors: Vec<String>,
    /// players / targets from the open song's own copies
    pub overlay_names: Vec<String>,
    /// Library files those copies stand in for: "Players/Zed.json"
    pub shadowed: Vec<String>,
    pub folder: String,
}

impl Default for LibraryData {
    fn default() -> Self {
        Self {
            personas: Vec::new(),
            by_seed: [None; K_MAX_SEED + 1],
            roster: [[0; K_PLAYER_SLOTS]; K_INSTRUMENTS],
            players: std::array::from_fn(|_| Vec::new()),
            targets: [None; K_TARGET_SLOTS],
            target_store: Vec::new(),
            errors: Vec::new(),
            overlay_names: Vec::new(),
            shadowed: Vec::new(),
            folder: String::new(),
        }
    }
}

impl LibraryData {
    pub fn plays(&self, instrument: i32, seed: i32) -> bool {
        let Some(list) = usize::try_from(instrument).ok().and_then(|i| self.players.get(i)) else {
            return false;
        };
        list.contains(&seed)
    }

    pub fn seed_for_name(&self, n: &str) -> i32 {
        for p in &self.personas {
            if p.name == n {
                return p.seed;
            }
        }
        0
    }

    pub fn target_slot_for_name(&self, n: &str) -> i32 {
        for i in 0..K_TARGET_SLOTS as i32 {
            if let Some(t) = self.target(i)
                && t.name == n
            {
                return i;
            }
        }
        -1
    }

    /// Slot (1..16) of a persona on an instrument, 0 if it does not play that instrument.
    pub fn slot_for(&self, instrument: i32, seed: i32) -> i32 {
        let Some(row) = usize::try_from(instrument).ok().and_then(|i| self.roster.get(i)) else {
            return 0;
        };
        if seed <= 0 {
            return 0;
        }
        for (s, &r) in row.iter().enumerate() {
            if r == seed {
                return s as i32 + 1;
            }
        }
        0
    }

    /// The persona for a seed (C++: `bySeed[seed]`), if any.
    pub fn persona(&self, seed: i32) -> Option<&PersonaData> {
        let i = (*self.by_seed.get(usize::try_from(seed).ok()?)?)?;
        self.personas.get(usize::from(i))
    }

    /// The target in a slot (C++: `targets[slot]`), if any.
    pub fn target(&self, slot: i32) -> Option<&TargetData> {
        let i = (*self.targets.get(usize::try_from(slot).ok()?)?)?;
        self.target_store.get(usize::from(i))
    }

    /// Built-in fallback: Generic at slot 0, no personas. Always valid.
    pub fn make_empty() -> Arc<LibraryData> {
        let mut lib = LibraryData::default();
        lib.add_built_in_generic();
        Arc::new(lib)
    }

    pub fn add_built_in_generic(&mut self) {
        let g = TargetData {
            name: "Generic".to_string(),
            slot: 0,
            driver: Driver::Generic,
            file: "(built in)".to_string(),
            instruments: [true; K_INSTRUMENTS],
            ..TargetData::default()
        };
        self.target_store.push(g);
        self.relink();
    }

    /// Re-point `by_seed`/`targets` after the vectors are final (indices into own storage).
    pub fn relink(&mut self) {
        self.by_seed = [None; K_MAX_SEED + 1];
        for (i, p) in self.personas.iter().enumerate() {
            if p.seed > 0
                && p.seed as usize <= K_MAX_SEED
                && let (Some(cell), Ok(idx)) = (self.by_seed.get_mut(p.seed as usize), u16::try_from(i))
            {
                *cell = Some(idx);
            }
        }
        self.targets = [None; K_TARGET_SLOTS];
        for (i, t) in self.target_store.iter().enumerate() {
            if t.slot >= 0
                && (t.slot as usize) < K_TARGET_SLOTS
                && let (Some(cell), Ok(idx)) = (self.targets.get_mut(t.slot as usize), u16::try_from(i))
            {
                *cell = Some(idx);
            }
        }
    }
}

/// One documented key of a Library file. `block` is "player", "player.<block>" or "target.<driver>".
#[derive(Clone, Copy, Debug)]
pub struct KeyDoc {
    pub block: &'static str,
    pub key: &'static str,
    pub type_: &'static str,
    pub def: &'static str,
    pub help: &'static str,
}
