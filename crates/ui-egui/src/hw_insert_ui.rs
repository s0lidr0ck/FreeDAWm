//! The Hardware Insert's drawer rows: the output and input paths it plays through, the round trip
//! it compensates, and Ping, which measures that round trip on its own paths
//! (`setup.measure_latency` with `insert`).

use crate::SoundApp;
use crate::theme::Tokens;
use egui::{Rect, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{Insert, TrackId};

/// The rows, from `y` in `inner`; returns the y below them.
pub(crate) fn drawer_rows(app: &mut SoundApp, ui: &mut Ui, track: TrackId, ins: &Insert, inner: Rect, y: f32) -> f32 {
    let s = app.engine.session();
    let out = ins.params.get("out").copied().unwrap_or(1.0).max(0.0) as usize;
    let input = ins.params.get("in").copied().unwrap_or(0.0).max(0.0) as usize;
    let latency = ins.params.get("latency").copied().unwrap_or(0.0).max(0.0) as i64;
    let outputs: Vec<(usize, String)> = s.outputs.iter().enumerate().skip(1).map(|(i, o)| (i, o.name.clone())).collect();
    let inputs: Vec<(usize, String)> = s.inputs.iter().enumerate().map(|(i, p)| (i, p.name.clone())).collect();
    let name = |list: &[(usize, String)], i: usize| list.iter().find(|(k, _)| *k == i).map_or_else(|| "(none)".to_string(), |(_, n)| n.clone());
    let (out_name, in_name) = (name(&outputs, out), name(&inputs, input));
    let rate = s.sample_rate.as_f64().max(1.0);
    let rect = Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), 74.0));
    let mut set: Option<(&str, f32)> = None;
    let mut ping = false;
    let mut child = ui.new_child(egui::UiBuilder::new().id_salt(("hw_insert", ins.id.0)).max_rect(rect));
    child.horizontal(|ui| {
        egui::ComboBox::from_label("Send").selected_text(&out_name).show_ui(ui, |ui| {
            for (i, n) in &outputs {
                if ui.selectable_label(*i == out, n).clicked() {
                    set = Some(("out", *i as f32));
                }
            }
        });
        egui::ComboBox::from_label("Return").selected_text(&in_name).show_ui(ui, |ui| {
            for (i, n) in &inputs {
                if ui.selectable_label(*i == input, n).clicked() {
                    set = Some(("in", *i as f32));
                }
            }
        });
    });
    child.horizontal(|ui| {
        let text = if latency > 0 {
            format!("Round trip {latency} samples ({:.1} ms)", latency as f64 * 1000.0 / rate)
        } else {
            "Round trip not measured: no delay compensation until it is pinged".to_string()
        };
        ui.label(egui::RichText::new(text).color(Tokens::current().text_dim));
        ping = ui
            .add_enabled(!outputs.is_empty() && !inputs.is_empty(), egui::Button::new("Ping"))
            .on_hover_text("Plays a click out of the send and listens for it on the return: patch the gear (or a cable) first")
            .clicked();
    });
    if outputs.is_empty() || inputs.is_empty() {
        child.label(egui::RichText::new("Name an output and an input in Setup › I/O first").color(Tokens::current().text_dim));
    }
    if let Some((param, value)) = set
        && let Err(e) = app.run("mix.module_param", json!({"track": track.0, "module": ins.id.0, "param": param, "value": value}))
    {
        app.ui.status = e;
    }
    if ping
        && let Err(e) =
            app.run("setup.measure_latency", json!({"output": out_name, "input": in_name, "insert": {"track": track.0, "module": ins.id.0}}))
    {
        app.ui.status = e;
    }
    rect.max.y + 6.0
}
