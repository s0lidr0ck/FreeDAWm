//! Small custom widgets in the studio style.

use craft_ui::buttons::{ButtonVisuals, IconButton, IconButtonStyle};

use crate::icons;
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Color32, CornerRadius, Mesh, Painter, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

/// `c` moved towards white (`k > 0`) or black (`k < 0`).
pub fn shade(c: Color32, k: f32) -> Color32 {
    if k >= 0.0 { c.lerp_to_gamma(Color32::WHITE, k) } else { c.lerp_to_gamma(Color32::BLACK, -k) }
}

fn quad(mesh: &mut Mesh, r: Rect, [tl, tr, bl, br]: [Color32; 4]) {
    let i = mesh.vertices.len() as u32;
    for (p, c) in [(r.left_top(), tl), (r.right_top(), tr), (r.left_bottom(), bl), (r.right_bottom(), br)] {
        mesh.colored_vertex(p, c);
    }
    mesh.add_triangle(i, i + 1, i + 2);
    mesh.add_triangle(i + 1, i + 2, i + 3);
}

/// Fills `r` with a vertical gradient from `top` to `bottom`.
pub fn vgradient(p: &Painter, r: Rect, top: Color32, bottom: Color32) {
    let mut mesh = Mesh::default();
    quad(&mut mesh, r, [top, top, bottom, bottom]);
    p.add(mesh);
}

/// A rounded rectangle shaded from `top` to `bottom`, with egui's anti-aliased edges.
pub fn rounded_vgradient(p: &Painter, r: Rect, radius: f32, top: Color32, bottom: Color32) {
    let options = p.ctx().tessellation_options(|o| *o);
    let mut tessellator = egui::epaint::Tessellator::new(p.pixels_per_point(), options, [1, 1], Vec::new());
    let mut mesh = Mesh::default();
    tessellator.tessellate_rect(&egui::epaint::RectShape::filled(r, radius, Color32::WHITE), &mut mesh);
    for v in &mut mesh.vertices {
        let k = ((v.pos.y - r.min.y) / r.height()).clamp(0.0, 1.0);
        v.color = top.lerp_to_gamma(bottom, k).gamma_multiply(v.color.a() as f32 / 255.0);
    }
    p.add(mesh);
}

/// A raised key as Pro Tools draws them: a dark rim, a face shaded from `faces[0]` to `faces[1]`
/// and a lit top edge. Returns the face's top colour.
pub fn raised_key(p: &Painter, r: Rect, faces: [Color32; 2], hovered: bool) -> Color32 {
    let [top, bottom] = if hovered { faces.map(|c| shade(c, 0.06)) } else { faces };
    p.rect_filled(r, 3.0, shade(Tokens::current().slot_well, -0.3));
    rounded_vgradient(p, r.shrink(1.0), 2.0, top, bottom);
    p.hline(r.min.x + 2.0..=r.max.x - 2.0, r.min.y + 1.5, Stroke::new(1.0, shade(top, 0.17)));
    top
}

/// A groove across a raised key whose face starts at `top`: a dark line, then a light one.
pub fn groove(p: &Painter, x: f32, ys: impl Into<egui::Rangef>, top: Color32) {
    let ys = ys.into();
    p.vline(x, ys, Stroke::new(1.0, shade(top, -0.42)));
    p.vline(x + 1.0, ys, Stroke::new(1.0, shade(top, 0.12)));
}

pub fn inset(p: &Painter, r: Rect, fill: Color32) {
    let t = Tokens::current();
    p.rect(r, 2.0, fill, Stroke::new(1.0, t.border), StrokeKind::Inside);
    p.hline(r.min.x + 1.0..=r.max.x - 1.0, r.min.y + 1.5, Stroke::new(1.0, Color32::from_black_alpha(60)));
    p.hline(r.min.x + 2.0..=r.max.x - 2.0, r.max.y + 0.5, Stroke::new(1.0, t.edge_light));
}

pub fn key_fill(resp: &Response) -> Color32 {
    let t = Tokens::current();
    if resp.hovered() { t.button_hi } else { t.button }
}

/// A key in `fill`, softly lit from above; it sinks while pressed.
pub fn key(p: &Painter, r: Rect, resp: &Response, fill: Color32) {
    let face = r.shrink(1.0);
    if resp.is_pointer_button_down_on() {
        vgradient(p, face, shade(fill, -0.14), shade(fill, 0.02));
    } else {
        p.rect_filled(face, 0.0, fill);
        key_light(p, face);
    }
    p.rect_stroke(r, 3.0, Stroke::new(1.0, Tokens::current().button_border), StrokeKind::Inside);
}

/// Light from above over a key face already filled with its colour.
fn key_light(p: &Painter, face: Rect) {
    vgradient(p, face, Color32::from_white_alpha(26), Color32::from_black_alpha(31));
    p.hline(face.min.x + 1.0..=face.max.x - 1.0, face.min.y + 0.5, Stroke::new(1.0, Color32::from_white_alpha(30)));
}

/// Local colors for shared icon-button interaction; transport-specific states stay in the toolbar.
fn button_style(selected_fill: Color32, normal_icon: Color32, radius: u8) -> IconButtonStyle {
    let t = Tokens::current();
    let visuals = |fill, icon| ButtonVisuals { fill, stroke: Stroke::new(1.0, t.button_border), icon };
    IconButtonStyle {
        normal: visuals(t.button, normal_icon),
        hovered: visuals(t.button_hi, normal_icon),
        pressed: Some(visuals(shade(t.button, -0.1), normal_icon)),
        selected: visuals(selected_fill, Color32::WHITE),
        corner_radius: CornerRadius::same(radius),
        focus_stroke: Stroke::new(1.0, t.accent),
    }
}

/// A toolbar button with an icon; `on` draws the blue selected state.
pub fn icon_button(ui: &mut Ui, size: egui::Vec2, icon: &str, on: bool, tip: &str) -> Response {
    let t = Tokens::current();
    let style = button_style(t.accent, t.text, 3);
    let resp = IconButton::new(tip, size, &style).selected(on).single_shape_frame().show(ui, |ui, r, tint| {
        key_light(ui.painter(), r.shrink(1.0));
        let s = r.height().min(r.width()) * 0.78;
        icons::draw(ui.painter(), Rect::from_center_size(r.center(), vec2(s, s)), icon, tint);
    });
    resp.on_hover_text(tip)
}

/// Square text toggle used for S / M / I / record etc.
pub fn text_toggle(ui: &mut Ui, size: egui::Vec2, text: &str, on: bool, on_color: Color32, tip: &str) -> Response {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    key(ui.painter(), r, &resp, if on { on_color } else { key_fill(&resp) });
    let col = if on { t.text_dark } else { t.text };
    ui.painter().text(r.center(), Align2::CENTER_CENTER, text, bold((size.y * 0.62).clamp(8.0, 13.0)), col);
    resp.on_hover_text(tip)
}

/// The colour frozen tracks are marked in.
pub fn ice() -> Color32 {
    Tokens::current().text.lerp_to_gamma(Color32::from_rgb(96, 178, 236), 0.75)
}

/// A six-armed snowflake of `radius` at `c` (the Freeze mark).
pub fn snowflake(p: &Painter, c: egui::Pos2, radius: f32, color: Color32) {
    let stroke = Stroke::new((radius * 0.28).clamp(1.0, 1.6), color);
    for a in [0.0f32, 60.0, 120.0] {
        let (s, k) = a.to_radians().sin_cos();
        let d = vec2(s, k) * radius;
        p.line_segment([c - d, c + d], stroke);
        // Barbs near each end.
        for end in [c + d * 0.62, c - d * 0.62] {
            let n = vec2(-k, s) * radius * 0.28;
            let back = (end - c).normalized() * radius * 0.25;
            p.line_segment([end, end + back + n], stroke);
            p.line_segment([end, end + back - n], stroke);
        }
    }
}

/// How a Freeze key reads: live, frozen down to a freeze point (part of the chain stays live), or
/// frozen whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezeLook {
    Live,
    Partly,
    Whole,
}

/// The Mix strip's snowflake key (Freeze): lit ice-blue when the whole chain is frozen, its top half
/// lit when frozen down to a freeze point. `label` is its accessible name; selected while frozen.
pub fn freeze_key(ui: &mut Ui, size: Vec2, look: FreezeLook, label: &str) -> Response {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, look != FreezeLook::Live, label));
    let p = ui.painter();
    key(p, r, &resp, if look == FreezeLook::Whole { ice() } else { key_fill(&resp) });
    if look == FreezeLook::Partly {
        let face = r.shrink(1.0);
        p.rect_filled(Rect::from_min_max(face.min, pos2(face.max.x, face.center().y)), 0.0, ice().gamma_multiply(0.7));
        p.rect_stroke(r, 3.0, Stroke::new(1.0, ice()), StrokeKind::Inside);
    }
    let ink = match look {
        _ if !enabled => t.text_dim,
        FreezeLook::Whole => t.text_dark,
        _ => t.text,
    };
    snowflake(p, r.center(), (size.y * 0.3).clamp(3.5, 6.0), ink);
    resp
}

/// The "Frozen" badge over a frozen track's module chain: a snowflake and the word, on a dim
/// plate, with hover text saying how to edit. `label` is its accessible name.
pub fn frozen_badge(ui: &Ui, r: Rect, label: &str) -> Response {
    let t = Tokens::current();
    let resp = ui.interact(r, ui.id().with(("frozen-badge", label)), Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, label));
    let p = ui.painter();
    p.rect_filled(r, 3.0, t.strip_bg.gamma_multiply(0.92));
    p.rect_stroke(r, 3.0, Stroke::new(1.0, ice()), StrokeKind::Inside);
    let c = pos2(r.min.x + 10.0, r.center().y);
    snowflake(p, c, 5.0, ice());
    p.text(pos2(c.x + 9.0, c.y), Align2::LEFT_CENTER, "Frozen", bold(10.0), ice());
    resp.on_hover_text("Frozen: the track plays its frozen audio and its frozen modules are off. Unfreeze the track to edit them.")
}

/// What a solo means for one track, for its S and M keys (see [`solo_state`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SoloLook {
    /// No solo touches it.
    Off,
    /// Soloed itself: the lit S.
    Soloed,
    /// Playing because it is on the named soloed track's signal path: the S outlined and
    /// half-lit in the solo colour.
    Implied(String),
    /// Muted because other tracks are soloed: the M in a dark shade of the mute colour.
    MutedBySolo,
}

impl SoloLook {
    /// The S key's tooltip.
    pub fn solo_tip(&self) -> String {
        match self {
            SoloLook::Implied(cause) => format!("Playing because {cause} is soloed"),
            _ => "Solo".into(),
        }
    }

