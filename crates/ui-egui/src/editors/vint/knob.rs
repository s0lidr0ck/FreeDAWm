//! `VintKnob.h`, translated: the hardware knobs (Fluted, Chicken, Round, Knurled, Cream, Wing,
//! ApiGain, ApiFreq, SslCap) with their printed scales, and the panel's other hardware (the
//! bat-handle toggle, the name-plate button, the dome button, the SSL key with its LED, the VU).
//! Vint keeps these model-specific drawings (the owner's ruling), not the DAW knob face.
//!
//! A `VintKnob` is a `Dial` underneath: a press on the knob (its circle, `hitTest`, and the
//! Dial's knob square, `getKnobBounds`) drags it (`startValue + (startY - y) * 0.008`, Shift
//! 0.002), a double-click returns it to its default, the wheel moves it `deltaY * 0.25`; a switch
//! (a stepped control) snaps to its positions. The value shows while it is hovered or turned.

use super::config::{FaceStyle, Mark};
use super::draw::{
    Fill, Jc, ellipse, fill_ellipse, fill_round_rect, fill_star, line, quad, rotate, round_rect, stroke_closed, text_c, text_fitted, translate,
};
use crate::editor_kit::{consume_wheel, juce_wheel_delta_y};
use egui::{CursorIcon, Painter, Pos2, Rect, Sense, Ui, pos2, vec2};
use std::f32::consts::TAU;

/// A knob's face (`setFace`, `setStyle`, `setTravel`, `setDots`, `setPrintedName`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct KnobFace<'a> {
    pub style: FaceStyle,
    pub cap: Jc,
    pub ink: Jc,
    pub marks: &'a [Mark],
    /// Switch positions (0: a continuous knob).
    pub positions: usize,
    pub minor: i32,
    pub travel: Option<(f32, f32)>,
    pub dots: bool,
    pub name: &'a str,
    pub name_below: bool,
}

const LIT: Jc = Jc::argb(0xFFF2C94C);

/// `legendRadiusFor`.
pub(crate) fn legend_radius_for(w: f32, st: FaceStyle, dotted: bool) -> f32 {
    let natural = (if st == FaceStyle::Chicken { w * 0.25 + 11.0 } else { w * 0.31 + 13.0 }) + if dotted { 5.0 } else { 0.0 };
    natural.min(w * 0.5 - 13.0)
}
/// `radiusFor`.
pub(crate) fn radius_for(w: f32, st: FaceStyle, dotted: bool) -> f32 {
    let lr = legend_radius_for(w, st, dotted);
    if st == FaceStyle::Chicken { (w * 0.25).min(lr - 11.0) } else { (w * 0.31).min(lr - 13.0 - if dotted { 5.0 } else { 0.0 }) }
}
/// `centreOffset`.
pub(crate) fn centre_offset(w: f32, st: FaceStyle, dotted: bool, name_below: bool) -> f32 {
    legend_radius_for(w, st, dotted) + if name_below { 8.0 } else { 21.0 }
}
/// `heightFor`.
pub(crate) fn height_for(w: f32, st: FaceStyle, dotted: bool, name_below: bool) -> f32 {
    (centre_offset(w, st, dotted, name_below) + legend_radius_for(w, st, dotted) + if name_below { 19.0 } else { 6.0 }).round()
}

/// What a knob did this frame (`onDragStart` / `onDragging` / `onDragEnd`).
#[derive(Debug, Default)]
pub(crate) struct KnobOut {
    pub value: Option<f32>,
    pub started: bool,
    pub ended: bool,
}

#[derive(Clone, Copy, Default)]
struct Drag {
    start_y: f32,
    start_value: f32,
}

