//! The Notes tab (Window › Notes): the session's to-do list. "+ Note" adds one and starts typing
//! in it; "At playhead" adds one pinned to the playhead, whose time shows as a link that locates
//! there (and as a small flag on the Markers ruler while it is open). The box ticks a note done
//! (struck through, dim); Show filters Open / Done / All; `#words` are tags. Drag the grip to
//! reorder; right-click: Pin to Playhead, Unpin, Delete. Every change is one undo step.

use crate::SoundApp;
use crate::theme::{Tokens, mono, regular};
use egui::{Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{Note, Session};
use soundcraft_time::Samples;

const ROW_H: f32 = 38.0;
const GRIP_W: f32 = 12.0;

/// The note being typed into: its id and the text as typed (tags included).
#[derive(Clone, Default)]
struct Editing {
    id: u64,
    text: String,
    focus: bool,
}

fn editing_key() -> egui::Id {
    egui::Id::new("soundcraft-notes-editing")
}

/// The name of the place at `at`: the lyric line placed there ("Line 7"), else the last marker at
/// or before it (else none).
pub fn place_name(s: &Session, at: Samples) -> Option<String> {
    if let Some(n) = s.lyric_sheet.placed_at(at).and_then(|p| s.lyric_sheet.number_of(p.line)) {
        return Some(format!("Line {n}"));
    }
    s.markers
        .iter()
        .filter(|m| m.kind == soundcraft_model::MarkerKind::Marker && m.start <= at && m.track.is_none())
        .max_by_key(|m| m.start)
        .map(|m| m.name.clone())
}

/// A pinned note's link text: "<place> · <time in the main counter>".
pub fn pin_text(s: &Session, at: Samples) -> String {
    let time = s.format_position(at, s.edit.main_counter);
    match place_name(s, at) {
        Some(p) => format!("{p} \u{b7} {time}"),
        None => time,
    }
}

pub(crate) fn show(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    ui.painter().rect_filled(ui.max_rect(), 0.0, t.panel_bg);
    let mut editing: Option<Editing> = ui.ctx().data(|d| d.get_temp(editing_key()));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let add = egui::Button::new(egui::RichText::new("+ Note").color(Color32::WHITE)).fill(t.accent).min_size(vec2(0.0, 24.0));
        if ui.add(add).on_hover_text("Add a note and type it (#word adds a tag)").clicked()
            && let Ok(v) = app.run("notes.add", json!({}))
        {
            editing = Some(Editing { id: v["id"].as_u64().unwrap_or(0), text: String::new(), focus: true });
            app.ui.notes_show = "open".into();
        }
        let at = egui::Button::new("At playhead").min_size(vec2(0.0, 24.0));
        if ui.add(at).on_hover_text("Add a note pinned to the playhead").clicked()
            && let Ok(v) = app.run("notes.add", json!({"at_playhead": true}))
        {
            editing = Some(Editing { id: v["id"].as_u64().unwrap_or(0), text: String::new(), focus: true });
            app.ui.notes_show = "open".into();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            let label = match app.ui.notes_show.as_str() {
                "done" => "Done",
                "all" => "All",
                _ => "Open",
            };
            egui::ComboBox::from_id_salt("notes-show").selected_text(label).width(70.0).show_ui(ui, |ui| {
                for (id, l) in [("open", "Open"), ("done", "Done"), ("all", "All")] {
                    if ui.selectable_label(app.ui.notes_show == id, l).clicked() {
                        app.ui.notes_show = id.into();
                    }
                }
            });
            ui.label(egui::RichText::new("Show:").color(t.text_dim));
        });
    });
    ui.add_space(4.0);
    let (notes, pins): (Vec<Note>, Vec<Option<String>>) = {
        let s = app.engine.session();
        let notes: Vec<Note> = s.notes.iter().filter(|n| soundcraft_engine::cmd::notes::note_shown(n, &app.ui.notes_show, None)).cloned().collect();
        let pins = notes.iter().map(|n| n.at.map(|a| pin_text(s, a))).collect();
        (notes, pins)
    };
    if notes.is_empty() {
        let msg = match app.ui.notes_show.as_str() {
            "done" => "Nothing done yet.",
            _ if app.engine.session().notes.is_empty() => "No notes. Click + Note and type; \"At playhead\" pins a note to a time.",
            _ => "No open notes.",
        };
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(egui::RichText::new(msg).color(t.text_dim));
        });
    }
    let mut drop: Option<(u64, usize)> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut rows: Vec<Rect> = Vec::new();
        let mut dragging: Option<(usize, f32)> = None;
        for (i, (n, pin)) in notes.iter().zip(&pins).enumerate() {
            let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
            rows.push(row);
            let p = ui.painter();
            p.line_segment([pos2(row.min.x, row.max.y), pos2(row.max.x, row.max.y)], Stroke::new(1.0, t.border));
            // Grip: drag to reorder.
            let grip = Rect::from_min_size(row.min, vec2(GRIP_W, ROW_H));
            let g = ui.interact(grip, ui.id().with(("note-grip", n.id)), Sense::drag());
            if g.hovered() || g.dragged() {
                for k in 0..3 {
                    let y = row.center().y - 5.0 + k as f32 * 5.0;
                    p.line_segment([pos2(grip.min.x + 4.0, y), pos2(grip.max.x - 2.0, y)], Stroke::new(1.0, t.text_dim));
                }
                ui.ctx().set_cursor_icon(if g.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
            }
            if (g.dragged() || g.drag_stopped())
                && let Some(pt) = ui.ctx().pointer_latest_pos()
            {
                dragging = Some((i, pt.y));
                if g.drag_stopped() {
                    let to = gap_at(&notes, i, pt.y, row, ROW_H);
                    let global = app.engine.session().notes.iter().position(|m| Some(m.id) == notes.get(to).map(|x| x.id));
                    let global = global.unwrap_or(app.engine.session().notes.len().saturating_sub(1));
                    drop = Some((n.id, global));
                }
            }
            // The done box.
            let bx = Rect::from_center_size(pos2(row.min.x + GRIP_W + 12.0, row.center().y), vec2(18.0, 18.0));
            let b = ui.interact(bx.expand(3.0), ui.id().with(("note-done", n.id)), Sense::click());
            if n.done {
                p.rect_filled(bx, 3.0, Color32::from_rgb(80, 200, 110));
            } else {
                p.rect_stroke(bx, 3.0, Stroke::new(1.5, if b.hovered() { t.text } else { t.text_dim }), StrokeKind::Inside);
            }
            if b.on_hover_text(if n.done { "Mark open" } else { "Mark done" }).clicked() {
                let _ = app.run("notes.edit", json!({"id": n.id, "done": !n.done}));
            }
            // The pin link, right-aligned.
            let mut right = row.max.x - 12.0;
            if let (Some(pin), Some(at)) = (pin, n.at) {
                let g = p.layout_no_wrap(pin.clone(), mono(11.5), if n.done { t.text_dim } else { Color32::from_rgb(110, 160, 255) });
                let r = Rect::from_min_size(pos2(right - g.size().x, row.center().y - g.size().y / 2.0), g.size());
                let l = ui.interact(r.expand(2.0), ui.id().with(("note-pin", n.id)), Sense::click());
                if l.hovered() {
                    p.galley_with_override_text_color(r.min, g, Color32::WHITE);
                } else {
                    p.galley(r.min, g, Color32::WHITE);
                }
                if l.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Locate here").clicked() {
                    let _ = app.run("transport.locate", json!({"at": at}));
                }
                right = r.min.x - 12.0;
            }
            // Text (or the editor while typing).
            let tx = bx.max.x + 12.0;
            let text_rect = Rect::from_min_max(pos2(tx, row.min.y + 4.0), pos2(right.max(tx + 40.0), row.max.y - 4.0));
            let is_editing = editing.as_ref().is_some_and(|e| e.id == n.id);
            if is_editing {
                if let Some(ed) = editing.as_mut() {
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(text_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
                    let te = egui::TextEdit::singleline(&mut ed.text)
                        .frame(egui::Frame::NONE)
                        .font(regular(14.0))
                        .hint_text("Type a note (#tag)")
                        .desired_width(text_rect.width());
                    let r = child.add(te);
                    if std::mem::take(&mut ed.focus) {
                        r.request_focus();
                    }
                    if r.changed() {
                        let _ = app.engine.execute_merged("notes.edit", &json!({"id": n.id, "text": ed.text}), &format!("note-text:{}", n.id));
                    }
                    if r.lost_focus() {
                        app.engine.end_merge();
                        // A note left empty goes again.
                        if ed.text.trim().is_empty() {
                            let _ = app.engine.execute_joined("notes.delete", &json!({"id": n.id}));
                        }
                        editing = None;
                    }
                }
            } else {
                let col = if n.done { t.text_dim } else { t.text };
                let g = p.layout_no_wrap(n.text.clone(), regular(14.0), col);
                let gw = g.size().x.min(text_rect.width());
                let ty = row.center().y - g.size().y / 2.0;
                p.with_clip_rect(text_rect).galley(pos2(tx, ty), g, col);
                if n.done {
                    p.line_segment([pos2(tx, row.center().y), pos2(tx + gw, row.center().y)], Stroke::new(1.2, t.text_dim));
                }
                let mut x = tx + gw + 10.0;
                for tag in &n.tags {
                    let g = p.layout_no_wrap(tag.clone(), regular(11.0), t.text_dim);
                    let chip = Rect::from_min_size(pos2(x, row.center().y - 9.0), vec2(g.size().x + 14.0, 18.0));
                    if chip.max.x > right {
                        break;
                    }
                    p.rect_filled(chip, 9.0, t.button);
                    p.galley(pos2(chip.min.x + 7.0, chip.center().y - g.size().y / 2.0), g, t.text_dim);
                    x = chip.max.x + 6.0;
                }
                let hit = ui.interact(text_rect, ui.id().with(("note-text", n.id)), Sense::click());
                if hit.on_hover_cursor(egui::CursorIcon::Text).clicked() {
                    app.engine.end_merge();
                    editing = Some(Editing { id: n.id, text: soundcraft_model::note_typed(n), focus: true });
                }
            }
            resp.context_menu(|ui| {
                if ui.button("Pin to Playhead").clicked() {
                    let _ = app.run("notes.edit", json!({"id": n.id, "at_playhead": true}));
                    ui.close();
                }
                if n.at.is_some() && ui.button("Unpin").clicked() {
                    let _ = app.run("notes.edit", json!({"id": n.id, "at": null}));
                    ui.close();
                }
                ui.separator();
                if ui.button("Delete").clicked() {
                    let _ = app.run("notes.delete", json!({"id": n.id}));
                    ui.close();
                }
            });
        }
        // The gap a dragged note lands in.
        if let Some((_, y)) = dragging
            && let Some(first) = rows.first()
        {
            let gap = ((y - first.min.y) / ROW_H).round().clamp(0.0, rows.len() as f32);
            let ly = first.min.y + gap * ROW_H;
            ui.painter().line_segment([pos2(first.min.x, ly), pos2(first.max.x, ly)], Stroke::new(2.0, t.accent));
        }
    });
    if let Some((id, to)) = drop {
        let _ = app.run("notes.edit", json!({"id": id, "index": to}));
    }
    ui.ctx().data_mut(|d| match editing {
        Some(e) => {
            d.insert_temp(editing_key(), e);
        }
        None => d.remove::<Editing>(editing_key()),
    });
}

