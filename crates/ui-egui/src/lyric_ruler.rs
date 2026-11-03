//! The Lyrics ruler (View › Rulers › Lyrics): lyric-sheet lines placed in time, each a range
//! marker showing "<n> <text>" from the line's current text. Drag a marker to move it, drag its
//! ends to retime it (grid snapping as for selections); click selects its range (to loop, punch
//! or comp it); double-click shows the line in the Lyrics tab; right-click to remove it. Drop a
//! line dragged from the Lyrics tab's gutter to place it there.

use crate::SoundApp;
use crate::edit_window::{sample_at, snap, x_of};
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{LyricPlace, Session};
use soundcraft_time::Samples;

/// What a drag from the Lyrics tab carries: the sheet line's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineDrag(pub u64);

/// Which part of a marker a drag holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    Move,
    Start,
    End,
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    line: u64,
    grip: Grip,
    /// Pointer sample minus the marker start, when the drag began.
    offset: Samples,
    start: Samples,
    end: Samples,
}

/// Fill and edge of a placed line, and of the one whose range is the edit selection.
const FILL: Color32 = Color32::from_rgb(52, 40, 84);
const EDGE: Color32 = Color32::from_rgb(150, 112, 240);
/// The Lyrics tab's dot for a placed line.
pub const EDGE_DOT: Color32 = Color32::from_rgb(96, 130, 255);
const PICKED: Color32 = Color32::from_rgb(50, 110, 230);
/// Pixels at each end of a marker that retime it.
const END_PX: f32 = 5.0;

fn drag_key() -> egui::Id {
    egui::Id::new("soundcraft-lyric-ruler-drag")
}

/// The id the Lyrics tab scrolls to (and flashes) after a double-click on its marker.
pub fn jump_key() -> egui::Id {
    egui::Id::new("soundcraft-lyrics-jump")
}

/// The marker's title: "<n> <text>".
pub fn title(s: &Session, line: u64) -> String {
    let sheet = &s.lyric_sheet;
    let text = sheet.lines.iter().find(|l| l.id == line).map_or("", |l| l.text.trim());
    match sheet.number_of(line) {
        Some(n) => format!("{n}  {text}"),
        None => text.to_string(),
    }
}

/// Which marker (and which part of it) is at `x`: ends first, then the latest-starting body.
pub fn hit(places: &[(LyricPlace, f32, f32)], x: f32) -> Option<(u64, Grip)> {
    for (p, x0, x1) in places.iter().rev() {
        if (x - x0).abs() <= END_PX && x1 - x0 > 2.0 * END_PX {
            return Some((p.line, Grip::Start));
        }
        if (x - x1).abs() <= END_PX {
            return Some((p.line, Grip::End));
        }
    }
    places.iter().rev().find(|(_, x0, x1)| x >= *x0 && x <= *x1).map(|(p, _, _)| (p.line, Grip::Move))
}

/// The range a drag of `d` with the pointer at sample `at` gives (snapped; at least one sample).
fn dragged(s: &Session, d: Drag, at: Samples) -> (Samples, Samples) {
    match d.grip {
        Grip::Move => {
            let start = snap(s, at.saturating_sub(d.offset)).max(0);
            (start, start.saturating_add(d.end - d.start))
        }
        Grip::Start => (snap(s, at).clamp(0, d.end - 1), d.end),
        Grip::End => (d.start, snap(s, at).max(d.start + 1)),
    }
}