/// The knob in `r` (its component bounds), at `value` (0..1), with its gestures. `readout`: the
/// formatted value shown while hovered or turned.
#[allow(clippy::too_many_arguments)]
pub(crate) fn vint_knob(ui: &mut Ui, r: Rect, salt: usize, f: &KnobFace, value: f32, default: f32, readout: &str, enabled: bool) -> KnobOut {
    let value = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
    let w = r.width();
    let cy = r.top() + centre_offset(w, f.style, f.dots, f.name_below);
    let centre = pos2(r.center().x, cy);
    let hit_r = radius_for(w, f.style, f.dots) + 10.0;
    // `Dial::getKnobBounds` (LabelLayout::Below): presses outside it are ignored.
    let s = w.min(r.height() - 30.0).max(0.0);
    let knob_sq = Rect::from_min_size(pos2(r.center().x - s * 0.5, r.top()), vec2(s, s));
    let id = ui.id().with(("vint-knob", salt));
    let area = Rect::from_center_size(centre, vec2(hit_r * 2.0, hit_r * 2.0));
    let resp = ui.interact(area, id, if enabled { Sense::click_and_drag() } else { Sense::hover() });
    // unnamed hardware (the console knobs): its parameter's id
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, enabled, if f.name.is_empty() { super::id_of(salt) } else { f.name }));
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let over = pointer.is_some_and(|p| p.distance(centre) <= hit_r) && resp.hovered();
    let mut out = KnobOut::default();
    let key = id.with("drag");
    if enabled {
        if over || resp.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        }
        let press = ui.input(|i| i.pointer.press_origin());
        let accepts = |p: Pos2| p.distance(centre) <= hit_r && knob_sq.contains(p);
        if resp.drag_started()
            && let Some(p) = press
            && accepts(p)
        {
            ui.data_mut(|d| d.insert_temp(key, Drag { start_y: p.y, start_value: value }));
            out.started = true;
        }
        if resp.dragged()
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(dr) = ui.data(|d| d.get_temp::<Drag>(key))
        {
            let sens = if ui.input(|i| i.modifiers.shift) { 0.002 } else { 0.008 };
            let v = (dr.start_value + (dr.start_y - p.y) * sens).clamp(0.0, 1.0);
            if (v - value).abs() > f32::EPSILON {
                out.value = Some(v);
            }
        }
        if resp.drag_stopped() && ui.data_mut(|d| d.remove_temp::<Drag>(key)).is_some() {
            out.ended = true;
        }
        if resp.double_clicked() && press.is_some_and(accepts) {
            out.value = Some(default.clamp(0.0, 1.0));
            out.started = true;
            out.ended = true;
        }
        if over {
            let dy = juce_wheel_delta_y(ui);
            if dy != 0.0 {
                consume_wheel(ui);
                let v = (value + dy * 0.25).clamp(0.0, 1.0);
                if (v - value).abs() > 1e-6 {
                    out.value = Some(v);
                    out.started = true;
                    out.ended = true;
                }
            }
        }
    }
    let dragging = ui.data(|d| d.get_temp::<Drag>(key)).is_some();
    let shown = match (out.value, f.positions) {
        (Some(v), n) if n > 1 => (v * (n - 1) as f32).round() / (n - 1) as f32,
        (Some(v), _) => v,
        (None, _) => value,
    };
    paint_knob(ui.painter(), r, f, shown, if over || dragging { Some(readout) } else { None });
    out
}

/// `VintKnob::paint` (its angle literals verbatim).
#[allow(clippy::approx_constant)]
pub(crate) fn paint_knob(g: &Painter, b: Rect, f: &KnobFace, value: f32, readout: Option<&str>) {
    let w = b.width();
    let r = radius_for(w, f.style, f.dots);
    let legend_r = legend_radius_for(w, f.style, f.dots);
    let cx = b.center().x;
    let cy = b.top() + centre_offset(w, f.style, f.dots, f.name_below);

    let mut sweep = 4.712389f32;
    if f.positions > 1 {
        sweep = 5.235988f32.min(0.5235988 * (f.positions - 1) as f32);
    }
    let mut start = -0.5 * sweep;
    if let Some((s, sw)) = f.travel {
        sweep = sw.to_radians();
        start = s.to_radians();
    }
    let angle_of = |pos: f32| start + sweep * pos;
    let at = |a: f32, rad: f32| pos2(cx + a.sin() * rad, cy - a.cos() * rad);

    // name
    let name_row = if f.name_below {
        Rect::from_min_size(pos2(b.left(), cy + legend_r + 2.0), vec2(w, 15.0))
    } else {
        Rect::from_min_size(b.min, vec2(w, 15.0))
    };
    if !f.name.is_empty() {
        text_c(g, name_row, &f.name.to_uppercase(), 11.5, f.ink);
    }

    // the printed scale; the figure the knob is at, lit
    let mut lit_index = None;
    if !f.marks.is_empty() {
        let mut best = 1.0e9f32;
        let mut bi = 0;
        for (i, m) in f.marks.iter().enumerate() {
            let dd = (m.pos - value).abs();
            if dd < best {
                best = dd;
                bi = i;
            }
        }
        let tol = if f.positions > 1 { 0.5 / (f.positions - 1) as f32 } else { 0.03 };
        if best <= tol {
            lit_index = Some(bi);
        }
    }
    if f.minor > 0 && f.style != FaceStyle::Chicken {
        let c = if f.dots { f.ink } else { f.ink.alpha(f.ink.a * 0.45) };
        for i in 0..=f.minor {
            let a = angle_of(i as f32 / f.minor as f32);
            if f.dots {
                fill_ellipse(g, Rect::from_center_size(at(a, r + 5.0), vec2(4.6, 4.6)), Fill::Solid(c));
            } else {
                line(g, at(a, r + 1.5), at(a, r + 4.0), 1.0, c);
            }
        }
    } else if f.dots {
        for m in f.marks {
            fill_ellipse(g, Rect::from_center_size(at(angle_of(m.pos), r + 5.0), vec2(4.6, 4.6)), Fill::Solid(f.ink));
        }
    }
    let size = if f.dots { 11.0 } else { 12.0 };
    for (i, m) in f.marks.iter().enumerate() {
        let p = at(angle_of(m.pos), legend_r);
        text_c(g, Rect::from_center_size(p, vec2(44.0, 16.0)), m.text, size, if lit_index == Some(i) { LIT } else { f.ink });
    }

    let a = angle_of(value);
    let c = pos2(cx, cy);
    match f.style {
        FaceStyle::Fluted => paint_fluted(g, c, r, a, f.cap),
        FaceStyle::Chicken => paint_chicken(g, c, r, a, f.cap),
        FaceStyle::Knurled => paint_knurled(g, c, r, a, f.cap),
        FaceStyle::Cream => paint_cream(g, c, r, a),
        FaceStyle::Wing => paint_wing(g, c, r, a, f.cap),
        FaceStyle::ApiGain => paint_api_gain(g, c, r, a),
        FaceStyle::ApiFreq => paint_api_freq(g, c, r, a, f.cap),
        FaceStyle::SslCap => paint_ssl_cap(g, c, r, a, f.cap),
        FaceStyle::Round => paint_round(g, c, r, a, f.cap),
        FaceStyle::Silver => paint_silver(g, c, r, a, f.cap),
    }

    // the readout, while it is being looked at
    if let Some(t) = readout {
        let tr = Rect::from_min_size(pos2(cx - 38.0, cy - 8.0), vec2(76.0, 16.0));
        fill_round_rect(g, tr, 4.0, Fill::Solid(Jc::BLACK.alpha(0.72)));
        text_c(g, tr, t, 11.0, Jc::WHITE);
    }
}

