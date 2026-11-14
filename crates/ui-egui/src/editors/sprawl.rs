//! Sprawl's editor: FreeDAWm's own design (no VST behind it), built on [`crate::editor_kit`] in
//! the Engine family's look.
//!
//! - The header: the title, the preset strip (step through the presets, or open the browser from
//!   the name), the editor's Undo and A/B, and Mono Safe.
//! - The scope: the output as a vectorscope (mono is a vertical line, wide is a cloud, out of
//!   phase is a horizontal one) over a correlation meter.
//! - The bands, on a log frequency axis: the four bands between their three splits. A band's bar
//!   is its Width (drag up and down; double-click: 100 %), a split is dragged left
//!   and right (double-click: its default). Under each band, what it holds right now: a bar as
//!   wide as its side share, in the warning colour while its left and right oppose each other.
//! - The dials: Width, Stereoize, the four bands' widths and Trim.
//!
//! Every change goes through `mix.module_params`; a drag is one undo step. The slim layout
//! ([`slim`]) stacks the same sections at the classic drawer's width.

mod slim;

pub use slim::{slim_band_pos, slim_bands_rect};

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, PillLook, PillStyle, pill_ex, pill_height, tip};
use crate::editor_kit::family::{self, Family, FamilyState};
use crate::editor_kit::look::{CAPTION_SIZE, LABEL_SIZE, TINY_SIZE, paint_background, paint_panel, paint_title};
use crate::editor_kit::palette::al;
use crate::editor_kit::presets;
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, end_gesture, param, run_with, set_params_merged};
use crate::module_graphs::{freq_to_x, x_to_freq};
use crate::theme::{bold, regular};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::sprawl::{
    self as dsp, BAND_IDS, BAND_NAMES, BANDS, LIVE_BAND_CORRELATION, LIVE_BAND_GAIN, LIVE_BAND_SIDE, LIVE_CORRELATION, LIVE_SCOPE_L, LIVE_SCOPE_R,
    MAX_WIDTH, MIN_SPLIT_RATIO, SCOPE_POINTS, SPLIT_HZ, SPLIT_IDS, SPLITS,
};
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
const NOTE_H: f32 = 24.0;
const GROUP_LABEL_H: f32 = 18.0;
const CONTROLS_H: f32 = GROUP_LABEL_H + BIG_KNOB + 30.0 + 8.0 + NOTE_H + 12.0;
const HEIGHT: f32 = PAD_Y * 2.0 + HEADER_H + GAP + GRAPH_H + 4.0 + READOUT_H + GAP + CONTROLS_H;

/// The editor's size.
pub const SIZE: Vec2 = vec2(860.0, HEIGHT);

/// Sprawl's accent: leaf green.
pub const ACCENT: u32 = 0xFFA8_D86B;
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(ACCENT));

/// A/B and Undo walk every parameter.
const FAMILY: Family = Family::new("Sprawl", &[]);

/// The widths of the four control groups (they fill the editor with [`GAP`] between them).
const GROUP_W: [f32; 4] = [150.0, 150.0, 316.0, 154.0];

/// The lane under the bands that shows what each band holds, and the room for the axis labels.
const LANE_H: f32 = 12.0;
const AXIS_H: f32 = 16.0;
/// The room at the bands' right for the width scale's labels.
const SCALE_W: f32 = 30.0;
/// A split is grabbed within this many points of its line.
const SPLIT_GRAB: f32 = 6.0;

// ============================================================================ the bands' scale

fn width_y(plot: Rect, width: f32) -> f32 {
    plot.bottom() - (width / MAX_WIDTH).clamp(0.0, 1.0) * plot.height()
}

