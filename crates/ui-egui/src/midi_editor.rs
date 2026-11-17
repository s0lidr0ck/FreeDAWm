//! The MIDI Editor: a piano roll with a velocity lane and controller lanes
//! ([`crate::midi_lanes`]), docked under the Edit window.
//!
//! It has its own zoom and scroll, kept per clip for the session ([`MidiView`]): a clip opens
//! fitted across the roll; Ctrl+wheel zooms time around the pointer, Ctrl+Alt+wheel the note
//! height (5-30 points), Shift+wheel (or the scrollbar under the roll) scrolls time and the wheel
//! alone scrolls the notes; the toolbar's Zoom and Height buttons do the same, and Fit (or F over
//! the roll) fits the clip again. The navigating and editing tools (the zoom factors, the Grid
//! menu, bar lines, the note bar, Select all / Split / Join, the piano keys, Space) live in
//! [`crate::midi_edit_tools`], ghost notes in [`crate::midi_ghosts`].

use crate::midi_edit_tools as tools;
use crate::theme::{Tokens, bold, regular};
use crate::{SoundApp, midi_cleanup_panel, midi_ghosts};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{ClipContent, ClipId, Tool};
use std::collections::HashMap;

const KEY_W: f32 = 48.0;
/// The note height a clip opens with.
const ROW_H: f32 = 9.0;
/// The note heights Ctrl+Alt+wheel and the Height buttons reach.
pub const ROW_H_RANGE: (f32, f32) = (5.0, 30.0);
/// The closest time zoom: points per tick (a quarter note 480 points wide).
pub const MAX_PX_PER_TICK: f32 = 0.5;
/// One Zoom or Height button press.
const STEP: f32 = 1.5;
const VEL_H: f32 = 64.0;
/// The horizontal scrollbar under the roll.
const BAR_H: f32 = 8.0;

/// One clip's zoom and scroll in the editor (kept for the session, not saved).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MidiView {
    /// Points per tick; `None` fits the whole clip across the roll.
    pub zoom: Option<f32>,
    /// The tick (from the clip's start) at the roll's left edge when zoomed.
    pub left: f64,
    /// Note row height, in points.
    pub row_h: f32,
    /// Vertical scroll, in points from pitch 127's row.
    pub scroll: f32,
}

impl Default for MidiView {
    fn default() -> Self {
        MidiView { zoom: None, left: 0.0, row_h: ROW_H, scroll: 0.0 }
    }
}

/// The roll as last drawn: where ticks and pitches are on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RollGeometry {
    pub clip: ClipId,
    pub roll: Rect,
    pub px_per_tick: f32,
    /// What fitting the clip would be.
    pub fit: f32,
    pub left: f64,
    pub row_h: f32,
    pub scroll: f32,
    /// Folded to the scale: the pitches that have a row (bit `p` for pitch `p`); 0, every pitch.
    pub fold: u128,
}

impl RollGeometry {
    /// Rows above `pitch`'s row (fractional for a pitch with no row when folded: it sits on the
    /// line under the row above it).
    fn row_of(&self, pitch: i32) -> f32 {
        if self.fold == 0 {
            return (127 - pitch) as f32;
        }
        if pitch > 127 {
            return -((pitch - 127) as f32);
        }
        let above = if pitch >= 127 { 0 } else { (self.fold >> ((pitch.max(-1) + 1) as u32)).count_ones() };
        let below0 = if pitch < 0 { (-pitch) as u32 - 1 } else { 0 };
        (above + below0) as f32
    }

    /// How many rows the roll has.
    pub fn rows(&self) -> i32 {
        if self.fold == 0 { 128 } else { self.fold.count_ones() as i32 }
    }

    pub fn x_of(&self, tick: i64) -> f32 {
        self.roll.min.x + ((tick as f64 - self.left) * f64::from(self.px_per_tick)) as f32
    }

    /// The tick (from the clip's start) under `x`.
    pub fn tick_at(&self, x: f32) -> f64 {
        self.left + f64::from(x - self.roll.min.x) / f64::from(self.px_per_tick.max(1e-6))
    }

    pub fn y_of(&self, pitch: i32) -> f32 {
        self.roll.min.y + self.row_of(pitch) * self.row_h - self.scroll
    }

    pub fn pitch_at(&self, y: f32) -> i32 {
        let row = ((y - self.roll.min.y + self.scroll) / self.row_h.max(1.0)).floor() as i32;
        if self.fold == 0 {
            return 127 - row;
        }
        if row < 0 {
            return 127 - row;
        }
        let mut seen = 0;
        let mut lowest = 127;
        for p in (0..=127).rev() {
            if self.fold >> p & 1 == 1 {
                if seen == row {
                    return p;
                }
                seen += 1;
                lowest = p;
            }
        }
        lowest - (row - seen + 1)
    }
}

/// `v` zoomed by `factor` keeping the tick under `x` (points from the roll's left) still, never
/// wider than fitting `len` ticks into `width` points nor closer than [`MAX_PX_PER_TICK`].
pub fn zoom_about(v: MidiView, factor: f32, x: f32, width: f32, len: i64) -> MidiView {
    zoom_about_span(v, factor, x, width, (0, len))
}

/// [`zoom_about`] over the span `lo..hi` (ticks from the edited clip's start: with several
/// clips in the editor the roll spans them all, so `lo` can be before the clip).
pub fn zoom_about_span(v: MidiView, factor: f32, x: f32, width: f32, (lo, hi): (i64, i64)) -> MidiView {
    let fit = width / hi.saturating_sub(lo).max(1) as f32;
    let now = v.zoom.unwrap_or(fit).max(fit);
    let new = (now * factor).clamp(fit, MAX_PX_PER_TICK.max(fit));
    let under = v.left + f64::from(x) / f64::from(now);
    let left = under - f64::from(x) / f64::from(new);
    let zoom = (new > fit * 1.0001).then_some(new);
    clamp_left(MidiView { zoom, left: if zoom.is_some() { left } else { lo as f64 }, ..v }, width, (lo, hi))
}

/// `v` fitted to `notes`: their time from the first start to the last end (with a little room
/// either side) across `width`, and, with `rows`, their pitches from the highest to the lowest
/// (a row's room above and below) down `height`. Unchanged with no notes.
pub fn fit_notes(v: MidiView, notes: &[&soundcraft_midi::Note], width: f32, height: f32, span: (i64, i64), rows: bool) -> MidiView {
    let (Some(t0), Some(t1)) = (notes.iter().map(|n| n.start).min(), notes.iter().map(|n| n.end()).max()) else { return v };
    let len = t1.saturating_sub(t0).max(soundcraft_time::TICKS_PER_QUARTER / 4);
    let pad = len / 16;
    let fit = width / span.1.saturating_sub(span.0).max(1) as f32;
    let z = (width / (len + 2 * pad).max(1) as f32).clamp(fit, MAX_PX_PER_TICK.max(fit));
    let zoom = (z > fit * 1.0001).then_some(z);
    let mut out = clamp_left(MidiView { zoom, left: t0.saturating_sub(pad) as f64, ..v }, width, span);
    if rows {
        let hi = notes.iter().map(|n| i32::from(n.pitch)).max().unwrap_or(60);
        let lo = notes.iter().map(|n| i32::from(n.pitch)).min().unwrap_or(60);
        let (min_h, max_h) = ROW_H_RANGE;
        let row_h = (height / (hi - lo + 3).max(1) as f32).clamp(min_h, max_h);
        // The highest note's row one row below the top.
        out = MidiView { row_h, scroll: ((127 - hi - 1).max(0) as f32 * row_h).max(0.0), ..out };
    }
    out
}

/// `v` with its left edge inside the span `lo..hi`.
fn clamp_left(v: MidiView, width: f32, (lo, hi): (i64, i64)) -> MidiView {
    let Some(z) = v.zoom else { return MidiView { left: lo as f64, ..v } };
    let shown = f64::from(width) / f64::from(z.max(1e-6));
    let max = (hi as f64 - shown).max(lo as f64);
    MidiView { left: v.left.clamp(lo as f64, max), ..v }
}

/// `v`'s note height times `factor` (within [`ROW_H_RANGE`]), the pitch at `y` (points from the
/// roll's top) kept still.
pub fn row_zoom(v: MidiView, factor: f32, y: f32) -> MidiView {
    let (lo, hi) = ROW_H_RANGE;
    let row_h = (v.row_h * factor).clamp(lo, hi);
    let scroll = (v.scroll + y) * row_h / v.row_h.max(1.0) - y;
    MidiView { row_h, scroll, ..v }
}

/// A toolbar press, applied once the roll is laid out.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ViewOp {
    Rows(f32),
    Fit,
    /// Fit the selected notes (or every note in the clip's window), in time and pitch.
    Notes,
    /// A zoom factor: that many times the Edit window's zoom.
    Factor(f32),
}

/// Whether the MIDI Editor takes `key` before the Edit window's shortcuts. Over the roll: F
/// (Fit), Ctrl+C, X, V and D (its own copy, cut, paste and duplicate of notes), Ctrl+A (Select
/// all), Space (play the selection) and, with notes selected, the arrows (nudge). Esc while a
/// drag is in progress (cancels it), wherever the pointer is. Never while a text field (the note
/// bar's, a dialog's) has the keyboard.
pub fn owns_key(app: &SoundApp, ctx: &egui::Context, key: egui::Key, m: egui::Modifiers) -> bool {
    use egui::Key;
    let plain = !(m.command || m.ctrl || m.alt);
    let fit = key == Key::F && plain && !m.shift;
    let clipboard = matches!(key, Key::C | Key::X | Key::V | Key::D) && m.command && !m.alt && !m.shift;
    let select_all = key == Key::A && m.command && !m.alt && !m.shift;
    let space = key == Key::Space && plain;
    let arrows = matches!(key, Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight)
        && plain
        && (!app.midi.selected.is_empty() || selection_spans_clips(app));
    let escape = key == Key::Escape && plain && tools::dragging(app);
    // In its own window (or docked), keys typed into another window aren't its.
    if !app.ui.show_midi_editor
        || !(fit || clipboard || select_all || space || arrows || escape)
        || ctx.viewport_id() != crate::midi_editor_window::home(ctx)
        || ctx.egui_wants_keyboard_input()
    {
        return false;
    }
    if escape {
        return true;
    }
    let roll: Option<(Rect, u64)> = ctx.data(|d| d.get_temp(egui::Id::new("soundcraft-midi-roll-pass")));
    let roll = roll.filter(|(_, pass)| pass + 1 >= ctx.cumulative_pass_nr()).map(|(r, _)| r);
    roll.zip(ctx.pointer_latest_pos()).is_some_and(|(r, p)| r.contains(p))
}

#[derive(Debug, Clone)]
pub struct MidiEditorState {
    pub selected: Vec<usize>,
    pub drag: Option<NoteDrag>,
    pub scroll: f32,
    pub clip: Option<ClipId>,
    /// Rubber-band selection start (screen position).
    pub band: Option<egui::Pos2>,
    /// The Cleanup panel and the chord strip are shown.
    pub show_cleanup: bool,
    /// The audio track the Cleanup panel's "Detect chords" runs on.
    pub chord_source: Option<soundcraft_model::TrackId>,
    /// Each clip's zoom and scroll, for the session.
    pub views: HashMap<ClipId, MidiView>,
    /// The roll as last drawn.
    pub drawn: Option<RollGeometry>,
    /// A drag in the velocity lane.
    pub vel_drag: Option<VelDrag>,
    /// Counts gestures, so each drag is its own undo step.
    pub gesture: u64,
    /// The velocity last set by hand (or of the note last clicked): new notes get it.
    pub last_velocity: u8,
    /// The length (ticks) of the note last drawn, resized or clicked: the Pencil's next note
    /// (`None`: one grid step).
    pub last_length: Option<i64>,
    /// A clicked, moved or drawn note plays once on the track's instrument.
    pub audition: bool,
    /// While an audition plays: the edit selection and transport position to put back after.
    pub auditioning: Option<(soundcraft_time::Range, soundcraft_time::Samples)>,
    /// Copied notes (Ctrl+C), times as they were in their clip.
    pub clipboard: Vec<soundcraft_midi::Note>,
    /// Where Paste puts notes (a tick from the clip start): set by a click on empty space.
    pub cursor: Option<i64>,
    /// A Pencil drag drawing a note: its start tick and pitch.
    pub draw: Option<(i64, i32)>,
    /// The controller lanes shown under the velocity lane, top to bottom.
    pub lanes: Vec<soundcraft_midi::CtrlKind>,
    /// The selected controller points: their lane and indices in the clip's controller list.
    pub ctrl_sel: Option<(soundcraft_midi::CtrlKind, Vec<usize>)>,
    /// The MIDI channel (0-15) the controller lanes show and write; `None`: every channel, each
    /// its own line, new points on the clip's main channel.
    pub ctrl_channel: Option<u8>,
    /// With a key on the Key ruler: only the scale's rows (and those of pitches with notes).
    pub fold_scale: bool,
    /// The MIDI Event List's filters, Insert row and a position being typed.
    pub event_list: crate::midi_event_list::EventListState,
    /// The drum view (named rows, hits as diamonds): `None` follows the instrument (on for Juke
    /// and the drum synth).
    pub drum_view: Option<bool>,
    /// The drum view shows every named drum, not only the rows with hits.
    pub drum_all_rows: bool,
    /// The rows the roll was last folded to (0: none), to centre the notes when folding turns
    /// on or off.
    pub fold_seen: u128,
    /// Drawn and dragged notes land on the scale.
    pub keep_scale: bool,
    /// A moved note's start lands on a grid line (off: it keeps its offset from the grid).
    pub snap_moves: bool,
    /// The Pencil stamps this chord (`None`: one note).
    pub chord: Option<crate::midi_edit_tools::Chord>,
    /// Of several selected MIDI clips, the one being edited (the others' notes show in their
    /// track's colour; a click on one makes its clip this one).
    pub active: Option<ClipId>,
    /// With several MIDI clips in the editor: the selected notes of the clips other than the
    /// edited one (indices in each clip's notes). Moves, resizes, deletes and quantizes take them
    /// with [`MidiEditorState::selected`], in one undo step.
    pub others: Vec<(ClipId, Vec<usize>)>,
    /// What a drag on empty lane space does: select points, or draw a line or a curve.
    pub ctrl_tool: crate::midi_lanes::LaneTool,
    /// Scale Points' percent (the lanes' right-click menu).
    pub ctrl_scale_pct: f64,
    /// Moving or copying notes takes the controllers under them along.
    pub ctrls_follow: bool,
    /// A drag in a controller lane.
    pub ctrl_drag: Option<crate::midi_lanes::CtrlDrag>,
    /// The controller lanes as last drawn.
    pub drawn_lanes: Vec<crate::midi_lanes::LaneGeometry>,
    /// The controller point a lane's right-click menu is about.
    pub ctrl_menu: Option<usize>,
    /// The editor's own grid (follows the Edit window's until one is picked).
    pub grid: tools::MidiGrid,
    /// Ghost notes of the other MIDI clips are shown.
    pub ghosts: bool,
    /// The tracks whose notes show as ghosts (`None`: every MIDI track).
    pub ghost_tracks: Option<Vec<soundcraft_model::TrackId>>,
    /// The piano key held down: its note sounds until it is let go.
    pub piano_key: Option<u8>,
    /// The pitch a note dragged to another pitch sounds at, while it is dragged.
    pub drag_heard: Option<u8>,
    /// The hover details shown this frame (a note's or a ghost's).
    pub hover: Option<String>,
    /// A zoom factor `ui.midi_editor` asked for (0: Fit), applied when the roll is laid out.
    pub pending_zoom: Option<f32>,
    /// The zoom factor the view is at, if it is at one.
    pub zoom_factor: Option<f32>,
}

