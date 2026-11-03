//! The Mix window's pitch box on vocal (or tuned) strips, under the inserts, above Sends. It is
//! compact by default ([`compact_section`], spec 2026-11-02 item 1): a ~70 px box whose colour
//! says how far the note is off, a mini trace and a cents bar; strips without one give that room
//! to their inserts. ▸ or a double-click expands it (per track, saved: `mix.pitch_expanded`) to
//! the full PITCH section ([`pitch_section`]): the last ~3 s
//! of the track's notes around the playhead (bars and curve as in the Vocal Editor), the note
//! under the playhead and its offset, and two quick moves on that note, Centre note (one 40 %
//! Centre step) and Flatten drift (one 30 % Drift step), through `vocal.step`. A double-click on
//! the graph opens the Vocal Editor there. Stretched clips (untuned, untunable) are left out of
//! the graph; the buttons then say why. It reads the shared vocal cache only.

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use crate::vocal_editor::hook;
use crate::vocal_editor::layout::{NoteShape, RollView, curve_points, note_shapes, notes_in};
use crate::vocal_style::{fmt_cents, note_name, off_colour};
use crate::vocal_track::{lane_view, target_for, tuned_for};
use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_engine::vocal::NoteRef;
use soundcraft_model::{Clip, Session, Track, TrackId};
use soundcraft_time::Samples;
use soundcraft_vocal::{EditCurves, VocalAnalysis};
use std::sync::Arc;

/// The PITCH row's height: title 16 + graph 72 + readout 18 + two buttons 2×20 + gaps 4.
pub const PITCH_H: f32 = 150.0;
/// The graph's window before and after the playhead (seconds): the playhead sits at 87 %.
pub const BEFORE_S: f64 = 2.6;
pub const AFTER_S: f64 = 0.4;
/// A note that ended this recently (seconds) before the playhead is still "the note".
pub const RECENT_S: f64 = 0.5;
/// The Centre note button's fill (the mockup's accent blue).
pub const CENTRE_BLUE: Color32 = Color32::from_rgb(46, 92, 150);

/// The note the section shows and acts on.
#[derive(Debug, Clone, PartialEq)]
pub struct Current {
    pub note: NoteRef,
    /// Its bar in the graph.
    pub rect: Rect,
    /// The tuned note's offset from its target (cents).
    pub offset: f32,
    /// The target's name ("G#4").
    pub name: String,
    /// The middle of the note (inside its clip's window): what `vocal.step` is given.
    pub at: Samples,
}

/// What the graph shows.
#[derive(Debug, Clone, PartialEq)]
pub struct PitchWindow {
    pub t0: Samples,
    pub t1: Samples,
    pub view: RollView,
    /// The note bars (rounded, coloured by the tuned offset, thick by loudness).
    pub bars: Vec<NoteShape>,
    /// The (tuned) pitch curve through them.
    pub curve: Vec<Vec<Pos2>>,
    pub current: Option<Current>,
}

impl PitchWindow {
    /// Screen x of the playhead.
    pub fn playhead_x(&self, playhead: Samples) -> f32 {
        self.view.x(playhead)
    }
}

/// The analysis and curves of a clip (cached or derived).
type Get<'a> = dyn FnMut(&Clip) -> Option<(Arc<VocalAnalysis>, Option<Arc<EditCurves>>)> + 'a;

/// The graph of `track` in `graph` around `playhead` (see the module docs); `None` when no
/// analysed clip of the track meets the window.
pub fn pitch_window(graph: Rect, s: &Session, track: TrackId, playhead: Samples) -> Option<PitchWindow> {
    let mut get = |c: &Clip| {
        let a = soundcraft_engine::vocal::analysis_for(s, c)?;
        Some((Arc::new(a), soundcraft_engine::vocal::curves_for(s, c).map(Arc::new)))
    };
    window_with(graph, s, track, playhead, -21.0, &mut get)
}

