//! Scout metrics: a faithful, line-by-line translation of Scout2's capture engine
//! (`C:\projects\A18\FireStation\Scout2\Source\ScoutDSP.h`).
//!
//! A capture produces one [`Metrics`] record: level and loudness (BS.1770 gated LUFS, LRA,
//! 4x Hermite true peak, crest, dynamic range), a 31-band IIR 1/3-octave spectrum (6th-order
//! Butterworth bands since 2026-10-21, [`BANDS_VERSION`] 2) with seven
//! summary zones, stereo behaviour (correlation, width, mid/side, low-frequency correlation)
//! and activity statistics (transients, noise floor, clipping).
//!
//! Same constants, order of operations and float/double types as the C++. The C++ per-sample
//! `accumulate` is [`ScoutMeter::feed`] (audio-thread safe: no allocation, no panic) and its
//! `finish` is [`ScoutMeter::finish`] (off the audio thread). Dropped from the C++: the live
//! ~10 Hz meter window and the atomics (the mix engine owns live progress), the duration index
//! and the self-ending target (the caller decides when a capture ends, at most
//! [`MAX_CAPTURE_SECONDS`]).
//!
//! Additions, never-crash guards only (no effect on any real audio): non-finite input samples
//! are read as 0.0, finite samples are clamped to +-[`HOSTILE_LIMIT`] (+120 dBFS) so the f32
//! accumulators cannot overflow, a non-finite or non-positive sample rate falls back to 48 kHz
//! (as the C++ does for <= 0), and `finish` replaces any non-finite field with its default.

#[cfg(test)]
mod tests;

/// A capture ends itself after this many seconds (owner decision; see the Scout design).
pub const MAX_CAPTURE_SECONDS: u32 = 30;

/// The band design `spectrum31` was measured with: 1 = the original single 2nd-order RBJ
/// band-pass per band (Q 4.32, ~6 dB/oct skirts), 2 = 6th-order Butterworth 1/3-octave bands
/// (2026-10-21). Readings of the two do not compare directly (see the Scout2 README).
pub const BANDS_VERSION: u32 = 2;

/// `ScoutDSP::numBands`: 1/3-octave, 20 Hz .. 20 kHz.
pub const NUM_BANDS: usize = 31;
/// `ScoutDSP::maxBlocks`: 400 ms K-weighted blocks -> 102 s of capture.
const MAX_BLOCKS: usize = 256;
/// `ScoutDSP::histBins`: 1 dB bins for the window-level histogram.
const HIST_BINS: usize = 96;
/// `ScoutDSP::pi`.
const PI: f64 = std::f64::consts::PI;

/// Never-crash guard: finite input samples are clamped to this magnitude (+120 dBFS).
pub const HOSTILE_LIMIT: f32 = 1.0e6;

/// `ScoutDSP::isoCentre`: ISO 1/3-octave centres, 1 kHz is band 17; exact, not nominal.
pub fn iso_centre(i: usize) -> f32 {
    1000.0f32 * 2.0f32.powf((i as i32 - 17) as f32 / 3.0f32)
}

/// `ScoutDSP::Metrics`, field for field.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Metrics {
    pub valid: bool,
    pub duration_sec: f32,
    pub sample_rate: f32,
    pub activity_ratio: f32,
    pub clipped_samples: i32,
    pub peak_hold_dbfs: f32,
    pub noise_floor_dbfs: f32,
    pub peak_dbfs: f32,
    pub true_peak_dbtp: f32,
    pub rms_dbfs: f32,
    pub lufs_integrated: f32,
    pub lra: f32,
    pub crest_factor_db: f32,
    pub dynamic_range_db: f32,
    pub peak_to_lufs_db: f32,
    pub transient_density: f32,
    pub spectrum31: [f32; NUM_BANDS],
    pub zone_sub: f32,
    pub zone_low: f32,
    pub zone_low_mid: f32,
    pub zone_mid: f32,
    pub zone_high_mid: f32,
    pub zone_high: f32,
    pub zone_air: f32,
    pub stereo_correlation: f32,
    pub stereo_width: f32,
    pub low_freq_correlation: f32,
    pub lr_balance_db: f32,
    pub dc_offset: f32,
    pub spectral_centroid_hz: f32,
    pub mid_side_ratio_db: f32,
    pub side_low_ratio_db: f32,
    pub noise_floor_p10_dbfs: f32,
    pub lufs_p10: f32,
    pub lufs_p50: f32,
    pub lufs_p90: f32,
    pub lufs_momentary_max: f32,
    pub lufs_short_term_max: f32,
}

