//! The Vocal Editor: a piano roll of a vocal track's notes (coloured by how far off they are,
//! thick by loudness) with the pitch curves, the Level lane and the Tuning panel, docked beside
//! the MIDI Editor. It only reads the cached analyses and curves and acts through the vocal
//! commands.

pub mod drag;
pub mod layout;
pub mod level_lane;
pub mod note_bar;
pub mod panel;
pub mod presets;
pub mod preview;
pub mod roll;
pub mod window;

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Pos2, Rect, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_engine::vocal::NoteRef;
use soundcraft_model::{ClipId, Session, TrackId, TrackKind};
use soundcraft_time::{Range, Samples};
use std::collections::BTreeSet;

/// The default row height (px per semitone).
pub const ROW_H: f32 = 20.0;
/// Row heights the − / + buttons and Ctrl+Shift+wheel (or Ctrl+Alt+wheel) move between.
pub const ROW_H_RANGE: (f32, f32) = (12.0, 64.0);
/// The factor one − / + click changes the row height by.
pub const ROW_H_STEP: f32 = 1.25;
/// The header row's height.
pub const HEADER_H: f32 = 30.0;
/// The Level lane's height (as the MIDI Editor's velocity lane).
pub const LEVEL_H: f32 = 64.0;
/// The lower dock's height `open_at` (Tune Vocal…) grows it to, when smaller (at most
/// [`DOCK_SHARE`] of the dock area).
pub const DOCK_H: f32 = 620.0;
/// The most of the Edit window's dock area `open_at` grows the lower dock to.
pub const DOCK_SHARE: f32 = 0.6;
/// The Tuning panel's width.
pub const PANEL_W: f32 = 280.0;
/// The zoom factors the header's segmented control offers, relative to the Edit window.
pub const ZOOM_FACTORS: [f32; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];

/// A factor's label: "¼×", "½×", "1×", "2×", "4×".
pub fn zoom_label(f: f32) -> String {
    if (f - 0.25).abs() < 1e-6 {
        "\u{bc}\u{d7}".into()
    } else if (f - 0.5).abs() < 1e-6 {
        "\u{bd}\u{d7}".into()
    } else {
        format!("{f:.0}\u{d7}")
    }
}

/// The editor's state (not saved).
#[derive(Debug, Clone)]
pub struct VocalEditorState {
    /// The track shown; `None` picks one (the selected audio track, else the first vocal one).
    pub track: Option<TrackId>,
    pub selected: BTreeSet<NoteRef>,
    /// Vertical scroll in rows: the roll's top edge is at `12800 − 100·scroll_rows` cents.
    pub scroll_rows: f32,
    /// Row height (px per semitone), within [`ROW_H_RANGE`]; kept in `ui.json` (`vocal.row_h`).
    pub row_h: f32,
    /// Rubber-band selection start (screen position).
    pub band: Option<Pos2>,
    /// The Tuning panel is shown.
    pub show_panel: bool,
    /// The note whose transition popup is open.
    pub popup: Option<NoteRef>,
    /// An `open_at` waiting for the roll's size: track, time and the note to select.
    pub pending_open: Option<(TrackId, Samples, Option<NoteRef>)>,
    /// The track the rows were last centred on (they centre on a track's notes once).
    pub centred: Option<TrackId>,
    /// The selection is the whole track's (Select all, Ctrl+A): levelling then refers to the
    /// track's typical level.
    pub whole_track: bool,
    /// The vocal data derived from the session (analyses, curves, twins, levels, the track
    /// lanes' lines), shared by the editor, the track pitch lanes and the Mix window's PITCH
    /// sections; each clip's and track's keyed on what it derives from.
    pub cache: roll::Cache,
    /// The macro preset chosen in the Tuning panel.
    pub preset: String,
    /// The Save… name field's text while it is open.
    pub save_name: Option<String>,
    /// The macro presets listed (read from disk once, again after a save).
    pub presets: Option<Vec<soundcraft_vocal::MacroPreset>>,
    /// A note drag moving pitch, while it lasts.
    pub drag: Option<drag::NoteDrag>,
    /// Esc cancelled a drag this frame: that Esc clears no selection.
    pub drag_cancelled: bool,
    /// A note-bar field drag, while it lasts.
    pub bar_drag: Option<note_bar::BarDrag>,
    /// The note-bar field being typed into, and its text.
    pub bar_edit: Option<(note_bar::BarField, String)>,
    /// The last click on a note-bar field (field, egui time): a second one soon after types.
    pub bar_click: Option<(note_bar::BarField, f64)>,
    /// A note preview asked for, waiting for its render, or playing.
    pub preview: Option<preview::Preview>,
    /// The roll's own view: session time at its left edge and samples per px. `None` until the
    /// roll is first drawn (then the Edit window's, at the zoom factor).
    pub t0: Option<Samples>,
    pub spp: Option<f64>,
    /// The roll was zoomed by hand (Ctrl+wheel): it keeps its own zoom, no factor selected,
    /// until a factor is clicked again.
    pub hand_zoom: bool,
    /// The Edit window's scroll last seen: the roll moves by the same amount it does.
    pub last_edit_scroll: Option<Samples>,
}

impl Default for VocalEditorState {
    fn default() -> Self {
        Self {
            track: None,
            selected: BTreeSet::new(),
            scroll_rows: 60.0,
            row_h: ROW_H,
            band: None,
            show_panel: true,
            popup: None,
            pending_open: None,
            centred: None,
            whole_track: false,
            cache: roll::Cache::default(),
            preset: "First pass (gentle)".into(),
            save_name: None,
            presets: None,
            drag: None,
            drag_cancelled: false,
            bar_drag: None,
            bar_edit: None,
            bar_click: None,
            preview: None,
            t0: None,
            spp: None,
            hand_zoom: false,
            last_edit_scroll: None,
        }
    }
}