fn window_with(graph: Rect, s: &Session, track: TrackId, playhead: Samples, typical: f32, get: &mut Get<'_>) -> Option<PitchWindow> {
    let rate = s.sample_rate.as_f64();
    let t0 = playhead.saturating_sub((BEFORE_S * rate) as Samples);
    let t1 = playhead.saturating_add((AFTER_S * rate) as Samples);
    let recent = (RECENT_S * rate) as Samples;
    let t = s.track(track)?;
    // Pass 1: the analysed clips in the window and their notes' pitch range.
    let wide = lane_view(graph, t0, t1, 0.0, 12_800.0);
    let mut clips = Vec::new();
    let mut centres = Vec::new();
    // Stretched clips play untuned and can't be tuned: not in the graph (as the track's line).
    for c in
        t.clips().iter().filter(|c| c.is_audio() && !soundcraft_engine::vocal::stretched(c) && c.end() > t0.saturating_sub(recent) && c.start < t1)
    {
        let Some((a, curves)) = get(c) else { continue };
        let curves = curves.filter(|_| !c.tune_live().is_some_and(|tu| tu.original));
        let target = target_for(s, c);
        centres
            .extend(note_shapes(&wide, c, &a, curves.as_deref(), typical, &target, tuned_for(s, c, &a)).iter().map(|sh| sh.target + sh.tuned_offset));
        clips.push((c, a, curves));
    }
    if clips.is_empty() {
        return None;
    }
    let (lo, hi) = range_of(&centres);
    let view = lane_view(graph, t0, t1, lo, hi);
    let mut bars = Vec::new();
    let mut curve = Vec::new();
    let mut sounding: Option<Current> = None;
    let mut last: Option<(Samples, Current)> = None;
    for (c, a, curves) in &clips {
        let target = target_for(s, c);
        let shapes = note_shapes(&view, c, a, curves.as_deref(), typical, &target, tuned_for(s, c, a));
        curve.extend(curve_points(&view, c, &a.track, curves.as_ref().map(|cv| cv.shift_cents.as_slice())));
        let fm = crate::vocal_editor::layout::Frames::of(c, a.track.hop_samples);
        // The note under the playhead, else the last that ended within RECENT_S before it: only
        // the notes around the playhead are looked at.
        let near = notes_in(a, fm.frame(playhead.saturating_sub(recent)).saturating_sub(1), fm.frame(playhead).saturating_add(2));
        for n in a.notes.get(near).unwrap_or(&[]) {
            let (ns, ne) = (fm.clamp(fm.time(n.start)), fm.clamp(fm.time(n.end)));
            if ne <= ns {
                continue;
            }
            let make = || {
                let sh = shapes.iter().find(|sh| sh.r.note == n.id);
                Current {
                    note: NoteRef { clip: c.id, note: n.id },
                    rect: sh.map_or(Rect::NOTHING, |sh| sh.rect),
                    offset: sh.map_or(n.centre_cents - target(n), |sh| sh.tuned_offset),
                    name: note_name(target(n)),
                    // Mid-note: a time at a frame edge could be read as the neighbouring note.
                    at: ns + (ne - ns) / 2,
                }
            };
            if ns <= playhead && playhead < ne {
                sounding = Some(make());
            } else if ne <= playhead && playhead - ne <= recent && last.as_ref().is_none_or(|(e, _)| ne > *e) {
                last = Some((ne, make()));
            }
        }
        bars.extend(shapes);
    }
    let current = sounding.or(last.map(|(_, c)| c));
    Some(PitchWindow { t0, t1, view, bars, curve, current })
}

/// The graph's pitch range: the notes ±1 semitone, at least 5 semitones (centred).
fn range_of(centres: &[f32]) -> (f32, f32) {
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for c in centres.iter().filter(|c| c.is_finite()) {
        lo = lo.min(*c);
        hi = hi.max(*c);
    }
    if lo > hi {
        return (6000.0, 6500.0);
    }
    let (lo, hi) = (lo - 100.0, hi + 100.0);
    if hi - lo < 500.0 {
        let mid = (lo + hi) / 2.0;
        (mid - 250.0, mid + 250.0)
    } else {
        (lo, hi)
    }
}

/// Whether a clip of `track` that is not analysed yet meets the graph's window around
/// `playhead` and passes `pick` (not stretched: those play untuned).
fn unanalysed_in_window(s: &Session, track: TrackId, playhead: Samples, pick: impl Fn(&Clip) -> bool) -> bool {
    let rate = s.sample_rate.as_f64();
    let t0 = playhead.saturating_sub(((BEFORE_S + RECENT_S) * rate) as Samples);
    let t1 = playhead.saturating_add((AFTER_S * rate) as Samples);
    s.track(track).is_some_and(|t| {
        t.clips().iter().any(|c| {
            c.is_audio() && c.end() > t0 && c.start < t1 && !soundcraft_engine::vocal::stretched(c) && !crate::vocal_track::analysed(s, c) && pick(c)
        })
    })
}

/// Whether a clip of `track` that is being analysed meets the graph's window around `playhead`
/// (the section then says "Analysing…").
pub fn waiting(s: &Session, track: TrackId, playhead: Samples) -> bool {
    unanalysed_in_window(s, track, playhead, |c| !soundcraft_engine::vocal::cannot_analyse(s, c))
}

