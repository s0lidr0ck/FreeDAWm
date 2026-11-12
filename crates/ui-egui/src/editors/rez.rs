//! Rez's editor: FreeDAWm's own design (no VST behind it), built on [`crate::editor_kit`] in the
//! Engine family's look.
//!
//! - The header: the title, the editor's Undo and A/B, and Delta (hear what is being removed).
//! - The graph, on a log frequency axis: the input and output spectra rising from the bottom, the
//!   cut in force hanging from the top (both sides when they are not linked), and the sensitivity
//!   curve with its four bands as dots (drag: frequency and sensitivity; Alt-drag or the wheel:
//!   width; double-click: flat; right-click: on/off) between the Low and High range handles.
//! - The dials: Depth with Soft/Hard, then Sharpness, Selectivity, Attack and Release, the stereo
//!   group (L/R or M/S, Link, Balance) and the output group (Mix, Trim).
//!
//! Every change goes through `mix.module_params`; a drag is one undo step.

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, PillLook, PillStyle, SelectionStyle, pill_ex, pill_height, tip};
use crate::editor_kit::family::{self, Family, FamilyState};
use crate::editor_kit::look::{CAPTION_SIZE, LABEL_SIZE, TINY_SIZE, paint_background, paint_panel, paint_title};
use crate::editor_kit::palette::al;
use crate::editor_kit::{
    Host, VstEditor, VstPalette, argb, consume_wheel, end_gesture, juce_wheel_delta_y, param, run_with, set_params, set_params_merged,
};
use crate::module_graphs::{GraphKind, SPEC_FFT, freq_to_x, module_streams, x_to_freq};
use crate::theme::{bold, regular};
use egui::{Align2, Color32, Id, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::rez::{self as dsp, BAND_GAIN_DB, BAND_IDS, BAND_Q, BANDS, Band, LIVE_CUT_A, LIVE_CUT_B, LIVE_POINTS};
use soundcraft_mix::StripMeter;
use soundcraft_model::{ModuleId, Track};

// ============================================================================ layout

const PAD_X: f32 = 24.0;
const PAD_Y: f32 = 15.0;
const GAP: f32 = 14.0;
const HEADER_H: f32 = 26.0;
const GRAPH_H: f32 = 268.0;
const READOUT_H: f32 = 16.0;
const BIG_KNOB: f32 = 104.0;
const SMALL_KNOB: f32 = 62.0;
const SEG_H: f32 = 24.0;
const GROUP_LABEL_H: f32 = 18.0;
const CONTROLS_H: f32 = GROUP_LABEL_H + BIG_KNOB + 30.0 + 8.0 + SEG_H + 12.0;
const HEIGHT: f32 = PAD_Y * 2.0 + HEADER_H + GAP + GRAPH_H + 4.0 + READOUT_H + GAP + CONTROLS_H;

/// The editor's size.
pub const SIZE: Vec2 = vec2(860.0, HEIGHT);

/// Rez's accent: lavender.
pub const ACCENT: u32 = 0xFFB5_9CFF;
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(ACCENT));
/// The second side's cut (right, or side) when the sides are not linked.
const SIDE_B: Color32 = argb(0xFFFF_C46B);

/// A/B and Undo walk every parameter.
const FAMILY: Family = Family::new("Rez", &[]);

/// The widths of the four control groups (they fill the editor with [`GAP`] between them).
const GROUP_W: [f32; 4] = [150.0, 296.0, 170.0, 154.0];

// ============================================================================ the graph's scales

/// The cut shown from the top of the plot down: this many dB over [`CUT_SPAN`] of its height.
const CUT_RANGE_DB: f32 = 24.0;
const CUT_SPAN: f32 = 0.62;
/// Where the sensitivity curve's 0 dB sits (from the top), and the height of 12 dB.
const SENS_ZERO: f32 = 0.36;
const SENS_12DB: f32 = 0.24;
/// The spectra: this many dB from the bottom of the plot to the top, tilted so pink noise is flat.
const SPEC_FLOOR_DB: f32 = -96.0;
const SPEC_TOP_DB: f32 = -12.0;
const SPEC_TILT_DB_PER_OCT: f32 = 3.0;
const NODE_R: f32 = 9.0;

