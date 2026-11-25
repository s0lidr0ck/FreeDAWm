//! The Gild panel (the Vocal Editor's right panel, beside Tuning) and the Gild strip under the
//! word lane: effects on the words chosen in the word lane (docs/gild.md). Everything goes
//! through the `gild.*` commands; a slider drag is one undo step.
//!
//! - Select words in the word lane (click, Shift-click a phrase, Ctrl-click separate words, or the
//!   helpers here: last or first word of every line, every occurrence). The effect buttons turn
//!   an effect on or off for the selection (gilding words that are not gilded yet, from the
//!   settings last used); a card per effect that is on holds its controls and level.
//! - Copy / Paste, Remove, Audition (one gilding), presets, and Remove All on the track (asks
//!   first). The track's Dry switch and Gild level sit at the top.
//! - The Gild strip draws every gilding as a bar with a short mark per effect (letters, so they
//!   read without colour); a selected single gilding has a handle at each end to drag.

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use egui::{Align, Align2, Color32, CornerRadius, Layout, Rect, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_engine::word_time::{WordRef, track_words};
use soundcraft_model::{ClipId, GILD_KINDS, GildFx, Session, TrackId};
use soundcraft_time::Samples;

/// The Gild strip's height (under the word lane, when the track has gildings).
pub const STRIP_H: f32 = 16.0;
/// Gilded gold.
pub const GOLD: Color32 = Color32::from_rgb(222, 178, 60);

/// An effect's name in the panel.
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "shift" => "Shift",
        "grit" => "Grit",
        "double" => "Double",
        "harmony" => "Harmony",
        "delay" => "Delay",
        "reverb" => "Reverb",
        "swell" => "Swell-in",
        _ => "",
    }
}

/// An effect's short mark on the strip and the lanes (letters: readable without colour).
pub fn kind_mark(kind: &str) -> &'static str {
    match kind {
        "shift" => "Sh",
        "grit" => "Gr",
        "double" => "Db",
        "harmony" => "Hm",
        "delay" => "Dl",
        "reverb" => "Rv",
        "swell" => "Sw",
        _ => "",
    }
}

fn kind_help(kind: &str) -> &'static str {
    match kind {
        "shift" => "Moves the word's pitch and throat size, layered beside it or in its place",
        "grit" => "Saturates or distorts the word, blended with itself",
        "double" => "A second copy a little late and a little sharp",
        "harmony" => "Voices that follow the word in the song's key (the Key ruler)",
        "delay" => "Only this word feeds an echo that repeats on after it",
        "reverb" => "The word rings on in a space after it is sung",
        "swell" => "A reversed reverb that rises into the word and stops as it starts",
        _ => "",
    }
}

/// A gilding on a track as the panel and the strip use it.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackGild {
    pub clip: ClipId,
    pub id: u32,
    /// Timeline span (the part heard).
    pub start: Samples,
    pub end: Samples,
    pub text: String,
    pub held: bool,
    pub fx: GildFx,
    /// Its words, as the word lanes name them.
    pub words: Vec<WordRef>,
}

/// The track's gildings heard on its playing clips, in time order.
pub fn track_gildings(s: &Session, track: TrackId) -> Vec<TrackGild> {
    let Some(t) = s.track(track) else { return Vec::new() };
    let mut out = Vec::new();
    for c in t.clips().iter().filter(|c| c.is_audio() && !c.muted) {
        for g in &c.gild {
            let Some((a, b)) = g.on_timeline(c) else { continue };
            let mut words = Vec::new();
            for (li, l) in c.lyrics.iter().enumerate() {
                for (wi, w) in l.words.iter().enumerate() {
                    if (w.id != 0 && g.words.contains(&w.id)) || (g.words.is_empty() && w.start < g.end && w.end > g.start) {
                        words.push(WordRef { clip: c.id, line: li, word: wi });
                    }
                }
            }
            out.push(TrackGild { clip: c.id, id: g.id, start: a, end: b, text: g.text.clone(), held: g.held, fx: g.fx.clone(), words });
        }
    }
    out.sort_by_key(|g| g.start);
    out
}