/// Whether a clip of `track` that will never be analysed (missing audio, too long, or failed)
/// meets the graph's window around `playhead`.
pub fn cant_analyse(s: &Session, track: TrackId, playhead: Samples) -> bool {
    unanalysed_in_window(s, track, playhead, |c| soundcraft_engine::vocal::cannot_analyse(s, c))
}

/// Whether `track` plays a stretched audio clip at `playhead`.
pub fn stretched_at(s: &Session, track: TrackId, playhead: Samples) -> bool {
    s.track(track).is_some_and(|t| t.clips().iter().any(|c| c.is_audio() && c.range().contains(playhead) && soundcraft_engine::vocal::stretched(c)))
}

/// What the Centre note / Flatten drift buttons' tooltip says while they are disabled; `None`
/// when they act on `current`.
pub fn disabled_reason(current: Option<&Current>, stretched: bool, waiting: bool, cant: bool) -> Option<&'static str> {
    match current {
        _ if stretched => Some("This clip is stretched: it plays untuned and can't be tuned"),
        Some(_) => None,
        None if waiting => Some("Analysing\u{2026}"),
        None if cant => Some("Can't be analysed (missing audio, or longer than 30 min)"),
        None => Some("no note"),
    }
}

/// Whether `track`'s strip shows a pitch box: a track marked (or named) as a vocal, or one with
/// tuning on a clip.
pub fn shows_pitch(track: &Track) -> bool {
    track.is_vocal() || (track.kind == soundcraft_model::TrackKind::Audio && track.clips().iter().any(|c| c.tune_live().is_some()))
}

/// The height of `track`'s pitch box: the full panel when expanded, else the compact box; 0 on a
/// track that shows none.
pub fn box_height(s: &Session, track: &Track) -> f32 {
    match shows_pitch(track) {
        false => 0.0,
        true if soundcraft_engine::cmd::mix_view::pitch_expanded(s, track.id) => PITCH_H,
        true => COMPACT_H,
    }
}

/// The PITCH row's height across the shown strips: the tallest pitch box while the Pitch view is
/// on, 0 when no shown strip has one. Strips with a shorter box (or none) give the rest of it to
/// their chain.
pub fn row_height(app: &SoundApp, shown: &[TrackId]) -> f32 {
    let on = app.ui.mix_views.iter().any(|v| v == "pitch" || v == "all");
    let s = app.engine.session();
    if !on {
        return 0.0;
    }
    shown.iter().filter_map(|t| s.track(*t)).map(|t| box_height(s, t)).fold(0.0, f32::max)
}

/// Draws `track`'s pitch box in `rect`: the compact box, or the full Pitch panel when expanded.
pub fn pitch_box(app: &mut SoundApp, ui: &mut Ui, rect: Rect, track: &Track) {
    if !shows_pitch(track) {
        return;
    }
    if soundcraft_engine::cmd::mix_view::pitch_expanded(app.engine.session(), track.id) {
        pitch_section(app, ui, rect, track);
    } else {
        compact_section(app, ui, rect, track);
    }
}

/// Expands (or collapses) `track`'s pitch box through `mix.pitch_expanded`.
fn set_expanded(app: &mut SoundApp, track: TrackId, on: bool) {
    if let Err(e) = app.run("mix.pitch_expanded", json!({"track": track.0, "expanded": on})) {
        app.ui.status = e;
    }
}

