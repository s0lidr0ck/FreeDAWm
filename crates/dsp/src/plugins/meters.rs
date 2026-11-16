//! Meter plug-ins and the channel utility: FreeDAWm's own (no VST behind them).
//!
//! - **Spectrum** (`spectrum`): passes the sound and streams its left channel on
//!   [`Analysis::pre`] and its right on [`Analysis::post`] (the same on a mono track); the editor
//!   runs the FFT ([`crate::spectrum::Analyzer`]) on the UI thread.
//! - **Loudness** (`loudness`): the session's BS.1770 meter ([`LoudnessMeter`], the one the
//!   bounce uses) on the insert, with the loudness range ([`LoudnessRange`]) and a history of the
//!   short-term loudness every 100 ms for the last ten minutes, shared with the editor through
//!   [`LoudnessHistory`]. The action `reset` starts again.
//! - **Phase** (`phase_meter`): correlation and balance over 300 ms, and the two channels
//!   streamed for a goniometer.
//! - **Tuner** (`tuner`): streams the mono sum; the editor reads the pitch with
//!   [`tuner_reading`]. Mute silences the output while tuning.
//! - **Channel Tool** (`channel_tool`): swap, mono, left or right only, mid/side encode and
//!   decode, polarity per channel, gain and a DC filter. A change of routing or polarity
//!   crossfades over 10 ms.
//!
//! None of them allocates or locks in `process`; the meters pass the sound untouched (Tuner's
//! Mute aside) and report no latency.

use crate::analysis::Analysis;
use crate::meter::{LoudnessMeter, LoudnessRange};
use crate::params::{Params, choice, lin, param_plumbing, toggle};
use crate::util::{DcBlocker, Smoother, frames_in, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

const fn info(id: &'static str, name: &'static str, short_name: &'static str, params: &'static [ParamInfo]) -> PluginInfo {
    PluginInfo {
        id,
        name,
        short_name,
        category: Category::Other,
        params,
        is_instrument: false,
        audiosuite: false,
        sidechain: false,
        link: None,
        references: 0,
        midi_out: false,
    }
}

/// Streams `io`'s first channel on `a.pre` and its second (or the first again) on `a.post`.
fn stream_lr(a: &Analysis, io: &[Vec<f32>], frames: usize) {
    if let Some(l) = io.first() {
        let n = frames.min(l.len());
        a.pre.push_slice(&l[..n]);
        let r = io.get(1).filter(|r| r.len() >= n).unwrap_or(l);
        a.post.push_slice(&r[..n]);
    }
}

// ============================================================================ Spectrum

static SPECTRUM_PARAMS: [ParamInfo; 4] = [
    choice("view", "View", &["Stereo Sum", "Left", "Right", "Mid", "Side", "Left and Right"], 0),
    choice("slope", "Slope", &["Flat", "3 dB/oct", "4.5 dB/oct"], 1),
    choice("speed", "Speed", &["Fast", "Medium", "Slow"], 1),
    toggle("hold", "Peak Hold", true),
];

pub static SPECTRUM_INFO: PluginInfo = info("spectrum", "Spectrum", "Spectrum", &SPECTRUM_PARAMS);

/// The spectrum analyser (see the module docs). Its parameters are the display's.
pub struct Spectrum {
    p: Params,
    analysis: Arc<Analysis>,
}

impl Default for Spectrum {
    fn default() -> Self {
        Self::new()
    }
}

impl Spectrum {
    pub fn new() -> Self {
        Self { p: Params::new(&SPECTRUM_PARAMS), analysis: Analysis::new() }
    }
    fn update(&mut self) {}
}

impl Plugin for Spectrum {
    param_plumbing!(SPECTRUM_INFO);
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}
    fn reset(&mut self) {}
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        stream_lr(&self.analysis, io, frames);
    }
    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(self.analysis.clone())
    }
}

// ============================================================================ Loudness

/// Loudness targets the meter offers, LUFS, as named in [`LOUDNESS_TARGETS`].
pub const LOUDNESS_TARGET_LUFS: [f32; 5] = [-14.0, -16.0, -18.0, -23.0, -24.0];
pub const LOUDNESS_TARGETS: [&str; 5] = ["-14 LUFS (streaming)", "-16 LUFS (podcast)", "-18 LUFS", "-23 LUFS (EBU R128)", "-24 LUFS (ATSC A/85)"];