/// The gildings the selected words are in (on `track`).
pub fn selected_gildings(app: &SoundApp, track: TrackId) -> Vec<TrackGild> {
    if app.words.track != Some(track) || app.words.selected.is_empty() {
        return Vec::new();
    }
    let s = app.engine.session();
    let words = track_words(s, track);
    let spans: Vec<(Samples, Samples)> = words.iter().filter(|w| app.words.selected.contains(&w.r)).map(|w| (w.start, w.end)).collect();
    track_gildings(s, track)
        .into_iter()
        .filter(|g| g.words.iter().any(|r| app.words.selected.contains(r)) || spans.iter().any(|(a, b)| *a < g.end && *b > g.start))
        .collect()
}

fn refs_json(gs: &[TrackGild]) -> Value {
    json!(gs.iter().map(|g| json!({"clip": g.clip.0, "id": g.id})).collect::<Vec<_>>())
}

fn words_json(refs: &[WordRef]) -> Value {
    json!(refs.iter().map(|r| json!({"clip": r.clip.0, "line": r.line, "word": r.word})).collect::<Vec<_>>())
}

fn hook<T: Clone + Send + Sync + 'static>(ctx: &egui::Context, name: &str, v: T) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(format!("soundcraft-gild-{name}")), v));
}

fn report(app: &mut SoundApp, r: Result<Value, String>) -> Option<Value> {
    match r {
        Ok(v) => Some(v),
        Err(e) => {
            app.ui.status = e;
            None
        }
    }
}

