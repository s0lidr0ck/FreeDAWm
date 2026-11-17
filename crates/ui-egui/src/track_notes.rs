//! Notes edited directly on a MIDI or instrument track showing Notes (the default view of such
//! tracks), without opening the MIDI Editor. The track's clip is a small piano roll: its lowest to
//! highest note fill the clip's height. With the Smart, Grabber or Pencil tool a drag on a note
//! moves it in time and pitch (the editor's selected notes with it when it is one of them), a drag
//! on its right end resizes it, a click selects it in the editor, Alt-click deletes it, and the
//! Pencil clicked on empty space in a MIDI clip adds a note there. Every edit is one of the
//! editor's commands (`midi.notes_move`, `midi.notes_resize`, `midi.note_add`,
//! `midi.note_delete`), so it is one undo step and scriptable. Moves snap to the editor's grid
//! counted from the bar (Alt: free), as the editor's do. Other views (Clips) leave the notes
//! alone, so a clip can still be dragged by its notes there.

use crate::SoundApp;
use crate::edit_window::{sample_at, x_of};
use egui::{Color32, Pos2, Rect, Response, Stroke, StrokeKind, Ui, pos2};
use serde_json::json;
use soundcraft_midi::Sequence;
use soundcraft_model::{Clip, ClipContent, ClipId, Session, Tool, Track};

/// A note dragged on the track.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackNoteDrag {
    pub clip: ClipId,
    pub index: usize,
    /// The right end is dragged (a resize), not the note.
    pub end: bool,
    pub from: Pos2,
    /// Screen points per semitone when the drag began.
    pub row: f32,
}

/// The pitch range a clip's notes are drawn over: two semitones past its lowest and highest
/// notes (C3 to C5 when it is empty).
pub fn pitch_span(seq: &Sequence) -> (u8, u8) {
    let (lo, hi) = seq.notes.iter().filter(|n| n.start >= 0).fold((127u8, 0u8), |(lo, hi), n| (lo.min(n.pitch), hi.max(n.pitch)));
    if lo > hi { (48, 72) } else { (lo.saturating_sub(2), hi.saturating_add(2)) }
}

/// Screen points per semitone in `body`.
pub fn row_height(seq: &Sequence, body: Rect) -> f32 {
    let (lo, hi) = pitch_span(seq);
    (body.height() - 6.0).max(1.0) / f32::from(hi.saturating_sub(lo).max(1))
}

/// Where each visible note of a MIDI clip is drawn in `body` (the clip's rectangle under its
/// name bar): index in the clip's notes, rectangle.
pub fn note_rects(s: &Session, clip: &Clip, tl: Rect, body: Rect) -> Vec<(usize, Rect)> {
    let ClipContent::Midi { sequence } = &clip.content else { return Vec::new() };
    let (lo, hi) = pitch_span(sequence);
    let span = f32::from(hi.saturating_sub(lo).max(1));
    let base = s.tempo.samples_to_ticks(clip.start, s.sample_rate);
    let nh = ((body.height() - 6.0) / span).clamp(1.5, 6.0);
    let mut out = Vec::new();
    for (i, n) in sequence.notes.iter().enumerate().filter(|(_, n)| n.start >= 0) {
        let a = s.tempo.tick_to_samples(base.saturating_add(n.start), s.sample_rate);
        if a >= clip.end() {
            continue;
        }
        let b = s.tempo.tick_to_samples(base.saturating_add(n.end()), s.sample_rate).min(clip.end());
        let x0 = x_of(s, tl, a);
        let x1 = x_of(s, tl, b).max(x0 + 2.0);
        let y = body.max.y - 3.0 - (f32::from(n.pitch.saturating_sub(lo)) / span) * (body.height() - 6.0);
        out.push((i, Rect::from_min_max(pos2(x0, y - nh * 0.5), pos2(x1, y + nh * 0.5))));
    }
    out
}

/// The clip's body on the track: its rectangle in `lane` under its name bar.
pub fn body_rect(s: &Session, clip: &Clip, tl: Rect, lane: Rect) -> Rect {
    let x0 = x_of(s, tl, clip.start);
    let x1 = x_of(s, tl, clip.end()).max(x0 + 1.0);
    let name_h = crate::edit_window::clip_name_height(s, lane);
    Rect::from_min_max(pos2(x0, lane.min.y + 1.0 + name_h), pos2(x1, lane.max.y - 1.0))
}

/// Whether `track` edits its notes on the timeline with `tool`.
pub fn edits_notes(track: &Track, tool: Tool) -> bool {
    track.kind.is_midi() && track.view == "notes" && matches!(tool, Tool::Smart | Tool::Grabber | Tool::Pencil)
}

