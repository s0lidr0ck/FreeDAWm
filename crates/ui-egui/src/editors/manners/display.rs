//! Manners' display: the module's input spectrum (dim, filled) and output spectrum (a line), the
//! gain it is applying right now over frequency (cuts hang under the 0 dB line, lifts stand on
//! it), and the working range, whose two edges drag.

use super::hz_text;
use crate::editor_kit::look::{CAPTION_SIZE, TINY_SIZE, paint_panel};
use crate::editor_kit::palette::{VstPalette, al, argb};
use crate::module_graphs::{LiveValues, Streams, db_y, freq_to_x, spectrum, x_to_freq};
use crate::theme::regular;
use egui::{Align2, Color32, CornerRadius, CursorIcon, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

/// The gain scale, ±dB.
pub(crate) const RANGE_DB: f32 = 12.0;
/// How near an edge the pointer grabs it, px either side.
const GRAB: f32 = 6.0;
/// The two edges never come closer than this, in octaves.
const MIN_SPAN_OCT: f32 = 0.5;

/// What the pointer did to the range this frame.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(crate) struct Out {
    /// A drag began (push undo).
    pub started: bool,
    /// An edge moved: (0 low / 1 high, the new frequency).
    pub moved: Option<(usize, f32)>,
    /// The drag ended.
    pub ended: bool,
}

/// The colour of a cut.
fn cut_colour() -> Color32 {
    argb(0xFFE8_9A5B)
}