/// One control of an effect's card.
enum Ctl {
    /// key, label, min, max, unit, shown × scale, decimals.
    Num(&'static str, &'static str, f64, f64, &'static str, f64, usize),
    /// key, label, text on, text off.
    Toggle(&'static str, &'static str, &'static str, &'static str),
    /// key, label, (value, label)s.
    Choice(&'static str, &'static str, &'static [(&'static str, &'static str)]),
}

const SPACES: &[(&str, &str)] = &[("room", "Room"), ("hall", "Hall"), ("plate", "Plate"), ("cloud", "Cloud")];
const NOTES: &[(&str, &str)] = &[
    ("1/16", "1/16"),
    ("1/8t", "1/8 triplet"),
    ("1/8", "1/8"),
    ("1/8d", "dotted 1/8"),
    ("1/4t", "1/4 triplet"),
    ("1/4", "1/4"),
    ("1/4d", "dotted 1/4"),
    ("1/2", "1/2"),
    ("1/1", "whole bar"),
];

fn controls(kind: &str) -> Vec<Ctl> {
    use Ctl::*;
    match kind {
        "shift" => vec![
            Num("semitones", "Pitch", -12.0, 12.0, " st", 1.0, 0),
            Num("formant", "Throat", -6.0, 6.0, " st", 1.0, 1),
            Toggle("layer", "Mode", "Layered", "In place"),
            Num("level_db", "Level", -24.0, 6.0, " dB", 1.0, 1),
        ],
        "grit" => vec![
            Num("drive", "Drive", 0.0, 1.0, "%", 100.0, 0),
            Choice("flavour", "Character", &[("warm", "Warm"), ("bright", "Bright"), ("hard", "Hard")]),
            Num("blend", "Blend", 0.0, 1.0, "%", 100.0, 0),
            Num("level_db", "Level", -12.0, 6.0, " dB", 1.0, 1),
        ],
        "double" => vec![
            Num("cents", "Detune", 2.0, 30.0, " c", 1.0, 0),
            Num("ms", "Late by", 5.0, 60.0, " ms", 1.0, 0),
            Num("level_db", "Level", -24.0, 0.0, " dB", 1.0, 1),
        ],
        "delay" => vec![
            Toggle("sync", "Time", "Tempo", "Free"),
            Choice("note", "Note", NOTES),
            Num("ms", "Free time", 30.0, 1500.0, " ms", 1.0, 0),
            Num("repeats", "Repeats", 0.0, 0.9, "%", 100.0, 0),
            Num("low_cut", "Low cut", 40.0, 1000.0, " Hz", 1.0, 0),
            Num("high_cut", "High cut", 1500.0, 18000.0, " Hz", 1.0, 0),
            Toggle("bounce", "Bounce", "Left–right", "Centre"),
            Num("level_db", "Level", -36.0, 6.0, " dB", 1.0, 1),
        ],
        "reverb" => vec![
            Choice("space", "Space", SPACES),
            Num("size", "Size", 0.0, 1.0, "%", 100.0, 0),
            Num("decay_s", "Decay", 0.4, 12.0, " s", 1.0, 1),
            Num("pre_ms", "Pre-delay", 0.0, 200.0, " ms", 1.0, 0),
            Num("width", "Width", 0.0, 1.5, "%", 100.0, 0),
            Num("level_db", "Level", -36.0, 6.0, " dB", 1.0, 1),
        ],
        "swell" => vec![
            Num("length_s", "Length", 0.3, 4.0, " s", 1.0, 1),
            Num("offset_ms", "Ends at", -400.0, 100.0, " ms", 1.0, 0),
            Choice("space", "Space", SPACES),
            Num("level_db", "Level", -24.0, 6.0, " dB", 1.0, 1),
        ],
        _ => Vec::new(),
    }
}

/// Runs `gild.set` on `gs` with `fx`; while a drag lasts, merged into one undo step.
fn set(app: &mut SoundApp, gs: &[TrackGild], fx: Value, drag: Option<&str>) {
    let p = json!({"gildings": refs_json(gs), "fx": fx});
    let r = match drag {
        Some(key) => app.engine.execute_merged("gild.set", &p, key).map_err(|e| e.to_string()),
        None => app.run("gild.set", p),
    };
    report(app, r);
}

/// The Gild panel in `rect` for the Vocal Editor's track.
pub fn panel(app: &mut SoundApp, ui: &mut Ui, rect: Rect, track: TrackId) {
    let t = Tokens::current();
    ui.painter().rect_filled(rect, 0.0, t.panel_bg2);
    ui.painter().line_segment([rect.left_top(), rect.left_bottom()], Stroke::new(1.0, t.border));
    let inner = rect.shrink2(vec2(14.0, 10.0));
    let mut ui = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::LEFT)));
    ui.set_clip_rect(rect.intersect(ui.clip_rect()));
    ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
    hook(ui.ctx(), "panel", rect);
    egui::ScrollArea::vertical().id_salt(("gild-panel", track.0)).auto_shrink([false, false]).show(&mut ui, |ui| {
        ui.label(RichText::new("GILD").font(bold(13.0)).color(t.header_text))
            .on_hover_text("Effects on chosen words: pick words in the word lane, then turn effects on");
        track_row(app, ui, track);
        let s = app.engine.session_arc();
        if track_words(&s, track).is_empty() {
            let msg = "Time the words first: Gild works on the words of the lyric, timed on this track.";
            ui.add_space(6.0);
            ui.label(RichText::new(msg).font(regular(12.0)).color(t.text));
            hook(ui.ctx(), "message", msg.to_string());
            ui.horizontal(|ui| {
                if ui
                    .button("Time Words to Vocal")
                    .on_hover_text("Spread the typed lines placed on the Lyrics ruler over this track's notes")
                    .clicked()
                {
                    match app.run("lyrics.time_words", json!({"track": track.0})) {
                        Ok(v) => app.ui.status = format!("{} words timed: check the guessed ones", v["words"]),
                        Err(e) => app.ui.status = e,
                    }
                }
                if ui.button("Tap Words\u{2026}").on_hover_text("Tap each word in as the song plays").clicked() {
                    let r = app.run("ui.lyrics_tap", json!({}));
                    report(app, r);
                }
            });
            return;
        }
        hook(ui.ctx(), "message", String::new());
        select_row(app, ui, track);
        let sel = selected_gildings(app, track);
        let nwords = if app.words.track == Some(track) { app.words.selected.len() } else { 0 };
        let summary = match (nwords, sel.len()) {
            (0, _) => "No words selected \u{b7} click a word in the word lane (Shift: a phrase, Ctrl: separate words)".to_string(),
            (n, 0) => format!("{n} word{} selected \u{b7} not gilded: turn an effect on", if n == 1 { "" } else { "s" }),
            (n, 1) => format!("{n} word{} \u{b7} \u{201c}{}\u{201d}", if n == 1 { "" } else { "s" }, sel[0].text),
            (n, k) => format!("{n} words \u{b7} {k} gildings (changed together)"),
        };
        ui.label(RichText::new(&summary).font(regular(12.0)).color(t.text_dim));
        hook(ui.ctx(), "summary", summary);
        effect_buttons(app, ui, track, &sel, nwords > 0);
        if !sel.is_empty() {
            let fx = sel[0].fx.clone();
            for kind in GILD_KINDS.iter().filter(|k| fx.has(k)) {
                card(app, ui, &sel, kind, &fx);
            }
            if fx.has("harmony") {
                harmony_card(app, ui, &sel, &fx);
            }
            span_row(app, ui, &sel);
        }
        action_rows(app, ui, track, &sel, nwords > 0);
    });
    confirm_clear(app, ui.ctx(), track);
}