/// The C++ member initialisers.
impl Default for Metrics {
    fn default() -> Self {
        Self {
            valid: false,
            duration_sec: 0.0,
            sample_rate: 0.0,
            activity_ratio: 0.0,
            clipped_samples: 0,
            peak_hold_dbfs: -100.0,
            noise_floor_dbfs: -100.0,
            peak_dbfs: -100.0,
            true_peak_dbtp: -100.0,
            rms_dbfs: -100.0,
            lufs_integrated: -100.0,
            lra: 0.0,
            crest_factor_db: 0.0,
            dynamic_range_db: 0.0,
            peak_to_lufs_db: 0.0,
            transient_density: 0.0,
            spectrum31: [0.0; NUM_BANDS],
            zone_sub: 0.0,
            zone_low: 0.0,
            zone_low_mid: 0.0,
            zone_mid: 0.0,
            zone_high_mid: 0.0,
            zone_high: 0.0,
            zone_air: 0.0,
            stereo_correlation: 1.0,
            stereo_width: 0.0,
            low_freq_correlation: 1.0,
            lr_balance_db: 0.0,
            dc_offset: 0.0,
            spectral_centroid_hz: 0.0,
            mid_side_ratio_db: -100.0,
            side_low_ratio_db: -100.0,
            noise_floor_p10_dbfs: -100.0,
            lufs_p10: -100.0,
            lufs_p50: -100.0,
            lufs_p90: -100.0,
            lufs_momentary_max: -100.0,
            lufs_short_term_max: -100.0,
        }
    }
}

impl Metrics {
    /// Never-crash guard (not in the C++): any non-finite field takes its default value.
    fn sanitize(&mut self) {
        let d = Metrics::default();
        let fix = |v: &mut f32, dv: f32| {
            if !v.is_finite() {
                *v = dv;
            }
        };
        fix(&mut self.duration_sec, d.duration_sec);
        fix(&mut self.sample_rate, d.sample_rate);
        fix(&mut self.activity_ratio, d.activity_ratio);
        fix(&mut self.peak_hold_dbfs, d.peak_hold_dbfs);
        fix(&mut self.noise_floor_dbfs, d.noise_floor_dbfs);
        fix(&mut self.peak_dbfs, d.peak_dbfs);
        fix(&mut self.true_peak_dbtp, d.true_peak_dbtp);
        fix(&mut self.rms_dbfs, d.rms_dbfs);
        fix(&mut self.lufs_integrated, d.lufs_integrated);
        fix(&mut self.lra, d.lra);
        fix(&mut self.crest_factor_db, d.crest_factor_db);
        fix(&mut self.dynamic_range_db, d.dynamic_range_db);
        fix(&mut self.peak_to_lufs_db, d.peak_to_lufs_db);
        fix(&mut self.transient_density, d.transient_density);
        for v in &mut self.spectrum31 {
            // log10 of the 1e-10 floor.
            fix(v, -100.0);
        }
        fix(&mut self.zone_sub, d.zone_sub);
        fix(&mut self.zone_low, d.zone_low);
        fix(&mut self.zone_low_mid, d.zone_low_mid);
        fix(&mut self.zone_mid, d.zone_mid);
        fix(&mut self.zone_high_mid, d.zone_high_mid);
        fix(&mut self.zone_high, d.zone_high);
        fix(&mut self.zone_air, d.zone_air);
        fix(&mut self.stereo_correlation, d.stereo_correlation);
        fix(&mut self.stereo_width, d.stereo_width);
        fix(&mut self.low_freq_correlation, d.low_freq_correlation);
        fix(&mut self.lr_balance_db, d.lr_balance_db);
        fix(&mut self.dc_offset, d.dc_offset);
        fix(&mut self.spectral_centroid_hz, d.spectral_centroid_hz);
        fix(&mut self.mid_side_ratio_db, d.mid_side_ratio_db);
        fix(&mut self.side_low_ratio_db, d.side_low_ratio_db);
        fix(&mut self.noise_floor_p10_dbfs, d.noise_floor_p10_dbfs);
        fix(&mut self.lufs_p10, d.lufs_p10);
        fix(&mut self.lufs_p50, d.lufs_p50);
        fix(&mut self.lufs_p90, d.lufs_p90);
        fix(&mut self.lufs_momentary_max, d.lufs_momentary_max);
        fix(&mut self.lufs_short_term_max, d.lufs_short_term_max);
    }
}

// ------------------------------------------------------------------ biquads

/// `ScoutDSP::Biquad`: transposed direct form II, all f32.
#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

/// `ScoutDSP::bq`.
fn bq(q: &mut Biquad, x: f32) -> f32 {
    let y = q.b0 * x + q.z1;
    q.z1 = q.b1 * x - q.a1 * y + q.z2;
    q.z2 = q.b2 * x - q.a2 * y;
    y
}

