//! `fs_shale`: the owner's Shale drum gate (a band-limited onset detector, a lookahead gate whose
//! close follows each hit's own decay, DeBleed, Kit Link and mic phase alignment), a faithful
//! translation of `Shale/Source/ShaleDSP.h` ([`shale_dsp`]), `ShaleAlign.h` ([`shale_align`]),
//! Kit Link (`ShaleLink.{h,cpp}`, `ShaleAudioShare.{h,cpp}`: [`shale_link`]) and the processor
//! (`PluginProcessor.{h,cpp}`, here).
//!
//! Wrapper (`PluginProcessor.{h,cpp}`) audit, everything that affects the sound:
//! - `prepareToPlay`: `dsp.prepare (sr, block)`, `forcePush`, `alignDelay.prepare (sr, block * 4)`,
//!   `alignActive` from Align, then the latency reported straight away (`takeGeometryChanged`,
//!   `updateLatency`). Here [`Plugin::prepare`] does the same and also runs the forced push at
//!   once (the C++ runs it at the top of the first block, with no audio in between, so the order
//!   of events is the same), so [`Plugin::latency`] is right before the first block. The link's
//!   scratch (`monoScratch`, the event lists) and the measurement's windows are sized there too.
//! - `processBlock`, in order:
//!   1. the push of only what moved (`v != lastPushed[i]`; everything after a prepare). It
//!      matters: the Mic setter rebuilds every filter and clears the learned DeBleed profile;
//!   2. `takeGeometryChanged` → `updateLatency` (Lookahead moved);
//!   3. the clock for Kit Link: the host's sample position, here `Transport.samples` (the DAW
//!      always gives one, so the C++'s wall-clock fallback never runs);
//!   4. the shared audio: while Kit Link is on, this mic's input (the mono mean of its channels,
//!      before the gate) goes into its slot's ring, for whoever measures against it;
//!   5. Kit Link in: presence published, then `collect` over the group from 12 ms before the block
//!      to its end, each partner trigger and neighbour beacon into the DSP at its offset in the
//!      block (clamped into it);
//!   6. the sidechain: used when the host gives the bus AND Trigger From is Sidechain. SoundCraft's
//!      [`Plugin::process_with_sidechain`] maps it faithfully: `None` (no source chosen) is the
//!      C++ with no sidechain bus, which keys from the track whatever Trigger From says; `Some`
//!      is an enabled bus of that many channels (at most 2 used), silent when the source is
//!      muted, deleted or would loop (B1), so Trigger From = Sidechain then never opens, as the
//!      C++ with a silent sidechain. A key buffer shorter than the block is silent where it ends
//!      (the C++ bus always has the block's length);
//!   7. `dsp.process`;
//!   8. this mic's hits for its own phase measurement: its own triggers, and the partners'
//!      positions for the hits it opened on;
//!   9. the alignment delay, last: Align switched on or off re-reports the latency (+10 ms base
//!      while on); while on, the offset (and the flip) is the target, zero while the engine's own
//!      bypass is on (it keeps the latency, not the offset);
//!   10. Kit Link out: every beacon, and its own triggers unless it is a Follow;
//!   11. MIDI out: with MIDI Out on, one note per hit on channel 10 (the MIDI Note knob's note,
//!       velocity from the hit level) at the hit's offset, handed to the host through
//!       [`Plugin::midi_out`]; the note-off comes a sixteenth note later (the host's tempo, 120
//!       when it gives none) or at the next hit, whichever is first. Off, a pending note-off still
//!       goes out, so nothing hangs;
//!   12. `dsp.clearTriggers()`.
//! - Latency: the lookahead (`dsp.latencySamples()`) plus the align base while Align is on,
//!   reported only when it moves (here [`Plugin::latency`] returns it; the mix's
//!   delay-compensation pass re-aligns when it changes). C++ quirk kept: the DSP computes the
//!   lookahead in samples only when the Lookahead choice CHANGES, so the default 2 ms is 96
//!   samples at every rate (2.18 ms at 44.1 kHz, 1 ms at 96 kHz) until it is moved. Safety
//!   deviation: after a live re-prepare at a LOWER rate the lookahead is clamped again into the
//!   new delay line (`ShaleDsp::reclamp_lookahead`), so the reported latency stays the real delay.
//! - Tail: `getTailLengthSeconds` is 0. `tail_samples` is a computed lookahead + align delay +
//!   100 ms (the detector band's ring, which Monitor ▸ Detector outputs), at least the C++ value.
//! - Bypass: the engine's own `bypass` keeps running the delay line (no shift), translated in the
//!   engine; hidden in the UI (the module's bypass stands for it).
//! - `ScopedNoDenormals`: not emulated, see [`shale_dsp`] (no decision can see a denormal).
//! - Kit Link: the C++'s memory-mapped `link.dat` and `audio.dat` are the session's link group slab
//!   (plan B4, [`shale_link::KitLink`]; the C++'s Link Group A–D stays a group within it). The host
//!   orders a group by role: a Lead and a Pair publish triggers and every member beacons, so they
//!   share as peers ([`LinkRole::Peer`]); a Follow only reads its partners' triggers
//!   ([`LinkRole::Consumer`]), so the host renders it after them and it opens in the same block.
//!   Between peers the order is the session's, as a C++ host's is arbitrary: the 12 ms reach back
//!   catches a beacon published a block earlier. Kit Link and Link Role set that order, so
//!   neither is offered for automation (`soundcraft_dsp::link`).
//! - Phase alignment (`runAlignment`, the C++ message thread's 4 Hz timer): the hits go into a
//!   pending list (`hitFifo`, the newest 64), and each block (a module has no message thread) the
//!   measurement runs on the audio thread: the reference is found among the live instances, the
//!   ready hits are sorted and deduplicated, and each one's windows are read from the shared audio
//!   and measured ([`shale_align::Measurer`], [`shale_align::measure`]'s arithmetic a few lags per
//!   block, 128 multiply-adds a sample of the block (at least one lag), one hit at a time, so the
//!   cost is a fixed share of the block's time and a block never pays for a whole correlation). The report (`AlignReport`) goes to the UI through [`Analysis::report`]
//!   ([`REPORT_STATE`] …); Line Up (`applyAlignment`) is the UI writing Align Offset, Align
//!   Polarity and Align from it, as the C++ editor's button does.
//! - SC Listen: stored by the C++ engine and never read (Monitor ▸ Detector is how you hear the
//!   key), so it has no effect here either; hidden as inert.
//! - Reset (the DAW's, on every Play, Stop and Locate): the C++ `prepareToPlay` on the live
//!   instance: push what moved, then `dsp.prepare` on its own state (every filter, envelope, the
//!   delay line, the gate, the beacons, the scope and the learned DeBleed profile cleared; the
//!   parameters kept; the trim and output smoothing start at their targets), then the forced
//!   push. Rust additions make it independent of the history: DeBleed's velocity tracking
//!   (`lastVelOut`, which the C++ keeps across a prepare) restarts at 0; the align delay and
//!   polarity start at their targets (no 80 ms glide or 10 ms polarity crossfade from the C++'s
//!   fresh defaults); and this instance's Kit Link event ring and shared audio start afresh, with
//!   its pending hits and a measurement under way dropped (positions from before a locate mean
//!   nothing after it; the measurements already made are kept, as the C++ keeps its estimator).
//!   No allocation.
//!
//! The scope (the C++ editor's `ShaleDisplay`: 2500 bins of 4 ms, each the input peak, the gate's
//! most-closed gain and the detector's peak) is computed in the DSP as the C++ does and each
//! finished bin is pushed to the analysis `post` ring as `[peak, gain, det]` (`pre` unfed); a
//! reset pushes a cleared history, as the C++ `prepare` clears its scope.
//!
//! Station has no Shale module; the key knobs are the editor's main dial row (Threshold, Onset,
//! Length, Reduction), the accent its `kAccent` 0xFFC8A06A, the model parameter the Mic.