thread_local! {
    /// How many analyses the editor has copied out of the session on this thread (each
    /// `analysis_for` copies one; `track_notes` one per audio clip). The shared
    /// [`roll::Cache`] keeps this flat while nothing changes; tests read it.
    static ANALYSIS_COPIES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The number of analysis copies the editor made so far on this thread.
pub fn analysis_copies() -> u64 {
    ANALYSIS_COPIES.with(std::cell::Cell::get)
}

fn count_copies(n: u64) {
    ANALYSIS_COPIES.with(|c| c.set(c.get().saturating_add(n)));
}

/// `soundcraft_engine::vocal::analysis_for`, counted.
pub(crate) fn analysis_for(s: &Session, c: &soundcraft_model::Clip) -> Option<soundcraft_vocal::VocalAnalysis> {
    count_copies(1);
    soundcraft_engine::vocal::analysis_for(s, c)
}

/// `soundcraft_engine::vocal::track_notes`, counted (it copies every analysed clip's analysis).
pub(crate) fn track_notes(s: &Session, track: TrackId, range: Option<Range>) -> Vec<NoteRef> {
    let clips = s.track(track).map_or(0, |t| t.clips().iter().filter(|c| c.is_audio()).count());
    count_copies(clips as u64);
    soundcraft_engine::vocal::track_notes(s, track, range)
}

/// Stores `v` in the context's temp data as `soundcraft-vocal-<name>` (tests read it).
pub(crate) fn store<T: Clone + Send + Sync + 'static>(ctx: &egui::Context, name: &str, v: T) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(format!("soundcraft-vocal-{name}")), v));
}

/// Whether test hooks are written ([`hook`]): in debug builds (tests) only, so a release build
/// clones and stores nothing per frame for them.
pub fn hooks_on() -> bool {
    cfg!(debug_assertions)
}

/// [`store`] for values only tests read: skipped in release builds ([`hooks_on`]).
pub(crate) fn hook<T: Clone + Send + Sync + 'static>(ctx: &egui::Context, name: &str, v: T) {
    if hooks_on() {
        store(ctx, name, v);
    }
}

/// The track the editor shows: its own when it still exists, else the selected audio track,
/// else the first vocal track.
pub fn editor_track(app: &SoundApp) -> Option<TrackId> {
    let s = app.engine.session();
    if let Some(t) = app.vocal.track.filter(|t| s.track(*t).is_some()) {
        return Some(t);
    }
    s.tracks
        .iter()
        .find(|t| t.kind == TrackKind::Audio && s.edit.selected_tracks.contains(&t.id))
        .or_else(|| s.tracks.iter().find(|t| t.is_vocal()))
        .map(|t| t.id)
}

/// The Edit window's timeline width (px), else `fallback`.
fn timeline_w(app: &SoundApp, fallback: f32) -> f32 {
    let w = app.edit_layout.timeline[2] - app.edit_layout.timeline[0];
    if w.is_finite() && w > 50.0 { w } else { fallback.max(50.0) }
}

/// The roll's samples per px as the zoom factor asks: the Edit window's ÷ the factor.
fn factor_spp(app: &SoundApp) -> f64 {
    let f = f64::from(app.ui.vocal_zoom_factor);
    let f = if f.is_finite() && f > 0.0 { f } else { 1.0 };
    (app.engine.session().edit.zoom.samples_per_px / f).max(0.01)
}

/// The time kept at the same screen x when the roll's zoom changes: the first selected note's
/// start, else the playhead.
fn anchor_time(app: &mut SoundApp, track: TrackId) -> Samples {
    let first = app.vocal.selected.iter().next().copied();
    first.and_then(|r| preview::note_span(app, track, r)).map_or_else(|| app.position(), |(a, _)| a)
}

/// Brings the roll's own view up to date for a roll `roll_w` px wide: it starts as the Edit
/// window's view at the zoom factor, moves with the Edit window's scroll, follows the Edit
/// window's zoom at the factor (unless zoomed by hand), and keeps its anchor at the same x when
/// its zoom changes.
fn sync_view(app: &mut SoundApp, track: TrackId, roll_w: f32) {
    let (scroll, _) = {
        let z = &app.engine.session().edit.zoom;
        (z.scroll, z.samples_per_px)
    };
    let want = if app.vocal.hand_zoom { app.vocal.spp.unwrap_or_else(|| factor_spp(app)) } else { factor_spp(app) };
    let (Some(mut t0), Some(spp)) = (app.vocal.t0, app.vocal.spp) else {
        app.vocal.t0 = Some(scroll);
        app.vocal.spp = Some(want);
        app.vocal.last_edit_scroll = Some(scroll);
        return;
    };
    if let Some(last) = app.vocal.last_edit_scroll
        && last != scroll
    {
        // The Edit window back at the start takes the roll there too: a zoom about an anchor
        // can leave the roll's edge ahead of the Edit window's, and the Edit window can't
        // scroll past 0 to take that offset back.
        t0 = if scroll <= 0 { 0 } else { t0.saturating_add(scroll.saturating_sub(last)).max(0) };
    }
    app.vocal.last_edit_scroll = Some(scroll);
    if (spp - want).abs() > 1e-9 {
        let anchor = anchor_time(app, track);
        let x = (anchor.saturating_sub(t0)) as f64 / spp;
        if x.is_finite() && (0.0..=f64::from(roll_w)).contains(&x) {
            t0 = soundcraft_time::to_samples(anchor as f64 - x * want).max(0);
        }
        app.vocal.spp = Some(want);
    }
    app.vocal.t0 = Some(t0);
}

/// The session time the roll shows across `w` px: its own view ([`sync_view`]).
pub fn view_range(app: &SoundApp, w: f32) -> (Samples, Samples) {
    let t0 = app.vocal.t0.unwrap_or(app.engine.session().edit.zoom.scroll);
    let spp = app.vocal.spp.unwrap_or_else(|| factor_spp(app));
    let span = (spp * f64::from(w.max(1.0))).clamp(1.0, 1e15) as Samples;
    (t0, t0.saturating_add(span.max(1)))
}

