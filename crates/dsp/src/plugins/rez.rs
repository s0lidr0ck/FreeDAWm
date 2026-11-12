//! Rez: a dynamic resonance suppressor.
//!
//! Rez listens for frequencies that stick out of the sound's own spectrum and turns down only
//! those, only while they stick out. Nothing is cut by a fixed curve: the reference is the signal
//! itself. Its constants were fitted to measurements of a reference suppressor on the test signal
//! in `tools/rez-tuning`, in both modes: its depth -10..20 is Depth 0..100 % here (its 10 is
//! 67 %), and its sharpness and selectivity 0..10 are 0..100 % (its 4.6 and 3.6 are 46 % and
//! 36 %); its fastest release is about 10 ms here.
//!
//! Per frame (an STFT: 2048 points up to 50 kHz, a new frame every N/8 samples, root-Hann windows
//! on the way in and out):
//!
//! 1. The power spectrum of each side (left and right, or mid and side), or of the sidechain key,
//!    with Link deciding how much of the other side's power each side also listens to, followed
//!    in time with Attack.
//! 2. FINE: that power averaged over a narrow band around each bin (Sharpness sets the width), in
//!    dB.
//! 3. ENVELOPE: FINE averaged, in dB, over a wide band (Selectivity sets the width). This is what
//!    the sound roughly looks like without its resonances.
//! 4. EXCESS = FINE - ENVELOPE. The part of it above a small threshold is a cut request, scaled
//!    by how hard Rez works at that frequency: a built-in tilt ([`tilt_weight`]: little at the
//!    low end, in full from about 4 kHz up) times the SENSITIVITY (the Low/High range and four
//!    bands that raise or lower it), limited to [`MAX_CUT_DB`]. Depth is a level gate on top:
//!    where the sound around a frequency is quieter than the gate, less of the excess counts, and
//!    more Depth lowers the gate, so quieter material is treated too.
//! 5. Each bin's cut rises at once and falls back with Release.
//! 6. The bins are scaled and the frame is resynthesised by overlap-add.
//!
//! With Link at 100 % both sides get the same cut, so the stereo image does not move. More than
//! two channels are always linked.
//!
//! Latency: one frame (N samples), always reported. Delta plays what is being removed. Every
//! buffer is allocated in `prepare`; `process` and `set_param` never allocate.
//!
//! For its editor Rez publishes the cut in force as [`LIVE_POINTS`] values per side on a log
//! frequency axis ([`Analysis::live`]: [`LIVE_CUT_A`], [`LIVE_CUT_B`]), and feeds the analysis
//! rings with the (delayed) input on `pre` and the output on `post`.

use crate::analysis::Analysis;
use crate::params::{Params, choice, lin, log, toggle};
use crate::util::{frames_in, sane_channels, sane_sr, time_coef};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// The deepest cut any one frequency gets, in dB.
pub const MAX_CUT_DB: f32 = 30.0;
/// Frames per frame length (the overlap).
const OVERLAP: usize = 8;
/// The knee of the Soft mode, in dB.
const SOFT_KNEE_DB: f32 = 6.0;
/// dB of cut per dB of excess above the threshold, in the Soft mode.
const SOFT_SLOPE: f32 = 0.82;
/// The Hard mode: dB of cut per dB of excess, counted from [`HARD_OFFSET_DB`] below the Soft
/// mode's threshold and with no knee; the tilt raised to [`HARD_TILT_POWER`] (so it reaches much
/// further down) and scaled by [`HARD_GAIN`]; the cut fading by a factor of e for every
/// [`HARD_GATE_DB`] the sound is below the gate, and levelling off towards [`HARD_MAX_CUT_DB`].
const HARD_SLOPE: f32 = 1.0;
const HARD_OFFSET_DB: f32 = 3.3;
const HARD_TILT_POWER: f32 = 0.38;
const HARD_GAIN: f32 = 1.42;
const HARD_GATE_DB: f32 = 11.0;
const HARD_MAX_CUT_DB: f32 = 26.0;
/// The narrowest and widest FINE band (Sharpness 100 % and 0 %), in octaves, and the curve of
/// the way between them (below 1: most of the narrowing happens early in the knob's travel).
const FINE_OCT: (f32, f32) = (0.02, 2.5);
const FINE_CURVE: f32 = 0.55;
/// How much of the slope each part of Sharpness's travel gets: (travel, factor) points joined by
/// straight lines. At the bottom the cuts are so broad they do little.
const SHARP_SLOPE: [(f32, f32); 5] = [(0.0, 0.5), (0.25, 0.73), (0.46, 1.0), (0.75, 1.22), (1.0, 1.12)];
/// The narrowest and widest ENVELOPE band (Selectivity 100 % and 0 %), in octaves, and the curve
/// of the way between them (above 1: most of the narrowing happens late in the knob's travel).
const ENV_OCT: (f32, f32) = (0.24, 4.0);
const ENV_CURVE: f32 = 2.3;
/// At the bottom of Selectivity everything is cut a little less: the slope there, rising to 1
/// by [`SELECT_FULL_AT`] of the knob's travel.
const SELECT_LOW_SLOPE: f32 = 0.8;
const SELECT_FULL_AT: f32 = 0.36;
/// The excess, in dB, above which Rez cuts.
const THRESHOLD_DB: f32 = 1.25;
/// Depth is a level gate: where the sound around a frequency (its ENVELOPE) is above the gate,
/// a resonance there is cut in full; for every dB the sound is below it, [`GATE_RATIO`] dB of the
/// excess no longer counts, eased in over [`GATE_KNEE_DB`]. The gate, in dBFS, at Depth 0 %, and
/// how far each percent of Depth lowers it.
const GATE_DB_AT_ZERO: f32 = 20.0;
const GATE_DB_PER_PERCENT: f32 = 1.29;
const GATE_RATIO: f32 = 0.27;
const GATE_KNEE_DB: f32 = 9.0;
/// The gate reads the level with this many dB per octave added around 1 kHz (so sound that falls
/// away toward the highs, as most does, is gated evenly).
const GATE_TILT_DB_PER_OCT: f32 = 3.0;
/// Below this much Depth the cut fades out altogether (0 % is off).
const DEPTH_FADE_PERCENT: f32 = 3.0;
/// How hard Rez works by frequency, before the range and the bands: (Hz, weight) points joined
/// by straight lines on log-log axes. It leaves the low end nearly alone and works in full from
/// about 4 kHz up.
const TILT: [(f32, f32); 10] = [
    (60.0, 0.015),
    (120.0, 0.022),
    (250.0, 0.052),
    (500.0, 0.098),
    (1000.0, 0.157),
    (2000.0, 0.337),
    (4000.0, 0.74),
    (8000.0, 0.78),
    (14_000.0, 1.05),
    (20_000.0, 1.05),
];
/// FINE never averages fewer than this many bins each side (the low end, where a fraction of an
/// octave is less than a bin).
const MIN_HALF_BINS: f32 = 1.0;
/// Power below this is silence (-200 dB): keeps the logarithm finite.
const POWER_FLOOR: f32 = 1.0e-20;
/// The most a sensitivity above 0 dB multiplies the cut by (+12 dB).
const MAX_WEIGHT: f32 = 4.0;
/// How quickly Mix and Trim glide to a new value, in ms.
const GLIDE_MS: f32 = 20.0;

/// The sensitivity bands.
pub const BANDS: usize = 4;
/// The points of the live cut curve per side, log-spaced from [`LIVE_MIN_HZ`] to [`LIVE_MAX_HZ`].
pub const LIVE_POINTS: usize = 96;
pub const LIVE_MIN_HZ: f32 = 20.0;
pub const LIVE_MAX_HZ: f32 = 20_000.0;
/// [`Analysis::live`]: the first side's cut in dB (left, mid, or everything when linked), then
/// the second side's (right or side; the same values as the first when there is one detector).
pub const LIVE_CUT_A: usize = 0;
pub const LIVE_CUT_B: usize = LIVE_POINTS;
/// How many live values Rez publishes.
pub const LIVE_LEN: usize = 2 * LIVE_POINTS;

