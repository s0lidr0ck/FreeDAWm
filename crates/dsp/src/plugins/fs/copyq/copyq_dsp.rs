//! CopyQDsp: makes one sound's tone match another's. Rust-native: there is no C++ original, and
//! nothing here is fitted to or copied from any product; it is textbook spectral matching.
//!
//! Per frame (an STFT: [`frame_len`] points, a new frame every quarter of that, Hann in and out):
//!
//! 1. MEASURE. The frame's power in each twelfth of an octave, for five views of the sound (the
//!    sum, left, right, mid, side), goes to the [`Meter`]: the long-term average of this module's
//!    own input, with silence and bleed left out, kept three ways (all of it, its loud passages,
//!    its quiet ones). That is the SOURCE, unless a frozen one came with the module's state.
//! 2. COMPARE. The TARGET (the profiles in the module's state, blended; or what the link group
//!    publishes) minus the source, in dB, per twelfth of an octave, with the overall level
//!    difference taken out (one number for every view, so how far the side sits under the mid
//!    carries over too). Dynamics leans both curves towards their loud or quiet versions by
//!    where the input's level now sits between its own quiet and loud levels.
//! 3. SHAPE. The difference is smoothed over frequency (Detail: from two octaves to none),
//!    scaled (Amount, times the four ranges' own amounts; negative pushes away from the target),
//!    held under the limit with a soft knee, faded out outside the working range, and smoothed
//!    in time.
//! 4. LEVEL. The level the curve changes is given back (Match Level), on a K-weighted estimate
//!    from the frame itself.
//! 5. APPLY. Linear phase: the gains are spread onto the FFT bins and the frame is resynthesised
//!    by overlap-add; the latency is the frame, always. Minimum phase: no latency; the curve is
//!    handed to a bank of [`BANDS`] third-octave bells in series, whose gains are solved so the
//!    bank's real response meets the curve at every band centre (the neighbours' skirts are
//!    worked out of each band's gain), so the match is third-octave fine there.
//!
//! Stereo: one curve for both sides, or separate curves for mid and side, or for left and right
//! (when both the target and the source were measured in stereo; otherwise one curve).
//!
//! `prepare` allocates every buffer; `process`, `reset` and the setters never allocate.

use super::measure::{ABS_GATE_DB, BandMap, Meter, MeterCfg, bin_powers, frame_len, gather_views, hann, power_scale, split_lr};
use super::profile::{ALL, CH, Curve, FLOOR_DB, GRID_N, LEFT, LOUD, LV, MID, PER_OCTAVE, QUIET, RIGHT, SIDE, SUM, Shape, grid_hz, grid_pos};
use rustfft::{Fft, FftPlanner, num_complex::Complex};
use std::sync::Arc;

/// Bells in the minimum-phase bank: a third of an octave apart from 20 Hz.
pub const BANDS: usize = 31;
/// The bells' Q.
const BANK_Q: f32 = 2.9;
/// Samples between the bank's coefficient updates.
const BANK_STEP: usize = 32;
/// A curve is compared no further than this under its own highest point, dB.
const CURVE_DEPTH_DB: f32 = 70.0;
/// The gains' time constant, ms.
const GAIN_MS: f32 = 90.0;
/// The level follower's time constant (Dynamics), ms.
const LEVEL_MS: f32 = 160.0;
/// The level estimate's time constant (Match Level), ms.
const COMP_MS: f32 = 1_200.0;
/// A live source is trusted in full once this much of it was measured, seconds.
const TRUST_SECONDS: f32 = 2.0;
/// How much of Max Gain is left exactly as asked before the limit starts to bend it.
const CAP_KNEE: f32 = 0.6;
/// The most Match Level gives back, dB.
const MAX_COMP_DB: f32 = 24.0;

/// Which curves are matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelMode {
    /// One curve (the sum's) for both sides.
    #[default]
    Stereo,
    MidSide,
    LeftRight,
}

/// How the curve is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// By STFT: the latency is the frame.
    #[default]
    Linear,
    /// By the bell bank: no latency.
    Minimum,
}

/// What the module does in its link group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Role {
    #[default]
    Off,
    /// Measures and publishes; its audio passes untouched.
    Send,
    /// Matches what the group publishes.
    Match,
    /// Publishes and matches the group's average.
    Group,
}

/// The knobs, in engine units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// -1..2: how much of the difference is applied (1: all of it).
    pub amount: f32,
    /// 0..1: how fine the curve is.
    pub detail: f32,
    /// The most any frequency is turned up or down, dB.
    pub limit_db: f32,
    /// 0..1: how much the curve follows the loud and quiet profiles.
    pub dynamics: f32,
    /// Keep measuring this module's input (off: the frozen source, or the measurement as it stands).
    pub follow: bool,
    /// The averaging window while following, seconds.
    pub window_s: f32,
    /// Frames this far under the programme level are not measured, dB.
    pub gate_db: f32,
    pub channels: ChannelMode,
    pub phase: Phase,
    /// Give back the level the curve changes.
    pub auto_gain: bool,
    /// The working range, Hz.
    pub lo_hz: f32,
    pub hi_hz: f32,
    /// Where the four ranges meet, Hz.
    pub split_hz: [f32; 3],
    /// Each range's own amount (0..2).
    pub range_amount: [f32; 4],
    /// Measure everything from now on and pass the audio untouched.
    pub capture: bool,
    pub role: Role,
    /// Play only what the curve changes.
    pub delta: bool,
    pub output_db: f32,
    pub bypass: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            amount: 1.0,
            detail: 0.4,
            limit_db: 12.0,
            dynamics: 0.0,
            follow: true,
            window_s: 10.0,
            gate_db: 24.0,
            channels: ChannelMode::Stereo,
            phase: Phase::Linear,
            auto_gain: true,
            lo_hz: 20.0,
            hi_hz: 20_000.0,
            split_hz: [150.0, 1_000.0, 6_000.0],
            range_amount: [1.0; 4],
            capture: false,
            role: Role::Off,
            delta: false,
            output_db: 0.0,
            bypass: false,
        }
    }
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Holds `x` within `cap`: untouched up to [`CAP_KNEE`] of the cap, then bending over so it never
/// gets past it.
fn soft_cap(x: f32, cap: f32) -> f32 {
    if cap <= 0.0 {
        return 0.0;
    }
    let r = x.abs() / cap;
    if r <= CAP_KNEE { x } else { x.signum() * cap * (CAP_KNEE + (1.0 - CAP_KNEE) * ((r - CAP_KNEE) / (1.0 - CAP_KNEE)).tanh()) }
}

