//! CopyQ's display: this track's sound (dim, filled) and the target's (a line), both as measured
//! and at the same level, the response being applied over them (cuts hang under the 0 dB line,
//! lifts stand on it; a second line when mid and side or left and right are matched apart), the
//! working range, whose two edges drag, and the three splits between the four ranges, which drag
//! too.

use super::hz_text;
use crate::editor_kit::look::{CAPTION_SIZE, TINY_SIZE, paint_panel};
use crate::editor_kit::palette::{VstPalette, al, argb};
use crate::module_graphs::{db_y, freq_to_x, x_to_freq};
use crate::theme::regular;
use egui::{Align2, Color32, CornerRadius, CursorIcon, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use soundcraft_dsp::plugins::fs::copyq::GRID_N;
use soundcraft_dsp::plugins::fs::copyq::profile::{Curve, FLOOR_DB, grid_hz};

/// How near a handle the pointer grabs it, px either side.
const GRAB: f32 = 6.0;
/// The range's edges never come closer than this, in octaves.
const MIN_SPAN_OCT: f32 = 0.5;
/// The sounds' scale: this many dB from the top of the plot to its bottom.
const SOUND_SPAN_DB: f32 = 60.0;
/// The handles: the range's low and high edges, then the three splits.
pub(crate) const EDGE_LO: usize = 0;
pub(crate) const EDGE_HI: usize = 1;
pub(crate) const SPLIT: usize = 2;

/// What is drawn. A curve that is `None` is not known (nothing playing and nothing stored).
#[derive(Clone, Default)]
pub(crate) struct Shown {
    /// This track's sound and the target's, for curve A and curve B.
    pub source: Option<[Curve; 2]>,
    pub target: Option<[Curve; 2]>,
    /// The response applied, dB.
    pub applied: Option<[Curve; 2]>,
    /// The two curves differ, and what the second is ("Side", "Right").
    pub second: Option<&'static str>,
    /// What the first is when they differ ("Mid", "Left").
    pub first: &'static str,
    /// One line of status over the plot's right.
    pub status: String,
}

/// The knobs the display draws and moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Knobs {
    pub range: (f32, f32),
    pub splits: [f32; 3],
    pub amounts: [f32; 4],
    /// The gain scale, ±dB.
    pub scale_db: f32,
}

/// What the pointer did this frame.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(crate) struct Out {
    /// A drag began (push undo).
    pub started: bool,
    /// A handle moved: (which, the new frequency).
    pub moved: Option<(usize, f32)>,
    /// The drag ended.
    pub ended: bool,
}

/// The colour of a cut.
pub(crate) fn cut_colour() -> Color32 {
    argb(0xFFE8_9A5B)
}

/// The plot area inside the display `r` (under the legend, above the frequency labels).
pub(crate) fn plot_rect(r: Rect, compact: bool) -> Rect {
    if compact {
        let b = r.shrink2(vec2(8.0, 8.0));
        Rect::from_min_max(pos2(b.left(), b.top() + 18.0), pos2(b.right(), b.bottom() - 13.0))
    } else {
        let b = r.shrink2(vec2(12.0, 10.0));
        Rect::from_min_max(pos2(b.left(), b.top() + 22.0), pos2(b.right(), b.bottom() - 16.0))
    }
}

/// The highest point of the curves between 40 Hz and 16 kHz (the sounds' scale hangs from it).
fn top_of(curves: &[Option<&Curve>]) -> Option<f32> {
    let mut top = f32::NEG_INFINITY;
    for c in curves.iter().flatten() {
        for (i, v) in c.iter().enumerate() {
            if (40.0..=16_000.0).contains(&grid_hz(i)) && v.is_finite() && *v > FLOOR_DB + 1.0 {
                top = top.max(*v);
            }
        }
    }
    top.is_finite().then_some(top)
}

fn sound_points(plot: Rect, c: &Curve, top: f32) -> Vec<Pos2> {
    (0..GRID_N)
        .filter_map(|i| {
            let v = c.get(i).copied().filter(|v| v.is_finite())?;
            let t = ((top + 6.0 - v) / SOUND_SPAN_DB).clamp(0.0, 1.0);
            Some(pos2(freq_to_x(plot, grid_hz(i)), plot.top() + t * plot.height()))
        })
        .collect()
}