impl Default for MidiEditorState {
    fn default() -> Self {
        Self {
            selected: Vec::new(),
            drag: None,
            scroll: 0.0,
            clip: None,
            band: None,
            show_cleanup: true,
            chord_source: None,
            views: HashMap::new(),
            drawn: None,
            vel_drag: None,
            gesture: 0,
            last_velocity: 100,
            last_length: None,
            audition: true,
            auditioning: None,
            clipboard: Vec::new(),
            cursor: None,
            draw: None,
            lanes: Vec::new(),
            ctrl_sel: None,
            ctrl_channel: None,
            fold_scale: false,
            event_list: crate::midi_event_list::EventListState::default(),
            drum_view: None,
            drum_all_rows: false,
            fold_seen: 0,
            keep_scale: false,
            snap_moves: false,
            chord: None,
            active: None,
            others: Vec::new(),
            ctrl_tool: crate::midi_lanes::LaneTool::Select,
            ctrl_scale_pct: 100.0,
            ctrls_follow: false,
            ctrl_drag: None,
            drawn_lanes: Vec::new(),
            ctrl_menu: None,
            grid: tools::MidiGrid::default(),
            ghosts: true,
            ghost_tracks: None,
            piano_key: None,
            drag_heard: None,
            hover: None,
            pending_zoom: None,
            zoom_factor: None,
        }
    }
}

/// The More menu's Playback Quantize: Off, a grid, a groove, and Apply to Notes.
fn playback_quantize_menu(app: &mut SoundApp, ui: &mut egui::Ui, cid: ClipId, cur: Option<&soundcraft_midi::ops::PlayQuantize>) {
    let run = |app: &mut SoundApp, p: serde_json::Value| {
        let mut p = p;
        p["clips"] = json!([cid.0]);
        if let Err(e) = app.run("clip.play_quantize", p) {
            app.ui.status = e;
        }
    };
    if ui.radio(cur.is_none(), "Off").clicked() {
        run(app, json!({"grid": "off"}));
        ui.close();
    }
    for g in ["1/4", "1/8", "1/8t", "1/16", "1/16t", "1/32"] {
        let ticks = soundcraft_engine::cmd::event::note_value_ticks(g).unwrap_or(0);
        let on = cur.is_some_and(|q| q.groove.is_none() && q.grid == ticks);
        if ui.radio(on, g).clicked() {
            run(app, json!({"grid": g}));
            ui.close();
        }
    }
    ui.menu_button("Groove", |ui| {
        for name in crate::ops_windows::groove_names(app) {
            let on = cur.and_then(|q| q.groove.as_ref()).is_some_and(|g| g.name == name);
            if ui.radio(on, name.as_str()).clicked() {
                run(app, json!({"groove": name}));
                ui.close();
            }
        }
    });
    ui.separator();
    if ui
        .add_enabled(cur.is_some(), egui::Button::new("Apply to Notes"))
        .on_hover_text("Move the notes where they play (Restore Performance brings them back)")
        .clicked()
    {
        run(app, json!({"apply": true}));
        ui.close();
    }
}

/// Which part of a note a drag holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The body: moves the notes (Alt: copies them).
    Move,
    /// The left edge: the start moves, the end stays.
    Start,
    /// The right edge: the length changes.
    End,
}

#[derive(Debug, Clone)]
pub struct NoteDrag {
    /// The grabbed note's index in the edited clip (in `from`'s clip when that is set).
    pub index: usize,
    /// The grabbed note belongs to another selected clip (its notes show in the roll).
    pub from: Option<ClipId>,
    /// The grabbed note as it was, in the edited clip's ticks.
    pub grabbed: soundcraft_midi::Note,
    pub edge: Edge,
    pub start: egui::Pos2,
    pub dx_ticks: i64,
    pub dy_semi: i64,
    /// Alt was held when the drag began: a move leaves the notes and drops copies.
    pub copy: bool,
}

/// How near (points) a note's end a press takes the edge, not the body.
pub const EDGE_GRIP: f32 = 5.0;

/// The part of a note `w` points wide a press `dx` points from its left edge takes: an edge
/// within [`EDGE_GRIP`] (the left one only on notes at least 12 points wide), else the body.
pub fn edge_at(dx: f32, w: f32) -> Edge {
    if dx > w - EDGE_GRIP {
        Edge::End
    } else if w >= 12.0 && dx < EDGE_GRIP {
        Edge::Start
    } else {
        Edge::Move
    }
}

/// Where note `n` shows while `d` drags it, as (start, length, pitch), and the copy a copying
/// move drops; `floor` is the earliest start (the start of the note's clip in the roll's ticks).
pub fn drag_shape(n: &soundcraft_midi::Note, d: &NoteDrag, floor: i64) -> ((i64, i64, i32), Option<(i64, i64, i32)>) {
    let (st, ln, p) = (n.start, n.length, i32::from(n.pitch));
    match d.edge {
        Edge::End => ((st, ln.saturating_add(d.dx_ticks).max(1), p), None),
        Edge::Start => {
            let end = n.end();
            let st = st.saturating_add(d.dx_ticks).clamp(floor, end.saturating_sub(1).max(floor));
            ((st, end.saturating_sub(st).max(1), p), None)
        }
        Edge::Move => {
            let moved = (st.saturating_add(d.dx_ticks).max(floor), ln, (p + d.dy_semi as i32).clamp(0, 127));
            if d.copy { ((st, ln, p), Some(moved)) } else { (moved, None) }
        }
    }
}

/// `notes` placed so the earliest starts at `at` (they keep their spacing), as `midi.notes_add`
/// takes them.
pub fn placed(notes: &[soundcraft_midi::Note], at: i64) -> serde_json::Value {
    let first = notes.iter().map(|n| n.start).min().unwrap_or(0);
    let v: Vec<serde_json::Value> = notes
        .iter()
        .map(|n| {
            json!({"pitch": n.pitch, "start": n.start.saturating_sub(first).saturating_add(at).max(0), "length": n.length, "velocity": n.velocity, "channel": n.channel, "probability": n.probability})
        })
        .collect();
    serde_json::Value::Array(v)
}

/// How far Duplicate puts the copies: the notes' span (first start to last end), rounded up to
/// the grid.
pub fn duplicate_offset(notes: &[soundcraft_midi::Note], grid: i64) -> i64 {
    let first = notes.iter().map(|n| n.start).min().unwrap_or(0);
    let last = notes.iter().map(soundcraft_midi::Note::end).max().unwrap_or(first);
    let span = last.saturating_sub(first).max(1);
    let g = grid.max(1);
    span.saturating_add(g - 1) / g * g
}

/// The MIDI clip being edited: the first selected MIDI clip, else a MIDI clip under the insertion
/// point on a selected track.
/// The selected MIDI clips, in selection order: the ones the editor can switch between.
pub fn selected_midi_clips(s: &soundcraft_model::Session) -> Vec<ClipId> {
    s.edit.selected_clips.iter().copied().filter(|id| s.find_clip(*id).is_some_and(|(_, c)| matches!(c.content, ClipContent::Midi { .. }))).collect()
}

/// Makes `to` (one of the selected MIDI clips) the clip the editor edits: its view keeps the
/// same bars in sight, and `select` becomes the note selection.
pub fn activate_clip(app: &mut SoundApp, to: ClipId, select: Vec<usize>) {
    let s = app.engine.session_arc();
    let tick = |id: ClipId| s.find_clip(id).map(|(_, c)| s.tempo.samples_to_ticks(c.start, s.sample_rate));
    if let (Some(from), Some(a), Some(b)) = (app.midi.clip, app.midi.clip.and_then(tick), tick(to))
        && let Some(v) = app.midi.views.get(&from).copied()
    {
        app.midi.views.insert(to, MidiView { left: (v.left + (a - b) as f64).max(0.0), ..v });
    }
    app.midi.active = Some(to);
    app.midi.clip = Some(to);
    app.midi.selected = select;
    app.midi.others.clear();
    app.midi.ctrl_sel = None;
}

/// The notes an edit acts on, clip by clip: the edited clip's selection, then each other
/// selected clip's (those with none left out).
pub fn selection_parts(app: &SoundApp, cid: ClipId) -> Vec<(ClipId, Vec<usize>)> {
    let mut parts = vec![(cid, app.midi.selected.clone())];
    parts.extend(app.midi.others.iter().filter(|(c, i)| *c != cid && !i.is_empty()).cloned());
    parts.retain(|(_, i)| !i.is_empty());
    parts
}

/// Whether notes of more than the edited clip are selected.
pub fn selection_spans_clips(app: &SoundApp) -> bool {
    app.midi.others.iter().any(|(_, i)| !i.is_empty())
}

/// Runs command `id` once for each clip of `parts` (its parameters from `params`) as one undo
/// step: every run after the first that changed the session joins that one's step. Each clip's
/// notes as the command returns them (`indices`, where they sorted to) come back; a refusal
/// stops there and shows in the status line.
pub fn run_parts(
    app: &mut SoundApp,
    id: &str,
    parts: &[(ClipId, Vec<usize>)],
    params: impl Fn(ClipId, &[usize]) -> serde_json::Value,
) -> Vec<(ClipId, Vec<usize>)> {
    let mut out = Vec::new();
    let mut joined = false;
    for (clip, idx) in parts {
        let before = app.engine.session_arc();
        let p = params(*clip, idx);
        let r = if joined { app.engine.execute_joined(id, &p).map_err(|e| e.to_string()) } else { app.run(id, p) };
        match r {
            Ok(v) => {
                joined |= !std::sync::Arc::ptr_eq(&before, &app.engine.session_arc());
                let back = v.get("indices").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_u64()).map(|v| v as usize).collect());
                out.push((*clip, back.unwrap_or_else(|| idx.clone())));
            }
            Err(e) => {
                app.ui.status = e;
                break;
            }
        }
    }
    out
}

/// Every note in the windows of the other selected MIDI clips, clip by clip (Select All).
pub fn all_sibling_notes(s: &soundcraft_model::Session, cid: ClipId) -> Vec<(ClipId, Vec<usize>)> {
    let mut out: Vec<(ClipId, Vec<usize>)> = Vec::new();
    for g in midi_ghosts::siblings(s, cid) {
        match out.iter_mut().find(|(c, _)| *c == g.clip) {
            Some((_, v)) => v.push(g.index),
            None => out.push((g.clip, vec![g.index])),
        }
    }
    out
}

/// Puts the selection back from [`run_parts`]' answer: the edited clip's into `selected`, the
/// others' into `others`.
pub fn set_parts(app: &mut SoundApp, cid: ClipId, parts: Vec<(ClipId, Vec<usize>)>) {
    app.midi.selected.clear();
    app.midi.others.clear();
    for (c, i) in parts {
        if c == cid {
            app.midi.selected = i;
        } else {
            app.midi.others.push((c, i));
        }
    }
}

pub fn target_clip(app: &SoundApp) -> Option<ClipId> {
    let s = app.engine.session();
    // Of several selected MIDI clips, the one made active (a click on one of its notes).
    if let Some(a) = app.midi.active.filter(|a| s.edit.selected_clips.contains(a) && s.find_clip(*a).is_some()) {
        return Some(a);
    }
    for id in &s.edit.selected_clips {
        if let Some((_, c)) = s.find_clip(*id)
            && matches!(c.content, ClipContent::Midi { .. })
        {
            return Some(*id);
        }
    }
    let at = s.edit.selection.start;
    s.tracks
        .iter()
        .filter(|t| s.edit.selected_tracks.contains(&t.id) && t.kind.is_midi())
        .flat_map(|t| t.clips().iter())
        .find(|c| c.range().contains(at) || c.range().overlaps(&s.edit.selection))
        .or_else(|| s.tracks.iter().filter(|t| s.edit.selected_tracks.contains(&t.id) && t.kind.is_midi()).flat_map(|t| t.clips().iter()).next())
        .map(|c| c.id)
}

fn is_black(p: i32) -> bool {
    matches!(p.rem_euclid(12), 1 | 3 | 6 | 8 | 10)
}

pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    // Painter-only contents do not advance the layout. Claim the resized
    // space, including the empty state, so the dock keeps its chosen height.
    ui.take_available_space();
    let t = Tokens::current();
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, t.panel_bg);
    let header = Rect::from_min_size(full.min, vec2(full.width(), 22.0));
    ui.painter().rect_filled(header, 0.0, t.panel_bg2);
    ui.painter().text(pos2(header.min.x + 8.0, header.center().y), Align2::LEFT_CENTER, "MIDI EDITOR", bold(11.5), t.header_text);
    crate::midi_editor_window::drawn_in(ui.ctx());
    // Window / Dock: the editor in its own window, or back in the Edit window.
    let sep = app.ui.midi_editor_separate;
    let wrect = Rect::from_min_size(pos2(header.max.x - 72.0, header.min.y + 2.0), vec2(66.0, header.height() - 4.0));
    let win = ui.put(wrect, egui::Button::new(if sep { "Dock" } else { "Window" }).small());
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("soundcraft-midi-separate"), win.rect));
    if win.on_hover_text(if sep { "Put the MIDI Editor back in the Edit window" } else { "Open the MIDI Editor in its own window" }).clicked() {
        let _ = app.run("window.midi_editor_separate", json!({}));
    }
    let header = Rect::from_min_max(header.min, pos2(header.max.x - 74.0, header.max.y));
    let Some(cid) = target_clip(app) else {
        let track = {
            let s = app.engine.session();
            s.tracks.iter().find(|t| t.kind.is_midi() && s.edit.selected_tracks.contains(&t.id)).map(|t| (t.id, t.name.clone()))
        };
        let msg = match &track {
            Some((_, name)) => format!("Click to add a MIDI clip to {name} at the insertion point."),
            None => "Select a MIDI clip (or a MIDI track) to edit its notes.".into(),
        };
        ui.painter().text(full.center(), Align2::CENTER_CENTER, msg, regular(13.0), t.text_dim);
        let body = Rect::from_min_max(pos2(full.min.x, header.max.y), full.max);
        if let Some((id, _)) = track
            && ui.interact(body, ui.id().with("new_clip"), Sense::click()).clicked()
        {
            let sel = app.engine.session().edit.selection;
            if let Ok(v) = app.run("midi.new_clip", json!({"track": id.0, "start": sel.start, "end": sel.end})) {
                let _ = app.run("edit.select", json!({"clips": [v["clip"]]}));
            }
        }
        return;
    };
    let Some((track, clip)) = app.engine.session().find_clip(cid).map(|(t, c)| (t, c.clone())) else { return };
    let ClipContent::Midi { sequence } = &clip.content else { return };
    // A pattern clip's steps are edited in one place, the Juke window's Steps view.
    if clip.pattern.is_some() {
        ui.painter().text(pos2(header.min.x + 110.0, header.center().y), Align2::LEFT_CENTER, &clip.name, regular(11.0), t.text_dim);
        let body = Rect::from_min_max(pos2(full.min.x, full.min.y + 22.0), full.max);
        pattern_notice(app, ui, body, track, cid, &clip.name);
        return;
    }
    let track_color = app.engine.session().track(track).map_or([120, 120, 200], |t| t.color);
    let editable = selected_midi_clips(app.engine.session());
    let header_text = if editable.len() > 1 {
        format!(
            "{} ({} of {} clips) · {} selected",
            clip.name,
            editable.iter().position(|c| *c == cid).map_or(1, |k| k + 1),
            editable.len(),
            app.midi.selected.len() + app.midi.others.iter().map(|(_, i)| i.len()).sum::<usize>()
        )
    } else {
        format!("{} · {} selected", clip.name, app.midi.selected.len())
    };
    let edit_grid = app.engine.session().edit.grid;
    // Snapping (1 when the editor's grid is off) and the step nudges, Duplicate and Quantize use.
    let snap_ticks = app.midi.grid.ticks(edit_grid);
    let grid_ticks = app.midi.grid.step(edit_grid);
    let alt = ui.input(|i| i.modifiers.alt);
    let text_w = ui.painter().layout_no_wrap(header_text.clone(), regular(11.0), t.text_dim).size().x;
    let text_end = header.min.x + 110.0 + text_w;
    let mut op: Option<(&str, serde_json::Value)> = None;
    // Header, right (laid out first, right to left): the view. Cleanup, Ghosts and its menu, the
    // overflow menu (Audition, Lanes, Legato), Link, Height, the zoom factors, Fit.
    let bar = Rect::from_min_max(pos2(text_end + 8.0, header.min.y + 1.0), pos2(header.max.x - 6.0, header.max.y - 1.0));
    let mut tb = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::right_to_left(egui::Align::Center)));
    tb.spacing_mut().item_spacing.x = 4.0;
    let lit_button = |label: &str, on: bool| {
        egui::Button::new(egui::RichText::new(label).color(if on { t.mode_on_text } else { t.text })).small().fill(if on {
            t.mode_on
        } else {
            t.button
        })
    };
    if tb.add(lit_button("Cleanup", app.midi.show_cleanup)).on_hover_text("Show the MIDI Cleanup panel and the chord strip").clicked() {
        app.midi.show_cleanup = !app.midi.show_cleanup;
    }
    // Ghosts: on or off, and which tracks, in one menu (lit while they show).
    let ghosts_label = egui::RichText::new("Ghosts").color(if app.midi.ghosts { t.accent } else { t.text });
    tb.menu_button(ghosts_label, |ui| midi_ghosts::menu(app, ui, cid))
        .response
        .on_hover_text("Ghost notes: the other MIDI clips' notes faintly behind these, from all tracks or the ones picked");
    // The less-used tools, in one menu.
    tb.menu_button("More", |ui| {
        let mut on = app.midi.audition;
        if ui.checkbox(&mut on, "Audition").on_hover_text("Play a note when it is clicked, moved or drawn").changed() {
            app.midi.audition = on;
        }
        let lanes_text = if app.midi.lanes.is_empty() { "Lanes".to_string() } else { format!("Lanes ({})", app.midi.lanes.len()) };
        ui.menu_button(lanes_text, |ui| crate::midi_lanes::menu(app, ui, sequence))
            .response
            .on_hover_text("Show CC, pitch bend, aftertouch, poly pressure and program lanes under the velocity lane");
        // The drum view.
        let has_drums = app.engine.session().track(track).and_then(tools::drum_names).is_some();
        let mut dv = app.midi.drum_view.unwrap_or(true) && has_drums;
        if ui
            .add_enabled(has_drums, egui::Checkbox::new(&mut dv, "Drum View"))
            .on_hover_text("A named row per drum, hits as diamonds")
            .on_disabled_hover_text("For Juke and the drum synth")
            .changed()
        {
            app.midi.drum_view = Some(dv);
        }
        let mut all_rows = app.midi.drum_all_rows;
        if ui
            .add_enabled(dv, egui::Checkbox::new(&mut all_rows, "Every Drum Row"))
            .on_hover_text("Show every drum, not only the rows with hits")
            .changed()
        {
            app.midi.drum_all_rows = all_rows;
        }
        // Scale and chords.
        let keyed = !app.engine.session().key_signatures.is_empty();
        let mut fold = app.midi.fold_scale;
        if ui
            .add_enabled(keyed, egui::Checkbox::new(&mut fold, "Fold to Scale"))
            .on_hover_text("Only the rows of the scale (and of pitches with notes)")
            .on_disabled_hover_text("Set a key on the Key ruler first")
            .changed()
        {
            app.midi.fold_scale = fold;
        }
        let mut snap_moves = app.midi.snap_moves;
        if ui
            .checkbox(&mut snap_moves, "Snap Moves to the Grid")
            .on_hover_text("A moved note's start lands on a grid line; off, it keeps its distance from the grid")
            .changed()
        {
            app.midi.snap_moves = snap_moves;
        }
        let mut keep = app.midi.keep_scale;
        if ui
            .add_enabled(keyed, egui::Checkbox::new(&mut keep, "Keep to Scale"))
            .on_hover_text("Notes drawn or dragged land on the scale")
            .on_disabled_hover_text("Set a key on the Key ruler first")
            .changed()
        {
            app.midi.keep_scale = keep;
        }
        let chord_text = match app.midi.chord {
            Some(c) => format!("Chord Stamp: {}", c.name()),
            None => "Chord Stamp".to_string(),
        };
        ui.menu_button(chord_text, |ui| {
            if ui.radio(app.midi.chord.is_none(), "Off (one note)").clicked() {
                app.midi.chord = None;
                ui.close();
            }
            for c in tools::Chord::ALL {
                if ui.radio(app.midi.chord == Some(c), c.name()).clicked() {
                    app.midi.chord = Some(c);
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text("The Pencil draws a chord on the note clicked (its root)");
        // The clip's own transpose and velocity, applied as it plays.
        ui.horizontal(|ui| {
            let mut tr = i64::from(clip.midi_transpose);
            let r = ui
                .add(egui::DragValue::new(&mut tr).range(-48..=48).prefix("Clip transpose "))
                .on_hover_text("Semitones, as the clip plays; the notes stay as written");
            if r.changed() {
                let _ = app.engine.execute_merged("clip.midi_offsets", &json!({"clips": [cid.0], "transpose": tr}), &format!("clip_tr.{}", cid.0));
            }
            if r.drag_stopped() || r.lost_focus() {
                app.engine.end_merge();
            }
        });
        ui.horizontal(|ui| {
            let mut ve = i64::from(clip.midi_velocity);
            let r = ui
                .add(egui::DragValue::new(&mut ve).range(-127..=127).prefix("Clip velocity "))
                .on_hover_text("Added to each note's velocity as the clip plays");
            if r.changed() {
                let _ = app.engine.execute_merged("clip.midi_offsets", &json!({"clips": [cid.0], "velocity": ve}), &format!("clip_ve.{}", cid.0));
            }
            if r.drag_stopped() || r.lost_focus() {
                app.engine.end_merge();
            }
        });
        if ui.button("Reverse Clip").on_hover_text("The clip's notes and controllers play backwards").clicked() {
            let _ = app.run("clip.midi_reverse", json!({"clips": [cid.0]}));
            app.midi.selected.clear();
            ui.close();
        }
        let mut chase = app.engine.session().edit.flag("midi.note_chase");
        if ui.checkbox(&mut chase, "Note Chase").on_hover_text("A note already sounding where playback starts is started there").changed() {
            let _ = app.run("midi.note_chase", json!({"on": chase}));
        }
        let mut split = app.engine.session().edit.flag(soundcraft_engine::edit::SPLIT_NOTES);
        if ui
            .checkbox(&mut split, "Split Notes at the Cut")
            .on_hover_text("Splitting a MIDI clip cuts a note across the cut in two, so both clips play their part of it")
            .changed()
        {
            let _ = app.run("midi.split_notes", json!({"on": split}));
        }
        let mut follows = clip.follow_chords.is_some();
        if ui
            .checkbox(&mut follows, "Follow Chord Lane")
            .on_hover_text("When the chord lane changes, this clip's notes move with it (chord tones to the new chord's, others with its root)")
            .changed()
        {
            let _ = app.run("clip.follow_chords", json!({"clips": [cid.0], "on": follows}));
        }
        let mut follow = app.midi.ctrls_follow;
        if ui
            .checkbox(&mut follow, "Controllers Follow Notes")
            .on_hover_text("Moving or copying notes takes the controllers under them along")
            .changed()
        {
            app.midi.ctrls_follow = follow;
        }
        if ui.button("Legato").on_hover_text("Each selected note (every note when none is) lasts until the next one").clicked() {
            op = Some(("event.change_duration", json!({"legato": 0})));
            ui.close();
        }
        if ui.button("Invert Selection").on_hover_text("Select the notes that are not selected, and not the others").clicked() {
            app.midi.selected = tools::inverted_selection(sequence, &app.midi.selected);
            ui.close();
        }
        if ui.button("Select Same Pitch").on_hover_text("Add every note of the selected notes' pitches").clicked() {
            app.midi.selected = tools::same_pitch_selection(sequence, &app.midi.selected, None);
            ui.close();
        }
        if ui.button("Select by Rule…").on_hover_text("Select the notes in pitch, velocity, length and beat ranges").clicked() {
            app.ops.midi_op = "select notes".into();
            app.ops.rule_split = false;
            app.ui.show_midi_ops = true;
            ui.close();
        }
        ui.menu_button(if clip.play_quantize.is_some() { "Playback Quantize •" } else { "Playback Quantize" }, |ui| {
            playback_quantize_menu(app, ui, cid, clip.play_quantize.as_ref())
        })
        .response
        .on_hover_text("The clip plays quantized; its notes stay where they are");
        // Mute the selected notes, or unmute them when every one is muted.
        let picked: Vec<&soundcraft_midi::Note> = app.midi.selected.iter().filter_map(|i| sequence.notes.get(*i)).collect();
        let all_muted = !picked.is_empty() && picked.iter().all(|n| n.muted);
        let mute_text = if all_muted { "Unmute Notes" } else { "Mute Notes" };
        if ui
            .add_enabled(!picked.is_empty(), egui::Button::new(mute_text))
            .on_hover_text("A muted note stays in the clip but does not play")
            .clicked()
        {
            let sel = app.midi.selected.clone();
            let _ = app.run("midi.notes_set", json!({"clip": cid.0, "indices": sel, "muted": !all_muted}));
            ui.close();
        }
        // The selected notes' chance of playing on a pass (one drag is one undo step).
        ui.add_enabled_ui(!picked.is_empty(), |ui| {
            ui.horizontal(|ui| {
                let mut pr = i64::from(picked.first().map_or(100, |n| n.probability));
                let r = ui
                    .add(egui::DragValue::new(&mut pr).range(0..=100).prefix("Probability ").suffix(" %"))
                    .on_hover_text("The chance each selected note plays on a pass (rolled again each pass; a bounce repeats)");
                if r.changed() {
                    let sel = app.midi.selected.clone();
                    let p = json!({"clip": cid.0, "indices": sel, "probability": pr});
                    let _ = app.engine.execute_merged("midi.notes_set", &p, &format!("note_prob.{}", cid.0));
                }
                if r.drag_stopped() || r.lost_focus() {
                    app.engine.end_merge();
                }
            });
        });
    })
    .response
    .on_hover_text("More: Audition, controller lanes, Legato");
    tb.separator();
    crate::midi_editor_link::link_button(app, &mut tb);
    let mut view_op = None;
    if tb.small_button("+").on_hover_text("Taller notes (Ctrl+Shift+wheel)").clicked() {
        view_op = Some(ViewOp::Rows(STEP));
    }
    if tb.small_button("−").on_hover_text("Shorter notes (Ctrl+Shift+wheel)").clicked() {
        view_op = Some(ViewOp::Rows(1.0 / STEP));
    }
    tb.label(egui::RichText::new("Height").small().color(t.text_dim));
    tb.separator();
    // The zoom factors (right to left: 4× first), lit when the view is at one.
    tb.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for f in tools::ZOOM_FACTORS.iter().rev() {
            let lit = app.midi.zoom_factor.is_some_and(|z| (z - f).abs() < 1e-6);
            let label = tools::zoom_label(*f);
            if ui.add(lit_button(&label, lit)).on_hover_text(format!("Roll zoom: {label} the Edit window's (Ctrl+wheel zooms by hand)")).clicked() {
                view_op = Some(ViewOp::Factor(*f));
            }
        }
    });
    if tb.small_button("Fit").on_hover_text("Fit the whole clip (F over the notes)").clicked() {
        view_op = Some(ViewOp::Fit);
    }
    if tb.small_button("Notes").on_hover_text("Zoom to the selected notes, or to every note").clicked() {
        view_op = Some(ViewOp::Notes);
    }
    match app.midi.pending_zoom.take() {
        Some(f) if f > 0.0 => view_op = Some(ViewOp::Factor(f)),
        Some(f) if f < 0.0 => view_op = Some(ViewOp::Notes),
        Some(_) => view_op = Some(ViewOp::Fit),
        None => {}
    }
    // Header, left: after the clip's line, the editing tools, in the room the view leaves.
    let left_bar = Rect::from_min_max(pos2(text_end + 10.0, header.min.y + 1.0), pos2(tb.min_rect().min.x - 8.0, header.max.y - 1.0));
    let mut lt = ui.new_child(egui::UiBuilder::new().max_rect(left_bar).layout(egui::Layout::left_to_right(egui::Align::Center)));
    lt.set_clip_rect(left_bar.intersect(lt.clip_rect()));
    lt.spacing_mut().item_spacing.x = 4.0;
    if lt.small_button("Select all").on_hover_text("Select every note of the clip, or of every clip shown (Ctrl+A)").clicked() {
        tools::select_all(app, cid);
        app.midi.others = all_sibling_notes(app.engine.session(), cid);
    }
    if lt.small_button("Split").on_hover_text("Split the selected notes at the playhead (Alt-click a note: split it there)").clicked() {
        let _ = tools::split(app, cid, None);
    }
    if lt.small_button("Join").on_hover_text("Join the selected notes of each pitch into one").clicked() {
        let _ = tools::join(app, cid);
    }
    if lt.small_button("Quantize").on_hover_text("Quantize the selected notes (every note when none is) to the grid").clicked() {
        op = Some(("event.quantize", json!({"grid": grid_ticks, "strength": 100})));
    }
    if lt
        .add(egui::Button::selectable(app.step.open, "Step"))
        .on_hover_text("Step Input: enter notes from a MIDI keyboard into this clip with the transport stopped")
        .clicked()
    {
        crate::step_input::toggle(app);
    }
    // Several selected clips: which one is edited.
    if editable.len() > 1 {
        let names: Vec<(ClipId, String)> = {
            let s = app.engine.session();
            editable.iter().filter_map(|id| s.find_clip(*id).map(|(_, c)| (*id, c.name.clone()))).collect()
        };
        lt.menu_button("Clip", |ui| {
            for (id, name) in &names {
                if ui.radio(*id == cid, name.as_str()).clicked() {
                    activate_clip(app, *id, Vec::new());
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text("The selected clips: the one edited (a click on another's note switches to it)");
    }
    let grid_text = format!("Grid: {}", app.midi.grid.label(edit_grid));
    lt.menu_button(grid_text, |ui| tools::grid_menu(app, ui)).response.on_hover_text("The MIDI Editor's own grid (hold Alt while dragging: no snap)");
    if let Some((id, p)) = op {
        run_on_selection(app, cid, id, p);
    }
    // The clip's line, clipped short of the tools as they are laid out.
    let text_end = left_bar.min.x - 4.0;
    ui.painter().with_clip_rect(Rect::from_min_max(header.min, pos2(text_end, header.max.y))).text(
        pos2(header.min.x + 110.0, header.center().y),
        Align2::LEFT_CENTER,
        header_text,
        regular(11.0),
        t.text_dim,
    );
    let s = app.engine.session_arc();
    let sr = s.sample_rate;
    let base_tick = s.tempo.samples_to_ticks(clip.start, sr);
    let len_ticks = (s.tempo.samples_to_ticks(clip.end(), sr) - base_tick).max(1);
    // Playback quantize on this clip (or its track): the drawing grid takes its swing or groove
    // (its lines and the snap), and the notes it moves are outlined where they play.
    let play_q = s.track(track).and_then(|tr| soundcraft_mix::play_quantize(&s, tr, &clip)).map(std::borrow::Cow::into_owned);
    let swung_grid = drawing_grid(play_q.as_ref(), snap_ticks);
    let swing_tick = |tk: i64| swung_grid.as_ref().map_or(tk, |g| soundcraft_mix::quantized_start(&s, base_tick, g, tk));
    // Several selected MIDI clips: the roll spans them all (ticks from this clip's start), each
    // other clip at its offset; their notes are selected and edited with this one's.
    let multi = editable.len() > 1 && editable.contains(&cid);
    let clip_spans: Vec<(ClipId, i64, i64)> = if multi {
        editable
            .iter()
            .filter_map(|id| s.find_clip(*id))
            .map(|(_, c)| {
                let at = s.tempo.samples_to_ticks(c.start, sr);
                (c.id, at.saturating_sub(base_tick), s.tempo.samples_to_ticks(c.end(), sr).saturating_sub(at).max(1))
            })
            .collect()
    } else {
        vec![(cid, 0, len_ticks)]
    };
    let span = clip_spans.iter().fold((0, len_ticks), |(lo, hi), (_, off, len)| (lo.min(*off), hi.max(off.saturating_add(*len))));
    let span_range = soundcraft_time::Range::new(
        s.tempo.tick_to_samples(base_tick.saturating_add(span.0), sr),
        s.tempo.tick_to_samples(base_tick.saturating_add(span.1), sr),
    );
    if multi {
        app.midi.others.retain(|(c, _)| *c != cid && editable.contains(c));
    } else {
        app.midi.others.clear();
    }
    // Where a note drawn at `tick` goes: this clip where it covers the tick, else the other
    // selected clip that does (in its own ticks).
    let draw_target = |tick: i64| -> (ClipId, i64) {
        if (0..len_ticks).contains(&tick) {
            return (cid, tick);
        }
        clip_spans
            .iter()
            .find(|(id, off, len)| *id != cid && tick >= *off && tick < off.saturating_add(*len))
            .map_or((cid, tick.max(0)), |(id, off, _)| (*id, tick.saturating_sub(*off)))
    };
    let cleanup = app.midi.show_cleanup;
    let right = if cleanup { full.max.x - midi_cleanup_panel::PANEL_W - 4.0 } else { full.max.x - 4.0 };
    // The chord strip, then the ruler right above the roll; the note bar along the bottom.
    let top = (if cleanup { header.max.y + midi_cleanup_panel::CHORD_H } else { header.max.y }) + crate::midi_editor_link::RULER_H;
    let bottom = full.max.y - tools::NOTEBAR_H;
    // A clip seen for the first time shows the controller lanes it has data on.
    if !app.midi.views.contains_key(&cid) {
        for k in crate::midi_lanes::lanes_with_data(sequence) {
            crate::midi_lanes::toggle(app, k, true);
        }
    }
    // Controller lanes under the velocity lane, shrunk (not below 18 points) to leave the roll 80.
    let n_lanes = app.midi.lanes.len();
    let room = bottom - top - VEL_H - 4.0 - BAR_H - 80.0;
    let lane_h = crate::midi_lanes::lane_height(n_lanes, room);
    let lanes_h = n_lanes as f32 * (lane_h + crate::midi_lanes::LANE_GAP);
    let roll = Rect::from_min_max(pos2(full.min.x + KEY_W, top), pos2(right, bottom - VEL_H - 4.0 - BAR_H - lanes_h));
    let keys = Rect::from_min_max(pos2(full.min.x, roll.min.y), pos2(roll.min.x, roll.max.y));
    let hbar = Rect::from_min_max(pos2(roll.min.x, roll.max.y + 1.0), pos2(roll.max.x, roll.max.y + BAR_H));
    let vel = Rect::from_min_max(pos2(roll.min.x, hbar.max.y + 3.0), pos2(roll.max.x, bottom - 2.0 - lanes_h));
    let lane_rects: Vec<crate::midi_lanes::LaneGeometry> = app
        .midi
        .lanes
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let y = vel.max.y + crate::midi_lanes::LANE_GAP + i as f32 * (lane_h + crate::midi_lanes::LANE_GAP);
            crate::midi_lanes::LaneGeometry { kind: *k, rect: Rect::from_min_max(pos2(roll.min.x, y), pos2(roll.max.x, y + lane_h)) }
        })
        .collect();
    app.midi.drawn_lanes = lane_rects.clone();
    let lanes_area = lane_rects.iter().fold(Rect::NOTHING, |a, l| a.union(l.rect));
    // The clip's view: fitted, and centred on its notes, the first time it opens.
    // Views of clips gone from the session go too.
    app.midi.views.retain(|id, _| s.find_clip(*id).is_some());
    let mut view = match app.midi.views.get(&cid) {
        Some(v) => *v,
        None => {
            let hi = sequence.notes.iter().map(|n| i32::from(n.pitch)).max().unwrap_or(72);
            let lo = sequence.notes.iter().map(|n| i32::from(n.pitch)).min().unwrap_or(48);
            let rows = (roll.height() / ROW_H).floor() as i32;
            MidiView { scroll: (127 - (hi + lo) / 2 - rows / 2) as f32 * ROW_H, ..MidiView::default() }
        }
    };
    app.midi.clip = Some(cid);
    let view_before = view;
    let width = roll.width().max(1.0);
    let edit_ppt = tools::edit_px_per_tick(&s, base_tick);
    match view_op {
        Some(ViewOp::Rows(f)) => view = row_zoom(view, f, roll.height() / 2.0),
        Some(ViewOp::Fit) => view = MidiView { zoom: None, left: span.0 as f64, ..view },
        Some(ViewOp::Notes) => {
            let picked: Vec<&soundcraft_midi::Note> = if app.midi.clip == Some(cid) && !app.midi.selected.is_empty() {
                app.midi.selected.iter().filter_map(|i| sequence.notes.get(*i)).collect()
            } else {
                sequence.notes.iter().filter(|n| n.start >= 0 && n.start < len_ticks).collect()
            };
            // Folded rows are not semitones: only the time is fitted then.
            let folded = app.midi.drawn.is_some_and(|g| g.fold != 0);
            view = fit_notes(view, &picked, width, roll.height(), span, !folded);
        }
        Some(ViewOp::Factor(f)) => {
            // About the middle of the roll.
            let fit = width / span.1.saturating_sub(span.0).max(1) as f32;
            let now = view.zoom.unwrap_or(fit).max(fit);
            view = zoom_about_span(view, (edit_ppt * f) / now, width / 2.0, width, span);
        }
        None => {}
    }
    wheel(ui, roll.union(vel).union(hbar).union(lanes_area), roll, &mut view, span);
    if fit_key(app, ui, roll) {
        view = MidiView { zoom: None, left: span.0 as f64, ..view };
    }
    view = clamp_left(view, width, span);
    // The key at the clip's start (with one on the Key ruler) and, folded, the rows kept.
    let clip_key = (!s.key_signatures.is_empty()).then(|| soundcraft_engine::vocal::key_pc_at(&s, clip.start));
    // The drum view: the instrument's drum names, one row each (only rows with hits unless
    // every drum is asked for); it folds the roll in place of the scale.
    let drum_names = s.track(track).and_then(tools::drum_names).filter(|_| app.midi.drum_view != Some(false));
    let fold = match (&drum_names, clip_key.filter(|_| app.midi.fold_scale)) {
        (Some(names), _) => tools::drum_mask(names, sequence, app.midi.drum_all_rows),
        (None, Some((k, sc))) => tools::fold_mask(soundcraft_engine::vocal::key_set(k, sc), sequence),
        (None, None) => 0,
    };
    let drums = drum_names.is_some();
    let n_rows = if fold == 0 { 128.0 } else { fold.count_ones() as f32 };
    // Folding or unfolding changes what a scroll means: the notes are centred again.
    if (fold != 0) != (app.midi.fold_seen != 0) {
        let probe = RollGeometry { clip: cid, roll, px_per_tick: 1.0, fit: 1.0, left: 0.0, row_h: view.row_h, scroll: 0.0, fold };
        let (lo, hi) = sequence.notes.iter().fold((127, 0), |(lo, hi), n| (lo.min(i32::from(n.pitch)), hi.max(i32::from(n.pitch))));
        let mid = if lo <= hi { (lo + hi) / 2 } else { 60 };
        view.scroll = probe.y_of(mid) - roll.min.y - roll.height() / 2.0;
    }
    app.midi.fold_seen = fold;
    view.scroll = view.scroll.clamp(0.0, (n_rows * view.row_h - roll.height()).max(0.0));
    scrollbar(ui, hbar, &mut view, width, span, cid);
    crate::midi_editor_link::follow(app, ui.ctx(), &mut view, view_before, width, base_tick);
    view = clamp_left(view, width, span);
    let fit = width / span.1.saturating_sub(span.0).max(1) as f32;
    let px_per_tick = view.zoom.unwrap_or(fit).max(fit);
    let geo = RollGeometry { clip: cid, roll, px_per_tick, fit, left: view.left, row_h: view.row_h, scroll: view.scroll, fold };
    app.midi.zoom_factor = tools::factor_of(view.zoom, fit, edit_ppt);
    app.midi.views.insert(cid, view);
    app.midi.drawn = Some(geo);
    app.midi.scroll = geo.scroll;
    // An adaptive grid follows the zoom (from the next frame's snapping and lines).
    if let tools::MidiGrid::Adaptive(t) = app.midi.grid {
        let a = tools::adaptive_ticks(px_per_tick);
        if a != t {
            app.midi.grid = tools::MidiGrid::Adaptive(a);
        }
    }
    let row_h = geo.row_h;
    let rows = (roll.height() / row_h).floor() as i32;
    let x_of = |tick: i64| geo.x_of(tick);
    let y_of = |pitch: i32| geo.y_of(pitch);
    let pitch_at = |y: f32| geo.pitch_at(y);
    let painter = ui.painter().with_clip_rect(roll.union(keys));
    // Background rows and keys. With a key on the Key ruler, the rows out of the key at the clip's
    // start are shaded darker, so the scale shows.
    let scale = clip_key.map(|(k, sc)| soundcraft_engine::vocal::key_set(k, sc));
    // Keep to Scale: a pitch drawn or dragged to goes to the nearest one in the scale.
    let keep_scale = app.midi.keep_scale;
    let snap_moves = app.midi.snap_moves;
    let to_scale = |p: i32| match scale.filter(|_| keep_scale) {
        Some(set) => tools::nearest_in(set, p),
        None => p,
    };
    for r in 0..=rows + 1 {
        let pitch = pitch_at(roll.min.y + r as f32 * row_h);
        let y = y_of(pitch);
        let row = Rect::from_min_size(pos2(roll.min.x, y), vec2(roll.width(), row_h));
        let out_of_key = scale.is_some_and(|set| !set.contains(pitch.rem_euclid(12) as u8));
        let fill = if out_of_key {
            Color32::from_rgb(24, 24, 26)
        } else if is_black(pitch) {
            Color32::from_rgb(30, 30, 32)
        } else {
            Color32::from_rgb(40, 40, 42)
        };
        painter.rect_filled(row, 0.0, fill);
        let key = Rect::from_min_size(pos2(keys.min.x, y), vec2(KEY_W - 2.0, row_h - 1.0));
        painter.rect_filled(key, 1.0, if is_black(pitch) { Color32::from_rgb(20, 20, 20) } else { Color32::from_rgb(225, 225, 225) });
        if let Some(name) = drum_names.as_ref().and_then(|n| n.get(usize::try_from(pitch).ok()?)).and_then(|n| n.as_deref()) {
            // The drum's name across the key, and a line under its row.
            painter.with_clip_rect(key.intersect(painter.clip_rect())).text(
                pos2(key.min.x + 3.0, key.center().y),
                Align2::LEFT_CENTER,
                name,
                regular(8.5),
                Color32::from_rgb(30, 30, 30),
            );
            painter.line_segment([pos2(roll.min.x, y + row_h), pos2(roll.max.x, y + row_h)], Stroke::new(1.0, Color32::from_rgb(54, 54, 58)));
        } else if pitch.rem_euclid(12) == 0 && (0..=127).contains(&pitch) {
            painter.text(
                pos2(key.max.x - 3.0, key.center().y),
                Align2::RIGHT_CENTER,
                soundcraft_midi::note_name(pitch as u8),
                regular(8.0),
                Color32::from_rgb(40, 40, 40),
            );
            painter.line_segment([pos2(roll.min.x, y + row_h), pos2(roll.max.x, y + row_h)], Stroke::new(1.0, Color32::from_rgb(64, 64, 68)));
        }
        if app.midi.piano_key.is_some_and(|k| i32::from(k) == pitch) {
            tools::lit_key(&painter, key, pitch);
        }
    }
    // Bars and beats as the meter in force sets them, and the grid's lines, those in view (beats
    // and grid lines only where they stand 4 points apart).
    let first = (geo.tick_at(roll.min.x).floor() as i64).max(span.0);
    let last_tick = (geo.tick_at(roll.max.x).ceil() as i64).min(span.1);
    let beats = s.tempo.meter_at_tick(base_tick.saturating_add(first)).ticks_per_beat() as f32 * px_per_tick > 4.0;
    let grid_lines = if snap_ticks as f32 * px_per_tick > 4.0 && snap_ticks > 1 { snap_ticks } else { 0 };
    for (tick, kind) in tools::grid_lines(&s.tempo, base_tick, first, last_tick, beats, grid_lines) {
        let color = match kind {
            tools::LineKind::Bar => Color32::from_rgb(88, 88, 96),
            tools::LineKind::Beat => Color32::from_rgb(60, 60, 66),
            tools::LineKind::Grid => Color32::from_rgb(48, 48, 52),
        };
        let x = x_of(if kind == tools::LineKind::Grid { swing_tick(tick) } else { tick });
        painter.line_segment([pos2(x, roll.min.y), pos2(x, roll.max.y)], Stroke::new(1.0, color));
    }
    if cleanup {
        let strip = Rect::from_min_max(pos2(roll.min.x, header.max.y), pos2(roll.max.x, roll.min.y - crate::midi_editor_link::RULER_H));
        midi_cleanup_panel::chord_strip(&ui.painter().with_clip_rect(strip), &s, &clip, strip, x_of);
    }
    // Notes: the take; on Cleaned, a shortened note shows its kept part (the overlays draw the rest).
    let base = egui::Color32::from_rgb(track_color[0], track_color[1], track_color[2]);
    // Notes stay in the roll (not over the keys) when the view starts inside one.
    let notes_p = painter.with_clip_rect(roll);
    let drag = app.midi.drag.clone();
    let edits = midi_cleanup_panel::shown_edits(&clip).filter(|_| drag.is_none());
    // The grabbed note is always among the selected ones (a press selects it).
    let in_drag = |i: usize, d: &NoteDrag| (d.from.is_none() && d.index == i) || app.midi.selected.contains(&i);
    let shade = |v: u8| {
        let k = 0.45 + 0.55 * f32::from(v) / 127.0;
        Color32::from_rgb((f32::from(base.r()) * k) as u8, (f32::from(base.g()) * k) as u8, (f32::from(base.b()) * k) as u8)
    };
    // In the drum view a hit is a diamond on its start, its length not shown.
    let note_rect = |st: i64, ln: i64, p: i32| {
        if drums {
            let h = (row_h - 2.0).max(4.0);
            return Rect::from_center_size(pos2(x_of(st), y_of(p) + row_h / 2.0), vec2(h, h));
        }
        Rect::from_min_max(pos2(x_of(st), y_of(p) + 1.0), pos2(x_of(st.saturating_add(ln)).max(x_of(st) + 3.0), y_of(p) + row_h - 1.0))
    };
    // Ghost notes of the other clips, behind this clip's.
    let ghost_notes = midi_ghosts::shown(app, &s, cid);
    let ghost_rects = midi_ghosts::rects(&ghost_notes, &x_of, &y_of, row_h);
    midi_ghosts::draw(&notes_p, &ghost_notes, &ghost_rects);
    let sibling_notes = midi_ghosts::siblings(&s, cid);
    let sibling_rects = midi_ghosts::rects(&sibling_notes, &x_of, &y_of, row_h);
    let others = app.midi.others.clone();
    let picked = |g: &midi_ghosts::Ghost| others.iter().any(|(c, i)| *c == g.clip && i.contains(&g.index));
    // The other clips' selected notes follow a drag, as this clip's do.
    let mut shown_siblings = sibling_notes.clone();
    let mut sibling_copies: Vec<midi_ghosts::Ghost> = Vec::new();
    if let Some(d) = drag.as_ref() {
        for g in shown_siblings.iter_mut().filter(|g| picked(g)) {
            let floor = clip_spans.iter().find(|(c, _, _)| *c == g.clip).map_or(0, |(_, off, _)| *off);
            let ((st, ln, p), copy) = drag_shape(&g.note, d, floor);
            if let Some((cs, cl, cp)) = copy {
                sibling_copies
                    .push(midi_ghosts::Ghost { note: soundcraft_midi::Note { start: cs, length: cl, pitch: cp as u8, ..g.note }, ..g.clone() });
            }
            g.note = soundcraft_midi::Note { start: st, length: ln, pitch: p as u8, ..g.note };
        }
    }
    let shown_rects = midi_ghosts::rects(&shown_siblings, &x_of, &y_of, row_h);
    midi_ghosts::draw_siblings(&notes_p, &shown_siblings, &shown_rects, &picked);
    let copy_rects = midi_ghosts::rects(&sibling_copies, &x_of, &y_of, row_h);
    let mut ghosts: Vec<Rect> = Vec::new();
    for (i, n) in sequence.notes.iter().enumerate() {
        let (mut st, mut ln, mut p) = (n.start, n.length, i32::from(n.pitch));
        if let Some(d) = drag.as_ref().filter(|d| in_drag(i, d)) {
            let (shape, copy) = drag_shape(n, d, 0);
            (st, ln, p) = shape;
            if let Some((cs, cl, cp)) = copy {
                ghosts.push(note_rect(cs, cl, cp));
            }
        }
        if let Some(end) = edits.and_then(|e| midi_cleanup_panel::edit_of(e, i)).and_then(|e| e.new_end).filter(|end| *end < st.saturating_add(ln)) {
            ln = (end - st).max(0);
        }
        let selected = app.midi.selected.contains(&i);
        let fill = if selected { Color32::WHITE } else { shade(n.velocity) };
        // A muted note is drawn faint, with its outline.
        let (fill, edge) = if n.muted { (fill.gamma_multiply(0.25), Color32::from_gray(150)) } else { (fill, Color32::BLACK) };
        let r = note_rect(st, ln, p);
        // Where playback quantize plays it, when that is elsewhere: an outline.
        if let Some(q) = play_q.as_ref().filter(|_| drag.is_none() && !n.muted) {
            let played = soundcraft_mix::quantized_start(&s, base_tick, q, n.start);
            if played != n.start {
                notes_p.rect(note_rect(played, ln, p), 2.0, Color32::TRANSPARENT, Stroke::new(1.0, fill.gamma_multiply(0.8)), StrokeKind::Inside);
            }
        }
        if drums {
            let c = r.center();
            let h = r.width() / 2.0;
            let pts = vec![pos2(c.x, c.y - h), pos2(c.x + h, c.y), pos2(c.x, c.y + h), pos2(c.x - h, c.y)];
            notes_p.add(egui::Shape::convex_polygon(pts, fill, Stroke::new(1.0, edge)));
            continue;
        }
        notes_p.rect(r, 2.0, fill, Stroke::new(1.0, edge), StrokeKind::Inside);
        // The note's name, when it fits.
        if r.width() >= 24.0 && r.height() >= 8.0 && r.intersects(roll) {
            let name = soundcraft_midi::note_name(p.clamp(0, 127) as u8);
            let size = (r.height() - 1.0).clamp(7.0, 10.0);
            let color = if selected { Color32::from_rgb(20, 20, 24) } else { Color32::from_white_alpha(225) };
            notes_p.with_clip_rect(r.intersect(roll)).text(pos2(r.min.x + 3.0, r.center().y), Align2::LEFT_CENTER, name, regular(size), color);
        }
    }
    for g in ghosts.into_iter().chain(copy_rects) {
        notes_p.rect(g, 2.0, Color32::from_white_alpha(90), Stroke::new(1.0, Color32::WHITE), StrokeKind::Inside);
    }
    if edits.is_some() {
        midi_cleanup_panel::overlays(&painter.with_clip_rect(roll), &clip, x_of, y_of);
    }
    // The paste cursor.
    if let Some(c) = app.midi.cursor.filter(|c| (0..=len_ticks).contains(c)) {
        let x = x_of(c);
        notes_p.line_segment([pos2(x, roll.min.y), pos2(x, roll.max.y)], Stroke::new(1.0, t.accent.gamma_multiply(0.7)));
    }
    let ruler = Rect::from_min_max(pos2(roll.min.x, roll.min.y - crate::midi_editor_link::RULER_H), pos2(roll.max.x, roll.min.y));
    crate::midi_editor_link::ruler(app, ui, ruler, &geo, base_tick, if multi { span_range } else { clip.range() });
    // Interaction.
    let resp = ui.interact(roll, ui.id().with(("roll", cid.0)), Sense::click_and_drag());
    let pass = ui.ctx().cumulative_pass_nr();
    ui.ctx().data_mut(|d| {
        d.insert_temp(egui::Id::new("soundcraft-midi-roll"), roll);
        d.insert_temp(egui::Id::new("soundcraft-midi-roll-pass"), (roll, pass));
        d.insert_temp(egui::Id::new("soundcraft-midi-vel"), vel);
    });
    let hit = |pos: egui::Pos2| -> Option<(usize, Edge)> {
        sequence.notes.iter().enumerate().rev().find_map(|(i, n)| {
            // A drum hit is only moved: its diamond has no edges to resize.
            if drums {
                return note_rect(n.start, n.length, i32::from(n.pitch)).contains(pos).then_some((i, Edge::Move));
            }
            let r = Rect::from_min_max(
                pos2(x_of(n.start), y_of(i32::from(n.pitch))),
                pos2(x_of(n.end()).max(x_of(n.start) + 3.0), y_of(i32::from(n.pitch)) + row_h),
            );
            r.contains(pos).then(|| (i, edge_at(pos.x - r.min.x, r.width())))
        })
    };
    // A tick from the clip start on the grid (lines counted from each bar's start); a span of
    // ticks to a whole number of grid steps. Alt held: no snap.
    let free = |tk: f64| tk.round() as i64;
    let snap = |tk: f64| -> i64 {
        if alt { free(tk) } else { swing_tick(tools::snap_abs(&s.tempo, base_tick as f64 + tk, snap_ticks).saturating_sub(base_tick)) }
    };
    let snap_rel = |tk: f64| -> i64 { if alt { free(tk) } else { ((tk / snap_ticks as f64).round() as i64).saturating_mul(snap_ticks) } };
    let floor_grid = |tk: f64| -> i64 {
        if alt {
            return (tk.floor() as i64).max(span.0);
        }
        let at = tools::snap_abs(&s.tempo, base_tick as f64 + tk, snap_ticks).saturating_sub(base_tick);
        (if at as f64 > tk { at.saturating_sub(snap_ticks) } else { at }).max(span.0)
    };
    let pencil = s.edit.tool == Tool::Pencil;
    // Esc cancels a drag in progress: the notes stay as they were.
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) && !ui.ctx().egui_wants_keyboard_input() && tools::cancel_drag(app) {
        app.ui.status = "Drag cancelled".into();
    }
    // The hover details: a note's, else a ghost's.
    app.midi.hover = None;
    if let Some(hp) = resp.hover_pos().filter(|_| !tools::dragging(app)) {
        let text = match hit(hp) {
            Some((i, _)) => sequence.notes.get(i).map(|n| tools::note_details(&s, base_tick, n)),
            None => midi_ghosts::hit(&ghost_rects, hp).and_then(|g| ghost_notes.get(g)).map(|g| midi_ghosts::details(&s, base_tick, g)),
        };
        if let Some(text) = text {
            tools::hover_box(ui, hp, &text, roll);
            app.midi.hover = Some(text);
        }
    }
    // Another selected clip's note under `p`, and the part of it a press there takes.
    let sibling_at = |p: egui::Pos2| -> Option<(&midi_ghosts::Ghost, Edge)> {
        let k = midi_ghosts::hit(&sibling_rects, p)?;
        let (g, r) = (sibling_notes.get(k)?, sibling_rects.get(k)?);
        Some((g, if drums { Edge::Move } else { edge_at(p.x - r.min.x, r.width()) }))
    };
    // A click on another selected clip's note makes that clip the edited one, the note selected;
    // Shift-click adds it to the selection, which can span the clips.
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
        && hit(p).is_none()
        && let Some((g, _)) = sibling_at(p)
    {
        if ui.input(|i| i.modifiers.shift) {
            match app.midi.others.iter_mut().find(|(c, _)| *c == g.clip) {
                Some((_, v)) if !v.contains(&g.index) => v.push(g.index),
                Some(_) => {}
                None => app.midi.others.push((g.clip, vec![g.index])),
            }
        } else {
            activate_clip(app, g.clip, vec![g.index]);
        }
        return;
    }
    // A double-click on a ghost opens its clip.
    if resp.double_clicked()
        && let Some(p) = resp.interact_pointer_pos()
        && hit(p).is_none()
        && let Some(g) = midi_ghosts::hit(&ghost_rects, p).and_then(|g| ghost_notes.get(g))
    {
        let _ = app.run("edit.select", json!({"clips": [g.clip.0]}));
        app.midi.selected.clear();
        return;
    }
    if let Some(hp) = resp.hover_pos()
        && app.midi.drag.is_none()
        && matches!(hit(hp), Some((_, Edge::Start | Edge::End)))
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    // Where the button went down, not where the drag was first noticed (a few points on).
    if resp.drag_started()
        && let Some(p) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
    {
        let copy = |edge: Edge| alt && edge == Edge::Move;
        match (hit(p), sibling_at(p)) {
            (Some((i, edge)), _) => {
                if !app.midi.selected.contains(&i) {
                    app.midi.selected = vec![i];
                    app.midi.others.clear();
                }
                if let Some(grabbed) = sequence.notes.get(i).copied() {
                    app.midi.drag = Some(NoteDrag { index: i, from: None, grabbed, edge, start: p, dx_ticks: 0, dy_semi: 0, copy: copy(edge) });
                }
            }
            // Another selected clip's note: the selection (it, unless it was already among the
            // selected notes) moves or resizes across the clips.
            (None, Some((g, edge))) => {
                if !picked(g) {
                    app.midi.selected.clear();
                    app.midi.others = vec![(g.clip, vec![g.index])];
                }
                let d = NoteDrag { index: g.index, from: Some(g.clip), grabbed: g.note, edge, start: p, dx_ticks: 0, dy_semi: 0, copy: copy(edge) };
                app.midi.drag = Some(d);
            }
            (None, None) if pencil => app.midi.draw = Some((floor_grid(geo.tick_at(p.x)), to_scale(pitch_at(p.y)).clamp(0, 127))),
            (None, None) => app.midi.band = Some(p),
        }
    }
    // Rubber-band selection on empty space.
    if let (Some(a), Some(b)) = (app.midi.band, ui.ctx().pointer_latest_pos()) {
        let band = Rect::from_two_pos(a, b);
        ui.painter().rect(
            band,
            0.0,
            Color32::from_rgba_unmultiplied(120, 170, 255, 40),
            Stroke::new(1.0, Color32::from_rgb(120, 170, 255)),
            StrokeKind::Inside,
        );
        if resp.drag_stopped() {
            app.midi.selected = sequence
                .notes
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    let r = Rect::from_min_max(pos2(x_of(n.start), y_of(i32::from(n.pitch))), pos2(x_of(n.end()), y_of(i32::from(n.pitch)) + row_h));
                    r.intersects(band)
                })
                .map(|(i, _)| i)
                .collect();
            // The other clips' notes under the band too.
            app.midi.others.clear();
            for (g, r) in sibling_notes.iter().zip(&sibling_rects) {
                if r.intersects(band) {
                    match app.midi.others.iter_mut().find(|(c, _)| *c == g.clip) {
                        Some((_, v)) => v.push(g.index),
                        None => app.midi.others.push((g.clip, vec![g.index])),
                    }
                }
            }
            app.midi.band = None;
        }
    }
    // The Pencil drawing a note: from its start to the pointer, at least one grid step.
    let drawn_len = |start: i64, x: f32| (snap(geo.tick_at(x)).saturating_sub(start)).max(grid_ticks);
    if let (Some((start, pitch)), Some(p)) = (app.midi.draw, resp.interact_pointer_pos().or(ui.ctx().pointer_latest_pos())) {
        notes_p.rect(
            note_rect(start, drawn_len(start, p.x), pitch),
            2.0,
            shade(app.midi.last_velocity),
            Stroke::new(1.0, Color32::WHITE),
            StrokeKind::Inside,
        );
        if resp.drag_stopped() {
            let length = drawn_len(start, p.x);
            let note = soundcraft_midi::Note {
                probability: 100,
                pitch: pitch as u8,
                velocity: app.midi.last_velocity,
                muted: false,
                release_velocity: 64,
                channel: 0,
                start,
                length,
            };
            // Into the selected clip under the note's start (this one where it covers it).
            let (into, at) = draw_target(start);
            let notes = tools::stamp(app.midi.chord, clip_key, soundcraft_midi::Note { start: at, ..note });
            if app.run("midi.notes_add", json!({"clip": into.0, "notes": placed(&notes, at)})).is_ok() {
                app.midi.last_length = Some(length);
                audition(app, into, &notes);
            }
            app.midi.selected.clear();
            app.midi.others.clear();
            app.midi.draw = None;
        }
    }
    let mut hear: Option<u8> = None;
    if resp.dragged()
        && let (Some(d), Some(p)) = (&mut app.midi.drag, resp.interact_pointer_pos())
    {
        let n = d.grabbed;
        match d.edge {
            // A moved note keeps its offset from the grid (with Snap Moves, its start lands on a
            // line); an edge lands on a grid line.
            Edge::Move => {
                let raw = f64::from((p.x - d.start.x) / px_per_tick);
                d.dx_ticks = if snap_moves && !alt { snap(n.start as f64 + raw).saturating_sub(n.start) } else { snap_rel(raw) };
                // Folded, a row is not a semitone: the grabbed note goes to the row's pitch.
                d.dy_semi = if geo.fold == 0 { -((p.y - d.start.y) / row_h).round() as i64 } else { i64::from(pitch_at(p.y) - i32::from(n.pitch)) };
                if scale.is_some() && keep_scale {
                    let to = to_scale(i32::from(n.pitch) + d.dy_semi as i32);
                    d.dy_semi = i64::from(to - i32::from(n.pitch));
                }
            }
            Edge::Start => d.dx_ticks = snap(geo.tick_at(p.x)).saturating_sub(n.start),
            Edge::End => d.dx_ticks = snap(geo.tick_at(p.x)).saturating_sub(n.end()),
        }
        if d.edge == Edge::Move && d.dy_semi != 0 {
            hear = u8::try_from(i64::from(n.pitch).saturating_add(d.dy_semi)).ok().filter(|p| *p <= 127);
        }
    }
    // While a note is dragged to another pitch, that pitch sounds (as a held piano key does).
    if let Some(h) = hear
        && app.midi.audition
        && app.midi.drag_heard != Some(h)
    {
        app.midi.drag_heard = Some(h);
        tools::sound_pitch(app, cid, h);
    }
    if resp.drag_stopped()
        && let Some(d) = app.midi.drag.take()
    {
        if app.midi.drag_heard.take().is_some() && app.midi.auditioning.is_some() {
            app.stop_play();
        }
        if d.from.is_none() && !app.midi.selected.contains(&d.index) {
            app.midi.selected.push(d.index);
        }
        // Every selected note, in this clip and the other selected clips, as one undo step.
        let parts = selection_parts(app, cid);
        let follow = app.midi.ctrls_follow;
        let result = match d.edge {
            Edge::End | Edge::Start if d.dx_ticks != 0 => {
                let key = if d.edge == Edge::End { "length" } else { "start" };
                run_parts(app, "midi.notes_resize", &parts, |c, i| json!({"clip": c.0, "indices": i, key: d.dx_ticks}))
            }
            Edge::Move if d.dx_ticks != 0 || d.dy_semi != 0 => {
                // Alt held at the press leaves the notes and drops copies (Alt during the drag
                // only stops the snapping).
                let id = if d.copy { "midi.notes_copy" } else { "midi.notes_move" };
                run_parts(
                    app,
                    id,
                    &parts,
                    |c, i| json!({"clip": c.0, "indices": i, "ticks": d.dx_ticks, "semitones": d.dy_semi, "with_ctrls": follow}),
                )
            }
            _ => Vec::new(),
        };
        // The selection stays on the notes (or moves to the copies) wherever they sorted to.
        let done = !result.is_empty();
        if done {
            set_parts(app, cid, result);
        }
        if done {
            let n = d.grabbed;
            match d.edge {
                Edge::Move if d.dy_semi != 0 && d.from.is_none() => {
                    let moved = soundcraft_midi::Note { pitch: (i64::from(n.pitch) + d.dy_semi).clamp(0, 127) as u8, ..n };
                    audition(app, cid, &[moved]);
                }
                Edge::End => app.midi.last_length = Some(n.length.saturating_add(d.dx_ticks).max(1)),
                Edge::Start => app.midi.last_length = Some(n.end().saturating_sub(n.start.saturating_add(d.dx_ticks)).max(1)),
                Edge::Move => {}
            }
        }
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        match hit(p) {
            // Alt-click: split the note there (on the grid when the grid line is inside it).
            Some((i, _)) if alt => {
                let free_at = geo.tick_at(p.x).round() as i64;
                let grid_at = tools::snap_abs(&s.tempo, base_tick as f64 + geo.tick_at(p.x), snap_ticks).saturating_sub(base_tick);
                let inside = |t: i64| sequence.notes.get(i).is_some_and(|n| n.start < t && t < n.end());
                app.midi.selected = vec![i];
                let _ = tools::split(app, cid, Some(if inside(grid_at) { grid_at } else { free_at }));
            }
            Some((i, _)) => {
                if ui.input(|x| x.modifiers.shift) {
                    if !app.midi.selected.contains(&i) {
                        app.midi.selected.push(i);
                    }
                } else {
                    app.midi.selected = vec![i];
                    app.midi.others.clear();
                }
                // The Pencil's next note takes this one's length and velocity.
                if let Some(n) = sequence.notes.get(i).copied() {
                    app.midi.last_length = Some(n.length);
                    app.midi.last_velocity = n.velocity;
                    audition(app, cid, &[n]);
                }
            }
            None if pencil => {
                let pitch = to_scale(pitch_at(p.y)).clamp(0, 127);
                let start = floor_grid(geo.tick_at(p.x));
                let length = app.midi.last_length.unwrap_or(grid_ticks).max(1);
                let note = soundcraft_midi::Note {
                    probability: 100,
                    pitch: pitch as u8,
                    velocity: app.midi.last_velocity,
                    muted: false,
                    release_velocity: 64,
                    channel: 0,
                    start,
                    length,
                };
                let (into, at) = draw_target(start);
                let notes = tools::stamp(app.midi.chord, clip_key, soundcraft_midi::Note { start: at, ..note });
                if app.run("midi.notes_add", json!({"clip": into.0, "notes": placed(&notes, at)})).is_ok() {
                    audition(app, into, &notes);
                }
                app.midi.selected.clear();
                app.midi.others.clear();
            }
            None => {
                app.midi.selected.clear();
                app.midi.others.clear();
                app.midi.cursor = Some(snap(geo.tick_at(p.x)).clamp(0, len_ticks));
            }
        }
    }
    // Keyboard: Cmd+A select all; arrows transpose (Shift = octave) and move by the grid.
    if ui.rect_contains_pointer(roll) && !ui.ctx().egui_wants_keyboard_input() {
        let (all, up, down, left, right, shift) = ui.input(|i| {
            let arrows = !i.modifiers.alt && !i.modifiers.command && !i.modifiers.ctrl;
            (
                i.events.iter().any(
                    |e| matches!(e, egui::Event::Key { key: egui::Key::A, pressed: true, modifiers, .. } if modifiers.command && !modifiers.alt),
                ),
                arrows && i.key_pressed(egui::Key::ArrowUp),
                arrows && i.key_pressed(egui::Key::ArrowDown),
                arrows && !i.modifiers.shift && i.key_pressed(egui::Key::ArrowLeft),
                arrows && !i.modifiers.shift && i.key_pressed(egui::Key::ArrowRight),
                i.modifiers.shift,
            )
        });
        if all {
            app.midi.selected = sequence.notes.iter().enumerate().filter(|(_, n)| n.start >= 0).map(|(i, _)| i).collect();
            app.midi.others = all_sibling_notes(&s, cid);
        }
        let parts = selection_parts(app, cid);
        let follow = app.midi.ctrls_follow;
        let semi = if up {
            1
        } else if down {
            -1
        } else {
            0
        } * if shift { 12 } else { 1 };
        let ticks = if right {
            grid_ticks
        } else if left {
            -grid_ticks
        } else {
            0
        };
        if (semi != 0 || ticks != 0) && !parts.is_empty() {
            let r = run_parts(
                app,
                "midi.notes_move",
                &parts,
                |c, i| json!({"clip": c.0, "indices": i, "ticks": ticks, "semitones": semi, "with_ctrls": follow}),
            );
            set_parts(app, cid, r);
        }
        // Alt+Left / Alt+Right: the selected notes a grid step shorter or longer.
        let (shorter, longer) = ui.input(|i| {
            let alt = i.modifiers.alt && !i.modifiers.command && !i.modifiers.ctrl && !i.modifiers.shift;
            (alt && i.key_pressed(egui::Key::ArrowLeft), alt && i.key_pressed(egui::Key::ArrowRight))
        });
        if (shorter || longer) && !parts.is_empty() {
            let by = if longer { grid_ticks } else { -grid_ticks };
            let r = run_parts(app, "midi.notes_resize", &parts, |c, i| json!({"clip": c.0, "indices": i, "length": by}));
            set_parts(app, cid, r);
        }
        clipboard_keys(app, ui, cid, &sequence.notes, grid_ticks, len_ticks, base_tick);
    }
    // Space over the roll plays the selected notes (or stops).
    if ui.rect_contains_pointer(roll)
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input(|i| i.key_pressed(egui::Key::Space) && !i.modifiers.command && !i.modifiers.ctrl && !i.modifiers.alt)
    {
        tools::space(app, cid);
    }
    // The piano keys play their notes.
    tools::piano_keys(app, ui, cid, keys, &pitch_at);
    // With the Cleanup panel showing, only for the roll: Delete pressed while using the panel
    // leaves the notes alone. Without it, anywhere in the editor as before.
    if (!app.midi.selected.is_empty() || selection_spans_clips(app))
        && (!cleanup || ui.rect_contains_pointer(roll) || resp.has_focus())
        // Over a controller lane, Delete is the lane's (its selected points).
        && !(lanes_area.is_positive() && ui.rect_contains_pointer(lanes_area))
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
    {
        let parts = selection_parts(app, cid);
        run_parts(app, "midi.note_delete", &parts, |c, i| json!({"clip": c.0, "indices": i}));
        app.midi.selected.clear();
        app.midi.others.clear();
    }
    // Snap to Scale: right-click on the roll, or Ctrl+Shift+K over it.
    let mut snap = false;
    resp.context_menu(|ui| {
        let n = app.midi.selected.len();
        let text = if n == 0 { "Snap All to Scale".to_string() } else { format!("Snap {n} to Scale") };
        if ui.button(text).clicked() {
            snap = true;
            ui.close();
        }
    });
    if ui.rect_contains_pointer(roll)
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input(|i| i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::K))
    {
        snap = true;
    }
    if snap {
        snap_selection(app, cid);
    }
    velocity_lane(app, ui, cid, &sequence.notes, vel, full.min.x, base, &x_of);
    crate::midi_lanes::lanes(app, ui, cid, sequence, &lane_rects, full.min.x, &geo, grid_ticks, len_ticks);
    tools::note_bar(app, ui, cid, Rect::from_min_max(pos2(full.min.x, bottom), pos2(right, full.max.y)), alt);
    if cleanup {
        let rect = Rect::from_min_max(pos2(full.max.x - midi_cleanup_panel::PANEL_W, header.max.y), full.max);
        midi_cleanup_panel::panel(app, ui, cid, rect);
    }
}

