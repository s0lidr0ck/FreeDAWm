//! The track's "waveform + pitch" view: a thick pitch line per note, coloured by how far the
//! (tuned) note is off and as thick as it is loud, over the dimmed waveform; hovering shows the
//! note and its offset, a double-click opens the Vocal Editor there. Tuned clips carry a TUNED
//! (or UNTUNED) badge in their name bar. Everything reads the shared vocal cache
//! (`vocal_editor::roll::Cache`): the analyses and curves, the track's pitch range and each clip's
//! line are worked out again only when what they derive from changes.

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use crate::vocal_editor::hook;
use crate::vocal_editor::layout::{Frames, RollView, note_shapes, notes_in};
use crate::vocal_editor::roll::{self, LaneKey};
use crate::vocal_style::{fmt_cents, note_name, note_thickness, off_colour};
use egui::{Align2, Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use soundcraft_engine::vocal::NoteRef;
use soundcraft_model::{AnalysisKey, Clip, Session, Track};
use soundcraft_time::Samples;
use soundcraft_vocal::{EditCurves, Note, ScaleAt, VocalAnalysis, moved_target, target_cents};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The pointer must be this close (px) to a line to hover it.
pub const HOVER_PX: f32 = 6.0;
/// The TUNED badge's fill.
pub const TUNED_PINK: Color32 = Color32::from_rgb(200, 60, 140);
/// The track header shows "Tuned · n notes edited" from this height up (px).
pub const TUNED_LINE_MIN_H: f32 = 112.0;

thread_local! {
    /// How many track pitch ranges and clip pitch lines were worked out on this thread (tests
    /// read it: nothing is rebuilt while nothing changes).
    static BUILDS: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
}

/// The (pitch ranges, pitch lines) worked out so far on this thread.
pub fn lane_builds() -> (u64, u64) {
    BUILDS.with(Cell::get)
}

fn count(range: u64, line: u64) {
    BUILDS.with(|b| {
        let (r, l) = b.get();
        b.set((r + range, l + line));
    });
}

/// One note's stretch of the pitch line.
#[derive(Debug, Clone, PartialEq)]
pub struct PitchSeg {
    pub note: NoteRef,
    pub pts: Vec<Pos2>,
    pub colour: Color32,
    pub width: f32,
    /// The tuned note's offset from its target (cents).
    pub offset: f32,
    /// The note's target pitch (MIDI × 100).
    pub target: f32,
    /// Session time of the note's start (inside the clip's window).
    pub start: Samples,
}

/// `clip`'s analysis and curves from the shared cache (`None` before it is analysed).
pub fn cached(app: &mut SoundApp, clip: &Clip) -> roll::Derived {
    roll::derived(app, clip)
}

/// Whether `clip` has a cached analysis (cheap: no clone).
pub fn analysed(s: &Session, clip: &Clip) -> bool {
    AnalysisKey::of(s, clip).is_some_and(|k| s.vocal.analyses.contains_key(&k.hash()))
}

/// Whether the pitch line is drawn on `clip` of `track` (so its waveform is dimmed): the pitch
/// view, a vocal track, an analysed clip that is not stretched.
pub fn line_shown(s: &Session, track: &Track, clip: &Clip) -> bool {
    track.view == "pitch" && track.is_vocal() && clip.is_audio() && !soundcraft_engine::vocal::stretched(clip) && analysed(s, clip)
}

/// Why `clip` of `track` shows no pitch line in the pitch view, when it doesn't.
pub fn lane_hint(s: &Session, track: &Track, clip: &Clip) -> Option<&'static str> {
    if !track.is_vocal() {
        Some("Not a vocal track")
    } else if soundcraft_engine::vocal::stretched(clip) {
        Some("Stretched: plays untuned")
    } else if analysed(s, clip) {
        None
    } else if soundcraft_engine::vocal::cannot_analyse(s, clip) {
        Some("Can't be analysed")
    } else {
        Some("No pitch yet")
    }
}

