//! The Spectrum editor (Window › Editors › Spectrum, a tab of the lower dock beside the Vocal and
//! MIDI editors): the live frequency spectrum of every shown track in one graph, each curve in its
//! track's colour, so tracks that crowd the same range stand out.
//!
//! The audio side is `soundcraft_mix::spectrum_tap`: each shown track's mono sum after its fader,
//! mute and pan goes into a ring. Here each ring is analysed at CleanQ's analyser's pace and with
//! its dB range and fall-off (its frame, `module_graphs::feed_spectrum`; its peak hold, copied in
//! [`hold_peaks`]): a 4096-point Hann FFT about 30 times a second, each bin smoothed 0.7 old to
//! 0.3 new, a hold that rises at once and falls 35% of the way each drawn frame, -96..0 dBFS over
//! the graph's height. CleanQ itself is not touched. Unlike CleanQ's picture, each point of a
//! curve is then the power average of a third of an octave around it ([`HALF_BAND`]), integrated
//! over the spectrum drawn as straight lines between bins, so the curves are clean shapes with no
//! steps in the lows. The Tilt switch adds 4.5 dB an octave about 1 kHz to the drawn curves only;
//! readings stay untilted. At most [`FFTS_PER_FRAME`] curves are analysed in a frame, the rest wait their turn,
//! so 24 curves keep the 30 Hz pace at 60 frames a second. A hidden curve has no tap and no
//! analysis.
//!
//! Tracks are picked with chips along the top, grouped Tracks | Buses | Master, each group with its
//! own show/hide, so the graph has the full width. Every shown curve is a soft filled area under a
//! thin line in its colour (shades of their hue where colours collide, [`looks`]); pointing at a
//! curve or its chip brightens it, keeps the curves that overlap it ([`overlap_share`]) in colour,
//! names them, and greys the rest. The stereo scope (Stereo scope in the toolbar) draws each shown
//! track's left and right as a vectorscope at the right, in the same tones. What is shown is kept with the
//! session (`soundcraft_engine::cmd::spectrum`); see `docs/spectrum.md`.

use crate::SoundApp;
use crate::editor_kit::icons::{fill_strip, radial_bg};
use crate::editor_kit::palette::al;
use crate::editor_kit::rgb;
use crate::module_graphs::{SPEC_FFT, SPEC_INTERVAL, SPEC_SMOOTH, feed_spectrum};
use crate::theme::{Tokens, bold, mono, regular};
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::spectrum::Analyzer;
use soundcraft_engine::cmd::spectrum::{self as cmd, Curve, Section};
use soundcraft_mix::spectrum_tap::{MASTER_KEY, SpectrumTaps, track_key};
use std::collections::HashMap;

/// Curves analysed per frame at most (the rest wait their turn).
pub const FFTS_PER_FRAME: usize = 12;
pub const LOW_HZ: f32 = 20.0;
pub const HIGH_HZ: f32 = 20_000.0;
/// The graph's level range, dBFS (CleanQ's analyser's).
pub const TOP_DB: f32 = 0.0;
pub const BOTTOM_DB: f32 = -96.0;
/// Each point of a curve averages this many octaves either side of it (a third of an octave in
/// all; a sixth read as busier with eight curves over each other).
pub const HALF_BAND: f32 = 1.0 / 6.0;
/// The Tilt switch: dB an octave, about this frequency.
pub const TILT_DB_PER_OCTAVE: f32 = 4.5;
pub const TILT_PIVOT_HZ: f32 = 1000.0;

/// What Tilt adds to the drawn curve at `hz` (dB).
pub fn tilt_db(hz: f32) -> f32 {
    TILT_DB_PER_OCTAVE * (hz.max(1.0) / TILT_PIVOT_HZ).log2()
}

/// CleanQ's analyser hold, once per drawn frame, on its 0..1 scale (-96..0 dBFS):
/// `held = v > held ? v : held + (v - held) * 0.35` (a copy of `editors::cleanq`'s
/// `hold_peaks`, which is left as it is).
fn hold_peaks(held: &mut Vec<f32>, db: &[f32]) {
    if db.is_empty() {
        held.clear();
        return;
    }
    held.resize(db.len(), 0.0);
    for (h, d) in held.iter_mut().zip(db) {
        let v = if d.is_finite() { ((d + 96.0) / 96.0).clamp(0.0, 1.0) } else { 0.0 };
        *h = if v > *h { v } else { *h + (v - *h) * 0.35 };
    }
}

/// A hold's power per bin and the running integral of the spectrum drawn as straight lines
/// between bins (the trapezoids up to each bin), for band averages at any fractional bins.
fn integrate(held: &[f32], pw: &mut Vec<f64>, cum: &mut Vec<f64>) {
    pw.clear();
    pw.extend(held.iter().map(|h| 10f64.powf(f64::from(norm_db(*h)) / 10.0)));
    // The DC bin reads as its neighbour (CleanQ skips it too).
    if let Some(next) = pw.get(1).copied()
        && let Some(p) = pw.first_mut()
    {
        *p = next;
    }
    cum.clear();
    cum.push(0.0);
    for w in pw.windows(2) {
        if let ([a, b], Some(last)) = (w, cum.last().copied()) {
            cum.push(last + (a + b) * 0.5);
        }
    }
}

/// `∫ P` from bin 0 to fractional bin `x`.
fn cum_at(pw: &[f64], cum: &[f64], x: f64) -> f64 {
    let n = pw.len();
    if n < 2 {
        return 0.0;
    }
    let x = x.clamp(0.0, (n - 1) as f64);
    let k = (x.floor() as usize).min(n - 2);
    let f = x - k as f64;
    let (a, b) = (pw.get(k).copied().unwrap_or(0.0), pw.get(k + 1).copied().unwrap_or(0.0));
    cum.get(k).copied().unwrap_or(0.0) + f * a + f * f * 0.5 * (b - a)
}

/// The power average (dBFS) of the bins-as-lines between `f0` and `f1` Hz.
fn band_db(pw: &[f64], cum: &[f64], f0: f64, f1: f64, sr: f64) -> f32 {
    let n = pw.len();
    if n < 2 || sr <= 0.0 {
        return BOTTOM_DB;
    }
    let bin_hz = sr / SPEC_FFT as f64;
    let (a, b) = ((f0 / bin_hz).max(1.0), (f1 / bin_hz).max(1.0));
    let p = if b - a < 1e-6 {
        let k = (a.floor() as usize).min(n - 2);
        let f = a - k as f64;
        let (x, y) = (pw.get(k).copied().unwrap_or(0.0), pw.get(k + 1).copied().unwrap_or(0.0));
        x + (y - x) * f
    } else {
        (cum_at(pw, cum, b) - cum_at(pw, cum, a)) / (b - a)
    };
    if p > 1e-14 { (10.0 * p.log10()) as f32 } else { BOTTOM_DB - 48.0 }
}

/// The band [`HALF_BAND`] octaves either side of `hz`.
fn band(hz: f32) -> (f64, f64) {
    let k = 2f64.powf(f64::from(HALF_BAND));
    (f64::from(hz) / k, f64::from(hz) * k)
}
/// A ring that has not moved for this long reads silence (stopped, or the track not rendering).
const STALE_S: f64 = 0.25;
/// What a silent ring's spectrum is eased towards (dBFS).
const SILENT_DB: f32 = -144.0;
/// The frequency scale's strip under the plot (CleanQ's `SCALE_H`).
const SCALE_H: f32 = 14.0;
/// Samples the scope reads per curve, and every how many it draws a dot (256 dots, ~43 ms).
const SCOPE_READ: usize = 2048;
const SCOPE_STEP: usize = 8;

/// The plot's ground: CleanQ's graph (a radial lift towards the bottom centre), in both themes.
const BG_STOPS: [(f32, Color32); 3] = [(0.0, rgb(0x1A2530)), (0.45, rgb(0x12161B)), (1.0, rgb(0x0D1013))];
/// The lightest of that ground: what a curve's colour is checked against.
pub const PLOT_BG_LIGHTEST: Color32 = rgb(0x1A2530);
const FRAME: Color32 = rgb(0x2A313A);
const TEXT_FAINT: Color32 = rgb(0x6F7A86);
const TEXT_DIM: Color32 = rgb(0x8FA1B3);
const TEXT_BRIGHT: Color32 = rgb(0xEEF1F4);

/// One curve's picture in the scope: its dots as (mid, side) and its correlation (-1..1; `None`
/// in silence).
#[derive(Clone, Debug, Default)]
pub struct ScopeCurve {
    pub points: Vec<(f32, f32)>,
    pub correlation: Option<f32>,
}

/// One shown curve.
struct CurveState {
    slot: usize,
    /// The ring's write count when the slot was claimed: older samples are someone else's.
    claimed_at: usize,
    /// The write count `feed_spectrum` last analysed; the count last seen, and when it moved.
    seen: usize,
    moved: usize,
    moved_at: f64,
    /// When the spectrum last took a frame.
    spec_at: Option<f64>,
    /// The smoothed spectrum (dBFS per bin) and the peak hold over it (0..1 per bin).
    spec: Vec<f32>,
    held: Vec<f32>,
    /// The hold's power per bin and its running integral ([`integrate`]).
    pw: Vec<f64>,
    cum: Vec<f64>,
    /// What is drawn in each pixel column of the plot (0..1 over -96..0 dB, tilted with Tilt).
    cols: Vec<f32>,
}

