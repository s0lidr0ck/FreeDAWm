//! The monitor stage: what the listener hears after the mix, and nothing a bounce ever holds.
//!
//! Two stages run in order on the stereo main mix on its way to the device:
//! 1. **Sim**: a simulated output (a phone, a car…): one curve on both sides, optional high- and
//!    low-pass, and a stereo width (0 = mono).
//! 2. **Ear**: a hearing correction: one curve per ear.
//!
//! A curve is a list of `(Hz, dB)` points, realised as a cascade of third-octave peaking biquads
//! fitted to it (minimum phase: the stage adds no latency). A [`Plan`] is designed off the audio
//! thread; [`State::load`] takes it on the audio thread without allocating, and
//! [`State::process`] runs it. Turning a stage on or off fades it over [`FADE_SECS`].

use crate::biquad::{Biquad, Coeffs, FilterType, butterworth_qs};

/// Third-octave bands a curve is fitted with (20 Hz to 20 kHz).
pub const BANDS: usize = 31;
/// Sections of one channel of one stage: the bands, a 24 dB/oct high-pass and low-pass.
pub const MAX_SECTIONS: usize = BANDS + 4;
/// Most points of a curve that are read.
pub const MAX_POINTS: usize = 256;
/// The largest gain of a curve point, either way, in dB.
pub const MAX_POINT_DB: f32 = 40.0;
/// How long a stage takes to fade in or out.
pub const FADE_SECS: f32 = 0.015;

const BAND_Q: f32 = 3.0;
const FIT_PASSES: usize = 24;

/// The centre of band `i`, Hz (band 17 is 1 kHz).
pub fn band_hz(i: usize) -> f32 {
    1000.0 * 2f32.powf((i as f32 - 17.0) / 3.0)
}

/// `points` as they are used: finite, 10 Hz..24 kHz, ±[`MAX_POINT_DB`], sorted by frequency, at
/// most [`MAX_POINTS`].
pub fn clean_points(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut v: Vec<[f32; 2]> = points
        .iter()
        .filter(|p| p[0].is_finite() && p[1].is_finite() && p[0] >= 10.0 && p[0] <= 24_000.0)
        .take(MAX_POINTS)
        .map(|p| [p[0], p[1].clamp(-MAX_POINT_DB, MAX_POINT_DB)])
        .collect();
    v.sort_by(|a, b| a[0].total_cmp(&b[0]));
    v
}

/// The curve through `points` (cleaned, see [`clean_points`]) at `hz`, in dB: straight lines on a
/// log frequency axis, flat past the ends; 0 without points.
pub fn curve_db(points: &[[f32; 2]], hz: f32) -> f32 {
    let (Some(first), Some(last)) = (points.first(), points.last()) else { return 0.0 };
    if hz <= first[0] {
        return first[1];
    }
    if hz >= last[0] {
        return last[1];
    }
    for w in points.windows(2) {
        if let [a, b] = w
            && hz >= a[0]
            && hz <= b[0]
        {
            let span = (b[0] / a[0]).ln();
            let t = if span > 1e-9 { (hz / a[0]).ln() / span } else { 0.0 };
            return a[1] + (b[1] - a[1]) * t;
        }
    }
    last[1]
}

/// One channel's filter of one stage: sections in series and a flat gain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Curve {
    sections: [Coeffs; MAX_SECTIONS],
    len: usize,
    gain: f32,
}

impl Default for Curve {
    fn default() -> Self {
        Curve { sections: [Coeffs::IDENTITY; MAX_SECTIONS], len: 0, gain: 1.0 }
    }
}

impl Curve {
    /// The filter that follows `points` at `rate`, moved by `offset_db` as a whole.
    pub fn fit(points: &[[f32; 2]], offset_db: f32, rate: f32) -> Curve {
        let points = clean_points(points);
        let mut c = Curve::default();
        let offset = if offset_db.is_finite() { offset_db.clamp(-80.0, 24.0) } else { 0.0 };
        c.gain = 10f32.powf(offset / 20.0);
        if points.is_empty() || !rate.is_finite() || rate < 8000.0 {
            return c;
        }
        // The flat part of the curve is plain gain; the bands shape what is left.
        let centres: Vec<f32> = (0..BANDS).map(band_hz).filter(|f| *f < 0.45 * rate).collect();
        let targets: Vec<f32> = centres.iter().map(|f| curve_db(&points, *f)).collect();
        let (lo, hi) = targets.iter().fold((f32::MAX, f32::MIN), |(lo, hi), t| (lo.min(*t), hi.max(*t)));
        let flat = if lo <= hi { (lo + hi) / 2.0 } else { 0.0 };
        c.gain = 10f32.powf((offset + flat) / 20.0);
        let targets: Vec<f32> = targets.iter().map(|t| t - flat).collect();
        let mut gains = targets.clone();
        let design = |gains: &[f32]| -> Vec<Coeffs> {
            centres.iter().zip(gains).map(|(f, g)| Coeffs::design(FilterType::Peak, *f, BAND_Q, *g, rate)).collect()
        };
        for _ in 0..FIT_PASSES {
            let bands = design(&gains);
            for ((g, t), f) in gains.iter_mut().zip(&targets).zip(&centres) {
                let got: f32 = bands.iter().map(|b| b.magnitude_db(*f, rate)).sum();
                *g = (*g + 0.5 * (t - got)).clamp(-46.0, 46.0);
            }
        }
        for b in design(&gains) {
            c.push(b);
        }
        c
    }

