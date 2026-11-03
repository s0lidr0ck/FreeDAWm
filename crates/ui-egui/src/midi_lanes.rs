//! The MIDI Editor's controller lanes, under the velocity lane: CC (mod wheel, volume,
//! expression, sustain, any other number), pitch bend and aftertouch, picked from the header's
//! **Lanes** menu. Each lane draws its events as a stepped line with a point per event and edits
//! them as the automation lanes do: a click adds a point (the Pencil too), a drag on a point moves
//! the selected points, Alt-click deletes one, Shift-click adds to the selection, a drag on empty
//! space selects the points inside, the Pencil drags a freehand line that replaces what it crosses,
//! Delete removes the selected points. The sustain lane is on/off.

use crate::SoundApp;
use crate::midi_editor::RollGeometry;
use crate::theme::{Tokens, regular};
use egui::{Align2, Color32, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2};
use serde_json::json;
use soundcraft_engine::cmd::midi_ctrl::{lane_id, lane_range};
use soundcraft_midi::{CtrlEvent, CtrlKind, Sequence};
use soundcraft_model::{ClipId, Tool};

/// A lane's height, when there is room.
pub const LANE_H: f32 = 52.0;
/// The gap above each lane.
pub const LANE_GAP: f32 = 3.0;

/// The lanes the Lanes menu offers first (number, name).
pub const COMMON: [(CtrlKind, &str); 7] = [
    (CtrlKind::Cc(1), "Mod Wheel (CC1)"),
    (CtrlKind::Cc(7), "Volume (CC7)"),
    (CtrlKind::Cc(10), "Pan (CC10)"),
    (CtrlKind::Cc(11), "Expression (CC11)"),
    (CtrlKind::Cc(64), "Sustain (CC64)"),
    (CtrlKind::PitchBend, "Pitch Bend"),
    (CtrlKind::ChannelPressure, "Aftertouch"),
];

/// A lane's name.
pub fn lane_label(kind: CtrlKind) -> String {
    match COMMON.iter().find(|(k, _)| *k == kind) {
        Some((_, name)) => (*name).to_string(),
        None => match kind {
            CtrlKind::Cc(n) => format!("CC{n}"),
            CtrlKind::PolyPressure(p) => format!("Poly Pressure {p}"),
            CtrlKind::Program => "Program".into(),
            _ => String::new(),
        },
    }
}

/// A lane's name for its narrow label column: a short name over its number ("Mod" / "CC1").
pub fn lane_short(kind: CtrlKind) -> (String, String) {
    let name = match kind {
        CtrlKind::Cc(1) => "Mod",
        CtrlKind::Cc(7) => "Volume",
        CtrlKind::Cc(10) => "Pan",
        CtrlKind::Cc(11) => "Expr",
        CtrlKind::Cc(64) => "Sustain",
        CtrlKind::PitchBend => return ("Bend".into(), "pitch".into()),
        CtrlKind::ChannelPressure => return ("Touch".into(), "after".into()),
        _ => "",
    };
    match kind {
        CtrlKind::Cc(n) if name.is_empty() => (format!("CC{n}"), String::new()),
        CtrlKind::Cc(n) => (name.to_string(), format!("CC{n}")),
        _ => (lane_label(kind), String::new()),
    }
}

/// Whether a lane takes only off (0) and on (127).
pub fn is_switch(kind: CtrlKind) -> bool {
    matches!(kind, CtrlKind::Cc(64..=69))
}

/// The lanes `seq` has events on (those a lane can show), in the Lanes menu's order.
pub fn lanes_with_data(seq: &Sequence) -> Vec<CtrlKind> {
    let mut v: Vec<CtrlKind> = Vec::new();
    for c in &seq.ctrls {
        if lane_id(c.kind).is_some() && !v.contains(&c.kind) {
            v.push(c.kind);
        }
    }
    let order = |k: &CtrlKind| match k {
        CtrlKind::Cc(n) => u32::from(*n),
        CtrlKind::PitchBend => 200,
        _ => 300,
    };
    v.sort_by_key(order);
    v
}

/// The height each of `n` lanes gets when `room` points are left under the roll's minimum.
pub fn lane_height(n: usize, room: f32) -> f32 {
    if n == 0 {
        return 0.0;
    }
    (room / n as f32 - LANE_GAP).clamp(18.0, LANE_H)
}

