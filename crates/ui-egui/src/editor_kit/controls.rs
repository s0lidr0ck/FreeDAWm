//! `Common/Controls.h` (`Dial`, `Segmented`, `Pill`, `Led`, `PresetNameButton`) and the CleanQ
//! pilot's generic controls (`CardButton`, `NumField`, the JUCE linear slider), translated: the
//! same geometry and paint, the same gestures.
//!
//! Every control here is immediate-mode: it paints, reads the pointer, and returns what changed;
//! the editor sends the change as a command.

use super::look::{font, glow_rect, tracked_text, tracked_width};
use super::palette::{VstPalette, al, rgb};
use super::{consume_wheel, juce_wheel_delta_y};
use crate::theme::{bold, regular};
use crate::widgets::{KnobStyle, paint_knob_face};
use egui::{Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Painter, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use std::hash::Hash;

// ============================================================================ Dial

/// `Dial::LabelLayout`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LabelLayout {
    /// Value then name stacked under the knob.
    #[default]
    Below,
    /// Knob on the left, value and name stacked to its right.
    Right,
}

/// A dial's settings (`setLabel`, `setDefault`, `setSmall`, `setLabelLayout`, `setLocked`,
/// `setInactive`, `setCalFocus`, `setCapColour`, `setTrackedLabels`), and the readout text.
#[derive(Clone, Copy, Debug)]
pub struct DialOpts<'a> {
    pub label: &'a str,
    pub value_text: &'a str,
    /// Normalised default (double-click).
    pub default: f32,
    pub layout: LabelLayout,
    pub small: bool,
    /// The editor's large main knob (the DAW's hero knob face).
    pub hero: bool,
    pub locked: bool,
    pub inactive: bool,
    pub cal_focus: bool,
    pub cap: Option<Color32>,
    /// Uppercase letter-spaced labels (DistEngine) rather than mixed case (the family look).
    pub tracked: bool,
}

impl Default for DialOpts<'_> {
    fn default() -> Self {
        DialOpts {
            label: "",
            value_text: "",
            default: 0.5,
            layout: LabelLayout::Below,
            small: false,
            hero: false,
            locked: false,
            inactive: false,
            cal_focus: false,
            cap: None,
            tracked: false,
        }
    }
}

/// What a dial did this frame.
#[derive(Debug)]
pub struct DialOut {
    /// The new normalised value, when it moved.
    pub value: Option<f32>,
    /// A gesture began (`onDragStart`): a press on the knob, a double-click or a wheel step.
    pub started: bool,
    /// A gesture ended (`onDragEnd`).
    pub ended: bool,
    pub response: Response,
}

/// `Dial::getKnobBounds`.
pub(crate) fn dial_knob_bounds(r: Rect, layout: LabelLayout) -> Rect {
    match layout {
        LabelLayout::Right => {
            let s = r.height().min(r.width() * 0.45).max(0.0);
            Rect::from_min_size(pos2(r.left(), r.center().y - s * 0.5), vec2(s, s))
        }
        LabelLayout::Below => {
            let s = r.width().min(r.height() - 30.0).max(0.0);
            Rect::from_min_size(pos2(r.center().x - s * 0.5, r.top()), vec2(s, s))
        }
    }
}

#[derive(Clone, Copy, Default)]
struct DialDrag {
    start_y: f32,
    start_value: f32,
}