/// The frequency of live point `i`, in Hz.
pub fn live_hz(i: usize) -> f32 {
    LIVE_MIN_HZ * (LIVE_MAX_HZ / LIVE_MIN_HZ).powf(i as f32 / (LIVE_POINTS - 1) as f32)
}

const P_DEPTH: usize = 0;
const P_SHARPNESS: usize = 1;
const P_SELECTIVITY: usize = 2;
const P_ATTACK: usize = 3;
const P_RELEASE: usize = 4;
const P_LOW: usize = 5;
const P_HIGH: usize = 6;
const P_MODE: usize = 7;
const P_MIX: usize = 8;
const P_DELTA: usize = 9;
const P_TRIM: usize = 10;
const P_STEREO: usize = 11;
const P_LINK: usize = 12;
const P_BALANCE: usize = 13;
/// The first band's first parameter; each band has [`BAND_PARAMS`]: on, freq, gain, q.
const P_BANDS: usize = 14;
const BAND_PARAMS: usize = 4;

/// The Stereo choices.
pub const STEREO_NAMES: [&str; 2] = ["L/R", "M/S"];
/// The Mode choices.
pub const MODE_NAMES: [&str; 2] = ["Soft", "Hard"];

/// A band's sensitivity range in dB, and its Q range.
pub const BAND_GAIN_DB: (f32, f32) = (-24.0, 12.0);
pub const BAND_Q: (f32, f32) = (0.3, 8.0);

static REZ_PARAMS: [ParamInfo; P_BANDS + BANDS * BAND_PARAMS] = [
    lin("depth", "Depth", 0.0, 100.0, 50.0, Unit::Percent),
    lin("sharpness", "Sharpness", 0.0, 100.0, 45.0, Unit::Percent),
    lin("selectivity", "Selectivity", 0.0, 100.0, 35.0, Unit::Percent),
    log("attack", "Attack", 1.0, 2000.0, 20.0, Unit::Ms),
    log("release", "Release", 5.0, 2000.0, 10.0, Unit::Ms),
    log("low", "Low", 20.0, 2000.0, 60.0, Unit::Hz),
    log("high", "High", 1000.0, 20000.0, 20000.0, Unit::Hz),
    choice("mode", "Mode", &MODE_NAMES, 0),
    lin("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
    toggle("delta", "Delta", false),
    lin("trim", "Trim", -12.0, 12.0, 0.0, Unit::Db),
    choice("stereo", "Stereo", &STEREO_NAMES, 0),
    lin("link", "Link", 0.0, 100.0, 100.0, Unit::Percent),
    lin("balance", "Balance", -100.0, 100.0, 0.0, Unit::Percent),
    toggle("b1_on", "Band 1", true),
    log("b1_freq", "Band 1 Freq", 20.0, 20000.0, 150.0, Unit::Hz),
    lin("b1_gain", "Band 1 Sensitivity", BAND_GAIN_DB.0, BAND_GAIN_DB.1, 0.0, Unit::Db),
    log("b1_q", "Band 1 Q", BAND_Q.0, BAND_Q.1, 1.0, Unit::None),
    toggle("b2_on", "Band 2", true),
    log("b2_freq", "Band 2 Freq", 20.0, 20000.0, 600.0, Unit::Hz),
    lin("b2_gain", "Band 2 Sensitivity", BAND_GAIN_DB.0, BAND_GAIN_DB.1, 0.0, Unit::Db),
    log("b2_q", "Band 2 Q", BAND_Q.0, BAND_Q.1, 1.0, Unit::None),
    toggle("b3_on", "Band 3", true),
    log("b3_freq", "Band 3 Freq", 20.0, 20000.0, 2500.0, Unit::Hz),
    lin("b3_gain", "Band 3 Sensitivity", BAND_GAIN_DB.0, BAND_GAIN_DB.1, 0.0, Unit::Db),
    log("b3_q", "Band 3 Q", BAND_Q.0, BAND_Q.1, 1.0, Unit::None),
    toggle("b4_on", "Band 4", true),
    log("b4_freq", "Band 4 Freq", 20.0, 20000.0, 8000.0, Unit::Hz),
    lin("b4_gain", "Band 4 Sensitivity", BAND_GAIN_DB.0, BAND_GAIN_DB.1, 0.0, Unit::Db),
    log("b4_q", "Band 4 Q", BAND_Q.0, BAND_Q.1, 1.0, Unit::None),
];

/// The parameter ids of band `i` (0-based): on, freq, gain, q.
pub const BAND_IDS: [[&str; BAND_PARAMS]; BANDS] = [
    ["b1_on", "b1_freq", "b1_gain", "b1_q"],
    ["b2_on", "b2_freq", "b2_gain", "b2_q"],
    ["b3_on", "b3_freq", "b3_gain", "b3_q"],
    ["b4_on", "b4_freq", "b4_gain", "b4_q"],
];

pub static REZ_INFO: PluginInfo = PluginInfo {
    id: "rez",
    name: "Rez",
    short_name: "Rez",
    category: Category::Dynamics,
    params: &REZ_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: true,
    link: None,
    references: 0,
    midi_out: false,
};

/// One sensitivity band: a bell on the sensitivity curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Band {
    pub on: bool,
    pub freq: f32,
    /// dB of sensitivity: above 0 Rez works harder there, below 0 less, at the bottom not at all.
    pub gain: f32,
    pub q: f32,
}

impl Band {
    /// The band's bell at `hz`, in dB (0 when off or flat).
    pub fn db_at(&self, hz: f32) -> f32 {
        if !self.on || self.gain.abs() < 1.0e-3 || self.freq <= 0.0 || hz <= 0.0 {
            return 0.0;
        }
        // An analogue peaking filter's magnitude.
        let a = 10.0f32.powf(self.gain / 40.0);
        let q = self.q.max(0.05);
        let x = hz / self.freq;
        let d = 1.0 - x * x;
        let (num, den) = (d * d + (x * a / q).powi(2), d * d + (x / (a * q)).powi(2));
        if den > 0.0 && num > 0.0 { 10.0 * (num / den).log10() } else { 0.0 }
    }
}

