//! The window's pieces, drawn to the design's sizes: text in its weights, buttons (filled,
//! outlined, plain), the round Record, Stop and Play buttons' icons, level bars, text boxes, a
//! tick box and a volume slider. Every control is at least [`look::TOUCH`] tall, is reached with
//! Tab (a blue ring shows which has the keyboard), and tells screen readers (and the tests) its
//! name.

use super::look;
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, WidgetType, pos2, vec2};
use std::sync::Arc;

/// How heavy text is. Only Inter Regular is bundled: heavier weights are the regular face drawn
/// again a fraction of a point to the right (the strokes thicken, the shapes stay Inter's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Regular,
    /// 500.
    Medium,
    /// 600.
    Semi,
    /// 700.
    Bold,
}

impl Weight {
    /// How far the copies go, as a share of the size.
    fn spread(self) -> f32 {
        match self {
            Weight::Regular => 0.0,
            Weight::Medium => 0.018,
            Weight::Semi => 0.032,
            Weight::Bold => 0.045,
        }
    }
}

/// `text` laid out on one line.
pub fn galley(ui: &Ui, text: &str, size: f32, colour: Color32) -> Arc<egui::Galley> {
    ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(size), colour)
}

/// Draws a laid-out text at `pos` (its top left) in `weight`.
pub fn draw_galley(p: &egui::Painter, pos: Pos2, g: &Arc<egui::Galley>, size: f32, weight: Weight, colour: Color32) {
    p.galley(pos, Arc::clone(g), colour);
    let spread = size * weight.spread();
    if spread > 0.0 {
        let steps = (spread / 0.3).ceil().max(1.0) as usize;
        for i in 1..=steps {
            p.galley(pos + vec2(spread * i as f32 / steps as f32, 0.0), Arc::clone(g), colour);
        }
    }
}

/// Text at `pos`, anchored by `anchor`; the rectangle it took.
pub fn text_at(ui: &Ui, pos: Pos2, anchor: Align2, text: &str, size: f32, weight: Weight, colour: Color32) -> Rect {
    let g = galley(ui, text, size, colour);
    let r = anchor.anchor_size(pos, g.size() + vec2(size * weight.spread(), 0.0));
    draw_galley(ui.painter(), r.min, &g, size, weight, colour);
    r
}

/// The width text takes.
pub fn text_width(ui: &Ui, text: &str, size: f32, weight: Weight) -> f32 {
    galley(ui, text, size, look::TEXT).size().x + size * weight.spread()
}

/// A line of text in the layout (cut with `…` to the width there is).
pub fn label(ui: &mut Ui, text: &str, size: f32, weight: Weight, colour: Color32) -> Response {
    let fitted = fit(ui, text, size, weight, ui.available_width());
    let g = galley(ui, &fitted, size, colour);
    let (rect, r) = ui.allocate_exact_size(g.size() + vec2(size * weight.spread(), 0.0), Sense::hover());
    draw_galley(ui.painter(), rect.min, &g, size, weight, colour);
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, text));
    r
}

/// Text cut with `…` to `width`.
pub fn fit(ui: &Ui, text: &str, size: f32, weight: Weight, width: f32) -> String {
    if text_width(ui, text, size, weight) <= width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut n = chars.len();
    while n > 1 {
        n -= 1;
        let s: String = chars.iter().take(n).collect::<String>().trim_end().to_string() + "…";
        if text_width(ui, &s, size, weight) <= width {
            return s;
        }
    }
    "…".into()
}

/// Text wrapped to the width there is, `line` points apart.
pub fn paragraph(ui: &mut Ui, text: &str, size: f32, weight: Weight, colour: Color32, line: f32) -> Response {
    let width = ui.available_width();
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat { font_id: FontId::proportional(size), color: colour, line_height: Some(line), ..Default::default() },
    );
    job.wrap.max_width = width;
    let g = ui.painter().layout_job(job);
    let (rect, r) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
    draw_galley(ui.painter(), rect.min, &g, size, weight, colour);
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, text));
    r
}

