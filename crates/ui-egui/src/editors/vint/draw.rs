//! The bits of JUCE's `Graphics` Vint's hardware paint code leans on, for egui: `Colour`'s
//! `brighter` / `darker` / `getPerceivedBrightness` / `withMultipliedBrightness`, linear and radial
//! `ColourGradient` fills of paths (filled as meshes whose vertices carry the gradient's colour),
//! ellipses, rounded rectangles, quadratic segments and rotated text.

use crate::theme::bold;
use egui::{Align2, Color32, Mesh, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};

/// A JUCE colour: 8-bit channels, straight (not premultiplied) alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Jc {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f32,
}

impl Jc {
    /// `0xAARRGGBB`.
    pub const fn argb(v: u32) -> Jc {
        Jc { r: (v >> 16) as u8, g: (v >> 8) as u8, b: v as u8, a: ((v >> 24) & 0xFF) as f32 / 255.0 }
    }
    pub const BLACK: Jc = Jc::argb(0xFF00_0000);
    pub const WHITE: Jc = Jc::argb(0xFFFF_FFFF);

    /// `withAlpha`.
    pub fn alpha(self, a: f32) -> Jc {
        Jc { a: a.clamp(0.0, 1.0), ..self }
    }
    /// `brighter (amount)`.
    pub fn brighter(self, amount: f32) -> Jc {
        let k = 1.0 / (1.0 + amount);
        let f = |c: u8| (255.0 - k * (255.0 - f32::from(c))) as u8;
        Jc { r: f(self.r), g: f(self.g), b: f(self.b), a: self.a }
    }
    /// `darker (amount)`.
    pub fn darker(self, amount: f32) -> Jc {
        let k = 1.0 / (1.0 + amount);
        let f = |c: u8| (k * f32::from(c)) as u8;
        Jc { r: f(self.r), g: f(self.g), b: f(self.b), a: self.a }
    }
    /// `getPerceivedBrightness`.
    pub fn perceived_brightness(self) -> f32 {
        let (r, g, b) = (f32::from(self.r) / 255.0, f32::from(self.g) / 255.0, f32::from(self.b) / 255.0);
        (0.241 * r * r + 0.691 * g * g + 0.068 * b * b).sqrt()
    }
    /// `withMultipliedBrightness`: HSB brightness times `k`.
    pub fn mul_brightness(self, k: f32) -> Jc {
        let (r, g, b) = (f32::from(self.r), f32::from(self.g), f32::from(self.b));
        let hi = r.max(g).max(b);
        if hi <= 0.0 {
            return self;
        }
        let v = (hi * k).clamp(0.0, 255.0);
        let s = v / hi;
        Jc { r: (r * s).round() as u8, g: (g * s).round() as u8, b: (b * s).round() as u8, a: self.a }
    }
    pub fn c32(self) -> Color32 {
        Color32::from_rgba_unmultiplied(self.r, self.g, self.b, (self.a * 255.0).round().clamp(0.0, 255.0) as u8)
    }
    fn lerp(self, o: Jc, t: f32) -> Jc {
        let t = t.clamp(0.0, 1.0);
        let m = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        Jc { r: m(self.r, o.r), g: m(self.g, o.g), b: m(self.b, o.b), a: self.a + (o.a - self.a) * t }
    }
}

/// A fill: one colour or a `ColourGradient` (`radial`: `c1` at `p1`, `c2` at the distance to `p2`).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Fill {
    Solid(Jc),
    Grad { c1: Jc, p1: Pos2, c2: Jc, p2: Pos2, radial: bool },
}

impl Fill {
    pub fn linear(c1: Jc, p1: Pos2, c2: Jc, p2: Pos2) -> Fill {
        Fill::Grad { c1, p1, c2, p2, radial: false }
    }
    pub fn radial(c1: Jc, p1: Pos2, c2: Jc, p2: Pos2) -> Fill {
        Fill::Grad { c1, p1, c2, p2, radial: true }
    }
    fn at(&self, q: Pos2) -> Color32 {
        match *self {
            Fill::Solid(c) => c.c32(),
            Fill::Grad { c1, p1, c2, p2, radial } => {
                let d = p2 - p1;
                let t = if radial {
                    let r = d.length();
                    if r > 0.0 { (q - p1).length() / r } else { 1.0 }
                } else {
                    let l2 = d.length_sq();
                    if l2 > 0.0 { (q - p1).dot(d) / l2 } else { 0.0 }
                };
                c1.lerp(c2, t).c32()
            }
        }
    }
}

/// Fills the star-shaped (seen whole from `centre`) outline `pts` with `fill`: concentric rings
/// of vertices, so a gradient reads smoothly.
pub(crate) fn fill_star(p: &Painter, centre: Pos2, pts: &[Pos2], fill: Fill) {
    let n = pts.len();
    if n < 3 {
        return;
    }
    const RINGS: usize = 6;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(centre, fill.at(centre));
    for k in 1..=RINGS {
        let t = k as f32 / RINGS as f32;
        for q in pts {
            let v = centre + (*q - centre) * t;
            mesh.colored_vertex(v, fill.at(v));
        }
    }
    let n32 = n as u32;
    for i in 0..n32 {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n32);
    }
    for k in 1..RINGS as u32 {
        let (a, b) = (1 + (k - 1) * n32, 1 + k * n32);
        for i in 0..n32 {
            let j = (i + 1) % n32;
            mesh.add_triangle(a + i, b + i, b + j);
            mesh.add_triangle(a + i, b + j, a + j);
        }
    }
    p.add(Shape::mesh(mesh));
}

