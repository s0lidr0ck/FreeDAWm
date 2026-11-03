//! The Tempo ruler: each tempo change is a marker showing its BPM. Drag it left/right to move it
//! (the grid snaps), up/down to change its BPM (Shift: fine); double-click it to type the BPM;
//! right-click › Delete / Ramp to Next / Edit Tempo…; double-click empty ruler space to add a
//! change there at the tempo in effect. Each gesture is one command, so one undo step.

use crate::SoundApp;
use crate::edit_window::{sample_at, snap, x_of};
use crate::theme::{Tokens, bold};
use egui::{Align2, Color32, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::Session;
use soundcraft_time::TempoEvent;

/// BPM per point of vertical drag (Shift: a tenth of it).
pub const BPM_PER_POINT: f64 = 0.25;
/// Points the pointer moves before a drag picks its direction.
const AXIS_SLOP: f32 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Axis {
    Time,
    Tempo,
}

/// A marker being dragged (kept in egui's memory between frames).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    tick: i64,
    bpm: f64,
    from: Pos2,
    axis: Option<Axis>,
}

/// The BPM after dragging `dy` points (down is negative), rounded to 0.1 (fine: 0.01).
pub fn dragged_bpm(bpm: f64, dy: f32, fine: bool) -> f64 {
    let per = if fine { BPM_PER_POINT / 10.0 } else { BPM_PER_POINT };
    let step = if fine { 100.0 } else { 10.0 };
    ((bpm - f64::from(dy) * per) * step).round().clamp(5.0 * step, 1000.0 * step) / step
}

/// The label a marker shows (a ramp also draws a line to the next marker).
pub fn label(ev: &TempoEvent) -> String {
    let bpm = (ev.bpm * 100.0).round() / 100.0;
    if ev.ramp { format!("{bpm} ramp") } else { format!("{bpm}") }
}

/// The marker under `x`: its flag or its label.
fn hit(spots: &[(TempoEvent, Rect)], x: f32) -> Option<TempoEvent> {
    spots.iter().rev().find(|(_, r)| x >= r.min.x && x <= r.max.x).map(|(e, _)| *e)
}

