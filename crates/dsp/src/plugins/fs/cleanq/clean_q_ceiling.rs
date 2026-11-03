//! CleanQCeiling -- noise guides, a captured spectrum, a ceiling, and Conform.
//! Line-by-line translation of `CleanQ/Source/CleanQCeiling.h` (`namespace ceiling`).
//!
//! The idea is Ayaic's "Ceilings of Sound": measure what a source's spectrum really is, draw the
//! shape you want it to have -- a "ceiling" made of straight runs in dB per octave -- and have the
//! EQ written for you. Four pieces, all maths:
//!
//! - `SpectrumAverager`: a long-term POWER average of the analyser's bins, smoothed to a fraction
//!   of an octave, on which pink noise measures -3 dB/oct.
//! - noise guides: straight lines on that scale: white 0, pink -3, brown -6, blue +3, violet +6.
//! - `Ceiling`: always 16 nodes joined by straight runs on log frequency.
//! - `conform()`: the EQ that moves the capture onto the ceiling (smoothed to 1 octave, the level
//!   taken out), written as up to 12 fixed bells at Q 1 solved by least squares, with the make-up
//!   gain that keeps the level. It REPLACES the EQ.
//!
//! None of this runs on the audio thread: these are the editor's tools (they allocate). The audio
//! thread only uses the grid (`GRID_N`, `GRID_LO`, `GRID_HI`, `grid_hz`), via the dynamic ceiling.
//!
//! Translation notes: `std::nth_element` is `select_nth_unstable_by` (the n-th value is the same
//! whatever the algorithm); `std::sort` on the points uses a total order on `hz`; `std::signbit`
//! is `is_sign_negative`; `std::round` is `f64::round` (half away from zero, as the C++).

use super::clean_q_dsp::{self as dsp, PI, TYPE_BELL};

// ------------------------------------------------------------------ the working grid
/// Everything happens on one log-spaced grid, 20 Hz .. 20 kHz; 256 points is ~26 per octave.
pub const GRID_N: usize = 256;
pub const GRID_LO: f64 = 20.0;
pub const GRID_HI: f64 = 20000.0;
pub const FLOOR_DB: f64 = -120.0;

pub fn grid_hz(i: usize) -> f64 {
    GRID_LO * (GRID_HI / GRID_LO).powf(i as f64 / (GRID_N - 1) as f64)
}

pub fn octaves_from_1k(hz: f64) -> f64 {
    (hz / 1000.0).log2()
}

// ------------------------------------------------------------------ noise colours
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoiseColour {
    pub name: &'static str,
    pub slope_db_per_oct: f64,
    pub argb: u32,
}

pub const NUM_NOISES: usize = 5;
pub static NOISES: [NoiseColour; NUM_NOISES] = [
    NoiseColour { name: "White", slope_db_per_oct: 0.0, argb: 0xFFE8ECEF },
    NoiseColour { name: "Pink", slope_db_per_oct: -3.0, argb: 0xFFFF8FC7 },
    NoiseColour { name: "Brown", slope_db_per_oct: -6.0, argb: 0xFFC08A5A },
    NoiseColour { name: "Blue", slope_db_per_oct: 3.0, argb: 0xFF6FA8FF },
    NoiseColour { name: "Violet", slope_db_per_oct: 6.0, argb: 0xFFB58CFF },
];

/// dB at `hz` of a noise line with this slope, passing through `level_at_1k`.
pub fn noise_line_db(slope: f64, level_at_1k: f64, hz: f64) -> f64 {
    level_at_1k + slope * octaves_from_1k(hz)
}

// ------------------------------------------------------------------ averaging
/// A running power average of the analyser's one-sided magnitude bins, with an exponential
/// memory of `tau_seconds`. The first frames are averaged equally.
#[derive(Debug, Clone, Default)]
pub struct SpectrumAverager {
    power: Vec<f64>,
    frames: i32,
    seconds: f64,
    /// The largest |sample| of the frames averaged (linear), for Conform's make-up headroom.
    peak: f64,
}

impl SpectrumAverager {
    pub fn reset(&mut self) {
        let n = self.power.len();
        self.power.clear();
        self.power.resize(n, 0.0);
        self.frames = 0;
        self.seconds = 0.0;
        self.peak = 0.0;
    }

    /// The learned material's sample peak so far: `abs_peak` is the largest |sample| of a frame
    /// that was averaged. Kept as the maximum since the last reset (whatever the memory).
    pub fn note_peak(&mut self, abs_peak: f64) {
        if abs_peak.is_finite() && abs_peak > self.peak {
            self.peak = abs_peak;
        }
    }

    /// The learned material's sample peak in dBFS; -inf when nothing has been noted.
    pub fn peak_db(&self) -> f64 {
        if self.peak > 0.0 { 20.0 * self.peak.log10() } else { f64::NEG_INFINITY }
    }

    /// `mag` is |X[k]| already scaled to the display's full-scale convention, i.e. the raw FFT
    /// magnitude times `FFT_MAGNITUDE_SCALE`.
    pub fn add(&mut self, mag: &[f32], dt_seconds: f64, tau_seconds: f64) {
        let num_bins = mag.len();
        if self.power.len() != num_bins {
            self.power.clear();
            self.power.resize(num_bins, 0.0);
            self.frames = 0;
            self.seconds = 0.0;
        }

        // tau 0 means "everything played": a plain average over every frame
        let alpha_tau = if tau_seconds <= 0.0 { 0.0 } else { 1.0 - (-dt_seconds / 0.05f64.max(tau_seconds)).exp() };
        let alpha = alpha_tau.max(1.0 / f64::from(self.frames + 1));

        for (p, &m) in self.power.iter_mut().zip(mag) {
            let x = f64::from(m) * f64::from(m);
            *p += (x - *p) * alpha;
        }

        self.frames += 1;
        self.seconds += dt_seconds;
    }

    pub fn has_data(&self) -> bool {
        self.frames > 0
    }
    pub fn frame_count(&self) -> i32 {
        self.frames
    }
    pub fn seconds_averaged(&self) -> f64 {
        self.seconds
    }
    pub fn bins(&self) -> &[f64] {
        &self.power
    }
}

/// Mean power over a band [f/2^(1/2N), f*2^(1/2N)] at every grid point, in dB. `fraction` is N.
pub fn smooth_to_grid(power: &[f64], sample_rate: f64, fft_size: usize, fraction: f64) -> Vec<f64> {
    let mut out = vec![FLOOR_DB; GRID_N];
    if power.is_empty() || sample_rate <= 0.0 {
        return out;
    }

    let n = power.len() as i64;
    let bin_hz = sample_rate / fft_size as f64;
    let half = 2.0f64.powf(0.5 / 1.0f64.max(fraction));
    let at = |k: i64| power.get(k.max(0) as usize).copied().unwrap_or(0.0);

    for (i, o) in out.iter_mut().enumerate() {
        let f = grid_hz(i);
        let lo = f / half / bin_hz;
        let hi = f * half / bin_hz; // in bins
        let mean = if hi - lo < 1.0 {
            // narrower than a bin: interpolate at the centre
            let c = f / bin_hz;
            let k0 = (c.floor() as i64).clamp(1, (n - 1).max(1));
            let k1 = (k0 + 1).min(n - 1);
            let t = (c - k0 as f64).clamp(0.0, 1.0);
            at(k0) * (1.0 - t) + at(k1) * t
        } else {
            // bin k covers [k - 0.5, k + 0.5]
            let mut sum = 0.0;
            let mut wsum = 0.0;
            let k0 = 1i64.max((lo + 0.5).floor() as i64);
            let k1 = (n - 1).min((hi - 0.5).ceil() as i64);
            for k in k0..=k1 {
                let a = lo.max(k as f64 - 0.5);
                let b = hi.min(k as f64 + 0.5);
                let w = 0.0f64.max(b - a);
                sum += w * at(k);
                wsum += w;
            }
            if wsum > 0.0 { sum / wsum } else { 0.0 }
        };

        *o = if mean > 1.0e-12 { 10.0 * mean.log10() } else { FLOOR_DB };
    }

    out
}

/// The level a noise line of this slope needs to sit on `db` through [lo, hi]: the weighted mean
/// of (spectrum - slope * octaves). `floor_ok` is -100 in the C++ default.
pub fn fit_noise_level(db: &[f64], slope: f64, lo: f64, hi: f64, floor_ok: f64) -> f64 {
    let mut s = 0.0;
    let mut w = 0.0;
    for (i, &d) in db.iter().enumerate().take(GRID_N) {
        let f = grid_hz(i);
        if f < lo || f > hi || d <= floor_ok {
            continue;
        }
        s += d - slope * octaves_from_1k(f);
        w += 1.0;
    }
    if w > 0.0 { s / w } else { -60.0 }
}

