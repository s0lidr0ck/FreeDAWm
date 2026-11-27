//! The Spectrum editor (Window › Editors › Spectrum, a tab of the lower dock beside the Vocal and
//! MIDI editors): the live frequency spectrum of every shown track in one graph, each curve in its
//! track's colour with its name, so tracks that crowd the same range stand out.
//!
//! The audio side is `soundcraft_mix::spectrum_tap`: each shown track's mono sum after its fader,
//! mute and pan goes into a ring. Here, on the UI thread, each ring's newest samples are analysed
//! (Hann window, FFT), averaged into 1/3-octave bands on a fixed log grid from 20 Hz to 20 kHz,
//! and eased (fast up, a steady fall) so the curves read steadily. At most [`FFTS_PER_FRAME`] curves
//! are analysed per frame, round robin, so forty tracks cost no more per frame than eight. A hidden
//! curve has no tap and no analysis.
//!
//! The stereo scope (the list's Stereo scope switch) sits beside the graph: each shown track's
//! left and right as they reach the mix, as a vectorscope after Sprawl's (mid up, side across,
//! about 40 ms of sound), its dots in the curve's colour and line, all on one shared zoom so the
//! tracks' widths and levels compare. Its taps carry left and right only while it is shown. What
//! is shown is kept with the session
//! (`soundcraft_engine::cmd::spectrum`); see `docs/spectrum.md`.

use crate::SoundApp;
use crate::theme::{Tokens, mono, regular};
use egui::{Align2, Color32, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::spectrum::Analyzer;
use soundcraft_engine::cmd::spectrum::{self as cmd, Curve};
use soundcraft_mix::spectrum_tap::{MASTER_KEY, SpectrumTaps, track_key};
use std::collections::HashMap;

/// Curves analysed per frame at most (the rest wait their turn).
pub const FFTS_PER_FRAME: usize = 8;
/// Points across the graph, log-spaced from [`LOW_HZ`] to [`HIGH_HZ`].
pub const POINTS: usize = 240;
pub const LOW_HZ: f32 = 20.0;
pub const HIGH_HZ: f32 = 20_000.0;
/// The graph's level range, dB (per 1/3-octave band, a power average of its bins).
pub const TOP_DB: f32 = 0.0;
pub const BOTTOM_DB: f32 = -96.0;
/// Below the graph: where a silent curve rests.
const FLOOR_DB: f32 = -140.0;
/// Each point averages the bins within this many octaves either side (1/3 octave in all: about
/// the width of a critical band in the mids, wide enough that the curves read as shapes).
const HALF_BAND: f32 = 1.0 / 6.0;
/// Seconds a curve takes to rise (to 63%), and how fast it falls (dB a second): a kick drawn on
/// one beat is still there, lower, at the next.
const RISE_S: f32 = 0.04;
const FALL_DB_PER_S: f32 = 30.0;
/// A ring that has not moved for this long reads silence (stopped, or the track not rendering).
const STALE_S: f64 = 0.25;
/// The list's width.
const LIST_W: f32 = 200.0;
/// Samples the scope reads per curve, and every how many it draws a dot (256 dots, ~43 ms).
const SCOPE_READ: usize = 2048;
const SCOPE_STEP: usize = 8;

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
    /// The ring's write count at the last analysis, and when it last moved.
    seen: usize,
    moved_at: f64,
    /// The latest analysis, and what is drawn (eased towards it), dB per grid point.
    target: Vec<f32>,
    level: Vec<f32>,
}