/// The editor's state between frames (not saved: the shown curves are, in the session).
pub struct SpectrumView {
    /// The audio side's taps: the live player's (`soundcraft_playback::spectrum_taps`), or a
    /// test's own.
    pub taps: &'static SpectrumTaps,
    curves: HashMap<Curve, CurveState>,
    analyzer: Analyzer,
    samples: Vec<f32>,
    fresh: Vec<f32>,
    /// Where the round robin goes on from.
    next: usize,
    /// The pass the editor was last drawn in (the taps run only while it is).
    drawn_pass: Option<u64>,
    /// Curves analysed in the last frame drawn.
    pub analysed_last_frame: usize,
    /// Curves drawn in the last frame, in order (tracks, buses, then the master).
    pub drawn: Vec<Curve>,
    /// The curve the pointer is on (in the graph, the scope or the list), and the readout under
    /// it: (Hz, dBFS).
    pub hovered: Option<Curve>,
    pub readout: Option<(f32, f32)>,
    /// Where the graph's plot area was last drawn.
    pub plot: Option<Rect>,
    /// Each listed curve's look (colour, line) as last drawn.
    pub looks: HashMap<Curve, CurveLook>,
    /// The names written on the graph last frame, and where.
    pub labels: Vec<(Curve, Rect)>,
    /// What overlaps the curve pointed at, in words ("Vox overlaps Keys, Pad"), as last drawn.
    pub overlap_text: Option<String>,
    /// Rows the track chips took last frame (0: too many, one sideways-scrolling row).
    pub chip_rows: usize,
    /// The scope is on but there was no room for it last frame.
    scope_squeezed: bool,
    /// The stereo scope's pictures (only while it is shown), its zoom, and where it was drawn.
    pub scope: HashMap<Curve, ScopeCurve>,
    scope_gain: f32,
    pub scope_rect: Option<Rect>,
    /// The curve the pointer was on in the scope last frame (the graph highlights it too).
    scope_hover: Option<Curve>,
    left: Vec<f32>,
    right: Vec<f32>,
    rate: f64,
}

impl Default for SpectrumView {
    fn default() -> Self {
        SpectrumView {
            taps: soundcraft_playback::spectrum_taps(),
            curves: HashMap::new(),
            analyzer: Analyzer::default(),
            samples: Vec::new(),
            fresh: Vec::new(),
            next: 0,
            drawn_pass: None,
            analysed_last_frame: 0,
            drawn: Vec::new(),
            hovered: None,
            readout: None,
            plot: None,
            looks: HashMap::new(),
            labels: Vec::new(),
            overlap_text: None,
            chip_rows: 0,
            scope_squeezed: false,
            scope: HashMap::new(),
            scope_gain: 0.0,
            scope_rect: None,
            scope_hover: None,
            left: Vec::new(),
            right: Vec::new(),
            rate: 48_000.0,
        }
    }
}

/// dBFS of a 0..1 analyser reading.
pub fn norm_db(v: f32) -> f32 {
    BOTTOM_DB + v.clamp(0.0, 1.0) * (TOP_DB - BOTTOM_DB)
}

impl SpectrumView {
    /// The taps run while the editor was drawn in the last pass or this one: called each frame
    /// (`SoundApp::logic`), so a closed or covered editor stops them.
    pub fn tick(&mut self, pass: u64) {
        let open = self.drawn_pass.is_some_and(|p| p.saturating_add(1) >= pass);
        self.taps.set_active(open);
        if !open && !self.curves.is_empty() {
            self.release_all();
        }
    }

    fn release_all(&mut self) {
        for (_, c) in self.curves.drain() {
            self.taps.release(c.slot);
        }
        self.drawn.clear();
    }

    /// `c`'s level at `hz` (dBFS, its smoothed band there, never tilted), if `c` is shown.
    pub fn level(&self, c: Curve, hz: f32) -> Option<f32> {
        let s = self.curves.get(&c)?;
        let (f0, f1) = band(hz);
        Some(band_db(&s.pw, &s.cum, f0, f1, self.rate))
    }

    /// The peak hold of `c`, per bin (0..1), as CleanQ's analyser keeps its own.
    pub fn held(&self, c: Curve) -> Option<&[f32]> {
        self.curves.get(&c).map(|s| s.held.as_slice())
    }

    /// Claims taps for the curves to show, frees the others'.
    fn sync(&mut self, want: &[Curve], now: f64) {
        let gone: Vec<Curve> = self.curves.keys().filter(|c| !want.contains(c)).copied().collect();
        for c in gone {
            if let Some(s) = self.curves.remove(&c) {
                self.taps.release(s.slot);
            }
        }
        for c in want {
            if self.curves.contains_key(c) {
                continue;
            }
            let key = match c {
                Curve::Track(t) => track_key(*t),
                Curve::Master => MASTER_KEY,
            };
            let Some(slot) = self.taps.claim(key) else { continue };
            self.taps.set_on(slot, true);
            let at = self.taps.ring(slot).map_or(0, soundcraft_dsp::analysis::AnalysisRing::written);
            self.curves.insert(
                *c,
                CurveState {
                    slot,
                    claimed_at: at,
                    seen: at,
                    moved: at,
                    moved_at: now,
                    spec_at: None,
                    spec: Vec::new(),
                    held: Vec::new(),
                    pw: Vec::new(),
                    cum: Vec::new(),
                    cols: Vec::new(),
                },
            );
        }
    }

    /// Takes an analyser frame for each curve that is due one (CleanQ's pace), at most
    /// [`FFTS_PER_FRAME`] from where the round robin left off; a ring that stopped moving eases
    /// towards silence instead. Then the peak hold, every frame, as CleanQ's editor does.
    fn analyse(&mut self, order: &[Curve], rate: u32, now: f64) {
        self.rate = f64::from(rate.max(1));
        self.analysed_last_frame = 0;
        let count = order.len();
        let start = if count == 0 { 0 } else { self.next % count };
        let mut done = 0;
        for k in 0..count {
            let i = (start + k) % count;
            let Some(c) = order.get(i) else { continue };
            let Some(state) = self.curves.get_mut(c) else { continue };
            let Some(ring) = self.taps.ring(state.slot) else { continue };
            let written = ring.written();
            if written != state.moved {
                state.moved = written;
                state.moved_at = now;
            }
            let due = match state.spec_at {
                Some(t) => !now.is_finite() || now < t || now - t >= SPEC_INTERVAL - 1e-3,
                None => true,
            };
            if !due {
                continue;
            }
            if now - state.moved_at > STALE_S {
                // Silence: the same smoothing, towards nothing.
                state.spec.iter_mut().for_each(|s| *s = SPEC_SMOOTH * *s + (1.0 - SPEC_SMOOTH) * SILENT_DB);
                state.spec_at = Some(now);
                continue;
            }
            if written.saturating_sub(state.claimed_at) < SPEC_FFT || done >= FFTS_PER_FRAME {
                // Not a whole window of this curve's own audio yet, or no room this frame.
                continue;
            }
            feed_spectrum(&mut state.spec, &mut state.seen, ring, &mut self.samples, &mut self.fresh, &mut self.analyzer);
            state.spec_at = Some(now);
            done += 1;
            self.next = i + 1;
        }
        self.analysed_last_frame = done;
        for state in self.curves.values_mut() {
            hold_peaks(&mut state.held, &state.spec);
            integrate(&state.held, &mut state.pw, &mut state.cum);
        }
    }

    /// Reads each shown curve's hold into the plot's `w` pixel columns. Returns whether any is
    /// above the floor (worth another frame soon).
    fn columns(&mut self, w: usize, tilt: bool) -> bool {
        let rate = self.rate;
        let mut live = false;
        for s in self.curves.values_mut() {
            s.cols.clear();
            s.cols.extend((0..w).map(|px| {
                let hz = hz_at((px as f32 + 0.5) / w as f32);
                let (f0, f1) = band(hz);
                let db = band_db(&s.pw, &s.cum, f0, f1, rate);
                // Tilt moves what is there; the silent floor stays the floor.
                let db = if tilt && db > BOTTOM_DB { db + tilt_db(hz) } else { db };
                ((db - BOTTOM_DB) / (TOP_DB - BOTTOM_DB)).clamp(0.0, 1.0)
            }));
            live |= s.cols.iter().any(|v| *v > 0.0);
        }
        live
    }