/// `fs::Dial`: vertical drag from the knob (`startValue + (startY - y) * 0.008`, Shift 0.002),
/// double-click back to the default, the wheel (`value + deltaY * 0.25`); locked or inactive take
/// no input. The face is the DAW's knob (the owner's rule: the new knob style where the VST has
/// knobs); the readout and name follow `Dial::paint`.
pub(crate) fn dial(ui: &mut Ui, rect: Rect, salt: impl Hash + std::fmt::Debug, pal: &VstPalette, v01: f32, o: &DialOpts) -> DialOut {
    let v01 = if v01.is_finite() { v01.clamp(0.0, 1.0) } else { 0.0 };
    let knob = dial_knob_bounds(rect, o.layout);
    let id = ui.id().with(("vst-dial", salt));
    let frozen = o.locked || o.inactive;
    let resp = ui.interact(knob, id, if frozen { Sense::hover() } else { Sense::click_and_drag() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, !frozen, o.label));
    let mut out = DialOut { value: None, started: false, ended: false, response: resp.clone() };
    if !frozen {
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        }
        let key = id.with("drag");
        if resp.drag_started()
            && let Some(p) = resp.interact_pointer_pos()
        {
            // mouseDown records the press point; egui reports the drag a few pixels in.
            let press = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
            ui.data_mut(|d| d.insert_temp(key, DialDrag { start_y: press.y, start_value: v01 }));
            out.started = true;
        }
        if resp.dragged()
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(d) = ui.data(|d| d.get_temp::<DialDrag>(key))
        {
            let sens = if ui.input(|i| i.modifiers.shift) { 0.002 } else { 0.008 };
            let v = (d.start_value + (d.start_y - p.y) * sens).clamp(0.0, 1.0);
            if (v - v01).abs() > f32::EPSILON {
                out.value = Some(v);
            }
        }
        if resp.drag_stopped() {
            ui.data_mut(|d| d.remove::<DialDrag>(key));
            out.ended = true;
        }
        if resp.double_clicked() {
            out.value = Some(o.default.clamp(0.0, 1.0));
            out.started = true;
            out.ended = true;
        }
        if resp.hovered() {
            let dy = juce_wheel_delta_y(ui);
            if dy != 0.0 {
                consume_wheel(ui);
                let v = (v01 + dy * 0.25).clamp(0.0, 1.0);
                if (v - v01).abs() > f32::EPSILON {
                    out.value = Some(v);
                    out.started = true;
                    out.ended = true;
                }
            }
        }
    }
    let shown = out.value.unwrap_or(v01);
    paint_dial(ui.painter(), rect, knob, pal, shown, o, resp.hovered(), resp.dragged());
    out
}

#[allow(clippy::too_many_arguments)]
fn paint_dial(p: &Painter, rect: Rect, knob: Rect, pal: &VstPalette, v: f32, o: &DialOpts, hovered: bool, active: bool) {
    let alpha = if o.locked || o.inactive { 0.45 } else { 1.0 };
    let accent = if o.small { pal.text_bright } else { pal.accent };
    let style = if o.hero { KnobStyle::Hero } else { KnobStyle::Dark };
    // The DAW knob face, dimmed as the C++ multiplies every colour by `alpha`.
    if alpha < 1.0 {
        let mut layer = p.clone();
        layer.multiply_opacity(alpha);
        paint_knob_face(&layer, knob, v, accent, style, false, false);
    } else {
        paint_knob_face(p, knob, v, accent, style, hovered, active);
    }
    if o.cal_focus {
        p.circle_stroke(knob.center(), knob.width() * 0.459, Stroke::new((knob.width() * 0.0375).max(2.0), al(pal.accent, 0.40)));
    }
    if let Some(cap) = o.cap {
        p.circle_filled(knob.center(), knob.width() * 0.12, al(cap, alpha));
    }

    // ---- readout + name
    let (value_row, label_row, align) = match o.layout {
        LabelLayout::Right => {
            let text = Rect::from_min_max(pos2(knob.right() + 6.0, rect.top()), rect.max);
            let h = text.height().min(36.0);
            let text = Rect::from_center_size(text.center(), vec2(text.width(), h));
            let v = Rect::from_min_size(text.min, vec2(text.width(), h * 0.55));
            (v, Rect::from_min_max(pos2(text.left(), v.bottom()), text.max), Align2::LEFT_CENTER)
        }
        LabelLayout::Below => {
            let v = Rect::from_min_size(pos2(rect.left(), knob.bottom()), vec2(rect.width(), 17.0));
            (v, Rect::from_min_size(pos2(rect.left(), v.bottom()), vec2(rect.width(), 13.0)), Align2::CENTER_CENTER)
        }
    };
    let at = |r: Rect| if align == Align2::LEFT_CENTER { pos2(r.left(), r.center().y) } else { r.center() };
    p.text(at(value_row), align, o.value_text, regular(13.0), pal.text_bright);
    let label_colour = if o.cal_focus { pal.accent } else { pal.text_label };
    if o.locked {
        // ".label::after { content: '  ·  LINKED' }"
        let suffix = if o.tracked { "  \u{00B7}  LINKED" } else { "  \u{00B7}  Linked" };
        if o.tracked {
            let w1 = tracked_width(p, o.label, 9.0, 0.18, true);
            let w2 = tracked_width(p, suffix, 9.0, 0.18, true);
            let row = Rect::from_center_size(label_row.center(), vec2(w1 + w2, label_row.height()));
            tracked_text(p, Rect::from_min_size(row.min, vec2(w1, row.height())), Align2::LEFT_CENTER, o.label, 9.0, 0.18, true, false, label_colour);
            tracked_text(
                p,
                Rect::from_min_max(pos2(row.left() + w1, row.top()), row.max),
                Align2::LEFT_CENTER,
                suffix,
                9.0,
                0.18,
                true,
                false,
                pal.accent,
            );
        } else {
            let w1 = p.layout_no_wrap(o.label.to_string(), regular(11.0), label_colour).size().x;
            let w2 = p.layout_no_wrap(suffix.to_string(), regular(11.0), pal.accent).size().x;
            let x = label_row.center().x - (w1 + w2) * 0.5;
            p.text(pos2(x, label_row.center().y), Align2::LEFT_CENTER, o.label, regular(11.0), label_colour);
            p.text(pos2(x + w1, label_row.center().y), Align2::LEFT_CENTER, suffix, regular(11.0), pal.accent);
        }
    } else if o.tracked {
        tracked_text(p, label_row, align, o.label, 9.0, 0.18, true, false, label_colour);
    } else {
        p.text(at(label_row), align, o.label, regular(11.0), label_colour);
    }
}