fn gain_points(plot: Rect, c: &Curve, scale: f32) -> Vec<Pos2> {
    (0..GRID_N).map(|i| pos2(freq_to_x(plot, grid_hz(i)), db_y(plot, c.get(i).copied().unwrap_or(0.0), scale))).collect()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw(ui: &mut Ui, r: Rect, pal: &VstPalette, shown: &Shown, knobs: Knobs, dragging: &mut Option<usize>, compact: bool) -> Out {
    let p = ui.painter().clone();
    paint_panel(&p, r, 10.0);
    let b = r.shrink2(if compact { vec2(8.0, 8.0) } else { vec2(12.0, 10.0) });
    let plot = plot_rect(r, compact);
    let (lo, hi) = if knobs.range.0 <= knobs.range.1 { knobs.range } else { (knobs.range.1, knobs.range.0) };
    let scale = if knobs.scale_db.is_finite() { knobs.scale_db.clamp(6.0, 24.0) } else { 12.0 };

    // ---- legend and status
    let head_y = b.top() + 9.0;
    let mut x = b.left();
    let second = shown.second.unwrap_or("");
    let mut legend: Vec<(Color32, &str)> = if compact {
        vec![(pal.accent, "Target"), (pal.text_bright, "Applied")]
    } else {
        vec![
            (al(pal.text_body, 0.45), "This track"),
            (pal.accent, "Target"),
            (pal.text_bright, if shown.second.is_some() { shown.first } else { "Applied" }),
        ]
    };
    if shown.second.is_some() && !compact {
        legend.push((al(pal.text_bright, 0.55), second));
    }
    for (c, t) in legend {
        p.rect_filled(Rect::from_center_size(pos2(x + 5.0, head_y), vec2(10.0, 10.0)), CornerRadius::same(2), c);
        let g = p.text(pos2(x + 15.0, head_y), Align2::LEFT_CENTER, t, regular(CAPTION_SIZE), pal.text_body);
        x = g.right() + 14.0;
    }
    let status_clip = Rect::from_min_max(pos2(x + 4.0, b.top()), pos2(b.right(), plot.top()));
    p.with_clip_rect(status_clip.intersect(p.clip_rect())).text(
        pos2(b.right(), head_y),
        Align2::RIGHT_CENTER,
        &shown.status,
        regular(CAPTION_SIZE),
        pal.text_body,
    );

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
    let half = (scale / 2.0).round();
    for db in [-half, half] {
        let y = db_y(plot, db, scale);
        p.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], grid);
        p.text(pos2(plot.left() + 3.0, y - 1.0), Align2::LEFT_BOTTOM, format!("{db:+.0} dB"), regular(TINY_SIZE), pal.text_faint);
    }

    // ---- the two sounds, behind the response
    let pc = p.with_clip_rect(plot.intersect(p.clip_rect()));
    let (src, tgt) = (shown.source.as_ref().and_then(|c| c.first()), shown.target.as_ref().and_then(|c| c.first()));
    if let Some(top) = top_of(&[src, tgt]) {
        if let Some(c) = src {
            let pts = sound_points(plot, c, top);
            for w in pts.windows(2) {
                if let [a, c] = w {
                    pc.add(Shape::convex_polygon(
                        vec![*a, *c, pos2(c.x, plot.bottom()), pos2(a.x, plot.bottom())],
                        al(pal.text_body, 0.14),
                        Stroke::NONE,
                    ));
                }
            }
            pc.add(Shape::line(pts, Stroke::new(1.0, al(pal.text_body, 0.4))));
        }
        if let Some(c) = tgt {
            pc.add(Shape::line(sound_points(plot, c, top), Stroke::new(1.4, al(pal.accent, 0.85))));
        }
    } else if !compact {
        pc.text(
            plot.center_top() + vec2(0.0, 14.0),
            Align2::CENTER_TOP,
            "Play to see this track; pick a profile to see its target",
            regular(CAPTION_SIZE),
            pal.text_faint,
        );
    }

    // ---- the response applied
    let zero = db_y(plot, 0.0, scale);
    pc.line_segment([pos2(plot.left(), zero), pos2(plot.right(), zero)], Stroke::new(1.0, al(Color32::WHITE, 0.22)));
    if let Some(applied) = &shown.applied {
        if let Some(a) = applied.first() {
            let pts = gain_points(plot, a, scale);
            for w in pts.windows(2) {
                if let [a, c] = w {
                    let fill = |a: Pos2, c: Pos2| {
                        let col = if (a.y + c.y) * 0.5 > zero { cut_colour() } else { pal.accent };
                        pc.add(Shape::convex_polygon(vec![a, c, pos2(c.x, zero), pos2(a.x, zero)], al(col, 0.30), Stroke::NONE));
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
        if let (Some(_), Some(bc)) = (shown.second, applied.get(1)) {
            pc.add(Shape::line(gain_points(plot, bc, scale), Stroke::new(1.2, al(pal.text_bright, 0.55))));
        }
    }

    // ---- the working range: dimmed outside
    let (x_lo, x_hi) = (freq_to_x(plot, lo), freq_to_x(plot, hi));
    let shade = al(Color32::BLACK, 0.45);
    if x_lo > plot.left() + 0.5 {
        pc.rect_filled(Rect::from_min_max(plot.min, pos2(x_lo, plot.bottom())), 0.0, shade);
    }
    if x_hi < plot.right() - 0.5 {
        pc.rect_filled(Rect::from_min_max(pos2(x_hi, plot.top()), plot.max), 0.0, shade);
    }

    // ---- the four ranges: each one's amount along the bottom, where it is not 100 %
    let mut bounds = [plot.left(), 0.0, 0.0, 0.0, plot.right()];
    for (i, s) in knobs.splits.iter().enumerate() {
        if let Some(slot) = bounds.get_mut(i + 1) {
            *slot = freq_to_x(plot, *s);
        }
    }
    if !compact {
        for (i, w) in bounds.windows(2).enumerate() {
            let amount = knobs.amounts.get(i).copied().unwrap_or(100.0);
            if let [a, c] = w
                && (amount - 100.0).abs() > 0.5
                && c - a > 34.0
            {
                pc.text(
                    pos2((a + c) * 0.5, plot.bottom() - 3.0),
                    Align2::CENTER_BOTTOM,
                    format!("{amount:.0} %"),
                    regular(TINY_SIZE),
                    al(pal.accent, 0.9),
                );
            }
        }
    }

    // ---- the handles: the splits (short, at the bottom), then the range's edges (full height)
    let mut out = Out::default();
    let mut handle = |ui: &mut Ui, which: usize, ex: f32, full: bool, dragging: &mut Option<usize>| {
        let reach = if full {
            Rect::from_min_max(pos2(ex - GRAB, plot.top()), pos2(ex + GRAB, plot.bottom()))
        } else {
            Rect::from_min_max(pos2(ex - GRAB, plot.bottom() - 30.0), pos2(ex + GRAB, plot.bottom()))
        };
        let resp = ui.interact(reach, ui.id().with(("copyq-handle", which)), Sense::click_and_drag());
        let hot = resp.hovered() || *dragging == Some(which);
        if hot {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
        if resp.drag_started() {
            *dragging = Some(which);
            out.started = true;
        }
        if resp.dragged()
            && *dragging == Some(which)
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let hz = x_to_freq(plot, pos.x);
            let k = 2f32.powf(MIN_SPAN_OCT);
            let hz = match which {
                EDGE_LO => hz.min(hi / k),
                EDGE_HI => hz.max(lo * k),
                _ => hz,
            };
            out.moved = Some((which, hz.clamp(20.0, 20_000.0)));
        }
        if resp.drag_stopped() {
            *dragging = None;
            out.ended = true;
        }
        if resp.double_clicked() {
            // Back to where it starts.
            let home = match which {
                EDGE_LO => 20.0,
                EDGE_HI => 20_000.0,
                w => [150.0, 1_000.0, 6_000.0].get(w - SPLIT).copied().unwrap_or(1_000.0),
            };
            out.started = true;
            out.moved = Some((which, home));
            out.ended = true;
        }
        hot
    };
    if !compact {
        for (i, s) in knobs.splits.iter().enumerate() {
            let ex = freq_to_x(plot, *s);
            let hot = handle(ui, SPLIT + i, ex, false, dragging);
            let col = if hot { pal.accent } else { al(pal.text_body, 0.55) };
            p.line_segment([pos2(ex, plot.bottom() - 30.0), pos2(ex, plot.bottom())], Stroke::new(if hot { 2.0 } else { 1.0 }, col));
            p.circle_filled(pos2(ex, plot.bottom() - 30.0), 3.0, col);
            if hot {
                p.text(pos2(ex, plot.bottom() - 36.0), Align2::CENTER_BOTTOM, hz_text(*s), regular(TINY_SIZE), pal.text_bright);
            }
        }
    }
    for (edge, ex) in [(EDGE_LO, x_lo), (EDGE_HI, x_hi)] {
        let hot = handle(ui, edge, ex, true, dragging);
        let col = if hot { pal.accent } else { al(pal.accent, 0.6) };
        p.line_segment([pos2(ex, plot.top()), pos2(ex, plot.bottom())], Stroke::new(if hot { 2.0 } else { 1.0 }, col));
        p.rect_filled(Rect::from_center_size(pos2(ex, plot.center().y), vec2(6.0, 28.0)), CornerRadius::same(3), col);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plot_sits_inside_the_display_and_the_sounds_hang_from_their_top() {
        let r = Rect::from_min_size(pos2(24.0, 55.0), vec2(700.0, 300.0));
        for compact in [false, true] {
            assert!(r.contains_rect(plot_rect(r, compact)));
        }
        let plot = plot_rect(r, false);
        let mut c = [-40.0f32; GRID_N];
        c[60] = -20.0;
        assert_eq!(top_of(&[Some(&c), None]), Some(-20.0));
        assert_eq!(top_of(&[None]), None);
        assert_eq!(top_of(&[Some(&[FLOOR_DB; GRID_N])]), None, "nothing measured has no top");
        let pts = sound_points(plot, &c, -20.0);
        assert_eq!(pts.len(), GRID_N);
        // The top sits 6 dB under the plot's top; 60 dB further down is its bottom.
        assert!((pts[60].y - (plot.top() + 0.1 * plot.height())).abs() < 0.5);
        assert!(pts.iter().all(|p| plot.expand(0.5).contains(*p)));
        let g = gain_points(plot, &[6.0; GRID_N], 12.0);
        assert!((g[0].y - (plot.center().y - plot.height() * 0.25)).abs() < 0.5);
    }
}
