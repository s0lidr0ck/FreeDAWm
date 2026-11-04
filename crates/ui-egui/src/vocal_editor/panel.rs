//! The Tuning panel, right of the roll: Tuned/Original, the selection summary, the Centre, Drift,
//! Vibrato, Raise quiet and Lower loud sliders (a drag previews live and is one undo step), Snap to
//! key, the macro presets, the key and chord at the playhead, and Reset / Remove / Commit.

use super::layout::Frames;
use super::{editor_track, hook, presets, roll, store};
use crate::SoundApp;
use crate::slider_drag::report;
use crate::theme::{Tokens, bold, regular};
use egui::{Align, Align2, Color32, Layout, Rect, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_engine::vocal::NoteRef;
use soundcraft_model::{ClipId, TrackId};
use soundcraft_vocal::{MacroPreset, NoteEdit, NoteMode, selection_summary_with};

const GREEN: Color32 = Color32::from_rgb(61, 187, 94);
const BLUE: Color32 = Color32::from_rgb(47, 127, 208);
const AMBER: Color32 = Color32::from_rgb(201, 138, 58);

/// One slider's field.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Field {
    Centre,
    Drift,
    Vibrato,
    Raise,
    Lower,
}

impl Field {
    fn key(self) -> &'static str {
        match self {
            Field::Centre => "centre",
            Field::Drift => "drift",
            Field::Vibrato => "vibrato",
            Field::Raise => "raise",
            Field::Lower => "lower",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Field::Centre => "Centre",
            Field::Drift => "Drift",
            Field::Vibrato => "Vibrato",
            Field::Raise => "Raise quiet",
            Field::Lower => "Lower loud",
        }
    }
    fn of(self, e: &NoteEdit) -> f32 {
        match self {
            Field::Centre => e.centre,
            Field::Drift => e.drift,
            Field::Vibrato => e.vibrato,
            Field::Raise => e.raise,
            Field::Lower => e.lower,
        }
    }
    fn bipolar(self) -> bool {
        self == Field::Vibrato
    }
}

/// A percentage as the panel shows it: "40%", bipolar "−10%" / "+10%".
fn pct_text(v: i32, bipolar: bool) -> String {
    if v < 0 {
        format!("\u{2212}{}%", -v)
    } else if bipolar && v > 0 {
        format!("+{v}%")
    } else {
        format!("{v}%")
    }
}

/// The selection's numbers, across clips.
#[derive(Debug, Clone, Default)]
struct Summary {
    count: usize,
    start: f64,
    end: f64,
    off_before: f64,
    off_after: f64,
    drift_before: f64,
    drift_after: f64,
    spread: f32,
}

/// What the panel acts on this frame.
struct Target {
    track: TrackId,
    refs: Vec<NoteRef>,
    edits: Vec<NoteEdit>,
    summary: Option<Summary>,
    /// Tuned clips holding a selected note, else every tuned clip on the track.
    clips: Vec<ClipId>,
    original: bool,
    /// The track's audio clips (what the Notes selector sets), and whether every one is Musical.
    audio: Vec<ClipId>,
    musical: bool,
}