/// `ScoutDSP::bqReset`.
fn bq_reset(q: &mut Biquad) {
    q.z1 = 0.0;
    q.z2 = 0.0;
}

/// `(float) (2.0 * pi * (double) f / sr)`.
fn omega(sr: f64, f: f32) -> f32 {
    (2.0 * PI * f as f64 / sr) as f32
}

/// `ScoutDSP::makeHighShelf`.
fn make_high_shelf(sr: f64, f: f32, q: f32, gain_db: f32) -> Biquad {
    let a = 10.0f32.powf(gain_db / 40.0);
    let w = omega(sr, f);
    let (cw, sw) = (w.cos(), w.sin());
    let alpha = sw / (2.0 * q);
    let sq_a2a = 2.0 * a.sqrt() * alpha;
    let a0 = (a + 1.0) - (a - 1.0) * cw + sq_a2a;
    Biquad {
        b0: a * ((a + 1.0) + (a - 1.0) * cw + sq_a2a) / a0,
        b1: -2.0 * a * ((a - 1.0) + (a + 1.0) * cw) / a0,
        b2: a * ((a + 1.0) + (a - 1.0) * cw - sq_a2a) / a0,
        a1: 2.0 * ((a - 1.0) - (a + 1.0) * cw) / a0,
        a2: ((a + 1.0) - (a - 1.0) * cw - sq_a2a) / a0,
        ..Biquad::default()
    }
}

/// `ScoutDSP::makeHighPass`.
fn make_high_pass(sr: f64, f: f32, q: f32) -> Biquad {
    let w = omega(sr, f);
    let cw = w.cos();
    let alpha = w.sin() / (2.0 * q);
    let a0 = 1.0 + alpha;
    Biquad {
        b0: ((1.0 + cw) * 0.5) / a0,
        b1: (-(1.0 + cw)) / a0,
        b2: ((1.0 + cw) * 0.5) / a0,
        a1: (-2.0 * cw) / a0,
        a2: (1.0 - alpha) / a0,
        ..Biquad::default()
    }
}

/// `ScoutDSP::makeLowPass`.
fn make_low_pass(sr: f64, f: f32, q: f32) -> Biquad {
    let w = omega(sr, f);
    let cw = w.cos();
    let alpha = w.sin() / (2.0 * q);
    let a0 = 1.0 + alpha;
    Biquad {
        b0: ((1.0 - cw) * 0.5) / a0,
        b1: (1.0 - cw) / a0,
        b2: ((1.0 - cw) * 0.5) / a0,
        a1: (-2.0 * cw) / a0,
        a2: (1.0 - alpha) / a0,
        ..Biquad::default()
    }
}

// ------------------------------------------------------------------ 1/3-octave bands
//
// `ScoutDSP`'s bands since 2026-10-21 ("v2 bands"): each band is a 6th-order Butterworth
// band-pass (ANSI S1.11 / IEC 61260 class-1 style), a 3rd-order Butterworth low-pass prototype,
// low-pass -> band-pass transformed onto the band edges fc * 2^(+-1/6), bilinear-transformed
// with both edges pre-warped, run as three 2nd-order sections. Flat 0 dB passband, -3 dB at the
// band edges. Everything in the bands is f64 (design, coefficients, state); only the band output
// is rounded to f32 before it is squared into `band_energy`, as in the C++.

