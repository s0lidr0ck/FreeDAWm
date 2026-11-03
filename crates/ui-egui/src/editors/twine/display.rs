//! `TwineDisplay`: the last ten seconds, scrolling. What went in (grey), what came out (straw),
//! the ceiling, and underneath how much the clipper (orange) and the limiter (blue) each took off.

use super::{ACCENT, CLIP_COL, IN_COL, LIM_COL, PALETTE, PANEL_FILL, PANEL_LINE, brighter};
use crate::editor_kit::icons::fill_strip;
use crate::editor_kit::palette::al;
use crate::theme::regular;
use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};

/// 10 s at the editor's 30 Hz.
pub const COLUMNS: usize = 300;

/// One column of history.
#[derive(Clone, Copy, Debug)]
pub struct Column {
    pub in_db: f32,
    pub out_db: f32,
    pub clip_db: f32,
    pub lim_db: f32,
}

impl Default for Column {
    fn default() -> Self {
        Column { in_db: -120.0, out_db: -120.0, clip_db: 0.0, lim_db: 0.0 }
    }
}

/// The history ring (`hist`, `head`).
#[derive(Clone)]
pub struct History {
    hist: Vec<Column>,
    head: usize,
    /// Scratch points, reused every frame.
    pts: Vec<Pos2>,
}

impl Default for History {
    fn default() -> Self {
        History { hist: vec![Column::default(); COLUMNS], head: 0, pts: Vec::with_capacity(COLUMNS) }
    }
}

impl History {
    /// `push`.
    pub fn push(&mut self, c: Column) {
        let fix = |v: f32, d: f32| if v.is_finite() { v } else { d };
        if let Some(slot) = self.hist.get_mut(self.head) {
            *slot = Column { in_db: fix(c.in_db, -120.0), out_db: fix(c.out_db, -120.0), clip_db: fix(c.clip_db, 0.0), lim_db: fix(c.lim_db, 0.0) };
        }
        self.head = (self.head + 1) % COLUMNS;
    }

    fn at(&self, i: usize) -> Column {
        self.hist.get((self.head + i) % COLUMNS).copied().unwrap_or_default()
    }
}

/// `TwineDisplay::paint`.
pub fn paint(p: &egui::Painter, b: Rect, h: &mut History, ceiling_db: f32) {
    p.rect_filled(b, CornerRadius::same(5), PANEL_FILL);
    p.rect_stroke(b.shrink(0.5), CornerRadius::same(5), Stroke::new(1.0, PANEL_LINE), StrokeKind::Middle);
    let inner = b.shrink2(vec2(10.0, 8.0));
    let gr_h = inner.height() * 0.30;
    let gr_area = Rect::from_min_max(pos2(inner.left(), inner.bottom() - gr_h), inner.max);
    let lvl = Rect::from_min_max(inner.min, pos2(inner.right(), gr_area.top() - 6.0));

    // ---- level: 0 at the top, -36 dBFS at the bottom
    let y_of = |db: f32| lvl.top() + lvl.height() * (-db / 36.0).clamp(0.0, 1.0);
    let x_of = |i: usize| lvl.left() + lvl.width() * i as f32 / (COLUMNS - 1) as f32;
    for db in [-6.0, -12.0, -18.0, -24.0, -30.0] {
        let y = y_of(db).round() + 0.5;
        p.line_segment([pos2(lvl.left(), y), pos2(lvl.right(), y)], Stroke::new(1.0, al(Color32::WHITE, 0.04)));
    }
    for db in [-6.0f32, -12.0, -24.0] {
        let r = Rect::from_min_size(pos2(lvl.left() + 2.0, y_of(db) - 11.0), vec2(30.0, 10.0));
        p.text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, format!("{}", db as i32), regular(9.0), PALETTE.text_faint);
    }
    let area = |get: fn(&Column) -> f32, c: Color32, h: &mut History| {
        h.pts.clear();
        for i in 0..COLUMNS {
            let col = h.at(i);
            h.pts.push(pos2(x_of(i), y_of(get(&col))));
        }
        fill_strip(p, &h.pts, |_| lvl.bottom(), al(c, 0.55));
    };
    area(|c| c.in_db, IN_COL, h);
    area(|c| c.out_db, ACCENT, h);

    // the ceiling
    let cy = y_of(ceiling_db);
    p.extend(egui::Shape::dashed_line(&[pos2(lvl.left(), cy), pos2(lvl.right(), cy)], Stroke::new(1.2, al(ACCENT, 0.9)), 5.0, 4.0));
    let cr = Rect::from_min_size(pos2(lvl.right() - 100.0, cy + 2.0), vec2(98.0, 11.0));
    p.text(pos2(cr.right(), cr.center().y), Align2::RIGHT_CENTER, format!("ceiling {ceiling_db:.1}"), regular(9.5), al(ACCENT, 0.9));

    // ---- what each stage took off: 0 at the top of the strip, 12 dB at the bottom
    p.rect_filled(gr_area, CornerRadius::same(3), al(Color32::BLACK, 0.25));
    let gy = |db: f32| gr_area.top() + gr_area.height() * (db / 12.0).clamp(0.0, 1.0);
    h.pts.clear();
    for i in 0..COLUMNS {
        let col = h.at(i);
        h.pts.push(pos2(x_of(i), gy(col.clip_db)));
    }
    fill_strip(p, &h.pts, |_| gr_area.top(), al(CLIP_COL, 0.45));
    h.pts.clear();
    for i in 0..COLUMNS {
        let col = h.at(i);
        h.pts.push(pos2(x_of(i), gy(col.lim_db)));
    }
    p.add(egui::Shape::line(h.pts.clone(), Stroke::new(1.5, LIM_COL)));

    let bottom_row = |r: Rect, left: f32| Rect::from_min_max(pos2(r.left() + left, r.bottom() - 12.0), r.max);
    let f = regular(9.5);
    let left_text = |r: Rect, t: &str, c: Color32| {
        p.text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, t, f.clone(), c);
    };
    left_text(bottom_row(gr_area, 4.0), "clip", CLIP_COL);
    left_text(bottom_row(gr_area, 34.0), "limit", LIM_COL);
    left_text(bottom_row(lvl, 40.0), "driven input", brighter(IN_COL, 0.6));
    left_text(bottom_row(lvl, 120.0), "output", ACCENT);
    let r = bottom_row(gr_area, 0.0);
    p.text(pos2(r.right() - 4.0, r.center().y), Align2::RIGHT_CENTER, "12 dB", f, PALETTE.text_faint);
}