// ============================================================================ tooltips

/// `SettableTooltipClient::setTooltip`: the hover help on the kit widget drawn last frame under
/// `id` (a control that hands back no response: [`segmented_id`], [`crossover_id`]).
pub(crate) fn tip(ui: &Ui, id: Id, text: &str) {
    if let Some(r) = ui.ctx().read_response(id) {
        let _ = r.on_hover_text(text);
    }
}

/// The id [`segmented_ex`] draws under `salt` in `ui`.
pub(crate) fn segmented_id(ui: &Ui, salt: impl Hash + std::fmt::Debug) -> Id {
    ui.id().with(("vst-seg", salt))
}

/// The id `crossover::crossover` draws under in `ui`.
pub(crate) fn crossover_id(ui: &Ui) -> Id {
    ui.id().with("vst-crossover")
}

// ============================================================================ Segmented

/// `Segmented::SelectionStyle`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SelectionStyle {
    /// A solid accent block with dark text (the Engine family).
    #[default]
    Filled,
    /// Accent text inside an accent border with an inset glow (Swale's own build).
    Outlined,
}

/// `fs::Segmented`: `options` over `rows` rows; a click (mouse up inside) picks the segment under
/// it, from the grid so a click on a 1 px border still lands. `tracked`: uppercase letter-spaced
/// (DistEngine) or mixed case. Returns the clicked index.
#[allow(clippy::too_many_arguments)]
pub(crate) fn segmented(
    ui: &mut Ui,
    rect: Rect,
    salt: impl Hash + std::fmt::Debug,
    pal: &VstPalette,
    options: &[&str],
    rows: usize,
    selected: Option<usize>,
    style: SelectionStyle,
) -> Option<usize> {
    segmented_ex(ui, rect, salt, pal, options, rows, selected, style, false)
}