    /// The M key's tooltip.
    pub fn mute_tip(&self) -> &'static str {
        if *self == SoloLook::MutedBySolo { "Mute (muted by another track's solo)" } else { "Mute" }
    }

    /// Accessible names of the S and M keys of track `name`.
    pub fn labels(&self, name: &str) -> (String, String) {
        let solo = match self {
            SoloLook::Implied(cause) => format!("{name} solo, implied by {cause}"),
            _ => format!("{name} solo"),
        };
        let mute = if *self == SoloLook::MutedBySolo { format!("{name} mute, muted by solo") } else { format!("{name} mute") };
        (solo, mute)
    }
}

/// How the solos of `s` (whose [`SoloScope`](soundcraft_model::SoloScope) is `scope`) leave track
/// `id`.
pub fn solo_state(s: &soundcraft_model::Session, scope: &soundcraft_model::SoloScope, id: soundcraft_model::TrackId) -> SoloLook {
    let Some(i) = s.track_index(id) else { return SoloLook::Off };
    if scope.soloed(i) {
        SoloLook::Soloed
    } else if scope.implied(i) {
        let cause = scope.cause(i).and_then(|c| s.tracks.get(c)).map_or_else(String::new, |t| t.name.clone());
        SoloLook::Implied(cause)
    } else if scope.muted(i) {
        SoloLook::MutedBySolo
    } else {
        SoloLook::Off
    }
}

/// Paints the implied-solo face over a key in `r`: outlined, its lower half lit in `solo`, the
/// letter in the solo colour (shared by the Mix strip and the Edit header).
pub fn paint_implied_solo(p: &Painter, r: Rect, solo: Color32, font_size: f32) {
    let t = Tokens::current();
    let face = r.shrink(1.5);
    // A low band, so it never reads as the fully lit key of the soloed track itself.
    let band = Rect::from_min_max(pos2(face.min.x, face.max.y - face.height() * 0.32), face.max);
    p.rect_filled(band, CornerRadius { nw: 0, ne: 0, sw: 2, se: 2 }, solo.gamma_multiply(0.55));
    p.rect_stroke(r.shrink(0.5), 3.0, Stroke::new(1.5, solo), StrokeKind::Inside);
    p.text(r.center(), Align2::CENTER_CENTER, "S", bold(font_size), t.text.lerp_to_gamma(solo, 0.6));
}

/// The Mix strip's S key: lit when soloed, outlined and half-lit when implied.
pub fn solo_key(ui: &mut Ui, size: Vec2, look: &SoloLook, label: &str) -> Response {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    let on = *look == SoloLook::Soloed;
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), on, label));
    let font = (size.y * 0.62).clamp(8.0, 13.0);
    key(ui.painter(), r, &resp, if on { t.solo } else { key_fill(&resp) });
    if matches!(look, SoloLook::Implied(_)) {
        paint_implied_solo(ui.painter(), r, t.solo, font);
    } else {
        ui.painter().text(r.center(), Align2::CENTER_CENTER, "S", bold(font), if on { t.text_dark } else { t.text });
    }
    resp.on_hover_text(look.solo_tip())
}

/// Marks a solo-safe track's S key (Solo Safe: its solo never mutes it out): a small padlock in
/// the key's top-right corner, so it reads at a glance in both windows.
pub fn paint_solo_safe(p: &Painter, r: Rect) {
    let t = Tokens::current();
    let c = pos2(r.max.x - 5.5, r.min.y + 6.0);
    let body = Rect::from_center_size(c + vec2(0.0, 1.5), vec2(6.0, 4.5));
    p.circle_stroke(c + vec2(0.0, -0.75), 2.0, Stroke::new(1.0, t.solo));
    p.rect_filled(body, 1.0, t.solo);
    p.rect_stroke(body, 1.0, Stroke::new(0.75, t.text_dark), StrokeKind::Outside);
}

/// The Mix strip's M key: lit when muted, a dark shade of the mute colour when another track's
/// solo mutes it (the Edit header's implied mute).
pub fn mute_key(ui: &mut Ui, size: Vec2, muted: bool, look: &SoloLook, label: &str) -> Response {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), muted, label));
    let implied = !muted && *look == SoloLook::MutedBySolo;
    let fill = if muted {
        t.mute
    } else if implied {
        key_fill(&resp).lerp_to_gamma(shade(t.mute, -0.35), 0.55)
    } else {
        key_fill(&resp)
    };
    key(ui.painter(), r, &resp, fill);
    let ink = if muted {
        t.text_dark
    } else if implied {
        t.text.lerp_to_gamma(t.mute, 0.3)
    } else {
        t.text
    };
    ui.painter().text(r.center(), Align2::CENTER_CENTER, "M", bold((size.y * 0.62).clamp(8.0, 13.0)), ink);
    resp.on_hover_text(look.mute_tip())
}

/// The three-state input-monitoring key ("I"). Auto is unlit; On is lit in the input colour; Off
/// (direct monitoring) is outlined in the record colour with its letter struck through, so it
/// never reads as Auto. `tip` (the state's name) is also its accessible label.
pub fn input_monitor_key(ui: &mut Ui, size: Vec2, state: soundcraft_model::InputMonitor, tip: &str) -> Response {
    use soundcraft_model::InputMonitor;
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), state == InputMonitor::On, tip));
    paint_input_monitor(ui.painter(), r, &resp, state, size.y);
    resp.on_hover_text(tip)
}

/// Paints the I key in `r` for `state` (shared by the Mix strip and the Edit header).
pub fn paint_input_monitor(p: &Painter, r: Rect, resp: &Response, state: soundcraft_model::InputMonitor, height: f32) {
    use soundcraft_model::InputMonitor;
    let t = Tokens::current();
    let on = state == InputMonitor::On;
    key(p, r, resp, if on { t.input } else { key_fill(resp) });
    let font = bold((height * 0.62).clamp(8.0, 13.0));
    let c = r.center();
    match state {
        InputMonitor::On => {
            p.text(c, Align2::CENTER_CENTER, "I", font, t.text_dark);
        }
        InputMonitor::Auto => {
            p.text(c, Align2::CENTER_CENTER, "I", font, t.text);
        }
        InputMonitor::Off => {
            p.rect_stroke(r.shrink(0.5), 3.0, Stroke::new(1.5, t.rec), StrokeKind::Inside);
            let ink = t.text.lerp_to_gamma(t.rec, 0.55);
            p.text(c, Align2::CENTER_CENTER, "I", font, ink);
            let d = vec2(height * 0.22, height * 0.22);
            p.line_segment([c - d, c + d], Stroke::new(1.5, t.rec));
        }
    }
}

/// Record-enable button (red dot); the caller owns track arming and command dispatch.
pub fn rec_toggle(ui: &mut Ui, size: egui::Vec2, on: bool, tip: &str) -> Response {
    let t = Tokens::current();
    let style = button_style(t.rec, Color32::from_rgb(218, 112, 106), 2);
    let resp = IconButton::new(tip, size, &style).selected(on).single_shape_frame().show(ui, |ui, r, tint| {
        key_light(ui.painter(), r.shrink(1.0));
        let rad = size.y * 0.24;
        if on {
            ui.painter().circle_filled(r.center(), rad, tint);
        } else {
            rounded_vgradient(
                ui.painter(),
                Rect::from_center_size(r.center(), Vec2::splat(2.0 * rad)),
                2.0 * rad,
                tint,
                Color32::from_rgb(176, 76, 72),
            );
        }
        ui.painter().circle_stroke(r.center(), rad, Stroke::new(1.0, Color32::from_black_alpha(90)));
    });
    resp.on_hover_text(tip)
}

/// A dropdown-looking selector box. Returns the response (caller opens a menu on click).
pub fn selector_box(ui: &mut Ui, width: f32, height: f32, text: &str, color: Color32) -> Response {
    selector_box_with_tooltip(ui, width, height, text, color, text)
}

/// A selector with a compact visible label and the complete meaning on hover.
pub(crate) fn selector_box_with_tooltip(ui: &mut Ui, width: f32, height: f32, text: &str, color: Color32, tooltip: &str) -> Response {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    key(ui.painter(), r, &resp, key_fill(&resp));
    let tr = Rect::from_min_max(pos2(r.max.x - height * 0.8, r.min.y), r.max);
    icons::draw(ui.painter(), tr.shrink(height * 0.15), "triangle_down", t.text_dim);
    let text_rect = Rect::from_min_max(r.min + vec2(3.0, 1.0), pos2(tr.left() - 2.0, r.bottom() - 1.0));
    let mut job = egui::text::LayoutJob::simple(text.to_owned(), regular((height * 0.62).clamp(8.0, 12.0)), color, text_rect.width().max(1.0));
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let galley = ui.painter().layout_job(job);
    let pos = text_rect.center() - galley.size() * 0.5;
    ui.painter().with_clip_rect(r.shrink(1.0)).galley(pos, galley, color);
    resp.on_hover_text(tooltip)
}

/// Rotary knob for pan / send pan (-1..1). Drag vertically; double-click resets.
pub fn pan_knob(ui: &mut Ui, size: f32, value: &mut f32, tip: &str) -> Response {
    let t = Tokens::current();
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
    if resp.dragged() {
        let d = resp.drag_delta();
        *value = (*value + (d.x - d.y) * 0.01).clamp(-1.0, 1.0);
    }
    if resp.double_clicked() {
        *value = 0.0;
    }
    let p = ui.painter();
    let c = r.center();
    let rad = size * 0.42;
    let n = 24;
    let start = std::f32::consts::PI * 0.75;
    let sweep = std::f32::consts::PI * 1.5;
    let arc = |a0: f32, a1: f32| -> Vec<egui::Pos2> {
        (0..=n)
            .map(|i| {
                let a = a0 + (a1 - a0) * i as f32 / n as f32;
                c + vec2(a.cos(), a.sin()) * (rad + 2.5)
            })
            .collect()
    };
    let mid = start + sweep * 0.5;
    let at = start + sweep * (*value * 0.5 + 0.5);
    let (a0, a1) = if at < mid { (at, mid) } else { (mid, at) };
    p.add(egui::Shape::line(arc(start, start + sweep), Stroke::new(2.0, t.meter_bg)));
    p.add(egui::Shape::line(arc(a0, a1), Stroke::new(2.0, t.counter_text)));
    p.circle_filled(c + vec2(0.0, 1.0), rad, Color32::from_black_alpha(90));
    rounded_vgradient(p, Rect::from_center_size(c, Vec2::splat(2.0 * rad)), 2.0 * rad, shade(t.knob_bg, 0.14), shade(t.knob_bg, -0.2));
    p.circle_stroke(c, rad - 0.75, Stroke::new(1.5, t.knob_ring));
    let d = vec2(at.cos(), at.sin());
    p.line_segment([c + d * rad * 0.15, c + d * (rad - 2.5)], Stroke::new(2.0, t.knob_pointer));
    resp.on_hover_text(tip)
}