/// A small disclosure triangle in `r`: pointing right (closed) or down (open).
fn disclosure(p: &egui::Painter, r: Rect, open: bool, color: Color32) {
    let c = r.center();
    let pts = if open {
        vec![c + vec2(-3.5, -2.0), c + vec2(3.5, -2.0), c + vec2(0.0, 2.5)]
    } else {
        vec![c + vec2(-2.0, -3.5), c + vec2(2.5, 0.0), c + vec2(-2.0, 3.5)]
    };
    p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// Draws the full PITCH section of `track`'s strip in `rect` (the expanded pitch box).
pub fn pitch_section(app: &mut SoundApp, ui: &mut Ui, rect: Rect, track: &Track) {
    let t = Tokens::current();
    let id = track.id;
    let key = |name: &str| format!("mix-{}-{name}", id.0);
    let p = ui.painter().clone();
    crate::widgets::inset(&p, rect, t.strip_section);
    p.rect_stroke(rect, 3.0, Stroke::new(1.0, crate::vocal_track::TUNED_PINK.gamma_multiply(0.8)), StrokeKind::Inside);
    p.text(pos2(rect.center().x, rect.min.y + 8.5), Align2::CENTER_CENTER, "PITCH", bold(10.0), t.text);
    // ▾ folds it back to the compact box.
    let fold = Rect::from_min_size(pos2(rect.max.x - 16.0, rect.min.y + 1.0), vec2(14.0, 14.0));
    hook(ui.ctx(), &key("collapse"), fold);
    let fresp = ui.interact(fold, ui.id().with(("vocal-mix-fold", id.0)), Sense::click());
    fresp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Compact pitch box"));
    disclosure(&p, fold, true, if fresp.hovered() { t.text } else { t.text_dim });
    if fresp.on_hover_text("Show the compact pitch box").clicked() {
        set_expanded(app, id, false);
    }
    let graph = Rect::from_min_size(pos2(rect.min.x + 4.0, rect.min.y + 16.0), vec2(rect.width() - 8.0, 72.0));
    let readout = Rect::from_min_size(pos2(rect.min.x + 4.0, graph.max.y), vec2(rect.width() - 8.0, 18.0));
    let centre_r = Rect::from_min_size(pos2(rect.min.x + 4.0, readout.max.y), vec2(rect.width() - 8.0, 20.0));
    let drift_r = Rect::from_min_size(pos2(rect.min.x + 4.0, centre_r.max.y + 2.0), vec2(rect.width() - 8.0, 20.0));
    hook(ui.ctx(), &key("graph"), graph);
    hook(ui.ctx(), &key("centre"), centre_r);
    hook(ui.ctx(), &key("drift"), drift_r);
    let graph_bg = if crate::theme::is_light() { Color32::from_rgb(232, 232, 236) } else { Color32::from_rgb(28, 28, 32) };
    p.rect_filled(graph, 2.0, graph_bg);
    let playhead = app.position();
    let s = app.engine.session_arc();
    let typical = crate::vocal_editor::roll::typical(app, id);
    let win = {
        let mut get = |c: &Clip| crate::vocal_track::cached(app, c);
        window_with(graph, &s, id, playhead, typical, &mut get)
    };
    let gp = p.with_clip_rect(graph.intersect(ui.clip_rect()));
    let waiting = win.is_none() && waiting(&s, id, playhead);
    let cant = win.is_none() && !waiting && cant_analyse(&s, id, playhead);
    if let Some(w) = &win {
        for b in &w.bars {
            gp.rect_filled(b.rect, b.rect.height().min(b.rect.width()) / 2.0, b.colour);
        }
        let line = if crate::theme::is_light() { Color32::from_black_alpha(170) } else { Color32::from_white_alpha(200) };
        for pts in &w.curve {
            gp.add(egui::Shape::line(pts.clone(), Stroke::new(1.0, line)));
        }
        if let Some(c) = &w.current
            && c.rect.is_positive()
        {
            gp.rect_stroke(
                c.rect.expand(1.0),
                c.rect.height().min(c.rect.width()) / 2.0,
                Stroke::new(1.5, if crate::theme::is_light() { Color32::from_gray(20) } else { Color32::WHITE }),
                StrokeKind::Outside,
            );
        }
        let x = w.playhead_x(playhead);
        gp.line_segment([pos2(x, graph.min.y), pos2(x, graph.max.y)], Stroke::new(1.5, Color32::from_rgb(80, 150, 255)));
    } else if waiting {
        gp.text(graph.center(), Align2::CENTER_CENTER, "Analysing\u{2026}", regular(10.0), t.text_dim);
    } else if cant {
        gp.text(graph.center(), Align2::CENTER_CENTER, "Can't analyse", regular(10.0), t.text_dim);
    }
    let current = win.as_ref().and_then(|w| w.current.clone());
    // Stretched means the current note's clip (the graph leaves stretched clips out, so a current
    // note is on a tunable clip); with no current note, a stretched clip under the playhead.
    let stretched = match &current {
        Some(c) => s.find_clip(c.note.clip).is_some_and(|(_, cl)| soundcraft_engine::vocal::stretched(cl)),
        None => stretched_at(&s, id, playhead),
    };
    let text = match &current {
        Some(c) => format!("{} {}", c.name, fmt_cents(c.offset)),
        None => "\u{2014}".to_string(),
    };
    hook(ui.ctx(), &key("readout"), text.clone());
    // Darker in the light theme, so amber and teal read on the light strip.
    let ink = |c: Color32| {
        if crate::theme::is_light() { Color32::from_rgb(c.r() / 2 + c.r() / 8, c.g() / 2 + c.g() / 8, c.b() / 2 + c.b() / 8) } else { c }
    };
    let col = current.as_ref().map_or(t.text_dim, |c| ink(off_colour(c.offset)));
    p.text(readout.center(), Align2::CENTER_CENTER, &text, bold(12.0), col);
    // The buttons.
    let why = disabled_reason(current.as_ref(), stretched, waiting, cant);
    hook(ui.ctx(), &key("why"), why.unwrap_or_default().to_string());
    for (r, label, kind, tip) in [
        (centre_r, "Centre note", "centre", "Pull the note 40 % further to its target (one Centre step, undoable)"),
        (drift_r, "Flatten drift", "drift", "Remove 30 % more of the note's drift (one Drift step, undoable)"),
    ] {
        let mut c = ui.new_child(egui::UiBuilder::new().id_salt(("vocal-mix", id.0, kind)).max_rect(r));
        if why.is_some() {
            c.disable();
        }
        let text = egui::RichText::new(label).font(regular(11.0));
        let b = if kind == "centre" {
            egui::Button::new(text.color(Color32::from_rgb(235, 240, 250))).fill(CENTRE_BLUE)
        } else {
            egui::Button::new(text)
        };
        let resp = c.add_sized(r.size(), b);
        let resp = match why {
            Some(w) => resp.on_disabled_hover_text(w),
            None => resp.on_hover_text(tip),
        };
        if resp.clicked()
            && let Some(cur) = &current
            && let Err(e) = app.run(crate::vocal_ids::VOCAL_STEP, json!({"track": id.0, "at": cur.at, "kind": kind}))
        {
            app.ui.status = e;
        }
    }
    // A double-click on the graph opens the Vocal Editor there.
    let gresp = ui.interact(graph, ui.id().with(("vocal-mix-graph", id.0)), Sense::click());
    if gresp.double_clicked()
        && let (Some(w), Some(pos)) = (&win, gresp.interact_pointer_pos())
    {
        let at = w.view.at(pos.x);
        let note = soundcraft_engine::vocal::note_at(&s, id, at);
        crate::vocal_editor::open_at(app, id, at, note);
    } else if win.is_some() {
        let _ = gresp.on_hover_text("Double-click: open the Vocal Editor here");
    }
}

// ---- The compact pitch box (spec 2026-11-02 item 1) ---------------------------------------------

/// The compact pitch box's height.
pub const COMPACT_H: f32 = 70.0;
/// Seconds of pitch the compact box's trace shows, up to the playhead.
pub const TRACE_S: f64 = 2.0;
/// The compact box changes colour only once a new state has lasted this long (seconds), so a
/// slide between notes doesn't flicker it.
pub const HOLD_S: f64 = 0.3;
/// In tune: within this many cents (rounded) of the target.
pub const IN_TUNE_CENTS: f32 = 10.0;
/// Drifting: up to this many cents; beyond it the note is off.
pub const DRIFT_CENTS: f32 = 17.0;
/// The trace and the cents bar reach their edge at this many cents.
pub const TRACE_CENTS: f32 = 50.0;

/// What the compact box's colour says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// No note sounding: grey.
    Silent,
    /// Within ±10 cents: green.
    InTune,
    /// 11–17 cents: amber.
    Drifting,
    /// Beyond 17 cents: red, the box filled.
    Off,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Silent => "silent",
            Level::InTune => "in tune",
            Level::Drifting => "drifting",
            Level::Off => "off",
        }
    }
}