fn track_row(app: &mut SoundApp, ui: &mut Ui, track: TrackId) {
    let t = Tokens::current();
    let Some(g) = app.engine.session().track(track).map(|t| t.gild) else { return };
    ui.horizontal(|ui| {
        let dry = ui
            .add(
                egui::Button::new(RichText::new(if g.dry { "Dry on" } else { "Dry off" }).color(if g.dry { t.text } else { t.mode_on_text }))
                    .fill(if g.dry { t.button } else { t.mode_on }),
            )
            .on_hover_text("Off: this track gives only what Gild adds (put Gild on a duplicate track and mix it on its own)");
        hook(ui.ctx(), "dry", dry.rect);
        if dry.clicked() {
            let r = app.run("gild.track", json!({"track": track.0, "dry": !g.dry}));
            report(app, r);
        }
        let mut lv = g.level_db;
        let s = ui.add(egui::Slider::new(&mut lv, -24.0..=12.0).suffix(" dB").fixed_decimals(1).text("Gild level"));
        hook(ui.ctx(), "level", s.rect);
        if s.changed() {
            let p = json!({"track": track.0, "level_db": lv});
            let r = if s.dragged() {
                app.engine.execute_merged("gild.track", &p, "gild-track-level").map_err(|e| e.to_string())
            } else {
                app.run("gild.track", p)
            };
            report(app, r);
        }
    });
}

fn select_row(app: &mut SoundApp, ui: &mut Ui, track: TrackId) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Select:").font(regular(11.5)));
        if ui.small_button("Last of lines").on_hover_text("The last word of every line").clicked() {
            crate::word_lane::select_which(app, track, "last_of_line", "");
        }
        if ui.small_button("First of lines").on_hover_text("The first word of every line").clicked() {
            crate::word_lane::select_which(app, track, "first_of_line", "");
        }
        let s = app.engine.session_arc();
        let first = app.words.selected.first().and_then(|r| track_words(&s, track).into_iter().find(|w| w.r == *r)).map(|w| w.text);
        if let Some(w) = first {
            let shown = w.trim_matches(|c: char| !c.is_alphanumeric()).to_string();
            if ui.small_button(format!("Every \u{201c}{shown}\u{201d}")).on_hover_text("Every time this word is sung on the track").clicked() {
                crate::word_lane::select_which(app, track, "same_word", &w);
            }
        }
    });
}

/// The effect buttons: on when every selected gilding has the effect.
fn effect_buttons(app: &mut SoundApp, ui: &mut Ui, track: TrackId, sel: &[TrackGild], any_words: bool) {
    let t = Tokens::current();
    let mut rects = Vec::new();
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for kind in GILD_KINDS {
            let on = !sel.is_empty() && sel.iter().all(|g| g.fx.has(kind));
            let label = format!("{} {}", kind_mark(kind), kind_label(kind));
            let b = ui
                .add_enabled(
                    any_words,
                    egui::Button::new(RichText::new(label).color(if on { t.mode_on_text } else { t.text })).fill(if on {
                        t.mode_on
                    } else {
                        t.button
                    }),
                )
                .on_hover_text(kind_help(kind))
                .on_disabled_hover_text("Select words in the word lane first");
            rects.push((kind.to_string(), b.rect, on));
            if b.clicked() {
                toggle(app, track, sel, kind, on);
            }
        }
    });
    hook(ui.ctx(), "buttons", rects);
}

fn toggle(app: &mut SoundApp, track: TrackId, sel: &[TrackGild], kind: &str, on: bool) {
    let mut fx = serde_json::Map::new();
    fx.insert(kind.to_string(), json!(!on));
    // Words not gilded yet: gilded with this effect.
    let s = app.engine.session_arc();
    let covered: Vec<WordRef> = sel.iter().flat_map(|g| g.words.iter().copied()).collect();
    let words = track_words(&s, track);
    let bare: Vec<WordRef> = app
        .words
        .selected
        .iter()
        .copied()
        .filter(|r| !covered.contains(r) && !sel.iter().any(|g| words.iter().any(|w| w.r == *r && w.start < g.end && w.end > g.start)))
        .collect();
    if !on && !bare.is_empty() {
        let r = app.run("gild.add", json!({"track": track.0, "words": words_json(&bare), "fx": Value::Object(fx.clone())}));
        report(app, r);
    }
    if sel.is_empty() {
        return;
    }
    // Off on the last effect of a gilding: the gilding goes.
    if on {
        let (last, rest): (Vec<TrackGild>, Vec<TrackGild>) = sel.iter().cloned().partition(|g| g.fx.kinds() == vec![kind]);
        if !last.is_empty() {
            let r = app.run("gild.remove", json!({"gildings": refs_json(&last)}));
            report(app, r);
        }
        if !rest.is_empty() {
            set(app, &rest, Value::Object(fx), None);
        }
    } else {
        set(app, sel, Value::Object(fx), None);
    }
}