/// Sets the zoom factor (the segmented control, Ctrl+Alt+Plus/Minus): the roll follows the
/// Edit window at it again, its anchor kept at the same x.
pub fn set_zoom_factor(app: &mut SoundApp, f: f32) {
    app.ui.vocal_zoom_factor = f;
    app.vocal.hand_zoom = false;
    app.ui.status = format!("Vocal Editor zoom {} the Edit window's", zoom_label(f));
}

/// Sets the row height (px per semitone), within [`ROW_H_RANGE`], and keeps it in `ui.json`.
pub fn set_row_h(app: &mut SoundApp, h: f32) {
    let (lo, hi) = ROW_H_RANGE;
    let h = if h.is_finite() { h.clamp(lo, hi) } else { ROW_H };
    app.vocal.row_h = h;
    app.ui.vocal_row_h = h;
}

/// The − / + buttons: one [`ROW_H_STEP`] shorter or taller, keeping the pitch at the roll's
/// middle where it was.
fn step_row_h(app: &mut SoundApp, taller: bool, roll_h: f32) {
    let old = app.vocal.row_h.max(1.0);
    let mid = app.vocal.scroll_rows + roll_h / old / 2.0;
    set_row_h(app, if taller { old * ROW_H_STEP } else { old / ROW_H_STEP });
    app.vocal.scroll_rows = mid - roll_h / app.vocal.row_h.max(1.0) / 2.0;
}

/// Steps the factor along [`ZOOM_FACTORS`] (`up`: zoom in).
fn step_zoom_factor(app: &mut SoundApp, up: bool) {
    let cur = app.ui.vocal_zoom_factor;
    let i = ZOOM_FACTORS.iter().position(|f| (f - cur).abs() < 1e-6).unwrap_or(2);
    let j = if up { (i + 1).min(ZOOM_FACTORS.len() - 1) } else { i.saturating_sub(1) };
    if let Some(f) = ZOOM_FACTORS.get(j) {
        set_zoom_factor(app, *f);
    }
}

/// The clip the background worker should render first: the first selected note's, else the
/// editor track's clip under the playhead.
pub fn focus_clip(app: &SoundApp) -> Option<ClipId> {
    let s = app.engine.session();
    if let Some(r) = app.vocal.selected.iter().find(|r| s.find_clip(r.clip).is_some()) {
        return Some(r.clip);
    }
    let at = app.position();
    let t = s.track(editor_track(app)?)?;
    t.clips().iter().find(|c| c.is_audio() && c.range().contains(at)).map(|c| c.id)
}

/// Opens the editor on `track` at session time `at`: scrolls the Edit view so `at` sits one
/// third in, centres the rows on the note (or the pitch at `at`) and selects `select` alone.
pub fn open_at(app: &mut SoundApp, track: TrackId, at: Samples, select: Option<NoteRef>) {
    app.ui.show_vocal_editor = true;
    app.vocal.track = Some(track);
    app.vocal.selected = select.into_iter().collect();
    app.vocal.popup = None;
    app.vocal.band = None;
    app.vocal.whole_track = false;
    // Tall enough for the roll and the whole Tuning panel, leaving the Edit window room.
    let grow = crate::docking::edit_dock_height(app).map_or(DOCK_H, |h| DOCK_H.min(h * DOCK_SHARE));
    if let Some(root) = app.ui.docking.layout.root.as_mut() {
        crate::docking::grow_second_holding(root, crate::docking::Panel::Vocal, grow);
    }
    let spp = app.engine.session().edit.zoom.samples_per_px;
    let third = (spp * f64::from(timeline_w(app, 900.0)) / 3.0).clamp(0.0, 1e15) as Samples;
    let _ = app.engine.execute("view.scroll", &json!({"to": at.saturating_sub(third).max(0)}));
    // The roll follows the Edit window's scroll from here, at its own zoom.
    app.vocal.t0 = None;
    app.vocal.spp = None;
    app.vocal.pending_open = Some((track, at, select));
    app.vocal.centred = Some(track);
}

/// `ui.vocal_editor {clip?, track?, at?}`: opens the editor, on the clip's or track's notes at
/// `at` (default: the clip's start, else the playhead) when one is given.
pub fn open_command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    const ID: &str = "ui.vocal_editor";
    let clip = match p.get("clip") {
        None => None,
        Some(v) => {
            let id = v.as_u64().map(ClipId).ok_or("`clip` must be a clip id")?;
            let (t, c) = app.engine.session().find_clip(id).ok_or_else(|| format!("no clip with id {}", id.0))?;
            if !c.is_audio() {
                return Err(format!("`{}` is not an audio clip", c.name));
            }
            Some((t, c.start))
        }
    };
    let track = soundcraft_engine::cmd::track_param(&app.engine, ID, p, "track").map_err(|e| e.to_string())?;
    let at = soundcraft_engine::cmd::position_param(&app.engine, ID, p, "at").map_err(|e| e.to_string())?;
    match track.or(clip.map(|c| c.0)) {
        Some(t) => {
            let at = at.or(clip.map(|c| c.1)).unwrap_or_else(|| app.position());
            open_at(app, t, at, None);
            Ok(json!({"track": t.0, "at": at}))
        }
        None => {
            app.ui.show_vocal_editor = true;
            Ok(json!({}))
        }
    }
}