/// An ellipse's outline (`n` points).
pub(crate) fn ellipse(c: Pos2, rx: f32, ry: f32, n: usize) -> Vec<Pos2> {
    (0..n).map(|i| std::f32::consts::TAU * i as f32 / n as f32).map(|t| pos2(c.x + t.cos() * rx, c.y + t.sin() * ry)).collect()
}

/// `fillEllipse (x, y, w, h)` (the box's top-left and size).
pub(crate) fn fill_ellipse(p: &Painter, r: Rect, fill: Fill) {
    let pts = ellipse(r.center(), r.width() * 0.5, r.height() * 0.5, 48);
    fill_star(p, r.center(), &pts, fill);
}

/// A rounded rectangle's outline.
pub(crate) fn round_rect(r: Rect, corner: f32) -> Vec<Pos2> {
    let c = corner.min(r.width() * 0.5).min(r.height() * 0.5).max(0.0);
    let mut out = Vec::with_capacity(40);
    let corners = [
        (r.right() - c, r.top() + c, -90.0f32),
        (r.right() - c, r.bottom() - c, 0.0),
        (r.left() + c, r.bottom() - c, 90.0),
        (r.left() + c, r.top() + c, 180.0),
    ];
    for (x, y, a0) in corners {
        for k in 0..=8 {
            let a = (a0 + 90.0 * k as f32 / 8.0).to_radians();
            out.push(pos2(x + a.cos() * c, y + a.sin() * c));
        }
    }
    out
}

pub(crate) fn fill_round_rect(p: &Painter, r: Rect, corner: f32, fill: Fill) {
    if !r.is_positive() {
        return;
    }
    fill_star(p, r.center(), &round_rect(r, corner), fill);
}

/// A closed outline stroked.
pub(crate) fn stroke_closed(p: &Painter, pts: &[Pos2], width: f32, c: Jc) {
    if pts.len() < 2 {
        return;
    }
    p.add(Shape::closed_line(pts.to_vec(), Stroke::new(width, c.c32())));
}

pub(crate) fn line(p: &Painter, a: Pos2, b: Pos2, width: f32, c: Jc) {
    p.line_segment([a, b], Stroke::new(width, c.c32()));
}

/// `quadraticTo` from `p0` (8 points, without `p0`).
pub(crate) fn quad(out: &mut Vec<Pos2>, p0: Pos2, c: Pos2, p1: Pos2) {
    for i in 1..=8 {
        let t = i as f32 / 8.0;
        let u = 1.0 - t;
        out.push((p0.to_vec2() * (u * u) + c.to_vec2() * (2.0 * u * t) + p1.to_vec2() * (t * t)).to_pos2());
    }
}

/// `AffineTransform::rotation (a).translated (c)` applied to `pts`.
pub(crate) fn rotate(pts: &mut [Pos2], a: f32, c: Pos2) {
    let (s, co) = a.sin_cos();
    for q in pts.iter_mut() {
        *q = pos2(c.x + q.x * co - q.y * s, c.y + q.x * s + q.y * co);
    }
}

pub(crate) fn translate(pts: &[Pos2], dx: f32, dy: f32) -> Vec<Pos2> {
    pts.iter().map(|q| *q + vec2(dx, dy)).collect()
}

/// `drawText (text, rect, centred)` in the bold UI face at `size`.
pub(crate) fn text_c(p: &Painter, r: Rect, text: &str, size: f32, c: Jc) {
    p.text(r.center(), Align2::CENTER_CENTER, text, bold(size), c.c32());
}

/// `drawText` centred in `r`, turned a quarter left about `pivot` (reading upward).
pub(crate) fn text_up(p: &Painter, centre: Pos2, text: &str, size: f32, c: Jc) {
    let galley = p.layout_no_wrap(text.to_string(), bold(size), c.c32());
    let (w, h) = (galley.size().x, galley.size().y);
    let pos = pos2(centre.x - h * 0.5, centre.y + w * 0.5);
    p.add(egui::epaint::TextShape::new(pos, galley, c.c32()).with_angle(-std::f32::consts::FRAC_PI_2));
}

/// `drawFittedText (..., 1 line, minimumHorizontalScale 0.6)`: the font shrinks until the text
/// fits (never below 0.6 of its size).
pub(crate) fn text_fitted(p: &Painter, r: Rect, text: &str, size: f32, c: Jc, up: bool) {
    let avail = if up { r.height() } else { r.width() };
    let w = p.layout_no_wrap(text.to_string(), bold(size), c.c32()).size().x;
    let s = if w > avail && w > 0.0 { size * (avail / w).max(0.6) } else { size };
    if up {
        text_up(p, r.center(), text, s, c);
    } else {
        text_c(p, r, text, s, c);
    }
}