mod presets;
pub mod shale_align;
pub mod shale_config;
pub mod shale_dsp;
pub mod shale_link;

pub use presets::PRESETS;
pub use shale_dsp::{NUM_PARAMS, ShaleDsp};

use crate::analysis::Analysis;
use crate::link::{LinkRole, LinkSlab, LinkSpec};
use crate::params::{Params, fs_choice, fs_toggle, knob01};
use crate::{Category, MidiOut, ParamInfo, Plugin, PluginInfo, Transport};
use shale_align::{AlignDelay, Estimator, Measurer};
use shale_config::{ALIGN_TOS, CLOSE_MODES, DEBLEED_MODES, INSTRUMENTS, LINK_GROUPS, LINK_ROLES, LOOKAHEADS, MONITORS, SC_SOURCES, format_alone};
use shale_dsp as d;
use shale_link::{CLK_TRANSPORT, EV_BEACON, EV_TRIGGER, FLAG_PHASE_REF, KitLink, LinkEvent, RING_SIZE};
use std::sync::Arc;

macro_rules! display_fns {
    ($($name:ident => $idx:expr),* $(,)?) => {$(
        fn $name(v: f32) -> String {
            format_alone($idx, v)
        }
    )*};
}
display_fns!(
    threshold_text => d::P_THRESHOLD,
    sens_text => d::P_SENS,
    reduction_text => d::P_REDUCTION,
    length_text => d::P_LENGTH,
    hold_text => d::P_HOLD,
    retrig_text => d::P_RETRIG,
    debleed_text => d::P_DEBLEED,
    xover_text => d::P_XOVER,
    vel_track_text => d::P_VEL_TRACK,
    det_low_text => d::P_DET_LOW,
    det_high_text => d::P_DET_HIGH,
    mix_text => d::P_MIX,
    trim_text => d::P_TRIM,
    output_text => d::P_OUTPUT,
    reject_text => d::P_REJECT,
    midi_note_text => d::P_MIDI_NOTE,
    align_offset_text => d::P_ALIGN_OFFSET,
);

/// The drawer's groups, as the editor's panels: the gate (the Mic, the main dial row and the
/// behaviour row), the link strip with its phase row ([`LINK_TAB`]) and the More panel.
const GATE: &str = "Gate";
/// Kit Link's tab: the link strip and the phase row.
pub const LINK_TAB: &str = "Kit Link";
const MORE: &str = "More";

