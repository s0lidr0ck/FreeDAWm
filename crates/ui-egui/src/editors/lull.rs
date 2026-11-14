//! Lull's editor: FreeDAWm's own design (no VST behind it), built on [`crate::editor_kit`] in the
//! Engine family's look.
//!
//! - The header: the title, the preset strip, the editor's Undo and A/B, and Listen (hear the
//!   detector).
//! - The display: on the left the curve (output level against input level) with the threshold as
//!   a handle and the live level as a dot; on the right the last seconds of the detector's level
//!   and the output under it, the gain reduction hanging from the top, and the threshold as a
//!   line to drag (a second, dashed line shows it raised while a guarding key is loud).
//! - The dials: Threshold with Expand/Gate; the curve's shape (Range, Ratio, Knee, Hysteresis);
//!   its timing (Attack, Hold, Release, Lookahead); the detector (its two filters, what a key
//!   does, Guard and Guard Delay); and the output (Link, Mix, Trim).
//!
//! Every change goes through `mix.module_params`; a drag is one undo step. The slim layout
//! ([`slim`]) stacks the same sections at the classic drawer's width.

mod slim;

pub use slim::slim_scope_rect;

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, PillLook, PillStyle, SelectionStyle, pill_ex, pill_height};
use crate::editor_kit::family::{self, Family, FamilyState};
use crate::editor_kit::look::{CAPTION_SIZE, LABEL_SIZE, TINY_SIZE, paint_background, paint_panel, paint_title};
use crate::editor_kit::palette::al;
use crate::editor_kit::presets;
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, end_gesture, param, run_with, set_params_merged};
use crate::theme::{bold, regular};
use egui::{Align2, Color32, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::lull::{Curve, LIVE_GR, LIVE_LEVEL, LIVE_OPEN, LIVE_OUT, LIVE_THRESHOLD};
use soundcraft_mix::StripMeter;
use soundcraft_model::{ModuleId, Track};
use std::collections::VecDeque;

// ============================================================================ layout

const PAD_X: f32 = 24.0;
const PAD_Y: f32 = 15.0;
const GAP: f32 = 14.0;
const HEADER_H: f32 = 26.0;
const DISPLAY_H: f32 = 236.0;
const READOUT_H: f32 = 16.0;
const BIG_KNOB: f32 = 104.0;
const SMALL_KNOB: f32 = 62.0;
const SEG_H: f32 = 24.0;
const GROUP_LABEL_H: f32 = 18.0;
const ROW_A_H: f32 = GROUP_LABEL_H + BIG_KNOB + 30.0 + 8.0 + SEG_H + 12.0;
const ROW_B_H: f32 = GROUP_LABEL_H + SMALL_KNOB + 28.0 + 8.0 + SEG_H + 12.0;
const HEIGHT: f32 = PAD_Y * 2.0 + HEADER_H + GAP + DISPLAY_H + 4.0 + READOUT_H + GAP + ROW_A_H + GAP + ROW_B_H;

/// The editor's size.
pub const SIZE: Vec2 = vec2(860.0, HEIGHT);

/// Lull's accent: a soft sea green.
pub const ACCENT: u32 = 0xFF6F_D6B4;
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(ACCENT));
/// The threshold as a guarding key has raised it.
const GUARD_COLOUR: Color32 = argb(0xFFFF_C46B);

/// A/B and Undo walk every parameter.
const FAMILY: Family = Family::new("Lull", &[]);

/// The widths of the first row's groups (Threshold, Shape, Time) and the second's (Detector,
/// Output); each row fills the editor with [`GAP`] between its groups.
const ROW_A_W: [f32; 3] = [150.0, 331.0, 303.0];
const ROW_B_W: [f32; 2] = [490.0, 308.0];

// ============================================================================ the display's scales

/// The level axis of the curve and the scope: this many dB below full scale to full scale.
const LEVEL_FLOOR_DB: f32 = -80.0;
/// The gain reduction shown from the top of the scope down: this many dB over [`GR_SPAN`] of it.
const GR_RANGE_DB: f32 = 40.0;
const GR_SPAN: f32 = 0.5;
/// How many frames of history the scope keeps (about six seconds at 30 a second).
const HISTORY: usize = 180;