/// How much of the cut the Low/High range lets through at `hz` (0..1): all of it between Low and
/// High, fading out over half an octave outside.
pub fn range_weight(low: f32, high: f32, hz: f32) -> f32 {
    let fade = |octaves_outside: f32| {
        let t = (1.0 - octaves_outside * 2.0).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let high = high.max(low);
    if hz <= 0.0 || low <= 0.0 {
        0.0
    } else if hz < low {
        fade((low / hz).log2())
    } else if hz > high {
        fade((hz / high).log2())
    } else {
        1.0
    }
}

/// The sensitivity bands at `hz`, summed, in dB.
pub fn bands_db(bands: &[Band; BANDS], hz: f32) -> f32 {
    bands.iter().map(|b| b.db_at(hz)).sum()
}

/// How hard Rez works at `hz` before the range and the bands have their say: [`TILT`].
pub fn tilt_weight(hz: f32) -> f32 {
    let (first, last) = (TILT[0], TILT[TILT.len() - 1]);
    if hz.is_nan() || hz <= 0.0 {
        return 0.0;
    }
    if hz <= first.0 {
        // Below the table: in proportion to the frequency.
        return first.1 * hz / first.0;
    }
    for pair in TILT.windows(2) {
        if let [(f0, w0), (f1, w1)] = *pair
            && hz <= f1
        {
            let t = (hz / f0).ln() / (f1 / f0).ln();
            return w0 * (w1 / w0).powf(t);
        }
    }
    last.1
}

/// What the cut at `hz` is multiplied by: the tilt, the range and the bands (the last two
/// together at most [`MAX_WEIGHT`]).
fn weight_at(low: f32, high: f32, bands: &[Band; BANDS], hz: f32, hard: bool) -> f32 {
    let w = range_weight(low, high, hz) * db_to_gain(bands_db(bands, hz));
    let tilt = if hard { HARD_GAIN * tilt_weight(hz).powf(HARD_TILT_POWER) } else { tilt_weight(hz) };
    if w.is_finite() { w.clamp(0.0, MAX_WEIGHT) * tilt } else { 0.0 }
}

/// The frame length for a sample rate: 2048 up to 50 kHz, doubling with the rate.
pub fn frame_size_for(sr: f32) -> usize {
    let mut n = 2048usize;
    let mut limit = 50_000.0f32;
    while sr > limit && n < 16_384 {
        n *= 2;
        limit *= 2.0;
    }
    n
}

/// The cut for an excess of `over` dB above the threshold: `slope` dB per dB, eased in over
/// `knee` dB centred on the threshold.
#[inline]
fn cut_for(over: f32, slope: f32, knee: f32) -> f32 {
    if knee > 1.0e-3 && 2.0 * over.abs() <= knee {
        let t = over + knee * 0.5;
        slope * t * t / (2.0 * knee)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

/// `a` to `b` on a log scale (both positive).
#[inline]
fn log_lerp(a: f32, b: f32, t: f32) -> f32 {
    a * (b / a).powf(t)
}

/// The prefix sum `cum` (`cum[i]` = the sum of the first `i` values) read at a fractional
/// position, each value taken to fill one unit.
#[inline]
fn cum_at(cum: &[f64], x: f32) -> f64 {
    let last = cum.len().saturating_sub(1);
    let x = x.clamp(0.0, last as f32);
    let i = (x as usize).min(last);
    let frac = f64::from(x - i as f32);
    let a = cum.get(i).copied().unwrap_or(0.0);
    let b = cum.get(i + 1).copied().unwrap_or(a);
    a + (b - a) * frac
}

/// The mean of the values behind `cum` over the bins `lo..=hi` (fractional, each bin `k` taken
/// to span `k - 0.5 .. k + 0.5`).
#[inline]
fn band_mean(cum: &[f64], lo: f32, hi: f32) -> f32 {
    let a = lo + 0.5;
    let b = hi + 0.5;
    let width = f64::from((b - a).max(1.0e-3));
    ((cum_at(cum, b) - cum_at(cum, a)) / width) as f32
}

/// One side's detector: what it hears and the cut it has decided on, per bin.
struct Detector {
    power: Vec<f32>,
    /// `power` followed with Attack.
    detect: Vec<f32>,
    /// The cut in force, in dB (>= 0).
    cut: Vec<f32>,
    gain: Vec<f32>,
}

impl Detector {
    fn new(bins: usize) -> Self {
        Detector { power: vec![0.0; bins], detect: vec![0.0; bins], cut: vec![0.0; bins], gain: vec![1.0; bins] }
    }

    fn reset(&mut self) {
        self.power.fill(0.0);
        self.detect.fill(0.0);
        self.cut.fill(0.0);
        self.gain.fill(1.0);
    }
}

/// The per-bin tables the parameters come to, and the detectors' shared work space.
struct Tables {
    /// The FINE and ENVELOPE bands' edges (in bins) and the sensitivity weight.
    fine_lo: Vec<f32>,
    fine_hi: Vec<f32>,
    env_lo: Vec<f32>,
    env_hi: Vec<f32>,
    weight: Vec<f32>,
    /// What the gate adds to the level it reads at each bin, in dB ([`GATE_TILT_DB_PER_OCT`]).
    gate_tilt: Vec<f32>,
    cum: Vec<f64>,
    fine_db: Vec<f32>,
    env_db: Vec<f32>,
}

/// What `prepare` builds for one sample rate and channel count.
struct Engine {
    n: usize,
    hop: usize,
    /// Bins from DC to Nyquist: `n / 2 + 1`.
    bins: usize,
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    /// One spectrum per channel, and the key's likewise.
    spec: Vec<Vec<Complex<f32>>>,
    key_spec: Vec<Vec<Complex<f32>>>,
    /// The root-Hann window, and the same scaled for the way out (the inverse transform's `n` and
    /// the overlap's sum folded in).
    window: Vec<f32>,
    window_out: Vec<f32>,
    /// The last `n` input samples per channel, the key's likewise, and the overlap-add
    /// accumulator: rings read and written at `pos`.
    input: Vec<Vec<f32>>,
    key: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    pos: usize,
    /// Samples since the last frame.
    since: usize,
    /// Whether the key ring holds a key (the last block had one).
    keyed: bool,
    /// Two with two channels (left and right, or mid and side), else one for all.
    detectors: Vec<Detector>,
    tables: Tables,
    /// The bins each live point covers (first, last).
    live_bins: Vec<(usize, usize)>,
}

impl Engine {
    fn new(sr: f32, channels: usize) -> Self {
        let n = frame_size_for(sr);
        let hop = n / OVERLAP;
        let bins = n / 2 + 1;
        let mut planner = FftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(n);
        let inverse = planner.plan_fft_inverse(n);
        let scratch_len = forward.get_inplace_scratch_len().max(inverse.get_inplace_scratch_len());
        let zero = Complex::new(0.0f32, 0.0);
        let window: Vec<f32> = (0..n).map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos()).sqrt() as f32).collect();
        // window * window is a Hann, which sums to OVERLAP / 2 over the overlapping frames.
        let scale = 1.0 / (n as f32 * OVERLAP as f32 * 0.5);
        let window_out = window.iter().map(|w| w * scale).collect();
        let step = (LIVE_MAX_HZ / LIVE_MIN_HZ).powf(0.5 / (LIVE_POINTS - 1) as f32);
        let bin_of = |hz: f32| hz * n as f32 / sr;
        let live_bins = (0..LIVE_POINTS)
            .map(|i| {
                let hz = live_hz(i);
                let lo = (bin_of(hz / step).round().max(1.0) as usize).min(bins - 1);
                let hi = (bin_of(hz * step).round().max(1.0) as usize).min(bins - 1);
                (lo, hi.max(lo))
            })
            .collect();
        Engine {
            n,
            hop,
            bins,
            forward,
            inverse,
            scratch: vec![zero; scratch_len],
            spec: vec![vec![zero; n]; channels],
            key_spec: vec![vec![zero; n]; channels],
            window,
            window_out,
            input: vec![vec![0.0; n]; channels],
            key: vec![vec![0.0; n]; channels],
            output: vec![vec![0.0; n]; channels],
            pos: 0,
            since: 0,
            keyed: false,
            detectors: (0..if channels == 2 { 2 } else { 1 }).map(|_| Detector::new(bins)).collect(),
            tables: Tables {
                fine_lo: vec![0.0; bins],
                fine_hi: vec![0.0; bins],
                env_lo: vec![0.0; bins],
                env_hi: vec![0.0; bins],
                weight: vec![0.0; bins],
                gate_tilt: (0..bins).map(|k| if k == 0 { 0.0 } else { GATE_TILT_DB_PER_OCT * (k as f32 * sr / n as f32 / 1000.0).log2() }).collect(),
                cum: vec![0.0; bins + 1],
                fine_db: vec![0.0; bins],
                env_db: vec![0.0; bins],
            },
            live_bins,
        }
    }

    fn reset(&mut self) {
        for ring in self.input.iter_mut().chain(self.key.iter_mut()).chain(self.output.iter_mut()) {
            ring.fill(0.0);
        }
        self.pos = 0;
        self.since = 0;
        self.keyed = false;
        for d in &mut self.detectors {
            d.reset();
        }
    }
}

/// What the parameters come to, per frame.
#[derive(Debug, Clone, Copy, Default)]
struct Settings {
    /// dB of cut per dB of excess above the threshold.
    slope: f32,
    knee: f32,
    hard: bool,
    /// The level gate (Depth), as a detector power: a bin at this power gets half its cut.
    gate_power: f32,
    /// One-pole coefficients per frame.
    attack: f32,
    release: f32,
    mix: f32,
    /// Trim as a gain.
    trim: f32,
    delta: bool,
    mid_side: bool,
    /// How much of the other side's power each side also listens to (0..1).
    link: f32,
    /// How much of the cut each side gets (Balance).
    amount: [f32; 2],
}

/// The spectra of left and right turned into mid and side, in place.
fn to_mid_side(spec: &mut [Vec<Complex<f32>>]) {
    if let [l, r] = spec {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let (m, s) = ((*a + *b) * 0.5, (*a - *b) * 0.5);
            *a = m;
            *b = s;
        }
    }
}