/// A note's target: the nearest semitone, or with the note's Snap to key the nearest scale tone;
/// moved with a note dragged up or down (its offset's whole semitones) while the clip plays tuned.
pub fn target_for<'a>(s: &Session, clip: &'a Clip) -> impl Fn(&Note) -> f32 + 'a {
    let scale = soundcraft_engine::vocal::scale_at(s, clip);
    let tune = clip.tune_live();
    let plays_tuned = tune.is_some_and(|t| !t.original);
    let edits = tune.map(|t| &t.edits);
    move |n: &Note| {
        let e = edits.and_then(|e| e.get(&n.id));
        let snap = e.is_some_and(|e| e.snap_to_key);
        let t = target_cents(n.centre_cents, if snap { scale.scale_at(n.start) } else { None });
        moved_target(t, e.filter(|_| plays_tuned).map_or(0.0, |e| e.offset_cents))
    }
}

/// An edited note's tuned centre as the engine computes it (where its bar sits while the clip
/// plays tuned); `None` for a note with no edit or a clip playing Original.
pub fn tuned_for<'a>(s: &Session, clip: &'a Clip, a: &'a VocalAnalysis) -> impl Fn(&Note) -> Option<f32> + 'a {
    let scale = soundcraft_engine::vocal::scale_at(s, clip);
    let tune = clip.tune_live().filter(|t| !t.original);
    let mode = soundcraft_engine::vocal::note_mode(clip);
    move |n: &Note| {
        let e = tune?.edits.get(&n.id).filter(|e| !soundcraft_vocal::NoteEdit::clone(e).clamped().is_identity())?;
        Some(soundcraft_vocal::tuned_centre_with(a, n, e, &scale, mode))
    }
}

/// The view mapping of a lane showing `[t0, t1)` and `lo_cents..hi_cents`.
pub fn lane_view(lane: Rect, t0: Samples, t1: Samples, lo_cents: f32, hi_cents: f32) -> RollView {
    let semis = ((hi_cents - lo_cents) / 100.0).max(1.0);
    let row_h = lane.height() / semis;
    RollView { rect: lane, t0, t1: t1.max(t0.saturating_add(1)), top_cents: hi_cents, row_h: if row_h.is_finite() { row_h.max(0.1) } else { 1.0 } }
}

/// The lane's pitch range: the notes' range ±1 semitone, on whole semitones, at least 7 wide
/// (centred). `(6000, 6700)` without notes.
pub fn lane_range(centres: impl IntoIterator<Item = f32>) -> (f32, f32) {
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for c in centres.into_iter().filter(|c| c.is_finite()) {
        lo = lo.min(c);
        hi = hi.max(c);
    }
    if lo > hi {
        return (6000.0, 6700.0);
    }
    let mut lo = ((lo - 100.0) / 100.0).floor() * 100.0;
    let mut hi = ((hi + 100.0) / 100.0).ceil() * 100.0;
    if hi - lo < 700.0 {
        let extra = ((700.0 - (hi - lo)) / 100.0).round();
        lo -= (extra / 2.0).floor() * 100.0;
        hi = lo + 700.0;
    }
    (lo, hi)
}

/// Points decimated to the pixel grid: per pixel column the lowest and highest point, in the
/// order they came, so a line never carries more than two points a pixel.
#[derive(Default)]
struct Decimate {
    out: Vec<Pos2>,
    col: Option<(i64, Pos2, Pos2)>,
}

impl Decimate {
    fn push(&mut self, p: Pos2) {
        let k = p.x.floor() as i64;
        match &mut self.col {
            Some((c, lo, hi)) if *c == k => {
                if p.y < lo.y {
                    *lo = p;
                }
                if p.y > hi.y {
                    *hi = p;
                }
            }
            _ => {
                self.flush();
                self.col = Some((k, p, p));
            }
        }
    }

    fn flush(&mut self) {
        if let Some((_, a, b)) = self.col.take() {
            if a == b {
                self.out.push(a);
            } else if a.x <= b.x {
                self.out.extend([a, b]);
            } else {
                self.out.extend([b, a]);
            }
        }
    }

    fn finish(mut self) -> Vec<Pos2> {
        self.flush();
        self.out
    }
}