/// Parameters in the C++ order (`Parameters.h`), ids, names and defaults verbatim.
pub static SHALE_PARAMS: [ParamInfo; NUM_PARAMS] = [
    // The engine's own bypass, kept in order; the UI uses the module's bypass instead.
    fs_toggle("bypass", "Bypass", 0.0, ""),
    fs_choice("instrument", "Mic", 3.0, GATE, &INSTRUMENTS),
    knob01("threshold", "Threshold", 0.55, GATE, threshold_text),
    knob01("sens", "Onset", 0.5, GATE, sens_text),
    fs_toggle("ghost", "Ghost Notes", 0.0, GATE),
    knob01("reduction", "Reduction", 1.0, GATE, reduction_text),
    knob01("length", "Length", 0.4, GATE, length_text),
    fs_choice("closeMode", "Close", 0.0, GATE, &CLOSE_MODES),
    fs_choice("lookahead", "Lookahead", 2.0, GATE, &LOOKAHEADS),
    knob01("hold", "Hold", 0.35, MORE, hold_text),
    knob01("retrig", "Retrigger", 0.2, MORE, retrig_text),
    knob01("debleed", "DeBleed", 0.0, GATE, debleed_text),
    fs_choice("debleedMode", "DeBleed Mode", 0.0, GATE, &DEBLEED_MODES),
    knob01("xover", "Crossover", 0.5, MORE, xover_text),
    knob01("velTrack", "Vel Track", 0.5, MORE, vel_track_text),
    fs_toggle("autoBand", "Auto Band", 1.0, MORE),
    knob01("detLow", "Det Low", 0.4, MORE, det_low_text),
    knob01("detHigh", "Det High", 0.6, MORE, det_high_text),
    fs_choice("scSource", "Trigger From", 0.0, MORE, &SC_SOURCES),
    fs_toggle("scListen", "SC Listen", 0.0, MORE),
    fs_choice("monitor", "Monitor", 0.0, GATE, &MONITORS),
    knob01("mix", "Mix", 1.0, MORE, mix_text),
    knob01("trim", "Input", 0.3333, MORE, trim_text),
    knob01("output", "Output", 0.5, MORE, output_text),
    fs_toggle("link", "Kit Link", 0.0, LINK_TAB),
    fs_choice("linkGroup", "Link Group", 0.0, LINK_TAB, &LINK_GROUPS),
    fs_choice("linkRole", "Link Role", 0.0, LINK_TAB, &LINK_ROLES),
    knob01("reject", "Reject", 0.5, LINK_TAB, reject_text),
    fs_toggle("midiOut", "MIDI Out", 0.0, MORE),
    knob01("midiNote", "MIDI Note", 0.1667, MORE, midi_note_text),
    fs_toggle("phaseRef", "Phase Reference", 0.0, LINK_TAB),
    fs_toggle("align", "Align", 0.0, LINK_TAB),
    knob01("alignOffset", "Align Offset", 0.5, LINK_TAB, align_offset_text),
    fs_toggle("alignFlip", "Align Polarity", 0.0, LINK_TAB),
    fs_choice("alignTo", "Align To", 0.0, LINK_TAB, &ALIGN_TOS),
];

pub static SHALE_INFO: PluginInfo = PluginInfo {
    id: "fs_shale",
    name: "Shale",
    short_name: "Shale",
    category: Category::FireStation,
    params: &SHALE_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: true,
    link: Some(LinkSpec {
        enable_param: Some("link"),
        role_param: Some("linkRole"),
        // Lead and Pair publish triggers (and every member beacons): peers; a Follow reads its
        // partners' triggers: a consumer, rendered after them.
        roles: &[Some(LinkRole::Peer), Some(LinkRole::Consumer), Some(LinkRole::Peer)],
        layout: shale_link::LAYOUT,
        words: shale_link::WORDS,
    }),
    references: 0,
    midi_out: true,
};

/// This engine's registration (collected by `crates/dsp/build.rs`; see [`super::EngineReg`]).
pub const ENGINE: super::EngineReg = super::EngineReg {
    info: &SHALE_INFO,
    make: || Box::new(Shale::new()),
    presets: PRESETS,
    order: 170,
    fidelity: Some(super::Fidelity { name: "shale", source_dir: "Shale", render_tool: "Shale", tables: &[] }),
    prepare_state: None,
    actions: &[],
};

/// Parameters with no effect in the DAW: SC Listen (the C++ engine never reads it). The drawer,
/// the generic window and the lane picker hide them through one predicate. MIDI Out and MIDI Note
/// used to be here: a module's MIDI output now feeds any track whose input is MIDI from it.
pub const INERT_PARAMS: [&str; 1] = ["scListen"];

/// `readouts[4]` and `[7]` keep their counters to 24 bits, so every count is exact as an `f32`.
pub const COUNT_MASK: i32 = 0x00FF_FFFF;

/// Room for a block's link events (`outEvents.reserve (192)` and the partner/neighbour lists).
const LINK_EVENTS: usize = 192;

/// `AlignReport::State`, as [`REPORT_STATE`] carries it.
pub const AL_LINK_OFF: i32 = 0;
pub const AL_IS_REFERENCE: i32 = 1;
pub const AL_NO_REFERENCE: i32 = 2;
pub const AL_NO_TRANSPORT: i32 = 3;
pub const AL_LISTENING: i32 = 4;
pub const AL_MEASURED: i32 = 5;
pub const AL_UNAVAILABLE: i32 = 6;

/// The phase measurement's report in [`Analysis::report`] (`getAlignReport`): the state, the
/// measurements kept, the median offset and its spread (ms), the majority polarity (±1), the mean
/// correlation, the reference's Mic (-1: none), and [`REPORT_VALID`] once written; and, not part
/// of the C++ report, 1 when this instance is in a link group but got no slot (a member index past
/// [`shale_link::MAX_INSTANCES`]), for the link strip.
pub const REPORT_STATE: usize = 0;
pub const REPORT_HITS: usize = 1;
pub const REPORT_OFFSET_MS: usize = 2;
pub const REPORT_SPREAD_MS: usize = 3;
pub const REPORT_POLARITY: usize = 4;
pub const REPORT_CORR: usize = 5;
pub const REPORT_REF: usize = 6;
pub const REPORT_MARK: usize = 7;
pub const REPORT_VALID: f32 = 1.0;
pub const REPORT_NO_SLOT: usize = 8;