/// Format a pan value like a console: "<45", "0", "45>".
pub fn pan_text(v: f32) -> String {
    let n = (v * 100.0).round() as i32;
    match n {
        0 => "0".into(),
        n if n < 0 => format!("<{}", -n),
        n => format!("{n}>"),
    }
}

/// The dB marks of a meter's scale, also ruled across its bar.
const METER_MARKS: [f32; 8] = [0.0, -5.0, -10.0, -15.0, -20.0, -30.0, -40.0, -60.0];

/// The bar of a [`meter`] drawn in `r`, below its clip light.
fn meter_bar(r: Rect) -> Rect {
    Rect::from_min_max(pos2(r.min.x + 1.0, r.min.y + 5.0), pos2(r.max.x - 1.0, r.max.y - 1.0))
}

/// The marks that fit on `bar`, at least 9 pt apart from the top down, with their heights.
fn meter_marks(bar: Rect) -> impl Iterator<Item = (f32, f32)> {
    let mut last = f32::NEG_INFINITY;
    METER_MARKS.into_iter().filter_map(move |db| {
        let y = bar.max.y - bar.height() * meter_pos(db);
        (y - last >= 9.0).then(|| {
            last = y;
            (db, y)
        })
    })
}

/// Vertical peak meter as Pro Tools draws it: a clip light above a bar that lights green, yellow
/// and red over dark tints of those zones, ruled at the scale's marks. Values are linear peaks.
pub fn meter(ui: &Ui, r: Rect, level: f32, hold: f32, clip: bool) {
    let t = Tokens::current();
    let p = ui.painter();
    p.rect_filled(Rect::from_min_max(r.min, pos2(r.max.x, r.min.y + 3.0)), 1.0, if clip { t.meter_red } else { t.meter_clip_off });
    p.rect_filled(Rect::from_min_max(pos2(r.min.x, r.min.y + 4.0), r.max), 0.0, t.meter_bg);
    let bar = meter_bar(r);
    let y = |pos: f32| bar.max.y - bar.height() * pos;
    let (amber, red) = (meter_pos(-12.0), meter_pos(-3.0));
    let zones = [(0.0, amber, t.meter_green), (amber, red, t.meter_yellow), (red, 1.0, t.meter_red)];
    let lit = level_pos(level);
    let unlit = if t.meter_bg.intensity() > 0.5 { 0.08 } else { 0.18 };
    let mut mesh = Mesh::default();
    for (lo, hi, color) in zones {
        let top = lit.clamp(lo, hi);
        quad(&mut mesh, Rect::from_x_y_ranges(bar.x_range(), y(top)..=y(lo)), [color; 4]);
        quad(&mut mesh, Rect::from_x_y_ranges(bar.x_range(), y(hi)..=y(top)), [color.gamma_multiply(unlit); 4]);
    }
    p.add(mesh);
    for (_, at) in meter_marks(bar).skip(1) {
        p.hline(bar.x_range(), p.round_to_pixel_center(at), Stroke::new(1.0, Color32::from_black_alpha(90)));
    }
    let held = level_pos(hold);
    if held > 0.01 {
        let color = zones.iter().find(|z| held <= z.1).map_or(t.meter_red, |z| z.2);
        p.rect_filled(Rect::from_x_y_ranges(bar.x_range(), y(held) - 1.0..=y(held) + 1.0), 0.0, color);
    }
}

/// The dB scale of a [`meter`] drawn in `r`, its numbers ending at `x`.
pub fn meter_scale(p: &Painter, r: Rect, x: f32) {
    let t = Tokens::current();
    for (db, y) in meter_marks(meter_bar(r)) {
        p.text(pos2(x - 3.0, y), Align2::RIGHT_CENTER, format!("{}", db.abs()), regular(8.0), t.text_dim);
        p.hline(x - 2.0..=x, y, Stroke::new(1.0, t.text_dim.gamma_multiply(0.7)));
    }
}

/// A linear peak → 0..1 meter position.
pub fn level_pos(v: f32) -> f32 {
    meter_pos(if v <= 1e-6 { -80.0 } else { 20.0 * v.log10() })
}

/// dB → 0..1 meter position (logarithmic-ish scale like hardware meters).
pub fn meter_pos(db: f32) -> f32 {
    if !db.is_finite() || db <= -80.0 {
        return 0.0;
    }
    if db >= 0.0 {
        return 1.0;
    }
    // -80..-40: bottom 10 %, -40..-20: 20 %, -20..0: 70 %.
    if db < -40.0 {
        (db + 80.0) / 40.0 * 0.1
    } else if db < -20.0 {
        0.1 + (db + 40.0) / 20.0 * 0.2
    } else {
        0.3 + (db + 20.0) / 20.0 * 0.7
    }
}

/// Clip gain fader taper: the top three quarters span -36..+36 dB evenly, the rest compresses to -144.
pub fn clip_gain_from_pos(pos: f32) -> f32 {
    let p = if pos.is_finite() { pos.clamp(0.0, 1.0) } else { 0.0 };
    if p >= 0.25 { -36.0 + (p - 0.25) / 0.75 * 72.0 } else { -36.0 - (1.0 - p / 0.25).powf(1.5) * 108.0 }
}

pub fn clip_gain_to_pos(db: f32) -> f32 {
    let db = if db.is_finite() { db.clamp(-144.0, 36.0) } else { -144.0 };
    if db >= -36.0 { 0.25 + (db + 36.0) / 72.0 * 0.75 } else { (1.0 - ((-36.0 - db) / 108.0).powf(1.0 / 1.5)) * 0.25 }
}

const FADER_CAP_H: f32 = 40.0;

/// The travel of a [`fader`] drawn in `r`: its cap's centre runs from the bottom (-inf) to the
/// top (+12 dB).
pub fn fader_slot(r: Rect) -> Rect {
    Rect::from_center_size(r.center(), vec2(4.0, r.height() - FADER_CAP_H))
}

/// A fader: returns the new dB value when dragged. `db` in -144..12.
pub fn fader(ui: &mut Ui, r: Rect, db: f32, id: egui::Id) -> Option<f32> {
    let t = Tokens::current();
    let resp = ui.interact(r, id, Sense::click_and_drag());
    let p = ui.painter();
    let slot = fader_slot(r);
    let cap_w = (r.width() * 0.4).clamp(16.0, 18.0);
    p.rect_filled(slot, 2.0, t.fader_track);
    p.vline(slot.min.x + 0.5, slot.min.y + 1.0..=slot.max.y - 1.0, Stroke::new(1.0, Color32::from_black_alpha(120)));
    p.vline(slot.max.x + 0.5, slot.min.y + 2.0..=slot.max.y - 2.0, Stroke::new(1.0, Color32::from_white_alpha(18)));
    let numbers_x = slot.center().x - cap_w * 0.5 - 1.0;
    let mut last = f32::NEG_INFINITY;
    for mark in [12.0f32, 6.0, 0.0, -5.0, -10.0, -15.0, -20.0, -30.0, -40.0, -60.0] {
        let y = slot.max.y - slot.height() * soundcraft_model::fader_db_to_pos(mark);
        p.hline(slot.min.x - 6.0..=slot.min.x - 2.0, y, Stroke::new(1.0, t.text_dim.gamma_multiply(0.7)));
        if numbers_x - r.min.x >= 10.0 && y - last >= 9.0 {
            p.text(pos2(numbers_x, y), Align2::RIGHT_CENTER, format!("{}", mark.abs()), regular(8.0), t.text_dim);
            last = y;
        }
    }
    let pos = soundcraft_model::fader_db_to_pos(db);
    let y = slot.max.y - slot.height() * pos;
    let cap = Rect::from_center_size(pos2(slot.center().x, y), vec2(cap_w, FADER_CAP_H));
    p.rect_filled(cap.translate(vec2(0.0, 2.0)).expand(1.0), 4.0, Color32::from_black_alpha(80));
    let g = Color32::from_gray;
    p.rect_filled(cap, 3.0, g(34));
    let face = cap.shrink(1.0);
    rounded_vgradient(p, face, 2.0, g(236), g(104));
    vgradient(p, Rect::from_x_y_ranges(face.x_range(), y + 1.0..=face.max.y - 2.0), g(214), g(150));
    p.hline(face.min.x + 2.0..=face.max.x - 2.0, face.min.y + 1.5, Stroke::new(1.0, Color32::from_white_alpha(110)));
    p.hline(face.x_range(), y, Stroke::new(2.0, g(30)));
    for (x, dx) in [(cap.min.x, 2.5), (cap.max.x, -2.5)] {
        p.add(egui::Shape::convex_polygon(vec![pos2(x, y - 3.0), pos2(x + dx, y), pos2(x, y + 3.0)], g(34), Stroke::NONE));
    }
    if resp.double_clicked() || (resp.clicked() && ui.input(|i| i.modifiers.alt)) {
        return Some(0.0);
    }
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
        let dy = resp.drag_delta().y / slot.height() * if fine { 0.1 } else { 1.0 };
        let np = (pos - dy).clamp(0.0, 1.0);
        return Some(soundcraft_model::fader_pos_to_db(np).clamp(-144.0, 12.0));
    }
    None
}

pub fn db_text(db: f32) -> String {
    if db <= -143.9 { "-inf".into() } else { format!("{db:.1}") }
}

/// A speaker dot on the surround panner: label, square position (-1..1, y up = front), height layer.
pub struct PannerSpeaker {
    pub label: &'static str,
    pub x: f32,
    pub y: f32,
    pub height: bool,
}

/// What the user did on a [`surround_panner`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PannerEdit {
    /// The puck moved (x, y).
    Position(f32, f32),
    /// Divergence changed (Alt-drag on the square, or the slider).
    Divergence(f32),
    /// Elevation changed (height formats).
    Height(f32),
}