/// What the roll shows instead of notes, and whether it offers "Mark as vocal".
fn empty_state(app: &mut SoundApp, track: Option<TrackId>, range: Range) -> Option<(String, bool)> {
    let s = app.engine.session_arc();
    let Some(t) = track.and_then(|t| s.track(t)).filter(|t| t.kind == TrackKind::Audio) else {
        return Some(("Pick an audio track".into(), false));
    };
    if !t.is_vocal() {
        return Some(("Not marked as a vocal track".into(), true));
    }
    let audio: Vec<_> = t.clips().iter().filter(|c| c.is_audio()).collect();
    if audio.is_empty() {
        return Some((format!("No audio clips on {}", t.name), false));
    }
    let shown: Vec<&soundcraft_model::Clip> = audio.iter().copied().filter(|c| c.range().overlaps(&range)).collect();
    let ids: Vec<ClipId> = shown.iter().map(|c| c.id).collect();
    if shown.is_empty() || roll::any_analysed(app, &ids) {
        return None;
    }
    // Missing audio, too long, or failed: the worker never offers them, so don't wait.
    if shown.iter().all(|c| soundcraft_engine::vocal::cannot_analyse(&s, c)) {
        return Some((soundcraft_engine::vocal::cannot_analyse_text(&t.name), false));
    }
    Some((format!("Analysing {}\u{2026}", t.name), false))
}

/// The waveforms of `track`'s audio clips across `rect` (session time `[t0, t1)`), faint: all of
/// them, or only those without an analysis yet. Stores their rects as `waves` for the tests.
pub(crate) fn draw_waves(app: &mut SoundApp, ui: &Ui, track: TrackId, rect: Rect, (t0, t1): (Samples, Samples), only_unanalysed: bool) {
    let s = app.engine.session_arc();
    let Some(t) = s.track(track) else { return };
    let spp = (t1.saturating_sub(t0)).max(1) as f64 / f64::from(rect.width().max(1.0));
    let dark = Tokens::current().panel_bg.r() < 128;
    let wave = if dark { egui::Color32::from_rgba_unmultiplied(150, 160, 175, 110) } else { egui::Color32::from_rgba_unmultiplied(70, 80, 95, 110) };
    let mut drawn = Vec::new();
    for c in t.clips().iter().filter(|c| c.is_audio() && c.end() > t0 && c.start < t1) {
        if only_unanalysed && roll::any_analysed(app, &[c.id]) {
            continue;
        }
        let Some(audio) = c.source().and_then(|id| s.pool.get(id)) else { continue };
        let x = |at: Samples| rect.left() + (at.saturating_sub(t0) as f64 / spp) as f32;
        let r = Rect::from_min_max(pos2(x(c.start).max(rect.left()), rect.top()), pos2(x(c.end()).min(rect.right()), rect.bottom()));
        if r.width() <= 0.0 {
            continue;
        }
        let body = Rect::from_min_max(pos2(x(c.start), rect.top()), pos2(x(c.end()), rect.bottom()));
        crate::waveform::draw_mapped(&ui.painter().with_clip_rect(r), &s, (rect.left(), t0 as f64, spp), c, audio, t.channels(), body, wave);
        drawn.push((c.id, r));
    }
    store(ui.ctx(), "waves", drawn);
}

/// Whether the editor takes `key` from the app's shortcuts: S, J, Escape, Delete,
/// Ctrl(+Shift)+A, Space (and Shift+Space) and, with notes selected, Alt+Up/Down while the
/// pointer is over it (Alt+Shift+Up/Down stays the app's Copy Selection to Target/New Playlist).
pub fn owns_key(app: &SoundApp, ctx: &egui::Context, key: egui::Key, m: egui::Modifiers) -> bool {
    use egui::Key;
    // The editor is in the Edit window, or in its own: keys typed into another window (the
    // separate Mix window, or Edit while the editor has its own) aren't its.
    if !app.ui.show_vocal_editor || ctx.viewport_id() != window::home(ctx) {
        return false;
    }
    // Only a rect drawn this pass or the one before: the editor not drawn (Mix window, another
    // tab in front, the dock moved) leaves a stale one behind.
    let area: Option<(Rect, u64)> = ctx.data(|d| d.get_temp(egui::Id::new("soundcraft-vocal-area-pass")));
    let area = area.filter(|(_, pass)| pass + 1 >= ctx.cumulative_pass_nr()).map(|(r, _)| r);
    let over = area.zip(ctx.pointer_latest_pos()).is_some_and(|(r, p)| r.contains(p));
    let plain = !m.command && !m.ctrl && !m.alt;
    over && match key {
        Key::Minus | Key::Plus | Key::Equals => (m.command || m.ctrl) && m.alt,
        Key::S | Key::J => plain && !m.shift,
        Key::Escape | Key::Delete | Key::Backspace => plain,
        Key::A => m.command && !m.alt,
        Key::Space => !m.command && !m.ctrl && !m.alt,
        Key::ArrowUp | Key::ArrowDown => m.alt && !m.shift && !m.command && !m.ctrl && !app.vocal.selected.is_empty(),
        _ => false,
    }
}

