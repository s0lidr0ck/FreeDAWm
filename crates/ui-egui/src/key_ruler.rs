//! The Key ruler (id `key`): the session's key changes.
//!
//! A double-click on empty space opens the key picker at that time (it starts on the key in
//! force there); a click picks a key change (outlined) for Delete; a double-click on one, or its
//! right-click › Edit Key…, opens the picker on it; right-click › Delete Key removes it; a drag
//! moves it to the grid. The ruler's name offers Clear All Key Changes… (`ruler_items`). With no
//! key changes the ruler shows the band key, dimmed, or "No key set" (`session_key`). Every action
//! is one engine command: `event.add_key_change`, `event.key_change_edit`,
//! `event.key_change_delete`.

use crate::SoundApp;
use crate::dialogs::Dialog;
use crate::edit_window::{sample_at, x_of};
use crate::ruler_items::{RulerItem, pick};
use crate::session_key::{key_pc_at, root_spelling};
use crate::theme::{Tokens, bold};
use egui::{Align2, Color32, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::Session;
use soundcraft_time::Samples;

const BG: Color32 = Color32::from_rgb(64, 52, 88);
const LINE: Color32 = Color32::from_rgb(200, 180, 240);
const TEXT: Color32 = Color32::from_rgb(236, 230, 250);
const DIM: Color32 = Color32::from_rgb(160, 148, 186);

/// A drag in progress: the key change's position and the pointer's x at the press.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    at: Samples,
    press_x: f32,
}

/// Opens the key picker at `at`: on the key change `edit` names, or (adding one) on the key in
/// force there, else the band key.
pub fn open_picker(app: &mut SoundApp, at: Samples, edit: Option<&str>) {
    let s = app.engine.session();
    let (root, scale) = edit.and_then(soundcraft_engine::vocal::parse_key).unwrap_or_else(|| key_pc_at(s, at));
    app.dialogs.open = Some(Dialog::KeyPicker { at, edit: edit.is_some(), root: root % 12, minor: scale == 1 });
}

/// The picker's body: Major / Minor, then the 12 roots spelled as that mode's keys spell them.
pub fn picker_body(ui: &mut Ui, root: &mut u8, minor: &mut bool) {
    ui.horizontal(|ui| {
        if ui.selectable_label(!*minor, "Major").clicked() {
            *minor = false;
        }
        if ui.selectable_label(*minor, "Minor").clicked() {
            *minor = true;
        }
    });
    egui::Grid::new("key_picker_roots").spacing(vec2(4.0, 4.0)).show(ui, |ui| {
        for pc in 0..12u8 {
            let b = egui::Button::selectable(*root == pc, root_spelling(pc, *minor)).min_size(vec2(38.0, 24.0));
            if ui.add(b).clicked() {
                *root = pc;
            }
            if pc == 5 {
                ui.end_row();
            }
        }
    });
}

/// The nearest grid line (a moved key always lands on the grid).
fn grid_snap(s: &Session, at: Samples) -> Samples {
    s.edit.grid.snap(at.max(0), s.sample_rate, &s.tempo, s.frame_rate).max(0)
}