/// [`segmented`] with the label treatment chosen (`setTracked`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn segmented_ex(
    ui: &mut Ui,
    rect: Rect,
    salt: impl Hash + std::fmt::Debug,
    pal: &VstPalette,
    options: &[&str],
    rows: usize,
    selected: Option<usize>,
    style: SelectionStyle,
    tracked: bool,
) -> Option<usize> {
    if options.is_empty() {
        return None;
    }
    let rows = rows.max(1);
    let n = options.len();
    let per_row = n.div_ceil(rows).max(1);
    let resp = ui.interact(rect, ui.id().with(("vst-seg", salt)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::RadioGroup, true, options.join("|")));
    let mut picked = None;
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let col = (((p.x - rect.left()) / (rect.width() / per_row as f32)).floor().max(0.0) as usize).min(per_row - 1);
        let row = (((p.y - rect.top()) / (rect.height() / rows as f32)).floor().max(0.0) as usize).min(rows - 1);
        let i = row * per_row + col;
        if i < n {
            picked = Some(i);
        }
    }

    let painter = ui.painter();
    let b = rect.shrink(0.5);
    let seg_w = b.width() / per_row as f32;
    let seg_h = b.height() / rows as f32;
    painter.rect_filled(b, CornerRadius::same(3), pal.panel);
    // Shrink the type until the longest option fits its segment, never below 8.
    let mut font_h = 10.5f32;
    while font_h > 8.0 {
        let widest = options.iter().map(|o| tracked_width(painter, o, font_h, 0.12, true)).fold(0.0f32, f32::max);
        if widest <= seg_w - 10.0 {
            break;
        }
        font_h -= 0.25;
    }
    let shown = picked.or(selected);
    for (i, text) in options.iter().enumerate() {
        let seg = Rect::from_min_size(pos2(b.left() + seg_w * (i % per_row) as f32, b.top() + seg_h * (i / per_row) as f32), vec2(seg_w, seg_h));
        let active = shown == Some(i);
        let filled = active && style == SelectionStyle::Filled;
        if active {
            let fill = seg.shrink(1.0);
            if filled {
                painter.rect_filled(fill, CornerRadius::same(3), pal.accent);
                glow_rect(painter, fill, 2.5, pal.accent, 1.4);
            } else {
                painter.rect_filled(fill, CornerRadius::same(3), al(pal.accent, 0.13));
                painter.rect_stroke(fill, CornerRadius::same(3), Stroke::new(1.0, pal.accent), StrokeKind::Middle);
            }
        }
        let ink = if filled {
            pal.on_accent
        } else if active {
            pal.accent
        } else {
            pal.text_dim
        };
        if tracked {
            tracked_text(painter, seg, Align2::CENTER_CENTER, text, font_h, 0.12, true, active, ink);
        } else {
            painter.with_clip_rect(seg).text(seg.center(), Align2::CENTER_CENTER, *text, font(font_h + 1.5, active), ink);
        }
    }
    let grid = Stroke::new(1.0, pal.border);
    for c in 1..per_row {
        let x = b.left() + seg_w * c as f32;
        painter.line_segment([pos2(x, b.top()), pos2(x, b.bottom())], grid);
    }
    for r in 1..rows {
        let y = b.top() + seg_h * r as f32;
        painter.line_segment([pos2(b.left(), y), pos2(b.right(), y)], grid);
    }
    painter.rect_stroke(b, CornerRadius::same(3), grid, StrokeKind::Middle);
    picked
}

// ============================================================================ Pill

/// `Pill::Style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PillStyle {
    #[default]
    Normal,
    Small,
}

/// A pill's optional looks (`setActiveColour`, `setTracked`, `setDimmed`).
#[derive(Clone, Copy, Debug)]
pub struct PillLook {
    pub active_colour: Option<Color32>,
    pub tracked: bool,
    pub dimmed: bool,
}

impl Default for PillLook {
    fn default() -> Self {
        PillLook { active_colour: None, tracked: true, dimmed: false }
    }
}

/// `Pill::preferredWidth` (tracked, as the default).
pub(crate) fn pill_width(ui: &Ui, text: &str, style: PillStyle) -> f32 {
    pill_width_ex(ui, text, style, true)
}

/// `Pill::preferredWidth` with the label treatment chosen.
pub(crate) fn pill_width_ex(ui: &Ui, text: &str, style: PillStyle, tracked: bool) -> f32 {
    let (f, pad) = if style == PillStyle::Small { (9.0, 28.0) } else { (10.0, 32.0) };
    let p = ui.painter();
    if tracked {
        tracked_width(p, text, f, 0.16, true) + pad
    } else {
        p.layout_no_wrap(text.to_string(), regular(f + 2.0), Color32::WHITE).size().x + pad
    }
}

/// `Pill::preferredHeight`.
pub(crate) fn pill_height(style: PillStyle) -> f32 {
    if style == PillStyle::Small { 21.0 } else { 25.0 }
}

/// `fs::Pill`: a toggle-looking button; clicks on mouse up inside.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pill(ui: &mut Ui, rect: Rect, salt: impl Hash + std::fmt::Debug, pal: &VstPalette, text: &str, on: bool, style: PillStyle) -> Response {
    pill_ex(ui, rect, salt, pal, text, on, style, PillLook::default())
}

/// [`pill`] with its optional looks.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pill_ex(
    ui: &mut Ui,
    rect: Rect,
    salt: impl Hash + std::fmt::Debug,
    pal: &VstPalette,
    text: &str,
    on: bool,
    style: PillStyle,
    look: PillLook,
) -> Response {
    let resp = ui.interact(rect, ui.id().with(("vst-pill", salt)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, text));
    let p = ui.painter();
    let b = rect.shrink(0.5);
    let f = if style == PillStyle::Small { 9.0 } else { 10.0 };
    let lit = look.active_colour.unwrap_or(pal.accent);
    let alpha = if look.dimmed { 0.35 } else { 1.0 };
    let hovered = resp.hovered();
    p.rect_filled(b, CornerRadius::same(4), al(if on { lit } else { pal.panel }, alpha));
    if on && !look.dimmed {
        glow_rect(p, b, 4.0, lit, 1.6);
    }
    let edge = if on {
        lit
    } else if hovered {
        pal.border_hover
    } else {
        pal.border
    };
    p.rect_stroke(b, CornerRadius::same(4), Stroke::new(1.0, al(edge, alpha)), StrokeKind::Middle);
    let ink = al(
        if on {
            pal.on_accent
        } else if hovered {
            pal.text_bright
        } else {
            pal.text_dim
        },
        alpha,
    );
    if look.tracked {
        tracked_text(p, b, Align2::CENTER_CENTER, text, f, 0.16, true, false, ink);
    } else {
        p.with_clip_rect(b).text(b.center(), Align2::CENTER_CENTER, text, font(f + 2.0, on), ink);
    }
    resp
}