/// `ScoutDSP::BandSection`: transposed direct form II with `b1 = 0`, all f64.
#[derive(Clone, Copy, Debug, Default)]
struct BandSection {
    b0: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

/// `ScoutDSP::BandFilter`: one band, three sections in series.
#[derive(Clone, Copy, Debug, Default)]
struct BandFilter {
    s: [BandSection; 3],
}

/// `ScoutDSP::bandSectionTick`.
fn band_section_tick(q: &mut BandSection, x: f64) -> f64 {
    let y = q.b0 * x + q.z1;
    q.z1 = q.z2 - q.a1 * y;
    q.z2 = q.b2 * x - q.a2 * y;
    y
}

/// `ScoutDSP::bandTick`.
fn band_tick(f: &mut BandFilter, x: f32) -> f32 {
    let mut y = x as f64;
    y = band_section_tick(&mut f.s[0], y);
    y = band_section_tick(&mut f.s[1], y);
    y = band_section_tick(&mut f.s[2], y);
    y as f32
}

/// `ScoutDSP::bandReset`.
fn band_reset(f: &mut BandFilter) {
    for q in &mut f.s {
        q.z1 = 0.0;
        q.z2 = 0.0;
    }
}

/// `std::max` semantics for f64 (`a < b ? b : a`).
fn cxx_max_f64(a: f64, b: f64) -> f64 {
    if a < b { b } else { a }
}

/// `ScoutDSP::bandSection`: the analog section `g s / (s^2 + a s + b)` (pre-warped, bilinear
/// constant 1), bilinear-transformed, `g` set for unity gain at the band centre `w0`. A section
/// that comes out non-finite (only for absurd sample rates) is silent.
fn band_section(a: f64, b: f64, w0: f64) -> BandSection {
    let d0 = 1.0 + a + b;
    let re = b - w0 * w0;
    let im = a * w0;
    let g = (re * re + im * im).sqrt() / w0;
    let q = BandSection { b0: g / d0, b2: -g / d0, a1: 2.0 * (b - 1.0) / d0, a2: (1.0 - a + b) / d0, z1: 0.0, z2: 0.0 };
    if q.b0.is_finite() && q.b2.is_finite() && q.a1.is_finite() && q.a2.is_finite() { q } else { BandSection::default() }
}

/// `ScoutDSP::makeThirdOctaveBand`: the band centred on `f` (already clamped to 0.45 * sr). The
/// edges are f / 2^(1/6) and f * 2^(1/6), the upper one held below 0.49 * sr (only the 20 kHz
/// band, only below ~46.2 kHz: narrower on its top side, still stable).
fn make_third_octave_band(sr: f64, f: f32) -> BandFilter {
    let r6 = 1.122_462_048_309_373f64; // 2^(1/6)
    let h3 = 0.866_025_403_784_438_6f64; // sqrt(3) / 2
    let fc = f as f64;
    let f1 = fc / r6;
    // `std::min (fc * r6, sr * 0.49)`
    let (hi, lim) = (fc * r6, sr * 0.49);
    let f2 = if lim < hi { lim } else { hi };
    let o1 = (PI * f1 / sr).tan(); // pre-warped edges
    let o2 = (PI * f2 / sr).tan();
    let bw = o2 - o1;
    let w0sq = o1 * o2;
    let w0 = w0sq.sqrt();

    // Prototype poles -1 and -1/2 +- j sqrt(3)/2. The real pole maps to s^2 + bw s + w0^2; the
    // complex pair to the roots r = (p bw +- sqrt((p bw)^2 - 4 w0^2)) / 2 and their conjugates.
    let pre = -0.5 * bw;
    let pim = h3 * bw;
    let dre = pre * pre - pim * pim - 4.0 * w0sq;
    let dim = 2.0 * pre * pim;
    let mag = (dre * dre + dim * dim).sqrt();
    let sre = cxx_max_f64((mag + dre) * 0.5, 0.0).sqrt();
    let mut sim = cxx_max_f64((mag - dre) * 0.5, 0.0).sqrt();
    if dim < 0.0 {
        sim = -sim;
    }

    let (ure, uim) = (pre + sre, pim + sim); // 2 * r+
    let (vre, vim) = (pre - sre, pim - sim); // 2 * r-

    BandFilter {
        s: [
            band_section(bw, w0sq, w0),
            band_section(-ure, (ure * ure + uim * uim) * 0.25, w0),
            band_section(-vre, (vre * vre + vim * vim) * 0.25, w0),
        ],
    }
}

// ------------------------------------------------------------------ helpers

/// `std::max` semantics (`a < b ? b : a`), kept so NaN handling matches the C++.
fn cxx_max_f32(a: f32, b: f32) -> f32 {
    if a < b { b } else { a }
}

/// `ScoutDSP::dbOf`.
fn db_of(lin: f32) -> f32 {
    20.0 * cxx_max_f32(lin, 1e-10).log10()
}

/// `ScoutDSP::msToLufs`.
fn ms_to_lufs(ms: f32) -> f32 {
    -0.691 + 10.0 * cxx_max_f32(ms, 1e-12).log10()
}

/// `ScoutDSP::hermite`: 4-point, 3rd-order Hermite between `x0` and `x1`.
fn hermite(xm1: f32, x0: f32, x1: f32, x2: f32, t: f32) -> f32 {
    let c = (x1 - xm1) * 0.5;
    let v = x0 - x1;
    let w = c + v;
    let a = w + v + (x2 - x0) * 0.5;
    let b = w + a;
    (((a * t) - b) * t + c) * t + x0
}

/// Never-crash guard on every input sample (not in the C++): non-finite reads as 0.0, finite
/// samples are clamped to +-`HOSTILE_LIMIT`. Real audio is never touched.
fn guard(x: f32) -> f32 {
    if x.is_finite() { x.clamp(-HOSTILE_LIMIT, HOSTILE_LIMIT) } else { 0.0 }
}

/// Percentile index as the C++ computes it: `(int) (p * (float) (n - 1) + 0.5f)`, kept in range.
fn pct_index(p: f32, n: usize) -> usize {
    ((p * (n - 1) as f32 + 0.5) as usize).min(n - 1)
}

// ------------------------------------------------------------------ the meter

/// The accumulators of one capture (`ScoutDSP`'s capture state). Everything is allocated in
/// [`ScoutMeter::new`]; [`start`](Self::start) and [`feed`](Self::feed) never allocate.
#[derive(Clone, Debug)]
pub struct ScoutMeter {
    sr: f64,
    collected: u64,

