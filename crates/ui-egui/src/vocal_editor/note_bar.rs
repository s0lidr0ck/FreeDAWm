//! The note bar: a floating strip under the selected note (the first selected when several;
//! it follows scrolling and zoom and hides when that note is off screen) with the fields
//! `cents`, `semi`, `drift`, `vibrato` and `level`. Drag a value sideways to change it
//! ([`BarField::per_px`] a pixel, Shift fine ×0.1), double-click to type (Enter sets, Esc
//! cancels; two clicks within 0.35 s). A gesture is one undo step (as the Tuning panel's sliders: `vocal.edit` merged at
//! most every [`DRAG_EVERY`] and on release); Esc mid-drag puts the values back. With several
//! notes selected the bar shows their mean and "N notes", and a drag moves every note by the
//! same delta (one value per note in one `vocal.edit`).

use super::roll::{Drawn, clip_views, selection_with_twins};
use super::{hook, store};
use crate::SoundApp;
use crate::slider_drag::{DRAG_EVERY, How, report};
use crate::theme::{Tokens, bold, regular};
use crate::vocal_style::fmt_cents;
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_engine::vocal::NoteRef;
use soundcraft_model::TrackId;
use soundcraft_vocal::{MAX_GAIN_DB, MAX_OFFSET_CENTS, NoteEdit};

/// A field's width (px).
pub const FIELD_W: f32 = 64.0;
/// The bar's height (px).
pub const BAR_H: f32 = 40.0;
/// The gap between the note bar and the note (px).
const GAP: f32 = 18.0;
/// The Shift fine factor.
const FINE: f32 = 0.1;
/// Two clicks on a field this close (seconds) open its type-in.
const DOUBLE_CLICK_SECS: f64 = 0.35;

/// One field of the note bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarField {
    Cents,
    Semi,
    Drift,
    Vibrato,
    Level,
}

/// The fields, left to right.
pub const FIELDS: [BarField; 5] = [BarField::Cents, BarField::Semi, BarField::Drift, BarField::Vibrato, BarField::Level];

