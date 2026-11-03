//! `CompControls.h` `CurveDisplay`: the static transfer curve, the threshold marker, the operating
//! point and the in / out level bars down its right-hand edge.

use super::model::{self, CompValues};
use super::polyline;
use crate::editor_kit::look::paint_panel;
use crate::editor_kit::palette::{al, argb};
use crate::theme::regular;
use egui::{Align2, Color32, Painter, Pos2, Rect, Stroke, pos2, vec2};

/// Left / bottom of the plot, in dB.
const K_LO: f32 = -60.0;

/// The curve's points (`levIn`, `levOut`), cached by the values they depend on.
#[derive(Clone, Default)]
pub struct CurveCache {
    key: Option<u64>,
    pts: Vec<(f32, f32)>,
}

impl CurveCache {
    fn points(&mut self, v: &CompValues) -> &[(f32, f32)] {
        let key = v.key();
        if self.key != Some(key) {
            self.key = Some(key);
            self.pts.clear();
            for i in 0..=120 {
                let lev_in = K_LO + i as f32 * 0.5;
                let lev_out = model::static_out(v, lev_in);
                if lev_out.is_finite() {
                    self.pts.push((lev_in, lev_out));
                }
            }
        }
        &self.pts
    }
}

/// `CurveDisplay::paint` with `inShown`, `outShown`, `grShown`.
#[allow(clippy::too_many_arguments)]
pub fn paint(p: &Painter, b: Rect, accent: Color32, v: &CompValues, cache: &mut CurveCache, in_shown: f32, out_shown: f32, gr_shown: f32) {
    paint_panel(p, b, crate::editor_kit::look::PANEL_CORNER);
    let (w, h) = (b.width(), b.height());
    let u = w.min(h) / 100.0;
    let pad = 9.0 * u;
    let bar_w = 3.2 * u;
    let gw = w - pad * 2.0 - bar_w * 2.0 - 6.0 * u;
    let gh = h - pad * 2.0;
    let to_x = |db: f32| b.left() + pad + (db.clamp(K_LO, 0.0) - K_LO) / -K_LO * gw;
    let to_y = |db: f32| b.top() + pad + gh - (db.clamp(K_LO, 6.0) - K_LO) / -K_LO * gh * (60.0 / 66.0);

    // --- grid every 12 dB, with the input scale labelled along the bottom
    let axis = regular((3.6 * u).max(7.0));
    let mut db = K_LO;
    while db <= 0.0 {
        let grid = Stroke::new(1.0, al(Color32::WHITE, 0.07));
        p.line_segment([pos2(to_x(db), to_y(K_LO)), pos2(to_x(db), to_y(0.0))], grid);
        p.line_segment([pos2(to_x(K_LO), to_y(db)), pos2(to_x(0.0), to_y(db))], grid);
        p.text(pos2(to_x(db), to_y(K_LO) + 1.5 * u), Align2::CENTER_TOP, format!("{}", db as i32), axis.clone(), al(argb(0xFFEBEBE6), 0.4));
        db += 12.0;
    }
    // --- unity diagonal, dashed
    let d = 3.0 * u;
    p.extend(egui::Shape::dashed_line(&[pos2(to_x(K_LO), to_y(K_LO)), pos2(to_x(0.0), to_y(0.0))], Stroke::new(1.0, al(Color32::WHITE, 0.16)), d, d));
    // --- the curve
    let pts: Vec<Pos2> = cache.points(v).iter().map(|(i, o)| pos2(to_x(*i), to_y(*o))).collect();
    if !pts.is_empty() {
        polyline(p, pts, Stroke::new(1.6 * u, accent));
    }
    // --- threshold marker
    let thr = model::threshold(v);
    if thr.is_finite() {
        p.line_segment([pos2(to_x(thr), to_y(K_LO)), pos2(to_x(thr), to_y(0.0))], Stroke::new(1.0, al(Color32::WHITE, 0.22)));
    }
    // --- where the signal is sitting on the curve right now
    if in_shown.is_finite() && gr_shown.is_finite() && in_shown > K_LO + 1.0 {
        p.circle_filled(pos2(to_x(in_shown), to_y(in_shown - gr_shown)), 1.9 * u, argb(0xFFF4F1E8));
    }
    // --- in / out bars down the right-hand edge
    let levels = [in_shown, out_shown];
    let cols = [al(argb(0xFFEBEBE6), 0.55), accent];
    for i in 0..2 {
        let x = b.left() + w - pad - bar_w * (2 - i) as f32 - (1 - i) as f32 * 2.0 * u;
        p.rect_filled(Rect::from_min_max(pos2(x, to_y(0.0)), pos2(x + bar_w, to_y(K_LO))), 0.0, al(Color32::WHITE, 0.06));
        let lv = if levels[i].is_finite() { levels[i] } else { K_LO };
        let top = to_y(lv.clamp(K_LO, 0.0));
        p.rect_filled(Rect::from_min_size(pos2(x, top), vec2(bar_w, to_y(K_LO) - top)), 0.0, cols[i]);
    }
}
