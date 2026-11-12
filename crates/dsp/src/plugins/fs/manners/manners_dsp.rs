//! MannersDsp: an automatic balance equaliser. Rust-native: there is no C++ original.
//!
//! It listens to the spectrum many times a second, works out which regions cover up their
//! neighbours and which are covered up, and turns the first down (Tame) and the second up
//! (Recover). The model is textbook psychoacoustics, not a copy of any product:
//!
//! 1. An STFT (2048 points at 44.1/48 kHz, a new frame every N/8 samples, Hann in and out).
//!    Stereo is linked: one complex FFT carries both channels (left real, right imaginary) and
//!    both get the same gain.
//! 2. The frame's power, gathered into bands half an ERB wide (Glasberg and Moore's ERB-rate
//!    scale) and smoothed in time (Attack/Release on the level).
//! 3. Each band against its neighbourhood: the power its neighbours spread onto it, with the
//!    spread falling [`Tuning::slope_up_db`] per ERB above a masker and
//!    [`Tuning::slope_down_db`] below it (low sounds cover high ones more than the other way
//!    round). The band's own surroundings ([`Tuning::self_width_erb`]) do not count. In dB, band
//!    minus neighbourhood is its *dominance*.
//! 4. Minus the dominance's median over the working range: shape, not level, and not tilt.
//!    Above zero the band is dominant and Tame cuts it; below zero it is covered and Recover
//!    lifts it, unless the band is down at the floor (nothing there but noise to lift).
//! 5. The broad balance, beside that local one: the level a few ERB wide, against pink noise
//!    (equal power per octave) and against its own median, is how far a whole region stands over
//!    or under the rest. Tame cuts what stands over its target ([`Tuning::tame_target`]) and
//!    Recover lifts what lies under its own ([`Tuning::recover_target`]); the two targets are not
//!    the same curve, and both lean further out as the signal gets quieter
//!    ([`Tuning::tame_level`], [`Tuning::recover_level`]), as hearing does.
//! 6. The band gains are smoothed over frequency and time, tilted by Brighten and Boost,
//!    levelled so the frame keeps its power ([`Tuning::level_comp`]) and spread back onto the
//!    FFT bins.
//!
//! Latency: the frame length N, always. `prepare` allocates every buffer; `process`, `reset` and
//! the setters never allocate.

use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::sync::Arc;

/// Most bands the engine works in (the tables are this long).
pub const MAX_BANDS: usize = 96;

/// The constants of the model: what a tuning session moves. The plugin runs [`Tuning::default`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tuning {
    /// Bands per ERB (where the FFT resolves that finely; wider at the bottom).
    pub bands_per_erb: f32,
    /// Neighbours closer than this, in ERB, are part of the band itself, not its maskers.
    pub self_width_erb: f32,
    /// How fast masking falls above the masker, dB per ERB.
    pub slope_up_db: f32,
    /// How fast masking falls below the masker, dB per ERB.
    pub slope_down_db: f32,
    /// The band levels' rise and fall times.
    pub level_attack_ms: f32,
    pub level_release_ms: f32,
    /// The gains' times: moving away from 0 dB and coming back.
    pub gain_attack_ms: f32,
    pub gain_release_ms: f32,
    /// dB of cut per dB of dominance at Tame 100 %.
    pub tame_ratio: f32,
    /// dB of lift per dB a band is covered at Recover 100 %.
    pub recover_ratio: f32,
    /// How far from balance a band is before anything happens (a soft knee), dB.
    pub knee_db: f32,
    /// The most Tame cuts and Recover lifts, at 100 %.
    pub max_cut_db: f32,
    pub max_boost_db: f32,
    /// How far Bias at its ends moves the balance point, dB.
    pub bias_db: f32,
    /// Recover leaves a band alone once it is this far under the loudest band, fading in over
    /// [`Tuning::floor_fade_db`] above that.
    pub floor_db: f32,
    pub floor_fade_db: f32,
    /// And once it is under this absolute level (dBFS, a full-scale sine reads 0).
    pub abs_floor_db: f32,
    /// Passes of a 1-2-1 smoothing of the gains over the bands.
    pub smooth_passes: u8,
    /// How much of the gain curve's change in level is given back (0: none, 1: all).
    pub level_comp: f32,
    /// Brighten at its ends: dB at the top of the spectrum (the opposite at the bottom).
    pub brighten_db: f32,
    /// Boost at its ends: dB of bass (with half as much taken out of the low mids).
    pub boost_db: f32,
    /// How wide the broad balance looks, in ERB to one sigma.
    pub broad_width_erb: f32,
    /// dB of cut per dB a region stands over Tame's target, at Tame 100 %.
    pub tame_broad: f32,
    /// dB of lift per dB a region lies under Recover's target, at Recover 100 %.
    pub recover_broad: f32,
    /// How much the targets lean out per 20 dB the signal is under [`Tuning::level_ref_db`]
    /// (and in, over it).
    pub tame_level: f32,
    pub recover_level: f32,
    /// The level the targets are written for, dBFS (a full-scale sine reads 0).
    pub level_ref_db: f32,
    /// Where Tame starts cutting a region, in dB against pink at [`TARGET_HZ`].
    pub tame_target: [f32; TARGET_POINTS],
    /// Where Recover starts lifting a region, in dB against pink at [`TARGET_HZ`].
    pub recover_target: [f32; TARGET_POINTS],
}