impl BarField {
    pub fn key(self) -> &'static str {
        match self {
            BarField::Cents => "cents",
            BarField::Semi => "semi",
            BarField::Drift => "drift",
            BarField::Vibrato => "vibrato",
            BarField::Level => "level",
        }
    }

    fn label(self) -> &'static str {
        match self {
            BarField::Cents => "CENTS",
            BarField::Semi => "SEMI",
            BarField::Drift => "DRIFT",
            BarField::Vibrato => "VIBRATO",
            BarField::Level => "LEVEL",
        }
    }

    /// The change per pixel of drag, in the field's unit.
    pub fn per_px(self) -> f32 {
        match self {
            BarField::Cents => 1.0,
            BarField::Semi => 0.1,
            BarField::Drift | BarField::Vibrato => 1.0,
            BarField::Level => 0.1,
        }
    }

    /// The field's range in its unit.
    fn range(self) -> (f32, f32) {
        match self {
            BarField::Cents => (-MAX_OFFSET_CENTS, MAX_OFFSET_CENTS),
            BarField::Semi => (-MAX_OFFSET_CENTS / 100.0, MAX_OFFSET_CENTS / 100.0),
            BarField::Drift => (0.0, 100.0),
            BarField::Vibrato => (-100.0, 100.0),
            BarField::Level => (-MAX_GAIN_DB, MAX_GAIN_DB),
        }
    }

    /// A value's resolution: whole cents, semitones and %, 0.1 dB.
    fn quantise(self, v: f32) -> f32 {
        match self {
            BarField::Level => (v * 10.0).round() / 10.0,
            _ => v.round(),
        }
    }

    /// The value of `e` in the field's unit: cents, semitones (whole), %, %, dB.
    pub fn of(self, e: &NoteEdit) -> f32 {
        match self {
            BarField::Cents => e.offset_cents,
            BarField::Semi => (e.offset_cents / 100.0).round(),
            BarField::Drift => e.drift * 100.0,
            BarField::Vibrato => e.vibrato * 100.0,
            BarField::Level => e.gain_db,
        }
    }

    /// `v` clamped to the range and quantised.
    pub fn clamp(self, v: f32) -> f32 {
        let (lo, hi) = self.range();
        if v.is_finite() { self.quantise(v).clamp(lo, hi) } else { 0.0 }
    }

    /// The `vocal.edit` key and value that set `v` on a note.
    pub fn param(self, v: f32) -> (&'static str, serde_json::Value) {
        match self {
            BarField::Cents => ("offset", json!(v)),
            BarField::Semi => ("offset", json!(v * 100.0)),
            BarField::Drift => ("drift", json!(v)),
            BarField::Vibrato => ("vibrato", json!(v)),
            BarField::Level => ("gain_db", json!(v)),
        }
    }

    /// The value as the bar shows it: "−5c", "+2 st", "35%", "+10%", "−1.5 dB".
    pub fn text(self, v: f32) -> String {
        let v = self.quantise(if v.is_finite() { v } else { 0.0 });
        let sign = |x: f32| if x < 0.0 { "\u{2212}" } else { "+" };
        match self {
            BarField::Cents => fmt_cents(v),
            BarField::Semi => format!("{}{:.0} st", sign(v), v.abs()),
            BarField::Drift => format!("{v:.0}%"),
            BarField::Vibrato => {
                if v == 0.0 {
                    "0%".into()
                } else {
                    format!("{}{:.0}%", sign(v), v.abs())
                }
            }
            BarField::Level => {
                if v == 0.0 {
                    "0.0 dB".into()
                } else {
                    format!("{}{:.1} dB", sign(v), v.abs())
                }
            }
        }
    }

    /// A typed value: a number with or without the unit, "−" or "-" for minus.
    pub fn parse(self, text: &str) -> Option<f32> {
        let t: String = text.trim().replace('\u{2212}', "-").chars().filter(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.')).collect();
        t.parse::<f32>().ok().filter(|v| v.is_finite()).map(|v| self.clamp(v))
    }
}

/// One note-bar drag in progress.
#[derive(Debug, Clone, PartialEq)]
pub struct BarDrag {
    pub field: BarField,
    /// Where the button went down (x) and the pointer's last position.
    origin_x: f32,
    last_x: f32,
    /// Each note with its value when the drag began.
    base: Vec<(NoteRef, f32)>,
    /// The values the pointer asks for now, and those last sent.
    shown: Vec<f32>,
    sent: Vec<f32>,
    last_send: f64,
    ran: bool,
    undo_before: usize,
    /// The gesture changed something (for the preview on release).
    pub changed: bool,
}

impl BarDrag {
    fn base_values(&self) -> Vec<f32> {
        self.base.iter().map(|b| b.1).collect()
    }

    /// The mean of the values shown.
    pub fn mean(&self) -> f32 {
        mean(&self.shown)
    }
}

fn mean(v: &[f32]) -> f32 {
    if v.is_empty() { 0.0 } else { v.iter().sum::<f32>() / v.len() as f32 }
}

/// Each note's value for a drag of `delta` (the field's unit) from `base`: the same delta on
/// every note, clamped per note.
pub fn dragged_values(field: BarField, base: &[f32], delta: f32) -> Vec<f32> {
    base.iter().map(|b| field.clamp(b + delta)).collect()
}

/// The selected notes (with their twins) and their edits.
fn targets(app: &mut SoundApp, track: TrackId) -> Vec<(NoteRef, NoteEdit)> {
    let sel = selection_with_twins(app, track);
    let cvs = clip_views(app, track, None);
    let mut out = Vec::new();
    for r in sel {
        let Some(cv) = cvs.iter().find(|c| c.clip.id == r.clip && !c.stretched) else { continue };
        if !cv.a.notes.iter().any(|n| n.id == r.note) {
            continue;
        }
        out.push((r, cv.edits.get(&r.note).cloned().unwrap_or_default()));
    }
    out
}

