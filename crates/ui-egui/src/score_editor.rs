//! Score Editor: the selected MIDI clip (or track) as notation on a grand staff, where notes are
//! entered and edited too.
//!
//! A readable sketch rather than an engraving engine: note heads by duration (whole, half,
//! quarter, eighth with flags), stems, accidentals (flats in a flat key, else sharps), ledger
//! lines, bar lines from the meter map, and the clef split at middle C.
//!
//! Entry goes through the piano roll's commands, so every edit is one undo step and scriptable:
//! the value row above the staff (whole to 32nd, dotted, triplet; ♯ ♭ ♮ for the next note) sets
//! the length of new notes and the step the cursor moves. A click on empty staff adds a note
//! there (`midi.note_add`, snapped to the bar-counted grid of the value; its pitch the line's or
//! space's natural note, raised or lowered by the chosen accidental or else by the key on the
//! Key ruler, so an F in G major is F♯); a click on a head selects it (the piano roll's
//! selection; Shift adds). With the pointer over the staff: Delete removes the selected notes,
//! Up and Down move them a step of the scale (Alt: a semitone), the value keys 1–6 (whole to
//! 32nd) choose the value and set the selected notes' length, A–G enter that note nearest the
//! last one at the cursor and move the cursor on by the value, R moves it on without a note
//! (a rest), and Left and Right move it back and on.

use crate::SoundApp;
use crate::theme::{bold, regular};
use egui::{Align2, Color32, Key, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};
use serde_json::json;
use soundcraft_model::{ClipContent, ClipId};
use soundcraft_time::TICKS_PER_QUARTER;

const STEP: f32 = 6.0; // half the distance between staff lines

/// The note values the value row offers, whole to 32nd.
pub const VALUES: [(&str, i64); 6] = [
    ("1", TICKS_PER_QUARTER * 4),
    ("1/2", TICKS_PER_QUARTER * 2),
    ("1/4", TICKS_PER_QUARTER),
    ("1/8", TICKS_PER_QUARTER / 2),
    ("1/16", TICKS_PER_QUARTER / 4),
    ("1/32", TICKS_PER_QUARTER / 8),
];

/// The Score Editor's entry state (for the session, not saved).
#[derive(Debug, Clone)]
pub struct ScoreState {
    /// Index into [`VALUES`].
    pub value: usize,
    pub dotted: bool,
    pub triplet: bool,
    /// The accidental for the next note entered: `None` follows the key, `Some(0)` a natural,
    /// `Some(1)` a sharp, `Some(-1)` a flat. Cleared once a note is entered.
    pub accidental: Option<i8>,
    /// Where step entry (A–G, R) puts the next note: ticks from the clip's start.
    pub cursor: i64,
    /// The pitch step entry starts from (the last note entered or clicked).
    pub last_pitch: u8,
    /// The staff as last drawn (for hit tests, and for tests).
    pub drawn: Option<ScoreGeometry>,
}

impl Default for ScoreState {
    fn default() -> Self {
        ScoreState { value: 2, dotted: false, triplet: false, accidental: None, cursor: 0, last_pitch: 67, drawn: None }
    }
}

impl ScoreState {
    /// The chosen value's length in ticks (dotted: half again; triplet: two thirds).
    pub fn ticks(&self) -> i64 {
        let base = VALUES.get(self.value).map_or(TICKS_PER_QUARTER, |v| v.1);
        let t = if self.dotted { base * 3 / 2 } else { base };
        if self.triplet { (t * 2 / 3).max(1) } else { t }
    }
}

/// Where the staff was drawn: x of the clip's tick 0, y of middle C, the scale, the clip it
/// shows and the clip's first tick in the session (for the bar-counted grid).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreGeometry {
    pub clip: ClipId,
    pub staff: Rect,
    pub left: f32,
    pub c4_y: f32,
    pub px_per_tick: f32,
    pub base_tick: i64,
    pub len_ticks: i64,
}

