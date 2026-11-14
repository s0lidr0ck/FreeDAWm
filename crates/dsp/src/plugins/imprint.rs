//! Imprint: captures the impulse response of whatever sits between two points of the signal
//! path (another plug-in, outboard gear, a room through a speaker and a microphone).
//!
//! It plays an exponential sine sweep into the thing being measured and records what comes back;
//! [`deconvolve`] then divides the recording by the sweep and what is left is the impulse
//! response, ready to save as a file an IR loader or convolution reverb plays. A sweep puts far
//! more energy into the measurement than a click does, so the result sits well above the noise,
//! and with a sweep that rises exponentially the harmonics a device adds land *before* the
//! response, where they are cut off.
//!
//! One module has one of three jobs ([`ROLES`]):
//! - **Send** plays the sweep (its input is replaced: it is a measuring tool, not an effect).
//! - **Return** records its input and passes it on unchanged.
//! - **Loop** does both: the sweep goes out of the track, the response comes back into it (the
//!   track's output to the gear, the gear into the track's input).
//!
//! A capture runs when the transport starts: [`PRE_ROLL_S`] of silence, the sweep, then the Tail.
//! Send and Return each start from their own first playing block, so they need not be in step to
//! the sample: the response is found in the recording by its onset ([`deconvolve`]), which also
//! takes out whatever delay the path has. Stopping or locating before the end abandons the take.
//!
//! The recording lives in a [`Take`] the editor shares with the audio thread
//! ([`Analysis::extra`] holds the [`TakeHandle`]): every sample is an atomic store, so `process`
//! never allocates, locks or blocks. The take is allocated in `prepare` for the longest capture
//! at that rate; a `prepare` at the same rate and channel count keeps it (and the take in it).
//!
//! `readouts`: `[0]` state (0 idle, 1 capturing, 2 a take is ready), `[1]` progress 0..1, `[2]`
//! the input's peak during the capture in dBFS. [`Analysis::live`] carries the same three
//! ([`LIVE_STATE`], [`LIVE_PROGRESS`], [`LIVE_PEAK`], the peak linear).

use crate::analysis::Analysis;
use crate::params::{Params, choice, lin};
use crate::util::{frames_in, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Transport, Unit, db_to_gain, gain_to_db};
use rustfft::FftPlanner;
use rustfft::num_complex::Complex;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// What a module does: the `role` parameter's choices.
pub const ROLES: [&str; 3] = ["Send", "Return", "Loop"];
const ROLE_SEND: usize = 0;
const ROLE_RETURN: usize = 1;

/// Silence before the sweep, in seconds: room for a Return that started a few blocks after its
/// Send, and for the noise floor to be measured.
pub const PRE_ROLL_S: f32 = 0.5;
/// Recorded after the tail, in seconds (the path's own delay must not eat into the tail).
pub const POST_S: f32 = 0.25;
/// The sweep's length and the tail's, in seconds: their ranges.
pub const SWEEP_S: (f32, f32) = (1.0, 10.0);
pub const TAIL_S: (f32, f32) = (0.25, 10.0);
/// The sweep starts here, in Hz, and ends at [`sweep_top_hz`].
pub const SWEEP_LOW_HZ: f64 = 20.0;
/// Channels a take records (the first two of the track).
pub const TAKE_CHANNELS: usize = 2;

/// [`Analysis::live`] indices.
pub const LIVE_STATE: usize = 0;
pub const LIVE_PROGRESS: usize = 1;
pub const LIVE_PEAK: usize = 2;
const LIVE_LEN: usize = 3;

/// [`Take::state`] values.
pub const TAKE_IDLE: u32 = 0;
pub const TAKE_RECORDING: u32 = 1;
pub const TAKE_READY: u32 = 2;

const P_ROLE: usize = 0;
const P_LENGTH: usize = 1;
const P_TAIL: usize = 2;
const P_LEVEL: usize = 3;

static IMPRINT_PARAMS: [ParamInfo; 4] = [
    choice("role", "Role", &ROLES, 2),
    lin("length", "Sweep", SWEEP_S.0, SWEEP_S.1, 4.0, Unit::Seconds),
    lin("tail", "Tail", TAIL_S.0, TAIL_S.1, 3.0, Unit::Seconds),
    lin("level", "Level", -40.0, 0.0, -12.0, Unit::Db),
];

