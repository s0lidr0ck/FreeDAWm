//! Dragging notes up and down in the roll (Melodyne-style pitch moves): a drag on a note moves it,
//! or the selection when the note is part of it, by setting each note's manual offset
//! (`vocal.edit` with per-note `offset`s). The move is in cents, [`CENTS_PER_ROW`] per row of
//! pointer travel; a note whose tuned centre comes within [`SNAP_CENTS`] of a semitone snaps onto
//! it (magnetic: a long drag sticks briefly at each semitone, with a tick on the key column while
//! it holds); Alt turns the snap off. A 4 px dead zone keeps a click from nudging pitch.
//! The notes redraw at the pointer's pitch every frame, the command runs at most every
//! [`DRAG_EVERY`] (merged into one undo step, as the panel's sliders) and on release. A drag that
//! ends back where it began, or Esc mid-drag, leaves the notes as they were and no undo step.
//! Alt+Up/Down moves the selection a semitone.

use super::roll::{Drawn, click_select, clip_views, selection_with_twins};
use super::{hook, store};
use crate::SoundApp;
use crate::slider_drag::{DRAG_EVERY, How, report};
use crate::theme::{Tokens, bold};
use crate::vocal_style::{fmt_cents, note_name};
use egui::{Pos2, Rect, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_engine::vocal::NoteRef;
use soundcraft_model::TrackId;
use soundcraft_vocal::{MAX_OFFSET_CENTS, NoteEdit, tuned_centre_with};
use std::collections::BTreeSet;

/// How far (px) the pointer moves up or down before a drag changes pitch.
pub const DEAD_ZONE_PX: f32 = 4.0;
/// Cents per row height of pointer travel (half a semitone a row: a slow drag).
pub const CENTS_PER_ROW: f32 = 50.0;
/// A note whose tuned centre comes this close (cents) to a semitone snaps onto it.
pub const SNAP_CENTS: f32 = 15.0;
/// Alt+Up/Down's step (cents).
pub const KEY_STEP: f32 = 100.0;

/// One note drag in progress.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteDrag {
    pub track: TrackId,
    /// Where the button went down.
    origin: Pos2,
    /// Where the pointer was last seen (kept while it is off the window).
    last: Pos2,
    /// The note under the pointer (the readout follows it).
    grabbed: NoteRef,
    /// Each dragged note with its offset and tuned centre when the drag began.
    base: Vec<(NoteRef, f32, f32)>,
    /// The offsets the pointer asks for now.
    shown: Vec<f32>,
    /// The offsets last sent (what the curves show).
    sent: Vec<f32>,
    /// When the command last ran (egui time), and whether it ran this gesture.
    last_send: f64,
    ran: bool,
    /// The undo history's length before the gesture's step was pushed.
    undo_before: usize,
    /// The pointer left the dead zone.
    moved: bool,
    /// "−5c · A4": the grabbed note's tuned centre against its nearest semitone.
    pub readout: String,
    /// The semitone (cents) the grabbed note is snapped onto this frame.
    pub snapped: Option<f32>,
}

impl NoteDrag {
    /// How far (cents) note `r` is drawn from where its curves put it: the move not sent yet.
    pub fn preview_cents(&self, r: NoteRef) -> Option<f32> {
        let i = self.base.iter().position(|(x, _, _)| *x == r)?;
        let d = self.shown.get(i)? - self.sent.get(i)?;
        (d != 0.0).then_some(d)
    }

    /// The notes being dragged.
    pub fn notes(&self) -> impl Iterator<Item = NoteRef> + '_ {
        self.base.iter().map(|(r, _, _)| *r)
    }

    /// Each note's offset when the drag began.
    fn base_offsets(&self) -> Vec<f32> {
        self.base.iter().map(|b| b.1).collect()
    }
}