fn cut_y(plot: Rect, db: f32) -> f32 {
    plot.top() + (db / CUT_RANGE_DB).clamp(0.0, 1.0 / CUT_SPAN) * CUT_SPAN * plot.height()
}

fn sens_y(plot: Rect, db: f32) -> f32 {
    (plot.top() + plot.height() * (SENS_ZERO - db / 12.0 * SENS_12DB)).clamp(plot.top(), plot.bottom())
}

fn y_to_sens(plot: Rect, y: f32) -> f32 {
    ((SENS_ZERO - (y - plot.top()) / plot.height().max(1.0)) / SENS_12DB * 12.0).clamp(BAND_GAIN_DB.0, BAND_GAIN_DB.1)
}

// ============================================================================ formatting

fn hz_text(hz: f32) -> String {
    if hz >= 9995.0 {
        format!("{:.1} kHz", hz / 1000.0)
    } else if hz >= 1000.0 {
        format!("{:.2} kHz", hz / 1000.0)
    } else {
        format!("{hz:.0} Hz")
    }
}

fn signed_db(db: f32) -> String {
    let db = if db.abs() < 0.05 { 0.0 } else { db };
    format!("{}{db:.1} dB", if db > 0.0 { "+" } else { "" })
}

/// Balance's readout: which side gets less.
fn balance_text(v: f32, mid_side: bool) -> String {
    let (a, b) = if mid_side { ("Mid", "Side") } else { ("Left", "Right") };
    if v.abs() < 0.5 { "Even".into() } else { format!("{} {:.0} %", if v < 0.0 { a } else { b }, v.abs()) }
}

fn value_text(host: &Host, id: &str) -> String {
    let v = param(host, id);
    match id {
        "trim" => signed_db(v),
        "balance" => balance_text(v, param(host, "stereo") > 0.5),
        _ => host.info.param(id).map_or_else(String::new, |p| p.format(v)),
    }
}

// ============================================================================ the editor

#[derive(Clone, Default)]
pub(crate) struct Editor {
    fam: FamilyState,
    /// The band last hovered or dragged: its numbers stay in the readout.
    shown_band: Option<usize>,
    /// The cut curves as drawn (they fall back smoothly between frames).
    cut_a: Vec<f32>,
    cut_b: Vec<f32>,
}

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    run_with::<Editor>(app, ui, track, module, meters, |_, _| {});
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "rez";

    fn size(&self, _ins: &soundcraft_model::Insert) -> Vec2 {
        SIZE
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        editor_ui(self, app, ui, host);
    }
}

fn bands_of(host: &Host) -> [Band; BANDS] {
    std::array::from_fn(|i| {
        let [on, freq, gain, q] = BAND_IDS[i];
        Band { on: param(host, on) > 0.5, freq: param(host, freq), gain: param(host, gain), q: param(host, q) }
    })
}

fn keep_centre(r: Rect, w: f32, h: f32) -> Rect {
    Rect::from_center_size(r.center(), vec2(w.min(r.width()), h.min(r.height())))
}

fn text_left(p: &egui::Painter, at: Pos2, text: &str, size: f32, c: Color32) {
    p.text(at, Align2::LEFT_CENTER, text, regular(size), c);
}

/// One dial bound to parameter `id`.
#[allow(clippy::too_many_arguments)]
fn dial_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect, id: &str, label: &str, big: bool) {
    let value_text = value_text(host, id);
    let o = DialOpts { label, value_text: &value_text, default: family::default01(host, id), small: !big, hero: big, ..DialOpts::default() };
    family::dial_param(app, ui, host, fam, &FAMILY, r, &PALETTE, id, &o);
}

