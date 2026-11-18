//! The Wavetable Synth's panel, over its parameter list in the instrument window: the current
//! wave (the chosen table at the Position control) drawn bold, with frames further along the
//! table drawn faintly behind it, the range the second envelope moves the position across marked
//! under it, and the amp envelope and envelope 2 drawn to scale. Dragging the wave sideways moves
//! the Position (one undo step per drag, through `mix.instrument_param`).

use crate::SoundApp;
use crate::synth_panel::{envelope_points, frame, line};
use crate::theme::Tokens;
use egui::{Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::plugins::wavetable::tables;
use soundcraft_model::{Insert, TrackId};

/// The Wavetable Synth's plugin id.
pub const WAVETABLE: &str = "wavetable_synth";

fn param(ins: &Insert, id: &str) -> f32 {
    let default = soundcraft_dsp::plugin_info(WAVETABLE).and_then(|i| i.param(id)).map_or(0.0, |p| p.default);
    ins.params.get(id).copied().filter(|v| v.is_finite()).unwrap_or(default)
}

/// One cycle of `bank` at `pos` (0..1) as `n` points across `r`, peak ±1 at the top and bottom.
pub fn wave_points(bank: usize, pos: f32, n: usize, r: Rect) -> Vec<Pos2> {
    let mut y = vec![0.0f32; n.max(2)];
    tables::draw(bank, pos, &mut y);
    let last = (y.len() - 1) as f32;
    y.iter()
        .enumerate()
        .map(|(i, v)| pos2(r.min.x + r.width() * i as f32 / last, r.center().y - r.height() * 0.5 * v.clamp(-1.2, 1.2) / 1.2))
        .collect()
}

/// Draws the panel for `track`'s instrument `ins`.
pub fn draw(app: &mut SoundApp, ui: &mut Ui, track: TrackId, ins: &Insert) {
    let t = Tokens::current();
    let width = ui.available_width().max(300.0);
    let (row, _) = ui.allocate_exact_size(vec2(width, 128.0), Sense::hover());
    let wave_r = Rect::from_min_size(row.min, vec2(width * 0.5 - 4.0, row.height()));
    let amp_r = Rect::from_min_size(pos2(wave_r.max.x + 8.0, row.min.y), vec2(width * 0.25 - 6.0, row.height()));
    let env_r = Rect::from_min_size(pos2(amp_r.max.x + 8.0, row.min.y), vec2(row.max.x - amp_r.max.x - 8.0, row.height()));
    let bank = param(ins, "table").round().clamp(0.0, (tables::BANKS.len() - 1) as f32) as usize;
    let pos = (param(ins, "position") / 100.0).clamp(0.0, 1.0);
    let name = tables::BANKS.get(bank).copied().unwrap_or("");
    frame(ui, wave_r, &format!("Table · {name} · {:.0}%", pos * 100.0));
    let plot = wave_r.shrink2(vec2(8.0, 10.0)).with_min_y(wave_r.min.y + 22.0);
    // Frames further along, faint and stepped up and to the right behind the current one.
    for k in (1..=4).rev() {
        let p = (pos + k as f32 * 0.12).min(1.0);
        if p <= pos {
            continue;
        }
        let shift = vec2(k as f32 * 6.0, -(k as f32) * 4.0);
        let pts: Vec<Pos2> = wave_points(bank, p, 96, plot.shrink2(vec2(12.0, 8.0))).iter().map(|q| *q + shift).collect();
        ui.painter().add(egui::Shape::line(pts, Stroke::new(1.0, t.text_dim.gamma_multiply(0.35))));
    }
    ui.painter().hline(plot.x_range(), plot.center().y, Stroke::new(1.0, t.text_dim.gamma_multiply(0.3)));
    line(ui, &wave_points(bank, pos, 160, plot.shrink2(vec2(12.0, 8.0))), t.accent);
    // The table as a bar along the bottom: the position, and where envelope 2 takes it.
    let bar = Rect::from_min_max(pos2(plot.min.x, plot.max.y - 3.0), pos2(plot.max.x, plot.max.y));
    ui.painter().rect_filled(bar, 1.0, t.text_dim.gamma_multiply(0.2));
    let to = (pos + param(ins, "env2_position") / 100.0).clamp(0.0, 1.0);
    let x_of = |p: f32| bar.min.x + bar.width() * p;
    ui.painter().rect_filled(
        Rect::from_x_y_ranges(x_of(pos.min(to))..=x_of(pos.max(to)).max(x_of(pos.min(to)) + 1.0), bar.y_range()),
        1.0,
        t.accent.gamma_multiply(0.5),
    );
    ui.painter().circle_filled(pos2(x_of(pos), bar.center().y), 3.5, t.accent);
    let resp = ui.interact(plot, egui::Id::new(("wavetable-wave", track.0)), Sense::drag());
    let resp = resp.on_hover_text(format!("{name} at {:.0}% · drag sideways to move the position", pos * 100.0));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, "Wavetable position"));
    if resp.dragged() {
        let new = ((pos + resp.drag_delta().x / plot.width()) * 100.0).clamp(0.0, 100.0);
        if (new - pos * 100.0).abs() > 1e-3
            && let Err(e) = app.engine.execute_merged(
                "mix.instrument_param",
                &json!({"track": track.0, "param": "position", "value": new}),
                &format!("wavetable_panel:{}:position", track.0),
            )
        {
            app.ui.status = e.to_string();
        }
    }
    for (r, title, prefix) in [(amp_r, "Amp envelope", "amp"), (env_r, "Envelope 2", "env2")] {
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
        ui.painter().text(r.max - vec2(6.0, 4.0), egui::Align2::RIGHT_BOTTOM, format!("{total:.2} s"), egui::FontId::proportional(10.0), t.text_dim);
        ui.interact(r, egui::Id::new(("wavetable-env", track.0, prefix)), Sense::hover())
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, title));
    }
    ui.add_space(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_drawn_wave_is_the_table_at_the_position() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 60.0));
        // Classic at 0 is a sine: a quarter of the way along it is at its top.
        let p = wave_points(0, 0.0, 101, r);
        assert_eq!(p.len(), 101);
        let quarter = p[25];
        assert!(quarter.y < 6.0, "{quarter:?}");
        assert!((p[0].y - 30.0).abs() < 1.0 && (p[50].y - 30.0).abs() < 1.0);
        // Another position draws another wave.
        let q = wave_points(0, 1.0, 101, r);
        assert!(p.iter().zip(&q).any(|(a, b)| (a.y - b.y).abs() > 3.0));
    }
}
