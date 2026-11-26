//! A lane of timed words (the Vocal Editor's word lane and the track word strip in the Edit
//! window): each word a box from its start to where its voice stops, guesses drawn dashed and dim,
//! confirmed words solid, the word being sung lit while the song plays.
//!
//! Editing, all through engine commands (one undo step each): drag an edge (a touching
//! neighbour's edge follows; Alt moves only this edge; edges snap to note starts and ends and
//! breath edges, Ctrl/Cmd turns snapping off), drag the body to move the word, double-click to
//! retype, Delete removes, Alt+←/→ nudges by 10 ms (Shift: 50 ms), the right-click menu splits,
//! merges, auditions, inserts a word at the playhead, confirms, and selects by the shape of the
//! lyric (first or last word of every line, every occurrence of a word, find). A click selects a
//! word: its time becomes the edit selection and the playhead goes to it (in the Vocal Editor its
//! notes are selected too).

use crate::SoundApp;
use crate::theme::{Tokens, regular};
use egui::{Align2, Color32, CornerRadius, Key, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_engine::word_time::{TrackWord, WordRef, track_words};
use soundcraft_model::{TrackId, WordTiming};
use soundcraft_time::Samples;

/// The Vocal Editor's word lane height.
pub const LANE_H: f32 = 24.0;
/// How close (px) to a word's edge a drag takes the edge.
const EDGE_PX: f32 = 5.0;

/// Which part of a word a drag holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Start,
    End,
    Body,
}

/// A word drag under way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WordDrag {
    pub track: TrackId,
    pub r: WordRef,
    pub part: Part,
    /// The word's times when the drag began.
    pub start: Samples,
    pub end: Samples,
    /// Pointer x when it began.
    pub x0: f32,
    /// The time the pointer is at now.
    pub now: Samples,
}

/// The word lanes' state: the selected words (on one track), a drag, a word being retyped.
#[derive(Debug, Clone, Default)]
pub struct WordUi {
    pub track: Option<TrackId>,
    pub selected: Vec<WordRef>,
    pub drag: Option<WordDrag>,
    pub editing: Option<(WordRef, String)>,
    /// The lane the word is retyped in (a word shows in more than one).
    pub editing_lane: String,
    /// The Find field of the lane menu.
    pub find: String,
    pub tap: crate::lyrics_tap::TapState,
}

/// A lane's time axis: `x = left + (at - t0) / spp`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeMap {
    pub left: f32,
    pub t0: Samples,
    pub spp: f64,
}

impl TimeMap {
    pub fn x(&self, at: Samples) -> f32 {
        self.left + ((at - self.t0) as f64 / self.spp.max(1e-6)) as f32
    }

    pub fn at(&self, x: f32) -> Samples {
        self.t0 + soundcraft_time::to_samples(f64::from(x - self.left) * self.spp)
    }
}

/// How a word is drawn.
pub fn word_colours(timing: WordTiming, lit: bool, dark: bool) -> (Color32, Color32) {
    let t = Tokens::current();
    if lit {
        return (Color32::from_rgb(242, 196, 64), Color32::from_rgb(24, 20, 8));
    }
    match timing {
        WordTiming::Confirmed => (t.accent.gamma_multiply(0.85), Color32::WHITE),
        WordTiming::Guess => (if dark { Color32::from_rgb(58, 60, 70) } else { Color32::from_rgb(214, 216, 224) }, t.text),
        WordTiming::Heard => (if dark { Color32::from_rgb(70, 74, 84) } else { Color32::from_rgb(196, 200, 210) }, t.text),
    }
}

/// The words drawn in a lane last frame (for tests and the screenshots): each ref, its box, text
/// and timing, whether it is lit and selected.
#[derive(Debug, Clone, PartialEq)]
pub struct Drawn {
    pub r: WordRef,
    pub rect: Rect,
    pub text: String,
    pub timing: WordTiming,
    pub lit: bool,
    pub selected: bool,
}

fn store<T: Clone + Send + Sync + 'static>(ctx: &egui::Context, name: &str, v: T) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(format!("soundcraft-words-{name}")), v));
}

/// The word being sung at `at` on the track's words.
pub fn word_at(words: &[TrackWord], at: Samples) -> Option<WordRef> {
    words.iter().find(|w| w.start <= at && at < w.end).map(|w| w.r)
}