/// A row of small dials centred in `r`.
fn dial_row(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect, dials: &[(&str, &str)]) {
    let cell_w = (r.width() / dials.len().max(1) as f32).trunc();
    for (i, (id, label)) in dials.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(r.left() + cell_w * i as f32, r.top()), vec2(cell_w, r.height()));
        dial_ui(app, ui, host, fam, keep_centre(cell, (cell_w - 4.0).min(96.0), SMALL_KNOB + 28.0), id, label, false);
    }
}

fn group_label(p: &egui::Painter, panel: Rect, text: &str) {
    p.text(pos2(panel.left() + 12.0, panel.top() + 13.0), Align2::LEFT_CENTER, text.to_uppercase(), regular(TINY_SIZE), PALETTE.text_faint);
}

// ---------------------------------------------------------------------------- the header

fn header_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, header: Rect) {
    let pal = &PALETTE;
    paint_title(ui.painter(), header, "Rez", "Resonance", pal);
    let h = pill_height(PillStyle::Small);
    let plain = PillLook { tracked: false, ..PillLook::default() };
    let mut right = header.right();
    let mut place = |w: f32, gap: f32| {
        let r = Rect::from_min_size(pos2(right - w, header.center().y - h * 0.5), vec2(w, h));
        right -= w + gap;
        r
    };
    let delta_on = param(host, "delta") > 0.5;
    let look = PillLook { active_colour: Some(pal.warn), ..plain };
    let delta = pill_ex(ui, place(58.0, 10.0), "rez-delta", pal, "Delta", delta_on, PillStyle::Small, look);
    if delta.clicked() {
        family::pill_set(app, host, &mut st.fam, &FAMILY, "delta", if delta_on { 0.0 } else { 1.0 });
    }
    delta.on_hover_text("Hear only what Rez is removing");
    if pill_ex(ui, place(28.0, 3.0), "rez-b", pal, "B", st.fam.on_b(), PillStyle::Small, plain).clicked() {
        family::select_ab(app, host, &mut st.fam, &FAMILY, true);
    }
    if pill_ex(ui, place(28.0, 8.0), "rez-a", pal, "A", !st.fam.on_b(), PillStyle::Small, plain).clicked() {
        family::select_ab(app, host, &mut st.fam, &FAMILY, false);
    }
    let look = PillLook { dimmed: !st.fam.can_undo(), ..plain };
    if pill_ex(ui, place(48.0, 0.0), "rez-undo", pal, "Undo", false, PillStyle::Small, look).clicked() {
        family::do_undo(app, host, &mut st.fam);
    }
}

// ---------------------------------------------------------------------------- the graph

/// A spectrum (dB per bin of a [`SPEC_FFT`]-point transform) as points across `plot`.
fn spectrum_points(plot: Rect, spec: &[f32], sr: f32) -> Vec<Pos2> {
    if spec.len() < 4 {
        return Vec::new();
    }
    let n = (plot.width() as usize / 2).clamp(2, 420);
    let last = spec.len() - 1;
    let bin_of = |f: f32| f * SPEC_FFT as f32 / sr;
    let half = plot.width() / (n - 1) as f32 * 0.5;
    (0..n)
        .map(|i| {
            let x = plot.left() + plot.width() * i as f32 / (n - 1) as f32;
            let (f0, f1, f) = (x_to_freq(plot, x - half), x_to_freq(plot, x + half), x_to_freq(plot, x));
            let (k0, k1) = ((bin_of(f0).floor().max(1.0) as usize).min(last), (bin_of(f1).ceil().max(1.0) as usize).min(last));
            let db = if k1 > k0 + 1 {
                spec.get(k0..=k1).map_or(SPEC_FLOOR_DB, |s| s.iter().copied().fold(f32::MIN, f32::max))
            } else {
                let k = bin_of(f).clamp(1.0, last as f32);
                let (a, t) = (k.floor() as usize, k.fract());
                spec.get(a).copied().unwrap_or(SPEC_FLOOR_DB) * (1.0 - t) + spec.get((a + 1).min(last)).copied().unwrap_or(SPEC_FLOOR_DB) * t
            };
            let db = db + SPEC_TILT_DB_PER_OCT * (f / 1000.0).log2();
            let v = if db.is_finite() { ((db - SPEC_FLOOR_DB) / (SPEC_TOP_DB - SPEC_FLOOR_DB)).clamp(0.0, 1.0) } else { 0.0 };
            pos2(x, plot.bottom() - plot.height() * v)
        })
        .collect()
}