pub static IMPRINT_INFO: PluginInfo = PluginInfo {
    id: "imprint",
    name: "Imprint",
    short_name: "Imprint",
    category: Category::Other,
    params: &IMPRINT_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// Where the sweep ends at `rate`, in Hz: 22 kHz, or just under Nyquist at a lower rate.
pub fn sweep_top_hz(rate: f64) -> f64 {
    (0.46 * rate).min(22_000.0)
}

/// Sample `n` of a sweep `len` samples long at `rate`, at full scale: an exponential sine sweep
/// from [`SWEEP_LOW_HZ`] to [`sweep_top_hz`], faded in over its first tenth of a second and out
/// over its last hundredth. 0 outside `0..len`. The same function makes the sweep that is played
/// and the one the recording is divided by.
pub fn sweep_sample(n: usize, len: usize, rate: f64) -> f32 {
    if n >= len || len < 2 || !(rate.is_finite() && rate > 0.0) {
        return 0.0;
    }
    let seconds = len as f64 / rate;
    let octaves = (sweep_top_hz(rate) / SWEEP_LOW_HZ).ln();
    let tau = seconds / octaves;
    let t = n as f64 / rate;
    let phase = std::f64::consts::TAU * SWEEP_LOW_HZ * tau * ((t / tau).exp() - 1.0);
    let fade = |x: f64, over: f64| if x < over { 0.5 - 0.5 * (std::f64::consts::PI * x / over).cos() } else { 1.0 };
    let fade_in = (0.1 * rate).min(len as f64 / 10.0).max(1.0);
    let fade_out = (0.01 * rate).min(len as f64 / 10.0).max(1.0);
    (phase.sin() * fade(n as f64, fade_in) * fade((len - 1 - n) as f64, fade_out)) as f32
}

/// A capture's recording, shared by the audio thread (which writes it) and the editor (which
/// reads it once it is ready). Planar, [`TAKE_CHANNELS`] at most.
pub struct Take {
    rate: f32,
    channels: usize,
    capacity: usize,
    data: Box<[AtomicU32]>,
    state: AtomicU32,
    /// Frames recorded so far.
    frames: AtomicUsize,
    /// The sweep this take was made with: its length in frames and its level (linear, as bits).
    sweep_frames: AtomicUsize,
    level: AtomicU32,
    /// The tail asked for, in frames.
    tail_frames: AtomicUsize,
    /// The recording's peak (linear, as bits).
    peak: AtomicU32,
    /// Counts the takes finished: an editor sees a new one.
    serial: AtomicU32,
}

impl std::fmt::Debug for Take {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Take").field("rate", &self.rate).field("channels", &self.channels).field("state", &self.state()).finish()
    }
}

impl Take {
    /// A take for `channels` channels (1 or 2) of at most `capacity` frames at `rate`. Allocates.
    pub fn new(rate: f32, channels: usize, capacity: usize) -> Self {
        let channels = channels.clamp(1, TAKE_CHANNELS);
        Take {
            rate,
            channels,
            capacity,
            data: (0..channels.saturating_mul(capacity)).map(|_| AtomicU32::new(0)).collect(),
            state: AtomicU32::new(TAKE_IDLE),
            frames: AtomicUsize::new(0),
            sweep_frames: AtomicUsize::new(0),
            level: AtomicU32::new(0),
            tail_frames: AtomicUsize::new(0),
            peak: AtomicU32::new(0),
            serial: AtomicU32::new(0),
        }
    }

    pub fn rate(&self) -> f32 {
        self.rate
    }
    pub fn channels(&self) -> usize {
        self.channels
    }
    /// [`TAKE_IDLE`], [`TAKE_RECORDING`] or [`TAKE_READY`].
    pub fn state(&self) -> u32 {
        self.state.load(Ordering::Acquire)
    }
    /// Frames recorded so far.
    pub fn frames(&self) -> usize {
        self.frames.load(Ordering::Acquire).min(self.capacity)
    }
    /// The takes finished so far.
    pub fn serial(&self) -> u32 {
        self.serial.load(Ordering::Acquire)
    }
    /// The recording's peak in dBFS.
    pub fn peak_db(&self) -> f32 {
        gain_to_db(f32::from_bits(self.peak.load(Ordering::Relaxed)))
    }

    fn begin(&self, sweep_frames: usize, tail_frames: usize, level: f32) {
        self.frames.store(0, Ordering::Release);
        self.sweep_frames.store(sweep_frames, Ordering::Relaxed);
        self.tail_frames.store(tail_frames, Ordering::Relaxed);
        self.level.store(level.to_bits(), Ordering::Relaxed);
        self.peak.store(0, Ordering::Relaxed);
        self.state.store(TAKE_RECORDING, Ordering::Release);
    }

    fn write(&self, ch: usize, frame: usize, x: f32) {
        if ch < self.channels
            && frame < self.capacity
            && let Some(w) = self.data.get(ch * self.capacity + frame)
        {
            w.store(x.to_bits(), Ordering::Relaxed);
        }
    }