/// Copy, cut, paste and duplicate of notes over the roll: Ctrl+C, X, V, D (as keys, or as the
/// copy, cut and paste events the window system sends for them). Paste puts the notes at the
/// paste cursor (a click on empty space), else at the edit insertion when it is in the clip, else
/// where they were copied from; the pasted notes are selected and the cursor moves past them.
/// Duplicate puts copies right after the selection, rounded up to the grid.
fn clipboard_keys(app: &mut SoundApp, ui: &Ui, cid: ClipId, notes: &[soundcraft_midi::Note], grid: i64, len_ticks: i64, base_tick: i64) {
    let (copy, cut, paste, dup) = ui.input(|i| {
        let ev = |f: &dyn Fn(&egui::Event) -> bool| i.events.iter().any(f);
        let k = |want: egui::Key| {
            ev(&|e| matches!(e, egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } if *key == want && modifiers.command && !modifiers.alt && !modifiers.shift))
        };
        (
            k(egui::Key::C) || ev(&|e| matches!(e, egui::Event::Copy)),
            k(egui::Key::X) || ev(&|e| matches!(e, egui::Event::Cut)),
            k(egui::Key::V) || ev(&|e| matches!(e, egui::Event::Paste(_))),
            k(egui::Key::D),
        )
    });
    let chosen: Vec<soundcraft_midi::Note> = app.midi.selected.iter().filter_map(|i| notes.get(*i).copied()).collect();
    if (copy || cut) && !chosen.is_empty() {
        app.midi.clipboard = chosen.clone();
        // The window system sends a paste only when its clipboard holds text.
        ui.ctx().copy_text(format!("{} MIDI notes", chosen.len()));
        if cut {
            let _ = app.run("midi.note_delete", json!({"clip": cid.0, "indices": app.midi.selected}));
            app.midi.selected.clear();
        }
    }
    if paste && !app.midi.clipboard.is_empty() {
        let s = app.engine.session();
        let insertion = s.tempo.samples_to_ticks(s.edit.selection.start, s.sample_rate).saturating_sub(base_tick);
        let from = app.midi.clipboard.iter().map(|n| n.start).min().unwrap_or(0);
        let at = app.midi.cursor.or_else(|| (0..len_ticks).contains(&insertion).then_some(insertion)).unwrap_or(from);
        let clip = app.midi.clipboard.clone();
        if let Ok(r) = app.run("midi.notes_add", json!({"clip": cid.0, "notes": placed(&clip, at)}))
            && let Some(a) = r.get("indices").and_then(|v| v.as_array())
        {
            app.midi.selected = a.iter().filter_map(|v| v.as_u64()).map(|v| v as usize).collect();
            app.midi.cursor = Some(at.saturating_add(duplicate_offset(&clip, grid)));
        }
    }
    if dup && !chosen.is_empty() {
        let ticks = duplicate_offset(&chosen, grid);
        if let Ok(r) =
            app.run("midi.notes_copy", json!({"clip": cid.0, "indices": app.midi.selected, "ticks": ticks, "with_ctrls": app.midi.ctrls_follow}))
            && let Some(a) = r.get("indices").and_then(|v| v.as_array())
        {
            app.midi.selected = a.iter().filter_map(|v| v.as_u64()).map(|v| v as usize).collect();
        }
    }
}

