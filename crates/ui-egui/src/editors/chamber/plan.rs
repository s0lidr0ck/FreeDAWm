//! `ChamberPlan.h`, translated: the floor plan (`drawRoom()`, `layout()` and the pointer handling
//! of `ChamberMadeUI.js`) — the drafting grid, the floor and walls in their materials, the
//! dimension labels and the two resize handles, the RT60 / volume / flutter readouts, the sound
//! paths, the objects with their glyphs, the speaker with its cone and the microphones with their
//! aim handles, and the Add row — and `ChamberEcho` (the echogram in More).
//!
//! Everything drawn comes out of the engine's geometry (`build_chamber_for`, the call the DSP
//! makes), cached by the parameter values (`chamber_plan::chamber_for`).

use super::controls::{cm, text_rotated, text_width, ui_font};
use super::{obj_rotate_param, obj_size_param, obj_type_param, obj_x_param, obj_y_param};
use crate::editor_kit::palette::{al, argb};
use crate::theme::{mono, mono_bold};
use egui::{Align2, Color32, CornerRadius, Mesh, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};
use soundcraft_dsp::plugins::fs::chamber::chamber_geometry::{
    self as geo, Chamber, OBJECT_SPECS, ObjectState, PI, TAP_DIRECT, TAP_OBJECT, TAP_SECOND, TAPS_PER_MIC,
};
use soundcraft_dsp::plugins::fs::chamber::chamber_made_dsp as d;

/// `selRoom`, `selSpeaker`, `selMicA`, `selMicB`, `selObject0`.
pub(crate) const SEL_ROOM: i32 = -1;
pub(crate) const SEL_SPEAKER: i32 = 0;
pub(crate) const SEL_MIC_A: i32 = 1;
pub(crate) const SEL_MIC_B: i32 = 2;
pub(crate) const SEL_OBJECT0: i32 = 3;

/// `plan::`, the Amorph build's palette.
mod pc {
    use super::{Color32, argb};
    pub const INK: Color32 = argb(0xFFD7DBE0);
    pub const DIM: Color32 = argb(0xFF8A929C);
    pub const FAINT: Color32 = argb(0xFF5D6670);
    pub const BAR: Color32 = argb(0xFF1B1E22);
    pub const BG: Color32 = argb(0xFF15181C);
    pub const SPK: Color32 = argb(0xFFF2F4F7);
    pub const CONE: Color32 = argb(0xFFFFC857);
    pub const MIC_A: Color32 = argb(0xFF4FC3F7);
    pub const MIC_B: Color32 = argb(0xFFFF6B6B);
    pub const OBJ: Color32 = argb(0xFFCFD4DA);
}

/// `plan::material`: `MAT[].c`.
pub(crate) fn material(m: usize) -> Color32 {
    const C: [u32; 15] = [
        0xFFB9C2C9, 0xFFE6E1D3, 0xFF9FD8E6, 0xFFC9714F, 0xFF8F958E, 0xFF7FA0C4, 0xFFC8F2FF, 0xFFC89A5B, 0xFFA87942, 0xFFD5D5CF, 0xFF7D6248,
        0xFF8C6FB0, 0xFFA0527A, 0xFF55606B, 0xFF9AA3A0,
    ];
    argb(C[m.min(14)])
}

/// `plan::shortName`.
pub(crate) fn short_name(t: i32) -> &'static str {
    const N: [&str; 9] = ["", "Pillar", "Pot", "Diffuser", "Drape", "Bass Trap", "Barrel", "Sofa", "Glass Gobo"];
    N[t.clamp(0, 8) as usize]
}

/// `plan::tip`.
pub(crate) fn tip(t: i32) -> &'static str {
    const T: [&str; 9] = [
        "",
        "Hard scatterer. Breaks up flutter and thickens the tail.",
        "Helmholtz absorber. Pulls one low band out of the decay; size sets the pitch.",
        "Strongest scatterer. Smooths the tail without shortening it much.",
        "Soaks up mids and highs. Shortens and darkens the room.",
        "Soaks up lows. Tightens a boomy room.",
        "Scatters, and rings along with the room at its own pitch.",
        "Broadband absorber. One sofa tames a small room.",
        "Reflective screen. Blocks the direct path, keeps the room bright.",
    ];
    T[t.clamp(0, 8) as usize]
}

/// `plan::resonanceHz`.
pub(crate) fn resonance_hz(o: &ObjectState) -> f32 {
    if !o.on {
        return 0.0;
    }
    let spec = &OBJECT_SPECS[o.ty.clamp(0, OBJECT_SPECS.len() as i32 - 1) as usize];
    if spec.resonator != 0 { spec.res_f0_times_radius / o.radius.max(0.01) } else { 0.0 }
}

fn db(g: f32) -> f32 {
    if g > 1.0e-6 { 20.0 * g.log10() } else { -120.0 }
}