    fn abandon(&self) {
        self.state.store(TAKE_IDLE, Ordering::Release);
        self.frames.store(0, Ordering::Release);
    }

    fn finish(&self, frames: usize, peak: f32) {
        self.peak.store(peak.to_bits(), Ordering::Relaxed);
        self.frames.store(frames.min(self.capacity), Ordering::Release);
        self.serial.fetch_add(1, Ordering::AcqRel);
        self.state.store(TAKE_READY, Ordering::Release);
    }

    /// The impulse response this take holds ([`deconvolve`]); an error says why there is none
    /// (no take yet, one still running, silence). Allocates and takes a moment: never on the
    /// audio thread.
    pub fn impulse_response(&self) -> Result<CapturedIr, String> {
        match self.state() {
            TAKE_READY => {}
            TAKE_RECORDING => return Err("the capture is still running".into()),
            _ => return Err("nothing captured yet: press Play to run a capture".into()),
        }
        let serial = self.serial();
        let frames = self.frames();
        let recording: Vec<Vec<f32>> = (0..self.channels)
            .map(|ch| {
                let at = ch * self.capacity;
                self.data.get(at..at + frames).unwrap_or(&[]).iter().map(|w| f32::from_bits(w.load(Ordering::Relaxed))).collect()
            })
            .collect();
        // A capture that started while this was reading would have mixed two takes.
        if self.state() != TAKE_READY || self.serial() != serial {
            return Err("a new capture started: wait for it to finish".into());
        }
        let level = f32::from_bits(self.level.load(Ordering::Relaxed));
        deconvolve(&recording, self.rate, self.sweep_frames.load(Ordering::Relaxed), level, self.tail_frames.load(Ordering::Relaxed))
    }
}

/// The editor's way to a module's [`Take`] ([`Analysis::extra`] holds one of these). The audio
/// thread never touches the lock: `prepare` (off the audio thread) puts a new take here when the
/// rate or the channel count changes.
#[derive(Debug)]
pub struct TakeHandle(Mutex<Arc<Take>>);

