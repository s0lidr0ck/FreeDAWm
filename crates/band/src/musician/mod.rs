//! Port of `Musician.h`: the struct with every member and every inline method, in the header's order.
//!
//! The port of DSP.txt `processor Musician` (MIDI Studio Band v0.8.x). Each `.cpp` is one file here
//! (`core.rs`, `queue.rs`, `keys.rs`, …) holding `impl Musician { … }` with every method that `.cpp` defines.
//! Differences from the C++, all of form (never of result):
//!   * **Pointers become arguments** ([`Ctx`]): the Library, the song's timeline and the interplay record are not
//!     members; a method that reads them takes `cx: &Ctx` / `&mut Ctx` (or `lib: &LibraryData` when it reads only
//!     the Library). `cur` (a pointer into the timeline) is a copy of the entry (`Option<TimelineEntry>`).
//!   * **The sink is an argument**: every method that (directly or through a callee) sends MIDI takes
//!     `out: &mut dyn MidiSink`. `setParameter` / `setHostTransport` / `setPersonaSeed` / `setSong` /
//!     `setPerformance` take the Band's idle sink as `idle` (C++ `sink = idleSink`).
//!   * Context arguments come **after** the C++ arguments: `(…, cx, out)` / `(…, lib)`. C++ default arguments are
//!     passed explicitly.
//!   * **The scheduling queue** (`queue`, `nextDuePpq`, `scheduleFlags`, `scheduleOffVel`, `scheduleCh`) is the
//!     [`Scheduler`] in `self.q`, so a driver can borrow it (`driver_kit::DriverCtx`) while the driver's own state
//!     (`ExprCover`, `ReturnMode`, …) is borrowed alongside. `kitCtx()` is written in place:
//!     ```text
//!     let ms = self.ppq_per_ms();
//!     let mut s = SchedAt { q: &mut self.q, now: self.current_ppq };
//!     let mut c = kit::DriverCtx { sink: &mut s, now: self.current_ppq, ms };
//!     kit::cc_before_notes(&mut c, &mut self.in_expr, 11, v, &ons, n, 5.0, gap);
//!     ```
//!   * `CArr::at()` wraps its index like Cmajor's `.at()` ([`CArr`]); never replace it with `[]`.
//!   * Every event a musician sends carries `musician = index` (Rust only, not compared).
//!   * Kept takes are out of scope: `CaptureRing` pushes go to the no-op [`Capture`]; `FrozenPart` / `playFrozen`
//!     are ported but never fed.
#![allow(clippy::too_many_arguments)]
// the signatures are the C++ ones plus the context arguments
// Scaffolding (Task 9): the stubbed `.cpp` bodies leave most members and helpers unused until Tasks 11 and 15–17
// port them; Task 20 removes this allow.
#![allow(dead_code)]

mod bass;
mod bowed;
mod core;
mod drums;
mod ensemble;
mod evolution;
mod guitar;
mod indiginus;
mod keys;
mod keyswitch;
mod organ;
mod queue;

use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, Ordering};

#[allow(unused_imports)]
pub(crate) use crate::ample_slots::{AmpleSlotTable, AmpleSongChords, build_ample_slots, collect_ample_chords};
pub(crate) use crate::driver_kit as kit;
#[allow(unused_imports)]
pub(crate) use crate::driver_kit::{DriverCtx, ExprCover, ReturnMode, Sched};
pub(crate) use crate::fiddle_voice::Splits;
#[allow(unused_imports)]
pub(crate) use crate::fiddle_voice::licks::{LickNote, LickSet};
pub(crate) use crate::guitar_shapes::Shape as GShape;
#[allow(unused_imports)]
pub(crate) use crate::library::{
    Driver, K_INSTRUMENTS, K_MAX_SEED, K_PLAYER_SLOTS, K_TARGET_SLOTS, KS_LONG, KS_PLUCK, KS_SHORT, KS_TREMOLO, LibraryData, PersonaData, TargetData,
};
pub(crate) use crate::live::{BandShared, PerformanceState};
#[allow(unused_imports)]
pub(crate) use crate::meter::{GridRole, Meter, accent_src, accent12_scale, fill_zone_start, half_step, info, is_beat, is12, map16, meter_from_host};
pub(crate) use crate::midi_types::{MidiEvent, MidiSink};
pub(crate) use crate::plucked_voice::{HeldKey, Join};
pub(crate) use crate::session_tables;
pub(crate) use crate::song::{FrozenPart, SongLoc, SongSection, SongTimeline, TimelineEntry};
pub(crate) use crate::song_feel::FeelShift;

/// What a musician's methods read through the C++ pointers (`lib`, `song`, `shared`), passed in by the Band.
/// `perf` is the Band's copy of the performance state; a musician reads its own `perf` member (set by
/// [`Musician::set_performance`], always equal to it), as the C++ does.
pub(crate) struct Ctx<'a> {
    pub lib: &'a LibraryData,
    pub song: Option<&'a SongTimeline>,
    pub perf: &'a PerformanceState,
    pub shared: &'a mut BandShared,
}

/// Fixed array with Cmajor semantics: zero-initialised, and `at()` wraps the index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CArr<T, const N: usize> {
    pub v: [T; N],
}

impl<T: Copy + Default, const N: usize> Default for CArr<T, N> {
    fn default() -> Self {
        Self { v: [T::default(); N] }
    }
}

impl<T: Copy, const N: usize> CArr<T, N> {
    pub(crate) const fn new(v: [T; N]) -> Self {
        Self { v }
    }
    /// `int r = i % N; return r < 0 ? r + N : r;` (N > 0 for every CArr in the engine).
    pub(crate) const fn wrap(i: i32) -> usize {
        let n = N as i32;
        let r = i % n;
        (if r < 0 { r + n } else { r }) as usize
    }
    /// `at (i)`, read by reference.
    pub(crate) fn at(&self, i: i32) -> &T {
        &self.v[Self::wrap(i)]
    }
    /// `at (i)`, written.
    pub(crate) fn at_mut(&mut self, i: i32) -> &mut T {
        &mut self.v[Self::wrap(i)]
    }
    /// `at (i)`, read by value.
    pub(crate) fn val(&self, i: i32) -> T {
        self.v[Self::wrap(i)]
    }
    /// `at (i) = x`.
    pub(crate) fn set(&mut self, i: i32, x: T) {
        self.v[Self::wrap(i)] = x;
    }
    pub(crate) const fn size() -> i32 {
        N as i32
    }
}

/// `CRef::at`: a compiled table read with the same wrapping (`cref (session::offsets).at (i)`).
pub(crate) fn cref_at<T: Copy + Default>(a: &[T], i: i32) -> T {
    let n = i32::try_from(a.len()).unwrap_or(i32::MAX);
    if n == 0 {
        return T::default();
    }
    let r = i % n;
    let k = if r < 0 { r + n } else { r };
    usize::try_from(k).ok().and_then(|k| a.get(k)).copied().unwrap_or_default()
}

/// A plain C++ `a[(size_t) i]` on a fixed table: out of range (UB in C++) reads the default here.
pub(crate) fn at_or_default<T: Copy + Default>(a: &[T], i: i32) -> T {
    usize::try_from(i).ok().and_then(|k| a.get(k)).copied().unwrap_or_default()
}

/// A plain C++ `a[(size_t) i] = x` on a fixed table: out of range (UB in C++) writes nothing here.
pub(crate) fn put_at<T>(a: &mut [T], i: i32, x: T) {
    if let Some(s) = usize::try_from(i).ok().and_then(|k| a.get_mut(k)) {
        *s = x;
    }
}

// Cmajor's min / max / clamp take one type; these do the same (NaN behaves as the C++ comparisons do).
pub(crate) fn min<T: PartialOrd>(a: T, b: T) -> T {
    if b < a { b } else { a }
}
pub(crate) fn max<T: PartialOrd>(a: T, b: T) -> T {
    if a < b { b } else { a }
}
pub(crate) fn clamp<T: PartialOrd>(v: T, lo: T, hi: T) -> T {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// The Cmajor processor's specialisation parameters (DSP.txt line 7).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MusicianDefaults {
    pub channel: i32,
    pub on: bool,
    pub instrument: i32,
    pub player: i32,
    pub idea: i32,
    pub energy: i32,
    pub swing: f32,
    pub humanize: f32,
    pub dynamics: f32,
    pub fills: f32,
    pub respect: bool,
    pub octave: i32,
    pub length: f32,
    pub velocity: i32,
    pub target: i32,
    pub ks: i32,
    pub swam_inst: i32,
    pub voice: i32,
    pub speed: i32,
    pub art_map: i32,
}

impl Default for MusicianDefaults {
    fn default() -> Self {
        Self {
            channel: 0,
            on: false,
            instrument: 0,
            player: 0,
            idea: 0,
            energy: 3,
            swing: 50.0,
            humanize: 50.0,
            dynamics: 50.0,
            fills: 40.0,
            respect: false,
            octave: 0,
            length: 0.85,
            velocity: 96,
            target: 0,
            ks: 0,
            swam_inst: 0,
            voice: 0,
            speed: 1,
            art_map: 12816,
        }
    }
}

/// `TakeCapture.h` `CaptureEvent`: what a kept take records (the Rust port keeps no takes; see [`Capture`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CaptureEvent {
    /// song position (host ppq + any performance jump offset)
    pub ppq: f64,
    /// 0..5; -1 = a break (stop, start, jump back); -2 = a heartbeat (where the band is)
    pub musician: i8,
    /// status without the channel
    pub status: u8,
    pub d1: u8,
    pub d2: u8,
    pub keyswitch: bool,
}

/// The kept-take capture ring, as a no-op: kept takes are out of scope, so pushes are kept as calls (the output is
/// unchanged) and go nowhere.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Capture;

impl Capture {
    pub(crate) fn push(&mut self, c: CaptureEvent) {
        let _ = c;
    }
}

// Test hooks (`inline static int`): shared by every musician, as in the C++.
/// lick notes from a set written for another meter (meter suite)
pub(crate) static LICK_OFF_METER: AtomicI32 = AtomicI32::new(0);
/// hemiola-grouped notes; ones off a phrase end or in 4/4 (never)
pub(crate) static HEMIOLA_NOTES: AtomicI32 = AtomicI32::new(0);
pub(crate) static HEMIOLA_OFF: AtomicI32 = AtomicI32::new(0);
/// lick notes struck, all musicians (meter suite)
pub(crate) static LICK_NOTES_PLAYED: AtomicI32 = AtomicI32::new(0);

// Scheduled events: kind 0 = note (d1 note, d2 velocity), kind 1 = CC (d1 controller, d2 value), 3 = note-off,
// 4 = pitch wheel (d1 LSB, d2 MSB; centre 8192).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Pending {
    pub used: bool,
    pub kind: i32,
    pub d1: i32,
    pub d2: i32,
    pub on_ppq: f64,
    pub dur_ppq: f32,
    pub flags: i32,
    pub off_vel: i32,
    /// 0 = the musician's channel, 1-16 = that one
    pub ch: i32,
}

impl Default for Pending {
    fn default() -> Self {
        Self { used: false, kind: 0, d1: 0, d2: 0, on_ppq: 0.0, dur_ppq: 0.0, flags: 0, off_vel: -1, ch: 0 }
    }
}

/// The scheduling queue and what `schedule()` stamps on each event (C++ members of Musician, grouped so a driver
/// context can borrow them on their own).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Scheduler {
    pub queue: CArr<Pending, 64>,
    pub next_due_ppq: f64,
    pub schedule_flags: i32,
    /// a kept take's recorded note-off velocity (-1 = work it out)
    pub schedule_off_vel: i32,
    /// the event's channel: 0 = the musician's, 1-16 = that one (the Hammond's lower manual)
    pub schedule_ch: i32,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self { queue: CArr::default(), next_due_ppq: 1.0e18, schedule_flags: 0, schedule_off_vel: -1, schedule_ch: 0 }
    }
}

impl Scheduler {
    /// `schedule (kind, d1, d2, onPpq, durPpq)`, with `currentPpq` passed in.
    pub(crate) fn schedule(&mut self, current_ppq: f64, kind: i32, d1: i32, d2: i32, on_ppq: f64, dur_ppq: f32) {
        let when = max(on_ppq, current_ppq);
        let mut slot = -1;
        let mut weakest = 999;

        for i in 0..64 {
            let p = *self.queue.at(i);
            if !p.used {
                slot = i;
                break;
            }
            if p.kind == 0 && (p.flags & 8192) == 0 && p.d2 < weakest {
                // a keyswitch / held key is never the victim
                weakest = p.d2;
                slot = i;
            }
        }

        // Queue full of louder notes: drop this one rather than hang.
        if slot < 0 || (self.queue.at(slot).used && (kind == 0 && d2 <= weakest)) {
            return;
        }

        let (flags, off_vel, ch) = (self.schedule_flags, self.schedule_off_vel, self.schedule_ch);
        let p = self.queue.at_mut(slot);
        p.used = true;
        p.flags = flags;
        p.off_vel = off_vel;
        p.ch = ch;
        p.kind = kind;
        p.d1 = d1;
        p.d2 = d2;
        p.on_ppq = when;
        p.dur_ppq = dur_ppq;
        if when < self.next_due_ppq {
            self.next_due_ppq = when;
        }
    }
}

/// `kitCtx()`'s scheduler half: the queue at `currentPpq`, as `driver_kit::DriverCtx` borrows it.
pub(crate) struct SchedAt<'a> {
    pub q: &'a mut Scheduler,
    pub now: f64,
}

