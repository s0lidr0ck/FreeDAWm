//! `ChamberControls.h` (and `PluginEditor.h`'s `PresetField`), translated: ChamberMade's own look
//! (`cm::`), `ChamberKnob` (its face the DAW's knob, the owner's rule; its gestures, readout and
//! label as the C++), `ChamberButton`, `ChamberLed`, `Dropdown` with its popup menu, and the
//! header's preset field.

use crate::editor_kit::palette::{al, argb};
use crate::editor_kit::{consume_wheel, juce_wheel_delta_y};
use crate::theme::{bold, mono_bold, regular};
use crate::widgets::{KnobStyle, paint_knob_face};
use egui::{Align2, Color32, CornerRadius, CursorIcon, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use std::hash::Hash;

/// `cm::`: navy panels, an orange accent, a tan for the tape machine.
pub(crate) mod cm {
    use super::{Color32, argb};
    pub const BACKGROUND: Color32 = argb(0xFF0B141C);
    pub const PANEL: Color32 = argb(0xFF0F1B26);
    pub const PANEL_LINE: Color32 = argb(0xFF243649);
    pub const GROUP: Color32 = argb(0xFF112130);
    pub const GROUP_LINE: Color32 = argb(0xFF2A3E52);
    pub const FIELD: Color32 = argb(0xFF132330);
    pub const FIELD_HOVER: Color32 = argb(0xFF182B3A);
    pub const FIELD_LINE: Color32 = argb(0xFF2C4155);
    pub const FIELD_LINE_HOVER: Color32 = argb(0xFF3D5670);
    pub const TEXT: Color32 = argb(0xFFE8EEF4);
    pub const LABEL: Color32 = argb(0xFF97AABD);
    pub const LABEL_BLUE: Color32 = argb(0xFF8FB2D4);
    pub const FAINT: Color32 = argb(0xFF5D7086);
    pub const ACCENT: Color32 = argb(0xFFFF9A3D);
    pub const BYPASS: Color32 = argb(0xFFFFA646);
    pub const TAPE: Color32 = argb(0xFFD9A56B);
    pub const ON_ACCENT: Color32 = argb(0xFF10161C);
    pub const ARC: Color32 = argb(0xFFDCE4EC);
    /// `cm::label.brighter (0.2f)`.
    pub const LABEL_BRIGHT: Color32 = argb(0xFFA8B8C8);
    pub const WARN: Color32 = argb(0xFFFF6B6B);
}

/// `Colour::brighter (amount)`.
pub(crate) fn brighter(c: Color32, amount: f32) -> Color32 {
    let k = 1.0 / (1.0 + amount);
    let f = |x: u8| (255.0 - k * (255.0 - f32::from(x))) as u8;
    Color32::from_rgba_unmultiplied(f(c.r()), f(c.g()), f(c.b()), c.a())
}

/// `cm::drawPanel`.
pub(crate) fn draw_panel(p: &egui::Painter, r: Rect, fill: Color32, line: Color32, corner: f32) {
    let c = CornerRadius::same(corner.round() as u8);
    p.rect_filled(r, c, fill);
    p.rect_stroke(r.shrink(0.5), c, Stroke::new(1.0, line), StrokeKind::Middle);
}

/// `cm::ui`.
pub(crate) fn ui_font(h: f32, b: bool) -> egui::FontId {
    if b { bold(h) } else { regular(h) }
}

pub(crate) fn text_width(p: &egui::Painter, s: &str, f: egui::FontId) -> f32 {
    p.layout_no_wrap(s.to_string(), f, Color32::WHITE).size().x
}

// ============================================================================ ChamberKnob

/// A knob's settings (`setLabel`, `setArcColour`, `setLabelColour`, `setLarge`, `setLocked`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct KnobOpts<'a> {
    pub label: &'a str,
    pub value_text: &'a str,
    pub default: f32,
    pub arc: Color32,
    pub label_colour: Color32,
    pub large: bool,
    pub locked: bool,
}

impl Default for KnobOpts<'_> {
    fn default() -> Self {
        KnobOpts { label: "", value_text: "", default: 0.5, arc: cm::ARC, label_colour: cm::LABEL, large: false, locked: false }
    }
}

#[derive(Debug, Default)]
pub(crate) struct KnobOut {
    pub value: Option<f32>,
    pub started: bool,
    pub ended: bool,
}

#[derive(Clone, Copy, Default)]
struct KnobDrag {
    start_y: f32,
    start_value: f32,
}

