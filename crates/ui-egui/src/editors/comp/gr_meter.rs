//! `CompControls.h` `GrMeter`: the needle VU with a numbered, non-linear gain-reduction scale, and
//! its ballistics.

use super::{baseline_text, polyline};
use crate::editor_kit::look::paint_panel;
use crate::editor_kit::palette::{al, argb};
use crate::theme::regular;
use egui::{Align2, Color32, Painter, Pos2, Rect, Stroke, pos2};

const K_A0: f32 = 3.83274; // pi * 1.22, lower left
const K_A1: f32 = 5.59203; // pi * 1.78, lower right
const K_MAX_DB: f32 = 20.0;

/// The meter's shown values (`grShown`, `inShown`, `outShown`).
#[derive(Clone, Copy, Debug)]
pub struct Meter {
    pub gr: f32,
    pub input: f32,
    pub output: f32,
}

impl Default for Meter {
    fn default() -> Self {
        Meter { gr: 0.0, input: -96.0, output: -96.0 }
    }
}

impl Meter {
    /// `setValues`: ballistics straight from the original, snap up, ease down. Non-finite
    /// readings are ignored.
    pub fn set_values(&mut self, gr_db: f32, in_db: f32, out_db: f32) {
        let step = |shown: &mut f32, v: f32, up: f32, down: f32| {
            if v.is_finite() {
                *shown += (v - *shown) * if v > *shown { up } else { down };
            }
        };
        step(&mut self.gr, gr_db, 0.55, 0.12);
        step(&mut self.input, in_db, 0.6, 0.1);
        step(&mut self.output, out_db, 0.6, 0.1);
    }
}

fn pos(db: f32) -> f32 {
    let t = (db.clamp(0.0, K_MAX_DB) / K_MAX_DB).powf(0.62);
    K_A1 - t * (K_A1 - K_A0)
}

/// `GrMeter::paint`.
pub fn paint(p: &Painter, b: Rect, accent: Color32, m: &Meter) {
    paint_panel(p, b, crate::editor_kit::look::PANEL_CORNER);
    let (w, h) = (b.width(), b.height());
    let u = w.min(h * 1.55) / 100.0;
    // pivot just below the bottom edge, so the needle sweeps the upper area
    let (cx, cy) = (b.left() + w * 0.5, b.top() + h * 0.98);
    let r = (w * 0.46).min(h * 0.86);
    let pt = |a: f32, rad: f32| pos2(cx + a.cos() * rad, cy + a.sin() * rad);
    let arc = |from: f32, to: f32| -> Vec<Pos2> { (0..=48).map(|i| pt(from + (to - from) * i as f32 / 48.0, r)).collect() };
    let gr = if m.gr.is_finite() { m.gr } else { 0.0 };

    // --- scale arc
    polyline(p, arc(K_A0, K_A1), Stroke::new(1.2 * u, al(Color32::WHITE, 0.14)));
    // --- the part of the scale the needle has swept
    if gr > 0.02 {
        polyline(p, arc(pos(gr), K_A1), Stroke::new(2.2 * u, al(accent, 0.85)));
    }
    // --- ticks and numbers
    let label_font = regular((4.2 * u).max(7.0));
    for db in [0, 1, 2, 3, 5, 7, 10, 15, 20] {
        let a = pos(db as f32);
        p.line_segment([pt(a, r - 3.0 * u), pt(a, r + 2.5 * u)], Stroke::new(0.8 * u, al(argb(0xFFEBEBE6), 0.5)));
        let c = pt(a, r + 8.0 * u);
        p.text(c, Align2::CENTER_CENTER, db.to_string(), label_font.clone(), al(argb(0xFFEBEBE6), 0.62));
    }
    // --- needle
    let a = pos(gr);
    p.line_segment([pt(a, r * 0.18), pt(a, r - 5.0 * u)], Stroke::new(1.1 * u, argb(0xFFF4F1E8)));

    // --- readout, low in the dial like the original's absolutely-positioned block
    let num = format!("{:.1}", if gr < 0.05 { 0.0 } else { -gr });
    let num_font = regular((11.0 * u).max(14.0));
    let unit_font = regular((4.6 * u).max(8.0));
    let num_w = p.layout_no_wrap(num.clone(), num_font.clone(), Color32::WHITE).size().x;
    let unit_w = p.layout_no_wrap("dB reduction".into(), unit_font.clone(), Color32::WHITE).size().x;
    let gap = 3.0 * u;
    let x = cx - (num_w + gap + unit_w) * 0.5;
    let baseline = b.top() + h * 0.94 - 4.0 * u;
    baseline_text(p, pos2(x, baseline), &num, num_font, argb(0xFFF4F1E8));
    baseline_text(p, pos2(x + num_w + gap, baseline), "dB reduction", unit_font, argb(0xFF8D8B84));
}
