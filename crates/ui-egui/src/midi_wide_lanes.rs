//! The MIDI Editor's 14-bit and RPN/NRPN lanes (Lanes › 14-bit and RPN), under its controller
//! lanes: a 14-bit controller pair (CC n with CC n+32) or a registered or non-registered
//! parameter, shown and edited as one value 0..16383 (`midi.ctrl14s`, `midi.ctrl14_draw`,
//! `midi.ctrl14_clear`; [`soundcraft_engine::cmd::midi_wide`]). A click sets a value at the grid
//! line (one undo step), a drag draws a line of values freehand, Alt-click removes the value
//! there. They show and write the channel the Lanes menu picks, as the controller lanes do.

use crate::SoundApp;
use crate::midi_editor::RollGeometry;
use crate::theme::{Tokens, regular};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, Ui, pos2};
use serde_json::json;
use soundcraft_engine::cmd::midi_wide::{WIDE_MAX, WideLane, lanes_of, points_of};
use soundcraft_midi::Sequence;
use soundcraft_model::ClipId;

/// The lanes the menu always offers, before those the clip has data on.
pub const OFFERED: [WideLane; 7] =
    [WideLane::Cc14(1), WideLane::Cc14(7), WideLane::Cc14(11), WideLane::Rpn(0), WideLane::Rpn(1), WideLane::Rpn(2), WideLane::Rpn(5)];

/// Shows (`on`) or hides a lane.
pub fn toggle(app: &mut SoundApp, lane: WideLane, on: bool) {
    if on && !app.midi.wide_lanes.contains(&lane) {
        app.midi.wide_lanes.push(lane);
    } else if !on {
        app.midi.wide_lanes.retain(|l| *l != lane);
    }
}

/// Lanes › 14-bit and RPN: a check item per lane offered or with data, and every other 14-bit
/// controller.
pub fn menu(app: &mut SoundApp, ui: &mut Ui, seq: &Sequence) {
    let with_data = lanes_of(seq);
    let item = |app: &mut SoundApp, ui: &mut Ui, l: WideLane| {
        let mut on = app.midi.wide_lanes.contains(&l);
        let text = if with_data.contains(&l) { format!("{} \u{2022}", l.label()) } else { l.label() };
        if ui.checkbox(&mut on, text).changed() {
            toggle(app, l, on);
        }
    };
    for l in OFFERED.into_iter().chain(with_data.iter().copied().filter(|l| !OFFERED.contains(l))) {
        item(app, ui, l);
    }
    ui.menu_button("Other 14-bit CC", |ui| {
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for n in 0u8..=31 {
                let l = WideLane::Cc14(n);
                if !OFFERED.contains(&l) {
                    item(app, ui, l);
                }
            }
        });
    });
}

fn y_of(r: Rect, v: i32) -> f32 {
    r.max.y - 3.0 - (r.height() - 6.0).max(1.0) * v.clamp(0, WIDE_MAX) as f32 / WIDE_MAX as f32
}

fn value_at(r: Rect, y: f32) -> i32 {
    (((r.max.y - 3.0 - y) / (r.height() - 6.0).max(1.0)) * WIDE_MAX as f32).round().clamp(0.0, WIDE_MAX as f32) as i32
}

