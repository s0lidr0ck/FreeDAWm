//! `ShaleDisplay.h`, translated: the waveform on a mirrored dB axis (-66..0 dB across the half
//! height), the input dim and the gated output bright over it, the detector as a line, the
//! threshold line you drag (with its tag), the strip where the gate was open, a faint line a
//! second, and the two view buttons (span 2.5 / 5 / 10 s, Freeze).
//!
//! The bins are the engine's own (`ShaleDSP::scope`, 4 ms each, pushed to its analysis ring as
//! `[peak, gain, det]` and kept by `module_graphs::Streams`); columns merge them as the C++ does
//! (`shale_ui::columns`: the loudest peak, the most-closed gate, the loudest detector).

use crate::editor_kit::look::tracked_text;
use crate::editor_kit::palette::{VstPalette, al, argb, rgb};
use crate::shale_ui::{BINS_PER_SECOND, SCOPE_BINS, SCOPE_VALUES, columns, norm, to_db};
use egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};
use std::collections::VecDeque;

const GROUND: Color32 = rgb(0x0C0F12);
const GRID: Color32 = rgb(0x171C21);
const SECOND: Color32 = rgb(0x151A1F);
const INPUT: Color32 = rgb(0x2B333B);
const DETECTOR: Color32 = rgb(0x7FD4FF);
const ORANGE: Color32 = rgb(0xFFB454);
const TAG: Color32 = rgb(0x1A1E23);
const BUTTON_OFF: Color32 = argb(0xE010_1317);
const BUTTON_EDGE: Color32 = rgb(0x2A3038);
const EDGE: Color32 = rgb(0x1F252B);

/// `spanButton()` in the display's bounds.
pub fn span_button(bounds: Rect) -> Rect {
    Rect::from_min_size(bounds.min + vec2(8.0, 7.0), vec2(50.0, 16.0))
}

/// `freezeButton()`.
pub fn freeze_button(bounds: Rect) -> Rect {
    Rect::from_min_size(bounds.min + vec2(62.0, 7.0), vec2(54.0, 16.0))
}

/// `(r, mid, half)`: the drawn area (`reduced (1)`), the centre line and the half height.
pub fn geometry(bounds: Rect) -> (Rect, f32, f32) {
    let r = bounds.shrink(1.0);
    (r, r.center().y, r.height() * 0.5 - 3.0)
}

/// `dragTo`: the threshold value at pointer height `y` (above or below the centre: mirrored).
pub fn threshold_at(bounds: Rect, y: f32) -> Option<f32> {
    let (_, mid, half) = geometry(bounds);
    (half > 1.0 && y.is_finite()).then(|| ((y - mid).abs() / half).clamp(0.0, 1.0))
}

/// What the display draws this frame.
pub struct View<'a> {
    pub bins: &'a VecDeque<[f32; SCOPE_VALUES]>,
    pub seconds: f32,
    pub frozen: bool,
    pub thresh_db: f32,
    pub dragging: bool,
}