/// `knobBounds`.
fn knob_bounds(b: Rect, large: bool) -> Rect {
    let text = if large { 50.0 } else { 44.0 };
    let s = (b.width() - 4.0).min(b.height() - text).min(if large { 80.0 } else { 58.0 }).max(0.0);
    Rect::from_min_size(pos2(b.center().x - s * 0.5, b.top() + 2.0), vec2(s, s))
}

/// `ChamberKnob`: press on the knob (its bounds grown 4 px) and drag (`0.006`/px, Shift
/// `0.0015`), double-click to the default, the wheel (`deltaY * 0.2`); locked takes nothing.
pub(crate) fn chamber_knob(ui: &mut Ui, r: Rect, salt: impl Hash + std::fmt::Debug, v: f32, o: &KnobOpts) -> KnobOut {
    let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    let k = knob_bounds(r, o.large);
    let id = ui.id().with(("chamber-knob", salt));
    let resp = ui.interact(r, id, if o.locked { Sense::hover() } else { Sense::click_and_drag() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, !o.locked, o.label));
    let mut out = KnobOut::default();
    let key = id.with("drag");
    if !o.locked {
        let press = ui.input(|i| i.pointer.press_origin());
        if resp.drag_started()
            && let Some(p) = press
            && k.expand(4.0).contains(p)
        {
            ui.data_mut(|d| d.insert_temp(key, KnobDrag { start_y: p.y, start_value: v }));
            out.started = true;
        }
        if resp.dragged()
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(dr) = ui.data(|d| d.get_temp::<KnobDrag>(key))
        {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
            let sens = if ui.input(|i| i.modifiers.shift) { 0.0015 } else { 0.006 };
            let nv = (dr.start_value + (dr.start_y - p.y) * sens).clamp(0.0, 1.0);
            if (nv - v).abs() > f32::EPSILON {
                out.value = Some(nv);
            }
        }
        if resp.drag_stopped() && ui.data_mut(|d| d.remove_temp::<KnobDrag>(key)).is_some() {
            out.ended = true;
        }
        if resp.double_clicked() && press.is_some_and(|p| k.contains(p)) {
            out.value = Some(o.default.clamp(0.0, 1.0));
            out.started = true;
            out.ended = true;
        }
        if resp.hovered() {
            let dy = juce_wheel_delta_y(ui);
            if dy != 0.0 {
                consume_wheel(ui);
                let nv = (v + dy * 0.2).clamp(0.0, 1.0);
                if (nv - v).abs() > 1e-6 {
                    out.value = Some(nv);
                    out.started = true;
                    out.ended = true;
                }
            }
        }
    }
    let shown = out.value.unwrap_or(v);
    let p = ui.painter();
    let alpha = if o.locked { 0.4 } else { 1.0 };
    if o.locked {
        let mut l = p.clone();
        l.multiply_opacity(alpha);
        paint_knob_face(&l, k, shown, o.arc, KnobStyle::Dark, false, false);
    } else {
        paint_knob_face(p, k, shown, o.arc, KnobStyle::Dark, resp.hovered(), resp.dragged());
    }
    let below_top = k.bottom() + if o.large { 8.0 } else { 6.0 };
    let vh = if o.large { 18.0 } else { 16.0 };
    let lh = if o.large { 20.0 } else { 18.0 };
    let value_row = Rect::from_min_size(pos2(r.left(), below_top), vec2(r.width(), vh));
    let label_row = Rect::from_min_size(pos2(r.left(), below_top + vh), vec2(r.width(), lh));
    p.text(value_row.center(), Align2::CENTER_CENTER, o.value_text, mono_bold(if o.large { 13.5 } else { 12.5 }), al(cm::TEXT, alpha));
    p.text(label_row.center(), Align2::CENTER_CENTER, o.label, regular(if o.large { 14.5 } else { 14.0 }), al(o.label_colour, alpha));
    out
}

// ============================================================================ ChamberButton

/// A `ChamberButton`'s state.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ButtonLook {
    pub active: bool,
    pub always_filled: bool,
    pub fill: Color32,
    pub font_h: f32,
    /// (joinLeft, joinRight): square corners on that side.
    pub join: (bool, bool),
    pub warn: bool,
}

impl Default for ButtonLook {
    fn default() -> Self {
        ButtonLook { active: false, always_filled: false, fill: cm::ACCENT, font_h: 14.0, join: (false, false), warn: false }
    }
}