/// `lengthString`.
pub(crate) fn length_string(metres: f32, bare: bool, metric: bool) -> String {
    let x = if metric { metres } else { metres * 3.280_84 };
    let n = if x < 100.0 { format!("{x:.1}") } else { format!("{x:.0}") };
    if bare { n } else { format!("{n}{}", if metric { " m" } else { " ft" }) }
}

/// `cubicFeetString`.
pub(crate) fn cubic_string(m3: f32, metric: bool) -> String {
    if metric {
        return format!("{} m\u{b3}", (m3 + 0.5).floor() as i64);
    }
    let v = (m3 * 3.280_84f32.powi(3) + 0.5).floor() as i64;
    let s = v.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) && ch.is_ascii_digit() {
            out.push(',');
        }
        out.push(ch);
    }
    format!("{out} ft\u{b3}")
}

/// What a hit target is (`Kind`), in pick order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    SpkAim,
    MicAim,
    ObjRot,
    Spk,
    Mic,
    Obj,
    DimW,
    DimL,
}

#[derive(Clone, Copy, Debug)]
struct Hit {
    kind: Kind,
    k: usize,
    at: Pos2,
    r: f32,
}

/// `layout()`: room metres to the screen, turned on its side when that is bigger.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Layout {
    sc: f32,
    ox: f32,
    oy: f32,
    rot: bool,
}

impl Layout {
    fn p(&self, x: f32, y: f32) -> Pos2 {
        if self.rot { pos2(self.ox + y * self.sc, self.oy + x * self.sc) } else { pos2(self.ox + x * self.sc, self.oy - y * self.sc) }
    }
    fn d(&self, vx: f32, vy: f32) -> egui::Vec2 {
        if self.rot { vec2(vy, vx) } else { vec2(vx, -vy) }
    }
    fn inv(&self, px: f32, py: f32) -> egui::Vec2 {
        if self.rot { vec2((py - self.oy) / self.sc, (px - self.ox) / self.sc) } else { vec2((px - self.ox) / self.sc, (self.oy - py) / self.sc) }
    }
    fn inv_d(&self, dx: f32, dy: f32) -> egui::Vec2 {
        if self.rot { vec2(dy, dx) } else { vec2(dx, -dy) }
    }
}

/// A drag in progress (`Drag`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Drag {
    kind: Kind,
    k: usize,
    dx: f32,
    dy: f32,
    off: egui::Vec2,
    frozen: Option<Layout>,
}

/// The plan's state, kept by the editor.
#[derive(Clone, Debug, Default)]
pub(crate) struct PlanState {
    pub drag: Option<Drag>,
    pub add_warn: bool,
}

/// What the plan asks of the editor this frame.
#[derive(Debug, Default)]
pub(crate) struct PlanOut {
    pub select: Option<i32>,
    pub gesture_start: bool,
    pub gesture_end: bool,
    pub add_object: Option<i32>,
    /// Parameter changes of the drag (or the wheel), by index.
    pub set: Vec<(usize, f32)>,
}

/// `layout()` for the plan `r` and room `ch`; `compact` (the slim editor): the readouts on two
/// lines and the Add row in two.
fn layout(r: Rect, ch: &Chamber, compact: bool) -> Layout {
    let rm = &ch.room;
    let (w, h) = (r.width(), r.height());
    let (pad, pad_b) = if compact { (44.0, 78.0) } else { (36.0, 66.0) };
    let avail_h = h - pad - pad_b;
    let wm = rm.width_m.max(0.1);
    let lm = rm.length_m.max(0.1);
    let side = if compact { 34.0 } else { pad };
    let s0 = ((w - 2.0 * side) / wm).min(avail_h / lm);
    let s1 = ((w - 2.0 * side) / lm).min(avail_h / wm);
    let rot = s1 > s0 * 1.02;
    let sc = (if rot { s1 } else { s0 }).max(0.01);
    let ox = r.left() + (w - if rot { lm } else { wm } * sc) * 0.5;
    let mid_y = r.top() + pad + avail_h * 0.5;
    let oy = if rot { mid_y - wm * sc * 0.5 } else { mid_y + lm * sc * 0.5 };
    Layout { sc, ox, oy, rot }
}

/// `addRowArea`.
fn add_row_area(r: Rect) -> Rect {
    let b = r.shrink2(vec2(12.0, 0.0));
    Rect::from_min_max(pos2(b.left(), b.bottom() - 38.0), pos2(b.right(), b.bottom() - 10.0))
}

/// The compact Add rows: two rows of four, after a column for "Add" and the counter.
fn add_area_compact(r: Rect) -> Rect {
    let b = r.shrink2(vec2(8.0, 0.0));
    Rect::from_min_max(pos2(b.left(), b.bottom() - 62.0), pos2(b.right(), b.bottom() - 8.0))
}