    bands: [BandFilter; NUM_BANDS],
    band_energy: [f32; NUM_BANDS],
    band_centre: [f32; NUM_BANDS],
    k_shelf_l: Biquad,
    k_shelf_r: Biquad,
    k_hp_l: Biquad,
    k_hp_r: Biquad,
    lp_l: Biquad,
    lp_r: Biquad,

    peak_level: f32,
    true_peak_lin: f32,
    rms_sum: f64,
    clipped: u64,
    active: u64,
    corr_sum: f64,
    l_energy: f64,
    r_energy: f64,
    dc_l: f64,
    dc_r: f64,
    lf_corr: f64,
    lf_l: f64,
    lf_r: f64,
    centroid_w: f64,
    centroid_p: f64,

    /// `blocks`: `MAX_BLOCKS` long, allocated once.
    blocks: Vec<f32>,
    n_blocks: usize,
    k_sum: f64,
    k_count: u64,
    k_block_size: u64,

    win_sum: f64,
    win_count: u64,
    win_size: u64,
    transients: u64,
    prev_win_rms: f32,
    min_win_rms: f32,
    max_win_rms: f32,

    mid_e: f64,
    side_e: f64,
    side_lf_e: f64,
    /// `winHist`: `HIST_BINS` long, allocated once.
    win_hist: Vec<u64>,
    win_hist_n: u64,

    lm3: f32,
    lm2: f32,
    lm1: f32,
    rm3: f32,
    rm2: f32,
    rm1: f32,
}

impl ScoutMeter {
    /// `ScoutDSP::prepare`: coefficients for `sample_rate` and cleared accumulators. Allocates.
    pub fn new(sample_rate: f64) -> ScoutMeter {
        // C++: `sampleRate > 0.0 ? sampleRate : 48000.0`; a non-finite rate also falls back
        // (never-crash guard).
        let sr = if sample_rate > 0.0 && sample_rate.is_finite() { sample_rate } else { 48000.0 };
        let k_shelf = make_high_shelf(sr, 1500.0, 0.707, 4.0);
        let k_hp = make_high_pass(sr, 38.0, 0.5);
        let lp = make_low_pass(sr, 150.0, 0.707);

        let mut band_centre = [0.0f32; NUM_BANDS];
        let mut bands = [BandFilter::default(); NUM_BANDS];
        for (i, (c, b)) in band_centre.iter_mut().zip(bands.iter_mut()).enumerate() {
            *c = iso_centre(i);
            // `std::min (bandCentre[i], (float) sr * 0.45f)`
            let lim = sr as f32 * 0.45;
            let fc = if lim < *c { lim } else { *c };
            *b = make_third_octave_band(sr, fc);
        }

        let mut m = ScoutMeter {
            sr,
            collected: 0,
            bands,
            band_energy: [0.0; NUM_BANDS],
            band_centre,
            k_shelf_l: k_shelf,
            k_shelf_r: k_shelf,
            k_hp_l: k_hp,
            k_hp_r: k_hp,
            lp_l: lp,
            lp_r: lp,
            peak_level: 0.0,
            true_peak_lin: 0.0,
            rms_sum: 0.0,
            clipped: 0,
            active: 0,
            corr_sum: 0.0,
            l_energy: 0.0,
            r_energy: 0.0,
            dc_l: 0.0,
            dc_r: 0.0,
            lf_corr: 0.0,
            lf_l: 0.0,
            lf_r: 0.0,
            centroid_w: 0.0,
            centroid_p: 0.0,
            blocks: vec![0.0; MAX_BLOCKS],
            n_blocks: 0,
            k_sum: 0.0,
            k_count: 0,
            // `(int) (sr * 0.4)` etc. (`as` truncates toward zero and saturates).
            k_block_size: (sr * 0.4) as u64,
            win_sum: 0.0,
            win_count: 0,
            win_size: (sr * 0.01) as u64,
            transients: 0,
            prev_win_rms: 0.0,
            min_win_rms: 1.0,
            max_win_rms: 0.0,
            mid_e: 0.0,
            side_e: 0.0,
            side_lf_e: 0.0,
            win_hist: vec![0; HIST_BINS],
            win_hist_n: 0,
            lm3: 0.0,
            lm2: 0.0,
            lm1: 0.0,
            rm3: 0.0,
            rm2: 0.0,
            rm1: 0.0,
        };
        m.start();
        m
    }