/// `ChamberButton`: filled when active (or always), outlined otherwise; clicks on mouse up.
pub(crate) fn chamber_button(ui: &mut Ui, r: Rect, salt: impl Hash + std::fmt::Debug, text: &str, l: ButtonLook) -> egui::Response {
    let resp = ui.interact(r, ui.id().with(("chamber-btn", salt)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, l.active, text));
    let p = ui.painter();
    let b = r.shrink(0.5);
    let c = 4u8;
    let corners = CornerRadius {
        nw: if l.join.0 { 0 } else { c },
        sw: if l.join.0 { 0 } else { c },
        ne: if l.join.1 { 0 } else { c },
        se: if l.join.1 { 0 } else { c },
    };
    let filled = l.active || l.always_filled;
    let hovered = resp.hovered();
    if filled {
        p.rect_filled(b, corners, if hovered { brighter(l.fill, 0.08) } else { l.fill });
    } else {
        p.rect_filled(b, corners, if hovered { cm::FIELD_HOVER } else { cm::FIELD });
        let line = if l.warn {
            cm::WARN
        } else if hovered {
            cm::FIELD_LINE_HOVER
        } else {
            cm::FIELD_LINE
        };
        p.rect_stroke(b, corners, Stroke::new(1.0, line), StrokeKind::Middle);
    }
    let ink = if filled {
        cm::ON_ACCENT
    } else if l.warn {
        cm::WARN
    } else {
        cm::LABEL_BRIGHT
    };
    p.text(r.center(), Align2::CENTER_CENTER, text, regular(l.font_h), ink);
    resp
}

/// The ◀ / ▶ a header button carries (U+25C0 / U+25B6).
pub(crate) fn tri_side(p: &egui::Painter, c: Pos2, left: bool, col: Color32) {
    let s = if left { -1.0 } else { 1.0 };
    p.add(Shape::convex_polygon(vec![c + vec2(4.0 * s, 0.0), c + vec2(-3.0 * s, -4.5), c + vec2(-3.0 * s, 4.5)], col, Stroke::NONE));
}

/// `ChamberLed`.
pub(crate) fn led(p: &egui::Painter, r: Rect, on: bool) {
    let c = r.center();
    p.circle_filled(c, 5.0, if on { cm::BYPASS } else { argb(0xFF2B2A22) });
    p.circle_stroke(c, 5.0, Stroke::new(1.0, if on { brighter(cm::BYPASS, 0.3) } else { argb(0xFF3A382C) }));
}

/// The small caret a field carries.
fn caret(p: &egui::Painter, c: Pos2) {
    p.add(Shape::convex_polygon(vec![pos2(c.x - 3.0, c.y - 1.5), pos2(c.x + 3.0, c.y - 1.5), pos2(c.x, c.y + 2.0)], cm::LABEL, Stroke::NONE));
}