/// A drag in a controller lane.
#[derive(Debug, Clone, PartialEq)]
pub enum CtrlDrag {
    /// The selected points, grabbed at `from`.
    Points { lane: CtrlKind, from: Pos2 },
    /// The Pencil: the (tick, value) pairs drawn so far.
    Pencil { lane: CtrlKind, points: Vec<(i64, i32)> },
    /// A rubber band from `from`.
    Band { lane: CtrlKind, from: Pos2 },
}

/// One lane on screen: where values sit.
#[derive(Debug, Clone, Copy)]
pub struct LaneGeometry {
    pub kind: CtrlKind,
    pub rect: Rect,
}

impl LaneGeometry {
    pub fn y_of(&self, v: i32) -> f32 {
        let (lo, hi) = lane_range(self.kind);
        let span = (self.rect.height() - 6.0).max(1.0);
        self.rect.max.y - 3.0 - span * (v - lo) as f32 / (hi - lo).max(1) as f32
    }

    /// The value at `y`; a switch lane reads off below the middle, on above.
    pub fn value_at(&self, y: f32) -> i32 {
        let (lo, hi) = lane_range(self.kind);
        let span = (self.rect.height() - 6.0).max(1.0);
        let v = (lo as f32 + ((self.rect.max.y - 3.0 - y) / span) * (hi - lo) as f32).round() as i32;
        let v = v.clamp(lo, hi);
        if is_switch(self.kind) { if v >= 64 { 127 } else { 0 } } else { v }
    }

    /// `dy` points as a change of value (up is more).
    pub fn value_delta(&self, dy: f32) -> i64 {
        let (lo, hi) = lane_range(self.kind);
        let span = (self.rect.height() - 6.0).max(1.0);
        (-dy / span * (hi - lo) as f32).round() as i64
    }
}

/// The lane's events (index in the clip's controller list, event).
fn events_of(seq: &Sequence, kind: CtrlKind) -> Vec<(usize, CtrlEvent)> {
    seq.ctrls.iter().copied().enumerate().filter(|(_, c)| c.kind == kind).collect()
}

/// The selected points of `kind` (stale ones dropped).
fn selected(app: &SoundApp, seq: &Sequence, kind: CtrlKind) -> Vec<usize> {
    match &app.midi.ctrl_sel {
        Some((k, v)) if *k == kind => v.iter().copied().filter(|i| seq.ctrls.get(*i).is_some_and(|c| c.kind == kind)).collect(),
        _ => Vec::new(),
    }
}

/// The point under `p`: the nearest within 6 points.
fn point_at(lg: &LaneGeometry, geo: &RollGeometry, events: &[(usize, CtrlEvent)], p: Pos2) -> Option<usize> {
    events
        .iter()
        .map(|(i, c)| (*i, pos2(geo.x_of(c.tick), lg.y_of(c.value)).distance(p)))
        .filter(|(_, d)| *d <= 6.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The Lanes menu: a check item per common lane, every other CC number, show the lanes with data.
pub fn menu(app: &mut SoundApp, ui: &mut Ui, seq: &Sequence) {
    for (kind, name) in COMMON {
        let mut on = app.midi.lanes.contains(&kind);
        if ui.checkbox(&mut on, name).changed() {
            toggle(app, kind, on);
        }
    }
    ui.menu_button("Other CC", |ui| {
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for n in 0u8..=127 {
                let kind = CtrlKind::Cc(n);
                if COMMON.iter().any(|(k, _)| *k == kind) {
                    continue;
                }
                let mut on = app.midi.lanes.contains(&kind);
                let has = seq.ctrls.iter().any(|c| c.kind == kind);
                let text = if has { format!("CC{n} •") } else { format!("CC{n}") };
                if ui.checkbox(&mut on, text).changed() {
                    toggle(app, kind, on);
                }
            }
        });
    });
    ui.separator();
    if ui.button("Show Lanes with Data").clicked() {
        for k in lanes_with_data(seq) {
            toggle(app, k, true);
        }
        ui.close();
    }
    if ui.add_enabled(!app.midi.lanes.is_empty(), egui::Button::new("Hide All Lanes")).clicked() {
        app.midi.lanes.clear();
        ui.close();
    }
}