// ============================================================================ Led

/// `fs::Led`: a 10 px lamp centred in `rect`, lit in `colour` with a glow, or dark.
pub(crate) fn led(p: &Painter, rect: Rect, pal: &VstPalette, on: bool, colour: Color32) {
    let c = rect.center();
    if on {
        for i in (1..=4).rev() {
            p.circle_filled(c, 5.0 + i as f32 * 1.5, al(colour, 0.16 / i as f32));
        }
        p.circle_filled(c, 5.0, colour);
    } else {
        p.circle_filled(c, 5.0, pal.led_off);
        p.circle_stroke(c, 4.5, Stroke::new(1.0, pal.led_off_ring));
    }
}

// ============================================================================ PresetNameButton

/// `fs::PresetNameButton`: the preset's name (letter-spaced, mixed case) with
/// `setPresetName (name, isDirty)`'s accent " *"; the accent border on hover; a click opens the
/// browser.
pub(crate) fn preset_name_button_ex(
    ui: &mut Ui,
    rect: Rect,
    salt: impl Hash + std::fmt::Debug,
    pal: &VstPalette,
    name: &str,
    dirty: bool,
) -> Response {
    let resp = ui.interact(rect, ui.id().with(("vst-preset-name", salt)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Preset {name}")));
    let p = ui.painter();
    let b = rect.shrink(0.5);
    p.rect_filled(b, CornerRadius::same(3), pal.panel);
    p.rect_stroke(b, CornerRadius::same(3), Stroke::new(1.0, if resp.hovered() { pal.accent } else { pal.border }), StrokeKind::Middle);
    let name_w = tracked_width(p, name, 11.0, 0.08, false);
    let star_w = if dirty { tracked_width(p, " *", 11.0, 0.08, false) } else { 0.0 };
    let w = (name_w + star_w).min(b.width() - 8.0).max(0.0);
    let row = Rect::from_center_size(b.center(), vec2(w, b.height()));
    let pc = p.with_clip_rect(b.shrink(2.0));
    let name_r = Rect::from_min_size(row.min, vec2((row.width() - star_w).max(0.0), row.height()));
    tracked_text(&pc, name_r, Align2::LEFT_CENTER, name, 11.0, 0.08, false, false, pal.text_bright);
    if dirty {
        let star_r = Rect::from_min_max(pos2(name_r.right(), row.top()), row.max);
        tracked_text(&pc, star_r, Align2::LEFT_CENTER, " *", 11.0, 0.08, false, false, pal.accent);
    }
    resp
}

// ============================================================================ CardButton

/// Which corners a button in a joined group rounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Alone,
    Left,
    Middle,
    Right,
}

/// A `CardButton`'s state.
#[derive(Clone, Copy, Debug)]
pub struct Btn<'a> {
    pub text: &'a str,
    pub active: bool,
    /// Lit in the caution colour rather than the accent.
    pub warn: bool,
    /// Red on hover (delete, remove).
    pub danger: bool,
    pub edge: Edge,
    pub bold: bool,
    pub enabled: bool,
}

impl<'a> Btn<'a> {
    pub fn new(text: &'a str) -> Self {
        Btn { text, active: false, warn: false, danger: false, edge: Edge::Alone, bold: false, enabled: true }
    }
    pub fn on(mut self, a: bool) -> Self {
        self.active = a;
        self
    }
    pub fn edge(mut self, e: Edge) -> Self {
        self.edge = e;
        self
    }
    pub fn warn(mut self) -> Self {
        self.warn = true;
        self
    }
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
}

/// `CardButton::preferredWidth (padding)`.
pub(crate) fn pref_w(ui: &Ui, text: &str, padding: f32) -> f32 {
    ui.painter().layout_no_wrap(text.to_string(), regular(11.0), Color32::WHITE).size().x.round() + padding
}