/// The pitch line of `clip` (analysis `a`, curves when tuned) across `lane` showing session time
/// `[t0, t1)` and pitches `lo_cents..hi_cents`: one segment per note, following the tuned curve
/// unless the clip plays Original, coloured by the tuned offset, clipped to the clip's window and
/// the range, with no points on unvoiced frames, and at most two points per pixel column.
/// Stretched clips (untuned) draw nothing.
#[allow(clippy::too_many_arguments)]
pub fn pitch_line(
    lane: Rect,
    t0: Samples,
    t1: Samples,
    lo_cents: f32,
    hi_cents: f32,
    clip: &Clip,
    a: &VocalAnalysis,
    curves: Option<&EditCurves>,
    typical_db: f32,
    target: &dyn Fn(&Note) -> f32,
    tuned: &dyn Fn(&Note) -> Option<f32>,
) -> Vec<PitchSeg> {
    if soundcraft_engine::vocal::stretched(clip) || t1 <= t0 || lane.width() <= 0.0 || lane.height() <= 0.0 {
        return Vec::new();
    }
    let curves = curves.filter(|_| !clip.tune_live().is_some_and(|t| t.original));
    let view = lane_view(lane, t0, t1, lo_cents, hi_cents);
    let fm = Frames::of(clip, a.track.hop_samples);
    let Some((lo, hi)) = fm.visible(&view) else { return Vec::new() };
    let shapes: BTreeMap<_, _> =
        note_shapes(&view, clip, a, curves, typical_db, target, tuned).into_iter().map(|sh| (sh.r.note, (sh.tuned_offset, sh.target))).collect();
    let frames = u32::try_from(a.track.frames.len()).unwrap_or(u32::MAX);
    let (w0, w1) = (clip.start.max(t0), clip.end().min(t1));
    let mut out = Vec::new();
    for n in a.notes.get(notes_in(a, lo, hi)).unwrap_or(&[]) {
        let Some((offset, tgt)) = shapes.get(&n.id).copied() else { continue };
        let mut pts = Decimate::default();
        for f in n.start.max(lo)..n.end.min(hi).min(frames) {
            let t = fm.time(f);
            if t < w0 || t > w1 {
                continue;
            }
            let Some(c) = a.track.frames.get(f as usize).and_then(|fr| fr.cents).filter(|c| c.is_finite()) else { continue };
            let shift = curves.and_then(|cv| cv.shift_cents.get(f as usize)).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
            pts.push(pos2(view.x(t), view.y(c + shift)));
        }
        let pts = pts.finish();
        if pts.is_empty() {
            continue;
        }
        let width = (note_thickness(n.level_db, typical_db, view.row_h) * 0.35).clamp(2.0, 6.0);
        out.push(PitchSeg {
            note: NoteRef { clip: clip.id, note: n.id },
            pts,
            colour: off_colour(offset),
            width,
            offset,
            target: tgt,
            start: fm.clamp(fm.time(n.start)),
        });
    }
    out
}

/// Distance from `p` to the polyline `pts`.
fn dist(pts: &[Pos2], p: Pos2) -> f32 {
    match pts {
        [] => f32::INFINITY,
        [one] => one.distance(p),
        _ => pts
            .windows(2)
            .map(|w| {
                let (a, b) = (w.first().copied().unwrap_or(p), w.get(1).copied().unwrap_or(p));
                let ab = b - a;
                let k = if ab.length_sq() > 0.0 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
                (a + ab * k).distance(p)
            })
            .fold(f32::INFINITY, f32::min),
    }
}

/// The segment nearest `p` within [`HOVER_PX`].
pub fn hit(segs: &[PitchSeg], p: Pos2) -> Option<&PitchSeg> {
    segs.iter().map(|s| (dist(&s.pts, p), s)).filter(|(d, _)| *d <= HOVER_PX).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(_, s)| s)
}

/// "0:03.1": minutes, seconds and tenths (truncated).
pub fn fmt_time(at: Samples, rate: f64) -> String {
    let tenths = (at.max(0) as f64 / rate.max(1.0) * 10.0).floor();
    let tenths = if tenths.is_finite() { tenths as i64 } else { 0 };
    format!("{}:{:02}.{}", tenths / 600, (tenths % 600) / 10, tenths % 10)
}

