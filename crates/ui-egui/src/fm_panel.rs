//! The FM Synth's panel, over its parameter list in the instrument window: the algorithm drawn
//! as boxes (the operators heard on the bottom row over the output line, each modulator above
//! what it modulates, an arrow for each modulation, a loop on an operator with feedback, each box
//! showing its ratio or fixed frequency and filled by its level), and the four operators'
//! envelopes drawn to scale. Clicking the diagram steps to the next algorithm (one undo step).

use crate::SoundApp;
use crate::synth_panel::{envelope_points, frame, line};
use crate::theme::Tokens;
use egui::{Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::plugins::fm::{ALGORITHMS, CARRIERS, MODULATORS};
use soundcraft_model::{Insert, TrackId};

/// The FM Synth's plugin id.
pub const FM: &str = "fm_synth";

fn param(ins: &Insert, id: &str) -> f32 {
    let default = soundcraft_dsp::plugin_info(FM).and_then(|i| i.param(id)).map_or(0.0, |p| p.default);
    ins.params.get(id).copied().filter(|v| v.is_finite()).unwrap_or(default)
}

/// Each operator's row in the diagram (0 the bottom: the operators heard) for algorithm `alg`:
/// a modulator sits one row above the highest operator it modulates.
pub fn rows(alg: usize) -> [usize; 4] {
    let mods = MODULATORS.get(alg).copied().unwrap_or([0; 4]);
    let mut row = [0usize; 4];
    // Modulators always sit above (higher numbered than) what they modulate: fill from 1 up.
    for n in 0..4 {
        for (target, mask) in mods.iter().enumerate() {
            if mask & (1 << n) != 0 {
                row[n] = row[n].max(row[target] + 1);
            }
        }
    }
    row
}

/// The centre of each operator's box in `r` for algorithm `alg`.
pub fn layout(alg: usize, r: Rect) -> [Pos2; 4] {
    let row = rows(alg);
    let top = row.iter().copied().max().unwrap_or(0);
    let mut out = [r.center(); 4];
    for (n, p) in out.iter_mut().enumerate() {
        let same: Vec<usize> = (0..4).filter(|m| row[*m] == row[n]).collect();
        let i = same.iter().position(|m| *m == n).unwrap_or(0);
        let x = r.min.x + r.width() * (i as f32 + 1.0) / (same.len() as f32 + 1.0);
        let y = r.max.y - 14.0 - (r.height() - 28.0) * row[n] as f32 / (top.max(1) as f32);
        *p = pos2(x, if top == 0 { r.max.y - 14.0 } else { y });
    }
    out
}

fn ratio_text(ins: &Insert, n: usize) -> String {
    if param(ins, &format!("op{n}_fixed")) >= 0.5 {
        return format!("{:.0} Hz", param(ins, &format!("op{n}_freq")));
    }
    let coarse = param(ins, &format!("op{n}_coarse")).round().max(0.0) as usize;
    let r = soundcraft_dsp::plugins::fm::ratio_of(coarse, param(ins, &format!("op{n}_fine")), 0.0);
    format!("×{}", (r * 100.0).round() / 100.0)
}

/// Draws the panel for `track`'s instrument `ins`.
pub fn draw(app: &mut SoundApp, ui: &mut Ui, track: TrackId, ins: &Insert) {
    let t = Tokens::current();
    let width = ui.available_width().max(300.0);
    let (row, _) = ui.allocate_exact_size(vec2(width, 140.0), Sense::hover());
    let alg_r = Rect::from_min_size(row.min, vec2(width * 0.5 - 4.0, row.height()));
    let alg = (param(ins, "algorithm").round().max(0.0) as usize).min(ALGORITHMS.len() - 1);
    frame(ui, alg_r, &format!("Algorithm {}", ALGORITHMS.get(alg).copied().unwrap_or("")));
    let plot = alg_r.shrink2(vec2(10.0, 8.0)).with_min_y(alg_r.min.y + 22.0).with_max_y(alg_r.max.y - 14.0);
    let at = layout(alg, plot);
    let size = vec2(42.0, 22.0);
    let mods = MODULATORS.get(alg).copied().unwrap_or([0; 4]);
    let carriers = CARRIERS.get(alg).copied().unwrap_or(1);
    let dim = Stroke::new(1.4, t.text_dim);
    // The output line under the carriers.
    let out_y = alg_r.max.y - 8.0;
    let xs: Vec<f32> = (0..4).filter(|n| carriers & (1 << n) != 0).map(|n| at[n].x).collect();
    let (lo, hi) = (xs.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(f32::MIN, f32::max));
    ui.painter().hline(lo.min(hi)..=hi.max(lo), out_y, Stroke::new(2.0, t.accent));
    for n in (0..4).filter(|n| carriers & (1 << n) != 0) {
        ui.painter().line_segment([at[n] + vec2(0.0, size.y / 2.0), pos2(at[n].x, out_y)], Stroke::new(2.0, t.accent));
    }
    for (target, mask) in mods.iter().enumerate() {
        for m in (0..4).filter(|m| mask & (1 << m) != 0) {
            let (a, b) = (at[m] + vec2(0.0, size.y / 2.0), at[target] - vec2(0.0, size.y / 2.0));
            ui.painter().line_segment([a, b], dim);
            let d = (b - a).normalized();
            let side = vec2(-d.y, d.x) * 3.5;
            ui.painter().add(egui::Shape::convex_polygon(vec![b, b - d * 7.0 + side, b - d * 7.0 - side], t.text_dim, Stroke::NONE));
        }
    }
    for (n, centre) in at.iter().enumerate() {
        let r = Rect::from_center_size(*centre, size);
        let level = (param(ins, &format!("op{}_level", n + 1)) / 100.0).clamp(0.0, 1.0);
        ui.painter().rect_filled(r, 4.0, t.panel_bg);
        ui.painter().rect_filled(Rect::from_min_max(pos2(r.min.x, r.max.y - r.height() * level), r.max), 4.0, t.accent.gamma_multiply(0.35));
        ui.painter().rect_stroke(r, 4.0, Stroke::new(1.2, if carriers & (1 << n) != 0 { t.accent } else { t.text_dim }), egui::StrokeKind::Inside);
        ui.painter().text(r.center() - vec2(0.0, 4.0), egui::Align2::CENTER_CENTER, format!("{}", n + 1), egui::FontId::proportional(11.0), t.text);
        ui.painter().text(
            r.center() + vec2(0.0, 6.0),
            egui::Align2::CENTER_CENTER,
            ratio_text(ins, n + 1),
            egui::FontId::proportional(8.5),
            t.text_dim,
        );
        if param(ins, &format!("op{}_feedback", n + 1)) > 0.0 {
            let c = pos2(r.max.x + 5.0, r.min.y + 2.0);
            ui.painter().circle_stroke(c, 5.0, dim);
        }
    }
    let resp = ui.interact(plot, egui::Id::new(("fm-algorithm", track.0)), Sense::click());
    let resp = resp.on_hover_text("Click: the next algorithm");
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "FM algorithm"));
    if resp.clicked() {
        let next = (alg + 1) % ALGORITHMS.len();
        if let Err(e) = app.engine.execute("mix.instrument_param", &json!({"track": track.0, "param": "algorithm", "value": next})) {
            app.ui.status = e.to_string();
        }
    }
    // The four envelopes, one quarter of the right half each.
    let env_w = (row.max.x - alg_r.max.x - 8.0) / 2.0 - 4.0;
    for n in 0..4 {
        let (cx, cy) = (n % 2, n / 2);
        let r = Rect::from_min_size(pos2(alg_r.max.x + 8.0 + cx as f32 * (env_w + 8.0), row.min.y + cy as f32 * 72.0), vec2(env_w, 68.0));
        let title = format!("Op {} envelope", n + 1);
        frame(ui, r, &title);
        let p = |s: &str| param(ins, &format!("op{}_{s}", n + 1));
        let pts = envelope_points(p("attack"), p("decay"), p("sustain") / 100.0, p("release"));
        let total = pts.last().map_or(1.0, |q| q.0).max(1e-3);
        let plot = r.shrink2(vec2(6.0, 6.0)).with_min_y(r.min.y + 20.0);
        let drawn: Vec<Pos2> = pts.iter().map(|(x, y)| pos2(plot.min.x + plot.width() * x / total, plot.max.y - plot.height() * y)).collect();
        line(ui, &drawn, if carriers & (1 << n) != 0 { t.accent } else { t.text_dim });
        ui.interact(r, egui::Id::new(("fm-env", track.0, n)), Sense::hover())
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, &title));
    }
    ui.add_space(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modulators_are_drawn_above_what_they_modulate() {
        assert_eq!(rows(0), [0, 1, 2, 3], "a stack");
        assert_eq!(rows(1), [0, 1, 2, 2]);
        assert_eq!(rows(5), [0, 0, 0, 1]);
        assert_eq!(rows(7), [0, 0, 0, 0]);
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 100.0));
        for (alg, mods) in MODULATORS.iter().enumerate() {
            let at = layout(alg, r);
            for (target, mask) in mods.iter().enumerate() {
                for m in (0..4).filter(|m| mask & (1 << m) != 0) {
                    assert!(at[m].y < at[target].y, "algorithm {alg}: {m} above {target}");
                }
            }
            // No two boxes in the same place.
            for a in 0..4 {
                for b in a + 1..4 {
                    assert!(at[a].distance(at[b]) > 20.0, "algorithm {alg}");
                }
            }
        }
    }
}