fn target(app: &mut SoundApp, track: TrackId) -> Target {
    let s = app.engine.session_arc();
    // The Vocal Studio's Edit All: the selection on any shown track, else every shown track's
    // notes (the amounts act on all of them, one undo step).
    let all = app.vocal.studio.on && app.vocal.studio.edit_all;
    let tracks = if all { super::studio::edit_tracks(app, track) } else { vec![track] };
    let mut sel = roll::selection_with_twins(app);
    if all && sel.is_empty() {
        sel = tracks.iter().flat_map(|t| roll::tunable_notes(app, *t)).collect();
    }
    let cvs: Vec<roll::ClipView> = tracks.iter().flat_map(|t| roll::clip_views(app, *t, None)).collect();
    let rate = f64::from(s.sample_rate.hz().max(1));
    let mut refs = Vec::new();
    let mut edits = Vec::new();
    let mut sum = Summary::default();
    let mut levels = Vec::new();
    for cv in &cvs {
        let ids: Vec<_> = cv.a.notes.iter().filter(|n| sel.contains(&NoteRef { clip: cv.clip.id, note: n.id })).map(|n| n.id).collect();
        if ids.is_empty() {
            continue;
        }
        for id in &ids {
            refs.push(NoteRef { clip: cv.clip.id, note: *id });
            edits.push(cv.edits.get(id).cloned().unwrap_or_default());
        }
        levels.extend(cv.a.notes.iter().filter(|n| ids.contains(&n.id)).map(|n| n.level_db));
        if let Some(sm) = selection_summary_with(&cv.a, &cv.edits, &ids, &cv.scale, soundcraft_engine::vocal::note_mode(&cv.clip)) {
            let fm = Frames::of(&cv.clip, cv.a.track.hop_samples);
            let w = sm.count as f64;
            let (a, b) = (fm.time(sm.start_frame) as f64 / rate, fm.time(sm.end_frame) as f64 / rate);
            sum.start = if sum.count == 0 { a } else { sum.start.min(a) };
            sum.end = if sum.count == 0 { b } else { sum.end.max(b) };
            sum.count += sm.count;
            sum.off_before += f64::from(sm.avg_off_before) * w;
            sum.off_after += f64::from(sm.avg_off_after) * w;
            sum.drift_before += f64::from(sm.drift_before) * w;
            sum.drift_after += f64::from(sm.drift_after) * w;
        }
    }
    let summary = (sum.count > 0).then(|| {
        let c = sum.count as f64;
        let max = levels.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let min = levels.iter().copied().fold(f32::INFINITY, f32::min);
        Summary {
            off_before: sum.off_before / c,
            off_after: sum.off_after / c,
            drift_before: sum.drift_before / c,
            drift_after: sum.drift_after / c,
            spread: if max.is_finite() && min.is_finite() { max - min } else { 0.0 },
            ..sum
        }
    });
    let tuned: Vec<(ClipId, bool)> = tracks
        .iter()
        .filter_map(|t| s.track(*t))
        .flat_map(|t| t.clips().iter().filter_map(|c| c.tune_live().map(|tu| (c.id, tu.original))))
        .collect();
    let holding: Vec<(ClipId, bool)> = tuned.iter().copied().filter(|(c, _)| refs.iter().any(|r| r.clip == *c)).collect();
    let chosen = if holding.is_empty() { tuned } else { holding };
    let audio_clips: Vec<&soundcraft_model::Clip> =
        tracks.iter().filter_map(|t| s.track(*t)).flat_map(|t| t.clips().iter().filter(|c| c.is_audio())).collect();
    let musical = !audio_clips.is_empty() && audio_clips.iter().all(|c| soundcraft_engine::vocal::note_mode(c) == NoteMode::Musical);
    Target {
        audio: audio_clips.iter().map(|c| c.id).collect(),
        musical,
        track,
        original: !chosen.is_empty() && chosen.iter().all(|(_, o)| *o),
        clips: chosen.into_iter().map(|(c, _)| c).collect(),
        refs,
        edits,
        summary,
    }
}

/// The notes as the commands take them.
fn notes_json(refs: &[NoteRef]) -> Value {
    json!(refs)
}

/// The Tuned | Original switch; returns the clicked side (true = Original).
fn switch(ui: &mut Ui, original: bool, enabled: bool) -> Option<bool> {
    let t = Tokens::current();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let mut out = None;
    for (i, label) in ["Tuned", "Original"].into_iter().enumerate() {
        let right = i == 1;
        let half =
            if right { Rect::from_min_max(pos2(r.center().x, r.top()), r.max) } else { Rect::from_min_max(r.min, pos2(r.center().x, r.bottom())) };
        store(ui.ctx(), if right { "switch-original" } else { "switch-tuned" }, half);
        let resp = ui.interact(half, ui.id().with(("vocal-switch", i)), if enabled { Sense::click() } else { Sense::hover() });
        let on = right == original;
        let fill = if on && !right {
            GREEN
        } else if on {
            t.mode_on
        } else if resp.hovered() && enabled {
            t.button_hi
        } else {
            t.button
        };
        ui.painter().rect(half, 0.0, fill, Stroke::new(1.0, t.button_border), StrokeKind::Inside);
        let text_c = if on {
            Color32::WHITE
        } else if enabled {
            t.text
        } else {
            t.text_dim
        };
        ui.painter().text(half.center(), Align2::CENTER_CENTER, label, if on { bold(12.0) } else { regular(12.0) }, text_c);
        if resp.clicked() {
            out = Some(right);
        }
    }
    out
}