/// The points the targets are given at.
pub const TARGET_POINTS: usize = 8;
/// Their frequencies, Hz (straight lines between them on the ERB scale, flat beyond the ends).
pub const TARGET_HZ: [f32; TARGET_POINTS] = [40.0, 80.0, 160.0, 400.0, 1_000.0, 4_000.0, 10_000.0, 16_000.0];

impl Default for Tuning {
    /// Fitted to reference bounces of the probe (`docs/manners-tuning.md`) at Tame 100 %,
    /// Recover 100 % and both at 50 %: 0.6 dB RMS from the reference over every moment and
    /// third octave. Bias, Brighten, Boost and the floor were not in those passes and are as first
    /// written.
    fn default() -> Self {
        Tuning {
            bands_per_erb: 1.5,
            self_width_erb: 0.5,
            slope_up_db: 39.5,
            slope_down_db: 24.0,
            level_attack_ms: 22.0,
            level_release_ms: 54.0,
            gain_attack_ms: 11.0,
            gain_release_ms: 13.5,
            tame_ratio: 0.05,
            recover_ratio: 0.1,
            knee_db: 1.5,
            max_cut_db: 6.2,
            max_boost_db: 7.3,
            bias_db: 4.0,
            floor_db: -60.0,
            floor_fade_db: 12.0,
            abs_floor_db: -84.0,
            smooth_passes: 2,
            level_comp: 0.65,
            brighten_db: 4.5,
            boost_db: 4.0,
            broad_width_erb: 1.0,
            tame_broad: 0.35,
            recover_broad: 0.31,
            tame_level: 0.3,
            recover_level: 2.9,
            level_ref_db: -13.0,
            tame_target: [-7.95, -11.3, -4.3, -0.3, 0.0, -1.3, -6.05, -11.45],
            recover_target: [7.1, 11.45, 5.25, -1.7, -2.0, -0.25, 0.1, 9.1],
        }
    }
}

impl Tuning {
    /// The same tuning with every field finite and in a workable range (hostile input from a
    /// tuning tool can not break the tables).
    pub fn sanitized(self) -> Tuning {
        let d = Tuning::default();
        let f = |v: f32, lo: f32, hi: f32, def: f32| if v.is_finite() { v.clamp(lo, hi) } else { def };
        Tuning {
            bands_per_erb: f(self.bands_per_erb, 0.5, 4.0, d.bands_per_erb),
            self_width_erb: f(self.self_width_erb, 0.25, 6.0, d.self_width_erb),
            slope_up_db: f(self.slope_up_db, 1.0, 60.0, d.slope_up_db),
            slope_down_db: f(self.slope_down_db, 1.0, 60.0, d.slope_down_db),
            level_attack_ms: f(self.level_attack_ms, 0.0, 2000.0, d.level_attack_ms),
            level_release_ms: f(self.level_release_ms, 0.0, 5000.0, d.level_release_ms),
            gain_attack_ms: f(self.gain_attack_ms, 0.0, 2000.0, d.gain_attack_ms),
            gain_release_ms: f(self.gain_release_ms, 0.0, 5000.0, d.gain_release_ms),
            tame_ratio: f(self.tame_ratio, 0.0, 2.0, d.tame_ratio),
            recover_ratio: f(self.recover_ratio, 0.0, 2.0, d.recover_ratio),
            knee_db: f(self.knee_db, 0.0, 12.0, d.knee_db),
            max_cut_db: f(self.max_cut_db, 0.0, 24.0, d.max_cut_db),
            max_boost_db: f(self.max_boost_db, 0.0, 18.0, d.max_boost_db),
            bias_db: f(self.bias_db, 0.0, 12.0, d.bias_db),
            floor_db: f(self.floor_db, -120.0, -20.0, d.floor_db),
            floor_fade_db: f(self.floor_fade_db, 1.0, 40.0, d.floor_fade_db),
            abs_floor_db: f(self.abs_floor_db, -140.0, -40.0, d.abs_floor_db),
            smooth_passes: self.smooth_passes.min(8),
            level_comp: f(self.level_comp, 0.0, 1.0, d.level_comp),
            brighten_db: f(self.brighten_db, 0.0, 12.0, d.brighten_db),
            boost_db: f(self.boost_db, 0.0, 12.0, d.boost_db),
            broad_width_erb: f(self.broad_width_erb, 0.5, 12.0, d.broad_width_erb),
            tame_broad: f(self.tame_broad, 0.0, 2.0, d.tame_broad),
            recover_broad: f(self.recover_broad, 0.0, 2.0, d.recover_broad),
            tame_level: f(self.tame_level, 0.0, 4.0, d.tame_level),
            recover_level: f(self.recover_level, 0.0, 4.0, d.recover_level),
            level_ref_db: f(self.level_ref_db, -80.0, 0.0, d.level_ref_db),
            tame_target: std::array::from_fn(|i| f(self.tame_target[i], -40.0, 40.0, d.tame_target[i])),
            recover_target: std::array::from_fn(|i| f(self.recover_target[i], -40.0, 40.0, d.recover_target[i])),
        }
    }
}

/// What the knobs say, in the engine's units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// 0..1.
    pub tame: f32,
    /// 0..1.
    pub recover: f32,
    /// -1..1: below zero favours Tame, above it Recover.
    pub bias: f32,
    /// -1..1: darker .. brighter.
    pub brighten: f32,
    /// -1..1: less .. more bass against the low mids.
    pub boost: f32,
    /// The working range, Hz.
    pub lo_hz: f32,
    pub hi_hz: f32,
    /// Hear only what the engine changes (output minus input).
    pub delta: bool,
    /// Pass the input through untouched (still a frame late, so the latency never changes).
    pub bypass: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { tame: 0.0, recover: 0.0, bias: 0.0, brighten: 0.0, boost: 0.0, lo_hz: 20.0, hi_hz: 20_000.0, delta: false, bypass: false }
    }
}

