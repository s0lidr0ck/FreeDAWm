//! The Subtractive Synth's panel, over its parameter list in the instrument window: the filter's
//! response drawn at its cutoff and resonance (the type's own shape, the 24 dB low-pass as two
//! stages, the range its envelope sweeps shaded), and the amp and filter envelopes drawn to scale.
//! Dragging the filter curve sideways moves the cutoff, up and down the resonance (one undo step
//! per drag, through `mix.instrument_param`); the envelopes show their settings as they are moved
//! in the list below.

use crate::SoundApp;
use crate::theme::Tokens;
use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{Insert, TrackId};

/// The Subtractive Synth's plugin id.
pub const SYNTH: &str = "subtractive_synth";

const LO_HZ: f32 = 20.0;
const HI_HZ: f32 = 20_000.0;
/// The curve's floor, dB.
const FLOOR_DB: f32 = -36.0;
const TOP_DB: f32 = 24.0;

fn param(ins: &Insert, id: &str) -> f32 {
    let default = soundcraft_dsp::plugin_info(SYNTH).and_then(|i| i.param(id)).map_or(0.0, |p| p.default);
    ins.params.get(id).copied().filter(|v| v.is_finite()).unwrap_or(default)
}

/// The filter's gain in dB at `hz` for cutoff `fc`, damping `k` (the synth's: 2 − 1.95 ×
/// resonance) and type `kind` (0 and 1 low-pass 12 and 24 dB, 2 high-pass, 3 band-pass, 4
/// notch): the analogue state-variable filter the synth's TPT filter is drawn from.
pub fn response_db(hz: f32, fc: f32, k: f32, kind: usize) -> f32 {
    let w = f64::from(hz / fc.max(1.0));
    let k = f64::from(k);
    // H(s) = N(s) / (s² + k s + 1), s = jw.
    let (den_re, den_im) = (1.0 - w * w, k * w);
    let den = (den_re * den_re + den_im * den_im).sqrt().max(1e-12);
    let num = match kind {
        2 => w * w,
        3 => w,
        4 => (1.0 - w * w).abs(),
        _ => 1.0,
    };
    let mut g = num / den;
    if kind == 1 {
        g *= g;
    }
    (20.0 * g.max(1e-9).log10()) as f32
}

/// The points of an ADSR drawn over `width` seconds: attack (linear), decay and release
/// (exponential, their times to -60 dB), and the sustain held for a quarter of the drawing.
pub fn envelope_points(a_ms: f32, d_ms: f32, sustain: f32, r_ms: f32) -> Vec<(f32, f32)> {
    let (a, d, r) = (a_ms.max(0.0) / 1000.0, d_ms.max(1.0) / 1000.0, r_ms.max(1.0) / 1000.0);
    let s = sustain.clamp(0.0, 1.0);
    let hold = ((a + d + r) * 0.25).max(0.05);
    let mut pts = vec![(0.0, 0.0), (a, 1.0)];
    let curve = |t0: f32, len: f32, from: f32, to: f32, pts: &mut Vec<(f32, f32)>| {
        for i in 1..=24 {
            let x = i as f32 / 24.0;
            // To -60 dB over `len`: e^(ln(0.001) · x).
            let e = (0.001f32.ln() * x).exp();
            pts.push((t0 + len * x, to + (from - to) * e));
        }
    };
    curve(a, d, 1.0, s, &mut pts);
    let release_from = pts.last().map_or(s, |p| p.1);
    pts.push((a + d + hold, release_from));
    curve(a + d + hold, r, release_from, 0.0, &mut pts);
    pts
}

fn frame(ui: &mut Ui, r: Rect, title: &str) {
    let t = Tokens::current();
    ui.painter().rect_filled(r, 6.0, t.panel_bg2);
    ui.painter().text(r.min + vec2(8.0, 6.0), egui::Align2::LEFT_TOP, title, egui::FontId::proportional(11.0), t.text_dim);
}

fn line(ui: &Ui, pts: &[Pos2], color: Color32) {
    ui.painter().add(egui::Shape::line(pts.to_vec(), Stroke::new(1.8, color)));
}