/// The inverse of [`to_mid_side`].
fn to_left_right(spec: &mut [Vec<Complex<f32>>]) {
    if let [m, s] = spec {
        for (a, b) in m.iter_mut().zip(s.iter_mut()) {
            let (l, r) = (*a + *b, *a - *b);
            *a = l;
            *b = r;
        }
    }
}

/// Steps 1 to 5 for one detector, from its `power`: its `cut` and `gain`. Returns the deepest cut.
fn detect(d: &mut Detector, t: &mut Tables, s: &Settings, amount: f32) -> f32 {
    // 1. The detector follows the power with Attack.
    let mut sum = 0.0f64;
    if let Some(first) = t.cum.first_mut() {
        *first = 0.0;
    }
    for ((det, p), c) in d.detect.iter_mut().zip(&d.power).zip(t.cum.iter_mut().skip(1)) {
        let v = *p + (*det - *p) * s.attack;
        *det = if v.is_finite() && v > POWER_FLOOR { v } else { 0.0 };
        sum += f64::from(*det);
        *c = sum;
    }
    // 2. FINE, in dB.
    for ((f, lo), hi) in t.fine_db.iter_mut().zip(&t.fine_lo).zip(&t.fine_hi) {
        *f = 10.0 * (band_mean(&t.cum, *lo, *hi).max(0.0) + POWER_FLOOR).log10();
    }
    // 3. ENVELOPE: FINE averaged in dB.
    let mut sum = 0.0f64;
    for (f, c) in t.fine_db.iter().zip(t.cum.iter_mut().skip(1)) {
        sum += f64::from(*f);
        *c = sum;
    }
    for ((v, lo), hi) in t.env_db.iter_mut().zip(&t.env_lo).zip(&t.env_hi) {
        *v = band_mean(&t.cum, *lo, *hi);
    }
    // 4 and 5. The cut request (the excess counts for less the further the sound around it is
    // below the gate), and the cut that follows it.
    let gate_db = 10.0 * (s.gate_power.max(POWER_FLOOR)).log10();
    let mut deepest = 0.0f32;
    for ((((cut, f), v), w), tilt) in d.cut.iter_mut().zip(&t.fine_db).zip(&t.env_db).zip(&t.weight).zip(&t.gate_tilt) {
        // How far below the gate, eased in: knee * ln(1 + e^(below / knee)).
        let below = (gate_db - v - tilt) / GATE_KNEE_DB;
        let below = if below > 20.0 { below } else { below.exp().ln_1p() } * GATE_KNEE_DB;
        let want = if s.hard {
            let x = s.slope * (f - v - THRESHOLD_DB + HARD_OFFSET_DB).max(0.0) * w * amount * (-below / HARD_GATE_DB).exp();
            HARD_MAX_CUT_DB * (x / HARD_MAX_CUT_DB).tanh()
        } else {
            cut_for(f - v - THRESHOLD_DB - GATE_RATIO * below, s.slope, s.knee) * w * amount
        }
        .clamp(0.0, MAX_CUT_DB);
        let now = if want >= *cut { want } else { want + (*cut - want) * s.release };
        *cut = if now.is_finite() && now > 1.0e-4 { now } else { 0.0 };
        deepest = deepest.max(*cut);
    }
    // The gain per bin: the cut, lightly smoothed across neighbours.
    let last = d.cut.len().saturating_sub(1);
    for (k, g) in d.gain.iter_mut().enumerate() {
        let at = |i: usize| d.cut.get(i).copied().unwrap_or(0.0);
        let c = 0.25 * at(k.saturating_sub(1)) + 0.5 * at(k) + 0.25 * at((k + 1).min(last));
        *g = if c > 1.0e-4 { (-c * (std::f32::consts::LN_10 / 20.0)).exp() } else { 1.0 };
    }
    deepest
}

/// The resonance suppressor.
///
/// `gain_reduction_db` is the deepest cut at any frequency during the last block.
pub struct Rez {
    p: Params,
    sr: f32,
    ch: usize,
    engine: Option<Box<Engine>>,
    s: Settings,
    /// Mix and Trim as they glide, and the glide's coefficient per sample.
    mix_now: f32,
    trim_now: f32,
    glide: f32,
    gr_meter: f32,
    analysis: Arc<Analysis>,
}

impl Default for Rez {
    fn default() -> Self {
        Self::new()
    }
}

impl Rez {
    pub fn new() -> Self {
        let mut r = Rez {
            p: Params::new(&REZ_PARAMS),
            sr: 48_000.0,
            ch: 0,
            engine: None,
            s: Settings::default(),
            mix_now: 1.0,
            trim_now: 1.0,
            glide: 0.0,
            gr_meter: 0.0,
            analysis: Arc::new(Analysis::with_live(LIVE_LEN)),
        };
        r.update();
        r.mix_now = r.s.mix;
        r.trim_now = r.s.trim;
        r
    }

    /// The cut in force at each bin from DC to Nyquist, in dB: the deeper of the two sides'
    /// (empty before `prepare`). Not for the audio thread (it allocates).
    pub fn cut_db(&self) -> Vec<f32> {
        let Some(e) = self.engine.as_ref() else { return Vec::new() };
        (0..e.bins).map(|k| e.detectors.iter().filter_map(|d| d.cut.get(k)).fold(0.0f32, |m, c| m.max(*c))).collect()
    }

    /// The frequency of bin `k` of [`Rez::cut_db`], in Hz.
    pub fn bin_hz(&self, k: usize) -> f32 {
        self.engine.as_ref().map_or(0.0, |e| k as f32 * self.sr / e.n as f32)
    }

    /// The sensitivity bands as the parameters stand.
    fn bands(&self) -> [Band; BANDS] {
        std::array::from_fn(|i| {
            let at = P_BANDS + i * BAND_PARAMS;
            Band { on: self.p.on(at), freq: self.p.v(at + 1), gain: self.p.v(at + 2), q: self.p.v(at + 3) }
        })
    }