/// Runs a word command and reports a refusal in the status line.
fn run(app: &mut SoundApp, id: &str, p: Value) -> Option<Value> {
    match app.run(id, p) {
        Ok(v) => Some(v),
        Err(e) => {
            app.ui.status = e;
            None
        }
    }
}

fn ref_json(r: WordRef) -> Value {
    json!({"clip": r.clip.0, "line": r.line, "word": r.word})
}

fn ref_of(v: &Value) -> Option<WordRef> {
    Some(WordRef {
        clip: soundcraft_model::ClipId(v.get("clip")?.as_u64()?),
        line: usize::try_from(v.get("line")?.as_u64()?).ok()?,
        word: usize::try_from(v.get("word")?.as_u64()?).ok()?,
    })
}

/// Selects `refs` (on `track`): their span becomes the edit selection, the playhead goes to the
/// first; with `notes`, the Vocal Editor selects the notes under them.
pub fn select(app: &mut SoundApp, track: TrackId, refs: Vec<WordRef>, notes: bool, play: bool) {
    app.words.track = Some(track);
    app.words.selected = refs.clone();
    if refs.is_empty() {
        return;
    }
    let words: Vec<Value> = refs.iter().map(|r| ref_json(*r)).collect();
    let Some(v) = run(app, "lyrics.select_words", json!({"track": track.0, "words": words, "play": play})) else { return };
    if notes && let (Some(a), Some(b)) = (v["start"].as_i64(), v["end"].as_i64()) {
        let range = soundcraft_time::Range::new(a, b);
        let ns = soundcraft_engine::vocal::track_notes(app.engine.session(), track, Some(range));
        app.vocal.selected = ns.into_iter().collect();
    }
}

/// Selects the words `lyrics.select_words` picks with `which` (and `text`).
pub fn select_which(app: &mut SoundApp, track: TrackId, which: &str, text: &str) {
    if let Some(v) = run(app, "lyrics.select_words", json!({"track": track.0, "which": which, "text": text})) {
        app.words.track = Some(track);
        app.words.selected = v["words"].as_array().map(|a| a.iter().filter_map(ref_of).collect()).unwrap_or_default();
        if app.words.selected.is_empty() {
            app.ui.status = "No words match".into();
        }
    }
}