    /// `ScoutDSP::resetAccumulators` (what `startCapture` does). No allocation.
    pub fn start(&mut self) {
        self.peak_level = 0.0;
        self.true_peak_lin = 0.0;
        self.rms_sum = 0.0;
        self.clipped = 0;
        self.active = 0;
        self.corr_sum = 0.0;
        self.l_energy = 0.0;
        self.r_energy = 0.0;
        self.dc_l = 0.0;
        self.dc_r = 0.0;
        self.lf_corr = 0.0;
        self.lf_l = 0.0;
        self.lf_r = 0.0;
        self.centroid_w = 0.0;
        self.centroid_p = 0.0;
        self.n_blocks = 0;
        self.k_sum = 0.0;
        self.k_count = 0;
        self.win_sum = 0.0;
        self.win_count = 0;
        self.transients = 0;
        self.prev_win_rms = 0.0;
        self.min_win_rms = 1.0;
        self.max_win_rms = 0.0;
        self.lm3 = 0.0;
        self.lm2 = 0.0;
        self.lm1 = 0.0;
        self.rm3 = 0.0;
        self.rm2 = 0.0;
        self.rm1 = 0.0;
        self.collected = 0;
        self.mid_e = 0.0;
        self.side_e = 0.0;
        self.side_lf_e = 0.0;
        self.win_hist.iter_mut().for_each(|b| *b = 0);
        self.win_hist_n = 0;

        bq_reset(&mut self.k_shelf_l);
        bq_reset(&mut self.k_shelf_r);
        bq_reset(&mut self.k_hp_l);
        bq_reset(&mut self.k_hp_r);
        bq_reset(&mut self.lp_l);
        bq_reset(&mut self.lp_r);
        for (b, e) in self.bands.iter_mut().zip(self.band_energy.iter_mut()) {
            band_reset(b);
            *e = 0.0;
        }
    }

    /// Feeds one block (`ScoutDSP::process` while capturing: `accumulate` per sample). Mono
    /// callers pass the same slice twice (the C++ reads channel 0 as R when there is one
    /// channel). Only `min(l.len(), r.len())` frames are read. Audio-thread safe.
    pub fn feed(&mut self, l: &[f32], r: &[f32]) {
        for (&x_l, &x_r) in l.iter().zip(r) {
            self.accumulate(guard(x_l), guard(x_r));
            self.collected += 1;
        }
    }

    /// Samples fed since [`start`](Self::start).
    pub fn samples(&self) -> u64 {
        self.collected
    }

    /// `ScoutDSP::accumulate`.
    fn accumulate(&mut self, l: f32, r: f32) {
        let mono = (l + r) * 0.5;
        let a = mono.abs();

        if a > self.peak_level {
            self.peak_level = a;
        }
        if a >= 1.0 {
            self.clipped += 1;
        }
        if a > 0.0001 {
            self.active += 1;
        }
        self.rms_sum += mono as f64 * mono as f64;

        // stereo
        self.corr_sum += l as f64 * r as f64;
        self.l_energy += l as f64 * l as f64;
        self.r_energy += r as f64 * r as f64;
        self.dc_l += l as f64;
        self.dc_r += r as f64;

        let mid = (l + r) * 0.5;
        let side = (l - r) * 0.5;
        self.mid_e += mid as f64 * mid as f64;
        self.side_e += side as f64 * side as f64;

        let ll_l = bq(&mut self.lp_l, l);
        let ll_r = bq(&mut self.lp_r, r);
        let side_lf = (ll_l - ll_r) * 0.5;
        self.side_lf_e += side_lf as f64 * side_lf as f64;
        self.lf_corr += ll_l as f64 * ll_r as f64;
        self.lf_l += ll_l as f64 * ll_l as f64;
        self.lf_r += ll_r as f64 * ll_r as f64;

        // K-weighting
        let kl = bq(&mut self.k_hp_l, bq(&mut self.k_shelf_l, l));
        let kr = bq(&mut self.k_hp_r, bq(&mut self.k_shelf_r, r));
        self.k_sum += kl as f64 * kl as f64 + kr as f64 * kr as f64;

        self.k_count += 1;
        if self.k_count >= self.k_block_size {
            if self.n_blocks < MAX_BLOCKS {
                if let Some(b) = self.blocks.get_mut(self.n_blocks) {
                    *b = (self.k_sum / self.k_count as f64) as f32;
                }
                self.n_blocks += 1;
            }
            self.k_sum = 0.0;
            self.k_count = 0;
        }

        // true peak: 4x Hermite between the previous and current sample, both channels
        for k in 1..4 {
            let t = k as f32 * 0.25;
            let pl = hermite(self.lm3, self.lm2, self.lm1, l, t).abs();
            let pr = hermite(self.rm3, self.rm2, self.rm1, r, t).abs();
            if pl > self.true_peak_lin {
                self.true_peak_lin = pl;
            }
            if pr > self.true_peak_lin {
                self.true_peak_lin = pr;
            }
        }
        if l.abs() > self.true_peak_lin {
            self.true_peak_lin = l.abs();
        }
        if r.abs() > self.true_peak_lin {
            self.true_peak_lin = r.abs();
        }
        self.lm3 = self.lm2;
        self.lm2 = self.lm1;
        self.lm1 = l;
        self.rm3 = self.rm2;
        self.rm2 = self.rm1;
        self.rm1 = r;

        // 31-band spectrum + centroid
        for ((b, e_acc), &c) in self.bands.iter_mut().zip(self.band_energy.iter_mut()).zip(&self.band_centre) {
            let y = band_tick(b, mono);
            let e = y * y;
            *e_acc += e;
            self.centroid_w += c as f64 * e as f64;
            self.centroid_p += e as f64;
        }

        // 10 ms windows
        self.win_sum += mono as f64 * mono as f64;
        self.win_count += 1;
        if self.win_count >= self.win_size {
            let wr = ((self.win_sum / self.win_count as f64) as f32).sqrt();
            if self.prev_win_rms > 0.0 && wr > self.prev_win_rms * 1.5 {
                self.transients += 1;
            }
            self.prev_win_rms = wr;
            if wr > 0.0 && wr < self.min_win_rms {
                self.min_win_rms = wr;
            }
            if wr > self.max_win_rms {
                self.max_win_rms = wr;
            }

            // `clampT ((int) (-dbOf (wr)), 0, histBins - 1)`
            let bin = ((-db_of(wr)) as i32).clamp(0, HIST_BINS as i32 - 1) as usize;
            if let Some(h) = self.win_hist.get_mut(bin) {
                *h += 1;
            }
            self.win_hist_n += 1;

            self.win_sum = 0.0;
            self.win_count = 0;
        }
    }