/// Pro Tools-style X/Y surround panner: a square room with speaker dots and a draggable puck.
/// Drag moves the puck; double-click returns it to front centre; Alt-drag (or the slider below)
/// sets divergence; height layouts get a second slider for elevation. Returns the edit, if any.
pub fn surround_panner(ui: &mut Ui, size: f32, speakers: &[PannerSpeaker], puck: (f32, f32), divergence: f32, z: Option<f32>) -> Option<PannerEdit> {
    let t = Tokens::current();
    let mut edit = None;
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
    let half = size * 0.5 - 6.0;
    let c = r.center();
    let to_screen = |x: f32, y: f32| pos2(c.x + x * half, c.y - y * half);
    let painter = ui.painter_at(r.expand(1.0));
    painter.rect(r, CornerRadius::same(2), Color32::from_rgb(14, 18, 22), Stroke::new(1.0, Color32::from_rgb(70, 74, 80)), StrokeKind::Inside);
    let grid = Stroke::new(1.0, Color32::from_rgb(38, 44, 50));
    painter.line_segment([to_screen(-1.0, 0.0), to_screen(1.0, 0.0)], grid);
    painter.line_segment([to_screen(0.0, -1.0), to_screen(0.0, 1.0)], grid);
    painter.rect_stroke(Rect::from_two_pos(to_screen(-1.0, -1.0), to_screen(1.0, 1.0)), 0.0, grid, StrokeKind::Middle);
    // "Front" marker.
    painter.line_segment([pos2(c.x - 5.0, r.min.y + 2.0), pos2(c.x + 5.0, r.min.y + 2.0)], Stroke::new(2.0, Color32::from_rgb(90, 110, 130)));
    for s in speakers {
        let (x, y) = if s.height { (s.x * 0.55, s.y * 0.55) } else { (s.x, s.y) };
        let p = to_screen(x, y);
        if s.height {
            painter.circle_stroke(p, 2.5, Stroke::new(1.0, Color32::from_rgb(150, 150, 170)));
        } else {
            painter.circle_filled(p, 3.0, Color32::from_rgb(170, 170, 176));
        }
        if size >= 70.0 && !s.height {
            let inward = vec2(-(x.signum() * if x.abs() > 0.5 { 1.0 } else { 0.0 }), if y.abs() > 0.5 { y.signum() } else { 0.0 });
            let lp = p + vec2(inward.x * 9.0, inward.y * 7.0);
            painter.text(lp, Align2::CENTER_CENTER, s.label, regular(7.5), t.text_dim);
        }
    }
    let alt = ui.input(|i| i.modifiers.alt);
    if resp.double_clicked() {
        edit = Some(PannerEdit::Position(0.0, 1.0));
    } else if resp.dragged() && alt {
        let d = (divergence - resp.drag_delta().y * 0.01).clamp(0.0, 1.0);
        edit = Some(PannerEdit::Divergence(d));
    } else if (resp.dragged() || resp.clicked())
        && !alt
        && let Some(p) = resp.interact_pointer_pos()
    {
        let x = ((p.x - c.x) / half).clamp(-1.0, 1.0);
        let y = ((c.y - p.y) / half).clamp(-1.0, 1.0);
        edit = Some(PannerEdit::Position(x, y));
    }
    let (px, py) = match edit {
        Some(PannerEdit::Position(x, y)) => (x, y),
        _ => puck,
    };
    let pp = to_screen(px, py);
    let div = match edit {
        Some(PannerEdit::Divergence(d)) => d,
        _ => divergence,
    };
    if div > 0.01 {
        painter.circle(pp, 5.0 + div * half, Color32::from_rgba_unmultiplied(80, 160, 230, 28), Stroke::new(1.0, Color32::from_rgb(70, 130, 190)));
    }
    painter.circle(pp, 5.0, t.counter_text, Stroke::new(1.0, Color32::BLACK));
    let tip = "Surround pan: drag the puck · double-click: front centre · Alt-drag: divergence";
    let _ = resp.on_hover_text(tip);
    // Sliders below: divergence, then elevation for height formats.
    let mut sliders: Vec<(&str, f32, u8)> = vec![("div", div, 0)];
    if let Some(z) = z {
        sliders.push(("z", z, 1));
    }
    for (label, v, kind) in sliders {
        let (sr, sresp) = ui.allocate_exact_size(vec2(size, 11.0), Sense::click_and_drag());
        let track = Rect::from_min_max(pos2(sr.min.x + 16.0, sr.center().y - 2.0), pos2(sr.max.x - 2.0, sr.center().y + 2.0));
        ui.painter().text(pos2(sr.min.x + 1.0, sr.center().y), Align2::LEFT_CENTER, label, regular(8.0), t.text_dim);
        ui.painter().rect_filled(track, 1.0, Color32::from_rgb(24, 26, 30));
        ui.painter().rect_filled(
            Rect::from_min_max(track.min, pos2(track.min.x + track.width() * v.clamp(0.0, 1.0), track.max.y)),
            1.0,
            t.counter_text,
        );
        if (sresp.dragged() || sresp.clicked())
            && let Some(p) = sresp.interact_pointer_pos()
        {
            let nv = ((p.x - track.min.x) / track.width().max(1.0)).clamp(0.0, 1.0);
            edit = Some(if kind == 0 { PannerEdit::Divergence(nv) } else { PannerEdit::Height(nv) });
        }
        if sresp.double_clicked() {
            edit = Some(if kind == 0 { PannerEdit::Divergence(0.0) } else { PannerEdit::Height(0.0) });
        }
        let _ = sresp.on_hover_text(if kind == 0 { "Divergence (0 = point source)" } else { "Elevation (height speakers)" });
    }
    edit
}

/// A bank of thin vertical peak meters, one per channel, labelled underneath (rotated text).
pub fn multi_meter(ui: &Ui, r: Rect, levels: &[f32], labels: &[String], clip: bool) {
    let n = levels.len().max(1);
    let label_h = 16.0;
    let mr = Rect::from_min_max(r.min, pos2(r.max.x, r.max.y - label_h));
    let gap = 1.0;
    let w = ((mr.width() - gap * (n as f32 - 1.0)) / n as f32).max(1.0);
    for (i, lv) in levels.iter().enumerate() {
        let x = mr.min.x + i as f32 * (w + gap);
        let br = Rect::from_min_size(pos2(x, mr.min.y), vec2(w, mr.height()));
        meter(ui, br, *lv, *lv, clip);
        if let Some(l) = labels.get(i) {
            let galley = ui.painter().layout_no_wrap(l.clone(), regular(7.0), Tokens::current().text_dim);
            let gw = galley.size().x;
            let gh = galley.size().y;
            // Rotated 90° anticlockwise, reading bottom-to-top, centred under the bar.
            let pos = pos2(br.center().x - gh * 0.5, mr.max.y + 2.0 + gw.min(label_h - 2.0));
            ui.painter().add(egui::epaint::TextShape::new(pos, galley, Tokens::current().text_dim).with_angle(-std::f32::consts::FRAC_PI_2));
        }
    }
}

/// egui's checkbox, plus drag-to-set (#399): press one box and drag over its neighbours, and each
/// box the pointer passes takes the state the first one switched to. A click toggles one box.
pub fn checkbox(ui: &mut Ui, on: &mut bool, label: impl Into<egui::WidgetText>) -> Response {
    // egui's own toggle works on a copy: `drag_toggle` decides clicks and drags alike.
    let mut copy = *on;
    let mut resp = ui.checkbox(&mut copy, label.into()).interact(Sense::drag());
    if drag_toggle(ui, &resp, on) {
        resp.mark_changed();
    }
    resp
}

/// The new state lives in the egui context from the first box's drag start until the button is
/// released. Returns whether `on` changed.
fn drag_toggle(ui: &Ui, resp: &Response, on: &mut bool) -> bool {
    let ctx = ui.ctx();
    let key = egui::Id::new("soundcraft-checkbox-drag-target");
    if !ctx.input(|i| i.pointer.primary_down()) {
        ctx.data_mut(|d| d.remove::<bool>(key));
    }
    if resp.drag_started_by(egui::PointerButton::Primary) {
        *on = !*on;
        ctx.data_mut(|d| d.insert_temp(key, *on));
        return true;
    }
    if resp.clicked() {
        *on = !*on;
        return true;
    }
    let Some(target) = ctx.data(|d| d.get_temp::<bool>(key)) else { return false };
    // Clipped and layer-aware: rows scrolled out of view or under another window stay as they are.
    if ui.rect_contains_pointer(resp.rect) && *on != target {
        *on = target;
        return true;
    }
    false
}

/// What a [`knob`] did this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct KnobResponse {
    /// The new value, when the user changed it.
    pub changed: Option<f32>,
    /// Double-click: the caller should reset to the default (`changed` carries it too).
    pub reset: bool,
    /// The pointer is dragging the knob.
    pub dragging: bool,
}

/// Pixels of vertical drag that sweep a knob's whole range.
const KNOB_DRAG_PX: f32 = 200.0;

/// The value after dragging `dy_px` (screen y; negative = up = more) from `value`. 200 px is the
/// full range in normalised space, so log tapers feel even; `fine` is 10x slower. Always legal.
pub fn knob_drag(info: &soundcraft_dsp::ParamInfo, value: f32, dy_px: f32, fine: bool) -> f32 {
    let dy = if dy_px.is_finite() { dy_px } else { 0.0 };
    let k = if fine { 0.1 } else { 1.0 };
    let t = info.to_normalized(value) - dy / KNOB_DRAG_PX * k;
    info.from_normalized(t)
}

/// The value after `notches` of scroll wheel (positive = up = more): 1/100 of the range each.
pub fn knob_scroll(info: &soundcraft_dsp::ParamInfo, value: f32, notches: f32) -> f32 {
    let n = if notches.is_finite() { notches } else { 0.0 };
    info.from_normalized(info.to_normalized(value) + n * 0.01)
}

/// The next value of a toggle or choice parameter when clicked (wraps around).
pub fn knob_cycle(info: &soundcraft_dsp::ParamInfo, value: f32) -> f32 {
    let v = info.clamp(value);
    let next = if v + 1.0 > info.max { info.min } else { v + 1.0 };
    info.clamp(next)
}

/// Look of a [`knob`]: brushed dark metal (the default), light silver, or the large "hero" knob
/// with a segmented value ring for a module's main control.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KnobStyle {
    #[default]
    Dark,
    Silver,
    Hero,
}

/// `r` moved (and, when larger, shrunk) so all of it is inside `screen`: a window opened near a
/// screen edge slides in; one larger than the screen keeps its top-left (title bar) in view at
/// the largest size that fits (spec 2026-11-02 item 6).
pub fn fit_on_screen(r: Rect, screen: Rect) -> Rect {
    if !r.is_finite() || !screen.is_finite() || screen.width() <= 0.0 || screen.height() <= 0.0 {
        return r;
    }
    let size = r.size().max(Vec2::ZERO).min(screen.size());
    let x = r.min.x.min(screen.max.x - size.x).max(screen.min.x);
    let y = r.min.y.min(screen.max.y - size.y).max(screen.min.y);
    Rect::from_min_size(pos2(x, y), size)
}

fn wheel_scrolls_id() -> egui::Id {
    egui::Id::new("soundcraft-wheel-scrolls")
}

/// Marks what is drawn next (until [`end_wheel_scrolls`]) as a scrolling area of knobs, the Mix
/// window's strips and drawers: there the wheel always scrolls and knobs, sliders and value
/// boxes ignore it (spec 2026-11-02 item 3). Plugin windows of their own keep the wheel.
pub(crate) fn begin_wheel_scrolls(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(wheel_scrolls_id(), true));
}