/// Draws and runs the lane `name` of `track`'s words in `rect` on the time axis `map`. With
/// `caption`, the lane is labelled ("Words", and how many are still guessed); with `notes`,
/// selecting words selects their notes (the Vocal Editor).
pub fn lane(app: &mut SoundApp, ui: &mut Ui, name: &str, track: TrackId, rect: Rect, map: TimeMap, caption: bool, notes: bool) {
    let t = Tokens::current();
    let dark = u32::from(t.panel_bg.r()) + u32::from(t.panel_bg.g()) + u32::from(t.panel_bg.b()) < 384;
    let s = app.engine.session_arc();
    let words = track_words(&s, track);
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 0.0, if dark { Color32::from_rgb(28, 29, 33) } else { t.panel_bg2 });
    let playing = app.engine.transport.playing;
    let lit = if playing { word_at(&words, app.position()) } else { None };
    let sel: Vec<WordRef> = if app.words.track == Some(track) { app.words.selected.clone() } else { Vec::new() };
    let drag = app.words.drag.filter(|d| d.track == track);
    let font = regular(if rect.height() >= 20.0 { 12.0 } else { 10.0 });
    let mut drawn: Vec<Drawn> = Vec::new();
    let gilded = crate::gild_panel::track_gildings(&s, track);
    let mut gilded_rects: Vec<(Rect, String)> = Vec::new();
    for w in &words {
        // A word being dragged shows where it will go.
        let (a, b) = match drag {
            Some(d) if d.r == w.r => match d.part {
                Part::Start => (d.now.min(d.end - 1), d.end),
                Part::End => (d.start, d.now.max(d.start + 1)),
                Part::Body => (d.start + (d.now - map.at(d.x0)), d.end + (d.now - map.at(d.x0))),
            },
            _ => (w.start, w.end),
        };
        let (x0, x1) = (map.x(a), map.x(b).max(map.x(a) + 3.0));
        if x1 < rect.left() || x0 > rect.right() {
            continue;
        }
        let r = Rect::from_min_max(pos2(x0, rect.top() + 2.0), pos2(x1, rect.bottom() - 2.0));
        let is_lit = lit == Some(w.r);
        let is_sel = sel.contains(&w.r);
        let (fill, ink) = word_colours(w.timing, is_lit, dark);
        painter.rect_filled(r, CornerRadius::same(3), fill);
        if w.timing == WordTiming::Guess && !is_lit {
            // A guess: dashed outline, and a small mark at its start.
            let c = t.text_dim;
            let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
            painter.extend(Shape::dashed_line(&pts, Stroke::new(1.0, c), 3.0, 2.0));
        }
        if is_sel {
            painter.rect_stroke(r, CornerRadius::same(3), Stroke::new(2.0, if dark { Color32::WHITE } else { Color32::BLACK }), StrokeKind::Inside);
        }
        // The text, clipped to the box (a short box shows what fits).
        let tp = painter.with_clip_rect(r.intersect(rect));
        let text = if w.timing == WordTiming::Guess { format!("{}?", w.text) } else { w.text.clone() };
        tp.text(pos2(r.left() + 3.0, r.center().y), Align2::LEFT_CENTER, &text, font.clone(), ink);
        // A gilded word: a gold bar along its foot (Gild's marks are on the Gild strip and in the
        // hover text).
        if let Some(g) = gilded.iter().find(|g| g.words.contains(&w.r)) {
            let foot = Rect::from_min_max(pos2(r.left(), r.bottom() - 3.0), r.right_bottom());
            painter.rect_filled(foot, CornerRadius::same(1), crate::gild_panel::GOLD);
            gilded_rects.push((r, g.fx.kinds().iter().map(|k| crate::gild_panel::kind_label(k)).collect::<Vec<_>>().join(", ")));
        }
        drawn.push(Drawn { r: w.r, rect: r, text: w.text.clone(), timing: w.timing, lit: is_lit, selected: is_sel });
    }
    // The playhead.
    let ph = app.position();
    let px = map.x(ph);
    if px >= rect.left() && px <= rect.right() {
        painter.vline(px, rect.y_range(), Stroke::new(1.5, t.playhead));
    }
    let guessed = words.iter().filter(|w| w.timing == WordTiming::Guess).count();
    let caption_text = match guessed {
        0 => String::new(),
        1 => "1 word still guessed".to_string(),
        n => format!("{n} words still guessed"),
    };
    if caption {
        if words.is_empty() && rect.height() >= 20.0 {
            painter.text(
                pos2(rect.left() + 8.0, rect.center().y),
                Align2::LEFT_CENTER,
                "No timed words: right-click › Time Words to Vocal, or tap them in (Lyrics › Timing)",
                regular(11.0),
                t.text_dim,
            );
        }
        // The count at the lane's right end, only where no word is under it (the Vocal Editor's
        // header says it too).
        let galley = painter.layout_no_wrap(caption_text.clone(), regular(10.5), t.text_dim);
        let g = Rect::from_min_size(pos2(rect.right() - 6.0 - galley.size().x, rect.center().y - galley.size().y / 2.0), galley.size());
        if !caption_text.is_empty() && !drawn.iter().any(|d| d.rect.intersects(g.expand(3.0))) {
            painter.galley(g.min, galley, t.text_dim);
        }
    }
    // Hovering a gilded word says what is on it.
    if let Some(p) = ui.ctx().pointer_hover_pos()
        && rect.contains(p)
        && let Some((_, kinds)) = gilded_rects.iter().find(|(r, _)| r.contains(p))
    {
        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), ui.id().with(("gilded", name)), egui::PopupAnchor::Pointer)
            .show(|ui| ui.label(format!("Gild: {kinds}")));
    }
    store(ui.ctx(), &format!("{name}-gilded"), gilded_rects.iter().map(|g| g.0).collect::<Vec<_>>());
    store(ui.ctx(), &format!("{name}-drawn"), drawn.clone());
    store(ui.ctx(), &format!("{name}-rect"), rect);
    store(ui.ctx(), &format!("{name}-guessed"), caption_text.clone());
    interact(app, ui, name, track, rect, map, &drawn, notes);
}

