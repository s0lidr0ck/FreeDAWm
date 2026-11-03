//! MIDI Cleanup in the MIDI editor (Layout A): the Cleanup panel docked on the right of the piano
//! roll, the chord strip across the roll's top, and the dashed overlays that show what the cleanup
//! removes, cuts off and extends. The roll keeps drawing the untouched take; the overlays show the
//! cleaned notes over it until Commit.

use crate::SoundApp;
use crate::slider_drag::report;
use crate::theme::{Tokens, bold, regular};
use egui::{Align, Align2, Color32, Layout, Painter, Rect, Sense, Shape, Stroke, Ui, UiBuilder, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_midi::cleanup::{CleanupSettings, NoteEdit};
use soundcraft_model::{Clip, ClipContent, ClipId, Session};
use soundcraft_time::Samples;

/// The panel's width, right of the roll and the velocity lane.
pub const PANEL_W: f32 = 220.0;
/// The chord strip's height, across the top of the roll.
pub const CHORD_H: f32 = 14.0;
/// Removed notes and cut-off parts.
pub const REMOVED: Color32 = Color32::from_rgb(230, 80, 70);
/// Extensions.
pub const EXTENDED: Color32 = Color32::from_rgb(90, 200, 110);
const CHORD_TEXT: Color32 = Color32::from_rgb(224, 164, 58);

/// The cleanup's per-note edits to draw over the take: only on Cleaned, and only when the preview
/// was made from the clip's notes as they are (it always is after a command).
pub fn shown_edits(clip: &Clip) -> Option<&[NoteEdit]> {
    let m = clip.midi_cleanup.as_ref().filter(|m| !m.original)?;
    let p = m.preview.as_ref()?;
    let ClipContent::Midi { sequence } = &clip.content else { return None };
    (p.out.report.total == sequence.notes.len()).then_some(p.out.edits.as_slice())
}

/// The edit of note `index` (edits are sorted by index).
pub fn edit_of(edits: &[NoteEdit], index: usize) -> Option<&NoteEdit> {
    edits.binary_search_by_key(&index, |e| e.index).ok().and_then(|i| edits.get(i))
}

fn dashed_rect(painter: &Painter, r: Rect, color: Color32) {
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    painter.extend(Shape::dashed_line(&pts, Stroke::new(1.0, color), 4.0, 3.0));
}

/// The dashed parts over the roll's notes: a removed note's outline (its fill dimmed), the cut-off
/// part of a shortened note, the extension of a lengthened one. Nothing on Original.
pub fn overlays(painter: &Painter, clip: &Clip, x_of: impl Fn(i64) -> f32, y_of: impl Fn(i32) -> f32) {
    let (Some(edits), ClipContent::Midi { sequence }) = (shown_edits(clip), &clip.content) else { return };
    let view = painter.clip_rect();
    for e in edits {
        let Some(n) = sequence.notes.get(e.index) else { continue };
        let p = i32::from(n.pitch);
        let (top, row) = (y_of(p), (y_of(p) - y_of(p + 1)).abs());
        // Only what can be seen: no dashed outlines for edits off screen.
        let (left, right) = (x_of(n.start), x_of(e.old_end.max(e.new_end.unwrap_or(e.old_end))).max(x_of(n.start) + 3.0));
        if right < view.left() || left > view.right() || top + row < view.top() || top > view.bottom() {
            continue;
        }
        let span = |a: i64, b: i64| Rect::from_min_max(pos2(x_of(a), top + 1.0), pos2(x_of(b).max(x_of(a) + 3.0), top + row - 1.0));
        match e.new_end {
            None => {
                let r = span(n.start, e.old_end);
                painter.rect_filled(r, 2.0, Color32::from_black_alpha(150));
                dashed_rect(painter, r, REMOVED);
            }
            Some(end) if end < e.old_end => dashed_rect(painter, span(end, e.old_end), REMOVED),
            Some(end) if end > e.old_end => {
                let r = span(e.old_end, end);
                painter.rect_filled(r, 2.0, EXTENDED.gamma_multiply(0.15));
                dashed_rect(painter, r, EXTENDED);
            }
            Some(_) => {}
        }
    }
}

/// The band's chords over `clip` in `rect` (clip-relative ticks through `x_of`), each named and
/// clipped to its span.
pub fn chord_strip(painter: &Painter, s: &Session, clip: &Clip, rect: Rect, x_of: impl Fn(i64) -> f32) {
    chord_strip_range(painter, s, rect, clip.start, clip.end(), x_of);
}

/// The band's chords over session time `[t0, t1)` in `rect`, each named and clipped to its span;
/// `x_of` maps ticks from `t0`'s tick to screen x.
pub fn chord_strip_range(painter: &Painter, s: &Session, rect: Rect, t0: Samples, t1: Samples, x_of: impl Fn(i64) -> f32) {
    // Themed: the dark strip of the mockups, or the light theme's panel colours.
    let t = Tokens::current();
    let dark = u32::from(t.panel_bg.r()) + u32::from(t.panel_bg.g()) + u32::from(t.panel_bg.b()) < 384;
    let (bg, edge, tick, text) = if dark {
        (Color32::from_rgb(32, 35, 38), Color32::from_rgb(52, 56, 61), Color32::from_rgb(70, 74, 80), CHORD_TEXT)
    } else {
        (t.panel_bg2, t.border, t.border, Color32::from_rgb(150, 92, 10))
    };
    painter.rect_filled(rect, 0.0, bg);
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, edge));
    let base = s.tempo.samples_to_ticks(t0, s.sample_rate);
    let len = s.tempo.samples_to_ticks(t1, s.sample_rate).saturating_sub(base);
    let (key, scale) = (i32::from(s.band.key), i32::from(s.band.scale));
    for c in s.band.chords.iter().filter(|c| c.length_ticks > 0) {
        let start = c.tick.saturating_sub(base);
        let end = start.saturating_add(c.length_ticks);
        if start >= len || end <= 0 {
            continue;
        }
        let (x0, x1) = (x_of(start).max(rect.left()), x_of(end).min(rect.right()));
        if x1 <= x0 {
            continue;
        }
        let span = Rect::from_min_max(pos2(x0, rect.top()), pos2(x1, rect.bottom()));
        painter.line_segment([span.left_top(), span.left_bottom()], Stroke::new(1.0, tick));
        let name = soundcraft_band::chord_theory::name_for(i32::from(c.slot), key, scale, i32::from(c.bass));
        painter.with_clip_rect(span.shrink2(vec2(1.0, 0.0)).intersect(painter.clip_rect())).text(
            pos2(span.left() + 4.0, span.center().y),
            Align2::LEFT_CENTER,
            name,
            bold(10.0),
            text,
        );
    }
}