fn drop_shadow(g: &Painter, centre: Pos2, pts: &[Pos2], dx: f32, dy: f32) {
    fill_star(g, centre + vec2(dx, dy), &translate(pts, dx, dy), Fill::Solid(Jc::BLACK.alpha(0.45)));
}

fn polar(c: Pos2, n: usize, rr: impl Fn(f32) -> f32) -> Vec<Pos2> {
    (0..n).map(|i| TAU * i as f32 / n as f32).map(|t| pos2(c.x + t.sin() * rr(t), c.y - t.cos() * rr(t))).collect()
}

fn paint_fluted(g: &Painter, c: Pos2, r: f32, a: f32, cap: Jc) {
    // the scalloped skirt: nine lobes, turning with the knob
    let skirt = polar(c, 144, |t| r * (1.0 - 0.075 * (0.5 + 0.5 * (9.0 * (t - a)).cos())));
    drop_shadow(g, c, &skirt, 2.0, 4.0);
    fill_star(g, c, &skirt, Fill::radial(cap.brighter(0.55), pos2(c.x - r * 0.5, c.y - r * 0.7), cap.darker(0.6), pos2(c.x + r * 0.6, c.y + r)));
    stroke_closed(g, &skirt, 1.0, Jc::WHITE.alpha(0.10));
    // the raised centre
    let cr = r * 0.62;
    fill_ellipse(
        g,
        Rect::from_center_size(c, vec2(2.0 * cr, 2.0 * cr)),
        Fill::radial(cap.brighter(0.9), pos2(c.x - cr * 0.4, c.y - cr * 0.6), cap.darker(0.3), pos2(c.x + cr * 0.4, c.y + cr)),
    );
    stroke_closed(g, &ellipse(c, cr, cr, 48), 1.2, Jc::BLACK.alpha(0.55));
    stroke_closed(g, &ellipse(c, cr - 1.5, cr - 1.5, 48), 1.0, Jc::WHITE.alpha(0.08));
    // the index line on the skirt
    let p0 = pos2(c.x + a.sin() * r * 0.66, c.y - a.cos() * r * 0.66);
    let p1 = pos2(c.x + a.sin() * r * 0.97, c.y - a.cos() * r * 0.97);
    line(g, p0, p1, (r * 0.07).max(2.2), Jc::WHITE.alpha(0.95));
}

fn paint_chicken(g: &Painter, c: Pos2, r: f32, a: f32, cap: Jc) {
    // a tapered bar: broad at the tail, narrowing to the point it points with
    let (tail, tip, w_base, w_tip) = (r * 0.55, r * 1.12, r * 0.36, r * 0.17);
    let mut bar = vec![pos2(-w_base, tail), pos2(-w_tip, -tip)];
    quad(&mut bar, pos2(-w_tip, -tip), pos2(0.0, -tip - w_tip * 0.9), pos2(w_tip, -tip));
    bar.push(pos2(w_base, tail));
    quad(&mut bar, pos2(w_base, tail), pos2(0.0, tail + w_base * 0.9), pos2(-w_base, tail));
    rotate(&mut bar, a, c);
    let hub = ellipse(c, r * 0.52, r * 0.52, 40);
    drop_shadow(g, c, &hub, 2.0, 4.0);
    drop_shadow(g, c, &bar, 3.0, 5.0);
    fill_star(g, c, &hub, Fill::Solid(cap.darker(0.4)));
    fill_star(g, c, &bar, Fill::linear(cap.brighter(0.7), pos2(c.x - r * 0.5, c.y - r), cap.darker(0.5), pos2(c.x + r * 0.5, c.y + r)));
    stroke_closed(g, &bar, 1.0, Jc::WHITE.alpha(0.18));
    // the ridge along its length
    let mut ridge = [pos2(0.0, tail * 0.6), pos2(0.0, -tip * 0.8)];
    rotate(&mut ridge, a, c);
    line(g, ridge[0], ridge[1], 1.4, Jc::WHITE.alpha(0.22));
}