impl ScoreGeometry {
    pub fn x_of(&self, tick: i64) -> f32 {
        self.left + tick as f32 * self.px_per_tick
    }
    /// y of a diatonic step above middle C.
    pub fn y_of(&self, step: i32) -> f32 {
        self.c4_y - step as f32 * STEP
    }
    /// The diatonic step at `y` (the nearest line or space).
    pub fn step_at(&self, y: f32) -> i32 {
        ((self.c4_y - y) / STEP).round() as i32
    }
    /// Ticks from the clip's start at `x` (unsnapped).
    pub fn tick_at(&self, x: f32) -> f64 {
        f64::from((x - self.left) / self.px_per_tick.max(1e-6))
    }
}

/// Diatonic step of a MIDI pitch (C4 = 60 → 0) and whether it needs a sharp.
#[cfg(test)]
fn diatonic(pitch: u8) -> (i32, bool) {
    let (step, acc) = spell(pitch, false);
    (step, acc != 0)
}

/// A pitch spelled on the staff: its diatonic step above middle C and its accidental (-1 flat,
/// 1 sharp), with flats for the black keys when `flats`.
pub fn spell(pitch: u8, flats: bool) -> (i32, i8) {
    let p = i32::from(pitch);
    let oct = p.div_euclid(12) - 5;
    let (step, acc) = match (p.rem_euclid(12), flats) {
        (0, _) => (0, 0),
        (1, false) => (0, 1),
        (1, true) => (1, -1),
        (2, _) => (1, 0),
        (3, false) => (1, 1),
        (3, true) => (2, -1),
        (4, _) => (2, 0),
        (5, _) => (3, 0),
        (6, false) => (3, 1),
        (6, true) => (4, -1),
        (7, _) => (4, 0),
        (8, false) => (4, 1),
        (8, true) => (5, -1),
        (9, _) => (5, 0),
        (10, false) => (5, 1),
        (10, true) => (6, -1),
        _ => (6, 0),
    };
    (oct * 7 + step, acc)
}

/// The natural (white-key) pitch of a diatonic step above middle C (clamped to the keyboard).
pub fn natural_pitch(step: i32) -> i32 {
    const PC: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
    60 + step.div_euclid(7) * 12 + PC[step.rem_euclid(7) as usize]
}

/// Whether a key's scale is spelled with flats: its tones need fewer clashing letters as flats
/// than as sharps (F major, C minor), else sharps (G major, A minor).
pub fn key_has_flats(set: soundcraft_vocal::PitchClassSet) -> bool {
    let clashes = |flats: bool| {
        (0..12u8)
            .filter(|pc| set.contains(*pc) && matches!(pc, 1 | 3 | 6 | 8 | 10))
            .filter(|pc| {
                let neighbour = if flats { (pc + 1) % 12 } else { (pc + 11) % 12 };
                set.contains(neighbour)
            })
            .count()
    };
    clashes(true) < clashes(false)
}

/// The pitch a click on diatonic `step` enters: its natural note, moved by `accidental` when one
/// is chosen, else to the key's tone next to it when the natural is not in the key (`set`).
pub fn entry_pitch(step: i32, accidental: Option<i8>, key: Option<soundcraft_vocal::PitchClassSet>) -> u8 {
    let nat = natural_pitch(step);
    let p = match (accidental, key) {
        (Some(a), _) => nat + i32::from(a),
        (None, Some(set)) if !set.contains(nat.rem_euclid(12) as u8) => {
            let up = set.contains((nat + 1).rem_euclid(12) as u8);
            let down = set.contains((nat - 1).rem_euclid(12) as u8);
            match (up, down) {
                (true, true) if key_has_flats(set) => nat - 1,
                (true, _) => nat + 1,
                (_, true) => nat - 1,
                _ => nat,
            }
        }
        _ => nat,
    };
    p.clamp(0, 127) as u8
}