/// Line Up (`applyAlignment`) needs at least this many measurements.
pub const LINE_UP_HITS: usize = 4;

/// `pendingHits` keeps the newest 64.
const PENDING: usize = 64;
/// Multiply-adds of correlation per sample of the block: the measurement's cost is a share of
/// the block's time, whatever its size (a 512-sample block does about 65k, some 50 µs).
const MEASURE_PER_SAMPLE: usize = 128;

/// The correlation budget for a block of `frames` samples: [`MEASURE_PER_SAMPLE`] a sample, and
/// [`shale_align::Measurer::step`] always does at least one lag so a measurement always finishes.
pub(crate) fn measure_budget(frames: usize) -> usize {
    frames.saturating_mul(MEASURE_PER_SAMPLE)
}

/// `AlignReport`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlignReport {
    pub state: i32,
    /// The reference's Mic (`refName`), -1 for none yet.
    pub ref_instrument: i32,
    pub hits: usize,
    pub offset_ms: f64,
    pub spread_ms: f64,
    pub corr: f64,
    pub polarity: i32,
}

impl Default for AlignReport {
    fn default() -> Self {
        AlignReport { state: AL_LINK_OFF, ref_instrument: -1, hits: 0, offset_ms: 0.0, spread_ms: 0.0, corr: 0.0, polarity: 1 }
    }
}

impl AlignReport {
    /// The values written to [`Analysis::report`].
    pub fn to_values(&self) -> [f32; crate::analysis::REPORT_LEN] {
        let mut v = [0.0f32; crate::analysis::REPORT_LEN];
        v[REPORT_STATE] = self.state as f32;
        v[REPORT_HITS] = self.hits as f32;
        v[REPORT_OFFSET_MS] = self.offset_ms as f32;
        v[REPORT_SPREAD_MS] = self.spread_ms as f32;
        v[REPORT_POLARITY] = self.polarity as f32;
        v[REPORT_CORR] = self.corr as f32;
        v[REPORT_REF] = self.ref_instrument as f32;
        v[REPORT_MARK] = REPORT_VALID;
        v
    }
}

/// The processor's phase-alignment state (`runAlignment`'s, message thread in the C++).
#[derive(Debug)]
struct Alignment {
    /// `pendingHits`: own hit positions not measured yet, the newest [`PENDING`].
    pending: [i64; PENDING],
    pending_len: usize,
    /// `lastHitMeasured`: drops a hit seen twice.
    last_hit_measured: i64,
    estimator: Estimator,
    estimator_ref_slot: i64,
    report: AlignReport,
    measurer: Measurer,
}

impl Alignment {
    fn new() -> Self {
        Alignment {
            pending: [0; PENDING],
            pending_len: 0,
            last_hit_measured: -1,
            estimator: Estimator::default(),
            estimator_ref_slot: -1,
            report: AlignReport::default(),
            measurer: Measurer::default(),
        }
    }

    /// Appends this block's hits (`hitFifo` drained into `pendingHits`), keeping the newest 64.
    fn push_hits(&mut self, hits: &[i64]) {
        for &h in hits {
            if self.pending_len == PENDING {
                self.pending.copy_within(1.., 0);
                self.pending_len -= 1;
            }
            self.pending[self.pending_len] = h;
            self.pending_len += 1;
        }
    }

    /// `finish (st)`: the state, with the numbers of the last measurement kept.
    fn finish(&mut self, state: i32) {
        self.report.state = state;
    }

    /// The report's numbers from the estimator (`rep.hits = …` to the end of `runAlignment`).
    fn tally(&mut self, sr: f64) {
        self.report.hits = self.estimator.size();
        if self.estimator.size() == 0 {
            return self.finish(AL_LISTENING);
        }
        self.report.offset_ms = self.estimator.median() / sr * 1000.0;
        self.report.spread_ms = self.estimator.spread() / sr * 1000.0;
        self.report.polarity = self.estimator.polarity();
        self.report.corr = self.estimator.mean_corr();
        self.finish(AL_MEASURED);
    }
}

/// The measurement's windows at `sr`: `(maxLag, pre, post)`.
fn align_windows(sr: f64) -> (usize, usize, usize) {
    let max_lag = (shale_align::MAX_LAG_MS * 0.001 * sr).ceil().max(0.0) as usize;
    let pre = (shale_align::PRE_MS * 0.001 * sr).round().max(0.0) as usize;
    let post = (shale_align::POST_MS * 0.001 * sr).round().max(0.0) as usize;
    (max_lag, pre, post)
}