fn store<T: Clone + Send + Sync + 'static>(ui: &Ui, name: &str, v: T) {
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(format!("soundcraft-midi-cleanup-{name}")), v));
}

/// `midi.cleanup {clip, key: v}`, one undo step.
fn set(app: &mut SoundApp, cid: ClipId, key: &str, v: Value) {
    let mut p = json!({"clip": cid.0});
    p[key] = v;
    let r = app.run("midi.cleanup", p);
    report(app, r);
}

/// A 0-100 % slider; a drag is one undo step. Returns nothing: it acts through the engine.
#[allow(clippy::too_many_arguments)]
fn slider(app: &mut SoundApp, ui: &mut Ui, cid: ClipId, key: &str, name: &str, label: &str, value: u8, enabled: bool, active: bool) {
    let t = Tokens::current();
    let w = ui.available_width();
    let (both, _) = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover());
    let row = Rect::from_min_size(both.min, vec2(w, 14.0));
    let r = Rect::from_min_max(pos2(both.left(), row.bottom()), both.max);
    let resp = ui.interact(r, ui.id().with(("cleanup-slider", name)), if enabled { Sense::click_and_drag() } else { Sense::hover() });
    store(ui, name, r);
    let shown = if enabled { drive(app, ui, &resp, r, cid, key, name, value, active) } else { value };
    let text = if enabled { t.text } else { t.text_dim };
    ui.painter().text(row.left_center(), Align2::LEFT_CENTER, label, regular(11.0), text);
    ui.painter().text(row.right_center(), Align2::RIGHT_CENTER, format!("{shown}%"), regular(11.0), t.text_dim);
    let track = Rect::from_center_size(r.center(), vec2(r.width(), 6.0));
    let frac = f32::from(shown.min(100)) / 100.0;
    ui.painter().rect_filled(track, 3.0, t.fader_track);
    let fill = Rect::from_min_max(track.min, pos2(track.left() + track.width() * frac, track.bottom()));
    ui.painter().rect_filled(fill, 3.0, if enabled { t.accent } else { t.accent.gamma_multiply(0.35) });
    let kx = (track.left() + track.width() * frac).clamp(track.left() + 5.0, (track.right() - 5.0).max(track.left() + 5.0));
    ui.painter().rect_filled(Rect::from_center_size(pos2(kx, r.center().y), vec2(10.0, 13.0)), 3.0, if enabled { t.text } else { t.text_dim });
    if !enabled {
        resp.on_hover_text("Needs a chord track");
    }
}