    /// The scope's pictures: each shown curve's newest left and right (its taps switched to carry
    /// them), or none with the scope hidden.
    fn read_scope(&mut self, order: &[Curve], on: bool, now: f64) {
        self.scope.clear();
        for c in order {
            let Some(state) = self.curves.get(c) else { continue };
            self.taps.set_stereo(state.slot, on);
            if !on || now - state.moved_at > STALE_S {
                continue;
            }
            let Some((l, r)) = self.taps.stereo_rings(state.slot) else { continue };
            self.left.resize(SCOPE_READ, 0.0);
            self.right.resize(SCOPE_READ, 0.0);
            let n = l.latest(&mut self.left).min(r.latest(&mut self.right));
            let start = SCOPE_READ - n;
            let (mut lr, mut ll, mut rr) = (0.0f64, 0.0f64, 0.0f64);
            let mut points = Vec::with_capacity(SCOPE_READ / SCOPE_STEP);
            for (i, (a, b)) in self.left.iter().zip(&self.right).enumerate().skip(start) {
                lr += f64::from(a * b);
                ll += f64::from(a * a);
                rr += f64::from(b * b);
                if i % SCOPE_STEP == 0 {
                    points.push((0.5 * (a + b), 0.5 * (a - b)));
                }
            }
            let power = (ll * rr).sqrt();
            let correlation = (power > 1e-12).then(|| (lr / power).clamp(-1.0, 1.0) as f32);
            self.scope.insert(*c, ScopeCurve { points, correlation });
        }
    }
}

// ------------------------------------------------------------------------------ looks

/// How a curve's line is drawn: solid, or (only when more tracks share a hue than there are
/// shades, see [`looks`]) dashed or dotted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineStyle {
    Solid,
    Dashed,
    Dotted,
}

/// A curve's look: the colour it is drawn in (its track's, or a shade of its hue) and its line.
/// The list's swatch and the scope's dots draw the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CurveLook {
    pub color: Color32,
    pub style: LineStyle,
}

/// Colours this close (summed channel difference) are taken for the same colour.
const SAME_COLOR: i32 = 40;
/// Shades one hue is split into at most; past that the next tracks are dashed, then dotted.
pub const MAX_SHADES: usize = 4;
/// Every curve stands out from the plot's ground by at least this contrast (WCAG ratio, the
/// one for large text and graphics).
pub const MIN_CONTRAST: f32 = 3.0;
/// The lightest shade's luminance (a pale tint still showing its hue).
const MAX_LUMINANCE: f32 = 0.78;

fn near(a: Color32, b: Color32) -> bool {
    let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).abs();
    d(a.r(), b.r()) + d(a.g(), b.g()) + d(a.b(), b.b()) <= SAME_COLOR
}