/// The ERB-rate (Cams) of a frequency, Glasberg and Moore 1990.
pub fn hz_to_erb(hz: f32) -> f32 {
    21.4 * (1.0 + 0.00437 * hz.max(0.0)).log10()
}

/// The width of one ERB at a frequency, Hz.
pub fn erb_width_hz(hz: f32) -> f32 {
    24.7 * (0.00437 * hz.max(0.0) + 1.0)
}

/// The frame length for a sample rate.
pub fn frame_len(sample_rate: f64) -> usize {
    if sample_rate <= 50_000.0 {
        2048
    } else if sample_rate <= 100_000.0 {
        4096
    } else {
        8192
    }
}

const LN10_OVER_20: f32 = std::f32::consts::LN_10 / 20.0;
/// Power below this is silence (about -200 dB).
const TINY: f32 = 1e-20;

fn pow_db(p: f32) -> f32 {
    10.0 * p.max(TINY).log10()
}

/// `x² / (x + knee)` above zero, else zero: nothing at balance, one for one far from it.
fn knee(x: f32, knee: f32) -> f32 {
    if x <= 0.0 { 0.0 } else { x * x / (x + knee.max(1e-3)) }
}

/// A soft limit: `x` for small values, never past `cap`.
fn soft_cap(x: f32, cap: f32) -> f32 {
    if cap <= 1e-6 { 0.0 } else { cap * (x / cap).tanh() }
}

/// A one-pole step's coefficient for a time constant, at the frame rate.
fn frame_coef(ms: f32, frames_per_sec: f32) -> f32 {
    if ms <= 0.0 || frames_per_sec <= 0.0 { 0.0 } else { (-1000.0 / (ms * frames_per_sec)).exp() }
}

/// The engine.
pub struct MannersDsp {
    tuning: Tuning,
    set: Settings,
    sr: f64,
    n: usize,
    hop: usize,
    channels: usize,
    fft: Option<Arc<dyn Fft<f32>>>,
    ifft: Option<Arc<dyn Fft<f32>>>,
    scratch: Vec<Complex<f32>>,
    buf: Vec<Complex<f32>>,
    window: Vec<f32>,
    /// The last N input samples per channel, and the output being overlap-added, both circular
    /// on `pos`.
    inp: [Vec<f32>; 2],
    ola: [Vec<f32>; 2],
    pos: usize,
    count: usize,
    /// Per bin 0..=N/2: the band below it and how far towards the next (0..1).
    bin_band: Vec<u16>,
    bin_frac: Vec<f32>,
    bin_gain: Vec<f32>,
    bands: usize,
    /// Band centres (Hz and ERB), `1 / (weight of the bins)` and bins per ERB there.
    centre_hz: [f32; MAX_BANDS],
    centre_erb: [f32; MAX_BANDS],
    inv_weight: [f32; MAX_BANDS],
    bins_per_erb: [f32; MAX_BANDS],
    /// The neighbourhood weights, row per band, each row summing to 1 (`bands` × `bands`).
    spread: Vec<f32>,
    /// The broad balance's weights, row per band, each row summing to 1 (`bands` × `bands`).
    wide: Vec<f32>,
    /// Pink noise's level per ERB at each band, dB against 1 kHz.
    pink_db: [f32; MAX_BANDS],
    /// The targets at each band, dB against pink.
    tame_at: [f32; MAX_BANDS],
    recover_at: [f32; MAX_BANDS],
    /// Scratch: each band's broad level against pink, dB.
    broad: [f32; MAX_BANDS],
    /// 0..1 per band: inside the working range.
    range: [f32; MAX_BANDS],
    /// The static tilt per band, dB (Brighten and Boost).
    tilt: [f32; MAX_BANDS],
    /// Scratch per frame and the smoothed state.
    raw: [f32; MAX_BANDS],
    level: [f32; MAX_BANDS],
    dom: [f32; MAX_BANDS],
    sorted: [f32; MAX_BANDS],
    want: [f32; MAX_BANDS],
    tmp: [f32; MAX_BANDS],
    gain_db: [f32; MAX_BANDS],
    out_db: [f32; MAX_BANDS],
    comp_db: f32,
    level_att: f32,
    level_rel: f32,
    gain_att: f32,
    gain_rel: f32,
    /// Largest cut and lift of the last frame, dB (both non-negative).
    max_cut: f32,
    max_boost: f32,
}

impl Default for MannersDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl MannersDsp {
    pub fn new() -> Self {
        Self::with_tuning(Tuning::default())
    }