pub fn tempo_ruler(app: &mut SoundApp, ui: &mut Ui, painter: &egui::Painter, s: &Session, tl: Rect, row: Rect) {
    let t = Tokens::current();
    let sr = s.sample_rate;
    painter.rect_filled(row.shrink2(vec2(0.0, 1.0)), 0.0, t.tempo_ruler);
    let drag_id = ui.id().with("tempo_marker_drag");
    let menu_id = ui.id().with("tempo_marker_menu");
    let drag: Option<Drag> = ui.data(|d| d.get_temp(drag_id));
    let evs = s.tempo.tempos();
    let mut spots: Vec<(TempoEvent, Rect)> = Vec::with_capacity(evs.len());
    let mut last_label = f32::MIN;
    let text_col = Color32::from_rgb(230, 240, 230);
    for (k, ev) in evs.iter().enumerate() {
        let x = x_of(s, tl, s.tempo.tick_to_samples(ev.tick, sr));
        let dragged = drag.is_some_and(|d| d.tick == ev.tick && d.axis.is_some());
        let col = if dragged { Color32::from_rgb(220, 60, 50).gamma_multiply(0.4) } else { Color32::from_rgb(220, 60, 50) };
        painter.add(Shape::convex_polygon(
            vec![pos2(x, row.min.y + 3.0), pos2(x + 7.0, row.center().y), pos2(x, row.max.y - 3.0)],
            col,
            Stroke::NONE,
        ));
        if ev.ramp
            && let Some(n) = evs.get(k + 1)
        {
            // A ramp: a rising (or falling) line to the next change.
            let xn = x_of(s, tl, s.tempo.tick_to_samples(n.tick, sr));
            let (y0, y1) = if n.bpm >= ev.bpm { (row.max.y - 3.0, row.min.y + 3.0) } else { (row.min.y + 3.0, row.max.y - 3.0) };
            painter.line_segment([pos2(x + 7.0, y0), pos2(xn, y1)], Stroke::new(1.0, text_col.gamma_multiply(0.5)));
        }
        let text = label(ev);
        let w = painter.layout_no_wrap(text.clone(), bold(11.0), text_col).size().x;
        if x > last_label + 4.0 {
            last_label = x + 10.0 + w;
            painter.text(
                pos2(x + 10.0, row.center().y),
                Align2::LEFT_CENTER,
                text,
                bold(11.0),
                if dragged { text_col.gamma_multiply(0.4) } else { text_col },
            );
            spots.push((*ev, Rect::from_min_max(pos2(x - 3.0, row.min.y), pos2(x + 12.0 + w, row.max.y))));
        } else {
            spots.push((*ev, Rect::from_min_max(pos2(x - 3.0, row.min.y), pos2(x + 8.0, row.max.y))));
        }
    }

    let resp = ui.interact(row, ui.id().with("tempo_ruler"), Sense::click_and_drag());
    let fine = ui.input(|i| i.modifiers.shift);
    // Drag: pick the marker on press, its direction after a few points, run once on release.
    if resp.drag_started()
        && let Some(p) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
        && let Some(ev) = hit(&spots, p.x)
    {
        ui.data_mut(|d| d.insert_temp(drag_id, Drag { tick: ev.tick, bpm: ev.bpm, from: p, axis: None }));
    }
    if let Some(mut d) = drag {
        let p = ui.input(|i| i.pointer.interact_pos()).unwrap_or(d.from);
        let delta = p - d.from;
        if d.axis.is_none() && delta.length() >= AXIS_SLOP {
            d.axis = Some(if delta.x.abs() >= delta.y.abs() { Axis::Time } else { Axis::Tempo });
            ui.data_mut(|m| m.insert_temp(drag_id, d));
        }
        // The preview: the marker where it would land, or its new tempo.
        let (to, bpm) = match d.axis {
            Some(Axis::Time) if d.tick > 0 => (Some(snap(s, sample_at(s, tl, p.x).max(0))), d.bpm),
            Some(Axis::Tempo) => (None, dragged_bpm(d.bpm, delta.y, fine)),
            _ => (None, d.bpm),
        };
        if d.axis.is_some() {
            let at = to.unwrap_or_else(|| s.tempo.tick_to_samples(d.tick, sr));
            let x = x_of(s, tl, at);
            painter.add(Shape::convex_polygon(
                vec![pos2(x, row.min.y + 3.0), pos2(x + 7.0, row.center().y), pos2(x, row.max.y - 3.0)],
                Color32::from_rgb(255, 120, 90),
                Stroke::NONE,
            ));
            painter.text(
                pos2(x + 10.0, row.center().y),
                Align2::LEFT_CENTER,
                format!("{}", (bpm * 100.0).round() / 100.0),
                bold(11.0),
                Color32::WHITE,
            );
        }
        if resp.drag_stopped() || !ui.input(|i| i.pointer.any_down()) {
            ui.data_mut(|m| m.remove::<Drag>(drag_id));
            match (d.axis, to) {
                (Some(Axis::Time), Some(at)) => {
                    let to_tick = s.tempo.samples_to_ticks(at, sr);
                    if to_tick != d.tick {
                        let _ = app.run("event.tempo_move", json!({"tick": d.tick, "to_tick": to_tick}));
                    }
                }
                (Some(Axis::Tempo), _) if (bpm - d.bpm).abs() > 1e-9 => {
                    let _ = app.run("event.tempo_move", json!({"tick": d.tick, "bpm": bpm}));
                }
                _ => {}
            }
        }
    }
    if resp.double_clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        match hit(&spots, p.x) {
            // On a marker: type its BPM.
            Some(ev) => app.dialogs.open = Some(crate::dialogs::Dialog::TempoChange { at: s.tempo.tick_to_samples(ev.tick, sr), bpm: ev.bpm }),
            // Empty space: a change there at the tempo in effect.
            None => {
                let at = snap(s, sample_at(s, tl, p.x).max(0));
                let bpm = s.tempo.tempo_at_tick(s.tempo.samples_to_ticks(at, sr));
                let _ = app.run("event.tempo", json!({"bpm": (bpm * 100.0).round() / 100.0, "at": at}));
            }
        }
    }
    if resp.secondary_clicked() {
        let target = resp.interact_pointer_pos().and_then(|p| hit(&spots, p.x));
        ui.data_mut(|d| d.insert_temp(menu_id, target));
    }
    resp.context_menu(|ui| {
        let target: Option<TempoEvent> = ui.data(|d| d.get_temp(menu_id)).flatten();
        if let Some(ev) = target {
            marker_menu(app, ui, s, ev);
            ui.separator();
        }
        crate::menus::ruler_context_menu(app, ui, "tempo");
    });
}

/// A tempo marker's own menu.
fn marker_menu(app: &mut SoundApp, ui: &mut Ui, s: &Session, ev: TempoEvent) {
    let has_next = s.tempo.tempos().iter().any(|t| t.tick > ev.tick);
    if ui.button("Edit Tempo…").clicked() {
        app.dialogs.open = Some(crate::dialogs::Dialog::TempoChange { at: s.tempo.tick_to_samples(ev.tick, s.sample_rate), bpm: ev.bpm });
        ui.close();
    }
    let mut ramp = ev.ramp;
    if ui
        .add_enabled(has_next || ev.ramp, egui::Checkbox::new(&mut ramp, "Ramp to Next"))
        .on_hover_text("Change the tempo smoothly up to the next tempo change")
        .clicked()
    {
        let _ = app.run("event.tempo_ramp", json!({"tick": ev.tick, "ramp": ramp}));
        ui.close();
    }
    if ui.add_enabled(ev.tick > 0, egui::Button::new("Delete")).clicked() {
        let _ = app.run("event.tempo_remove", json!({"tick": ev.tick}));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_drag_changes_bpm_and_shift_is_fine() {
        assert_eq!(dragged_bpm(120.0, -40.0, false), 130.0);
        assert_eq!(dragged_bpm(120.0, 40.0, false), 110.0);
        assert_eq!(dragged_bpm(120.0, -4.0, true), 120.1);
        assert_eq!(dragged_bpm(6.0, 1e6, false), 5.0);
        assert_eq!(dragged_bpm(990.0, -1e6, false), 1000.0);
    }

    #[test]
    fn labels_show_bpm_and_ramp() {
        assert_eq!(label(&TempoEvent { tick: 0, bpm: 104.256, ramp: false }), "104.26");
        assert_eq!(label(&TempoEvent { tick: 0, bpm: 90.0, ramp: true }), "90 ramp");
    }
}