/// WCAG relative luminance (0..1).
pub fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let s = f32::from(v) / 255.0;
        if s <= 0.040_45 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio of two colours (1..21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn to_hsl(c: Color32) -> (f32, f32, f32) {
    let (r, g, b) = (f32::from(c.r()) / 255.0, f32::from(c.g()) / 255.0, f32::from(c.b()) / 255.0);
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    let l = (mx + mn) * 0.5;
    if mx - mn < 1e-6 {
        return (0.0, 0.0, l);
    }
    let d = mx - mn;
    let s = if l > 0.5 { d / (2.0 - mx - mn) } else { d / (mx + mn) };
    let h = if mx == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn from_hsl(h: f32, s: f32, l: f32) -> Color32 {
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let ch = |t: f32| {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    Color32::from_rgb(ch(h + 1.0 / 3.0), ch(h), ch(h - 1.0 / 3.0))
}

/// `c`'s hue and saturation at relative luminance `y` (found by halving the lightness range).
fn at_luminance(c: Color32, y: f32) -> Color32 {
    let (h, s, _) = to_hsl(c);
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) * 0.5;
        if luminance(from_hsl(h, s, mid)) < y {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    from_hsl(h, s, (lo + hi) * 0.5)
}

/// The luminance a colour needs for [`MIN_CONTRAST`] against the plot's lightest ground.
fn min_luminance() -> f32 {
    MIN_CONTRAST * (luminance(PLOT_BG_LIGHTEST) + 0.05) - 0.05
}

/// `n` luminance steps from the darkest that still stands out from the plot to a pale tint,
/// spaced evenly in contrast (each step the same ratio lighter than the last).
fn ladder(n: usize) -> Vec<f32> {
    let (a, b) = (min_luminance() + 0.05, MAX_LUMINANCE + 0.05);
    if n <= 1 {
        return vec![(a * b).sqrt() - 0.05];
    }
    (0..n).map(|i| a * (b / a).powf(i as f32 / (n - 1) as f32) - 0.05).collect()
}

/// The looks of `rows` (curve, track colour) in list order. A colour no other row shares is drawn
/// as it is (lifted, if it is too dark to see on the plot, to the least that is). Rows whose
/// colours collide are drawn in shades of the first one's hue, spaced evenly from dark to pale
/// (each at least [`MIN_CONTRAST`] from the plot, and each step about 1.6 times the last's
/// luminance with four): the row whose own colour is nearest a shade keeps that one. Up to
/// [`MAX_SHADES`] rows of a hue are told apart by shade alone; the 5th to 8th are dashed and the
/// 9th on dotted, reusing the shades. The user's track colours are never changed.
pub fn looks(rows: &[(Curve, Color32)]) -> HashMap<Curve, CurveLook> {
    let mut out = HashMap::new();
    let mut done = vec![false; rows.len()];
    for i in 0..rows.len() {
        if done.get(i).copied().unwrap_or(true) {
            continue;
        }
        let Some((_, base)) = rows.get(i).copied() else { continue };
        let group: Vec<usize> =
            (i..rows.len()).filter(|j| !done.get(*j).copied().unwrap_or(true) && rows.get(*j).is_some_and(|(_, c)| near(*c, base))).collect();
        for j in &group {
            if let Some(d) = done.get_mut(*j) {
                *d = true;
            }
        }
        if group.len() == 1 {
            let color = if luminance(base) < min_luminance() { at_luminance(base, min_luminance()) } else { base };
            out.insert(rows[i].0, CurveLook { color, style: LineStyle::Solid });
            continue;
        }
        let steps = ladder(group.len().min(MAX_SHADES));
        // The first row takes the shade nearest its own colour; the rest the others, dark to pale.
        let own = luminance(base);
        let first = (0..steps.len()).min_by(|a, b| (steps[*a] - own).abs().total_cmp(&(steps[*b] - own).abs())).unwrap_or(0);
        let mut order: Vec<usize> = std::iter::once(first).chain((0..steps.len()).filter(|k| *k != first)).collect();
        order.truncate(steps.len());
        for (n, j) in group.iter().enumerate() {
            let step = order.get(n % order.len().max(1)).copied().unwrap_or(0);
            let style = match n / MAX_SHADES {
                0 => LineStyle::Solid,
                1 => LineStyle::Dashed,
                _ => LineStyle::Dotted,
            };
            let y = steps.get(step).copied().unwrap_or(0.3);
            if let Some((c, _)) = rows.get(*j) {
                out.insert(*c, CurveLook { color: at_luminance(base, y), style });
            }
        }
    }
    out
}

/// Draws `pts` in `style`.
fn stroke_line(p: &egui::Painter, pts: Vec<Pos2>, style: LineStyle, width: f32, color: Color32) {
    match style {
        LineStyle::Solid => {
            p.add(Shape::line(pts, Stroke::new(width, color)));
        }
        LineStyle::Dashed => {
            p.extend(Shape::dashed_line(&pts, Stroke::new(width, color), 7.0, 4.0));
        }
        LineStyle::Dotted => {
            p.extend(Shape::dotted_line(&pts, color, 4.0, width * 0.85));
        }
    }
}

/// Every row's look, in the list's order (so a curve keeps its look as others come and go).
fn all_looks(app: &SoundApp) -> HashMap<Curve, CurveLook> {
    let s = app.engine.session();
    let rows: Vec<(Curve, Color32)> = Section::ALL.iter().flat_map(|sec| cmd::section_curves(s, *sec)).map(|c| (c, identity(app, c).1)).collect();
    looks(&rows)
}

/// The curve's name and track colour (the master: a neutral light grey).
fn identity(app: &SoundApp, c: Curve) -> (String, Color32) {
    match c {
        Curve::Track(t) => app
            .engine
            .session()
            .track(t)
            .map_or_else(|| ("?".to_string(), Color32::GRAY), |t| (t.name.clone(), Color32::from_rgb(t.color[0], t.color[1], t.color[2]))),
        Curve::Master => ("Master".to_string(), rgb(0xD8DEE4)),
    }
}

fn look_of(app: &SoundApp, c: Curve) -> CurveLook {
    app.spectrum.looks.get(&c).copied().unwrap_or(CurveLook { color: Color32::GRAY, style: LineStyle::Solid })
}

// ------------------------------------------------------------------------------ geometry

/// The frequency at `t` (0..1) across the graph.
fn hz_at(t: f32) -> f32 {
    LOW_HZ * (HIGH_HZ / LOW_HZ).powf(t.clamp(0.0, 1.0))
}

/// x of frequency `hz` in `r`.
pub fn x_of(r: Rect, hz: f32) -> f32 {
    let t = (hz.max(LOW_HZ) / LOW_HZ).ln() / (HIGH_HZ / LOW_HZ).ln();
    r.left() + t.clamp(0.0, 1.0) * r.width()
}

fn hz_of(r: Rect, x: f32) -> f32 {
    hz_at((x - r.left()) / r.width().max(1.0))
}

/// y of level `db` (dBFS) in `r`.
pub fn y_of(r: Rect, db: f32) -> f32 {
    let t = (TOP_DB - db) / (TOP_DB - BOTTOM_DB);
    r.top() + t * r.height()
}

fn db_of(r: Rect, y: f32) -> f32 {
    TOP_DB - (y - r.top()) / r.height().max(1.0) * (TOP_DB - BOTTOM_DB)
}

/// "1.25 kHz", "80 Hz".
pub fn hz_text(hz: f32) -> String {
    if hz >= 1000.0 {
        let k = hz / 1000.0;
        if k >= 10.0 { format!("{k:.1} kHz") } else { format!("{k:.2} kHz") }
    } else {
        format!("{hz:.0} Hz")
    }
}

/// "+0.82", "-0.30".
pub fn correlation_text(c: f32) -> String {
    if c.abs() < 0.005 { "0.00".to_string() } else { format!("{c:+.2}") }
}

/// A framed panel in CleanQ's graph look: the radial ground and a hairline frame.
fn panel(p: &egui::Painter, r: Rect) {
    let centre = pos2(r.center().x, r.bottom() + r.height() * 0.2);
    radial_bg(p, r, centre, r.height().max(r.width() * 0.5) * 1.2, &BG_STOPS);
    p.rect_stroke(r, CornerRadius::same(4), Stroke::new(1.0, FRAME), StrokeKind::Inside);
}

// ------------------------------------------------------------------------------ the editor

/// A curve's tone while another is pointed at: the pointed-at one bright, the ones that overlap
/// it in their colours, the rest quiet grey. With nothing pointed at, every curve is `Rest`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Rest,
    Focused,
    Overlapping,
    Quiet,
}

/// The grey a curve fades to while another is pointed at and it is not in that one's way.
const QUIET: Color32 = rgb(0x4A4F5A);

/// The tone of `c` given the curve pointed at and the curves that overlap it.
pub fn tone(c: Curve, focus: Option<Curve>, overlaps: &[Curve]) -> Tone {
    match focus {
        None => Tone::Rest,
        Some(f) if f == c => Tone::Focused,
        Some(_) if overlaps.contains(&c) => Tone::Overlapping,
        Some(_) => Tone::Quiet,
    }
}

/// (line width, line alpha, fill alpha, colour) for a curve of `look` in `tone`.
fn paint_of(look: CurveLook, tone: Tone) -> (f32, f32, f32, Color32) {
    match tone {
        Tone::Rest => (1.1, 0.55, 0.15, look.color),
        Tone::Overlapping => (1.1, 0.60, 0.16, look.color),
        Tone::Focused => (1.8, 0.95, 0.30, look.color),
        Tone::Quiet => (1.1, 0.35, 0.05, QUIET),
    }
}

/// Only the top this many dB of the pointed-at curve count in [`overlap_share`] (below its own
/// peak less this, a curve's quiet skirts are in nobody's way, nor is anything else there).
pub const OVERLAP_RANGE_DB: f32 = 30.0;
/// A track is named as overlapping the pointed-at one when it covers at least this share of it.
pub const OVERLAP_SHARE: f32 = 0.4;

/// How much of curve `a` curve `b` covers, 0..1, from the drawn curves (columns of 0..1 over the
/// graph's -96..0 dB, so equal room per octave): each one's height above a floor
/// [`OVERLAP_RANGE_DB`] under `a`'s own peak, and of `a`'s, the part `b` reaches as well
/// (`Σ min(a, b) / Σ a`). A track as loud as `a` across `a`'s band covers it wholly; one a few dB
/// lower covers most of it; one elsewhere, or far below, little or none.
pub fn overlap_share(a: &[f32], b: &[f32]) -> f32 {
    let peak = a.iter().copied().fold(0.0f32, f32::max);
    let floor = (peak - OVERLAP_RANGE_DB / (TOP_DB - BOTTOM_DB)).max(0.0);
    let lift = |v: f32| (v - floor).max(0.0);
    let (mut both, mut all) = (0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (lift(*x), lift(*y));
        both += x.min(y);
        all += x;
    }
    if all > 1e-6 { both / all } else { 0.0 }
}

impl SpectrumView {
    /// How much of `a` `b` covers ([`overlap_share`]), as last drawn (0 when either is not shown).
    pub fn share_of(&self, a: Curve, b: Curve) -> f32 {
        match (self.curves.get(&a), self.curves.get(&b)) {
            (Some(x), Some(y)) => overlap_share(&x.cols, &y.cols),
            _ => 0.0,
        }
    }

    /// The shown curves that overlap `c` (see [`overlap_share`]), most first. The master is never
    /// one (it holds every track), nor, pointing at the master, is anything.
    pub fn overlaps(&self, c: Curve) -> Vec<Curve> {
        let Some(a) = self.curves.get(&c).filter(|_| c != Curve::Master) else { return Vec::new() };
        let mut v: Vec<(Curve, f32)> = self
            .drawn
            .iter()
            .filter(|o| **o != c && **o != Curve::Master)
            .filter_map(|o| self.curves.get(o).map(|s| (*o, overlap_share(&a.cols, &s.cols))))
            .filter(|(_, k)| *k >= OVERLAP_SHARE)
            .collect();
        v.sort_by(|x, y| y.1.total_cmp(&x.1));
        v.into_iter().map(|(o, _)| o).collect()
    }
}

/// Draws the editor into the dock tab: the bar (buttons, then the track chips), the graph across
/// the full width, and the scope at the right when it is on.
pub(crate) fn show(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    app.spectrum.drawn_pass = Some(ctx.cumulative_pass_nr());
    // Drawn now: the taps run from this frame on (`tick` turns them off once it stops drawing).
    app.spectrum.taps.set_active(true);
    let want = cmd::drawn(app.engine.session());
    app.spectrum.sync(&want, now);
    let rate = app.engine.session().sample_rate.hz();
    app.spectrum.analyse(&want, rate, now);
    app.spectrum.drawn = want.clone();
    app.spectrum.looks = all_looks(app);
    let scope_on = app.engine.session().edit.flag(cmd::SCOPE);

    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, t.panel_bg);
    let (bar_bottom, chip_hover) = bar(app, ui, full);
    ui.painter().line_segment([pos2(full.left(), bar_bottom), pos2(full.right(), bar_bottom)], Stroke::new(1.0, t.border));
    let area = Rect::from_min_max(pos2(full.left(), bar_bottom), full.max).shrink(6.0);
    // The scope: a square at the right, when there is room for it beside a usable graph.
    let side = area.height().min(area.width() * 0.38);
    let scope_rect =
        (scope_on && side >= 120.0 && area.width() - side >= 240.0).then(|| Rect::from_min_max(pos2(area.right() - side, area.top()), area.max));
    app.spectrum.scope_squeezed = scope_on && scope_rect.is_none();
    let graph_rect = scope_rect.map_or(area, |s| Rect::from_min_max(area.min, pos2(s.left() - 6.0, area.bottom())));
    // The scope's taps carry left and right only while it is drawn.
    app.spectrum.read_scope(&want, scope_rect.is_some(), now);
    let graph_hover = graph(app, ui, graph_rect, chip_hover.or(app.spectrum.scope_hover));
    let focus = graph_hover.or(chip_hover).or(app.spectrum.scope_hover);
    let overlaps = focus.map(|c| app.spectrum.overlaps(c)).unwrap_or_default();
    let scope_hover = scope_rect.and_then(|r| scope(app, ui, r, graph_hover.or(chip_hover), &overlaps));
    app.spectrum.scope_rect = scope_rect;
    app.spectrum.scope_hover = scope_hover;
    app.spectrum.hovered = graph_hover.or(scope_hover).or(chip_hover);
    if app.spectrum.curves.values().any(|s| s.cols.iter().any(|v| *v > 0.0)) || app.is_playing() {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}

/// A toolbar button: a small rounded box, filled while `on`. Its accessible name is `label`.
fn tool_button(ui: &mut Ui, r: Rect, label: &str, on: Option<bool>, tip: &str) -> Response {
    let t = Tokens::current();
    let resp = ui.interact(r, ui.id().with(("spectrum-tool", label)), Sense::click());
    resp.widget_info(|| match on {
        Some(on) => egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, label),
        None => egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label),
    });
    let lit = on == Some(true);
    let fill = if lit {
        t.accent_dark
    } else if resp.hovered() {
        t.button_hi
    } else {
        t.button
    };
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same(4), fill);
    p.rect_stroke(r, CornerRadius::same(4), Stroke::new(1.0, if lit { t.accent } else { t.button_border }), StrokeKind::Inside);
    p.text(r.center(), Align2::CENTER_CENTER, label, regular(11.5), if lit { Color32::WHITE } else { t.text });
    resp.on_hover_text(tip)
}

/// One thing in the chip rows.
enum Item {
    /// A group's name and its show/hide (`any`: something in it is on).
    Group(Section, bool),
    /// A track's chip: curve, name, look, its switch, drawn now.
    Chip(Curve, String, CurveLook, bool, bool),
    /// The thin line between groups.
    Sep,
}

const CHIP_H: f32 = 22.0;
const CHIP_GAP: f32 = 5.0;
/// Chip rows the bar wraps to at most; more than that scrolls sideways in one row.
pub const MAX_CHIP_ROWS: usize = 3;