/// Shows (`on`) or hides a lane.
pub fn toggle(app: &mut SoundApp, kind: CtrlKind, on: bool) {
    if on && !app.midi.lanes.contains(&kind) {
        app.midi.lanes.push(kind);
    } else if !on {
        app.midi.lanes.retain(|k| *k != kind);
    }
}

/// Draws and edits the lanes, each in its rectangle; `label_x` is the left edge of the label
/// column (the keys' width). `grid` and `len` are ticks.
pub fn lanes(
    app: &mut SoundApp,
    ui: &mut Ui,
    cid: ClipId,
    seq: &Sequence,
    rects: &[LaneGeometry],
    label_x: f32,
    geo: &RollGeometry,
    grid: i64,
    len: i64,
) {
    for lg in rects {
        lane(app, ui, cid, seq, lg, label_x, geo, grid, len);
    }
}

fn lane(app: &mut SoundApp, ui: &mut Ui, cid: ClipId, seq: &Sequence, lg: &LaneGeometry, label_x: f32, geo: &RollGeometry, grid: i64, len: i64) {
    let t = Tokens::current();
    let r = lg.rect;
    let kind = lg.kind;
    ui.painter().rect_filled(r, 0.0, Color32::from_rgb(28, 28, 30));
    // The label column: the lane's name and a close box.
    let label = Rect::from_min_max(pos2(label_x, r.min.y), pos2(r.min.x - 2.0, r.max.y));
    ui.painter().rect_filled(label, 0.0, t.panel_bg2);
    let (name, number) = lane_short(kind);
    let lp = ui.painter().with_clip_rect(label);
    lp.text(pos2(label.min.x + 3.0, label.min.y + 7.0), Align2::LEFT_CENTER, name, regular(9.5), t.text);
    lp.text(pos2(label.min.x + 3.0, label.min.y + 18.0), Align2::LEFT_CENTER, number, regular(8.5), t.text_dim);
    ui.interact(label, ui.id().with(("ctrl_label", lane_id(kind))), Sense::hover()).on_hover_text(lane_label(kind));
    let close = Rect::from_min_size(pos2(label.min.x + 2.0, label.max.y - 14.0), egui::vec2(12.0, 12.0));
    let cresp = ui.interact(close, ui.id().with(("ctrl_close", lane_id(kind))), Sense::click()).on_hover_text("Hide this lane");
    ui.painter().text(close.center(), Align2::CENTER_CENTER, "×", regular(11.0), if cresp.hovered() { t.text } else { t.text_dim });
    if cresp.clicked() {
        toggle(app, kind, false);
        return;
    }
    let p = ui.painter().with_clip_rect(r);
    if kind == CtrlKind::PitchBend {
        let y = lg.y_of(0);
        p.line_segment([pos2(r.min.x, y), pos2(r.max.x, y)], Stroke::new(1.0, Color32::from_rgb(60, 60, 64)));
    }
    let events = events_of(seq, kind);
    let sel = selected(app, seq, kind);
    let resp = ui.interact(r, ui.id().with(("ctrl_lane", cid.0, lane_id(kind))), Sense::click_and_drag());
    let pencil = app.engine.session().edit.tool == Tool::Pencil;
    let pointer = resp.interact_pointer_pos().or(ui.ctx().pointer_latest_pos());
    let snap = |tick: f64| ((tick / grid.max(1) as f64).round() as i64).saturating_mul(grid.max(1)).clamp(0, len.max(0));
    let tick_at = |x: f32| geo.tick_at(x).round().clamp(0.0, len.max(0) as f64) as i64;
    // Where the button went down, not where the drag was first noticed.
    if resp.drag_started()
        && let Some(at) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
    {
        app.midi.ctrl_drag = Some(if pencil {
            CtrlDrag::Pencil { lane: kind, points: vec![(tick_at(at.x), lg.value_at(at.y))] }
        } else if let Some(i) = point_at(lg, geo, &events, at) {
            if !sel.contains(&i) {
                app.midi.ctrl_sel = Some((kind, vec![i]));
            }
            CtrlDrag::Points { lane: kind, from: at }
        } else {
            CtrlDrag::Band { lane: kind, from: at }
        });
    }
    let drag = app.midi.ctrl_drag.clone().filter(|d| match d {
        CtrlDrag::Points { lane, .. } | CtrlDrag::Pencil { lane, .. } | CtrlDrag::Band { lane, .. } => *lane == kind,
    });
    let sel = selected(app, seq, kind);
    // A points drag: the offset so far (time keeps the grid offset, as notes do).
    let offset = match (&drag, pointer) {
        (Some(CtrlDrag::Points { from, .. }), Some(p)) => {
            let dt = ((f64::from(p.x - from.x) / f64::from(geo.px_per_tick.max(1e-6))) / grid.max(1) as f64).round() as i64 * grid.max(1);
            let dv = if is_switch(kind) { 0 } else { lg.value_delta(p.y - from.y) };
            Some((dt, dv))
        }
        _ => None,
    };
    // The line: stepped, each value held to the next event.
    let (lo, hi) = lane_range(kind);
    let shown: Vec<(usize, i64, i32)> = events
        .iter()
        .map(|(i, c)| match offset.filter(|_| sel.contains(i)) {
            Some((dt, dv)) => (*i, c.tick.saturating_add(dt).max(0), (i64::from(c.value) + dv).clamp(i64::from(lo), i64::from(hi)) as i32),
            None => (*i, c.tick, c.value),
        })
        .collect();
    let mut order = shown.clone();
    order.sort_by_key(|x| x.1);
    if !order.is_empty() {
        let mut line: Vec<Pos2> = Vec::with_capacity(order.len() * 2 + 1);
        for (k, (_, tick, v)) in order.iter().enumerate() {
            let (x, y) = (geo.x_of(*tick), lg.y_of(*v));
            if k > 0
                && let Some(prev) = line.last().copied()
            {
                line.push(pos2(x, prev.y));
            }
            line.push(pos2(x, y));
        }
        if let Some(last) = line.last().copied() {
            line.push(pos2(r.max.x, last.y));
        }
        p.add(Shape::line(line, Stroke::new(1.5, t.automation_line)));
    }
    for (i, tick, v) in &shown {
        let c = pos2(geo.x_of(*tick), lg.y_of(*v));
        if sel.contains(i) {
            p.circle(c, 3.5, t.accent, Stroke::new(1.5, Color32::WHITE));
        } else {
            p.circle(c, 2.5, Color32::from_rgb(40, 40, 40), Stroke::new(1.5, t.automation_line));
        }
    }
    // Readout while dragging a point.
    if let (Some(_), Some(first)) = (offset, sel.first().and_then(|i| shown.iter().find(|x| x.0 == *i))) {
        p.text(pos2(geo.x_of(first.1) + 6.0, lg.y_of(first.2)), Align2::LEFT_CENTER, first.2.to_string(), regular(10.0), t.text);
    }
    match (&drag, pointer) {
        (Some(CtrlDrag::Pencil { .. }), Some(at)) if resp.dragged() => {
            if let Some(CtrlDrag::Pencil { points, .. }) = &mut app.midi.ctrl_drag {
                let tick = tick_at(at.x);
                // A point every few points of pointer travel is plenty.
                if points.last().is_none_or(|(t0, _)| (geo.x_of(tick) - geo.x_of(*t0)).abs() >= 3.0) {
                    points.push((tick, lg.value_at(at.y)));
                }
                let line: Vec<Pos2> = points.iter().map(|(tk, v)| pos2(geo.x_of(*tk), lg.y_of(*v))).collect();
                p.add(Shape::line(line, Stroke::new(1.5, t.accent)));
            }
        }
        (Some(CtrlDrag::Band { from, .. }), Some(at)) => {
            p.rect(Rect::from_two_pos(*from, at), 0.0, t.accent.gamma_multiply(0.15), Stroke::new(1.0, t.accent), StrokeKind::Inside);
        }
        _ => {}
    }
    if resp.drag_stopped() {
        match app.midi.ctrl_drag.take() {
            Some(CtrlDrag::Pencil { points, .. }) => {
                let pts: Vec<[i64; 2]> = points.iter().map(|(tk, v)| [*tk, i64::from(*v)]).collect();
                let _ = app.run("midi.ctrl_draw", json!({"clip": cid.0, "lane": lane_id(kind), "points": pts}));
                app.midi.ctrl_sel = None;
            }
            Some(CtrlDrag::Points { .. }) => {
                if let Some((dt, dv)) = offset.filter(|o| *o != (0, 0))
                    && let Ok(v) = app.run("midi.ctrl_move", json!({"clip": cid.0, "indices": sel, "ticks": dt, "value": dv}))
                {
                    let moved =
                        v["indices"].as_array().map(|a| a.iter().filter_map(|x| x.as_u64()).map(|x| x as usize).collect()).unwrap_or_default();
                    app.midi.ctrl_sel = Some((kind, moved));
                }
            }
            Some(CtrlDrag::Band { from, .. }) => {
                if let Some(at) = pointer {
                    let band = Rect::from_two_pos(from, at);
                    let inside = events.iter().filter(|(_, c)| band.contains(pos2(geo.x_of(c.tick), lg.y_of(c.value)))).map(|(i, _)| *i).collect();
                    app.midi.ctrl_sel = Some((kind, inside));
                }
            }
            None => {}
        }
    }
    if resp.clicked()
        && let Some(at) = resp.interact_pointer_pos()
    {
        let mods = ui.input(|i| i.modifiers);
        match point_at(lg, geo, &events, at) {
            Some(i) if mods.alt => {
                let _ = app.run("midi.ctrl_delete", json!({"clip": cid.0, "indices": [i]}));
                app.midi.ctrl_sel = None;
            }
            Some(i) if mods.shift => {
                let mut v = sel.clone();
                if let Some(k) = v.iter().position(|x| *x == i) {
                    v.remove(k);
                } else {
                    v.push(i);
                }
                app.midi.ctrl_sel = Some((kind, v));
            }
            Some(i) => app.midi.ctrl_sel = Some((kind, vec![i])),
            // A single click on empty space adds a point (on the grid), as Studio One's arrow does.
            None => {
                let tick = snap(geo.tick_at(at.x));
                if let Ok(v) = app.run("midi.ctrl_add", json!({"clip": cid.0, "lane": lane_id(kind), "tick": tick, "value": lg.value_at(at.y)}))
                    && let Some(i) = v["index"].as_u64()
                {
                    app.midi.ctrl_sel = Some((kind, vec![i as usize]));
                }
            }
        }
    }
    // Right-click: a point's value to type or drag, Delete Point, Hide Lane.
    if resp.secondary_clicked()
        && let Some(at) = resp.interact_pointer_pos()
    {
        app.midi.ctrl_menu = point_at(lg, geo, &events, at);
        if let Some(i) = app.midi.ctrl_menu {
            app.midi.ctrl_sel = Some((kind, vec![i]));
        }
    }
    resp.context_menu(|ui| point_menu(app, ui, cid, seq, kind));
    // Delete over the lane removes its selected points.
    if !sel.is_empty()
        && ui.rect_contains_pointer(r)
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
    {
        let _ = app.run("midi.ctrl_delete", json!({"clip": cid.0, "indices": sel}));
        app.midi.ctrl_sel = None;
    }
}