static LOUDNESS_PARAMS: [ParamInfo; 2] =
    [choice("target", "Target", &LOUDNESS_TARGETS, 0), lin("tp_ceiling", "True Peak Ceiling", -3.0, 0.0, -1.0, Unit::Db)];

pub static LOUDNESS_INFO: PluginInfo = info("loudness", "Loudness Meter", "Loudness", &LOUDNESS_PARAMS);

/// [`Analysis::live`] indices of the loudness meter.
pub const LIVE_MOMENTARY: usize = 0;
pub const LIVE_SHORT_TERM: usize = 1;
pub const LIVE_INTEGRATED: usize = 2;
pub const LIVE_MAX_MOMENTARY: usize = 3;
pub const LIVE_TRUE_PEAK: usize = 4;
pub const LIVE_RANGE: usize = 5;
const LOUDNESS_LIVE: usize = 6;

/// Short-term values the history keeps: ten minutes at ten a second.
pub const HISTORY_LEN: usize = 6000;

/// The loudness meter's short-term history, written by the audio thread every 100 ms and read by
/// the editor (atomics only).
pub struct LoudnessHistory {
    values: Vec<AtomicU32>,
    written: AtomicUsize,
}

impl LoudnessHistory {
    fn new() -> Self {
        Self { values: (0..HISTORY_LEN).map(|_| AtomicU32::new(f32::NEG_INFINITY.to_bits())).collect(), written: AtomicUsize::new(0) }
    }

    fn push(&self, v: f32) {
        let n = self.written.load(Ordering::Relaxed);
        if let Some(slot) = self.values.get(n % HISTORY_LEN) {
            slot.store(v.to_bits(), Ordering::Relaxed);
        }
        self.written.store(n.wrapping_add(1), Ordering::Release);
    }

    fn clear(&self) {
        self.written.store(0, Ordering::Release);
    }

    /// The values written so far (at most [`HISTORY_LEN`]), oldest first, into `out` (cleared).
    pub fn read(&self, out: &mut Vec<f32>) {
        out.clear();
        let n = self.written.load(Ordering::Acquire);
        let have = n.min(HISTORY_LEN);
        for k in n - have..n {
            out.push(self.values.get(k % HISTORY_LEN).map_or(f32::NEG_INFINITY, |v| f32::from_bits(v.load(Ordering::Relaxed))));
        }
    }
}

/// The loudness meter (see the module docs).
pub struct Loudness {
    p: Params,
    sr: f32,
    meter: Option<LoudnessMeter>,
    range: LoudnessRange,
    /// Samples until the next 100 ms history step.
    tick: usize,
    history: Arc<LoudnessHistory>,
    analysis: Arc<Analysis>,
}

impl Default for Loudness {
    fn default() -> Self {
        Self::new()
    }
}

impl Loudness {
    pub fn new() -> Self {
        let history = Arc::new(LoudnessHistory::new());
        let extra: Arc<dyn std::any::Any + Send + Sync> = history.clone();
        Self {
            p: Params::new(&LOUDNESS_PARAMS),
            sr: 48_000.0,
            meter: None,
            range: LoudnessRange::new(),
            tick: 4800,
            history,
            analysis: Arc::new(Analysis::with_extra(LOUDNESS_LIVE, extra)),
        }
    }
    fn update(&mut self) {}

    fn publish(&self) {
        let live = &self.analysis.live;
        match &self.meter {
            Some(m) => {
                live.set(LIVE_MOMENTARY, m.momentary_lufs());
                live.set(LIVE_SHORT_TERM, m.short_term_lufs());
                live.set(LIVE_INTEGRATED, m.integrated_lufs());
                live.set(LIVE_MAX_MOMENTARY, m.max_momentary_lufs());
                live.set(LIVE_TRUE_PEAK, m.true_peak_db());
            }
            None => (0..LIVE_RANGE).for_each(|i| live.set(i, f32::NEG_INFINITY)),
        }
        live.set(LIVE_RANGE, self.range.range_lu());
    }
}