/// The Shale port.
///
/// `readouts`, the editor's polled values: `[0]` the gate gain, linear (`gainOut`), `[1]` the
/// detector envelope's peak over the block, linear (`detOut`), `[2]` the onset ratio's peak, dB
/// (`fluxOut`), `[3]` dB taken out of the highs by DeBleed (`hfCutOut`), `[4]` the trigger count
/// (`trigSeq`, masked by [`COUNT_MASK`]), `[5]` the gate state (0 closed, 1 opening, 2 open, 3
/// closing), `[6]` the last hit's velocity 0..1, `[7]` hits handed to another mic (`deferSeq`,
/// masked). The phase measurement's report is in [`Analysis::report`] ([`REPORT_STATE`] …).
pub struct Shale {
    p: Params,
    dsp: Box<ShaleDsp>,
    align: AlignDelay,
    /// `alignActive` (audio thread).
    align_active: bool,
    /// The wrapper's `lastPushed` and `forcePush`.
    last_pushed: [f32; NUM_PARAMS],
    force_push: bool,
    /// `reportedLatency`.
    reported_latency: i32,
    channels: usize,
    max_block: i32,
    /// Audio has run since the instance was made: a prepare is then the live one.
    processed: bool,
    /// The block's position on the host timeline (`blockStart`, Kit Link's clock).
    block_start: i64,
    analysis: Arc<Analysis>,
    /// Kit Link: this instance's slot in its group's slab.
    kit: KitLink,
    /// Kit Link's events in (`partnerEvents`, `neighbourEvents`) and out (`outEvents`), made in
    /// `new` with room for a block's worth; never grown on the audio thread.
    partner_events: Vec<LinkEvent>,
    neighbour_events: Vec<LinkEvent>,
    out_events: Vec<LinkEvent>,
    /// `monoScratch`, sized in `prepare`.
    mono: Vec<f32>,
    /// Phase alignment (`runAlignment`).
    al: Alignment,
    /// MIDI out of the block just run (step 11), taken by [`Plugin::midi_out`].
    midi: MidiOut,
    /// Samples from the start of the next block to the pending note-off; `-1`: none sounding.
    note_off_in: i64,
    /// The note the pending note-off closes.
    note_off_note: u8,
    /// The host's tempo (quarters per minute) for the sixteenth-note length.
    tempo: f64,
}

impl Default for Shale {
    fn default() -> Self {
        Self::new()
    }
}