/// Where the bar sits for the first selected note's shape: under it, inside the roll (above
/// it when there is no room below), or `None` when that note is off screen.
pub fn place(d: &Drawn, first: NoteRef, w: f32) -> Option<Rect> {
    let sh = d.shapes.iter().find(|s| s.r == first)?;
    let roll = d.view.rect;
    let x = sh.rect.left().clamp(roll.left(), (roll.right() - w).max(roll.left()));
    let below = sh.rect.bottom() + GAP;
    let y = if below + BAR_H <= roll.bottom() { below } else { (sh.rect.top() - GAP - BAR_H).max(roll.top()) };
    Some(Rect::from_min_size(pos2(x, y), vec2(w, BAR_H)))
}

/// Draws the bar and runs its gestures. Call after the roll's own interaction (the fields sit
/// above it) and before the keys (a field's Esc is its own).
pub fn show(app: &mut SoundApp, ui: &mut Ui, track: TrackId, d: &Drawn) {
    let Some(first) = app.vocal.selected.iter().next().copied() else {
        hide(app, ui);
        return;
    };
    let tg = targets(app, track);
    if tg.is_empty() {
        hide(app, ui);
        return;
    }
    let count = tg.len();
    let count_w = if count > 1 { 58.0 } else { 0.0 };
    let w = count_w + FIELD_W * FIELDS.len() as f32 + 8.0;
    let Some(bar) = place(d, first, w) else {
        hide(app, ui);
        return;
    };
    let t = Tokens::current();
    let p = ui.painter().clone();
    let dark = u32::from(t.panel_bg.r()) + u32::from(t.panel_bg.g()) + u32::from(t.panel_bg.b()) < 384;
    let fill = if dark { Color32::from_rgb(28, 29, 33).gamma_multiply(0.97) } else { Color32::from_rgb(246, 247, 250) };
    p.rect(bar, 5.0, fill, Stroke::new(1.0, if dark { Color32::from_rgb(70, 72, 80) } else { t.border }), StrokeKind::Inside);
    store(ui.ctx(), "note-bar", bar);
    let mut x = bar.left() + 4.0;
    if count > 1 {
        let r = Rect::from_min_size(pos2(x, bar.top()), vec2(count_w, BAR_H));
        let text = format!("{count} notes");
        p.text(pos2(r.center().x, r.center().y - 7.0), Align2::CENTER_CENTER, &text, bold(11.5), t.text);
        p.text(pos2(r.center().x, r.center().y + 8.0), Align2::CENTER_CENTER, "AVG", regular(9.5), t.text_dim);
        p.line_segment([pos2(r.right(), bar.top() + 6.0), pos2(r.right(), bar.bottom() - 6.0)], Stroke::new(1.0, t.border));
        hook(ui.ctx(), "bar-count", text);
        x = r.right();
    } else {
        hook(ui.ctx(), "bar-count", String::new());
    }
    let (esc, now) = ui.input(|i| (i.key_pressed(egui::Key::Escape), i.time));
    for (i, f) in FIELDS.into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(x, bar.top()), vec2(FIELD_W, BAR_H));
        x = r.right();
        if i + 1 < FIELDS.len() {
            p.line_segment([pos2(r.right(), bar.top() + 6.0), pos2(r.right(), bar.bottom() - 6.0)], Stroke::new(1.0, t.border));
        }
        store(ui.ctx(), &format!("bar-{}", f.key()), r);
        let resp = ui.interact(r, ui.id().with(("vocal-note-bar", f.key())), Sense::click_and_drag());
        let vals: Vec<f32> = tg.iter().map(|(_, e)| f.of(e)).collect();
        // Typing.
        // A second click on the same field within [`DOUBLE_CLICK_SECS`] (counted here: the
        // roll's own click handling is under the bar).
        if resp.clicked() {
            let double = app.vocal.bar_click.take().is_some_and(|(lf, at)| lf == f && now - at < DOUBLE_CLICK_SECS);
            if double {
                app.vocal.bar_drag = None;
                app.vocal.bar_edit = Some((f, f.text(mean(&vals)).replace(['c', '%'], "").trim().to_string()));
            } else {
                app.vocal.bar_click = Some((f, now));
            }
        }
        if app.vocal.bar_edit.as_ref().is_some_and(|(ef, _)| *ef == f) {
            type_in(app, ui, &tg, f, r, esc);
            label(&p, r, f, &t);
            continue;
        }
        // Dragging.
        if resp.drag_started() && app.vocal.bar_drag.is_none() {
            let origin_x = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos()).map_or(r.center().x, |o| o.x);
            app.engine.end_merge();
            app.vocal.bar_drag = Some(BarDrag {
                field: f,
                origin_x,
                last_x: origin_x,
                base: tg.iter().map(|(r, e)| (*r, f.of(e))).collect(),
                shown: vals.clone(),
                sent: vals.clone(),
                last_send: f64::NEG_INFINITY,
                ran: false,
                undo_before: 0,
                changed: false,
            });
        }
        let dragging = app.vocal.bar_drag.as_ref().is_some_and(|g| g.field == f);
        let shown = if dragging { update(app, ui, resp.drag_stopped(), esc) } else { None };
        let value = shown.unwrap_or_else(|| mean(&vals));
        let mixed = !dragging && vals.iter().any(|v| (v - value).abs() > 0.05);
        let text = f.text(value);
        p.text(pos2(r.center().x, r.center().y - 7.0), Align2::CENTER_CENTER, &text, bold(12.5), if mixed { t.text_dim } else { t.text });
        hook(ui.ctx(), &format!("bar-value-{}", f.key()), text);
        label(&p, r, f, &t);
        if resp.hovered() && !dragging {
            resp.on_hover_text(format!("Drag sideways: {} a pixel (Shift: fine) \u{b7} double-click to type", f.text(f.per_px())));
        }
    }
    if app.vocal.bar_drag.is_some() {
        ui.ctx().request_repaint();
    }
}

