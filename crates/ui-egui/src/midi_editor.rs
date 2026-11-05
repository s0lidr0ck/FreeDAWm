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
}

impl RollGeometry {
    pub fn x_of(&self, tick: i64) -> f32 {
        self.roll.min.x + ((tick as f64 - self.left) * f64::from(self.px_per_tick)) as f32
    }

    /// The tick (from the clip's start) under `x`.
    pub fn tick_at(&self, x: f32) -> f64 {
        self.left + f64::from(x - self.roll.min.x) / f64::from(self.px_per_tick.max(1e-6))
    }

    pub fn y_of(&self, pitch: i32) -> f32 {
        self.roll.min.y + (127 - pitch) as f32 * self.row_h - self.scroll
    }

    pub fn pitch_at(&self, y: f32) -> i32 {
        127 - ((y - self.roll.min.y + self.scroll) / self.row_h.max(1.0)).floor() as i32
    }
}

/// `v` zoomed by `factor` keeping the tick under `x` (points from the roll's left) still, never
/// wider than fitting `len` ticks into `width` points nor closer than [`MAX_PX_PER_TICK`].
pub fn zoom_about(v: MidiView, factor: f32, x: f32, width: f32, len: i64) -> MidiView {
    let fit = width / len.max(1) as f32;
    let now = v.zoom.unwrap_or(fit).max(fit);
    let new = (now * factor).clamp(fit, MAX_PX_PER_TICK.max(fit));
    let under = v.left + f64::from(x) / f64::from(now);
    let left = under - f64::from(x) / f64::from(new);
    let zoom = (new > fit * 1.0001).then_some(new);
    clamp_left(MidiView { zoom, left: if zoom.is_some() { left } else { 0.0 }, ..v }, width, len)
}

/// `v` with its left edge inside the clip.
fn clamp_left(v: MidiView, width: f32, len: i64) -> MidiView {
    let Some(z) = v.zoom else { return MidiView { left: 0.0, ..v } };
    let shown = f64::from(width) / f64::from(z.max(1e-6));
    let max = (len as f64 - shown).max(0.0);
    MidiView { left: v.left.clamp(0.0, max), ..v }
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
    let arrows = matches!(key, Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight) && plain && !app.midi.selected.is_empty();
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
            ctrl_drag: None,
            drawn_lanes: Vec::new(),
            ctrl_menu: None,
            grid: tools::MidiGrid::default(),
            ghosts: true,
            ghost_tracks: None,
            piano_key: None,
            hover: None,
            pending_zoom: None,
            zoom_factor: None,
        }
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
    pub index: usize,
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