/// The semitone (cents) a tuned centre of `tc` moved by `d_cents` snaps onto: the nearest one
/// when it is within [`SNAP_CENTS`], else `None`.
pub fn snap_target(tc: f32, d_cents: f32) -> Option<f32> {
    let tcn = tc + d_cents.round();
    if !tcn.is_finite() {
        return None;
    }
    let near = (tcn / 100.0).round() * 100.0;
    ((tcn - near).abs() <= SNAP_CENTS).then_some(near)
}

/// Each note's new offset for a pointer move of `d_cents` (up positive, rounded to the cent):
/// `base` holds each note's (offset, tuned centre) when the drag began. With `snap`, a note
/// whose moved tuned centre comes within [`SNAP_CENTS`] of a semitone lands on it exactly.
/// Offsets stay within ±2400 c.
pub fn drag_offsets(base: &[(f32, f32)], d_cents: f32, snap: bool) -> Vec<f32> {
    let d = if d_cents.is_finite() { d_cents.round() } else { 0.0 };
    base.iter()
        .map(|&(off, tc)| {
            let mut moved = off + d;
            if snap && let Some(near) = snap_target(tc, d) {
                moved += near - (tc + d);
            }
            moved.clamp(-MAX_OFFSET_CENTS, MAX_OFFSET_CENTS)
        })
        .collect()
}

/// The readout for a tuned centre of `cents`: its offset from the nearest semitone and that
/// semitone's name, "−5c · A4".
pub fn readout_text(cents: f32) -> String {
    let near = if cents.is_finite() { (cents / 100.0).round() * 100.0 } else { cents };
    format!("{} \u{b7} {}", fmt_cents(cents - near), note_name(cents))
}

/// Starts dragging note `r` (pressed at `origin`): the selection when `r` is in it, else `r` alone
/// (added to the selection with `toggle`). The notes come from every clip of the track, as the
/// panel's sliders do: a twin in a clip scrolled out of view moves too.
pub fn start(app: &mut SoundApp, track: TrackId, r: NoteRef, origin: Pos2, toggle: bool) {
    if !app.vocal.selected.contains(&r) {
        click_select(app, track, Some(r), toggle);
    }
    let refs: BTreeSet<NoteRef> = selection_with_twins(app, track);
    let clips = clip_views(app, track, None);
    let mut base = Vec::new();
    for x in &refs {
        let Some(cv) = clips.iter().find(|c| c.clip.id == x.clip && !c.stretched) else { continue };
        let Some(n) = cv.a.notes.iter().find(|n| n.id == x.note) else { continue };
        let e = cv.edits.get(&n.id).cloned().unwrap_or_default();
        let mode = soundcraft_engine::vocal::note_mode(&cv.clip);
        base.push((*x, e.clone().clamped().offset_cents, tuned_centre_with(&cv.a, n, &e, &cv.scale, mode)));
    }
    if base.is_empty() {
        return;
    }
    let offs: Vec<f32> = base.iter().map(|b| b.1).collect();
    // A new undo step.
    app.engine.end_merge();
    app.vocal.drag = Some(NoteDrag {
        track,
        origin,
        last: origin,
        grabbed: r,
        base,
        shown: offs.clone(),
        sent: offs,
        last_send: f64::NEG_INFINITY,
        ran: false,
        undo_before: 0,
        moved: false,
        readout: String::new(),
        snapped: None,
    });
}