    /// Works the parameters out into [`Settings`] and the per-bin tables. No allocation.
    fn update(&mut self) {
        let sharp = self.p.v(P_SHARPNESS) / 100.0;
        let select = self.p.v(P_SELECTIVITY) / 100.0;
        let hard = self.p.choice(P_MODE) == 1;
        let depth = self.p.v(P_DEPTH);
        let select_slope = SELECT_LOW_SLOPE + (1.0 - SELECT_LOW_SLOPE) * (select / SELECT_FULL_AT).clamp(0.0, 1.0);
        let sharp_slope = SHARP_SLOPE
            .windows(2)
            .find_map(|w| match *w {
                [(x0, y0), (x1, y1)] if sharp <= x1 => Some(y0 + (y1 - y0) * ((sharp - x0) / (x1 - x0)).clamp(0.0, 1.0)),
                _ => None,
            })
            .unwrap_or(1.0);
        self.s.slope = if hard { HARD_SLOPE } else { SOFT_SLOPE } * select_slope * sharp_slope * (depth / DEPTH_FADE_PERCENT).clamp(0.0, 1.0);
        self.s.hard = hard;
        self.s.knee = if hard { 0.0 } else { SOFT_KNEE_DB };
        self.s.mix = self.p.v(P_MIX) / 100.0;
        self.s.trim = db_to_gain(self.p.v(P_TRIM));
        self.s.delta = self.p.on(P_DELTA);
        self.s.mid_side = self.p.choice(P_STEREO) == 1;
        self.s.link = self.p.v(P_LINK) / 100.0;
        let balance = self.p.v(P_BALANCE) / 100.0;
        self.s.amount = [1.0 - balance.max(0.0), 1.0 + balance.min(0.0)];
        let bands = self.bands();
        let Some(e) = self.engine.as_mut() else { return };
        let frame_ms = 1000.0 * e.hop as f32 / self.sr;
        let coef = |ms: f32| (-frame_ms / ms.max(0.01)).exp();
        self.s.attack = coef(self.p.v(P_ATTACK));
        self.s.release = coef(self.p.v(P_RELEASE));

        // Half a band, as a frequency ratio.
        let fine = 2.0f32.powf(0.5 * log_lerp(FINE_OCT.1, FINE_OCT.0, sharp.powf(FINE_CURVE)));
        let env = 2.0f32.powf(0.5 * log_lerp(ENV_OCT.1, ENV_OCT.0, select.powf(ENV_CURVE)));
        // The gate as a detector power: a full-scale sine's bin holds (n / pi)^2 (the root-Hann
        // window's sum, halved, squared).
        let full_scale = (e.n as f32 / std::f32::consts::PI).powi(2);
        self.s.gate_power = full_scale * db_to_gain(GATE_DB_AT_ZERO - GATE_DB_PER_PERCENT * depth).powi(2);
        let top = (e.bins - 1) as f32;
        let hz_per_bin = self.sr / e.n as f32;
        let (low, high) = (self.p.v(P_LOW), self.p.v(P_HIGH));
        let t = &mut e.tables;
        let edges = t.fine_lo.iter_mut().zip(t.fine_hi.iter_mut()).zip(t.env_lo.iter_mut().zip(t.env_hi.iter_mut()));
        for (k, (((fine_lo, fine_hi), (env_lo, env_hi)), weight)) in edges.zip(t.weight.iter_mut()).enumerate() {
            let kf = k as f32;
            *fine_lo = (kf / fine).min(kf - MIN_HALF_BINS).max(-0.5);
            *fine_hi = (kf * fine).max(kf + MIN_HALF_BINS).min(top + 0.5);
            // The envelope is at least twice as wide as the fine band, and never reaches below
            // bin 1 (DC is not part of the sound).
            let half = (kf - *fine_lo).max(*fine_hi - kf) * 2.0;
            *env_lo = (kf / env).min(kf - half).max(0.5);
            *env_hi = (kf * env).max(kf + half).min(top + 0.5);
            *weight = if k == 0 { 0.0 } else { weight_at(low, high, &bands, kf * hz_per_bin, hard) };
        }
    }

    /// One frame: analyse, work out the cuts, resynthesise into the overlap-add ring.
    fn frame(e: &mut Engine, s: &Settings) -> f32 {
        let (n, pos) = (e.n, e.pos);
        // Analysis: each channel's last `n` samples, oldest first, windowed.
        for (spec, ring) in e.spec.iter_mut().zip(&e.input) {
            let (newer, older) = ring.split_at(pos.min(ring.len()));
            for ((bin, x), w) in spec.iter_mut().zip(older.iter().chain(newer)).zip(&e.window) {
                *bin = Complex::new(x * w, 0.0);
            }
            e.forward.process_with_scratch(spec, &mut e.scratch);
        }
        if e.keyed {
            for (spec, ring) in e.key_spec.iter_mut().zip(&e.key) {
                let (newer, older) = ring.split_at(pos.min(ring.len()));
                for ((bin, x), w) in spec.iter_mut().zip(older.iter().chain(newer)).zip(&e.window) {
                    *bin = Complex::new(x * w, 0.0);
                }
                e.forward.process_with_scratch(spec, &mut e.scratch);
            }
        }
        let sides = e.detectors.len() == 2;
        let mid_side = s.mid_side && sides;
        if mid_side {
            to_mid_side(&mut e.spec);
            if e.keyed {
                to_mid_side(&mut e.key_spec);
            }
        }

        // What each detector hears: its own side (or every channel), then Link's share of the
        // other side.
        let heard = if e.keyed { &e.key_spec } else { &e.spec };
        for d in &mut e.detectors {
            d.power.fill(0.0);
        }
        for (c, spec) in heard.iter().enumerate() {
            let Some(d) = e.detectors.get_mut(if sides { c } else { 0 }) else { continue };
            for (p, bin) in d.power.iter_mut().zip(spec.iter()) {
                *p += bin.norm_sqr();
            }
        }
        // (Scaled back to one channel's worth, so the level gate reads the same however many
        // channels are heard.)
        if let [a, b] = e.detectors.as_mut_slice() {
            let scale = 1.0 / (1.0 + s.link);
            for (pa, pb) in a.power.iter_mut().zip(b.power.iter_mut()) {
                let (x, y) = (*pa, *pb);
                *pa = (x + s.link * y) * scale;
                *pb = (y + s.link * x) * scale;
            }
        } else if let Some(d) = e.detectors.first_mut() {
            let scale = 1.0 / heard.len().max(1) as f32;
            for p in &mut d.power {
                *p *= scale;
            }
        }
        let mut deepest = 0.0f32;
        for (i, d) in e.detectors.iter_mut().enumerate() {
            let amount = if sides { s.amount.get(i).copied().unwrap_or(1.0) } else { 1.0 };
            deepest = deepest.max(detect(d, &mut e.tables, s, amount));
        }

        // 6. Scale the bins (and their mirror images) and overlap-add the frame.
        for (c, spec) in e.spec.iter_mut().enumerate() {
            let Some(d) = e.detectors.get(if sides { c } else { 0 }) else { continue };
            for (k, g) in d.gain.iter().enumerate() {
                if let Some(bin) = spec.get_mut(k) {
                    *bin *= *g;
                }
                if k > 0
                    && k < n - k
                    && let Some(bin) = spec.get_mut(n - k)
                {
                    *bin *= *g;
                }
            }
        }
        if mid_side {
            to_left_right(&mut e.spec);
        }
        for (spec, out) in e.spec.iter_mut().zip(e.output.iter_mut()) {
            e.inverse.process_with_scratch(spec, &mut e.scratch);
            let (newer, older) = out.split_at_mut(pos.min(n));
            for ((o, bin), w) in older.iter_mut().chain(newer.iter_mut()).zip(spec.iter()).zip(&e.window_out) {
                *o += bin.re * w;
            }
        }
        deepest
    }

    /// The live cut curves for the editor, from the detectors as they stand.
    fn publish(e: &Engine, analysis: &Analysis) {
        for (side, at) in [(0usize, LIVE_CUT_A), (1, LIVE_CUT_B)] {
            let Some(d) = e.detectors.get(side).or(e.detectors.first()) else { continue };
            for (i, (lo, hi)) in e.live_bins.iter().enumerate() {
                let cut = d.cut.get(*lo..=*hi).map_or(0.0, |c| c.iter().fold(0.0f32, |m, v| m.max(*v)));
                analysis.live.set(at + i, cut);
            }
        }
    }