/// The word and part under `p`.
fn hit(drawn: &[Drawn], p: Pos2) -> Option<(WordRef, Part)> {
    // Edges first (a touching pair: the nearer word).
    let mut best: Option<(f32, WordRef, Part)> = None;
    for d in drawn.iter().filter(|d| d.rect.y_range().contains(p.y)) {
        for (x, part) in [(d.rect.left(), Part::Start), (d.rect.right(), Part::End)] {
            let dx = (p.x - x).abs();
            if dx <= EDGE_PX && best.is_none_or(|b| dx < b.0) {
                best = Some((dx, d.r, part));
            }
        }
    }
    if let Some((_, r, part)) = best
        && drawn.iter().find(|d| d.r == r).is_some_and(|d| d.rect.width() >= 2.0 * EDGE_PX || part == Part::Start)
    {
        return Some((r, part));
    }
    drawn.iter().rev().find(|d| d.rect.contains(p)).map(|d| (d.r, Part::Body))
}

#[allow(clippy::too_many_arguments)]
fn interact(app: &mut SoundApp, ui: &mut Ui, name: &str, track: TrackId, rect: Rect, map: TimeMap, drawn: &[Drawn], notes: bool) {
    let id = ui.id().with(("word-lane", name, track.0));
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let m = ui.input(|i| i.modifiers);
    if let Some(p) = resp.hover_pos() {
        match hit(drawn, p) {
            Some((_, Part::Start | Part::End)) => ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal),
            Some((_, Part::Body)) => ui.ctx().set_cursor_icon(egui::CursorIcon::Grab),
            None => {}
        }
    }
    let s = app.engine.session_arc();
    let words = track_words(&s, track);
    let times = |r: WordRef| words.iter().find(|w| w.r == r).map(|w| (w.start, w.end));
    // The drag holds what was under the pointer where the button went down.
    if resp.drag_started()
        && let Some(p) = ui.input(|i| i.pointer.press_origin()).or_else(|| resp.interact_pointer_pos())
        && let Some((r, part)) = hit(drawn, p)
        && let Some((a, b)) = times(r)
    {
        app.words.drag = Some(WordDrag { track, r, part, start: a, end: b, x0: p.x, now: map.at(p.x) });
    }
    if let Some(d) = app.words.drag.as_mut().filter(|d| d.track == track)
        && let Some(p) = ui.input(|i| i.pointer.interact_pos())
    {
        d.now = map.at(p.x);
    }
    if resp.drag_stopped()
        && let Some(d) = app.words.drag.take().filter(|d| d.track == track)
    {
        let snap = !(m.command || m.ctrl);
        let alone = m.alt;
        let moved = (map.x(d.now) - d.x0).abs() >= 1.0;
        let res = match d.part {
            _ if !moved => None,
            Part::Start => run(app, "lyrics.word_set", merge(ref_json(d.r), json!({"start": d.now, "alone": alone, "snap": snap}))),
            Part::End => run(app, "lyrics.word_set", merge(ref_json(d.r), json!({"end": d.now, "alone": alone, "snap": snap}))),
            Part::Body => run(app, "lyrics.word_move", merge(ref_json(d.r), json!({"by": d.now - map.at(d.x0), "alone": alone}))),
        };
        if let Some(r) = res.as_ref().and_then(ref_of) {
            app.words.track = Some(track);
            app.words.selected = vec![r];
        }
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        match hit(drawn, p) {
            Some((r, _)) => {
                let mut refs = if app.words.track == Some(track) { app.words.selected.clone() } else { Vec::new() };
                if m.command || m.ctrl {
                    if let Some(i) = refs.iter().position(|x| *x == r) {
                        refs.remove(i);
                    } else {
                        refs.push(r);
                    }
                } else if m.shift && !refs.is_empty() {
                    // A run of words from the first selected to this one.
                    let idx = |x: &WordRef| words.iter().position(|w| w.r == *x);
                    let (lo, hi) = (refs.iter().filter_map(idx).min().unwrap_or(0), idx(&r).unwrap_or(0));
                    let (lo, hi) = (lo.min(hi), lo.max(hi));
                    refs = words.iter().skip(lo).take(hi - lo + 1).map(|w| w.r).collect();
                } else {
                    refs = vec![r];
                }
                select(app, track, refs, notes, false);
            }
            None => {
                app.words.selected.clear();
                let at = map.at(p.x).max(0);
                let _ = app.run("edit.select", json!({"start": at}));
                let _ = app.run("transport.locate", json!({"at": at}));
            }
        }
    }
    if resp.double_clicked()
        && let Some(p) = resp.interact_pointer_pos()
        && let Some((r, _)) = hit(drawn, p)
        && let Some(w) = words.iter().find(|w| w.r == r)
    {
        app.words.editing = Some((r, w.text.clone()));
        app.words.editing_lane = name.to_string();
    }
    if app.words.editing_lane == name {
        retype(app, ui, name, track, drawn);
    }
    // Keys while the pointer is over the lane.
    if resp.hovered() && app.words.track == Some(track) && !app.words.selected.is_empty() && app.words.editing.is_none() {
        let (del, left, right) = ui.input(|i| {
            (
                i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
                i.modifiers.alt && i.key_pressed(Key::ArrowLeft),
                i.modifiers.alt && i.key_pressed(Key::ArrowRight),
            )
        });
        if del {
            let refs: Vec<Value> = app.words.selected.iter().map(|r| ref_json(*r)).collect();
            if run(app, "lyrics.word_delete", json!({"words": refs})).is_some() {
                app.words.selected.clear();
            }
        } else if (left || right)
            && let Some(r) = app.words.selected.first().copied()
        {
            let ms = if m.shift { 50.0 } else { 10.0 } * if left { -1.0 } else { 1.0 };
            if let Some(nr) = run(app, "lyrics.word_move", merge(ref_json(r), json!({"by_ms": ms}))).as_ref().and_then(ref_of) {
                app.words.selected = vec![nr];
            }
        }
    }
    menu(app, ui, name, &resp, track, drawn, map, notes);
}

fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(o), Some(b)) = (a.as_object_mut(), b.as_object()) {
        for (k, v) in b {
            o.insert(k.clone(), v.clone());
        }
    }
    a
}

/// The retype field over the word being edited: Enter keeps it, Esc leaves it.
fn retype(app: &mut SoundApp, ui: &mut Ui, name: &str, track: TrackId, drawn: &[Drawn]) {
    let Some((r, mut text)) = app.words.editing.clone() else { return };
    let Some(d) = drawn.iter().find(|d| d.r == r) else { return };
    let box_r = Rect::from_min_size(d.rect.min, vec2(d.rect.width().max(90.0), d.rect.height().max(18.0)));
    let id = egui::Id::new(("word-retype", name, track.0));
    // Read before the field, which takes Enter and Esc for itself.
    let (enter, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(box_r));
    let out = child.add_sized(box_r.size(), egui::TextEdit::singleline(&mut text).id(id).font(regular(12.0)));
    // Focus once, the whole word selected so typing replaces it; after it has had the keyboard,
    // losing it keeps what was typed.
    let focus_key = id.with("had-focus");
    let had = ui.ctx().data(|x| x.get_temp::<bool>(focus_key)).unwrap_or(false);
    if out.has_focus() {
        ui.ctx().data_mut(|x| x.insert_temp(focus_key, true));
    } else if !had {
        let mut st = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
        st.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(text.chars().count()))));
        st.store(ui.ctx(), id);
        out.request_focus();
    }
    store(ui.ctx(), &format!("{name}-retype"), box_r);
    let done = |ui: &Ui| ui.ctx().data_mut(|x| x.remove::<bool>(focus_key));
    if esc {
        app.words.editing = None;
        done(ui);
    } else if enter || (had && !out.has_focus()) {
        app.words.editing = None;
        done(ui);
        if text.trim() != d.text
            && let Some(nr) = run(app, "lyrics.word_set", merge(ref_json(r), json!({"text": text.trim()}))).as_ref().and_then(ref_of)
        {
            app.words.selected = vec![nr];
        }
    } else {
        app.words.editing = Some((r, text));
    }
}