/// The longest an audition plays, seconds.
pub const AUDITION_MAX_S: f64 = 0.6;

/// Plays `notes` (ticks from the clip start) once on the clip's track, soloed, when Audition is
/// on and the transport is free. The player is handed a copy of the session where the clip holds
/// only these notes (the document is untouched); the selection and position come back after.
pub fn audition(app: &mut SoundApp, cid: ClipId, notes: &[soundcraft_midi::Note]) {
    if !app.midi.audition || notes.is_empty() || app.engine.transport.recording {
        return;
    }
    // The transport playing the document (or another preview) is left alone.
    if app.is_playing() && app.midi.auditioning.is_none() {
        return;
    }
    let Some((s, from, to)) = audition_session(app.engine.session(), cid, notes) else { return };
    start_audition(app, s, from, to);
}

/// Plays `s` (an audition copy of the session) from `from` to `to`, ending any audition playing;
/// the edit selection and position come back after ([`step`]).
pub(crate) fn start_audition(app: &mut SoundApp, s: soundcraft_model::Session, from: soundcraft_time::Samples, to: soundcraft_time::Samples) {
    let keep = match app.midi.auditioning.take() {
        Some(k) => {
            app.stop_play();
            k
        }
        None => (app.engine.session().edit.selection, app.engine.transport.position),
    };
    app.audition_session = Some(std::sync::Arc::new(s));
    app.midi.auditioning = Some(keep);
    app.start_play(from, Some(to), None);
}