    fn push(&mut self, s: Coeffs) {
        if let Some(slot) = self.sections.get_mut(self.len) {
            *slot = s;
            self.len += 1;
        }
    }

    /// Adds a 24 dB/oct high-pass (`low_pass` false) or low-pass at `hz`.
    pub fn cut(mut self, low_pass: bool, hz: f32, rate: f32) -> Curve {
        if !hz.is_finite() || !rate.is_finite() || hz < 10.0 || hz >= 0.49 * rate {
            return self;
        }
        let kind = if low_pass { FilterType::LowPass } else { FilterType::HighPass };
        for q in butterworth_qs(4) {
            self.push(Coeffs::design(kind, hz, *q, 0.0, rate));
        }
        self
    }

    /// The filter's gain at `hz`, in dB.
    pub fn magnitude_db(&self, hz: f32, rate: f32) -> f32 {
        let shape: f32 = self.sections.iter().take(self.len).map(|s| s.magnitude_db(hz, rate)).sum();
        shape + 20.0 * self.gain.max(1e-9).log10()
    }
}

/// The simulated output stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sim {
    pub curve: Curve,
    /// 1 = stereo as mixed, 0 = mono.
    pub width: f32,
}

/// What the monitor stage does, designed for one sample rate.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Plan {
    /// The sample rate the filters were designed for.
    pub rate: f32,
    pub sim: Option<Sim>,
    pub sim_on: bool,
    /// Left and right.
    pub ear: Option<[Curve; 2]>,
    pub ear_on: bool,
}

impl Plan {
    /// Nothing is on: the stage passes the mix as it is.
    pub fn is_idle(&self) -> bool {
        !(self.sim_on && self.sim.is_some()) && !(self.ear_on && self.ear.is_some())
    }
}

/// An ear correction as it is applied: both ears lowered together so neither is ever boosted
/// above the mix at any band (the listener turns the headphones up instead), so the stage cannot
/// clip the output.
pub fn ear_curves(left: &[[f32; 2]], right: &[[f32; 2]], rate: f32) -> [Curve; 2] {
    let (l, r) = (clean_points(left), clean_points(right));
    let top = (0..BANDS).map(band_hz).flat_map(|f| [curve_db(&l, f), curve_db(&r, f)]).fold(0.0f32, f32::max);
    // The fitted bands overshoot a little between centres: a small margin keeps the peak under 0 dB.
    let fit = |p: &[[f32; 2]]| {
        let c = Curve::fit(p, -top, rate);
        let peak = (0..BANDS * 4).map(|i| c.magnitude_db(20.0 * 2f32.powf(i as f32 / 12.0), rate)).fold(f32::MIN, f32::max);
        if peak > 0.0 { Curve { gain: c.gain * 10f32.powf(-peak / 20.0), ..c } } else { c }
    };
    [fit(&l), fit(&r)]
}

#[derive(Debug, Clone, Copy)]
struct Chain {
    sections: [[Biquad; MAX_SECTIONS]; 2],
    len: [usize; 2],
    gain: [f32; 2],
    width: f32,
    loaded: bool,
    /// How much of the stage is heard now (0..1) and where that is heading.
    mix: f32,
    target: f32,
}

impl Default for Chain {
    fn default() -> Self {
        Chain { sections: [[Biquad::default(); MAX_SECTIONS]; 2], len: [0; 2], gain: [1.0; 2], width: 1.0, loaded: false, mix: 0.0, target: 0.0 }
    }
}