/// The Add buttons (`resized`), or two rows of four when `compact`.
fn add_buttons(p: &Painter, r: Rect, compact: bool) -> [Rect; 8] {
    if compact {
        let a = add_area_compact(r);
        let x0 = a.left() + 34.0;
        let bw = ((a.right() - x0 - 3.0 * 4.0) / 4.0).floor();
        let bh = (a.height() - 4.0) / 2.0;
        let mut out = [Rect::NOTHING; 8];
        for (t, o) in out.iter_mut().enumerate() {
            let (row, col) = ((t / 4) as f32, (t % 4) as f32);
            *o = Rect::from_min_size(pos2(x0 + col * (bw + 4.0), a.top() + row * (bh + 4.0)), vec2(bw, bh));
        }
        return out;
    }
    let row = add_row_area(r);
    let mut x = row.left() + 36.0;
    let mut out = [Rect::NOTHING; 8];
    for (t, o) in out.iter_mut().enumerate() {
        let w = text_width(p, short_name(t as i32 + 1), ui_font(12.5, false)) + 22.0;
        *o = Rect::from_min_size(pos2(x, row.top()), vec2(w, row.height()));
        x += w + 6.0;
    }
    out
}

/// Fills a polygon star-shaped from its centroid (the room's outline).
fn fill_poly(p: &Painter, pts: &[Pos2], c: Color32) {
    if pts.len() < 3 {
        return;
    }
    let n = pts.len() as f32;
    let centre = pos2(pts.iter().map(|q| q.x).sum::<f32>() / n, pts.iter().map(|q| q.y).sum::<f32>() / n);
    let mut mesh = Mesh::default();
    mesh.colored_vertex(centre, c);
    for q in pts {
        mesh.colored_vertex(*q, c);
    }
    let m = pts.len() as u32;
    for i in 0..m {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % m);
    }
    p.add(Shape::mesh(mesh));
}

fn ellipse_pts(c: Pos2, r: f32) -> Vec<Pos2> {
    (0..36).map(|i| std::f32::consts::TAU * i as f32 / 36.0).map(|t| pos2(c.x + t.cos() * r, c.y + t.sin() * r)).collect()
}

/// `addRoundedRectangle`'s outline, in local coordinates.
fn round_box(w: f32, hh: f32, rad: f32) -> Vec<Pos2> {
    let c = rad.min(w).min(hh).max(0.0);
    let mut out = Vec::new();
    for (x, y, a0) in [(w - c, -hh + c, -90.0f32), (w - c, hh - c, 0.0), (-w + c, hh - c, 90.0), (-w + c, -hh + c, 180.0)] {
        for k in 0..=6 {
            let a = (a0 + 15.0 * k as f32).to_radians();
            out.push(pos2(x + a.cos() * c, y + a.sin() * c));
        }
    }
    out
}

/// `drawGlyph`: one glyph per type, in its own colours; `tf` places local points.
fn draw_glyph(p: &Painter, ty: i32, r: f32, tf: &dyn Fn(Pos2) -> Pos2) {
    let map = |pts: Vec<Pos2>| -> Vec<Pos2> { pts.into_iter().map(tf).collect() };
    let circle = |rr: f32| map(ellipse_pts(Pos2::ZERO, rr));
    let fill_stroke = |pts: &[Pos2], fill: u32, line: u32| {
        fill_poly(p, pts, argb(fill));
        p.add(Shape::closed_line(pts.to_vec(), Stroke::new(1.5, argb(line))));
    };
    let stroke = |c: u32| Stroke::new(1.5, argb(c));
    match ty {
        1 => {
            fill_stroke(&circle(r), 0xFF4A525B, 0xFF9AA3AD);
            p.line_segment([tf(pos2(-r * 0.6, -r * 0.6)), tf(pos2(r * 0.6, r * 0.6))], stroke(0xFF9AA3AD));
            p.line_segment([tf(pos2(r * 0.6, -r * 0.6)), tf(pos2(-r * 0.6, r * 0.6))], stroke(0xFF9AA3AD));
        }
        2 => {
            fill_stroke(&circle(r), 0xFFC9714F, 0xFFF0B79C);
            fill_poly(p, &circle(r * 0.42), argb(0xFF3A1D12));
        }
        3 => {
            let hh = (r * 0.3).max(5.0);
            fill_stroke(&map(round_box(r, hh, 2.0)), 0xFFA87942, 0xFFE3BF8C);
            for i in -3..=3 {
                let x = i as f32 * r / 4.0;
                p.line_segment([tf(pos2(x, -hh)), tf(pos2(x, hh))], stroke(0xFFE3BF8C));
            }
        }
        4 => {
            let pts: Vec<Pos2> = (0..=24).map(|i| tf(pos2(-r + 2.0 * r * i as f32 / 24.0, (i as f32 * 1.3).sin() * 3.5))).collect();
            p.add(Shape::line(pts, Stroke::new(5.0, argb(0xFFA0527A))));
        }
        5 => fill_stroke(&map(vec![pos2(-r, r * 0.8), pos2(r, r * 0.8), pos2(0.0, -r * 0.9)]), 0xFF3D4854, 0xFF8696A5),
        6 => {
            fill_stroke(&circle(r), 0xFF4F6B88, 0xFFA9C4DE);
            p.add(Shape::closed_line(circle(r * 0.66), stroke(0xFFA9C4DE)));
            fill_poly(p, &circle(2.0), argb(0xFFA9C4DE));
        }
        7 => {
            fill_stroke(&map(round_box(r, r * 0.5, 5.0)), 0xFF6D5A86, 0xFFB7A3D3);
            p.add(Shape::closed_line(map(round_box(r * 0.8, r * 0.25, 3.0)), stroke(0xFFB7A3D3)));
        }
        _ => fill_stroke(&map(round_box(r, 3.0, 1.0)), 0x8CC8F2FF, 0xFFC8F2FF),
    }
}