fn item_width(ui: &Ui, it: &Item) -> f32 {
    let w = |s: &str, f: egui::FontId| ui.painter().layout_no_wrap(s.to_string(), f, Color32::WHITE).size().x;
    match it {
        Item::Group(sec, any) => w(&sec.label().to_uppercase(), bold(9.5)) + w(if *any { "Hide" } else { "Show" }, regular(10.5)) + 16.0,
        Item::Chip(_, name, ..) => w(name, regular(11.5)) + 31.0,
        Item::Sep => 9.0,
    }
}

/// The bar across the top of the editor: the buttons with the readout at the right, then the
/// tracks as chips, grouped Tracks | Buses | Master, wrapping onto up to [`MAX_CHIP_ROWS`] rows
/// (more scroll sideways in one). Returns where the bar ends and the curve whose chip the pointer
/// is on.
fn bar(app: &mut SoundApp, ui: &mut Ui, full: Rect) -> (f32, Option<Curve>) {
    let t = Tokens::current();
    let s = app.engine.session();
    let only = s.edit.flag(cmd::ONLY_SELECTED);
    let tilt = s.edit.flag(cmd::TILT);
    let scope_on = s.edit.flag(cmd::SCOPE);
    let on_n = Section::ALL.iter().flat_map(|sec| cmd::section_curves(s, *sec)).filter(|c| cmd::switched_on(s, *c)).count();
    let full_cap = on_n >= cmd::MAX_VISIBLE;
    let mut items: Vec<Item> = Vec::new();
    for sec in Section::ALL {
        let curves = cmd::section_curves(s, sec);
        if curves.is_empty() {
            continue;
        }
        if !items.is_empty() {
            items.push(Item::Sep);
        }
        let any = curves.iter().any(|c| if only { app.spectrum.drawn.contains(c) } else { cmd::switched_on(s, *c) });
        items.push(Item::Group(sec, any));
        for c in curves {
            let look = look_of(app, c);
            items.push(Item::Chip(c, identity(app, c).0, look, cmd::switched_on(s, c), app.spectrum.drawn.contains(&c)));
        }
    }
    let mut run: Option<(&str, serde_json::Value)> = None;

    // ---- the buttons
    let top = full.top() + 5.0;
    let mut x = full.left() + 8.0;
    let mut place = |ui: &Ui, label: &str, gap: f32| {
        let w = ui.painter().layout_no_wrap(label.to_string(), regular(11.5), t.text).size().x + 16.0;
        let r = Rect::from_min_size(pos2(x, top), vec2(w, CHIP_H));
        x += w + gap;
        r
    };
    let r = place(ui, "Show all", 2.0);
    if tool_button(ui, r, "Show all", None, "Show every track, bus and the master (up to the limit)").clicked() {
        run = Some(("spectrum.show_all", json!({})));
    }
    let r = place(ui, "Hide all", 8.0);
    if tool_button(ui, r, "Hide all", None, "Hide every curve").clicked() {
        run = Some(("spectrum.hide_all", json!({})));
    }
    let r = place(ui, "Only selected tracks", 2.0);
    if tool_button(ui, r, "Only selected tracks", Some(only), "Show just the tracks selected in the Edit window, following the selection").clicked() {
        run = Some(("spectrum.only_selected", json!({})));
    }
    let r = place(ui, "Tilt", 2.0);
    let tip = "Tilt the curves up 4.5 dB an octave about 1 kHz, so a mix reads level rather than sloping down to the highs (the drawing only: readings stay true)";
    if tool_button(ui, r, "Tilt", Some(tilt), tip).clicked() {
        run = Some(("spectrum.tilt", json!({})));
    }
    let r = place(ui, "Stereo scope", 8.0);
    let tip = if app.spectrum.scope_squeezed {
        "Stereo scope: the editor is too small to show it beside the graph; make the dock taller or wider"
    } else {
        "Show each shown track's left and right beside the graph: a vertical line is mono, a cloud is wide, a horizontal line is out of phase"
    };
    if tool_button(ui, r, "Stereo scope", Some(scope_on), tip).clicked() {
        run = Some(("spectrum.scope", json!({})));
    }
    readout(app, ui, Rect::from_min_max(pos2(x, top), pos2(full.right() - 8.0, top + CHIP_H)), tilt);

    // ---- the chips: wrapped into rows, or one sideways-scrolling row when there are too many
    let left = full.left() + 8.0;
    let width = (full.width() - 16.0).max(40.0);
    let widths: Vec<f32> = items.iter().map(|it| item_width(ui, it)).collect();
    let mut rows = 1usize;
    let mut cx = 0.0f32;
    for w in &widths {
        if cx > 0.0 && cx + w > width {
            rows += 1;
            cx = 0.0;
        }
        cx += w + CHIP_GAP;
    }
    let wrap = rows <= MAX_CHIP_ROWS;
    app.spectrum.chip_rows = if wrap { rows } else { 0 };
    let first = top + CHIP_H + 6.0;
    let mut hovered = None;
    let mut draw = |ui: &mut Ui, it: &Item, r: Rect| match it {
        Item::Sep => {
            ui.painter().line_segment([pos2(r.center().x, r.top() + 3.0), pos2(r.center().x, r.bottom() - 3.0)], Stroke::new(1.0, t.border));
        }
        Item::Group(sec, any) => {
            let p = ui.painter();
            let label = sec.label().to_uppercase();
            p.text(pos2(r.left() + 2.0, r.center().y), Align2::LEFT_CENTER, &label, bold(9.5), t.text_dim);
            let lw = p.layout_no_wrap(label, bold(9.5), t.text_dim).size().x;
            let word = if *any { "Hide" } else { "Show" };
            let br = Rect::from_min_max(pos2(r.left() + lw + 6.0, r.top() + 2.0), pos2(r.right(), r.bottom() - 2.0));
            let name = format!("{word} all {}", sec.label().to_lowercase());
            let resp = ui.interact(br, ui.id().with(("spectrum-group", sec.id())), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
            if resp.hovered() {
                ui.painter().rect_filled(br, CornerRadius::same(3), t.button_hi);
            }
            ui.painter().text(br.center(), Align2::CENTER_CENTER, word, regular(10.5), t.accent);
            let tip = match (*sec, *any) {
                (Section::Buses, false) => "Show the buses (Aux inputs and routing folders): what each bus sends into the mix",
                _ => "Show or hide this whole group",
            };
            if resp.on_hover_text(tip).clicked() {
                run = Some(("spectrum.show", json!({"section": sec.id(), "shown": !any})));
            }
        }
        Item::Chip(c, name, look, on, visible) => {
            let capped = !only && !on && full_cap;
            let enabled = !only && !capped;
            let resp = ui.interact(r, ui.id().with(("spectrum-chip", *c)), if enabled { Sense::click() } else { Sense::hover() });
            let checked = if only { *visible } else { *on };
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, checked, name));
            let p = ui.painter();
            let rr = CornerRadius::same((CHIP_H * 0.5) as u8);
            if resp.hovered() {
                p.rect_filled(r, rr, t.button_hi);
            }
            p.rect_stroke(r, rr, Stroke::new(1.0, if *visible { al(look.color, 0.55) } else { t.border }), StrokeKind::Inside);
            let dot = pos2(r.left() + 12.0, r.center().y);
            if look.style == LineStyle::Solid {
                p.circle_filled(dot, 4.5, if *visible { look.color } else { al(look.color, 0.3) });
            } else {
                // A dashed or dotted curve (past four of one colour): a ring, its line's mark.
                p.circle_stroke(dot, 4.0, Stroke::new(1.5, if *visible { look.color } else { al(look.color, 0.3) }));
            }
            p.text(
                pos2(r.left() + 21.0, r.center().y),
                Align2::LEFT_CENTER,
                name,
                regular(11.5),
                if *visible { t.text } else { al(t.text_dim, 0.7) },
            );
            if resp.hovered() && *visible {
                hovered = Some(*c);
            }
            let resp = if only {
                resp.on_hover_text("Only selected tracks is on: the Edit window's selection decides")
            } else if capped {
                resp.on_hover_text(cmd::cap_reason())
            } else {
                resp.on_hover_text(if *on { format!("Hide {name}") } else { format!("Show {name}") })
            };
            if enabled && resp.clicked() {
                run = Some((
                    "spectrum.show",
                    match c {
                        Curve::Track(id) => json!({"track": id.0, "shown": !on}),
                        Curve::Master => json!({"master": true, "shown": !on}),
                    },
                ));
            }
        }
    };
    let bottom = if wrap {
        let (mut cx, mut cy) = (left, first);
        for (it, w) in items.iter().zip(&widths) {
            if cx > left && cx + w > left + width {
                cx = left;
                cy += CHIP_H + 4.0;
            }
            draw(ui, it, Rect::from_min_size(pos2(cx, cy), vec2(*w, CHIP_H)));
            cx += w + CHIP_GAP;
        }
        cy + CHIP_H + 6.0
    } else {
        let strip = Rect::from_min_size(pos2(left, first), vec2(width, CHIP_H + 10.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(strip).id_salt("spectrum-chip-strip"));
        egui::ScrollArea::horizontal().id_salt("spectrum-chips").auto_shrink([false, true]).show(&mut child, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = CHIP_GAP;
                for (it, w) in items.iter().zip(&widths) {
                    let (r, _) = ui.allocate_exact_size(vec2(*w, CHIP_H), Sense::hover());
                    draw(ui, it, r);
                }
            });
        });
        strip.bottom() + 4.0
    };
    if let Some((id, p)) = run
        && let Err(e) = app.run(id, p)
    {
        app.ui.status = e;
    }
    (bottom.min(full.bottom()), hovered)
}