/// The copy of `s` an audition plays, and the span to play: the clip holds only `notes` (no
/// controllers, no cleanup), its track soloed and unmuted.
pub fn audition_session(
    s: &soundcraft_model::Session,
    cid: ClipId,
    notes: &[soundcraft_midi::Note],
) -> Option<(soundcraft_model::Session, soundcraft_time::Samples, soundcraft_time::Samples)> {
    let (track, clip) = s.find_clip(cid)?;
    let sr = s.sample_rate;
    let base = s.tempo.samples_to_ticks(clip.start, sr);
    let first = notes.iter().map(|n| n.start).min()?;
    let last = notes.iter().map(soundcraft_midi::Note::end).max()?;
    let from = s.tempo.tick_to_samples(base.saturating_add(first), sr);
    let end = s.tempo.tick_to_samples(base.saturating_add(last), sr);
    let most = (AUDITION_MAX_S * sr.as_f64()) as i64;
    let to = end.min(from.saturating_add(most)).max(from.saturating_add(most / 12));
    let mut copy = s.clone();
    for t in &mut copy.tracks {
        t.mixer.solo = t.id == track;
        if t.id == track {
            t.mixer.mute = false;
        }
    }
    let c = copy.find_clip_mut(cid)?;
    c.content = ClipContent::Midi { sequence: soundcraft_midi::Sequence { notes: notes.to_vec(), ctrls: Vec::new() } };
    c.midi_cleanup = None;
    c.muted = false;
    Some((copy, from, to))
}