/// A level in dB as a share of the level axis (0 at the floor, 1 at full scale).
fn level01(db: f32) -> f32 {
    if db.is_finite() { ((db - LEVEL_FLOOR_DB) / -LEVEL_FLOOR_DB).clamp(0.0, 1.0) } else { 0.0 }
}

fn level_y(r: Rect, db: f32) -> f32 {
    r.bottom() - r.height() * level01(db)
}

fn y_to_level(r: Rect, y: f32) -> f32 {
    LEVEL_FLOOR_DB * (1.0 - ((r.bottom() - y) / r.height().max(1.0)).clamp(0.0, 1.0))
}

fn level_x(r: Rect, db: f32) -> f32 {
    r.left() + r.width() * level01(db)
}

fn x_to_level(r: Rect, x: f32) -> f32 {
    LEVEL_FLOOR_DB * (1.0 - ((x - r.left()) / r.width().max(1.0)).clamp(0.0, 1.0))
}

// ============================================================================ the editor

/// One frame of the scope: the detector's level, the output's level, the gain reduction, the
/// threshold in force (all dB) and whether the gate was open.
#[derive(Clone, Copy, Debug, Default)]
struct Frame {
    level: f32,
    out: f32,
    gr: f32,
    threshold: f32,
    open: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    fam: FamilyState,
    /// The scope's history, newest last.
    history: VecDeque<Frame>,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// The slim layout's More section is open.
    slim_more: bool,
}

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "lull";

    fn size(&self, _ins: &soundcraft_model::Insert) -> Vec2 {
        if self.slim { crate::editor_kit::slim::size(self.slim_h) } else { SIZE }
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        if self.slim {
            slim::editor_ui(self, app, ui, host);
        } else {
            editor_ui(self, app, ui, host);
        }
    }
}

/// The static curve as the module's parameters stand.
fn curve_of(host: &Host) -> Curve {
    Curve {
        gate: param(host, "mode") > 0.5,
        threshold: param(host, "threshold"),
        range: param(host, "range"),
        ratio: param(host, "ratio"),
        knee: param(host, "knee"),
    }
}

fn keep_centre(r: Rect, w: f32, h: f32) -> Rect {
    Rect::from_center_size(r.center(), vec2(w.min(r.width()), h.min(r.height())))
}

/// A parameter's readout: its own text, except where a setting means "off".
fn value_text(host: &Host, id: &str) -> String {
    let v = param(host, id);
    match id {
        "sc_hp" if v <= 20.5 => "Off".into(),
        "sc_lp" if v >= 19_900.0 => "Off".into(),
        "lookahead" if v < 0.05 => "Off".into(),
        "guard" if v < 0.05 => "Off".into(),
        "trim" => {
            let v = if v.abs() < 0.05 { 0.0 } else { v };
            format!("{}{v:.1} dB", if v > 0.0 { "+" } else { "" })
        }
        _ => host.info.param(id).map_or_else(String::new, |p| p.format(v)),
    }
}

/// One dial bound to parameter `id`; `inactive` when it does nothing as things stand.
#[allow(clippy::too_many_arguments)]
fn dial_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect, id: &str, label: &str, big: bool, inactive: bool) {
    let value_text = value_text(host, id);
    let o =
        DialOpts { label, value_text: &value_text, default: family::default01(host, id), small: !big, hero: big, inactive, ..DialOpts::default() };
    family::dial_param(app, ui, host, fam, &FAMILY, r, &PALETTE, id, &o);
}

/// Whether a dial does anything as the module stands: Ratio only expands, and the Guard dials
/// only matter while a key is picked and it guards.
fn inactive(host: &Host, id: &str) -> bool {
    match id {
        "ratio" => param(host, "mode") > 0.5,
        "guard" | "guard_delay" => host.ins.sidechain.is_none() || param(host, "key") > 0.5,
        _ => false,
    }
}