/// The level of a note `offset` cents from its target (`None`: nothing sounding).
pub fn level_of(offset: Option<f32>) -> Level {
    match offset.filter(|c| c.is_finite()).map(|c| c.abs().round()) {
        None => Level::Silent,
        Some(c) if c <= IN_TUNE_CENTS => Level::InTune,
        Some(c) if c <= DRIFT_CENTS => Level::Drifting,
        Some(_) => Level::Off,
    }
}

/// The level the box shows, held: a new level replaces it once it has been seen for [`HOLD_S`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Held {
    pub shown: Level,
    /// A different level being seen, since when (seconds).
    pending: Option<(Level, f64)>,
}

impl Held {
    pub fn new(level: Level) -> Held {
        Held { shown: level, pending: None }
    }

    /// The state after seeing `seen` at time `now` (seconds).
    pub fn step(self, seen: Level, now: f64) -> Held {
        if seen == self.shown {
            return Held::new(seen);
        }
        match self.pending {
            Some((l, since)) if l == seen => {
                if now - since >= HOLD_S {
                    Held::new(seen)
                } else {
                    self
                }
            }
            _ => Held { shown: self.shown, pending: Some((seen, now)) },
        }
    }
}

/// One stretch of the trace: `(x in 0..=1 across the last TRACE_S, offset from target in cents)`.
pub type TraceLine = Vec<(f32, f32)>;