fn label(p: &egui::Painter, r: Rect, f: BarField, t: &Tokens) {
    p.text(pos2(r.center().x, r.center().y + 8.0), Align2::CENTER_CENTER, f.label(), regular(9.5), t.text_dim);
}

fn hide(app: &mut SoundApp, ui: &Ui) {
    ui.ctx().data_mut(|d| d.remove::<Rect>(egui::Id::new("soundcraft-vocal-note-bar")));
    app.vocal.bar_edit = None;
    if let Some(g) = app.vocal.bar_drag.take()
        && g.ran
    {
        restore(app, &g);
        app.engine.end_merge();
    }
}

/// The drag this frame: follows the pointer, sends when due and ends on release. Returns the
/// mean value to draw while it lasts.
fn update(app: &mut SoundApp, ui: &Ui, stopped: bool, esc: bool) -> Option<f32> {
    let mut g = app.vocal.bar_drag.take()?;
    if esc {
        if g.ran {
            restore(app, &g);
        }
        app.engine.end_merge();
        app.vocal.drag_cancelled = true;
        app.ui.status = "Change cancelled".into();
        return None;
    }
    let (pointer, fine, down, now) = ui.input(|i| (i.pointer.latest_pos().map(|p| p.x), i.modifiers.shift, i.pointer.primary_down(), i.time));
    let x = pointer.unwrap_or(g.last_x);
    g.last_x = x;
    let delta = (x - g.origin_x) * g.field.per_px() * if fine { FINE } else { 1.0 };
    g.shown = dragged_values(g.field, &g.base_values(), delta);
    let done = stopped || !down;
    let due = done || now - g.last_send >= DRAG_EVERY;
    if done && g.ran && g.shown == g.base_values() {
        restore(app, &g);
        g.changed = false;
    } else if due && g.shown != g.sent {
        let how = if !done {
            How::Merged
        } else if g.ran {
            How::Joined
        } else {
            How::Single
        };
        let before = app.engine.undo_history().len();
        let r = send(app, how, &g.base, &g.shown, g.field);
        if r.is_ok() {
            if !g.ran {
                g.undo_before = before;
            }
            g.sent = g.shown.clone();
            g.ran = true;
            g.changed = true;
            g.last_send = now;
            app.ui.status = format!(
                "{} {} on {} note{}",
                g.field.label().to_lowercase(),
                g.field.text(g.mean()),
                g.base.len(),
                if g.base.len() == 1 { "" } else { "s" }
            );
        }
        report(app, r);
    }
    let shown = g.mean();
    if done {
        app.engine.end_merge();
        if g.changed {
            super::preview::changed(app);
        }
        return Some(shown);
    }
    app.vocal.bar_drag = Some(g);
    Some(shown)
}