/// A row of small dials centred in `r`.
fn dial_row(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect, dials: &[(&str, &str)]) {
    let cell_w = (r.width() / dials.len().max(1) as f32).trunc();
    for (i, (id, label)) in dials.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(r.left() + cell_w * i as f32, r.top()), vec2(cell_w, r.height()));
        dial_ui(app, ui, host, fam, keep_centre(cell, (cell_w - 4.0).min(96.0), SMALL_KNOB + 28.0), id, label, false, inactive(host, id));
    }
}

fn group_label(p: &egui::Painter, panel: Rect, text: &str) {
    p.text(pos2(panel.left() + 12.0, panel.top() + 13.0), Align2::LEFT_CENTER, text.to_uppercase(), regular(TINY_SIZE), PALETTE.text_faint);
}

// ---------------------------------------------------------------------------- the header

/// The header's pills in their rectangles: the editor's Undo, A and B, and Listen.
fn header_pills(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, [undo, a, b, listen]: [Rect; 4]) {
    let pal = &PALETTE;
    let plain = PillLook { tracked: false, ..PillLook::default() };
    let look = PillLook { dimmed: !st.fam.can_undo(), ..plain };
    if pill_ex(ui, undo, "lull-undo", pal, "Undo", false, PillStyle::Small, look).clicked() {
        family::do_undo(app, host, &mut st.fam);
    }
    if pill_ex(ui, a, "lull-a", pal, "A", !st.fam.on_b(), PillStyle::Small, plain).clicked() {
        family::select_ab(app, host, &mut st.fam, &FAMILY, false);
    }
    if pill_ex(ui, b, "lull-b", pal, "B", st.fam.on_b(), PillStyle::Small, plain).clicked() {
        family::select_ab(app, host, &mut st.fam, &FAMILY, true);
    }
    let on = param(host, "listen") > 0.5;
    let look = PillLook { active_colour: Some(pal.warn), ..plain };
    let resp = pill_ex(ui, listen, "lull-listen", pal, "Listen", on, PillStyle::Small, look);
    if resp.clicked() {
        family::pill_set(app, host, &mut st.fam, &FAMILY, "listen", if on { 0.0 } else { 1.0 });
    }
    resp.on_hover_text("Hear what the detector hears: the sound through its two filters, or the key in Trigger");
}

/// The preset strip (previous, the name, next) in its rectangles.
fn preset_strip_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, [prev, name, next]: [Rect; 3]) {
    let pal = &PALETTE;
    for (rect, left) in [(prev, true), (next, false)] {
        if family::arrow_pill(ui, rect, ("lull-preset-step", left), pal, left).clicked() {
            let out = presets::step(app, host, &mut st.fam.presets, if left { -1 } else { 1 });
            if out.recalled.is_some() {
                end_gesture(app);
            }
        }
    }
    let shown = presets::shown_name(host).to_string();
    let dirty = presets::is_dirty(app, host, &mut st.fam.presets);
    if family::preset_name(ui, name, pal, &shown, dirty).clicked() {
        presets::open(app, host, &mut st.fam.presets);
    }
}

/// What both layouts do first: the preset browser, which covers the whole editor while it is
/// open (then `true`: paint nothing else).
fn prelude(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host) -> bool {
    if !st.init {
        st.init = true;
        st.fam.presets.category = "All".into();
    }
    family::browser(app, ui, host, &PALETTE, &mut st.fam, &FAMILY, "")
}