/// The pitch a diatonic move by `steps` from `pitch` gives: the scale's next tone on the staff
/// (in the key, or the natural notes without one).
pub fn step_pitch(pitch: u8, steps: i32, key: Option<soundcraft_vocal::PitchClassSet>) -> u8 {
    let flats = key.is_some_and(key_has_flats);
    let (step, _) = spell(pitch, flats);
    entry_pitch(step + steps, None, key)
}

/// The pitch with letter `letter` (0 = C … 6 = B) nearest `last`, in the key (step entry).
pub fn letter_pitch(letter: i32, last: u8, accidental: Option<i8>, key: Option<soundcraft_vocal::PitchClassSet>) -> u8 {
    let (last_step, _) = spell(last, false);
    let oct = last_step.div_euclid(7);
    (oct - 1..=oct + 1).map(|o| entry_pitch(o * 7 + letter, accidental, key)).min_by_key(|p| (i32::from(*p) - i32::from(last)).abs()).unwrap_or(last)
}

/// The ledger lines a note `step` diatonic steps above middle C needs on the grand staff: above
/// the treble staff (A5 up), middle C's own line, below the bass staff (E2 down). The D above
/// middle C and the B below it sit in the spaces next to the staves and need none.
fn ledgers(step: i32) -> Vec<i32> {
    if step >= 12 {
        (12..=step).step_by(2).collect()
    } else if step <= -12 {
        (step..=-12).filter(|s| s % 2 == 0).collect()
    } else if step == 0 {
        vec![0]
    } else {
        Vec::new()
    }
}

/// Whether the Score Editor takes `key` before the Edit window's shortcuts: with the pointer
/// over its staff, the letters A–G and R, the value keys 1–6, Delete and Backspace, and the
/// arrows (Up and Down with Alt too). Never while a text field has the keyboard.
pub fn owns_key(app: &SoundApp, ctx: &egui::Context, key: Key, m: egui::Modifiers) -> bool {
    if !app.ui.show_score || m.command || m.ctrl || ctx.egui_wants_keyboard_input() {
        return false;
    }
    let ours = score_key(key, m).is_some();
    let staff = app.score.drawn.map(|g| g.staff);
    ours && staff.zip(ctx.pointer_latest_pos()).is_some_and(|(r, p)| r.contains(p))
}

/// What a key does over the staff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScoreKey {
    Letter(i32),
    Rest,
    Value(usize),
    Delete,
    /// Up (1) or down (-1); semitones with Alt.
    Move(i32, bool),
    Cursor(i32),
}

fn score_key(key: Key, m: egui::Modifiers) -> Option<ScoreKey> {
    let letter = |l: i32| (!m.alt).then_some(ScoreKey::Letter(l));
    Some(match key {
        Key::C => letter(0)?,
        Key::D => letter(1)?,
        Key::E => letter(2)?,
        Key::F => letter(3)?,
        Key::G => letter(4)?,
        Key::A => letter(5)?,
        Key::B => letter(6)?,
        Key::R if !m.alt => ScoreKey::Rest,
        Key::Num1 => ScoreKey::Value(0),
        Key::Num2 => ScoreKey::Value(1),
        Key::Num3 => ScoreKey::Value(2),
        Key::Num4 => ScoreKey::Value(3),
        Key::Num5 => ScoreKey::Value(4),
        Key::Num6 => ScoreKey::Value(5),
        Key::Delete | Key::Backspace => ScoreKey::Delete,
        Key::ArrowUp => ScoreKey::Move(1, m.alt),
        Key::ArrowDown => ScoreKey::Move(-1, m.alt),
        Key::ArrowLeft if !m.alt => ScoreKey::Cursor(-1),
        Key::ArrowRight if !m.alt => ScoreKey::Cursor(1),
        _ => return None,
    })
}

