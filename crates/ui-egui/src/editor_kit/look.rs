//! `Common/EngineLook.h` and the text helpers of `Common/Theme.h`: the family's type scale,
//! background, panels, title, glow and letter-spaced text.

use super::palette::{VstPalette, al, rgb};
use crate::theme::{bold, regular};
use egui::{Align2, Color32, CornerRadius, Mesh, Painter, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};

/// Type scale (`fs::look`).
pub const TITLE_SIZE: f32 = 16.0;
pub const VALUE_SIZE: f32 = 13.0;
pub const LABEL_SIZE: f32 = 11.0;
pub const CAPTION_SIZE: f32 = 10.0;
pub const BLURB_SIZE: f32 = 11.5;
pub const TINY_SIZE: f32 = 9.0;

pub const PANEL_CORNER: f32 = 10.0;
pub const BUTTON_CORNER: f32 = 4.0;

/// `theme::font (height, semiBold)`.
pub(crate) fn font(size: f32, semi_bold: bool) -> egui::FontId {
    if semi_bold { bold(size) } else { regular(size) }
}

/// A corner radius as egui takes it.
pub(crate) fn corner(r: f32) -> CornerRadius {
    CornerRadius::same(r.round().clamp(0.0, 255.0) as u8)
}

/// A radial gradient filling `r` (JUCE `ColourGradient (..., isRadial = true)`): `stops` are
/// `(proportion, colour)` from `centre` out to `radius`. Drawn as a vertex-coloured grid.
pub(crate) fn radial_gradient(p: &Painter, r: Rect, centre: Pos2, radius: f32, stops: &[(f32, Color32)]) {
    if stops.is_empty() || !r.is_positive() {
        return;
    }
    let radius = radius.max(1.0);
    let colour = |pt: Pos2| {
        let t = (pt.distance(centre) / radius).clamp(0.0, 1.0);
        let mut a = stops[0];
        for b in stops.iter().copied() {
            if t <= b.0 {
                let span = b.0 - a.0;
                let u = if span > 0.0 { (t - a.0) / span } else { 1.0 };
                return a.1.lerp_to_gamma(b.1, u.clamp(0.0, 1.0));
            }
            a = b;
        }
        a.1
    };
    let (nx, ny) = (40usize, 16usize);
    let mut mesh = Mesh::default();
    for j in 0..=ny {
        for i in 0..=nx {
            let pt = pos2(r.left() + r.width() * i as f32 / nx as f32, r.top() + r.height() * j as f32 / ny as f32);
            mesh.colored_vertex(pt, colour(pt));
        }
    }
    let w = (nx + 1) as u32;
    for j in 0..ny as u32 {
        for i in 0..nx as u32 {
            let a = j * w + i;
            mesh.add_triangle(a, a + 1, a + w + 1);
            mesh.add_triangle(a, a + w + 1, a + w);
        }
    }
    p.add(egui::Shape::mesh(mesh));
}

/// `paintBackground`: warm grey with a soft lift towards the top centre (0x2B2D31 at
/// (cx, y - 0.1 h), 0x1C1D20 at 0.55, 0x151618 at the bottom centre).
pub(crate) fn paint_background(p: &Painter, r: Rect) {
    let centre = pos2(r.center().x, r.top() - r.height() * 0.1);
    let radius = centre.distance(pos2(r.center().x, r.bottom()));
    radial_gradient(p, r, centre, radius, &[(0.0, rgb(0x2B2D31)), (0.55, rgb(0x1C1D20)), (1.0, rgb(0x151618))]);
}

/// `paintPanel`: black at 40 %, a hairline white border at 7 %.
pub(crate) fn paint_panel(p: &Painter, r: Rect, corner_r: f32) {
    p.rect_filled(r, corner(corner_r), al(Color32::BLACK, 0.4));
    p.rect_stroke(r.shrink(0.5), corner(corner_r), Stroke::new(1.0, al(Color32::WHITE, 0.07)), StrokeKind::Middle);
}

/// `paintTitle`: the plain first word, then the accented rest ("Dist" + "Engine"), centred
/// vertically in `area`. Returns the width used.
pub(crate) fn paint_title(p: &Painter, area: Rect, first: &str, rest: &str, pal: &VstPalette) -> f32 {
    let f = font(TITLE_SIZE, true);
    let w1 = p.layout_no_wrap(format!("{first} "), f.clone(), pal.text_bright).size().x;
    p.text(pos2(area.left(), area.center().y), Align2::LEFT_CENTER, first, f.clone(), pal.text_bright);
    let w2 = p.layout_no_wrap(rest.to_string(), f.clone(), pal.accent).size().x;
    p.text(pos2(area.left() + w1, area.center().y), Align2::LEFT_CENTER, rest, f, pal.accent);
    w1 + w2
}

/// `glowRect`: CSS `box-shadow: 0 0 Npx` on a filled shape, three widening strokes.
pub(crate) fn glow_rect(p: &Painter, r: Rect, corner_r: f32, c: Color32, strength: f32) {
    for i in (1..=3).rev() {
        let i = i as f32;
        p.rect_stroke(r.expand(i * 1.4), corner(corner_r), Stroke::new(1.6, al(c, 0.10 * strength / i)), StrokeKind::Middle);
    }
}

/// `trackedWidth`: the width of `text` letter-spaced by `tracking_em` of its size.
pub(crate) fn tracked_width(p: &Painter, text: &str, size: f32, tracking_em: f32, uppercase: bool) -> f32 {
    let s = if uppercase { text.to_uppercase() } else { text.to_string() };
    let n = s.chars().count();
    if n == 0 {
        return 0.0;
    }
    let f = font(size, false);
    let glyphs: f32 = s.chars().map(|c| p.layout_no_wrap(c.to_string(), f.clone(), Color32::WHITE).size().x).sum();
    glyphs + tracking_em * size * n.saturating_sub(1) as f32
}

/// `drawTracked`: glyphs laid out one by one with `tracking_em` between them (JUCE has no
/// tracking; nor does egui). `align` is the horizontal justification inside `area`; the baseline
/// sits at the centre plus 0.35 of the size, as the C++ puts it. Returns the text's rect.
#[allow(clippy::too_many_arguments)]
pub(crate) fn tracked_text(
    p: &Painter,
    area: Rect,
    align: Align2,
    text: &str,
    size: f32,
    tracking_em: f32,
    uppercase: bool,
    semi_bold: bool,
    c: Color32,
) -> Rect {
    let s = if uppercase { text.to_uppercase() } else { text.to_string() };
    if s.is_empty() {
        return Rect::from_min_size(area.center(), vec2(0.0, 0.0));
    }
    let f = font(size, semi_bold);
    let tracking = tracking_em * size;
    let widths: Vec<f32> = s.chars().map(|ch| p.layout_no_wrap(ch.to_string(), f.clone(), c).size().x).collect();
    let total = widths.iter().sum::<f32>() + tracking * widths.len().saturating_sub(1) as f32;
    let mut x = match align.x() {
        egui::Align::Center => area.center().x - total * 0.5,
        egui::Align::Max => area.right() - total,
        egui::Align::Min => area.left(),
    };
    let x0 = x;
    let baseline = area.center().y + size * 0.35;
    for (ch, w) in s.chars().zip(&widths) {
        // egui places text by its row's top; a row is about 1.2 x the size, the ascent ~0.95.
        p.text(pos2(x, baseline - size * 0.95), Align2::LEFT_TOP, ch, f.clone(), c);
        x += w + tracking;
    }
    Rect::from_min_max(pos2(x0, baseline - size), pos2(x0 + total, baseline + size * 0.25))
}