/// The editor's state between frames (not saved: the shown curves are, in the session).
pub struct SpectrumView {
    /// The audio side's taps: the live player's (`soundcraft_playback::spectrum_taps`), or a
    /// test's own.
    pub taps: &'static SpectrumTaps,
    curves: HashMap<Curve, CurveState>,
    analyzer: Analyzer,
    samples: Vec<f32>,
    mags: Vec<f32>,
    /// The band averaging for the FFT size and rate it was made for.
    bands: Option<(usize, u32, Vec<Band>)>,
    /// Where the round robin goes on from.
    next: usize,
    /// The pass the editor was last drawn in (the taps run only while it is).
    drawn_pass: Option<u64>,
    /// Curves analysed in the last frame drawn.
    pub analysed_last_frame: usize,
    /// Curves drawn in the last frame, in order (tracks as listed, then the master).
    pub drawn: Vec<Curve>,
    /// The curve the pointer is on (in the graph or the list), and the readout under it.
    pub hovered: Option<Curve>,
    pub readout: Option<(f32, f32)>,
    /// Where the graph's plot area was last drawn.
    pub plot: Option<Rect>,
    /// Each listed curve's line (colour and style) as last drawn.
    pub looks: HashMap<Curve, CurveLook>,
    /// The stereo scope's pictures (only while it is shown), its zoom, and where it was drawn.
    pub scope: HashMap<Curve, ScopeCurve>,
    scope_gain: f32,
    pub scope_rect: Option<Rect>,
    /// The curve the pointer was on in the scope last frame (the graph highlights it too).
    scope_hover: Option<Curve>,
    left: Vec<f32>,
    right: Vec<f32>,
    last_time: Option<f64>,
}

impl Default for SpectrumView {
    fn default() -> Self {
        SpectrumView {
            taps: soundcraft_playback::spectrum_taps(),
            curves: HashMap::new(),
            analyzer: Analyzer::default(),
            samples: Vec::new(),
            mags: Vec::new(),
            bands: None,
            next: 0,
            drawn_pass: None,
            analysed_last_frame: 0,
            drawn: Vec::new(),
            hovered: None,
            readout: None,
            plot: None,
            looks: HashMap::new(),
            scope: HashMap::new(),
            scope_gain: 0.0,
            scope_rect: None,
            scope_hover: None,
            left: Vec::new(),
            right: Vec::new(),
            last_time: None,
        }
    }
}

/// The bins one grid point averages: `lo..=hi`, or (when the band is narrower than a bin) the
/// two bins either side of it, weighted.
#[derive(Clone, Copy, Debug)]
enum Band {
    Range(usize, usize),
    Between(usize, f32),
}

/// The frequency of grid point `i`.
pub fn point_hz(i: usize) -> f32 {
    let t = i as f32 / (POINTS - 1) as f32;
    LOW_HZ * (HIGH_HZ / LOW_HZ).powf(t)
}

fn bands_for(n: usize, rate: u32) -> Vec<Band> {
    let per_bin = rate as f32 / n as f32;
    let last = n / 2;
    (0..POINTS)
        .map(|i| {
            let f = point_hz(i);
            let lo = (f * 2f32.powf(-HALF_BAND) / per_bin).ceil() as usize;
            let hi = ((f * 2f32.powf(HALF_BAND) / per_bin).floor() as usize).min(last);
            if lo <= hi {
                Band::Range(lo, hi)
            } else {
                let x = (f / per_bin).min(last as f32);
                let k = (x.floor() as usize).min(last.saturating_sub(1));
                Band::Between(k, x - k as f32)
            }
        })
        .collect()
}

/// Analysis size for a sample rate: about 85 ms of audio (4096 at 48 kHz).
fn fft_size(rate: u32) -> usize {
    if rate > 64_000 { 8192 } else { 4096 }
}

fn power(db: f32) -> f32 {
    10f32.powf(db / 10.0)
}