/// The key on the Key ruler at the clip's start (none without a key change on the ruler).
fn clip_key(app: &SoundApp, cid: ClipId) -> Option<soundcraft_vocal::PitchClassSet> {
    let s = app.engine.session();
    let clip = s.find_clip(cid)?.1;
    (!s.key_signatures.is_empty()).then(|| {
        let (k, sc) = soundcraft_engine::vocal::key_pc_at(s, clip.start);
        soundcraft_engine::vocal::key_set(k, sc)
    })
}

fn clip_notes(app: &SoundApp, cid: ClipId) -> Vec<soundcraft_midi::Note> {
    match app.engine.session().find_clip(cid).map(|(_, c)| &c.content) {
        Some(ClipContent::Midi { sequence }) => sequence.notes.clone(),
        _ => Vec::new(),
    }
}

/// Adds a note of the chosen value at `tick` (ticks from the clip's start) and selects it; the
/// cursor moves on past it.
pub fn enter_note(app: &mut SoundApp, cid: ClipId, tick: i64, pitch: u8) {
    let len = app.score.ticks();
    let p = json!({"clip": cid.0, "pitch": pitch, "start_ticks": tick.max(0), "length_ticks": len, "velocity": app.midi.last_velocity});
    if let Ok(r) = app.run("midi.note_add", p) {
        if let Some(i) = r.get("index").and_then(serde_json::Value::as_u64) {
            app.midi.clip = Some(cid);
            app.midi.selected = vec![i as usize];
        }
        app.score.cursor = tick.max(0).saturating_add(len);
        app.score.last_pitch = pitch;
        app.score.accidental = None;
        crate::midi_editor::audition(
            app,
            cid,
            &clip_notes(app, cid).into_iter().filter(|n| n.start == tick && n.pitch == pitch).take(1).collect::<Vec<_>>(),
        );
    }
}

/// The selected notes of `cid` (the piano roll's selection while it is on that clip).
fn selected(app: &SoundApp, cid: ClipId) -> Vec<usize> {
    if app.midi.clip == Some(cid) { app.midi.selected.clone() } else { Vec::new() }
}

/// Moves the selected notes a scale step (or a semitone with `semis`) up or down: one undo step.
fn move_selected(app: &mut SoundApp, cid: ClipId, dir: i32, semis: bool) {
    let sel = selected(app, cid);
    if sel.is_empty() {
        return;
    }
    if semis {
        if let Ok(r) = app.run("midi.notes_move", json!({"clip": cid.0, "indices": sel, "semitones": dir}))
            && let Some(ix) = r.get("indices").and_then(|v| serde_json::from_value::<Vec<usize>>(v.clone()).ok())
        {
            app.midi.selected = ix;
        }
        return;
    }
    let key = clip_key(app, cid);
    let picked: Vec<soundcraft_midi::Note> = {
        let all = clip_notes(app, cid);
        sel.iter().filter_map(|i| all.get(*i).copied()).collect()
    };
    let mut moved = Vec::with_capacity(picked.len());
    for (k, n) in picked.iter().enumerate() {
        let to = step_pitch(n.pitch, dir, key);
        // Each edit re-sorts the notes, so each note is found again by what it is.
        let Some(i) = clip_notes(app, cid).iter().position(|x| x == n) else { continue };
        let p = json!({"clip": cid.0, "index": i, "pitch": to});
        let r = if k == 0 { app.run("midi.note_edit", p) } else { app.engine.execute_joined("midi.note_edit", &p).map_err(|e| e.to_string()) };
        if r.is_ok() {
            moved.push(soundcraft_midi::Note { pitch: to, ..*n });
        }
    }
    let all = clip_notes(app, cid);
    app.midi.selected = moved.iter().filter_map(|m| all.iter().position(|x| x == m)).collect();
    if let Some(m) = moved.last() {
        app.score.last_pitch = m.pitch;
        crate::midi_editor::audition(app, cid, &moved);
    }
}