fn frame_coef(ms: f32, frames_per_sec: f32) -> f32 {
    let frames = ms * 0.001 * frames_per_sec;
    if frames > 1.0e-3 { (-1.0 / frames).exp() } else { 0.0 }
}

fn db_lin(db: f32) -> f32 {
    10f32.powf(db.clamp(-120.0, 60.0) / 20.0)
}

/// A box average `radius` points each side (narrower at the ends), in place.
fn box_smooth(c: &mut Curve, radius: usize) {
    if radius == 0 {
        return;
    }
    let mut prefix = [0.0f32; GRID_N + 1];
    let mut run = 0.0f32;
    for (i, v) in c.iter().enumerate() {
        run += *v;
        if let Some(p) = prefix.get_mut(i + 1) {
            *p = run;
        }
    }
    for (i, v) in c.iter_mut().enumerate() {
        let lo = i.saturating_sub(radius);
        let hi = (i + radius + 1).min(GRID_N);
        let sum = prefix.get(hi).copied().unwrap_or(0.0) - prefix.get(lo).copied().unwrap_or(0.0);
        *v = sum / (hi - lo).max(1) as f32;
    }
}

/// One view of a shape as it is matched: all of it, leaned towards the loud or quiet version by
/// `lean` (0: none) at position `t` (0 quiet, 1 loud), and held above its own floor.
fn view(shape: &Shape, ch: usize, t: f32, lean: f32, out: &mut Curve) {
    let all = shape.curve(ALL, ch);
    let both = lean > 0.0 && shape.has.get(LOUD).copied().unwrap_or(false) && shape.has.get(QUIET).copied().unwrap_or(false);
    if both {
        let (loud, quiet) = (shape.curve(LOUD, ch), shape.curve(QUIET, ch));
        for (((o, a), l), q) in out.iter_mut().zip(all).zip(loud).zip(quiet) {
            // A level that has nothing at a frequency says nothing about it.
            let moved = if *l > FLOOR_DB + 1.0 && *q > FLOOR_DB + 1.0 { q + (l - q) * t - a } else { 0.0 };
            *o = a + lean * moved;
        }
    } else {
        *out = *all;
    }
}

/// The peaking bell's response as a power ratio, `x` being the frequency over the bell's own,
/// both through `tan(w / 2)`: the RBJ bell is the bilinear transform of the analogue one, so its
/// response at `w` is the analogue bell's at `tan(w/2) / tan(w0/2)`. (Worked out this way and
/// not from the coefficients, which cancel to nothing in single precision at 20 Hz.)
fn bell_ratio(x: f32, gain_db: f32) -> f32 {
    let a = 10f32.powf(gain_db / 40.0);
    let d = (1.0 - x * x).powi(2);
    let num = d + (a * x / BANK_Q).powi(2);
    let den = d + (x / (a * BANK_Q)).powi(2);
    if den > 1.0e-20 && num.is_finite() { num / den } else { 1.0 }
}

/// The same in dB.
fn bell_db(x: f32, gain_db: f32) -> f32 {
    10.0 * bell_ratio(x, gain_db).max(1.0e-12).log10()
}

/// The minimum-phase path: [`BANDS`] bells in series for each of two channels.
struct Bank {
    /// Bells under the Nyquist frequency (the first `active` of them).
    active: usize,
    cosw: [f64; BANDS],
    alpha: [f64; BANDS],
    /// `tan(w / 2)` of each band centre.
    centre: [f32; BANDS],
    /// The inverse of "what band i does at centre m per dB".
    inv: Box<[[f32; BANDS]; BANDS]>,
    /// `tan(w / 2)` of each grid point.
    grid: Box<[f32; GRID_N]>,
    target: [[f32; BANDS]; 2],
    cur: [[f32; BANDS]; 2],
    /// The bells run in double precision: at 20 Hz a single-precision biquad is mostly noise.
    coef: [[[f64; 5]; BANDS]; 2],
    z: [[[f64; 2]; BANDS]; 2],
    live: [[bool; BANDS]; 2],
    /// Samples until the next coefficient update, and updates left until `cur` is `target`.
    wait: usize,
    steps_left: usize,
    steps_per_hop: usize,
}

impl Bank {
    fn new() -> Self {
        Bank {
            active: 0,
            cosw: [0.0; BANDS],
            alpha: [0.0; BANDS],
            centre: [1.0; BANDS],
            inv: Box::new([[0.0; BANDS]; BANDS]),
            grid: Box::new([1.0; GRID_N]),
            target: [[0.0; BANDS]; 2],
            cur: [[0.0; BANDS]; 2],
            coef: [[[1.0, 0.0, 0.0, 0.0, 0.0]; BANDS]; 2],
            z: [[[0.0; 2]; BANDS]; 2],
            live: [[false; BANDS]; 2],
            wait: 0,
            steps_left: 0,
            steps_per_hop: 1,
        }
    }

    fn prepare(&mut self, sr: f64, hop: usize) {
        let sr = sr as f32;
        self.active = 0;
        for i in 0..BANDS {
            let hz = 20.0 * 2f32.powf(i as f32 / 3.0);
            if hz < 0.45 * sr {
                self.active = i + 1;
            }
            let w = std::f64::consts::TAU * f64::from(hz.min(0.49 * sr)) / f64::from(sr);
            if let (Some(c), Some(a), Some(m)) = (self.cosw.get_mut(i), self.alpha.get_mut(i), self.centre.get_mut(i)) {
                *c = w.cos();
                *a = w.sin() / (2.0 * f64::from(BANK_Q));
                *m = (w * 0.5).tan() as f32;
            }
        }
        for (j, g) in self.grid.iter_mut().enumerate() {
            let w = std::f64::consts::TAU * f64::from(grid_hz(j).min(0.49 * sr)) / f64::from(sr);
            *g = (w * 0.5).tan() as f32;
        }
        self.build_inverse();
        self.steps_per_hop = (hop / BANK_STEP).max(1);
        self.target = [[0.0; BANDS]; 2];
        self.cur = [[0.0; BANDS]; 2];
        self.clear();
        self.update_coefs();
    }