    fn run(&mut self, io: &mut [Vec<f32>], frames: usize, key: Option<&[Vec<f32>]>) {
        let s = self.s;
        let Some(e) = self.engine.as_mut() else { return };
        let ch = self.ch.min(io.len());
        if ch == 0 {
            return;
        }
        let frames = frames_in(io, frames, ch);
        // A key that does not cover the block is no key.
        let key = key.filter(|k| !k.is_empty() && k.iter().take(ch).all(|b| b.len() >= frames));
        e.keyed = key.is_some();
        let per_channel = 1.0 / ch as f32;
        let mut deepest = 0.0f32;
        let mut framed = false;
        for i in 0..frames {
            let pos = e.pos;
            self.mix_now = s.mix + (self.mix_now - s.mix) * self.glide;
            self.trim_now = s.trim + (self.trim_now - s.trim) * self.glide;
            let (mut pre, mut post) = (0.0f32, 0.0f32);
            for (c, buf) in io.iter_mut().take(ch).enumerate() {
                let (Some(x), Some(inp), Some(out)) =
                    (buf.get_mut(i), e.input.get_mut(c).and_then(|r| r.get_mut(pos)), e.output.get_mut(c).and_then(|r| r.get_mut(pos)))
                else {
                    continue;
                };
                // The slot holds the input of `n` samples ago and its finished overlap-add sum.
                let dry = *inp;
                let wet = std::mem::take(out);
                *inp = if x.is_finite() { *x } else { 0.0 };
                let y = if s.delta { (dry - wet) * self.mix_now } else { dry + (wet - dry) * self.mix_now };
                *x = y * self.trim_now;
                pre += dry;
                post += *x;
                let k = key.and_then(|k| k.get(c)).and_then(|b| b.get(i)).copied().unwrap_or(0.0);
                if let Some(slot) = e.key.get_mut(c).and_then(|r| r.get_mut(pos)) {
                    *slot = if k.is_finite() { k } else { 0.0 };
                }
            }
            self.analysis.pre.push(pre * per_channel);
            self.analysis.post.push(post * per_channel);
            e.pos = if pos + 1 >= e.n { 0 } else { pos + 1 };
            e.since += 1;
            if e.since >= e.hop {
                e.since = 0;
                framed = true;
                deepest = deepest.max(Self::frame(e, &s));
            }
        }
        if framed {
            self.gr_meter = deepest;
            Self::publish(e, &self.analysis);
        }
    }
}

impl Plugin for Rez {
    crate::params::param_plumbing!(REZ_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.engine = Some(Box::new(Engine::new(self.sr, self.ch)));
        self.glide = time_coef(GLIDE_MS, self.sr);
        self.gr_meter = 0.0;
        self.update();
        self.mix_now = self.s.mix;
        self.trim_now = self.s.trim;
    }

    fn reset(&mut self) {
        if let Some(e) = self.engine.as_mut() {
            e.reset();
            Self::publish(e, &self.analysis);
        }
        self.mix_now = self.s.mix;
        self.trim_now = self.s.trim;
        self.gr_meter = 0.0;
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.run(io, frames, None);
    }

    fn process_with_sidechain(&mut self, io: &mut [Vec<f32>], frames: usize, key: Option<&[Vec<f32>]>) {
        self.run(io, frames, key);
    }

    fn latency(&self) -> usize {
        self.engine.as_ref().map_or(0, |e| e.n)
    }

    fn gain_reduction_db(&self) -> f32 {
        self.gr_meter
    }

    fn analysis(&self) -> Option<Arc<Analysis>> {
        Some(self.analysis.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// A small deterministic generator (xorshift), uniform in -1..1.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 >> 40) as f32 / (1u64 << 23) as f32) - 1.0
        }
    }

    /// Roughly pink noise (Paul Kellet's filter), about -24 dBFS RMS.
    fn pink(len: usize, seed: u64) -> Vec<f32> {
        let mut rng = Rng(seed);
        let (mut b0, mut b1, mut b2) = (0.0f32, 0.0f32, 0.0f32);
        (0..len)
            .map(|_| {
                let w = rng.next();
                b0 = 0.99765 * b0 + w * 0.099_046;
                b1 = 0.96300 * b1 + w * 0.296_516_4;
                b2 = 0.57000 * b2 + w * 1.052_691_3;
                (b0 + b1 + b2 + w * 0.1848) * 0.05
            })
            .collect()
    }