impl TakeHandle {
    /// The take in use.
    pub fn take(&self) -> Arc<Take> {
        Arc::clone(&self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    fn replace(&self, take: Arc<Take>) {
        *self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = take;
    }
}

/// The take handle behind a module's analysis streams (`None`: not an Imprint module).
pub fn take_handle(analysis: &Analysis) -> Option<&TakeHandle> {
    analysis.extra.as_ref()?.downcast_ref::<TakeHandle>()
}

/// An impulse response made from a take.
#[derive(Debug, Clone, PartialEq)]
pub struct CapturedIr {
    pub rate: f32,
    /// One response per recorded channel, all the same length.
    pub channels: Vec<Vec<f32>>,
    /// The noise left in the measurement, in dB under the response's peak (`None`: the response
    /// began too early in the recording to tell).
    pub noise_db: Option<f32>,
}

impl CapturedIr {
    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }
}

/// The longest recording [`deconvolve`] takes, in frames (the longest capture at 192 kHz).
const MAX_RECORDING: usize = 4 << 20;

/// The impulse response in `recording` (one buffer per channel): the response of a path to a
/// [`sweep_sample`] sweep of `sweep_frames` at `level` (linear) that began somewhere in the
/// recording's first stretch. Each channel's spectrum is divided by the sweep's (where the sweep
/// has energy; elsewhere the result is left at nothing rather than at amplified noise), which
/// leaves the response at the lag the sweep was recorded with. The response is taken from half a
/// millisecond before its onset, so the path's delay is not part of it, for `tail_frames` at most, with a quiet end trimmed and faded.
///
/// The level is the path's own: a wire gives a click at unity (band-limited to the sweep's range,
/// so a little under 1 at its peak).
pub fn deconvolve(recording: &[Vec<f32>], rate: f32, sweep_frames: usize, level: f32, tail_frames: usize) -> Result<CapturedIr, String> {
    let rate64 = f64::from(rate);
    let len = recording.iter().map(Vec::len).min().unwrap_or(0);
    if recording.is_empty() || len == 0 {
        return Err("the capture is empty".into());
    }
    if !(rate.is_finite() && rate > 0.0) || sweep_frames < 2 || !(level.is_finite() && level > 0.0) {
        return Err("the capture has no usable sweep".into());
    }
    if len <= sweep_frames || len > MAX_RECORDING {
        return Err("the capture's length makes no sense for its sweep".into());
    }
    if recording.iter().flatten().any(|x| !x.is_finite()) {
        return Err("the capture holds non-finite samples".into());
    }
    let size = (len + sweep_frames).next_power_of_two();
    let mut planner = FftPlanner::<f32>::new();
    let (forward, inverse) = (planner.plan_fft_forward(size), planner.plan_fft_inverse(size));
    let mut sweep: Vec<Complex<f32>> = (0..size).map(|n| Complex::new(level * sweep_sample(n, sweep_frames, rate64), 0.0)).collect();
    forward.process(&mut sweep);
    let strongest = sweep.iter().map(Complex::norm_sqr).fold(0.0f32, f32::max);
    if !(strongest.is_finite() && strongest > 0.0) {
        return Err("the capture has no usable sweep".into());
    }
    // Under 50 dB below the sweep's strongest bin there is no sweep to divide by.
    let floor = strongest * 1.0e-5;
    let scale = 1.0 / size as f32;
    let mut buf = vec![Complex::new(0.0f32, 0.0); size];
    // Lags up to here were recorded to the sweep's end.
    let valid = len - sweep_frames;
    let mut responses: Vec<Vec<f32>> = Vec::with_capacity(recording.len());
    for ch in recording {
        for (b, x) in buf.iter_mut().zip(ch.iter().take(len).map(|x| Complex::new(*x, 0.0)).chain(std::iter::repeat(Complex::new(0.0, 0.0)))) {
            *b = x;
        }
        forward.process(&mut buf);
        for (y, x) in buf.iter_mut().zip(&sweep) {
            *y = *y * x.conj() * (scale / (x.norm_sqr() + floor));
        }
        inverse.process(&mut buf);
        responses.push(buf.iter().take(valid).map(|c| c.re).collect());
    }
    let peak = responses.iter().flatten().fold(0.0f32, |m, x| m.max(x.abs()));
    if !(peak.is_finite() && peak > 1.0e-7) {
        return Err("nothing came back: check the routing and the levels, then capture again".into());
    }
    // The onset: the first sample within 20 dB of the peak, or where the first hundredth of the
    // response's energy has arrived if that is sooner (a response that swells has no early
    // peak). A lower mark would catch the ripple a band-limited measurement puts before a click.
    let loud = (0..valid).find(|&i| responses.iter().any(|r| r.get(i).is_some_and(|x| x.abs() >= peak * 0.1))).unwrap_or(0);
    let power = |i: usize| responses.iter().filter_map(|r| r.get(i)).map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>();
    let total: f64 = (0..valid).map(power).sum();
    let mut sum = 0.0;
    let swell = (0..=loud)
        .find(|&i| {
            sum += power(i);
            sum >= total * 0.01
        })
        .unwrap_or(loud);
    let onset = loud.min(swell);
    let lead = (rate64 * 0.0005).round() as usize;
    let start = onset.saturating_sub(lead);
    // The noise: what the measurement holds before anything of the response.
    let quiet = start.saturating_sub(lead.saturating_mul(8));
    let noise_db = (quiet >= 256).then(|| {
        let sum: f64 = responses.iter().flat_map(|r| r.iter().take(quiet)).map(|x| f64::from(*x) * f64::from(*x)).sum();
        let rms = (sum / (quiet * responses.len()) as f64).sqrt() as f32;
        gain_to_db(rms / peak)
    });
    let want = tail_frames.max(1).min(valid - start);
    // Trim an end that has fallen 100 dB under the peak, then fade the last few milliseconds.
    let last = (start..start + want).rev().find(|&i| responses.iter().any(|r| r.get(i).is_some_and(|x| x.abs() >= peak * 1.0e-5))).unwrap_or(start);
    let out_len = (last + 1 - start).max(64.min(want));
    let fade = ((rate64 * 0.005).round() as usize).min(out_len / 4);
    let channels = responses
        .iter()
        .map(|r| {
            let mut out: Vec<f32> = r.get(start..start + out_len).unwrap_or(&[]).to_vec();
            let n = out.len();
            for (i, x) in out.iter_mut().enumerate().skip(n.saturating_sub(fade)) {
                *x *= (n - i) as f32 / (fade + 1) as f32;
            }
            out
        })
        .collect();
    Ok(CapturedIr { rate, channels, noise_db })
}

/// A capture in progress.
#[derive(Debug, Clone, Copy)]
struct Run {
    /// Frames since the transport started.
    at: usize,
    /// Where the next block is expected on the timeline.
    next_pos: i64,
    pre: usize,
    sweep: usize,
    /// Frames to record (0: this role records nothing) and, at least, to run for.
    record: usize,
    total: usize,
    level: f32,
    plays: bool,
    peak: f32,
}

pub struct Imprint {
    p: Params,
    sr: f32,
    ch: usize,
    take: Arc<Take>,
    handle: Arc<TakeHandle>,
    analysis: Arc<Analysis>,
    transport: Option<Transport>,
    was_playing: bool,
    run: Option<Run>,
    state: f32,
    progress: f32,
    peak_db: f32,
}

impl Default for Imprint {
    fn default() -> Self {
        Self::new()
    }
}

impl Imprint {
    pub fn new() -> Self {
        // An empty take until `prepare` sizes one.
        let take = Arc::new(Take::new(48_000.0, TAKE_CHANNELS, 0));
        let handle = Arc::new(TakeHandle(Mutex::new(Arc::clone(&take))));
        let extra: Arc<dyn std::any::Any + Send + Sync> = handle.clone();
        Imprint {
            p: Params::new(&IMPRINT_PARAMS),
            sr: 48_000.0,
            ch: 2,
            take,
            handle,
            analysis: Arc::new(Analysis::with_extra(LIVE_LEN, extra)),
            transport: None,
            was_playing: false,
            run: None,
            state: 0.0,
            progress: 0.0,
            peak_db: crate::MIN_DB,
        }
    }