/// Ends [`begin_wheel_scrolls`].
pub(crate) fn end_wheel_scrolls(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<bool>(wheel_scrolls_id()));
}

/// Whether the wheel only scrolls here (see [`begin_wheel_scrolls`]): controls leave it alone.
pub fn wheel_scrolls(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(wheel_scrolls_id())).unwrap_or(false)
}

/// Points of smooth scroll that make one wheel notch (egui's native line scroll speed).
pub(crate) const KNOB_NOTCH_PX: f32 = 40.0;

/// The pointer angle for a normalised value, in radians: 0 = straight up, clockwise positive,
/// value 0 at -135° and 1 at +135°. Non-finite input reads as 0.
pub fn value_angle(v01: f32) -> f32 {
    let v = if v01.is_finite() { v01.clamp(0.0, 1.0) } else { 0.0 };
    (-135.0 + 270.0 * v).to_radians()
}

/// The 11 tick angles (every 27° over the sweep), in radians.
pub fn tick_angles() -> [f32; 11] {
    std::array::from_fn(|i| value_angle(i as f32 / 10.0))
}

/// The point at `radius` from `c` in the direction of `angle` (see [`value_angle`]).
fn polar(c: egui::Pos2, angle: f32, radius: f32) -> egui::Pos2 {
    c + vec2(angle.sin(), -angle.cos()) * radius
}

/// Every measurement of a knob of diameter `size`, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
struct KnobGeom {
    /// Half the side of the allocated square.
    half: f32,
    /// Scale relative to the reference size of the style.
    s: f32,
    cap_r: f32,
    tick_r: f32,
    tick_w: f32,
    tick_len: f32,
    /// Pointer: start and end distance from the centre, and width.
    ptr_from: f32,
    ptr_to: f32,
    ptr_w: f32,
    /// Hero only: segmented ring radius and width.
    ring: Option<(f32, f32)>,
    glow_idle: f32,
    glow_hot: f32,
}

fn knob_geom(size: f32, style: KnobStyle) -> KnobGeom {
    let size = if size.is_finite() { size.clamp(8.0, 512.0) } else { 28.0 };
    let hero = style == KnobStyle::Hero;
    // The reference diameters the pixel values of the look were drawn at.
    let s = (size / if hero { 140.0 } else { 64.0 }).clamp(0.5, 2.0);
    let (tick_r, tick_w, tick_len) = if size < 24.0 { (size * 0.5 + 4.0, 1.0, 3.0) } else { (size * 0.5 + 9.0 * s, 2.0 * s, 6.0 * s) };
    let cap_r = if hero { size * (0.5 - 11.0 / 140.0) } else { size * 0.5 };
    let (top, len, w) = if hero { (0.10, 0.24, 4.0) } else { (0.11, 0.27, 3.0) };
    let d = 2.0 * cap_r;
    KnobGeom {
        half: (tick_r + tick_len * 0.5).max(size * 0.5) + 1.0,
        s,
        cap_r,
        tick_r,
        tick_w,
        tick_len,
        ptr_from: cap_r - top * d,
        ptr_to: cap_r - (top + len) * d,
        ptr_w: (w * s).max(1.5),
        ring: hero.then_some((size * 64.0 / 140.0, size * 10.0 / 140.0)),
        glow_idle: size * 0.5 + 8.0 * s,
        glow_hot: size * 0.5 + 28.0 * s,
    }
}

const DARK_CONIC: (f32, [(f32, [u8; 3]); 10]) = (
    20.0,
    [
        (0.00, [0x26, 0x28, 0x2e]),
        (0.10, [0x43, 0x46, 0x4f]),
        (0.22, [0x1b, 0x1c, 0x20]),
        (0.34, [0x2b, 0x2d, 0x33]),
        (0.46, [0x4a, 0x4d, 0x57]),
        (0.58, [0x1a, 0x1b, 0x1f]),
        (0.70, [0x30, 0x32, 0x3a]),
        (0.82, [0x49, 0x4c, 0x55]),
        (0.92, [0x20, 0x22, 0x27]),
        (1.00, [0x26, 0x28, 0x2e]),
    ],
);

const SILVER_CONIC: (f32, [(f32, [u8; 3]); 9]) = (
    0.0,
    [
        (0.00, [0xf1, 0xf1, 0xf1]),
        (0.12, [0xb9, 0xba, 0xbd]),
        (0.25, [0xfb, 0xfb, 0xfb]),
        (0.37, [0xa9, 0xaa, 0xad]),
        (0.50, [0xf4, 0xf4, 0xf4]),
        (0.62, [0xb7, 0xb8, 0xbb]),
        (0.75, [0xfc, 0xfc, 0xfc]),
        (0.87, [0xac, 0xad, 0xb1]),
        (1.00, [0xf1, 0xf1, 0xf1]),
    ],
);

/// The colour of a conic gradient at `t` (0..1 of the turn), interpolating between `stops`.
fn conic_color(stops: &[(f32, [u8; 3])], t: f32) -> Color32 {
    let t = if t.is_finite() { t.rem_euclid(1.0) } else { 0.0 };
    let mut prev = stops.first().copied().unwrap_or((0.0, [0, 0, 0]));
    for &(at, c) in stops {
        if t <= at {
            let span = at - prev.0;
            let k = if span > 0.0 { (t - prev.0) / span } else { 1.0 };
            let [a, b] = [prev.1, c].map(|[r, g, b]| Color32::from_rgb(r, g, b));
            return a.lerp_to_gamma(b, k);
        }
        prev = (at, c);
    }
    let [r, g, b] = prev.1;
    Color32::from_rgb(r, g, b)
}

/// `c` brightened by the fraction `k` (0.07 = 7 % brighter).
fn brighten(c: Color32, k: f32) -> Color32 {
    let f = 1.0 + k.clamp(0.0, 1.0);
    let ch = |v: u8| (f32::from(v) * f).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_premultiplied(ch(c.r()), ch(c.g()), ch(c.b()), c.a())
}

/// A disabled/bypassed knob's colour: desaturated to 30 % and at 42 % opacity.
fn knob_disabled(c: Color32) -> Color32 {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    let grey = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
    let mix = |v: u8| (grey + (f32::from(v) - grey) * 0.3).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(mix(r), mix(g), mix(b), a).gamma_multiply(0.42)
}

/// Adds a ring band from radius `r0` to `r1` around `c`, coloured per angle at each edge.
fn ring_band(mesh: &mut Mesh, c: egui::Pos2, r0: f32, r1: f32, inner: impl Fn(f32) -> Color32, outer: impl Fn(f32) -> Color32) {
    const N: u32 = 72;
    let base = mesh.vertices.len() as u32;
    for i in 0..=N {
        let a = std::f32::consts::TAU * i as f32 / N as f32;
        mesh.colored_vertex(polar(c, a, r0), inner(a));
        mesh.colored_vertex(polar(c, a, r1), outer(a));
    }
    for i in 0..N {
        let k = base + 2 * i;
        mesh.add_triangle(k, k + 1, k + 2);
        mesh.add_triangle(k + 1, k + 2, k + 3);
    }
}

/// A thick arc from angle `a0` to `a1` (see [`value_angle`]).
fn arc(p: &Painter, c: egui::Pos2, r: f32, a0: f32, a1: f32, stroke: Stroke) {
    let n = (((a1 - a0).abs() * r / 3.0).ceil() as usize).clamp(2, 128);
    let pts: Vec<egui::Pos2> = (0..=n).map(|i| polar(c, a0 + (a1 - a0) * i as f32 / n as f32, r)).collect();
    p.add(egui::Shape::line(pts, stroke));
}