/// `paint`. `cols` is the editor's reused column buffer.
pub fn paint(p: &Painter, bounds: Rect, pal: &VstPalette, v: &View, cols: &mut Vec<[f32; SCOPE_VALUES]>) {
    let (r, mid, half) = geometry(bounds);
    p.rect_filled(r, CornerRadius::same(4), GROUND);
    if r.width() < 4.0 || half <= 0.5 {
        return;
    }
    let p = p.with_clip_rect(bounds.intersect(p.clip_rect()));
    let x0 = r.left() + 1.0;
    let w = r.width() - 2.0;
    let seconds = if v.seconds.is_finite() { v.seconds.clamp(2.5, 10.0) } else { 5.0 };
    let shown = ((seconds * BINS_PER_SECOND).round() as usize).clamp(10, SCOPE_BINS);

    // ---- grid, every 12 dB
    for db in [-12.0f32, -24.0, -36.0, -48.0, -60.0] {
        let h = half * norm(db);
        for y in [(mid - h).round(), (mid + h).round()] {
            p.line_segment([pos2(r.left(), y), pos2(r.right(), y)], Stroke::new(1.0, GRID));
        }
    }
    // ---- a faint line every second, counted back from now at the right edge
    let mut s = 1.0f32;
    while s < seconds {
        let x = (x0 + w * (1.0 - s / seconds)).round();
        p.line_segment([pos2(x, r.top() + 2.0), pos2(x, r.bottom() - 2.0)], Stroke::new(1.0, SECOND));
        s += 1.0;
    }
    // ---- merge bins into pixel columns
    let n = (w.round() as usize).clamp(1, 2048);
    cols.resize(n, [0.0, 1.0, 0.0]);
    columns(v.bins, shown, &mut cols[..]);
    // ---- the waveform: input dim, output bright
    for (c, b) in cols.iter().enumerate() {
        let x = x0 + c as f32;
        let in_h = half * norm(to_db(b[0]));
        if in_h <= 0.0 {
            continue;
        }
        p.rect_filled(Rect::from_min_size(pos2(x, mid - in_h), vec2(1.0, in_h * 2.0)), CornerRadius::ZERO, INPUT);
        let out_h = half * norm(to_db(b[0] * b[1]));
        if out_h > 0.0 {
            p.rect_filled(Rect::from_min_size(pos2(x, mid - out_h), vec2(1.0, out_h * 2.0)), CornerRadius::ZERO, al(pal.accent, 0.92));
        }
    }
    // ---- the detector, as a line: what the threshold is compared against
    let det_at = |c: usize, b: &[f32; SCOPE_VALUES]| pos2(x0 + c as f32, mid - half * norm(to_db(b[2])));
    for (c, pair) in cols.windows(2).enumerate() {
        p.line_segment([det_at(c, &pair[0]), det_at(c + 1, &pair[1])], Stroke::new(1.0, al(DETECTOR, 0.55)));
    }
    // ---- the threshold line
    let ty = mid - half * norm(v.thresh_db);
    let orange = al(ORANGE, if v.dragging { 1.0 } else { 0.85 });
    p.line_segment([pos2(r.left(), ty), pos2(r.right(), ty)], Stroke::new(if v.dragging { 2.0 } else { 1.4 }, orange));
    p.line_segment([pos2(r.left(), mid + (mid - ty)), pos2(r.right(), mid + (mid - ty))], Stroke::new(0.7, orange));
    let tag = Rect::from_min_size(pos2(r.right() - 62.0, ty - 8.0), vec2(56.0, 16.0));
    p.rect_filled(tag, CornerRadius::same(3), TAG);
    tracked_text(&p, tag, Align2::CENTER_CENTER, &format!("{:.1} dB", v.thresh_db), 9.5, 0.04, false, false, ORANGE);
    // ---- a strip along the bottom showing where the gate was open
    let sy = r.bottom() - 4.0;
    for (c, b) in cols.iter().enumerate() {
        if b[1] >= 0.5 {
            p.rect_filled(Rect::from_min_size(pos2(x0 + c as f32, sy), vec2(1.0, 3.0)), CornerRadius::ZERO, al(pal.accent, 0.35));
        }
    }
    // ---- the two view controls, top left
    let button = |b: Rect, text: &str, on: bool| {
        p.rect_filled(b, CornerRadius::same(3), if on { pal.accent } else { BUTTON_OFF });
        p.rect_stroke(b.shrink(0.5), CornerRadius::same(3), Stroke::new(1.0, if on { pal.accent } else { BUTTON_EDGE }), StrokeKind::Middle);
        tracked_text(&p, b, Align2::CENTER_CENTER, text, 9.0, 0.06, true, false, if on { pal.on_accent } else { pal.text_dim });
    };
    let span_text = if seconds < 3.0 { "2.5 sec".to_string() } else { format!("{} sec", seconds.round() as i32) };
    button(span_button(bounds), &span_text, false);
    button(freeze_button(bounds), if v.frozen { "Frozen" } else { "Freeze" }, v.frozen);
    p.rect_stroke(r, CornerRadius::same(4), Stroke::new(1.0, EDGE), StrokeKind::Middle);
}

/// `mouseDown` on the span button: 2.5 → 5 → 10 → 2.5.
pub fn next_span(s: f32) -> f32 {
    if s < 3.0 {
        5.0
    } else if s < 7.0 {
        10.0
    } else {
        2.5
    }
}

/// A point inside the display, for tests: `db` above the centre line.
pub fn point_at_db(bounds: Rect, db: f32) -> Pos2 {
    let (r, mid, half) = geometry(bounds);
    pos2(r.center().x, mid - half * norm(db))
}