/// Ends an audition whose playback stopped: the player hears the document again and the edit
/// selection and position come back. Called every frame.
pub fn step(app: &mut SoundApp) {
    if app.midi.auditioning.is_none() || app.is_playing() {
        return;
    }
    let Some((sel, pos)) = app.midi.auditioning.take() else { return };
    app.audition_session = None;
    if let Some(p) = &app.player {
        p.update_session(app.player_session());
    }
    app.engine.session_mut().edit.selection = sel;
    app.engine.transport.position = pos;
}

/// A drag in the velocity lane.
#[derive(Debug, Clone, PartialEq)]
pub enum VelDrag {
    /// A stem was grabbed: it (absolute), or, when it is one of several selected notes, the whole
    /// selection scaled by the same factor. `notes` holds each note's velocity at the press.
    Stems { grabbed: usize, notes: Vec<(usize, u8)>, relative: bool },
    /// A line drawn from `from` (screen): on release the notes under it ramp along it.
    Line { from: egui::Pos2 },
}

/// How far (points) each further note of a chord's stem stands to the right of the first, so
/// stacked notes can be told apart and grabbed.
pub const STEM_STACK: f32 = 4.0;

/// Each note's stem offset: its rank among the notes that start at the same tick (notes are
/// sorted by start, then pitch), times [`STEM_STACK`], at most 6 deep.
pub fn stem_offsets(notes: &[soundcraft_midi::Note]) -> Vec<f32> {
    let mut out = Vec::with_capacity(notes.len());
    let mut rank = 0u8;
    for (i, n) in notes.iter().enumerate() {
        rank = match i.checked_sub(1).and_then(|j| notes.get(j)) {
            Some(prev) if prev.start == n.start => rank.saturating_add(1),
            _ => 0,
        };
        out.push(f32::from(rank.min(6)) * STEM_STACK);
    }
    out
}

/// The stem under `p`: of `heads` (note index, the stem's head on screen) within 6 points
/// sideways, the one whose head is nearest, so a chord's notes are reached by their heads. With
/// `column` false only a stem at or below `p` (8 points of slack over its head) counts: above
/// every head is empty space, where a drag draws a line.
pub fn pick_stem(heads: &[(usize, egui::Pos2)], p: egui::Pos2, column: bool) -> Option<usize> {
    heads
        .iter()
        .filter(|(_, h)| (h.x - p.x).abs() <= 6.0 && (column || p.y >= h.y - 8.0))
        .min_by(|a, b| a.1.distance(p).total_cmp(&b.1.distance(p)))
        .map(|(i, _)| *i)
}

/// The velocities a line from `a` to `b` gives the stems (note index, x) between its ends:
/// `vel_at(y)` reads a lane height as a velocity.
pub fn line_values(stems: &[(usize, f32)], a: egui::Pos2, b: egui::Pos2, vel_at: impl Fn(f32) -> i64) -> Vec<(usize, i64)> {
    let (l, r) = if a.x <= b.x { (a, b) } else { (b, a) };
    stems
        .iter()
        .filter(|(_, x)| *x >= l.x - 1.0 && *x <= r.x + 1.0)
        .map(|(i, x)| {
            let t = if r.x - l.x > 0.5 { ((x - l.x) / (r.x - l.x)).clamp(0.0, 1.0) } else { 0.0 };
            (*i, vel_at(l.y + (r.y - l.y) * t))
        })
        .collect()
}

/// Each note of `notes` (index, velocity at the press) scaled by `to / from` (the grabbed
/// note's new and old velocity), within 1-127.
pub fn scaled(notes: &[(usize, u8)], from: u8, to: i64) -> Vec<(usize, i64)> {
    let k = to as f64 / f64::from(from.max(1));
    notes.iter().map(|(i, v)| (*i, (f64::from(*v) * k).round().clamp(1.0, 127.0) as i64)).collect()
}

/// Sets velocities as one undo step; a refusal (the track is frozen) shows in the status line.
fn set_velocities(app: &mut SoundApp, cid: ClipId, values: &[(usize, i64)], key: Option<&str>) {
    if values.is_empty() {
        return;
    }
    let params =
        json!({"clip": cid.0, "indices": values.iter().map(|v| v.0).collect::<Vec<_>>(), "values": values.iter().map(|v| v.1).collect::<Vec<_>>()});
    let r = match key {
        Some(k) => app.engine.execute_merged("midi.notes_velocity", &params, k).map_err(|e| e.to_string()),
        None => app.run("midi.notes_velocity", params),
    };
    if let Err(e) = r {
        app.ui.status = e;
    }
}

