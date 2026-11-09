//! The look of Setup › Network: drawn icons, a switch, tiles, a segmented control and the map
//! of who is connected. Plain egui painting in the app's theme colours; no assets.

use crate::theme::Tokens;
use egui::{Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

/// The colour of a link on this network, over the internet directly, and through a relay.
pub const LAN: Color32 = Color32::from_rgb(88, 196, 120);
pub const NET: Color32 = Color32::from_rgb(92, 160, 255);
pub const RELAY: Color32 = Color32::from_rgb(240, 198, 116);

/// The icons drawn here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Pc,
    Phone,
    Globe,
    Wifi,
    Mic,
    Faders,
}

/// Draws `icon` centred on `c`, about `s` points across.
pub fn icon(p: &Painter, c: Pos2, s: f32, icon: Icon, col: Color32) {
    let w = (s * 0.09).max(1.2);
    let st = Stroke::new(w, col);
    let h = s * 0.5;
    match icon {
        Icon::Pc => {
            let screen = Rect::from_center_size(c - vec2(0.0, s * 0.08), vec2(s * 0.86, s * 0.56));
            p.rect_stroke(screen, CornerRadius::same((s * 0.08) as u8), st, StrokeKind::Middle);
            p.line_segment([pos2(c.x, screen.max.y), pos2(c.x, screen.max.y + s * 0.16)], st);
            p.line_segment([pos2(c.x - s * 0.2, screen.max.y + s * 0.18), pos2(c.x + s * 0.2, screen.max.y + s * 0.18)], st);
        }
        Icon::Phone => {
            let body = Rect::from_center_size(c, vec2(s * 0.48, s * 0.86));
            p.rect_stroke(body, CornerRadius::same((s * 0.1) as u8), st, StrokeKind::Middle);
            p.circle_filled(pos2(c.x, body.max.y - s * 0.11), w * 0.9, col);
        }
        Icon::Globe => {
            let r = h * 0.86;
            p.circle_stroke(c, r, st);
            let ell = |rx: f32| -> Vec<Pos2> {
                (0..=24).map(|i| i as f32 / 24.0 * std::f32::consts::TAU).map(|a| pos2(c.x + rx * a.cos(), c.y + r * a.sin())).collect()
            };
            p.add(egui::Shape::closed_line(ell(r * 0.42), st));
            p.line_segment([pos2(c.x - r, c.y), pos2(c.x + r, c.y)], st);
            for dy in [-0.55f32, 0.55] {
                let y = c.y + r * dy;
                let half = r * (1.0 - dy * dy).sqrt();
                p.line_segment([pos2(c.x - half, y), pos2(c.x + half, y)], st);
            }
        }
        Icon::Wifi => {
            let base = pos2(c.x, c.y + s * 0.3);
            for (i, k) in [0.32f32, 0.56, 0.8].iter().enumerate() {
                let r = s * k * 0.62;
                let pts: Vec<Pos2> = (0..=16)
                    .map(|j| -std::f32::consts::FRAC_PI_4 * 3.0 + j as f32 / 16.0 * std::f32::consts::FRAC_PI_2)
                    .map(|a| pos2(base.x + r * a.cos(), base.y + r * a.sin()))
                    .collect();
                let _ = i;
                p.add(egui::Shape::line(pts, st));
            }
            p.circle_filled(base, w * 1.3, col);
        }
        Icon::Mic => {
            let cap = Rect::from_center_size(c - vec2(0.0, s * 0.14), vec2(s * 0.3, s * 0.46));
            p.rect_stroke(cap, CornerRadius::same((s * 0.15) as u8), st, StrokeKind::Middle);
            let cy = c.y - s * 0.06;
            let pts: Vec<Pos2> =
                (0..=16).map(|j| j as f32 / 16.0 * std::f32::consts::PI).map(|a| pos2(c.x + s * 0.27 * a.cos(), cy + s * 0.22 * a.sin())).collect();
            p.add(egui::Shape::line(pts, st));
            p.line_segment([pos2(c.x, cy + s * 0.22), pos2(c.x, c.y + s * 0.36)], st);
            p.line_segment([pos2(c.x - s * 0.14, c.y + s * 0.38), pos2(c.x + s * 0.14, c.y + s * 0.38)], st);
        }
        Icon::Faders => {
            for (i, k) in [0.25f32, -0.2, 0.08].iter().enumerate() {
                let x = c.x + (i as f32 - 1.0) * s * 0.3;
                p.line_segment([pos2(x, c.y - h * 0.9), pos2(x, c.y + h * 0.9)], st);
                let cap = Rect::from_center_size(pos2(x, c.y + h * k * 1.6), vec2(s * 0.2, s * 0.12));
                p.rect_filled(cap, CornerRadius::same(2), col);
            }
        }
    }
}

/// A round badge with an icon in it.
pub fn avatar(ui: &mut Ui, size: f32, ic: Icon, fill: Color32, fg: Color32) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    ui.painter().circle_filled(r.center(), size * 0.5, fill);
    icon(ui.painter(), r.center(), size * 0.56, ic, fg);
    resp
}