    /// Inverts the interaction matrix (Gauss-Jordan in f64; the identity if it will not invert).
    fn build_inverse(&mut self) {
        const PROTO_DB: f32 = 6.0;
        let n = self.active;
        let mut a = vec![vec![0.0f64; 2 * BANDS]; BANDS];
        for (m, row) in a.iter_mut().enumerate() {
            for i in 0..BANDS {
                let v = if m < n && i < n {
                    f64::from(bell_db(self.ratio(m, i), PROTO_DB) / PROTO_DB)
                } else if m == i {
                    1.0
                } else {
                    0.0
                };
                if let Some(s) = row.get_mut(i) {
                    *s = v;
                }
            }
            if let Some(s) = row.get_mut(BANDS + m) {
                *s = 1.0;
            }
        }
        let mut ok = true;
        for col in 0..BANDS {
            let pivot = (col..BANDS).max_by(|x, y| {
                let v = |r: &usize| a.get(*r).and_then(|row| row.get(col)).map_or(0.0, |v| v.abs());
                v(x).partial_cmp(&v(y)).unwrap_or(std::cmp::Ordering::Equal)
            });
            let Some(pivot) = pivot else { break };
            a.swap(col, pivot);
            let p = a.get(col).and_then(|r| r.get(col)).copied().unwrap_or(0.0);
            if !(p.is_finite() && p.abs() > 1.0e-9) {
                ok = false;
                break;
            }
            if let Some(row) = a.get_mut(col) {
                for v in row.iter_mut() {
                    *v /= p;
                }
            }
            let lead = a.get(col).cloned().unwrap_or_default();
            for (r, row) in a.iter_mut().enumerate() {
                let f = row.get(col).copied().unwrap_or(0.0);
                if r != col && f != 0.0 {
                    for (v, l) in row.iter_mut().zip(&lead) {
                        *v -= f * l;
                    }
                }
            }
        }
        for (m, out) in self.inv.iter_mut().enumerate() {
            for (i, o) in out.iter_mut().enumerate() {
                let v = if ok { a.get(m).and_then(|r| r.get(BANDS + i)).copied().unwrap_or(0.0) } else { f64::from(u8::from(m == i)) };
                *o = if v.is_finite() { v as f32 } else { f32::from(u8::from(m == i)) };
            }
        }
    }

    /// Centre `m`'s frequency over band `i`'s.
    fn ratio(&self, m: usize, i: usize) -> f32 {
        self.centre.get(m).copied().unwrap_or(1.0) / self.centre.get(i).copied().unwrap_or(1.0).max(1.0e-9)
    }

    /// No audio in flight.
    fn clear(&mut self) {
        self.z = [[[0.0; 2]; BANDS]; 2];
        self.wait = 0;
    }

    /// Solves the band gains for channel `ch` so the bank reads `want` (dB) at the band centres.
    fn solve(&mut self, ch: usize, want: &[f32; BANDS]) {
        let n = self.active;
        let mut g = [0.0f32; BANDS];
        let apply = |inv: &[[f32; BANDS]; BANDS], e: &[f32; BANDS], g: &mut [f32; BANDS]| {
            for (m, gm) in g.iter_mut().enumerate().take(n) {
                let row = inv.get(m);
                let add: f32 = e.iter().take(n).enumerate().map(|(i, v)| v * row.and_then(|r| r.get(i)).copied().unwrap_or(0.0)).sum();
                *gm = (*gm + add).clamp(-36.0, 36.0);
            }
        };
        apply(&self.inv, want, &mut g);
        for _ in 0..3 {
            let mut err = [0.0f32; BANDS];
            for (m, e) in err.iter_mut().enumerate().take(n) {
                let mut ratio = 1.0f32;
                for (i, gi) in g.iter().enumerate().take(n) {
                    if gi.abs() > 1.0e-4 {
                        ratio *= bell_ratio(self.ratio(m, i), *gi);
                    }
                }
                *e = want.get(m).copied().unwrap_or(0.0) - 10.0 * ratio.max(1.0e-12).log10();
            }
            apply(&self.inv, &err, &mut g);
        }
        if let Some(t) = self.target.get_mut(ch) {
            for (t, v) in t.iter_mut().zip(&g) {
                *t = if v.is_finite() { *v } else { 0.0 };
            }
        }
    }

    /// The solved gains take effect over the next hop (`glide`) or at once.
    fn commit(&mut self, glide: bool) {
        if glide {
            self.steps_left = self.steps_per_hop;
        } else {
            self.cur = self.target;
            self.steps_left = 0;
            self.update_coefs();
        }
    }

    fn update_coefs(&mut self) {
        for ch in 0..2 {
            for i in 0..BANDS {
                let g = self.cur.get(ch).and_then(|c| c.get(i)).copied().unwrap_or(0.0);
                let t = self.target.get(ch).and_then(|c| c.get(i)).copied().unwrap_or(0.0);
                let live = i < self.active && (g.abs() > 1.0e-3 || t.abs() > 1.0e-3);
                let was = self.live.get(ch).and_then(|l| l.get(i)).copied().unwrap_or(false);
                if let Some(l) = self.live.get_mut(ch).and_then(|l| l.get_mut(i)) {
                    *l = live;
                }
                if !live {
                    if was && let Some(z) = self.z.get_mut(ch).and_then(|z| z.get_mut(i)) {
                        *z = [0.0; 2];
                    }
                    continue;
                }
                let a = 10f64.powf(f64::from(g) / 40.0);
                let alpha = self.alpha.get(i).copied().unwrap_or(0.1);
                let cosw = self.cosw.get(i).copied().unwrap_or(0.0);
                let a0 = 1.0 + alpha / a;
                if let Some(c) = self.coef.get_mut(ch).and_then(|c| c.get_mut(i)) {
                    *c = [(1.0 + alpha * a) / a0, -2.0 * cosw / a0, (1.0 - alpha * a) / a0, -2.0 * cosw / a0, (1.0 - alpha / a) / a0];
                }
            }
        }
    }

    /// Once per sample, before [`Bank::run`]: glides the gains towards the solved ones.
    fn tick(&mut self) {
        if self.wait > 0 {
            self.wait -= 1;
            return;
        }
        self.wait = BANK_STEP - 1;
        if self.steps_left == 0 {
            return;
        }
        let left = self.steps_left as f32;
        for (cur, target) in self.cur.iter_mut().zip(&self.target) {
            for (c, t) in cur.iter_mut().zip(target) {
                *c += (*t - *c) / left;
            }
        }
        self.steps_left -= 1;
        self.update_coefs();
    }