/// The hover tooltip's first line: "A4 −22c · 0:03.1".
pub fn tip_text(seg: &PitchSeg, rate: f64) -> String {
    format!("{} {} \u{b7} {}", note_name(seg.target), fmt_cents(seg.offset), fmt_time(seg.start, rate))
}

/// The pitch range of `track`'s analysed notes in `[t0, t1)` (tuned and sung centres): worked out
/// once per track and span, and kept until the track's clips change.
fn track_range(app: &mut SoundApp, track: &Track, t0: Samples, t1: Samples) -> (f32, f32) {
    let s = roll::checked(app);
    let cache = &mut app.vocal.cache;
    if let Some(r) = cache.range(&s, track.id, (t0, t1)) {
        return r;
    }
    count(1, 0);
    let view = lane_view(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 100.0)), t0, t1, 0.0, 12_800.0);
    let mut centres = Vec::new();
    if let Some(t) = s.track(track.id) {
        for c in t.clips().iter().filter(|c| c.is_audio() && c.end() > t0 && c.start < t1 && !soundcraft_engine::vocal::stretched(c)) {
            let Some((a, curves)) = cache.get(&s, c) else { continue };
            let target = target_for(&s, c);
            let curves = curves.filter(|_| !c.tune_live().is_some_and(|t| t.original));
            for sh in note_shapes(&view, c, &a, curves.as_deref(), -21.0, &target, tuned_for(&s, c, &a)) {
                centres.extend([sh.target + sh.tuned_offset, sh.target + sh.offset]);
            }
        }
    }
    let r = lane_range(centres);
    cache.set_range(&s, track.id, (t0, t1), r);
    r
}