fn to_db(p: f32) -> f32 {
    if p > 1e-14 { 10.0 * p.log10() } else { FLOOR_DB }
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

    /// The level drawn for `c` at grid point `i` (dB), if `c` is shown.
    pub fn level(&self, c: Curve, i: usize) -> Option<f32> {
        self.curves.get(&c).and_then(|s| s.level.get(i).copied())
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
                CurveState { slot, claimed_at: at, seen: at, moved_at: now, target: vec![FLOOR_DB; POINTS], level: vec![FLOOR_DB; POINTS] },
            );
        }
    }

    /// Analyses up to [`FFTS_PER_FRAME`] of `order`'s curves whose rings moved, from where the
    /// round robin left off; a ring that stopped moving reads silence.
    fn analyse(&mut self, order: &[Curve], rate: u32, now: f64) {
        self.analysed_last_frame = 0;
        let n = fft_size(rate);
        if self.bands.as_ref().is_none_or(|(bn, br, _)| *bn != n || *br != rate) {
            self.bands = Some((n, rate, bands_for(n, rate)));
        }
        if self.samples.len() != n {
            self.samples = vec![0.0; n];
        }
        let count = order.len();
        if count == 0 {
            return;
        }
        let start = self.next % count;
        let mut done = 0;
        for k in 0..count {
            let i = (start + k) % count;
            let Some(c) = order.get(i) else { continue };
            let Some(state) = self.curves.get_mut(c) else { continue };
            let Some(ring) = self.taps.ring(state.slot) else { continue };
            let written = ring.written();
            if written == state.seen {
                if now - state.moved_at > STALE_S {
                    state.target.fill(FLOOR_DB);
                }
                continue;
            }
            if done >= FFTS_PER_FRAME {
                continue;
            }
            state.seen = written;
            state.moved_at = now;
            if written.saturating_sub(state.claimed_at) < n {
                // Not a whole window of this curve's own audio yet.
                continue;
            }
            ring.latest(&mut self.samples);
            self.analyzer.magnitude_db_into(&self.samples, n, &mut self.mags);
            if let Some((_, _, bands)) = &self.bands {
                for (t, b) in state.target.iter_mut().zip(bands) {
                    *t = match *b {
                        Band::Range(lo, hi) => {
                            let bins = self.mags.get(lo..=hi).unwrap_or(&[]);
                            if bins.is_empty() { FLOOR_DB } else { to_db(bins.iter().map(|d| power(*d)).sum::<f32>() / bins.len() as f32) }
                        }
                        Band::Between(k, w) => {
                            let a = self.mags.get(k).copied().map_or(0.0, power);
                            let b = self.mags.get(k + 1).copied().map_or(0.0, power);
                            to_db(a + (b - a) * w)
                        }
                    };
                }
            }
            done += 1;
            self.next = i + 1;
        }
        self.analysed_last_frame = done;
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

    /// Eases every shown curve towards its latest analysis. Returns whether any is still moving
    /// or above the floor (worth another frame soon).
    fn ease(&mut self, dt: f32) -> bool {
        let up = 1.0 - (-dt / RISE_S).exp();
        let fall = FALL_DB_PER_S * dt;
        let mut live = false;
        for s in self.curves.values_mut() {
            for (l, t) in s.level.iter_mut().zip(&s.target) {
                if *t > *l {
                    *l += (*t - *l) * up;
                } else {
                    *l = (*l - fall).max(*t);
                }
                live |= *l > BOTTOM_DB - 6.0 || (*t - *l).abs() > 0.5;
            }
        }
        live
    }
}

/// The curves to show now, in list order (tracks as listed, then the master).
fn wanted(app: &SoundApp) -> Vec<Curve> {
    cmd::drawn(app.engine.session())
}

/// How a curve's line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineStyle {
    Solid,
    Dashed,
    Dotted,
}

/// A curve's line: its track's colour (a little lighter when three curves before it already have
/// that colour) and its style. The list's swatch draws the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CurveLook {
    pub color: Color32,
    pub style: LineStyle,
}

/// Colours this close (summed channel difference) are taken for the same colour.
const SAME_COLOR: i32 = 30;

fn near(a: Color32, b: Color32) -> bool {
    let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).abs();
    d(a.r(), b.r()) + d(a.g(), b.g()) + d(a.b(), b.b()) <= SAME_COLOR
}