/// The corners an [`Edge`] rounds, by `r`.
pub(crate) fn rounded(edge: Edge, r: u8) -> CornerRadius {
    match edge {
        Edge::Alone => CornerRadius::same(r),
        Edge::Left => CornerRadius { nw: r, sw: r, ne: 0, se: 0 },
        Edge::Middle => CornerRadius::ZERO,
        Edge::Right => CornerRadius { nw: 0, sw: 0, ne: r, se: r },
    }
}

/// `CardButton`'s hover fill, danger fill, danger border, danger ink and warn ink (literals in
/// `CleanQBandCard.h`).
const CARD_HOVER: Color32 = rgb(0x222932);
const CARD_DANGER: Color32 = rgb(0x4A1D1D);
const CARD_DANGER_EDGE: Color32 = rgb(0x6B2A2A);
const CARD_DANGER_INK: Color32 = rgb(0xFFB4B4);
const CARD_WARN_INK: Color32 = rgb(0x201A05);

/// `CardButton::paint` and its click: the accent (or caution) fill when active, a hover fill,
/// joined-group corners. Returns the response (the caller draws any glyph with [`ink_of`]).
pub(crate) fn card_button(ui: &mut Ui, r: Rect, salt: impl Hash + std::fmt::Debug, pal: &VstPalette, b: Btn, sense: Sense) -> Response {
    let sense = if b.enabled { sense } else { Sense::hover() };
    let resp = ui.interact(r, ui.id().with(("vst-btn", salt)), sense);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, b.enabled, b.active, b.text));
    let hovering = b.enabled && resp.hovered();
    let on_colour = if b.warn { pal.warn } else { pal.accent };
    let fill = if b.active {
        Some(on_colour)
    } else if hovering {
        Some(if b.danger { CARD_DANGER } else { CARD_HOVER })
    } else {
        None
    };
    let p = ui.painter();
    let round = rounded(b.edge, 5);
    if let Some(f) = fill {
        p.rect_filled(r, round, f);
    }
    let outline = if b.active {
        on_colour
    } else if b.danger && hovering {
        CARD_DANGER_EDGE
    } else {
        pal.border
    };
    p.rect_stroke(r, round, Stroke::new(1.0, outline), StrokeKind::Inside);
    let ink = card_ink(pal, b, hovering);
    if !b.text.is_empty() {
        let f = if b.active || b.bold { bold(11.0) } else { regular(11.0) };
        p.with_clip_rect(r).text(r.center(), Align2::CENTER_CENTER, b.text, f, ink);
    }
    resp
}

fn card_ink(pal: &VstPalette, b: Btn, hovering: bool) -> Color32 {
    if b.active {
        if b.warn { CARD_WARN_INK } else { pal.on_accent }
    } else if b.danger && hovering {
        CARD_DANGER_INK
    } else if hovering {
        pal.text_bright
    } else {
        pal.text_dim
    }
}

/// The ink a [`card_button`] draws its text in, for glyphs drawn over one.
pub(crate) fn ink_of(pal: &VstPalette, resp: &Response, active: bool) -> Color32 {
    if active {
        pal.on_accent
    } else if resp.hovered() {
        pal.text_bright
    } else {
        pal.text_dim
    }
}

// ============================================================================ NumField (value box)

/// A value box's range and feel (`NumField`'s setup).
#[derive(Clone, Copy, Debug)]
pub struct ValueSpec {
    pub label: &'static str,
    pub lo: f64,
    pub hi: f64,
    /// A ratio per pixel (`perPx` as a factor) rather than a step.
    pub log: bool,
    /// `perPx`: the step (or ratio) a pixel of drag moves.
    pub fine: f64,
    /// Decimals shown when typing.
    pub decimals: usize,
    pub unit: &'static str,
    pub default: f64,
    /// The readout (`formatValue`); `None`: the number with `decimals` and `unit`.
    pub fmt: Option<fn(f64) -> String>,
}

impl ValueSpec {
    fn text(&self, v: f64) -> String {
        match self.fmt {
            Some(f) => f(v),
            None => format!("{v:.*}{}", self.decimals, self.unit),
        }
    }
}

/// One editor's value boxes: which (if any) is being typed in, and the drag start. One state can
/// serve every box of an editor (they are told apart by their salt).
#[derive(Clone, Default, Debug)]
pub struct ValueBoxState {
    /// The box being typed in, its text, and whether focus must still be requested.
    pub editing: Option<(Id, String, bool)>,
    drag: Option<(Id, f64)>,
}