/// The drag this frame: follows the pointer, sends when due and ends on release. `stopped` is the
/// roll response's `drag_stopped`.
pub fn update(app: &mut SoundApp, ui: &Ui, d: &Drawn, stopped: bool) {
    let Some(mut g) = app.vocal.drag.take() else { return };
    let (pointer, alt, down, now, esc) =
        ui.input(|i| (i.pointer.latest_pos(), i.modifiers.alt, i.pointer.primary_down(), i.time, i.key_pressed(egui::Key::Escape)));
    let snap = !alt;
    if esc {
        // egui ends the drag on Esc; the move is cancelled rather than committed.
        cancel(app, ui.ctx(), &g);
        return;
    }
    // Off the window the pointer keeps its last position: the notes stay where they are.
    let p = pointer.unwrap_or(g.last);
    g.last = p;
    let dy = p.y - g.origin.y;
    g.moved |= dy.abs() >= DEAD_ZONE_PX;
    let d_cents = if g.moved { -dy / d.view.row_h.max(1.0) * CENTS_PER_ROW } else { 0.0 };
    let base: Vec<(f32, f32)> = g.base.iter().map(|b| (b.1, b.2)).collect();
    g.shown = if g.moved { drag_offsets(&base, d_cents, snap) } else { g.base_offsets() };
    // The grabbed note's tuned centre where the pointer has it.
    let gi = g.base.iter().position(|b| b.0 == g.grabbed).unwrap_or(0);
    let tc = g.base.get(gi).map_or(0.0, |b| b.2 + g.shown.get(gi).copied().unwrap_or(b.1) - b.1);
    g.readout = if g.moved { readout_text(tc) } else { String::new() };
    g.snapped = if g.moved && snap { snap_target(tc, 0.0) } else { None };
    let done = stopped || !down;
    let due = done || now - g.last_send >= DRAG_EVERY;
    if done && g.ran && g.shown == g.base_offsets() {
        // Released where it began: the notes go back and the gesture leaves no undo step.
        restore(app, &g);
    } else if due && g.shown != g.sent {
        let how = if !done {
            How::Merged
        } else if g.ran {
            How::Joined
        } else {
            How::Single
        };
        let notes: Vec<_> = g.base.iter().zip(&g.shown).map(|((r, _, _), o)| json!({"clip": r.clip, "note": r.note, "offset": o})).collect();
        let before = app.engine.undo_history().len();
        let r = how.run(app, crate::vocal_ids::VOCAL_EDIT, &json!({"notes": notes}), &format!("vocal-drag:{}", g.track.0));
        if r.is_ok() {
            if !g.ran {
                g.undo_before = before;
            }
            g.sent = g.shown.clone();
            g.ran = true;
            g.last_send = now;
            let n = g.base.len();
            app.ui.status = format!("Moved {n} note{} {}", if n == 1 { "" } else { "s" }, g.readout);
        }
        report(app, r);
    }
    hook(ui.ctx(), "drag-readout", g.readout.clone());
    hook(ui.ctx(), "drag-snapped", g.snapped);
    if done {
        app.engine.end_merge();
        if g.ran && g.shown != g.base_offsets() {
            super::preview::request(app, g.grabbed);
        }
        return;
    }
    if !g.readout.is_empty() {
        draw_readout(ui, p, &g.readout);
    }
    if let Some(near) = g.snapped {
        draw_snap_tick(ui, &d.view, near);
    }
    ui.ctx().request_repaint();
    app.vocal.drag = Some(g);
}

/// Esc mid-drag: the notes go back to where the drag found them, the gesture leaves no undo
/// step, and the editor's Esc handling leaves the selection alone.
fn cancel(app: &mut SoundApp, ctx: &egui::Context, g: &NoteDrag) {
    if g.ran {
        restore(app, g);
    }
    app.engine.end_merge();
    app.vocal.drag_cancelled = true;
    hook(ctx, "drag-readout", String::new());
    hook(ctx, "drag-snapped", None::<f32>);
    app.ui.status = "Move cancelled".into();
}

/// Puts the dragged notes back at their pre-drag offsets after the gesture sent something: its
/// one undo step still holds the document from before the drag, so dropping it restores every
/// note at once; should anything else have pushed a step meanwhile, the base offsets are sent
/// joined onto the gesture's step instead.
fn restore(app: &mut SoundApp, g: &NoteDrag) {
    if app.engine.undo_history().len() == g.undo_before.saturating_add(1) && app.engine.undo_discard() {
        return;
    }
    let notes: Vec<_> = g.base.iter().map(|(r, o, _)| json!({"clip": r.clip, "note": r.note, "offset": o})).collect();
    let r = How::Joined.run(app, crate::vocal_ids::VOCAL_EDIT, &json!({"notes": notes}), "");
    report(app, r);
}