    /// An engine with its own constants (the tuning tool's). Unprepared: call [`MannersDsp::prepare`].
    pub fn with_tuning(tuning: Tuning) -> Self {
        MannersDsp {
            tuning: tuning.sanitized(),
            set: Settings::default(),
            sr: 48_000.0,
            n: 0,
            hop: 0,
            channels: 2,
            fft: None,
            ifft: None,
            scratch: Vec::new(),
            buf: Vec::new(),
            window: Vec::new(),
            inp: [Vec::new(), Vec::new()],
            ola: [Vec::new(), Vec::new()],
            pos: 0,
            count: 0,
            bin_band: Vec::new(),
            bin_frac: Vec::new(),
            bin_gain: Vec::new(),
            bands: 0,
            centre_hz: [0.0; MAX_BANDS],
            centre_erb: [0.0; MAX_BANDS],
            inv_weight: [0.0; MAX_BANDS],
            bins_per_erb: [1.0; MAX_BANDS],
            spread: Vec::new(),
            wide: Vec::new(),
            pink_db: [0.0; MAX_BANDS],
            tame_at: [0.0; MAX_BANDS],
            recover_at: [0.0; MAX_BANDS],
            broad: [0.0; MAX_BANDS],
            range: [1.0; MAX_BANDS],
            tilt: [0.0; MAX_BANDS],
            raw: [0.0; MAX_BANDS],
            level: [0.0; MAX_BANDS],
            dom: [0.0; MAX_BANDS],
            sorted: [0.0; MAX_BANDS],
            want: [0.0; MAX_BANDS],
            tmp: [0.0; MAX_BANDS],
            gain_db: [0.0; MAX_BANDS],
            out_db: [0.0; MAX_BANDS],
            comp_db: 0.0,
            level_att: 0.0,
            level_rel: 0.0,
            gain_att: 0.0,
            gain_rel: 0.0,
            max_cut: 0.0,
            max_boost: 0.0,
        }
    }

    pub fn tuning(&self) -> &Tuning {
        &self.tuning
    }

    pub fn sample_rate(&self) -> f64 {
        self.sr
    }

    /// The latency, samples: the frame length (0 before `prepare`).
    pub fn latency_samples(&self) -> usize {
        self.n
    }

    /// How many bands the engine works in at this rate.
    pub fn bands(&self) -> usize {
        self.bands
    }

    /// The bands' centre frequencies, Hz.
    pub fn band_hz(&self) -> &[f32] {
        self.centre_hz.get(..self.bands).unwrap_or(&[])
    }

    /// The gain applied per band in the last frame, dB (everything: Tame, Recover, the tilts and
    /// the level compensation).
    pub fn band_gain_db(&self) -> &[f32] {
        self.out_db.get(..self.bands).unwrap_or(&[])
    }

    /// How much each band is inside the working range, 0..1.
    pub fn range_weights(&self) -> &[f32] {
        self.range.get(..self.bands).unwrap_or(&[])
    }

    /// The largest cut and the largest lift Tame and Recover made in the last frame, dB.
    pub fn extremes_db(&self) -> (f32, f32) {
        (self.max_cut, self.max_boost)
    }

    /// The level given back in the last frame, dB.
    pub fn level_comp_db(&self) -> f32 {
        self.comp_db
    }

    /// Sizes everything for a rate and channel count (1 or 2) and goes to rest. Allocates.
    pub fn prepare(&mut self, sample_rate: f64, channels: usize) {
        let sr = if sample_rate.is_finite() { sample_rate.clamp(8_000.0, 384_000.0) } else { 48_000.0 };
        self.sr = sr;
        self.channels = channels.clamp(1, 2);
        let n = frame_len(sr);
        self.n = n;
        self.hop = n / 8;
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);
        let ifft = planner.plan_fft_inverse(n);
        let scratch = fft.get_inplace_scratch_len().max(ifft.get_inplace_scratch_len());
        self.scratch = vec![Complex::new(0.0, 0.0); scratch];
        self.fft = Some(fft);
        self.ifft = Some(ifft);
        self.buf = vec![Complex::new(0.0, 0.0); n];
        self.window = (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect();
        for c in 0..2 {
            if let (Some(i), Some(o)) = (self.inp.get_mut(c), self.ola.get_mut(c)) {
                *i = vec![0.0; n];
                *o = vec![0.0; n];
            }
        }
        self.bin_gain = vec![1.0; n / 2 + 1];
        self.build_bands();
        self.build_broad();
        let fps = (sr / self.hop.max(1) as f64) as f32;
        self.level_att = frame_coef(self.tuning.level_attack_ms, fps);
        self.level_rel = frame_coef(self.tuning.level_release_ms, fps);
        self.gain_att = frame_coef(self.tuning.gain_attack_ms, fps);
        self.gain_rel = frame_coef(self.tuning.gain_release_ms, fps);
        self.update_curves();
        self.reset();
    }

    /// Back to rest: no audio in flight, no gain. Keeps the settings.
    pub fn reset(&mut self) {
        for v in self.inp.iter_mut().chain(self.ola.iter_mut()) {
            v.fill(0.0);
        }
        self.pos = 0;
        self.count = 0;
        self.level = [0.0; MAX_BANDS];
        self.gain_db = [0.0; MAX_BANDS];
        self.out_db = [0.0; MAX_BANDS];
        self.comp_db = 0.0;
        self.max_cut = 0.0;
        self.max_boost = 0.0;
        self.bin_gain.fill(1.0);
    }

    /// Takes the knobs (clamped; a non-finite value keeps its default).
    pub fn set(&mut self, s: Settings) {
        let d = Settings::default();
        let f = |v: f32, lo: f32, hi: f32, def: f32| if v.is_finite() { v.clamp(lo, hi) } else { def };
        let new = Settings {
            tame: f(s.tame, 0.0, 1.0, d.tame),
            recover: f(s.recover, 0.0, 1.0, d.recover),
            bias: f(s.bias, -1.0, 1.0, d.bias),
            brighten: f(s.brighten, -1.0, 1.0, d.brighten),
            boost: f(s.boost, -1.0, 1.0, d.boost),
            lo_hz: f(s.lo_hz, 20.0, 20_000.0, d.lo_hz),
            hi_hz: f(s.hi_hz, 20.0, 20_000.0, d.hi_hz),
            delta: s.delta,
            bypass: s.bypass,
        };
        if new != self.set {
            self.set = new;
            self.update_curves();
        }
    }