/// A clear or smoked plastic skirt with a chrome edge.
fn paint_skirt(g: &Painter, c: Pos2, r: f32, tint: Jc) {
    let s = ellipse(c, r, r, 48);
    drop_shadow(g, c, &s, 1.5, 3.0);
    fill_star(g, c, &s, Fill::linear(tint.brighter(0.4), pos2(c.x - r, c.y - r), tint.darker(0.5), pos2(c.x + r, c.y + r)));
    stroke_closed(g, &s, 1.2, Jc::WHITE.alpha(0.35));
}

fn paint_knurled(g: &Painter, c: Pos2, r: f32, a: f32, cap: Jc) {
    paint_skirt(g, c, r, Jc::argb(0xFF2B3238));
    // the knurled body: a toothed edge that turns with the knob
    let br = r * 0.74;
    let body = polar(c, 224, |t| br * (1.0 - 0.045 * if (28.0 * (t - a)).cos() > 0.0 { 1.0 } else { 0.0 }));
    drop_shadow(g, c, &body, 1.5, 2.5);
    fill_star(g, c, &body, Fill::linear(cap.brighter(0.35), pos2(c.x - br, c.y - br), cap.darker(0.45), pos2(c.x + br, c.y + br)));
    stroke_closed(g, &body, 0.8, Jc::BLACK.alpha(0.3));
    // the pale flat cap
    let cr = br * 0.72;
    fill_ellipse(
        g,
        Rect::from_center_size(c, vec2(2.0 * cr, 2.0 * cr)),
        Fill::linear(Jc::argb(0xFFE4E7EA), pos2(c.x - cr, c.y - cr), Jc::argb(0xFFB7BCC1), pos2(c.x + cr, c.y + cr)),
    );
    stroke_closed(g, &ellipse(c, cr, cr, 48), 1.0, Jc::BLACK.alpha(0.25));
    // the index: a white line across the body's edge
    line(
        g,
        pos2(c.x + a.sin() * br * 0.74, c.y - a.cos() * br * 0.74),
        pos2(c.x + a.sin() * r * 0.98, c.y - a.cos() * r * 0.98),
        (r * 0.07).max(2.0),
        Jc::WHITE,
    );
}

fn paint_cream(g: &Painter, c: Pos2, r: f32, a: f32) {
    let s = ellipse(c, r, r, 48);
    drop_shadow(g, c, &s, 1.5, 3.5);
    fill_star(g, c, &s, Fill::radial(Jc::argb(0xFFFBF6E8), pos2(c.x - r * 0.6, c.y - r * 0.8), Jc::argb(0xFFCFC6AE), pos2(c.x + r, c.y + r)));
    stroke_closed(g, &s, 1.0, Jc::BLACK.alpha(0.25));
    let cr = r * 0.62;
    fill_ellipse(
        g,
        Rect::from_center_size(c, vec2(2.0 * cr, 2.0 * cr)),
        Fill::linear(Jc::argb(0xFFF4EEDD), pos2(c.x - cr, c.y - cr), Jc::argb(0xFFD8D0B9), pos2(c.x + cr, c.y + cr)),
    );
    stroke_closed(g, &ellipse(c, cr, cr, 48), 1.0, Jc::BLACK.alpha(0.12));
    // the index: a dark engraved line on the skirt
    line(
        g,
        pos2(c.x + a.sin() * r * 0.66, c.y - a.cos() * r * 0.66),
        pos2(c.x + a.sin() * r * 0.98, c.y - a.cos() * r * 0.98),
        (r * 0.06).max(2.0),
        Jc::argb(0xFF3A3326),
    );
}

fn paint_wing(g: &Painter, c: Pos2, r: f32, a: f32, cap: Jc) {
    paint_skirt(g, c, r, Jc::argb(0xFF3A4247));
    // the wing: a fin from the cap to the rim, pointing where the knob is set
    let cr = r * 0.6;
    let mut wing = vec![pos2(-cr * 0.55, 0.0), pos2(-cr * 0.22, -r * 0.98)];
    quad(&mut wing, pos2(-cr * 0.22, -r * 0.98), pos2(0.0, -r * 1.06), pos2(cr * 0.22, -r * 0.98));
    wing.push(pos2(cr * 0.55, 0.0));
    rotate(&mut wing, a, c);
    let mid = pos2(c.x + a.sin() * r * 0.5, c.y - a.cos() * r * 0.5);
    drop_shadow(g, mid, &wing, 2.0, 3.5);
    fill_star(g, mid, &wing, Fill::linear(cap.brighter(0.5), pos2(c.x - r, c.y - r), cap.darker(0.45), pos2(c.x + r, c.y + r)));
    let circ = ellipse(c, cr, cr, 48);
    drop_shadow(g, c, &circ, 1.5, 2.5);
    fill_star(g, c, &circ, Fill::linear(cap.brighter(0.45), pos2(c.x - cr, c.y - cr), cap.darker(0.35), pos2(c.x + cr, c.y + cr)));
    stroke_closed(g, &circ, 1.0, Jc::WHITE.alpha(0.28));
    line(
        g,
        pos2(c.x + a.sin() * cr * 1.05, c.y - a.cos() * cr * 1.05),
        pos2(c.x + a.sin() * r * 0.9, c.y - a.cos() * r * 0.9),
        (r * 0.05).max(1.6),
        Jc::WHITE.alpha(0.85),
    );
}