/// Paints a knob body centred on `c`. `hot` (0..1) is the hover/drag glow, `fx` maps every colour
/// (disabled look).
fn paint_knob(
    p: &Painter,
    c: egui::Pos2,
    g: &KnobGeom,
    style: KnobStyle,
    norm: f32,
    accent: Color32,
    hot: f32,
    dark_bg: bool,
    fx: &dyn Fn(Color32) -> Color32,
) {
    let hot = hot.clamp(0.0, 1.0);
    let angle = value_angle(norm);

    // Accent glow halo: a soft radial falloff from the cap edge.
    let glow_a = 0.10 + (0.65 - 0.10) * hot;
    let glow_r = g.glow_idle + (g.glow_hot - g.glow_idle) * hot;
    let mid = g.cap_r + (glow_r - g.cap_r) * 0.35;
    let mut glow = Mesh::default();
    ring_band(&mut glow, c, g.cap_r * 0.9, mid, |_| fx(accent.gamma_multiply(glow_a)), |_| fx(accent.gamma_multiply(glow_a * 0.3)));
    ring_band(&mut glow, c, mid, glow_r, |_| fx(accent.gamma_multiply(glow_a * 0.3)), |_| Color32::TRANSPARENT);
    p.add(glow);

    // Ticks.
    let tick = if dark_bg { Color32::from_rgba_unmultiplied(190, 194, 204, 115) } else { Color32::from_rgba_unmultiplied(70, 72, 80, 115) };
    for a in tick_angles() {
        p.line_segment([polar(c, a, g.tick_r - g.tick_len * 0.5), polar(c, a, g.tick_r + g.tick_len * 0.5)], Stroke::new(g.tick_w, fx(tick)));
    }

    // Hero: segmented value ring over a dark base, lit in the accent up to the value.
    if let Some((ring_r, ring_w)) = g.ring {
        let (a0, a1) = (value_angle(0.0), value_angle(1.0));
        arc(p, c, ring_r, a0, a1, Stroke::new(ring_w, fx(Color32::from_rgb(0x15, 0x16, 0x1a))));
        let period = std::f32::consts::TAU / 60.0;
        let dashes = ((a1 - a0) / period).round() as usize;
        let unlit = Color32::from_rgba_unmultiplied(0x9a, 0x9d, 0xa6, 178);
        for i in 0..dashes {
            let d0 = a0 + period * (i as f32 + 0.25);
            let d1 = d0 + period * 0.5;
            let lit = (d0 + d1) * 0.5 <= angle + 1e-4 && norm > 0.0;
            arc(p, c, ring_r, d0, d1, Stroke::new(ring_w * 0.6, fx(if lit { accent } else { unlit })));
        }
    }

    // Drop shadow below the cap.
    for (dy, grow, a) in [(2.0, 0.0, 0.30), (4.0, 1.0, 0.20), (7.0, 3.0, 0.12), (10.0, 5.0, 0.07)] {
        p.circle_filled(c + vec2(0.0, dy * g.s), g.cap_r + grow * g.s, fx(Color32::from_black_alpha((a * 255.0) as u8)));
    }

    // Cap: conic brushed metal.
    let silver = style == KnobStyle::Silver;
    let (from_deg, stops): (f32, &[(f32, [u8; 3])]) = if silver { (SILVER_CONIC.0, &SILVER_CONIC.1) } else { (DARK_CONIC.0, &DARK_CONIC.1) };
    let lift = 0.07 * hot;
    let mut cap = Mesh::default();
    const SEG: u32 = 72;
    for i in 0..SEG {
        let (t0, t1) = (i as f32 / SEG as f32, (i + 1) as f32 / SEG as f32);
        let ang = |t: f32| from_deg.to_radians() + std::f32::consts::TAU * t;
        let k = cap.vertices.len() as u32;
        cap.colored_vertex(c, fx(brighten(conic_color(stops, (t0 + t1) * 0.5), lift)));
        cap.colored_vertex(polar(c, ang(t0), g.cap_r), fx(brighten(conic_color(stops, t0), lift)));
        cap.colored_vertex(polar(c, ang(t1), g.cap_r), fx(brighten(conic_color(stops, t1), lift)));
        cap.add_triangle(k, k + 1, k + 2);
    }
    p.add(cap);

    // Fine machined rings.
    if g.cap_r * 2.0 >= 24.0 {
        let mut r = 2.4;
        let mut light = true;
        while r < g.cap_r - 1.0 {
            let col = if light { Color32::from_white_alpha(11) } else { Color32::from_black_alpha(26) };
            p.circle_stroke(c, r, Stroke::new(1.0, fx(col)));
            r += 2.4;
            light = !light;
        }
    }

    // Top highlight: radial glow at (50 %, 28 %) of the cap.
    let hl_c = c + vec2(0.0, -g.cap_r * if silver { 0.48 } else { 0.44 });
    let hl_a = if silver { 0.8 } else { 0.16 };
    let mut hl = Mesh::default();
    ring_band(&mut hl, hl_c, 0.0, g.cap_r * 0.55, |_| fx(Color32::from_white_alpha((hl_a * 255.0) as u8)), |_| Color32::TRANSPARENT);
    p.add(hl);

    // Rim: inner light edge, inner bottom shadow, thin outer ring.
    p.circle_stroke(c, g.cap_r - 0.75, Stroke::new(1.5, fx(Color32::from_white_alpha(41))));
    let band = (g.cap_r * 0.18).max(1.5);
    let shadow_a = if silver { 0.35 } else { 0.75 };
    let mut inner = Mesh::default();
    ring_band(
        &mut inner,
        c,
        g.cap_r - band,
        g.cap_r,
        |_| Color32::TRANSPARENT,
        |a| {
            // Strongest at the bottom (angle pi), none above the equator.
            let k = (-a.cos()).max(0.0);
            fx(Color32::from_black_alpha((shadow_a * k * k * 255.0) as u8))
        },
    );
    p.add(inner);
    if silver {
        p.circle_stroke(c, g.cap_r + 0.5, Stroke::new(1.0, fx(Color32::from_rgba_unmultiplied(90, 90, 98, 153))));
    } else {
        p.circle_stroke(c, g.cap_r + 1.0, Stroke::new(2.0, fx(Color32::from_rgb(0x0e, 0x0f, 0x12))));
    }

    // Pointer: a rounded bar with a faint dark edge.
    let (a, b) = (polar(c, angle, g.ptr_from), polar(c, angle, g.ptr_to));
    let ptr = if silver { Color32::from_rgb(0x24, 0x26, 0x2b) } else { Color32::from_rgb(0xf3, 0xf1, 0xec) };
    let edge = fx(Color32::from_black_alpha(if silver { 40 } else { 110 }));
    let w = g.ptr_w;
    p.line_segment([a, b], Stroke::new(w + 1.2, edge));
    p.circle_filled(a, (w + 1.2) * 0.5, edge);
    p.circle_filled(b, (w + 1.2) * 0.5, edge);
    p.line_segment([a, b], Stroke::new(w, fx(ptr)));
    p.circle_filled(a, w * 0.5, fx(ptr));
    p.circle_filled(b, w * 0.5, fx(ptr));
}

/// The DAW knob's face, filling the square `rect` (its ticks and glow included) for a normalised
/// `v01`, with no interaction: the editors draw it where the VST has a knob and handle the
/// gestures as the VST does. `hovered` or `active` light the glow as [`knob`] does.
pub fn paint_knob_face(p: &Painter, rect: Rect, v01: f32, accent: Color32, style: KnobStyle, hovered: bool, active: bool) {
    let side = rect.width().min(rect.height());
    if !side.is_finite() || side <= 2.0 {
        return;
    }
    // The diameter whose allocated square (`2 * half`) fits `side`: one correction of a guess.
    let mut size = side * 0.6;
    for _ in 0..3 {
        let g = knob_geom(size, style);
        size *= side / (2.0 * g.half).max(1.0);
    }
    let g = knob_geom(size, style);
    let v = if v01.is_finite() { v01.clamp(0.0, 1.0) } else { 0.0 };
    let hot = if hovered || active { 1.0 } else { 0.0 };
    paint_knob(p, rect.center(), &g, style, v, accent, hot, true, &|c| c);
}

/// A parameter knob: vertical drag (Shift = fine), scroll wheel (not in the Mix window's
/// strips, [`wheel_scrolls`]), double-click to reset, tooltip
/// with the formatted value while hovered or dragged. Toggle and choice parameters draw a small
/// pill that cycles on click. `accent` is the module/engine colour for the glow and hero ring; a
/// disabled `ui` draws the knob dimmed and desaturated.
pub fn knob(ui: &mut Ui, info: &soundcraft_dsp::ParamInfo, value: f32, size: f32, accent: Color32, style: KnobStyle) -> KnobResponse {
    knob_inner(ui, info, value, size, accent, style, false)
}

fn knob_inner(
    ui: &mut Ui,
    info: &soundcraft_dsp::ParamInfo,
    value: f32,
    size: f32,
    accent: Color32,
    style: KnobStyle,
    force_hot: bool,
) -> KnobResponse {
    use soundcraft_dsp::Unit;
    let t = Tokens::current();
    let value = info.clamp(value);
    let size = if size.is_finite() { size.clamp(8.0, 512.0) } else { 28.0 };
    let mut out = KnobResponse::default();
    let enabled = ui.is_enabled();
    let fx = move |c: Color32| if enabled { c } else { knob_disabled(c) };
    if matches!(info.unit, Unit::Toggle | Unit::Choice) {
        let h = (size * 0.5).max(14.0);
        let (r, resp) = ui.allocate_exact_size(vec2(size.max(28.0), h), Sense::click());
        let on = matches!(info.unit, Unit::Choice) || value >= 0.5;
        let fill = if on && matches!(info.unit, Unit::Toggle) { accent } else { t.button };
        let p = ui.painter();
        p.rect_filled(r, CornerRadius::same((h * 0.5) as u8), fx(fill));
        p.rect_stroke(r, CornerRadius::same((h * 0.5) as u8), Stroke::new(1.0, fx(t.button_border)), StrokeKind::Inside);
        let label = if matches!(info.unit, Unit::Choice) { info.format(value) } else { info.name.to_string() };
        let color = if on && matches!(info.unit, Unit::Toggle) { t.mode_on_text } else { t.text };
        p.with_clip_rect(r.shrink(2.0)).text(r.center(), Align2::CENTER_CENTER, label, regular(10.0), fx(color));
        if resp.clicked() {
            out.changed = Some(knob_cycle(info, value));
        }
        resp.on_hover_text(format!("{}: {}", info.name, info.format(value)));
        return out;
    }

    let g = knob_geom(size, style);
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(g.half * 2.0), Sense::click_and_drag());
    let start_key = resp.id.with("knob-start");
    let ctx = ui.ctx().clone();
    if resp.drag_started() {
        ctx.data_mut(|d| d.insert_temp(start_key, value));
    }
    if resp.dragged() {
        let start = ctx.data(|d| d.get_temp::<f32>(start_key)).unwrap_or(value);
        let dy = resp.total_drag_delta().map_or(0.0, |d| d.y);
        let fine = ui.input(|i| i.modifiers.shift);
        let v = knob_drag(info, start, dy, fine);
        if (v - value).abs() > f32::EPSILON {
            out.changed = Some(v);
        }
        out.dragging = true;
    }
    if resp.hovered() && enabled && !wheel_scrolls(ui.ctx()) {
        let notches = ui.input(|i| i.smooth_scroll_delta.y) / KNOB_NOTCH_PX;
        if notches != 0.0 {
            // The knob takes the wheel; the surrounding scroll area must not move too.
            ui.input_mut(|i| i.smooth_scroll_delta.y = 0.0);
            let v = knob_scroll(info, value, notches);
            if (v - value).abs() > f32::EPSILON {
                out.changed = Some(v);
            }
        }
    }
    if resp.double_clicked() {
        out.changed = Some(info.default);
        out.reset = true;
    }

    let shown = out.changed.unwrap_or(value);
    let hot_now = force_hot || (enabled && (resp.hovered() || out.dragging));
    let hot = if force_hot { 1.0 } else { ctx.animate_bool_with_time(resp.id.with("knob-glow"), hot_now, 0.12) };
    paint_knob(ui.painter(), r.center(), &g, style, info.to_normalized(shown), accent, hot, ui.visuals().dark_mode, &fx);
    let tip = format!("{}: {}", info.name, info.format(shown));
    if out.dragging {
        resp.show_tooltip_text(tip);
    } else {
        resp.on_hover_text(tip);
    }
    out
}

/// A [`knob`] with its name above (semi-bold; not for [`KnobStyle::Hero`]) and its value below.
pub fn knob_with_text(ui: &mut Ui, info: &soundcraft_dsp::ParamInfo, value: f32, size: f32, accent: Color32, style: KnobStyle) -> KnobResponse {
    let t = Tokens::current();
    let hero = style == KnobStyle::Hero;
    let size = if size.is_finite() { size.clamp(8.0, 512.0) } else { 28.0 };
    let label_px = (size * 0.2).clamp(9.0, 15.0);
    let value_px = if hero { (size * 0.14).clamp(11.0, 20.0) } else { (size * 0.2).clamp(9.0, 17.0) };
    let label_h = if hero { 0.0 } else { label_px + 4.0 };
    let side = knob_geom(size, style).half * 2.0;
    let w = side.max(size + 24.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, label_h + side + value_px + 4.0), Sense::hover());
    let knob_rect = Rect::from_center_size(egui::pos2(rect.center().x, rect.top() + label_h + side * 0.5), Vec2::splat(side));
    let out = ui.scope_builder(egui::UiBuilder::new().max_rect(knob_rect), |ui| knob(ui, info, value, size, accent, style)).inner;
    let fx = |c: Color32| if ui.is_enabled() { c } else { knob_disabled(c) };
    let p = ui.painter();
    if !hero {
        p.text(egui::pos2(rect.center().x, rect.top()), Align2::CENTER_TOP, info.name, bold(label_px), fx(t.text));
    }
    let shown = out.changed.unwrap_or(value);
    p.text(egui::pos2(rect.center().x, rect.bottom()), Align2::CENTER_BOTTOM, info.format(shown), regular(value_px), fx(t.text_dim));
    out
}