    pub fn settings(&self) -> Settings {
        self.set
    }

    /// The bands, the bins' places among them and the neighbourhood weights.
    fn build_bands(&mut self) {
        let t = self.tuning;
        let n = self.n.max(2);
        let bin_hz = (self.sr / n as f64) as f32;
        let top = (0.475 * self.sr as f32).min(20_000.0);
        // Centres: half an ERB apart, never closer than a bin.
        let mut bands = 0usize;
        let mut hz = 20.0f32.max(bin_hz);
        while bands < MAX_BANDS && hz <= top {
            if let (Some(c), Some(e)) = (self.centre_hz.get_mut(bands), self.centre_erb.get_mut(bands)) {
                *c = hz;
                *e = hz_to_erb(hz);
            }
            bands += 1;
            hz += (erb_width_hz(hz) / t.bands_per_erb).max(bin_hz);
        }
        let bands = bands.max(1);
        self.bands = bands;
        // Bins among the bands.
        let half = n / 2;
        self.bin_band = vec![0; half + 1];
        self.bin_frac = vec![0.0; half + 1];
        let mut weight = [0.0f32; MAX_BANDS];
        let mut b = 0usize;
        for k in 0..=half {
            let f = k as f32 * bin_hz;
            while b + 1 < bands && self.centre_hz.get(b + 1).is_some_and(|c| *c <= f) {
                b += 1;
            }
            let lo = self.centre_hz.get(b).copied().unwrap_or(0.0);
            let frac = match self.centre_hz.get(b + 1) {
                Some(hi) if b + 1 < bands && f > lo && *hi > lo => ((f - lo) / (hi - lo)).clamp(0.0, 1.0),
                _ => 0.0,
            };
            if let (Some(bb), Some(bf)) = (self.bin_band.get_mut(k), self.bin_frac.get_mut(k)) {
                *bb = b as u16;
                *bf = frac;
            }
            if k > 0 {
                if let Some(w) = weight.get_mut(b) {
                    *w += 1.0 - frac;
                }
                if let Some(w) = weight.get_mut(b + 1) {
                    *w += frac;
                }
            }
        }
        for i in 0..MAX_BANDS {
            let w = weight.get(i).copied().unwrap_or(0.0);
            let hz = self.centre_hz.get(i).copied().unwrap_or(0.0);
            if let Some(v) = self.inv_weight.get_mut(i) {
                *v = if w > 1e-6 { 1.0 / w } else { 0.0 };
            }
            if let Some(v) = self.bins_per_erb.get_mut(i) {
                *v = (erb_width_hz(hz) / bin_hz.max(1e-6)).max(1e-3);
            }
        }
        // Neighbourhood weights: masker `j` onto band `i`, each weighed by the stretch of the
        // ERB scale it stands for, rows normalised (so an even spectrum reads 0 dB everywhere).
        self.spread = vec![0.0; bands * bands];
        let erb = |i: usize| self.centre_erb.get(i).copied().unwrap_or(0.0);
        for i in 0..bands {
            let mut sum = 0.0f32;
            for j in 0..bands {
                let dz = erb(i) - erb(j);
                let width = 0.5 * (erb((j + 1).min(bands - 1)) - erb(j.saturating_sub(1)));
                let w = if dz.abs() < t.self_width_erb {
                    0.0
                } else if dz > 0.0 {
                    width * 10f32.powf(-t.slope_up_db * dz / 10.0)
                } else {
                    width * 10f32.powf(t.slope_down_db * dz / 10.0)
                };
                if let Some(s) = self.spread.get_mut(i * bands + j) {
                    *s = w;
                }
                sum += w;
            }
            if sum > 0.0 {
                for j in 0..bands {
                    if let Some(s) = self.spread.get_mut(i * bands + j) {
                        *s /= sum;
                    }
                }
            }
        }
    }

    /// The broad balance's tables: its weights, pink noise per ERB and the two targets per band.
    fn build_broad(&mut self) {
        let t = self.tuning;
        let bands = self.bands;
        let erb = |i: usize| self.centre_erb.get(i).copied().unwrap_or(0.0);
        let mut wide = vec![0.0; bands * bands];
        for i in 0..bands {
            let mut sum = 0.0f32;
            for j in 0..bands {
                let dz = (erb(i) - erb(j)) / t.broad_width_erb;
                let width = 0.5 * (erb((j + 1).min(bands - 1)) - erb(j.saturating_sub(1)));
                let w = width.max(1e-6) * (-0.5 * dz * dz).exp();
                if let Some(s) = wide.get_mut(i * bands + j) {
                    *s = w;
                }
                sum += w;
            }
            if sum > 0.0 {
                for j in 0..bands {
                    if let Some(s) = wide.get_mut(i * bands + j) {
                        *s /= sum;
                    }
                }
            }
        }
        self.wide = wide;
        let target_erb: [f32; TARGET_POINTS] = std::array::from_fn(|i| hz_to_erb(TARGET_HZ[i]));
        let at = |table: &[f32; TARGET_POINTS], z: f32| {
            let mut v = table[0];
            for k in 1..TARGET_POINTS {
                if z >= target_erb[k - 1] {
                    let f = ((z - target_erb[k - 1]) / (target_erb[k] - target_erb[k - 1])).clamp(0.0, 1.0);
                    v = table[k - 1] + (table[k] - table[k - 1]) * f;
                }
            }
            v
        };
        // Pink: power per Hz falls as 1/f, so per ERB it is the ERB's width over f.
        let pink = |hz: f32| 10.0 * (erb_width_hz(hz) / hz.max(1.0)).log10();
        for i in 0..MAX_BANDS {
            let hz = self.centre_hz.get(i).copied().unwrap_or(1_000.0).max(1.0);
            let z = self.centre_erb.get(i).copied().unwrap_or(0.0);
            if let Some(v) = self.pink_db.get_mut(i) {
                *v = pink(hz) - pink(1_000.0);
            }
            if let Some(v) = self.tame_at.get_mut(i) {
                *v = at(&t.tame_target, z);
            }
            if let Some(v) = self.recover_at.get_mut(i) {
                *v = at(&t.recover_target, z);
            }
        }
    }