/// Paints the plan in `r` and handles its pointer (`mouseDown`, `mouseDrag`, `mouseUp`,
/// `mouseMove`, `mouseWheelMove`). `blocked`: something lies over it (the More card).
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan(
    ui: &mut egui::Ui,
    r: Rect,
    ch: &Chamber,
    st: &mut PlanState,
    selected: i32,
    rays: bool,
    metric: bool,
    flutter: f32,
    knobs: &[f32],
    blocked: bool,
    compact: bool,
) -> PlanOut {
    let mut out = PlanOut::default();
    let p = ui.painter().clone();
    super::controls::draw_panel(&p, r, cm::PANEL, cm::PANEL_LINE, 6.0);
    let lay = st.drag.and_then(|d| d.frozen).unwrap_or_else(|| layout(r, ch, compact));
    let mut hits: Vec<Hit> = Vec::with_capacity(24);
    let pv = |i: usize| knobs.get(i).copied().unwrap_or(0.0);

    // ---- drawGridAndRoom
    let rm = &ch.room;
    let u_m = if metric { 1.0 } else { 0.3048 };
    let px_u = lay.sc * u_m;
    let step_u = if px_u >= 18.0 {
        1
    } else if px_u >= 9.0 {
        2
    } else if px_u >= 3.6 {
        5
    } else if px_u >= 1.8 {
        10
    } else {
        25
    };
    let step_m = step_u as f32 * u_m;
    let grid = Stroke::new(1.0, al(Color32::WHITE, 0.045));
    let pc_ = p.with_clip_rect(r);
    let mut x = 0.0;
    let mut guard = 0;
    while x <= rm.width_m + 1.0e-4 && guard < 2000 {
        pc_.line_segment([lay.p(x, 0.0), lay.p(x, rm.length_m)], grid);
        x += step_m;
        guard += 1;
    }
    let mut y = 0.0;
    while y <= rm.length_m + 1.0e-4 && guard < 4000 {
        pc_.line_segment([lay.p(0.0, y), lay.p(rm.width_m, y)], grid);
        y += step_m;
        guard += 1;
    }
    let outline: Vec<Pos2> = (0..rm.num_walls.min(geo::MAX_WALLS)).map(|i| lay.p(rm.vx[i], rm.vy[i])).collect();
    let wall_mat = pv(d::P_WALL_MAT).round().max(0.0) as usize;
    let floor_mat = pv(d::P_FLOOR_MAT).round().max(0.0) as usize;
    fill_poly(&pc_, &outline, argb(0xFF1D2126));
    fill_poly(&pc_, &outline, al(material(floor_mat), 0.07));
    if outline.len() >= 2 {
        pc_.add(Shape::closed_line(outline.clone(), Stroke::new(3.0, al(material(wall_mat), 0.9))));
    }
    // dimensions, and the two square handles that resize the room
    let label = |text: String, rx: f32, ry: f32, dx: f32, dy: f32, off: f32| {
        let q = lay.p(rx, ry);
        let dd = lay.d(dx, dy);
        let c = pos2(q.x + dd.x * off, q.y + dd.y * off);
        let angle = if dd.x.abs() > dd.y.abs() { std::f32::consts::FRAC_PI_2 } else { 0.0 };
        text_rotated(&pc_, c, &text, ui_font(10.5, false), pc::FAINT, angle);
    };
    label(length_string(rm.width_m, false, metric), rm.width_m * 0.5, 0.0, 0.0, -1.0, 16.0);
    label(length_string(rm.length_m, false, metric), rm.width_m, rm.length_m * 0.5, 1.0, 0.0, 30.0);
    let mut handle = |c: Pos2, kind: Kind| {
        let hr = Rect::from_center_size(c, vec2(9.0, 9.0));
        pc_.rect_filled(hr, CornerRadius::ZERO, pc::BAR);
        pc_.rect_stroke(hr, CornerRadius::ZERO, Stroke::new(1.0, pc::DIM), egui::StrokeKind::Inside);
        hits.push(Hit { kind, k: 0, at: c, r: 12.0 });
    };
    {
        let q = lay.p(rm.width_m, rm.length_m * 0.5);
        let dd = lay.d(1.0, 0.0);
        handle(q + dd * 12.0, Kind::DimW);
        let q = lay.p(rm.width_m * 0.5, rm.length_m);
        let dd = lay.d(0.0, 1.0);
        handle(q + dd * 12.0, Kind::DimL);
    }

    // ---- drawReadouts
    {
        let ac = &ch.acoustics;
        let f1 = |x: f32| if x < 10.0 { format!("{x:.1}") } else { format!("{x:.0}") };
        let rt = format!("RT60 {} / {} / {} s", f1(ac.rt60.lo), f1(ac.rt60.mid), f1(ac.rt60.hi));
        let mut rest = format!("\u{b7} {}", cubic_string(ac.volume, metric));
        if flutter > 0.12 {
            rest.push_str(&format!(" \u{b7} flutter {}", if flutter > 0.35 { "strong" } else { "some" }));
        }
        let grid_text = format!("{step_u}{} grid", if metric { " m" } else { " ft" });
        if compact {
            // two lines: RT60 at the right (the display's toggle sits at the left), then the
            // volume (and flutter) with the grid at the right
            let f = mono(10.5);
            p.text(pos2(r.right() - 10.0, r.top() + 14.0), Align2::RIGHT_CENTER, &rt, f.clone(), pc::INK);
            let rest = rest.trim_start_matches('\u{b7}').trim_start().to_string();
            p.text(pos2(r.left() + 10.0, r.top() + 30.0), Align2::LEFT_CENTER, rest, f.clone(), pc::FAINT);
            p.text(pos2(r.right() - 10.0, r.top() + 30.0), Align2::RIGHT_CENTER, grid_text, f, pc::FAINT);
        } else {
            let w = text_width(&p, &rt, mono(12.0));
            p.text(pos2(r.left() + 16.0, r.top() + 18.0), Align2::LEFT_CENTER, &rt, mono(12.0), pc::INK);
            p.text(pos2(r.left() + 26.0 + w, r.top() + 18.0), Align2::LEFT_CENTER, rest, mono(12.0), pc::FAINT);
            p.text(pos2(r.right() - 16.0, r.top() + 18.0), Align2::RIGHT_CENTER, grid_text, mono(12.0), pc::FAINT);
        }
    }

    // ---- drawRays
    let pl = &ch.placement;
    if rays {
        for mi in 0..2 {
            let mic = if mi == 1 { pl.mic_b } else { pl.mic_a };
            for i in TAP_DIRECT..TAP_SECOND {
                let Some(t) = ch.taps.get(mi * TAPS_PER_MIC + i) else { continue };
                let gm = t.gain.mid.abs();
                if !t.live || gm < 1.0e-4 {
                    continue;
                }
                let mut pts = vec![lay.p(pl.speaker.x, pl.speaker.y)];
                if i != TAP_DIRECT {
                    if !t.has_point {
                        continue;
                    }
                    pts.push(lay.p(t.reflection_point.x, t.reflection_point.y));
                }
                pts.push(lay.p(mic.x, mic.y));
                let col = al(if mi == 1 { pc::MIC_B } else { pc::MIC_A }, (0.10 + gm * 2.0).min(0.8));
                if i == TAP_DIRECT {
                    pc_.add(Shape::line(pts, Stroke::new(1.4, col)));
                } else {
                    pc_.extend(Shape::dashed_line(&pts, Stroke::new(0.8, col), 4.0, 4.0));
                }
            }
        }
    }

    // ---- drawObjects
    for (k, o) in ch.objects.iter().enumerate() {
        if !o.on {
            continue;
        }
        let c = lay.p(o.pos.x, o.pos.y);
        let rr = (o.radius * lay.sc).max(7.0);
        let on = selected == SEL_OBJECT0 + k as i32;
        let ud = lay.d(o.ux, o.uy);
        if o.flat {
            let a = ud.y.atan2(ud.x);
            let (s, co) = a.sin_cos();
            draw_glyph(&pc_, o.ty, rr, &|q: Pos2| pos2(c.x + q.x * co - q.y * s, c.y + q.x * s + q.y * co));
        } else {
            draw_glyph(&pc_, o.ty, rr, &|q: Pos2| c + q.to_vec2());
        }
        if o.flat && on {
            let h = c + ud * (rr + 16.0);
            pc_.line_segment([c + ud * rr, h], Stroke::new(1.5, pc::OBJ));
            pc_.circle_filled(h, 4.5, pc::BG);
            pc_.circle_stroke(h, 4.5, Stroke::new(1.5, pc::OBJ));
            hits.push(Hit { kind: Kind::ObjRot, k, at: h, r: 10.0 });
        }
        if on {
            pc_.circle_stroke(c, rr + 5.0, Stroke::new(1.0, al(Color32::WHITE, 0.55)));
        }
        let mut name = short_name(o.ty).to_string();
        let f0 = resonance_hz(o);
        if o.ty == 2 && f0 > 0.0 {
            name.push_str(&format!(" {} Hz", (f0 + 0.5).floor() as i32));
        }
        pc_.text(pos2(c.x, c.y + rr + 11.0), Align2::CENTER_CENTER, name, ui_font(10.0, false), pc::DIM);
        hits.push(Hit { kind: Kind::Obj, k, at: c, r: rr.max(12.0) });
    }

    // ---- aim handles
    let aim_handle = |hits: &mut Vec<Hit>, c: Pos2, aim: geo::Vec3, col: Color32, kind: Kind, k: usize, faint: bool| {
        let dv = lay.d(aim.x, aim.y);
        let h = c + dv * 30.0;
        let a = if faint { 0.3 } else { 0.85 };
        pc_.line_segment([c, h], Stroke::new(1.0, al(col, a)));
        pc_.circle_filled(h, 3.5, al(pc::BG, a));
        pc_.circle_stroke(h, 3.5, Stroke::new(1.0, al(col, a)));
        hits.push(Hit { kind, k, at: h, r: 10.0 });
    };

    // ---- drawSpeaker
    {
        let c = lay.p(pl.speaker.x, pl.speaker.y);
        let sd = lay.d(pl.speaker_aim.x, pl.speaker_aim.y);
        let a = sd.y.atan2(sd.x);
        let mut cone = vec![c];
        cone.extend((0..=16).map(|i| a - 0.5 + i as f32 / 16.0).map(|t| pos2(c.x + t.cos() * 58.0, c.y + t.sin() * 58.0)));
        pc_.add(Shape::convex_polygon(cone, al(pc::CONE, 0.10), Stroke::NONE));
        aim_handle(&mut hits, c, pl.speaker_aim, pc::SPK, Kind::SpkAim, 0, false);
        let (s, co) = a.sin_cos();
        let tf = |x: f32, y: f32| pos2(c.x + x * co - y * s, c.y + x * s + y * co);
        pc_.add(Shape::convex_polygon(vec![tf(-9.0, -8.0), tf(1.0, -8.0), tf(1.0, 8.0), tf(-9.0, 8.0)], pc::SPK, Stroke::NONE));
        pc_.add(Shape::convex_polygon(vec![tf(1.0, -5.0), tf(10.0, -11.0), tf(10.0, 11.0), tf(1.0, 5.0)], pc::SPK, Stroke::NONE));
        if selected == SEL_SPEAKER {
            pc_.circle_stroke(c, 17.0, Stroke::new(1.0, al(Color32::WHITE, 0.55)));
        }
        hits.push(Hit { kind: Kind::Spk, k: 0, at: c, r: 15.0 });
    }

    // ---- drawMics
    let omni = pv(d::P_MIC_PAT).round() as i32 == 0;
    for k in 0..2usize {
        let pos = if k == 1 { pl.mic_b } else { pl.mic_a };
        let c = lay.p(pos.x, pos.y);
        let col = if k == 1 { pc::MIC_B } else { pc::MIC_A };
        aim_handle(&mut hits, c, if k == 1 { pl.mic_b_aim } else { pl.mic_a_aim }, col, Kind::MicAim, k, omni);
        pc_.circle_filled(c, 8.5, col);
        pc_.circle_stroke(c, 8.5, Stroke::new(2.0, argb(0xFFF2F4F7)));
        pc_.text(c + vec2(0.0, 0.5), Align2::CENTER_CENTER, if k == 1 { "B" } else { "A" }, ui_font(10.0, true), argb(0xFF101316));
        if selected == SEL_MIC_A + k as i32 {
            pc_.circle_stroke(c, 14.0, Stroke::new(1.0, al(Color32::WHITE, 0.55)));
        }
        hits.push(Hit { kind: Kind::Mic, k, at: c, r: 14.0 });
    }

    // ---- drawAddRow
    let used = ch.objects.iter().filter(|o| o.on).count();
    let buttons = add_buttons(&p, r, compact);
    let count_c = if used >= 8 || st.add_warn { cm::WARN } else { cm::FAINT };
    let font_h = if compact { 10.0 } else { 12.5 };
    if compact {
        let a = add_area_compact(r);
        let (r1, r2) = (buttons[0].center().y, buttons[4].center().y);
        p.text(pos2(a.left(), r1), Align2::LEFT_CENTER, "Add", ui_font(font_h, false), cm::LABEL);
        p.text(pos2(a.left(), r2), Align2::LEFT_CENTER, format!("{used}/8"), ui_font(font_h, false), count_c);
    } else {
        let row = add_row_area(r);
        p.text(pos2(row.left(), row.center().y), Align2::LEFT_CENTER, "Add", ui_font(12.5, false), cm::LABEL);
        p.text(pos2(row.right(), row.center().y), Align2::RIGHT_CENTER, format!("{used} / 8"), ui_font(12.5, false), count_c);
    }
    for (t, b) in buttons.iter().enumerate() {
        p.rect_filled(*b, CornerRadius::same(4), cm::FIELD);
        p.rect_stroke(b.shrink(0.5), CornerRadius::same(4), Stroke::new(1.0, cm::FIELD_LINE), egui::StrokeKind::Middle);
        p.text(
            b.center(),
            Align2::CENTER_CENTER,
            short_name(t as i32 + 1),
            ui_font(font_h, false),
            if used >= 8 { cm::FAINT } else { cm::LABEL_BRIGHT },
        );
    }

    // ---- the pointer
    if blocked {
        return out;
    }
    let id = ui.id().with("chamber-plan");
    let resp = ui.interact(r, id, egui::Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Room plan"));
    for (t, b) in buttons.iter().enumerate() {
        let br = ui.interact(*b, id.with(("add", t)), egui::Sense::click());
        br.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Add {}", short_name(t as i32 + 1))));
    }
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let pick = |at: Pos2| -> Option<Hit> {
        for kind in [Kind::SpkAim, Kind::MicAim, Kind::ObjRot, Kind::Spk, Kind::Mic, Kind::Obj, Kind::DimW, Kind::DimL] {
            if let Some(h) = hits.iter().rev().find(|h| h.kind == kind && at.distance(h.at) <= h.r) {
                return Some(*h);
            }
        }
        None
    };
    // mouseMove: the pointing hand over a target.
    if let Some(at) = pointer
        && r.contains(at)
        && (hits.iter().any(|h| at.distance(h.at) <= h.r) || buttons.iter().any(|b| b.contains(at)))
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // mouseDown
    let pressed = ui.input(|i| i.pointer.primary_pressed());
    if pressed
        && let Some(at) = ui.input(|i| i.pointer.press_origin())
        && resp.hovered()
        && r.contains(at)
    {
        if let Some(t) = buttons.iter().position(|b| b.contains(at)) {
            out.add_object = Some(t as i32 + 1);
            return out;
        }
        match pick(at) {
            None => out.select = Some(SEL_ROOM),
            Some(h) => {
                out.gesture_start = true;
                if matches!(h.kind, Kind::DimW | Kind::DimL) {
                    let l0 = layout(r, ch, compact);
                    let d0 = if h.kind == Kind::DimW { l0.d(1.0, 0.0) } else { l0.d(0.0, 1.0) };
                    st.drag = Some(Drag { kind: h.kind, k: h.k, dx: 0.0, dy: 0.0, off: d0 * 12.0, frozen: Some(l0) });
                    out.select = Some(SEL_ROOM);
                } else {
                    st.drag = Some(Drag { kind: h.kind, k: h.k, dx: h.at.x - at.x, dy: h.at.y - at.y, off: egui::Vec2::ZERO, frozen: None });
                    out.select = Some(match h.kind {
                        Kind::Spk | Kind::SpkAim => SEL_SPEAKER,
                        Kind::Mic | Kind::MicAim => {
                            if h.k == 1 {
                                SEL_MIC_B
                            } else {
                                SEL_MIC_A
                            }
                        }
                        _ => SEL_OBJECT0 + h.k as i32,
                    });
                }
            }
        }
    }
    // mouseDrag
    if let Some(dr) = st.drag
        && ui.input(|i| i.pointer.primary_down())
        && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO)
        && let Some(at) = pointer
    {
        let lay = dr.frozen.unwrap_or(lay);
        let m_to_v = |m: f32| (m.clamp(1.5, 30.0) / 1.5).ln() / 20f32.ln();
        let wrap = |v: f32| {
            let v = v % 1.0;
            if v < 0.0 { v + 1.0 } else { v }
        };
        let (px, py) = (at.x, at.y);
        let set = &mut out.set;
        match dr.kind {
            Kind::DimW => set.push((d::P_ROOM_W, m_to_v(lay.inv(px - dr.off.x, py - dr.off.y).x))),
            Kind::DimL => set.push((d::P_ROOM_L, m_to_v(lay.inv(px - dr.off.x, py - dr.off.y).y))),
            Kind::Spk | Kind::Mic | Kind::Obj => {
                let q = lay.inv(px + dr.dx, py + dr.dy);
                let u = q.x / rm.width_m.max(0.01);
                let v = q.y / rm.length_m.max(0.01);
                let (pu, pv_) = match dr.kind {
                    Kind::Spk => (d::P_SPK_X, d::P_SPK_Y),
                    Kind::Mic => {
                        if dr.k == 1 {
                            (d::P_MIC_BX, d::P_MIC_BY)
                        } else {
                            (d::P_MIC_AX, d::P_MIC_AY)
                        }
                    }
                    _ => (obj_x_param(dr.k), obj_y_param(dr.k)),
                };
                set.push((pu, u));
                set.push((pv_, v));
            }
            Kind::ObjRot => {
                if let Some(o) = ch.objects.get(dr.k).filter(|o| o.on) {
                    let cs = lay.p(o.pos.x, o.pos.y);
                    let rd = lay.inv_d(px - cs.x, py - cs.y);
                    set.push((obj_rotate_param(dr.k), wrap(rd.y.atan2(rd.x) / PI)));
                }
            }
            Kind::SpkAim | Kind::MicAim => {
                let c = if dr.kind == Kind::SpkAim {
                    pl.speaker
                } else if dr.k == 1 {
                    pl.mic_b
                } else {
                    pl.mic_a
                };
                let cs = lay.p(c.x, c.y);
                let rd = lay.inv_d(px - cs.x, py - cs.y);
                let param = if dr.kind == Kind::SpkAim {
                    d::P_SPK_AIM
                } else if dr.k == 1 {
                    d::P_MIC_B_AIM
                } else {
                    d::P_MIC_A_AIM
                };
                set.push((param, wrap(rd.y.atan2(rd.x) / (2.0 * PI))));
            }
        }
        for s in set.iter_mut() {
            s.1 = if s.1.is_finite() { s.1.clamp(0.0, 1.0) } else { 0.0 };
        }
    }
    // mouseUp
    if st.drag.is_some() && ui.input(|i| i.pointer.primary_released() || !i.pointer.primary_down()) && !pressed {
        st.drag = None;
        out.gesture_end = true;
    }
    // mouseWheelMove: the wheel over an object resizes it.
    if resp.hovered()
        && let Some(at) = pointer
    {
        let dy = crate::editor_kit::juce_wheel_delta_y(ui);
        if dy != 0.0
            && let Some(h) = hits.iter().find(|h| h.kind == Kind::Obj && at.distance(h.at) <= h.r)
        {
            crate::editor_kit::consume_wheel(ui);
            let param = obj_size_param(h.k);
            out.gesture_start = true;
            out.set.push((param, (pv(param) + if dy > 0.0 { 0.05 } else { -0.05 }).clamp(0.0, 1.0)));
            out.gesture_end = true;
        }
    }
    let _ = obj_type_param;
    out
}