#[cfg(test)]
mod tests {
    use super::{clip_gain_from_pos, clip_gain_to_pos};
    use egui::{Event, Modifiers, PointerButton, Pos2};
    use egui_kittest::{Harness, kittest::Queryable};

    fn press(h: &mut Harness<'_, [bool; 4]>, pos: Pos2, pressed: bool) {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.run_steps(2);
    }

    fn boxes() -> (Harness<'static, [bool; 4]>, Vec<Pos2>) {
        let mut h = Harness::new_ui_state(
            |ui, s: &mut [bool; 4]| {
                ui.vertical(|ui| {
                    for b in s.iter_mut() {
                        super::checkbox(ui, b, "Box");
                    }
                });
            },
            [false, true, false, false],
        );
        h.run();
        let at = h.get_all_by_label("Box").map(|n| n.rect().center()).collect();
        (h, at)
    }

    /// #399: pressing a box and dragging over its neighbours sets them all to the state the first
    /// one switched to; the box past the drag stays as it was.
    #[test]
    fn dragging_across_checkboxes_sets_them_all_like_the_first() {
        let (mut h, at) = boxes();
        assert_eq!(at.len(), 4);
        h.event(Event::PointerMoved(at[0]));
        h.run_steps(1);
        press(&mut h, at[0], true);
        for p in &at[1..3] {
            h.event(Event::PointerMoved(*p));
            h.run_steps(2);
        }
        press(&mut h, at[2], false);
        assert_eq!(*h.state(), [true, true, true, false]);
    }

    /// Dragging from a checked box unchecks; after release, moving over boxes changes nothing.
    #[test]
    fn dragging_from_a_checked_box_unchecks_and_release_ends_it() {
        let (mut h, at) = boxes();
        h.state_mut().copy_from_slice(&[true, true, true, true]);
        h.run();
        h.event(Event::PointerMoved(at[1]));
        h.run_steps(1);
        press(&mut h, at[1], true);
        h.event(Event::PointerMoved(at[2]));
        h.run_steps(2);
        press(&mut h, at[2], false);
        h.event(Event::PointerMoved(at[3]));
        h.run_steps(2);
        assert_eq!(*h.state(), [true, false, false, true]);
    }

    /// A row scrolled out of view isn't set when the drag passes over the place it would be.
    #[test]
    fn dragging_past_a_scroll_area_leaves_hidden_rows_alone() {
        let mut h = Harness::new_ui_state(
            |ui, s: &mut [bool; 4]| {
                egui::ScrollArea::vertical().max_height(30.0).show(ui, |ui| {
                    for b in s.iter_mut() {
                        super::checkbox(ui, b, "Box");
                    }
                });
            },
            [false; 4],
        );
        h.run();
        let at: Vec<Pos2> = h.get_all_by_label("Box").map(|n| n.rect().center()).collect();
        assert_eq!(at.len(), 4);
        h.event(Event::PointerMoved(at[0]));
        h.run_steps(1);
        press(&mut h, at[0], true);
        h.event(Event::PointerMoved(at[3]));
        h.run_steps(2);
        press(&mut h, at[3], false);
        assert_eq!(*h.state(), [true, false, false, false]);
    }

    /// A click still toggles just the one box, once.
    #[test]
    fn a_click_toggles_one_checkbox() {
        let (mut h, at) = boxes();
        h.event(Event::PointerMoved(at[1]));
        h.run_steps(1);
        press(&mut h, at[1], true);
        press(&mut h, at[1], false);
        assert_eq!(*h.state(), [false, false, false, false]);
    }

    #[test]
    fn clip_gain_taper_spans_minus_144_to_plus_36_and_round_trips() {
        assert_eq!(clip_gain_from_pos(0.0), -144.0);
        assert_eq!(clip_gain_from_pos(1.0), 36.0);
        assert_eq!(clip_gain_from_pos(f32::NAN), -144.0);
        assert_eq!(clip_gain_to_pos(f32::INFINITY), 0.0);
        for db in [-144.0, -90.0, -36.0, -12.5, 0.0, 6.0, 36.0] {
            assert!((clip_gain_from_pos(clip_gain_to_pos(db)) - db).abs() < 0.01, "{db}");
        }
    }
}

#[cfg(test)]
mod selector_visual_regressions {
    use super::*;