/// `pts` filled down to (or up to) the line `y = edge`.
fn fill_to(p: &egui::Painter, pts: &[Pos2], edge: f32, fill: Color32) {
    for w in pts.windows(2) {
        if let [a, b] = w {
            p.add(Shape::convex_polygon(vec![*a, *b, pos2(b.x, edge), pos2(a.x, edge)], fill, Stroke::NONE));
        }
    }
}

/// The live cut of one side, eased towards `fresh` (rises at once, falls back over a few frames).
fn ease_cut(shown: &mut Vec<f32>, fresh: impl Fn(usize) -> f32) {
    shown.resize(LIVE_POINTS, 0.0);
    for (i, s) in shown.iter_mut().enumerate() {
        let v = fresh(i);
        let v = if v.is_finite() { v.max(0.0) } else { 0.0 };
        *s = if v >= *s { v } else { *s + (v - *s) * 0.35 };
    }
}

fn cut_points(plot: Rect, cut: &[f32]) -> Vec<Pos2> {
    cut.iter().enumerate().map(|(i, db)| pos2(freq_to_x(plot, dsp::live_hz(i)), cut_y(plot, *db))).collect()
}

/// A vertical range handle (Low or High) at `x`: a drag sets `id`. Returns whether it is hot.
#[allow(clippy::too_many_arguments)]
fn range_handle(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, plot: Rect, id: &str, name: &str, x: f32) -> bool {
    let pal = &PALETTE;
    let hit = Rect::from_min_max(pos2(x - 6.0, plot.top()), pos2(x + 6.0, plot.bottom()));
    let resp = ui.interact(hit, ui.id().with(("rez-range", id)), Sense::click_and_drag());
    if resp.drag_started() {
        end_gesture(app);
        family::push_undo(&mut st.fam, host, &FAMILY);
    }
    if resp.dragged()
        && let (Some(at), Some(info)) = (resp.interact_pointer_pos(), host.info.param(id))
    {
        set_params_merged(app, host, &[(id, info.clamp(x_to_freq(plot, at.x)))], &format!("rez-{id}"));
    }
    if resp.drag_stopped() {
        end_gesture(app);
    }
    if resp.double_clicked()
        && let Some(info) = host.info.param(id)
    {
        family::set(app, host, &mut st.fam, &FAMILY, id, info.default);
    }
    let hot = resp.hovered() || resp.dragged();
    if hot {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let c = if hot { pal.text_bright } else { pal.text_dim };
    let p = ui.painter();
    p.vline(x, plot.y_range(), Stroke::new(if hot { 1.5 } else { 1.0 }, al(c, if hot { 0.9 } else { 0.45 })));
    let grip = Rect::from_center_size(pos2(x, plot.bottom() - 9.0), vec2(8.0, 12.0));
    p.rect_filled(grip, 2.0, al(c, if hot { 1.0 } else { 0.7 }));
    resp.on_hover_text(format!("{name}: Rez leaves everything beyond this alone. Drag to move, double-click to reset."));
    hot
}

/// A band's dot at `at`: its gestures, then its face.
fn band_node(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, plot: Rect, i: usize, band: &Band, at: Pos2) {
    let pal = &PALETTE;
    let [on_id, freq_id, gain_id, q_id] = BAND_IDS[i];
    let hit = Rect::from_center_size(at, Vec2::splat(NODE_R * 2.0 + 6.0));
    let resp = ui.interact(hit, ui.id().with(("rez-band", i)), Sense::click_and_drag());
    let gesture = format!("rez-band-{i}");
    if resp.drag_started() {
        end_gesture(app);
        family::push_undo(&mut st.fam, host, &FAMILY);
    }
    if resp.dragged() {
        if ui.input(|inp| inp.modifiers.alt) {
            // Alt: up widens the band, down narrows it.
            let q = (band.q * 2.0f32.powf(resp.drag_delta().y / 48.0)).clamp(BAND_Q.0, BAND_Q.1);
            set_params_merged(app, host, &[(q_id, q)], &gesture);
        } else if let Some(p) = resp.interact_pointer_pos() {
            let freq = x_to_freq(plot, p.x.clamp(plot.left(), plot.right()));
            set_params_merged(app, host, &[(freq_id, freq), (gain_id, y_to_sens(plot, p.y))], &gesture);
        }
    }
    if resp.drag_stopped() {
        end_gesture(app);
    }
    if resp.hovered() {
        let wheel = juce_wheel_delta_y(ui);
        if wheel != 0.0 {
            consume_wheel(ui);
            let q = (band.q * 2.0f32.powf(wheel)).clamp(BAND_Q.0, BAND_Q.1);
            set_params_merged(app, host, &[(q_id, q)], &gesture);
        }
    }
    if resp.double_clicked() {
        family::push_undo(&mut st.fam, host, &FAMILY);
        end_gesture(app);
        set_params(app, host, &[(gain_id, 0.0), (q_id, 1.0)]);
    }
    if resp.secondary_clicked() {
        family::set(app, host, &mut st.fam, &FAMILY, on_id, if band.on { 0.0 } else { 1.0 });
    }
    let hot = resp.hovered() || resp.dragged();
    if hot {
        st.shown_band = Some(i);
    }
    let p = ui.painter();
    let ring = if band.on { pal.accent } else { pal.text_faint };
    if hot {
        p.circle_filled(at, NODE_R + 4.0, al(ring, 0.18));
    }
    p.circle_filled(at, NODE_R, if band.on { al(pal.accent, if hot { 1.0 } else { 0.9 }) } else { pal.panel });
    p.circle_stroke(at, NODE_R, Stroke::new(1.5, ring));
    p.text(at, Align2::CENTER_CENTER, (i + 1).to_string(), bold(LABEL_SIZE), if band.on { pal.on_accent } else { pal.text_faint });
    resp.on_hover_text("Drag: frequency and sensitivity. Alt-drag or wheel: width. Double-click: flat. Right-click: on/off.");
}

/// The graph's panel in an editor drawn at `frame`.
fn graph_panel(frame: Rect) -> Rect {
    Rect::from_min_size(pos2(frame.left() + PAD_X, frame.top() + PAD_Y + HEADER_H + GAP), vec2(frame.width() - 2.0 * PAD_X, GRAPH_H))
}

/// The graph's plot area in an editor drawn at `frame` (for tests and scripts).
pub fn plot_rect(frame: Rect) -> Rect {
    let panel = graph_panel(frame);
    Rect::from_min_max(pos2(panel.left() + 10.0, panel.top() + 10.0), pos2(panel.right() - 10.0, panel.bottom() - 22.0))
}

/// Where a band at `freq` Hz and `gain` dB of sensitivity has its dot, in an editor drawn at
/// `frame` (for tests and scripts).
pub fn band_pos(frame: Rect, freq: f32, gain: f32) -> Pos2 {
    let plot = plot_rect(frame);
    pos2(freq_to_x(plot, freq), sens_y(plot, gain))
}

fn graph_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host) {
    let pal = &PALETTE;
    let panel = graph_panel(host.rect);
    paint_panel(ui.painter(), panel, 10.0);
    let plot = plot_rect(host.rect);
    let p = ui.painter().with_clip_rect(panel.intersect(ui.clip_rect()));
    let sr = host.sr as f32;

    // ---- the frequency grid and its labels
    for (hz, label) in
        [(50.0, "50"), (100.0, "100"), (200.0, "200"), (500.0, "500"), (1000.0, "1k"), (2000.0, "2k"), (5000.0, "5k"), (10_000.0, "10k")]
    {
        let x = freq_to_x(plot, hz);
        p.vline(x, plot.y_range(), Stroke::new(1.0, al(Color32::WHITE, 0.05)));
        p.text(pos2(x, plot.bottom() + 11.0), Align2::CENTER_CENTER, label, regular(TINY_SIZE), pal.text_faint);
    }
    // The cut's scale, on the right.
    for db in [6.0f32, 12.0, 18.0, 24.0] {
        let y = cut_y(plot, db);
        p.hline(plot.right() - 6.0..=plot.right(), y, Stroke::new(1.0, al(Color32::WHITE, 0.16)));
        p.text(pos2(plot.right() - 9.0, y), Align2::RIGHT_CENTER, format!("-{db:.0}"), regular(TINY_SIZE), pal.text_faint);
    }

    // ---- the spectra, while the module plays
    let analysis = host.meters.and_then(|m| m.analysis.iter().find(|(id, _)| *id == host.ins.id).map(|(_, a)| a));
    if let Some(a) = analysis {
        let streams = module_streams(ui.ctx(), Id::new(("rez-streams", host.track.0, host.ins.id.0)), a, GraphKind::CleanQ);
        let pre = spectrum_points(plot, &streams.pre, sr);
        fill_to(&p, &pre, plot.bottom(), al(pal.text_dim, 0.14));
        let post = spectrum_points(plot, &streams.post, sr);
        fill_to(&p, &post, plot.bottom(), al(pal.accent, 0.10));
        if post.len() > 1 {
            p.add(Shape::line(post, Stroke::new(1.0, al(pal.accent, 0.55))));
        }
    }

    // ---- outside the range: dimmed
    let (low, high) = (param(host, "low"), param(host, "high"));
    let (x_low, x_high) = (freq_to_x(plot, low), freq_to_x(plot, high.max(low)));
    for r in [Rect::from_min_max(plot.min, pos2(x_low, plot.bottom())), Rect::from_min_max(pos2(x_high, plot.top()), plot.max)] {
        if r.width() > 0.5 {
            p.rect_filled(r, 0.0, al(Color32::BLACK, 0.28));
        }
    }

    // ---- the cut in force
    let live = crate::editor_kit::live(host);
    ease_cut(&mut st.cut_a, |i| live.map_or(0.0, |l| l.get(LIVE_CUT_A + i)));
    ease_cut(&mut st.cut_b, |i| live.map_or(0.0, |l| l.get(LIVE_CUT_B + i)));
    let split = param(host, "link") < 99.5 || param(host, "balance").abs() > 0.5;
    let a = cut_points(plot, &st.cut_a);
    fill_to(&p, &a, plot.top(), al(pal.accent, 0.30));
    p.add(Shape::line(a, Stroke::new(1.5, pal.accent)));
    if split {
        p.add(Shape::line(cut_points(plot, &st.cut_b), Stroke::new(1.5, SIDE_B)));
    }
    let deepest = st.cut_a.iter().chain(&st.cut_b).copied().fold(0.0f32, f32::max);

    // ---- the sensitivity curve
    let bands = bands_of(host);
    let zero = sens_y(plot, 0.0);
    p.hline(plot.x_range(), zero, Stroke::new(1.0, al(Color32::WHITE, 0.10)));
    let steps = (plot.width() / 3.0) as usize;
    let curve: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let x = plot.left() + plot.width() * i as f32 / steps.max(1) as f32;
            let hz = x_to_freq(plot, x);
            // Outside the range the curve falls away with the range's fade.
            let range = dsp::range_weight(low, high, hz);
            let db = dsp::bands_db(&bands, hz) + if range > 1.0e-3 { 20.0 * range.log10() } else { -96.0 };
            pos2(x, sens_y(plot, db.max(-96.0)))
        })
        .collect();
    p.add(Shape::line(curve, Stroke::new(1.5, al(pal.text_bright, 0.85))));
    p.rect_stroke(plot, 0.0, Stroke::new(1.0, al(Color32::WHITE, 0.06)), StrokeKind::Inside);

    // ---- gestures: the range handles first, the bands over them
    let hot_low = range_handle(st, app, ui, host, plot, "low", "Low", x_low);
    let hot_high = range_handle(st, app, ui, host, plot, "high", "High", x_high);
    for (i, band) in bands.iter().enumerate() {
        band_node(st, app, ui, host, plot, i, band, pos2(freq_to_x(plot, band.freq), sens_y(plot, band.gain)));
    }

    // ---- the corner readouts
    let p = ui.painter();
    let side_names = if param(host, "stereo") > 0.5 { ("Mid", "Side") } else { ("Left", "Right") };
    let mut x = plot.left() + 8.0;
    let y = plot.top() + 11.0;
    let key = |x: &mut f32, c: Color32, text: &str| {
        p.hline(*x..=*x + 12.0, y, Stroke::new(2.0, c));
        let w = p.text(pos2(*x + 16.0, y), Align2::LEFT_CENTER, text, regular(TINY_SIZE), pal.text_label).width();
        *x += 16.0 + w + 12.0;
    };
    if split {
        key(&mut x, pal.accent, side_names.0);
        key(&mut x, SIDE_B, side_names.1);
    } else {
        key(&mut x, pal.accent, "Reduction");
    }
    key(&mut x, al(pal.text_bright, 0.85), "Sensitivity");
    let text = if deepest >= 0.05 { format!("-{deepest:.1} dB") } else { "0.0 dB".to_string() };
    p.text(pos2(plot.right() - 44.0, y), Align2::RIGHT_CENTER, text, bold(LABEL_SIZE), if deepest >= 0.05 { pal.accent } else { pal.text_faint });

    // ---- the line under the graph: the band (or handle) in hand
    let line = pos2(panel.left() + 12.0, panel.bottom() + 4.0 + READOUT_H * 0.5);
    let text = if hot_low {
        format!("Low  \u{00B7}  {}", hz_text(low))
    } else if hot_high {
        format!("High  \u{00B7}  {}", hz_text(high))
    } else if let Some(b) = st.shown_band.and_then(|i| bands.get(i).map(|b| (i, b))) {
        let (i, b) = b;
        let state = if b.on { "" } else { "  \u{00B7}  off" };
        format!("Band {}  \u{00B7}  {}  \u{00B7}  {} sensitivity  \u{00B7}  Q {:.2}{state}", i + 1, hz_text(b.freq), signed_db(b.gain), b.q)
    } else {
        "Drag a band to tell Rez where to work harder or back off. The handles at the edges set the range.".to_string()
    };
    text_left(p, line, &text, CAPTION_SIZE, pal.text_faint);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