/// The Notes selector, Classic | Musical; returns the clicked side (true = Musical).
fn notes_switch(ui: &mut Ui, musical: bool, enabled: bool) -> Option<bool> {
    let t = Tokens::current();
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let label_w = 46.0;
    ui.painter().text(row.left_center(), Align2::LEFT_CENTER, "Notes", regular(12.0), if enabled { t.text } else { t.text_dim });
    let r = Rect::from_min_max(pos2(row.left() + label_w, row.top()), row.max);
    let mut out = None;
    for (i, label) in ["Classic", "Musical"].into_iter().enumerate() {
        let right = i == 1;
        let half =
            if right { Rect::from_min_max(pos2(r.center().x, r.top()), r.max) } else { Rect::from_min_max(r.min, pos2(r.center().x, r.bottom())) };
        store(ui.ctx(), if right { "notes-musical" } else { "notes-classic" }, half);
        let resp = ui.interact(half, ui.id().with(("vocal-notes-mode", i)), if enabled { Sense::click() } else { Sense::hover() });
        let on = right == musical;
        let fill = if on {
            t.mode_on
        } else if resp.hovered() && enabled {
            t.button_hi
        } else {
            t.button
        };
        ui.painter().rect(half, 0.0, fill, Stroke::new(1.0, t.button_border), StrokeKind::Inside);
        let text_c = if on {
            Color32::WHITE
        } else if enabled {
            t.text
        } else {
            t.text_dim
        };
        ui.painter().text(half.center(), Align2::CENTER_CENTER, label, if on { bold(11.5) } else { regular(11.5) }, text_c);
        let resp = resp.on_hover_text(if right {
            "Musical: notes by syllable and settled pitch; scoops, slides and vibrato stay inside their note, and tuning moves whole notes with smooth glides between them (for loose vocals)"
        } else {
            "Classic: every pitch held for 70 ms is a note (tight on a vocal near its notes)"
        });
        if resp.clicked() {
            out = Some(right);
        }
    }
    store(ui.ctx(), "notes-mode", if musical { "musical".to_string() } else { "classic".to_string() });
    out
}