/// Draws `clip`'s pitch line in `lane` (the track's lane; the name bar is left alone) of the
/// pitch view, the outer note names at the left edge, the hover tooltip, and opens the editor
/// on a double-click. A clip with no analysis (or a non-vocal or stretched one) gets a small hint.
/// Returns whether it took a double-click (the lane then doesn't also select the clip).
pub fn draw_pitch_lane(app: &mut SoundApp, ui: &Ui, painter: &Painter, track: &Track, clip: &Clip, lane: Rect, tl: Rect) -> bool {
    if !clip.is_audio() {
        return false;
    }
    let s = app.engine.session_arc();
    let name_h = crate::edit_window::clip_name_height(&s, lane);
    let body = Rect::from_min_max(pos2(lane.min.x, lane.min.y + 1.0 + name_h), pos2(lane.max.x, lane.max.y - 1.0));
    let (x0, x1) = (crate::edit_window::x_of(&s, tl, clip.start).max(lane.min.x), crate::edit_window::x_of(&s, tl, clip.end()).min(lane.max.x));
    if let Some(h) = lane_hint(&s, track, clip) {
        hook(ui.ctx(), &format!("track-hint-{}", clip.id.0), h.to_string());
        hook(ui.ctx(), &format!("track-segs-{}", clip.id.0), Vec::<PitchSeg>::new());
        if x1 - x0 >= 70.0 && body.height() >= 16.0 {
            painter.text(pos2(x0 + 6.0, body.min.y + 3.0), Align2::LEFT_TOP, h, regular(9.5), Tokens::current().text_dim);
        }
        return false;
    }
    hook(ui.ctx(), &format!("track-hint-{}", clip.id.0), String::new());
    if body.height() < 8.0 || x1 <= x0 {
        return false;
    }
    let (t0, t1) = (crate::edit_window::sample_at(&s, tl, tl.min.x), crate::edit_window::sample_at(&s, tl, tl.max.x));
    let typical = roll::typical(app, track.id);
    let (lo, hi) = track_range(app, track, t0, t1);
    let full = Rect::from_min_max(pos2(tl.min.x, body.min.y), pos2(tl.max.x, body.max.y));
    let key: LaneKey = (
        t0,
        t1,
        [full.min.x.to_bits(), full.min.y.to_bits(), full.max.x.to_bits(), full.max.y.to_bits()],
        [lo.to_bits(), hi.to_bits(), typical.to_bits()],
    );
    let s = roll::checked(app);
    let segs = match app.vocal.cache.lane(&s, track.id, clip.id, &key) {
        Some(v) => v,
        None => {
            let Some((a, curves)) = app.vocal.cache.get(&s, clip) else { return false };
            count(0, 1);
            let target = target_for(&s, clip);
            let v = Arc::new(pitch_line(full, t0, t1, lo, hi, clip, &a, curves.as_deref(), typical, &target, &tuned_for(&s, clip, &a)));
            app.vocal.cache.set_lane(&s, track.id, clip.id, key, v.clone());
            v
        }
    };
    let p = painter.with_clip_rect(painter.clip_rect().intersect(Rect::from_min_max(pos2(x0, body.min.y), pos2(x1, body.max.y))));
    // The two outer whole-note names, at the left edge of the lane.
    let view = lane_view(full, t0, t1, lo, hi);
    let natural = |c: f32| matches!(((c / 100.0).round() as i32).rem_euclid(12), 0 | 2 | 4 | 5 | 7 | 9 | 11);
    let top = (1..=((hi - lo) / 100.0) as i32).map(|k| hi - 100.0 * k as f32).find(|c| natural(*c));
    let bottom = (1..=((hi - lo) / 100.0) as i32).map(|k| lo + 100.0 * k as f32).find(|c| natural(*c));
    let label = if crate::theme::is_light() { Color32::from_black_alpha(160) } else { Color32::from_white_alpha(150) };
    for c in [top, bottom].into_iter().flatten() {
        let y = view.y(c);
        p.text(pos2(x0 + 3.0, y), Align2::LEFT_BOTTOM, note_name(c), regular(9.0), label);
    }
    // Only the segments that meet the clip's visible part are handed to the painter. The notes
    // selected (the Vocal Editor's selection: one selection, shared) get a halo.
    let halo = if crate::theme::is_light() { Color32::from_rgb(25, 25, 28) } else { Color32::WHITE };
    let mut shown_selected = Vec::new();
    for seg in segs.iter().filter(|sg| sg.pts.last().is_some_and(|l| l.x >= x0) && sg.pts.first().is_some_and(|f| f.x <= x1)) {
        if app.vocal.selected.contains(&seg.note) {
            shown_selected.push(seg.note);
            if seg.pts.len() >= 2 {
                p.add(Shape::line(seg.pts.clone(), Stroke::new(seg.width + 6.0, halo)));
            } else if let Some(pt) = seg.pts.first() {
                p.circle_filled(*pt, seg.width / 2.0 + 3.0, halo);
            }
        }
        if seg.pts.len() >= 2 {
            p.add(Shape::line(seg.pts.clone(), Stroke::new(seg.width + 2.0, Color32::from_black_alpha(170))));
            p.add(Shape::line(seg.pts.clone(), Stroke::new(seg.width, seg.colour)));
        } else if let Some(pt) = seg.pts.first() {
            p.circle_filled(*pt, seg.width / 2.0 + 1.0, Color32::from_black_alpha(170));
            p.circle_filled(*pt, seg.width / 2.0, seg.colour);
        }
    }
    if crate::vocal_editor::hooks_on() {
        hook(ui.ctx(), &format!("track-segs-{}", clip.id.0), segs.as_ref().clone());
        hook(ui.ctx(), &format!("track-selected-{}", clip.id.0), shown_selected);
    }
    // Hover and double-click.
    let clip_body = Rect::from_min_max(pos2(x0, body.min.y), pos2(x1, body.max.y));
    if !ui.rect_contains_pointer(clip_body) {
        return false;
    }
    let Some(pos) = ui.ctx().pointer_hover_pos() else { return false };
    let Some(seg) = hit(&segs, pos) else { return false };
    let text = tip_text(seg, s.sample_rate.as_f64());
    hook(ui.ctx(), "track-tip", text.clone());
    if let Some(pt) = seg.pts.iter().min_by(|a, b| a.distance(pos).total_cmp(&b.distance(pos))) {
        p.circle_stroke(*pt, 4.0, Stroke::new(1.5, Color32::WHITE));
    }
    let tp = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new(("vocal-track-tip", clip.id.0))));
    let t = Tokens::current();
    let g1 = tp.layout_no_wrap(text, bold(11.0), seg.colour);
    let g2 = tp.layout_no_wrap("double-click: open editor".into(), regular(9.5), t.text_dim);
    let w = g1.size().x.max(g2.size().x) + 14.0;
    let r = Rect::from_min_size(pos + vec2(12.0, -38.0), vec2(w, 34.0));
    tp.rect(r, 3.0, t.panel_bg, Stroke::new(1.0, t.border), StrokeKind::Inside);
    tp.galley(r.min + vec2(7.0, 3.0), g1, seg.colour);
    tp.galley(r.min + vec2(7.0, 19.0), g2, t.text_dim);
    let note = seg.note;
    let (clicked, toggle) =
        ui.input(|i| (i.pointer.button_clicked(egui::PointerButton::Primary), i.modifiers.shift || i.modifiers.command || i.modifiers.ctrl));
    if clicked {
        select_from_lane(app, track.id, note, toggle);
    }
    if ui.input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary)) {
        let at = crate::edit_window::sample_at(&s, tl, pos.x).max(0);
        crate::vocal_editor::open_at(app, track.id, at, Some(note));
        return true;
    }
    false
}