/// Sends one `vocal.edit` with a value per note.
fn send(app: &mut SoundApp, how: How, base: &[(NoteRef, f32)], values: &[f32], field: BarField) -> Result<serde_json::Value, String> {
    let notes: Vec<serde_json::Value> = base
        .iter()
        .zip(values)
        .map(|((r, _), v)| {
            let (k, v) = field.param(*v);
            let mut n = json!({"clip": r.clip, "note": r.note});
            n[k] = v;
            n
        })
        .collect();
    how.run(app, crate::vocal_ids::VOCAL_EDIT, &json!({"notes": notes}), "vocal-note-bar")
}

/// Puts the values back after a gesture sent something (as `drag::restore`).
fn restore(app: &mut SoundApp, g: &BarDrag) {
    if app.engine.undo_history().len() == g.undo_before.saturating_add(1) && app.engine.undo_discard() {
        return;
    }
    let r = send(app, How::Joined, &g.base, &g.base_values(), g.field);
    report(app, r);
}

/// The type-in field in place of a value: Enter sets every selected note to the number, Esc
/// (or losing focus) cancels.
fn type_in(app: &mut SoundApp, ui: &mut Ui, tg: &[(NoteRef, NoteEdit)], f: BarField, r: Rect, esc: bool) {
    let Some((_, mut text)) = app.vocal.bar_edit.take() else { return };
    // Read before the field, which takes Enter for itself.
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let field_r = Rect::from_min_size(pos2(r.left() + 4.0, r.top() + 4.0), vec2(r.width() - 8.0, 18.0));
    let id = egui::Id::new(("soundcraft-vocal-bar-edit", f.key()));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field_r));
    let resp = child.add_sized(field_r.size(), egui::TextEdit::singleline(&mut text).id(id).font(bold(12.0)));
    store(ui.ctx(), "bar-edit", field_r);
    if !resp.has_focus() && !resp.lost_focus() {
        // Opening: the whole value selected, so typing replaces it.
        let mut st = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
        st.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(text.chars().count()))));
        st.store(ui.ctx(), id);
        resp.request_focus();
    }
    if esc {
        // The field's Esc: not the editor's (it would clear the selection).
        app.vocal.drag_cancelled = true;
        app.ui.status = "Change cancelled".into();
        return;
    }
    if enter {
        match f.parse(&text) {
            Some(v) => {
                let base: Vec<(NoteRef, f32)> = tg.iter().map(|(r, e)| (*r, f.of(e))).collect();
                let values = vec![v; base.len()];
                let r = send(app, How::Single, &base, &values, f);
                if r.is_ok() {
                    app.ui.status =
                        format!("{} {} on {} note{}", f.label().to_lowercase(), f.text(v), base.len(), if base.len() == 1 { "" } else { "s" });
                    super::preview::changed(app);
                }
                report(app, r);
            }
            None => app.ui.status = format!("Type a number for {}", f.label().to_lowercase()),
        }
        return;
    }
    if resp.lost_focus() {
        return;
    }
    app.vocal.bar_edit = Some((f, text));
}