fn key_actions(app: &mut SoundApp, ctx: &egui::Context, cid: ClipId) {
    let keys: Vec<(Key, egui::Modifiers)> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                _ => None,
            })
            .collect()
    });
    for (key, m) in keys {
        if !owns_key(app, ctx, key, m) {
            continue;
        }
        let Some(k) = score_key(key, m) else { continue };
        match k {
            ScoreKey::Letter(l) => {
                let pitch = letter_pitch(l, app.score.last_pitch, app.score.accidental, clip_key(app, cid));
                let at = app.score.cursor;
                enter_note(app, cid, at, pitch);
            }
            ScoreKey::Rest => app.score.cursor = app.score.cursor.saturating_add(app.score.ticks()),
            ScoreKey::Cursor(d) => app.score.cursor = app.score.cursor.saturating_add(app.score.ticks() * i64::from(d)).max(0),
            ScoreKey::Value(v) => {
                app.score.value = v;
                let sel = selected(app, cid);
                if !sel.is_empty() {
                    let _ = app.run("midi.notes_set", json!({"clip": cid.0, "indices": sel, "length": app.score.ticks()}));
                }
            }
            ScoreKey::Delete => {
                let sel = selected(app, cid);
                if !sel.is_empty() && app.run("midi.note_delete", json!({"clip": cid.0, "indices": sel})).is_ok() {
                    app.midi.selected.clear();
                }
            }
            ScoreKey::Move(dir, semis) => move_selected(app, cid, dir, semis),
        }
    }
}

fn value_row(app: &mut SoundApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label("Value");
        for (i, (name, _)) in VALUES.iter().enumerate() {
            if ui.selectable_label(app.score.value == i, *name).on_hover_text(format!("Key {}", i + 1)).clicked() {
                app.score.value = i;
            }
        }
        ui.checkbox(&mut app.score.dotted, "Dotted");
        ui.checkbox(&mut app.score.triplet, "Triplet");
        ui.separator();
        for (sym, a) in [("♯", 1i8), ("♭", -1), ("♮", 0)] {
            let on = app.score.accidental == Some(a);
            if ui.selectable_label(on, sym).on_hover_text("For the next note entered").clicked() {
                app.score.accidental = if on { None } else { Some(a) };
            }
        }
    });
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_score;
    if !open {
        app.score.drawn = None;
        return;
    }
    egui::Window::new("Score Editor").open(&mut open).default_size(vec2(900.0, 380.0)).show(ctx, |ui| {
        let Some(cid) = crate::midi_editor::target_clip(app) else {
            app.score.drawn = None;
            ui.label("Select a MIDI clip or MIDI track.");
            return;
        };
        let s = app.engine.session();
        let Some((tid, clip)) = s.find_clip(cid).map(|(t, c)| (t, c.clone())) else { return };
        if !matches!(clip.content, ClipContent::Midi { .. }) {
            return;
        }
        let title = s.track(tid).map_or(String::new(), |t| t.name.clone());
        ui.label(egui::RichText::new(format!("{title} — {}", clip.name)).font(bold(13.0)));
        value_row(app, ui);
        key_actions(app, ctx, cid);
        staff(app, ui, cid);
        ui.label(
            egui::RichText::new(
                "Click on the staff to add a note of the chosen value, on a head to select it. Over the staff: A–G enter at the cursor, R a rest, 1–6 the value, ↑↓ a scale step (Alt: a semitone), Delete removes.",
            )
            .small(),
        );
    });
    app.ui.show_score = open;
}