/// Looks for `rows` (curve, track colour) in list order: the first of each colour solid, the next
/// with that colour dashed, then dotted, then the same three again a little lighter. The user's
/// track colours are never changed; this is only how the editor draws them.
pub fn looks(rows: &[(Curve, Color32)]) -> HashMap<Curve, CurveLook> {
    const STYLES: [LineStyle; 3] = [LineStyle::Solid, LineStyle::Dashed, LineStyle::Dotted];
    let mut out = HashMap::new();
    for (i, (c, color)) in rows.iter().enumerate() {
        let k = rows.iter().take(i).filter(|(_, o)| near(*o, *color)).count();
        let style = STYLES.get(k % 3).copied().unwrap_or(LineStyle::Solid);
        let color = if (k / 3) % 2 == 1 { lighten(*color, 0.35) } else { *color };
        out.insert(*c, CurveLook { color, style });
    }
    out
}

fn lighten(c: Color32, k: f32) -> Color32 {
    let l = |x: u8| (f32::from(x) + (255.0 - f32::from(x)) * k).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(l(c.r()), l(c.g()), l(c.b()))
}

/// Draws `pts` in `style`.
pub(crate) fn stroke_line(p: &egui::Painter, pts: Vec<Pos2>, style: LineStyle, width: f32, color: Color32) {
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

/// Every row's look, from the list's order (so a curve keeps its look as others come and go).
fn all_looks(app: &SoundApp) -> HashMap<Curve, CurveLook> {
    let s = app.engine.session();
    let mut rows: Vec<Curve> = cmd::listed(s).map(|t| Curve::Track(t.id)).collect();
    rows.push(Curve::Master);
    let rows: Vec<(Curve, Color32)> = rows.into_iter().map(|c| (c, identity(app, c).1)).collect();
    looks(&rows)
}

/// The curve's name and colour.
fn identity(app: &SoundApp, c: Curve) -> (String, Color32) {
    match c {
        Curve::Track(t) => app
            .engine
            .session()
            .track(t)
            .map_or_else(|| ("?".to_string(), Color32::GRAY), |t| (t.name.clone(), Color32::from_rgb(t.color[0], t.color[1], t.color[2]))),
        Curve::Master => ("Master".to_string(), Tokens::current().text),
    }
}

/// x of frequency `hz` in `r`.
pub fn x_of(r: Rect, hz: f32) -> f32 {
    let t = (hz.max(LOW_HZ) / LOW_HZ).ln() / (HIGH_HZ / LOW_HZ).ln();
    r.left() + t.clamp(0.0, 1.0) * r.width()
}

fn hz_of(r: Rect, x: f32) -> f32 {
    let t = ((x - r.left()) / r.width().max(1.0)).clamp(0.0, 1.0);
    LOW_HZ * (HIGH_HZ / LOW_HZ).powf(t)
}

/// y of level `db` (dB) in `r`.
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

/// Draws the editor into the dock tab.
pub(crate) fn show(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    let dt = app.spectrum.last_time.replace(now).map_or(1.0 / 60.0, |p| (now - p) as f32).clamp(0.0, 0.25);
    app.spectrum.drawn_pass = Some(ctx.cumulative_pass_nr());
    // Drawn now: the taps run from this frame on (`tick` turns them off once it stops drawing).
    app.spectrum.taps.set_active(true);
    let want = wanted(app);
    app.spectrum.sync(&want, now);
    let rate = app.engine.session().sample_rate.hz();
    app.spectrum.analyse(&want, rate, now);
    let live = app.spectrum.ease(dt);
    let scope_on = app.engine.session().edit.flag(cmd::SCOPE);
    app.spectrum.read_scope(&want, scope_on, now);
    app.spectrum.drawn = want.clone();
    app.spectrum.looks = all_looks(app);
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, t.panel_bg);
    let list_rect = Rect::from_min_max(full.min, pos2((full.left() + LIST_W).min(full.right()), full.bottom()));
    let graph_rect = Rect::from_min_max(pos2(list_rect.right() + 1.0, full.top()), full.max);
    ui.painter().line_segment([pos2(list_rect.right(), full.top()), pos2(list_rect.right(), full.bottom())], Stroke::new(1.0, t.border));
    let list_hover = {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(list_rect.shrink2(vec2(8.0, 6.0))).id_salt("spectrum-list"));
        list(app, &mut child)
    };
    // The scope: a square at the right of the graph's area.
    let (graph_rect, scope_rect) = if scope_on {
        let side = graph_rect.height().min(graph_rect.width() * 0.42);
        let scope = Rect::from_min_max(pos2(graph_rect.right() - side, graph_rect.top()), graph_rect.max);
        (Rect::from_min_max(graph_rect.min, pos2(scope.left() - 1.0, graph_rect.bottom())), Some(scope))
    } else {
        (graph_rect, None)
    };
    let graph_hover = graph(app, ui, graph_rect, list_hover.or(app.spectrum.scope_hover));
    let scope_hover = scope_rect.and_then(|r| scope(app, ui, r, graph_hover.or(list_hover)));
    app.spectrum.scope_rect = scope_rect;
    app.spectrum.scope_hover = scope_hover;
    app.spectrum.hovered = graph_hover.or(scope_hover).or(list_hover);
    if live || app.is_playing() {
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

/// The list: Show all, Hide all, Only selected, then a row per track (and the master). Returns the
/// curve whose row the pointer is on.
fn list(app: &mut SoundApp, ui: &mut Ui) -> Option<Curve> {
    let t = Tokens::current();
    let only = app.engine.session().edit.flag(cmd::ONLY_SELECTED);
    let mut run: Option<(&str, serde_json::Value)> = None;
    ui.horizontal(|ui| {
        if ui.small_button("Show all").on_hover_text("Show every track's curve and the master's").clicked() {
            run = Some(("spectrum.show_all", json!({})));
        }
        if ui.small_button("Hide all").on_hover_text("Hide every curve").clicked() {
            run = Some(("spectrum.hide_all", json!({})));
        }
    });
    if ui
        .selectable_label(only, "Only selected tracks")
        .on_hover_text("Show just the tracks selected in the Edit window, following the selection")
        .clicked()
    {
        run = Some(("spectrum.only_selected", json!({})));
    }
    let scope_on = app.engine.session().edit.flag(cmd::SCOPE);
    if ui
        .selectable_label(scope_on, "Stereo scope")
        .on_hover_text(
            "Show each shown track's left and right beside the graph: a vertical line is mono, a cloud is wide, a horizontal line is out of phase",
        )
        .clicked()
    {
        run = Some(("spectrum.scope", json!({})));
    }
    ui.add_space(4.0);
    let (wanted_n, on_n) = {
        let s = app.engine.session();
        let on = cmd::listed(s).filter(|t| cmd::switched_on(s, Curve::Track(t.id))).count() + usize::from(cmd::switched_on(s, Curve::Master));
        (cmd::wanted(s).len(), on)
    };
    if wanted_n > cmd::MAX_VISIBLE {
        ui.label(
            egui::RichText::new(format!("Showing the first {} of {wanted_n}: at most {} curves at once.", cmd::MAX_VISIBLE, cmd::MAX_VISIBLE))
                .color(t.text_dim)
                .size(11.0),
        );
    }
    let full = on_n >= cmd::MAX_VISIBLE;
    let rows: Vec<(Curve, String, CurveLook, bool, bool)> = {
        let s = app.engine.session();
        let mut v: Vec<Curve> = cmd::listed(s).map(|t| Curve::Track(t.id)).collect();
        v.push(Curve::Master);
        v.into_iter()
            .map(|c| {
                let name = identity(app, c).0;
                let look = app.spectrum.looks.get(&c).copied().unwrap_or(CurveLook { color: Color32::GRAY, style: LineStyle::Solid });
                (c, name, look, cmd::switched_on(s, c), app.spectrum.drawn.contains(&c))
            })
            .collect()
    };
    if rows.len() == 1 {
        ui.label(egui::RichText::new("No tracks yet.").color(t.text_dim));
    }
    let mut hovered = None;
    egui::ScrollArea::vertical().id_salt("spectrum-rows").auto_shrink([false, false]).show(ui, |ui| {
        for (c, name, look, on, visible) in rows {
            let row = ui.horizontal(|ui| {
                // The swatch: a stroke of the curve's own line, colour and style.
                let (sw, _) = ui.allocate_exact_size(vec2(22.0, 12.0), Sense::hover());
                let color = if visible { look.color } else { look.color.gamma_multiply(0.35) };
                stroke_line(ui.painter(), vec![pos2(sw.left(), sw.center().y), pos2(sw.right(), sw.center().y)], look.style, 2.4, color);
                let mut v = if only { visible } else { on };
                let label = egui::RichText::new(&name).color(if visible { t.text } else { t.text_dim });
                let capped = !only && !on && full;
                let r = ui.add_enabled(!only && !capped, egui::Checkbox::new(&mut v, label));
                let r = if only {
                    r.on_disabled_hover_text("Only selected tracks is on: the Edit window's selection decides")
                } else if capped {
                    r.on_disabled_hover_text(cmd::cap_reason())
                } else {
                    r.on_hover_text(format!("Show or hide {name} in the Spectrum"))
                };
                if r.clicked() {
                    run = Some((
                        "spectrum.show",
                        match c {
                            Curve::Track(id) => json!({"track": id.0, "shown": v}),
                            Curve::Master => json!({"master": true, "shown": v}),
                        },
                    ));
                }
            });
            if row.response.contains_pointer() && visible {
                hovered = Some(c);
            }
        }
    });
    if let Some((id, p)) = run
        && let Err(e) = app.run(id, p)
    {
        app.ui.status = e;
    }
    hovered
}

/// The graph: grid, curves, names, and the readout under the pointer. Returns the curve the
/// pointer is on.
fn graph(app: &mut SoundApp, ui: &mut Ui, r: Rect, list_hover: Option<Curve>) -> Option<Curve> {
    let t = Tokens::current();
    let p = ui.painter_at(r);
    p.rect_filled(r, 0.0, t.window_bg);
    let plot = Rect::from_min_max(pos2(r.left() + 34.0, r.top() + 8.0), pos2(r.right() - 10.0, r.bottom() - 20.0));
    if plot.width() < 40.0 || plot.height() < 30.0 {
        app.spectrum.plot = None;
        return None;
    }
    app.spectrum.plot = Some(plot);
    let grid = Stroke::new(1.0, t.grid_line);
    for hz in [20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0, 20_000.0] {
        let x = x_of(plot, hz);
        p.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], grid);
        let label = if hz >= 1000.0 { format!("{}k", hz / 1000.0) } else { format!("{hz}") };
        p.text(pos2(x, plot.bottom() + 3.0), Align2::CENTER_TOP, label, mono(9.5), t.text_dim);
    }
    let mut db = TOP_DB;
    while db >= BOTTOM_DB {
        let y = y_of(plot, db);
        p.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], grid);
        p.text(pos2(plot.left() - 4.0, y), Align2::RIGHT_CENTER, format!("{db:.0}"), mono(9.5), t.text_dim);
        db -= 12.0;
    }
    let resp = ui.interact(r, ui.id().with("spectrum-graph"), Sense::hover());
    let pointer = resp.hover_pos().filter(|q| plot.contains(*q));
    let order = app.spectrum.drawn.clone();
    let xs: Vec<f32> = (0..POINTS).map(|i| x_of(plot, point_hz(i))).collect();
    // The curve under the pointer: the nearest within 12 px of its level at the pointer's x.
    let mut graph_hover = None;
    if let Some(q) = pointer {
        let i = (((q.x - plot.left()) / plot.width()) * (POINTS - 1) as f32).round().clamp(0.0, (POINTS - 1) as f32) as usize;
        let mut best = 12.0f32;
        for c in &order {
            if let Some(l) = app.spectrum.level(*c, i).filter(|l| *l > BOTTOM_DB) {
                let d = (y_of(plot, l) - q.y).abs();
                if d < best {
                    best = d;
                    graph_hover = Some(*c);
                }
            }
        }
        app.spectrum.readout = Some((hz_of(plot, q.x), db_of(plot, q.y)));
    } else {
        app.spectrum.readout = None;
    }
    let focus = graph_hover.or(list_hover);
    let mut labels: Vec<Rect> = Vec::new();
    // The focused curve last, on top.
    let mut drawing: Vec<Curve> = order.iter().copied().filter(|c| Some(*c) != focus).collect();
    drawing.extend(focus);
    for c in drawing {
        let name = identity(app, c).0;
        let look = app.spectrum.looks.get(&c).copied().unwrap_or(CurveLook { color: Color32::GRAY, style: LineStyle::Solid });
        let color = look.color;
        let Some(state) = app.spectrum.curves.get(&c) else { continue };
        let pts: Vec<Pos2> = xs.iter().zip(&state.level).map(|(x, l)| pos2(*x, y_of(plot, l.max(BOTTOM_DB - 4.0)).min(plot.bottom()))).collect();
        let focused = Some(c) == focus;
        let color = if focus.is_some() && !focused { color.gamma_multiply(0.35) } else { color };
        if focused {
            // Under the line: a soft wash in its colour, a strip of quads down to the floor.
            for w in pts.windows(2) {
                if let [a, b] = w {
                    p.add(Shape::convex_polygon(
                        vec![*a, *b, pos2(b.x, plot.bottom()), pos2(a.x, plot.bottom())],
                        color.gamma_multiply(0.18),
                        Stroke::NONE,
                    ));
                }
            }
        }
        stroke_line(&p, pts.clone(), look.style, if focused { 2.6 } else { 1.5 }, color);
        // The name at the curve's peak, where it doesn't cover another name.
        let peak = state.level.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, l)| (i, *l));
        if let Some((i, _)) = peak.filter(|(_, l)| *l > BOTTOM_DB + 3.0)
            && let Some(at) = pts.get(i)
        {
            let galley = p.layout_no_wrap(name.clone(), regular(if focused { 12.0 } else { 10.5 }), color);
            let mut rect = Rect::from_min_size(pos2(at.x - galley.size().x * 0.5, at.y - galley.size().y - 3.0), galley.size());
            rect = rect
                .translate(vec2((plot.left() - rect.left()).max(0.0) + (plot.right() - rect.right()).min(0.0), (plot.top() - rect.top()).max(0.0)));
            if focused || !labels.iter().any(|o| o.expand(2.0).intersects(rect)) {
                p.galley(rect.min, galley, color);
                labels.push(rect);
            }
        }
    }
    if order.is_empty() {
        p.text(plot.center(), Align2::CENTER_CENTER, "No curves shown: tick tracks in the list.", regular(12.0), t.text_dim);
    } else if app.spectrum.curves.values().all(|s| s.level.iter().all(|l| *l <= BOTTOM_DB)) {
        p.text(plot.center(), Align2::CENTER_CENTER, "Play, or monitor an input, to see the curves.", regular(12.0), t.text_dim);
    }
    if let (Some(q), Some((hz, level))) = (pointer, app.spectrum.readout) {
        p.line_segment([pos2(q.x, plot.top()), pos2(q.x, plot.bottom())], Stroke::new(1.0, t.text_dim.gamma_multiply(0.6)));
        let mut text = format!("{} · {level:.1} dB", hz_text(hz));
        if let Some(c) = graph_hover {
            text = format!("{} · {text}", identity(app, c).0);
        }
        let galley = p.layout_no_wrap(text, mono(10.5), t.text);
        let at = pos2((q.x + 10.0).min(plot.right() - galley.size().x - 4.0), (q.y - galley.size().y - 8.0).max(plot.top() + 2.0));
        let bg = Rect::from_min_size(at, galley.size()).expand(3.0);
        p.rect_filled(bg, 3.0, t.panel_bg.gamma_multiply(0.92));
        p.galley(at, galley, t.text);
    }
    graph_hover
}