impl Chain {
    fn load(&mut self, curves: Option<[&Curve; 2]>, width: f32, on: bool) {
        if let Some(curves) = curves {
            for (ch, c) in curves.iter().enumerate() {
                if let (Some(secs), Some(len), Some(gain)) = (self.sections.get_mut(ch), self.len.get_mut(ch), self.gain.get_mut(ch)) {
                    for (b, s) in secs.iter_mut().zip(c.sections.iter()) {
                        b.coeffs = *s;
                    }
                    *len = c.len.min(MAX_SECTIONS);
                    *gain = if c.gain.is_finite() { c.gain } else { 1.0 };
                }
            }
            self.width = if width.is_finite() { width.clamp(0.0, 1.0) } else { 1.0 };
            self.loaded = true;
        }
        self.target = if on && self.loaded { 1.0 } else { 0.0 };
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32], step: f32) {
        if self.mix <= 0.0 && self.target <= 0.0 {
            return;
        }
        let [sl, sr] = &mut self.sections;
        let (nl, nr) = (self.len[0], self.len[1]);
        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            let (mut yl, mut yr) = (*xl, *xr);
            if self.width < 1.0 {
                let (m, s) = (0.5 * (yl + yr), 0.5 * (yl - yr) * self.width);
                (yl, yr) = (m + s, m - s);
            }
            for b in sl.iter_mut().take(nl) {
                yl = b.process_sample(yl);
            }
            for b in sr.iter_mut().take(nr) {
                yr = b.process_sample(yr);
            }
            yl *= self.gain[0];
            yr *= self.gain[1];
            self.mix = if self.mix < self.target { (self.mix + step).min(self.target) } else { (self.mix - step).max(self.target) };
            *xl += self.mix * (yl - *xl);
            *xr += self.mix * (yr - *xr);
        }
        if self.mix <= 0.0 && self.target <= 0.0 {
            // Faded out: the next fade in starts from silence in the filters.
            for b in sl.iter_mut().chain(sr.iter_mut()) {
                b.reset();
            }
        }
    }
}

/// The audio thread's side of the stage: filter memory and fades. Plain data: making, loading
/// and running one never allocates.
#[derive(Debug, Clone, Copy, Default)]
pub struct State {
    sim: Chain,
    ear: Chain,
    step: f32,
}

impl State {
    /// Takes `plan`: the filters change at once and keep their memory; a stage turned on or off
    /// fades. Does not allocate.
    pub fn load(&mut self, plan: &Plan) {
        let rate = if plan.rate.is_finite() && plan.rate >= 8000.0 { plan.rate } else { 48_000.0 };
        self.step = 1.0 / (FADE_SECS * rate);
        self.sim.load(plan.sim.as_ref().map(|s| [&s.curve, &s.curve]), plan.sim.map_or(1.0, |s| s.width), plan.sim_on);
        self.ear.load(plan.ear.as_ref().map(|[l, r]| [l, r]), 1.0, plan.ear_on);
    }

    /// Nothing is heard of either stage: [`State::process`] would leave the audio as it is.
    pub fn is_idle(&self) -> bool {
        self.sim.mix <= 0.0 && self.sim.target <= 0.0 && self.ear.mix <= 0.0 && self.ear.target <= 0.0
    }

    /// Runs the stage on a stereo block in place (the shorter side's length). Does not allocate.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        self.sim.process(l, r, self.step);
        self.ear.process(l, r, self.step);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn tone(hz: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| 0.25 * (std::f32::consts::TAU * hz * i as f32 / RATE).sin()).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn a_fitted_curve_follows_its_points_between_the_bands_too() {
        let points = [[100.0, 0.0], [1000.0, -6.0], [4000.0, 9.0], [12000.0, 3.0]];
        let c = Curve::fit(&points, 0.0, RATE);
        let clean = clean_points(&points);
        for i in 0..100 {
            let hz = 40.0 * 2f32.powf(i as f32 * 0.085);
            let (want, got) = (curve_db(&clean, hz), c.magnitude_db(hz, RATE));
            assert!((want - got).abs() < 1.0, "{hz} Hz: wanted {want} dB, got {got} dB");
        }
    }

    #[test]
    fn a_flat_curve_is_plain_gain_and_no_points_is_nothing() {
        let c = Curve::fit(&[[100.0, -4.0], [8000.0, -4.0]], 0.0, RATE);
        assert!((c.magnitude_db(1000.0, RATE) + 4.0).abs() < 0.01);
        assert_eq!(Curve::fit(&[], 0.0, RATE).magnitude_db(1000.0, RATE), 0.0);
    }

    #[test]
    fn hostile_points_are_dropped_or_clamped() {
        let p = clean_points(&[[f32::NAN, 1.0], [1000.0, f32::INFINITY], [500.0, 900.0], [-3.0, 1.0], [2000.0, -2.0]]);
        assert_eq!(p, vec![[500.0, MAX_POINT_DB], [2000.0, -2.0]]);
        let c = Curve::fit(&[[1000.0, 5.0]], f32::NAN, f32::NAN);
        assert!(c.magnitude_db(1000.0, RATE).is_finite());
    }