/// A rounded fin from (±base, root) out to the tip at `-tip` (each the C++'s path), rotated.
fn fin(base: f32, root: f32, narrow: f32, tip: f32, crest: f32, a: f32, c: Pos2) -> Vec<Pos2> {
    let mut f = vec![pos2(-base, root), pos2(-narrow, -tip)];
    quad(&mut f, pos2(-narrow, -tip), pos2(0.0, -crest), pos2(narrow, -tip));
    f.push(pos2(base, root));
    rotate(&mut f, a, c);
    f
}

fn paint_api_gain(g: &Painter, c: Pos2, r: f32, a: f32) {
    // a white body, round with a pointer fin, and a flat grey cap
    let br = r * 0.8;
    let body = ellipse(c, br, br, 48);
    let f = fin(br * 0.42, -br * 0.6, br * 0.2, r * 1.08, r * 1.16, a, c);
    let fc = pos2(c.x + a.sin() * br * 0.85, c.y - a.cos() * br * 0.85);
    drop_shadow(g, c, &body, 2.0, 3.5);
    drop_shadow(g, fc, &f, 2.0, 3.5);
    let fill = Fill::linear(Jc::argb(0xFFFBFBFA), pos2(c.x - r, c.y - r), Jc::argb(0xFFB9BCBF), pos2(c.x + r, c.y + r));
    fill_star(g, c, &body, fill);
    fill_star(g, fc, &f, fill);
    stroke_closed(g, &body, 1.0, Jc::BLACK.alpha(0.18));
    let cr = br * 0.66;
    fill_ellipse(
        g,
        Rect::from_center_size(c, vec2(2.0 * cr, 2.0 * cr)),
        Fill::linear(Jc::argb(0xFFD9DBDD), pos2(c.x - cr, c.y - cr), Jc::argb(0xFFA3A7AB), pos2(c.x + cr, c.y + cr)),
    );
    stroke_closed(g, &ellipse(c, cr, cr, 48), 1.0, Jc::BLACK.alpha(0.2));
}

fn paint_api_freq(g: &Painter, c: Pos2, r: f32, a: f32, cap: Jc) {
    // the blue ring with its tab, round a dark hub
    let (ro, ri) = (r * 0.9, r * 0.6);
    let ring = ellipse(c, ro, ro, 48);
    let tab = fin(ro * 0.34, -ro * 0.8, ro * 0.18, r * 1.1, r * 1.16, a, c);
    let tc = pos2(c.x + a.sin() * ro * 0.95, c.y - a.cos() * ro * 0.95);
    drop_shadow(g, c, &ring, 2.0, 3.5);
    drop_shadow(g, tc, &tab, 2.0, 3.5);
    let fill = Fill::linear(cap.brighter(0.45), pos2(c.x - r, c.y - r), cap.darker(0.4), pos2(c.x + r, c.y + r));
    fill_star(g, c, &ring, fill);
    fill_star(g, tc, &tab, fill);
    fill_ellipse(
        g,
        Rect::from_center_size(c, vec2(2.0 * ri, 2.0 * ri)),
        Fill::linear(Jc::argb(0xFF2C2E31), pos2(c.x - ri, c.y - ri), Jc::argb(0xFF0C0D0E), pos2(c.x + ri, c.y + ri)),
    );
    line(
        g,
        pos2(c.x + a.sin() * ri * 0.3, c.y - a.cos() * ri * 0.3),
        pos2(c.x + a.sin() * ri * 0.95, c.y - a.cos() * ri * 0.95),
        (r * 0.05).max(1.8),
        Jc::WHITE.alpha(0.8),
    );
}

fn paint_ssl_cap(g: &Painter, c: Pos2, r: f32, a: f32, cap: Jc) {
    let cr = r * 0.9;
    let circ = ellipse(c, cr, cr, 48);
    drop_shadow(g, c, &circ, 2.5, 4.5);
    // the cap's side, then its flat top
    fill_star(g, c, &circ, Fill::Solid(cap.darker(0.55)));
    let tr = cr * 0.9;
    let tc = pos2(c.x, c.y - 1.0);
    fill_ellipse(
        g,
        Rect::from_center_size(tc, vec2(2.0 * tr, 2.0 * tr)),
        Fill::linear(cap.brighter(0.35), pos2(c.x - tr * 0.7, c.y - tr * 0.8), cap.darker(0.2), pos2(c.x + tr * 0.7, c.y + tr)),
    );
    stroke_closed(g, &ellipse(tc, tr, tr, 48), 1.0, Jc::WHITE.alpha(0.28));
    // the index bar, from the centre to the edge
    let pale = cap.perceived_brightness() > 0.6;
    line(
        g,
        tc,
        pos2(c.x + a.sin() * tr * 0.95, c.y - 1.0 - a.cos() * tr * 0.95),
        (r * 0.14).max(3.0),
        if pale { Jc::argb(0xFF151515) } else { Jc::WHITE },
    );
}