    #[inline]
    fn run(&mut self, ch: usize, x: f32) -> f32 {
        let (Some(coef), Some(z), Some(live)) = (self.coef.get(ch), self.z.get_mut(ch), self.live.get(ch)) else { return x };
        let mut v = f64::from(x);
        for ((c, z), on) in coef.iter().zip(z.iter_mut()).zip(live).take(self.active) {
            if !*on {
                continue;
            }
            let y = c[0] * v + z[0];
            z[0] = c[1] * v - c[3] * y + z[1];
            z[1] = c[2] * v - c[4] * y;
            // A tail that has died away is zero, not a denormal.
            if z[0].abs() < 1.0e-30 && z[1].abs() < 1.0e-30 {
                *z = [0.0; 2];
            }
            v = y;
        }
        if v.is_finite() {
            v as f32
        } else {
            z.fill([0.0; 2]);
            0.0
        }
    }

    /// The bank's response on the grid for channel `ch`, dB.
    fn response(&self, ch: usize, out: &mut Curve) {
        let Some(cur) = self.cur.get(ch) else { return out.fill(0.0) };
        for (o, at) in out.iter_mut().zip(self.grid.iter()) {
            let mut ratio = 1.0f32;
            for (i, g) in cur.iter().enumerate().take(self.active) {
                if g.abs() > 1.0e-3 {
                    ratio *= bell_ratio(at / self.centre.get(i).copied().unwrap_or(1.0).max(1.0e-9), *g);
                }
            }
            *o = 10.0 * ratio.max(1.0e-12).log10();
        }
    }
}

/// The engine. See the module docs.
pub struct CopyQDsp {
    sr: f64,
    channels: usize,
    n: usize,
    hop: usize,
    pos: usize,
    count: usize,
    fft: Option<Arc<dyn Fft<f32>>>,
    ifft: Option<Arc<dyn Fft<f32>>>,
    scratch: Vec<Complex<f32>>,
    buf: Vec<Complex<f32>>,
    window: Vec<f32>,
    inp: [Vec<f32>; 2],
    ola: [Vec<f32>; 2],
    /// Power per bin of the left, right, mid and side of the frame.
    pw: [Vec<f32>; 4],
    map: BandMap,
    /// Each bin's place on the grid: the point under it and how far to the next.
    bin_at: Vec<u16>,
    bin_frac: Vec<f32>,
    /// The frame's power per band, per view.
    views: Box<[Curve; CH]>,
    meter: Meter,
    /// The meter's measurement in dB, as a shape.
    own: Box<Shape>,
    target_state: Box<Shape>,
    source_state: Box<Shape>,
    target_link: Box<Shape>,
    set: Settings,
    range_w: Curve,
    range_amt: Curve,
    kweight: Curve,
    radius: usize,
    coef_gain: f32,
    coef_level: f32,
    coef_comp: f32,
    /// The gain of the two curves (A: both sides, mid or left; B: side or right), dB, smoothed.
    gain: Box<[Curve; 2]>,
    lin: Box<[Curve; 2]>,
    /// What is shown: the source and the target (moved to the source's level) of each curve, and
    /// the response really applied.
    shown_source: Box<[Curve; 2]>,
    shown_target: Box<[Curve; 2]>,
    applied: Box<[Curve; 2]>,
    /// Whether the two curves differ (mid/side or left/right in force).
    split: bool,
    matching: bool,
    level_s: f32,
    dyn_t: f32,
    comp_in: f32,
    comp_out: f32,
    comp_db: f32,
    /// The broadband factor (Match Level and Output) of the last frame and, per sample, the
    /// minimum-phase path's.
    total: f32,
    total_s: f32,
    max_cut: f32,
    max_boost: f32,
    bank: Bank,
    /// Frames run since the last [`CopyQDsp::take_fresh`].
    fresh: bool,
}

impl Default for CopyQDsp {
    fn default() -> Self {
        Self::new()
    }
}

impl CopyQDsp {
    pub fn new() -> Self {
        let mut s = CopyQDsp {
            sr: 48_000.0,
            channels: 2,
            n: 0,
            hop: 0,
            pos: 0,
            count: 0,
            fft: None,
            ifft: None,
            scratch: Vec::new(),
            buf: Vec::new(),
            window: Vec::new(),
            inp: [Vec::new(), Vec::new()],
            ola: [Vec::new(), Vec::new()],
            pw: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            map: BandMap::default(),
            bin_at: Vec::new(),
            bin_frac: Vec::new(),
            views: Box::new([[0.0; GRID_N]; CH]),
            meter: Meter::new(),
            own: Box::default(),
            target_state: Box::default(),
            source_state: Box::default(),
            target_link: Box::default(),
            set: Settings::default(),
            range_w: [1.0; GRID_N],
            range_amt: [1.0; GRID_N],
            kweight: [1.0; GRID_N],
            radius: 0,
            coef_gain: 0.0,
            coef_level: 0.0,
            coef_comp: 0.0,
            gain: Box::new([[0.0; GRID_N]; 2]),
            lin: Box::new([[1.0; GRID_N]; 2]),
            shown_source: Box::new([[FLOOR_DB; GRID_N]; 2]),
            shown_target: Box::new([[FLOOR_DB; GRID_N]; 2]),
            applied: Box::new([[0.0; GRID_N]; 2]),
            split: false,
            matching: false,
            level_s: FLOOR_DB,
            dyn_t: 0.5,
            comp_in: 0.0,
            comp_out: 0.0,
            comp_db: 0.0,
            total: 1.0,
            total_s: 1.0,
            max_cut: 0.0,
            max_boost: 0.0,
            bank: Bank::new(),
            fresh: false,
        };
        s.prepare(48_000.0, 2);
        s
    }

    pub fn sample_rate(&self) -> f64 {
        self.sr
    }

    /// The frame length (the linear-phase latency).
    pub fn frame(&self) -> usize {
        self.n
    }

    /// The latency in force: the frame in linear phase, none in minimum phase.
    pub fn latency_samples(&self) -> usize {
        match self.set.phase {
            Phase::Linear => self.n,
            Phase::Minimum => 0,
        }
    }

    pub fn settings(&self) -> Settings {
        self.set
    }