/// The tick on the key column at the semitone the grabbed note is snapped onto.
fn draw_snap_tick(ui: &Ui, view: &super::layout::RollView, cents: f32) {
    let t = Tokens::current();
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("soundcraft-vocal-drag-snap")));
    let y = view.y(cents);
    let (x0, x1) = (view.rect.left() - super::roll::KEY_W + 3.0, view.rect.left() - 3.0);
    painter.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(2.0, t.accent));
    painter.add(egui::Shape::convex_polygon(vec![pos2(x1, y), pos2(x1 - 6.0, y - 4.0), pos2(x1 - 6.0, y + 4.0)], t.accent, Stroke::NONE));
}

/// The move's readout in a small box by the pointer.
fn draw_readout(ui: &Ui, p: Pos2, text: &str) {
    let t = Tokens::current();
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("soundcraft-vocal-drag-readout")));
    let g = painter.layout_no_wrap(text.to_string(), bold(12.0), t.text);
    let r = Rect::from_min_size(p + vec2(14.0, -g.size().y - 12.0), g.size() + vec2(12.0, 6.0));
    painter.rect(r, 3.0, t.panel_bg2, Stroke::new(1.0, t.border), StrokeKind::Inside);
    painter.galley(r.min + vec2(6.0, 3.0), g, t.text);
    store(ui.ctx(), "drag-readout-rect", r);
}

/// Alt+Up/Down (a semitone) on the selection, one undo step each. Alt+Shift+Up/Down stays the
/// app's (Copy Selection to Target/New Playlist); finer moves are what a drag is for. The caller
/// checks the pointer is over the editor and no text field has focus.
pub fn keys(app: &mut SoundApp, ui: &Ui, track: TrackId) {
    use egui::Key;
    let steps: Vec<f32> = ui.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key: k @ (Key::ArrowUp | Key::ArrowDown), pressed: true, modifiers: m, .. }
                    if m.alt && !m.shift && !m.ctrl && !m.command =>
                {
                    Some(if *k == Key::ArrowUp { KEY_STEP } else { -KEY_STEP })
                }
                _ => None,
            })
            .collect()
    });
    if steps.is_empty() || app.vocal.selected.is_empty() {
        return;
    }
    let refs: Vec<NoteRef> = selection_with_twins(app, track).into_iter().collect();
    if refs.is_empty() {
        return;
    }
    let mut moved = false;
    for by in steps {
        let r = app.run(crate::vocal_ids::VOCAL_EDIT, json!({"notes": refs, "offset_by": by}));
        if r.is_ok() {
            let n = refs.len();
            app.ui.status = format!("Moved {n} note{} {}", if n == 1 { "" } else { "s" }, crate::vocal_style::fmt_cents(by));
            moved = true;
        }
        report(app, r);
    }
    if moved {
        super::preview::changed(app);
    }
}

/// The selection's manual offsets for the panel: "moved +200c" (or the mean when they differ),
/// `None` when no selected note is moved.
pub fn moved_text(edits: &[NoteEdit]) -> Option<String> {
    let offs: Vec<f32> = edits.iter().map(|e| e.offset_cents).collect();
    if offs.iter().all(|o| *o == 0.0) {
        return None;
    }
    let mean = offs.iter().sum::<f32>() / offs.len().max(1) as f32;
    let mixed = offs.iter().any(|o| (o - mean).abs() > 0.5);
    let c = crate::vocal_style::fmt_cents(mean);
    Some(if mixed { format!("moved mixed (avg {c})") } else { format!("moved {c}") })
}