pub(crate) fn lyrics_ruler(app: &mut SoundApp, ui: &mut Ui, painter: &egui::Painter, s: &Session, tl: Rect, row: Rect) {
    let t = Tokens::current();
    painter.rect_filled(row, 0.0, t.marker_ruler.gamma_multiply(0.85));
    let sel = s.edit.selection;
    let places: Vec<(LyricPlace, f32, f32)> = s
        .lyric_sheet
        .placed
        .iter()
        .map(|p| (*p, x_of(s, tl, p.start), x_of(s, tl, p.end)))
        .filter(|(_, a, b)| *b >= tl.min.x && *a <= tl.max.x)
        .collect();
    let drag: Option<Drag> = ui.ctx().data(|d| d.get_temp(drag_key()));
    let comp = soundcraft_engine::cmd::lyric_place::comp_track(s);
    let resp = ui.interact(row, ui.id().with("lyrics_ruler"), Sense::click_and_drag());
    let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
    let at = |x: f32| sample_at(s, tl, x).max(0);
    for (p, x0, x1) in &places {
        let (a, b) = match drag.filter(|d| d.line == p.line) {
            Some(d) => match pointer {
                Some(pt) => {
                    let (a, b) = dragged(s, d, at(pt.x));
                    (x_of(s, tl, a), x_of(s, tl, b))
                }
                None => (*x0, *x1),
            },
            None => (*x0, *x1),
        };
        let r = Rect::from_min_max(pos2(a, row.min.y + 3.0), pos2(b.max(a + 2.0), row.max.y - 3.0));
        let picked = sel.start == p.start && sel.end == p.end;
        painter.rect_filled(r, 2.0, if picked { PICKED } else { FILL });
        painter
            .line_segment([pos2(r.min.x + 1.0, r.min.y), pos2(r.min.x + 1.0, r.max.y)], Stroke::new(2.0, if picked { Color32::WHITE } else { EDGE }));
        painter.line_segment([pos2(r.max.x - 0.5, r.min.y), pos2(r.max.x - 0.5, r.max.y)], Stroke::new(1.0, EDGE.gamma_multiply(0.7)));
        let title = title(s, p.line);
        let inner = r.shrink2(vec2(6.0, 0.0));
        if inner.width() > 8.0 {
            let n_end = title.find("  ").unwrap_or(0);
            let (num, text) = title.split_at(n_end);
            let clip = painter.with_clip_rect(inner.intersect(row));
            let g = clip.layout_no_wrap(num.to_string(), regular(11.0), if picked { Color32::WHITE } else { EDGE });
            let w = g.size().x;
            clip.galley(pos2(inner.min.x, r.center().y - g.size().y / 2.0), g, EDGE);
            // With take layers on the vocal, the takes the comp plays over this line, at the right.
            let takes = comp.map(|t| soundcraft_engine::cmd::lyric_place::comp_takes(s, t, p.start, p.end)).unwrap_or_default();
            let mut text_right = inner.max.x;
            if !takes.is_empty() {
                let g = clip.layout_no_wrap(takes.join(" + "), regular(10.0), Color32::from_rgb(200, 190, 240));
                let tw = g.size().x + 10.0;
                if inner.width() > tw + w + 60.0 {
                    let pill = Rect::from_min_size(pos2(inner.max.x - tw, r.center().y - 8.0), vec2(tw, 16.0));
                    clip.rect_filled(pill, 8.0, Color32::from_black_alpha(90));
                    clip.galley(pos2(pill.min.x + 5.0, pill.center().y - g.size().y / 2.0), g, Color32::WHITE);
                    text_right = pill.min.x - 4.0;
                }
            }
            let clip = clip.with_clip_rect(Rect::from_min_max(inner.min, pos2(text_right, inner.max.y)).intersect(row));
            clip.text(pos2(inner.min.x + w + 6.0, r.center().y), Align2::LEFT_CENTER, text.trim(), regular(11.5), Color32::WHITE);
        }
    }
    let hit_at = |x: f32| hit(&places, x);
    // Hover: ends retime, the body moves.
    if let Some(pt) = resp.hover_pos()
        && drag.is_none()
    {
        match hit_at(pt.x) {
            Some((_, Grip::Start | Grip::End)) => ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal),
            Some((_, Grip::Move)) => ui.ctx().set_cursor_icon(egui::CursorIcon::Grab),
            None => {}
        }
    }
    if resp.drag_started()
        && let Some(pt) = resp.interact_pointer_pos()
        && let Some((line, grip)) = hit_at(pt.x)
        && let Some(p) = s.lyric_sheet.place_of(line)
    {
        let d = Drag { line, grip, offset: at(pt.x) - p.start, start: p.start, end: p.end };
        ui.ctx().data_mut(|m| m.insert_temp(drag_key(), d));
    }
    if let Some(d) = drag {
        ui.ctx().set_cursor_icon(if d.grip == Grip::Move { egui::CursorIcon::Grabbing } else { egui::CursorIcon::ResizeHorizontal });
        if !ui.ctx().input(|i| i.pointer.primary_down()) {
            ui.ctx().data_mut(|m| m.remove::<Drag>(drag_key()));
            if let Some(pt) = ui.ctx().pointer_latest_pos() {
                let (a, b) = dragged(s, d, at(pt.x));
                if (a, b) != (d.start, d.end) {
                    let _ = app.run("lyrics.place", json!({"line": d.line, "start": a, "end": b}));
                }
            }
        }
    }
    // A line dragged from the Lyrics tab lands where it is dropped.
    if let Some(payload) = egui::DragAndDrop::payload::<LineDrag>(ui.ctx())
        && let Some(pt) = ui.ctx().pointer_latest_pos()
        && row.contains(pt)
    {
        let x = x_of(s, tl, snap(s, at(pt.x)));
        painter.line_segment([pos2(x, row.min.y), pos2(x, row.max.y)], Stroke::new(2.0, EDGE));
        painter.text(pos2(x + 4.0, row.center().y), Align2::LEFT_CENTER, title(s, payload.0), bold(11.0), Color32::WHITE);
        if ui.ctx().input(|i| i.pointer.any_released()) {
            egui::DragAndDrop::clear_payload(ui.ctx());
            let _ = app.run("lyrics.place", json!({"line": payload.0, "start": snap(s, at(pt.x))}));
        }
    }
    let target = resp.interact_pointer_pos().and_then(|p| hit_at(p.x)).map(|(l, _)| l);
    if resp.double_clicked() {
        if let Some(line) = target {
            app.ui.show_lyrics = true;
            ui.ctx().data_mut(|m| m.insert_temp(jump_key(), line));
        }
    } else if resp.clicked() {
        match target {
            Some(line) => {
                let _ = app.run("lyrics.select_line", json!({"line": line}));
            }
            None => {
                let _ = app.run("transport.locate", json!({"at": snap(s, at(pointer.map_or(tl.min.x, |p| p.x)))}));
            }
        }
    }
    let menu_key = ui.id().with("lyrics_ruler_menu");
    if resp.secondary_clicked() {
        ui.ctx().data_mut(|m| m.insert_temp(menu_key, target));
    }
    let menu_line: Option<u64> = ui.ctx().data(|m| m.get_temp::<Option<u64>>(menu_key)).flatten();
    resp.context_menu(|ui| {
        if let Some(line) = menu_line {
            if ui.button("Select Line Range").clicked() {
                let _ = app.run("lyrics.select_line", json!({"line": line}));
                ui.close();
            }
            if ui.button("Show in Lyrics").clicked() {
                app.ui.show_lyrics = true;
                ui.ctx().data_mut(|m| m.insert_temp(jump_key(), line));
                ui.close();
            }
            if ui.button("Remove from Ruler").clicked() {
                let _ = app.run("lyrics.unplace", json!({"line": line}));
                ui.close();
            }
        } else if ui.button("Place Next Line Here (L)").clicked() {
            let _ = app.run("lyrics.place", json!({}));
            ui.close();
        }
        ui.separator();
        crate::menus::ruler_context_menu(app, ui, "lyrics");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: u64) -> LyricPlace {
        LyricPlace { line, start: 0, end: 1 }
    }

    #[test]
    fn hits_ends_then_bodies() {
        let places = [(p(1), 10.0, 100.0), (p(2), 100.0, 200.0)];
        assert_eq!(hit(&places, 12.0), Some((1, Grip::Start)));
        assert_eq!(hit(&places, 50.0), Some((1, Grip::Move)));
        // Where two meet, the later marker's start wins.
        assert_eq!(hit(&places, 101.0), Some((2, Grip::Start)));
        assert_eq!(hit(&places, 198.0), Some((2, Grip::End)));
        assert_eq!(hit(&places, 300.0), None);
    }

    #[test]
    fn drags_move_and_retime_without_snapping_off() {
        let s = Session::default();
        let d = Drag { line: 1, grip: Grip::Move, offset: 100, start: 1000, end: 2000 };
        assert_eq!(dragged(&s, d, 5100), (5000, 6000));
        assert_eq!(dragged(&s, d, 50), (0, 1000));
        assert_eq!(dragged(&s, Drag { grip: Grip::Start, ..d }, 1500), (1500, 2000));
        assert_eq!(dragged(&s, Drag { grip: Grip::Start, ..d }, 9000), (1999, 2000));
        assert_eq!(dragged(&s, Drag { grip: Grip::End, ..d }, 500), (1000, 1001));
    }
}