impl Plugin for Loudness {
    param_plumbing!(LOUDNESS_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.meter = Some(LoudnessMeter::new(self.sr, sane_channels(channels)));
        self.tick = ((self.sr * 0.1) as usize).max(1);
    }

    fn reset(&mut self) {
        // A transport locate keeps the reading: only the Reset action starts again.
    }

    fn action(&mut self, name: &str) -> bool {
        if name != "reset" {
            return false;
        }
        if let Some(m) = &mut self.meter {
            m.reset();
        }
        self.range.reset();
        self.history.clear();
        self.tick = (self.sr * 0.1) as usize;
        self.publish();
        true
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let Some(m) = &mut self.meter else { return };
        let frames = frames_in(io, frames, io.len());
        let step = ((self.sr * 0.1) as usize).max(1);
        let mut done = 0;
        // in pieces that end on the 100 ms steps, so each step sees the short-term value then
        while done < frames {
            let n = (frames - done).min(self.tick.max(1));
            m.process_range(io, done, n);
            done += n;
            self.tick = self.tick.saturating_sub(n);
            if self.tick == 0 {
                self.tick = step;
                let st = m.short_term_lufs();
                self.history.push(st);
                self.range.add(st);
            }
        }
        self.publish();
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(self.analysis.clone())
    }
}

// ============================================================================ Phase

static PHASE_PARAMS: [ParamInfo; 1] = [choice("scope", "Scope", &["Lissajous", "Polar"], 0)];

pub static PHASE_INFO: PluginInfo = info("phase_meter", "Phase Meter", "Phase", &PHASE_PARAMS);

/// [`Analysis::live`] indices of the phase meter: correlation (-1..1), balance (right over left,
/// dB) and the lowest correlation since the last `reset` action.
pub const LIVE_CORRELATION: usize = 0;
pub const LIVE_BALANCE: usize = 1;
pub const LIVE_MIN_CORRELATION: usize = 2;

/// The correlation of `l` and `r` from their running products (1 for silence or mono).
pub fn correlation(lr: f64, ll: f64, rr: f64) -> f32 {
    let d = (ll * rr).sqrt();
    if d < 1e-12 { 1.0 } else { (lr / d).clamp(-1.0, 1.0) as f32 }
}

/// The phase meter (see the module docs).
pub struct PhaseMeter {
    p: Params,
    coef: f64,
    lr: f64,
    ll: f64,
    rr: f64,
    min: f32,
    analysis: Arc<Analysis>,
}

impl Default for PhaseMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl PhaseMeter {
    pub fn new() -> Self {
        Self { p: Params::new(&PHASE_PARAMS), coef: 0.0, lr: 0.0, ll: 0.0, rr: 0.0, min: 1.0, analysis: Arc::new(Analysis::with_live(3)) }
    }
    fn update(&mut self) {}
}

impl Plugin for PhaseMeter {
    param_plumbing!(PHASE_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        // a 300 ms time constant
        self.coef = (-1.0 / (0.3 * f64::from(sane_sr(sample_rate)))).exp();
        self.reset();
    }

    fn reset(&mut self) {
        self.lr = 0.0;
        self.ll = 0.0;
        self.rr = 0.0;
    }

    fn action(&mut self, name: &str) -> bool {
        if name != "reset" {
            return false;
        }
        self.min = 1.0;
        true
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        stream_lr(&self.analysis, io, frames);
        let Some(l) = io.first() else { return };
        let r = io.get(1).unwrap_or(l);
        let a = self.coef;
        for (x, y) in l.iter().zip(r).take(frames) {
            let (x, y) = if x.is_finite() && y.is_finite() { (f64::from(*x), f64::from(*y)) } else { (0.0, 0.0) };
            self.lr = a * self.lr + (1.0 - a) * x * y;
            self.ll = a * self.ll + (1.0 - a) * x * x;
            self.rr = a * self.rr + (1.0 - a) * y * y;
        }
        let c = correlation(self.lr, self.ll, self.rr);
        // the hold counts only while there is something to measure
        if self.ll + self.rr > 1e-8 {
            self.min = self.min.min(c);
        }
        let live = &self.analysis.live;
        live.set(LIVE_CORRELATION, c);
        live.set(LIVE_BALANCE, if self.ll > 1e-12 && self.rr > 1e-12 { (10.0 * (self.rr / self.ll).log10()) as f32 } else { 0.0 });
        live.set(LIVE_MIN_CORRELATION, self.min);
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(self.analysis.clone())
    }
}