/// Draws the Vocal Editor into `ui`.
pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    ui.take_available_space();
    // Set by a drag cancelled this frame (see `interact`), read by `keyboard` below; an Esc
    // pressed with the pointer off the editor must not leave it for the next Esc over it.
    app.vocal.drag_cancelled = false;
    // The row height lives in ui.json.
    if app.vocal.row_h != app.ui.vocal_row_h {
        set_row_h(app, app.ui.vocal_row_h);
    }
    let t = Tokens::current();
    let full = ui.max_rect();
    store(ui.ctx(), "area", full);
    store(ui.ctx(), "area-pass", (full, ui.ctx().cumulative_pass_nr()));
    store(ui.ctx(), "viewport", ui.ctx().viewport_id());
    ui.painter().rect_filled(full, 0.0, t.panel_bg);
    let header = Rect::from_min_size(full.min, vec2(full.width(), HEADER_H));
    ui.painter().rect_filled(header, 0.0, t.panel_bg2);
    ui.painter().line_segment([header.left_bottom(), header.right_bottom()], egui::Stroke::new(1.0, t.border));
    ui.painter().text(pos2(header.min.x + 10.0, header.center().y), Align2::LEFT_CENTER, "VOCAL EDITOR", bold(12.5), t.header_text);
    let title = Rect::from_min_max(header.min, pos2(header.min.x + 110.0, header.max.y));
    ui.interact(title, ui.id().with("vocal_title"), egui::Sense::hover()).on_hover_text("Zoom: the Edit window's × the factor (Ctrl+wheel: by hand)");
    let body = Rect::from_min_max(pos2(full.min.x, header.max.y), full.max);
    let track = editor_track(app);
    let panel_w = if app.vocal.show_panel { PANEL_W } else { 0.0 };
    if let Some(t) = track {
        sync_view(app, t, body.width() - roll::KEY_W - panel_w);
    }
    let (t0, t1) = view_range(app, body.width() - roll::KEY_W - panel_w);
    let empty = empty_state(app, track, Range::new(t0, t1));
    if let Some((msg, mark)) = empty {
        if msg.starts_with("Analysing")
            && let Some(id) = track
        {
            // The clips' audio while the analysis runs (the roll fills in when it lands).
            let wr = Rect::from_min_max(pos2(body.left() + roll::KEY_W, body.top() + 40.0), pos2(body.right() - panel_w, body.bottom() - 40.0));
            draw_waves(app, ui, id, wr, (t0, t1), false);
        } else {
            store(ui.ctx(), "waves", Vec::<(ClipId, Rect)>::new());
        }
        ui.painter().text(body.center(), Align2::CENTER_CENTER, &msg, regular(13.0), t.text_dim);
        store(ui.ctx(), "message", msg);
        if mark && let Some(id) = track {
            let r = Rect::from_center_size(body.center() + vec2(0.0, 30.0), vec2(120.0, 24.0));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
            let b = child.add_sized(r.size(), egui::Button::new("Mark as vocal")).on_hover_text("Turn on Track \u{25b8} Vocal Track for this track");
            store(ui.ctx(), "mark", b.rect);
            if b.clicked()
                && let Err(e) = app.run(crate::vocal_ids::TRACK_VOCAL, json!({"tracks": [id.0], "value": true}))
            {
                app.ui.status = e;
            }
        }
        return;
    }
    ui.ctx().data_mut(|d| d.remove::<String>(egui::Id::new("soundcraft-vocal-message")));
    let Some(track) = track else { return };
    roll::prune(app, track);
    let right = (body.right() - panel_w).max(body.left() + roll::KEY_W + 60.0);
    let (l, top) = (body.left() + roll::KEY_W, body.top());
    let chords = Rect::from_min_max(pos2(l, top), pos2(right, top + roll::CHORD_H));
    let key = Rect::from_min_max(pos2(l, chords.bottom()), pos2(right, chords.bottom() + roll::KEY_STRIP_H));
    let ruler = Rect::from_min_max(pos2(l, key.bottom()), pos2(right, key.bottom() + roll::RULER_H));
    let lane_top = (body.bottom() - LEVEL_H).max(ruler.bottom() + 40.0);
    let roll_r = Rect::from_min_max(pos2(l, ruler.bottom()), pos2(right, lane_top - 4.0));
    let keys = Rect::from_min_max(pos2(body.left(), roll_r.top()), pos2(l, roll_r.bottom()));
    let lane = Rect::from_min_max(pos2(body.left(), lane_top), pos2(right, body.bottom()));
    let areas = roll::Areas { chords, key, ruler, roll: roll_r, keys };
    sync_view(app, track, roll_r.width());
    centre_rows(app, track, roll_r.height());
    wheel(app, ui, roll_r);
    let (t0, t1) = view_range(app, roll_r.width());
    let rows = roll_r.height() / app.vocal.row_h.max(1.0);
    app.vocal.scroll_rows = app.vocal.scroll_rows.clamp(0.0, (128.0 - rows).max(0.0));
    let view = layout::RollView { rect: roll_r, t0, t1, top_cents: roll::TOP_CENTS - app.vocal.scroll_rows * 100.0, row_h: app.vocal.row_h };
    let drawn = roll::draw(app, ui, track, &areas, view);
    header_row(app, ui, header, track, &drawn);
    roll::interact(app, ui, track, &drawn);
    note_bar::show(app, ui, track, &drawn);
    keyboard(app, ui, full, track, &drawn);
    level_lane::level_lane(app, ui, lane, &drawn.view);
    if panel_w > 0.0 {
        panel::panel(app, ui, Rect::from_min_max(pos2(right, body.top()), body.max));
    }
}

/// Centres the rows on the track's notes the first time it is shown, and on an `open_at`'s note
/// (or the pitch at its time).
fn centre_rows(app: &mut SoundApp, track: TrackId, roll_h: f32) {
    let rows = roll_h / app.vocal.row_h.max(1.0);
    let centre_on = |app: &mut SoundApp, cents: f32| {
        if cents.is_finite() {
            app.vocal.scroll_rows = (roll::TOP_CENTS - cents) / 100.0 - rows / 2.0;
        }
    };
    if let Some((t, at, note)) = app.vocal.pending_open.take() {
        if t != track {
            return;
        }
        let cvs = roll::clip_views(app, track, None);
        let by_note = note
            .and_then(|r| cvs.iter().find(|c| c.clip.id == r.clip).and_then(|c| c.a.notes.iter().find(|n| n.id == r.note)).map(|n| n.centre_cents));
        let by_time = cvs.iter().find(|c| c.clip.range().contains(at)).and_then(|c| {
            let fm = layout::Frames::of(&c.clip, c.a.track.hop_samples);
            let f = fm.frame(at);
            c.a.notes
                .iter()
                .find(|n| n.start <= f && f < n.end)
                .map(|n| n.centre_cents)
                .or_else(|| c.a.track.frames.get(f as usize).and_then(|x| x.cents))
        });
        if let Some(c) = by_note.or(by_time) {
            centre_on(app, c);
        }
        app.vocal.centred = Some(track);
        return;
    }
    if app.vocal.centred == Some(track) {
        return;
    }
    let mut centres: Vec<f32> =
        roll::clip_views(app, track, None).iter().flat_map(|c| c.a.notes.iter().map(|n| n.centre_cents)).filter(|c| c.is_finite()).collect();
    if centres.is_empty() {
        return;
    }
    centres.sort_by(f32::total_cmp);
    if let Some(m) = centres.get(centres.len() / 2).copied() {
        centre_on(app, m);
    }
    app.vocal.centred = Some(track);
}