fn staff(app: &mut SoundApp, ui: &mut egui::Ui, cid: ClipId) {
    let key = clip_key(app, cid);
    let flats = key.is_some_and(key_has_flats);
    let s = app.engine.session();
    let Some(clip) = s.find_clip(cid).map(|(_, c)| c.clone()) else { return };
    let ClipContent::Midi { sequence } = &clip.content else { return };
    let base = s.tempo.samples_to_ticks(clip.start, s.sample_rate);
    let len_ticks = (s.tempo.samples_to_ticks(clip.end(), s.sample_rate) - base).max(1);
    let px_per_tick = 0.07f32;
    let width = (len_ticks as f32 * px_per_tick + 120.0).max(ui.available_width());
    let sel = selected(app, cid);
    let cursor = app.score.cursor;
    let mut clicked: Option<(Pos2, bool)> = None;
    let mut geo: Option<ScoreGeometry> = None;
    let mut heads: Vec<(usize, Pos2)> = Vec::new();
    egui::ScrollArea::horizontal().show(ui, |ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(width, 230.0), Sense::click());
        let p = ui.painter_at(r);
        p.rect_filled(r, 4.0, Color32::from_rgb(246, 243, 236));
        let ink = Color32::from_rgb(20, 20, 24);
        let accent = Color32::from_rgb(40, 110, 220);
        let g = ScoreGeometry { clip: cid, staff: r, left: r.min.x + 70.0, c4_y: r.min.y + 115.0, px_per_tick, base_tick: base, len_ticks };
        geo = Some(g);
        let y_of = |step: i32| g.y_of(step);
        // Treble staff: E4..F5 (steps 2..10 above C4); bass: G2..A3 (steps -10..-2).
        for line in [2, 4, 6, 8, 10, -2, -4, -6, -8, -10] {
            let y = y_of(line);
            p.line_segment([pos2(r.min.x + 10.0, y), pos2(r.max.x - 10.0, y)], Stroke::new(1.0, ink));
        }
        p.text(pos2(r.min.x + 22.0, y_of(6)), Align2::CENTER_CENTER, "𝄞", regular(36.0), ink);
        // Bass clef, drawn: a curl from the F line plus two dots around it.
        let f = pos2(r.min.x + 18.0, y_of(-4));
        let curl: Vec<Pos2> = (0..=20)
            .map(|i| {
                let a = -2.6 + i as f32 * 0.19;
                let rad = 7.0 + i as f32 * 0.35;
                pos2(f.x + a.cos() * rad * 0.9 + 2.0, f.y + a.sin() * rad + 4.0)
            })
            .collect();
        p.circle_filled(pos2(f.x - 4.0, f.y), 3.0, ink);
        p.add(Shape::line(curl, Stroke::new(2.2, ink)));
        p.circle_filled(pos2(f.x + 14.0, y_of(-3)), 1.8, ink);
        p.circle_filled(pos2(f.x + 14.0, y_of(-5)), 1.8, ink);
        p.line_segment([pos2(r.min.x + 10.0, y_of(10)), pos2(r.min.x + 10.0, y_of(-10))], Stroke::new(2.0, ink));
        // Bar lines.
        let mut bar_tick = 0i64;
        let mut guard = 0;
        while bar_tick <= len_ticks && guard < 2000 {
            let x = g.x_of(bar_tick);
            p.line_segment([pos2(x - 8.0, y_of(10)), pos2(x - 8.0, y_of(-10))], Stroke::new(1.0, ink));
            let m = s.tempo.meter_at_tick(base + bar_tick);
            let bb = s.tempo.bar_beat_at_tick(base + bar_tick);
            p.text(pos2(x - 6.0, y_of(12)), Align2::LEFT_BOTTOM, bb.bar.to_string(), regular(9.0), Color32::from_rgb(120, 110, 100));
            bar_tick += m.ticks_per_bar().max(1);
            guard += 1;
        }
        // The step-entry cursor.
        if (0..=len_ticks).contains(&cursor) {
            let x = g.x_of(cursor);
            p.line_segment([pos2(x, y_of(12)), pos2(x, y_of(-12))], Stroke::new(1.0, accent.gamma_multiply(0.6)));
        }
        // Notes.
        for (i, n) in sequence.notes.iter().enumerate().filter(|(_, n)| n.start >= 0 && n.start < len_ticks) {
            let x = g.x_of(n.start);
            let (step, acc) = spell(n.pitch, flats);
            let y = y_of(step);
            heads.push((i, pos2(x, y)));
            let col = if sel.contains(&i) { accent } else { ink };
            for l in ledgers(step) {
                p.line_segment([pos2(x - 7.0, y_of(l)), pos2(x + 7.0, y_of(l))], Stroke::new(1.0, ink));
            }
            head(&p, pos2(x, y), n.length, step, col);
            match acc {
                1 => {
                    p.text(pos2(x - 12.0, y), Align2::CENTER_CENTER, "♯", regular(12.0), col);
                }
                -1 => {
                    p.text(pos2(x - 12.0, y), Align2::CENTER_CENTER, "♭", regular(12.0), col);
                }
                _ => {}
            }
        }
        // The ghost head where a click would add a note.
        if let Some(hp) = resp.hover_pos() {
            let tick = snap_tick(s, &g, hp.x, app.score.ticks());
            let step = g.step_at(hp.y);
            let ghost = Color32::from_rgba_unmultiplied(40, 110, 220, 110);
            for l in ledgers(step) {
                p.line_segment([pos2(g.x_of(tick) - 7.0, y_of(l)), pos2(g.x_of(tick) + 7.0, y_of(l))], Stroke::new(1.0, ghost));
            }
            p.add(Shape::ellipse_filled(pos2(g.x_of(tick), y_of(step)), vec2(5.0, 3.75), ghost));
        }
        if resp.clicked()
            && let Some(cp) = resp.interact_pointer_pos()
        {
            clicked = Some((cp, ui.input(|i| i.modifiers.shift)));
        }
    });
    app.score.drawn = geo;
    let (Some((cp, shift)), Some(g)) = (clicked, geo) else { return };
    // A click on a head selects it; on empty staff it adds a note there.
    if let Some((i, _)) = heads
        .iter()
        .filter(|(_, hp)| (hp.x - cp.x).abs() <= 7.0 && (hp.y - cp.y).abs() <= STEP)
        .min_by(|a, b| a.1.distance(cp).total_cmp(&b.1.distance(cp)))
    {
        app.midi.clip = Some(cid);
        if shift {
            if !app.midi.selected.contains(i) {
                app.midi.selected.push(*i);
            }
        } else {
            app.midi.selected = vec![*i];
        }
        if let Some(n) = clip_notes(app, cid).get(*i).copied() {
            app.score.last_pitch = n.pitch;
            app.score.cursor = n.end();
            crate::midi_editor::audition(app, cid, &[n]);
        }
        return;
    }
    let tick = snap_tick(app.engine.session(), &g, cp.x, app.score.ticks());
    let pitch = entry_pitch(g.step_at(cp.y), app.score.accidental, clip_key(app, cid));
    enter_note(app, cid, tick, pitch);
}