    /// `ScoutDSP::finish`: the record for everything fed since [`start`](Self::start). Runs off
    /// the audio thread.
    pub fn finish(&self) -> Metrics {
        let mut m = Metrics::default();
        let n = self.collected;
        let n1 = n.max(1);
        let sr = self.sr;

        m.valid = n as i64 > (sr as i32) as i64;
        m.duration_sec = n as f32 / sr as f32;
        m.sample_rate = sr as f32;
        m.activity_ratio = self.active as f32 / n1 as f32;
        m.clipped_samples = i32::try_from(self.clipped).unwrap_or(i32::MAX);
        m.peak_hold_dbfs = db_of(self.peak_level);
        m.peak_dbfs = m.peak_hold_dbfs;
        m.noise_floor_dbfs = db_of(self.min_win_rms);
        m.true_peak_dbtp = db_of(self.true_peak_lin);
        m.rms_dbfs = db_of(((self.rms_sum / n1 as f64) as f32).sqrt());

        let n_blocks = self.n_blocks.min(self.blocks.len());
        let blocks = self.blocks.get(..n_blocks).unwrap_or(&[]);

        // ---- gated integrated loudness + LRA (BS.1770: -70 absolute gate, then -10 LU relative)
        m.lufs_integrated = -100.0;
        m.lra = 0.0;
        {
            let mut g = [0.0f32; MAX_BLOCKS];
            let mut ng = 0usize;
            for &b in blocks {
                if ms_to_lufs(b) > -70.0 {
                    g[ng] = b;
                    ng += 1;
                }
            }

            if ng > 0 {
                let mut mean = 0.0f64;
                for &v in &g[..ng] {
                    mean += v as f64;
                }
                mean /= ng as f64;

                let rel_gate = ms_to_lufs(mean as f32) - 10.0;

                let mut r = [0.0f32; MAX_BLOCKS];
                let mut nr = 0usize;
                let mut m2 = 0.0f64;
                for &v in &g[..ng] {
                    let lu = ms_to_lufs(v);
                    if lu > rel_gate {
                        r[nr] = lu;
                        nr += 1;
                        m2 += v as f64;
                    }
                }

                if nr > 0 {
                    m.lufs_integrated = ms_to_lufs((m2 / nr as f64) as f32);

                    r[..nr].sort_unstable_by(f32::total_cmp);

                    if nr >= 2 {
                        let i95 = pct_index(0.95, nr);
                        let i10 = pct_index(0.10, nr);
                        m.lra = r[i95] - r[i10];
                    }
                }
            }
        }

        // ---- loudness distribution over all blocks above the -70 absolute gate
        {
            let mut lv = [0.0f32; MAX_BLOCKS];
            let mut nl = 0usize;
            for &b in blocks {
                let lu = ms_to_lufs(b);
                if lu > -70.0 {
                    lv[nl] = lu;
                    nl += 1;
                }
                if lu > m.lufs_momentary_max {
                    m.lufs_momentary_max = lu;
                }
            }

            // short-term: mean-square over 8 consecutive blocks (3.2 s)
            for w in blocks.windows(8) {
                let mut acc = 0.0f64;
                for &v in w {
                    acc += v as f64;
                }
                let lu = ms_to_lufs((acc / 8.0) as f32);
                if lu > m.lufs_short_term_max {
                    m.lufs_short_term_max = lu;
                }
            }

            if nl > 0 {
                lv[..nl].sort_unstable_by(f32::total_cmp);
                m.lufs_p10 = lv[pct_index(0.10, nl)];
                m.lufs_p50 = lv[pct_index(0.50, nl)];
                m.lufs_p90 = lv[pct_index(0.90, nl)];
            }
        }

        // ---- mid/side
        m.mid_side_ratio_db = if self.mid_e > 0.0 && self.side_e > 0.0 { 10.0 * ((self.side_e / self.mid_e) as f32).log10() } else { -100.0 };
        m.side_low_ratio_db = if self.side_e > 0.0 && self.side_lf_e > 0.0 { 10.0 * ((self.side_lf_e / self.side_e) as f32).log10() } else { -100.0 };

        // ---- noise floor: RMS at the 10th percentile of 10 ms window levels
        m.noise_floor_p10_dbfs = -100.0;
        if self.win_hist_n > 0 {
            let want = (self.win_hist_n as f32 * 0.10) as u64;
            let mut acc = 0u64;
            // walk from the quietest bin upward
            for (b, &h) in self.win_hist.iter().enumerate().rev() {
                acc += h;
                if acc >= want {
                    m.noise_floor_p10_dbfs = -(b as f32);
                    break;
                }
            }
        }

        m.crest_factor_db = m.peak_dbfs - m.rms_dbfs;
        m.dynamic_range_db = db_of(self.max_win_rms) - db_of(self.min_win_rms);
        m.peak_to_lufs_db = m.peak_dbfs - m.lufs_integrated;
        m.transient_density = self.transients as f32 / cxx_max_f32(m.duration_sec, 0.001);

        for (s, &e) in m.spectrum31.iter_mut().zip(&self.band_energy) {
            *s = 10.0 * cxx_max_f32(e / n1 as f32, 1e-10).log10();
        }

        let zone = |spec: &[f32; NUM_BANDS], lo: usize, hi: usize| {
            let mut acc = 0.0f32;
            for &v in &spec[lo..=hi] {
                acc += v;
            }
            acc / (hi - lo + 1) as f32
        };
        m.zone_sub = zone(&m.spectrum31, 0, 2);
        m.zone_low = zone(&m.spectrum31, 3, 5);
        m.zone_low_mid = zone(&m.spectrum31, 6, 10);
        m.zone_mid = zone(&m.spectrum31, 11, 16);
        m.zone_high_mid = zone(&m.spectrum31, 17, 22);
        m.zone_high = zone(&m.spectrum31, 23, 26);
        m.zone_air = zone(&m.spectrum31, 27, 30);

        let denom = (self.l_energy * self.r_energy).sqrt();
        m.stereo_correlation = if denom > 0.0 { (self.corr_sum / denom) as f32 } else { 1.0 };
        m.stereo_width = if (self.l_energy + self.r_energy) > 0.0 { 1.0 - m.stereo_correlation } else { 0.0 };

        let lfd = (self.lf_l * self.lf_r).sqrt();
        m.low_freq_correlation = if lfd > 0.0 { (self.lf_corr / lfd) as f32 } else { 1.0 };
        m.lr_balance_db = if self.l_energy > 0.0 && self.r_energy > 0.0 { 10.0 * ((self.l_energy / self.r_energy) as f32).log10() } else { 0.0 };
        m.dc_offset = cxx_max_f32(((self.dc_l / n1 as f64) as f32).abs(), ((self.dc_r / n1 as f64) as f32).abs());
        m.spectral_centroid_hz = if self.centroid_p > 0.0 { (self.centroid_w / self.centroid_p) as f32 } else { 0.0 };

        m.sanitize();
        m
    }

    /// Every heap block the meter owns (address, capacity), for the no-allocation tests.
    #[doc(hidden)]
    pub fn heap_blocks(&self) -> Vec<(usize, usize)> {
        vec![(self.blocks.as_ptr() as usize, self.blocks.capacity()), (self.win_hist.as_ptr() as usize, self.win_hist.capacity())]
    }
}