/// A slider's gesture through [`crate::slider_drag::drive`]: `midi.cleanup` merged into one undo
/// step. Returns the value to draw. On a clip with no cleanup (`!active`) a click at the slider's
/// own value still runs it: that turns the defaults on.
#[allow(clippy::too_many_arguments)]
fn drive(app: &mut SoundApp, ui: &Ui, resp: &egui::Response, r: Rect, cid: ClipId, key: &str, name: &str, value: u8, active: bool) -> u8 {
    let id = egui::Id::new(("soundcraft-midi-cleanup-pending", name));
    crate::slider_drag::drive(
        app,
        ui,
        resp,
        r,
        id,
        |k| (k * 100.0).round().clamp(0.0, 100.0) as u8,
        |v, done, ran| v != value || (done && !active && !ran),
        |app, v, how| {
            let mut params = json!({"clip": cid.0});
            params[key] = json!(v);
            how.run(app, "midi.cleanup", &params, &format!("cleanup:{}", cid.0))
        },
    )
    .unwrap_or(value)
}

/// A two-state switch; returns the clicked side (false = left).
fn switch(ui: &mut Ui, labels: [&str; 2], right_on: bool) -> Option<bool> {
    let t = Tokens::current();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    store(ui, "switch", r);
    let mut out = None;
    for (i, label) in labels.iter().enumerate() {
        let right = i == 1;
        let half =
            if right { Rect::from_min_max(pos2(r.center().x, r.top()), r.max) } else { Rect::from_min_max(r.min, pos2(r.center().x, r.bottom())) };
        let resp = ui.interact(half, ui.id().with(("cleanup-switch", i)), Sense::click());
        let on = right == right_on;
        let fill = if on {
            t.mode_on
        } else if resp.hovered() {
            t.button_hi
        } else {
            t.button
        };
        ui.painter().rect(half.shrink(1.0), 3.0, fill, Stroke::new(1.0, t.button_border), egui::StrokeKind::Inside);
        ui.painter().text(
            half.center(),
            Align2::CENTER_CENTER,
            *label,
            if on { bold(11.0) } else { regular(11.0) },
            if on { t.mode_on_text } else { t.text },
        );
        if resp.clicked() {
            out = Some(right);
        }
    }
    out
}

/// The audio track a chord detection starts on: the one named like the MIDI track without its
/// " MIDI" (what Audio to MIDI names it), else the nearest audio track above it, else the first.
#[cfg(not(target_arch = "wasm32"))]
fn default_chord_source(s: &Session, midi_track: soundcraft_model::TrackId) -> Option<soundcraft_model::TrackId> {
    let audio = |k: soundcraft_model::TrackKind| k == soundcraft_model::TrackKind::Audio;
    let pos = s.tracks.iter().position(|t| t.id == midi_track)?;
    let name = s.tracks.get(pos).map(|t| t.name.clone()).unwrap_or_default();
    if let Some(stem) = name.strip_suffix(" MIDI")
        && let Some(t) = s.tracks.iter().find(|t| audio(t.kind) && t.name == stem)
    {
        return Some(t.id);
    }
    s.tracks.iter().take(pos).rev().find(|t| audio(t.kind)).or_else(|| s.tracks.iter().find(|t| audio(t.kind))).map(|t| t.id)
}