/// The note under `p`: its clip, index, and whether `p` is on its right end. The topmost (last
/// drawn) note wins; a note is grabbed within 2 points above and below.
pub fn note_at(s: &Session, track: &Track, tl: Rect, lane: Rect, p: Pos2) -> Option<(ClipId, usize, bool)> {
    for clip in track.clips().iter().rev() {
        let body = body_rect(s, clip, tl, lane);
        if !body.contains(p) {
            continue;
        }
        let hit = note_rects(s, clip, tl, body).into_iter().rev().find(|(_, r)| r.expand2(egui::vec2(0.0, 2.0)).contains(p));
        if let Some((i, r)) = hit {
            let end = r.width() >= 9.0 && p.x >= r.max.x - 4.0;
            return Some((clip.id, i, end));
        }
        return None;
    }
    None
}

fn snap_ticks(app: &SoundApp) -> i64 {
    app.midi.grid.ticks(app.engine.session().edit.grid)
}

/// Handles the lane's pointer for note editing; returns whether it took the gesture (the rest of
/// the lane's handling is then skipped).
pub fn interact(app: &mut SoundApp, ui: &mut Ui, resp: &Response, track: &Track, lane: Rect, tl: Rect) -> bool {
    let s = app.engine.session_arc();
    let tool = s.edit.tool;
    if !edits_notes(track, tool) {
        app.midi.track_drag = None;
        return false;
    }
    let alt = ui.input(|i| i.modifiers.alt);
    // A drag under way: follow it with an outline, commit on release.
    if let Some(d) = app.midi.track_drag.clone() {
        if resp.dragged() || resp.drag_stopped() {
            let p = resp.interact_pointer_pos().or_else(|| ui.ctx().pointer_latest_pos()).unwrap_or(d.from);
            let Some((_, clip)) = s.find_clip(d.clip) else {
                app.midi.track_drag = None;
                return true;
            };
            let Some((dt, dp)) = drag_delta(app, &s, clip, &d, tl, p, alt) else {
                app.midi.track_drag = None;
                return true;
            };
            if resp.drag_stopped() {
                app.midi.track_drag = None;
                commit(app, &d, dt, dp);
            } else {
                let picked = dragged(app, &d);
                outline(ui, &s, clip, &d, &picked, tl, lane, (dt, dp));
            }
            return true;
        }
        app.midi.track_drag = None;
    }
    let Some(p) = resp.interact_pointer_pos().or_else(|| resp.hover_pos()) else { return false };
    let origin = if resp.drag_started() { ui.input(|i| i.pointer.press_origin()).unwrap_or(p) } else { p };
    match note_at(&s, track, tl, lane, origin) {
        Some((clip, index, end)) => {
            ui.ctx().set_cursor_icon(if end { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::Grab });
            if resp.double_clicked() {
                return false;
            }
            if resp.clicked() {
                if alt {
                    let _ = app.run("midi.note_delete", json!({"clip": clip.0, "indices": [index]}));
                    app.midi.selected.retain(|i| *i != index);
                } else {
                    select(app, clip, index, ui.input(|i| i.modifiers.shift));
                }
                return true;
            }
            if resp.drag_started() {
                let row = s.find_clip(clip).map_or(1.0, |(_, c)| match &c.content {
                    ClipContent::Midi { sequence } => row_height(sequence, body_rect(&s, c, tl, lane)),
                    _ => 1.0,
                });
                if !(app.midi.clip == Some(clip) && app.midi.selected.contains(&index)) {
                    select(app, clip, index, false);
                }
                app.midi.track_drag = Some(TrackNoteDrag { clip, index, end, from: origin, row });
                return true;
            }
            false
        }
        None if tool == Tool::Pencil && resp.clicked() && !resp.double_clicked() => {
            let at = sample_at(&s, tl, p.x).max(0);
            let Some(clip) = track.clips().iter().rev().find(|c| c.start <= at && at < c.end() && body_rect(&s, c, tl, lane).contains(p)) else {
                return false;
            };
            let ClipContent::Midi { sequence } = &clip.content else { return false };
            let body = body_rect(&s, clip, tl, lane);
            let (lo, _) = pitch_span(sequence);
            let row = row_height(sequence, body);
            let pitch = (f32::from(lo) + (body.max.y - 3.0 - p.y) / row).round().clamp(0.0, 127.0) as i64;
            let base = s.tempo.samples_to_ticks(clip.start, s.sample_rate);
            let tick = s.tempo.samples_to_ticks(at, s.sample_rate);
            let start = if alt { tick } else { crate::midi_edit_tools::snap_abs(&s.tempo, tick as f64, snap_ticks(app)) }.saturating_sub(base).max(0);
            let length = app.midi.last_length.unwrap_or_else(|| app.midi.grid.step(s.edit.grid)).max(1);
            let velocity = app.midi.last_velocity.clamp(1, 127);
            let added = app
                .run("midi.note_add", json!({"clip": clip.id.0, "pitch": pitch, "start_ticks": start, "length_ticks": length, "velocity": velocity}));
            if let Some(i) = added.ok().and_then(|r| r.get("index").and_then(serde_json::Value::as_u64)) {
                select(app, clip.id, i as usize, false);
            }
            true
        }
        None => false,
    }
}