/// Draws the Key ruler and runs its clicks, drags and menus.
pub fn key_ruler(app: &mut SoundApp, ui: &mut Ui, painter: &Painter, s: &Session, tl: Rect, row: Rect) {
    let t = Tokens::current();
    painter.rect_filled(row, 0.0, BG);
    let id = ui.id().with("key_ruler");
    let drag_id = id.with("drag");
    let menu_id = id.with("menu");
    let resp = ui.interact(row, id, Sense::click_and_drag());

    // Each key change's line and name, for hits (the name is part of the key change).
    let spots: Vec<(Samples, Rect)> = s
        .key_signatures
        .iter()
        .map(|(at, k)| {
            let x = x_of(s, tl, *at);
            let w = painter.layout_no_wrap(k.clone(), bold(11.0), TEXT).size().x;
            (*at, Rect::from_min_max(pos2(x - 4.0, row.min.y + 1.0), pos2(x + 9.0 + w, row.max.y - 1.0)))
        })
        .collect();
    let hit = |x: f32| spots.iter().rev().find(|(_, r)| x >= r.min.x && x <= r.max.x).map(|(at, _)| *at);

    // Drag: from the press on a key change to the grid line under its moved line.
    if resp.drag_started() {
        let origin = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
        let d = origin.and_then(|p| hit(p.x).map(|at| Drag { at, press_x: p.x }));
        ui.ctx().data_mut(|m| {
            if let Some(d) = d {
                m.insert_temp(drag_id, d);
            } else {
                m.remove::<Drag>(drag_id);
            }
        });
    }
    let drag = ui.ctx().data(|m| m.get_temp::<Drag>(drag_id));
    let target = |d: Drag, p: Pos2| grid_snap(s, sample_at(s, tl, x_of(s, tl, d.at) + p.x - d.press_x));
    let pointer = resp.interact_pointer_pos().or_else(|| ui.input(|i| i.pointer.latest_pos()));
    let live = drag.filter(|_| resp.dragged()).zip(pointer).map(|(d, p)| (d.at, target(d, p)));

    let picked = match app.ui.ruler_item {
        Some(RulerItem::Key(at)) => Some(at),
        _ => None,
    };
    if s.key_signatures.is_empty() {
        let text = crate::session_key::empty_ruler_text(s);
        painter.text(pos2(tl.min.x + 6.0, row.center().y), Align2::LEFT_CENTER, text, bold(11.0), DIM);
    }
    for ((at, k), (_, spot)) in s.key_signatures.iter().zip(&spots) {
        let x = x_of(s, tl, *at);
        let moving = live.is_some_and(|(from, _)| from == *at);
        let (line, text) = if moving { (LINE.gamma_multiply(0.4), TEXT.gamma_multiply(0.4)) } else { (LINE, TEXT) };
        painter.line_segment([pos2(x, row.min.y + 2.0), pos2(x, row.max.y - 2.0)], Stroke::new(2.0, line));
        painter.text(pos2(x + 5.0, row.center().y), Align2::LEFT_CENTER, k, bold(11.0), text);
        if picked == Some(*at) {
            painter.rect_stroke(*spot, 2.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
        }
    }
    if let Some((from, to)) = live
        && let Some((_, k)) = s.key_signatures.iter().find(|(t, _)| *t == from)
    {
        let x = x_of(s, tl, to);
        painter.line_segment([pos2(x, row.min.y + 2.0), pos2(x, row.max.y - 2.0)], Stroke::new(2.0, t.accent));
        painter.text(pos2(x + 5.0, row.center().y), Align2::LEFT_CENTER, k, bold(11.0), TEXT);
    }
    if resp.drag_stopped() {
        ui.ctx().data_mut(|m| m.remove::<Drag>(drag_id));
        if let (Some(d), Some(p)) = (drag, pointer) {
            let to = target(d, p);
            if to != d.at {
                match app.run("event.key_change_edit", json!({"at": d.at, "to": to})) {
                    Ok(_) => pick(app, RulerItem::Key(to)),
                    Err(e) => app.ui.status = e,
                }
            }
        }
    }

    // Click picks; double-click edits a key change or adds one (snapped as the Edit window snaps).
    if let Some(p) = resp.interact_pointer_pos() {
        let on = hit(p.x);
        if resp.double_clicked() {
            match on.and_then(|at| s.key_signatures.iter().find(|(t, _)| *t == at)) {
                Some((at, k)) => open_picker(app, *at, Some(k)),
                None => open_picker(app, crate::edit_window::snap(s, sample_at(s, tl, p.x).max(0)), None),
            }
        } else if resp.clicked()
            && let Some(at) = on
        {
            pick(app, RulerItem::Key(at));
        }
    }

    // Right-click: the key change's menu, then the ruler's.
    if resp.secondary_clicked() {
        let on = resp.interact_pointer_pos().and_then(|p| hit(p.x));
        if let Some(at) = on {
            pick(app, RulerItem::Key(at));
        }
        ui.ctx().data_mut(|m| m.insert_temp(menu_id, on));
    }
    let menu_target = ui.ctx().data(|m| m.get_temp::<Option<Samples>>(menu_id)).flatten();
    resp.context_menu(|ui| {
        if let Some(at) = menu_target {
            crate::ruler_items::key_menu(app, ui, at);
        }
        crate::menus::ruler_context_menu(app, ui, "key");
    });
}