/// The velocity lane: a stem per note (a chord's stems side by side). With notes selected only
/// they are edited. Click in a stem's column or drag its head to set its velocity; drag one of
/// several selected stems to scale the whole selection; drag from empty space (or with the
/// Pencil) to draw a line the notes under it ramp along. A drag is one undo step on release.
fn velocity_lane(
    app: &mut SoundApp,
    ui: &mut Ui,
    cid: ClipId,
    notes: &[soundcraft_midi::Note],
    vel: Rect,
    label_x: f32,
    base: Color32,
    x_of: &dyn Fn(i64) -> f32,
) {
    let t = Tokens::current();
    ui.painter().rect_filled(vel, 0.0, Color32::from_rgb(28, 28, 30));
    let vp = ui.painter().with_clip_rect(vel);
    ui.painter().text(pos2(label_x + 6.0, vel.center().y), Align2::LEFT_CENTER, "Velocity", regular(10.0), t.text_dim);
    let span = (vel.height() - 4.0).max(1.0);
    let y_of_v = |v: f32| vel.max.y - 2.0 - span * v / 127.0;
    let vel_at = |y: f32| (((vel.max.y - 2.0 - y) / span) * 127.0).round().clamp(1.0, 127.0) as i64;
    let offsets = stem_offsets(notes);
    let selected = app.midi.selected.clone();
    let editable = |i: usize| selected.is_empty() || selected.contains(&i);
    let stem_x = |i: usize, n: &soundcraft_midi::Note| x_of(n.start) + 1.0 + offsets.get(i).copied().unwrap_or(0.0);
    let heads: Vec<(usize, egui::Pos2)> =
        notes.iter().enumerate().filter(|(i, _)| editable(*i)).map(|(i, n)| (i, pos2(stem_x(i, n), y_of_v(f32::from(n.velocity))))).collect();
    let vresp = ui.interact(vel, ui.id().with(("vel", cid.0)), Sense::click_and_drag());
    let pencil = app.engine.session().edit.tool == Tool::Pencil;
    // Where the button went down, not where the drag was first noticed (a few points on).
    if vresp.drag_started()
        && let Some(p) = ui.input(|i| i.pointer.press_origin()).or(vresp.interact_pointer_pos())
    {
        app.midi.vel_drag = Some(match pick_stem(&heads, p, false).filter(|_| !pencil) {
            Some(i) => {
                let relative = selected.len() > 1 && selected.contains(&i);
                let picked: Vec<usize> = if relative { selected.clone() } else { vec![i] };
                let notes = picked.iter().filter_map(|j| notes.get(*j).map(|n| (*j, n.velocity))).collect();
                VelDrag::Stems { grabbed: i, notes, relative }
            }
            None => VelDrag::Line { from: p },
        });
    }
    // What the drag would set, shown before it is committed on release.
    let pointer = vresp.interact_pointer_pos().or(ui.ctx().pointer_latest_pos());
    let live = vresp.dragged() || vresp.drag_stopped();
    let mut preview: Vec<(usize, i64)> = Vec::new();
    let mut readout: Option<(egui::Pos2, i64)> = None;
    match (app.midi.vel_drag.clone(), pointer) {
        (Some(VelDrag::Stems { grabbed, notes: orig, relative }), Some(p)) if live => {
            let to = vel_at(p.y);
            preview = if relative {
                let from = orig.iter().find(|(i, _)| *i == grabbed).map_or(100, |(_, v)| *v);
                scaled(&orig, from, to)
            } else {
                vec![(grabbed, to)]
            };
            readout = notes.get(grabbed).map(|n| (pos2(stem_x(grabbed, n) + 6.0, y_of_v(to as f32)), to));
            app.midi.last_velocity = to.clamp(1, 127) as u8;
        }
        (Some(VelDrag::Line { from }), Some(p)) if live => {
            vp.line_segment([from, p], Stroke::new(1.5, t.accent));
            let stems: Vec<(usize, f32)> = heads.iter().map(|(i, h)| (*i, h.x)).collect();
            preview = line_values(&stems, from, p, vel_at);
        }
        _ => {}
    }
    for (i, n) in notes.iter().enumerate() {
        let x = stem_x(i, n);
        let v = preview.iter().find(|(j, _)| *j == i).map_or(f32::from(n.velocity), |(_, v)| *v as f32);
        let y = y_of_v(v);
        let col = if selected.contains(&i) {
            Color32::WHITE
        } else if editable(i) {
            base
        } else {
            base.gamma_multiply(0.35)
        };
        vp.line_segment([pos2(x, vel.max.y - 2.0), pos2(x, y)], Stroke::new(3.0, col));
        vp.circle_filled(pos2(x, y), 2.5, col);
    }
    if let Some((at, v)) = readout {
        ui.painter().text(at, Align2::LEFT_CENTER, v.to_string(), regular(10.0), t.text);
    }
    if vresp.drag_stopped() {
        app.midi.vel_drag = None;
        set_velocities(app, cid, &preview, None);
    }
    if vresp.clicked()
        && let Some(p) = vresp.interact_pointer_pos()
        && let Some(i) = pick_stem(&heads, p, true)
    {
        let v = vel_at(p.y);
        set_velocities(app, cid, &[(i, v)], None);
        app.midi.last_velocity = v.clamp(1, 127) as u8;
    }
}

/// F over the roll this frame (the shortcut handler left it to the editor: [`owns_key`]).
fn fit_key(app: &SoundApp, ui: &Ui, roll: Rect) -> bool {
    app.ui.show_midi_editor
        && app.dialogs.open.is_none()
        && ui.rect_contains_pointer(roll)
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input(|i| i.key_pressed(egui::Key::F) && !i.modifiers.command && !i.modifiers.ctrl && !i.modifiers.alt && !i.modifiers.shift)
}

/// The wheel over the editor (`area`): notes (plain), time (Shift or sideways), time zoom around
/// the pointer (Ctrl) and note height (Ctrl+Alt).
fn wheel(ui: &Ui, area: Rect, roll: Rect, v: &mut MidiView, span: (i64, i64)) {
    let len = span.1.saturating_sub(span.0);
    if !ui.rect_contains_pointer(area) {
        return;
    }
    let events: Vec<(egui::Vec2, egui::Modifiers, egui::Pos2)> = ui.input(|i| {
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
    let width = roll.width().max(1.0);
    for (d, m, p) in events {
        let ctrl = m.command || m.ctrl;
        let dy = if d.y != 0.0 { d.y } else { d.x };
        // Note height: Ctrl+Shift (as the Vocal Editor) or Ctrl+Alt.
        if ctrl && (m.alt || m.shift) {
            *v = row_zoom(*v, 1.0015f32.powf(dy * 4.0), p.y - roll.min.y);
        } else if ctrl {
            *v = zoom_about_span(*v, 1.0015f32.powf(dy * 4.0), (p.x - roll.min.x).clamp(0.0, width), width, span);
        } else {
            let dx = if m.shift { d.x + d.y } else { d.x };
            if dx != 0.0 {
                let ppt = f64::from(v.zoom.unwrap_or(width / len.max(1) as f32).max(1e-6));
                *v = clamp_left(MidiView { left: v.left - f64::from(dx) / ppt, ..*v }, width, span);
            }
            if !m.shift && d.y != 0.0 {
                v.scroll -= d.y;
            }
        }
    }
}

/// The horizontal scrollbar under the roll: drag the thumb, or click the track to jump there.
fn scrollbar(ui: &mut Ui, bar: Rect, v: &mut MidiView, width: f32, span: (i64, i64), cid: ClipId) {
    let t = Tokens::current();
    ui.painter().rect_filled(bar, 3.0, t.panel_bg2);
    let Some(z) = v.zoom else { return };
    let lo = span.0 as f64;
    let len_f = span.1.saturating_sub(span.0).max(1) as f64;
    let shown = (f64::from(width) / f64::from(z)).min(len_f);
    let to_x = |tick: f64| bar.min.x + ((tick - lo) / len_f * f64::from(bar.width())) as f32;
    let thumb = Rect::from_min_max(pos2(to_x(v.left), bar.min.y + 1.0), pos2(to_x(v.left + shown).max(to_x(v.left) + 12.0), bar.max.y - 1.0));
    let resp = ui.interact(bar, ui.id().with(("midi_hbar", cid.0)), Sense::click_and_drag());
    let fill = if resp.dragged() || resp.hovered() { t.accent } else { t.border };
    ui.painter().rect_filled(thumb, 3.0, fill);
    let per_px = len_f / f64::from(bar.width().max(1.0));
    if resp.dragged() {
        v.left += f64::from(resp.drag_delta().x) * per_px;
    } else if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        v.left = lo + f64::from(p.x - bar.min.x) * per_px - shown / 2.0;
    }
    *v = clamp_left(*v, width, span);
}

/// An Event operation (Quantize, Legato) from the editor: on the selected notes, or every note
/// of the clip when none is selected. The selection follows the notes through the re-sort.
pub fn run_on_selection(app: &mut SoundApp, cid: ClipId, id: &str, mut params: serde_json::Value) {
    let ours = app.midi.clip == Some(cid);
    // Notes selected in several clips: each clip's, in one undo step.
    if ours && selection_spans_clips(app) {
        let parts = selection_parts(app, cid);
        let r = run_parts(app, id, &parts, |c, i| {
            let mut p = params.clone();
            p["clips"] = json!([c.0]);
            p["indices"] = json!(i);
            p
        });
        set_parts(app, cid, r);
        return;
    }
    // None selected with several clips in the editor: every note of each.
    let editable = selected_midi_clips(app.engine.session());
    let all = ours && app.midi.selected.is_empty() && editable.len() > 1 && editable.contains(&cid);
    params["clips"] = if all { json!(editable.iter().map(|c| c.0).collect::<Vec<_>>()) } else { json!([cid.0]) };
    let had = !app.midi.selected.is_empty() && ours;
    if had {
        params["indices"] = json!(app.midi.selected);
    }
    if let Ok(r) = app.run(id, params)
        && had
        && let Some(a) = r.get("indices").and_then(|v| v.as_array())
    {
        app.midi.selected = a.iter().filter_map(|v| v.as_u64()).map(|v| v as usize).collect();
    }
}

/// Snap to Scale on the selected notes (every note when none is selected). The selection stays
/// on the same notes: the command returns where they sit after the re-sort.
/// The editor's drawing grid of `snap` ticks swung as the clip plays (`q`, its playback quantize):
/// its swing, or its groove's points. `None`: the grid is straight (no swing or groove, or no grid).
pub fn drawing_grid(q: Option<&soundcraft_midi::ops::PlayQuantize>, snap: i64) -> Option<soundcraft_midi::ops::PlayQuantize> {
    q.filter(|q| (q.groove.is_some() || q.swing > 0.0) && snap > 1).map(|q| soundcraft_midi::ops::PlayQuantize {
        grid: snap,
        strength: 1.0,
        swing: q.swing,
        groove: q.groove.clone(),
    })
}

pub fn snap_selection(app: &mut SoundApp, cid: ClipId) {
    let mut params = json!({"clip": cid.0});
    let had = !app.midi.selected.is_empty();
    if had {
        params["indices"] = json!(app.midi.selected);
    }
    if let Ok(r) = app.run("midi.snap_to_scale", params)
        && had
        && let Some(a) = r.get("indices").and_then(|v| v.as_array())
    {
        app.midi.selected = a.iter().filter_map(|v| v.as_u64()).map(|v| v as usize).collect();
    }
}

/// What the editor shows for a Juke pattern clip: its steps live in the Juke window, so a note,
/// a button that opens the Juke window on this pattern, and one that converts it to plain MIDI
/// notes to edit here.
fn pattern_notice(app: &mut SoundApp, ui: &mut Ui, body: Rect, track: soundcraft_model::TrackId, cid: ClipId, name: &str) {
    let t = Tokens::current();
    let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(body.shrink(24.0)));
    inner.label(egui::RichText::new(format!("{name} is a Juke pattern")).strong().size(14.0));
    inner.add_space(4.0);
    inner
        .label(egui::RichText::new("Its steps are edited in the Juke window's Steps view, with the rest of the track's patterns.").color(t.text_dim));
    inner.add_space(10.0);
    inner.horizontal(|ui| {
        if ui.button("Open in Juke").on_hover_text("The Juke window, on this pattern's steps").clicked() {
            crate::juke_window::show_steps(app, track, Some(cid));
        }
        if ui.button("Convert to MIDI").on_hover_text("Keep the notes as a plain MIDI clip, to edit them here by hand").clicked() {
            let _ = app.run("pattern.to_midi", json!({"clip": cid.0}));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_midi::ops::{PlayQuantize, builtin_grooves};

    #[test]
    fn the_drawing_grid_swings_as_the_clip_plays() {
        let s = soundcraft_model::Session::default();
        let at = |g: &PlayQuantize, t: i64| soundcraft_mix::quantized_start(&s, 0, g, t);
        // Swing: the off-beat 16th moves by a third of the swing's share of a step.
        let swing = PlayQuantize { grid: 240, strength: 0.5, swing: 0.5, groove: None };
        let g = drawing_grid(Some(&swing), 240).unwrap();
        assert_eq!((at(&g, 0), at(&g, 240), at(&g, 480)), (0, 280, 480), "full strength, on the editor's grid");
        // A groove: the editor's lines go to its points.
        let groove = builtin_grooves().into_iter().find(|g| g.name == "Swing 8ths 66%").unwrap();
        let q = PlayQuantize { grid: 480, strength: 1.0, swing: 0.0, groove: Some(groove) };
        assert_eq!(at(&drawing_grid(Some(&q), 480).unwrap(), 480), 633);
        // Straight playback quantize, none, or the grid off: a straight grid.
        assert!(drawing_grid(Some(&PlayQuantize { grid: 240, strength: 1.0, swing: 0.0, groove: None }), 240).is_none());
        assert!(drawing_grid(None, 240).is_none() && drawing_grid(Some(&swing), 1).is_none());
    }
}