/// The shown row a note dragged from row `from` (drawn at `row`) lands on at pointer `y`.
fn gap_at(notes: &[Note], from: usize, y: f32, row: Rect, h: f32) -> usize {
    let first_y = row.min.y - from as f32 * h;
    let gap = ((y - first_y) / h).round().clamp(0.0, notes.len() as f32) as usize;
    // Below its own row the gap counts the row itself.
    if gap > from { gap - 1 } else { gap }.min(notes.len().saturating_sub(1))
}

/// Flags for the open pinned notes on the Markers ruler (hover shows the note).
pub(crate) fn ruler_flags(app: &mut SoundApp, ui: &mut Ui, painter: &egui::Painter, tl: Rect, row: Rect, x_of: impl Fn(Samples) -> f32) {
    let s = app.engine.session_arc();
    for n in s.notes.iter().filter(|n| !n.done) {
        let Some(at) = n.at else { continue };
        let x = x_of(at);
        if x < tl.min.x - 8.0 || x > tl.max.x {
            continue;
        }
        let c = Color32::from_rgb(110, 160, 255);
        let top = row.min.y + 3.0;
        painter.line_segment([pos2(x, top), pos2(x, row.max.y - 2.0)], Stroke::new(1.0, c));
        painter.add(egui::Shape::convex_polygon(vec![pos2(x, top), pos2(x + 8.0, top + 3.5), pos2(x, top + 7.0)], c, Stroke::NONE));
        let r = Rect::from_min_max(pos2(x - 2.0, row.min.y), pos2(x + 9.0, row.max.y));
        let tip = if n.text.is_empty() { "Note".to_string() } else { format!("Note: {}", n.text) };
        let h = ui.interact(r, ui.id().with(("note-flag", n.id)), Sense::click());
        if h.on_hover_text(tip).clicked() {
            app.ui.show_notes = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_note_names_its_place() {
        let mut s = Session::default();
        let a = s.sample_rate.samples(10.0);
        s.add_marker("Chorus 2", soundcraft_model::MarkerKind::Marker, a, a);
        assert!(pin_text(&s, a - 1).find('\u{b7}').is_none());
        assert!(pin_text(&s, a + 100).starts_with("Chorus 2 \u{b7} "));
    }

    #[test]
    fn dragged_note_lands_in_the_gap() {
        let notes: Vec<Note> = (1..=4).map(|id| Note { id, ..Note::default() }).collect();
        let row = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 10.0));
        // Row 0 dragged below row 2.
        assert_eq!(gap_at(&notes, 0, 30.0, row, 10.0), 2);
        // Row 2 (drawn at y 20) dragged to the top.
        let row2 = Rect::from_min_size(pos2(0.0, 20.0), vec2(100.0, 10.0));
        assert_eq!(gap_at(&notes, 2, -3.0, row2, 10.0), 0);
        assert_eq!(gap_at(&notes, 2, 500.0, row2, 10.0), 3);
    }
}