/// An on/off switch (a pill with a knob that slides).
pub fn switch(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let t = Tokens::current();
    let (r, mut resp) = ui.allocate_exact_size(vec2(40.0, 22.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, label));
    let k = ui.ctx().animate_bool_responsive(resp.id, *on);
    let off = if resp.hovered() { t.button_hi } else { t.button };
    let fill = lerp(off, t.accent, k);
    ui.painter().rect_filled(r, CornerRadius::same(11), fill);
    let x = egui::lerp(r.min.x + 11.0..=r.max.x - 11.0, k);
    ui.painter().circle_filled(pos2(x, r.center().y), 8.0, Color32::WHITE);
    resp
}

fn lerp(a: Color32, b: Color32, k: f32) -> Color32 {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * k).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// A tile: a rounded panel, outlined in the accent colour when `active`.
pub fn tile<R>(ui: &mut Ui, active: bool, add: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<R> {
    let t = Tokens::current();
    let stroke = if active { Stroke::new(1.5, t.accent) } else { Stroke::new(1.0, t.border) };
    egui::Frame::new()
        .fill(if active { widgets_tint(t.panel_bg2, t.accent) } else { t.panel_bg2 })
        .stroke(stroke)
        .corner_radius(10.0)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
}

fn widgets_tint(base: Color32, accent: Color32) -> Color32 {
    lerp(base, accent, 0.07)
}

/// A tile's head: an icon badge, a title and a caption, with room on the right (`right`).
pub fn tile_head(ui: &mut Ui, ic: Icon, on: bool, title: &str, caption: &str, right: impl FnOnce(&mut Ui)) {
    let t = Tokens::current();
    ui.horizontal(|ui| {
        let (fill, fg) = if on { (lerp(t.panel_bg2, t.accent, 0.28), lerp(t.accent, Color32::WHITE, 0.35)) } else { (t.button, t.text_dim) };
        avatar(ui, 38.0, ic, fill, fg);
        ui.add_space(4.0);
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(title).size(15.0).strong().color(t.text));
            ui.label(egui::RichText::new(caption).size(11.5).color(t.text_dim));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), right);
    });
}

/// A filled button in the accent colour (the main thing to do here).
pub fn primary(ui: &mut Ui, text: &str) -> Response {
    let t = Tokens::current();
    let b = egui::Button::new(egui::RichText::new(text).strong().color(Color32::WHITE)).fill(t.accent).corner_radius(7.0).min_size(vec2(96.0, 30.0));
    // Its own size, also in a justified column.
    if ui.layout().is_horizontal() { ui.add(b) } else { ui.horizontal(|ui| ui.add(b)).inner }
}

/// A quieter button, the same height as [`primary`].
pub fn secondary(ui: &mut Ui, text: &str) -> Response {
    let b = egui::Button::new(text).corner_radius(7.0).min_size(vec2(96.0, 30.0));
    if ui.layout().is_horizontal() { ui.add(b) } else { ui.horizontal(|ui| ui.add(b)).inner }
}

/// A small rounded label.
pub fn pill(ui: &mut Ui, text: &str, fg: Color32, fill: Color32) {
    egui::Frame::new().fill(fill).corner_radius(10.0).inner_margin(egui::Margin::symmetric(9, 3)).show(ui, |ui| {
        ui.label(egui::RichText::new(text).size(11.5).color(fg));
    });
}

/// A small upper-case section label.
pub fn section(ui: &mut Ui, text: &str) {
    let t = Tokens::current();
    ui.add_space(10.0);
    ui.label(egui::RichText::new(text.to_uppercase()).size(11.0).strong().color(t.text_dim).extra_letter_spacing(1.2));
    ui.add_space(4.0);
}

/// Two segments side by side; returns the one clicked.
pub fn segmented(ui: &mut Ui, labels: [&str; 2], selected: usize) -> Option<usize> {
    let t = Tokens::current();
    let font = FontId::proportional(13.0);
    let widths: Vec<f32> =
        labels.iter().map(|l| ui.fonts_mut(|f| f.layout_no_wrap((*l).to_string(), font.clone(), t.text).size().x) + 28.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0;
    let (r, _) = ui.allocate_exact_size(vec2(total, 32.0), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::same(16), t.button);
    let mut x = r.min.x + 3.0;
    let mut clicked = None;
    for (i, (l, w)) in labels.iter().zip(&widths).enumerate() {
        let seg = Rect::from_min_size(pos2(x, r.min.y + 3.0), vec2(*w, 26.0));
        let resp = ui.interact(seg, ui.id().with(("seg", i)), Sense::click());
        let on = i == selected;
        if on {
            ui.painter().rect_filled(seg, CornerRadius::same(13), t.accent);
        } else if resp.hovered() {
            ui.painter().rect_filled(seg, CornerRadius::same(13), t.button_hi);
        }
        ui.painter().text(seg.center(), Align2::CENTER_CENTER, *l, font.clone(), if on { Color32::WHITE } else { t.text });
        if resp.clicked() {
            clicked = Some(i);
        }
        x += w;
    }
    clicked
}

/// A step number in a circle (a check when done).
pub fn step(ui: &mut Ui, n: u8, done: bool, title: &str) {
    let t = Tokens::current();
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
        if done {
            ui.painter().circle_filled(r.center(), 12.0, LAN);
            ui.painter().text(r.center(), Align2::CENTER_CENTER, "✔", FontId::proportional(13.0), Color32::WHITE);
        } else {
            ui.painter().circle_stroke(r.center(), 11.0, Stroke::new(1.5, t.accent));
            ui.painter().text(r.center(), Align2::CENTER_CENTER, n.to_string(), FontId::proportional(13.0), t.accent);
        }
        ui.label(egui::RichText::new(title).size(16.0).strong().color(t.text));
    });
    ui.add_space(4.0);
}