/// The readout at the right of the buttons: the frequency and true level under the pointer (and
/// whose curve), else the cap's note; on the plot's top right when the bar has no room.
fn readout(app: &mut SoundApp, ui: &mut Ui, room: Rect, tilt: bool) {
    let t = Tokens::current();
    let untilted = if tilt { "  (untilted)" } else { "" };
    let wanted = cmd::wanted(app.engine.session()).len();
    let text = match (app.spectrum.readout, app.spectrum.hovered) {
        (Some((hz, db)), Some(c)) => Some((format!("{}   {}   {db:.1} dBFS{untilted}", identity(app, c).0, hz_text(hz)), t.text)),
        (Some((hz, db)), None) => Some((format!("{}   {db:.1} dBFS{untilted}", hz_text(hz)), t.text_dim)),
        (None, None) if wanted > cmd::MAX_VISIBLE => {
            Some((format!("Showing the first {} of {wanted}: at most {} curves at once.", cmd::MAX_VISIBLE, cmd::MAX_VISIBLE), t.text_dim))
        }
        _ => None,
    };
    let Some((text, color)) = text else { return };
    let g = ui.painter().layout_no_wrap(text.clone(), mono(11.0), color);
    let r = if room.width() >= g.size().x {
        let at = pos2(room.right() - g.size().x, room.center().y - g.size().y * 0.5);
        let r = Rect::from_min_size(at, g.size());
        ui.painter().galley(at, g, color);
        Some(r)
    } else if let Some(plot) = app.spectrum.plot {
        let at = pos2(plot.right() - 8.0 - g.size().x, plot.top() + 6.0);
        let r = Rect::from_min_size(at, g.size());
        ui.painter().rect_filled(r.expand2(vec2(5.0, 2.0)), CornerRadius::same(3), al(rgb(0x0D1013), 0.85));
        ui.painter().galley(at, g, if color == t.text { TEXT_BRIGHT } else { TEXT_DIM });
        Some(r)
    } else {
        None
    };
    if let Some(r) = r {
        ui.interact(r, ui.id().with("spectrum-readout"), Sense::hover())
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &text));
    }
}

/// The graph in `r`, on CleanQ's ground and grid: every shown curve a soft filled area under a
/// thin line. Pointing at a curve (or its chip) brightens it, keeps the curves that overlap it in
/// colour and greys the rest, names them on the plot, and puts a crosshair under the pointer.
/// Returns the curve the pointer is on.
fn graph(app: &mut SoundApp, ui: &mut Ui, r: Rect, other_focus: Option<Curve>) -> Option<Curve> {
    let painter = ui.painter_at(r.intersect(ui.clip_rect()));
    panel(&painter, r);
    let plot = Rect::from_min_max(pos2(r.left() + 1.0, r.top() + 6.0), pos2(r.right() - 1.0, r.bottom() - SCALE_H - 1.0));
    if plot.width() < 60.0 || plot.height() < 40.0 {
        app.spectrum.plot = None;
        app.spectrum.labels.clear();
        app.spectrum.overlap_text = None;
        return None;
    }
    app.spectrum.plot = Some(plot);
    let p = ui.painter_at(plot.intersect(ui.clip_rect()));
    // ---- the grid: 1-2-5 per decade, labelled at the decades and (with room) the 2s and 5s
    let mut last_label = f32::MIN;
    for decade in 1..=4 {
        let base = 10f32.powi(decade);
        for mult in 1..=9 {
            let f = base * mult as f32;
            if !(LOW_HZ..=HIGH_HZ).contains(&f) {
                continue;
            }
            let x = x_of(plot, f).round();
            let major = mult == 1;
            p.line_segment(
                [pos2(x + 0.5, plot.top()), pos2(x + 0.5, plot.bottom())],
                Stroke::new(1.0, al(Color32::WHITE, if major { 0.10 } else { 0.04 })),
            );
            let room = x > plot.left() + 10.0 && x < plot.right() - 10.0 && x - last_label > 26.0;
            if room && (major || mult == 2 || mult == 5) {
                let label = if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") };
                painter.text(
                    pos2(x, plot.bottom() + 1.0 + SCALE_H * 0.5),
                    Align2::CENTER_CENTER,
                    label,
                    regular(9.0),
                    al(TEXT_FAINT, if major { 0.9 } else { 0.55 }),
                );
                last_label = x;
            }
        }
    }
    let step = if plot.height() < 110.0 { 24 } else { 12 };
    let mut db = 0;
    while db as f32 >= BOTTOM_DB {
        let y = y_of(plot, db as f32).round();
        p.line_segment(
            [pos2(plot.left(), y + 0.5), pos2(plot.right(), y + 0.5)],
            Stroke::new(1.0, al(Color32::WHITE, if db == 0 { 0.10 } else { 0.06 })),
        );
        if db < 0 && db as f32 > BOTTOM_DB {
            p.text(pos2(plot.left() + 4.0, y), Align2::LEFT_CENTER, format!("{db}"), regular(9.0), al(TEXT_FAINT, 0.7));
        }
        db -= step;
    }

    // ---- the curves' columns, the pointer, and which curve it is on
    let w = plot.width().max(1.0) as usize;
    let tilt = app.engine.session().edit.flag(cmd::TILT);
    app.spectrum.columns(w, tilt);
    let ys = |cols: &[f32]| -> Vec<Pos2> {
        cols.iter().enumerate().map(|(px, v)| pos2(plot.left() + px as f32 + 0.5, plot.bottom() - v * plot.height())).collect()
    };
    let pointer = ui.interact(r, ui.id().with("spectrum-graph"), Sense::hover()).hover_pos().filter(|q| plot.contains(*q));
    let order = app.spectrum.drawn.clone();
    let mut hover = None;
    if let Some(q) = pointer {
        let px = ((q.x - plot.left()) as usize).min(w.saturating_sub(1));
        let mut best = 12.0f32;
        for c in &order {
            if let Some(v) = app.spectrum.curves.get(c).and_then(|s| s.cols.get(px)).filter(|v| **v > 0.0) {
                let d = (plot.bottom() - v * plot.height() - q.y).abs();
                if d < best {
                    best = d;
                    hover = Some(*c);
                }
            }
        }
        // The true level under the pointer: the drawn one less what Tilt added there.
        let hz = hz_of(plot, q.x);
        app.spectrum.readout = Some((hz, db_of(plot, q.y) - if tilt { tilt_db(hz) } else { 0.0 }));
    } else {
        app.spectrum.readout = None;
    }
    let focus = hover.or(other_focus).filter(|c| order.contains(c));
    let overlaps = focus.map(|c| app.spectrum.overlaps(c)).unwrap_or_default();
    // Grey first, then the overlapping ones, the pointed-at one on top.
    let rank = |c: &Curve| match tone(*c, focus, &overlaps) {
        Tone::Quiet => 0,
        Tone::Rest | Tone::Overlapping => 1,
        Tone::Focused => 2,
    };
    let mut drawing = order.clone();
    drawing.sort_by_key(rank);
    for c in &drawing {
        let Some(state) = app.spectrum.curves.get(c) else { continue };
        if state.cols.iter().all(|v| *v <= 0.0) {
            continue;
        }
        let look = look_of(app, *c);
        let (width, line, fill, colour) = paint_of(look, tone(*c, focus, &overlaps));
        let pts = ys(&state.cols);
        fill_strip(&p, &pts, |_| plot.bottom(), al(colour, fill));
        stroke_line(&p, pts, look.style, width, al(colour, line));
    }

    // ---- names: the pointed-at curve's and those overlapping it, each at its peak where it
    // covers no other name (above the peak, else below, else beside), never off the plot
    app.spectrum.labels.clear();
    let named: Vec<Curve> = focus.into_iter().chain(overlaps.iter().copied()).collect();
    for c in &named {
        let Some(s) = app.spectrum.curves.get(c) else { continue };
        let Some((px, v)) = s.cols.iter().copied().enumerate().max_by(|a, b| a.1.total_cmp(&b.1)) else { continue };
        if v <= 0.04 {
            continue;
        }
        let focused = Some(*c) == focus;
        let look = look_of(app, *c);
        let g = p.layout_no_wrap(identity(app, *c).0, if focused { bold(11.0) } else { regular(10.5) }, look.color);
        let size = g.size() + vec2(8.0, 2.0);
        let at = pos2(plot.left() + px as f32, plot.bottom() - v * plot.height());
        let tries = [
            pos2(at.x - size.x * 0.5, at.y - size.y - 4.0),
            pos2(at.x - size.x * 0.5, at.y + 6.0),
            pos2(at.x + 6.0, at.y - size.y * 0.5),
            pos2(at.x - size.x - 6.0, at.y - size.y * 0.5),
            pos2(at.x - size.x * 0.5, at.y - 2.0 * size.y - 6.0),
        ];
        let fit = |min: Pos2| {
            let rr = Rect::from_min_size(min, size);
            rr.translate(vec2(
                (plot.left() + 2.0 - rr.left()).max(0.0) + (plot.right() - 2.0 - rr.right()).min(0.0),
                (plot.top() + 24.0 - rr.top()).max(0.0) + (plot.bottom() - 2.0 - rr.bottom()).min(0.0),
            ))
        };
        let free = tries.iter().map(|m| fit(*m)).find(|rr| !app.spectrum.labels.iter().any(|(_, o)| o.expand(2.0).intersects(*rr)));
        let Some(rr) = free.or_else(|| focused.then(|| fit(tries[0]))) else { continue };
        p.rect_filled(rr, CornerRadius::same(3), al(rgb(0x0D1013), 0.85));
        p.galley(rr.min + vec2(4.0, 1.0), g, look.color);
        app.spectrum.labels.push((*c, rr));
    }
    // ---- what overlaps the pointed-at curve, in words, at the plot's top left
    app.spectrum.overlap_text = focus.map(|c| {
        let names: Vec<String> = overlaps.iter().map(|o| identity(app, *o).0).collect();
        if names.is_empty() {
            format!("{}: nothing in its way", identity(app, c).0)
        } else {
            format!("{} overlaps {}", identity(app, c).0, names.join(", "))
        }
    });
    if let Some(text) = app.spectrum.overlap_text.clone() {
        let g = p.layout_no_wrap(text.clone(), bold(11.5), TEXT_BRIGHT);
        let at = pos2(plot.left() + 30.0, plot.top() + 4.0);
        let rr = Rect::from_min_size(at, g.size()).expand2(vec2(6.0, 3.0));
        p.rect_filled(rr, CornerRadius::same(3), al(rgb(0x0D1013), 0.85));
        p.galley(at, g, TEXT_BRIGHT);
        ui.interact(rr, ui.id().with("spectrum-overlaps"), Sense::hover())
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &text));
    }

    // ---- the crosshair, a dot where it meets the pointed-at curve, the empty states
    if let Some(q) = pointer {
        p.line_segment([pos2(q.x, plot.top()), pos2(q.x, plot.bottom())], Stroke::new(1.0, al(Color32::WHITE, 0.22)));
        p.line_segment([pos2(plot.left(), q.y), pos2(plot.right(), q.y)], Stroke::new(1.0, al(Color32::WHITE, 0.08)));
        if let Some(c) = hover {
            let px = ((q.x - plot.left()) as usize).min(w.saturating_sub(1));
            if let Some(v) = app.spectrum.curves.get(&c).and_then(|s| s.cols.get(px)) {
                p.circle_filled(pos2(q.x, plot.bottom() - v * plot.height()), 3.5, look_of(app, c).color);
            }
        }
    }
    let silent = app.spectrum.curves.values().all(|s| s.cols.iter().all(|v| *v <= 0.0));
    let empty = if order.is_empty() {
        Some("No curves shown: click tracks above to show them")
    } else if silent {
        Some("Play, or monitor an input, to see the curves")
    } else {
        None
    };
    if let Some(text) = empty {
        p.text(plot.center(), Align2::CENTER_CENTER, text, regular(12.0), TEXT_DIM);
    }
    hover
}