/// `notes` placed so the earliest starts at `at` (they keep their spacing), as `midi.notes_add`
/// takes them.
pub fn placed(notes: &[soundcraft_midi::Note], at: i64) -> serde_json::Value {
    let first = notes.iter().map(|n| n.start).min().unwrap_or(0);
    let v: Vec<serde_json::Value> = notes
        .iter()
        .map(|n| {
            json!({"pitch": n.pitch, "start": n.start.saturating_sub(first).saturating_add(at).max(0), "length": n.length, "velocity": n.velocity, "channel": n.channel})
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
pub fn target_clip(app: &SoundApp) -> Option<ClipId> {
    let s = app.engine.session();
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
    let track_color = app.engine.session().track(track).map_or([120, 120, 200], |t| t.color);
    let header_text = format!("{} · {} selected", clip.name, app.midi.selected.len());
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
            .on_hover_text("Show CC, pitch bend and aftertouch lanes under the velocity lane");
        if ui.button("Legato").on_hover_text("Each note lasts until the next one").clicked() {
            op = Some(("event.change_duration", json!({"legato": 0})));
            ui.close();
        }
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
    match app.midi.pending_zoom.take() {
        Some(f) if f > 0.0 => view_op = Some(ViewOp::Factor(f)),
        Some(_) => view_op = Some(ViewOp::Fit),
        None => {}
    }
    // Header, left: after the clip's line, the editing tools, in the room the view leaves.
    let left_bar = Rect::from_min_max(pos2(text_end + 10.0, header.min.y + 1.0), pos2(tb.min_rect().min.x - 8.0, header.max.y - 1.0));
    let mut lt = ui.new_child(egui::UiBuilder::new().max_rect(left_bar).layout(egui::Layout::left_to_right(egui::Align::Center)));
    lt.set_clip_rect(left_bar.intersect(lt.clip_rect()));
    lt.spacing_mut().item_spacing.x = 4.0;
    if lt.small_button("Select all").on_hover_text("Select every note of the clip (Ctrl+A)").clicked() {
        tools::select_all(app, cid);
    }
    if lt.small_button("Split").on_hover_text("Split the selected notes at the playhead (Alt-click a note: split it there)").clicked() {
        let _ = tools::split(app, cid, None);
    }
    if lt.small_button("Join").on_hover_text("Join the selected notes of each pitch into one").clicked() {
        let _ = tools::join(app, cid);
    }
    if lt.small_button("Quantize").on_hover_text("Quantize to the grid").clicked() {
        op = Some(("event.quantize", json!({"grid": grid_ticks, "strength": 100})));
    }
    let grid_text = format!("Grid: {}", app.midi.grid.label(edit_grid));
    lt.menu_button(grid_text, |ui| tools::grid_menu(app, ui)).response.on_hover_text("The MIDI Editor's own grid (hold Alt while dragging: no snap)");
    if let Some((id, mut p)) = op {
        p["clips"] = json!([cid.0]);
        let _ = app.run(id, p);
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
        Some(ViewOp::Fit) => view = MidiView { zoom: None, left: 0.0, ..view },
        Some(ViewOp::Factor(f)) => {
            // About the middle of the roll.
            let fit = width / len_ticks as f32;
            let now = view.zoom.unwrap_or(fit).max(fit);
            view = zoom_about(view, (edit_ppt * f) / now, width / 2.0, width, len_ticks);
        }
        None => {}
    }
    wheel(ui, roll.union(vel).union(hbar).union(lanes_area), roll, &mut view, len_ticks);
    if fit_key(app, ui, roll) {
        view = MidiView { zoom: None, left: 0.0, ..view };
    }
    view = clamp_left(view, width, len_ticks);
    view.scroll = view.scroll.clamp(0.0, (128.0 * view.row_h - roll.height()).max(0.0));
    scrollbar(ui, hbar, &mut view, width, len_ticks, cid);
    crate::midi_editor_link::follow(app, ui.ctx(), &mut view, view_before, width, base_tick);
    view = clamp_left(view, width, len_ticks);
    let fit = width / len_ticks as f32;
    let px_per_tick = view.zoom.unwrap_or(fit).max(fit);
    let geo = RollGeometry { clip: cid, roll, px_per_tick, fit, left: view.left, row_h: view.row_h, scroll: view.scroll };
    app.midi.zoom_factor = tools::factor_of(view.zoom, fit, edit_ppt);
    app.midi.views.insert(cid, view);
    app.midi.drawn = Some(geo);
    app.midi.scroll = geo.scroll;
    let row_h = geo.row_h;
    let rows = (roll.height() / row_h).floor() as i32;
    let x_of = |tick: i64| geo.x_of(tick);
    let y_of = |pitch: i32| geo.y_of(pitch);
    let pitch_at = |y: f32| geo.pitch_at(y);
    let painter = ui.painter().with_clip_rect(roll.union(keys));
    // Background rows and keys.
    let top = pitch_at(roll.min.y);
    for r in 0..=rows + 1 {
        let pitch = top - r;
        let y = y_of(pitch);
        let row = Rect::from_min_size(pos2(roll.min.x, y), vec2(roll.width(), row_h));
        painter.rect_filled(row, 0.0, if is_black(pitch) { Color32::from_rgb(30, 30, 32) } else { Color32::from_rgb(40, 40, 42) });
        let key = Rect::from_min_size(pos2(keys.min.x, y), vec2(KEY_W - 2.0, row_h - 1.0));
        painter.rect_filled(key, 1.0, if is_black(pitch) { Color32::from_rgb(20, 20, 20) } else { Color32::from_rgb(225, 225, 225) });
        if pitch.rem_euclid(12) == 0 && (0..=127).contains(&pitch) {
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
    let first = (geo.tick_at(roll.min.x).floor() as i64).max(0);
    let last_tick = (geo.tick_at(roll.max.x).ceil() as i64).min(len_ticks);
    let beats = s.tempo.meter_at_tick(base_tick.saturating_add(first)).ticks_per_beat() as f32 * px_per_tick > 4.0;
    let grid_lines = if snap_ticks as f32 * px_per_tick > 4.0 && snap_ticks > 1 { snap_ticks } else { 0 };
    for (tick, kind) in tools::grid_lines(&s.tempo, base_tick, first, last_tick, beats, grid_lines) {
        let color = match kind {
            tools::LineKind::Bar => Color32::from_rgb(88, 88, 96),
            tools::LineKind::Beat => Color32::from_rgb(60, 60, 66),
            tools::LineKind::Grid => Color32::from_rgb(48, 48, 52),
        };
        let x = x_of(tick);
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
    let in_drag = |i: usize, d: &NoteDrag| d.index == i || (app.midi.selected.contains(&i) && app.midi.selected.contains(&d.index));
    let shade = |v: u8| {
        let k = 0.45 + 0.55 * f32::from(v) / 127.0;
        Color32::from_rgb((f32::from(base.r()) * k) as u8, (f32::from(base.g()) * k) as u8, (f32::from(base.b()) * k) as u8)
    };
    let note_rect = |st: i64, ln: i64, p: i32| {
        Rect::from_min_max(pos2(x_of(st), y_of(p) + 1.0), pos2(x_of(st.saturating_add(ln)).max(x_of(st) + 3.0), y_of(p) + row_h - 1.0))
    };
    // Ghost notes of the other clips, behind this clip's.
    let ghost_notes = midi_ghosts::shown(app, &s, cid);
    let ghost_rects = midi_ghosts::rects(&ghost_notes, &x_of, &y_of, row_h);
    midi_ghosts::draw(&notes_p, &ghost_notes, &ghost_rects);
    let mut ghosts: Vec<Rect> = Vec::new();
    for (i, n) in sequence.notes.iter().enumerate() {
        let (mut st, mut ln, mut p) = (n.start, n.length, i32::from(n.pitch));
        if let Some(d) = drag.as_ref().filter(|d| in_drag(i, d)) {
            match d.edge {
                Edge::End => ln = ln.saturating_add(d.dx_ticks).max(1),
                Edge::Start => {
                    let end = n.end();
                    st = st.saturating_add(d.dx_ticks).clamp(0, end.saturating_sub(1).max(0));
                    ln = end.saturating_sub(st).max(1);
                }
                Edge::Move if d.copy => ghosts.push(note_rect(st.saturating_add(d.dx_ticks).max(0), ln, (p + d.dy_semi as i32).clamp(0, 127))),
                Edge::Move => {
                    st = st.saturating_add(d.dx_ticks).max(0);
                    p = (p + d.dy_semi as i32).clamp(0, 127);
                }
            }
        }
        if let Some(end) = edits.and_then(|e| midi_cleanup_panel::edit_of(e, i)).and_then(|e| e.new_end).filter(|end| *end < st + ln) {
            ln = (end - st).max(0);
        }
        let selected = app.midi.selected.contains(&i);
        let fill = if selected { Color32::WHITE } else { shade(n.velocity) };
        let r = note_rect(st, ln, p);
        notes_p.rect(r, 2.0, fill, Stroke::new(1.0, Color32::BLACK), StrokeKind::Inside);
        // The note's name, when it fits.
        if r.width() >= 24.0 && r.height() >= 8.0 && r.intersects(roll) {
            let name = soundcraft_midi::note_name(p.clamp(0, 127) as u8);
            let size = (r.height() - 1.0).clamp(7.0, 10.0);
            let color = if selected { Color32::from_rgb(20, 20, 24) } else { Color32::from_white_alpha(225) };
            notes_p.with_clip_rect(r.intersect(roll)).text(pos2(r.min.x + 3.0, r.center().y), Align2::LEFT_CENTER, name, regular(size), color);
        }
    }
    for g in ghosts {
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
    crate::midi_editor_link::ruler(app, ui, ruler, &geo, base_tick, clip.range());
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
            let r = Rect::from_min_max(
                pos2(x_of(n.start), y_of(i32::from(n.pitch))),
                pos2(x_of(n.start + n.length).max(x_of(n.start) + 3.0), y_of(i32::from(n.pitch)) + row_h),
            );
            r.contains(pos).then(|| (i, edge_at(pos.x - r.min.x, r.width())))
        })
    };
    // A tick from the clip start on the grid (lines counted from each bar's start); a span of
    // ticks to a whole number of grid steps. Alt held: no snap.
    let free = |tk: f64| tk.round() as i64;
    let snap =
        |tk: f64| -> i64 { if alt { free(tk) } else { tools::snap_abs(&s.tempo, base_tick as f64 + tk, snap_ticks).saturating_sub(base_tick) } };
    let snap_rel = |tk: f64| -> i64 { if alt { free(tk) } else { ((tk / snap_ticks as f64).round() as i64).saturating_mul(snap_ticks) } };
    let floor_grid = |tk: f64| -> i64 {
        if alt {
            return (tk.floor() as i64).max(0);
        }
        let at = tools::snap_abs(&s.tempo, base_tick as f64 + tk, snap_ticks).saturating_sub(base_tick);
        (if at as f64 > tk { at.saturating_sub(snap_ticks) } else { at }).max(0)
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
        match hit(p) {
            Some((i, edge)) => {
                if !app.midi.selected.contains(&i) {
                    app.midi.selected = vec![i];
                }
                app.midi.drag = Some(NoteDrag { index: i, edge, start: p, dx_ticks: 0, dy_semi: 0, copy: alt && edge == Edge::Move });
            }
            None if pencil => app.midi.draw = Some((floor_grid(geo.tick_at(p.x)), pitch_at(p.y).clamp(0, 127))),
            None => app.midi.band = Some(p),
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
                    let r = Rect::from_min_max(
                        pos2(x_of(n.start), y_of(i32::from(n.pitch))),
                        pos2(x_of(n.start + n.length), y_of(i32::from(n.pitch)) + row_h),
                    );
                    r.intersects(band)
                })
                .map(|(i, _)| i)
                .collect();
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
            let note =
                soundcraft_midi::Note { pitch: pitch as u8, velocity: app.midi.last_velocity, release_velocity: 64, channel: 0, start, length };
            if app.run("midi.notes_add", json!({"clip": cid.0, "notes": placed(&[note], start)})).is_ok() {
                app.midi.last_length = Some(length);
                audition(app, cid, &[note]);
            }
            app.midi.selected.clear();
            app.midi.draw = None;
        }
    }
    if resp.dragged()
        && let (Some(d), Some(p)) = (&mut app.midi.drag, resp.interact_pointer_pos())
        && let Some(n) = sequence.notes.get(d.index)
    {
        match d.edge {
            // A moved note keeps its offset from the grid; an edge lands on a grid line.
            Edge::Move => {
                d.dx_ticks = snap_rel(f64::from((p.x - d.start.x) / px_per_tick));
                d.dy_semi = -((p.y - d.start.y) / row_h).round() as i64;
            }
            Edge::Start => d.dx_ticks = snap(geo.tick_at(p.x)).saturating_sub(n.start),
            Edge::End => d.dx_ticks = snap(geo.tick_at(p.x)).saturating_sub(n.end()),
        }
    }
    if resp.drag_stopped()
        && let Some(d) = app.midi.drag.take()
    {
        let mut picked = app.midi.selected.clone();
        if !picked.contains(&d.index) {
            picked.push(d.index);
        }
        let grabbed = sequence.notes.get(d.index).copied();
        let result = match d.edge {
            Edge::End | Edge::Start if d.dx_ticks != 0 => {
                let key = if d.edge == Edge::End { "length" } else { "start" };
                app.run("midi.notes_resize", json!({"clip": cid.0, "indices": picked, key: d.dx_ticks})).ok()
            }
            Edge::Move if d.dx_ticks != 0 || d.dy_semi != 0 => {
                // Alt held at the press leaves the notes and drops copies (Alt during the drag
                // only stops the snapping).
                let id = if d.copy { "midi.notes_copy" } else { "midi.notes_move" };
                app.run(id, json!({"clip": cid.0, "indices": picked, "ticks": d.dx_ticks, "semitones": d.dy_semi})).ok()
            }
            _ => None,
        };
        // The selection stays on the notes (or moves to the copies) wherever they sorted to.
        if let Some(a) = result.as_ref().and_then(|r| r.get("indices")).and_then(|v| v.as_array()) {
            app.midi.selected = a.iter().filter_map(|v| v.as_u64()).map(|v| v as usize).collect();
        }
        if let Some(n) = grabbed.filter(|_| result.is_some()) {
            match d.edge {
                Edge::Move if d.dy_semi != 0 => {
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
                }
                // The Pencil's next note takes this one's length and velocity.
                if let Some(n) = sequence.notes.get(i).copied() {
                    app.midi.last_length = Some(n.length);
                    app.midi.last_velocity = n.velocity;
                    audition(app, cid, &[n]);
                }
            }
            None if pencil => {
                let pitch = pitch_at(p.y).clamp(0, 127);
                let start = floor_grid(geo.tick_at(p.x));
                let length = app.midi.last_length.unwrap_or(grid_ticks).max(1);
                let note =
                    soundcraft_midi::Note { pitch: pitch as u8, velocity: app.midi.last_velocity, release_velocity: 64, channel: 0, start, length };
                if app.run("midi.notes_add", json!({"clip": cid.0, "notes": placed(&[note], start)})).is_ok() {
                    audition(app, cid, &[note]);
                }
                app.midi.selected.clear();
            }
            None => {
                app.midi.selected.clear();
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
            app.midi.selected = (0..sequence.notes.len()).collect();
        }
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
        if (semi != 0 || ticks != 0)
            && !app.midi.selected.is_empty()
            && let Ok(r) = app.run("midi.notes_move", json!({"clip": cid.0, "indices": app.midi.selected, "ticks": ticks, "semitones": semi}))
            && let Some(a) = r.get("indices").and_then(|v| v.as_array())
        {
            app.midi.selected = a.iter().filter_map(|v| v.as_u64()).map(|v| v as usize).collect();
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
    if !app.midi.selected.is_empty()
        && (!cleanup || ui.rect_contains_pointer(roll) || resp.has_focus())
        // Over a controller lane, Delete is the lane's (its selected points).
        && !(lanes_area.is_positive() && ui.rect_contains_pointer(lanes_area))
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
    {
        let _ = app.run("midi.note_delete", json!({"clip": cid.0, "indices": app.midi.selected}));
        app.midi.selected.clear();
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
        if let Ok(r) = app.run("midi.notes_copy", json!({"clip": cid.0, "indices": app.midi.selected, "ticks": ticks}))
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
fn wheel(ui: &Ui, area: Rect, roll: Rect, v: &mut MidiView, len: i64) {
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
            *v = zoom_about(*v, 1.0015f32.powf(dy * 4.0), (p.x - roll.min.x).clamp(0.0, width), width, len);
        } else {
            let dx = if m.shift { d.x + d.y } else { d.x };
            if dx != 0.0 {
                let ppt = f64::from(v.zoom.unwrap_or(width / len.max(1) as f32).max(1e-6));
                *v = clamp_left(MidiView { left: v.left - f64::from(dx) / ppt, ..*v }, width, len);
            }
            if !m.shift && d.y != 0.0 {
                v.scroll -= d.y;
            }
        }
    }
}

/// The horizontal scrollbar under the roll: drag the thumb, or click the track to jump there.
fn scrollbar(ui: &mut Ui, bar: Rect, v: &mut MidiView, width: f32, len: i64, cid: ClipId) {
    let t = Tokens::current();
    ui.painter().rect_filled(bar, 3.0, t.panel_bg2);
    let Some(z) = v.zoom else { return };
    let len_f = len.max(1) as f64;
    let shown = (f64::from(width) / f64::from(z)).min(len_f);
    let to_x = |tick: f64| bar.min.x + (tick / len_f * f64::from(bar.width())) as f32;
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
        v.left = f64::from(p.x - bar.min.x) * per_px - shown / 2.0;
    }
    *v = clamp_left(*v, width, len);
}

/// Snap to Scale on the selected notes (every note when none is selected). The selection stays
/// on the same notes: the command returns where they sit after the re-sort.
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