/// The right-click menu.
#[allow(clippy::too_many_arguments)]
fn menu(app: &mut SoundApp, ui: &mut Ui, name: &str, resp: &egui::Response, track: TrackId, drawn: &[Drawn], map: TimeMap, notes: bool) {
    let key = egui::Id::new(("word-menu", track.0));
    if resp.secondary_clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let r = hit(drawn, p).map(|(r, _)| r);
        if let Some(r) = r
            && !(app.words.track == Some(track) && app.words.selected.contains(&r))
        {
            app.words.track = Some(track);
            app.words.selected = vec![r];
        }
        ui.ctx().data_mut(|d| d.insert_temp(key, (r, map.at(p.x))));
    }
    let (at_word, _at) = ui.ctx().data(|d| d.get_temp::<(Option<WordRef>, Samples)>(key)).unwrap_or((None, 0));
    let s = app.engine.session_arc();
    let words = track_words(&s, track);
    let word = at_word.and_then(|r| words.iter().find(|w| w.r == r).cloned());
    resp.context_menu(|ui| {
        if let Some(w) = &word {
            if ui.button("Audition").on_hover_text("Play just this word").clicked() {
                select(app, track, vec![w.r], notes, true);
                ui.close();
            }
            if ui.button("Retype\u{2026}").clicked() {
                app.words.editing = Some((w.r, w.text.clone()));
                app.words.editing_lane = name.to_string();
                ui.close();
            }
            let ph = app.position();
            let split_at = (ph > w.start && ph < w.end).then_some(ph);
            let label = if split_at.is_some() { "Split at Playhead" } else { "Split in Two" };
            if ui.button(label).clicked() {
                let mut p = ref_json(w.r);
                if let Some(at) = split_at {
                    p["at"] = json!(at);
                }
                run(app, "lyrics.word_split", p);
                app.words.selected.clear();
                ui.close();
            }
            if ui.button("Merge with Next Word").clicked() {
                if let Some(nr) = run(app, "lyrics.word_merge", ref_json(w.r)).as_ref().and_then(ref_of) {
                    app.words.selected = vec![nr];
                }
                ui.close();
            }
            let sel: Vec<Value> = app.words.selected.iter().map(|r| ref_json(*r)).collect();
            if w.timing == WordTiming::Guess && ui.button("Mark as Checked").on_hover_text("Its time is right: no longer a guess").clicked() {
                run(app, "lyrics.words_confirm", json!({"words": sel}));
                ui.close();
            }
            if ui.button("Delete").clicked() {
                run(app, "lyrics.word_delete", json!({"words": sel}));
                app.words.selected.clear();
                ui.close();
            }
            ui.separator();
        }
        if ui.button("Insert Word at Playhead\u{2026}").clicked() {
            let at = app.position();
            if let Some(nr) = run(app, "lyrics.word_insert", json!({"track": track.0, "at": at, "text": "word"})).as_ref().and_then(ref_of) {
                app.words.track = Some(track);
                app.words.selected = vec![nr];
                app.words.editing = Some((nr, "word".into()));
                app.words.editing_lane = name.to_string();
            }
            ui.close();
        }
        ui.menu_button("Select", |ui| {
            for (which, label) in [
                ("first_of_line", "First Word of Every Line"),
                ("last_of_line", "Last Word of Every Line"),
                ("guessed", "Guessed Words"),
                ("all", "All Words"),
            ] {
                if ui.button(label).clicked() {
                    select_which(app, track, which, "");
                    ui.close();
                }
            }
            if let Some(w) = &word
                && ui.button(format!("Every \u{201c}{}\u{201d}", w.text.trim_matches(|c: char| !c.is_alphanumeric()))).clicked()
            {
                select_which(app, track, "same_word", &w.text);
                ui.close();
            }
            ui.separator();
            ui.label("Find:");
            let r = ui.text_edit_singleline(&mut app.words.find);
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                let text = app.words.find.clone();
                select_which(app, track, "find", &text);
                ui.close();
            }
        });
        if ui.button("Time Words to Vocal").on_hover_text("Spread the typed lines placed on the Lyrics ruler over this track's notes").clicked() {
            match app.run("lyrics.time_words", json!({"track": track.0})) {
                Ok(v) => app.ui.status = format!("{} words timed: check the guessed ones", v["words"]),
                Err(e) => app.ui.status = e,
            }
            ui.close();
        }
    });
}

/// The Edit window's strip of a track's words at the bottom of its lane (when the lane is tall
/// enough to keep room for its clips).
pub fn track_strip(app: &mut SoundApp, ui: &mut Ui, track: &soundcraft_model::Track, tl: Rect, lane: Rect) {
    if !soundcraft_model::track_has_lyrics(track) || lane.height() < crate::lyrics_lane::STRIP_H * 2.5 {
        return;
    }
    let strip = Rect::from_min_max(pos2(lane.min.x, lane.max.y - crate::lyrics_lane::STRIP_H), lane.max);
    let s = app.engine.session();
    let map = TimeMap { left: tl.min.x, t0: s.edit.zoom.scroll, spp: s.edit.zoom.samples_per_px.max(0.01) };
    self::lane(app, ui, &format!("strip-{}", track.id.0), track.id, strip, map, true, false);
}