/// The last [`TRACE_S`] of `track`'s (tuned) pitch up to `playhead`, as offsets from each note's
/// target: one line per note, broken where the voice drops out.
fn trace_with(s: &Session, track: TrackId, playhead: Samples, get: &mut Get<'_>) -> Vec<TraceLine> {
    let rate = s.sample_rate.as_f64();
    let t0 = playhead.saturating_sub((TRACE_S * rate) as Samples);
    let span = (playhead - t0).max(1) as f32;
    let Some(t) = s.track(track) else { return Vec::new() };
    let mut out = Vec::new();
    for c in t.clips().iter().filter(|c| c.is_audio() && !soundcraft_engine::vocal::stretched(c) && c.end() > t0 && c.start < playhead) {
        let Some((a, curves)) = get(c) else { continue };
        let curves = curves.filter(|_| !c.tune_live().is_some_and(|tu| tu.original));
        let shift = curves.as_ref().map(|cv| cv.shift_cents.as_slice());
        let target = target_for(s, c);
        let fm = crate::vocal_editor::layout::Frames::of(c, a.track.hop_samples);
        let near = notes_in(&a, fm.frame(t0).saturating_sub(1), fm.frame(playhead).saturating_add(1));
        for n in a.notes.get(near).unwrap_or(&[]) {
            let goal = target(n);
            let mut line = TraceLine::new();
            for f in n.start..n.end {
                let at = fm.time(f);
                if at < t0 || at > playhead || at < c.start || at >= c.end() {
                    continue;
                }
                let cents = a.track.frames.get(f as usize).and_then(|fr| fr.cents).filter(|v| v.is_finite());
                let Some(cents) = cents else {
                    if !line.is_empty() {
                        out.push(std::mem::take(&mut line));
                    }
                    continue;
                };
                let sh = shift.and_then(|sh| sh.get(f as usize)).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
                line.push(((at - t0) as f32 / span, cents + sh - goal));
            }
            if !line.is_empty() {
                out.push(line);
            }
        }
    }
    out
}

/// What the compact box shows of `track` at `playhead`: the note sounding (with its offset) and
/// the trace. From the engine's analyses (tests); the box itself reads the shared cache.
pub fn compact_state(s: &Session, track: TrackId, playhead: Samples) -> (Option<Current>, Vec<TraceLine>) {
    let mut get = |c: &Clip| {
        let a = soundcraft_engine::vocal::analysis_for(s, c)?;
        Some((Arc::new(a), soundcraft_engine::vocal::curves_for(s, c).map(Arc::new)))
    };
    let unit = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 50.0));
    let current = window_with(unit, s, track, playhead, -21.0, &mut get).and_then(|w| w.current);
    (current, trace_with(s, track, playhead, &mut get))
}

/// The compact box's colour for `level` (darker on the light theme, so it reads on light strips).
pub fn level_colour(level: Level) -> Color32 {
    let light = crate::theme::is_light();
    match level {
        Level::Silent => Tokens::current().text_dim,
        Level::InTune if light => Color32::from_rgb(30, 140, 64),
        Level::InTune => Color32::from_rgb(74, 200, 104),
        Level::Drifting if light => Color32::from_rgb(178, 118, 10),
        Level::Drifting => Color32::from_rgb(240, 192, 96),
        Level::Off if light => Color32::from_rgb(200, 40, 48),
        Level::Off => Color32::from_rgb(236, 82, 88),
    }
}

/// The cents bar's filled span `(from x, to x)` for `offset` cents on a bar centred at `cx`,
/// `half` wide each side: from the centre, left when flat, right when sharp, at least 3 points.
pub fn cents_span(offset: f32, cx: f32, half: f32) -> (f32, f32) {
    let o = if offset.is_finite() { offset.clamp(-TRACE_CENTS, TRACE_CENTS) } else { 0.0 };
    let w = o / TRACE_CENTS * half.max(0.0);
    if w.abs() < 3.0 {
        (cx - 1.5 + w.min(0.0), cx + 1.5 + w.max(0.0))
    } else if w < 0.0 {
        (cx + w, cx)
    } else {
        (cx, cx + w)
    }
}