// ============================================================================ Tuner

static TUNER_PARAMS: [ParamInfo; 2] = [lin("reference", "A4", 415.0, 466.0, 440.0, Unit::Hz), toggle("mute", "Mute", false)];

pub static TUNER_INFO: PluginInfo = info("tuner", "Tuner", "Tuner", &TUNER_PARAMS);

/// What the tuner reads: the frequency, the nearest note (MIDI number) and how far from it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TunerReading {
    pub hz: f32,
    pub note: i32,
    pub cents: f32,
}

/// Note names for [`TunerReading::note`] modulo 12.
pub const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// The pitch of `samples` (mono, at `sample_rate`) against A4 = `a4` Hz, from 30 Hz to 2 kHz;
/// `None` for silence or a sound with no clear pitch. 4096 samples cover a low E at 48 kHz.
pub fn tuner_reading(samples: &[f32], sample_rate: f32, a4: f32) -> Option<TunerReading> {
    let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if peak.is_nan() || peak <= 1e-3 {
        return None;
    }
    let hz = crate::pitch_detect::yin(samples, sane_sr(sample_rate), 30.0, 2000.0, 0.15)?;
    let a4 = if a4.is_finite() && a4 > 0.0 { a4 } else { 440.0 };
    let semis = 12.0 * (hz / a4).log2() + 69.0;
    if !semis.is_finite() {
        return None;
    }
    let note = semis.round();
    Some(TunerReading { hz, note: note as i32, cents: (semis - note) * 100.0 })
}

/// The tuner (see the module docs).
pub struct Tuner {
    p: Params,
    sr: f32,
    gain: Smoother,
    analysis: Arc<Analysis>,
}

impl Default for Tuner {
    fn default() -> Self {
        Self::new()
    }
}

impl Tuner {
    pub fn new() -> Self {
        Self { p: Params::new(&TUNER_PARAMS), sr: 48_000.0, gain: Smoother::new(1.0), analysis: Analysis::new() }
    }
    fn update(&mut self) {
        self.gain.set(if self.p.on(1) { 0.0 } else { 1.0 });
    }
}

impl Plugin for Tuner {
    param_plumbing!(TUNER_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.gain.set_time(5.0, self.sr);
        self.update();
        self.gain.snap();
    }

    fn reset(&mut self) {}

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = io.len();
        let frames = frames_in(io, frames, ch);
        let mut chunk = [0.0f32; 256];
        let mut start = 0;
        while start < frames {
            let n = (frames - start).min(chunk.len());
            for (k, slot) in chunk[..n].iter_mut().enumerate() {
                let sum: f32 = io.iter().map(|c| c[start + k]).sum();
                *slot = sum / ch.max(1) as f32;
            }
            self.analysis.post.push_slice(&chunk[..n]);
            start += n;
        }
        if self.gain.settling() || self.gain.value() != 1.0 {
            for i in 0..frames {
                let g = self.gain.next();
                io.iter_mut().for_each(|c| c[i] *= g);
            }
        }
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(self.analysis.clone())
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        out[0] = self.sr;
    }
}

// ============================================================================ Channel Tool

/// How Channel Tool routes the first two channels.
pub const ROUTES: [&str; 7] = ["Stereo", "Swap", "Mono", "Left Only", "Right Only", "Mid/Side Encode", "Mid/Side Decode"];

static TOOL_PARAMS: [ParamInfo; 5] = [
    choice("route", "Route", &ROUTES, 0),
    toggle("invert_l", "Invert Left", false),
    toggle("invert_r", "Invert Right", false),
    lin("gain", "Gain", -24.0, 24.0, 0.0, Unit::Db),
    toggle("dc", "DC Filter", false),
];