/// The lane's right-click menu: the point's value (when one was under the pointer), Delete
/// Point, Hide Lane.
fn point_menu(app: &mut SoundApp, ui: &mut Ui, cid: ClipId, seq: &Sequence, kind: CtrlKind) {
    let point = app.midi.ctrl_menu.and_then(|i| seq.ctrls.get(i).filter(|c| c.kind == kind).map(|c| (i, *c)));
    if let Some((i, c)) = point {
        let (lo, hi) = lane_range(kind);
        let mut v = c.value;
        let r = ui.horizontal(|ui| {
            ui.label("Value");
            ui.add(egui::DragValue::new(&mut v).range(lo..=hi))
        });
        if r.inner.drag_started() || r.inner.gained_focus() {
            app.midi.gesture = app.midi.gesture.wrapping_add(1);
        }
        if r.inner.changed() {
            let key = format!("ctrl:{}:{}", cid.0, app.midi.gesture);
            if let Err(e) = app.engine.execute_merged("midi.ctrl_edit", &json!({"clip": cid.0, "index": i, "value": v}), &key) {
                app.ui.status = e.to_string();
            }
        }
        if ui.button("Delete Point").clicked() {
            let _ = app.run("midi.ctrl_delete", json!({"clip": cid.0, "indices": [i]}));
            app.midi.ctrl_sel = None;
            ui.close();
        }
        ui.separator();
    }
    if ui.button("Hide Lane").clicked() {
        toggle(app, kind, false);
        ui.close();
    }
}