/// Draws the synth panel for `track`'s instrument `ins`.
pub fn draw(app: &mut SoundApp, ui: &mut Ui, track: TrackId, ins: &Insert) {
    let t = Tokens::current();
    let width = ui.available_width().max(300.0);
    let (row, _) = ui.allocate_exact_size(vec2(width, 128.0), Sense::hover());
    let filter_r = Rect::from_min_size(row.min, vec2(width * 0.5 - 4.0, row.height()));
    let amp_r = Rect::from_min_size(pos2(filter_r.max.x + 8.0, row.min.y), vec2(width * 0.25 - 6.0, row.height()));
    let fenv_r = Rect::from_min_size(pos2(amp_r.max.x + 8.0, row.min.y), vec2(row.max.x - amp_r.max.x - 8.0, row.height()));
    // The filter.
    let kind = param(ins, "filter_type").round().clamp(0.0, 4.0) as usize;
    let names = ["Low-pass 12 dB", "Low-pass 24 dB", "High-pass", "Band-pass", "Notch"];
    frame(ui, filter_r, &format!("Filter · {}", names.get(kind).copied().unwrap_or("")));
    let cutoff = param(ins, "cutoff").clamp(LO_HZ, HI_HZ);
    let res = param(ins, "resonance").clamp(0.0, 100.0);
    let k = 2.0 - 1.95 * (res / 100.0);
    let plot = filter_r.shrink2(vec2(8.0, 10.0)).with_min_y(filter_r.min.y + 22.0);
    let x_of = |hz: f32| plot.min.x + plot.width() * ((hz / LO_HZ).ln() / (HI_HZ / LO_HZ).ln());
    let hz_of = |x: f32| LO_HZ * (HI_HZ / LO_HZ).powf(((x - plot.min.x) / plot.width()).clamp(0.0, 1.0));
    let y_of = |db: f32| plot.max.y - plot.height() * ((db.clamp(FLOOR_DB, TOP_DB) - FLOOR_DB) / (TOP_DB - FLOOR_DB));
    // The sweep of the filter envelope (its amount in octaves) shaded around the cutoff.
    let env_oct = param(ins, "filter_env_amount") / 100.0 * 6.0;
    let (a, b) = (cutoff, (cutoff * 2f32.powf(env_oct)).clamp(LO_HZ, HI_HZ));
    let sweep = Rect::from_x_y_ranges(x_of(a.min(b))..=x_of(a.max(b)), plot.y_range());
    ui.painter().rect_filled(sweep, 0.0, t.accent.gamma_multiply(0.12));
    ui.painter().hline(plot.x_range(), y_of(0.0), Stroke::new(1.0, t.text_dim.gamma_multiply(0.4)));
    let curve: Vec<Pos2> = (0..=160)
        .map(|i| {
            let x = plot.min.x + plot.width() * i as f32 / 160.0;
            pos2(x, y_of(response_db(hz_of(x), cutoff, k, kind)))
        })
        .collect();
    line(ui, &curve, t.accent);
    ui.painter().circle_filled(pos2(x_of(cutoff), y_of(response_db(cutoff, cutoff, k, kind))), 4.0, t.accent);
    let resp = ui.interact(plot, egui::Id::new(("synth-filter", track.0)), Sense::drag());
    let resp = resp.on_hover_text(format!("Cutoff {cutoff:.0} Hz · Resonance {res:.0}% · drag sideways: cutoff, up and down: resonance"));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, "Filter curve"));
    if resp.dragged() {
        let d = resp.drag_delta();
        let new_cut = (cutoff * 2f32.powf(d.x / plot.width() * (HI_HZ / LO_HZ).log2())).clamp(LO_HZ, HI_HZ);
        let new_res = (res - d.y * 0.5).clamp(0.0, 100.0);
        for (id, v, was) in [("cutoff", new_cut, cutoff), ("resonance", new_res, res)] {
            if (v - was).abs() > 1e-3
                && let Err(e) = app.engine.execute_merged(
                    "mix.instrument_param",
                    &json!({"track": track.0, "param": id, "value": v}),
                    &format!("synth_panel:{}:{id}", track.0),
                )
            {
                app.ui.status = e.to_string();
            }
        }
    }
    // The envelopes.
    for (r, title, prefix) in [(amp_r, "Amp envelope", "amp"), (fenv_r, "Filter envelope", "filter")] {
        frame(ui, r, title);
        let pts = envelope_points(
            param(ins, &format!("{prefix}_attack")),
            param(ins, &format!("{prefix}_decay")),
            param(ins, &format!("{prefix}_sustain")) / 100.0,
            param(ins, &format!("{prefix}_release")),
        );
        let total = pts.last().map_or(1.0, |p| p.0).max(1e-3);
        let plot = r.shrink2(vec2(8.0, 10.0)).with_min_y(r.min.y + 22.0);
        let drawn: Vec<Pos2> = pts.iter().map(|(x, y)| pos2(plot.min.x + plot.width() * x / total, plot.max.y - plot.height() * y)).collect();
        line(ui, &drawn, t.accent);
        let label = format!("{total:.2} s");
        ui.painter().text(r.max - vec2(6.0, 4.0), egui::Align2::RIGHT_BOTTOM, label, egui::FontId::proportional(10.0), t.text_dim);
        ui.interact(r, egui::Id::new(("synth-env", track.0, prefix)), Sense::hover())
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, title));
    }
    ui.add_space(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_filter_type_has_its_shape() {
        let k = 2.0;
        // Low-pass: flat below, -12 dB an octave above; 24 dB twice that.
        assert!(response_db(100.0, 1000.0, k, 0).abs() < 0.5);
        assert!((response_db(8000.0, 1000.0, k, 0) + 36.0).abs() < 2.0);
        assert!((response_db(8000.0, 1000.0, k, 1) - 2.0 * response_db(8000.0, 1000.0, k, 0)).abs() < 0.01);
        assert!(response_db(100.0, 1000.0, k, 2) < -30.0 && response_db(10_000.0, 1000.0, k, 2).abs() < 0.5);
        assert!(response_db(1000.0, 1000.0, k, 3) > response_db(50.0, 1000.0, k, 3) + 15.0);
        assert!(response_db(1000.0, 1000.0, k, 4) < -60.0);
        // Resonance raises the peak at the cutoff.
        assert!(response_db(1000.0, 1000.0, 0.05, 0) > 20.0);
    }

    #[test]
    fn an_envelope_is_drawn_to_its_times() {
        let p = envelope_points(100.0, 200.0, 0.5, 400.0);
        assert_eq!(p[1], (0.1, 1.0));
        let at_decay_end = p.iter().find(|q| (q.0 - 0.3).abs() < 1e-4).map(|q| q.1).unwrap_or(-1.0);
        assert!((at_decay_end - 0.5).abs() < 0.01, "{at_decay_end}");
        let last = p.last().copied().unwrap_or_default();
        assert!(last.1 < 0.01 && last.0 > 0.7);
        assert!(p.windows(2).all(|w| w[1].0 >= w[0].0));
    }
}