/// Draws one dot of `look`'s style at `at`: solid lines are filled dots, dashed ones small
/// crosses, dotted ones rings.
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

/// "+0.82", "-0.30".
pub fn correlation_text(c: f32) -> String {
    if c.abs() < 0.005 { "0.00".to_string() } else { format!("{c:+.2}") }
}

/// The stereo scope in `r`: guides, every shown curve's dots on one zoom, the focused one on top,
/// and under it the focused curve's correlation. Returns the curve the pointer is on.
fn scope(app: &mut SoundApp, ui: &mut Ui, r: Rect, focus_in: Option<Curve>) -> Option<Curve> {
    let t = Tokens::current();
    let p = ui.painter_at(r);
    p.rect_filled(r, 0.0, t.window_bg);
    p.line_segment([pos2(r.left(), r.top()), pos2(r.left(), r.bottom())], Stroke::new(1.0, t.border));
    let field = Rect::from_min_max(r.min + vec2(10.0, 10.0), pos2(r.right() - 10.0, r.bottom() - 26.0));
    let radius = (field.width().min(field.height()) * 0.5 - 2.0).max(4.0);
    let centre = field.center();
    let faint = Stroke::new(1.0, t.grid_line);
    p.circle_stroke(centre, radius, Stroke::new(1.0, t.text_dim.gamma_multiply(0.4)));
    p.circle_stroke(centre, radius * 0.5, faint);
    p.line_segment([centre - vec2(0.0, radius), centre + vec2(0.0, radius)], faint);
    p.line_segment([centre - vec2(radius, 0.0), centre + vec2(radius, 0.0)], faint);
    let d = radius * std::f32::consts::FRAC_1_SQRT_2;
    for sx in [-1.0f32, 1.0] {
        p.line_segment([centre + vec2(sx * d, -d), centre - vec2(sx * d, -d)], faint);
    }
    p.text(centre + vec2(-d - 3.0, -d - 3.0), Align2::RIGHT_BOTTOM, "L", regular(10.0), t.text_dim);
    p.text(centre + vec2(d + 3.0, -d - 3.0), Align2::LEFT_BOTTOM, "R", regular(10.0), t.text_dim);
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
    let focus = hover.or(focus_in);
    let mut order: Vec<Curve> = app.spectrum.drawn.iter().copied().filter(|c| Some(*c) != focus).collect();
    order.extend(focus.filter(|c| app.spectrum.drawn.contains(c)));
    for c in &order {
        let Some(sc) = app.spectrum.scope.get(c) else { continue };
        let look = app.spectrum.looks.get(c).copied().unwrap_or(CurveLook { color: Color32::GRAY, style: LineStyle::Solid });
        let focused = Some(*c) == focus;
        let color = if focus.is_some() && !focused { look.color.gamma_multiply(0.3) } else { look.color.gamma_multiply(0.85) };
        for pt in sc.points.iter().filter(|(m, s)| *m != 0.0 || *s != 0.0) {
            scope_dot(&p, place(*pt), look.style, color, focused);
        }
    }
    let line = match focus.and_then(|c| app.spectrum.scope.get(&c).map(|sc| (c, sc.correlation))) {
        Some((c, Some(k))) => format!("{} · correlation {}", identity(app, c).0, correlation_text(k)),
        Some((c, None)) => format!("{} · silent", identity(app, c).0),
        None if app.spectrum.scope.is_empty() => "Stereo scope: nothing playing".to_string(),
        None => "Point at a track for its correlation".to_string(),
    };
    p.text(pos2(r.center().x, r.bottom() - 13.0), Align2::CENTER_CENTER, line, regular(10.5), if focus.is_some() { t.text } else { t.text_dim });
    hover
}