    fn update(&mut self) {}

    /// The take this module records into.
    pub fn take(&self) -> Arc<Take> {
        Arc::clone(&self.take)
    }

    fn seconds(&self, s: f32) -> usize {
        (f64::from(s) * f64::from(self.sr)).round() as usize
    }

    /// The longest capture at this rate, in frames.
    fn capacity(&self) -> usize {
        self.seconds(PRE_ROLL_S + SWEEP_S.1 + TAIL_S.1 + POST_S)
    }

    fn start(&mut self, pos: i64) {
        let role = self.p.choice(P_ROLE);
        let (pre, sweep, tail) = (self.seconds(PRE_ROLL_S), self.seconds(self.p.v(P_LENGTH)), self.seconds(self.p.v(P_TAIL)));
        let level = db_to_gain(self.p.v(P_LEVEL));
        let records = role != ROLE_SEND;
        let record = if records { (pre + sweep + tail + self.seconds(POST_S)).min(self.take.capacity) } else { 0 };
        if records {
            self.take.begin(sweep, tail, level);
        }
        self.run =
            Some(Run { at: 0, next_pos: pos, pre, sweep, record, total: record.max(pre + sweep), level, plays: role != ROLE_RETURN, peak: 0.0 });
    }

    fn abandon(&mut self) {
        if let Some(run) = self.run.take()
            && run.record > 0
            && self.take.state() == TAKE_RECORDING
        {
            self.take.abandon();
        }
    }

    fn publish(&mut self) {
        let ready = self.take.state() == TAKE_READY;
        (self.state, self.progress) = match &self.run {
            Some(r) => (1.0, r.at as f32 / r.total.max(1) as f32),
            None if ready => (2.0, 1.0),
            None => (0.0, 0.0),
        };
        let peak = match &self.run {
            Some(r) => r.peak,
            None if ready => db_to_gain(self.take.peak_db()),
            None => 0.0,
        };
        self.peak_db = gain_to_db(peak);
        let live = &self.analysis.live;
        live.set(LIVE_STATE, self.state);
        live.set(LIVE_PROGRESS, self.progress);
        live.set(LIVE_PEAK, peak);
    }
}

impl Plugin for Imprint {
    crate::params::param_plumbing!(IMPRINT_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate).min(192_000.0);
        self.ch = sane_channels(channels);
        let (take_ch, capacity) = (self.ch.min(TAKE_CHANNELS), self.capacity());
        if self.take.rate != self.sr || self.take.channels != take_ch || self.take.capacity != capacity {
            self.take = Arc::new(Take::new(self.sr, take_ch, capacity));
            self.handle.replace(Arc::clone(&self.take));
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.abandon();
        self.was_playing = false;
        self.transport = None;
        self.publish();
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.transport = Some(*transport);
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        // A host that gives no transport never starts a capture.
        let (playing, pos) = self.transport.take().map_or((false, 0), |t| (t.playing, t.samples));
        if playing && !self.was_playing {
            self.abandon();
            self.start(pos);
        } else if self.run.is_some_and(|r| !playing || r.next_pos != pos) {
            // Stopped or located before the end.
            self.abandon();
        }
        self.was_playing = playing;
        let sends = self.p.choice(P_ROLE) != ROLE_RETURN;
        let rate = f64::from(self.sr);
        let mut run = self.run;
        for i in 0..frames {
            let mut out = 0.0;
            if let Some(r) = run.as_mut() {
                let at = r.at + i;
                if at < r.record {
                    for (c, b) in io.iter().enumerate().take(ch.min(TAKE_CHANNELS)) {
                        let x = b.get(i).copied().filter(|x| x.is_finite()).unwrap_or(0.0);
                        r.peak = r.peak.max(x.abs());
                        self.take.write(c, at, x);
                    }
                }
                if r.plays && at >= r.pre {
                    out = r.level * sweep_sample(at - r.pre, r.sweep, rate);
                }
            }
            if sends {
                for b in io.iter_mut().take(ch) {
                    if let Some(x) = b.get_mut(i) {
                        *x = out;
                    }
                }
            }
        }
        if let Some(r) = run.as_mut() {
            r.at += frames;
            r.next_pos = pos.saturating_add(frames as i64);
            if r.at >= r.total {
                if r.record > 0 {
                    self.take.finish(r.record, r.peak);
                }
                run = None;
            }
        }
        self.run = run;
        self.publish();
    }