    /// This module's own input as measured so far.
    pub fn own(&self) -> &Shape {
        &self.own
    }

    /// Seconds measured per level.
    pub fn measured_seconds(&self) -> [f32; LV] {
        self.meter.seconds()
    }

    /// Whether a curve is being applied (there is a target and a source, and the module is not
    /// capturing, sending or bypassed).
    pub fn matching(&self) -> bool {
        self.matching
    }

    /// Whether the two curves differ (mid/side or left/right in force).
    pub fn split(&self) -> bool {
        self.split
    }

    /// The source and target shown for curve `ab` (0 or 1) and the response applied, dB.
    pub fn shown(&self, ab: usize) -> (&Curve, &Curve, &Curve) {
        static ZERO: Curve = [0.0; GRID_N];
        (self.shown_source.get(ab).unwrap_or(&ZERO), self.shown_target.get(ab).unwrap_or(&ZERO), self.applied.get(ab).unwrap_or(&ZERO))
    }

    /// The largest cut and the largest lift in force (both non-negative), dB.
    pub fn extremes_db(&self) -> (f32, f32) {
        (self.max_cut, self.max_boost)
    }

    /// The level given back, dB.
    pub fn level_comp_db(&self) -> f32 {
        self.comp_db
    }

    /// Where the input's level sits between its quiet (0) and loud (1) levels.
    pub fn dynamics_position(&self) -> f32 {
        self.dyn_t
    }

    /// The input's level now, dB.
    pub fn level_db(&self) -> f32 {
        self.level_s
    }

    /// Whether a frame ran since the last call.
    pub fn take_fresh(&mut self) -> bool {
        std::mem::take(&mut self.fresh)
    }