impl Shale {
    pub fn new() -> Self {
        let mut s = Shale {
            p: Params::new(&SHALE_PARAMS),
            dsp: Box::new(ShaleDsp::new()),
            align: AlignDelay::default(),
            align_active: false,
            last_pushed: [0.0; NUM_PARAMS],
            force_push: true,
            reported_latency: -1,
            channels: 2,
            max_block: 512,
            processed: false,
            block_start: 0,
            analysis: Analysis::new(),
            kit: KitLink::default(),
            partner_events: Vec::with_capacity(LINK_EVENTS),
            neighbour_events: Vec::with_capacity(LINK_EVENTS),
            out_events: Vec::with_capacity(LINK_EVENTS),
            mono: Vec::new(),
            al: Alignment::new(),
            midi: MidiOut::new(),
            note_off_in: -1,
            note_off_note: 0,
            tempo: 120.0,
        };
        s.align.prepare(48000.0, 512 * 4);
        s.update_latency();
        s.analysis.set_report(&s.al.report.to_values());
        s
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

    /// `updateLatency`: the lookahead plus the align base while Align is on.
    fn update_latency(&mut self) {
        let want = self.dsp.latency_samples() + if self.align_active { self.align.latency() } else { 0 };
        if want != self.reported_latency {
            self.reported_latency = want;
        }
    }

    /// The alignment delay's target from the parameters (`setTarget` in `processBlock`).
    fn align_target(&mut self) {
        let byp = self.p.on(d::P_BYPASS);
        let offset = if byp { 0.0 } else { shale_align::knob_to_offset_ms(f64::from(self.p.v(d::P_ALIGN_OFFSET))) * 0.001 * self.dsp.sample_rate() };
        self.align.set_target(offset, !byp && self.p.v(d::P_ALIGN_FLIP) > 0.5);
    }

    /// `prepareToPlay` on this instance's own state (the first prepare, or the live one).
    fn prepare_to_play(&mut self, sr: f64, live: bool) {
        let old_sr = self.dsp.sample_rate();
        if live {
            // The C++ `prepare` builds from the engine's knobs: push what moved first.
            self.push_params();
        } else {
            // Before any audio this is the C++'s one `prepareToPlay` on a new instance, however
            // often the host prepares: a fresh engine (its `prepare` keeps state a second call
            // would see, such as the lookahead in samples worked out at the earlier rate).
            *self.dsp = ShaleDsp::new();
            self.last_pushed = [0.0; NUM_PARAMS];
        }
        let block = self.max_block.max(1);
        self.dsp.prepare(sr, block);
        if live && self.dsp.sample_rate() < old_sr {
            // The safety deviation (module docs): the lookahead fits the shorter delay line.
            self.dsp.reclamp_lookahead();
        }
        self.force_push = true;
        self.align.prepare(self.dsp.sample_rate(), block.saturating_mul(4));
        self.align_active = self.p.v(d::P_ALIGN) > 0.5;
        self.reported_latency = -1;
        // The forced push, which the C++ runs at the top of the next block (idempotent there).
        self.push_params();
        self.dsp.take_geometry_changed();
        self.update_latency();
        if live {
            self.seed_after_reset();
        }
        self.partner_events.clear();
        self.neighbour_events.clear();
        self.out_events.clear();
        // Kit Link starts afresh (module docs).
        self.kit.restart();
        self.al.pending_len = 0;
        self.al.last_hit_measured = -1;
        self.al.measurer.cancel();
    }

    /// The Rust-only seeding of a live reset (module docs): DeBleed's velocity tracking from 0,
    /// the align delay and polarity at their targets, and a cleared scope history for the UI.
    fn seed_after_reset(&mut self) {
        self.dsp.seed_after_reset();
        self.align_target();
        self.align.seat_at_target();
        let cleared = d::ScopeBin::default();
        for _ in 0..d::SCOPE_BINS {
            self.analysis.post.push_slice(&[cleared.peak, cleared.gain, cleared.det]);
        }
    }

    // ------------------------------------------------------------------ Kit Link

    /// The flags this instance publishes (`flagPhaseRef` from Phase Reference).
    fn presence_flags(&self) -> i32 {
        if self.p.v(d::P_PHASE_REF) > 0.5 { FLAG_PHASE_REF } else { 0 }
    }

    /// The shared audio of `processBlock`: this mic's input, before the gate.
    fn share_audio(&mut self, io: &[Vec<f32>], nc: usize, n: usize) {
        if self.kit.is_unavailable() || n > self.mono.len() || nc == 0 {
            return;
        }
        let g = 1.0 / nc as f32;
        for i in 0..n {
            let mut s = 0.0f32;
            for c in io.iter().take(nc) {
                s += c[i];
            }
            self.mono[i] = s * g;
        }
        self.kit.audio_write(&self.mono[..n], self.block_start, CLK_TRANSPORT, self.dsp.sample_rate());
    }

    /// The link input of `processBlock`: presence, `collect`, then each event at its offset in this
    /// block, clamped into it. The partner events stay for the hits ([`Shale::own_hits`]).
    fn link_in(&mut self, num_samples: usize) {
        self.partner_events.clear();
        self.neighbour_events.clear();
        if self.kit.is_unavailable() {
            return;
        }
        let (group, role, instrument) = (self.dsp.link_group(), self.dsp.link_role(), self.dsp.instrument());
        self.kit.publish_presence(group, role, instrument, self.presence_flags());
        // Reach back a little further than this block: a beacon from an instance the host called
        // first may sit just before our start.
        let guard = (0.012 * self.dsp.sample_rate()) as i64;
        self.kit.collect(
            group,
            role,
            CLK_TRANSPORT,
            self.block_start.saturating_sub(guard),
            self.block_start.saturating_add(num_samples as i64),
            &mut self.partner_events,
            &mut self.neighbour_events,
            instrument,
        );
        let last = num_samples.saturating_sub(1) as i64;
        let at = |pos: i64, start: i64| (pos.saturating_sub(start)).clamp(0, last) as i32;
        for e in &self.partner_events {
            self.dsp.add_partner_hit(e.velocity, at(e.pos, self.block_start));
        }
        for e in &self.neighbour_events {
            self.dsp.add_neighbour_beacon(e.level_db, at(e.pos, self.block_start));
        }
    }

    /// This mic's hits for its own phase measurement: its own triggers at their positions, and a
    /// partner's position for a hit it only opened on because the partner said so (its own stamp
    /// would sit a guard window late, on the ring); `runAlignment` drops repeats.
    fn own_hits(&mut self) {
        let mut found = [0i64; 64];
        let mut count = 0;
        for i in 0..self.dsp.num_triggers() {
            if count >= found.len() {
                break;
            }
            let t = self.dsp.trigger(i);
            if t.own {
                found[count] = self.block_start.saturating_add(i64::from(t.offset));
                count += 1;
            }
        }
        for e in &self.partner_events {
            if count < found.len() {
                found[count] = e.pos;
                count += 1;
            }
        }
        self.al.push_hits(&found[..count]);
    }

    /// The link output of `processBlock`: every beacon, and only a Lead's or a Pair's own triggers
    /// (a Follow republishing what it was told would trigger the pair for ever).
    fn link_out(&mut self) {
        self.out_events.clear();
        if self.kit.is_unavailable() {
            return;
        }
        for i in 0..self.dsp.num_beacons() {
            let b = self.dsp.beacon(i);
            if self.out_events.len() < self.out_events.capacity() {
                self.out_events.push(LinkEvent {
                    pos: self.block_start.saturating_add(i64::from(b.offset)),
                    level_db: b.level_db,
                    kind: EV_BEACON,
                    clock: CLK_TRANSPORT,
                    ..LinkEvent::default()
                });
            }
        }
        if self.dsp.link_role() != d::R_FOLLOW {
            for i in 0..self.dsp.num_triggers() {
                let t = self.dsp.trigger(i);
                if t.own && self.out_events.len() < self.out_events.capacity() {
                    self.out_events.push(LinkEvent {
                        pos: self.block_start.saturating_add(i64::from(t.offset)),
                        velocity: t.velocity,
                        kind: EV_TRIGGER,
                        clock: CLK_TRANSPORT,
                        ..LinkEvent::default()
                    });
                }
            }
        }
        self.kit.publish_events(&self.out_events);
    }

    /// What this instance published in its last block (tests and the UI's readings).
    pub fn link_events_out(&self) -> &[LinkEvent] {
        &self.out_events
    }

    /// `runAlignment`, a step per block (module docs): a measurement under way goes on; otherwise
    /// the C++ pass, which starts at most one measurement.
    fn run_alignment(&mut self, frames: usize) {
        let budget = measure_budget(frames);
        let sr = self.dsp.sample_rate();
        if self.al.measurer.busy() {
            if let Some(lag) = self.al.measurer.step(budget) {
                self.al.estimator.add(&lag);
                self.al.tally(sr);
            }
        } else {
            self.alignment_pass(sr, budget);
        }
        let mut v = self.al.report.to_values();
        v[REPORT_NO_SLOT] = if self.kit.is_linked() && self.kit.slot().is_none() { 1.0 } else { 0.0 };
        self.analysis.set_report(&v);
    }

    fn alignment_pass(&mut self, sr: f64, budget: usize) {
        let al = &mut self.al;
        if self.p.v(d::P_LINK) < 0.5 || self.kit.is_unavailable() {
            al.pending_len = 0;
            return al.finish(AL_LINK_OFF);
        }
        // A Phase Ref only measures itself when it has been pointed at a mic of its own.
        let align_to = self.p.v(d::P_ALIGN_TO).round() as i32 - 1;
        if self.p.v(d::P_PHASE_REF) > 0.5 && align_to < 0 {
            al.pending_len = 0;
            return al.finish(AL_IS_REFERENCE);
        }
        // The reference: with Align To on Group Ref, a live mic in this group that says it is one;
        // otherwise the named mic, in this group if there is one there, else in any group.
        let group = self.p.v(d::P_LINK_GROUP).round() as i32;
        let mut reference = None;
        for i in 0..shale_link::MAX_INSTANCES {
            let Some(p) = self.kit.presence(i) else { continue };
            if p.is_self {
                continue;
            }
            if align_to < 0 {
                if p.group == group && p.phase_ref {
                    reference = Some(p);
                    break;
                }
            } else if p.instrument == align_to && (reference.is_none() || p.group == group) {
                reference = Some(p);
            }
        }
        let Some(rf) = reference else {
            al.pending_len = 0;
            al.estimator.reset();
            al.estimator_ref_slot = -1;
            return al.finish(AL_NO_REFERENCE);
        };
        al.report.ref_instrument = rf.instrument;
        if rf.slot as i64 != al.estimator_ref_slot {
            al.estimator.reset();
            al.estimator_ref_slot = rf.slot as i64;
        }
        let Some(me) = self.kit.slot() else { return al.finish(AL_UNAVAILABLE) };

        let (max_lag, pre, post) = align_windows(sr);
        let n = pre + post;
        let own_newest = self.kit.audio_newest(me, sr);
        let ref_newest = self.kit.audio_newest(rf.slot, sr);
        let ready = own_newest.min(ref_newest);

        // Own triggers and a partner's positions for the same hit arrive together: one
        // measurement per hit, or the median counts it twice.
        let len = al.pending_len;
        al.pending[..len].sort_unstable();
        let same_hit = (0.030 * sr) as i64;
        let mut keep = 0usize;
        let mut started = false;
        for j in 0..len {
            let hit = al.pending[j];
            if keep > 0 && hit.saturating_sub(al.pending[keep - 1]) < same_hit {
                continue;
            }
            if al.last_hit_measured >= 0 && hit.saturating_sub(al.last_hit_measured).saturating_abs() < same_hit {
                continue;
            }
            let from = hit.saturating_sub(pre as i64);
            let not_there = ready >= 0 && from.saturating_add((n + max_lag) as i64) > ready;
            if not_there || started {
                // Not there yet, or a measurement already started this pass: next time.
                if (started && !not_there) || hit > ready.saturating_sub((RING_SIZE / 2) as i64) {
                    al.pending[keep] = hit;
                    keep += 1;
                }
                continue;
            }
            al.last_hit_measured = hit;
            let ok = match al.measurer.windows(n, max_lag) {
                Some((x, y)) => self.kit.audio_read(me, from, x, sr) && self.kit.audio_read(rf.slot, from.saturating_sub(max_lag as i64), y, sr),
                None => false,
            };
            if ok {
                al.measurer.start(n, max_lag, 0.3);
                started = true;
            }
        }
        al.pending_len = keep;
        if started {
            // The first lags now; the rest over the next blocks.
            if let Some(lag) = al.measurer.step(budget) {
                al.estimator.add(&lag);
            }
        }
        al.tally(sr);
    }

    /// Step 11 of `processBlock`: this block's hits as notes on channel 10 (see the module docs).
    /// Never allocates (`midi` is a fixed ring).
    fn midi_out_block(&mut self, frames: usize) {
        let last = frames.saturating_sub(1);
        let sr = self.dsp.sample_rate();
        let tempo = if self.tempo.is_finite() && self.tempo > 1.0 { self.tempo } else { 120.0 };
        let sixteenth = ((15.0 / tempo) * sr).round().max(1.0) as i64;
        let on = self.dsp.is_midi_out();
        let note = self.dsp.midi_note().clamp(0, 127) as u8;
        let hits = if on { self.dsp.num_triggers() } else { 0 };
        for i in 0..hits {
            let t = self.dsp.trigger(i);
            let at = usize::try_from(t.offset).unwrap_or(0).min(last);
            // The note still sounding closes first: where it was due, or at this hit.
            if self.note_off_in >= 0 {
                let off = usize::try_from(self.note_off_in).unwrap_or(0).min(at);
                self.midi.push(off as u32, [0x89, self.note_off_note, 0]);
                self.note_off_in = -1;
            }
            let vel = 1 + (t.velocity.clamp(0.0, 1.0) * 126.0).round() as u8;
            self.midi.push(at as u32, [0x99, note, vel]);
            self.note_off_in = (at as i64).saturating_add(sixteenth);
            self.note_off_note = note;
        }
        if self.note_off_in >= 0 {
            if self.note_off_in < frames as i64 {
                self.midi.push(self.note_off_in as u32, [0x89, self.note_off_note, 0]);
                self.note_off_in = -1;
            } else {
                self.note_off_in -= frames as i64;
            }
        }
    }

    /// The engine (tests and the UI's readings).
    pub fn engine(&self) -> &ShaleDsp {
        &self.dsp
    }

    /// The phase measurement's report (`getAlignReport`).
    pub fn align_report(&self) -> AlignReport {
        self.al.report
    }

    /// Kit Link's slot (tests).
    #[cfg(test)]
    pub(crate) fn kit(&self) -> &KitLink {
        &self.kit
    }

    /// The alignment delay (tests).
    #[cfg(test)]
    pub(crate) fn align_delay(&self) -> &AlignDelay {
        &self.align
    }
}

impl Plugin for Shale {
    fn info(&self) -> &'static PluginInfo {
        &SHALE_INFO
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.p.set(id, value).is_some()
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        // `prepareToPlay`: everything sized here for this rate and block (the delay lines grow
        // only when they must), then the forced push and the latency.
        self.channels = channels.min(2);
        self.max_block = i32::try_from(max_block.max(1)).unwrap_or(i32::MAX / 8);
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 { f64::from(sample_rate) } else { 48_000.0 };
        // `monoScratch.assign (max (1, block) * 4)` and the measurement's windows (grown only).
        let mono = (self.max_block.max(1) as usize).saturating_mul(4);
        if self.mono.len() < mono {
            self.mono.resize(mono, 0.0);
        }
        let (max_lag, pre, post) = align_windows(sr);
        self.al.measurer.reserve(pre + post, max_lag);
        let live = self.processed;
        self.prepare_to_play(sr, live);
    }