    fn tail_samples(&self) -> usize {
        self.seconds(PRE_ROLL_S + self.p.v(P_LENGTH) + self.p.v(P_TAIL) + POST_S)
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        out[0] = self.state;
        out[1] = self.progress;
        out[2] = self.peak_db;
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(self.analysis.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;
    const BLOCK: usize = 512;

    fn module(set: &[(&str, f32)]) -> Imprint {
        let mut m = Imprint::new();
        for (id, v) in set {
            assert!(m.set_param(id, *v), "{id}");
        }
        m.prepare(SR, BLOCK, 2);
        m
    }

    fn transport(playing: bool, samples: i64) -> Transport {
        Transport { playing, samples, ..Transport::default() }
    }

    /// Plays `blocks` blocks from `pos`, handing each to `through` between a Send and a Return.
    fn capture(send: &mut Imprint, ret: &mut Imprint, blocks: usize, mut through: impl FnMut(&mut [Vec<f32>])) {
        for b in 0..blocks {
            let t = transport(true, (b * BLOCK) as i64);
            let mut io = vec![vec![0.0f32; BLOCK]; 2];
            send.set_transport(&t);
            send.process(&mut io, BLOCK);
            through(&mut io);
            ret.set_transport(&t);
            ret.process(&mut io, BLOCK);
        }
    }

    /// A causal FIR filter run over consecutive blocks.
    struct Fir {
        taps: Vec<f32>,
        history: Vec<Vec<f32>>,
    }

    impl Fir {
        fn new(taps: Vec<f32>) -> Self {
            let n = taps.len();
            Fir { taps, history: vec![vec![0.0; n]; 2] }
        }

        fn run(&mut self, io: &mut [Vec<f32>]) {
            for (b, h) in io.iter_mut().zip(self.history.iter_mut()) {
                for x in b.iter_mut() {
                    h.rotate_right(1);
                    h[0] = *x;
                    *x = h.iter().zip(&self.taps).map(|(a, t)| a * t).sum();
                }
            }
        }
    }

    /// A made-up device: two decaying resonances, the second arriving later (little at DC, which
    /// a sweep from 20 Hz cannot measure).
    fn device() -> Vec<f32> {
        (0..400)
            .map(|k| {
                let k = k as f32;
                let a = (0.3 * k).sin() * (-k / 40.0).exp();
                let b = if k >= 150.0 { 0.4 * (0.7 * (k - 150.0)).sin() * (-(k - 150.0) / 60.0).exp() } else { 0.0 };
                a + b
            })
            .collect()
    }

    fn blocks_for(m: &Imprint) -> usize {
        m.tail_samples().div_ceil(BLOCK) + 1
    }

    /// How alike `got` is to `want` at the best alignment within the first `search` samples
    /// (1: the same shape), with the gain of `got` against `want` there.
    fn likeness(got: &[f32], want: &[f32], search: usize) -> (f32, f32) {
        let energy = |x: &[f32]| x.iter().map(|v| v * v).sum::<f32>();
        (0..search)
            .filter_map(|lag| {
                let g = got.get(lag..lag + want.len())?;
                let dot: f32 = g.iter().zip(want).map(|(a, b)| a * b).sum();
                Some((dot / (energy(g) * energy(want)).sqrt(), (energy(g) / energy(want)).sqrt()))
            })
            .fold((0.0, 0.0), |best, x| if x.0 > best.0 { x } else { best })
    }

    #[test]
    fn the_sweep_rises_from_20_hz_and_stays_within_full_scale() {
        let len = 48_000;
        let s: Vec<f32> = (0..len).map(|n| sweep_sample(n, len, 48_000.0)).collect();
        assert!(s.iter().all(|x| x.abs() <= 1.0));
        assert!(s[0] == 0.0 && s[len - 1].abs() < 1.0e-3, "faded at both ends");
        assert_eq!(sweep_sample(len, len, 48_000.0), 0.0);
        // Zero crossings get closer together: the frequency only rises.
        let crossings: Vec<usize> = (1..len).filter(|&i| s[i - 1] < 0.0 && s[i] >= 0.0).collect();
        let gaps: Vec<usize> = crossings.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.first().is_some_and(|g| (1400..2500).contains(g)), "20 Hz and rising at first: {:?}", gaps.first());
        assert!(gaps.last().is_some_and(|g| *g <= 3), "about 22 kHz at the end: {:?}", gaps.last());
        assert!(gaps.windows(2).all(|w| w[1] <= w[0] + 1));
        assert_eq!(sweep_top_hz(32_000.0), 14_720.0);
    }

    #[test]
    fn a_send_and_a_return_capture_the_device_between_them() {
        let set = [("length", 1.5), ("tail", 0.25), ("level", -6.0)];
        let (mut send, mut ret) = (module(&[("role", 0.0)]), module(&[("role", 1.0)]));
        for (id, v) in set {
            send.set_param(id, v);
            ret.set_param(id, v);
        }
        let mut fir = Fir::new(device());
        let blocks = blocks_for(&ret);
        capture(&mut send, &mut ret, blocks, |io| fir.run(io));
        let take = ret.take();
        assert_eq!(take.state(), TAKE_READY);
        assert_eq!(take.serial(), 1);
        let ir = take.impulse_response().unwrap();
        assert_eq!(ir.channels.len(), 2);
        assert_eq!(ir.rate, SR);
        let (shape, gain) = likeness(&ir.channels[0], &device(), 64);
        assert!(shape > 0.999, "the device's response, shape {shape}");
        assert!((20.0 * gain.log10()).abs() < 0.1, "at its own level: {gain}");
        assert_eq!(ir.channels[0], ir.channels[1]);
        assert!(ir.noise_db.is_some_and(|n| n < -60.0), "{:?}", ir.noise_db);
        assert!(ir.frames() <= (0.25 * SR) as usize);
        let mut out = [f32::NAN; 8];
        ret.readouts(&mut out);
        assert_eq!((out[0], out[1]), (2.0, 1.0));
        // The Send recorded nothing.
        assert_eq!(send.take().state(), TAKE_IDLE);
        assert!(send.take().impulse_response().is_err());
    }

    #[test]
    fn a_loop_module_measures_a_wire_as_a_click_at_unity_and_takes_out_the_delay() {
        // Loop: what the module played comes back into it 300 samples later.
        let mut m = module(&[("length", 1.0), ("tail", 0.25)]);
        let mut delay = vec![0.0f32; 300];
        let mut sent = vec![0.0f32; BLOCK];
        for b in 0..blocks_for(&m) {
            let mut io = vec![vec![0.0f32; BLOCK]; 2];
            for (x, s) in io[0].iter_mut().zip(&sent) {
                delay.rotate_right(1);
                *x = delay[0];
                delay[0] = *s;
            }
            io[1] = io[0].iter().map(|x| -0.5 * x).collect();
            m.set_transport(&transport(true, (b * BLOCK) as i64));
            m.process(&mut io, BLOCK);
            sent.copy_from_slice(&io[0]);
        }
        let ir = m.take().impulse_response().unwrap();
        let peak_at = |c: &[f32]| c.iter().enumerate().fold((0, 0.0f32), |m, (i, x)| if x.abs() > m.1.abs() { (i, *x) } else { m });
        let (at, peak) = peak_at(&ir.channels[0]);
        assert!(at <= 30, "the delay is gone: the peak is {at} samples in");
        // A click with nothing above 22 kHz in it peaks at 22/24 of a full one.
        assert!((peak - 22.0 / 24.0).abs() < 0.03, "a wire is a click at unity: {peak}");
        let (at_r, peak_r) = peak_at(&ir.channels[1]);
        assert_eq!(at_r, at);
        assert!((peak_r / peak + 0.5).abs() < 0.01, "the other side at its own level and polarity: {peak_r}");
    }

    #[test]
    fn a_send_replaces_its_input_and_a_return_passes_it_on() {
        let mut send = module(&[("role", 0.0), ("level", 0.0)]);
        let mut io = vec![vec![0.7f32; BLOCK]; 2];
        send.process(&mut io, BLOCK);
        assert!(io.iter().flatten().all(|x| *x == 0.0), "silent while stopped");
        let mut heard = 0.0f32;
        for b in 0..80 {
            let mut io = vec![vec![0.7f32; BLOCK]; 2];
            send.set_transport(&transport(true, (b * BLOCK) as i64));
            send.process(&mut io, BLOCK);
            if b * BLOCK + BLOCK <= (PRE_ROLL_S * SR) as usize {
                assert!(io.iter().flatten().all(|x| *x == 0.0), "silent before the sweep");
            }
            assert_eq!(io[0], io[1]);
            heard = io[0].iter().fold(heard, |m, x| m.max(x.abs()));
        }
        assert!((0.99..=1.0).contains(&heard), "the sweep at its level: {heard}");
        let mut ret = module(&[("role", 1.0)]);
        let mut io = vec![vec![0.7f32; BLOCK], vec![-0.2f32; BLOCK]];
        ret.set_transport(&transport(true, 0));
        ret.process(&mut io, BLOCK);
        assert!(io[0].iter().all(|x| *x == 0.7) && io[1].iter().all(|x| *x == -0.2));
    }

    #[test]
    fn stopping_or_locating_abandons_the_take_and_the_next_play_starts_again() {
        let mut m = module(&[("length", 1.0), ("tail", 0.25)]);
        let block = |m: &mut Imprint, playing: bool, pos: usize| {
            let mut io = vec![vec![0.1f32; BLOCK]; 2];
            m.set_transport(&transport(playing, pos as i64));
            m.process(&mut io, BLOCK);
        };
        for b in 0..10 {
            block(&mut m, true, b * BLOCK);
        }
        assert_eq!(m.take().state(), TAKE_RECORDING);
        block(&mut m, false, 10 * BLOCK);
        assert_eq!(m.take().state(), TAKE_IDLE);
        assert!(m.take().impulse_response().unwrap_err().contains("nothing captured"));
        // A locate while playing.
        block(&mut m, true, 0);
        block(&mut m, true, BLOCK);
        block(&mut m, true, 40 * BLOCK);
        assert_eq!(m.take().state(), TAKE_IDLE);
        // Stop, then a whole run: the take is ready and survives Stop and a same-rate prepare.
        block(&mut m, false, 0);
        for b in 0..blocks_for(&m) {
            block(&mut m, true, b * BLOCK);
        }
        assert_eq!(m.take().state(), TAKE_READY);
        block(&mut m, false, 0);
        m.reset();
        m.prepare(SR, BLOCK, 2);
        assert_eq!(m.take().state(), TAKE_READY);
        // Only silence came back (a constant has nothing of the sweep in it worth the name).
        let take = m.take();
        m.prepare(96_000.0, BLOCK, 2);
        assert!(!Arc::ptr_eq(&take, &m.take()), "another rate, another take");
        assert_eq!(m.take().rate(), 96_000.0);
        let handle = m.analysis().unwrap();
        assert!(Arc::ptr_eq(&take_handle(&handle).unwrap().take(), &m.take()));
    }

    #[test]
    fn silence_and_nonsense_are_errors_not_responses() {
        let silent = vec![vec![0.0f32; 100_000]; 2];
        assert!(deconvolve(&silent, SR, 48_000, 0.5, 4800).unwrap_err().contains("nothing came back"));
        assert!(deconvolve(&[], SR, 48_000, 0.5, 4800).is_err());
        assert!(deconvolve(&silent, SR, 200_000, 0.5, 4800).is_err(), "a sweep longer than the recording");
        assert!(deconvolve(&silent, f32::NAN, 48_000, 0.5, 4800).is_err());
        assert!(deconvolve(&silent, SR, 48_000, 0.0, 4800).is_err());
        assert!(deconvolve(&[vec![f32::NAN; 100_000]], SR, 48_000, 0.5, 4800).is_err());
        assert!(deconvolve(&[vec![0.0; MAX_RECORDING + 1]], SR, 48_000, 0.5, 4800).is_err());
    }

    #[test]
    fn process_never_moves_the_take_and_a_mono_track_records_one_channel() {
        let mut m = module(&[]);
        m.prepare(SR, BLOCK, 1);
        assert_eq!(m.take().channels(), 1);
        let before = Arc::as_ptr(&m.take());
        for b in 0..blocks_for(&m) {
            let mut io = vec![vec![0.0f32; BLOCK]];
            m.set_transport(&transport(true, (b * BLOCK) as i64));
            m.process(&mut io, BLOCK);
        }
        assert_eq!(Arc::as_ptr(&m.take()), before);
        assert_eq!(m.take().state(), TAKE_READY);
        assert_eq!(m.latency(), 0);
    }
}