impl ValueBoxState {
    /// Stops typing without committing.
    pub fn cancel(&mut self) {
        self.editing = None;
    }
}

/// What a value box did.
#[derive(Clone, Copy, Debug, Default)]
pub struct ValueOut {
    pub changed: Option<f64>,
    /// A gesture began (a drag pressed): close any open merge.
    pub started: bool,
    /// The gesture ended: a drag let go, or a one-shot change (typed, a wheel step).
    pub ended: bool,
    /// The change is a one-shot (typed, the wheel): its own undo step.
    pub single: bool,
}

/// `NumField`: drag up / down (a ratio per pixel for log fields, a step for linear ones; Shift
/// x0.25), the wheel (`deltaY * 40` pixels), double-click to type (Enter or losing focus
/// commits, Esc cancels), the hover border. `greyed`: drawn faint and inert.
#[allow(clippy::too_many_arguments)]
pub(crate) fn value_box(
    ui: &mut Ui,
    st: &mut ValueBoxState,
    r: Rect,
    salt: impl Hash + std::fmt::Debug,
    pal: &VstPalette,
    spec: &ValueSpec,
    value: f64,
    greyed: bool,
) -> ValueOut {
    let mut out = ValueOut::default();
    let id = ui.id().with(("vst-field", salt));
    let value = if value.is_finite() { value } else { spec.default };
    let resp = ui.interact(r, id, if greyed { Sense::hover() } else { Sense::click_and_drag() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::DragValue, !greyed, spec.label));
    let p = ui.painter().clone();
    p.rect_filled(r, CornerRadius::same(5), pal.field);
    p.rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0, if resp.hovered() { pal.border_hover } else { pal.border }), StrokeKind::Inside);
    let clamp = |v: f64| if v.is_finite() { v.clamp(spec.lo, spec.hi) } else { value };

    // Typing (`showEditor`).
    if let Some((_, text, fresh)) = st.editing.as_mut().filter(|(eid, ..)| *eid == id) {
        let er = Rect::from_min_size(pos2(r.left() + 6.0, r.top() + 3.0), vec2(r.width() - 12.0, 18.0));
        p.rect_filled(er, CornerRadius::same(2), pal.well);
        p.rect_stroke(er, CornerRadius::same(2), Stroke::new(1.0, pal.accent), StrokeKind::Inside);
        let te = egui::TextEdit::singleline(text)
            .id(id.with("text"))
            .font(FontId::proportional(12.0))
            .text_color(Color32::WHITE)
            .horizontal_align(egui::Align::Center)
            .frame(egui::Frame::NONE);
        let tr = ui.put(er, te);
        if *fresh {
            tr.request_focus();
            *fresh = false;
        }
        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if tr.lost_focus() {
            let typed = text.trim().trim_end_matches(spec.unit.trim()).trim().parse::<f64>().ok();
            st.editing = None;
            if !esc && let Some(v) = typed {
                let v = clamp(v);
                if (v - value).abs() > 1e-9 {
                    out.changed = Some(v);
                    out.single = true;
                    out.started = true;
                    out.ended = true;
                }
            }
        }
        return out;
    }

    if resp.drag_started() {
        st.drag = Some((id, value));
        out.started = true;
    }
    if resp.dragged()
        && let Some((_, start)) = st.drag.filter(|(d, _)| *d == id)
    {
        let dy = -f64::from(resp.total_drag_delta().map_or(0.0, |d| d.y));
        let fine = if ui.input(|i| i.modifiers.shift) { 0.25 } else { 1.0 };
        let v = clamp(if spec.log { start * spec.fine.powf(dy * fine) } else { start + spec.fine * dy * fine });
        if (v - value).abs() > 1e-12 {
            out.changed = Some(v);
        }
    }
    if resp.drag_stopped() {
        st.drag = None;
        out.ended = true;
    }
    if resp.hovered() && !greyed {
        let wheel = juce_wheel_delta_y(ui);
        if wheel != 0.0 {
            consume_wheel(ui);
            let fine = if ui.input(|i| i.modifiers.shift) { 0.25 } else { 1.0 };
            let dy = f64::from(wheel) * 40.0 * fine;
            let v = clamp(if spec.log { value * spec.fine.powf(dy) } else { value + spec.fine * dy });
            if (v - value).abs() > 1e-12 {
                out.changed = Some(v);
                out.single = true;
                out.started = true;
                out.ended = true;
            }
        }
    }
    if resp.double_clicked() && !greyed {
        st.editing = Some((id, format!("{value:.*}", spec.decimals), true));
    }

    let shown = out.changed.unwrap_or(value);
    let alpha = if greyed { 0.45 } else { 1.0 };
    let text_r = r.shrink2(vec2(2.0, 3.0));
    let value_r = Rect::from_min_max(text_r.min, pos2(text_r.max.x, text_r.min.y + text_r.height() * 0.62));
    let pc = p.with_clip_rect(text_r.intersect(p.clip_rect()));
    pc.text(value_r.center(), Align2::CENTER_CENTER, spec.text(shown), regular(12.5), al(pal.text_bright, alpha));
    pc.text(pos2(text_r.center().x, value_r.bottom()), Align2::CENTER_TOP, spec.label.to_uppercase(), regular(9.0), al(pal.text_faint, alpha));
    out
}