impl Sched for SchedAt<'_> {
    /// `kitSched`: flags compared with kKeepFlags (-1), not >= 0: -1 is the one "keep" value.
    fn sched(&mut self, kind: i32, d1: i32, d2: i32, at: f64, dur_ppq: f32, flags: i32) {
        if flags != kit::K_KEEP_FLAGS {
            self.q.schedule_flags = flags;
        }
        self.q.schedule(self.now, kind, d1, d2, at, dur_ppq);
        if flags != kit::K_KEEP_FLAGS {
            self.q.schedule_flags = 0;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Chord {
    pub degree: i32,
    pub flavor: i32,
    pub root_pc: i32,
    pub count: i32,
    pub seventh: i32,
    pub tones: CArr<i32, 5>,
    pub bass_pc: i32,
}

impl Default for Chord {
    fn default() -> Self {
        Self { degree: 0, flavor: 0, root_pc: 0, count: 0, seventh: 0, tones: CArr::default(), bass_pc: -1 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Voicing {
    pub count: i32,
    pub notes: CArr<i32, 6>,
}

/// The state after each recent Embertone note, so a regeneration rewinds to the last note that started before its cut.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct JbSnap {
    pub on: f64,
    pub note_off: f64,
    pub last_end: f64,
    pub prev_on: f64,
    pub run_end: f64,
    pub note: i32,
    pub partner: i32,
    pub port_bar: i32,
    pub lens_n: i32,
    pub subs: i32,
    pub prev_end: bool,
    pub prev_fast: bool,
    pub sustain_on: bool,
    pub lens: CArr<f32, 4>,
    pub sub_on: CArr<f64, 9>,
}

impl Default for JbSnap {
    fn default() -> Self {
        Self {
            on: 0.0,
            note_off: 0.0,
            last_end: 0.0,
            prev_on: 0.0,
            run_end: 0.0,
            note: -1,
            partner: -1,
            port_bar: -1000,
            lens_n: 0,
            subs: 0,
            prev_end: false,
            prev_fast: false,
            sustain_on: false,
            lens: CArr::default(),
            sub_on: CArr::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct KsHoldStep {
    pub owner_on: f64,
    pub until: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AmpleNote {
    pub note: i32,
    pub vel: f32,
    pub len: f32,
    pub on: f64,
    pub flags: i32,
    pub hand: i32,
}

impl Default for AmpleNote {
    fn default() -> Self {
        Self { note: 0, vel: 0.0, len: 0.0, on: 0.0, flags: 0, hand: 1 }
    }
}

/// A lick's marks for the step being performed. `band`: a bowed::Band forced (-1: chosen as usual).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LickMark {
    pub on: bool,
    pub land: bool,
    pub accent: bool,
    pub slide_in: bool,
    pub band: i32,
    pub partner_int: i32,
    pub absolute: bool,
}

impl Default for LickMark {
    fn default() -> Self {
        Self { on: false, land: false, accent: false, slide_in: false, band: -1, partner_int: 0, absolute: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EvPrev {
    pub note: i32,
    pub on: f64,
    pub end: f64,
}

impl Default for EvPrev {
    fn default() -> Self {
        Self { note: -1, on: -1.0e9, end: -1.0e9 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EvTrem {
    pub on: bool,
    pub pitch: i32,
    pub start: f64,
    pub end: f64,
    pub seed: u32,
}

impl Default for EvTrem {
    fn default() -> Self {
        Self { on: false, pitch: -1, start: 0.0, end: 0.0, seed: 0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BrNote {
    pub note: i32,
    pub vel: f32,
    pub len: f32,
    pub written: f32,
    pub on: f64,
    pub hand: i32,
}

impl Default for BrNote {
    fn default() -> Self {
        Self { note: 0, vel: 0.0, len: 0.0, written: 0.0, on: 0.0, hand: 1 }
    }
}

/// Row 0 of every Cmajor feel_* table: the Session player (tables.js `session`). Built once (the Band builds it
/// before any audio, so the audio thread never allocates it).
pub(crate) fn session_feel() -> &'static PersonaData {
    static S: OnceLock<PersonaData> = OnceLock::new();
    S.get_or_init(|| PersonaData {
        name: "Session".to_string(),
        seed: 0,
        heavy: 0,
        artic: 0,
        role: -1,
        voicing: 0,
        swing_base: 50.0,
        swing_steps: 4,
        timing_ms: 0.0,
        lane_ms: [0.0; 4],
        roll_ms: 8.0,
        jitter_ms: 8.0,
        vel_var: 8.0,
        arc: 0.06,
        pedal_mode: 0,
        fill_rate: 0.0,
        fill_count: 3,
        rh_low: 0,
        rh_high: 0,
        rh_anchor: 0,
        lh_low: 0,
        accents: [1.06, 0.96, 0.99, 0.96, 1.0, 0.96, 0.99, 0.96, 1.03, 0.96, 0.99, 0.96, 1.0, 0.96, 0.99, 0.97],
        ..PersonaData::default()
    })
}

/// `*lib->targets[0]` when a Library somehow has no slot 0 (LibraryData always has one): the built-in Generic.
pub(crate) fn generic_target() -> &'static TargetData {
    static G: OnceLock<TargetData> = OnceLock::new();
    G.get_or_init(|| TargetData {
        name: "Generic".to_string(),
        slot: 0,
        driver: Driver::Generic,
        file: "(built in)".to_string(),
        instruments: [true; K_INSTRUMENTS],
        ..TargetData::default()
    })
}

/// noStep
pub(crate) const NO_STEP: i32 = -2147483647;

pub(crate) struct Musician {
    // UI feed (were the transportStateOut / tempoOut / pulseOut / barOut endpoints).
    pub(crate) ui_playing: bool,
    /// song path: the section (index) and timeline entry playing
    pub(crate) ui_section: i32,
    pub(crate) ui_ordinal: i32,
    /// which musician (0..5) this is: a section's feel is per musician
    pub(crate) index: i32,

    // Time signatures: the meter being played (latched at render start / stopped / relocate; while playing at the next
    // bar line).
    meter: Meter,
    pending_meter: Meter,
    /// the host's time signature isn't 4/4, 3/4 or 6/8: played as 4/4
    host_meter_unsupported: bool,
    /// test hook: the highest step-in-bar dispatched (meter suite)
    pub(crate) last_sib_seen: i32,
    // Mid-play meter changes (Task 9): what the meter should be is noted every frame (pendingMeter) and played from the
    // next bar line of our clock. barOrigin: where bar 0 of the current meter's grid would be (0 until a mid-play
    // change; the grid is re-anchored at the switch, bars keep counting so the song carries on where it was).
    bar_origin: f64,
    pub(crate) busy_counted_ppq: f64,
    pub(crate) ui_tempo: f32,
    pub(crate) ui_pulse: i32,
    pub(crate) ui_bar: i32,

    // ---- private ----
    d: MusicianDefaults,
    seed_override: i32,
    sample_rate: f64,
    frame_offset: i32,

    // ======== translated body follows ========

    // ---- State ---------------------------------------------------------------
    variation: i32,
    pattern: i32,
    key_root: i32,
    scale_mode: i32,
    octave_shift: i32,
    speed: i32,
    velocity: i32,
    length: f32,
    enabled: bool,
    muted: bool,
    midi_channel0: i32,
    player: i32,
    energy: i32,
    progression: i32,
    bars_mode: i32,
    take: i32,
    slots: CArr<i32, 8>,
    swing_amt: f32,
    humanize: f32,
    dynamics: f32,
    fills: f32,
    // Song feel: the Speed knob as set (speed = it, moved by the song's Time), and the step's shift (set around each
    // generated step, like the section's Energy; neutral outside one).
    speed_knob: i32,
    step_shift: FeelShift,
    respect: bool,
    target: i32,
    ks_octave: i32,

    // Profile state: what the target plugin has been told so far.
    mute_cc: i32,
    style_ks: i32,
    ghost_held: i32,
    /// the last chordal strike (velocity, step): a half-bar re-voice strikes from it when nothing is held any more
    strike_last_vel: f32,
    strike_last_ppq: f64,
    soft_pedal: i32,
    /// scheduleFlags / scheduleOffVel / scheduleCh live in `q` (the scheduling queue).
    q: Scheduler,
    /// a kept take's recorded note-off velocity (-1 = work it out)
    emit_off_vel: i32,
    /// the event's channel: 0 = the musician's, 1-16 = that one (the Hammond's lower manual)
    emit_ch: i32,
    active_flags: CArr<i32, 64>,
    step_flags: CArr<i32, 16>,

    // ---- SWAM (Audio Modeling) profile state ---------------------------------
    swam_inst: i32,
    swam_voice: i32,
    expr_level: i32,
    vib_level: i32,
    swam_play_mode: i32,
    swam_trem: i32,
    swam_harm: i32,
    /// the one note SWAM is sounding (mono)
    swam_note: i32,
    /// when it is due to end
    swam_note_off: f64,
    // Embertone in Natural: sustain held over a fast run (and where the run ends), the last portamento's bar, whether the
    // last note closed a phrase or was fast, a double stop's partner still sounding, the last four lengths (beats).
    jb_sustain_on: bool,
    jb_run_end: f64,
    jb_port_bar: i32,
    jb_prev_end: bool,
    jb_prev_fast: bool,
    jb_partner: i32,
    jb_prev_on: f64,
    jb_lens: CArr<f32, 4>,
    jb_lens_n: i32,
    /// a CC64 127 went out (or is queued) since the last reset
    jb_sustain_sent: bool,
    /// releases never before the last regeneration's cut
    jb_rel_floor: f64,
    jb_stray_down: bool,
    /// the last note's segments (re-bows)
    jb_sub_on: CArr<f64, 9>,
    jb_subs: i32,
    jb_hist: CArr<JbSnap, 8>,
    jb_hist_n: i32,
    // The Fiddle in Natural: the last note played (output pitch), when it ends, and whether it closed a phrase or was fast.
    fd_prev_note: i32,
    fd_prev_on: f64,
    fd_prev_off: f64,
    fd_last_orn: bool,
    fd_orn_beat: i32,
    fd_prev_end: bool,
    fd_prev_fast: bool,
    fd_chop_dip: bool,
    /// a double stop's partner still sounding
    fd_partner: i32,
    // Sampled ornaments (return confirmed): the latched articulation key, when it was pressed and when its return is
    // due; the last return scheduled (a regeneration can drop it); the key held for the note in "hold" mode.
    /// latched / latchAt / due / retKey / held (DriverKit)
    fd_ret: ReturnMode,
    /// the last note's inner attacks (shuffle, re-bow)
    fd_sub_pitch: CArr<i32, 20>,
    fd_sub_on: CArr<f64, 20>,
    fd_subs: i32,
    /// for the breath model
    sounding_since: f64,
    last_note_end: f64,
    /// articulation flags for notes added by role code
    note_intent: i32,
    /// false when the pattern ends mid-chord (fast speeds): no anticipation
    pattern_meets_bar: bool,
    /// 0 = half a bar per chord, 1 = one bar, 2 = two bars
    chord_bars_mode: i32,
    // The song path: the timeline (Ctx), and the entry of the step being generated (valid inside generateStep).
    cur: Option<TimelineEntry>,
    cur_index: i32,
    cur_prog_bar: i32,
    cur_global: i32,
    cur_start_ppq: f64,
    cur_end_ppq: f64,
    /// Live / jumps / performance keys (Live.h)
    perf: PerformanceState,
    /// Live mode: the chord now, the next one, the one before
    live_entries: [TimelineEntry; 3],
    /// Ending: the final hit rings (for holdLen ppq)
    hold_step: bool,
    step_ppq_now: f64,
    bar_seen: i32,
    bar_notes: i32,
    bar_note_sum: f32,
    hold_len: f64,
    live_mode: bool,
    perf_fill: bool,
    perf_crash: bool,
    force_phrase_end: bool,
    stop_hit: bool,
    /// Tension and release: this chord's pull, and how far through its last bar
    ten_now: f32,
    ten_ramp: f32,
    /// the last beat before a release; a release's downbeat
    ten_tail: bool,
    rel_down: bool,
    jump_next_idx: i32,
    step_pass: i32,
    step_prog_bar: i32,
    /// a section this musician doesn't play in
    song_silent: bool,
    /// where its notes stop (the section's first step), once reached
    silence_at: f64,

    // ---- Crow Hill Pocket Strings profile ------------------------------------
    crow_art: i32,
    crow_dyn: i32,
    crow_expr: i32,

    // ---- keyswitch driver (MusicianKeyswitch.cpp) ----------
    ks_art_sent: i32,
    ks_dyn: i32,
    ks_expr: i32,
    ks_dyn_sent: i32,
    ks_expr_sent: i32,
    ks_init_pass: i32,
    ks_hold_key: i32,
    ks_same_key: i32,
    ks_ret: ReturnMode,
    ks_vib_until: f64,
    ks_vib_start: f64,
    ks_hold_at: f64,
    ks_hold_until: f64,
    ks_hold_owner_on: f64,
    ks_hold_owner_end: f64,
    ks_same_at: f64,
    ks_same_end_at: f64,
    ks_ret_fire_at: f64,
    ks_init_at: f64,
    /// the held key's releases as notes extended it
    ks_hold_hist: [KsHoldStep; 8],
    ks_hold_hist_n: i32,

    // ---- Miroslav Philharmonik 2 profile ---------------------------------------
    // artMap packs four keyswitch slots (0-15 semitones above C0): long | short<<4 | pluck<<8 | tremolo<<12.
    art_map: i32,

    // ---- UJAM Symphonic Elements (STRIIIINGS / BRAAASS) -----------------------------
    ujam_phrase: i32,
    ujam_cc1: i32,
    ujam_last_chord: i32,
    /// the musician's Ample switch (endpoint 37)
    ample_mode: i32,
    /// Indiginus: 0 our own strums, 1 the library's chord keys (endpoint 40)
    pluck_chords: i32,
    /// the musician's "Ample chords" switch (endpoint 39): 0 Chord Detect, 1 Ample's 12 chord slots
    ample_chords: i32,
    /// the song's chords (setSong)
    ample_song_chords: AmpleSongChords,
    /// ... on Ample's slots, for this key and scale
    ample_slot_table: AmpleSlotTable,
    /// the key / scale ampleSlotTable was built for; -1 = build again
    ample_slots_sig: i32,
    /// the musician's Groove slider (endpoint 38): 50 = the player's groove as written (Natural feel)
    groove_amt: f32,
    /// a groove audition's value (-1 = the slider)
    groove_override: f32,
    // ujamReset also runs after the target has changed away from UJAM (or the Library was
    // reloaded), so it uses the stop key and mod CC that were in force when the phrase was sent.
    ujam_stop_key: i32,
    ujam_mod_cc: i32,

    // ---- Ample Sound guitars (ampleGuitar) ------------------------------------------------------
    /// the setup Ample was sent (target, mode, position, open strings); -1 = send again
    ample_sig: i32,
    /// notes: the articulation key latched in Ample
    ample_art: i32,
    /// Strummer: what chord detect was last sent (root key, tones, bass); -1 = send again
    ample_chord_sent: i32,
    /// Strummer: strokes wait until chord detect is in
    ample_ready_at: f64,
    /// notes: the last single note (for hammer-ons and slides); -1 after a chord
    ample_prev_note: i32,
    ample_prev_on: f64,
    /// ... and the ones before it, so steps made again (regenerateFrom) can step back to what's still standing.
    ample_hist: [(i32, f64); 8],
    ample_hist_count: i32,
    ample_notes: [AmpleNote; 16],
    ample_count: i32,
    // Strummer, Natural (spec §10.3): legato across a chord change, slide-in and harmonic strums.
    /// the bar that last got a legato key (one per bar)
    ample_legato_bar: i32,
    /// the last stroke (or single string), as played
    ample_last_stroke_at: f64,
    /// a section's first stroke, or one after half a bar of rest
    ample_slide_in_armed: bool,
    /// this step is the Ending's final hit
    ample_final_hit: bool,
    /// a chord change waiting for its first stroke to decide on legato
    ample_legato_pending: bool,
    /// ... its detect keys' time
    ample_legato_chord_at: f64,
    /// ... and the legato key pressed for it
    ample_legato_key: i32,
    // Notes, Natural (spec §10.4): slide-outs (one per two bars), the pickup's scratch (one per bar), vibrato (CC1).
    ample_slide_out_bar: i32,
    ample_slide_out_prev_bar: i32,
    ample_slide_out_at: f64,
    ample_scratch_bar: i32,
    ample_scratch_at: f64,
    /// the last CC1 swell: its CC1 > 0 and its CC1 0
    ample_vib_start: f64,
    ample_vib_end: f64,
    /// the step ampleBegin last started
    ample_step_at: f64,
    /// where the chord next changes (as played: onset lead applied); -1 = not in this chord
    ample_next_chord_at: f64,
    /// Notes mode: the last chord shape fingered (GuitarShapes.h)
    guitar_prev: GShape,
    art_dyn_cc_sent: i32,
    art_expr_cc_sent: i32,
    swam_expr_cc_sent: i32,
    swam_vib_cc_sent: i32,
    swam_ks_offset_sent: i32,
    // A solo library's own keys (Embertone): one latched articulation (sustain / staccato / pizzicato)
    // re-sent only when it changes, and instant ones (tremolo, harmonic) held for as long as the note asks.
    custom_latch: i32,
    expr_used: bool,

    transport_slot: i32,
    host_playing: bool,
    transport_seen: bool,
    raw_ppq_seen: bool,
    awaiting_start_ppq: bool,
    host_tempo: f32,
    time_numerator: i32,
    time_denominator: i32,
    /// a numerator change waits for the denominator (same packet)
    meter_changed: bool,
    current_ppq: f64,
    previous_raw_ppq: f64,
    last_emitted_step: i32,
    last_display_step: i32,

    active_slots: CArr<bool, 64>,
    active_notes: CArr<i32, 64>,
    active_channels: CArr<i32, 64>,
    active_off_ppq: CArr<f64, 64>,
    /// -1 = releaseVelocity(); else as recorded
    active_off_vel: CArr<i32, 64>,
    /// the velocity it was struck at (a half-bar re-voice strikes the new chord from it)
    active_vel: CArr<i32, 64>,
    /// Indiginus latch / same: the press tap's note-off (5 ms), -1 = sent
    active_tap_off: CArr<f64, 64>,
    /// stopAllNotes: nothing queued survives it (return taps go now)
    in_stopping: bool,

    // Notes gathered for one step before timing/velocity shaping.
    step_count: i32,
    step_note: CArr<i32, 16>,
    step_vel: CArr<f32, 16>,
    step_len: CArr<f32, 16>,
    /// 0 = left / bass, 1 = right / melody
    step_hand: CArr<i32, 16>,
    /// extra per-note offset (grace notes)
    step_shift_ms: CArr<f32, 16>,

    /// An offline render reports what was decided (the Band view's marks): once per kind per bar, never in a look-ahead.
    annotated_bar: [i32; 4],

    // ---- Note output ---------------------------------------------------------
    // Keyswitch note sends are logged with status + 0x100 so an export can leave them out.
    log_flag: i32,

    // Offline render: the engine runs ahead of real time into logOut only, throttled so the host's
    // per-block event queue never overflows. Live playback is paused while it runs.
    rendering: bool,
    render_bars: i32,
    render_budget: i32,
    render_refill: i32,
    render_end: f64,
    /// kept takes (5d): owned by the Band (a no-op here)
    capture: Option<Capture>,

    // Controller numbers / keys in force when they were sent, for the releases above.
    kb_ghost_sent: i32,
    kb_palm_cc_sent: i32,
    soft_pedal_cc_sent: i32,
    // Held keyswitches release the exact note they sent: Keyswitch Octave may have changed since
    // (the original released through ks() with the new octave and stranded the key).
    kb_ghost_note_sent: i32,
    swam_trem_note: i32,
    swam_harm_note: i32,

    // ---- Natural feel -----------------------------------------------------------------------------------------
    // Pushes: the step on the last 8th before a chord change plays the next chord (pushNow, read once by
    // performStep), and the downbeat it anticipated isn't struck again (pushedAt, until that step is made).
    push_now: bool,
    pushed_at: f64,
    pushed_from: f64,
    push_ring_to: f64,
    /// the chord a push anticipated; a regenerate re-checks it at the downbeat
    pushed_chord: Chord,
    push_now_chord: Chord,
    push_verify: bool,
    // A dry run makes a step's notes without playing them (the downbeat a push brings forward).
    dry_run: bool,
    dry_count: i32,
    dry_note: CArr<i32, 16>,
    dry_hand: CArr<i32, 16>,
    dry_flags: CArr<i32, 16>,
    dry_vel: CArr<f32, 16>,
    dry_len: CArr<f32, 16>,
    dry_shift: CArr<f32, 16>,

    // Phrasing (Natural, song path): this step's pattern bar varied / thinned, or the section's last bar leading in.
    /// 0 as written, 1 borrow the bar from Energy +-1, 2 the other bar of the A/B pair, 3 thinned
    phrase_mode: i32,
    /// the bar before a new section: the player's fill
    pickup_now: bool,
    /// ... for that bar only (a crash after it asks about the bar before)
    pickup_prog_bar: i32,

    // ---- The fiddler's vocabulary (Natural, The Fiddle and the Embertone): licks made as steps ---------------
    lick_now: LickMark,
    // A kick-off's last note lands on the new section's downbeat: which lick, where, and the last pickup step that
    // decided it. lickLandWhich kTurnaroundLick: a turnaround's landing.
    lick_land_at: f64,
    lick_land_set_at: f64,
    lick_land_which: i32,
    // The bar start of the last fill (and the one before it, for a regeneration that remakes the last): at most one
    // per 4 bars. Forgotten on a reset and a host seek; a fill bar after the play position is a seek back.
    lick_fill_at: f64,
    lick_fill_prev_at: f64,

    // ---- Orange Tree Evolution (evolution; spec 2026-10-07 section 3) ------------------------------------------
    ev_notes: [AmpleNote; 16],
    ev_count: i32,
    ev_held: HeldKey,
    /// the owner's pitch (a trimmed owner moves its key's release)
    ev_held_pitch: i32,
    /// a held key released early (regeneration): not re-pressed before it is up
    ev_up_key: i32,
    ev_up_at: f64,
    /// the last single note (hammers and slides start from it); -1 after a chord
    ev_prev: EvPrev,
    /// ... and the ones before it, for a regeneration to step back to
    ev_hist: [EvPrev; 8],
    ev_hist_count: i32,
    /// the last CC1 swell: its first CC1 > 0 and its CC1 0
    ev_vib_start: f64,
    ev_vib_end: f64,
    // Task 3 (vocabulary).
    /// the held key is a section's (palm mute): held across its notes
    ev_sec_held: bool,
    ev_sec_id: i32,
    /// a roll / a clawhammer beat owns the steps starting before evOwnUntil
    ev_own_from: f64,
    ev_own_until: f64,
    /// a tremolo played as repeated notes (no tremolo keys): its strokes go out step by step
    ev_trem: EvTrem,
    /// the chord start that played a section's tremolo chord (its later steps are its)
    ev_trem_chord_at: f64,
    /// the section whose first tremolo chord was played (its "in" key), and where
    ev_trem_sec: i32,
    ev_trem_first_at: f64,
    /// harmonics: once per section
    ev_harm_sec: i32,
    ev_harm_at: f64,
    /// the last blues bend's return to centre
    ev_bend_end: f64,
    /// the phrase-start bar whose first melody note has been (a grace's chance)
    ev_phrase_bar: f64,

    // ---- Task 4: Indiginus The Banjo / Mandolin / Ukulele / Resonator (indiginusStrings; spec section 4) ----------
    /// the pass whose setup keys went, and when
    in_init_pass: i32,
    in_init_at: f64,
    /// notes wait for the setup keys (30 ms)
    in_ready_at: f64,
    /// the last note onset CC11 was decided for, the last CC11 sent
    in_expr: ExprCover,
    /// slots mode: the chord (root, quality) the library holds, when it was set
    in_slot_sig: i32,
    in_slot_at: f64,

    /// Natural: where the sustain pedal was left (-1 = not known)
    pedal_level: i32,
    /// the harmonic pedal's CC as last pressed (-1 = never)
    harmonic_cc_sent: i32,
    /// ... and whether it is down now
    harmonic_held: bool,
    /// where its queued lift lands
    harmonic_off_at: f64,

    // ---- Hammond B-3X (organ driver, spec section 14) ---------------------------------------------------------
    /// this step: loudest shaped velocity, first note-on
    og_loud: f32,
    og_first_on: f64,
    /// this step's keys so far (folding can double one)
    og_folded: CArr<i32, 16>,
    og_folded_n: i32,
    /// the step's Energy before the section's (for the next one's)
    og_base_energy: i32,
    og_nudge: i32,
    /// the swell as last scheduled
    og_expr_level: i32,
    og_expr_at: f64,
    /// ... and as last sent
    og_expr_fired: i32,
    og_expr_fired_at: f64,
    /// registration (Energy 1-4) scheduled / sent
    og_reg_sent: i32,
    og_reg_fired: i32,
    // The lower manual (vocabulary 2.5): its registration (Energy 1-4) scheduled / sent, the channel it was played on
    // (0-based, -1 none: stop silences it), this step's lower keys and first lower note-on.
    og_lower_reg_sent: i32,
    og_lower_reg_fired: i32,
    og_lower_ch_used: i32,
    og_low_folded: CArr<i32, 16>,
    og_low_folded_n: i32,
    og_low_first_on: f64,
    /// Leslie / percussion CC values
    og_leslie: i32,
    og_leslie_fired: i32,
    og_perc: i32,
    og_perc_fired: i32,
    /// the section whose start was played
    og_sec_key: i64,
    og_sec_key_prev: i64,
    og_sec_at: f64,
    /// the section whose last bar was looked at
    og_early_key: i64,
    og_early_key_prev: i64,
    og_early_at: f64,
    og_chorus_pass: i32,
    og_chorus_pass_prev: i32,
    og_chorus_at: f64,
    // Natural Leslie kicks (vocabulary 2.3): the bar of the last kick (and before it), where its CC went, when the
    // return to slow is due (-1 none), and the return as last sent (re-armed when a regeneration drops it).
    og_kick_bar: i64,
    og_kick_bar_prev: i64,
    og_kick_at: f64,
    og_kick_back: f64,
    og_kick_back_armed: f64,
    og_kick_back_sent_at: f64,
    // Palm smears (vocabulary 2.4): the bar (pass * 65536 + bar) whose smear was scheduled and where its chord lands;
    // after a regeneration cut inside it, the smear is decided again but keeps what was already before the cut.
    og_smear_bar: i64,
    og_smear_skip_for: i64,
    og_smear_end: f64,
    og_smear_skip_before: f64,
    /// The end of the last Break's rest bar.
    og_break_end: f64,

    // ---- Task 6: Indiginus Blue Street Brass (brassSection; spec section 5) ----------------------------------------
    br_notes: [BrNote; 16],
    br_n: i32,
    br_setup_pass: i32,
    br_setup_at: f64,
    br_ready_at: f64,
    /// legato key state scheduled / sent (-1 unknown, 0 off, 1 on)
    br_leg: i32,
    br_leg_fired: i32,
    /// CC1 as last scheduled / sent
    br_cc_level: i32,
    br_cc_fired: i32,
    br_cc_at: f64,
    br_cc_fired_at: f64,
    br_prev_note: i32,
    br_prev_on: f64,
    br_prev_end: f64,
    br_prev_out: i32,
    br_prev_leg: bool,
    br_phrase_leg: bool,
    br_phrase_sixth: bool,

    // ---- MODO BASS 2 in Natural ---------------------------------------------------------------------------------
    /// ORed into every step note (bass fills)
    step_extra_flags: i32,
    last_bass_pitch: i32,
    last_bass_root: i32,
    last_bass_step_ppq: f64,
    legato_cc_sent: i32,
    legato_on: bool,
    bass_vib_cc_sent: i32,
    bass_ring_cc_sent: i32,
    bass_vib_on: bool,
    bass_ring_on: bool,
    wheel_bent: bool,
    /// this step's first note: the next note in either lane (or step 12, a fill)
    bass_next_at: f64,
    /// this step's vibrato depth (naturalBassChoices -> naturalBassNote)
    bass_vib_depth: i32,
    /// the bar the last note was in; the bar that rings
    ring_bar: i32,
    ring_active_bar: i32,
    /// the section (first entry, pass) whose position was set
    pos_section_at: i32,
    pos_section_pass: i32,
    /// sent with the section's first note
    pos_key_pending: i32,
    pluck_pending: i32,
    pos_cc_pending: i32,
    /// ... where they were queued
    pos_sent_at: f64,

    // ---- Drums ---------------------------------------------------------------------
    /// A hat bark on an "and of 4": the step it was made on (-1 none), closed by the pedal hat on the next downbeat.
    dr_bark_from: f64,
    /// The bar (as heard) in which the ride was last played: the pedal hat keeps time under it.
    dr_ride_bar: i32,
    /// 3/4: the bar's downbeat kick/snare velocity (the light beats never go over it)
    dr_down_vel: f32,
    dr_down_bar: i32,
}

// Intent flags (shared with stepFlags): 32 pizzicato, 64 tremolo, 128 fall, 256 doit, 512 harmonics, 1024 staccato
// The instrument ranges (were swamLow / swamHigh, indexed by the global 0-12 SWAM instrument) are the
// swam target's data now, looked up by instrument name.
pub(crate) const K_SWAM_NAMES: [&str; 13] =
    ["Violin", "Viola", "Cello", "Double Bass", "Trumpet", "Trombone", "Horn", "Tuba", "Flute", "Clarinet", "Oboe", "Bassoon", "Alto Sax"];

// Natural on Ample (spec §10.1-§10.2). Classic: no lead, no flags, byte-identical.
pub(crate) const K_AMPLE_MUTED_FLAG: i32 = 1 << 21;
pub(crate) const K_AMPLE_CHANGE_FLAG: i32 = 1 << 22;
pub(crate) const K_AMPLE_CHORD_KEY_FLAG: i32 = 1 << 23;

/// a kept take replaying: the note as recorded, nothing added
pub(crate) const K_RAW_FLAG: i32 = 16384;
// Indiginus articulation keys in the latch / same return modes (with 8192): tapped at the press, the return tapped at
// the release. (bits 21-22 are Ample's; bit 2 was free, the sign bit caused two bugs)
pub(crate) const K_IN_LATCH_FLAG: i32 = 1 << 15;
pub(crate) const K_IN_SAME_FLAG: i32 = 1 << 2;
// MODO BASS 2 in Natural: what the bass step decided for a note (performStep sends the keys / CCs around it).
pub(crate) const K_SLIDE_FLAG: i32 = 1 << 16;
pub(crate) const K_HAMMER_FLAG: i32 = 1 << 17;
pub(crate) const K_STOP_FLAG: i32 = 1 << 18;
pub(crate) const K_FILL_FLAG: i32 = 1 << 19;
pub(crate) const K_DOWN_FLAG: i32 = 1 << 20;
// ... a fall (pitch wheel), vibrato (CC1), let ring from this note (CC64 127) or through it (the bar's ring goes on).
pub(crate) const K_FALL_FLAG: i32 = 1 << 24;
pub(crate) const K_VIB_FLAG: i32 = 1 << 25;
pub(crate) const K_RING_FLAG: i32 = 1 << 26;
pub(crate) const K_RING_MORE_FLAG: i32 = 1 << 27;
// MODO DRUM in Natural: the right hand plays this hit (sticking, a flam's main stroke), a flam's grace goes before it,
// the hi-hat aperture CC goes before it. Never sent on: performStep strips them.
pub(crate) const K_DRUM_RIGHT_FLAG: i32 = 1 << 28;
pub(crate) const K_FLAM_FLAG: i32 = 1 << 29;
pub(crate) const K_APERTURE_FLAG: i32 = 1 << 30;
pub(crate) const K_DRUM_FLAGS: i32 = K_DRUM_RIGHT_FLAG | K_FLAM_FLAG | K_APERTURE_FLAG;

/// Degree -> diatonic root of the Idea's authored chord, bar by bar.
pub(crate) const AUTHORED_ROOTS: CArr<i32, 16> = CArr::new([0, 5, 3, 4, 0, 3, 5, 4, 5, 3, 0, 4, 0, 1, 3, 4]);

/// lickLandWhich: a turnaround's landing
pub(crate) const K_TURNAROUND_LICK: i32 = 100;
pub(crate) const K_OG_NONE: i64 = -(1i64 << 40);

impl Musician {
    /// `Musician (defaults, library)`: the Library is not a member (see [`Ctx`]).
    pub(crate) fn new(d: MusicianDefaults) -> Musician {
        Musician {
            ui_playing: false,
            ui_section: 0,
            ui_ordinal: 0,
            index: 0,
            meter: Meter::M44,
            pending_meter: Meter::M44,
            host_meter_unsupported: false,
            last_sib_seen: -1,
            bar_origin: 0.0,
            busy_counted_ppq: -1.0e9,
            ui_tempo: 120.0,
            ui_pulse: 0,
            ui_bar: 0,
            d,
            seed_override: -1,
            sample_rate: 48000.0,
            frame_offset: 0,
            variation: d.idea,
            pattern: d.instrument,
            key_root: 0,
            scale_mode: 0,
            octave_shift: d.octave,
            speed: d.speed,
            velocity: d.velocity,
            length: d.length,
            enabled: d.on,
            muted: false,
            midi_channel0: d.channel,
            player: d.player,
            energy: d.energy,
            progression: 17,
            bars_mode: 0,
            take: 1,
            slots: CArr::new([0, 3, 80, 48, 0, 3, 80, 48]),
            swing_amt: d.swing,
            humanize: d.humanize,
            dynamics: d.dynamics,
            fills: d.fills,
            speed_knob: d.speed,
            step_shift: FeelShift::default(),
            respect: d.respect,
            target: d.target,
            ks_octave: d.ks,
            mute_cc: -1,
            style_ks: -1,
            ghost_held: 0,
            strike_last_vel: 0.0,
            strike_last_ppq: -1.0e9,
            soft_pedal: -1,
            q: Scheduler::default(),
            emit_off_vel: -1,
            emit_ch: 0,
            active_flags: CArr::default(),
            step_flags: CArr::default(),
            swam_inst: d.swam_inst,
            swam_voice: d.voice,
            expr_level: -1,
            vib_level: -1,
            swam_play_mode: -1,
            swam_trem: 0,
            swam_harm: 0,
            swam_note: -1,
            swam_note_off: 0.0,
            jb_sustain_on: false,
            jb_run_end: -1.0,
            jb_port_bar: -1000,
            jb_prev_end: false,
            jb_prev_fast: false,
            jb_partner: -1,
            jb_prev_on: -1.0,
            jb_lens: CArr::default(),
            jb_lens_n: 0,
            jb_sustain_sent: false,
            jb_rel_floor: -1.0e18,
            jb_stray_down: false,
            jb_sub_on: CArr::default(),
            jb_subs: 0,
            jb_hist: CArr::default(),
            jb_hist_n: 0,
            fd_prev_note: -1,
            fd_prev_on: -1.0,
            fd_prev_off: -1.0,
            fd_last_orn: false,
            fd_orn_beat: -1,
            fd_prev_end: false,
            fd_prev_fast: false,
            fd_chop_dip: false,
            fd_partner: -1,
            fd_ret: ReturnMode::default(),
            fd_sub_pitch: CArr::default(),
            fd_sub_on: CArr::default(),
            fd_subs: 0,
            sounding_since: -1.0,
            last_note_end: -1.0,
            note_intent: 0,
            pattern_meets_bar: true,
            chord_bars_mode: 2,
            cur: None,
            cur_index: 0,
            cur_prog_bar: 0,
            cur_global: 0,
            cur_start_ppq: 0.0,
            cur_end_ppq: 0.0,
            perf: PerformanceState::default(),
            live_entries: [TimelineEntry::default(); 3],
            hold_step: false,
            step_ppq_now: 0.0,
            bar_seen: -1_000_000,
            bar_notes: 0,
            bar_note_sum: 0.0,
            hold_len: 0.0,
            live_mode: false,
            perf_fill: false,
            perf_crash: false,
            force_phrase_end: false,
            stop_hit: false,
            ten_now: 0.0,
            ten_ramp: 0.0,
            ten_tail: false,
            rel_down: false,
            jump_next_idx: -1,
            step_pass: 0,
            step_prog_bar: 0,
            song_silent: false,
            silence_at: -1.0,
            crow_art: -1,
            crow_dyn: -1,
            crow_expr: -1,
            ks_art_sent: -1,
            ks_dyn: -1,
            ks_expr: -1,
            ks_dyn_sent: -1,
            ks_expr_sent: -1,
            ks_init_pass: -1,
            ks_hold_key: -1,
            ks_same_key: -1,
            ks_ret: ReturnMode::default(),
            ks_vib_until: -1.0e18,
            ks_vib_start: -1.0e18,
            ks_hold_at: -1.0e18,
            ks_hold_until: -1.0e18,
            ks_hold_owner_on: -1.0e18,
            ks_hold_owner_end: -1.0e18,
            ks_same_at: -1.0e18,
            ks_same_end_at: -1.0e18,
            ks_ret_fire_at: -1.0e18,
            ks_init_at: -1.0e18,
            ks_hold_hist: [KsHoldStep::default(); 8],
            ks_hold_hist_n: 0,
            art_map: d.art_map,
            ujam_phrase: -1,
            ujam_cc1: -1,
            ujam_last_chord: -1,
            ample_mode: 0,
            pluck_chords: 0,
            ample_chords: 0,
            ample_song_chords: AmpleSongChords::default(),
            ample_slot_table: AmpleSlotTable::default(),
            ample_slots_sig: -1,
            groove_amt: 50.0,
            groove_override: -1.0,
            ujam_stop_key: 35,
            ujam_mod_cc: 1,
            ample_sig: -1,
            ample_art: -1,
            ample_chord_sent: -1,
            ample_ready_at: -1.0e9,
            ample_prev_note: -1,
            ample_prev_on: -1.0e9,
            ample_hist: [(0, 0.0); 8],
            ample_hist_count: 0,
            ample_notes: [AmpleNote::default(); 16],
            ample_count: 0,
            ample_legato_bar: -1000,
            ample_last_stroke_at: -1.0e9,
            ample_slide_in_armed: false,
            ample_final_hit: false,
            ample_legato_pending: false,
            ample_legato_chord_at: 0.0,
            ample_legato_key: -1,
            ample_slide_out_bar: -1000,
            ample_slide_out_prev_bar: -1000,
            ample_slide_out_at: -1.0e9,
            ample_scratch_bar: -1000,
            ample_scratch_at: -1.0e9,
            ample_vib_start: -1.0e9,
            ample_vib_end: -1.0e9,
            ample_step_at: 0.0,
            ample_next_chord_at: -1.0,
            guitar_prev: GShape::default(),
            art_dyn_cc_sent: -1,
            art_expr_cc_sent: 11,
            swam_expr_cc_sent: 11,
            swam_vib_cc_sent: 1,
            swam_ks_offset_sent: 24,
            custom_latch: -1,
            expr_used: false,
            transport_slot: 0,
            host_playing: false,
            transport_seen: false,
            raw_ppq_seen: false,
            awaiting_start_ppq: true,
            host_tempo: 120.0,
            time_numerator: 4,
            time_denominator: 4,
            meter_changed: false,
            current_ppq: 0.0,
            previous_raw_ppq: 0.0,
            last_emitted_step: NO_STEP,
            last_display_step: NO_STEP,
            active_slots: CArr::default(),
            active_notes: CArr::default(),
            active_channels: CArr::default(),
            active_off_ppq: CArr::default(),
            active_off_vel: CArr::default(),
            active_vel: CArr::default(),
            active_tap_off: CArr::default(),
            in_stopping: false,
            step_count: 0,
            step_note: CArr::default(),
            step_vel: CArr::default(),
            step_len: CArr::default(),
            step_hand: CArr::default(),
            step_shift_ms: CArr::default(),
            annotated_bar: [-1, -1, -1, -1],
            log_flag: 0,
            rendering: false,
            render_bars: 0,
            render_budget: 0,
            render_refill: 0,
            render_end: 0.0,
            capture: None,
            kb_ghost_sent: 22,
            kb_palm_cc_sent: 9,
            soft_pedal_cc_sent: 67,
            kb_ghost_note_sent: 22,
            swam_trem_note: 32,
            swam_harm_note: 30,
            push_now: false,
            pushed_at: -1.0,
            pushed_from: -1.0,
            push_ring_to: -1.0,
            pushed_chord: Chord::default(),
            push_now_chord: Chord::default(),
            push_verify: false,
            dry_run: false,
            dry_count: 0,
            dry_note: CArr::default(),
            dry_hand: CArr::default(),
            dry_flags: CArr::default(),
            dry_vel: CArr::default(),
            dry_len: CArr::default(),
            dry_shift: CArr::default(),
            phrase_mode: 0,
            pickup_now: false,
            pickup_prog_bar: -1,
            lick_now: LickMark::default(),
            lick_land_at: -1.0,
            lick_land_set_at: -1.0,
            lick_land_which: -1,
            lick_fill_at: -1.0,
            lick_fill_prev_at: -1.0,
            ev_notes: [AmpleNote::default(); 16],
            ev_count: 0,
            ev_held: HeldKey::default(),
            ev_held_pitch: -1,
            ev_up_key: -1,
            ev_up_at: -1.0e9,
            ev_prev: EvPrev::default(),
            ev_hist: [EvPrev::default(); 8],
            ev_hist_count: 0,
            ev_vib_start: -1.0e9,
            ev_vib_end: -1.0e9,
            ev_sec_held: false,
            ev_sec_id: -1,
            ev_own_from: -1.0e9,
            ev_own_until: -1.0e9,
            ev_trem: EvTrem::default(),
            ev_trem_chord_at: -1.0e9,
            ev_trem_sec: -1,
            ev_trem_first_at: -1.0e9,
            ev_harm_sec: -1,
            ev_harm_at: -1.0e9,
            ev_bend_end: -1.0e9,
            ev_phrase_bar: -1.0e9,
            in_init_pass: -1,
            in_init_at: -1.0e9,
            in_ready_at: -1.0e18,
            in_expr: ExprCover::default(),
            in_slot_sig: -1,
            in_slot_at: -1.0e18,
            pedal_level: -1,
            harmonic_cc_sent: -1,
            harmonic_held: false,
            harmonic_off_at: -1.0,
            og_loud: 0.0,
            og_first_on: 1.0e18,
            og_folded: CArr::default(),
            og_folded_n: 0,
            og_base_energy: 3,
            og_nudge: 0,
            og_expr_level: -1,
            og_expr_at: -1.0e18,
            og_expr_fired: -1,
            og_expr_fired_at: -1.0e18,
            og_reg_sent: -1,
            og_reg_fired: -1,
            og_lower_reg_sent: -1,
            og_lower_reg_fired: -1,
            og_lower_ch_used: -1,
            og_low_folded: CArr::default(),
            og_low_folded_n: 0,
            og_low_first_on: 1.0e18,
            og_leslie: -1,
            og_leslie_fired: -1,
            og_perc: -1,
            og_perc_fired: -1,
            og_sec_key: K_OG_NONE,
            og_sec_key_prev: K_OG_NONE,
            og_sec_at: -1.0e18,
            og_early_key: K_OG_NONE,
            og_early_key_prev: K_OG_NONE,
            og_early_at: -1.0e18,
            og_chorus_pass: -1_000_000,
            og_chorus_pass_prev: -1_000_000,
            og_chorus_at: -1.0e18,
            og_kick_bar: -1_000_000,
            og_kick_bar_prev: -1_000_000,
            og_kick_at: -1.0e18,
            og_kick_back: -1.0,
            og_kick_back_armed: -1.0,
            og_kick_back_sent_at: -1.0e18,
            og_smear_bar: K_OG_NONE,
            og_smear_skip_for: K_OG_NONE,
            og_smear_end: -1.0e18,
            og_smear_skip_before: -1.0e18,
            og_break_end: -1.0e18,
            br_notes: [BrNote::default(); 16],
            br_n: 0,
            br_setup_pass: -1_000_000,
            br_setup_at: -1.0e18,
            br_ready_at: -1.0e18,
            br_leg: -1,
            br_leg_fired: -1,
            br_cc_level: -1,
            br_cc_fired: -1,
            br_cc_at: -1.0e18,
            br_cc_fired_at: -1.0e18,
            br_prev_note: -1,
            br_prev_on: -1.0e18,
            br_prev_end: -1.0e18,
            br_prev_out: -1,
            br_prev_leg: false,
            br_phrase_leg: false,
            br_phrase_sixth: false,
            step_extra_flags: 0,
            last_bass_pitch: -1,
            last_bass_root: -1,
            last_bass_step_ppq: -1.0e9,
            legato_cc_sent: -1,
            legato_on: false,
            bass_vib_cc_sent: -1,
            bass_ring_cc_sent: -1,
            bass_vib_on: false,
            bass_ring_on: false,
            wheel_bent: false,
            bass_next_at: 1.0e18,
            bass_vib_depth: 0,
            ring_bar: NO_STEP,
            ring_active_bar: NO_STEP,
            pos_section_at: -1,
            pos_section_pass: -1,
            pos_key_pending: -1,
            pluck_pending: -1,
            pos_cc_pending: -1,
            pos_sent_at: -1.0e9,
            dr_bark_from: -1.0,
            dr_ride_bar: -1,
            dr_down_vel: 0.0,
            dr_down_bar: -1,
        }
    }

    pub(crate) fn prepare(&mut self, new_sample_rate: f64) {
        self.sample_rate = new_sample_rate;
    }

    // `setIdleSink`: the idle sink is an argument (`idle`) of every out-of-frame entry point.

    pub(crate) fn spb(&self) -> i32 {
        info(self.meter).steps_per_bar
    }
    // The only places that index a 16-step row. 4/4: the step itself; 12-step meters: map16 / accentSrc, unless the
    // player has native grids12 for this meter (then the step itself: gridAt reads grids12 at bar * 16 + sib, sib < 12).
    // gridStep may return -1 (no source hit for this role at this 6/8 step).
    pub(crate) const fn meter_bit12(m: Meter) -> u8 {
        match m {
            Meter::M68 => 1,
            Meter::M34 => 2,
            Meter::M44 => 0,
        }
    }
    /// This player plays its own grids12 in the current meter (no mapping).
    pub(crate) fn native_here(&self, who: i32, lib: &LibraryData) -> bool {
        if who <= 0 || who > K_MAX_SEED as i32 {
            return false;
        }
        let Some(p) = lib.persona(who) else { return false };
        p.native12 && (p.meters12 & Self::meter_bit12(self.meter)) != 0
    }
    /// A 12-only player outside its meters (4/4 above all): it sits out, silent, never mapped back to 16 steps.
    pub(crate) fn sits_out_meter(&self, who: i32, lib: &LibraryData) -> bool {
        who > 0 && who <= K_MAX_SEED as i32 && lib.persona(who).is_some_and(|p| p.only12) && !self.native_here(who, lib)
    }
    pub(crate) fn grid_step(&self, who: i32, sib: i32, r: GridRole, lib: &LibraryData) -> i32 {
        if self.meter == Meter::M44 || self.native_here(who, lib) { sib } else { map16(self.meter, sib, r) }
    }
    /// gridStep plus the bar-context rules (spec 3.1 / 3.2); `has (s)` says whether the bar's source has a hit at
    /// 16-step s. Merges are "first hit wins". 4/4: the step itself. (Drums have their own lane rules: drumSrc12.)
    pub(crate) fn grid_step_in<F: Fn(i32) -> bool>(&self, who: i32, sib: i32, r: GridRole, has: F, lib: &LibraryData) -> i32 {
        let g = self.grid_step(who, sib, r, lib);
        if self.native_here(who, lib) {
            return g;
        }
        if self.meter == Meter::M34 {
            if r == GridRole::Bass && sib == 10 && !has(10) && has(12) {
                return 12; // the beat-4 approach survives on 10
            }
            if r == GridRole::Comp && sib == 8 && !has(8) && has(12) {
                return 12; // pah-pah: beat 4's chord on beat 3
            }
        } else if self.meter == Meter::M68 {
            let b = (sib / 6) * 8;
            let t = sib % 6;
            if r == GridRole::Comp && t == 2 && !has(b + 2) && has(b + 4) {
                return b + 4; // the source's beat chord on the eighth
            }
            if r == GridRole::Melodic && t == 0 && !has(b) && has(b + 1) {
                return b + 1; // floor (s * 6 / 8) collisions
            }
            if r == GridRole::Melodic && t == 3 && !has(b + 4) && has(b + 5) {
                return b + 5;
            }
        }
        g
    }
    pub(crate) fn accent_at(&self, who: i32, sib: i32, lib: &LibraryData) -> f32 {
        let f = self.feel_for(who, lib);
        let a = &f.accents;
        if self.meter != Meter::M44 && self.native_here(who, lib) {
            return at_or_default(&f.accents12, sib);
        }
        if self.meter == Meter::M44 { at_or_default(a, sib) } else { at_or_default(a, accent_src(self.meter, sib)) * accent12_scale(self.meter, sib) }
    }
    pub(crate) fn timing_at(&self, who: i32, sib: i32, lib: &LibraryData) -> f32 {
        let f = self.feel_for(who, lib);
        let t = &f.groove.timing;
        if self.meter != Meter::M44 && self.native_here(who, lib) {
            return at_or_default(&f.groove.timing12, sib);
        }
        if self.meter == Meter::M44 { at_or_default(t, sib) } else { at_or_default(t, accent_src(self.meter, sib)) }
    }
    /// The fill zone's first step: 4/4 beat 4; 6/8 the last eighth group (the whole 2nd dotted beat at Energy 4); 3/4 beat 3.
    pub(crate) fn fill_zone(&self) -> i32 {
        if self.meter == Meter::M68 && self.energy >= 4 { half_step(self.meter) } else { fill_zone_start(self.meter) }
    }
    /// The flag: this musician's player is 12-only and the meter isn't one of its own (it plays nothing).
    pub(crate) fn sits_out(&self, lib: &LibraryData) -> bool {
        self.sits_out_meter(self.active_player(lib), lib)
    }
    pub(crate) fn bar_length(&self) -> f32 {
        self.get_bar_length_ppq()
    }
    /// The song's override, else the host's meter (and whether the host's is unsupported).
    pub(crate) fn effective_meter(&self, unsupported: &mut bool, cx: &Ctx<'_>) -> Meter {
        let (h, u) = meter_from_host(self.time_numerator, self.time_denominator);
        *unsupported = u;
        match cx.song {
            Some(s) if s.meter_override >= 0 => Meter::from_index(s.meter_override),
            _ => h,
        }
    }
    pub(crate) fn latch_meter(&mut self, cx: &Ctx<'_>) {
        let mut u = false;
        self.meter = self.effective_meter(&mut u, cx);
        self.pending_meter = self.meter;
        self.host_meter_unsupported = u;
        self.bar_origin = 0.0;
    }
    pub(crate) fn origin_step(&self, lib: &LibraryData) -> i32 {
        if self.bar_origin == 0.0 { 0 } else { (self.bar_origin / f64::from(self.tick_ppq(lib))).round() as i32 }
    }
    pub(crate) fn step_in_bar(&self, step: i32, lib: &LibraryData) -> i32 {
        self.positive_modulo(step.wrapping_sub(self.origin_step(lib)), self.spb())
    }
    pub(crate) fn bar_floor(&self, p: f64, l: f64) -> f64 {
        self.bar_origin + ((p - self.bar_origin) / l + 1.0e-9).floor() * l
    }
    pub(crate) fn note_meter(&mut self, cx: &Ctx<'_>) {
        let mut u = false;
        self.pending_meter = self.effective_meter(&mut u, cx);
    }
    /// Not inside a Break (it rests its bar, then the new meter starts): the Performer waits the same way.
    pub(crate) fn meter_switch_due(&self, step: i32, lib: &LibraryData) -> bool {
        self.pending_meter != self.meter
            && self.on_heard_bar_line(step, lib)
            && !(self.perf.break_at >= 0.0 && f64::from(step) * f64::from(self.tick_ppq(lib)) < self.perf.break_at + self.perf.bar_len - 1.0e-6)
    }
    /// The step starts a heard bar (the meter's bar from barOrigin), whatever Speed makes of our own grid bar (Task 19).
    pub(crate) fn on_heard_bar_line(&self, step: i32, lib: &LibraryData) -> bool {
        let r = (f64::from(step) * f64::from(self.tick_ppq(lib)) - self.bar_origin) / f64::from(self.get_bar_length_ppq());
        (r - r.round()).abs() < 1.0e-6
    }
    /// A kept take this musician plays in that section (one kept in another meter is refused: not kept).
    pub(crate) fn kept_here(&self, sec: &SongSection) -> bool {
        let i = usize::try_from(clamp(self.index, 0, 5)).unwrap_or(0);
        sec.frozen.get(i).and_then(Option::as_ref).is_some_and(|fp| fp.meter == self.meter as u8)
    }

    /// `setSong (t)`. `cx.song` is the new timeline (None = the Amorph song endpoints: progression, bars, chord1..8,
    /// chordbars); `changed` = it is not the one this musician had (`t != song`), `had_song` = it had one
    /// (`song != nullptr`). No reset: an edit while playing is like a chord change.
    pub(crate) fn set_song(&mut self, changed: bool, had_song: bool, cx: &mut Ctx<'_>, idle: &mut dyn MidiSink) {
        let edited = changed && had_song && cx.song.is_some();
        if changed {
            self.ujam_last_chord = -1;
        }
        if changed {
            // Ample chord slots: the song's chords, collected here (off the audio thread in MS; no allocation here)
            let mut c = AmpleSongChords::default();
            if let Some(t) = cx.song {
                collect_ample_chords(&t.sections, &mut c);
            }
            self.ample_song_chords = c;
            self.ample_slots_sig = -1;
            if self.ample_chords == 1 {
                self.ample_chord_sent = -1;
            }
        }
        self.cur = None;
        if self.last_emitted_step == NO_STEP {
            self.latch_meter(cx); // stopped: the new song's meter now
        }
        self.apply_speed(false, cx, idle); // the song's feel Time (half / double)
        if edited && self.last_emitted_step != NO_STEP {
            self.regenerate_from(self.current_ppq, cx, idle); // steps made ahead from the old song are made again
        }
    }
    /// What the player is doing at the keyboard (Live chords, jumps, fills, breaks, energy, ending); all-zero = nothing.
    pub(crate) fn set_performance(&mut self, p: &PerformanceState, cx: &mut Ctx<'_>, idle: &mut dyn MidiSink) {
        // Steps are generated genAheadMs() ahead (40 ms; 150 ms for a smearing Natural organ). Anything that now lands
        // at or before a step already generated (a change played a hair before the line, a key just after it) gets
        // those steps generated again.
        let mut earliest = -1.0f64;
        let mut consider = |land: f64, changed: bool| {
            if changed && land >= 0.0 && (earliest < 0.0 || land < earliest) {
                earliest = land;
            }
        };
        let o = self.perf;
        consider(p.next_land, p.next_land != o.next_land || p.next != o.next);
        consider(p.jump_land, p.jump_land != o.jump_land || p.jump_entry != o.jump_entry);
        consider(p.energy_land, p.energy_land != o.energy_land || p.energy_next != o.energy_next);
        consider(p.fill_from, p.fill_from != o.fill_from);
        consider(p.break_at, p.break_at != o.break_at);
        consider(p.end_at, p.end_at != o.end_at);
        consider(p.wheel_land, p.wheel_land != o.wheel_land || p.wheel_next != o.wheel_next);
        consider(self.current_ppq, p.natural != o.natural); // Feel switched: steps ahead are made again
        consider(self.current_ppq, p.tension != o.tension || p.ab_on != o.ab_on || p.ab_a != o.ab_a || p.ab_b != o.ab_b);
        consider(p.ab_flip_at, p.ab_flip_at != o.ab_flip_at); // A/B: the next pass's Take
        if self.perf.natural && !p.natural {
            self.expression_off(cx, idle); // nothing Natural holds stays down in Classic
        }
        self.perf = *p;
        if earliest >= 0.0 && self.last_emitted_step != NO_STEP {
            self.regenerate_from(earliest, cx, idle);
        }
    }
    // `setShared`: the interplay record is `cx.shared`.
    /// A groove audition plays this musician at Groove g (-1 = its slider).
    pub(crate) fn set_groove_override(&mut self, g: f32) {
        self.groove_override = g;
    }
    pub(crate) fn set_capture(&mut self, c: Option<Capture>) {
        self.capture = c;
    }
    pub(crate) fn is_drums(&self) -> bool {
        Self::drums_pattern(self.pattern)
    }
    pub(crate) fn drums_pattern(p: i32) -> bool {
        p == 5 || p == 11
    }

    // Session tables (compiled in: the Session player is not a persona).
    pub(crate) fn offsets(i: i32) -> i32 {
        cref_at(&session_tables::OFFSETS, i)
    }
    pub(crate) fn degrees(i: i32) -> i32 {
        cref_at(&session_tables::DEGREES, i)
    }
    pub(crate) fn durations(i: i32) -> f32 {
        cref_at(&session_tables::DURATIONS, i)
    }
    pub(crate) fn velocities(i: i32) -> i32 {
        cref_at(&session_tables::VELOCITIES, i)
    }
    pub(crate) fn bases(i: i32) -> i32 {
        cref_at(&session_tables::BASES, i)
    }

    /// The feel row for persona `who` (0 = Session). A seed with no file plays as Session.
    pub(crate) fn feel_for<'a>(&self, who: i32, lib: &'a LibraryData) -> &'a PersonaData {
        if who > 0
            && who <= K_MAX_SEED as i32
            && let Some(p) = lib.persona(who)
        {
            return p;
        }
        session_feel()
    }
    /// The musician's target (Library slot), falling back to Generic.
    pub(crate) fn current_target<'a>(&self, lib: &'a LibraryData) -> &'a TargetData {
        if self.target >= 0
            && self.target < K_TARGET_SLOTS as i32
            && let Some(t) = lib.target(self.target)
        {
            return t;
        }
        lib.target(0).unwrap_or_else(|| generic_target())
    }
    pub(crate) fn driver_is(&self, d: Driver, lib: &LibraryData) -> bool {
        self.current_target(lib).driver == d
    }
    pub(crate) fn target_plays(&self, instrument: i32, lib: &LibraryData) -> bool {
        at_or_default(&self.current_target(lib).instruments, clamp(instrument, 0, K_INSTRUMENTS as i32 - 1))
    }

    // MODO BASS 2 default keyswitches (IK octave naming: middle C = C4). Now the keyswitchBass target's data.
    pub(crate) fn ks(&self, note: i32) -> i32 {
        clamp(note + self.ks_octave * 12, 0, 127)
    }
    pub(crate) fn bass_target(&self, lib: &LibraryData) -> bool {
        self.driver_is(Driver::KeyswitchBass, lib) && self.pattern == 2 && self.target_plays(self.pattern, lib)
    }
    pub(crate) fn continuous_pedal(&self, lib: &LibraryData) -> bool {
        self.driver_is(Driver::PianoPedal, lib) && (self.pattern == 0 || self.pattern == 3) && self.target_plays(self.pattern, lib)
    }

    pub(crate) fn mode_for_half(len_half: i32) -> i32 {
        if len_half == 1 {
            0
        } else if len_half == 4 {
            2
        } else {
            1
        }
    }
    pub(crate) fn len_ppq_for(&self, mode: i32) -> f64 {
        let b = f64::from(self.get_bar_length_ppq());
        if mode == 0 {
            b * 0.5
        } else if mode == 2 {
            b * 2.0
        } else {
            b
        }
    }
    pub(crate) fn chord_len_ppq(&self) -> f64 {
        self.len_ppq_for(match self.cur {
            Some(c) => Self::mode_for_half(c.len_half),
            None => self.chord_bars_mode,
        })
    }
    /// `entryRel (d)` (a copy: the C++ returns a reference into the timeline). No song (never the case where the C++
    /// calls it): a default entry.
    pub(crate) fn entry_rel(&self, d: i32, cx: &Ctx<'_>) -> TimelineEntry {
        if self.live_mode {
            // Live: now, what's coming, what was
            return self.live_entries[if d >= 1 {
                1
            } else if d < 0 {
                2
            } else {
                0
            }];
        }
        let Some(song) = cx.song else { return TimelineEntry::default() };
        let n = i32::try_from(song.entries.len()).unwrap_or(i32::MAX);
        let i = if d >= 1 && self.jump_next_idx >= 0 {
            // a queued jump: next is the target
            self.positive_modulo(self.jump_next_idx.wrapping_add(d).wrapping_sub(1), n)
        } else {
            self.positive_modulo(self.cur_index.wrapping_add(d), n)
        };
        at_or_default(&song.entries, i)
    }

    pub(crate) fn song_locate(&self, ppq: f64, cx: &Ctx<'_>) -> SongLoc {
        match cx.song {
            Some(s) => crate::song::song_locate(s, ppq, f64::from(self.get_bar_length_ppq()), false, self.bar_origin),
            None => SongLoc::default(),
        }
    }

    // The mono SWAM path; The Fiddle uses it in Classic (and in Live), with CC11 only.
    pub(crate) fn swam_active(&self, lib: &LibraryData) -> bool {
        (self.driver_is(Driver::Swam, lib) || (self.driver_is(Driver::Fiddle, lib) && (!self.perf.natural || self.live_mode)))
            && self.target_plays(self.pattern, lib)
    }
    pub(crate) fn fiddle_active(&self, lib: &LibraryData) -> bool {
        self.driver_is(Driver::Fiddle, lib) && self.target_plays(self.pattern, lib)
    }
    pub(crate) fn organ_active(&self, lib: &LibraryData) -> bool {
        self.driver_is(Driver::Organ, lib) && self.target_plays(self.pattern, lib)
    }
    pub(crate) fn fd_splits(&self, lib: &LibraryData) -> Splits {
        let t = self.current_target(lib);
        Splits { vibrato: t.fd_split_vib, portamento: t.fd_split_port, slur: t.fd_split_slur }
    }
    /// 0 strings (was target 5), 1 brass (6), 2 winds (7)
    pub(crate) fn swam_family(&self, lib: &LibraryData) -> i32 {
        self.current_target(lib).swam_family
    }

    // ---- Crow Hill Pocket Strings profile ------------------------------------
    // CC1 = timbre / dynamic layers, CC11 = expression (the plugin's defaults); polyphonic.
    // Crow Hill and Philharmonik 2 are one articulationKeys driver now; the target's data says
    // which keys, whether CC1 dynamics are sent and whether tremolo exists.
    pub(crate) fn art_active(&self, lib: &LibraryData) -> bool {
        self.driver_is(Driver::ArticulationKeys, lib) && self.target_plays(self.pattern, lib)
    }

    // ---- keyswitch driver (MusicianKeyswitch.cpp): the target's data names the keys, intents and CCs ----------
    // performStep's section phrasing (one articulation per step, CC dynamics / expression) runs for it as for
    // articulationKeys; Crow Hill Pocket Strings is on it.
    pub(crate) fn ks_active(&self, lib: &LibraryData) -> bool {
        self.driver_is(Driver::Keyswitch, lib) && self.target_plays(self.pattern, lib)
    }
    pub(crate) fn section_ks(&self, lib: &LibraryData) -> bool {
        self.art_active(lib) || self.ks_active(lib)
    }
    pub(crate) fn section_tremolo(&self, lib: &LibraryData) -> bool {
        if self.ks_active(lib) { self.current_target(lib).ks_use[KS_TREMOLO] >= 0 } else { self.current_target(lib).art_tremolo }
    }
    pub(crate) fn section_cap_below(&self, lib: &LibraryData) -> i32 {
        if self.ks_active(lib) { self.current_target(lib).ks_cap_below_energy } else { self.current_target(lib).art_pluck_cap_below_energy }
    }
    pub(crate) fn section_cap_vel(&self, lib: &LibraryData) -> i32 {
        if self.ks_active(lib) { self.current_target(lib).ks_cap_vel } else { self.current_target(lib).art_pluck_vel_cap }
    }
    pub(crate) fn section_lead_ms(&self, cc: i32, lib: &LibraryData) -> f64 {
        if self.ks_active(lib) {
            let t = self.current_target(lib);
            f64::from(if cc == 1 { t.ks_dyn_lead_ms } else { t.ks_expr_lead_ms })
        } else if cc == 1 {
            15.0
        } else {
            10.0
        }
    }
    pub(crate) fn ks_intent_of(flags: i32) -> i32 {
        (if (flags & 32) != 0 {
            KS_PLUCK
        } else if (flags & 64) != 0 {
            KS_TREMOLO
        } else if (flags & 1024) != 0 {
            KS_SHORT
        } else {
            KS_LONG
        }) as i32
    }
    pub(crate) fn ks_never(&self, note: i32, lib: &LibraryData) -> bool {
        let k = &self.current_target(lib).ks_never;
        self.driver_is(Driver::Keyswitch, lib) && kit::never_send(k, k.len() as i32, note)
    }

    // ---- UJAM Symphonic Elements (STRIIIINGS / BRAAASS) -----------------------------
    pub(crate) fn ujam_active(&self, lib: &LibraryData) -> bool {
        self.driver_is(Driver::UjamPhrase, lib) && self.target_plays(self.pattern, lib)
    }
    pub(crate) fn groove_now(&self) -> f32 {
        if self.groove_override >= 0.0 { self.groove_override } else { self.groove_amt }
    }
    // Phrase keys (C1 D1 E1 F1 G1 A1) and additions (C#1 D#1 F#1 G#1) are the target's data.

    // ---- Ample Sound guitars (ampleGuitar) ------------------------------------------------------
    // Two ways to play, per player (ampleStrums): the Strummer (chord detect at each chord change, one
    // stroke or single-string key per step, from the steps the guitar part already plays) or the notes
    // themselves with Ample's techniques. Every key is the target's data.
    pub(crate) fn ample_active(&self, lib: &LibraryData) -> bool {
        self.driver_is(Driver::AmpleGuitar, lib) && self.target_plays(self.pattern, lib)
    }
    pub(crate) fn ample_reset(&mut self) {
        self.guitar_prev = GShape::default();
        self.ample_sig = -1;
        self.ample_art = -1;
        self.ample_chord_sent = -1;
        self.ample_ready_at = -1.0e9;
        self.ample_prev_note = -1;
        self.ample_prev_on = -1.0e9;
        self.ample_hist_count = 0;
        self.ample_count = 0;
        self.ample_next_chord_at = -1.0;
        self.ample_legato_bar = -1000;
        self.ample_last_stroke_at = -1.0e9;
        self.ample_slide_in_armed = false;
        self.ample_final_hit = false;
        self.ample_legato_pending = false;
        self.ample_slide_out_bar = -1000;
        self.ample_slide_out_prev_bar = -1000;
        self.ample_slide_out_at = -1.0e9;
        self.ample_scratch_bar = -1000;
        self.ample_scratch_at = -1.0e9;
        self.ample_vib_start = -1.0e9;
        self.ample_vib_end = -1.0e9;
    }
    /// What Ample was told is no longer certain (steps made again, a kept take played): send it again.
    pub(crate) fn ample_forget(&mut self) {
        self.ample_art = -1;
        self.ample_chord_sent = -1;
        self.ample_prev_note = -1;
        self.ample_hist_count = 0;
        self.ample_next_chord_at = -1.0;
        self.ample_last_stroke_at = -1.0e9;
        self.ample_slide_in_armed = false;
        self.ample_legato_pending = false;
    }
    /// How early Ample's keys go before a step: inside a kept take's lead-in (TakeCapture kLead, 0.1 ppq).
    pub(crate) fn ample_lead_ppq(&self) -> f64 {
        min(40.0 * self.ppq_per_ms(), 0.09)
    }
    /// Ample speaks late: in Natural every key it is sent goes onsetMs early.
    pub(crate) fn ample_lead(&self, lib: &LibraryData) -> f64 {
        if self.expressive() && self.driver_is(Driver::AmpleGuitar, lib) {
            f64::from(self.current_target(lib).am_onset_ms) * self.ppq_per_ms()
        } else {
            0.0
        }
    }
    pub(crate) fn ample_release(&self, lib: &LibraryData) -> bool {
        self.expressive() && self.driver_is(Driver::AmpleGuitar, lib) && self.current_target(lib).am_release_velocity
    }
    /// Where a step at `stepPpq` will put its first stroke (ampleAt as that step will see it, the playhead aside).
    pub(crate) fn ample_at_for(&self, step_ppq: f64, lib: &LibraryData) -> f64 {
        step_ppq - min(self.ample_lead(lib), 0.095)
    }

    pub(crate) fn mp2_slot(&self, art: i32) -> i32 {
        u32::try_from(art.wrapping_mul(4)).ok().and_then(|s| self.art_map.checked_shr(s)).unwrap_or(0) & 15
    }
    pub(crate) fn swam_wind(&self, lib: &LibraryData) -> bool {
        self.swam_family(lib) == 1 || self.swam_family(lib) == 2
    }

    // ---- Note output ---------------------------------------------------------
    pub(crate) fn send_midi(&mut self, status: i32, d1: i32, d2: i32, cx: &Ctx<'_>, out: &mut dyn MidiSink) {
        if (status & 0xf0) == 0xb0 && d1 == 64 && self.in_drv(cx.lib) {
            return; // Indiginus: never the sustain pedal (it latches The Banjo's Picker)
        }
        if self.rendering {
            self.render_budget -= 1;
        }
        // One event carries what midiOut and logOut carried: the message, the logged ppq (as the float
        // the log stored) and the keyswitch flag. Rendered events went to the log only.
        // The Hammond's lower manual: exports put it with this musician, on its own channel.
        let lower_ev = self.og_lower_ch_used >= 0
            && (status & 15) == self.og_lower_ch_used
            && self.og_lower_ch_used != self.midi_channel0
            && (status & 0xff) >= 0x80;
        let e = MidiEvent {
            offset: if self.rendering { 0 } else { self.frame_offset },
            ppq: self.current_ppq as f32,
            status: (status & 0xff) as u8,
            d1: (d1 & 0x7f) as u8,
            d2: (d2 & 0x7f) as u8,
            keyswitch: self.log_flag == 0x100,
            rendered: self.rendering,
            lower: lower_ev,
            // Rust only (not compared): every event names its musician (C++: the lower manual's only).
            musician: clamp(self.index, 0, 5) as i8,
        };
        out.event(&e);
        if self.capture.is_some() && !self.rendering && !self.perf.live && !self.perf.audition && cx.song.is_some() && (status & 0xff) >= 0x80 {
            let jump = if self.perf.jump_land >= 0.0 && self.current_ppq >= self.perf.jump_land - 1.0e-9 {
                self.perf.jump_new_offset
            } else {
                self.perf.jump_offset
            };
            // The Hammond: its lower manual kept as low nibble 1 (a take replays it there), its upper registration
            // keys as nibble 2 (another target never plays either).
            let nibble = if lower_ev {
                1
            } else if e.keyswitch && self.driver_is(Driver::Organ, cx.lib) {
                2
            } else {
                0
            };
            let c = CaptureEvent {
                ppq: self.current_ppq + jump,
                musician: clamp(self.index, 0, 5) as i8,
                status: ((status & 0xf0) as u8) | nibble,
                d1: e.d1,
                d2: e.d2,
                keyswitch: e.keyswitch,
            };
            if let Some(capture) = self.capture.as_mut() {
                capture.push(c);
            }
        }
    }

    pub(crate) fn emit_note(&mut self, note: i32, velocity: i32, position: f64, duration_ppq: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.emit_note_f(note, velocity, position, duration_ppq, 0, cx, out);
    }

    /// Plugin expression (the expression bundle) plays only in Natural feel: Classic stays byte-identical.
    pub(crate) fn expressive(&self) -> bool {
        self.perf.natural
    }

    pub(crate) fn emit_note_f(
        &mut self,
        note: i32,
        velocity: i32,
        position: f64,
        duration_ppq: f32,
        flags: i32,
        cx: &mut Ctx<'_>,
        out: &mut dyn MidiSink,
    ) {
        let lib = cx.lib;
        let safe_note = clamp(note, 0, 127);
        let mut safe_velocity = clamp(velocity, 1, 127);

        let raw = (flags & K_RAW_FLAG) != 0;
        if self.in_drv(lib) && (if raw { self.in_never(safe_note, lib) } else { self.in_blocked(safe_note, flags, lib) }) {
            return;
        }
        // keyswitch: never its never keys (kept takes too)   // Indiginus: never its neverSend keys (kept takes too),
        // nothing outside its range but keys
        if self.ks_never(safe_note, lib) {
            return;
        }
        // Natural: the target's velocity calibration (never a keyswitch, never a kept take's replay).
        if !raw && (flags & 8192) == 0 && self.expressive() && self.target_plays(self.pattern, lib) && !self.brass_drv(lib) {
            safe_velocity = self.current_target(lib).calibrated(safe_velocity);
        }
        if !raw && (flags & 4096) != 0 && self.art_active(lib) {
            self.art_articulate(flags, cx, out);
        } else if !raw && (flags & 4096) != 0 && self.ks_active(lib) {
            self.ks_articulate(flags, cx, out);
        }

        if !raw && (flags & 2048) != 0 && self.swam_active(lib) {
            self.swam_articulate(flags, cx, out);
        }

        if !raw && self.bass_target(lib) && (flags & 8192) == 0 {
            // Latching style keyswitch: fingers, pick or slap, sent once and again whenever it changes.
            let kb = self.current_target(lib);
            let who = self.active_player(lib);
            let fw = self.feel_for(who, lib);
            let style = if fw.voicing == 0 && (fw.heavy == 1 || (flags & 8) != 0) {
                kb.kb_pick
            } else if (flags & 16) != 0 {
                kb.kb_slap
            } else {
                kb.kb_finger
            };
            if style != self.style_ks {
                let k = self.ks(style);
                self.send_keyswitch(0x90 + self.midi_channel0, k, 100, cx, out);
                self.send_keyswitch(0x80 + self.midi_channel0, k, 0, cx, out);
                self.style_ks = style;
            }

            // Palm mute = CC9 muting, only changed when it needs to be.
            let mut want_mute = if (flags & 2) != 0 { kb.kb_palm_on } else { 0 };
            if self.expressive() && kb.kb_mute_lo >= 0 {
                want_mute = self.natural_mute(kb, (flags & 2) != 0);
            }
            if want_mute != self.mute_cc {
                self.kb_palm_cc_sent = kb.kb_palm_cc;
                self.send_midi(0xB0 + self.midi_channel0, self.kb_palm_cc_sent, want_mute, cx, out);
                self.mute_cc = want_mute;
            }

            // Ghost mode keyswitch held for as long as a ghost note sounds.
            if (flags & 1) != 0 {
                if self.ghost_held == 0 {
                    self.kb_ghost_sent = kb.kb_ghost;
                    self.kb_ghost_note_sent = self.ks(self.kb_ghost_sent);
                    self.send_keyswitch(0x90 + self.midi_channel0, self.kb_ghost_note_sent, 100, cx, out);
                }
                self.ghost_held += 1;
            }
        }

        // MIDI 1.0 cannot independently identify overlapping copies of the
        // same pitch/channel. Close the prior copy before a clean retrigger.
        // Bass lines are mono: the new note closes any note still sounding.
        let raw_keyswitch = raw && (flags & 8192) != 0; // a kept keyswitch: it closes only its own copy
        // Bass is mono. In a kept take's replay and in Natural a keyswitch never closes a note nor is closed by one
        // (Classic: as before, the next note closes everything); in Natural a slide's source rings into it.
        let nat_bass = !raw && self.expressive() && self.pattern == 2;
        let out_ch = if self.emit_ch > 0 { self.emit_ch - 1 } else { self.midi_channel0 }; // the Hammond's lower manual has its own channel
        for i in 0..64 {
            if !self.active_slots.val(i) || self.active_channels.val(i) != out_ch {
                continue;
            }
            if self.active_notes.val(i) == safe_note {
                // Indiginus same: the return tap of a key pressed again leaves the new press's latch alone.
                if (self.active_flags.val(i) & (K_IN_LATCH_FLAG | K_IN_SAME_FLAG)) != 0
                    && (flags & 8192) != 0
                    && (flags & (K_IN_LATCH_FLAG | K_IN_SAME_FLAG)) == 0
                {
                    continue;
                }
                self.send_slot_note_off(i, cx, out);
                continue;
            }
            if self.pattern != 2 || raw_keyswitch {
                continue;
            }
            if ((flags | self.active_flags.val(i)) & 8192) != 0 && self.driver_is(Driver::Keyswitch, lib) {
                continue; // keyswitch: held keys and notes never close each other
            }
            if (raw || nat_bass) && ((flags & 8192) != 0 || (self.active_flags.val(i) & 8192) != 0) {
                continue; // kept takes / Natural: notes and keyswitches never close each other
            }
            if nat_bass && (flags & K_SLIDE_FLAG) != 0 {
                continue;
            }
            self.send_slot_note_off(i, cx, out);
        }

        for i in 0..64 {
            if !self.active_slots.val(i) {
                if (flags & 8192) != 0 {
                    self.log_flag = 0x100;
                }
                self.send_midi(0x90 + out_ch, safe_note, safe_velocity, cx, out);
                let tap = if (flags & (K_IN_LATCH_FLAG | K_IN_SAME_FLAG)) != 0 { position + 5.0 * self.ppq_per_ms() } else { -1.0 }; // a latching key: a 5 ms tap
                self.active_tap_off.set(i, tap);
                self.active_slots.set(i, true);
                self.active_notes.set(i, safe_note);
                self.active_channels.set(i, out_ch);
                self.active_off_ppq.set(i, position + f64::from(max(0.01f32, duration_ppq)));
                self.active_flags.set(i, flags);
                self.active_vel.set(i, safe_velocity);
                // Natural Pianoteq: its release gesture
                let off_vel = if (flags & K_RAW_FLAG) != 0 || self.expressive() { self.emit_off_vel } else { -1 };
                self.active_off_vel.set(i, off_vel);
                self.log_flag = 0;
                return;
            }
        }
    }

    // ---- Scheduling queue ----------------------------------------------------
    /// `schedule (kind, d1, d2, onPpq, durPpq)` (the queue is [`Scheduler`]; see the module doc for `kitCtx`).
    pub(crate) fn schedule(&mut self, kind: i32, d1: i32, d2: i32, on_ppq: f64, dur_ppq: f32) {
        self.q.schedule(self.current_ppq, kind, d1, d2, on_ppq, dur_ppq);
    }

    pub(crate) fn fire_due_events(&mut self, position: f64, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        if position < self.q.next_due_ppq {
            return;
        }
        let lib = cx.lib;

        // Controllers first so a pedal lift never lands after the notes it should precede.
        for pass in 0..2 {
            for i in 0..64 {
                let p = *self.q.queue.at(i);
                if p.used && (if pass == 0 { p.kind != 0 } else { p.kind == 0 }) && position >= p.on_ppq {
                    self.q.queue.at_mut(i).used = false;
                    // The queue entry is read again at each use, as the C++ does (a handler may reuse the slot).
                    if self.q.queue.at(i).kind == 1 {
                        let q = *self.q.queue.at(i);
                        self.send_midi(0xB0 + self.midi_channel0, q.d1, q.d2, cx, out);
                        let q = *self.q.queue.at(i);
                        if q.d1 == self.legato_cc_sent && self.legato_cc_sent >= 0 {
                            self.legato_on = q.d2 > 0;
                        }
                        if q.d1 == self.bass_vib_cc_sent && self.bass_vib_cc_sent >= 0 {
                            self.bass_vib_on = q.d2 > 0;
                        }
                        if q.d1 == self.bass_ring_cc_sent && self.bass_ring_cc_sent >= 0 {
                            self.bass_ring_on = q.d2 > 0;
                        }
                        if q.d1 == self.harmonic_cc_sent && self.harmonic_cc_sent >= 0 {
                            self.harmonic_held = q.d2 > 0;
                        }
                        self.organ_fired(1, q.d1, q.d2, q.flags, q.on_ppq, lib);
                        let q = *self.q.queue.at(i);
                        self.brass_fired(1, q.d1, q.d2, q.flags, q.on_ppq, lib);
                    } else if self.q.queue.at(i).kind == 3 {
                        let q = *self.q.queue.at(i);
                        self.send_midi(0x80 + (if q.ch > 0 { q.ch - 1 } else { self.midi_channel0 }), q.d1, 0, cx, out);
                    } else if self.q.queue.at(i).kind == 4 {
                        let q = *self.q.queue.at(i);
                        self.send_midi(0xE0 + self.midi_channel0, q.d1, q.d2, cx, out);
                        let q = *self.q.queue.at(i);
                        self.wheel_bent = q.d1 + q.d2 * 128 != 8192;
                    } else {
                        let q = *self.q.queue.at(i);
                        if q.ch > 0 {
                            self.organ_lower_fired(q.d1, q.flags, lib);
                        } else {
                            self.organ_fired(0, q.d1, q.d2, q.flags, q.on_ppq, lib);
                        }
                        let q = *self.q.queue.at(i);
                        if q.ch == 0 {
                            self.brass_fired(0, q.d1, q.d2, q.flags, q.on_ppq, lib);
                        }
                        let q = *self.q.queue.at(i);
                        self.emit_off_vel = q.off_vel;
                        self.emit_ch = q.ch;
                        self.emit_note_f(q.d1, q.d2, position, q.dur_ppq, q.flags, cx, out);
                        self.emit_off_vel = -1;
                        self.emit_ch = 0;
                    }
                }
            }
        }

        self.q.next_due_ppq = 1.0e18;
        for i in 0..64 {
            let p = *self.q.queue.at(i);
            if p.used && p.on_ppq < self.q.next_due_ppq {
                self.q.next_due_ppq = p.on_ppq;
            }
        }
    }

    // ---- Step note buffer ----------------------------------------------------
    pub(crate) fn clear_step(&mut self) {
        self.step_count = 0;
    }

    // ---- The fiddler's vocabulary -------------------------------------------------------------------------
    pub(crate) fn lick_land_forget(&mut self) {
        self.lick_land_at = -1.0;
        self.lick_land_set_at = -1.0;
        self.lick_land_which = -1;
    }
    pub(crate) fn lick_forget(&mut self) {
        self.lick_land_forget();
        self.lick_fill_at = -1.0;
        self.lick_fill_prev_at = -1.0;
    }
    /// A lick set plays only in the meter it is written for (the 4/4 sets in 4/4 only).
    pub(crate) fn lick_meter_ok(&self, set: &LickSet) -> bool {
        set.count > 0 && set.meter == self.meter
    }
    /// A lick note struck: the test hooks (lickOffMeter counts a set played outside its meter: never).
    pub(crate) fn lick_struck(&self, set: &LickSet) {
        LICK_NOTES_PLAYED.fetch_add(1, Ordering::Relaxed);
        if set.meter != self.meter {
            LICK_OFF_METER.fetch_add(1, Ordering::Relaxed);
        }
    }
    /// The 16th of the bar a lick note starts on (startBeats from the next downbeat; 4/4: x 4 + 16).
    pub(crate) fn lick_step(&self, l: &LickNote) -> i32 {
        self.floor_to_int(f64::from(l.start_beats * 4.0f32 + self.spb() as f32 + 0.5f32))
    }
    /// The most 16ths of the bar a lick's rest may start from: 2 beats in 4/4 and 3/4, a dotted beat in 6/8.
    pub(crate) fn lick_rest_max(&self) -> i32 {
        if self.meter == Meter::M34 { 4 } else { self.spb() - half_step(self.meter) }
    }

    // ---- Orange Tree Evolution / Indiginus plucked strings -------------------------------------------
    pub(crate) fn evolution_active(&self, lib: &LibraryData) -> bool {
        self.driver_is(Driver::Evolution, lib) && self.target_plays(self.pattern, lib)
    }
    // Task 4: the Indiginus plucked strings (indiginusStrings) play through the same engine, reading the evolution view
    // of their target (LibraryLoader indiginusPluckedView); what differs is behind inDrv().
    pub(crate) fn in_drv(&self, lib: &LibraryData) -> bool {
        self.driver_is(Driver::IndiginusStrings, lib)
    }
    pub(crate) fn indiginus_active(&self, lib: &LibraryData) -> bool {
        self.in_drv(lib) && self.target_plays(self.pattern, lib)
    }
    pub(crate) fn plucked_driver(&self, lib: &LibraryData) -> bool {
        self.driver_is(Driver::Evolution, lib) || self.in_drv(lib)
    }
    pub(crate) fn plucked_active(&self, lib: &LibraryData) -> bool {
        self.evolution_active(lib) || self.indiginus_active(lib)
    }
    pub(crate) fn ev_natural(&self, lib: &LibraryData) -> bool {
        self.plucked_active(lib) && self.expressive() && !self.live_mode
    }
    pub(crate) fn ev_section_id(&self, pass: i32) -> i32 {
        match self.cur {
            Some(c) => self.positive_modulo(pass, 4096) * 4096 + c.arr_pos,
            None => -1,
        }
    }
    /// One-key strums (the "Library chord keys" choice): Mandolin / Resonator / Ukulele have CHORDS keys; The
    /// Banjo's clash with trills.
    pub(crate) fn in_slots_on(&self, lib: &LibraryData) -> bool {
        if !self.in_drv(lib) || self.pluck_chords != 1 {
            return false;
        }
        let t = self.current_target(lib);
        t.in_chord_down >= 0 && t.in_chord_up >= 0 && t.in_layout != 1
    }
    /// Never sent on this target: its neverSend keys (The Banjo's Harmony, the Renegade C#0).
    pub(crate) fn in_never(&self, note: i32, lib: &LibraryData) -> bool {
        let k = &self.current_target(lib).in_never_send;
        kit::never_send(k, k.len() as i32, note)
    }

    /// A run's release, never before the cut of the last regeneration (a re-press there must not come after it).
    pub(crate) fn jb_release(&mut self, at: f64) {
        self.schedule(1, 64, 0, max(at, self.jb_rel_floor), 0.0);
    }

    // ---- Hammond B-3X ----------------------------------------------------------------------------------------
    pub(crate) fn organ_sec_key(&self, pass: i32) -> i64 {
        i64::from(pass) * 4096 + i64::from(self.cur.map_or(-1, |c| c.arr_pos) + 1)
    }
    /// The lower manual's channel (1-16; 0 = off). Off when it is the musician's own: the two manuals couldn't be told apart.
    pub(crate) fn organ_lower_channel(&self, lib: &LibraryData) -> i32 {
        let c = self.current_target(lib).og_lower_channel;
        if c > 0 && c - 1 != self.midi_channel0 { c } else { 0 }
    }
    pub(crate) fn organ_lower_reg_key(&self, en: i32, lib: &LibraryData) -> i32 {
        at_or_default(&self.current_target(lib).og_lower_keys, clamp(en, 1, 4) - 1)
    }
    pub(crate) fn organ_lower_reg_energy(&self, key: i32, lib: &LibraryData) -> i32 {
        for k in 0..4 {
            if self.organ_lower_reg_key(k + 1, lib) == key {
                return k + 1;
            }
        }
        0
    }

    // ---- Task 6: Indiginus Blue Street Brass ---------------------------------------------------------------
    pub(crate) fn brass_drv(&self, lib: &LibraryData) -> bool {
        self.driver_is(Driver::BrassSection, lib)
    }
    pub(crate) fn brass_active(&self, lib: &LibraryData) -> bool {
        self.brass_drv(lib) && self.target_plays(self.pattern, lib)
    }

    /// `pedalEvents (who, progBar, sib, newChord, stepPpq, sameBass = false)`.
    pub(crate) fn pedal_events(&mut self, who: i32, prog_bar: i32, sib: i32, new_chord: bool, step_ppq: f64, same_bass: bool, lib: &LibraryData) {
        if self.brass_active(lib) {
            return; // Blue Street Brass: CC1 is its dynamics, no pedal
        }
        if self.organ_active(lib) {
            return; // Hammond: no sustain / soft pedal, no player CC11 swells (the swell is its own)
        }
        let mode = self.feel_for(who, lib).pedal_mode;
        let ms = self.ppq_per_ms();
        let lead = f64::from(self.feel_for(who, lib).timing_ms);
        let beat = f64::from(self.tick_ppq(lib)) * 4.0; // 16-only: a quarter, the mode-3 lift (6/8 dotted beat: Task 6 audit)

        if self.expressive() && self.continuous_pedal(lib) && (mode == 1 || mode == 2) {
            if new_chord || sib == 0 {
                self.natural_pedal(mode, new_chord, same_bass, step_ppq, lead, ms, lib);
            }
            return;
        }

        if (mode == 1 || mode == 2) && sib == 0 {
            if self.continuous_pedal(lib) && mode == 2 {
                // Half-pedal change: lift only part way, then sink back in so the wash never fully breaks.
                self.schedule(1, 64, 24, step_ppq + (lead - 15.0) * ms, 0.0);
                self.schedule(1, 64, 70, step_ppq + (lead + 10.0) * ms, 0.0);
                self.schedule(1, 64, 127, step_ppq + (lead + 40.0) * ms, 0.0);
            } else {
                self.schedule(1, 64, 0, step_ppq + (lead - 15.0) * ms, 0.0);
                self.schedule(1, 64, 127, step_ppq + (lead + (if mode == 2 { 20.0 } else { 30.0 })) * ms, 0.0);
            }

            if self.continuous_pedal(lib) {
                let want_soft = if self.energy <= 1 { 127 } else { 0 };
                if want_soft != self.soft_pedal {
                    self.schedule(1, 67, want_soft, step_ppq + (lead - 20.0) * ms, 0.0);
                    self.soft_pedal = want_soft;
                }
            }
        } else if mode == 3 && is_beat(self.meter, sib) {
            self.schedule(1, 64, 0, step_ppq + (lead - 5.0) * ms, 0.0);
            self.schedule(1, 64, 127, step_ppq + (lead + 15.0) * ms, 0.0);
            self.schedule(1, 64, 0, step_ppq + beat * 0.6, 0.0);
        } else if mode == 4 && sib == 0 && new_chord {
            self.schedule(1, 64, 127, step_ppq + 20.0 * ms, 0.0);
            self.schedule(1, 64, 0, step_ppq + beat, 0.0);
        }
        // A solo target with no expression CC (dynamics on velocity) gets no CC11 swells from the player.
        else if (mode == 5 || mode == 6) && self.swam_active(lib) && self.current_target(lib).swam_expr_cc < 0 {
        } else if mode == 5 && sib % 2 == 0 {
            // Expression swell on every chord: dip at the change, bloom through the bar.
            let depth = 46.0f32 * self.dynamics / 50.0f32;
            let v = clamp((112.0f32 - depth + depth * sib as f32 / (self.spb() - 2) as f32) as i32, 1, 127);
            self.schedule(1, 11, v, step_ppq, 0.0);
            self.expr_used = true;
        } else if mode == 6 && sib % 4 == 0 {
            // One long crescendo over the whole progression, dropping back at the top.
            let t = (prog_bar * self.spb() + sib) as f32 / (self.num_bars() * self.spb()) as f32;
            let depth = 60.0f32 * self.dynamics / 50.0f32;
            let v = clamp((120.0f32 - depth + depth * t) as i32, 1, 127);
            self.schedule(1, 11, v, step_ppq, 0.0);
            self.expr_used = true;
        }
    }

    // ---- Shared lane helpers for the non-keys roles ------------------------------
    pub(crate) fn lane_type(&mut self, who: i32, lane: i32, gstep: i32, lib: &LibraryData) -> i32 {
        self.grid_at(who, lane, gstep, 3, lib)
    }
    pub(crate) fn lane_vel(&mut self, who: i32, lane: i32, gstep: i32, lib: &LibraryData) -> f32 {
        self.grid_at(who, lane, gstep, 1, lib) as f32
    }
    pub(crate) fn lane_len(&mut self, who: i32, lane: i32, gstep: i32, tick: f32, lib: &LibraryData) -> f32 {
        self.grid_at(who, lane, gstep, 2, lib) as f32 * tick * self.length
    }

    // Keep bass lines playable: nothing below D1.
    pub(crate) fn bass_range(&self, p: i32) -> i32 {
        let mut n = p;
        while n < 26 {
            n += 12;
        }
        n
    }
    /// 6/8: the dotted quarter
    pub(crate) fn beat_ppq(&self) -> f64 {
        if self.meter == Meter::M68 { 1.5 } else { f64::from(info(self.meter).bar_ppq) / f64::from(info(self.meter).beats_per_bar) }
    }
    pub(crate) fn fill_count(&self, who: i32, lib: &LibraryData) -> i32 {
        clamp(self.feel_for(who, lib).fill_count, 1, 5)
    }
    pub(crate) fn bass_of(c: &Chord) -> i32 {
        if c.bass_pc >= 0 { c.bass_pc } else { c.root_pc }
    }
    /// The closest pitch at or below `note` whose pitch class is the chord's slash bass.
    pub(crate) fn slash_at_or_below(&self, note: i32, c: &Chord) -> i32 {
        note - self.positive_modulo(note - (self.key_root + c.bass_pc), 12)
    }
    /// The arrangement position (the section) for per-section drum decisions; the pass without a song.
    pub(crate) fn drum_section(&self, pass: i32) -> i32 {
        self.cur.map_or(pass, |c| c.arr_pos)
    }

    // ======== end of translated body ========

    // ---- The endpoints, as C++ calls ------------------------------------------
    pub(crate) fn dispatch_parameter(&mut self, n: i32, v: f32, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        match n {
            1 => self.on_param1(v, cx, out),
            2 => self.on_param2(v, cx, out),
            3 => self.on_param3(v, cx, out),
            4 => self.on_param4(v, cx, out),
            5 => self.on_param5(v, cx, out),
            6 => self.on_param6(v, cx, out),
            7 => self.on_param7(v, cx, out),
            8 => self.on_param8(v, cx, out),
            9 => self.on_param9(v, cx, out),
            10 => self.on_param10(v, cx, out),
            11 => self.on_param11(v, cx, out),
            12 => self.on_param12(v, cx, out),
            13 => self.on_param13(v, cx, out),
            14 => self.on_param14(v, cx, out),
            15 => self.on_param15(v, cx, out),
            16 => self.on_param16(v, cx, out),
            17 => self.on_param17(v, cx, out),
            18 => self.on_param18(v, cx, out),
            19 => self.on_param19(v, cx, out),
            20 => self.on_param20(v, cx, out),
            21 => self.on_param21(v, cx, out),
            22 => self.on_param22(v, cx, out),
            23 => self.on_param23(v, cx, out),
            24 => self.on_param24(v, cx, out),
            25 => self.on_param25(v, cx, out),
            26 => self.on_param26(v, cx, out),
            27 => self.on_param27(v, cx, out),
            28 => self.on_param28(v, cx, out),
            29 => self.on_param29(v, cx, out),
            30 => self.on_param30(v, cx, out),
            31 => self.on_param31(v, cx, out),
            32 => self.on_param32(v, cx, out),
            33 => self.on_param33(v, cx, out),
            34 => self.on_param34(v, cx, out),
            35 => self.on_param35(v, cx, out),
            36 => self.on_param36(v, cx, out),
            37 => self.on_param37(v, cx, out),
            38 => self.on_param38(v, cx, out),
            39 => self.on_param39(v, cx, out),
            40 => self.on_param40(v, cx, out),
            _ => {}
        }
    }

    pub(crate) const NUM_PARAMS: i32 = 40;

    /// paramN of the Cmajor processor is index N-1.
    pub(crate) fn set_parameter(&mut self, index: usize, v: f32, cx: &mut Ctx<'_>, idle: &mut dyn MidiSink) {
        let n = i32::try_from(index).map_or(0, |i| i.saturating_add(1));
        self.dispatch_parameter(n, v, cx, idle);
    }

    /// The Amorph transportIn packet (play, bpm, numerator, denominator, ppq, bar start), fed through
    /// the ported handler in slot order so its start / seek / tempo rules apply unchanged.
    pub(crate) fn set_host_transport(&mut self, playing: bool, bpm: f64, ppq: f64, num: i32, den: i32, cx: &mut Ctx<'_>, idle: &mut dyn MidiSink) {
        for v in [if playing { 1.0f32 } else { 0.0f32 }, bpm as f32, num as f32, den as f32, ppq as f32, 0.0f32] {
            self.on_transport_in(v, cx, idle);
        }
    }

    pub(crate) fn midi_in(&mut self, data: &[u8], cx: &mut Ctx<'_>, idle: &mut dyn MidiSink) {
        self.on_midi_in(data, cx, idle);
    }

    /// -1 = use the roster slot (Amorph), 0 = Session, >0 = that persona. A change restarts like a player change.
    pub(crate) fn set_persona_seed(&mut self, seed: i32, cx: &mut Ctx<'_>, idle: &mut dyn MidiSink) {
        let seed = clamp(seed, -1, K_MAX_SEED as i32);
        if seed != self.seed_override {
            self.seed_override = seed;
            self.reset_scheduler(cx, idle);
            self.apply_speed(false, cx, idle); // a new role: the song's Time may (not) apply
        }
    }

    /// One frame of main(). `f` is the frame's position inside the host block.
    pub(crate) fn frame(&mut self, f: usize, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.frame_offset = i32::try_from(f).unwrap_or(i32::MAX);
        self.main_frame(cx, out);
        self.frame_offset = 0;
    }

    /// Closes every sounding note and resets controllers (reload, release, bypass).
    pub(crate) fn flush(&mut self, cx: &mut Ctx<'_>, out: &mut dyn MidiSink) {
        self.frame_offset = 0;
        self.reset_scheduler(cx, out);
    }

    // `setLibrary`: the Library is `cx.lib`; the Band swaps it between blocks. The engine's last-sent controller
    // numbers survive the swap.

    // For the UI and the processor.
    pub(crate) fn active_persona(&self, lib: &LibraryData) -> i32 {
        self.active_player(lib)
    }
    pub(crate) fn is_rendering(&self) -> bool {
        self.rendering
    }

    /// Test support: the address of every buffer the musician schedules into (they never move: fixed arrays).
    pub(crate) fn buffer_addresses(&self) -> [usize; 2] {
        [std::ptr::from_ref(&self.q.queue) as usize, std::ptr::from_ref(&self.active_slots) as usize]
    }
}