    fn sine(hz: f32, amp: f32, len: usize) -> Vec<f32> {
        (0..len).map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / SR).sin()).collect()
    }

    fn prepared() -> Rez {
        let mut r = Rez::new();
        r.prepare(SR, 512, 2);
        r
    }

    fn run(r: &mut Rez, x: &[f32]) -> Vec<f32> {
        let mut out = Vec::with_capacity(x.len());
        for block in x.chunks(512) {
            let mut io = vec![block.to_vec(), block.to_vec()];
            r.process(&mut io, block.len());
            out.extend_from_slice(&io[0]);
        }
        out
    }

    /// The level of `x` at `hz` in dB (a single-bin Fourier sum over the whole slice).
    fn level_at(x: &[f32], hz: f32) -> f32 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (i, v) in x.iter().enumerate() {
            let ph = std::f64::consts::TAU * f64::from(hz) * i as f64 / f64::from(SR);
            re += f64::from(*v) * ph.cos();
            im += f64::from(*v) * ph.sin();
        }
        (20.0 * ((re * re + im * im).sqrt() * 2.0 / x.len() as f64).max(1e-12).log10()) as f32
    }

    fn rms_db(x: &[f32]) -> f32 {
        let ms = x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / x.len().max(1) as f64;
        (10.0 * ms.max(1e-24).log10()) as f32
    }

    #[test]
    fn depth_zero_is_the_input_delayed_by_the_latency() {
        let mut r = prepared();
        r.set_param("depth", 0.0);
        let lat = r.latency();
        assert_eq!(lat, 2048);
        let x: Vec<f32> = pink(20_000, 7).iter().zip(sine(1000.0, 0.3, 20_000)).map(|(a, b)| a + b).collect();
        let y = run(&mut r, &x);
        let err = x.iter().zip(&y[lat..]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 2.0e-5, "reconstruction error {err}");
        assert_eq!(r.gain_reduction_db(), 0.0);
    }

    #[test]
    fn plain_noise_is_left_nearly_alone() {
        let mut r = prepared();
        let x = pink(96_000, 11);
        let y = run(&mut r, &x);
        let lat = r.latency();
        let change = rms_db(&y[lat + 24_000..]) - rms_db(&x[24_000..96_000 - lat]);
        assert!(change.abs() < 0.5, "plain pink noise changed by {change} dB");
    }

    #[test]
    fn a_tone_in_noise_is_cut_where_it_is_and_nowhere_else() {
        let mut r = prepared();
        let len = 96_000;
        let tone = sine(2000.0, 0.1, len);
        let probe = sine(300.0, 0.001, len);
        let x: Vec<f32> = pink(len, 3).iter().zip(&tone).zip(&probe).map(|((n, t), p)| n * 0.2 + t + p).collect();
        let y = run(&mut r, &x);
        let lat = r.latency();
        let (a, b) = (&x[24_000..len - lat], &y[lat + 24_000..]);
        let cut = level_at(a, 2000.0) - level_at(b, 2000.0);
        assert!(cut > 6.0, "the 2 kHz tone was cut by only {cut} dB");
        let elsewhere = level_at(a, 300.0) - level_at(b, 300.0);
        assert!(elsewhere.abs() < 1.0, "300 Hz moved by {elsewhere} dB");
        assert!(r.gain_reduction_db() > 6.0);
        // The deepest cut is at the tone.
        let k = r.cut_db().iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |m| m.0);
        assert!((r.bin_hz(k) - 2000.0).abs() < 120.0, "deepest cut at {} Hz", r.bin_hz(k));
    }

    #[test]
    fn depth_is_a_level_gate() {
        // The cut of a 4 kHz tone in noise at `depth` percent, the input scaled by `gain`.
        let cut_at = |depth: f32, gain: f32| {
            let mut r = prepared();
            r.set_param("depth", depth);
            let len = 72_000;
            let x: Vec<f32> = pink(len, 5).iter().zip(sine(4000.0, 0.1, len)).map(|(n, t)| (n * 0.2 + t) * gain).collect();
            let y = run(&mut r, &x);
            let lat = r.latency();
            level_at(&x[24_000..len - lat], 4000.0) - level_at(&y[lat + 24_000..], 4000.0)
        };
        // Deep: loud and 40 dB quieter are cut alike.
        let (loud, quiet) = (cut_at(100.0, 1.0), cut_at(100.0, 0.01));
        assert!(loud > 6.0 && (loud - quiet).abs() < 1.0, "depth 100 %: {loud} dB loud, {quiet} dB quiet");
        // Shallow: the quiet one falls below the gate and is cut much less.
        let (loud, quiet) = (cut_at(15.0, 1.0), cut_at(15.0, 0.01));
        assert!(loud > 3.0 && quiet < loud * 0.6, "depth 15 %: {loud} dB loud, {quiet} dB quiet");
        // More depth never cuts less, and none cuts nothing.
        assert!(cut_at(50.0, 0.1) >= cut_at(15.0, 0.1));
        assert!(cut_at(0.0, 1.0).abs() < 0.05);
    }

    #[test]
    fn rez_works_harder_toward_the_highs_and_a_band_can_bring_the_lows_back() {
        let cut = |hz: f32, set: &dyn Fn(&mut Rez)| {
            let mut r = prepared();
            set(&mut r);
            let len = 72_000;
            let x: Vec<f32> = pink(len, 61).iter().zip(sine(hz, 0.1, len)).map(|(n, t)| n * 0.2 + t).collect();
            let y = run(&mut r, &x);
            let lat = r.latency();
            level_at(&x[24_000..len - lat], hz) - level_at(&y[lat + 24_000..], hz)
        };
        let (low, high) = (cut(500.0, &|_| {}), cut(5000.0, &|_| {}));
        assert!(high > 3.0 * low, "500 Hz cut {low} dB, 5 kHz {high} dB");
        let lifted = cut(500.0, &|r| {
            r.set_param("b2_freq", 500.0);
            r.set_param("b2_gain", 12.0);
        });
        assert!(lifted > 2.5 * low, "+12 dB of sensitivity at 500 Hz: {lifted} dB against {low} dB");
        assert!(tilt_weight(100.0) < tilt_weight(1000.0) && tilt_weight(1000.0) < tilt_weight(8000.0));
        assert_eq!(tilt_weight(0.0), 0.0);
    }

    #[test]
    fn delta_plus_the_output_is_the_delayed_input() {
        let len = 48_000;
        let x: Vec<f32> = pink(len, 9).iter().zip(sine(1500.0, 0.2, len)).map(|(n, t)| n * 0.2 + t).collect();
        let mut wet = prepared();
        let mut delta = prepared();
        delta.set_param("delta", 1.0);
        let (w, d) = (run(&mut wet, &x), run(&mut delta, &x));
        let lat = wet.latency();
        assert!(rms_db(&d[lat + 12_000..]) > -60.0, "delta is silent");
        let err = x.iter().zip(w[lat..].iter().zip(&d[lat..])).map(|(x, (w, d))| (x - (w + d)).abs()).fold(0.0f32, f32::max);
        assert!(err < 2.0e-5, "output + delta differs from the input by {err}");
    }

    #[test]
    fn the_cut_recovers_after_the_resonance_stops() {
        let mut r = prepared();
        r.set_param("release", 50.0);
        let x: Vec<f32> = pink(48_000, 13).iter().zip(sine(2000.0, 0.1, 48_000)).map(|(n, t)| n * 0.2 + t).collect();
        let _ = run(&mut r, &x);
        assert!(r.gain_reduction_db() > 6.0);
        let _ = run(&mut r, &pink(48_000, 17).iter().map(|n| n * 0.2).collect::<Vec<_>>());
        let near = r.cut_db().iter().enumerate().filter(|(k, _)| (r.bin_hz(*k) - 2000.0).abs() < 100.0).fold(0.0f32, |m, (_, c)| m.max(*c));
        assert!(near < 1.5, "still cutting {near} dB at 2 kHz a second later");
    }

    #[test]
    fn the_range_keeps_the_cut_inside_low_and_high() {
        let mut r = prepared();
        r.set_param("low", 1000.0);
        let len = 72_000;
        let x: Vec<f32> = pink(len, 21).iter().zip(sine(200.0, 0.1, len)).map(|(n, t)| n * 0.2 + t).collect();
        let y = run(&mut r, &x);
        let lat = r.latency();
        let cut = level_at(&x[24_000..len - lat], 200.0) - level_at(&y[lat + 24_000..], 200.0);
        assert!(cut.abs() < 0.3, "200 Hz cut by {cut} dB with Low at 1 kHz");
    }

    #[test]
    fn a_key_decides_where_the_input_is_cut() {
        let mut r = prepared();
        let len = 72_000;
        let x: Vec<f32> = pink(len, 23).iter().zip(sine(4000.0, 0.01, len)).map(|(n, t)| n * 0.2 + t).collect();
        let key: Vec<f32> = pink(len, 29).iter().zip(sine(4000.0, 0.2, len)).map(|(n, t)| n * 0.2 + t).collect();
        let mut y = Vec::with_capacity(len);
        for (xb, kb) in x.chunks(512).zip(key.chunks(512)) {
            let mut io = vec![xb.to_vec(), xb.to_vec()];
            let k = vec![kb.to_vec(), kb.to_vec()];
            r.process_with_sidechain(&mut io, xb.len(), Some(&k));
            y.extend_from_slice(&io[0]);
        }
        let lat = r.latency();
        let cut = level_at(&x[24_000..len - lat], 4000.0) - level_at(&y[lat + 24_000..], 4000.0);
        assert!(cut > 6.0, "the key's 4 kHz resonance cut the input by only {cut} dB");
    }

    #[test]
    fn the_frame_grows_with_the_sample_rate() {
        assert_eq!(frame_size_for(44_100.0), 2048);
        assert_eq!(frame_size_for(48_000.0), 2048);
        assert_eq!(frame_size_for(96_000.0), 4096);
        assert_eq!(frame_size_for(192_000.0), 8192);
        assert_eq!(frame_size_for(1_536_000.0), 16_384);
    }

    #[test]
    fn unprepared_and_reset_are_safe() {
        let mut r = Rez::new();
        let mut io = vec![vec![0.5f32; 64]; 2];
        r.process(&mut io, 64);
        assert_eq!(io[0][0], 0.5);
        assert_eq!(r.latency(), 0);
        r.prepare(SR, 64, 2);
        let _ = run(&mut r, &sine(1000.0, 0.5, 8192));
        r.reset();
        let y = run(&mut r, &vec![0.0; 4096]);
        assert!(y.iter().all(|v| *v == 0.0), "reset left audio in the rings");
    }

    fn run_stereo(r: &mut Rez, l: &[f32], rt: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let (mut ol, mut or) = (Vec::with_capacity(l.len()), Vec::with_capacity(l.len()));
        for (a, b) in l.chunks(512).zip(rt.chunks(512)) {
            let mut io = vec![a.to_vec(), b.to_vec()];
            r.process(&mut io, a.len());
            ol.extend_from_slice(&io[0]);
            or.extend_from_slice(&io[1]);
        }
        (ol, or)
    }

    /// The cut of a 2 kHz tone in noise, with `set` applied to a fresh Rez.
    fn tone_cut(set: impl Fn(&mut Rez)) -> f32 {
        let mut r = prepared();
        set(&mut r);
        let len = 72_000;
        let x: Vec<f32> = pink(len, 31).iter().zip(sine(2000.0, 0.1, len)).map(|(n, t)| n * 0.2 + t).collect();
        let y = run(&mut r, &x);
        let lat = r.latency();
        level_at(&x[24_000..len - lat], 2000.0) - level_at(&y[lat + 24_000..], 2000.0)
    }

    #[test]
    fn a_band_raises_or_removes_the_cut_at_its_frequency() {
        let flat = tone_cut(|_| {});
        let more = tone_cut(|r| {
            r.set_param("b3_freq", 2000.0);
            r.set_param("b3_gain", 12.0);
        });
        let none = tone_cut(|r| {
            r.set_param("b3_freq", 2000.0);
            r.set_param("b3_gain", -24.0);
        });
        let off = tone_cut(|r| {
            r.set_param("b3_freq", 2000.0);
            r.set_param("b3_gain", -24.0);
            r.set_param("b3_on", 0.0);
        });
        assert!(more > flat + 3.0, "+12 dB of sensitivity: {more} dB against {flat} dB");
        assert!(none < 1.0, "-24 dB of sensitivity still cut {none} dB");
        assert!((off - flat).abs() < 0.05, "a band switched off changed the cut: {off} dB against {flat} dB");
    }

    #[test]
    fn the_sensitivity_curve_is_the_range_times_the_bands() {
        let mut bands = [Band { on: true, freq: 1000.0, gain: 0.0, q: 1.0 }; BANDS];
        assert_eq!(bands_db(&bands, 1000.0), 0.0);
        bands[1].gain = 12.0;
        assert!((bands_db(&bands, 1000.0) - 12.0).abs() < 1.0e-3);
        assert!(bands_db(&bands, 8000.0) < 1.0);
        bands[1].on = false;
        assert_eq!(bands_db(&bands, 1000.0), 0.0);
        assert_eq!(range_weight(100.0, 5000.0, 1000.0), 1.0);
        assert_eq!(range_weight(100.0, 5000.0, 40.0), 0.0);
        assert!(range_weight(100.0, 5000.0, 80.0) > 0.0 && range_weight(100.0, 5000.0, 80.0) < 1.0);
    }

    #[test]
    fn unlinked_sides_are_cut_on_their_own() {
        let len = 72_000;
        let left: Vec<f32> = pink(len, 41).iter().zip(sine(2000.0, 0.1, len)).map(|(n, t)| n * 0.2 + t).collect();
        // The right channel carries a quiet probe at the same frequency and no resonance.
        let right: Vec<f32> = pink(len, 43).iter().zip(sine(2000.0, 0.002, len)).map(|(n, t)| n * 0.2 + t).collect();
        let cuts = |link: f32| {
            let mut r = prepared();
            r.set_param("link", link);
            let (yl, yr) = run_stereo(&mut r, &left, &right);
            let lat = r.latency();
            let cut = |x: &[f32], y: &[f32]| level_at(&x[24_000..len - lat], 2000.0) - level_at(&y[lat + 24_000..], 2000.0);
            (cut(&left, &yl), cut(&right, &yr))
        };
        let (l, r) = cuts(0.0);
        assert!(l > 6.0 && r.abs() < 1.0, "unlinked: left {l} dB, right {r} dB");
        let (l, r) = cuts(100.0);
        assert!(l > 6.0 && (l - r).abs() < 0.5, "linked: left {l} dB, right {r} dB");
    }

    #[test]
    fn mid_side_cuts_the_side_and_leaves_the_mid() {
        let len = 72_000;
        let tone = sine(2000.0, 0.1, len);
        let noise = pink(len, 47);
        // The tone is out of phase between the channels (all side); a 500 Hz probe is in phase.
        let probe = sine(500.0, 0.002, len);
        let left: Vec<f32> = (0..len).map(|i| noise[i] * 0.2 + tone[i] + probe[i]).collect();
        let right: Vec<f32> = (0..len).map(|i| noise[i] * 0.2 - tone[i] + probe[i]).collect();
        let mut r = prepared();
        r.set_param("stereo", 1.0);
        r.set_param("link", 0.0);
        let (yl, yr) = run_stereo(&mut r, &left, &right);
        let lat = r.latency();
        let side = |l: &[f32], r: &[f32]| l.iter().zip(r).map(|(a, b)| (a - b) * 0.5).collect::<Vec<f32>>();
        let mid = |l: &[f32], r: &[f32]| l.iter().zip(r).map(|(a, b)| (a + b) * 0.5).collect::<Vec<f32>>();
        let cut = level_at(&side(&left, &right)[24_000..len - lat], 2000.0) - level_at(&side(&yl, &yr)[lat + 24_000..], 2000.0);
        assert!(cut > 6.0, "the side's 2 kHz tone was cut by only {cut} dB");
        let kept = level_at(&mid(&left, &right)[24_000..len - lat], 500.0) - level_at(&mid(&yl, &yr)[lat + 24_000..], 500.0);
        assert!(kept.abs() < 0.5, "the mid's 500 Hz moved by {kept} dB");
        // Balance all the way to the first side (mid): the side is left alone.
        r.reset();
        r.set_param("balance", -100.0);
        let (yl, yr) = run_stereo(&mut r, &left, &right);
        let cut = level_at(&side(&left, &right)[24_000..len - lat], 2000.0) - level_at(&side(&yl, &yr)[lat + 24_000..], 2000.0);
        assert!(cut.abs() < 0.3, "balance at the mid still cut the side by {cut} dB");
    }

    #[test]
    fn hard_cuts_far_more_than_soft_and_reaches_lower() {
        // (Full Depth, so the level gate is out of the way in both modes.)
        let soft = |r: &mut Rez| {
            r.set_param("depth", 100.0);
        };
        let hard = |r: &mut Rez| {
            r.set_param("depth", 100.0);
            r.set_param("mode", 1.0);
        };
        let (soft_cut, hard_cut) = (tone_cut(soft), tone_cut(hard));
        assert!(hard_cut > soft_cut + 4.0, "2 kHz: soft {soft_cut} dB, hard {hard_cut} dB");
        assert!(hard_cut <= MAX_CUT_DB + 0.5, "hard cut {hard_cut} dB");
        // Plain noise loses a few dB in Hard, hardly any in Soft.
        let x = pink(96_000, 71);
        let loss = |set: &dyn Fn(&mut Rez)| {
            let mut r = prepared();
            set(&mut r);
            let y = run(&mut r, &x);
            rms_db(&x[24_000..90_000]) - rms_db(&y[24_000 + r.latency()..])
        };
        let (soft_loss, hard_loss) = (loss(&soft), loss(&hard));
        assert!(soft_loss < 0.5 && hard_loss > soft_loss + 0.5 && hard_loss < 6.0, "soft {soft_loss} dB, hard {hard_loss} dB");
    }

    #[test]
    fn trim_scales_the_output() {
        let x = pink(48_000, 53);
        let mut plain = prepared();
        let mut trimmed = prepared();
        trimmed.set_param("trim", -6.0);
        let (a, b) = (run(&mut plain, &x), run(&mut trimmed, &x));
        let change = rms_db(&b[24_000..]) - rms_db(&a[24_000..]);
        assert!((change + 6.0).abs() < 0.05, "a -6 dB trim changed the level by {change} dB");
    }

    #[test]
    fn the_editor_gets_the_live_cut_curve_and_both_streams() {
        let mut r = prepared();
        let len = 48_000;
        let x: Vec<f32> = pink(len, 59).iter().zip(sine(2000.0, 0.1, len)).map(|(n, t)| n * 0.2 + t).collect();
        let _ = run(&mut r, &x);
        let a = r.analysis().unwrap();
        assert_eq!(a.live.len(), LIVE_LEN);
        let deepest = (0..LIVE_POINTS).max_by(|i, j| a.live.get(LIVE_CUT_A + i).total_cmp(&a.live.get(LIVE_CUT_A + j))).unwrap();
        assert!((live_hz(deepest) / 2000.0).log2().abs() < 0.12, "the live curve peaks at {} Hz", live_hz(deepest));
        assert!(a.live.get(LIVE_CUT_A + deepest) > 6.0);
        // Linked: the second side shows the same cut.
        assert_eq!(a.live.get(LIVE_CUT_B + deepest), a.live.get(LIVE_CUT_A + deepest));
        assert!(a.pre.written() >= len && a.post.written() >= len);
        r.reset();
        assert_eq!(a.live.get(LIVE_CUT_A + deepest), 0.0);
    }
}