fn header_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, header: Rect) {
    let title_w = paint_title(ui.painter(), header, "Lull", "Gate", &PALETTE);
    let h = pill_height(PillStyle::Small);
    let mut right = header.right();
    let mut place = |w: f32, gap: f32| {
        let r = Rect::from_min_size(pos2(right - w, header.center().y - h * 0.5), vec2(w, h));
        right -= w + gap;
        r
    };
    let listen = place(60.0, 10.0);
    let b = place(28.0, 3.0);
    let a = place(28.0, 8.0);
    let undo = place(48.0, 0.0);
    header_pills(st, app, ui, host, [undo, a, b, listen]);
    let free = Rect::from_min_max(pos2(header.left() + title_w + 16.0, header.top()), pos2(undo.left() - 16.0, header.bottom()));
    let name_w = (free.width() - 2.0 * 30.0).clamp(120.0, 240.0);
    let strip = Rect::from_center_size(free.center(), vec2(name_w + 60.0, h + 2.0));
    let prev = Rect::from_min_size(pos2(strip.left(), strip.center().y - h * 0.5), vec2(24.0, h));
    let next = Rect::from_min_size(pos2(strip.right() - 24.0, strip.center().y - h * 0.5), vec2(24.0, h));
    let name = Rect::from_min_max(pos2(prev.right() + 6.0, strip.top()), pos2(next.left() - 6.0, strip.bottom()));
    preset_strip_ui(st, app, ui, host, [prev, name, next]);
}

// ---------------------------------------------------------------------------- the display

/// Takes this frame's live values into the history (nothing while the module is not playing).
fn feed_history(st: &mut Editor, host: &Host) -> Option<Frame> {
    let live = crate::editor_kit::live(host)?;
    let f = Frame {
        level: live.get(LIVE_LEVEL),
        out: live.get(LIVE_OUT),
        gr: live.get(LIVE_GR).max(0.0),
        threshold: live.get(LIVE_THRESHOLD),
        open: live.get(LIVE_OPEN) > 0.5,
    };
    st.history.push_back(f);
    while st.history.len() > HISTORY {
        st.history.pop_front();
    }
    Some(f)
}

/// A drag of the threshold: `to_db` turns the pointer into a level. Returns whether it is hot.
fn threshold_drag(
    st: &mut Editor,
    app: &mut SoundApp,
    ui: &mut Ui,
    host: &Host,
    hit: Rect,
    salt: &str,
    horizontal: bool,
    to_db: impl Fn(Pos2) -> f32,
) -> bool {
    let resp = ui.interact(hit, ui.id().with(("lull-threshold", salt)), Sense::click_and_drag());
    if resp.drag_started() {
        end_gesture(app);
        family::push_undo(&mut st.fam, host, &FAMILY);
    }
    if resp.dragged()
        && let (Some(at), Some(info)) = (resp.interact_pointer_pos(), host.info.param("threshold"))
    {
        set_params_merged(app, host, &[("threshold", info.clamp(to_db(at)))], "lull-threshold");
    }
    if resp.drag_stopped() {
        end_gesture(app);
    }
    if resp.double_clicked()
        && let Some(info) = host.info.param("threshold")
    {
        family::set(app, host, &mut st.fam, &FAMILY, "threshold", info.default);
    }
    let hot = resp.hovered() || resp.dragged();
    if hot {
        ui.ctx().set_cursor_icon(if horizontal { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical });
    }
    resp.on_hover_text("Threshold: drag to set, double-click to reset");
    hot
}