/// `PresetField`: the name (and " *" when edited) on the left, a caret on the right; a click
/// opens the browser.
pub(crate) fn preset_field(ui: &mut Ui, r: Rect, name: &str, dirty: bool) -> egui::Response {
    let resp = ui.interact(r, ui.id().with("chamber-preset-field"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Preset {name}")));
    let p = ui.painter();
    let b = r.shrink(0.5);
    let h = resp.hovered();
    p.rect_filled(b, CornerRadius::same(4), if h { cm::FIELD_HOVER } else { cm::FIELD });
    p.rect_stroke(b, CornerRadius::same(4), Stroke::new(1.0, if h { cm::FIELD_LINE_HOVER } else { cm::FIELD_LINE }), StrokeKind::Middle);
    let t = b.shrink2(vec2(14.0, 0.0));
    let caret_r = Rect::from_min_max(pos2(t.right() - 12.0, t.top()), t.max);
    let text_r = Rect::from_min_max(t.min, pos2(caret_r.left(), t.bottom()));
    let s = if dirty { format!("{name} *") } else { name.to_string() };
    p.with_clip_rect(text_r).text(pos2(text_r.left(), text_r.center().y), Align2::LEFT_CENTER, s, regular(15.5), cm::TEXT);
    caret(p, caret_r.center());
    resp
}

// ============================================================================ Dropdown

/// `Dropdown`: the value, a chip for materials, and a caret; a click opens its menu (each
/// option, the selected one ticked). Returns the option picked.
#[allow(clippy::too_many_arguments)]
pub(crate) fn dropdown(
    ui: &mut Ui,
    r: Rect,
    salt: impl Hash + Copy + std::fmt::Debug,
    options: &[&str],
    selected: usize,
    swatch: Option<fn(usize) -> Color32>,
    font_h: f32,
) -> Option<usize> {
    let id = ui.id().with(("chamber-dropdown", salt));
    let resp = ui.interact(r, id, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, options.get(selected).copied().unwrap_or("")));
    let p = ui.painter();
    let b = r.shrink(0.5);
    let h = resp.hovered();
    p.rect_filled(b, CornerRadius::same(4), if h { cm::FIELD_HOVER } else { cm::FIELD });
    p.rect_stroke(b, CornerRadius::same(4), Stroke::new(1.0, if h { cm::FIELD_LINE_HOVER } else { cm::FIELD_LINE }), StrokeKind::Middle);
    let mut t = b.shrink2(vec2(11.0, 0.0));
    let chev = Rect::from_min_max(pos2(t.right() - 12.0, t.top()), t.max);
    t.max.x -= 12.0;
    if let Some(sw) = swatch {
        let chip = Rect::from_center_size(pos2(t.left() + 6.0, t.center().y), vec2(11.0, 11.0));
        p.rect_filled(chip, CornerRadius::same(2), sw(selected));
        t.min.x += 12.0 + 9.0;
    }
    let text = options.get(selected).copied().unwrap_or("");
    p.with_clip_rect(t).text(pos2(t.left(), t.center().y), Align2::LEFT_CENTER, text, regular(font_h), cm::TEXT);
    caret(p, chev.center());

    let open_key = id.with("open");
    let mut open = ui.data(|d| d.get_temp::<bool>(open_key)).unwrap_or(false);
    if resp.clicked() && !options.is_empty() {
        open = !open;
    }
    let mut picked = None;
    if open {
        let row_h = 24.0;
        let w = r.width().max(120.0);
        let area = egui::Area::new(id.with("menu")).order(egui::Order::Foreground).fixed_pos(r.left_bottom() + vec2(0.0, 2.0));
        let inner = area.show(ui.ctx(), |mui| {
            let (menu, _) = mui.allocate_exact_size(vec2(w, row_h * options.len() as f32 + 8.0), Sense::hover());
            let mp = mui.painter();
            mp.rect_filled(menu, CornerRadius::same(4), argb(0xFF1B2733));
            mp.rect_stroke(menu, CornerRadius::same(4), Stroke::new(1.0, cm::FIELD_LINE), StrokeKind::Inside);
            for (i, o) in options.iter().enumerate() {
                let row = Rect::from_min_size(menu.min + vec2(4.0, 4.0 + row_h * i as f32), vec2(w - 8.0, row_h));
                let rr = mui.interact(row, id.with(("item", i)), Sense::click());
                rr.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, i == selected, *o));
                if rr.hovered() {
                    mp.rect_filled(row, CornerRadius::same(3), al(cm::ACCENT, 0.25));
                }
                if i == selected {
                    let c = pos2(row.left() + 10.0, row.center().y);
                    mp.line_segment([c + vec2(-4.0, 0.0), c + vec2(-1.0, 3.0)], Stroke::new(1.5, cm::TEXT));
                    mp.line_segment([c + vec2(-1.0, 3.0), c + vec2(4.0, -3.5)], Stroke::new(1.5, cm::TEXT));
                }
                let mut tx = row.left() + 22.0;
                if let Some(sw) = swatch {
                    mp.rect_filled(Rect::from_center_size(pos2(tx + 5.0, row.center().y), vec2(11.0, 11.0)), CornerRadius::same(2), sw(i));
                    tx += 18.0;
                }
                mp.text(pos2(tx, row.center().y), Align2::LEFT_CENTER, *o, regular(13.5), cm::TEXT);
                if rr.clicked() {
                    picked = Some(i);
                }
            }
        });
        let clicked_elsewhere = ui.input(|i| i.pointer.any_click())
            && !resp.clicked()
            && !inner.response.rect.contains(ui.input(|i| i.pointer.interact_pos()).unwrap_or(Pos2::ZERO));
        if picked.is_some() || clicked_elsewhere || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
    }
    ui.data_mut(|d| d.insert_temp(open_key, open));
    picked
}

/// The text box `drawFittedText (..., topLeft, 5 lines)` lays out: wrapped to `r`'s width, clipped.
pub(crate) fn fitted_text(p: &egui::Painter, r: Rect, text: &str, size: f32, c: Color32) {
    if !r.is_positive() {
        return;
    }
    let galley = p.layout(text.to_string(), regular(size), c, r.width());
    p.with_clip_rect(r.intersect(p.clip_rect())).galley(r.min, galley, c);
}

/// Text rotated about its centre (`addTransform (rotation)` around a drawn box).
pub(crate) fn text_rotated(p: &egui::Painter, centre: Pos2, text: &str, f: egui::FontId, c: Color32, angle: f32) {
    let galley = p.layout_no_wrap(text.to_string(), f, c);
    let half = galley.size() * 0.5;
    let (s, co) = angle.sin_cos();
    let off = vec2(half.x * co - half.y * s, half.x * s + half.y * co);
    p.add(egui::epaint::TextShape::new(centre - off, galley, c).with_angle(angle));
}