fn paint_round(g: &Painter, c: Pos2, r: f32, a: f32, cap: Jc) {
    let skirt = cap.darker(0.55);
    let s = ellipse(c, r, r, 48);
    drop_shadow(g, c, &s, 1.5, 3.0);
    fill_star(g, c, &s, Fill::Solid(skirt));
    stroke_closed(g, &s, 1.2, Jc::WHITE.alpha(if skirt.perceived_brightness() < 0.2 { 0.30 } else { 0.16 }));
    let cr = r * 0.74;
    fill_ellipse(
        g,
        Rect::from_center_size(c, vec2(2.0 * cr, 2.0 * cr)),
        Fill::linear(cap.brighter(0.25), pos2(c.x, c.y - cr), cap.darker(0.25), pos2(c.x, c.y + cr)),
    );
    stroke_closed(g, &ellipse(c, cr, cr, 48), 1.0, Jc::WHITE.alpha(0.18));
    line(
        g,
        pos2(c.x + a.sin() * r * 0.18, c.y - a.cos() * r * 0.18),
        pos2(c.x + a.sin() * r * 0.97, c.y - a.cos() * r * 0.97),
        (r * 0.09).max(2.0),
        Jc::WHITE.alpha(0.92),
    );
}

/// The BBE 882's knob: a black ribbed skirt, a silver cap with a dark pointer line.
fn paint_silver(g: &Painter, c: Pos2, r: f32, a: f32, cap: Jc) {
    let skirt = polar(c, 144, |t| r * (1.0 - 0.02 * (0.5 + 0.5 * (36.0 * (t - a)).cos())));
    drop_shadow(g, c, &skirt, 1.5, 3.5);
    fill_star(
        g,
        c,
        &skirt,
        Fill::radial(Jc::argb(0xFF3A3B3E), pos2(c.x - r * 0.4, c.y - r * 0.6), Jc::argb(0xFF0E0E10), pos2(c.x + r * 0.7, c.y + r)),
    );
    stroke_closed(g, &skirt, 1.0, Jc::WHITE.alpha(0.20));
    let at = |rad: f32| pos2(c.x + a.sin() * rad, c.y - a.cos() * rad);
    line(g, at(r * 0.70), at(r * 0.98), (r * 0.07).max(1.6), Jc::WHITE.alpha(0.95));
    let cr = r * 0.62;
    fill_ellipse(
        g,
        Rect::from_center_size(c, vec2(2.0 * cr, 2.0 * cr)),
        Fill::radial(cap.brighter(0.6), pos2(c.x - cr * 0.35, c.y - cr * 0.45), cap.darker(0.28), pos2(c.x + cr * 0.6, c.y + cr)),
    );
    stroke_closed(g, &ellipse(c, cr, cr, 48), 1.0, Jc::BLACK.alpha(0.55));
    line(g, at(r * 0.08), at(cr * 0.96), (r * 0.085).max(2.0), Jc::argb(0xFF18181A));
}

// ============================================================================ the other hardware

/// `BatToggle::paint`: up is on (`inverted`: up while the parameter is off).
pub(crate) fn paint_bat(g: &Painter, b: Rect, label: &str, on: bool, inverted: bool, ink: Jc) {
    let up = on != inverted;
    let (cx, cy) = (b.center().x, b.center().y + 6.0);
    let nut = b.width().min(b.height()) * 0.26;
    text_c(g, Rect::from_min_size(b.min, vec2(b.width(), 14.0)), label, 11.5, if up { LIT } else { ink });
    let c = pos2(cx, cy);
    let hex: Vec<Pos2> = (0..6).map(|i| std::f32::consts::PI / 3.0 * i as f32 + 0.52).map(|t| pos2(cx + t.cos() * nut, cy + t.sin() * nut)).collect();
    fill_star(g, c + vec2(1.5, 2.5), &translate(&hex, 1.5, 2.5), Fill::Solid(Jc::BLACK.alpha(0.4)));
    fill_star(g, c, &hex, Fill::linear(Jc::argb(0xFFD9DCDF), pos2(cx - nut, cy - nut), Jc::argb(0xFF6E7378), pos2(cx + nut, cy + nut)));
    // the bat, up or down
    let len = nut * 1.9;
    let dir = if up { -1.0 } else { 1.0 };
    let tip = pos2(cx, cy + dir * len);
    let bat = vec![pos2(cx - nut * 0.22, cy), pos2(tip.x - nut * 0.34, tip.y), pos2(tip.x + nut * 0.34, tip.y), pos2(cx + nut * 0.22, cy)];
    let mid = pos2(cx, cy + dir * len * 0.5);
    fill_star(g, mid + vec2(2.0, 3.0), &translate(&bat, 2.0, 3.0), Fill::Solid(Jc::BLACK.alpha(0.45)));
    fill_star(g, mid, &bat, Fill::linear(Jc::argb(0xFFF4F5F6), pos2(cx - nut * 0.4, cy), Jc::argb(0xFF8A8F94), pos2(cx + nut * 0.4, cy)));
    fill_ellipse(g, Rect::from_center_size(tip, vec2(nut * 0.72, nut * 0.72)), Fill::Solid(Jc::argb(0xFFEDEFF1)));
}