/// A slider row: label and value, the track, and its drag. `value` is the selection's mean
/// (fraction), `mixed` when the notes differ.
fn slider(app: &mut SoundApp, ui: &mut Ui, tg: &Target, f: Field, enabled: bool) {
    let t = Tokens::current();
    let vals: Vec<f32> = tg.edits.iter().map(|e| f.of(e)).collect();
    let mean = if vals.is_empty() { 0.0 } else { vals.iter().sum::<f32>() / vals.len() as f32 };
    let mixed = vals.iter().any(|v| (v - mean).abs() > 0.005);
    let w = ui.available_width();
    let (both, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
    let row = Rect::from_min_size(both.min, vec2(w, 16.0));
    let r = Rect::from_min_max(pos2(both.left(), row.bottom() + 2.0), both.max);
    let resp = ui.interact(r, ui.id().with(("vocal-slider", f.key())), if enabled { Sense::click_and_drag() } else { Sense::hover() });
    store(ui.ctx(), f.key(), r);
    let lo = if f.bipolar() { -100 } else { 0 };
    let shown_pct = drive(app, ui, &resp, r, tg, f, lo);
    let dragging = shown_pct.is_some();
    let pct = shown_pct.unwrap_or((mean * 100.0).round() as i32);
    let value = if mixed && !dragging { format!("mixed (avg {})", pct_text(pct, f.bipolar())) } else { pct_text(pct, f.bipolar()) };
    let hollow = mixed && !dragging;
    ui.painter().text(row.left_center(), Align2::LEFT_CENTER, f.label(), regular(12.5), if enabled { t.text } else { t.text_dim });
    ui.painter().text(row.right_center(), Align2::RIGHT_CENTER, &value, regular(12.0), t.text_dim);
    store(ui.ctx(), &format!("value-{}", f.key()), value);
    let track = Rect::from_center_size(r.center(), vec2(r.width(), 6.0));
    ui.painter().rect_filled(track, 3.0, t.fader_track);
    let frac = (pct - lo) as f32 / (100 - lo) as f32;
    let kx = track.left() + track.width() * frac.clamp(0.0, 1.0);
    let fill_c = if !enabled {
        BLUE.gamma_multiply(0.35)
    } else if f.bipolar() {
        AMBER
    } else {
        BLUE
    };
    let x0 = if f.bipolar() { track.center().x } else { track.left() };
    ui.painter().rect_filled(Rect::from_min_max(pos2(x0.min(kx), track.top()), pos2(x0.max(kx), track.bottom())), 3.0, fill_c);
    if f.bipolar() {
        ui.painter().line_segment([pos2(track.center().x, r.top() + 2.0), pos2(track.center().x, r.bottom() - 2.0)], Stroke::new(2.0, t.text_dim));
    }
    let knob =
        Rect::from_center_size(pos2(kx.clamp(track.left() + 6.0, (track.right() - 6.0).max(track.left() + 6.0)), r.center().y), vec2(13.0, 13.0));
    if hollow {
        ui.painter().rect(knob, 3.0, t.panel_bg2, Stroke::new(1.5, t.text), StrokeKind::Inside);
    } else {
        ui.painter().rect(
            knob,
            3.0,
            if enabled { Color32::from_rgb(233, 237, 242) } else { t.text_dim },
            Stroke::new(1.0, Color32::from_rgb(154, 163, 173)),
            StrokeKind::Inside,
        );
    }
    if !enabled {
        resp.on_hover_text("Select notes in the roll first");
    }
}

/// A slider's gesture through [`crate::slider_drag::drive`] (as MIDI Cleanup's): `vocal.edit`
/// merged into one undo step. Returns the percentage to draw while the gesture lasts.
fn drive(app: &mut SoundApp, ui: &Ui, resp: &egui::Response, r: Rect, tg: &Target, f: Field, lo: i32) -> Option<i32> {
    let id = egui::Id::new(("soundcraft-vocal-pending", f.key()));
    let whole = app.vocal.whole_track;
    let v = crate::slider_drag::drive(
        app,
        ui,
        resp,
        r,
        id,
        |k| (lo as f32 + k * (100 - lo) as f32).round() as i32,
        |v, done, ran| {
            let differs = tg.edits.iter().any(|e| (f.of(e) * 100.0).round() as i32 != v);
            (differs || (done && !ran)) && !tg.refs.is_empty()
        },
        |app, v, how| {
            let mut params = json!({"notes": notes_json(&tg.refs)});
            params[f.key()] = json!(v);
            if whole {
                params["reference"] = json!("track");
            }
            let r = how.run(app, crate::vocal_ids::VOCAL_EDIT, &params, &format!("vocal:{}", tg.track.0));
            // Progress only after a send that took (never "on 0 notes", never over a refusal).
            let n = tg.refs.len();
            if r.is_ok() && n > 0 {
                app.ui.status = format!("{} {} on {n} note{}", f.label(), pct_text(v, f.bipolar()), if n == 1 { "" } else { "s" });
                // The gesture's end (a release, or a click) previews the edited note.
                if how != crate::slider_drag::How::Merged {
                    super::preview::changed(app);
                }
            }
            r
        },
    )?;
    Some(v)
}

fn caption(ui: &mut Ui, text: &str) {
    let t = Tokens::current();
    ui.label(RichText::new(text).font(regular(11.5)).color(t.text_dim));
}

fn section(ui: &mut Ui, label: &str) {
    let t = Tokens::current();
    ui.add_space(2.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().line_segment([r.left_center(), r.right_center()], Stroke::new(1.0, t.border));
    ui.add_space(4.0);
    if !label.is_empty() {
        ui.label(RichText::new(label).font(bold(11.0)).color(t.text_dim));
    }
}

/// The Tuning panel in `rect`.
pub fn panel(app: &mut SoundApp, ui: &mut Ui, rect: Rect) {
    let t = Tokens::current();
    let Some(track) = editor_track(app) else { return };
    ui.painter().rect_filled(rect, 0.0, t.panel_bg2);
    ui.painter().line_segment([rect.left_top(), rect.left_bottom()], Stroke::new(1.0, t.border));
    let tg = target(app, track);
    let has_sel = !tg.refs.is_empty();
    let inner = rect.shrink2(vec2(14.0, 10.0));
    let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::LEFT)));
    ui.set_clip_rect(rect.intersect(ui.clip_rect()));
    ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
    egui::ScrollArea::vertical().id_salt(("vocal-panel", track.0)).auto_shrink([false, false]).show(&mut ui, |ui| {
        ui.label(RichText::new("TUNING").font(bold(13.0)).color(t.header_text));
        ui.add_space(4.0);
        if let Some(original) = switch(ui, tg.original, !tg.clips.is_empty())
            && original != tg.original
        {
            let r = app.run(crate::vocal_ids::VOCAL_TOGGLE, json!({"clips": tg.clips}));
            report(app, r);
        }
        ui.add_space(4.0);
        if let Some(musical) = notes_switch(ui, tg.musical, !tg.audio.is_empty())
            && musical != tg.musical
        {
            let mode = if musical { NoteMode::Musical } else { NoteMode::Classic };
            let r = app.run(crate::vocal_ids::VOCAL_NOTE_MODE, json!({"clips": tg.audio, "mode": mode.name()}));
            if r.is_ok() {
                app.ui.status = format!("Notes: {}", if musical { "Musical" } else { "Classic" });
            }
            report(app, r);
        }
        ui.add_space(4.0);
        summary(ui, tg.summary.as_ref());
        moved_row(app, ui, &tg);
        ui.add_space(4.0);
        slider(app, ui, &tg, Field::Centre, has_sel);
        caption(ui, "pulls centres to the target \u{b7} keeps vibrato");
        ui.add_space(2.0);
        slider(app, ui, &tg, Field::Drift, has_sel);
        match &tg.summary {
            Some(sm) => caption(ui, &format!("selection drift {:.0}c \u{2192} {:.0}c", sm.drift_before, sm.drift_after)),
            None => caption(ui, "takes out slow pitch wander"),
        }
        ui.add_space(2.0);
        slider(app, ui, &tg, Field::Vibrato, has_sel);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
        for (a, x, text) in [
            (Align2::LEFT_CENTER, r.left(), "\u{2212} less"),
            (Align2::CENTER_CENTER, r.center().x, "0"),
            (Align2::RIGHT_CENTER, r.right(), "more +"),
        ] {
            ui.painter().text(pos2(x, r.center().y), a, text, regular(11.0), t.text_dim);
        }
        section(ui, "LOUDNESS");
        slider(app, ui, &tg, Field::Raise, has_sel);
        slider(app, ui, &tg, Field::Lower, has_sel);
        let typical = roll::selection_typical(app, track);
        let whose = if app.vocal.whole_track || app.vocal.selected.is_empty() { "the track's" } else { "selection's" };
        let loud = format!("towards {whose} typical level {}", fmt_db(typical));
        caption(ui, &loud);
        store(ui.ctx(), "loud-caption", loud);
        ui.add_space(4.0);
        snap_row(app, ui, &tg);
        section(ui, "MACRO");
        macro_rows(app, ui, &tg);
        section(ui, "");
        key_lines(app, ui);
        ui.add_space(6.0);
        bottom_row(app, ui, &tg);
        // Below everything else, so the controls above keep their places.
        section(ui, "FORMANT");
        formant_row(app, ui, &tg);
    });
}