/// The wheel over the roll: rows (plain), time (Shift or sideways), the roll's own zoom (Ctrl:
/// a hand zoom, which deselects the factor) and row height (Ctrl+Shift, or Ctrl+Alt as in the
/// MIDI Editor).
fn wheel(app: &mut SoundApp, ui: &Ui, roll: Rect) {
    if !ui.rect_contains_pointer(roll) {
        return;
    }
    let events: Vec<(egui::Vec2, egui::Modifiers, Pos2)> = ui.input(|i| {
        let p = i.pointer.latest_pos().unwrap_or(roll.center());
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::MouseWheel { unit, delta, modifiers, .. } => {
                    let k = match unit {
                        egui::MouseWheelUnit::Point => 1.0,
                        egui::MouseWheelUnit::Line => 20.0,
                        egui::MouseWheelUnit::Page => roll.height(),
                    };
                    Some((*delta * k, *modifiers, p))
                }
                _ => None,
            })
            .collect()
    });
    let to_edit_px = timeline_w(app, roll.width()) / roll.width().max(1.0);
    for (d, m, p) in events {
        let ctrl = m.command || m.ctrl;
        if ctrl && (m.shift || m.alt) {
            let dy = if d.y != 0.0 { d.y } else { d.x };
            let h = app.vocal.row_h * (1.0 + dy * 0.004);
            set_row_h(app, h);
        } else if ctrl {
            // By hand: about the pointer, the factor buttons deselected.
            let factor = 1.0015f64.powf(f64::from(d.y));
            let (t0, spp) = (app.vocal.t0.unwrap_or(0), app.vocal.spp.unwrap_or_else(|| factor_spp(app)));
            let x = f64::from(p.x - roll.left());
            let at = t0 as f64 + x * spp;
            let new = (spp * factor).clamp(0.01, 1e9);
            app.vocal.spp = Some(new);
            app.vocal.t0 = Some(soundcraft_time::to_samples(at - x * new).max(0));
            app.vocal.hand_zoom = true;
        } else {
            let dx = if m.shift { d.x + d.y } else { d.x };
            if dx != 0.0 {
                let by_px = f64::from(-dx * to_edit_px);
                let (before, edit_spp) = {
                    let z = &app.engine.session().edit.zoom;
                    (z.scroll, z.samples_per_px)
                };
                let _ = app.engine.execute("view.scroll", &json!({"by_px": by_px}));
                // What the Edit window couldn't take (it stops at 0) moves the roll itself, so
                // the roll reaches the session start even when its edge is ahead of the Edit
                // window's.
                let moved = app.engine.session().edit.zoom.scroll.saturating_sub(before);
                let asked = soundcraft_time::to_samples(by_px * edit_spp);
                let rest = asked.saturating_sub(moved);
                if rest < 0
                    && let Some(t0) = app.vocal.t0
                {
                    app.vocal.t0 = Some(t0.saturating_add(rest).max(0));
                }
            }
            if !m.shift && d.y != 0.0 {
                app.vocal.scroll_rows -= d.y / app.vocal.row_h.max(1.0);
            }
        }
    }
}

/// The header's right-hand bar width (px): Select all / Split / Join / Preview / row height /
/// zoom / Tuning / Window (Dock).
const HEADER_BAR_W: f32 = 740.0;