/// The tick (from the clip's start) a click at `x` lands on: the nearest line of the value's
/// grid counted from the bar, inside the clip.
fn snap_tick(s: &soundcraft_model::Session, g: &ScoreGeometry, x: f32, value: i64) -> i64 {
    let abs = crate::midi_edit_tools::snap_abs(&s.tempo, g.base_tick as f64 + g.tick_at(x), value);
    abs.saturating_sub(g.base_tick).clamp(0, (g.len_ticks - 1).max(0))
}

/// A note head with its stem and flags, by length.
fn head(p: &egui::Painter, at: Pos2, length: i64, step: i32, ink: Color32) {
    let (x, y) = (at.x, at.y);
    let q = length as f32 / TICKS_PER_QUARTER as f32;
    let hollow = q >= 1.75;
    let r = Rect::from_center_size(pos2(x, y), vec2(10.0, 7.5));
    if hollow {
        p.add(Shape::ellipse_stroke(r.center(), r.size() * 0.5, Stroke::new(1.6, ink)));
    } else {
        p.add(Shape::ellipse_filled(r.center(), r.size() * 0.5, ink));
    }
    if q < 3.5 {
        // Stems up below the middle line of each staff, down above it.
        let up = if step >= 0 { step < 6 } else { step < -6 };
        let (sx, sy0, sy1): (f32, f32, f32) = if up { (x + 4.5, y, y - 26.0) } else { (x - 4.5, y, y + 26.0) };
        p.line_segment([pos2(sx, sy0), pos2(sx, sy1)], Stroke::new(1.2, ink));
        let flags = if q <= 0.3 {
            2
        } else if q <= 0.6 {
            1
        } else {
            0
        };
        for f in 0..flags {
            let fy = sy1 + if up { f as f32 * 6.0 } else { -(f as f32) * 6.0 };
            let tip: Pos2 = pos2(sx + 8.0, fy + if up { 10.0 } else { -10.0 });
            p.line_segment([pos2(sx, fy), tip], Stroke::new(1.6, ink));
        }
    }
}