/// A small heading in capitals, spaced out (`YOUR IDEAS`).
pub fn caps(ui: &mut Ui, text: &str) -> Response {
    let size = 12.0;
    let job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat { font_id: FontId::proportional(size), color: look::DIM, extra_letter_spacing: 0.04 * size, ..Default::default() },
    );
    let g = ui.painter().layout_job(job);
    let (rect, r) = ui.allocate_exact_size(g.size() + vec2(1.0, 0.0), Sense::hover());
    draw_galley(ui.painter(), rect.min, &g, size, Weight::Semi, look::DIM);
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, text));
    r
}

/// A button's colours and text.
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub fill: Option<Color32>,
    pub outline: Option<Color32>,
    pub ink: Color32,
    pub size: f32,
    pub weight: Weight,
    pub radius: f32,
}

impl Style {
    /// Filled, `fill` with `ink` text.
    pub fn filled(fill: Color32, ink: Color32, size: f32, weight: Weight, radius: f32) -> Style {
        Style { fill: Some(fill), outline: None, ink, size, weight, radius }
    }
    /// Outlined (the design's secondary buttons).
    pub fn outlined(size: f32, weight: Weight, radius: f32) -> Style {
        Style { fill: None, outline: Some(look::LINE), ink: look::TEXT, size, weight, radius }
    }
    /// Plain text (links, Throw away).
    pub fn plain(ink: Color32, size: f32, weight: Weight) -> Style {
        Style { fill: None, outline: None, ink, size, weight, radius: 10.0 }
    }
}

/// Lighter (hovered) or darker (pressed).
pub fn shade(c: Color32, by: i16) -> Color32 {
    let f = |v: u8| u8::try_from((i16::from(v) + by).clamp(0, 255)).unwrap_or(v);
    Color32::from_rgba_unmultiplied(f(c.r()), f(c.g()), f(c.b()), c.a())
}

/// A clickable area (`label` for screen readers and tests), with the focus ring when it has the
/// keyboard. Greyed: it neither clicks nor takes the keyboard.
pub fn hit(ui: &mut Ui, rect: Rect, label: &str, enabled: bool) -> Response {
    let id = ui.id().with(("hit", label));
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let r = ui.interact(rect, id, sense);
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    r
}

/// Text drawn by hand in `rect`, read out (and found by the tests) as a label.
pub fn read_as(ui: &mut Ui, rect: Rect, text: &str) {
    let id = ui.id().with(("text", text, rect.min.x as i32, rect.min.y as i32));
    ui.interact(rect, id, Sense::hover()).widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, text));
}

/// The ring round the control that has the keyboard.
pub fn focus_ring(ui: &Ui, r: &Response, radius: f32) {
    if r.has_focus() {
        ui.painter().rect_stroke(
            r.rect.expand(3.0),
            CornerRadius::same((radius + 3.0).min(255.0) as u8),
            Stroke::new(2.0, look::LINK),
            StrokeKind::Outside,
        );
    }
}

/// A button filling `rect` with `text` centred.
pub fn button_in(ui: &mut Ui, rect: Rect, text: &str, s: Style, enabled: bool) -> Response {
    button_named(ui, rect, text, text, s, enabled)
}