/// The header: "VOCAL EDITOR", the counts and hints, and Select all / Split / Join / Preview /
/// Tuning.
fn header_row(app: &mut SoundApp, ui: &mut Ui, header: Rect, track: TrackId, d: &roll::Drawn) {
    let t = Tokens::current();
    let s = app.engine.session_arc();
    let name = s.track(track).map(|t| t.name.clone()).unwrap_or_default();
    let total = roll::note_count(app, track);
    let k = app.vocal.selected.len();
    let p = ui.painter().with_clip_rect(Rect::from_min_max(header.min, pos2(header.max.x - HEADER_BAR_W, header.max.y)));
    let mut x = header.left() + 112.0;
    let selected = if app.vocal.whole_track && k > 0 { format!("Select all \u{b7} {k} notes") } else { format!("{k} selected") };
    hook(ui.ctx(), "selected-text", selected.clone());
    for (text, strong) in [
        (format!("{name} \u{b7} {total} notes \u{b7} "), false),
        (selected, true),
        (" \u{b7} drag a note: cents, snaps at semitones (Alt: free) \u{b7} drag: rubber-band \u{b7} S split \u{b7} J join".to_string(), false),
    ] {
        let g = p.layout_no_wrap(text, if strong { bold(11.5) } else { regular(11.5) }, if strong { t.text } else { t.text_dim });
        let w = g.size().x;
        p.galley(pos2(x, header.center().y - g.size().y / 2.0), g, t.text);
        x += w;
    }
    let bar = Rect::from_min_max(pos2(header.max.x - HEADER_BAR_W, header.min.y + 3.0), pos2(header.max.x - 8.0, header.max.y - 3.0));
    let mut tb = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::right_to_left(egui::Align::Center)));
    tb.spacing_mut().item_spacing.x = 6.0;
    let sep = app.ui.vocal_separate;
    let win = tb.button(if sep { "Dock" } else { "Window" });
    store(ui.ctx(), "separate", win.rect);
    if win.on_hover_text(if sep { "Put the Vocal Editor back in the Edit window" } else { "Open the Vocal Editor in its own window" }).clicked() {
        let _ = app.run("window.vocal_separate", json!({}));
    }
    let on = app.vocal.show_panel;
    let tuning = tb.add(egui::Button::new(egui::RichText::new("Tuning").color(if on { t.mode_on_text } else { t.text })).fill(if on {
        t.mode_on
    } else {
        t.button
    }));
    store(ui.ctx(), "tuning", tuning.rect);
    if tuning.on_hover_text("Show or hide the Tuning panel").clicked() {
        let _ = app.run("ui.vocal_panel", json!({}));
    }
    zoom_control(app, &mut tb);
    row_h_control(app, &mut tb, d.view.rect.height());
    let pv = app.ui.vocal_preview;
    let preview = tb.add(egui::Button::new(egui::RichText::new("Preview").color(if pv { t.mode_on_text } else { t.text })).fill(if pv {
        t.mode_on
    } else {
        t.button
    }));
    store(ui.ctx(), "preview", preview.rect);
    if preview.on_hover_text("Play a note once when it is clicked or edited (the track soloed)").clicked() {
        app.ui.vocal_preview = !pv;
        app.ui.status = if pv { "Preview off".into() } else { "Preview on: a clicked or edited note plays once".into() };
    }
    let join = tb.button("Join");
    store(ui.ctx(), "join", join.rect);
    if join.on_hover_text("Join two neighbouring selected notes (J)").clicked() {
        join_selected(app);
    }
    let split_b = tb.button("Split");
    store(ui.ctx(), "split", split_b.rect);
    if split_b.on_hover_text("Split the selected note at the playhead, or in its middle (S splits under the mouse)").clicked() {
        split_selected(app, d);
    }
    let all = tb.button("Select all");
    store(ui.ctx(), "select-all", all.rect);
    if all.on_hover_text("Select every note of the clips shown (Ctrl+A; Ctrl+Shift+A: only those in view)").clicked() {
        select_all(app, track, d, false);
    }
}

/// The zoom factor's segmented control, right of Preview: ¼× ½× 1× 2× 4× relative to the Edit
/// window; none lit after a hand zoom.
fn zoom_control(app: &mut SoundApp, tb: &mut Ui) {
    let t = Tokens::current();
    let chosen = (!app.vocal.hand_zoom).then_some(app.ui.vocal_zoom_factor);
    hook(tb.ctx(), "zoom-factor", chosen);
    tb.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        // Right to left: the largest factor first.
        for f in ZOOM_FACTORS.iter().rev() {
            let on = chosen.is_some_and(|c| (c - f).abs() < 1e-6);
            let label = zoom_label(*f);
            let b = ui.add(egui::Button::new(egui::RichText::new(&label).color(if on { t.mode_on_text } else { t.text })).fill(if on {
                t.mode_on
            } else {
                t.button
            }));
            store(ui.ctx(), &format!("zoom-{}", label.trim_end_matches('\u{d7}')), b.rect);
            if b.on_hover_text(format!("Roll zoom: {label} the Edit window's (Ctrl+Alt+Plus/Minus step it)")).clicked() {
                set_zoom_factor(app, *f);
            }
        }
    });
}

/// The row height's − / + buttons, left of the zoom factors (the vertical zoom).
fn row_h_control(app: &mut SoundApp, tb: &mut Ui, roll_h: f32) {
    let (lo, hi) = ROW_H_RANGE;
    let h = app.vocal.row_h;
    hook(tb.ctx(), "row-h", h);
    tb.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        // Right to left: + first.
        let plus = ui.add_enabled(h < hi - 0.01, egui::Button::new("+"));
        store(ui.ctx(), "row-taller", plus.rect);
        if plus.on_hover_text("Taller rows: zoom in vertically (Ctrl+Shift+wheel)").clicked() {
            step_row_h(app, true, roll_h);
        }
        let minus = ui.add_enabled(h > lo + 0.01, egui::Button::new("\u{2212}"));
        store(ui.ctx(), "row-shorter", minus.rect);
        if minus.on_hover_text("Shorter rows: zoom out vertically (Ctrl+Shift+wheel)").clicked() {
            step_row_h(app, false, roll_h);
        }
    });
    tb.label(egui::RichText::new("Rows").color(Tokens::current().text_dim)).on_hover_text("Row height (vertical zoom)");
}

/// Space over the editor: stops the transport when it plays; else with notes selected plays
/// from the first selected note's start to the last selected note's end and stops there;
/// else (or with Shift) plays from the playhead as everywhere.
fn space_key(app: &mut SoundApp, track: TrackId, from_playhead: bool) {
    if app.is_playing() {
        let _ = app.run("transport.stop", json!({}));
        return;
    }
    let span = if from_playhead { None } else { selection_span(app, track) };
    match span {
        Some((a, b)) => {
            app.vocal.preview = None;
            app.start_play(a, Some(b), None);
            app.ui.status = "Playing the selected notes".into();
        }
        None => {
            let _ = app.run("transport.play", json!({}));
        }
    }
}

/// The selected notes' span in session time: the first one's start to the last one's end.
pub fn selection_span(app: &mut SoundApp, track: TrackId) -> Option<(Samples, Samples)> {
    let sel: Vec<NoteRef> = app.vocal.selected.iter().copied().collect();
    let mut span: Option<(Samples, Samples)> = None;
    for r in sel {
        let Some((a, b)) = preview::note_span(app, track, r) else { continue };
        span = Some(span.map_or((a, b), |(x, y)| (x.min(a), y.max(b))));
    }
    span
}