// ---------------------------------------------------------------------------- the controls

fn controls_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, row: Rect) {
    let pal = &PALETTE;
    let mut x = row.left();
    let panels: Vec<Rect> = GROUP_W
        .iter()
        .map(|w| {
            let r = Rect::from_min_size(pos2(x, row.top()), vec2(*w, row.height()));
            x += w + GAP;
            r
        })
        .collect();
    let [reduction, character, stereo, output] = panels.as_slice() else { return };
    for (r, name) in [(reduction, "Reduction"), (character, "Character"), (stereo, "Stereo"), (output, "Output")] {
        paint_panel(ui.painter(), *r, 10.0);
        group_label(ui.painter(), *r, name);
    }
    let body = |r: &Rect| Rect::from_min_max(pos2(r.left() + 8.0, r.top() + GROUP_LABEL_H), pos2(r.right() - 8.0, r.bottom() - 12.0));
    let seg_of = |b: Rect| Rect::from_min_size(pos2(b.left() + 6.0, b.bottom() - SEG_H), vec2(b.width() - 12.0, SEG_H));
    let small_row = |b: Rect| keep_centre(Rect::from_min_max(b.min, pos2(b.right(), b.bottom() - SEG_H - 8.0)), b.width(), SMALL_KNOB + 28.0);

    // Reduction: Depth over Soft / Hard.
    let b = body(reduction);
    let depth = Rect::from_min_size(pos2(b.center().x - BIG_KNOB * 0.5, b.top()), vec2(BIG_KNOB, BIG_KNOB + 30.0));
    dial_ui(app, ui, host, &mut st.fam, depth, "depth", "Depth", true);
    family::segmented_param(app, ui, host, &mut st.fam, &FAMILY, seg_of(b), pal, "mode", None, 1, SelectionStyle::Filled, false);

    // Character: how narrow, how picky, how fast.
    let b = body(character);
    let dials = [("sharpness", "Sharpness"), ("selectivity", "Selectivity"), ("attack", "Attack"), ("release", "Release")];
    dial_row(app, ui, host, &mut st.fam, small_row(b), &dials);
    let note = "The shape of each cut, and how fast it comes and goes";
    ui.painter().text(seg_of(b).center(), Align2::CENTER_CENTER, note, regular(TINY_SIZE), pal.text_faint);

    // Stereo: Link and Balance over L/R or M/S.
    let b = body(stereo);
    dial_row(app, ui, host, &mut st.fam, small_row(b), &[("link", "Link"), ("balance", "Balance")]);
    family::segmented_param(app, ui, host, &mut st.fam, &FAMILY, seg_of(b), pal, "stereo", None, 1, SelectionStyle::Filled, false);

    // Output.
    let b = body(output);
    dial_row(app, ui, host, &mut st.fam, small_row(b), &[("mix", "Mix"), ("trim", "Trim")]);
    let note = if host.ins.sidechain.is_some() { "Listening to the sidechain" } else { "Listening to the input" };
    ui.painter().text(seg_of(b).center(), Align2::CENTER_CENTER, note, regular(TINY_SIZE), pal.text_faint);
    tip(ui, ui.id().with(("vst-dial", "link")), "How much each side also listens to the other: 100 % cuts both the same");
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    let full = host.rect;
    paint_background(ui.painter(), full);
    let b = full.shrink2(vec2(PAD_X, PAD_Y));
    let header = Rect::from_min_size(b.min, vec2(b.width(), HEADER_H));
    let graph = graph_panel(full);
    let controls = Rect::from_min_size(pos2(b.left(), graph.bottom() + 4.0 + READOUT_H + GAP), vec2(b.width(), CONTROLS_H));
    header_ui(st, app, ui, host, header);
    graph_ui(st, app, ui, host);
    controls_ui(st, app, ui, host, controls);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_groups_fill_the_width() {
        let used: f32 = GROUP_W.iter().sum::<f32>() + GAP * (GROUP_W.len() - 1) as f32;
        assert_eq!(used, SIZE.x - 2.0 * PAD_X);
    }

    #[test]
    fn the_sensitivity_scale_round_trips() {
        let plot = Rect::from_min_size(pos2(10.0, 20.0), vec2(800.0, 236.0));
        for db in [-24.0f32, -6.0, 0.0, 12.0] {
            assert!((y_to_sens(plot, sens_y(plot, db)) - db).abs() < 0.01, "{db}");
        }
        assert_eq!(cut_y(plot, 0.0), plot.top());
        assert!(cut_y(plot, 24.0) < plot.bottom());
    }

    #[test]
    fn readouts_read_naturally() {
        assert_eq!(hz_text(150.0), "150 Hz");
        assert_eq!(hz_text(2500.0), "2.50 kHz");
        assert_eq!(hz_text(12_000.0), "12.0 kHz");
        assert_eq!(signed_db(0.0), "0.0 dB");
        assert_eq!(signed_db(3.0), "+3.0 dB");
        assert_eq!(balance_text(0.0, false), "Even");
        assert_eq!(balance_text(-30.0, false), "Left 30 %");
        assert_eq!(balance_text(40.0, true), "Side 40 %");
    }
}