// ------------------------------------------------------------------ the ceiling
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub hz: f64,
    pub db: f64,
}

impl Default for Point {
    fn default() -> Self {
        Point { hz: 1000.0, db: -40.0 }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ceiling {
    /// Ascending in frequency.
    pub pts: Vec<Point>,
}

impl Ceiling {
    pub fn is_valid(&self) -> bool {
        self.pts.len() >= 2
    }
    pub fn lo_hz(&self) -> f64 {
        if self.is_valid() { self.pts[0].hz } else { 0.0 }
    }
    pub fn hi_hz(&self) -> f64 {
        match self.pts.last() {
            Some(p) if self.is_valid() => p.hz,
            _ => 0.0,
        }
    }

    pub fn covers(&self, hz: f64) -> bool {
        self.is_valid() && hz >= self.lo_hz() && hz <= self.hi_hz()
    }

    /// Straight lines on log frequency between the points. Outside the ends it holds the end
    /// value; callers that care use `covers()`.
    pub fn at(&self, hz: f64) -> f64 {
        let (Some(first), Some(last)) = (self.pts.first(), self.pts.last()) else { return 0.0 };
        if !self.is_valid() {
            return 0.0;
        }
        if hz <= first.hz {
            return first.db;
        }
        if hz >= last.hz {
            return last.db;
        }

        for w in self.pts.windows(2) {
            if hz <= w[1].hz {
                let x0 = w[0].hz.log2();
                let x1 = w[1].hz.log2();
                let t = (hz.log2() - x0) / 1.0e-9f64.max(x1 - x0);
                return w[0].db + (w[1].db - w[0].db) * t;
            }
        }
        last.db
    }

    /// dB per octave of the run from point i to point i+1.
    pub fn slope(&self, i: usize) -> f64 {
        let (Some(a), Some(b)) = (self.pts.get(i), self.pts.get(i + 1)) else { return 0.0 };
        let oct = (b.hz / a.hz).log2();
        if oct > 1.0e-9 { (b.db - a.db) / oct } else { 0.0 }
    }

    pub fn sort(&mut self) {
        self.pts.sort_by(|a, b| a.hz.total_cmp(&b.hz));
    }

    /// The ceiling on the dynamic ceiling's grid, in dBFS (`pushCeilingToAudio`): empty when the
    /// shape is not valid.
    pub fn to_grid(&self) -> Vec<f64> {
        if !self.is_valid() {
            return Vec::new();
        }
        (0..GRID_N).map(|i| self.at(grid_hz(i))).collect()
    }
}

pub const NUM_NODES: usize = 16;

/// 16 nodes evenly spaced 20 Hz .. 20 kHz, flat at `db`: Reset.
pub fn flat_ceiling(db: f64) -> Ceiling {
    Ceiling { pts: (0..NUM_NODES).map(|k| Point { hz: GRID_LO * (GRID_HI / GRID_LO).powf(k as f64 / (NUM_NODES - 1) as f64), db }).collect() }
}

/// Where the ceiling's value is read from the capture: its dB, held out of the floor.
pub fn node_level(db: f64) -> f64 {
    db.clamp(-96.0, 0.0)
}

/// Auto Generate: 16 nodes ON the capture, 20 Hz .. 20 kHz, placed where it turns (dynamic
/// programming over the grid; nodes at least a sixth of an octave apart).
pub fn auto_ceiling(db: &[f64]) -> Ceiling {
    let mut c = Ceiling::default();
    if db.len() != GRID_N {
        return c;
    }

    let x: Vec<f64> = (0..GRID_N).map(|i| grid_hz(i).log2()).collect();
    let y: Vec<f64> = db.iter().map(|&d| node_level(d)).collect();

    let min_gap = 1usize.max(((GRID_N - 1) as f64 / (6.0 * (GRID_HI / GRID_LO).log2())).round() as usize);
    let segs = NUM_NODES - 1;

    // cost of a straight run from node a to node b through the capture between them
    let mut cost = vec![0.0f64; GRID_N * GRID_N];
    for a in 0..GRID_N {
        for b in (a + min_gap)..GRID_N {
            let mut e = 0.0;
            let dx = x[b] - x[a];
            let dy = y[b] - y[a];
            for k in (a + 1)..b {
                let r = y[a] + dy * (x[k] - x[a]) / dx - y[k];
                e += r * r;
            }
            cost[a * GRID_N + b] = e;
        }
    }

    let inf = f64::INFINITY;
    let mut best = vec![vec![inf; GRID_N]; segs + 1];
    let mut from = vec![vec![-1i64; GRID_N]; segs + 1];
    best[0][0] = 0.0;

    for s_idx in 1..=segs {
        for b in (s_idx * min_gap)..GRID_N {
            let mut a = (s_idx - 1) * min_gap;
            while a + min_gap <= b {
                let prev = best[s_idx - 1][a];
                if prev != inf {
                    let v = prev + cost[a * GRID_N + b];
                    if v < best[s_idx][b] {
                        best[s_idx][b] = v;
                        from[s_idx][b] = a as i64;
                    }
                }
                a += 1;
            }
        }
    }

    let mut idx: Vec<usize> = vec![GRID_N - 1];
    let mut b = GRID_N - 1;
    for s_idx in (1..=segs).rev() {
        let f = from[s_idx][b];
        if f < 0 {
            return Ceiling::default(); // unreachable for a 256-point grid; never index with -1
        }
        b = f as usize;
        idx.push(b);
    }
    idx.reverse();

    for i in idx {
        c.pts.push(Point { hz: grid_hz(i), db: y[i] });
    }
    c
}

/// Move node `i` to `db`. With `chain`, every node to its right moves by the same amount. Levels
/// stay inside the plot, -96 .. 0 dBFS.
pub fn move_node(c: &mut Ceiling, i: usize, db: f64, chain: bool) {
    let Some(p) = c.pts.get(i) else { return };
    let delta = db.clamp(-96.0, 0.0) - p.db;
    let last = if chain { c.pts.len() - 1 } else { i };
    for p in &mut c.pts[i..=last] {
        p.db = (p.db + delta).clamp(-96.0, 0.0);
    }
}

// ------------------------------------------------------------------ Conform
//
// Conform is Ayaic's: the EQ is the ceiling minus the capture, with the level taken out, realised
// as a smooth curve. It is NOT a fit of every wiggle: a sung capture still shows its notes at
// 1/3 octave, and chasing them with narrow bells (the first build's greedy fitter, Q up to 8)
// gave spikes Ayaic's plugin never makes. So:
//
// - the correction is smoothed to 1 octave (the ceiling and the capture alike, in dB), over
//   30 Hz .. 16 kHz only, and points more than 25 dB under the capture's median (bleed, rumble,
//   hiss) count for nothing: the correction is bridged across them and held flat past the ends,
//   so no shelf lifts what is only noise;
// - it is realised by 12 bells at fixed, log-spaced frequencies with one Q (about 1), their gains
//   solved together by weighted least squares on the summed response (a small cost on the
//   difference between neighbouring gains keeps them from fighting);
// - the range (+/-12 dB by default) holds the SUMMED curve, not each band;
// - the result carries the make-up gain that leaves the capture as loud through the new EQ as
//   it was without it, and the correction asked for before the clamp (for an honest status).

/// The span Conform corrects: outside it the correction is held flat.
pub const CONFORM_LO_HZ: f64 = 30.0;
pub const CONFORM_HI_HZ: f64 = 16000.0;
/// Points this far under the capture's median level are not corrected (bridged instead).
pub const CONFORM_QUIET_DB: f64 = 25.0;
/// The correction is smoothed over this many octaves (centred).
pub const CONFORM_SMOOTH_OCT: f64 = 1.0;
/// Fixed bells, log-spaced 20 Hz .. 16 kHz: the lowest holds the correction flat down to the
/// bottom of the grid; above 16 kHz the top one's skirt lets it fall back toward 0 dB (a bell up
/// at 20 kHz is cramped against Nyquist and fights its neighbour).
pub const CONFORM_MAX_BELLS: usize = 12;
pub const CONFORM_BELL_LO_HZ: f64 = 20.0;
pub const CONFORM_BELL_HI_HZ: f64 = 16000.0;
/// Their Q with all 12 (fewer bells are made wider to keep the span covered).
pub const CONFORM_Q: f64 = 1.0;
/// The weight of a point that is only bridged or held (not measured).
pub const CONFORM_HOLD_WEIGHT: f64 = 0.1;
/// What a difference between neighbouring bells' gains costs, per dB squared, against the
/// weighted squared error (a bell covers about 35 grid points).
pub const CONFORM_NEIGHBOUR_COST: f64 = 1.0;
/// No one band past this (the gain knob's own range is +/-30 dB).
pub const CONFORM_BAND_LIMIT_DB: f64 = 30.0;
/// A positive make-up never lifts the learned material's sample peak past -this dBFS.
pub const CONFORM_PEAK_HEADROOM_DB: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConformSettings {
    /// 0..1 of the correction.
    pub amount: f64,
    /// The summed curve stays inside +/- this.
    pub max_db: f64,
    /// Bells to use: at most [`CONFORM_MAX_BELLS`].
    pub max_bands: i32,
    pub cuts_only: bool,
    pub sample_rate: f64,
    /// The learned material's sample peak (dBFS, [`SpectrumAverager::peak_db`]): a positive
    /// make-up is held so that peak stays [`CONFORM_PEAK_HEADROOM_DB`] under 0 dBFS. -inf or NaN:
    /// unknown, no cap.
    pub input_peak_db: f64,
}

impl Default for ConformSettings {
    fn default() -> Self {
        ConformSettings {
            amount: 1.0,
            max_db: 12.0,
            max_bands: CONFORM_MAX_BELLS as i32,
            cuts_only: false,
            sample_rate: 48000.0,
            input_peak_db: f64::NEG_INFINITY,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GenBand {
    pub type_: i32,
    pub hz: f64,
    pub db: f64,
    pub q: f64,
}

impl Default for GenBand {
    fn default() -> Self {
        GenBand { type_: TYPE_BELL, hz: 1000.0, db: 0.0, q: 1.0 }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConformResult {
    pub bands: Vec<GenBand>,
    /// The correction asked for, per grid point, after Amount and the range.
    pub target: Vec<f64>,
    /// How much each point counts (1 measured, [`CONFORM_HOLD_WEIGHT`] bridged or held).
    pub weight: Vec<f64>,
    /// The summed response of `bands`, per grid point.
    pub curve: Vec<f64>,
    pub rms_before: f64,
    pub rms_after: f64,
    pub worst_after: f64,
    /// The correction asked for BEFORE the range clamp (after Amount): its lowest and highest.
    pub asked_lo: f64,
    pub asked_hi: f64,
    /// The summed curve's lowest and highest, 20 Hz .. 20 kHz.
    pub curve_lo: f64,
    pub curve_hi: f64,
    /// Output gain that keeps the capture as loud through the EQ as without it (dB), after the
    /// peak cap.
    pub makeup_db: f64,
    /// The make-up the level asked for, before the peak cap.
    pub makeup_wanted_db: f64,
    /// The peak cap held the make-up under what the level asked for.
    pub makeup_capped: bool,
}

impl ConformResult {
    /// The correction asked for went past the range: the ceiling is far from the source.
    pub fn clamped(&self, max_db: f64) -> bool {
        self.asked_lo < -max_db - 0.05 || self.asked_hi > max_db + 0.05
    }
}

/// |H| in dB of one generated band at every grid point. Same design call the audio uses.
pub fn band_response(b: &GenBand, sr: f64, out: &mut Vec<f64>) {
    out.resize(GRID_N, 0.0);
    let d = dsp::design_filter(b.type_, 2, b.hz, b.db, b.q, sr);
    for (i, o) in out.iter_mut().enumerate() {
        let w = 2.0 * PI * grid_hz(i).min(sr * 0.499) / sr;
        *o = dsp::magnitude_db(&d, w);
    }
}

/// The summed response of `bands` on the grid.
pub fn bands_curve(bands: &[GenBand], sr: f64) -> Vec<f64> {
    let mut sum = vec![0.0f64; GRID_N];
    let mut r = Vec::new();
    for b in bands {
        band_response(b, sr, &mut r);
        for (s, x) in sum.iter_mut().zip(&r) {
            *s += x;
        }
    }
    sum
}

/// The middle value (`std::nth_element` at size / 2); 0 for none.
fn median_of(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mid = v.len() / 2;
    v.select_nth_unstable_by(mid, f64::total_cmp);
    v[mid]
}

/// The correction Conform asks for, with its weights and the range it asked for before the clamp.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConformTarget {
    pub target: Vec<f64>,
    pub weight: Vec<f64>,
    pub asked_lo: f64,
    pub asked_hi: f64,
}

/// The correction: (ceiling - capture) smoothed to 1 octave, over 30 Hz .. 16 kHz where the
/// capture is within 25 dB of its median, its median removed (shape, not level), scaled by Amount,
/// cuts only when asked, then held to +/- the range. Points not measured are bridged in a straight
/// line between their measured neighbours, and held at the edge value past the ends, at a light
/// weight. `measured` is the capture of the INPUT: Conform replaces the whole EQ.
pub fn conform_target(measured: &[f64], c: &Ceiling, s: &ConformSettings) -> ConformTarget {
    let mut t = ConformTarget { target: vec![0.0; GRID_N], weight: vec![0.0; GRID_N], asked_lo: 0.0, asked_hi: 0.0 };
    if !c.is_valid() || measured.len() < GRID_N {
        return t;
    }

    // The capture's typical level over the span.
    let mut lv = Vec::new();
    for (i, &m) in measured.iter().enumerate().take(GRID_N) {
        let f = grid_hz(i);
        if (CONFORM_LO_HZ..=CONFORM_HI_HZ).contains(&f) && m > -100.0 {
            lv.push(m);
        }
    }
    if lv.is_empty() {
        return t;
    }
    let level = median_of(&mut lv);

    let mut valid = [false; GRID_N];
    let mut raw = [0.0f64; GRID_N];
    for i in 0..GRID_N {
        let f = grid_hz(i);
        let m = measured[i];
        if (CONFORM_LO_HZ..=CONFORM_HI_HZ).contains(&f) && c.covers(f) && m > -100.0 && m >= level - CONFORM_QUIET_DB {
            valid[i] = true;
            raw[i] = c.at(f) - m;
        }
    }

    // Smoothed to 1 octave: the mean over the measured points within half an octave each side.
    let k = (0.5 * CONFORM_SMOOTH_OCT * (GRID_N - 1) as f64 / (GRID_HI / GRID_LO).log2()).floor() as usize;
    let mut sm = [0.0f64; GRID_N];
    let mut inside = Vec::new();
    for i in 0..GRID_N {
        if !valid[i] {
            continue;
        }
        let mut sum = 0.0;
        let mut n = 0.0;
        for j in i.saturating_sub(k)..=(i + k).min(GRID_N - 1) {
            if valid[j] {
                sum += raw[j];
                n += 1.0;
            }
        }
        sm[i] = sum / n;
        inside.push(sm[i]);
    }
    if inside.is_empty() {
        return t;
    }
    let mid = median_of(&mut inside);

    let amount = if s.amount.is_finite() { s.amount.clamp(0.0, 1.0) } else { 1.0 };
    let max_db = s.max_db.max(0.0); // NaN reads as 0 (and `clamp` never sees min > max)
    let (mut lo, mut hi) = (0.0f64, 0.0f64);
    for i in 0..GRID_N {
        if !valid[i] {
            continue;
        }
        let mut v = (sm[i] - mid) * amount;
        if s.cuts_only {
            v = v.min(0.0);
        }
        lo = lo.min(v);
        hi = hi.max(v);
        t.target[i] = v.clamp(-max_db, max_db);
        t.weight[i] = 1.0;
    }
    t.asked_lo = lo;
    t.asked_hi = hi;

    // Bridged across what was not measured, held flat past the ends.
    let mut prev = [usize::MAX; GRID_N];
    let mut next = [usize::MAX; GRID_N];
    let mut p = usize::MAX;
    for i in 0..GRID_N {
        if valid[i] {
            p = i;
        }
        prev[i] = p;
    }
    p = usize::MAX;
    for i in (0..GRID_N).rev() {
        if valid[i] {
            p = i;
        }
        next[i] = p;
    }
    for i in 0..GRID_N {
        if valid[i] {
            continue;
        }
        let (a, b) = (prev[i], next[i]);
        t.target[i] = if a != usize::MAX && b != usize::MAX {
            t.target[a] + (t.target[b] - t.target[a]) * (i - a) as f64 / (b - a) as f64
        } else if a != usize::MAX {
            t.target[a]
        } else {
            t.target[b]
        };
        t.weight[i] = CONFORM_HOLD_WEIGHT;
    }
    t
}

/// Solves `a x = b` by Gaussian elimination with partial pivoting (the first largest pivot).
/// Indexed loops, as the C++ writes them (the same operations in the same order).
#[allow(clippy::needless_range_loop)]
fn solve_linear(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Vec<f64> {
    let n = b.len();
    for c in 0..n {
        let mut p = c;
        for r in (c + 1)..n {
            if a[r][c].abs() > a[p][c].abs() {
                p = r;
            }
        }
        a.swap(c, p);
        b.swap(c, p);
        if a[c][c].abs() < 1.0e-300 {
            continue;
        }
        for r in (c + 1)..n {
            let f = a[r][c] / a[c][c];
            for k in c..n {
                a[r][k] -= f * a[c][k];
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = vec![0.0f64; n];
    for c in (0..n).rev() {
        let mut v = b[c];
        for k in (c + 1)..n {
            v -= a[c][k] * x[k];
        }
        x[c] = if a[c][c].abs() < 1.0e-300 { 0.0 } else { v / a[c][c] };
    }
    x
}

/// The summed response of the bands that are `on`.
fn live_curve(bands: &[GenBand], on: &[bool], sr: f64) -> Vec<f64> {
    let live: Vec<GenBand> = bands.iter().zip(on).filter(|(_, o)| **o).map(|(b, _)| *b).collect();
    bands_curve(&live, sr)
}

/// Gauss-Newton on the gains of the `on` bands: each step solves the weighted least squares for a
/// gain change through each band's response per dB (taken at +6 dB), against the real summed
/// response; a small ridge keeps neighbours from fighting.
#[allow(clippy::too_many_arguments)]
fn solve_gains(bands: &mut [GenBand], on: &[bool], unit: &[Vec<f64>], target: &[f64], weight: &[f64], sr: f64, cuts_only: bool, iterations: usize) {
    let idx: Vec<usize> = (0..bands.len()).filter(|&j| on[j]).collect();
    let m = idx.len();
    if m == 0 {
        return;
    }
    for _ in 0..iterations {
        let sum = live_curve(bands, on, sr);
        let mut a = vec![vec![0.0f64; m]; m];
        let mut b = vec![0.0f64; m];
        for i in 0..GRID_N {
            let r = target[i] - sum[i];
            for (p, &jp) in idx.iter().enumerate() {
                let up = weight[i] * unit[jp][i];
                b[p] += up * r;
                for (q, &jq) in idx.iter().enumerate() {
                    a[p][q] += up * unit[jq][i];
                }
            }
        }
        for (p, row) in a.iter_mut().enumerate() {
            row[p] += 1.0e-3;
        }
        // Neighbouring bells kept from fighting: their gain differences cost a little.
        let lam = CONFORM_NEIGHBOUR_COST;
        for p in 0..m.saturating_sub(1) {
            let d = bands[idx[p]].db - bands[idx[p + 1]].db;
            a[p][p] += lam;
            a[p + 1][p + 1] += lam;
            a[p][p + 1] -= lam;
            a[p + 1][p] -= lam;
            b[p] -= lam * d;
            b[p + 1] += lam * d;
        }
        let dg = solve_linear(a, b);
        for (p, &j) in idx.iter().enumerate() {
            let mut g = (bands[j].db + dg[p]).clamp(-CONFORM_BAND_LIMIT_DB, CONFORM_BAND_LIMIT_DB);
            if cuts_only {
                g = g.min(0.0);
            }
            bands[j].db = g;
        }
    }
}

/// The fixed bells for `s`: [`CONFORM_MAX_BELLS`] (or the budget, if smaller) log-spaced across
/// 20 Hz .. 16 kHz (held under 0.45 x the sample rate), at gain 0. Fewer bells are made wider in
/// step with their spacing.
pub fn conform_bells(s: &ConformSettings) -> Vec<GenBand> {
    let sr = if s.sample_rate > 0.0 { s.sample_rate } else { 48000.0 };
    let n = s.max_bands.clamp(1, CONFORM_MAX_BELLS as i32) as usize;
    let (blo, bhi) = (CONFORM_BELL_LO_HZ, CONFORM_BELL_HI_HZ);
    let span = (bhi / blo).log2();
    let spacing = if n > 1 { span / (n - 1) as f64 } else { span };
    let spacing_full = span / (CONFORM_MAX_BELLS - 1) as f64;
    let q = (CONFORM_Q * spacing_full / spacing).clamp(0.3, CONFORM_Q);
    (0..n)
        .map(|k| {
            let hz = if n > 1 { blo * (bhi / blo).powf(k as f64 / (n - 1) as f64) } else { (CONFORM_BELL_LO_HZ * CONFORM_BELL_HI_HZ).sqrt() };
            GenBand { type_: TYPE_BELL, hz: hz.min(sr * 0.45), db: 0.0, q }
        })
        .collect()
}

/// Realise a correction with the fixed bells: gains by least squares, bands under a quarter of a
/// dB dropped (and the rest solved again), then the summed curve held inside +/- the range --
/// first by solving again with the overshooting points pulled in and weighted up, and finally by
/// scaling every gain down.
pub fn fit_bands(target: &[f64], weight: &[f64], s: &ConformSettings) -> ConformResult {
    let mut res = ConformResult { target: target.to_vec(), weight: weight.to_vec(), ..ConformResult::default() };
    if target.len() < GRID_N || weight.len() < GRID_N {
        return res;
    }
    let sr = if s.sample_rate > 0.0 { s.sample_rate } else { 48000.0 };
    let max_db = s.max_db.max(0.0);

    let rms_of = |sum: &[f64], worst: &mut f64| {
        let mut e = 0.0;
        let mut w = 0.0;
        let mut m = 0.0f64;
        for i in 0..GRID_N {
            if weight[i] < 0.5 {
                continue;
            }
            let r = target[i] - sum[i];
            e += r * r;
            w += 1.0;
            m = m.max(r.abs());
        }
        *worst = m;
        if w > 0.0 { (e / w).sqrt() } else { 0.0 }
    };
    let mut worst = 0.0;
    res.rms_before = rms_of(&[0.0; GRID_N], &mut worst);

    let mut bands = conform_bells(s);
    let unit: Vec<Vec<f64>> = bands
        .iter()
        .map(|b| {
            let mut r = Vec::new();
            band_response(&GenBand { db: 6.0, ..*b }, sr, &mut r);
            r.iter().map(|v| v / 6.0).collect()
        })
        .collect();
    let mut on = vec![true; bands.len()];

    if worst >= 0.25 {
        solve_gains(&mut bands, &on, &unit, target, weight, sr, s.cuts_only, 8);
        // Drop what does next to nothing, and solve the rest again.
        for _ in 0..bands.len() {
            let mut dropped = false;
            for (b, o) in bands.iter_mut().zip(on.iter_mut()) {
                if *o && b.db.abs() < 0.25 {
                    *o = false;
                    b.db = 0.0;
                    dropped = true;
                }
            }
            if !dropped {
                break;
            }
            solve_gains(&mut bands, &on, &unit, target, weight, sr, s.cuts_only, 4);
        }

        // The range holds the summed curve.
        let mut t2 = target.to_vec();
        let mut w2 = weight.to_vec();
        for _ in 0..4 {
            let sum = live_curve(&bands, &on, sr);
            let mut over = false;
            for i in 0..GRID_N {
                let held = sum[i].clamp(-max_db, max_db);
                if sum[i] != held {
                    over = true;
                    t2[i] -= sum[i] - held;
                    w2[i] = w2[i].max(1.0) * 4.0;
                }
            }
            if !over {
                break;
            }
            solve_gains(&mut bands, &on, &unit, &t2, &w2, sr, s.cuts_only, 4);
        }
        for _ in 0..12 {
            let sum = live_curve(&bands, &on, sr);
            let peak = sum.iter().fold(0.0f64, |m, v| m.max(v.abs()));
            if peak <= max_db + 1.0e-6 || peak <= 0.0 {
                break;
            }
            let f = (max_db / peak) * 0.9995;
            for (b, o) in bands.iter_mut().zip(&on) {
                if *o {
                    b.db *= f;
                }
            }
        }
    }

    res.bands = bands.iter().zip(&on).filter(|(b, o)| **o && b.db.abs() >= 0.05).map(|(b, _)| *b).collect();
    res.curve = bands_curve(&res.bands, sr);
    res.rms_after = rms_of(&res.curve, &mut worst);
    res.worst_after = worst;
    res.curve_lo = res.curve.iter().fold(0.0f64, |m, v| m.min(*v));
    res.curve_hi = res.curve.iter().fold(0.0f64, |m, v| m.max(*v));
    res
}

/// The output gain (dB) that keeps the capture's level through `eq` (dB per grid point) what it
/// was without it: the capture is a power density per Hz, so each grid point stands for
/// power x its frequency (the grid is log-spaced). Held to the output knob's +/-24 dB.
pub fn makeup_gain_db(measured: &[f64], eq: &[f64]) -> f64 {
    if measured.len() < GRID_N || eq.len() < GRID_N {
        return 0.0;
    }
    let mut num = 0.0;
    let mut den = 0.0;
    for i in 0..GRID_N {
        if measured[i] <= -100.0 {
            continue;
        }
        let p = 10.0f64.powf(measured[i] / 10.0) * grid_hz(i);
        den += p;
        num += p * 10.0f64.powf(eq[i] / 10.0);
    }
    if den <= 0.0 || num <= 0.0 {
        return 0.0;
    }
    (-10.0 * (num / den).log10()).clamp(-24.0, 24.0)
}

/// The whole of Conform: target from capture and ceiling, the bells for it, and the make-up gain.
pub fn conform(measured: &[f64], c: &Ceiling, s: &ConformSettings) -> ConformResult {
    let t = conform_target(measured, c, s);
    let mut r = fit_bands(&t.target, &t.weight, s);
    r.asked_lo = t.asked_lo;
    r.asked_hi = t.asked_hi;
    r.makeup_wanted_db = if r.bands.is_empty() { 0.0 } else { makeup_gain_db(measured, &r.curve) };
    r.makeup_db = cap_makeup_db(r.makeup_wanted_db, s.input_peak_db);
    r.makeup_capped = r.makeup_db < r.makeup_wanted_db;
    r
}

/// The make-up held by the learned material's peak headroom: a POSITIVE make-up never takes
/// `input_peak_db` past -[`CONFORM_PEAK_HEADROOM_DB`] dBFS (and is dropped to 0 when the material
/// is already hotter than that); a cut, or an unknown peak (-inf, NaN), passes as it is. No
/// limiter: the level is simply restored less. A positive make-up only follows an EQ that took
/// loudness off, which in practice does not raise the peaks (measured: at most +0.1 dB over 24
/// signal/ceiling cases), so the material's own peak is the one to keep under the line.
pub fn cap_makeup_db(makeup_db: f64, input_peak_db: f64) -> f64 {
    if makeup_db <= 0.0 || !input_peak_db.is_finite() {
        return makeup_db;
    }
    makeup_db.min((-CONFORM_PEAK_HEADROOM_DB - input_peak_db).max(0.0))
}

/// The output trim after a Conform (or a Remove, with `new_makeup_db` 0): the last Conform's
/// make-up taken back, this one's put on, held to the knob's +/-24 dB. Returns (output dB, the
/// make-up actually applied).
pub fn apply_makeup(out_before_db: f64, old_makeup_db: f64, new_makeup_db: f64) -> (f64, f64) {
    let trim = out_before_db - old_makeup_db;
    let out = (trim + new_makeup_db).clamp(-24.0, 24.0);
    (out, out - trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_and_flat_ceiling() {
        assert!((grid_hz(0) - 20.0).abs() < 1e-9 && (grid_hz(GRID_N - 1) - 20000.0).abs() < 1e-6);
        let c = flat_ceiling(-30.0);
        assert_eq!(c.pts.len(), NUM_NODES);
        assert_eq!(c.at(1234.0), -30.0);
        assert_eq!(c.to_grid().len(), GRID_N);
        assert!(Ceiling::default().to_grid().is_empty());
    }

    #[test]
    fn pink_noise_on_the_grid_measures_minus_3_db_per_octave() {
        // Pink power spectrum: power ~ 1/f.
        let n = 2049;
        let power: Vec<f64> = (0..n).map(|k| if k == 0 { 0.0 } else { 1.0e-3 / k as f64 }).collect();
        let db = smooth_to_grid(&power, 48000.0, 4096, 3.0);
        let lo = db[(0..GRID_N).find(|&i| grid_hz(i) >= 500.0).unwrap_or(0)];
        let hi = db[(0..GRID_N).find(|&i| grid_hz(i) >= 1000.0).unwrap_or(0)];
        assert!((hi - lo + 3.0).abs() < 0.3, "{lo} {hi}");
    }

    #[test]
    fn conform_cuts_a_bump_and_auto_ceiling_sits_on_the_capture() {
        let measured: Vec<f64> = (0..GRID_N)
            .map(|i| {
                let o = octaves_from_1k(grid_hz(i)) - 1.0; // a bump at 2 kHz
                -40.0 + 8.0 * (-o * o * 8.0).exp()
            })
            .collect();
        let c = flat_ceiling(-40.0);
        let r = conform(&measured, &c, &ConformSettings::default());
        assert!(!r.bands.is_empty());
        assert!(r.rms_after < r.rms_before, "{r:?}");
        let a = auto_ceiling(&measured);
        assert_eq!(a.pts.len(), NUM_NODES);
        for p in &a.pts {
            assert!(p.db >= -96.0 && p.db <= 0.0);
        }
        let mut m = flat_ceiling(-40.0);
        move_node(&mut m, 3, -30.0, true);
        assert_eq!(m.pts[2].db, -40.0);
        assert_eq!(m.pts[15].db, -30.0);
    }

    /// A sung-vocal-like capture: a broad hump 200 Hz .. 4 kHz, bleed-level lows and hiss-level
    /// highs, and the notes: +-5 dB ripple every sixth of an octave (what a 1/3-octave capture of
    /// a short sung passage still shows).
    fn vocal_like() -> Vec<f64> {
        (0..GRID_N)
            .map(|i| {
                let f = grid_hz(i);
                let o = octaves_from_1k(f);
                let mut v = -40.0 - 6.0 * o * o;
                if f < 120.0 {
                    v -= 30.0 * (120.0 / f).log2().min(2.0);
                }
                if f > 8000.0 {
                    v -= 25.0 * (f / 8000.0).log2();
                }
                v + 5.0 * (2.0 * PI * 6.0 * o).sin()
            })
            .collect()
    }

    fn idx_of(hz: f64) -> usize {
        (0..GRID_N).min_by(|&a, &b| (grid_hz(a) / hz).log2().abs().total_cmp(&(grid_hz(b) / hz).log2().abs())).unwrap_or(0)
    }

    #[test]
    fn conform_is_a_smooth_curve_of_up_to_12_wide_bells() {
        let cap = vocal_like();
        // Auto Generate, then the owner's kind of move: everything from ~1 kHz up 8 dB.
        let mut c = auto_ceiling(&cap);
        let k = c.pts.iter().position(|p| p.hz >= 1000.0).unwrap_or(8);
        let db = c.pts[k].db + 8.0;
        move_node(&mut c, k, db, true);
        let r = conform(&cap, &c, &ConformSettings::default());
        assert!(!r.bands.is_empty() && r.bands.len() <= CONFORM_MAX_BELLS, "{} bands", r.bands.len());
        for b in &r.bands {
            assert_eq!(b.type_, TYPE_BELL);
            assert!(b.q <= 1.5 && b.q >= 0.3, "{b:?}");
        }
        // No stacked bands: every centre its own.
        for w in r.bands.windows(2) {
            assert!((w[1].hz / w[0].hz).log2() > 0.5, "{w:?}");
        }
        // Smooth: nowhere does the curve stand off its own 1/2-octave mean by a dB (the notes'
        // +-5 dB ripple is not chased).
        let half = 6;
        for i in half..GRID_N - half {
            let mean = r.curve[i - half..=i + half].iter().sum::<f64>() / (2 * half + 1) as f64;
            assert!((r.curve[i] - mean).abs() < 1.0, "at {:.0} Hz: {:.2} vs {:.2}", grid_hz(i), r.curve[i], mean);
        }
        // And it does the move: the top lifted ~8 dB against the middle of the voice.
        let lift = r.curve[idx_of(4000.0)] - r.curve[idx_of(400.0)];
        assert!(lift > 5.0 && lift < 10.0, "{lift}");
        // Untouched, Auto Generate's ceiling asks for (almost) nothing.
        let r0 = conform(&cap, &auto_ceiling(&cap), &ConformSettings::default());
        assert!(r0.curve_lo > -1.5 && r0.curve_hi < 1.5, "{} {}", r0.curve_lo, r0.curve_hi);
    }

    #[test]
    fn the_range_holds_the_summed_curve_and_the_status_knows_what_was_asked() {
        let cap = vocal_like();
        // The ceiling laid on a pink line: far from a voice.
        let lvl = fit_noise_level(&cap, -3.0, 40.0, 12000.0, -100.0);
        let mut pink = flat_ceiling(-30.0);
        for p in &mut pink.pts {
            p.db = noise_line_db(-3.0, lvl, p.hz).clamp(-96.0, 0.0);
        }
        for max_db in [3.0, 6.0, 12.0] {
            let s = ConformSettings { max_db, ..ConformSettings::default() };
            let r = conform(&cap, &pink, &s);
            let peak = r.curve.iter().fold(0.0f64, |m, v| m.max(v.abs()));
            assert!(peak <= max_db + 1e-3, "+-{max_db}: the summed curve reaches {peak}");
            assert!(r.curve_lo >= -max_db - 1e-3 && r.curve_hi <= max_db + 1e-3);
            assert!(r.clamped(max_db), "+-{max_db}: asked {} .. {}", r.asked_lo, r.asked_hi);
            assert!(r.asked_hi - r.asked_lo > 2.0 * max_db);
        }
        // A small move is not past the range.
        let mut c = auto_ceiling(&cap);
        let to = c.pts[10].db + 3.0;
        move_node(&mut c, 10, to, true);
        assert!(!conform(&cap, &c, &ConformSettings::default()).clamped(12.0));
    }

    #[test]
    fn flat_outside_the_span_and_no_boost_for_bleed() {
        // A capture with nothing under 80 Hz but rumble 45 dB down, and only hiss above 16 kHz;
        // the ceiling's nodes under 60 Hz ask for +20 .. +30 dB of the rumble.
        let cap: Vec<f64> = (0..GRID_N)
            .map(|i| {
                let f = grid_hz(i);
                if f < 80.0 {
                    -95.0
                } else if f > 16000.0 {
                    -100.5
                } else {
                    -45.0 - 3.0 * octaves_from_1k(f)
                }
            })
            .collect();
        let mut c = Ceiling::default();
        for k in 0..NUM_NODES {
            let hz = GRID_LO * (GRID_HI / GRID_LO).powf(k as f64 / (NUM_NODES - 1) as f64);
            let on = -45.0 - 3.0 * octaves_from_1k(hz);
            let db = if hz < 60.0 { on + 20.0 + 10.0 * (k % 2) as f64 } else { on };
            c.pts.push(Point { hz, db: db.clamp(-96.0, 0.0) });
        }
        let r = conform(&cap, &c, &ConformSettings::default());
        // Nothing is boosted for the rumble and the hiss: there was nothing to correct.
        let peak = r.curve.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        assert!(peak < 1.0, "the curve reaches {peak}");

        // A real correction at the edges is held flat past them, not chased.
        let mut tilt = c.clone();
        for p in &mut tilt.pts {
            p.db = (-45.0 - 3.0 * octaves_from_1k(p.hz) + 2.0 * octaves_from_1k(p.hz)).clamp(-96.0, 0.0);
        }
        let cap2: Vec<f64> = (0..GRID_N).map(|i| -45.0 - 3.0 * octaves_from_1k(grid_hz(i))).collect();
        let r = conform(&cap2, &tilt, &ConformSettings::default());
        let at = |hz: f64| r.curve[idx_of(hz)];
        let (lo_edge, hi_edge) = (at(30.0), at(16000.0));
        assert!(lo_edge < -5.0 && hi_edge > 5.0, "{lo_edge} {hi_edge}");
        for i in 0..GRID_N {
            let f = grid_hz(i);
            if f < 30.0 {
                assert!((r.curve[i] - lo_edge).abs() < 1.5, "{f:.0} Hz: {} vs the edge {lo_edge}", r.curve[i]);
            }
            if f > 16000.0 {
                // the top bell's skirt: back toward 0, never past the edge's correction
                assert!(r.curve[i] > -0.5 && r.curve[i] < hi_edge + 1.0, "{f:.0} Hz: {} vs the edge {hi_edge}", r.curve[i]);
            }
        }
        let first = r.weight.iter().position(|w| *w >= 1.0).unwrap_or(0);
        assert!(grid_hz(first) >= CONFORM_LO_HZ && grid_hz(first.saturating_sub(1)) < CONFORM_LO_HZ);
        for i in 0..first {
            assert_eq!(r.target[i], r.target[first], "held flat");
            assert_eq!(r.weight[i], CONFORM_HOLD_WEIGHT);
        }
    }

    #[test]
    fn amount_and_cuts_only() {
        let cap = vocal_like();
        let mut c = auto_ceiling(&cap);
        let k = c.pts.iter().position(|p| p.hz >= 300.0).unwrap_or(5);
        let to = c.pts[k].db - 5.0;
        move_node(&mut c, k, to, false);
        let k2 = c.pts.iter().position(|p| p.hz >= 3000.0).unwrap_or(11);
        let to = c.pts[k2].db + 5.0;
        move_node(&mut c, k2, to, false);
        let full = conform(&cap, &c, &ConformSettings::default());
        let half = conform(&cap, &c, &ConformSettings { amount: 0.5, ..ConformSettings::default() });
        for i in (0..GRID_N).step_by(8) {
            assert!((half.curve[i] - 0.5 * full.curve[i]).abs() < 0.6, "{:.0} Hz: {} vs {}", grid_hz(i), half.curve[i], full.curve[i]);
        }
        let cuts = conform(&cap, &c, &ConformSettings { cuts_only: true, ..ConformSettings::default() });
        assert!(!cuts.bands.is_empty());
        assert!(cuts.bands.iter().all(|b| b.db <= 0.0), "{:?}", cuts.bands);
        assert!(cuts.curve.iter().all(|v| *v <= 1e-9) && cuts.asked_hi == 0.0);
        // A budget of 3: three wider bells.
        let three = conform(&cap, &c, &ConformSettings { max_bands: 3, ..ConformSettings::default() });
        assert!(three.bands.len() <= 3 && three.bands.iter().all(|b| b.q < CONFORM_Q));
        // Hostile settings never panic.
        for s in [
            ConformSettings { max_db: f64::NAN, amount: f64::NAN, ..ConformSettings::default() },
            ConformSettings { max_db: -5.0, max_bands: -3, sample_rate: 0.0, ..ConformSettings::default() },
        ] {
            let r = conform(&cap, &c, &s);
            assert!(r.curve.iter().all(|v| v.is_finite()) && r.makeup_db.is_finite());
        }
    }

    /// Through the real plugin: Conform's bands with its make-up gain on the output leave the
    /// signal it was captured from as loud as it was (to a dB), where the EQ alone does not.
    #[test]
    fn make_up_gain_keeps_the_level_through_the_real_plugin() {
        use super::super::clean_q_capture::CeilingCapture;
        use super::super::clean_q_dsp::FFT_SIZE;
        let sr = 48_000.0;
        // A voice-ish test signal: noise through a resonance near 700 Hz, plus a little hiss.
        let n = 48_000 * 4;
        let mut seed: u32 = 99;
        let (mut y1, mut y2, mut lp) = (0.0f32, 0.0f32, 0.0f32);
        let sig: Vec<f32> = (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let w = (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
                lp += 0.2 * (w - lp);
                let y = 1.98 * y1 - 0.9817 * y2 + 0.02 * lp;
                y2 = y1;
                y1 = y;
                0.4 * y + 0.01 * w
            })
            .collect();
        let mut cap = CeilingCapture::new();
        let mut t = FFT_SIZE;
        while t <= n {
            cap.feed(&sig[t - FFT_SIZE..t], t as f64 / sr, 0.0);
            t += 1600;
        }
        let grid = cap.grid(sr, 3);
        assert_eq!(grid.len(), GRID_N);
        // The ceiling laid on a pink line: a big tilt, the level well down through the EQ.
        let lvl = fit_noise_level(&grid, -3.0, 40.0, 12000.0, -100.0);
        let mut pink = flat_ceiling(-30.0);
        for p in &mut pink.pts {
            p.db = noise_line_db(-3.0, lvl, p.hz).clamp(-96.0, 0.0);
        }
        let mut lifted = auto_ceiling(&grid);
        let k = lifted.pts.iter().position(|p| p.hz >= 1500.0).unwrap_or(10);
        let to = lifted.pts[k].db + 9.0;
        move_node(&mut lifted, k, to, true);
        for c in [&pink, &lifted] {
            let r = conform(&grid, c, &ConformSettings { sample_rate: sr, ..ConformSettings::default() });
            assert!(!r.bands.is_empty());
            let render = |out_db: f64| -> f64 {
                let Some(mut p) = crate::create("fs_cleanq") else { return 0.0 };
                p.prepare(sr as f32, 512, 1);
                p.set_param("outGain", dsp::out_db_to_knob(out_db).clamp(0.0, 1.0));
                for (slot, b) in r.bands.iter().enumerate() {
                    let tag = format!("b{:02}", slot + 1);
                    p.set_param(&format!("{tag}_freq"), dsp::freq_to_knob(b.hz).clamp(0.0, 1.0));
                    p.set_param(&format!("{tag}_gain"), dsp::gain_to_knob(b.db).clamp(0.0, 1.0));
                    p.set_param(&format!("{tag}_q"), dsp::q_to_knob(b.q).clamp(0.0, 1.0));
                    p.set_param(&format!("{tag}_type"), b.type_ as f32);
                    p.set_param(&format!("{tag}_place"), dsp::PLACE_STEREO as f32);
                    p.set_param(&format!("{tag}_on"), dsp::ON_ACTIVE as f32);
                }
                let mut z = vec![vec![0.0f32; 512]];
                for _ in 0..100 {
                    p.process(&mut z, 512);
                }
                let (mut e_in, mut e_out) = (0.0f64, 0.0f64);
                for (at, ch) in sig.chunks(512).enumerate() {
                    let mut io = vec![ch.to_vec()];
                    p.process(&mut io, ch.len());
                    if at >= 20 {
                        e_in += ch.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
                        e_out += io[0].iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
                    }
                }
                10.0 * (e_out / e_in).log10()
            };
            let eq_alone = render(0.0);
            let with = render(r.makeup_db);
            assert!(with.abs() < 1.0, "with make-up {with:+.2} dB (EQ alone {eq_alone:+.2}, make-up {:+.2})", r.makeup_db);
            assert!((eq_alone + r.makeup_db).abs() < 1.0);
        }
    }

    #[test]
    fn the_make_up_is_held_by_the_learned_peak() {
        // the rule itself
        assert_eq!(cap_makeup_db(8.0, -20.0), 8.0, "plenty of headroom: untouched");
        assert_eq!(cap_makeup_db(8.0, -6.0), 5.0, "held to -1 dBFS");
        assert_eq!(cap_makeup_db(8.0, -0.3), 0.0, "already hot: none");
        assert_eq!(cap_makeup_db(-4.0, -0.3), -4.0, "a cut passes");
        assert_eq!(cap_makeup_db(8.0, f64::NEG_INFINITY), 8.0, "unknown peak: no cap");
        assert_eq!(cap_makeup_db(8.0, f64::NAN), 8.0);
        assert_eq!(apply_makeup(3.0, 0.0, 5.0), (8.0, 5.0), "the user's +3 trim kept");
        assert_eq!(apply_makeup(8.0, 5.0, 2.0), (5.0, 2.0), "the last make-up taken back");
        assert_eq!(apply_makeup(20.0, 0.0, 9.0), (24.0, 4.0), "held to the knob");
        assert_eq!(apply_makeup(8.0, 5.0, 0.0), (3.0, 0.0), "Remove");

        // Through the real plugin: material learned at three levels, Conform onto ceilings that
        // take loudness off (so a big make-up is asked for). Hot material is not pushed past
        // -1 dBFS and says so; quiet material gets its level back in full.
        use super::super::clean_q_capture::CeilingCapture;
        let sr = 48_000.0;
        let n = 48_000 * 3;
        let mut seed: u32 = 4242;
        let (mut y1, mut y2, mut lp) = (0.0f32, 0.0f32, 0.0f32);
        let base: Vec<f32> = (0..n)
            .map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let w = (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
                lp += 0.2 * (w - lp);
                let y = 1.98 * y1 - 0.9817 * y2 + 0.02 * lp;
                y2 = y1;
                y1 = y;
                // a voice-ish resonance, a decaying low thump every half second, a little hiss
                let t = (i % 24_000) as f32 / 48_000.0;
                0.4 * y + 0.3 * (-t * 20.0).exp() * (std::f32::consts::TAU * 70.0 * t).sin() + 0.01 * w
            })
            .collect();
        let base_peak = base.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let mut capped_any = false;
        for (peak_dbfs, hot) in [(-0.3f32, true), (-4.0, true), (-24.0, false)] {
            let g = 10f32.powf(peak_dbfs / 20.0) / base_peak;
            let x: Vec<f32> = base.iter().map(|v| v * g).collect();
            let mut cap = CeilingCapture::new();
            let mut t = super::super::clean_q_dsp::FFT_SIZE;
            while t <= n {
                cap.feed(&x[t - super::super::clean_q_dsp::FFT_SIZE..t], t as f64 / sr, 0.0);
                t += 1600;
            }
            let input_peak_db = cap.averager().peak_db();
            assert!((input_peak_db - f64::from(peak_dbfs)).abs() < 0.05, "learned peak {input_peak_db}");
            let grid = cap.grid(sr, 3);
            let lvl = fit_noise_level(&grid, -3.0, 40.0, 12000.0, -100.0);
            let mut pink = flat_ceiling(-30.0);
            for p in &mut pink.pts {
                p.db = noise_line_db(-3.0, lvl, p.hz).clamp(-96.0, 0.0);
            }
            let mut mid_cut = auto_ceiling(&grid);
            for p in mid_cut.pts.iter_mut().filter(|p| p.hz > 300.0 && p.hz < 3000.0) {
                p.db -= 18.0;
            }
            for c in [&pink, &mid_cut] {
                let s = ConformSettings { sample_rate: sr, input_peak_db, ..ConformSettings::default() };
                let r = conform(&grid, c, &s);
                assert!(r.makeup_wanted_db > 1.0, "the test asks for a real make-up: {}", r.makeup_wanted_db);
                let room = (-CONFORM_PEAK_HEADROOM_DB - input_peak_db).max(0.0);
                assert_eq!(r.makeup_capped, r.makeup_wanted_db > room, "{peak_dbfs} dBFS: {} of {}", r.makeup_db, r.makeup_wanted_db);
                assert_eq!(r.makeup_db, r.makeup_wanted_db.min(room));
                if hot {
                    capped_any |= r.makeup_capped;
                }
                let Some(mut p) = crate::create("fs_cleanq") else { panic!("fs_cleanq") };
                p.prepare(sr as f32, 512, 1);
                p.set_param("outGain", dsp::out_db_to_knob(r.makeup_db).clamp(0.0, 1.0));
                for (slot, b) in r.bands.iter().enumerate() {
                    let tag = format!("b{:02}", slot + 1);
                    p.set_param(&format!("{tag}_freq"), dsp::freq_to_knob(b.hz).clamp(0.0, 1.0));
                    p.set_param(&format!("{tag}_gain"), dsp::gain_to_knob(b.db).clamp(0.0, 1.0));
                    p.set_param(&format!("{tag}_q"), dsp::q_to_knob(b.q).clamp(0.0, 1.0));
                    p.set_param(&format!("{tag}_type"), b.type_ as f32);
                    p.set_param(&format!("{tag}_place"), dsp::PLACE_STEREO as f32);
                    p.set_param(&format!("{tag}_on"), dsp::ON_ACTIVE as f32);
                }
                let mut z = vec![vec![0.0f32; 512]];
                for _ in 0..100 {
                    p.process(&mut z, 512);
                }
                let (mut e_in, mut e_out, mut pk) = (0.0f64, 0.0f64, 0.0f32);
                for (at, ch) in x.chunks(512).enumerate() {
                    let mut io = vec![ch.to_vec()];
                    p.process(&mut io, ch.len());
                    if at >= 20 {
                        e_in += ch.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
                        e_out += io[0].iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
                        pk = io[0].iter().fold(pk, |m, v| m.max(v.abs()));
                    }
                }
                let peak_out = 20.0 * f64::from(pk).log10();
                let level = 10.0 * (e_out / e_in).log10();
                if hot {
                    // never past -1 dBFS, or past the material's own peak when it was hotter
                    let line = (-CONFORM_PEAK_HEADROOM_DB).max(input_peak_db);
                    assert!(peak_out <= line + 0.05, "{peak_dbfs} dBFS in: {peak_out:.2} dBFS out");
                } else {
                    assert!(level.abs() < 1.0, "quiet material keeps its level: {level:+.2} dB");
                    assert!(peak_out < -1.0);
                    assert!(!r.makeup_capped);
                }
            }
        }
        assert!(capped_any, "the hot cases needed the cap");
    }

    /// The radix-2 FFT of the C++ reference harness, operation for operation.
    fn ref_fft(re: &mut [f64], im: &mut [f64]) {
        let n = re.len();
        let mut j = 0usize;
        for i in 1..n {
            let mut bit = n >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j ^= bit;
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let ang = -2.0 * PI / len as f64;
            let (wr, wi) = (ang.cos(), ang.sin());
            for i in (0..n).step_by(len) {
                let (mut cr, mut ci) = (1.0f64, 0.0f64);
                for k in 0..len / 2 {
                    let (ur, ui) = (re[i + k], im[i + k]);
                    let (xr, xi) = (re[i + k + len / 2], im[i + k + len / 2]);
                    let vr = xr * cr - xi * ci;
                    let vi = xr * ci + xi * cr;
                    re[i + k] = ur + vr;
                    im[i + k] = ui + vi;
                    re[i + k + len / 2] = ur - vr;
                    im[i + k + len / 2] = ui - vi;
                    let nr = cr * wr - ci * wi;
                    let ni = cr * wi + ci * wr;
                    cr = nr;
                    ci = ni;
                }
            }
            len <<= 1;
        }
    }

    /// Learn from a fixed input (`learned_ceiling_ref.cpp`'s), through the translated averager,
    /// `smooth_to_grid` and `auto_ceiling`, then Conform onto the learned ceiling with one node
    /// pulled down: everything matches what the C++ maths printed, on the curve, bit for bit.
    #[test]
    fn a_learned_ceiling_matches_the_cpp_maths() {
        use super::super::clean_q_dsp::{FFT_MAGNITUDE_SCALE, FFT_SIZE, NUM_BINS, fill_window};
        let reference = include_str!("../../../../tests/fidelity/cleanq/learned_ceiling.ref.txt");
        let mut want: std::collections::BTreeMap<&str, Vec<Vec<f64>>> = std::collections::BTreeMap::new();
        for line in reference.lines() {
            let mut it = line.split_whitespace();
            let Some(tag) = it.next() else { continue };
            want.entry(tag).or_default().push(it.map(|v| v.parse::<f64>().unwrap()).collect());
        }
        let col = |tag: &str| -> Vec<f64> { want.get(tag).map(|v| v.iter().map(|r| r[0]).collect()).unwrap_or_default() };

        // The fixed input, in float exactly as the harness makes it.
        let total = 48_000 * 2;
        let mut sig = vec![0.0f32; total];
        let mut seed: u32 = 12345;
        let (mut lp, mut lp2, mut y1, mut y2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for v in sig.iter_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let w = (seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
            lp += 0.05 * (w - lp);
            lp2 += 0.3 * (w - lp2);
            let y = 1.964_574_5f32 * y1 - 0.9801 * y2 + 0.02 * w;
            y2 = y1;
            y1 = y;
            *v = 0.6 * lp + 0.1 * w + 0.2 * lp2 + 0.5 * y;
        }

        let mut window = vec![0.0f32; FFT_SIZE];
        fill_window(&mut window);
        let mut avg = SpectrumAverager::default();
        let mut mag = vec![0.0f32; NUM_BINS];
        let mut frames = 0;
        let mut start = 0;
        while start + FFT_SIZE <= total {
            let mut re: Vec<f64> = (0..FFT_SIZE).map(|k| f64::from(sig[start + k] * window[k])).collect();
            let mut im = vec![0.0f64; FFT_SIZE];
            ref_fft(&mut re, &mut im);
            for (k, m) in mag.iter_mut().enumerate() {
                *m = ((re[k] * re[k] + im[k] * im[k]).sqrt() * f64::from(FFT_MAGNITUDE_SCALE)) as f32;
            }
            avg.add(&mag, 1.0 / 30.0, 0.0);
            frames += 1;
            start += FFT_SIZE / 2;
        }
        assert_eq!(f64::from(frames), want["frames"][0][0]);

        let grid = smooth_to_grid(avg.bins(), 48_000.0, FFT_SIZE, 3.0);
        assert_eq!(grid, col("grid"), "the averaged, smoothed capture");
        let c = auto_ceiling(&grid);
        let nodes: Vec<Vec<f64>> = c.pts.iter().map(|p| vec![p.hz, p.db]).collect();
        assert_eq!(nodes, want["node"], "the learned nodes");
        let curve: Vec<f64> = (0..GRID_N).map(|i| c.at(grid_hz(i))).collect();
        assert_eq!(curve, col("curve"), "the learned ceiling on the curve, sample for sample");
        assert_eq!(c.to_grid(), curve, "and what the dynamic ceiling is handed");

        let mut d = c.clone();
        let db = d.pts[9].db - 6.0;
        move_node(&mut d, 9, db, false);
        // Conform three ways: onto the learned ceiling with one node down (the defaults), and onto
        // a pink line through the capture at 70 %, 8 bells, +/-6 dB, then cuts only. The target
        // is exact; the bells' gains iterate (Gauss-Newton over sums of 256 responses), so they
        // and everything after them match to 1e-9.
        let near = |a: f64, b: f64| (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0);
        let same = |got: &[f64], want: &[f64], what: &str| {
            assert_eq!(got.len(), want.len(), "{what}");
            for (a, b) in got.iter().zip(want) {
                assert!(near(*a, *b), "{what}: {a} vs {b}");
            }
        };
        let band_rows = |r: &ConformResult| -> Vec<Vec<f64>> { r.bands.iter().map(|b| vec![f64::from(b.type_), b.hz, b.db, b.q]).collect() };
        let r = conform(&grid, &d, &ConformSettings::default());
        assert_eq!(r.target, col("target"), "Conform's correction");
        assert_eq!(r.weight, col("weight"), "and its weights");
        let bands = band_rows(&r);
        assert_eq!(bands.len(), want["band"].len(), "Conform's bands");
        for (got, want) in bands.iter().zip(&want["band"]) {
            same(got, want, "band");
        }
        same(&[r.rms_before, r.rms_after, r.worst_after], &want["rms"][0], "rms");
        same(&[r.asked_lo, r.asked_hi, r.curve_lo, r.curve_hi, r.makeup_db], &want["conform"][0], "asked, curve, make-up");

        let mut pink = flat_ceiling(-30.0);
        let lvl = fit_noise_level(&grid, -3.0, 40.0, 12000.0, -100.0);
        for p in &mut pink.pts {
            p.db = noise_line_db(-3.0, lvl, p.hz).clamp(-96.0, 0.0);
        }
        for (cuts, tag) in [(false, "pink"), (true, "pcut")] {
            let input_peak_db = if cuts { -2.5 } else { -40.0 };
            let ps = ConformSettings { amount: 0.7, max_bands: 8, max_db: 6.0, cuts_only: cuts, input_peak_db, ..ConformSettings::default() };
            let r = conform(&grid, &pink, &ps);
            assert_eq!(r.makeup_capped, cuts, "{tag}: the hot one is held, the quiet one is not");
            assert_eq!(r.target, col(&format!("{tag}_target")), "{tag}: the correction");
            let rows = band_rows(&r);
            let want_rows = &want[format!("{tag}_band").as_str()];
            assert_eq!(rows.len(), want_rows.len(), "{tag}: bands");
            for (got, want) in rows.iter().zip(want_rows) {
                same(got, want, tag);
            }
            same(
                &[
                    r.rms_before,
                    r.rms_after,
                    r.worst_after,
                    r.asked_lo,
                    r.asked_hi,
                    r.curve_lo,
                    r.curve_hi,
                    r.makeup_db,
                    r.makeup_wanted_db,
                    f64::from(u8::from(r.makeup_capped)),
                ],
                &want[format!("{tag}_conform").as_str()][0],
                tag,
            );
        }

        // The cutting: the dynamic ceiling on the same input (the right channel half a second
        // on) against the learned ceiling 6 dB lower, 512-sample blocks, default settings on.
        // Every output sample is bit-identical to the C++ (FNV-1a over the float bits).
        use super::super::clean_q_dyn_ceiling::{DynCeiling, Settings};
        let mut dyn_ceil = DynCeiling::new();
        dyn_ceil.prepare(48_000.0);
        let lowered: Vec<f64> = curve.iter().map(|v| v - 6.0).collect();
        dyn_ceil.set_ceiling(&lowered);
        dyn_ceil.set_settings(&Settings { on: true, ..Settings::default() });
        let mut l = sig.clone();
        let mut r: Vec<f32> = (0..total).map(|n| sig[(n + 24_000) % total]).collect();
        let mut at = 0;
        while at < total {
            let n = 512.min(total - at);
            dyn_ceil.process(&mut l[at..at + n], Some(&mut r[at..at + n]), n);
            at += n;
        }
        let mut h: u64 = 1_469_598_103_934_665_603;
        for n in 0..total {
            for v in [l[n], r[n]] {
                for byte in v.to_bits().to_le_bytes() {
                    h ^= u64::from(byte);
                    h = h.wrapping_mul(1_099_511_628_211);
                }
            }
        }
        assert_eq!(h.to_string(), reference.lines().find_map(|x| x.strip_prefix("hash ")).unwrap_or(""), "the cut audio, bit for bit");
        for row in &want["out"] {
            let n = row[0] as usize;
            // printed with 9 significant digits: exactly the float again
            assert_eq!((l[n], r[n]), (row[1] as f32, row[2] as f32), "sample {n}");
        }
        let cut: Vec<f32> = (0..GRID_N).map(|i| dyn_ceil.cut_at(i)).collect();
        assert_eq!(cut, col("cut").iter().map(|v| *v as f32).collect::<Vec<_>>());
        // And it did cut: the output is not the input delayed by the frame.
        let moved = (2048..total).filter(|&n| (l[n] - sig[n - 2048]).abs() > 1e-3).count();
        assert!(moved > total / 20, "{moved}");
    }
}