/// The left and right edges of band `i` in `plot`, with the splits `at`.
fn band_x(plot: Rect, at: &[f32; SPLITS], i: usize) -> (f32, f32) {
    let edge = |k: usize| at.get(k).map_or(plot.right(), |hz| freq_to_x(plot, *hz));
    (if i == 0 { plot.left() } else { edge(i - 1) }, edge(i))
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

fn correlation_text(c: f32) -> String {
    let c = if c.abs() < 0.005 { 0.0 } else { c };
    format!("{}{c:.2}", if c > 0.0 { "+" } else { "" })
}

/// A Width in words: where 0, 100 and 200 % are.
fn width_text(w: f32) -> String {
    if w < 0.5 { "Mono".into() } else { format!("{w:.0} %") }
}

/// Where a band lies, in words.
fn band_range(at: &[f32; SPLITS], i: usize) -> String {
    match (i.checked_sub(1).and_then(|k| at.get(k)), at.get(i)) {
        (None, Some(hi)) => format!("below {}", hz_text(*hi)),
        (Some(lo), Some(hi)) => format!("{} to {}", hz_text(*lo), hz_text(*hi)),
        (Some(lo), None) => format!("above {}", hz_text(*lo)),
        (None, None) => String::new(),
    }
}

fn value_text(host: &Host, id: &str) -> String {
    let v = param(host, id);
    match id {
        "trim" => signed_db(v),
        "stereoize" if v < 0.5 => "Off".into(),
        "stereoize" => format!("{v:.0} %"),
        _ => width_text(v),
    }
}

// ============================================================================ the editor

#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    fam: FamilyState,
    /// The band last hovered or dragged: its numbers stay in the readout.
    shown_band: Option<usize>,
    /// The readings as drawn (eased between frames).
    correlation: f32,
    band_correlation: [f32; BANDS],
    band_side: [f32; BANDS],
    /// The scope's zoom (it follows the level, so quiet sound is still a picture).
    scope_gain: f32,
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
    const PLUGIN: &'static str = "sprawl";

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

/// The splits in force for the module's settings.
fn splits_of(host: &Host) -> [f32; SPLITS] {
    dsp::splits(SPLIT_IDS.map(|id| param(host, id)), host.sr as f32)
}

fn keep_centre(r: Rect, w: f32, h: f32) -> Rect {
    Rect::from_center_size(r.center(), vec2(w.min(r.width()), h.min(r.height())))
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

/// The band dials, lowest first: (parameter, label).
fn band_dials() -> [(&'static str, &'static str); BANDS] {
    std::array::from_fn(|i| (BAND_IDS[i], BAND_NAMES[i]))
}

// ---------------------------------------------------------------------------- the header

/// The header's pills in their rectangles: the editor's Undo, A and B, and Mono Safe.
fn header_pills(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, [undo, a, b, safe]: [Rect; 4]) {
    let pal = &PALETTE;
    let plain = PillLook { tracked: false, ..PillLook::default() };
    let look = PillLook { dimmed: !st.fam.can_undo(), ..plain };
    if pill_ex(ui, undo, "sprawl-undo", pal, "Undo", false, PillStyle::Small, look).clicked() {
        family::do_undo(app, host, &mut st.fam);
    }
    if pill_ex(ui, a, "sprawl-a", pal, "A", !st.fam.on_b(), PillStyle::Small, plain).clicked() {
        family::select_ab(app, host, &mut st.fam, &FAMILY, false);
    }
    if pill_ex(ui, b, "sprawl-b", pal, "B", st.fam.on_b(), PillStyle::Small, plain).clicked() {
        family::select_ab(app, host, &mut st.fam, &FAMILY, true);
    }
    let safe_on = param(host, "safe") > 0.5;
    let resp = pill_ex(ui, safe, "sprawl-safe", pal, "Mono Safe", safe_on, PillStyle::Small, plain);
    if resp.clicked() {
        family::pill_set(app, host, &mut st.fam, &FAMILY, "safe", if safe_on { 0.0 } else { 1.0 });
    }
    resp.on_hover_text("Hold a widened band back while its left and right would cancel in mono");
}

/// The preset strip (previous, the name, next) in its rectangles: the arrows step through the
/// presets, the name opens the browser.
fn preset_strip_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, [prev, name, next]: [Rect; 3]) {
    let pal = &PALETTE;
    for (rect, left) in [(prev, true), (next, false)] {
        if family::arrow_pill(ui, rect, ("sprawl-preset-step", left), pal, left).clicked() {
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
    let title_w = paint_title(ui.painter(), header, "Sprawl", "Stereo Imager", &PALETTE);
    let h = pill_height(PillStyle::Small);
    let mut right = header.right();
    let mut place = |w: f32, gap: f32| {
        let r = Rect::from_min_size(pos2(right - w, header.center().y - h * 0.5), vec2(w, h));
        right -= w + gap;
        r
    };
    let safe = place(84.0, 10.0);
    let b = place(28.0, 3.0);
    let a = place(28.0, 8.0);
    let undo = place(48.0, 0.0);
    header_pills(st, app, ui, host, [undo, a, b, safe]);
    // The preset strip, centred in what the title and the pills leave.
    let free = Rect::from_min_max(pos2(header.left() + title_w + 16.0, header.top()), pos2(undo.left() - 16.0, header.bottom()));
    let name_w = (free.width() - 2.0 * 30.0).clamp(120.0, 240.0);
    let strip = Rect::from_center_size(free.center(), vec2(name_w + 60.0, h + 2.0));
    let prev = Rect::from_min_size(pos2(strip.left(), strip.center().y - h * 0.5), vec2(24.0, h));
    let next = Rect::from_min_size(pos2(strip.right() - 24.0, strip.center().y - h * 0.5), vec2(24.0, h));
    let name = Rect::from_min_max(pos2(prev.right() + 6.0, strip.top()), pos2(next.left() - 6.0, strip.bottom()));
    preset_strip_ui(st, app, ui, host, [prev, name, next]);
}

// ---------------------------------------------------------------------------- the scope

/// A reading eased towards `fresh` (a NaN, or nothing playing, eases to 0).
fn ease(shown: &mut f32, fresh: Option<f32>) {
    let v = fresh.filter(|v| v.is_finite()).unwrap_or(0.0);
    *shown += (v - *shown) * 0.3;
    if !shown.is_finite() {
        *shown = 0.0;
    }
}

/// The vectorscope in `panel` over the correlation meter.
fn scope_ui(st: &mut Editor, ui: &mut Ui, host: &Host, panel: Rect, compact: bool) {
    let pal = &PALETTE;
    paint_panel(ui.painter(), panel, 10.0);
    let p = ui.painter().with_clip_rect(panel.intersect(ui.clip_rect()));
    let meter_h = 34.0;
    let field = Rect::from_min_max(panel.min + vec2(8.0, 8.0), pos2(panel.right() - 8.0, panel.bottom() - meter_h));
    let radius = (field.width().min(field.height()) * 0.5 - 2.0).max(4.0);
    let centre = field.center();

    // ---- the guides: the circle, mid (up and down), side (across), and the two channels
    let faint = Stroke::new(1.0, al(Color32::WHITE, 0.07));
    p.circle_stroke(centre, radius, Stroke::new(1.0, al(Color32::WHITE, 0.10)));
    p.circle_stroke(centre, radius * 0.5, faint);
    p.vline(centre.x, centre.y - radius..=centre.y + radius, faint);
    p.hline(centre.x - radius..=centre.x + radius, centre.y, faint);
    let d = radius * std::f32::consts::FRAC_1_SQRT_2;
    for sx in [-1.0f32, 1.0] {
        p.line_segment([centre + vec2(sx * d, -d), centre - vec2(sx * d, -d)], faint);
    }
    let label = |at: Pos2, anchor: Align2, text: &str| {
        p.text(at, anchor, text, regular(TINY_SIZE), pal.text_faint);
    };
    label(centre + vec2(-d - 3.0, -d - 3.0), Align2::RIGHT_BOTTOM, "L");
    label(centre + vec2(d + 3.0, -d - 3.0), Align2::LEFT_BOTTOM, "R");
    if !compact {
        label(pos2(field.left() + 2.0, field.top() + 6.0), Align2::LEFT_CENTER, "MID \u{2195}");
        label(pos2(field.left() + 2.0, field.bottom() - 6.0), Align2::LEFT_CENTER, "SIDE \u{2194}");
    }

    // ---- the sound: each recent sample a dot, mid upwards and left to the left
    let live = crate::editor_kit::live(host);
    if let Some(l) = live {
        let points: Vec<(f32, f32)> = (0..SCOPE_POINTS)
            .map(|i| (l.get(LIVE_SCOPE_L + i), l.get(LIVE_SCOPE_R + i)))
            .filter(|(a, b)| a.is_finite() && b.is_finite())
            .map(|(a, b)| (0.5 * (a + b), 0.5 * (a - b)))
            .collect();
        let peak = points.iter().fold(0.0f32, |m, (mid, side)| m.max(mid.abs()).max(side.abs()));
        let want = 0.9 / peak.max(0.02);
        // Zoom out at once (nothing leaves the circle), back in slowly.
        st.scope_gain = if st.scope_gain <= 0.0 || want < st.scope_gain { want } else { st.scope_gain + (want - st.scope_gain) * 0.05 };
        let dot = al(pal.accent, 0.6);
        for (mid, side) in points.iter().filter(|(m, s)| *m != 0.0 || *s != 0.0) {
            let v = vec2(-side * st.scope_gain, -mid * st.scope_gain);
            let v = if v.length() > 1.0 { v.normalized() } else { v };
            p.circle_filled(centre + v * radius, 1.2, dot);
        }
    } else {
        st.scope_gain = 0.0;
    }

    // ---- the correlation meter: -1 on the left, +1 on the right
    ease(&mut st.correlation, live.map(|l| l.get(LIVE_CORRELATION)));
    let bar = Rect::from_min_size(pos2(panel.left() + 28.0, panel.bottom() - meter_h + 8.0), vec2(panel.width() - 56.0, 6.0));
    p.rect_filled(bar, 3.0, pal.well);
    p.rect_filled(Rect::from_min_max(bar.min, pos2(bar.center().x, bar.bottom())), 3.0, al(pal.warn, 0.14));
    let x = bar.center().x + st.correlation.clamp(-1.0, 1.0) * bar.width() * 0.5;
    let colour = if st.correlation < -0.02 { pal.warn } else { pal.accent };
    if live.is_some() {
        p.rect_filled(Rect::from_min_max(pos2(x.min(bar.center().x), bar.top()), pos2(x.max(bar.center().x), bar.bottom())), 0.0, al(colour, 0.85));
        p.rect_filled(Rect::from_center_size(pos2(x, bar.center().y), vec2(3.0, 10.0)), 1.0, colour);
    }
    p.vline(bar.center().x, bar.top() - 2.0..=bar.bottom() + 2.0, Stroke::new(1.0, al(Color32::WHITE, 0.25)));
    label(pos2(bar.left() - 4.0, bar.center().y), Align2::RIGHT_CENTER, "-1");
    label(pos2(bar.right() + 4.0, bar.center().y), Align2::LEFT_CENTER, "+1");
    let text = if live.is_some() { format!("Correlation {}", correlation_text(st.correlation)) } else { "Correlation".to_string() };
    p.text(
        pos2(bar.center().x, bar.bottom() + 9.0),
        Align2::CENTER_CENTER,
        text,
        regular(TINY_SIZE),
        if live.is_some() { pal.text_label } else { pal.text_faint },
    );
}

// ---------------------------------------------------------------------------- the bands

/// A split's line at `x`: a drag moves it between its neighbours. Returns whether it is hot.
fn split_handle(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, area: Rect, at: &[f32; SPLITS], i: usize) -> bool {
    let pal = &PALETTE;
    let (Some(id), Some(hz)) = (SPLIT_IDS.get(i), at.get(i)) else { return false };
    let x = freq_to_x(area, *hz);
    let hit = Rect::from_min_max(pos2(x - SPLIT_GRAB, area.top()), pos2(x + SPLIT_GRAB, area.bottom()));
    let resp = ui.interact(hit, ui.id().with(("sprawl-split", i)), Sense::click_and_drag());
    if resp.drag_started() {
        end_gesture(app);
        family::push_undo(&mut st.fam, host, &FAMILY);
    }
    if resp.dragged()
        && let (Some(p), Some(info)) = (resp.interact_pointer_pos(), host.info.param(id))
    {
        // Between its neighbours, so a drag never pushes another split along.
        let low = i.checked_sub(1).and_then(|k| at.get(k)).map_or(SPLIT_HZ.0, |n| n * MIN_SPLIT_RATIO);
        let high = at.get(i + 1).map_or(SPLIT_HZ.1.min(host.sr as f32 * 0.45), |n| n / MIN_SPLIT_RATIO);
        let hz = x_to_freq(area, p.x).min(high).max(low);
        set_params_merged(app, host, &[(id, info.clamp(hz))], &format!("sprawl-{id}"));
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
    p.vline(x, area.y_range(), Stroke::new(if hot { 1.5 } else { 1.0 }, al(c, if hot { 0.9 } else { 0.45 })));
    p.rect_filled(Rect::from_center_size(pos2(x, area.top() + 9.0), vec2(8.0, 12.0)), 2.0, al(c, if hot { 1.0 } else { 0.7 }));
    resp.on_hover_text("A split between two bands. Drag to move, double-click to reset.");
    hot
}

/// Band `i` between `x0` and `x1`: its gestures, then its bar.
#[allow(clippy::too_many_arguments)]
fn band_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, plot: Rect, i: usize, (x0, x1): (f32, f32), compact: bool) {
    let pal = &PALETTE;
    let Some(id) = BAND_IDS.get(i).copied() else { return };
    let width = param(host, id);
    let cell = Rect::from_min_max(pos2(x0, plot.top()), pos2(x1.max(x0), plot.bottom()));
    let hit = cell.shrink2(vec2(SPLIT_GRAB.min(cell.width() * 0.25), 0.0));
    let resp = ui.interact(hit, ui.id().with(("sprawl-band", i)), Sense::click_and_drag());
    let gesture = format!("sprawl-band-{i}");
    if resp.drag_started() {
        end_gesture(app);
        family::push_undo(&mut st.fam, host, &FAMILY);
    }
    if resp.dragged() {
        let w = (width - resp.drag_delta().y / plot.height().max(1.0) * MAX_WIDTH).clamp(0.0, MAX_WIDTH);
        set_params_merged(app, host, &[(id, w)], &gesture);
    }
    if resp.drag_stopped() {
        end_gesture(app);
    }
    if resp.double_clicked() {
        family::set(app, host, &mut st.fam, &FAMILY, id, 100.0);
    }
    let hot = resp.hovered() || resp.dragged();
    if hot {
        st.shown_band = Some(i);
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }

    // ---- the bar: from "as it came" to the band's Width
    let p = ui.painter().with_clip_rect(cell.expand2(vec2(0.0, 1.0)).intersect(ui.clip_rect()));
    let (y, y100) = (width_y(plot, width), width_y(plot, 100.0));
    let inner = Rect::from_min_max(pos2(cell.left() + 1.0, y.min(y100)), pos2(cell.right() - 1.0, y.max(y100)));
    if hot {
        p.rect_filled(cell, 0.0, al(Color32::WHITE, 0.025));
    }
    p.rect_filled(inner, 0.0, al(pal.accent, if width >= 100.0 { 0.22 } else { 0.10 }));
    // The overall Width on top of it, where the two differ.
    let total = dsp::side_gain(param(host, "width"), width) * 100.0;
    if (total - width).abs() > 0.5 {
        p.hline(cell.left() + 3.0..=cell.right() - 3.0, width_y(plot, total), Stroke::new(1.0, al(pal.text_dim, 0.45)));
    }
    // Held by Mono Safe: where the band really is.
    let held = crate::editor_kit::live(host).map(|l| l.get(LIVE_BAND_GAIN + i) * 100.0).filter(|g| g.is_finite() && *g < total - 2.0);
    if let Some(g) = held {
        p.hline(cell.left() + 3.0..=cell.right() - 3.0, width_y(plot, g), Stroke::new(1.5, pal.warn));
    }
    p.hline(cell.left() + 1.0..=cell.right() - 1.0, y, Stroke::new(if hot { 2.5 } else { 2.0 }, pal.accent));
    let grip = Rect::from_center_size(pos2(cell.center().x, y), vec2(22.0f32.min(cell.width() - 6.0).max(4.0), 8.0));
    p.rect_filled(grip, 3.0, if hot { pal.text_bright } else { pal.accent });
    let name = BAND_NAMES.get(i).copied().unwrap_or_default().to_uppercase();
    if cell.width() >= 12.0 + 5.5 * name.len() as f32 {
        p.text(pos2(cell.center().x, plot.top() + 11.0), Align2::CENTER_CENTER, name, regular(TINY_SIZE), pal.text_faint);
    }
    if cell.width() >= 34.0 {
        // The value, clear of the bar: above it, or below when the bar is near the top.
        let text_y = if y - plot.top() > 40.0 { y - 11.0 } else { y + 12.0 };
        let size = if compact { CAPTION_SIZE } else { LABEL_SIZE };
        p.text(pos2(cell.center().x, text_y), Align2::CENTER_CENTER, width_text(width), bold(size), if hot { pal.text_bright } else { pal.text_dim });
    }
    resp.on_hover_text("Drag up and down: this band's width. Double-click: as it came.");
}

/// What band `i` holds right now, in `lane`: a bar as wide as its side share.
fn band_lane(st: &mut Editor, ui: &Ui, host: &Host, lane: Rect, i: usize) {
    let pal = &PALETTE;
    let live = crate::editor_kit::live(host);
    let (Some(corr), Some(side)) = (st.band_correlation.get_mut(i), st.band_side.get_mut(i)) else { return };
    ease(corr, live.map(|l| l.get(LIVE_BAND_CORRELATION + i)));
    ease(side, live.map(|l| l.get(LIVE_BAND_SIDE + i)));
    let p = ui.painter();
    let track = lane.shrink2(vec2(3.0, 3.0));
    if track.width() < 4.0 {
        return;
    }
    p.rect_filled(track, 2.0, pal.well);
    if live.is_some() {
        // The square root: a side 20 dB under the mid is still a visible sliver.
        let w = (side.clamp(0.0, 1.0).sqrt() * track.width()).max(2.0);
        let colour = if *corr < -0.02 { pal.warn } else { pal.accent };
        p.rect_filled(Rect::from_center_size(track.center(), vec2(w, track.height())), 2.0, al(colour, 0.8));
    }
}

/// The bands' panel in an editor drawn at `frame`.
fn bands_panel(frame: Rect) -> Rect {
    let left = frame.left() + PAD_X + GRAPH_H + GAP;
    Rect::from_min_max(pos2(left, frame.top() + PAD_Y + HEADER_H + GAP), pos2(frame.right() - PAD_X, frame.top() + PAD_Y + HEADER_H + GAP + GRAPH_H))
}

/// The scope's panel in an editor drawn at `frame`: a square on the left.
fn scope_panel(frame: Rect) -> Rect {
    Rect::from_min_size(pos2(frame.left() + PAD_X, frame.top() + PAD_Y + HEADER_H + GAP), Vec2::splat(GRAPH_H))
}

/// The bands' plot area (where the widths are drawn and dragged) in an editor drawn at `frame`
/// (for tests and scripts).
pub fn bands_rect(frame: Rect) -> Rect {
    let panel = bands_panel(frame);
    Rect::from_min_max(pos2(panel.left() + 10.0, panel.top() + 10.0), pos2(panel.right() - 6.0 - SCALE_W, panel.bottom() - LANE_H - AXIS_H - 6.0))
}

/// Where band `i`'s bar is grabbed when its Width is `width` percent and the splits are `at` Hz,
/// in a plot at `plot` (for tests and scripts).
pub fn band_pos_in(plot: Rect, at: &[f32; SPLITS], i: usize, width: f32) -> Pos2 {
    let (x0, x1) = band_x(plot, at, i);
    pos2((x0 + x1) * 0.5, width_y(plot, width))
}

/// [`band_pos_in`] in an editor drawn at `frame`.
pub fn band_pos(frame: Rect, at: &[f32; SPLITS], i: usize, width: f32) -> Pos2 {
    band_pos_in(bands_rect(frame), at, i, width)
}

/// Where a split at `hz` is grabbed in an editor drawn at `frame` (for tests and scripts).
pub fn split_pos(frame: Rect, hz: f32) -> Pos2 {
    let plot = bands_rect(frame);
    pos2(freq_to_x(plot, hz), plot.center().y)
}

/// The bands in `panel`, plotting in `plot`, with the line about the band in hand in `readout`.
/// `compact` (the slim layout): fewer labels, the line wrapped onto two rows.
fn bands_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, (panel, plot, readout): (Rect, Rect, Rect), compact: bool) {
    let pal = &PALETTE;
    paint_panel(ui.painter(), panel, 10.0);
    let p = ui.painter().with_clip_rect(panel.intersect(ui.clip_rect()));
    let lane = Rect::from_min_size(pos2(plot.left(), plot.bottom() + 3.0), vec2(plot.width(), LANE_H));

    // ---- the frequency grid and its labels, and the width scale
    let grid = [(50.0, "50"), (100.0, "100"), (200.0, "200"), (500.0, "500"), (1000.0, "1k"), (2000.0, "2k"), (5000.0, "5k"), (10_000.0, "10k")];
    for (hz, label) in grid {
        let x = freq_to_x(plot, hz);
        p.vline(x, plot.y_range(), Stroke::new(1.0, al(Color32::WHITE, 0.04)));
        if !compact || matches!(label, "100" | "1k" | "10k") {
            p.text(pos2(x, lane.bottom() + 9.0), Align2::CENTER_CENTER, label, regular(TINY_SIZE), pal.text_faint);
        }
    }
    for (w, label) in [(0.0, "Mono"), (100.0, "100"), (200.0, "200")] {
        let y = width_y(plot, w);
        let strong = w == 100.0;
        p.hline(plot.x_range(), y, Stroke::new(1.0, al(Color32::WHITE, if strong { 0.16 } else { 0.06 })));
        p.text(pos2(plot.right() + 5.0, y), Align2::LEFT_CENTER, label, regular(TINY_SIZE), pal.text_faint);
    }
    p.rect_stroke(plot, 0.0, Stroke::new(1.0, al(Color32::WHITE, 0.06)), StrokeKind::Inside);

    // ---- the bands, then the splits over them
    let at = splits_of(host);
    for i in 0..BANDS {
        let (x0, x1) = band_x(plot, &at, i);
        band_ui(st, app, ui, host, plot, i, (x0, x1), compact);
        band_lane(st, ui, host, Rect::from_min_max(pos2(x0, lane.top()), pos2(x1.max(x0), lane.bottom())), i);
    }
    let grab = Rect::from_min_max(plot.min, pos2(plot.right(), lane.bottom()));
    let hot_split = (0..SPLITS).fold(None, |hot, i| if split_handle(st, app, ui, host, grab, &at, i) { Some(i) } else { hot });

    // ---- the line under the panel: the split or band in hand
    let text = if let Some((i, hz)) = hot_split.and_then(|i| at.get(i).map(|hz| (i, *hz))) {
        format!("Split {}  \u{00B7}  {}", i + 1, hz_text(hz))
    } else if let Some(i) = st.shown_band.filter(|i| *i < BANDS) {
        let width = BAND_IDS.get(i).map_or(100.0, |id| param(host, id));
        let name = BAND_NAMES.get(i).copied().unwrap_or_default();
        let live = if crate::editor_kit::live(host).is_some() {
            format!("  \u{00B7}  correlation {}", correlation_text(st.band_correlation.get(i).copied().unwrap_or(0.0)))
        } else {
            String::new()
        };
        // With the overall Width on top, where that changes it.
        let total = dsp::side_gain(param(host, "width"), width) * 100.0;
        let with = if (total - width).abs() > 0.5 { format!(" ({total:.0} % with Width)") } else { String::new() };
        format!("{name}  \u{00B7}  {}  \u{00B7}  {}{with}{live}", band_range(&at, i), width_text(width).to_lowercase())
    } else {
        "Drag a band up to widen it, down to narrow it. Drag a line to move a split.".to_string()
    };
    if compact {
        crate::editor_kit::slim::wrapped(ui, readout, &text, CAPTION_SIZE, pal.text_faint, 2);
    } else {
        ui.painter().text(readout.left_center(), Align2::LEFT_CENTER, text, regular(CAPTION_SIZE), pal.text_faint);
    }
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
    let [width, mono, bands, output] = panels.as_slice() else { return };
    for (r, name) in [(width, "Width"), (mono, "Mono Sources"), (bands, "Bands"), (output, "Output")] {
        paint_panel(ui.painter(), *r, 10.0);
        group_label(ui.painter(), *r, name);
    }
    let body = |r: &Rect| Rect::from_min_max(pos2(r.left() + 8.0, r.top() + GROUP_LABEL_H), pos2(r.right() - 8.0, r.bottom() - 12.0));
    let note_of = |b: Rect| Rect::from_min_size(pos2(b.left(), b.bottom() - NOTE_H), vec2(b.width(), NOTE_H));
    let small_row = |b: Rect| keep_centre(Rect::from_min_max(b.min, pos2(b.right(), b.bottom() - NOTE_H - 8.0)), b.width(), SMALL_KNOB + 28.0);
    let note = |ui: &Ui, b: Rect, text: &str| {
        crate::editor_kit::slim::wrapped(ui, note_of(b), text, TINY_SIZE, pal.text_faint, 2);
    };

    // Width: every band at once.
    let b = body(width);
    let big = Rect::from_min_size(pos2(b.center().x - BIG_KNOB * 0.5, b.top()), vec2(BIG_KNOB, BIG_KNOB + 30.0));
    dial_ui(app, ui, host, &mut st.fam, big, "width", "Width", true);
    note(ui, b, "Every band at once. The mono sum never changes.");

    // Mono sources: Stereoize.
    let b = body(mono);
    dial_row(app, ui, host, &mut st.fam, small_row(b), &[("stereoize", "Stereoize")]);
    note(ui, b, "Sides for a mono source. They cancel again in mono.");

    // The bands' own widths.
    let b = body(bands);
    dial_row(app, ui, host, &mut st.fam, small_row(b), &band_dials());
    note(ui, b, "Each band's own width: mono at the bottom, as it came at 100 %, double at the top.");

    // Output.
    let b = body(output);
    dial_row(app, ui, host, &mut st.fam, small_row(b), &[("trim", "Trim")]);
    let text = if param(host, "safe") > 0.5 { "Mono Safe is holding back what would cancel." } else { "Wider is louder: trim it back here." };
    note(ui, b, text);
    tip(ui, ui.id().with(("vst-dial", "stereoize")), "Makes a side signal from the middle, above 180 Hz");
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
    let (scope, bands) = (scope_panel(full), bands_panel(full));
    let readout = Rect::from_min_size(pos2(bands.left() + 12.0, bands.bottom() + 4.0), vec2(bands.width() - 24.0, READOUT_H));
    let controls = Rect::from_min_size(pos2(b.left(), readout.bottom() + GAP), vec2(b.width(), CONTROLS_H));
    header_ui(st, app, ui, host, header);
    scope_ui(st, ui, host, scope, false);
    bands_ui(st, app, ui, host, (bands, bands_rect(full), readout), false);
    controls_ui(st, app, ui, host, controls);
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_dsp::plugins::sprawl::SPLIT_DEFAULTS;

    #[test]
    fn the_groups_fill_the_width() {
        let used: f32 = GROUP_W.iter().sum::<f32>() + GAP * (GROUP_W.len() - 1) as f32;
        assert_eq!(used, SIZE.x - 2.0 * PAD_X);
    }

    #[test]
    fn the_bands_tile_the_plot_and_the_width_scale_has_mono_at_the_bottom() {
        let plot = Rect::from_min_size(pos2(10.0, 20.0), vec2(500.0, 200.0));
        let mut x = plot.left();
        for i in 0..BANDS {
            let (x0, x1) = band_x(plot, &SPLIT_DEFAULTS, i);
            assert!(x0 == x && x1 > x0, "band {i}: {x0}..{x1}");
            x = x1;
        }
        assert_eq!(x, plot.right());
        assert_eq!(width_y(plot, 0.0), plot.bottom());
        assert_eq!(width_y(plot, 100.0), plot.center().y);
        assert_eq!(width_y(plot, 900.0), plot.top());
        let at = band_pos_in(plot, &SPLIT_DEFAULTS, 0, 100.0);
        assert!(at.x > plot.left() && at.x < freq_to_x(plot, 120.0) && at.y == plot.center().y);
    }

    #[test]
    fn readouts_read_naturally() {
        assert_eq!(hz_text(120.0), "120 Hz");
        assert_eq!(hz_text(6000.0), "6.00 kHz");
        assert_eq!(width_text(0.0), "Mono");
        assert_eq!(width_text(125.0), "125 %");
        assert_eq!(correlation_text(0.0), "0.00");
        assert_eq!(correlation_text(0.824), "+0.82");
        assert_eq!(correlation_text(-0.3), "-0.30");
        assert_eq!(band_range(&SPLIT_DEFAULTS, 0), "below 120 Hz");
        assert_eq!(band_range(&SPLIT_DEFAULTS, 2), "1.00 kHz to 6.00 kHz");
        assert_eq!(band_range(&SPLIT_DEFAULTS, 3), "above 6.00 kHz");
    }
}