    /// The working range and the tilts, from the settings.
    fn update_curves(&mut self) {
        let s = self.set;
        let t = self.tuning;
        let (lo, hi) = if s.lo_hz <= s.hi_hz { (s.lo_hz, s.hi_hz) } else { (s.hi_hz, s.lo_hz) };
        // The ends fade over a third of an octave; wide open, the range is everything.
        let open_lo = lo <= 20.5;
        let open_hi = hi >= 19_900.0;
        let first = self.centre_erb.first().copied().unwrap_or(0.0);
        let last = self.centre_erb.get(self.bands.saturating_sub(1)).copied().unwrap_or(first);
        let mid = 0.5 * (first + last);
        let half = (0.5 * (last - first)).max(1e-3);
        for i in 0..MAX_BANDS {
            let hz = self.centre_hz.get(i).copied().unwrap_or(0.0).max(1.0);
            let erb = self.centre_erb.get(i).copied().unwrap_or(0.0);
            let fade = |oct: f32| (0.5 + 1.5 * oct).clamp(0.0, 1.0);
            let r_lo = if open_lo { 1.0 } else { fade((hz / lo).log2()) };
            let r_hi = if open_hi { 1.0 } else { fade((hi / hz).log2()) };
            if let Some(r) = self.range.get_mut(i) {
                *r = if i < self.bands { r_lo * r_hi } else { 0.0 };
            }
            // Brighten: a straight tilt on the ERB scale. Boost: a bass shelf (full under 60 Hz,
            // gone by 240 Hz) against a dip of half its size around 450 Hz, two octaves wide.
            let bright = s.brighten * t.brighten_db * ((erb - mid) / half).clamp(-1.0, 1.0);
            let shelf = (1.0 - (hz / 60.0).log2() / 2.0).clamp(0.0, 1.0);
            let shelf = shelf * shelf * (3.0 - 2.0 * shelf);
            let oct = (hz / 450.0).log2();
            let dip = (-oct * oct).exp();
            let boost = s.boost * t.boost_db * (shelf - 0.5 * dip);
            if let Some(v) = self.tilt.get_mut(i) {
                *v = if i < self.bands { bright + boost } else { 0.0 };
            }
        }
    }

