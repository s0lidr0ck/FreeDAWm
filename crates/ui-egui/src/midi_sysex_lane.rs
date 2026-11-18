//! The MIDI Editor's SysEx lane (Lanes › SysEx), after the other lanes: each System Exclusive
//! message of the clip as a marker at its tick, its bytes in hex when hovered. A double-click on
//! empty space (or a click with the Pencil) adds General MIDI System On there, snapped to the
//! grid, to retype in the Event List; a drag moves a marker (snapped; one undo step on release);
//! Alt-click or the marker's right-click Delete removes it. The same commands as the Event List's
//! (`midi.sysex_add`, `midi.sysex_edit`, `midi.sysex_delete`), one undo step each.

use crate::SoundApp;
use crate::midi_editor::RollGeometry;
use crate::theme::{Tokens, regular};
use egui::{Align2, Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_engine::cmd::midi_sysex::hex;
use soundcraft_midi::Sequence;
use soundcraft_model::{ClipId, Tool};

/// What a click adds: General MIDI System On, the Event List's default too.
pub const DEFAULT_SYSEX: &str = "F0 7E 7F 09 01 F7";

/// How near (points) a pointer must be to a marker to take it.
const GRAB: f32 = 5.0;

/// The marker under `x`, nearest first: its index in the clip's SysEx list.
pub fn marker_at(seq: &Sequence, geo: &RollGeometry, x: f32) -> Option<usize> {
    seq.sysex
        .iter()
        .enumerate()
        .map(|(i, m)| (i, (geo.x_of(m.tick) - x).abs()))
        .filter(|(_, d)| *d <= GRAB)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The text a marker's hover shows: where it is, its length and its bytes (the first 48 of a
/// long one).
pub fn hover_text(seq: &Sequence, i: usize) -> String {
    let Some(m) = seq.sysex.get(i) else { return String::new() };
    let shown = m.data.get(..m.data.len().min(48)).unwrap_or(&[]);
    let more = if m.data.len() > 48 { " …" } else { "" };
    format!("SysEx at tick {} ({} bytes)\n{}{more}\nDrag to move, Alt-click to delete; retype it in the Event List", m.tick, m.data.len(), hex(shown))
}

/// Draws and edits the lane in `r`; `label_x` is the left edge of the label column. `grid` and
/// `len` are ticks.
#[allow(clippy::too_many_arguments)]
pub fn lane(app: &mut SoundApp, ui: &mut Ui, cid: ClipId, seq: &Sequence, r: Rect, label_x: f32, geo: &RollGeometry, grid: i64, len: i64) {
    let t = Tokens::current();
    ui.painter().rect_filled(r, 0.0, Color32::from_rgb(28, 28, 30));
    let label = Rect::from_min_max(pos2(label_x, r.min.y), pos2(r.min.x - 2.0, r.max.y));
    ui.painter().rect_filled(label, 0.0, t.panel_bg2);
    let lp = ui.painter().with_clip_rect(label);
    lp.text(pos2(label.min.x + 3.0, label.min.y + 7.0), Align2::LEFT_CENTER, "SysEx", regular(9.5), t.text);
    lp.text(pos2(label.min.x + 3.0, label.min.y + 18.0), Align2::LEFT_CENTER, format!("{}", seq.sysex.len()), regular(8.5), t.text_dim);
    ui.interact(label, ui.id().with("sysex_label"), Sense::hover()).on_hover_text("System Exclusive messages: a marker each");
    let close = Rect::from_min_size(pos2(label.min.x + 2.0, label.max.y - 14.0), vec2(12.0, 12.0));
    let cresp = ui.interact(close, ui.id().with("sysex_close"), Sense::click()).on_hover_text("Hide this lane");
    ui.painter().text(close.center(), Align2::CENTER_CENTER, "×", regular(11.0), if cresp.hovered() { t.text } else { t.text_dim });
    if cresp.clicked() {
        app.midi.sysex_lane = false;
        return;
    }
    let resp = ui.interact(r, ui.id().with(("sysex_lane", cid.0)), Sense::click_and_drag());
    let session = app.engine.session_arc();
    let base = session.find_clip(cid).map_or(0, |(_, c)| session.tempo.samples_to_ticks(c.start, session.sample_rate));
    let snap = |x: f32| {
        crate::midi_edit_tools::snap_abs(&session.tempo, base as f64 + geo.tick_at(x), grid.max(1)).saturating_sub(base).clamp(0, len.max(0))
    };
    let hovered = resp.hover_pos().and_then(|h| marker_at(seq, geo, h.x));
    let dragging = app.midi.sysex_drag.filter(|(c, _)| *c == cid).map(|(_, i)| i);
    // The markers: a flag on a stem, the hovered or dragged one bright.
    let p = ui.painter().with_clip_rect(r);
    let colour = Color32::from_rgb(230, 170, 90);
    for (i, m) in seq.sysex.iter().enumerate() {
        let x = geo.x_of(m.tick);
        let lit = hovered == Some(i) || dragging == Some(i);
        let c = if lit { Color32::WHITE } else { colour };
        p.line_segment([pos2(x, r.min.y + 3.0), pos2(x, r.max.y - 3.0)], Stroke::new(1.5, c));
        p.add(egui::Shape::convex_polygon(vec![pos2(x, r.min.y + 3.0), pos2(x + 8.0, r.min.y + 7.0), pos2(x, r.min.y + 11.0)], c, Stroke::NONE));
    }
    if let Some(i) = hovered.or(dragging) {
        resp.clone().on_hover_text_at_pointer(hover_text(seq, i));
    }
    // A drag on a marker moves it, drawn where it would land until released.
    if resp.drag_started()
        && let Some(at) = ui.input(|i| i.pointer.press_origin())
        && let Some(i) = marker_at(seq, geo, at.x)
    {
        app.midi.sysex_drag = Some((cid, i));
    }
    if let Some(i) = dragging {
        if let Some(at) = resp.interact_pointer_pos() {
            let x = geo.x_of(snap(at.x));
            p.line_segment([pos2(x, r.min.y + 1.0), pos2(x, r.max.y - 1.0)], Stroke::new(1.0, Color32::WHITE));
            if resp.drag_stopped() {
                app.midi.sysex_drag = None;
                if seq.sysex.get(i).is_some_and(|m| m.tick != snap(at.x))
                    && let Err(e) = app.run("midi.sysex_edit", json!({"clip": cid.0, "index": i, "tick": snap(at.x)}))
                {
                    app.ui.status = e;
                }
            }
        }
        if !ui.input(|i| i.pointer.any_down()) {
            app.midi.sysex_drag = None;
        }
        return;
    }
    let alt = ui.input(|i| i.modifiers.alt);
    let pencil = app.engine.session().edit.tool == Tool::Pencil;
    if let Some(at) = resp.interact_pointer_pos() {
        let on = marker_at(seq, geo, at.x);
        let res = match on {
            Some(i) if resp.clicked() && alt => Some(app.run("midi.sysex_delete", json!({"clip": cid.0, "indices": [i]}))),
            None if (resp.double_clicked() || (resp.clicked() && pencil)) && !alt => {
                Some(app.run("midi.sysex_add", json!({"clip": cid.0, "tick": snap(at.x), "bytes": DEFAULT_SYSEX})))
            }
            _ => None,
        };
        if let Some(Err(e)) = res {
            app.ui.status = e;
        }
    }
    resp.context_menu(|ui| {
        let at = ui.input(|i| i.pointer.press_origin()).map_or(r.min.x, |p| p.x);
        match marker_at(seq, geo, at) {
            Some(i) => {
                ui.label(egui::RichText::new(hover_text(seq, i).lines().take(2).collect::<Vec<_>>().join("\n")).small());
                if ui.button("Delete").clicked() {
                    if let Err(e) = app.run("midi.sysex_delete", json!({"clip": cid.0, "indices": [i]})) {
                        app.ui.status = e;
                    }
                    ui.close();
                }
            }
            None => {
                if ui.button("Add General MIDI System On Here").clicked() {
                    if let Err(e) = app.run("midi.sysex_add", json!({"clip": cid.0, "tick": snap(at), "bytes": DEFAULT_SYSEX})) {
                        app.ui.status = e;
                    }
                    ui.close();
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_markers_hover_names_its_tick_length_and_bytes() {
        let mut seq = Sequence::default();
        seq.sysex.push(soundcraft_midi::SysEx { tick: 480, data: vec![0xF0, 0x7E, 0x7F, 0x09, 0x01, 0xF7] });
        let h = hover_text(&seq, 0);
        assert!(h.contains("tick 480") && h.contains("6 bytes") && h.contains("F0 7E 7F 09 01 F7"), "{h}");
        assert_eq!(hover_text(&seq, 1), "");
        seq.sysex.push(soundcraft_midi::SysEx { tick: 0, data: (0..60).map(|b: u8| if b == 0 { 0xF0 } else { b }).collect() });
        assert!(hover_text(&seq, 1).contains(" …"));
    }

    /// A pointer within a few points of a marker takes it (the nearer of two); further away,
    /// none (so a double-click there adds one).
    #[test]
    fn the_pointer_takes_the_nearest_marker() {
        let mut seq = Sequence::default();
        for tick in [480, 500, 960] {
            seq.sysex.push(soundcraft_midi::SysEx { tick, data: vec![0xF0, 0x7E, 0x7F, 0x09, 0x01, 0xF7] });
        }
        let geo = RollGeometry {
            clip: ClipId(1),
            roll: Rect::from_min_size(pos2(100.0, 0.0), vec2(800.0, 200.0)),
            px_per_tick: 0.1,
            fit: 0.1,
            left: 0.0,
            row_h: 10.0,
            scroll: 0.0,
            fold: 0,
        };
        let x = |tick: i64| geo.x_of(tick);
        assert_eq!(marker_at(&seq, &geo, x(960) + 3.0), Some(2));
        assert_eq!(marker_at(&seq, &geo, x(500) + 0.5), Some(1), "the nearer of two close ones");
        assert_eq!(marker_at(&seq, &geo, x(720)), None);
        assert_eq!(marker_at(&Sequence::default(), &geo, x(480)), None);
    }
}