pub static TOOL_INFO: PluginInfo = info("channel_tool", "Channel Tool", "ChanTool", &TOOL_PARAMS);

/// Fade length of a routing or polarity change.
const TOOL_FADE_MS: f32 = 10.0;

/// The routing and polarity Channel Tool applies.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ToolState {
    route: usize,
    inv_l: bool,
    inv_r: bool,
}

impl ToolState {
    /// `(l, r)` through this state. Mono is (L+R)/2 on both; mid/side encode gives M = (L+R)/2
    /// and S = (L-R)/2, decode L = M+S and R = M-S, so the two in a row give back the input.
    fn apply(self, l: f32, r: f32) -> (f32, f32) {
        let l = if self.inv_l { -l } else { l };
        let r = if self.inv_r { -r } else { r };
        match self.route {
            1 => (r, l),
            2 => {
                let m = 0.5 * (l + r);
                (m, m)
            }
            3 => (l, l),
            4 => (r, r),
            5 => (0.5 * (l + r), 0.5 * (l - r)),
            6 => (l + r, l - r),
            _ => (l, r),
        }
    }
}

/// Channel Tool (see the module docs). On a mono track only the left polarity, the gain and the
/// DC filter apply; channels past the second get the gain and the DC filter.
pub struct ChannelTool {
    p: Params,
    sr: f32,
    gain: Smoother,
    dc: Vec<DcBlocker>,
    cur: ToolState,
    from: ToolState,
    fade: usize,
    fade_len: usize,
}

impl Default for ChannelTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelTool {
    pub fn new() -> Self {
        let s = ToolState { route: 0, inv_l: false, inv_r: false };
        Self { p: Params::new(&TOOL_PARAMS), sr: 48_000.0, gain: Smoother::new(1.0), dc: Vec::new(), cur: s, from: s, fade: 0, fade_len: 480 }
    }

    fn wanted(&self) -> ToolState {
        ToolState { route: self.p.choice(0).min(ROUTES.len() - 1), inv_l: self.p.on(1), inv_r: self.p.on(2) }
    }

    fn update(&mut self) {
        self.gain.set(db_to_gain(self.p.v(3)));
    }
}