fn fmt_db(db: f32) -> String {
    let r = if db.is_finite() { db.round() } else { 0.0 };
    if r < 0.0 { format!("\u{2212}{:.0} dB", -r) } else { format!("{r:.0} dB") }
}

fn summary(ui: &mut Ui, sm: Option<&Summary>) {
    let t = Tokens::current();
    let Some(sm) = sm else {
        let s = "No notes selected \u{b7} click or drag in the roll".to_string();
        ui.label(RichText::new(&s).font(regular(12.0)).color(t.text_dim));
        store(ui.ctx(), "summary", s);
        store(ui.ctx(), "summary2", String::new());
        return;
    };
    let head = format!("{} note{} selected", sm.count, if sm.count == 1 { "" } else { "s" });
    let span = format!(" \u{b7} {:.1}\u{2013}{:.1} s", sm.start, sm.end);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label(RichText::new(&head).font(bold(12.0)).color(t.text));
        ui.label(RichText::new(&span).font(regular(12.0)).color(t.text_dim));
    });
    let line2 = format!("avg {:.0}c off \u{2192} now {:.0}c \u{b7} spread {:.1} dB", sm.off_before, sm.off_after, sm.spread);
    ui.label(RichText::new(&line2).font(regular(12.0)).color(t.text_dim));
    store(ui.ctx(), "summary", format!("{head}{span}"));
    store(ui.ctx(), "summary2", line2);
}

/// "moved +200c ×" when a selected note was dragged up or down; × clears the move (one undo step).
fn moved_row(app: &mut SoundApp, ui: &mut Ui, tg: &Target) {
    let t = Tokens::current();
    let Some(text) = super::drag::moved_text(&tg.edits) else {
        store(ui.ctx(), "moved", String::new());
        return;
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(RichText::new(&text).font(regular(12.0)).color(t.text));
        let x = ui
            .add(egui::Button::new(RichText::new("\u{d7}").font(bold(12.0))).small())
            .on_hover_text("Clear the move: the notes back to their tuned pitch");
        store(ui.ctx(), "moved-clear", x.rect);
        if x.clicked() {
            let r = app.run(crate::vocal_ids::VOCAL_EDIT, json!({"notes": notes_json(&tg.refs), "offset": 0}));
            if r.is_ok() {
                super::preview::changed(app);
            }
            report(app, r);
        }
    });
    store(ui.ctx(), "moved", text);
}