    fn scaled_input(size: egui::Vec2, scale: f32) -> egui::RawInput {
        let mut input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)), ..Default::default() };
        input.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(scale);
        input
    }

    #[test]
    fn capture_selector_visual_fixtures() {
        let _palette = crate::theme::lock_test_palette();
        let Some(directory) = std::env::var_os("CRAFT_UI_VISUAL_FIXTURES").map(std::path::PathBuf::from) else { return };
        std::fs::create_dir_all(&directory).unwrap();
        for scale in [1.0, 1.5, 2.0] {
            let mut ready = false;
            let mut harness = egui_kittest::Harness::builder().with_size(vec2(320.0, 120.0)).with_pixels_per_point(scale).wgpu().build_ui_state(
                move |ui, regions: &mut serde_json::Map<String, serde_json::Value>| {
                    if !ready {
                        crate::fonts::install(ui.ctx());
                        crate::theme::apply(ui.ctx(), false);
                        ready = true;
                        return;
                    }
                    regions.clear();
                    for (index, label) in ["auto read", "no group", "A very long mixer group name"].into_iter().enumerate() {
                        ui.horizontal(|ui| {
                            for width in [46.0, 74.0, 110.0] {
                                let r = selector_box(ui, width, 16.0, label, Tokens::current().text).rect;
                                regions.insert(format!("selector-{index}-{width}"), serde_json::json!([r.left(), r.top(), r.width(), r.height()]));
                            }
                        });
                    }
                },
                serde_json::Map::new(),
            );
            harness.input_mut().max_texture_side = Some(8192);
            harness.run_steps(4);
            let stem = format!("soundcraft-selectors-dark-{scale}x");
            harness.render().unwrap().save(directory.join(format!("{stem}.png"))).unwrap();
            std::fs::write(
                directory.join(format!("{stem}.json")),
                serde_json::to_vec_pretty(&serde_json::json!({
                    "screen": [320.0, 120.0], "scale": scale, "regions": harness.state()
                }))
                .unwrap(),
            )
            .unwrap();
        }
    }

    fn mixer_app(narrow: bool, mode: soundcraft_model::AutomationMode) -> crate::SoundApp {
        let mut app = crate::SoundApp::new(soundcraft_engine::Engine::default(), None, Default::default());
        app.run("track.new", serde_json::json!({"name": "Voice"})).unwrap();
        app.run("track.new", serde_json::json!({"name": "Music"})).unwrap();
        let ids: Vec<_> = app.engine.session().tracks.iter().map(|track| track.id).collect();
        for id in ids {
            app.run("mix.automation_mode", serde_json::json!({"track": id.0, "mode": mode.label()})).unwrap();
        }
        app.ui.narrow_mix = narrow;
        app.ui.show_tracks_list = false;
        app.ui.mix_views = vec!["io".into()];
        app
    }

    #[test]
    fn actual_mixer_keeps_every_active_automation_mode_legible() {
        let _palette = crate::theme::lock_test_palette();
        for narrow in [false, true] {
            for mode in soundcraft_model::AutomationMode::ALL {
                for scale in [1.0, 1.5, 2.0] {
                    let ctx = egui::Context::default();
                    crate::fonts::install(&ctx);
                    crate::theme::apply(&ctx, false);
                    let mut app = mixer_app(narrow, mode);
                    let mut output = ctx.run_ui(scaled_input(vec2(320.0, 640.0), scale), |ui| crate::mix_window::show(&mut app, ui));
                    output.textures_delta.clear();
                    assert_eq!(ctx.content_rect().size(), vec2(320.0, 640.0));
                    assert_eq!(ctx.pixels_per_point(), scale);
                    let long = format!("auto {}", mode.label());
                    let short = if mode == soundcraft_model::AutomationMode::TouchLatch { "t/l" } else { mode.label() };
                    let texts: Vec<_> = output
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::Shape::Text(text)
                                if text.galley.job.text == long || text.galley.job.text == mode.label() || text.galley.job.text == short =>
                            {
                                Some(text)
                            }
                            _ => None,
                        })
                        .collect();
                    assert_eq!(texts.len(), 2, "both tracks paint their automation mode: {mode:?}");
                    for text in texts {
                        assert_eq!(text.galley.rows.len(), 1, "{mode:?} narrow={narrow} at {scale}x: automation text wraps");
                        assert!(
                            !text.galley.elided,
                            "{mode:?} narrow={narrow} at {scale}x: the active mode must remain distinguishable without hovering"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn capture_actual_mixer_visual_fixtures() {
        let _palette = crate::theme::lock_test_palette();
        let Some(directory) = std::env::var_os("CRAFT_UI_VISUAL_FIXTURES").map(std::path::PathBuf::from) else { return };
        std::fs::create_dir_all(&directory).unwrap();
        for mode in soundcraft_model::AutomationMode::ALL {
            for narrow in [false, true] {
                for scale in [1.0, 1.5, 2.0] {
                    let mut ready = false;
                    let mut harness =
                        egui_kittest::Harness::builder().with_size(vec2(320.0, 640.0)).with_pixels_per_point(scale).wgpu().build_ui_state(
                            move |ui, app: &mut crate::SoundApp| {
                                if !ready {
                                    crate::fonts::install(ui.ctx());
                                    crate::theme::apply(ui.ctx(), false);
                                    ready = true;
                                    return;
                                }
                                crate::mix_window::show(app, ui);
                            },
                            mixer_app(narrow, mode),
                        );
                    harness.input_mut().max_texture_side = Some(8192);
                    harness.run_steps(4);
                    let stem = format!("soundcraft-mixer-narrow-{narrow}-{}-{scale}x", mode.label().replace('/', "-"));
                    harness.render().unwrap().save(directory.join(format!("{stem}.png"))).unwrap();
                    let strips = harness
                        .output()
                        .shapes
                        .iter()
                        .filter_map(|shape| match &shape.shape {
                            egui::Shape::Rect(rect)
                                if (rect.rect.width()
                                    - crate::mix_window::strip_width(
                                        if narrow { soundcraft_model::StripSize::Slim } else { soundcraft_model::StripSize::Wide },
                                        true,
                                    ))
                                .abs()
                                    < 0.1
                                    && rect.rect.height() >= 560.0 =>
                            {
                                Some(rect.rect)
                            }
                            _ => None,
                        })
                        .fold(Rect::NOTHING, Rect::union);
                    assert!(strips.is_finite());
                    std::fs::write(
                        directory.join(format!("{stem}.json")),
                        serde_json::to_vec_pretty(&serde_json::json!({
                            "screen": [320.0, 640.0], "scale": scale, "mode": mode.label(), "narrow": narrow,
                            "regions": {"mixer-selectors": [strips.left(), strips.top(), strips.width(), 120.0]}
                        }))
                        .unwrap(),
                    )
                    .unwrap();
                }
            }
        }
    }

    #[test]
    fn compact_mixer_selectors_keep_labels_on_one_visible_line() {
        let _palette = crate::theme::lock_test_palette();
        for width in [46.0, 74.0, 110.0] {
            for label in ["auto read", "no group", "A very long mixer group name"] {
                for scale in [1.0, 1.5, 2.0] {
                    let ctx = egui::Context::default();
                    crate::fonts::install(&ctx);
                    crate::theme::apply(&ctx, false);
                    let mut rect = Rect::NOTHING;
                    let mut output = ctx.run_ui(scaled_input(vec2(320.0, 120.0), scale), |ui| {
                        rect = selector_box(ui, width, 16.0, label, Tokens::current().text).rect;
                    });
                    output.textures_delta.clear();
                    assert_eq!(ctx.content_rect().size(), vec2(320.0, 120.0));
                    assert_eq!(ctx.pixels_per_point(), scale);
                    let text = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.job.text == label => Some(text),
                            _ => None,
                        })
                        .expect("selector label is painted");
                    assert_eq!(text.galley.rows.len(), 1, "{label:?} at {width} must not wrap into clipped rows");
                    let bounds = text.visual_bounding_rect();
                    assert!(bounds.top() >= rect.top() && bounds.bottom() <= rect.bottom(), "{label:?}: {bounds:?} outside {rect:?}");
                    assert!(
                        bounds.left() >= rect.left() && bounds.right() <= rect.right() - 16.0 * 0.8,
                        "text must leave room for the dropdown arrow"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod knob_tests {
    use super::*;

    fn hz() -> soundcraft_dsp::ParamInfo {
        soundcraft_dsp::ParamInfo {
            id: "f",
            name: "F",
            min: 20.0,
            max: 20000.0,
            default: 1000.0,
            unit: soundcraft_dsp::Unit::Hz,
            taper: soundcraft_dsp::Taper::Log,
            choices: &[],
            display: None,
            group: "",
        }
    }

    #[test]
    fn knob_drag_maths_respects_taper() {
        let p = hz();
        let v = knob_drag(&p, 20.0, -100.0, false);
        assert!((v / 632.5 - 1.0).abs() < 0.05, "{v}");
        let fine = knob_drag(&p, 20.0, -100.0, true);
        // 10x smaller travel: 0.05 of the log range.
        assert!((fine / (20.0 * 1000f32.powf(0.05)) - 1.0).abs() < 0.05, "{fine}");
        assert_eq!(knob_drag(&p, 20.0, 500.0, false), 20.0);
        assert_eq!(knob_drag(&p, 20000.0, -500.0, false), 20000.0);
        assert_eq!(knob_drag(&p, f32::NAN, 0.0, false), p.default);
        assert!(knob_drag(&p, 100.0, f32::NAN, false).is_finite());
    }

    #[test]
    fn scroll_and_cycle() {
        let p = hz();
        let up = knob_scroll(&p, 20.0, 1.0);
        assert!((p.to_normalized(up) - 0.01).abs() < 1e-3);
        let tog = soundcraft_dsp::plugin_info("compressor").and_then(|i| i.param("limiter")).cloned();
        let Some(tog) = tog else { return };
        assert_eq!(knob_cycle(&tog, 0.0), 1.0);
        assert_eq!(knob_cycle(&tog, 1.0), 0.0);
    }

    #[test]
    fn knob_angles_and_ticks() {
        let deg = |r: f32| r.to_degrees();
        assert!((deg(value_angle(0.0)) + 135.0).abs() < 1e-3);
        assert!(deg(value_angle(0.5)).abs() < 1e-3);
        assert!((deg(value_angle(1.0)) - 135.0).abs() < 1e-3);
        assert!((deg(value_angle(7.0)) - 135.0).abs() < 1e-3);
        assert!((deg(value_angle(f32::NAN)) + 135.0).abs() < 1e-3);
        let ticks = tick_angles();
        for (i, a) in ticks.iter().enumerate() {
            assert!((deg(*a) - (-135.0 + 27.0 * i as f32)).abs() < 1e-3, "{i}");
        }
        // 0 = straight up, clockwise: +90 degrees points right.
        let p = polar(egui::Pos2::ZERO, 90f32.to_radians(), 10.0);
        assert!((p.x - 10.0).abs() < 1e-4 && p.y.abs() < 1e-4);
    }

    #[test]
    fn knob_geometry_scales() {
        let small = knob_geom(18.0, KnobStyle::Dark);
        assert_eq!((small.tick_w, small.tick_len, small.tick_r), (1.0, 3.0, 13.0));
        let mid = knob_geom(64.0, KnobStyle::Dark);
        assert_eq!((mid.tick_w, mid.tick_len, mid.tick_r), (2.0, 6.0, 41.0));
        assert!((mid.ptr_from - 32.0 * 0.78).abs() < 1e-3 && (mid.ptr_to - 32.0 * 0.24).abs() < 1e-3);
        assert_eq!(mid.ptr_w, 3.0);
        assert_eq!((mid.glow_idle, mid.glow_hot), (40.0, 60.0));
        let hero = knob_geom(140.0, KnobStyle::Hero);
        assert!((hero.cap_r - 59.0).abs() < 1e-3);
        assert_eq!(hero.ring, Some((64.0, 10.0)));
        assert_eq!(hero.ptr_w, 4.0);
        assert!(knob_geom(f32::NAN, KnobStyle::Silver).half.is_finite());
        assert!(knob_geom(18.0, KnobStyle::Dark).ptr_w >= 1.5);
    }

    #[test]
    fn knob_colours() {
        assert_eq!(conic_color(&DARK_CONIC.1, 0.0), Color32::from_rgb(0x26, 0x28, 0x2e));
        assert_eq!(conic_color(&DARK_CONIC.1, 0.46), Color32::from_rgb(0x4a, 0x4d, 0x57));
        assert_eq!(conic_color(&SILVER_CONIC.1, 1.0), Color32::from_rgb(0xf1, 0xf1, 0xf1));
        assert!(conic_color(&SILVER_CONIC.1, f32::NAN).a() == 255);
        assert_eq!(brighten(Color32::from_rgb(100, 100, 100), 0.07), Color32::from_rgb(107, 107, 107));
        let d = knob_disabled(Color32::from_rgb(226, 58, 48));
        let [r, g, b, a] = d.to_srgba_unmultiplied();
        assert!(a.abs_diff(107) <= 1, "{a}");
        assert!(r.abs_diff(g) < 226 - 58, "desaturated {r} {g} {b}");
    }

    #[test]
    fn knobs_draw_without_panicking() {
        let p = hz();
        let tog = soundcraft_dsp::plugin_info("compressor").and_then(|i| i.param("limiter")).cloned();
        let mut ready = false;
        let mut h = egui_kittest::Harness::new_ui(move |ui| {
            if !ready {
                crate::fonts::install(ui.ctx());
                ready = true;
                return;
            }
            for style in [KnobStyle::Dark, KnobStyle::Silver, KnobStyle::Hero] {
                for size in [f32::NAN, 0.0, 18.0, 64.0, 1e9] {
                    let r = knob(ui, &p, f32::NAN, size, Color32::RED, style);
                    assert!(r.changed.is_none() && !r.reset);
                    knob_with_text(ui, &p, 440.0, size, Color32::RED, style);
                }
            }
            if let Some(tog) = &tog {
                knob(ui, tog, 1.0, 28.0, Color32::RED, KnobStyle::Dark);
            }
            ui.add_enabled_ui(false, |ui| knob_with_text(ui, &p, 440.0, 28.0, Color32::RED, KnobStyle::Dark));
        });
        h.run_steps(3);
    }

    /// Writes a gallery of every knob style and size, idle and hot, to `$KNOB_GALLERY` (a PNG
    /// path); does nothing without it. For looking at the knob, not a regression check.
    #[test]
    fn knob_gallery() {
        let _palette = crate::theme::lock_test_palette();
        let Some(path) = std::env::var_os("KNOB_GALLERY").map(std::path::PathBuf::from) else { return };
        let accent = Color32::from_rgb(226, 58, 48);
        let hero_accent = Color32::from_rgb(0xff, 0x5f, 0x4d);
        let gain = soundcraft_dsp::plugin_info("compressor").and_then(|i| i.param("threshold")).unwrap();
        for light in [false, true] {
            let mut ready = false;
            let mut harness = egui_kittest::Harness::builder().with_size(vec2(1180.0, 760.0)).with_pixels_per_point(2.0).wgpu().build_ui(move |ui| {
                if !ready {
                    crate::fonts::install(ui.ctx());
                    crate::theme::apply(ui.ctx(), light);
                    ready = true;
                    return;
                }
                egui::Frame::NONE.fill(Tokens::current().module_tile_bg).inner_margin(12.0).show(ui, |ui| {
                    for style in [KnobStyle::Dark, KnobStyle::Silver, KnobStyle::Hero] {
                        ui.horizontal(|ui| {
                            for size in [18.0, 28.0, 64.0, 140.0] {
                                for hot in [false, true] {
                                    let a = if style == KnobStyle::Hero { hero_accent } else { accent };
                                    let v = if hot { -6.0 } else { -30.0 };
                                    knob_inner(ui, gain, v, size, a, style, hot);
                                }
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        knob_with_text(ui, gain, -12.0, 48.0, accent, KnobStyle::Dark);
                        knob_with_text(ui, gain, -12.0, 48.0, accent, KnobStyle::Silver);
                        knob_with_text(ui, gain, -12.0, 96.0, hero_accent, KnobStyle::Hero);
                        ui.add_enabled_ui(false, |ui| knob_with_text(ui, gain, -12.0, 48.0, accent, KnobStyle::Dark));
                    });
                });
            });
            harness.input_mut().max_texture_side = Some(8192);
            harness.run_steps(4);
            let out = if light { path.with_extension("light.png") } else { path.clone() };
            harness.render().unwrap().save(out).unwrap();
        }
    }
}