/// How a device on the map is connected.
#[derive(Debug, Clone, PartialEq)]
pub enum Link {
    Lan,
    /// Over the internet: direct or through a relay, and its round trip (ms) when known.
    Internet {
        direct: bool,
        rtt_ms: Option<f64>,
    },
}

/// A device on the map.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub name: String,
    pub role: String,
    pub icon: Icon,
    pub link: Link,
    /// Something is wrong with it (amber).
    pub warn: bool,
}

/// This PC in the middle, everyone connected around it, each line coloured by how it is
/// connected (green: this network, blue: internet, amber dashes: through a relay).
pub fn map(ui: &mut Ui, me: &str, nodes: &[Node], empty: &str) {
    let t = Tokens::current();
    let h = match nodes.len() {
        0 => 140.0,
        1 | 2 => 170.0,
        _ => 230.0,
    };
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter_at(r);
    p.rect_filled(r, CornerRadius::same(10), t.panel_bg2);
    // A faint dot grid.
    let mut y = r.min.y + 12.0;
    while y < r.max.y {
        let mut x = r.min.x + 12.0;
        while x < r.max.x {
            p.circle_filled(pos2(x, y), 0.8, t.border);
            x += 18.0;
        }
        y += 18.0;
    }
    let c = if nodes.is_empty() { pos2(r.center().x, r.min.y + 58.0) } else { r.center() - vec2(0.0, 10.0) };
    let n = nodes.len();
    let (rx, ry) = ((r.width() * 0.36).min(240.0), if n <= 2 { 40.0 } else { 72.0 });
    let at = |i: usize| {
        // The first to the right, the next to the left, then around.
        let a = i as f32 / n.max(1) as f32 * std::f32::consts::TAU;
        pos2(c.x + rx * a.cos(), c.y + ry * a.sin())
    };
    for (i, node) in nodes.iter().enumerate() {
        let q = at(i);
        let (col, dashed, text) = match &node.link {
            Link::Lan => (LAN, false, "this network".to_string()),
            Link::Internet { direct, rtt_ms } => {
                let ms = rtt_ms.map(|m| format!(" · {m:.0} ms")).unwrap_or_default();
                if *direct { (NET, false, format!("internet{ms}")) } else { (RELAY, true, format!("relay{ms}")) }
            }
        };
        let st = Stroke::new(2.0, col.gamma_multiply(0.8));
        if dashed {
            p.extend(egui::Shape::dashed_line(&[c, q], st, 6.0, 4.0));
        } else {
            p.line_segment([c, q], st);
        }
        let mid = c + (q - c) * 0.5;
        let galley = p.layout_no_wrap(text, FontId::proportional(10.5), col);
        let lr = Rect::from_center_size(mid, galley.size() + vec2(10.0, 4.0));
        p.rect_filled(lr, CornerRadius::same(8), t.panel_bg2);
        p.galley(lr.center() - galley.size() * 0.5, galley, col);
    }
    for (i, node) in nodes.iter().enumerate() {
        let q = at(i);
        let ring = if node.warn { RELAY } else { t.border_light };
        p.circle_filled(q, 21.0, t.window_bg);
        p.circle_stroke(q, 21.0, Stroke::new(1.5, ring));
        icon(&p, q, 20.0, node.icon, t.text);
        p.text(q + vec2(0.0, 27.0), Align2::CENTER_TOP, &node.name, FontId::proportional(12.5), t.text);
        p.text(q + vec2(0.0, 43.0), Align2::CENTER_TOP, &node.role, FontId::proportional(10.5), if node.warn { RELAY } else { t.text_dim });
    }
    // This PC.
    p.circle_filled(c, 30.0, t.accent.gamma_multiply(0.18));
    p.circle_filled(c, 25.0, t.accent);
    icon(&p, c, 24.0, Icon::Pc, Color32::WHITE);
    p.text(c + vec2(0.0, 32.0), Align2::CENTER_TOP, me, FontId::proportional(13.0), t.text);
    if nodes.is_empty() {
        p.text(c + vec2(0.0, 52.0), Align2::CENTER_TOP, empty, FontId::proportional(11.5), t.text_dim);
    }
}