/// The plot area inside the display `r` (under the legend, above the frequency labels).
pub(crate) fn plot_rect(r: Rect) -> Rect {
    let b = r.shrink2(vec2(12.0, 10.0));
    Rect::from_min_max(pos2(b.left(), b.top() + 22.0), pos2(b.right(), b.bottom() - 16.0))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw(
    ui: &mut Ui,
    r: Rect,
    pal: &VstPalette,
    streams: Option<&Streams>,
    live: Option<&LiveValues>,
    range: (f32, f32),
    sr: f32,
    dragging: &mut Option<usize>,
    compact: bool,
) -> Out {
    let p = ui.painter().clone();
    paint_panel(&p, r, 10.0);
    let b = r.shrink2(if compact { vec2(8.0, 8.0) } else { vec2(12.0, 10.0) });
    let plot = if compact { Rect::from_min_max(pos2(b.left(), b.top() + 18.0), pos2(b.right(), b.bottom() - 13.0)) } else { plot_rect(r) };
    let (lo, hi) = if range.0 <= range.1 { range } else { (range.1, range.0) };

    // ---- legend and readouts
    let head_y = b.top() + 9.0;
    let mut x = b.left();
    let legend: &[(Color32, &str)] = if compact {
        &[(pal.accent, "Lift"), (cut_colour(), "Cut")]
    } else {
        &[(al(pal.text_body, 0.45), "Input"), (pal.text_bright, "Output"), (pal.accent, "Lift"), (cut_colour(), "Cut")]
    };
    for (c, t) in legend.iter().copied() {
        p.rect_filled(Rect::from_center_size(pos2(x + 5.0, head_y), vec2(10.0, 10.0)), CornerRadius::same(2), c);
        let g = p.text(pos2(x + 15.0, head_y), Align2::LEFT_CENTER, t, regular(CAPTION_SIZE), pal.text_body);
        x = g.right() + 14.0;
    }
    let reading = |i: usize| live.and_then(|l| l.readouts.get(i).copied()).filter(|v| v.is_finite()).unwrap_or(0.0);
    let (cut, lift) = (reading(0).max(0.0), reading(1).max(0.0));
    let text = if compact {
        format!("\u{2212}{cut:.1} / +{lift:.1} dB")
    } else {
        format!("Tame \u{2212}{cut:.1} dB   Recover +{lift:.1} dB   {} \u{2013} {}", hz_text(lo), hz_text(hi))
    };
    p.text(pos2(b.right(), head_y), Align2::RIGHT_CENTER, text, regular(CAPTION_SIZE), pal.text_body);

    // ---- the well, the grid
    p.rect_filled(plot, CornerRadius::same(3), argb(0xFF0C_0F12));
    let grid = Stroke::new(1.0, al(Color32::WHITE, 0.06));
    let marks: &[(f32, &str)] = if compact {
        &[(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")]
    } else {
        &[(50.0, "50"), (100.0, "100"), (200.0, "200"), (500.0, "500"), (1000.0, "1k"), (2000.0, "2k"), (5000.0, "5k"), (10_000.0, "10k")]
    };
    for (f, name) in marks.iter().copied() {
        let gx = freq_to_x(plot, f);
        p.line_segment([pos2(gx, plot.top()), pos2(gx, plot.bottom())], grid);
        p.text(pos2(gx, plot.bottom() + 3.0), Align2::CENTER_TOP, name, regular(TINY_SIZE), pal.text_faint);
    }
    for db in [-6.0, 6.0] {
        let y = db_y(plot, db, RANGE_DB);
        p.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], grid);
        p.text(pos2(plot.left() + 3.0, y - 1.0), Align2::LEFT_BOTTOM, format!("{db:+.0} dB"), regular(TINY_SIZE), pal.text_faint);
    }

    // ---- the spectra, behind the curve
    let pc = p.with_clip_rect(plot.intersect(p.clip_rect()));
    if let Some(s) = streams {
        spectrum(&pc, plot, &s.pre, sr, al(pal.text_body, 0.16), None);
        spectrum(&pc, plot, &s.post, sr, Color32::TRANSPARENT, Some(Stroke::new(1.0, al(pal.text_bright, 0.4))));
    }

    // ---- the gain curve
    let zero = db_y(plot, 0.0, RANGE_DB);
    pc.line_segment([pos2(plot.left(), zero), pos2(plot.right(), zero)], Stroke::new(1.0, al(Color32::WHITE, 0.22)));
    if let Some(curve) = streams.map(|s| &s.manners).filter(|c| c.len() >= 2) {
        let pts: Vec<Pos2> = curve.iter().map(|(hz, db)| pos2(freq_to_x(plot, *hz), db_y(plot, *db, RANGE_DB))).collect();
        for w in pts.windows(2) {
            if let [a, c] = w {
                let fill = |a: Pos2, c: Pos2| {
                    let col = if (a.y + c.y) * 0.5 > zero { cut_colour() } else { pal.accent };
                    pc.add(Shape::convex_polygon(vec![a, c, pos2(c.x, zero), pos2(a.x, zero)], al(col, 0.38), Stroke::NONE));
                };
                if (a.y - zero) * (c.y - zero) < 0.0 {
                    let t = (zero - a.y) / (c.y - a.y);
                    let mid = pos2(a.x + (c.x - a.x) * t, zero);
                    fill(*a, mid);
                    fill(mid, *c);
                } else {
                    fill(*a, *c);
                }
            }
        }
        pc.add(Shape::line(pts, Stroke::new(1.6, pal.text_bright)));
    }

    // ---- the working range: dimmed outside, an edge to drag at each end
    let (x_lo, x_hi) = (freq_to_x(plot, lo), freq_to_x(plot, hi));
    let shade = al(Color32::BLACK, 0.45);
    if x_lo > plot.left() + 0.5 {
        pc.rect_filled(Rect::from_min_max(plot.min, pos2(x_lo, plot.bottom())), 0.0, shade);
    }
    if x_hi < plot.right() - 0.5 {
        pc.rect_filled(Rect::from_min_max(pos2(x_hi, plot.top()), plot.max), 0.0, shade);
    }
    let mut out = Out::default();
    for (edge, ex) in [(0usize, x_lo), (1usize, x_hi)] {
        let grab = Rect::from_min_max(pos2(ex - GRAB, plot.top()), pos2(ex + GRAB, plot.bottom()));
        let resp = ui.interact(grab, ui.id().with(("manners-range", edge)), Sense::click_and_drag());
        let hot = resp.hovered() || *dragging == Some(edge);
        if hot {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
        if resp.drag_started() {
            *dragging = Some(edge);
            out.started = true;
        }
        if resp.dragged()
            && *dragging == Some(edge)
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let hz = x_to_freq(plot, pos.x);
            // The edges keep their order and a little room between them.
            let k = 2f32.powf(MIN_SPAN_OCT);
            let hz = if edge == 0 { hz.min(hi / k) } else { hz.max(lo * k) };
            out.moved = Some((edge, hz.clamp(20.0, 20_000.0)));
        }
        if resp.drag_stopped() {
            *dragging = None;
            out.ended = true;
        }
        if resp.double_clicked() {
            // Back to wide open.
            out.started = true;
            out.moved = Some((edge, if edge == 0 { 20.0 } else { 20_000.0 }));
            out.ended = true;
        }
        let col = if hot { pal.accent } else { al(pal.accent, 0.6) };
        p.line_segment([pos2(ex, plot.top()), pos2(ex, plot.bottom())], Stroke::new(if hot { 2.0 } else { 1.0 }, col));
        let tab = Rect::from_center_size(pos2(ex, plot.center().y), vec2(6.0, 28.0));
        p.rect_filled(tab, CornerRadius::same(3), col);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plot_sits_inside_the_display() {
        let r = Rect::from_min_size(pos2(24.0, 55.0), vec2(812.0, 250.0));
        let plot = plot_rect(r);
        assert!(r.contains_rect(plot));
        assert_eq!(plot, Rect::from_min_max(pos2(36.0, 87.0), pos2(824.0, 279.0)));
    }
}