/// The curve: output level against input level, the threshold as a handle, the live level as a
/// dot.
fn curve_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, panel: Rect, now: Option<Frame>) {
    let pal = &PALETTE;
    paint_panel(ui.painter(), panel, 10.0);
    let plot = panel.shrink(12.0);
    let p = ui.painter().with_clip_rect(panel.intersect(ui.clip_rect()));
    for db in [-60.0f32, -40.0, -20.0] {
        p.vline(level_x(plot, db), plot.y_range(), Stroke::new(1.0, al(Color32::WHITE, 0.05)));
        p.hline(plot.x_range(), level_y(plot, db), Stroke::new(1.0, al(Color32::WHITE, 0.05)));
    }
    // Unity: what goes in comes out.
    p.line_segment([plot.left_bottom(), plot.right_top()], Stroke::new(1.0, al(Color32::WHITE, 0.12)));
    let curve = curve_of(host);
    let steps = (plot.width() / 2.0) as usize;
    let points: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let db = LEVEL_FLOOR_DB * (1.0 - i as f32 / steps.max(1) as f32);
            pos2(level_x(plot, db), level_y(plot, db - curve.reduction(db)))
        })
        .collect();
    for w in points.windows(2) {
        if let [a, b] = w {
            p.add(Shape::convex_polygon(vec![*a, *b, pos2(b.x, plot.bottom()), pos2(a.x, plot.bottom())], al(pal.accent, 0.10), Stroke::NONE));
        }
    }
    p.add(Shape::line(points, Stroke::new(2.0, pal.accent)));
    p.rect_stroke(plot, 0.0, Stroke::new(1.0, al(Color32::WHITE, 0.06)), StrokeKind::Inside);

    // The threshold: a vertical handle.
    let x = level_x(plot, curve.threshold);
    let hit = Rect::from_min_max(pos2(x - 6.0, plot.top()), pos2(x + 6.0, plot.bottom()));
    let hot = threshold_drag(st, app, ui, host, hit, "curve", true, |at| x_to_level(plot, at.x));
    let p = ui.painter().with_clip_rect(panel.intersect(ui.clip_rect()));
    let c = if hot { pal.text_bright } else { pal.text_dim };
    p.vline(x, plot.y_range(), Stroke::new(if hot { 1.5 } else { 1.0 }, al(c, if hot { 0.9 } else { 0.5 })));
    p.rect_filled(Rect::from_center_size(pos2(x, plot.top() + 9.0), vec2(8.0, 12.0)), 2.0, al(c, if hot { 1.0 } else { 0.7 }));

    // The live level, on the curve as it is being applied.
    if let Some(f) = now.filter(|f| f.level > LEVEL_FLOOR_DB) {
        let at = pos2(level_x(plot, f.level), level_y(plot, f.level - f.gr));
        p.circle_filled(at, 7.0, al(pal.accent, 0.25));
        p.circle_filled(at, 4.0, if f.open { pal.accent } else { pal.text_dim });
    }
    p.text(pos2(plot.left() + 6.0, plot.top() + 9.0), Align2::LEFT_CENTER, "OUT", regular(TINY_SIZE), pal.text_faint);
    p.text(pos2(plot.right() - 6.0, plot.bottom() - 9.0), Align2::RIGHT_CENTER, "IN", regular(TINY_SIZE), pal.text_faint);
}