/// A click on a note's line in the track lane: the note becomes the Vocal Editor's selection
/// (Shift or Ctrl: toggled in it), the editor's track this one. The lane's own click (the edit
/// cursor, the clip) still happens.
pub fn select_from_lane(app: &mut SoundApp, track: soundcraft_model::TrackId, r: NoteRef, toggle: bool) {
    if crate::vocal_editor::editor_track(app) != Some(track) {
        app.vocal.selected.clear();
    }
    app.vocal.track = Some(track);
    app.vocal.popup = None;
    roll::click_select(app, Some(r), toggle);
}

/// The TUNED badge of a clip with a live tuning that edits something ("UNTUNED" while it plays
/// the untuned audio): in the clip's name bar right after its name (measured in the name's font).
pub fn tuned_badge(ctx: &egui::Context, s: &Session, tl: Rect, lane: Rect, clip: &Clip) -> Option<(Rect, &'static str)> {
    let tune = clip.tune_live()?;
    let text = if tune.original {
        "UNTUNED"
    } else if tune.edited_count() > 0 {
        "TUNED"
    } else {
        return None;
    };
    let x0 = crate::edit_window::x_of(s, tl, clip.start).max(lane.min.x);
    let x1 = crate::edit_window::x_of(s, tl, clip.end()).min(lane.max.x);
    let w = ctx.fonts_mut(|f| f.layout_no_wrap(text.to_string(), bold(8.0), Color32::WHITE).size().x) + 12.0;
    let name_h = crate::edit_window::clip_name_height(s, lane);
    let (x, y) = if name_h > 0.0 {
        let name_w = ctx.fonts_mut(|f| f.layout_no_wrap(clip.name.clone(), bold(10.5), Color32::WHITE).size().x);
        (x0 + 4.0 + name_w + 8.0, lane.min.y + 2.0)
    } else {
        (x0 + 4.0, lane.min.y + 3.0)
    };
    (x + w <= x1 - 2.0 && lane.height() >= 24.0).then(|| (Rect::from_min_size(pos2(x, y), vec2(w, 11.0)), text))
}

/// Paints the badge.
pub fn paint_badge(painter: &Painter, r: Rect, text: &str) {
    let fill = if text == "TUNED" { TUNED_PINK } else { Color32::from_rgb(96, 96, 102) };
    painter.rect(r, 2.0, fill, Stroke::new(1.0, Color32::from_rgb(12, 12, 12)), StrokeKind::Inside);
    painter.text(r.center(), Align2::CENTER_CENTER, text, bold(8.0), Color32::from_rgb(245, 245, 245));
}

/// The header's "Tuned · n notes edited" colour, readable on the track's header in both themes.
pub fn tuned_text_colour() -> Color32 {
    if crate::theme::is_light() { Color32::from_rgb(150, 20, 95) } else { Color32::from_rgb(240, 110, 185) }
}

/// How many notes of `track`'s clips have an edit (live, Tuned clips).
pub fn edited_notes(track: &Track) -> usize {
    track.clips().iter().filter_map(|c| c.tune_live()).filter(|t| !t.original).map(|t| t.edited_count()).sum()
}