    fn reset(&mut self) {
        // `prepareToPlay` on the live instance at the same rate (module docs).
        let sr = self.dsp.sample_rate();
        self.prepare_to_play(sr, true);
        self.midi.clear();
        self.note_off_in = -1;
    }

    fn set_transport(&mut self, transport: &Transport) {
        // `getTimeInSamples`: Kit Link's clock (the host's sample timeline).
        self.block_start = transport.samples;
        self.tempo = transport.tempo;
    }

    fn midi_out(&mut self, out: &mut MidiOut) {
        out.take_from(&mut self.midi);
    }

    fn set_link(&mut self, link: Option<Arc<LinkSlab>>, member: usize) {
        // `claimSlot` in the group's slab (`releaseSlot` of the old one); atomic stores only.
        self.kit.set(link, member);
    }

    fn latency(&self) -> usize {
        usize::try_from(self.reported_latency).unwrap_or(0)
    }

    fn tail_samples(&self) -> usize {
        // max (getTailLengthSeconds = 0, the delays plus the detector band's ring).
        let align = if self.align_active { 2 * self.align.latency() } else { 0 };
        let ring = (0.1 * self.dsp.sample_rate()) as i64;
        usize::try_from(i64::from(self.dsp.latency_samples()) + i64::from(align) + ring).unwrap_or(0)
    }