impl Plugin for ChannelTool {
    param_plumbing!(TOOL_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.gain.set_time(20.0, self.sr);
        self.fade_len = ((TOOL_FADE_MS * 0.001 * self.sr) as usize).max(1);
        self.dc = (0..sane_channels(channels)).map(|_| DcBlocker::new(5.0, self.sr)).collect();
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.gain.snap();
        self.cur = self.wanted();
        self.from = self.cur;
        self.fade = 0;
        self.dc.iter_mut().for_each(DcBlocker::reset);
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = io.len().min(self.dc.len());
        let frames = frames_in(io, frames, ch);
        let want = self.wanted();
        if want != self.cur && self.fade == 0 {
            self.from = self.cur;
            self.cur = want;
            self.fade = self.fade_len;
        }
        let dc_on = self.p.on(4);
        for i in 0..frames {
            let g = self.gain.next();
            if ch >= 2 {
                let (l, r) = (io[0][i], io[1][i]);
                let (mut a, mut b) = self.cur.apply(l, r);
                if self.fade > 0 {
                    let t = self.fade as f32 / self.fade_len as f32;
                    let (x, y) = self.from.apply(l, r);
                    a += (x - a) * t;
                    b += (y - b) * t;
                    self.fade -= 1;
                }
                io[0][i] = a;
                io[1][i] = b;
            } else if ch == 1 {
                let x = io[0][i];
                let s = |st: ToolState| if st.inv_l { -x } else { x };
                let mut a = s(self.cur);
                if self.fade > 0 {
                    a += (s(self.from) - a) * (self.fade as f32 / self.fade_len as f32);
                    self.fade -= 1;
                }
                io[0][i] = a;
            }
            for (c, dc) in io.iter_mut().zip(self.dc.iter_mut()).take(ch) {
                let x = c[i];
                let y = if dc_on { dc.process(x) } else { x };
                c[i] = y * g;
            }
        }
        // the filter's state follows the signal while it is off, so switching it on is quiet
        if !dc_on {
            self.dc.iter_mut().for_each(DcBlocker::reset);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, amp: f32, len: usize, sr: f32) -> Vec<f32> {
        (0..len).map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / sr).sin()).collect()
    }

    fn run(p: &mut dyn Plugin, io: &mut [Vec<f32>]) {
        let len = io[0].len();
        let mut pos = 0;
        while pos < len {
            let n = 512.min(len - pos);
            let mut buf: Vec<Vec<f32>> = io.iter().map(|c| c[pos..pos + n].to_vec()).collect();
            p.process(&mut buf, n);
            for (b, c) in buf.iter().zip(io.iter_mut()) {
                c[pos..pos + n].copy_from_slice(b);
            }
            pos += n;
        }
    }

    #[test]
    fn the_meters_pass_the_sound_untouched_and_report_no_latency() {
        for id in ["spectrum", "loudness", "phase_meter", "tuner"] {
            let mut p = crate::create(id).unwrap();
            assert!(p.info().listed(), "{id} is in the insert menu");
            p.prepare(48_000.0, 512, 2);
            let x = vec![tone(440.0, 0.5, 4096, 48_000.0), tone(330.0, 0.25, 4096, 48_000.0)];
            let mut io = x.clone();
            run(&mut *p, &mut io);
            assert_eq!(io, x, "{id}");
            assert_eq!(p.latency(), 0);
            assert!(p.analysis().is_some());
        }
    }

    /// A 1 kHz sine at -20 dBFS on both channels reads about -20 LUFS once three seconds have
    /// passed, at every rate, and its true peak about -20 dBTP; the history fills every 100 ms.
    #[test]
    fn the_loudness_meter_reads_a_known_tone_at_every_rate() {
        for sr in [44_100.0f32, 48_000.0, 96_000.0] {
            let mut p = Loudness::new();
            p.prepare(sr, 512, 2);
            let len = (sr * 4.0) as usize;
            let amp = 0.1 * 2f32.sqrt(); // -20 dBFS RMS
            let mut io = vec![tone(997.0, amp, len, sr), tone(997.0, amp, len, sr)];
            run(&mut p, &mut io);
            let a = p.analysis().unwrap();
            // two channels of -20 dB each: -20 + 3.01 - 0.691 + K-weighting at 1 kHz (~0)
            let st = a.live.get(LIVE_SHORT_TERM);
            assert!((st - (-17.0)).abs() < 0.6, "{sr}: short-term {st}");
            let it = a.live.get(LIVE_INTEGRATED);
            assert!((it - st).abs() < 0.3, "{sr}: integrated {it}");
            let tp = a.live.get(LIVE_TRUE_PEAK);
            assert!((tp - (-17.0)).abs() < 0.5, "{sr}: true peak {tp}");
            assert!(a.live.get(LIVE_RANGE) < 1.0, "a steady tone has no range");
            let mut h = Vec::new();
            let hist = a.extra.as_ref().unwrap().clone().downcast::<LoudnessHistory>().ok().unwrap();
            hist.read(&mut h);
            assert_eq!(h.len(), 40, "{sr}");
            assert!(p.action("reset"));
            hist.read(&mut h);
            assert!(h.is_empty());
        }
    }

    #[test]
    fn the_phase_meter_reads_mono_wide_and_inverted() {
        for (r_gain, want) in [(1.0f32, 1.0f32), (-1.0, -1.0)] {
            let mut p = PhaseMeter::new();
            p.prepare(48_000.0, 512, 2);
            let l = tone(200.0, 0.5, 48_000, 48_000.0);
            let r: Vec<f32> = l.iter().map(|v| v * r_gain).collect();
            run(&mut p, &mut [l, r]);
            let c = p.analysis().unwrap().live.get(LIVE_CORRELATION);
            assert!((c - want).abs() < 0.01, "{c}");
        }
        // two unrelated tones: about 0
        let mut p = PhaseMeter::new();
        p.prepare(48_000.0, 512, 2);
        run(&mut p, &mut [tone(200.0, 0.5, 48_000, 48_000.0), tone(317.0, 0.5, 48_000, 48_000.0)]);
        let a = p.analysis().unwrap();
        assert!(a.live.get(LIVE_CORRELATION).abs() < 0.1);
        assert!(a.live.get(LIVE_BALANCE).abs() < 0.2);
    }

    #[test]
    fn the_tuner_names_the_note_and_how_far_off_it_is() {
        for sr in [44_100.0f32, 48_000.0, 96_000.0] {
            // A2 a little sharp: 110 Hz * 2^(12/1200)
            let hz = 110.0 * 2f32.powf(12.0 / 1200.0);
            let r = tuner_reading(&tone(hz, 0.5, 8192, sr), sr, 440.0).unwrap();
            assert_eq!(r.note, 45, "{sr}");
            assert_eq!(NOTE_NAMES[(r.note % 12) as usize], "A");
            assert!((r.cents - 12.0).abs() < 2.0, "{sr}: {} cents", r.cents);
            // the same tone against A = 432 reads sharper
            let r432 = tuner_reading(&tone(hz, 0.5, 8192, sr), sr, 432.0).unwrap();
            assert!(r432.cents > r.cents + 25.0);
        }
        assert_eq!(tuner_reading(&[0.0; 4096], 48_000.0, 440.0), None);
        // Mute silences the output
        let mut t = Tuner::new();
        t.set_param("mute", 1.0);
        t.prepare(48_000.0, 512, 1);
        let mut io = vec![tone(440.0, 0.5, 2048, 48_000.0)];
        run(&mut t, &mut io);
        assert!(io[0].iter().all(|v| v.abs() < 1e-6));
    }

    #[test]
    fn channel_tool_routes_and_round_trips_mid_side() {
        let l = tone(200.0, 0.5, 4800, 48_000.0);
        let r = tone(300.0, 0.3, 4800, 48_000.0);
        let render = |sets: &[(&str, f32)], io: &mut [Vec<f32>]| {
            let mut t = ChannelTool::new();
            for (id, v) in sets {
                t.set_param(id, *v);
            }
            t.prepare(48_000.0, 512, 2);
            run(&mut t, io);
        };
        let mut io = vec![l.clone(), r.clone()];
        render(&[("route", 1.0)], &mut io);
        assert_eq!(io, [r.clone(), l.clone()], "swap");
        let mut io = vec![l.clone(), r.clone()];
        render(&[("route", 2.0)], &mut io);
        assert!(io[0] == io[1] && (io[0][100] - 0.5 * (l[100] + r[100])).abs() < 1e-6, "mono");
        let mut io = vec![l.clone(), r.clone()];
        render(&[("route", 5.0)], &mut io);
        render(&[("route", 6.0)], &mut io);
        for (a, b) in io[0].iter().zip(&l).chain(io[1].iter().zip(&r)) {
            assert!((a - b).abs() < 1e-6, "encode then decode gives the input back");
        }
        let mut io = vec![l.clone(), r.clone()];
        render(&[("invert_r", 1.0), ("gain", -6.0206)], &mut io);
        assert!((io[1][100] + 0.5 * r[100]).abs() < 1e-4 && (io[0][100] - 0.5 * l[100]).abs() < 1e-4);
        // the DC filter takes an offset away
        let mut io = vec![vec![0.3; 48_000], vec![0.3; 48_000]];
        render(&[("dc", 1.0)], &mut io);
        assert!(io[0][47_999].abs() < 1e-3);
    }

    /// A change of route while playing fades over 10 ms instead of jumping.
    #[test]
    fn channel_tool_fades_a_route_change() {
        let mut t = ChannelTool::new();
        t.prepare(48_000.0, 512, 2);
        let mut io = vec![vec![0.5; 512], vec![-0.5; 512]];
        t.process(&mut io, 512);
        t.set_param("route", 1.0);
        let mut io = vec![vec![0.5; 512], vec![-0.5; 512]];
        t.process(&mut io, 512);
        let steps = io[0].windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(steps < 0.01, "{steps}");
        assert!((io[0][511] + 0.5).abs() < 1e-6, "swapped once the fade is over");
    }
}