/// The compact pitch box (mockup `pitch_compact_mock.png`): made to be glanced at. Its colour
/// says how far the note under the playhead is from its target (green / amber / red, grey when
/// silent; held [`HOLD_S`]), with a mini trace of the last [`TRACE_S`] against a centre line, a
/// cents bar growing left (flat) or right (sharp), and the note and cents, small. ▸ or a
/// double-click expands it to the full Pitch panel.
pub fn compact_section(app: &mut SoundApp, ui: &mut Ui, rect: Rect, track: &Track) {
    let t = Tokens::current();
    let id = track.id;
    let key = |name: &str| format!("mix-{}-{name}", id.0);
    let light = crate::theme::is_light();
    let playhead = app.position();
    let s = app.engine.session_arc();
    let typical = crate::vocal_editor::roll::typical(app, id);
    let (current, trace) = {
        let mut get = |c: &Clip| crate::vocal_track::cached(app, c);
        let unit = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 50.0));
        let current = window_with(unit, &s, id, playhead, typical, &mut get).and_then(|w| w.current);
        (current, trace_with(&s, id, playhead, &mut get))
    };
    // The colour, held so a slide doesn't flicker it.
    let seen = level_of(current.as_ref().map(|c| c.offset));
    let now = ui.input(|i| i.time);
    let hold_id = egui::Id::new(("pitch-hold", id.0));
    let held = ui.data(|d| d.get_temp::<Held>(hold_id)).map_or(Held::new(seen), |h| h.step(seen, now));
    ui.data_mut(|d| d.insert_temp(hold_id, held));
    if held.pending.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(HOLD_S));
    }
    let level = held.shown;
    let col = level_colour(level);
    hook(ui.ctx(), &key("compact"), rect);
    hook(ui.ctx(), &key("level"), level.name().to_string());

    let p = ui.painter().clone();
    crate::widgets::inset(&p, rect, t.strip_section);
    let fill = match level {
        Level::Off if light => Color32::from_rgb(252, 222, 222),
        Level::Off => Color32::from_rgb(78, 24, 30),
        Level::Drifting => col.gamma_multiply(0.07),
        _ => Color32::TRANSPARENT,
    };
    p.rect_filled(rect.shrink(1.0), 3.0, fill);
    let edge = if level == Level::Silent { t.text_dim.gamma_multiply(0.5) } else { col };
    p.rect_stroke(rect, 3.0, Stroke::new(1.0, edge), StrokeKind::Inside);

    // Title row: PITCH (· OFF), the note and cents, ▸.
    let narrow = rect.width() < 100.0;
    let title = if level == Level::Off && !narrow { "PITCH \u{b7} OFF" } else { "PITCH" };
    let tg = p.layout_no_wrap(title.to_string(), bold(if narrow { 8.0 } else { 9.0 }), col);
    let title_w = tg.size().x;
    p.galley(pos2(rect.min.x + 5.0, rect.min.y + 9.0 - tg.size().y / 2.0), tg, col);
    let expand = Rect::from_min_size(pos2(rect.max.x - 14.0, rect.min.y + 2.0), vec2(12.0, 14.0));
    hook(ui.ctx(), &key("expand"), expand);
    let text = match &current {
        Some(c) => format!("{} {}", c.name, fmt_cents(c.offset)),
        None => "\u{2014}".to_string(),
    };
    hook(ui.ctx(), &key("readout"), text.clone());
    let room = (expand.min.x - 3.0 - (rect.min.x + 8.0 + title_w)).max(0.0);
    let ink = if level == Level::Silent { t.text_dim } else { t.text.lerp_to_gamma(col, 0.35) };
    let rg = p.layout_no_wrap(text, bold(if narrow { 7.5 } else { 8.5 }), ink);
    if rg.size().x <= room {
        p.galley(pos2(expand.min.x - 2.0 - rg.size().x, rect.min.y + 9.0 - rg.size().y / 2.0), rg, ink);
    }
    // The box's own double-click under ▸ (registered first, so ▸ sits on top of it).
    let body = ui.interact(rect, ui.id().with(("vocal-mix-compact", id.0)), Sense::click());
    let eresp = ui.interact(expand, ui.id().with(("vocal-mix-expand", id.0)), Sense::click());
    eresp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Expand pitch box"));
    disclosure(&p, expand, false, if eresp.hovered() { t.text } else { t.text_dim });

    // The trace: offsets from the target against the centre line.
    let graph = Rect::from_min_max(pos2(rect.min.x + 4.0, rect.min.y + 18.0), pos2(rect.max.x - 4.0, rect.min.y + 52.0));
    hook(ui.ctx(), &key("trace"), graph);
    let graph_bg = if light { Color32::from_rgb(232, 232, 236) } else { Color32::from_rgb(22, 22, 26) };
    p.rect_filled(graph, 2.0, graph_bg);
    let mid = graph.center().y;
    let line_c = if level == Level::Silent { t.text_dim.gamma_multiply(0.4) } else { col.gamma_multiply(0.35) };
    p.hline(graph.x_range().shrink(2.0), mid, Stroke::new(1.0, line_c));
    let half = graph.height() / 2.0 - 2.0;
    let gp = p.with_clip_rect(graph.intersect(ui.clip_rect()));
    for line in &trace {
        let pts: Vec<Pos2> = line
            .iter()
            .map(|(x, c)| {
                pos2(graph.min.x + 2.0 + x.clamp(0.0, 1.0) * (graph.width() - 4.0), mid - c.clamp(-TRACE_CENTS, TRACE_CENTS) / TRACE_CENTS * half)
            })
            .collect();
        if pts.len() >= 2 {
            gp.add(egui::Shape::line(pts, Stroke::new(1.5, col)));
        }
    }

    // The cents bar: from the centre, left when flat, right when sharp.
    let bar = Rect::from_min_max(pos2(rect.min.x + 4.0, rect.min.y + 56.0), pos2(rect.max.x - 4.0, rect.min.y + 64.0));
    p.rect_filled(bar, 4.0, if light { Color32::from_rgb(218, 218, 224) } else { Color32::from_rgb(16, 16, 20) });
    let cx = bar.center().x;
    if let Some(c) = &current {
        let (a, b) = cents_span(c.offset, cx, bar.width() / 2.0 - 1.0);
        hook(ui.ctx(), &key("cents_bar"), Rect::from_x_y_ranges(a..=b, bar.y_range()));
        p.rect_filled(Rect::from_x_y_ranges(a..=b, bar.y_range().shrink(1.0)), 3.0, col);
    }
    p.vline(cx, bar.y_range().expand(1.0), Stroke::new(1.0, t.text_dim.gamma_multiply(0.7)));

    // ▸ or a double-click opens the full Pitch panel.
    let tip = format!("{}\n\u{25b8} or double-click: the full Pitch panel", level_tip(level));
    let open = eresp.on_hover_text("Show the full Pitch panel").clicked();
    if open || body.on_hover_text(tip).double_clicked() {
        set_expanded(app, id, true);
    }
}