fn snap_row(app: &mut SoundApp, ui: &mut Ui, tg: &Target) {
    let on = !tg.edits.is_empty() && tg.edits.iter().all(|e| e.snap_to_key);
    let has_key = roll::has_key(app.engine.session());
    let enabled = has_key && !tg.refs.is_empty();
    let why = if !has_key {
        "Set a key in the timeline first"
    } else if tg.refs.is_empty() {
        "Select notes in the roll first"
    } else {
        ""
    };
    hook(ui.ctx(), "snap-why", why.to_string());
    let mut v = on;
    // A visible box in both themes (the panel's background hides the default one).
    let t = Tokens::current();
    let resp = ui
        .scope(|ui| {
            let w = &mut ui.visuals_mut().widgets;
            for s in [&mut w.inactive, &mut w.noninteractive, &mut w.hovered] {
                s.bg_stroke = Stroke::new(1.0, t.text_dim);
                s.bg_fill = t.button;
                s.weak_bg_fill = t.button;
            }
            ui.add_enabled(enabled, egui::Checkbox::new(&mut v, RichText::new("Snap to key").font(regular(12.5))))
        })
        .inner;
    store(ui.ctx(), "snap", resp.rect);
    let resp = resp.on_hover_text("Centre notes to the nearest note of the session key, not the nearest semitone").on_disabled_hover_text(why);
    if resp.changed() && enabled {
        let r = app.run(crate::vocal_ids::VOCAL_EDIT, json!({"notes": notes_json(&tg.refs), "snap_to_key": v}));
        if r.is_ok() {
            super::preview::changed(app);
        }
        report(app, r);
    }
}

/// A formant value as the panel shows it: "0 st", "+2.5 st", "−1 st".
pub fn formant_text(st: f32) -> String {
    let r = if st.is_finite() { (st * 10.0).round() / 10.0 } else { 0.0 };
    if r == 0.0 {
        "0 st".into()
    } else if r < 0.0 {
        format!("\u{2212}{} st", -r)
    } else {
        format!("+{r} st")
    }
}

/// One formant field: a value dragged (or typed) locally and sent once on release. Returns the
/// value to send.
fn formant_field(ui: &mut Ui, id: &str, current: f32, enabled: bool) -> Option<f32> {
    let key = egui::Id::new(("vocal-formant-edit", id.to_string()));
    let mut v: f32 = ui.ctx().data(|d| d.get_temp(key)).unwrap_or(current);
    let max = soundcraft_vocal::MAX_FORMANT_ST;
    let resp = ui.add_enabled(
        enabled,
        egui::DragValue::new(&mut v)
            .range(-max..=max)
            .speed(0.05)
            .custom_formatter(|n, _| formant_text(n as f32).trim_end_matches(" st").to_string()),
    );
    store(ui.ctx(), &format!("formant-{id}"), resp.rect);
    if resp.dragged() || resp.has_focus() {
        ui.ctx().data_mut(|d| d.insert_temp(key, v));
        return None;
    }
    ui.ctx().data_mut(|d| d.remove::<f32>(key));
    ((resp.drag_stopped() || resp.lost_focus() || resp.changed()) && (v - current).abs() >= 0.05).then_some(v)
}

/// Formant: the clip's shift (every audio clip of the track) and the selected notes' own.
fn formant_row(app: &mut SoundApp, ui: &mut Ui, tg: &Target) {
    let t = Tokens::current();
    let s = app.engine.session_arc();
    let clip_st = tg.audio.iter().filter_map(|c| s.find_clip(*c)).filter_map(|(_, c)| c.tune_live()).map(|tu| tu.formant_st).next().unwrap_or(0.0);
    let note_st = tg.edits.first().map_or(0.0, |e| e.formant_st);
    hook(ui.ctx(), "formant-values", (clip_st, note_st));
    ui.horizontal(|ui| {
        ui.label(RichText::new("Formant").font(regular(12.5)).color(t.text))
            .on_hover_text("Moves the voice's formants (darker below 0, brighter above) without changing the pitch");
        ui.label(RichText::new("clip").font(regular(11.0)).color(t.text_dim));
        if let Some(v) = formant_field(ui, "clip", clip_st, !tg.audio.is_empty()) {
            let r = app.run("vocal.formant", json!({"clips": tg.audio, "semitones": v}));
            if r.is_ok() {
                let what = if tg.audio.len() == 1 { "the clip".to_string() } else { format!("{} clips", tg.audio.len()) };
                app.ui.status = format!("Formant {} on {what}", formant_text(v));
            }
            report(app, r);
        }
        ui.label(RichText::new("notes").font(regular(11.0)).color(t.text_dim));
        if let Some(v) = formant_field(ui, "notes", note_st, !tg.refs.is_empty()) {
            let r = app.run(crate::vocal_ids::VOCAL_EDIT, json!({"notes": notes_json(&tg.refs), "formant": v}));
            if r.is_ok() {
                app.ui.status = format!("Formant {} on {} notes", formant_text(v), tg.refs.len());
                super::preview::changed(app);
            }
            report(app, r);
        }
    });
}