#[cfg(test)]
mod tests {
    use soundcraft_vocal::PitchClassSet;

    #[test]
    fn diatonic_steps() {
        assert_eq!(super::diatonic(60), (0, false));
        assert_eq!(super::diatonic(61), (0, true));
        assert_eq!(super::diatonic(72), (7, false));
        assert_eq!(super::diatonic(48), (-7, false));
        assert_eq!(super::diatonic(64), (2, false));
    }

    /// Bug 22: middle C has its ledger line; the D above it (step 1) and the B below (-1) none.
    #[test]
    fn only_middle_c_takes_the_ledger_between_the_staves() {
        assert_eq!(super::ledgers(0), vec![0]);
        assert!(super::ledgers(1).is_empty());
        assert!(super::ledgers(-1).is_empty());
        assert_eq!(super::ledgers(13), vec![12]);
        assert_eq!(super::ledgers(-14), vec![-14, -12]);
    }

    fn set(pcs: &[u8]) -> PitchClassSet {
        PitchClassSet::from_pcs(pcs)
    }

    #[test]
    fn keys_spell_with_flats_or_sharps() {
        let g_major = set(&[7, 9, 11, 0, 2, 4, 6]);
        let f_major = set(&[5, 7, 9, 10, 0, 2, 4]);
        let c_minor = set(&[0, 2, 3, 5, 7, 8, 10, 11]);
        let a_minor = set(&[9, 11, 0, 2, 4, 5, 7, 8]);
        assert!(!super::key_has_flats(g_major));
        assert!(super::key_has_flats(f_major));
        assert!(super::key_has_flats(c_minor));
        assert!(!super::key_has_flats(a_minor));
        // An F in G major is F#; a B in F major is Bb; with an accidental chosen, that.
        assert_eq!(super::entry_pitch(3, None, Some(g_major)), 66);
        assert_eq!(super::entry_pitch(6, None, Some(f_major)), 70);
        assert_eq!(super::entry_pitch(3, Some(0), Some(g_major)), 65);
        assert_eq!(super::entry_pitch(4, Some(1), None), 68);
        // Bb is drawn as a B with a flat in a flat key, as an A sharp otherwise.
        assert_eq!(super::spell(70, true), (6, -1));
        assert_eq!(super::spell(70, false), (5, 1));
        // A scale step from E in G major goes to F#, from F# to G.
        assert_eq!(super::step_pitch(64, 1, Some(g_major)), 66);
        assert_eq!(super::step_pitch(66, 1, Some(g_major)), 67);
        assert_eq!(super::step_pitch(60, -1, None), 59);
        // Step entry: the letter nearest the last note.
        assert_eq!(super::letter_pitch(0, 67, None, None), 72);
        assert_eq!(super::letter_pitch(3, 67, None, Some(g_major)), 66);
        assert_eq!(super::letter_pitch(1, 60, None, None), 62);
    }
}