fn card_frame(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) -> Rect {
    let t = Tokens::current();
    let r = egui::Frame::new()
        .fill(t.panel_bg)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).font(bold(11.5)).color(GOLD));
            add(ui);
        })
        .response
        .rect;
    ui.add_space(3.0);
    r
}

fn card(app: &mut SoundApp, ui: &mut Ui, sel: &[TrackGild], kind: &str, fx: &GildFx) {
    if kind == "harmony" {
        return;
    }
    let v = serde_json::to_value(fx).unwrap_or(Value::Null);
    let cur = v.get(kind).cloned().unwrap_or(Value::Null);
    let title = format!("{}  {}", kind_mark(kind), kind_label(kind).to_uppercase());
    let rect = card_frame(ui, &title, |ui| {
        let sync = cur.get("sync").and_then(Value::as_bool).unwrap_or(true);
        let layer = cur.get("layer").and_then(Value::as_bool).unwrap_or(true);
        for c in controls(kind) {
            match c {
                // A delay's note shows when it follows the tempo, its free time when not; an
                // in-place shift has no level of its own.
                Ctl::Choice("note", ..) if kind == "delay" && !sync => continue,
                Ctl::Num("ms", ..) if kind == "delay" && sync => continue,
                Ctl::Num("level_db", ..) if kind == "shift" && !layer => continue,
                Ctl::Num(key, label, lo, hi, unit, scale, dec) => {
                    let mut x = cur.get(key).and_then(Value::as_f64).unwrap_or(lo) * scale;
                    let was = x;
                    let s = ui.add(
                        egui::Slider::new(&mut x, lo * scale..=hi * scale)
                            .suffix(unit)
                            .fixed_decimals(dec)
                            .logarithmic(matches!(key, "low_cut" | "high_cut" | "decay_s"))
                            .text(label),
                    );
                    // A slider settling a value onto its own steps is not a change.
                    if s.changed() && (x - was).abs() > (hi - lo) * scale * 1e-4 {
                        let drag = format!("gild-{kind}-{key}");
                        set(app, sel, json!({kind: {key: x / scale}}), s.dragged().then_some(drag.as_str()));
                    }
                }
                Ctl::Toggle(key, label, on, off) => {
                    let val = cur.get(key).and_then(Value::as_bool).unwrap_or(false);
                    ui.horizontal(|ui| {
                        ui.label(label);
                        if ui.selectable_label(val, on).clicked() && !val {
                            set(app, sel, json!({kind: {key: true}}), None);
                        }
                        if ui.selectable_label(!val, off).clicked() && val {
                            set(app, sel, json!({kind: {key: false}}), None);
                        }
                    });
                }
                Ctl::Choice(key, label, opts) => {
                    let val = cur.get(key).and_then(Value::as_str).unwrap_or("").to_string();
                    let shown = opts.iter().find(|o| o.0 == val).map_or(val.as_str(), |o| o.1).to_string();
                    egui::ComboBox::from_label(label).selected_text(shown).show_ui(ui, |ui| {
                        for (o, l) in opts {
                            if ui.selectable_label(val == *o, *l).clicked() && val != *o {
                                set(app, sel, json!({kind: {key: o}}), None);
                            }
                        }
                    });
                }
            }
        }
    });
    let mut cards: Vec<(String, Rect)> = ui.ctx().data(|d| d.get_temp(egui::Id::new("soundcraft-gild-cards"))).unwrap_or_default();
    cards.retain(|c| c.0 != kind);
    cards.push((kind.to_string(), rect));
    hook(ui.ctx(), "cards", cards);
}