/// The selection's slider values as a preset (Save…).
fn current(tg: &Target, name: &str) -> MacroPreset {
    let mean = |f: Field| if tg.edits.is_empty() { 0.0 } else { tg.edits.iter().map(|e| f.of(e)).sum::<f32>() / tg.edits.len() as f32 };
    MacroPreset {
        name: name.to_string(),
        centre: mean(Field::Centre),
        drift: mean(Field::Drift),
        vibrato: mean(Field::Vibrato),
        raise: mean(Field::Raise),
        lower: mean(Field::Lower),
    }
}

fn macro_rows(app: &mut SoundApp, ui: &mut Ui, tg: &Target) {
    let t = Tokens::current();
    if app.vocal.presets.is_none() {
        app.vocal.presets = Some(presets::all(app));
    }
    let all = app.vocal.presets.clone().unwrap_or_default();
    if !all.iter().any(|p| p.name == app.vocal.preset) {
        app.vocal.preset = all.first().map(|p| p.name.clone()).unwrap_or_default();
    }
    let chosen = all.iter().find(|p| p.name == app.vocal.preset).cloned();
    let can_save = app.preset_dir.is_some();
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 112.0).max(60.0);
        egui::ComboBox::from_id_salt("vocal-macro-preset").width(w).selected_text(app.vocal.preset.clone()).show_ui(ui, |ui| {
            for p in &all {
                ui.selectable_value(&mut app.vocal.preset, p.name.clone(), &p.name);
            }
        });
        let apply = ui.add(egui::Button::new(RichText::new("Apply").color(Color32::WHITE)).fill(Color32::from_rgb(43, 90, 138)));
        let save = ui.add_enabled(can_save, egui::Button::new("Save\u{2026}"));
        store(ui.ctx(), "apply", apply.rect);
        store(ui.ctx(), "save", save.rect);
        if apply.on_hover_text("Apply the macro to the selected notes (all the track's notes when none is selected)").clicked()
            && let Some(p) = &chosen
        {
            let (refs, track_ref) = if tg.refs.is_empty() {
                (roll::tunable_notes(app, tg.track), true)
            } else {
                (tg.refs.clone(), app.vocal.whole_track)
            };
            if refs.is_empty() {
                app.ui.status = "No notes to tune yet".into();
            } else {
                let pc = |v: f32| (v * 100.0).round();
                let mut params = json!({"notes": notes_json(&refs), "centre": pc(p.centre), "drift": pc(p.drift), "vibrato": pc(p.vibrato), "raise": pc(p.raise), "lower": pc(p.lower)});
                if track_ref {
                    params["reference"] = json!("track");
                }
                let r = app.run(crate::vocal_ids::VOCAL_MACRO, params);
                if r.is_ok() {
                    super::preview::changed(app);
                }
                report(app, r);
            }
        }
        let save = if can_save { save.on_hover_text("Save the selection's values as a macro") } else { save.on_disabled_hover_text("No preset folder") };
        if save.clicked() && can_save {
            app.vocal.save_name = Some(String::new());
        }
    });
    store(ui.ctx(), "preset", app.vocal.preset.clone());
    if let Some(p) = &chosen {
        let line = presets::values_line(p);
        ui.label(RichText::new(&line).font(regular(11.5)).color(t.text_dim));
        store(ui.ctx(), "preset-values", line);
    }
    // Save…'s name field.
    if let Some(mut name) = app.vocal.save_name.take() {
        let mut keep = true;
        ui.horizontal(|ui| {
            let field = ui.add(egui::TextEdit::singleline(&mut name).id(egui::Id::new("soundcraft-vocal-save-name")).hint_text("Macro name").desired_width(130.0));
            store(ui.ctx(), "save-name", field.rect);
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let ok = ui.add_enabled(!name.trim().is_empty(), egui::Button::new("Save"));
            store(ui.ctx(), "save-ok", ok.rect);
            if (ok.clicked() || enter) && !name.trim().is_empty() {
                let p = current(tg, name.trim());
                let pc = |v: f32| (v * 100.0).round();
                let r = app.run(
                    "ui.vocal_preset_save",
                    json!({"name": p.name, "centre": pc(p.centre), "drift": pc(p.drift), "vibrato": pc(p.vibrato), "raise": pc(p.raise), "lower": pc(p.lower)}),
                );
                if r.is_ok() {
                    app.vocal.preset = p.name.clone();
                }
                report(app, r);
                keep = false;
            }
            if ui.button("Cancel").clicked() {
                keep = false;
            }
        });
        if keep {
            app.vocal.save_name = Some(name);
        }
    }
}

