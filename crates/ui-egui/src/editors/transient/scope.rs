//! `TransientScope.h` (`fs::transient::TransientScope`): the last 2.5 s scrolling. Top: the
//! input (dim, filled) and output (accent line) peaks in dB (−60 at the floor); bottom: the
//! attack and sustain gains applied (+18 at the top, −24 at the bottom, 0 marked). Read from the
//! module's scope bins (the DSP pushes one per 4 ms bin to its analysis ring: in, out, attack dB,
//! sustain dB), reduced per pixel column as the C++ does (`module_graphs::transient_columns`, the
//! same translation).

use crate::editor_kit::icons::fill_strip;
use crate::editor_kit::look::{CAPTION_SIZE, TINY_SIZE, paint_panel};
use crate::editor_kit::palette::{VstPalette, al, argb};
use crate::module_graphs::{TRANSIENT_VALUES, transient_columns};
use crate::theme::regular;
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Shape, Stroke, pos2, vec2};
use std::collections::VecDeque;

/// `kSeconds`.
const K_SECONDS: f32 = 2.5;
/// `TransientEngineDSP::scopeBins`.
const SCOPE_BINS: i32 = 2500;

/// `susColour`.
fn sus_colour(pal: &VstPalette) -> Color32 {
    pal.text_body
}

/// Paints the scope in `r` from `bins` (newest last); `sr`: the session's sample rate (the bins
/// per second: `sr / round (0.004 sr)`).
pub(crate) fn draw(p: &egui::Painter, r: Rect, pal: &VstPalette, bins: Option<&VecDeque<[f32; TRANSIENT_VALUES]>>, sr: f64) {
    paint_panel(p, r, 10.0);
    let mut b = r.shrink2(vec2(12.0, 10.0));

    // legend
    let head = Rect::from_min_size(b.min, vec2(b.width(), 18.0));
    p.text(pos2(head.left(), head.center().y), Align2::LEFT_CENTER, "Signal", regular(CAPTION_SIZE), pal.text_body);
    let mut right = head.right();
    for (c, t) in [(sus_colour(pal), "Sustain"), (pal.accent, "Attack")] {
        let cell = Rect::from_min_max(pos2(right - 78.0, head.top()), pos2(right, head.bottom()));
        right -= 78.0;
        p.rect_filled(Rect::from_center_size(pos2(cell.left() + 6.0, cell.center().y), vec2(10.0, 10.0)), CornerRadius::same(2), c);
        p.text(pos2(cell.left() + 17.0, cell.center().y), Align2::LEFT_CENTER, t, regular(CAPTION_SIZE), pal.text_body);
    }
    b.min.y += 18.0 + 4.0;

    let caption = Rect::from_min_max(pos2(b.left(), b.bottom() - 14.0), b.max);
    b.max.y -= 14.0 + 4.0;
    let lane_h = 40f32.max(b.height() * 0.36);
    let lane = Rect::from_min_max(pos2(b.left(), b.bottom() - lane_h), b.max);
    b.max.y -= lane_h + 6.0;
    let wave = b;

    let well = argb(0xFF0C0F12);
    p.rect_filled(wave, CornerRadius::same(3), well);
    p.rect_filled(lane, CornerRadius::same(3), well);

    let per_second = {
        let hop = (0.004 * sr).round().max(1.0);
        ((sr / hop) as f32).max(1.0)
    };
    let shown = ((K_SECONDS * per_second) as i32).clamp(16, SCOPE_BINS - 1) as usize;
    let w = (wave.width() as usize).max(1);

    let y_wave = |peak: f32| {
        let db = if peak > 1.0e-6 { 20.0 * peak.log10() } else { -120.0 };
        let t = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        wave.bottom() - t * wave.height()
    };
    let y_lane = |db: f32| {
        let t = ((18.0 - db) / 42.0).clamp(0.0, 1.0);
        lane.top() + t * lane.height()
    };
    let zero = y_lane(0.0);
    p.line_segment([pos2(lane.left(), zero.round() + 0.5), pos2(lane.right(), zero.round() + 0.5)], Stroke::new(1.0, al(pal.text_faint, 0.35)));

    let empty = VecDeque::new();
    let cols = transient_columns(bins.unwrap_or(&empty), shown, w);
    let mut ins: Vec<Pos2> = Vec::with_capacity(w + 2);
    let mut outs: Vec<Pos2> = Vec::with_capacity(w + 1);
    outs.push(pos2(wave.left(), wave.bottom()));
    ins.push(pos2(wave.left(), wave.bottom()));
    for (px, c) in cols.iter().enumerate() {
        let x = wave.left() + px as f32;
        ins.push(pos2(x, y_wave(c[0])));
        outs.push(pos2(x, y_wave(c[1])));
        let (att, sus) = (c[2], c[3]);
        let xl = x.trunc() + 0.5;
        if sus.abs() > 0.05 {
            p.line_segment([pos2(xl, zero.min(y_lane(sus))), pos2(xl, zero.max(y_lane(sus)))], Stroke::new(1.0, al(sus_colour(pal), 0.75)));
        }
        if att.abs() > 0.05 {
            p.line_segment([pos2(xl, zero.min(y_lane(att))), pos2(xl, zero.max(y_lane(att)))], Stroke::new(1.0, al(pal.accent, 0.9)));
        }
    }
    ins.push(pos2(wave.right(), wave.bottom()));
    fill_strip(p, &ins, |_| wave.bottom(), al(pal.text_faint, 0.35));
    p.add(Shape::line(outs, Stroke::new(1.3, pal.accent)));

    // scales
    let f = regular(TINY_SIZE);
    p.text(pos2(wave.right() - 4.0, wave.top()), Align2::RIGHT_TOP, "0 dB", f.clone(), pal.text_faint);
    p.text(pos2(wave.right() - 4.0, wave.bottom() - 7.0), Align2::RIGHT_CENTER, "-60", f.clone(), pal.text_faint);
    p.text(pos2(lane.right() - 4.0, lane.top()), Align2::RIGHT_TOP, "+18", f.clone(), pal.text_faint);
    p.text(pos2(lane.right() - 4.0, lane.bottom() - 7.0), Align2::RIGHT_CENTER, "-24", f, pal.text_faint);
    p.with_clip_rect(caption).text(
        pos2(caption.left(), caption.center().y),
        Align2::LEFT_CENTER,
        "Last 2.5 s. Top: in (dim) and out. Bottom: the gain applied, in dB.",
        regular(CAPTION_SIZE),
        pal.text_faint,
    );
}