    /// Processes `frames` samples of `io` in place (at most the prepared channels, at most each
    /// buffer's length). The output is the input N samples late with the gain applied.
    pub fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let n = self.n;
        if n == 0 || self.hop == 0 {
            return;
        }
        let nc = self.channels.min(io.len()).min(2);
        if nc == 0 {
            return;
        }
        let frames = io.iter().take(nc).map(Vec::len).min().unwrap_or(0).min(frames);
        for i in 0..frames {
            for c in 0..nc {
                let x = io.get(c).and_then(|v| v.get(i)).copied().unwrap_or(0.0);
                let x = if x.is_finite() { x } else { 0.0 };
                let mut y = 0.0;
                if let Some(slot) = self.inp.get_mut(c).and_then(|v| v.get_mut(self.pos)) {
                    *slot = x;
                }
                if let Some(slot) = self.ola.get_mut(c).and_then(|v| v.get_mut(self.pos)) {
                    y = *slot;
                    *slot = 0.0;
                }
                if let Some(o) = io.get_mut(c).and_then(|v| v.get_mut(i)) {
                    *o = y;
                }
            }
            self.pos += 1;
            if self.pos >= n {
                self.pos = 0;
            }
            self.count += 1;
            if self.count >= self.hop {
                self.count = 0;
                self.frame(nc);
            }
        }
    }

    /// One STFT frame: the last N samples in, their filtered version overlap-added out.
    fn frame(&mut self, nc: usize) {
        let n = self.n;
        let (Some(fft), Some(ifft)) = (self.fft.clone(), self.ifft.clone()) else { return };
        // Left in the real part, right in the imaginary one (mono: imaginary zero).
        for j in 0..n {
            let idx = (self.pos + j) % n;
            let w = self.window.get(j).copied().unwrap_or(0.0);
            let l = self.inp.first().and_then(|v| v.get(idx)).copied().unwrap_or(0.0);
            let r = if nc > 1 { self.inp.get(1).and_then(|v| v.get(idx)).copied().unwrap_or(0.0) } else { 0.0 };
            if let Some(b) = self.buf.get_mut(j) {
                *b = Complex::new(l * w, r * w);
            }
        }
        fft.process_with_scratch(&mut self.buf, &mut self.scratch);
        self.analyse(nc);
        self.gains();
        // The same real gain on a bin and its mirror keeps left real and right imaginary.
        let half = n / 2;
        let (delta, bypass) = (self.set.delta, self.set.bypass);
        for k in 0..=half {
            let g = if bypass { 1.0 } else { self.bin_gain.get(k).copied().unwrap_or(1.0) };
            let g = if delta && !bypass { g - 1.0 } else { g };
            if let Some(b) = self.buf.get_mut(k) {
                *b *= g;
            }
            if k > 0
                && k < half
                && let Some(b) = self.buf.get_mut(n - k)
            {
                *b *= g;
            }
        }
        ifft.process_with_scratch(&mut self.buf, &mut self.scratch);
        // Hann twice at a hop of N/8 sums to 3; the inverse FFT is unnormalised.
        let norm = 1.0 / (3.0 * n as f32);
        for j in 0..n {
            let idx = (self.pos + j) % n;
            let w = self.window.get(j).copied().unwrap_or(0.0) * norm;
            let v = self.buf.get(j).copied().unwrap_or_default();
            if let Some(o) = self.ola.first_mut().and_then(|o| o.get_mut(idx)) {
                *o += v.re * w;
            }
            if nc > 1
                && let Some(o) = self.ola.get_mut(1).and_then(|o| o.get_mut(idx))
            {
                *o += v.im * w;
            }
        }
    }

    /// The frame's power per band (per ERB, a full-scale sine near 0 dB), smoothed in time.
    fn analyse(&mut self, nc: usize) {
        let n = self.n;
        let half = n / 2;
        self.raw = [0.0; MAX_BANDS];
        // |Z[k]|² + |Z[N-k]|² is 2(|L|² + |R|²); a Hann-windowed sine of amplitude A peaks at
        // A·N/4.
        let scale = 0.5 / nc as f32 / (n as f32 * 0.25).powi(2);
        for k in 1..half {
            let a = self.buf.get(k).map_or(0.0, |c| c.norm_sqr());
            let b = self.buf.get(n - k).map_or(0.0, |c| c.norm_sqr());
            let p = (a + b) * scale;
            let band = self.bin_band.get(k).copied().unwrap_or(0) as usize;
            let frac = self.bin_frac.get(k).copied().unwrap_or(0.0);
            if let Some(r) = self.raw.get_mut(band) {
                *r += p * (1.0 - frac);
            }
            if let Some(r) = self.raw.get_mut(band + 1) {
                *r += p * frac;
            }
        }
        for i in 0..self.bands {
            let mean = self.raw.get(i).copied().unwrap_or(0.0) * self.inv_weight.get(i).copied().unwrap_or(0.0);
            let p = mean * self.bins_per_erb.get(i).copied().unwrap_or(1.0);
            let p = if p.is_finite() { p } else { 0.0 };
            if let Some(l) = self.level.get_mut(i) {
                let c = if p > *l { self.level_att } else { self.level_rel };
                *l = p + (*l - p) * c;
                if *l < TINY {
                    *l = 0.0;
                }
            }
        }
    }

    /// From the band levels to the gain per bin.
    fn gains(&mut self) {
        let t = self.tuning;
        let s = self.set;
        let bands = self.bands;
        // Dominance: each band against its neighbourhood; and how far over the floor it is.
        let mut loudest = TINY;
        for i in 0..bands {
            loudest = loudest.max(self.level.get(i).copied().unwrap_or(0.0));
        }
        let loudest_db = pow_db(loudest);
        let mut active = 0usize;
        for i in 0..bands {
            let own = self.level.get(i).copied().unwrap_or(0.0);
            let mut around = 0.0f32;
            if let Some(row) = self.spread.get(i * bands..(i + 1) * bands) {
                for (w, l) in row.iter().zip(self.level.iter()) {
                    around += w * l;
                }
            }
            let own_db = pow_db(own);
            let d = own_db - pow_db(around);
            if let Some(v) = self.dom.get_mut(i) {
                *v = d;
            }
            // 0 at the floor, 1 a fade above it (relative to the loudest band, and absolute).
            let rel = ((own_db - loudest_db - t.floor_db) / t.floor_fade_db).clamp(0.0, 1.0);
            let abs = ((own_db - t.abs_floor_db) / t.floor_fade_db).clamp(0.0, 1.0);
            let guard = rel * abs;
            if let Some(v) = self.tmp.get_mut(i) {
                *v = guard;
            }
            if guard > 0.0
                && self.range.get(i).copied().unwrap_or(0.0) > 0.5
                && let Some(v) = self.sorted.get_mut(active)
            {
                *v = d;
                active += 1;
            }
        }
        // The median over the bands that count: shape, not level.
        let median_of = |v: &mut [f32]| {
            let mid = v.len() / 2;
            let (_, m, _) = v.select_nth_unstable_by(mid, f32::total_cmp);
            *m
        };
        let median = match self.sorted.get_mut(..active) {
            Some(v) if active >= 3 => Some(median_of(v)),
            _ => None,
        };
        // The broad balance: each region against pink, and against the regions' own median.
        let mut total = 0.0f32;
        let mut broad_active = 0usize;
        for i in 0..bands {
            let mut around = 0.0f32;
            if let Some(row) = self.wide.get(i * bands..(i + 1) * bands) {
                for (w, l) in row.iter().zip(self.level.iter()) {
                    around += w * l;
                }
            }
            let d = pow_db(around) - self.pink_db.get(i).copied().unwrap_or(0.0);
            if let Some(v) = self.broad.get_mut(i) {
                *v = d;
            }
            if self.tmp.get(i).copied().unwrap_or(0.0) > 0.0 && self.range.get(i).copied().unwrap_or(0.0) > 0.5 {
                if let Some(v) = self.sorted.get_mut(broad_active) {
                    *v = d;
                    broad_active += 1;
                }
            }
            let weight = self.inv_weight.get(i).copied().unwrap_or(0.0);
            let per_erb = self.bins_per_erb.get(i).copied().unwrap_or(1.0);
            if weight > 0.0 {
                total += self.level.get(i).copied().unwrap_or(0.0) / (weight * per_erb);
            }
        }
        let broad_median = match self.sorted.get_mut(..broad_active) {
            Some(v) if broad_active >= 3 => median_of(v),
            _ => 0.0,
        };
        // Quieter than the reference the targets lean out, louder they lean in.
        let quiet = ((t.level_ref_db - pow_db(total)) / 20.0).clamp(-1.0, 2.0);
        let lean_tame = (1.0 + t.tame_level * quiet).max(0.0);
        let lean_recover = (1.0 + t.recover_level * quiet).max(0.0);
        for i in 0..bands {
            let guard = self.tmp.get(i).copied().unwrap_or(0.0);
            let want = match median {
                Some(m) => {
                    let shift = s.bias * t.bias_db;
                    let x = self.dom.get(i).copied().unwrap_or(0.0) - m - shift;
                    let b = self.broad.get(i).copied().unwrap_or(0.0) - broad_median - shift;
                    let over = (b - self.tame_at.get(i).copied().unwrap_or(0.0) * lean_tame).max(0.0);
                    let under = (self.recover_at.get(i).copied().unwrap_or(0.0) * lean_recover - b).max(0.0);
                    let cut = soft_cap(s.tame * (t.tame_ratio * knee(x, t.knee_db) + t.tame_broad * over), s.tame * t.max_cut_db);
                    let lift = soft_cap(s.recover * (t.recover_ratio * knee(-x, t.knee_db) + t.recover_broad * under), s.recover * t.max_boost_db) * guard;
                    lift - cut
                }
                None => 0.0,
            };
            if let Some(w) = self.want.get_mut(i) {
                *w = if want.is_finite() { want } else { 0.0 };
            }
        }
        // Over frequency: 1-2-1 passes (the ends keep their own weight).
        for _ in 0..t.smooth_passes {
            for i in 0..bands {
                let at = |j: usize| self.want.get(j).copied().unwrap_or(0.0);
                let lo = at(i.saturating_sub(1));
                let hi = at((i + 1).min(bands - 1));
                if let Some(v) = self.tmp.get_mut(i) {
                    *v = 0.25 * lo + 0.5 * at(i) + 0.25 * hi;
                }
            }
            for i in 0..bands {
                if let (Some(w), Some(v)) = (self.want.get_mut(i), self.tmp.get(i)) {
                    *w = *v;
                }
            }
        }
        // Over time, inside the working range; then the tilts.
        let mut max_cut = 0.0f32;
        let mut max_boost = 0.0f32;
        let mut before = 0.0f32;
        let mut after = 0.0f32;
        for i in 0..bands {
            let range = self.range.get(i).copied().unwrap_or(0.0);
            let want = self.want.get(i).copied().unwrap_or(0.0) * range;
            let mut g = self.gain_db.get(i).copied().unwrap_or(0.0);
            let c = if want.abs() > g.abs() { self.gain_att } else { self.gain_rel };
            g = want + (g - want) * c;
            if !g.is_finite() || g.abs() < 1e-5 {
                g = 0.0;
            }
            if let Some(v) = self.gain_db.get_mut(i) {
                *v = g;
            }
            max_cut = max_cut.max(-g);
            max_boost = max_boost.max(g);
            let total = g + self.tilt.get(i).copied().unwrap_or(0.0) * range;
            if let Some(v) = self.out_db.get_mut(i) {
                *v = total;
            }
            // The band's share of the frame's power (back from per-ERB to per-band).
            let weight = self.inv_weight.get(i).copied().unwrap_or(0.0);
            let per_erb = self.bins_per_erb.get(i).copied().unwrap_or(1.0);
            let power = if weight > 0.0 { self.level.get(i).copied().unwrap_or(0.0) / (weight * per_erb) } else { 0.0 };
            before += power;
            after += power * (total * 2.0 * LN10_OVER_20).exp();
        }
        self.max_cut = max_cut;
        self.max_boost = max_boost;
        // Give the level back: the curve changes the balance, not how loud it is.
        let comp = if before > TINY && after > TINY { (-t.level_comp * 10.0 * (after / before).log10()).clamp(-12.0, 12.0) } else { 0.0 };
        self.comp_db = if comp.is_finite() { comp } else { 0.0 };
        for i in 0..bands {
            if let Some(v) = self.out_db.get_mut(i) {
                *v += self.comp_db;
            }
        }
        // Onto the bins.
        let last = bands.saturating_sub(1);
        for k in 0..self.bin_gain.len() {
            let band = (self.bin_band.get(k).copied().unwrap_or(0) as usize).min(last);
            let frac = self.bin_frac.get(k).copied().unwrap_or(0.0);
            let a = self.out_db.get(band).copied().unwrap_or(0.0);
            let b = self.out_db.get((band + 1).min(last)).copied().unwrap_or(a);
            let db = a + (b - a) * frac;
            if let Some(g) = self.bin_gain.get_mut(k) {
                *g = (db * LN10_OVER_20).exp();
            }
        }
    }
}