/// The scope: the detector's level and the output over the last seconds, the gain reduction
/// from the top, and the threshold as a line to drag. `compact`: fewer labels (the slim layout).
fn scope_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, panel: Rect, now: Option<Frame>, compact: bool) {
    let pal = &PALETTE;
    paint_panel(ui.painter(), panel, 10.0);
    let plot = scope_plot(panel);
    let p = ui.painter().with_clip_rect(panel.intersect(ui.clip_rect()));
    for db in [-60.0f32, -40.0, -20.0] {
        let y = level_y(plot, db);
        p.hline(plot.x_range(), y, Stroke::new(1.0, al(Color32::WHITE, 0.05)));
        if !compact {
            p.text(pos2(plot.left() + 4.0, y - 6.0), Align2::LEFT_CENTER, format!("{db:.0}"), regular(TINY_SIZE), pal.text_faint);
        }
    }
    // The history, oldest at the left; the newest frame sits at the right edge.
    let n = st.history.len();
    let col_w = plot.width() / HISTORY as f32;
    let x_of = |i: usize| plot.right() - col_w * (n - i) as f32;
    let gr_y = |gr: f32| plot.top() + (gr / GR_RANGE_DB).clamp(0.0, 1.0 / GR_SPAN) * GR_SPAN * plot.height();
    let mut gr_line = Vec::with_capacity(n + 1);
    for (i, f) in st.history.iter().enumerate() {
        let (x0, x1) = (x_of(i), x_of(i) + col_w + 0.5);
        p.rect_filled(Rect::from_min_max(pos2(x0, level_y(plot, f.level)), pos2(x1, plot.bottom())), 0.0, al(pal.text_dim, 0.16));
        p.rect_filled(
            Rect::from_min_max(pos2(x0, level_y(plot, f.out)), pos2(x1, plot.bottom())),
            0.0,
            al(pal.accent, if f.open { 0.34 } else { 0.18 }),
        );
        if f.gr > 0.05 {
            p.rect_filled(Rect::from_min_max(pos2(x0, plot.top()), pos2(x1, gr_y(f.gr))), 0.0, al(pal.accent, 0.22));
        }
        gr_line.push(pos2(x0, gr_y(f.gr)));
        if i + 1 == n {
            gr_line.push(pos2(x1, gr_y(f.gr)));
        }
    }
    if gr_line.len() > 1 {
        p.add(Shape::line(gr_line, Stroke::new(1.5, pal.accent)));
    }
    p.rect_stroke(plot, 0.0, Stroke::new(1.0, al(Color32::WHITE, 0.06)), StrokeKind::Inside);

    // The threshold, and above it the threshold as a guarding key has raised it.
    let threshold = param(host, "threshold");
    let y = level_y(plot, threshold);
    let hit = Rect::from_min_max(pos2(plot.left(), y - 6.0), pos2(plot.right(), y + 6.0));
    let hot = threshold_drag(st, app, ui, host, hit, "scope", false, |at| y_to_level(plot, at.y));
    let p = ui.painter().with_clip_rect(panel.intersect(ui.clip_rect()));
    let c = if hot { pal.text_bright } else { pal.text_dim };
    p.hline(plot.x_range(), y, Stroke::new(if hot { 1.5 } else { 1.0 }, al(c, if hot { 0.95 } else { 0.6 })));
    p.rect_filled(Rect::from_center_size(pos2(plot.right() - 9.0, y), vec2(12.0, 8.0)), 2.0, al(c, if hot { 1.0 } else { 0.75 }));
    if let Some(f) = now.filter(|f| f.threshold > threshold + 0.2) {
        let raised = level_y(plot, f.threshold);
        let mut x = plot.left();
        while x < plot.right() {
            p.hline(x..=(x + 6.0).min(plot.right()), raised, Stroke::new(1.0, GUARD_COLOUR));
            x += 10.0;
        }
    }

    // The corner readouts: open or closed, and the reduction now.
    let top = plot.top() + 11.0;
    let (lamp, word) = match now {
        Some(f) if f.open => (pal.accent, "Open"),
        Some(_) => (pal.text_faint, "Closed"),
        None => (pal.led_off_ring, "Idle"),
    };
    p.circle_filled(pos2(plot.left() + 12.0, top), 4.0, lamp);
    p.text(pos2(plot.left() + 22.0, top), Align2::LEFT_CENTER, word, regular(TINY_SIZE), pal.text_label);
    let gr = now.map_or(0.0, |f| f.gr);
    let text = if gr >= 0.05 { format!("-{gr:.1} dB") } else { "0.0 dB".to_string() };
    p.text(pos2(plot.right() - 8.0, top), Align2::RIGHT_CENTER, text, bold(LABEL_SIZE), if gr >= 0.05 { pal.accent } else { pal.text_faint });
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

/// The scope's plot inside its panel.
fn scope_plot(panel: Rect) -> Rect {
    panel.shrink(10.0)
}

/// The display's two panels in an editor drawn at `frame`: the curve (a square at the left) and
/// the scope.
fn display_panels(frame: Rect) -> (Rect, Rect) {
    let top = frame.top() + PAD_Y + HEADER_H + GAP;
    let curve = Rect::from_min_size(pos2(frame.left() + PAD_X, top), vec2(DISPLAY_H, DISPLAY_H));
    let scope = Rect::from_min_max(pos2(curve.right() + GAP, top), pos2(frame.right() - PAD_X, top + DISPLAY_H));
    (curve, scope)
}

/// The scope's plot area in an editor drawn at `frame` (for tests and scripts): a level of `db`
/// is at [`scope_level_y`] in it.
pub fn scope_rect(frame: Rect) -> Rect {
    scope_plot(display_panels(frame).1)
}

/// The curve's plot area in an editor drawn at `frame` (for tests and scripts): a level of `db`
/// is at [`curve_level_x`] in it.
pub fn curve_rect(frame: Rect) -> Rect {
    display_panels(frame).0.shrink(12.0)
}

/// Where a level of `db` sits on a scope plot's level axis.
pub fn scope_level_y(plot: Rect, db: f32) -> f32 {
    level_y(plot, db)
}

/// Where a level of `db` sits on a curve plot's input axis.
pub fn curve_level_x(plot: Rect, db: f32) -> f32 {
    level_x(plot, db)
}

/// The line under the display: what the settings come to, in words.
fn summary(host: &Host) -> String {
    let c = curve_of(host);
    let what = if c.gate {
        format!("Gate: {:.0} dB down below {:.0} dB", c.range, c.threshold)
    } else {
        format!("Expander: {:.1}:1 below {:.0} dB, {:.0} dB at most", c.ratio, c.threshold, c.range)
    };
    let key = match (&host.ins.sidechain, param(host, "key") > 0.5) {
        (None, _) => "no key (pick one in the strip above to guard against bleed or to trigger)".to_string(),
        (Some(_), true) => "the key opens it".to_string(),
        (Some(_), false) => format!("the key guards it by up to {:.0} dB", param(host, "guard")),
    };
    format!("{what}  \u{00B7}  {key}")
}

// ---------------------------------------------------------------------------- the controls

fn panels_of(row: Rect, widths: &[f32]) -> Vec<Rect> {
    let mut x = row.left();
    widths
        .iter()
        .map(|w| {
            let r = Rect::from_min_size(pos2(x, row.top()), vec2(*w, row.height()));
            x += w + GAP;
            r
        })
        .collect()
}

fn controls_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, row_a: Rect, row_b: Rect) {
    let pal = &PALETTE;
    let a = panels_of(row_a, &ROW_A_W);
    let b = panels_of(row_b, &ROW_B_W);
    let ([threshold, shape, time], [detector, output]) = (a.as_slice(), b.as_slice()) else { return };
    for (r, name) in [(threshold, "Threshold"), (shape, "Shape"), (time, "Time"), (detector, "Detector"), (output, "Output")] {
        paint_panel(ui.painter(), *r, 10.0);
        group_label(ui.painter(), *r, name);
    }
    let body = |r: &Rect| Rect::from_min_max(pos2(r.left() + 8.0, r.top() + GROUP_LABEL_H), pos2(r.right() - 8.0, r.bottom() - 12.0));
    let seg_of = |b: Rect| Rect::from_min_size(pos2(b.left() + 6.0, b.bottom() - SEG_H), vec2(b.width() - 12.0, SEG_H));
    let small_row = |b: Rect| keep_centre(Rect::from_min_max(b.min, pos2(b.right(), b.bottom() - SEG_H - 8.0)), b.width(), SMALL_KNOB + 28.0);
    let note = |ui: &Ui, b: Rect, text: &str| {
        ui.painter().text(seg_of(b).center(), Align2::CENTER_CENTER, text, regular(TINY_SIZE), pal.text_faint);
    };

    // Threshold over Expand / Gate.
    let bd = body(threshold);
    let knob = Rect::from_min_size(pos2(bd.center().x - BIG_KNOB * 0.5, bd.top()), vec2(BIG_KNOB, BIG_KNOB + 30.0));
    dial_ui(app, ui, host, &mut st.fam, knob, "threshold", "Threshold", true, false);
    family::segmented_param(app, ui, host, &mut st.fam, &FAMILY, seg_of(bd), pal, "mode", None, 1, SelectionStyle::Filled, false);

    // Shape: how far down, how steeply, how gradually, and how sure.
    let bd = body(shape);
    dial_row(app, ui, host, &mut st.fam, small_row(bd), &[("range", "Range"), ("ratio", "Ratio"), ("knee", "Knee"), ("hysteresis", "Hysteresis")]);
    note(ui, bd, "Depth, slope, softness, and how sure it must be before it closes");

    // Time.
    let bd = body(time);
    dial_row(
        app,
        ui,
        host,
        &mut st.fam,
        small_row(bd),
        &[("attack", "Attack"), ("hold", "Hold"), ("release", "Release"), ("lookahead", "Lookahead")],
    );
    note(ui, bd, "Lookahead opens it ahead of the sound, and is the latency");

    // Detector: its filters, then what a key does.
    let bd = body(detector);
    let dials = [("sc_hp", "High-Pass"), ("sc_lp", "Low-Pass"), ("guard", "Guard"), ("guard_delay", "Guard Delay")];
    dial_row(
        app,
        ui,
        host,
        &mut st.fam,
        keep_centre(Rect::from_min_max(bd.min, pos2(bd.right(), bd.bottom() - SEG_H - 8.0)), bd.width(), SMALL_KNOB + 28.0),
        &dials,
    );
    let seg = seg_of(bd);
    let label_w = 34.0;
    ui.painter().text(pos2(seg.left(), seg.center().y), Align2::LEFT_CENTER, "Key", regular(LABEL_SIZE), pal.text_body);
    let seg = Rect::from_min_max(pos2(seg.left() + label_w, seg.top()), pos2(seg.left() + label_w + 190.0, seg.bottom()));
    family::segmented_param(app, ui, host, &mut st.fam, &FAMILY, seg, pal, "key", None, 1, SelectionStyle::Filled, false);
    let hint = if param(host, "key") > 0.5 { "the key opens the gate" } else { "a loud key raises the threshold" };
    ui.painter().text(pos2(seg.right() + 12.0, seg.center().y), Align2::LEFT_CENTER, hint, regular(TINY_SIZE), pal.text_faint);

    // Output.
    let bd = body(output);
    dial_row(app, ui, host, &mut st.fam, small_row(bd), &[("link", "Link"), ("mix", "Mix"), ("trim", "Trim")]);
    note(ui, bd, "Link: how much each side follows the louder one");
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    // Scripts: `ui.mix.drawer {tab: "Presets"}` opens the browser (the full editor has no other tab).
    let _ = family::take_drawer_tab(app, host, &mut st.fam);
    if prelude(st, app, ui, host) {
        return;
    }
    let full = host.rect;
    paint_background(ui.painter(), full);
    let b = full.shrink2(vec2(PAD_X, PAD_Y));
    let header = Rect::from_min_size(b.min, vec2(b.width(), HEADER_H));
    let (curve, scope) = display_panels(full);
    let readout = pos2(b.left() + 12.0, scope.bottom() + 4.0 + READOUT_H * 0.5);
    let row_a = Rect::from_min_size(pos2(b.left(), scope.bottom() + 4.0 + READOUT_H + GAP), vec2(b.width(), ROW_A_H));
    let row_b = Rect::from_min_size(pos2(b.left(), row_a.bottom() + GAP), vec2(b.width(), ROW_B_H));
    header_ui(st, app, ui, host, header);
    let now = feed_history(st, host);
    curve_ui(st, app, ui, host, curve, now);
    scope_ui(st, app, ui, host, scope, now, false);
    ui.painter().text(readout, Align2::LEFT_CENTER, summary(host), regular(CAPTION_SIZE), PALETTE.text_faint);
    controls_ui(st, app, ui, host, row_a, row_b);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_groups_fill_the_width() {
        for widths in [ROW_A_W.as_slice(), ROW_B_W.as_slice()] {
            let used: f32 = widths.iter().sum::<f32>() + GAP * (widths.len() - 1) as f32;
            assert_eq!(used, SIZE.x - 2.0 * PAD_X);
        }
    }

    #[test]
    fn the_level_axis_round_trips() {
        let r = Rect::from_min_size(pos2(10.0, 20.0), vec2(400.0, 216.0));
        for db in [-80.0f32, -40.0, -12.0, 0.0] {
            assert!((y_to_level(r, level_y(r, db)) - db).abs() < 0.01, "{db}");
            assert!((x_to_level(r, level_x(r, db)) - db).abs() < 0.01, "{db}");
        }
        assert_eq!(level_y(r, 0.0), r.top());
        assert_eq!(level_y(r, -200.0), r.bottom());
        assert_eq!(level_y(r, f32::NAN), r.bottom());
    }
}