/// `PlateButton::paint`: pressed in and lit while "in".
pub(crate) fn paint_plate(g: &Painter, bounds: Rect, label: &str, on: bool, inverted: bool) {
    let inn = on != inverted;
    let mut b = bounds.shrink(2.0);
    fill_round_rect(g, b.translate(vec2(1.5, if inn { 1.0 } else { 3.0 })), 3.0, Fill::Solid(Jc::BLACK.alpha(0.5)));
    if inn {
        b = b.translate(vec2(0.0, 1.5));
    }
    let (top, bottom) = if inn { (Jc::argb(0xFFFFFBEA), Jc::argb(0xFFE8DDB8)) } else { (Jc::argb(0xFFF1F1EE), Jc::argb(0xFFC9CBCC)) };
    fill_round_rect(g, b, 3.0, Fill::linear(top, b.left_top(), bottom, b.left_bottom()));
    if inn {
        fill_round_rect(g, b.expand(3.0), 5.0, Fill::Solid(Jc::argb(0x33FFD66B)));
    }
    stroke_closed(g, &round_rect(b, 3.0), 1.0, Jc::BLACK.alpha(0.35));
    text_c(g, b, label, 13.0, Jc::argb(0xFF111111));
}

/// `DomeButton::paint`: a round white push-button, its name under it.
pub(crate) fn paint_dome(g: &Painter, b: Rect, label: &str, on: bool, inverted: bool, ink: Jc) {
    let inn = on != inverted;
    let dd = b.width().min(b.height() - 18.0) * 0.86;
    let dome = Rect::from_center_size(pos2(b.center().x, b.top() + dd * 0.5 + 3.0), vec2(dd, dd));
    fill_ellipse(g, dome.translate(vec2(1.5, if inn { 1.5 } else { 3.5 })), Fill::Solid(Jc::BLACK.alpha(0.55)));
    let face = if inn { dome.shrink(1.5).translate(vec2(0.0, 1.0)) } else { dome };
    let (c1, c2) = if inn { (Jc::argb(0xFFDCD8CC), Jc::argb(0xFFB3AFA3)) } else { (Jc::argb(0xFFFFFEF8), Jc::argb(0xFFD8D5CB)) };
    fill_ellipse(g, face, Fill::linear(c1, face.left_top(), c2, face.right_bottom()));
    stroke_closed(g, &ellipse(face.center(), face.width() * 0.5, face.height() * 0.5, 48), 1.0, Jc::BLACK.alpha(0.25));
    if !label.is_empty() {
        text_c(g, Rect::from_min_size(pos2(b.left(), dome.bottom() + 3.0), vec2(b.width(), 15.0)), label, 12.5, ink);
    }
}

/// `SslButton::paint`: a pale grey key and an LED that lights while on (green; red for BYP).
pub(crate) fn paint_ssl(g: &Painter, bounds: Rect, label: &str, on: bool, red: bool, sideways: bool) {
    let mut b = bounds;
    let (led, mut key);
    if sideways {
        let led_d = (b.width() * 0.45).min(12.0);
        let top = Rect::from_min_size(b.min, vec2(b.width(), led_d + 10.0));
        led = Rect::from_center_size(top.center(), vec2(led_d, led_d));
        b.min.y += led_d + 10.0;
        key = b.shrink2(vec2(2.0, 1.0));
    } else {
        let led_d = (b.height() * 0.45).min(12.0);
        let right = Rect::from_min_max(pos2(b.right() - led_d - 10.0, b.top()), b.max);
        led = Rect::from_center_size(right.center(), vec2(led_d, led_d));
        b.max.x -= led_d + 10.0;
        key = b.shrink2(vec2(1.0, 2.0));
    }
    let led_d = led.width();
    fill_round_rect(g, key.translate(vec2(1.5, 3.0)), 3.0, Fill::Solid(Jc::BLACK.alpha(0.6)));
    if on {
        key = key.translate(vec2(0.0, 1.0));
    }
    fill_round_rect(g, key, 3.0, Fill::linear(Jc::argb(0xFFE4E3DF), key.left_top(), Jc::argb(0xFFA9A8A4), key.left_bottom()));
    stroke_closed(g, &round_rect(key, 3.0), 1.0, Jc::BLACK.alpha(0.4));
    text_fitted(g, if sideways { key.shrink2(vec2(0.0, 4.0)) } else { key.shrink2(vec2(3.0, 0.0)) }, label, 13.0, Jc::argb(0xFF16181C), sideways);
    let lit = if red { Jc::argb(0xFFFF4030) } else { Jc::argb(0xFF6CFF4A) };
    if on {
        fill_ellipse(g, led.expand(led_d * 1.2), Fill::radial(lit.alpha(0.55), led.center(), lit.alpha(0.0), led.center() + vec2(led_d * 1.6, 0.0)));
    }
    let (c1, c2) = if on { (lit.brighter(0.5), lit.darker(0.3)) } else { (lit.mul_brightness(0.25), lit.mul_brightness(0.12)) };
    fill_ellipse(g, led, Fill::linear(c1, led.left_top(), c2, led.right_bottom()));
}