fn level_tip(level: Level) -> &'static str {
    match level {
        Level::Silent => "No note sounding",
        Level::InTune => "In tune: within \u{b1}10 cents",
        Level::Drifting => "Drifting: 11\u{2013}17 cents off",
        Level::Off => "Off: more than 17 cents from the note",
    }
}

#[cfg(test)]
mod compact_tests {
    use super::*;

    #[test]
    fn levels_follow_the_owners_thresholds() {
        assert_eq!(level_of(None), Level::Silent);
        assert_eq!(level_of(Some(f32::NAN)), Level::Silent);
        assert_eq!(level_of(Some(0.0)), Level::InTune);
        assert_eq!(level_of(Some(-10.4)), Level::InTune);
        assert_eq!(level_of(Some(10.6)), Level::Drifting);
        assert_eq!(level_of(Some(-17.0)), Level::Drifting);
        assert_eq!(level_of(Some(17.6)), Level::Off);
        assert_eq!(level_of(Some(-41.0)), Level::Off);
    }

    #[test]
    fn a_new_level_shows_once_it_lasts_the_hold() {
        let h = Held::new(Level::InTune);
        // A slide through red for 0.2 s: still green.
        let h = h.step(Level::Off, 1.0).step(Level::Off, 1.2);
        assert_eq!(h.shown, Level::InTune);
        let h = h.step(Level::InTune, 1.25);
        assert_eq!(h.shown, Level::InTune);
        // Red for 0.3 s: red.
        let h = h.step(Level::Off, 2.0).step(Level::Off, 2.2);
        assert_eq!(h.shown, Level::InTune);
        assert_eq!(h.step(Level::Off, 2.31).shown, Level::Off);
        // A level that changes while pending starts its own wait.
        let h = Held::new(Level::InTune).step(Level::Off, 0.0).step(Level::Drifting, 0.2);
        assert_eq!(h.step(Level::Drifting, 0.4).shown, Level::InTune);
        assert_eq!(h.step(Level::Drifting, 0.5).shown, Level::Drifting);
    }

    #[test]
    fn the_cents_bar_grows_from_the_centre() {
        let (a, b) = cents_span(-25.0, 50.0, 40.0);
        assert!((a - 30.0).abs() < 1e-4 && (b - 50.0).abs() < 1e-4, "{a} {b}");
        let (a, b) = cents_span(100.0, 50.0, 40.0);
        assert!((a - 50.0).abs() < 1e-4 && (b - 90.0).abs() < 1e-4, "{a} {b}");
        let (a, b) = cents_span(0.0, 50.0, 40.0);
        assert!(b - a >= 3.0 && a < 50.0 && b > 50.0);
        let (a, b) = cents_span(f32::NAN, 50.0, 40.0);
        assert!(a.is_finite() && b.is_finite());
    }
}
