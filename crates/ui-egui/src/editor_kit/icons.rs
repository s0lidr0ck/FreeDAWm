//! Glyphs the editors draw in code (the UI font lacks them, and no image is copied from a VST):
//! a cross, the menu and stepper triangles, the undo/redo arrows, and the path helpers the paint
//! code uses (cubic segments, dashed lines, a filled strip under a curve).

use egui::{Color32, Mesh, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

/// An X drawn as two strokes (the font has no U+2715).
pub(crate) fn cross(p: &Painter, c: Pos2, s: f32, col: Color32) {
    p.line_segment([c - vec2(s, s), c + vec2(s, s)], Stroke::new(1.4, col));
    p.line_segment([c + vec2(-s, s), c + vec2(s, -s)], Stroke::new(1.4, col));
}

/// The small down triangle a menu button carries (U+25BE).
pub(crate) fn tri_down(p: &Painter, c: Pos2, col: Color32) {
    p.add(Shape::convex_polygon(vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)], col, Stroke::NONE));
}

/// A stepper triangle pointing left or right (U+25C0 / U+25B6).
pub(crate) fn tri_side(p: &Painter, c: Pos2, left: bool, col: Color32) {
    let s = if left { -1.0 } else { 1.0 };
    p.add(Shape::convex_polygon(vec![c + vec2(4.0 * s, 0.0), c + vec2(-3.0 * s, -4.5), c + vec2(-3.0 * s, 4.5)], col, Stroke::NONE));
}

/// U+21B6 / U+21B7: an open arc with its arrowhead, centred in `r`.
pub(crate) fn turn_arrow(p: &Painter, r: Rect, redo: bool, col: Color32) {
    let c = r.center() + vec2(0.0, 1.5);
    let sx = if redo { 1.0 } else { -1.0 };
    let pts: Vec<Pos2> =
        (0..=12).map(|i| (-90.0f32 + 225.0 * i as f32 / 12.0).to_radians()).map(|a| c + vec2(sx * -a.sin() * 5.0, -a.cos() * 5.0)).collect();
    let head = pts.first().copied().unwrap_or(c);
    p.add(Shape::line(pts, Stroke::new(1.4, col)));
    p.add(Shape::convex_polygon(vec![head + vec2(sx * -3.5, -0.5), head + vec2(sx * 1.0, -3.5), head + vec2(sx * 1.0, 3.0)], col, Stroke::NONE));
}

/// `Path::cubicTo` from `p0`: 16 points along the Bézier, appended to `out` (without `p0`).
pub(crate) fn cubic(out: &mut Vec<Pos2>, p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2) {
    for i in 1..=16 {
        let t = i as f32 / 16.0;
        let u = 1.0 - t;
        let v = p0.to_vec2() * (u * u * u) + p1.to_vec2() * (3.0 * u * u * t) + p2.to_vec2() * (3.0 * u * t * t) + p3.to_vec2() * (t * t * t);
        out.push(v.to_pos2());
    }
}

/// `PathStrokeType::createDashedStroke` along `pts`.
pub(crate) fn dashed_closed(p: &Painter, pts: &[Pos2], dash: f32, gap: f32, stroke: Stroke) {
    p.extend(Shape::dashed_line(pts, stroke, dash, gap));
}

/// The area between a polyline and `base(x)`, filled in one colour (a path closed down to a
/// line and filled).
pub(crate) fn fill_strip(p: &Painter, top: &[Pos2], base: impl Fn(f32) -> f32, col: Color32) {
    let mut mesh = Mesh::default();
    for (k, t) in top.iter().enumerate() {
        mesh.colored_vertex(*t, col);
        mesh.colored_vertex(pos2(t.x, base(t.x)), col);
        if k > 0 {
            let i = (k * 2) as u32;
            mesh.add_triangle(i - 2, i - 1, i);
            mesh.add_triangle(i - 1, i + 1, i);
        }
    }
    p.add(Shape::mesh(mesh));
}

/// A radial background: `stops` from `centre` out to `radius` (see `look::radial_gradient`).
pub(crate) fn radial_bg(p: &Painter, r: Rect, centre: Pos2, radius: f32, stops: &[(f32, Color32)]) {
    super::look::radial_gradient(p, r, centre, radius, stops);
}