/// Harmony's card: up to three voices, each scale steps, level and pan.
fn harmony_card(app: &mut SoundApp, ui: &mut Ui, sel: &[TrackGild], fx: &GildFx) {
    let Some(h) = fx.harmony.clone() else { return };
    const STEPS: [(i8, &str); 8] = [
        (-7, "octave below"),
        (-4, "5th below"),
        (-2, "3rd below"),
        (2, "3rd above"),
        (3, "4th above"),
        (4, "5th above"),
        (5, "6th above"),
        (7, "octave above"),
    ];
    card_frame(ui, &format!("{}  HARMONY", kind_mark("harmony")), |ui| {
        const NAMES: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
        let key = h
            .key
            .map(|[k, sc]| format!("{} {}", NAMES.get(usize::from(k % 12)).copied().unwrap_or("C"), if sc == 0 { "major" } else { "minor" }))
            .unwrap_or_else(|| "the song's key".into());
        ui.label(RichText::new(format!("in {key} (Key ruler)")).font(regular(11.0)));
        let mut voices = h.voices.clone();
        let mut changed = None;
        for (i, v) in voices.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let shown = STEPS.iter().find(|s| s.0 == v.steps).map_or_else(|| format!("{} steps", v.steps), |s| s.1.to_string());
                egui::ComboBox::from_id_salt(("gild-voice", i)).selected_text(shown).width(96.0).show_ui(ui, |ui| {
                    for (st, l) in STEPS {
                        if ui.selectable_label(v.steps == st, l).clicked() && v.steps != st {
                            v.steps = st;
                            changed = Some(false);
                        }
                    }
                });
                if ui.small_button("\u{d7}").on_hover_text("Remove this voice").clicked() && h.voices.len() > 1 {
                    v.steps = i8::MIN;
                    changed = Some(false);
                }
            });
            let s = ui.add(egui::Slider::new(&mut v.level_db, -24.0..=6.0).suffix(" dB").fixed_decimals(1).text("Level"));
            if s.changed() {
                changed = Some(s.dragged());
            }
            let p = ui.add(egui::Slider::new(&mut v.pan, -1.0..=1.0).fixed_decimals(2).text("Pan"));
            if p.changed() {
                changed = Some(p.dragged());
            }
        }
        voices.retain(|v| v.steps != i8::MIN);
        if voices.len() < soundcraft_model::gild::MAX_HARMONY_VOICES && ui.small_button("+ Voice").clicked() {
            voices.push(soundcraft_model::GildVoice { steps: 4, ..Default::default() });
            changed = Some(false);
        }
        if let Some(drag) = changed {
            set(app, sel, json!({"harmony": {"voices": voices}}), drag.then_some("gild-harmony"));
        }
    });
}

/// One gilding: its span, Audition, follow its words again.
fn span_row(app: &mut SoundApp, ui: &mut Ui, sel: &[TrackGild]) {
    let [g] = sel else { return };
    let t = Tokens::current();
    let rate = app.engine.session().sample_rate.as_f64().max(1.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{:.2}\u{2013}{:.2} s", g.start as f64 / rate, g.end as f64 / rate)).font(regular(11.5)).color(t.text_dim));
        let a = ui.button("Audition").on_hover_text("Play the span and its tail");
        hook(ui.ctx(), "audition", a.rect);
        if a.clicked() {
            let r = app.run("gild.audition", json!({"clip": g.clip.0, "id": g.id}));
            report(app, r);
        }
        if g.held && ui.small_button("Follow words").on_hover_text("Its handles were moved by hand: let it follow its words again").clicked() {
            let r = app.run("gild.set", json!({"clip": g.clip.0, "id": g.id, "held": false}));
            report(app, r);
        }
    });
    let st = soundcraft_model::gild::render_key;
    let s = app.engine.session_arc();
    if let Some((_, c)) = s.find_clip(g.clip)
        && let Some(gg) = c.gild.iter().find(|x| x.id == g.id)
        && let Some(k) = st(c, gg, s.sample_rate.hz())
        && !s.gild.renders.get(&(g.clip, g.id)).is_some_and(|r| r.key == k)
    {
        ui.horizontal(|ui| {
            let why = s.gild.failed.get(&k).cloned();
            ui.label(
                RichText::new(why.as_deref().unwrap_or("Making Shift / Grit / Double / Swell-in\u{2026}")).font(regular(11.0)).color(t.text_dim),
            );
            if why.is_none() && ui.small_button("Now").on_hover_text("Make it now rather than in the background").clicked() {
                let r = app.run("gild.render", json!({}));
                report(app, r);
            }
        });
    }
}