/// The Cleanup panel for clip `cid` in `rect`.
pub fn panel(app: &mut SoundApp, ui: &mut Ui, cid: ClipId, rect: Rect) {
    let t = Tokens::current();
    // Only what the panel shows, copied out of the session (no clip clone per frame).
    let (track, has_chords, k, original, active, counts) = {
        let s = app.engine.session_arc();
        let Some((track, c)) = s.find_clip(cid) else { return };
        let has_chords = !soundcraft_engine::midi_cleanup::chord_spans(&s, c).is_empty();
        let m = c.midi_cleanup.as_ref();
        (
            track,
            has_chords,
            m.map_or_else(CleanupSettings::default, |m| m.settings),
            m.is_some_and(|m| m.original),
            m.is_some(),
            m.and_then(|m| m.preview.as_ref()).map(|p| p.out.report),
        )
    };
    ui.painter().rect_filled(rect, 0.0, t.panel_bg2);
    ui.painter().line_segment([rect.left_top(), rect.left_bottom()], Stroke::new(1.0, t.border));
    let inner = rect.shrink2(vec2(10.0, 6.0));
    let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::LEFT)));
    ui.set_clip_rect(rect.intersect(ui.clip_rect()));
    ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
    egui::ScrollArea::vertical().id_salt(("midi-cleanup", cid.0)).auto_shrink([false, false]).show(&mut ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("CLEANUP").font(bold(11.0)).color(t.header_text));
            if !has_chords {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(egui::RichText::new("No chord track").font(regular(10.5)).color(REMOVED))
                        .on_hover_text("No chord track: Wrong notes and Held over need chords.");
                });
            }
        });
        // No cleanup yet: the take plays, so Original is lit; Cleaned turns the defaults on.
        let plays_original = original || !active;
        if let Some(right) = switch(ui, ["Cleaned", "Original"], plays_original)
            && right != plays_original
        {
            set(app, cid, "original", json!(right));
        }
        if !has_chords {
            detect_row(app, ui, track);
        }
        slider(app, ui, cid, "wrong", "wrong", "Wrong notes", k.wrong, has_chords, active);
        slider(app, ui, cid, "held", "held", "Held over chord change", k.held, has_chords, active);
        slider(app, ui, cid, "legato", "legato", "Legato fix", k.legato, true, active);
        slider(app, ui, cid, "extend", "extend", "Extend short notes", k.extend, true, active);
        let mut protect = k.protect_melody;
        if ui.checkbox(&mut protect, egui::RichText::new("Protect melody").font(regular(11.0))).changed() {
            set(app, cid, "protect_melody", json!(protect));
        }
        match counts {
            Some(report) => {
                let summary = report.summary();
                ui.label(egui::RichText::new(&summary).font(regular(10.5)).color(t.text_dim));
                store(ui, "summary", summary);
            }
            None => {
                ui.label(
                    egui::RichText::new("Defaults shown: click Cleaned or a slider to clean up this clip.").font(regular(10.5)).color(t.text_dim),
                );
            }
        }
        ui.horizontal(|ui| {
            let reset = ui.add_enabled(active, egui::Button::new("Reset")).on_hover_text("Settings back to the defaults");
            store(ui, "reset", reset.rect);
            if reset.clicked() {
                set(app, cid, "reset", json!(true));
            }
            if active {
                let remove =
                    ui.add(egui::Label::new(egui::RichText::new("Remove").font(regular(10.0)).color(t.text_dim).underline()).sense(Sense::click()));
                if remove.on_hover_text("Drop the cleanup; the take plays untouched").clicked() {
                    let r = app.run("midi.cleanup_remove", json!({"clip": cid.0}));
                    report(app, r);
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let commit = ui
                    .add_enabled(active, egui::Button::new(egui::RichText::new("Commit").strong()).fill(t.mode_on))
                    .on_hover_text("Replace the clip's notes with what plays and drop the cleanup");
                store(ui, "commit", commit.rect);
                if commit.clicked() {
                    let r = app.run("midi.cleanup_commit", json!({"clip": cid.0}));
                    report(app, r);
                }
            });
        });
    });
}

/// The chord detection row: a combo of the session's audio tracks and "Detect chords", which opens
/// the Detect Chords dialog (the background ML job) on the chosen track.
#[cfg(not(target_arch = "wasm32"))]
fn detect_row(app: &mut SoundApp, ui: &mut Ui, midi_track: soundcraft_model::TrackId) {
    let audio: Vec<(soundcraft_model::TrackId, String)> =
        app.engine.session().tracks.iter().filter(|t| t.kind == soundcraft_model::TrackKind::Audio).map(|t| (t.id, t.name.clone())).collect();
    if !app.midi.chord_source.is_some_and(|id| audio.iter().any(|(a, _)| *a == id)) {
        app.midi.chord_source = default_chord_source(app.engine.session(), midi_track);
    }
    let chosen = app.midi.chord_source.and_then(|id| audio.iter().find(|(a, _)| *a == id)).map(|(_, n)| n.clone());
    ui.horizontal(|ui| {
        let detect = ui.add_enabled(chosen.is_some(), egui::Button::new("Detect chords"));
        store(ui, "detect", detect.rect);
        let w = ui.available_width();
        egui::ComboBox::from_id_salt("midi-cleanup-chord-source")
            .width(w - 8.0)
            .selected_text(chosen.clone().unwrap_or_else(|| "No audio track".into()))
            .show_ui(ui, |ui| {
                for (id, name) in &audio {
                    ui.selectable_value(&mut app.midi.chord_source, Some(*id), name);
                }
            });
        if detect.on_hover_text("Detect the chords of this audio track into the chord track").clicked()
            && let (Some(id), Some(name)) = (app.midi.chord_source, chosen)
            && let Err(e) = crate::ml_ui::open(app, "ml.chords", Some((json!({"track": id.0}), name)))
        {
            app.ui.status = e;
        }
    });
}

#[cfg(target_arch = "wasm32")]
fn detect_row(_app: &mut SoundApp, _ui: &mut Ui, _midi_track: soundcraft_model::TrackId) {}