/// Draws one scope dot in `style`: filled for solid curves, a small cross for dashed, a ring for
/// dotted.
fn scope_dot(p: &egui::Painter, at: Pos2, style: LineStyle, color: Color32, big: bool) {
    let r = if big { 1.8 } else { 1.3 };
    match style {
        LineStyle::Solid => {
            p.circle_filled(at, r, color);
        }
        LineStyle::Dashed => {
            let d = r + 0.6;
            p.line_segment([at + vec2(-d, -d), at + vec2(d, d)], Stroke::new(1.0, color));
            p.line_segment([at + vec2(-d, d), at + vec2(d, -d)], Stroke::new(1.0, color));
        }
        LineStyle::Dotted => {
            p.circle_stroke(at, r + 0.6, Stroke::new(1.0, color));
        }
    }
}

/// The scope's field in its panel `r` (above the correlation strip).
pub fn scope_field(r: Rect) -> Rect {
    Rect::from_min_max(r.min + vec2(14.0, 14.0), pos2(r.right() - 14.0, r.bottom() - 34.0))
}

/// The stereo scope in `r`, in the graph's look: guides, every shown curve's dots on one zoom,
/// the focused one on top, and the focused curve's correlation on a strip underneath. Returns the
/// curve the pointer is on.
fn scope(app: &mut SoundApp, ui: &mut Ui, r: Rect, focus_in: Option<Curve>, overlaps_in: &[Curve]) -> Option<Curve> {
    let p = ui.painter_at(r.intersect(ui.clip_rect()));
    panel(&p, r);
    let field = scope_field(r);
    let radius = (field.width().min(field.height()) * 0.5 - 2.0).max(4.0);
    let centre = field.center();
    let faint = Stroke::new(1.0, al(Color32::WHITE, 0.06));
    p.circle_stroke(centre, radius, Stroke::new(1.0, al(Color32::WHITE, 0.12)));
    p.circle_stroke(centre, radius * 0.5, faint);
    p.line_segment([centre - vec2(0.0, radius), centre + vec2(0.0, radius)], faint);
    p.line_segment([centre - vec2(radius, 0.0), centre + vec2(radius, 0.0)], faint);
    let d = radius * std::f32::consts::FRAC_1_SQRT_2;
    for sx in [-1.0f32, 1.0] {
        p.line_segment([centre + vec2(sx * d, -d), centre - vec2(sx * d, -d)], faint);
    }
    let label = |at: Pos2, anchor: Align2, text: &str| {
        p.text(at, anchor, text, bold(10.0), TEXT_DIM);
    };
    label(centre + vec2(-d - 2.0, -d - 2.0), Align2::RIGHT_BOTTOM, "L");
    label(centre + vec2(d + 2.0, -d - 2.0), Align2::LEFT_BOTTOM, "R");
    label(centre - vec2(0.0, radius + 2.0), Align2::CENTER_BOTTOM, "M");
    label(centre + vec2(radius + 3.0, 0.0), Align2::LEFT_CENTER, "S");
    label(centre - vec2(radius + 3.0, 0.0), Align2::RIGHT_CENTER, "S");
    // One zoom for every curve: out at once (nothing leaves the circle), back in slowly.
    let peak = app.spectrum.scope.values().flat_map(|c| c.points.iter()).fold(0.0f32, |m, (mid, side)| m.max(mid.abs()).max(side.abs()));
    let want = 0.9 / peak.max(0.02);
    let g = app.spectrum.scope_gain;
    app.spectrum.scope_gain = if g <= 0.0 || want < g { want } else { g + (want - g) * 0.05 };
    let gain = app.spectrum.scope_gain;
    let place = |(mid, side): (f32, f32)| {
        let v = vec2(-side * gain, -mid * gain);
        centre + if v.length() > 1.0 { v.normalized() } else { v } * radius
    };
    // The curve under the pointer: the one with a dot nearest it, within 6 px.
    let pointer = ui.interact(r, ui.id().with("spectrum-scope"), Sense::hover()).hover_pos().filter(|q| field.contains(*q));
    let mut hover = None;
    if let Some(q) = pointer {
        let mut best = 6.0f32;
        for (c, sc) in &app.spectrum.scope {
            for pt in &sc.points {
                let dist = place(*pt).distance(q);
                if dist < best {
                    best = dist;
                    hover = Some(*c);
                }
            }
        }
    }
    let focus = hover.or(focus_in).filter(|c| app.spectrum.drawn.contains(c));
    // The same tones as the graph: grey first, the overlapping ones, the pointed-at one on top.
    let overlaps =
        if hover.is_some() && hover != focus_in { focus.map(|c| app.spectrum.overlaps(c)).unwrap_or_default() } else { overlaps_in.to_vec() };
    let mut order = app.spectrum.drawn.clone();
    order.sort_by_key(|c| match tone(*c, focus, &overlaps) {
        Tone::Quiet => 0,
        Tone::Rest | Tone::Overlapping => 1,
        Tone::Focused => 2,
    });
    for c in &order {
        let Some(sc) = app.spectrum.scope.get(c) else { continue };
        let look = look_of(app, *c);
        let t = tone(*c, focus, &overlaps);
        let (_, line, _, colour) = paint_of(look, t);
        let color = al(colour, (line + 0.15).min(1.0));
        for pt in sc.points.iter().filter(|(m, s)| *m != 0.0 || *s != 0.0) {
            scope_dot(&p, place(*pt), look.style, color, t == Tone::Focused);
        }
    }
    // ---- the correlation strip: -1 on the left, +1 on the right, the focused curve's mark
    let bar = Rect::from_min_size(pos2(r.left() + 26.0, r.bottom() - 26.0), vec2((r.width() - 52.0).max(10.0), 5.0));
    p.rect_filled(bar, CornerRadius::same(2), al(Color32::WHITE, 0.06));
    p.rect_filled(Rect::from_min_max(bar.min, pos2(bar.center().x, bar.bottom())), CornerRadius::same(2), al(rgb(0xFFD166), 0.10));
    p.line_segment([pos2(bar.center().x, bar.top() - 2.0), pos2(bar.center().x, bar.bottom() + 2.0)], Stroke::new(1.0, al(Color32::WHITE, 0.25)));
    p.text(pos2(bar.left() - 4.0, bar.center().y), Align2::RIGHT_CENTER, "-1", regular(9.0), TEXT_FAINT);
    p.text(pos2(bar.right() + 4.0, bar.center().y), Align2::LEFT_CENTER, "+1", regular(9.0), TEXT_FAINT);
    let reading = focus.and_then(|c| app.spectrum.scope.get(&c).map(|sc| (c, sc.correlation)));
    let (text, colour) = match reading {
        Some((c, Some(k))) => {
            let x = bar.center().x + k.clamp(-1.0, 1.0) * bar.width() * 0.5;
            let col = look_of(app, c).color;
            p.rect_filled(Rect::from_center_size(pos2(x, bar.center().y), vec2(3.0, 11.0)), CornerRadius::same(1), col);
            (format!("{}  correlation {}", identity(app, c).0, correlation_text(k)), TEXT_BRIGHT)
        }
        Some((c, None)) => (format!("{}  silent", identity(app, c).0), TEXT_DIM),
        None if app.spectrum.scope.is_empty() => ("Nothing playing".to_string(), TEXT_FAINT),
        None => ("Point at a track for its correlation".to_string(), TEXT_FAINT),
    };
    p.text(pos2(r.center().x, bar.bottom() + 9.0), Align2::CENTER_CENTER, text, regular(10.0), colour);
    hover
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module_graphs::{GraphKind, Streams};
    use soundcraft_dsp::analysis::Analysis;

    /// A signal reads the same in CleanQ's analyser and here: both pipelines fed one signal at
    /// the same pace end with the same hold, bin for bin, and so the same picture.
    #[test]
    fn a_signal_reads_the_same_as_in_cleanq() {
        let taps: &'static SpectrumTaps = Box::leak(Box::new(SpectrumTaps::new()));
        let mut view = SpectrumView { taps, ..SpectrumView::default() };
        let c = Curve::Track(soundcraft_model::TrackId(7));
        view.sync(&[c], 0.0);
        let slot = taps.slot_of(track_key(soundcraft_model::TrackId(7))).unwrap();
        let cq = Analysis::new();
        let (mut streams, mut held) = (Streams::default(), Vec::new());
        // A chord over a quiet noise floor, with a tilt in its own levels.
        let mut seed = 1u32;
        let mut x = |n: usize| -> f32 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            let t = n as f32 / 48_000.0;
            0.3 * (std::f32::consts::TAU * 110.0 * t).sin()
                + 0.1 * (std::f32::consts::TAU * 880.0 * t).sin()
                + 0.02 * (std::f32::consts::TAU * 5000.0 * t).sin()
                + 0.002 * noise
        };
        let mut n = 0;
        // A whole window before the first frame, so both start from the same first analysis.
        let first: Vec<f32> = (0..SPEC_FFT)
            .map(|_| {
                n += 1;
                x(n)
            })
            .collect();
        taps.ring(slot).unwrap().push_slice(&first);
        cq.post.push_slice(&first);
        for frame in 0..90 {
            let block: Vec<f32> = (0..800)
                .map(|_| {
                    n += 1;
                    x(n)
                })
                .collect();
            taps.ring(slot).unwrap().push_slice(&block);
            cq.post.push_slice(&block);
            let now = f64::from(frame) / 60.0;
            view.analyse(&[c], 48_000, now);
            streams.feed(&cq, GraphKind::CleanQ, now);
            hold_peaks(&mut held, &streams.post);
        }
        let ours = view.held(c).unwrap();
        assert_eq!(ours.len(), held.len());
        let worst = ours.iter().zip(&held).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "the same hold, bin for bin (worst {worst})");
        // Smoothed: each tone stands well clear of the space between, in the same dB scale.
        let lv = |hz: f32| view.level(c, hz).unwrap();
        assert!(lv(110.0) > lv(311.0) + 15.0 && lv(880.0) > lv(2100.0) + 15.0, "{} {} {} {}", lv(110.0), lv(311.0), lv(880.0), lv(2100.0));
        assert!(lv(110.0) > lv(880.0) && lv(880.0) > lv(5000.0), "louder tones read louder");
        assert!(lv(110.0) < -10.0 && lv(110.0) > -22.0, "a -10.5 dBFS tone, a third of an octave wide: {}", lv(110.0));
        let at_5k = lv(5000.0);
        // No stair steps in the lows: across 20-120 Hz on an 800-pixel graph the line moves a
        // little from each column to the next, and seldom stands still.
        view.columns(800, false);
        let cols = &view.curves[&c].cols;
        let lows: Vec<f32> = cols
            .iter()
            .copied()
            .take_while({
                let mut px = 0;
                move |_| {
                    px += 1;
                    hz_at(px as f32 / 800.0) < 120.0
                }
            })
            .map(norm_db)
            .collect();
        assert!(lows.len() > 100);
        let jumps = lows.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        let flat = lows.windows(2).filter(|w| w[0] == w[1]).count();
        assert!(jumps < 4.0, "smooth in the lows (the tone's skirt is the steepest part): largest step {jumps} dB");
        assert!(flat * 10 < lows.len(), "no flat treads: {flat} of {}", lows.len());
        // Tilt lifts the highs and lowers the lows about 1 kHz, the drawing only.
        let plain = view.curves[&c].cols.clone();
        view.columns(800, true);
        let tilted = &view.curves[&c].cols;
        let px_1k = (0..800)
            .min_by(|a, b| (hz_at((*a as f32 + 0.5) / 800.0) / 1000.0).ln().abs().total_cmp(&(hz_at((*b as f32 + 0.5) / 800.0) / 1000.0).ln().abs()))
            .unwrap();
        assert!((tilted[px_1k] - plain[px_1k]).abs() < 0.01, "no change at the pivot");
        assert!((tilt_db(2000.0) - 4.5).abs() < 1e-4 && (tilt_db(250.0) + 9.0).abs() < 1e-4);
        assert!((view.level(c, 5000.0).unwrap() - at_5k).abs() < 1e-6, "readings stay untilted");
    }

    #[test]
    fn shades_stand_apart_and_out_from_the_plot() {
        // Four tracks of one red, one of a near blue pair, a lone dark navy.
        let rows: Vec<(Curve, Color32)> = vec![
            (Curve::Track(soundcraft_model::TrackId(1)), Color32::from_rgb(200, 60, 60)),
            (Curve::Track(soundcraft_model::TrackId(2)), Color32::from_rgb(204, 58, 63)),
            (Curve::Track(soundcraft_model::TrackId(3)), Color32::from_rgb(200, 60, 60)),
            (Curve::Track(soundcraft_model::TrackId(4)), Color32::from_rgb(198, 62, 60)),
            (Curve::Track(soundcraft_model::TrackId(5)), Color32::from_rgb(20, 24, 70)),
            (Curve::Track(soundcraft_model::TrackId(6)), Color32::from_rgb(60, 120, 220)),
        ];
        let l = looks(&rows);
        let red: Vec<Color32> = (1..=4).map(|i| l[&Curve::Track(soundcraft_model::TrackId(i))].color).collect();
        for (i, a) in red.iter().enumerate() {
            assert!(contrast(*a, PLOT_BG_LIGHTEST) >= MIN_CONTRAST - 0.05, "shade {i} stands out from the plot");
            for b in red.iter().skip(i + 1) {
                assert!(contrast(*a, *b) >= 1.45, "shades apart: {a:?} {b:?}");
            }
            // Still red.
            let (h, s, _) = to_hsl(*a);
            assert!(!(0.05..=0.95).contains(&h) && s > 0.3, "{a:?} keeps the hue");
        }
        assert!(red.iter().all(|_| true) && l.values().all(|k| k.style == LineStyle::Solid), "four share a hue on shades alone");
        // The lone colour is its own, lifted only as far as the plot needs.
        let navy = l[&Curve::Track(soundcraft_model::TrackId(5))].color;
        assert!(contrast(navy, PLOT_BG_LIGHTEST) >= MIN_CONTRAST - 0.05);
        assert_eq!(l[&Curve::Track(soundcraft_model::TrackId(6))].color, Color32::from_rgb(60, 120, 220));
        // Past four of one hue: dashed, then dotted.
        let many: Vec<(Curve, Color32)> = (0..10u64).map(|i| (Curve::Track(soundcraft_model::TrackId(i)), Color32::RED)).collect();
        let l = looks(&many);
        let style = |i: u64| l[&Curve::Track(soundcraft_model::TrackId(i))].style;
        assert_eq!([style(3), style(4), style(7), style(8)], [LineStyle::Solid, LineStyle::Dashed, LineStyle::Dashed, LineStyle::Dotted]);
    }
}