/// [`button_in`], named `name` for screen readers (when the text alone is not enough: "Join"
/// beside each studio).
pub fn button_named(ui: &mut Ui, rect: Rect, text: &str, name: &str, s: Style, enabled: bool) -> Response {
    let r = hit(ui, rect, name, enabled);
    let by = if !enabled {
        0
    } else if r.is_pointer_button_down_on() {
        -14
    } else if r.hovered() {
        14
    } else {
        0
    };
    let radius = CornerRadius::same(s.radius.min(255.0) as u8);
    let p = ui.painter();
    match s.fill {
        Some(f) => {
            p.rect_filled(rect, radius, if enabled { shade(f, by) } else { look::OFF });
        }
        None if by != 0 => {
            p.rect_filled(rect, radius, Color32::from_white_alpha(if by > 0 { 10 } else { 4 }));
        }
        None => {}
    }
    if let Some(o) = s.outline {
        p.rect_stroke(rect, radius, Stroke::new(1.0, o), StrokeKind::Inside);
    }
    let ink = if enabled { s.ink } else { look::DIM };
    let shown = fit(ui, text, s.size, s.weight, rect.width() - 12.0);
    text_at(ui, rect.center(), Align2::CENTER_CENTER, &shown, s.size, s.weight, ink);
    focus_ring(ui, &r, s.radius);
    r
}

/// A button `size` big in the layout.
pub fn button(ui: &mut Ui, size: Vec2, text: &str, s: Style, enabled: bool) -> Response {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    button_in(ui, rect, text, s, enabled)
}

/// A button as wide as its text (`pad` each side), `height` tall.
pub fn button_fit(ui: &mut Ui, height: f32, pad: f32, text: &str, s: Style, enabled: bool) -> Response {
    let w = text_width(ui, text, s.size, s.weight) + 2.0 * pad;
    button(ui, vec2(w, height), text, s, enabled)
}

/// A play triangle (pointing right) in a `size` box at `c`.
pub fn play_icon(p: &egui::Painter, c: Pos2, size: f32, colour: Color32) {
    // The design's path: M7 4 L21 12 L7 20, in a 24 box.
    let k = size / 24.0;
    let at = |x: f32, y: f32| pos2(c.x + (x - 12.0) * k, c.y + (y - 12.0) * k);
    p.add(egui::Shape::convex_polygon(vec![at(7.0, 4.0), at(21.0, 12.0), at(7.0, 20.0)], colour, Stroke::NONE));
}

/// A stop square, `size` wide, at `c`.
pub fn stop_icon(p: &egui::Painter, c: Pos2, size: f32, radius: f32, colour: Color32) {
    p.rect_filled(Rect::from_center_size(c, vec2(size, size)), CornerRadius::same(radius.min(255.0) as u8), colour);
}

/// The settings icon (two sliders), 20 points, at `c`.
pub fn sliders_icon(p: &egui::Painter, c: Pos2, colour: Color32, behind: Color32) {
    let k = 20.0 / 24.0;
    let at = |x: f32, y: f32| pos2(c.x + (x - 12.0) * k, c.y + (y - 12.0) * k);
    let stroke = Stroke::new(2.0 * k, colour);
    for (y, knob) in [(7.0, 9.0), (17.0, 15.0)] {
        p.line_segment([at(4.0, y), at(20.0, y)], stroke);
        p.circle(at(knob, y), 2.5 * k, behind, stroke);
    }
}

/// A check mark, `size` wide, at `c`.
pub fn check_icon(p: &egui::Painter, c: Pos2, size: f32, colour: Color32, width: f32) {
    let k = size / 24.0;
    let at = |x: f32, y: f32| pos2(c.x + (x - 12.0) * k, c.y + (y - 12.0) * k);
    p.line(vec![at(5.0, 12.5), at(9.5, 17.0), at(19.0, 7.5)], Stroke::new(width * k, colour));
}

/// The loop icon (two arrows chasing round), 18 points, at `c`.
pub fn loop_icon(p: &egui::Painter, c: Pos2, colour: Color32) {
    let stroke = Stroke::new(1.8, colour);
    let r = 6.5;
    let arc = |from: f32, to: f32| -> Vec<Pos2> {
        (0..=10).map(|i| from + (to - from) * i as f32 / 10.0).map(|a| pos2(c.x + r * a.cos(), c.y + r * a.sin())).collect()
    };
    use std::f32::consts::PI;
    p.line(arc(PI * 1.15, PI * 1.95), stroke);
    p.line(arc(PI * 0.15, PI * 0.95), stroke);
    // Arrow heads at the arcs' ends.
    for a in [PI * 1.95, PI * 0.95] {
        let tip = pos2(c.x + r * a.cos(), c.y + r * a.sin());
        let along = vec2(-a.sin(), a.cos());
        let out = vec2(a.cos(), a.sin());
        let back = tip - along * 3.5;
        p.add(egui::Shape::convex_polygon(vec![tip + along * 1.5, back + out * 3.0, back - out * 3.0], colour, Stroke::NONE));
    }
}