// ============================================================================ the linear slider

/// What a slider did.
#[derive(Clone, Copy, Debug, Default)]
pub struct SliderOut {
    /// The new proportion, when it moved.
    pub prop: Option<f32>,
    /// The gesture ended (let go, a click, a double-click, a wheel step).
    pub ended: bool,
}

/// A JUCE `LinearHorizontal` slider (LookAndFeel_V4: a 4 px track, the filled part in the
/// accent, a round thumb): press or drag puts the thumb under the pointer, double-click returns to
/// `default_prop`, the wheel nudges it. Values are proportions 0..1.
#[allow(clippy::too_many_arguments)]
pub(crate) fn h_slider(
    ui: &mut Ui,
    r: Rect,
    salt: impl Hash + std::fmt::Debug,
    pal: &VstPalette,
    prop: f32,
    default_prop: f32,
    dim: bool,
) -> SliderOut {
    let resp = ui.interact(r, ui.id().with(("vst-slider", salt)), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, "slider"));
    let prop = if prop.is_finite() { prop.clamp(0.0, 1.0) } else { 0.0 };
    let margin = 6.0;
    let to_prop = |x: f32| ((x - r.left() - margin) / (r.width() - 2.0 * margin).max(1.0)).clamp(0.0, 1.0);
    let mut out = SliderOut::default();
    if (resp.is_pointer_button_down_on() || resp.dragged())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        out.prop = Some(to_prop(pos.x));
    }
    if resp.double_clicked() {
        out.prop = Some(default_prop.clamp(0.0, 1.0));
    }
    let mut wheeled = false;
    if resp.hovered() {
        let wheel = juce_wheel_delta_y(ui);
        if wheel != 0.0 {
            consume_wheel(ui);
            // `Slider::mouseWheelMove`: the proportion moves by `deltaY * 0.15`.
            out.prop = Some((prop + wheel * 0.15).clamp(0.0, 1.0));
            wheeled = true;
        }
    }
    let shown = out.prop.unwrap_or(prop);
    let p = ui.painter();
    let a = if dim { 0.5 } else { 1.0 };
    let y = r.center().y;
    let (x0, x1) = (r.left() + margin, r.right() - margin);
    let tx = x0 + (x1 - x0) * shown;
    p.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(4.0, al(pal.well, a)));
    p.line_segment([pos2(x0, y), pos2(tx, y)], Stroke::new(4.0, al(pal.accent, a)));
    p.circle_filled(pos2(tx, y), 5.0, al(pal.text_bright, a));
    out.prop = out.prop.filter(|v| (*v - prop).abs() > 1e-6);
    out.ended = resp.drag_stopped() || (resp.clicked() && !resp.dragged()) || resp.double_clicked() || wheeled;
    out
}

// ============================================================================ glyph pills

/// A `Pill` whose text is ◀ or ▶ (the family's preset steppers; the UI font lacks the glyphs, so
/// the triangle is drawn in the pill's ink).
pub(crate) fn glyph_pill(ui: &mut Ui, rect: Rect, salt: impl Hash + std::fmt::Debug, pal: &VstPalette, left: bool, style: PillStyle) -> Response {
    let look = PillLook { tracked: false, ..PillLook::default() };
    let resp = pill_ex(ui, rect, salt, pal, "", false, style, look);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, if left { "Previous preset" } else { "Next preset" }));
    let ink = if resp.hovered() { pal.text_bright } else { pal.text_dim };
    let c = rect.center();
    let s = if left { -1.0 } else { 1.0 };
    let tri = vec![c + vec2(3.5 * s, 0.0), c + vec2(-2.5 * s, -4.0), c + vec2(-2.5 * s, 4.0)];
    ui.painter().add(egui::Shape::convex_polygon(tri, ink, Stroke::NONE));
    resp
}