fn action_rows(app: &mut SoundApp, ui: &mut Ui, track: TrackId, sel: &[TrackGild], any_words: bool) {
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        let copy = ui.add_enabled(sel.len() == 1, egui::Button::new("Copy Gild")).on_disabled_hover_text("Select one gilding's words");
        if copy.clicked() {
            let r = app.run("gild.copy", json!({"clip": sel[0].clip.0, "id": sel[0].id}));
            if report(app, r).is_some() {
                app.ui.status = format!("Gild copied: {}", sel[0].fx.kinds().iter().map(|k| kind_label(k)).collect::<Vec<_>>().join(", "));
            }
        }
        let has_copy = app.engine.gild_copy.is_some();
        let paste =
            ui.add_enabled(has_copy && any_words, egui::Button::new("Paste Gild")).on_disabled_hover_text("Copy a gilding, then select words");
        hook(ui.ctx(), "paste", paste.rect);
        if paste.clicked() {
            let words = words_json(&app.words.selected);
            let r = app.run("gild.paste", json!({"track": track.0, "words": words}));
            report(app, r);
        }
        let rm = ui.add_enabled(!sel.is_empty(), egui::Button::new("Remove")).on_hover_text("Take Gild off the selected words");
        if rm.clicked() {
            let r = app.run("gild.remove", json!({"gildings": refs_json(sel)}));
            report(app, r);
        }
    });
    presets_menu(app, ui, track, sel, any_words);
    ui.add_space(6.0);
    let any = app.engine.session().track(track).is_some_and(|t| t.playlists.iter().any(|p| p.clips.iter().any(|c| !c.gild.is_empty())));
    let all = ui.add_enabled(any, egui::Button::new("Remove All Gild on This Track\u{2026}"));
    hook(ui.ctx(), "remove-all", all.rect);
    if all.clicked() {
        app.vocal.gild_confirm = true;
    }
}

/// The presets: a whole set, or one effect's settings.
fn presets_menu(app: &mut SoundApp, ui: &mut Ui, track: TrackId, sel: &[TrackGild], any_words: bool) {
    let list = app.run("gild.presets", json!({})).ok().and_then(|v| v["presets"].as_array().cloned()).unwrap_or_default();
    ui.add_enabled_ui(any_words, |ui| {
        egui::ComboBox::from_id_salt("gild-presets").selected_text("Presets\u{2026}").width(160.0).show_ui(ui, |ui| {
            for p in &list {
                let (name, kind) = (p["name"].as_str().unwrap_or(""), p["kind"].as_str().unwrap_or(""));
                let shown = if kind == "gilding" { format!("{name} (set)") } else { format!("{name} \u{b7} {}", kind_label(kind)) };
                if ui.selectable_label(false, shown).clicked() {
                    apply_preset(app, track, sel, name, kind, &p["fx"]);
                }
            }
        });
    });
}

fn apply_preset(app: &mut SoundApp, track: TrackId, sel: &[TrackGild], name: &str, kind: &str, fx: &Value) {
    if sel.is_empty() {
        let words = words_json(&app.words.selected);
        let r = app.run("gild.add", json!({"track": track.0, "words": words, "preset": name}));
        report(app, r);
        return;
    }
    let mut patch = fx.as_object().cloned().unwrap_or_default();
    if kind == "gilding" {
        // A whole set: what it doesn't have goes off.
        for k in GILD_KINDS {
            patch.entry(k.to_string()).or_insert(Value::Bool(false));
        }
    }
    set(app, sel, Value::Object(patch), None);
}

/// Remove All Gild on This Track: asks first.
fn confirm_clear(app: &mut SoundApp, ctx: &egui::Context, track: TrackId) {
    if !app.vocal.gild_confirm {
        return;
    }
    let n: usize = app.engine.session().track(track).map_or(0, |t| t.playlists.iter().flat_map(|p| p.clips.iter()).map(|c| c.gild.len()).sum());
    hook(ctx, "confirm", n);
    egui::Modal::new(egui::Id::new("gild_clear_confirm")).show(ctx, |ui| {
        ui.label(RichText::new("Remove all Gild on this track?").font(bold(13.0)));
        ui.label(format!("{n} gilding{} will be removed (Edit \u{203a} Undo brings them back).", if n == 1 { "" } else { "s" }));
        ui.horizontal(|ui| {
            if ui.button("Remove All").clicked() {
                let r = app.run("gild.clear_track", json!({"track": track.0}));
                report(app, r);
                app.vocal.gild_confirm = false;
            }
            if ui.button("Cancel").clicked() {
                app.vocal.gild_confirm = false;
            }
        });
    });
}