/// Selects a note (Shift: adds it) and makes its clip the editor's.
fn select(app: &mut SoundApp, clip: ClipId, index: usize, add: bool) {
    if app.midi.clip != Some(clip) || !add {
        app.midi.selected.clear();
    }
    app.midi.clip = Some(clip);
    app.midi.active = Some(clip);
    if !app.midi.selected.contains(&index) {
        app.midi.selected.push(index);
    }
    if !app.engine.session().edit.selected_clips.contains(&clip) {
        let _ = app.run("edit.select", json!({"clips": [clip.0]}));
    }
}

/// The drag's change: ticks (snapped to the grid from the bar unless Alt) and semitones (0 for a
/// resize).
fn drag_delta(app: &SoundApp, s: &Session, clip: &Clip, d: &TrackNoteDrag, tl: Rect, p: Pos2, alt: bool) -> Option<(i64, i64)> {
    let ClipContent::Midi { sequence } = &clip.content else { return None };
    let n = sequence.notes.get(d.index)?;
    let base = s.tempo.samples_to_ticks(clip.start, s.sample_rate);
    let tick_of = |x: f32| s.tempo.samples_to_ticks(sample_at(s, tl, x).max(0), s.sample_rate);
    let raw = tick_of(p.x).saturating_sub(tick_of(d.from.x));
    let anchor = base.saturating_add(if d.end { n.end() } else { n.start });
    let target = anchor.saturating_add(raw);
    let snapped = if alt { target } else { crate::midi_edit_tools::snap_abs(&s.tempo, target as f64, snap_ticks(app)) };
    let dt = snapped.saturating_sub(anchor);
    let dp = if d.end { 0 } else { (-(p.y - d.from.y) / d.row.max(0.5)).round() as i64 };
    Some((dt, dp))
}

/// The notes a drag on note `d.index` takes: the editor's selection when it holds that note.
fn dragged(app: &SoundApp, d: &TrackNoteDrag) -> Vec<usize> {
    if app.midi.clip == Some(d.clip) && app.midi.selected.contains(&d.index) { app.midi.selected.clone() } else { vec![d.index] }
}

fn commit(app: &mut SoundApp, d: &TrackNoteDrag, dt: i64, dp: i64) {
    if dt == 0 && dp == 0 {
        return;
    }
    let indices = dragged(app, d);
    let r = if d.end {
        app.run("midi.notes_resize", json!({"clip": d.clip.0, "indices": indices, "length": dt}))
    } else {
        app.run("midi.notes_move", json!({"clip": d.clip.0, "indices": indices, "ticks": dt, "semitones": dp}))
    };
    match r {
        Ok(v) => {
            if let Some(a) = v.get("indices").and_then(serde_json::Value::as_array) {
                app.midi.clip = Some(d.clip);
                app.midi.selected = a.iter().filter_map(serde_json::Value::as_u64).map(|i| i as usize).collect();
            }
        }
        Err(e) => app.ui.status = e,
    }
}

/// Outlines where the dragged notes will land.
#[allow(clippy::too_many_arguments)]
fn outline(ui: &Ui, s: &Session, clip: &Clip, d: &TrackNoteDrag, which: &[usize], tl: Rect, lane: Rect, (dt, dp): (i64, i64)) {
    let body = body_rect(s, clip, tl, lane);
    let picked = note_rects(s, clip, tl, body);
    let tick_px = {
        let base = s.tempo.samples_to_ticks(clip.start, s.sample_rate);
        let a = x_of(s, tl, s.tempo.tick_to_samples(base, s.sample_rate));
        let b = x_of(s, tl, s.tempo.tick_to_samples(base.saturating_add(dt), s.sample_rate));
        b - a
    };
    let p = ui.painter().with_clip_rect(lane);
    let color = Color32::from_rgb(250, 250, 250);
    for (_, r) in picked.into_iter().filter(|(i, _)| which.contains(i)) {
        let r = if d.end {
            Rect::from_min_max(r.min, pos2((r.max.x + tick_px).max(r.min.x + 2.0), r.max.y))
        } else {
            r.translate(egui::vec2(tick_px, -(dp as f32) * d.row))
        };
        p.rect_stroke(r, 1.0, Stroke::new(1.0, color), StrokeKind::Outside);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_midi::Note;

    fn n(pitch: u8, start: i64) -> Note {
        Note { probability: 100, pitch, velocity: 100, muted: false, release_velocity: 64, channel: 0, start, length: 480 }
    }

    #[test]
    fn the_pitch_span_keeps_two_rows_round_the_notes() {
        let seq = Sequence { notes: vec![n(60, 0), n(64, 480), n(20, -480)], ctrls: Vec::new() };
        assert_eq!(pitch_span(&seq), (58, 66), "a hidden note does not widen the span");
        assert_eq!(pitch_span(&Sequence::default()), (48, 72));
    }
}