/// A cross (×), `size` wide, at `c`.
pub fn cross_icon(p: &egui::Painter, c: Pos2, size: f32, colour: Color32) {
    let h = size / 2.0;
    let s = Stroke::new(2.0, colour);
    p.line_segment([c + vec2(-h, -h), c + vec2(h, h)], s);
    p.line_segment([c + vec2(-h, h), c + vec2(h, -h)], s);
}

/// A back chevron, `size` wide, at `c`.
pub fn back_icon(p: &egui::Painter, c: Pos2, size: f32, colour: Color32) {
    let k = size / 24.0;
    let at = |x: f32, y: f32| pos2(c.x + (x - 12.0) * k, c.y + (y - 12.0) * k);
    p.line(vec![at(15.0, 6.0), at(9.0, 12.0), at(15.0, 18.0)], Stroke::new(2.2 * k, colour));
}

/// A level bar (`level` 0..1 peak, shown on a -60..0 dB scale) in `rect`, rounded.
pub fn level_bar(ui: &Ui, rect: Rect, level: f32) {
    let p = ui.painter();
    let radius = CornerRadius::same((rect.height() / 2.0) as u8);
    p.rect_filled(rect, radius, look::TRACK);
    let pos = level_pos(level);
    if pos > 0.0 {
        let mut lit = rect;
        lit.set_width((rect.width() * pos).max(rect.height()));
        let colour = if level >= 0.97 { look::RED_LIVE } else { look::GREEN };
        p.rect_filled(lit, radius, colour);
    }
}

/// A peak (0..1) on the meters' scale (-60..0 dB) as 0..1.
pub fn level_pos(level: f32) -> f32 {
    if level <= 1e-6 {
        return 0.0;
    }
    ((20.0 * level.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// A text box filling `rect` (the design's: dark, rounded, outlined), its text `size` points.
pub fn text_box(ui: &mut Ui, rect: Rect, text: &mut String, hint: &str, id: &str, size: f32, radius: f32) -> Response {
    let cr = CornerRadius::same(radius.min(255.0) as u8);
    ui.painter().rect_filled(rect, cr, look::FIELD);
    let row = ui.fonts_mut(|f| f.row_height(&FontId::proportional(size)));
    let inner = rect.shrink2(vec2(14.0, ((rect.height() - row) / 2.0).max(0.0)));
    let edit = egui::TextEdit::singleline(text)
        .id_salt(id)
        .font(FontId::proportional(size))
        .text_color(look::TEXT)
        .hint_text(egui::RichText::new(hint).color(Color32::from_rgb(0x7a, 0x7e, 0x88)).size(size))
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::ZERO)
        .desired_width(inner.width());
    // In a child of its own: the box's rectangle was already taken in the layout.
    let r = ui.new_child(egui::UiBuilder::new().max_rect(inner)).add(edit);
    let line = if r.has_focus() { look::LINK } else { look::LINE_FIELD };
    ui.painter().rect_stroke(rect, cr, Stroke::new(1.0, line), StrokeKind::Inside);
    r
}

/// A tick box with its words; the whole row clicks. Returns whether it changed.
pub fn tick(ui: &mut Ui, on: &mut bool, text: &str, size: f32, width: f32) -> Response {
    let (rect, _) = ui.allocate_exact_size(vec2(width, look::TOUCH), Sense::hover());
    let id = ui.id().with(("tick", text));
    let mut r = ui.interact(rect, id, Sense::click());
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    r.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, true, *on, text));
    let p = ui.painter();
    let b = Rect::from_center_size(pos2(rect.left() + 11.0, rect.center().y), vec2(22.0, 22.0));
    if *on {
        p.rect_filled(b, CornerRadius::same(5), look::GREEN);
        check_icon(p, b.center(), 18.0, look::BG, 3.0);
    } else {
        p.rect_filled(b, CornerRadius::same(5), look::FIELD);
        p.rect_stroke(b, CornerRadius::same(5), Stroke::new(1.5, look::LINE_FIELD), StrokeKind::Inside);
    }
    text_at(ui, pos2(b.right() + 12.0, rect.center().y), Align2::LEFT_CENTER, text, size, Weight::Regular, look::TEXT);
    focus_ring(ui, &r, 6.0);
    r
}