/// A drag of a gilding's handle under way: (clip, id, which end, timeline position now).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HandleDrag {
    pub clip: ClipId,
    pub id: u32,
    pub end: bool,
    pub at: Samples,
}

/// The Gild strip under the word lane: each gilding a bar with its effects' marks; a single
/// selected gilding has a handle at each end. `map` is the word lane's time axis.
pub fn strip(app: &mut SoundApp, ui: &mut Ui, track: TrackId, rect: Rect, map: crate::word_lane::TimeMap) {
    let t = Tokens::current();
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    p.rect_filled(rect, 0.0, t.panel_bg2);
    let gs = track_gildings(app.engine.session(), track);
    let sel = selected_gildings(app, track);
    let one = if let [g] = sel.as_slice() { Some((g.clip, g.id)) } else { None };
    let drag = app.vocal.gild_drag;
    let mut bars: Vec<(u32, Rect, String)> = Vec::new();
    for g in &gs {
        let (mut a, mut b) = (g.start, g.end);
        if let Some(d) = drag.filter(|d| d.clip == g.clip && d.id == g.id) {
            if d.end { b = d.at.max(a + 1) } else { a = d.at.min(b - 1) }
        }
        let r = Rect::from_min_max(pos2(map.x(a), rect.top() + 2.0), pos2(map.x(b).max(map.x(a) + 3.0), rect.bottom() - 2.0));
        if r.right() < rect.left() || r.left() > rect.right() {
            continue;
        }
        let picked = sel.iter().any(|s| s.clip == g.clip && s.id == g.id);
        p.rect_filled(r, CornerRadius::same(2), if picked { GOLD } else { GOLD.gamma_multiply(0.55) });
        let marks = g.fx.kinds().iter().map(|k| kind_mark(k)).collect::<Vec<_>>().join(" ");
        p.with_clip_rect(r).text(pos2(r.left() + 3.0, r.center().y), Align2::LEFT_CENTER, &marks, regular(9.5), Color32::from_rgb(30, 24, 6));
        if one == Some((g.clip, g.id)) {
            for x in [r.left(), r.right()] {
                let h = Rect::from_center_size(pos2(x, r.center().y), vec2(5.0, rect.height()));
                p.rect_filled(h, CornerRadius::same(1), t.text);
                p.rect_stroke(h, CornerRadius::same(1), Stroke::new(1.0, t.panel_bg), StrokeKind::Inside);
            }
        }
        bars.push((g.id, r, marks));
    }
    hook(ui.ctx(), "strip", rect);
    hook(ui.ctx(), "bars", bars.clone());
    let resp = ui.interact(rect, ui.id().with(("gild-strip", track.0)), Sense::click_and_drag());
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let hit_bar = |x: f32| gs.iter().zip(&bars).find(|(_, b)| b.1.left() <= x && x <= b.1.right()).map(|(g, _)| g.clone());
    if let Some(pos) = resp.hover_pos()
        && let Some(g) = hit_bar(pos.x)
    {
        let kinds = g.fx.kinds().iter().map(|k| kind_label(k)).collect::<Vec<_>>().join(", ");
        resp.clone().on_hover_text(format!("\u{201c}{}\u{201d}: {kinds}", g.text));
    }
    if resp.drag_started()
        && let Some(pos) = ui.input(|i| i.pointer.press_origin())
        && let [g] = sel.as_slice()
    {
        let (xa, xb) = (map.x(g.start), map.x(g.end));
        if (pos.x - xa).abs() <= 6.0 || (pos.x - xb).abs() <= 6.0 {
            let end = (pos.x - xb).abs() < (pos.x - xa).abs();
            app.vocal.gild_drag = Some(HandleDrag { clip: g.clip, id: g.id, end, at: if end { g.end } else { g.start } });
        }
    }
    if let (Some(d), Some(pos)) = (app.vocal.gild_drag.as_mut(), pointer) {
        d.at = map.at(pos.x);
    }
    if resp.drag_stopped()
        && let Some(d) = app.vocal.gild_drag.take()
    {
        let key = if d.end { "end" } else { "start" };
        let r = app.run("gild.set", json!({"clip": d.clip.0, "id": d.id, key: d.at}));
        report(app, r);
    }
    if resp.clicked()
        && let Some(pos) = resp.interact_pointer_pos()
        && let Some(g) = hit_bar(pos.x)
    {
        crate::word_lane::select(app, track, g.words.clone(), true, false);
    }
}