fn key_lines(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let s = app.engine.session_arc();
    let at = app.position();
    let key = roll::has_key(&s).then(|| roll::key_at(&s, at));
    let chord = roll::chord_name_at(&s, at);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        match &key {
            Some(k) => {
                ui.label(RichText::new("Key ").font(regular(12.0)).color(t.text_dim));
                ui.label(RichText::new(k).font(bold(12.0)).color(t.text));
                ui.label(RichText::new(" \u{b7} from session").font(regular(12.0)).color(t.text_dim));
            }
            None => {
                ui.label(RichText::new("No key set").font(regular(12.0)).color(t.text_dim));
            }
        }
    });
    store(ui.ctx(), "key-line", key.as_ref().map_or_else(|| "No key set".to_string(), |k| format!("Key {k} \u{b7} from session")));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        match &chord {
            Some(c) => {
                ui.label(RichText::new("Chord at playhead ").font(regular(12.0)).color(t.text_dim));
                ui.label(
                    RichText::new(format!(" {c} "))
                        .font(bold(12.0))
                        .color(Color32::from_rgb(223, 233, 247))
                        .background_color(Color32::from_rgb(43, 58, 77)),
                );
                ui.label(RichText::new(" \u{b7} from chord track").font(regular(12.0)).color(t.text_dim));
            }
            None => {
                ui.label(RichText::new("No chord track").font(regular(12.0)).color(t.text_dim));
            }
        }
    });
    store(
        ui.ctx(),
        "chord-line",
        chord.as_ref().map_or_else(|| "No chord track".to_string(), |c| format!("Chord at playhead {c} \u{b7} from chord track")),
    );
    ui.label(RichText::new("Set key & chords in the timeline, not here.").font(regular(11.0)).color(t.text_dim));
}

fn bottom_row(app: &mut SoundApp, ui: &mut Ui, tg: &Target) {
    let t = Tokens::current();
    let has_clips = !tg.clips.is_empty();
    ui.horizontal(|ui| {
        let reset = ui.add_enabled(!tg.refs.is_empty(), egui::Button::new("Reset")).on_hover_text("The selected notes back to as sung");
        store(ui.ctx(), "reset", reset.rect);
        if reset.clicked() {
            let r = app.run(crate::vocal_ids::VOCAL_RESET, json!({"notes": notes_json(&tg.refs)}));
            if r.is_ok() {
                super::preview::changed(app);
            }
            report(app, r);
        }
        let remove = ui.add_enabled(
            has_clips,
            egui::Label::new(RichText::new("Remove").font(regular(12.0)).color(t.text_dim).underline()).sense(Sense::click()),
        );
        store(ui.ctx(), "remove", remove.rect);
        if remove.on_hover_text("Drop the tuning; the clips play as sung (undoable)").clicked() && has_clips {
            let r = app.run(crate::vocal_ids::VOCAL_REMOVE, json!({"clips": tg.clips}));
            report(app, r);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let commit = ui
                .add_enabled(has_clips, egui::Button::new(RichText::new("Commit").strong().color(Color32::WHITE)).fill(GREEN))
                .on_hover_text("Render the tuning into the clips' audio and drop the tuning (undoable)");
            store(ui.ctx(), "commit", commit.rect);
            if commit.clicked() {
                let r = app.run(crate::vocal_ids::VOCAL_COMMIT, json!({"clips": tg.clips}));
                report(app, r);
            }
        });
    });
}

/// `ui.vocal_preset_save {name, centre, drift, vibrato, raise, lower}` (percentages): writes a
/// user macro preset.
pub fn save_command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let dir = app.preset_dir.clone().ok_or("no preset folder: macros can't be saved here")?;
    let name = p.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).ok_or("`name` is required")?;
    if soundcraft_vocal::factory_presets().iter().any(|f| f.name.trim().eq_ignore_ascii_case(name)) {
        return Err(format!("name in use: \"{name}\" is a factory macro"));
    }
    let pct = |k: &str, lo: f64| -> Result<f32, String> {
        match p.get(k) {
            None => Ok(0.0),
            Some(v) => v
                .as_f64()
                .filter(|m| m.is_finite() && (lo..=100.0).contains(m))
                .map(|m| (m / 100.0) as f32)
                .ok_or_else(|| format!("`{k}` must be a number from {lo} to 100 (%)")),
        }
    };
    let preset = MacroPreset {
        name: name.to_string(),
        centre: pct("centre", 0.0)?,
        drift: pct("drift", 0.0)?,
        vibrato: pct("vibrato", -100.0)?,
        raise: pct("raise", 0.0)?,
        lower: pct("lower", 0.0)?,
    };
    presets::save(&dir, &preset).map_err(|e| format!("couldn't save the macro: {e}"))?;
    app.vocal.presets = None;
    Ok(json!({"name": preset.name, "file": presets::file_stem(&preset.name) + ".json"}))
}