    #[test]
    fn ear_curves_never_boost_above_the_mix() {
        let [l, r] = ear_curves(&[[250.0, 0.0], [4000.0, 20.0], [8000.0, 30.0]], &[[250.0, 0.0], [4000.0, 5.0]], RATE);
        for i in 0..120 {
            let hz = 20.0 * 2f32.powf(i as f32 / 12.0);
            assert!(l.magnitude_db(hz, RATE) <= 0.01 && r.magnitude_db(hz, RATE) <= 0.01, "{hz} Hz");
        }
        // The shape is kept: the left ear's 8 kHz sits 30 dB over its 250 Hz, and over the right's.
        assert!((l.magnitude_db(8000.0, RATE) - l.magnitude_db(250.0, RATE) - 30.0).abs() < 1.0);
        assert!((l.magnitude_db(8000.0, RATE) - r.magnitude_db(8000.0, RATE) - 25.0).abs() < 1.0);
    }

    #[test]
    fn an_idle_stage_leaves_the_audio_bit_for_bit() {
        let mut st = State::default();
        st.load(&Plan { rate: RATE, ear: Some(ear_curves(&[[1000.0, -9.0]], &[], RATE)), ..Plan::default() });
        assert!(st.is_idle());
        let (mut l, mut r) = (tone(440.0, 512), tone(660.0, 512));
        let (l0, r0) = (l.clone(), r.clone());
        st.process(&mut l, &mut r);
        assert_eq!((l, r), (l0, r0));
    }

    #[test]
    fn the_ear_stage_fades_in_shapes_each_side_and_fades_out_to_the_dry_mix() {
        let ear = ear_curves(&[[100.0, 0.0], [1000.0, 12.0], [10000.0, 0.0]], &[], RATE);
        let mut st = State::default();
        st.load(&Plan { rate: RATE, ear: Some(ear), ear_on: true, ..Plan::default() });
        let (mut l, mut r) = (tone(100.0, 48_000), tone(100.0, 48_000));
        let dry = l.clone();
        st.process(&mut l, &mut r);
        // The first sample is still the dry mix (no click), the settled left is 12 dB down at
        // 100 Hz (1 kHz is its 0 dB) and the flat right is lowered with it, by the same 12 dB.
        assert!((l[0] - dry[0]).abs() < 1e-3);
        let (tail_l, tail_r, tail_dry) = (&l[24_000..], &r[24_000..], &dry[24_000..]);
        assert!((20.0 * (rms(tail_l) / rms(tail_dry)).log10() + 12.0).abs() < 0.5);
        assert!((20.0 * (rms(tail_r) / rms(tail_dry)).log10() + 12.0).abs() < 0.1);
        st.load(&Plan { rate: RATE, ear: Some(ear), ear_on: false, ..Plan::default() });
        let (mut l, mut r) = (dry.clone(), dry.clone());
        st.process(&mut l, &mut r);
        assert!(st.is_idle());
        assert_eq!(l[4800..], dry[4800..]);
    }

    #[test]
    fn the_sim_stage_narrows_cuts_and_runs_before_the_ears() {
        let curve = Curve::fit(&[], 0.0, RATE).cut(false, 500.0, RATE).cut(true, 5000.0, RATE);
        assert!(curve.magnitude_db(60.0, RATE) < -60.0 && curve.magnitude_db(15_000.0, RATE) < -30.0);
        assert!(curve.magnitude_db(1500.0, RATE).abs() < 0.5);
        let mut st = State::default();
        st.load(&Plan { rate: RATE, sim: Some(Sim { curve, width: 0.0 }), sim_on: true, ..Plan::default() });
        // Opposite sides cancel in mono; equal sides pass.
        let (mut l, mut r): (Vec<f32>, Vec<f32>) = (tone(1500.0, 9600), tone(1500.0, 9600).iter().map(|v| -v).collect());
        st.process(&mut l, &mut r);
        assert!(rms(&l[4800..]) < 1e-4 && rms(&r[4800..]) < 1e-4);
        let (mut l, mut r) = (tone(1500.0, 9600), tone(1500.0, 9600));
        st.process(&mut l, &mut r);
        assert!((rms(&l[4800..]) / rms(&tone(1500.0, 4800)) - 1.0).abs() < 0.06);
        assert_eq!(l, r);
    }

    #[test]
    fn hostile_audio_never_sticks_in_the_filters() {
        let mut st = State::default();
        st.load(&Plan { rate: RATE, ear: Some(ear_curves(&[[1000.0, -6.0]], &[[1000.0, -6.0]], RATE)), ear_on: true, ..Plan::default() });
        let (mut l, mut r) = (vec![f32::NAN; 64], vec![f32::INFINITY; 64]);
        st.process(&mut l, &mut r);
        let (mut l, mut r) = (tone(440.0, 4800), tone(440.0, 4800));
        st.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|v| v.is_finite()));
        assert!(rms(&l[2400..]) > 0.01);
    }
}