/// A volume slider (0..1 of its range) `width` wide; arrow keys move it when it has the keyboard.
pub fn slider(ui: &mut Ui, value: &mut f32, width: f32, label: &str) -> Response {
    let (rect, _) = ui.allocate_exact_size(vec2(width, look::TOUCH), Sense::hover());
    let id = ui.id().with(("slider", label));
    let mut r = ui.interact(rect, id, Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), vec2(width - 16.0, 4.0));
    if let Some(p) = r.interact_pointer_pos().filter(|_| r.dragged() || r.clicked()) {
        *value = ((p.x - track.left()) / track.width()).clamp(0.0, 1.0);
        r.mark_changed();
    }
    if r.has_focus() {
        let step = ui.input(|i| {
            f32::from(u8::from(i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowUp)))
                - f32::from(u8::from(i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowDown)))
        });
        if step != 0.0 {
            *value = (*value + step * 0.05).clamp(0.0, 1.0);
            r.mark_changed();
        }
    }
    let v = *value;
    r.widget_info(|| {
        let mut w = WidgetInfo::labeled(WidgetType::Slider, true, label);
        w.value = Some(f64::from(v));
        w
    });
    let p = ui.painter();
    p.rect_filled(track, CornerRadius::same(2), look::LINE_FIELD);
    let mut lit = track;
    lit.set_width(track.width() * v);
    p.rect_filled(lit, CornerRadius::same(2), look::GREEN);
    p.circle_filled(pos2(track.left() + track.width() * v, track.center().y), 9.0, look::TEXT);
    focus_ring(ui, &r, 8.0);
    r
}

/// A dashed rounded outline (the studio screen's preview card).
pub fn dashed(p: &egui::Painter, rect: Rect, radius: f32, colour: Color32) {
    let stroke = Stroke::new(1.0, colour);
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    // Straight sides, dashed; the corners drawn whole.
    let dash = |a: Pos2, b: Pos2| p.add(egui::Shape::dashed_line(&[a, b], stroke, 5.0, 4.0));
    dash(pos2(rect.left() + r, rect.top()), pos2(rect.right() - r, rect.top()));
    dash(pos2(rect.left() + r, rect.bottom()), pos2(rect.right() - r, rect.bottom()));
    dash(pos2(rect.left(), rect.top() + r), pos2(rect.left(), rect.bottom() - r));
    dash(pos2(rect.right(), rect.top() + r), pos2(rect.right(), rect.bottom() - r));
    for (c, start) in [
        (pos2(rect.left() + r, rect.top() + r), std::f32::consts::PI),
        (pos2(rect.right() - r, rect.top() + r), 1.5 * std::f32::consts::PI),
        (pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (pos2(rect.left() + r, rect.bottom() - r), 0.5 * std::f32::consts::PI),
    ] {
        let pts: Vec<Pos2> = (0..=8).map(|i| start + i as f32 / 8.0 * std::f32::consts::FRAC_PI_2).map(|a| c + vec2(a.cos(), a.sin()) * r).collect();
        p.line(pts, stroke);
    }
}