// ============================================================================ ChamberEcho

/// `ChamberEcho::paint`: every tap's arrival time and level, A above the line, B below.
pub(crate) fn echo(p: &Painter, r: Rect, ch: &Chamber) {
    super::controls::draw_panel(p, r, argb(0xFF0D1720), cm::GROUP_LINE, 5.0);
    let (w, h) = (r.width(), r.height());
    const SPEED: f32 = 343.0;
    let mut t_max = 0.0f32;
    let mut g_max = 1.0e-6f32;
    for t in ch.taps.iter() {
        let gm = t.gain.mid.abs();
        if !t.live || gm <= 1.0e-5 {
            continue;
        }
        t_max = t_max.max(t.distance / SPEED * 1000.0);
        g_max = g_max.max(gm);
    }
    let span = (t_max * 1.08).clamp(40.0, 500.0);
    let (left, right) = (26.0, w - 10.0);
    let mid = (h - 16.0) * 0.5 + 2.0;
    let amp = mid - 10.0;
    const RANGE: f32 = 42.0;
    let tick = if span > 250.0 {
        50
    } else if span > 100.0 {
        20
    } else {
        10
    };
    let at = |x: f32, y: f32| pos2(r.left() + x, r.top() + y);
    let mut t = 0;
    while t as f32 <= span {
        let x = (left + (right - left) * t as f32 / span).trunc();
        p.line_segment([at(x + 0.5, 4.0), at(x + 0.5, h - 18.0)], Stroke::new(1.0, al(Color32::WHITE, 0.07)));
        if t > 0 {
            p.text(at(x, h - 9.0), Align2::CENTER_CENTER, format!("{t} ms"), mono(10.5), pc::DIM);
        }
        t += tick;
    }
    p.line_segment([at(left, mid.trunc() + 0.5), at(right, mid.trunc() + 0.5)], Stroke::new(1.0, super::controls::brighter(cm::GROUP_LINE, 0.2)));
    p.text(at(11.0, mid - amp * 0.5), Align2::CENTER_CENTER, "A", mono_bold(10.5), argb(0xFF6FD3C6));
    p.text(at(11.0, mid + amp * 0.5), Align2::CENTER_CENTER, "B", mono_bold(10.5), argb(0xFF6FD3C6));
    const ALPHA: [f32; 4] = [1.0, 0.85, 0.4, 0.65];
    let slate = argb(0xFF8196AD);
    for mi in 0..2 {
        for i in 0..TAPS_PER_MIC {
            let Some(tp) = ch.taps.get(mi * TAPS_PER_MIC + i) else { continue };
            let gm = tp.gain.mid.abs();
            if !tp.live || gm < 1.0e-5 {
                continue;
            }
            let lv = (1.0 + db(gm / g_max) / RANGE).max(0.0);
            if lv <= 0.0 {
                continue;
            }
            let order = if i == TAP_DIRECT {
                0
            } else if i < TAP_SECOND {
                1
            } else if i < TAP_OBJECT {
                2
            } else {
                3
            };
            let x = left + (right - left) * (tp.distance / SPEED * 1000.0) / span;
            let col = match order {
                0 => argb(0xFFF2F4F7),
                1 => cm::ACCENT,
                _ => slate,
            };
            let sign = if mi == 1 { 1.0 } else { -1.0 };
            p.line_segment([at(x, mid), at(x, mid + sign * lv * amp)], Stroke::new(if order == 2 { 1.0 } else { 2.0 }, al(col, ALPHA[order])));
        }
    }
}