/// `VuMeter`: VU ballistics (300 ms), 0 VU at -18 dBFS.
pub(crate) fn vu_set_level(needle: &mut f32, rms_dbfs: f32, dt: f32) {
    let vu = rms_dbfs + 18.0;
    let k = 1.0 - (-dt / 0.065).exp();
    *needle += k * (vu.clamp(-22.0, 4.0) - *needle);
    if !needle.is_finite() {
        *needle = -22.0;
    }
}

/// `VuMeter::paint`.
pub(crate) fn paint_vu(g: &Painter, b: Rect, needle: f32) {
    fill_round_rect(g, b.translate(vec2(2.0, 3.0)), 5.0, Fill::Solid(Jc::BLACK.alpha(0.5)));
    fill_round_rect(g, b, 5.0, Fill::linear(Jc::argb(0xFF2A2B2D), b.left_top(), Jc::argb(0xFF0D0E0F), b.left_bottom()));
    let face = b.shrink2(vec2(b.width() * 0.06, b.height() * 0.1));
    fill_round_rect(g, face, 3.0, Fill::linear(Jc::argb(0xFFF3E9CF), face.center_top(), Jc::argb(0xFFD9C9A2), face.center_bottom()));

    const SWEEP: f32 = 0.72;
    let rr = (face.width() * 0.6).min((face.width() * 0.5 - 16.0) / SWEEP.sin() - 13.0);
    let pivot = pos2(face.center().x, face.top() + rr + 26.0);
    let angle_of = |vu: f32| {
        let t = (vu.clamp(-20.0, 3.0) + 20.0) / 23.0;
        -SWEEP + 2.0 * SWEEP * t.powf(1.7)
    };
    let at = |a: f32, rad: f32| pos2(pivot.x + a.sin() * rad, pivot.y - a.cos() * rad);
    let gc = g.with_clip_rect(face.intersect(g.clip_rect()));
    let arc =
        |from: f32, to: f32| -> Vec<Pos2> { (0..=32).map(|i| at(angle_of(from) + (angle_of(to) - angle_of(from)) * i as f32 / 32.0, rr)).collect() };
    gc.add(egui::Shape::line(arc(0.0, 3.0), egui::Stroke::new(4.0, Jc::argb(0xFFC0392B).c32())));
    gc.add(egui::Shape::line(arc(-20.0, 3.0), egui::Stroke::new(1.0, Jc::argb(0xFF2B2B2B).c32())));
    let mut placed: Vec<Pos2> = Vec::with_capacity(11);
    for pass in 0..2 {
        for vu in [-20.0f32, -10.0, -7.0, -5.0, -3.0, -2.0, -1.0, 0.0, 1.0, 2.0, 3.0] {
            let main = vu == -20.0 || vu == -10.0 || vu == -5.0 || vu == 0.0 || vu == 3.0;
            if main != (pass == 0) {
                continue;
            }
            let a = angle_of(vu);
            let col = if vu > 0.0 { Jc::argb(0xFFC0392B) } else { Jc::argb(0xFF2B2B2B) };
            line(&gc, at(a, rr), at(a, rr + 5.0), 1.0, col);
            let p = at(a, rr + 13.0);
            if !main && placed.iter().any(|q| (p.x - q.x).abs() < 12.0 && (p.y - q.y).abs() < 11.0) {
                continue;
            }
            placed.push(p);
            text_c(&gc, Rect::from_center_size(p, vec2(20.0, 12.0)), &format!("{}", vu.abs() as i32), 9.5, col);
        }
    }
    text_c(&gc, Rect::from_min_size(pos2(face.center().x - 15.0, face.bottom() - 22.0), vec2(30.0, 12.0)), "VU", 9.5, Jc::argb(0xFF2B2B2B));
    let a = angle_of(needle);
    line(&gc, at(a, rr * 0.62), at(a, rr + 8.0), 1.3, Jc::argb(0xFF1A1A1A));
    // the glass
    fill_round_rect(g, face, 3.0, Fill::linear(Jc::WHITE.alpha(0.16), face.left_top(), Jc::WHITE.alpha(0.0), pos2(face.left(), face.center().y)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_matches_the_cpp() {
        // A 125-unit Pultec knob at the editor's 0.952 scale: 119 px.
        let w = 119.0;
        assert!((legend_radius_for(w, FaceStyle::Fluted, false) - 46.5).abs() < 1e-4);
        assert!((radius_for(w, FaceStyle::Fluted, false) - 33.5).abs() < 1e-4);
        assert_eq!(height_for(w, FaceStyle::Fluted, false, false), 120.0);
        let mut n = -22.0;
        vu_set_level(&mut n, -18.0, 10.0);
        assert!(n.abs() < 1e-3, "0 VU at -18 dBFS");
        vu_set_level(&mut n, f32::NAN, 0.03);
        assert!(n.is_finite());
    }
}