/// Select all (Ctrl+A): every note of the clips the editor shows (those overlapping the view),
/// in view or not; `in_view` (Ctrl+Shift+A): only the notes in view. The roll doesn't scroll.
fn select_all(app: &mut SoundApp, track: TrackId, d: &roll::Drawn, in_view: bool) {
    let s = app.engine.session();
    let stretched: Vec<ClipId> = d.clips.iter().filter(|c| c.stretched).map(|c| c.clip.id).collect();
    let shown: Vec<ClipId> = d.clips.iter().map(|c| c.clip.id).collect();
    let range = if in_view { Some(Range::new(d.view.t0, d.view.t1)) } else { None };
    app.vocal.selected = track_notes(s, track, range).into_iter().filter(|r| !stretched.contains(&r.clip) && shown.contains(&r.clip)).collect();
    app.vocal.whole_track = true;
}

/// Splits note `r` at session time `at`; the halves become the selection.
fn split(app: &mut SoundApp, r: NoteRef, at: Samples) {
    match app.run(crate::vocal_ids::VOCAL_SPLIT, json!({"clip": r.clip.0, "note": r.note.0, "at": at})) {
        Ok(v) => {
            let halves: Vec<NoteRef> = serde_json::from_value(v["notes"].clone()).unwrap_or_default();
            app.vocal.selected = halves.into_iter().collect();
            app.vocal.whole_track = false;
        }
        Err(e) => app.ui.status = e,
    }
}

/// The Split button: the one selected note, at the playhead when inside it, else in its middle.
fn split_selected(app: &mut SoundApp, d: &roll::Drawn) {
    let sel: Vec<NoteRef> = app.vocal.selected.iter().copied().collect();
    let [r] = sel[..] else {
        app.ui.status = "Split needs one selected note".into();
        return;
    };
    let Some(cv) = d.clips.iter().find(|c| c.clip.id == r.clip) else { return };
    let Some(n) = cv.a.notes.iter().find(|n| n.id == r.note) else { return };
    let fm = layout::Frames::of(&cv.clip, cv.a.track.hop_samples);
    let (a, b) = (fm.time(n.start), fm.time(n.end));
    let ph = app.position();
    let at = if ph > a && ph < b { ph } else { a + (b - a) / 2 };
    split(app, r, at);
}

/// Joins the two selected notes when they are neighbours in one clip.
fn join_selected(app: &mut SoundApp) {
    let sel: Vec<NoteRef> = app.vocal.selected.iter().copied().collect();
    let pair = match sel[..] {
        [a, b] if a.clip == b.clip => {
            let s = app.engine.session();
            s.find_clip(a.clip).and_then(|(_, c)| analysis_for(s, c)).and_then(|an| {
                let i = an.notes.iter().position(|n| n.id == a.note)?;
                let j = an.notes.iter().position(|n| n.id == b.note)?;
                if j == i + 1 {
                    Some((a, b))
                } else if i == j + 1 {
                    Some((b, a))
                } else {
                    None
                }
            })
        }
        _ => None,
    };
    let Some((a, b)) = pair else {
        app.ui.status = "Join needs two neighbouring notes".into();
        return;
    };
    match app.run(crate::vocal_ids::VOCAL_JOIN, json!({"clip": a.clip.0, "first": a.note.0, "second": b.note.0})) {
        Ok(v) => {
            if let Ok(r) = serde_json::from_value::<NoteRef>(v["note"].clone()) {
                app.vocal.selected = [r].into_iter().collect();
                app.vocal.whole_track = false;
            }
        }
        Err(e) => app.ui.status = e,
    }
}

/// The keys while the pointer is over the editor (never while a text field has focus).
fn keyboard(app: &mut SoundApp, ui: &Ui, full: Rect, track: TrackId, d: &roll::Drawn) {
    use egui::Key;
    if !ui.rect_contains_pointer(full) || ui.ctx().egui_wants_keyboard_input() {
        return;
    }
    let (zoom_in, zoom_out) = ui.input(|i| {
        let m = i.modifiers;
        let chord = (m.command || m.ctrl) && m.alt;
        (chord && (i.key_pressed(Key::Plus) || i.key_pressed(Key::Equals)), chord && i.key_pressed(Key::Minus))
    });
    if zoom_in || zoom_out {
        step_zoom_factor(app, zoom_in);
    }
    let (all, in_view_all, s_key, j_key, esc, space, shift_space) = ui.input(|i| {
        let m = i.modifiers;
        let plain = !m.command && !m.ctrl && !m.alt && !m.shift;
        let no_cmd = !m.command && !m.ctrl && !m.alt;
        (
            m.command && !m.shift && i.key_pressed(Key::A),
            m.command && m.shift && i.key_pressed(Key::A),
            plain && i.key_pressed(Key::S),
            plain && i.key_pressed(Key::J),
            i.key_pressed(Key::Escape),
            plain && i.key_pressed(Key::Space),
            no_cmd && m.shift && i.key_pressed(Key::Space),
        )
    });
    if space || shift_space {
        space_key(app, track, shift_space);
    }
    let pointer = ui.ctx().pointer_latest_pos();
    if all {
        select_all(app, track, d, false);
    }
    if in_view_all {
        select_all(app, track, d, true);
    }
    if s_key
        && let Some(p) = pointer
        && let Some(sh) = d.shapes.iter().rev().find(|sh| sh.rect.expand(2.0).contains(p))
    {
        let r = sh.r;
        split(app, r, d.view.at(p.x));
    }
    if j_key {
        join_selected(app);
    }
    drag::keys(app, ui, track);
    if esc {
        // Mid-drag, Esc cancels the move (see `drag::update`) and leaves the selection alone.
        if !std::mem::take(&mut app.vocal.drag_cancelled) {
            app.vocal.selected.clear();
            app.vocal.popup = None;
            app.vocal.whole_track = false;
        }
    }
}