/// Draws and edits one lane in `r`; `label_x` is the left edge of the label column. `grid` and
/// `len` are ticks.
#[allow(clippy::too_many_arguments)]
pub fn lane(
    app: &mut SoundApp,
    ui: &mut Ui,
    cid: ClipId,
    seq: &Sequence,
    lane: WideLane,
    r: Rect,
    label_x: f32,
    geo: &RollGeometry,
    grid: i64,
    len: i64,
) {
    let t = Tokens::current();
    ui.painter().rect_filled(r, 0.0, Color32::from_rgb(28, 28, 30));
    let label = Rect::from_min_max(pos2(label_x, r.min.y), pos2(r.min.x - 2.0, r.max.y));
    ui.painter().rect_filled(label, 0.0, t.panel_bg2);
    let (name, sub) = match lane {
        WideLane::Cc14(n) => (format!("CC{n}"), "14-bit".to_string()),
        WideLane::Rpn(p) => (format!("RPN {p}"), "param".to_string()),
        WideLane::Nrpn(p) => (format!("NRPN {}/{}", p >> 7, p & 127), "param".to_string()),
    };
    let lp = ui.painter().with_clip_rect(label);
    lp.text(pos2(label.min.x + 3.0, label.min.y + 7.0), Align2::LEFT_CENTER, name, regular(9.5), t.text);
    lp.text(pos2(label.min.x + 3.0, label.min.y + 18.0), Align2::LEFT_CENTER, sub, regular(8.5), t.text_dim);
    ui.interact(label, ui.id().with(("wide_label", lane.id())), Sense::hover()).on_hover_text(lane.label());
    let close = Rect::from_min_size(pos2(label.min.x + 2.0, label.max.y - 14.0), egui::vec2(12.0, 12.0));
    let cresp = ui.interact(close, ui.id().with(("wide_close", lane.id())), Sense::click()).on_hover_text("Hide this lane");
    ui.painter().text(close.center(), Align2::CENTER_CENTER, "×", regular(11.0), if cresp.hovered() { t.text } else { t.text_dim });
    if cresp.clicked() {
        toggle(app, lane, false);
        return;
    }
    let shown = app.midi.ctrl_channel;
    let channel = crate::midi_lanes::write_channel(shown, seq);
    let pts = points_of(seq, lane, shown);
    // The held value as a stepped line, a dot per point.
    let p = ui.painter().with_clip_rect(r);
    let line = Color32::from_rgb(120, 190, 250);
    for (k, (tick, ch, v)) in pts.iter().enumerate() {
        let x0 = geo.x_of(*tick);
        let x1 = pts.iter().skip(k + 1).find(|q| q.1 == *ch).map_or(r.max.x, |q| geo.x_of(q.0));
        let y = y_of(r, *v);
        let c = if *ch == channel { line } else { line.gamma_multiply(0.4) };
        p.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(1.5, c));
        p.circle_filled(pos2(x0, y), 2.5, c);
    }
    let resp = ui.interact(r, ui.id().with(("wide_lane", cid.0, lane.id())), Sense::click_and_drag());
    let session = app.engine.session_arc();
    let base = session.find_clip(cid).map_or(0, |(_, c)| session.tempo.samples_to_ticks(c.start, session.sample_rate));
    let snap = |x: f32| {
        crate::midi_edit_tools::snap_abs(&session.tempo, base as f64 + geo.tick_at(x), grid.max(1)).saturating_sub(base).clamp(0, len.max(0))
    };
    let free = |x: f32| geo.tick_at(x).round().clamp(0.0, len.max(0) as f64) as i64;
    if let Some(h) = resp.hover_pos() {
        resp.clone().on_hover_text_at_pointer(format!("{} = {}", lane.label(), value_at(r, h.y)));
    }
    let alt = ui.input(|i| i.modifiers.alt);
    let id = lane.id();
    if resp.drag_started()
        && let Some(at) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
    {
        app.midi.wide_draw = Some((lane, vec![(free(at.x), value_at(r, at.y))]));
    }
    if let Some((l, drawn)) = app.midi.wide_draw.as_mut()
        && *l == lane
    {
        if let Some(at) = resp.interact_pointer_pos() {
            let tick = free(at.x);
            match drawn.last_mut() {
                Some(last) if last.0 == tick => last.1 = value_at(r, at.y),
                _ => drawn.push((tick, value_at(r, at.y))),
            }
        }
        let pts: Vec<Pos2> = drawn.iter().map(|(t, v)| pos2(geo.x_of(*t), y_of(r, *v))).collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.5, Color32::WHITE)));
        if resp.drag_stopped() {
            let points: Vec<[i64; 2]> = drawn.iter().map(|(t, v)| [*t, i64::from(*v)]).collect();
            app.midi.wide_draw = None;
            if let Err(e) = app.run("midi.ctrl14_draw", json!({"clip": cid.0, "lane": id, "points": points, "channel": channel})) {
                app.ui.status = e;
            }
        }
        return;
    }
    if resp.clicked()
        && let Some(at) = resp.interact_pointer_pos()
    {
        let res = if alt {
            // The point nearest the click, within 6 points.
            match pts
                .iter()
                .filter(|q| q.1 == channel)
                .map(|q| (q.0, (geo.x_of(q.0) - at.x).abs()))
                .filter(|q| q.1 <= 6.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
            {
                Some((tick, _)) => app.run("midi.ctrl14_clear", json!({"clip": cid.0, "lane": id, "from": tick, "to": tick, "channel": channel})),
                None => return,
            }
        } else {
            app.run("midi.ctrl14_draw", json!({"clip": cid.0, "lane": id, "points": [[snap(at.x), value_at(r, at.y)]], "channel": channel}))
        };
        if let Err(e) = res {
            app.ui.status = e;
        }
    }
}