    fn gain_reduction_db(&self) -> f32 {
        // The gate's depth right now, 0 when open (90 dB for a full mute, its own close depth).
        let g = self.dsp.gain_out;
        if g.is_finite() && g > 0.0 { (-20.0 * g.log10()).clamp(0.0, 90.0) } else { 90.0 }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.process_with_sidechain(io, frames, None);
    }

    fn process_with_sidechain(&mut self, io: &mut [Vec<f32>], frames: usize, key: Option<&[Vec<f32>]>) {
        self.processed = true;
        // ---- parameters. Push only what moved.
        self.push_params();
        if self.dsp.take_geometry_changed() {
            self.update_latency();
        }

        let nc = self.channels.min(io.len());
        let frames = io.iter().take(nc).map(Vec::len).fold(frames, usize::min);

        // ---- Kit Link: the shared audio, then link in, before processing.
        let link_on = self.dsp.is_link_on();
        if link_on {
            self.share_audio(io, nc, frames);
            self.link_in(frames);
        } else {
            self.kit.withdraw();
            self.partner_events.clear();
            self.neighbour_events.clear();
        }

        // ---- the sidechain, if the host gave us one
        let sc_channels = key.map_or(0, |k| k.len().min(2));
        self.dsp.process(io, nc, frames, key, sc_channels, Some(&self.analysis.post));

        // ---- this mic's hits, for its own phase measurement
        if link_on {
            self.own_hits();
        }

        // ---- the alignment delay, last
        let on = self.p.v(d::P_ALIGN) > 0.5;
        if on != self.align_active {
            self.align_active = on;
            self.update_latency();
        }
        if self.align_active {
            self.align_target();
            self.align.process(io, nc, frames);
        }

        // ---- link out
        if link_on {
            self.link_out();
        } else {
            self.out_events.clear();
        }

        // ---- MIDI out (module docs, step 11)
        self.midi_out_block(frames);
        self.dsp.clear_triggers();

        // ---- the phase measurement (the C++ message thread's timer)
        self.run_alignment(frames);
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        let e = &self.dsp;
        out[0] = e.gain_out;
        out[1] = e.det_out;
        out[2] = e.flux_out;
        out[3] = e.hf_cut_out;
        out[4] = (e.trig_seq & COUNT_MASK) as f32;
        out[5] = e.state_out as f32;
        out[6] = e.last_vel_out;
        out[7] = (e.defer_seq & COUNT_MASK) as f32;
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(Arc::clone(&self.analysis))
    }
}

#[cfg(test)]
mod tests;