    /// Sizes everything for a rate and channel count (1 or 2) and goes to rest. Allocates. Keeps
    /// the settings, the shapes and the measurement (which is in bands, not bins).
    pub fn prepare(&mut self, sample_rate: f64, channels: usize) {
        let sr = if sample_rate.is_finite() { sample_rate.clamp(8_000.0, 384_000.0) } else { 48_000.0 };
        self.sr = sr;
        self.channels = channels.clamp(1, 2);
        let n = frame_len(sr);
        self.n = n;
        self.hop = n / 4;
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);
        let ifft = planner.plan_fft_inverse(n);
        self.scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len().max(ifft.get_inplace_scratch_len())];
        self.fft = Some(fft);
        self.ifft = Some(ifft);
        self.buf = vec![Complex::new(0.0, 0.0); n];
        self.window = hann(n);
        for v in self.inp.iter_mut().chain(self.ola.iter_mut()) {
            *v = vec![0.0; n];
        }
        for v in self.pw.iter_mut() {
            *v = vec![0.0; n / 2 + 1];
        }
        self.map = BandMap::new(n, sr);
        let df = (sr / n as f64) as f32;
        self.bin_at = Vec::with_capacity(n / 2 + 1);
        self.bin_frac = Vec::with_capacity(n / 2 + 1);
        for k in 0..=n / 2 {
            let p = grid_pos(k as f32 * df).clamp(0.0, (GRID_N - 1) as f32);
            let at = (p.floor() as usize).min(GRID_N - 2);
            self.bin_at.push(at as u16);
            self.bin_frac.push((p - at as f32).clamp(0.0, 1.0));
        }
        for (i, k) in self.kweight.iter_mut().enumerate() {
            // About the K weighting of BS.1770: a shelf of 4 dB above 1.5 kHz and a low cut at 38 Hz.
            let f = grid_hz(i);
            let db = 4.0 / (1.0 + (1_500.0 / f).powi(2)) - 10.0 * (1.0 + (38.0 / f).powi(4)).log10();
            *k = 10f32.powf(db / 10.0);
        }
        let fps = (sr / self.hop.max(1) as f64) as f32;
        self.coef_gain = frame_coef(GAIN_MS, fps);
        self.coef_level = frame_coef(LEVEL_MS, fps);
        self.coef_comp = frame_coef(COMP_MS, fps);
        self.bank.prepare(sr, self.hop);
        self.update_curves();
        self.reset();
    }

    /// No audio in flight. The settings, the measurement and the curve are kept: none of them
    /// is audio, and a transport that stops and starts must not start the match over.
    pub fn reset(&mut self) {
        for v in self.inp.iter_mut().chain(self.ola.iter_mut()) {
            v.fill(0.0);
        }
        self.pos = 0;
        self.count = 0;
        self.bank.clear();
        self.total_s = self.total;
    }

    /// Forgets what was measured of this module's input.
    pub fn reset_measure(&mut self) {
        self.meter.reset();
        self.own.clear();
        self.fresh = true;
    }

    /// Takes the knobs (clamped; a non-finite value keeps its default).
    pub fn set(&mut self, s: Settings) {
        let d = Settings::default();
        let f = |v: f32, lo: f32, hi: f32, def: f32| if v.is_finite() { v.clamp(lo, hi) } else { def };
        let mut split = [f(s.split_hz[0], 20.0, 20_000.0, d.split_hz[0]), f(s.split_hz[1], 20.0, 20_000.0, d.split_hz[1]), f(s.split_hz[2], 20.0, 20_000.0, d.split_hz[2])];
        split.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut range_amount = d.range_amount;
        for (o, v) in range_amount.iter_mut().zip(s.range_amount) {
            *o = f(v, 0.0, 2.0, 1.0);
        }
        let new = Settings {
            amount: f(s.amount, -1.0, 2.0, d.amount),
            detail: f(s.detail, 0.0, 1.0, d.detail),
            limit_db: f(s.limit_db, 1.0, 24.0, d.limit_db),
            dynamics: f(s.dynamics, 0.0, 1.0, d.dynamics),
            window_s: f(s.window_s, 1.0, 60.0, d.window_s),
            gate_db: f(s.gate_db, 6.0, 60.0, d.gate_db),
            lo_hz: f(s.lo_hz, 20.0, 20_000.0, d.lo_hz),
            hi_hz: f(s.hi_hz, 20.0, 20_000.0, d.hi_hz),
            split_hz: split,
            range_amount,
            output_db: f(s.output_db, -24.0, 24.0, d.output_db),
            ..s
        };
        if new == self.set {
            return;
        }
        let capture_starts = new.capture && !self.set.capture;
        let phase_changes = new.phase != self.set.phase;
        self.set = new;
        if capture_starts {
            self.reset_measure();
        }
        if phase_changes {
            self.bank.clear();
            self.bank.commit(false);
            self.total_s = self.total;
        }
        self.update_curves();
    }

    /// The target and the frozen source of the module's state ([`Shape::from_prepared`]'s data).
    /// Copies into fixed storage: no allocation.
    pub fn set_prepared(&mut self, data: &[f64]) {
        self.target_state.from_prepared(data, 0);
        self.source_state.from_prepared(data, 1);
        self.fresh = true;
    }

    /// The target the link group publishes (`None`: none). No allocation.
    pub fn set_link_target(&mut self, shape: Option<&Shape>) {
        match shape {
            Some(s) => self.target_link.copy_from(s),
            None => self.target_link.clear(),
        }
    }

    /// The target of the module's state, and its frozen source.
    pub fn state_shapes(&self) -> (&Shape, &Shape) {
        (&self.target_state, &self.source_state)
    }

    fn update_curves(&mut self) {
        let s = self.set;
        let (lo, hi) = (s.lo_hz.min(s.hi_hz), s.hi_hz.max(s.lo_hz));
        for (i, (w, a)) in self.range_w.iter_mut().zip(self.range_amt.iter_mut()).enumerate() {
            let f = grid_hz(i);
            // Half-octave fades at the range's ends; none at an end left at the bottom or top.
            let up = if lo <= 20.5 { 1.0 } else { smoothstep((f / lo).log2() * 2.0 + 0.5) };
            let down = if hi >= 19_900.0 { 1.0 } else { 1.0 - smoothstep((f / hi).log2() * 2.0 + 0.5) };
            *w = if self.map.valid(i) { up * down } else { 0.0 };
            // The four ranges, crossing over an octave around each split.
            let mut amt = s.range_amount[0];
            for (j, split) in s.split_hz.iter().enumerate() {
                let step = s.range_amount.get(j + 1).copied().unwrap_or(1.0) - s.range_amount.get(j).copied().unwrap_or(1.0);
                amt += step * smoothstep((f / split).log2() + 0.5);
            }
            *a = amt.clamp(0.0, 2.0);
        }
        // Two passes of a box: from about two octaves across (Detail 0) to none (Detail 100 %).
        let width_oct = 2.0 * (1.0f32 / 24.0).powf(s.detail);
        self.radius = (width_oct * PER_OCTAVE as f32 / 4.0).round() as usize;
    }

    /// Processes `frames` samples of `io` in place (at most the prepared channels, at most each
    /// buffer's length).
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
        let minimum = self.set.phase == Phase::Minimum;
        let (ms, delta, bypass) = (self.set.channels == ChannelMode::MidSide && nc > 1, self.set.delta, self.set.bypass);
        for i in 0..frames {
            let mut x = [0.0f32; 2];
            let mut wet = [0.0f32; 2];
            for c in 0..nc {
                let v = io.get(c).and_then(|v| v.get(i)).copied().unwrap_or(0.0);
                let v = if v.is_finite() { v } else { 0.0 };
                if let Some(slot) = x.get_mut(c) {
                    *slot = v;
                }
                if let Some(slot) = self.inp.get_mut(c).and_then(|b| b.get_mut(self.pos)) {
                    *slot = v;
                }
                if let (Some(slot), Some(w)) = (self.ola.get_mut(c).and_then(|b| b.get_mut(self.pos)), wet.get_mut(c)) {
                    *w = *slot;
                    *slot = 0.0;
                }
            }
            let y = if !minimum {
                wet
            } else if bypass {
                x
            } else {
                self.bank.tick();
                self.total_s += (self.total - self.total_s) * 0.002;
                let (a, b) = if ms { ((x[0] + x[1]) * 0.5, (x[0] - x[1]) * 0.5) } else { (x[0], x[1]) };
                let a = self.bank.run(0, a);
                let b = if nc > 1 { self.bank.run(1, b) } else { 0.0 };
                let (l, r) = if ms { (a + b, a - b) } else { (a, b) };
                let (l, r) = (l * self.total_s, r * self.total_s);
                if delta { [l - x[0], r - x[1]] } else { [l, r] }
            };
            for c in 0..nc {
                if let (Some(o), Some(v)) = (io.get_mut(c).and_then(|v| v.get_mut(i)), y.get(c)) {
                    *o = *v;
                }
            }
            self.pos += 1;
            if self.pos >= n {
                self.pos = 0;
            }
            self.count += 1;
            if self.count >= self.hop {
                self.count = 0;
                self.run_frame(nc);
            }
        }
    }

    /// One STFT frame: measure, work out the curve, and (linear phase) overlap-add the result.
    fn run_frame(&mut self, nc: usize) {
        let n = self.n;
        let (Some(fft), Some(ifft)) = (self.fft.clone(), self.ifft.clone()) else { return };
        for (j, b) in self.buf.iter_mut().enumerate() {
            let idx = (self.pos + j) % n;
            let w = self.window.get(j).copied().unwrap_or(0.0);
            let l = self.inp.first().and_then(|v| v.get(idx)).copied().unwrap_or(0.0);
            let r = if nc > 1 { self.inp.get(1).and_then(|v| v.get(idx)).copied().unwrap_or(0.0) } else { 0.0 };
            *b = Complex::new(l * w, r * w);
        }
        fft.process_with_scratch(&mut self.buf, &mut self.scratch);
        let mono = nc < 2;
        let [l, r, m, s] = &mut self.pw;
        bin_powers(&self.buf, mono, power_scale(n), l, r, m, s);
        gather_views(&self.map, l, r, m, s, &mut self.views);
        self.measure(mono);
        self.curves(mono);
        self.fresh = true;
        if self.set.phase == Phase::Minimum {
            // The bank does the work; nothing is resynthesised.
            return;
        }
        let half = n / 2;
        let (ms, delta, bypass) = (self.set.channels == ChannelMode::MidSide && !mono, self.set.delta, self.set.bypass);
        let total = self.total;
        for k in 0..=half {
            let m = (n - k) % n;
            let at = self.bin_at.get(k).copied().unwrap_or(0) as usize;
            let frac = self.bin_frac.get(k).copied().unwrap_or(0.0);
            let lerp = |c: Option<&Curve>| {
                let a = c.and_then(|c| c.get(at)).copied().unwrap_or(1.0);
                let b = c.and_then(|c| c.get(at + 1)).copied().unwrap_or(a);
                a + (b - a) * frac
            };
            let (mut ga, mut gb) = (lerp(self.lin.first()) * total, lerp(self.lin.get(1)) * total);
            if bypass {
                (ga, gb) = (1.0, 1.0);
            } else if delta {
                (ga, gb) = (ga - 1.0, gb - 1.0);
            }
            let (Some(zk), Some(zm)) = (self.buf.get(k).copied(), self.buf.get(m).copied()) else { continue };
            let (l, r) = split_lr(zk, zm);
            let (l, r) = if ms {
                let (mid, side) = ((l + r) * 0.5 * ga, (l - r) * 0.5 * gb);
                (mid + side, mid - side)
            } else {
                (l * ga, r * gb)
            };
            // Z = L + jR at the bin, and conj L + j conj R at its mirror.
            let jr = Complex::new(-r.im, r.re);
            if let Some(b) = self.buf.get_mut(k) {
                *b = l + jr;
            }
            if m != k
                && let Some(b) = self.buf.get_mut(m)
            {
                *b = l.conj() + Complex::new(r.im, r.re);
            }
        }
        ifft.process_with_scratch(&mut self.buf, &mut self.scratch);
        // Hann twice at a hop of N/4 sums to 1.5; the inverse FFT is unnormalised.
        let norm = 1.0 / (1.5 * n as f32);
        for (j, v) in self.buf.iter().enumerate() {
            let idx = (self.pos + j) % n;
            let w = self.window.get(j).copied().unwrap_or(0.0) * norm;
            if let Some(o) = self.ola.first_mut().and_then(|o| o.get_mut(idx)) {
                *o += v.re * w;
            }
            if !mono && let Some(o) = self.ola.get_mut(1).and_then(|o| o.get_mut(idx)) {
                *o += v.im * w;
            }
        }
    }

    /// The frame into the meter, and the meter into `own`.
    fn measure(&mut self, mono: bool) {
        let s = self.set;
        let fps = (self.sr / self.hop.max(1) as f64) as f32;
        let total: f32 = self.views.first().map_or(0.0, |c| c.iter().sum());
        let level = super::measure::pow_db(total);
        self.level_s = if self.level_s <= FLOOR_DB + 1.0 { level } else { level + (self.level_s - level) * self.coef_level };
        let running = s.capture || s.follow || matches!(s.role, Role::Send | Role::Group);
        if !running {
            return;
        }
        let cfg = MeterCfg { window_frames: if s.capture { 0.0 } else { s.window_s * fps }, gate_db: s.gate_db, frames_per_sec: fps };
        if !self.meter.add_frame(&self.views, cfg) {
            return;
        }
        let seconds = self.meter.seconds();
        self.own.present = seconds.first().copied().unwrap_or(0.0) > 0.0;
        self.own.stereo = !mono;
        self.own.level = self.meter.level();
        for lv in 0..LV {
            if let Some(h) = self.own.has.get_mut(lv) {
                *h = seconds.get(lv).copied().unwrap_or(0.0) >= if lv == ALL { f32::MIN_POSITIVE } else { super::profile::MIN_LEVEL_SECONDS };
            }
        }
        for (i, c) in self.own.curves.iter_mut().enumerate() {
            self.meter.curve_db(i, c);
        }
    }

    /// From the target and the source to the gains, the level given back and what is shown.
    fn curves(&mut self, mono: bool) {
        let s = self.set;
        let linked = matches!(s.role, Role::Match | Role::Group) && self.target_link.present;
        let target: &Shape = if linked { &self.target_link } else { &self.target_state };
        let frozen = !s.follow && self.source_state.present;
        let source: &Shape = if frozen { &self.source_state } else { &self.own };
        let matching = target.present && source.present && !s.capture && s.role != Role::Send && !s.bypass;
        self.matching = matching;
        // Mid/side and left/right need both sounds in stereo.
        let split = !mono && s.channels != ChannelMode::Stereo && target.stereo && source.stereo;
        self.split = split;
        let pair = match (split, s.channels) {
            (true, ChannelMode::MidSide) => [MID, SIDE],
            (true, ChannelMode::LeftRight) => [LEFT, RIGHT],
            _ => [SUM, SUM],
        };
        // Where the level now sits between the source's quiet and loud levels.
        let (loud, quiet) = (source.level.get(LOUD).copied().unwrap_or(0.0), source.level.get(QUIET).copied().unwrap_or(0.0));
        let both = source.has.get(LOUD).copied().unwrap_or(false) && source.has.get(QUIET).copied().unwrap_or(false);
        let t = if both { ((self.level_s - quiet) / (loud - quiet).max(1.0)).clamp(0.0, 1.0) } else { 0.5 };
        self.dyn_t += (t - self.dyn_t) * (1.0 - self.coef_level);
        let lean = if both { s.dynamics } else { 0.0 };
        // The level difference, from the sums: one number for every view.
        let mut tv = [0.0f32; GRID_N];
        let mut sv = [0.0f32; GRID_N];
        view(target, SUM, self.dyn_t, 0.0, &mut tv);
        view(source, SUM, self.dyn_t, 0.0, &mut sv);
        let floor_of = |c: &Curve| c.iter().copied().fold(FLOOR_DB, f32::max) - CURVE_DEPTH_DB;
        let (tf, sf) = (floor_of(&tv), floor_of(&sv));
        let (mut sum, mut weight) = (0.0f32, 0.0f32);
        for ((a, b), w) in tv.iter().zip(&sv).zip(&self.range_w) {
            if *a > tf && *b > sf && *w > 0.0 {
                sum += (a - b) * w;
                weight += w;
            }
        }
        let offset = if weight > 0.0 { sum / weight } else { 0.0 };
        // A live source is not trusted before enough of it was heard.
        let seconds = self.meter.seconds().first().copied().unwrap_or(0.0);
        let trust = if frozen { 1.0 } else { smoothstep(seconds / TRUST_SECONDS) };
        let mut want = [[0.0f32; GRID_N]; 2];
        for (ab, ch) in pair.iter().enumerate() {
            if ab == 1 && !split {
                want[1] = want[0];
                if let (Some(a), Some(b)) = (self.shown_source.first().copied(), self.shown_source.get_mut(1)) {
                    *b = a;
                }
                if let (Some(a), Some(b)) = (self.shown_target.first().copied(), self.shown_target.get_mut(1)) {
                    *b = a;
                }
                break;
            }
            view(target, *ch, self.dyn_t, lean, &mut tv);
            view(source, *ch, self.dyn_t, lean, &mut sv);
            let Some(d) = want.get_mut(ab) else { continue };
            for (i, o) in d.iter_mut().enumerate() {
                let (tt, ss) = (tv.get(i).copied().unwrap_or(FLOOR_DB), sv.get(i).copied().unwrap_or(FLOOR_DB));
                // Where the source has nothing there is nothing to turn up or down.
                *o = if ss > sf { tt.max(tf) - ss - offset } else { 0.0 };
            }
            box_smooth(d, self.radius);
            box_smooth(d, self.radius);
            for (i, o) in d.iter_mut().enumerate() {
                let scale = s.amount * self.range_amt.get(i).copied().unwrap_or(1.0) * trust;
                let v = soft_cap(*o * scale, s.limit_db) * self.range_w.get(i).copied().unwrap_or(0.0);
                *o = if matching && v.is_finite() { v } else { 0.0 };
            }
            if let Some(shown) = self.shown_source.get_mut(ab) {
                *shown = if source.present { sv } else { [FLOOR_DB; GRID_N] };
            }
            if let Some(shown) = self.shown_target.get_mut(ab) {
                for (o, v) in shown.iter_mut().zip(&tv) {
                    *o = if target.present { (v - offset).max(FLOOR_DB) } else { FLOOR_DB };
                }
            }
        }
        // Smoothed in time; the extremes; the linear gains.
        let (mut cut, mut boost) = (0.0f32, 0.0f32);
        for ((g, w), lin) in self.gain.iter_mut().zip(&want).zip(self.lin.iter_mut()) {
            for ((g, w), l) in g.iter_mut().zip(w).zip(lin.iter_mut()) {
                *g = w + (*g - w) * self.coef_gain;
                if g.abs() < 1.0e-4 {
                    *g = 0.0;
                }
                cut = cut.max(-*g);
                boost = boost.max(*g);
                *l = db_lin(*g);
            }
        }
        self.max_cut = cut;
        self.max_boost = boost;
        // The level given back: the frame's K-weighted power before and after the curve.
        let views = &self.views;
        let power = |ch: usize, gains: Option<&Curve>| -> f32 {
            let Some(p) = views.get(ch) else { return 0.0 };
            p.iter().zip(&self.kweight).enumerate().map(|(i, (p, k))| p * k * gains.and_then(|g| g.get(i)).map_or(1.0, |l| l * l)).sum()
        };
        let (a, b) = (pair[0], pair[1]);
        let p_in = power(a, None) + power(b, None);
        let p_out = power(a, self.lin.first()) + power(b, self.lin.get(1));
        if self.level_s > ABS_GATE_DB && p_in.is_finite() && p_out.is_finite() {
            self.comp_in += (p_in - self.comp_in) * (1.0 - self.coef_comp);
            self.comp_out += (p_out - self.comp_out) * (1.0 - self.coef_comp);
        }
        let comp = if s.auto_gain && matching && self.comp_in > 1.0e-18 && self.comp_out > 1.0e-18 {
            (10.0 * (self.comp_in / self.comp_out).log10()).clamp(-MAX_COMP_DB, MAX_COMP_DB)
        } else {
            0.0
        };
        self.comp_db = comp + (self.comp_db - comp) * self.coef_gain;
        if self.comp_db.abs() < 1.0e-4 {
            self.comp_db = 0.0;
        }
        self.total = db_lin(self.comp_db + s.output_db);
        // The bank's gains (always kept current, so a switch of phase lands on the curve), and
        // the response really applied.
        for ab in 0..2 {
            let mut at_centres = [0.0f32; BANDS];
            for (i, v) in at_centres.iter_mut().enumerate() {
                *v = self.gain.get(ab).and_then(|g| g.get(i * 4)).copied().unwrap_or(0.0);
            }
            self.bank.solve(ab, &at_centres);
        }
        let minimum = s.phase == Phase::Minimum;
        self.bank.commit(minimum);
        for ab in 0..2 {
            let Some(out) = self.applied.get_mut(ab) else { continue };
            if minimum {
                self.bank.response(ab, out);
            } else if let Some(g) = self.gain.get(ab) {
                *out = *g;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bank_meets_a_curve_at_its_band_centres() {
        let mut bank = Bank::new();
        bank.prepare(48_000.0, 1024);
        // A tilt with a bump: up to ±9 dB.
        let mut want = [0.0f32; BANDS];
        for (i, w) in want.iter_mut().enumerate() {
            *w = (i as f32 - 15.0) * 0.4 + if (12..=14).contains(&i) { 4.0 } else { 0.0 };
        }
        bank.solve(0, &want);
        bank.commit(false);
        let mut resp = [0.0f32; GRID_N];
        bank.response(0, &mut resp);
        for (i, w) in want.iter().enumerate().take(bank.active) {
            assert!((resp[i * 4] - w).abs() < 0.35, "band {i}: want {w}, got {}", resp[i * 4]);
        }
    }

    #[test]
    fn the_bank_at_rest_is_the_identity_and_survives_garbage() {
        let mut bank = Bank::new();
        bank.prepare(44_100.0, 1024);
        for i in 0..2000 {
            bank.tick();
            let x = (i as f32 * 0.37).sin();
            assert_eq!(bank.run(0, x), x);
        }
        bank.solve(1, &[f32::NAN; BANDS]);
        bank.commit(false);
        assert!(bank.run(1, 1.0).is_finite());
        assert!(bank.run(1, f32::INFINITY).is_finite());
        assert!(bank.run(7, 0.5) == 0.5);
    }

    #[test]
    fn box_smoothing_keeps_a_constant_and_spreads_a_spike() {
        let mut c = [3.0f32; GRID_N];
        box_smooth(&mut c, 5);
        assert!(c.iter().all(|v| (v - 3.0).abs() < 1.0e-4));
        let mut c = [0.0f32; GRID_N];
        c[60] = 11.0;
        box_smooth(&mut c, 5);
        assert!((c[60] - 1.0).abs() < 1.0e-4 && (c[55] - 1.0).abs() < 1.0e-4 && c[54] == 0.0);
        assert!((c.iter().sum::<f32>() - 11.0).abs() < 1.0e-3);
    }
}
