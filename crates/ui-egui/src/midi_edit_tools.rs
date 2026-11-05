//! The MIDI Editor's navigating and editing tools (docs/superpowers/specs/
//! 2026-11-04-midi-editor-editing-design.md): its own Grid menu, the ¼×–4× zoom factors, bar and
//! beat lines that follow the meter, the note bar (pitch, start, length, velocity and channel of
//! the selection), the hover details, the playable piano keys, Space, Select all / Split / Join,
//! and `ui.midi_editor`, the command that drives and reports all of it.
//!
//! [`crate::midi_editor`] draws the roll and calls in here; this module keeps its own state in
//! the editor's [`crate::midi_editor::MidiEditorState`].

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_midi::Note;
use soundcraft_model::{ClipContent, ClipId, Session};
use soundcraft_time::{BarBeat, GridValue, NoteValue, TICKS_PER_QUARTER, TempoMap, TimeFormat};

/// The note bar's height, along the editor's bottom.
pub const NOTEBAR_H: f32 = 24.0;
/// The roll's zoom factors, relative to the Edit window's zoom (as the Vocal Editor's).
pub const ZOOM_FACTORS: [f32; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];
/// A 16th note in ticks: the third part of a "34.1.1" position.
const SIXTEENTH: i64 = TICKS_PER_QUARTER / 4;
/// The most lines one frame draws across the roll.
const MAX_LINES: usize = 20_000;
/// How long a held piano key sounds at most, in seconds.
pub const KEY_HOLD_S: f64 = 8.0;

/// The editor's own grid: the Edit window's, a note value (straight or triplet), or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MidiGrid {
    /// Follows the Edit window's grid.
    #[default]
    Edit,
    Note {
        value: NoteValue,
        triplet: bool,
    },
    /// No snapping: drags move by the tick.
    Off,
}

/// "1/1" … "1/64".
fn value_name(v: NoteValue) -> &'static str {
    match v {
        NoteValue::Bar => "1/1",
        NoteValue::Half => "1/2",
        NoteValue::Quarter => "1/4",
        NoteValue::Eighth => "1/8",
        NoteValue::Sixteenth => "1/16",
        NoteValue::ThirtySecond => "1/32",
        NoteValue::SixtyFourth => "1/64",
    }
}

/// The Edit window's grid in ticks (a 16th when it isn't a note value).
pub fn edit_grid_ticks(g: GridValue) -> i64 {
    match g {
        GridValue::Note { value, dotted, triplet } => {
            let mut t = value.ticks();
            if dotted {
                t = t * 3 / 2;
            }
            if triplet {
                t = t * 2 / 3;
            }
            t.max(1)
        }
        _ => SIXTEENTH,
    }
}

impl MidiGrid {
    /// The snap step in ticks: 1 (no snapping) when off.
    pub fn ticks(self, edit: GridValue) -> i64 {
        match self {
            MidiGrid::Edit => edit_grid_ticks(edit),
            MidiGrid::Note { value, triplet } => (if triplet { value.ticks() * 2 / 3 } else { value.ticks() }).max(1),
            MidiGrid::Off => 1,
        }
    }

    /// The step arrow nudges, the Pencil's first length, Duplicate and Quantize use: the grid,
    /// or a 16th when it is off.
    pub fn step(self, edit: GridValue) -> i64 {
        match self {
            MidiGrid::Off => SIXTEENTH,
            g => g.ticks(edit),
        }
    }

    /// "1/16", "1/8T", "Off", or the Edit window's ("Edit 1/8").
    pub fn label(self, edit: GridValue) -> String {
        match self {
            MidiGrid::Edit => match edit {
                GridValue::Note { value, triplet, dotted } => {
                    format!("Edit {}{}{}", value_name(value), if triplet { "T" } else { "" }, if dotted { "." } else { "" })
                }
                _ => "Edit 1/16".into(),
            },
            MidiGrid::Note { value, triplet } => format!("{}{}", value_name(value), if triplet { "T" } else { "" }),
            MidiGrid::Off => "Off".into(),
        }
    }

    /// The id `ui.midi_editor {grid}` takes and reports: "edit", "off", "1/16", "1/8t".
    pub fn id(self) -> String {
        match self {
            MidiGrid::Edit => "edit".into(),
            MidiGrid::Off => "off".into(),
            MidiGrid::Note { value, triplet } => format!("{}{}", value_name(value), if triplet { "t" } else { "" }),
        }
    }

    pub fn parse(s: &str) -> Option<MidiGrid> {
        let s = s.trim().to_ascii_lowercase();
        match s.as_str() {
            "edit" => return Some(MidiGrid::Edit),
            "off" | "none" => return Some(MidiGrid::Off),
            _ => {}
        }
        let (body, triplet) = match s.strip_suffix('t') {
            Some(b) => (b, true),
            None => (s.as_str(), false),
        };
        NoteValue::ALL.into_iter().find(|v| value_name(*v) == body).map(|value| MidiGrid::Note { value, triplet })
    }
}

/// A line across the roll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Bar,
    Beat,
    Grid,
}

/// The lines between `from` and `to` (ticks from the clip's start, which is session tick
/// `base`): each bar's first beat and its beats as the meter in force there sets them (with
/// `beats`), and every `grid` ticks from each bar's start (0: none). Sorted; at most
/// [`MAX_LINES`]. Tempo changes don't move them: the roll is in ticks.
pub fn grid_lines(map: &TempoMap, base: i64, from: i64, to: i64, beats: bool, grid: i64) -> Vec<(i64, LineKind)> {
    let (a, b) = (base.saturating_add(from), base.saturating_add(to));
    let mut out: Vec<(i64, LineKind)> = Vec::new();
    let mut bar = map.bar_beat_at_tick(a).bar;
    while out.len() < MAX_LINES {
        let start = map.bar_start_tick(bar);
        if start > b {
            break;
        }
        let next = map.bar_start_tick(bar.saturating_add(1));
        if next <= start {
            break;
        }
        let beat = map.meter_at_tick(start).ticks_per_beat();
        let mut ticks: Vec<(i64, LineKind)> = vec![(start, LineKind::Bar)];
        if beats {
            ticks.extend((1..).map(|k| start.saturating_add(beat.saturating_mul(k))).take_while(|t| *t < next).map(|t| (t, LineKind::Beat)));
        }
        if grid > 0 {
            for t in (1..).map(|k| start.saturating_add(grid.saturating_mul(k))).take_while(|t| *t < next).take(MAX_LINES) {
                if !(beats && (t - start) % beat == 0) {
                    ticks.push((t, LineKind::Grid));
                }
            }
        }
        out.extend(ticks.into_iter().filter(|(t, _)| (a..=b).contains(t)).map(|(t, k)| (t - base, k)));
        bar = bar.saturating_add(1);
    }
    out.sort_by_key(|(t, _)| *t);
    out.truncate(MAX_LINES);
    out
}

/// `tick` (session ticks) on the grid: the nearest line `g` ticks apart counted from the bar
/// start (0 when `g` is 1).
pub fn snap_abs(map: &TempoMap, tick: f64, g: i64) -> i64 {
    let t = tick.round() as i64;
    if g <= 1 {
        return t;
    }
    let bar = map.bar_start_tick(map.bar_beat_at_tick(t).bar);
    let off = t.saturating_sub(bar);
    bar.saturating_add(((off as f64 / g as f64).round() as i64).saturating_mul(g))
}

/// "34.1.1": bar, beat and 16th (from 1), with "+ticks" when between 16ths.
pub fn bbt(map: &TempoMap, tick: i64) -> String {
    let b = map.bar_beat_at_tick(tick);
    let (six, rest) = (b.tick / SIXTEENTH + 1, b.tick % SIXTEENTH);
    if rest == 0 { format!("{}.{}.{six}", b.bar, b.beat) } else { format!("{}.{}.{six}+{rest}", b.bar, b.beat) }
}

/// Splits "a.b.c+t" (also with `|` or spaces) into its numbers; parts left out are `None`.
fn parts(text: &str) -> Option<(Vec<i64>, i64)> {
    let (body, extra) = match text.split_once('+') {
        Some((b, e)) => (b, e.trim().parse::<i64>().ok()?),
        None => (text, 0),
    };
    let nums: Vec<i64> =
        body.split(['.', '|', ' ', ':']).filter(|p| !p.is_empty()).map(|p| p.trim().parse::<i64>()).collect::<Result<_, _>>().ok()?;
    (!nums.is_empty() && nums.len() <= 3).then_some((nums, extra))
}

/// The session tick a "34.1.1" position names.
pub fn parse_bbt(map: &TempoMap, text: &str) -> Option<i64> {
    let (n, extra) = parts(text)?;
    let bar = *n.first()?;
    let beat = n.get(1).copied().unwrap_or(1).max(1);
    let six = n.get(2).copied().unwrap_or(1).max(1);
    let tick = (six - 1).saturating_mul(SIXTEENTH).saturating_add(extra.max(0));
    Some(map.tick_at_bar_beat(BarBeat { bar, beat, tick }))
}

/// "1.0.0": a length in bars, beats and 16ths of the meter at `start` (session ticks).
pub fn bbt_len(map: &TempoMap, start: i64, len: i64) -> String {
    let m = map.meter_at_tick(start);
    let (bar, beat) = (m.ticks_per_bar().max(1), m.ticks_per_beat().max(1));
    let len = len.max(0);
    let (bars, rem) = (len / bar, len % bar);
    let (beats, rem) = (rem / beat, rem % beat);
    let (six, rest) = (rem / SIXTEENTH, rem % SIXTEENTH);
    if rest == 0 { format!("{bars}.{beats}.{six}") } else { format!("{bars}.{beats}.{six}+{rest}") }
}

/// The ticks a "1.0.0" length names at `start`.
pub fn parse_bbt_len(map: &TempoMap, start: i64, text: &str) -> Option<i64> {
    let (n, extra) = parts(text)?;
    let m = map.meter_at_tick(start);
    let get = |i: usize| n.get(i).copied().unwrap_or(0).max(0);
    Some(
        get(0)
            .saturating_mul(m.ticks_per_bar())
            .saturating_add(get(1).saturating_mul(m.ticks_per_beat()))
            .saturating_add(get(2).saturating_mul(SIXTEENTH))
            .saturating_add(extra.max(0)),
    )
}

/// A position (session ticks) as the main counter shows it; Bars|Beats as "34.1.1".
pub fn position_text(s: &Session, tick: i64) -> String {
    match s.edit.main_counter {
        TimeFormat::BarsBeats => bbt(&s.tempo, tick),
        f => s.format_position(s.tempo.tick_to_samples(tick, s.sample_rate), f).trim().to_string(),
    }
}

/// The session tick a position typed in the main counter's format (or "34.1.1") names.
pub fn parse_position_text(s: &Session, text: &str) -> Option<i64> {
    match s.edit.main_counter {
        TimeFormat::BarsBeats => parse_bbt(&s.tempo, text),
        f => s.parse_position(text, f).ok().map(|at| s.tempo.samples_to_ticks(at, s.sample_rate)).or_else(|| parse_bbt(&s.tempo, text)),
    }
}

/// A pitch typed as a name ("F#3", "Gb3", "C-1"; middle C is C3) or a number (0-127).
pub fn parse_pitch(text: &str) -> Option<i64> {
    let t = text.trim();
    if let Ok(n) = t.parse::<i64>() {
        return (0..=127).contains(&n).then_some(n);
    }
    let mut chars = t.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    let class: i64 = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest: String = chars.collect();
    let (accidental, octave) = if let Some(r) = rest.strip_prefix('#') {
        (1, r)
    } else if let Some(r) = rest.strip_prefix('b') {
        (-1, r)
    } else {
        (0, rest.as_str())
    };
    let octave: i64 = octave.trim().parse().ok()?;
    let p = (octave + 2).saturating_mul(12).saturating_add(class).saturating_add(accidental);
    (0..=127).contains(&p).then_some(p)
}

/// "F4 · 34.1.1 · len 1.0.0 · vel 96 · ch 1": a note of the clip starting at session tick `base`.
pub fn note_details(s: &Session, base: i64, n: &Note) -> String {
    let start = base.saturating_add(n.start);
    format!(
        "{} · {} · len {} · vel {} · ch {}",
        soundcraft_midi::note_name(n.pitch),
        position_text(s, start),
        bbt_len(&s.tempo, start, n.length),
        n.velocity,
        u16::from(n.channel) + 1
    )
}

/// A field of the note bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Pitch,
    Start,
    Length,
    Velocity,
    Channel,
}

impl Field {
    pub const ALL: [Field; 5] = [Field::Pitch, Field::Start, Field::Length, Field::Velocity, Field::Channel];

    pub fn id(self) -> &'static str {
        match self {
            Field::Pitch => "pitch",
            Field::Start => "start",
            Field::Length => "length",
            Field::Velocity => "velocity",
            Field::Channel => "channel",
        }
    }

    pub fn from_id(id: &str) -> Option<Field> {
        Field::ALL.into_iter().find(|f| f.id() == id)
    }

    fn label(self) -> &'static str {
        match self {
            Field::Pitch => "PITCH",
            Field::Start => "START",
            Field::Length => "LENGTH",
            Field::Velocity => "VELOCITY",
            Field::Channel => "CHANNEL",
        }
    }
}

/// The selected notes' lowest and highest value of each field (start in ticks from the clip
/// start, channel from 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub count: usize,
    pub pitch: (i64, i64),
    pub start: (i64, i64),
    pub length: (i64, i64),
    pub velocity: (i64, i64),
    pub channel: (i64, i64),
}

impl Summary {
    pub fn of(notes: &[Note], selected: &[usize]) -> Option<Summary> {
        let picked: Vec<&Note> = selected.iter().filter_map(|i| notes.get(*i)).collect();
        let span = |f: &dyn Fn(&Note) -> i64| {
            let lo = picked.iter().map(|n| f(n)).min().unwrap_or(0);
            let hi = picked.iter().map(|n| f(n)).max().unwrap_or(0);
            (lo, hi)
        };
        (!picked.is_empty()).then(|| Summary {
            count: picked.len(),
            pitch: span(&|n| i64::from(n.pitch)),
            start: span(&|n| n.start),
            length: span(&|n| n.length),
            velocity: span(&|n| i64::from(n.velocity)),
            channel: span(&|n| i64::from(n.channel) + 1),
        })
    }

    pub fn get(&self, f: Field) -> (i64, i64) {
        match f {
            Field::Pitch => self.pitch,
            Field::Start => self.start,
            Field::Length => self.length,
            Field::Velocity => self.velocity,
            Field::Channel => self.channel,
        }
    }
}

/// One value of `f` as the note bar shows it (start and length from `start`, session ticks).
fn value_text(s: &Session, base: i64, f: Field, v: i64, start: i64) -> String {
    match f {
        Field::Pitch => soundcraft_midi::note_name(v.clamp(0, 127) as u8),
        Field::Start => position_text(s, base.saturating_add(v)),
        Field::Length => bbt_len(&s.tempo, base.saturating_add(start), v),
        Field::Velocity | Field::Channel => v.to_string(),
    }
}

/// What the note bar shows for `f`: the value, or "lo…hi" when the selection's differ.
pub fn field_text(s: &Session, base: i64, f: Field, sum: &Summary) -> String {
    let (lo, hi) = sum.get(f);
    let at = sum.start.0;
    if lo == hi { value_text(s, base, f, lo, at) } else { format!("{}\u{2026}{}", value_text(s, base, f, lo, at), value_text(s, base, f, hi, at)) }
}

/// The value typed text gives `f` (`None`: it doesn't read). Start is in ticks from the clip
/// start; a range as shown ("D4…A4") reads as its first value.
pub fn parse_field(s: &Session, base: i64, f: Field, at: i64, text: &str) -> Option<i64> {
    let text = text.split('\u{2026}').next().unwrap_or(text).trim();
    match f {
        Field::Pitch => parse_pitch(text),
        Field::Start => parse_position_text(s, text).map(|t| t.saturating_sub(base).max(0)),
        Field::Length => parse_bbt_len(&s.tempo, base.saturating_add(at), text).filter(|l| *l > 0),
        Field::Velocity => text.parse::<i64>().ok().filter(|v| (1..=127).contains(v)),
        Field::Channel => text.parse::<i64>().ok().filter(|v| (1..=16).contains(v)),
    }
}

/// Runs a note command, merged into the gesture `merge` when given; the result's `indices` (where
/// the notes sorted to) become the selection.
fn run_notes(app: &mut SoundApp, id: &str, params: Value, merge: Option<&str>) -> Result<(), String> {
    let r = match merge {
        Some(k) => app.engine.execute_merged(id, &params, k).map_err(|e| e.to_string()),
        None => app.run(id, params),
    };
    match r {
        Ok(v) => {
            if let Some(a) = v.get("indices").and_then(Value::as_array) {
                app.midi.selected = a.iter().filter_map(Value::as_u64).map(|v| v as usize).collect();
            }
            Ok(())
        }
        Err(e) => {
            app.ui.status = e.clone();
            Err(e)
        }
    }
}

/// Sets `f` on the selection to `v`: pitch and start move every note by the same amount (the
/// lowest lands on `v`, the others keep their distance); length, velocity and channel (from 1)
/// become `v` on every note. One undo step, or part of the gesture `merge`.
pub fn apply_value(app: &mut SoundApp, cid: ClipId, f: Field, v: i64, merge: Option<&str>) -> Result<(), String> {
    let notes = clip_notes(app.engine.session(), cid);
    let Some(sum) = Summary::of(&notes, &app.midi.selected) else { return Err("Select notes first".into()) };
    let sel = app.midi.selected.clone();
    let (id, params) = match f {
        Field::Pitch => ("midi.notes_move", json!({"clip": cid.0, "indices": sel, "semitones": v.clamp(0, 127) - sum.pitch.0})),
        Field::Start => ("midi.notes_move", json!({"clip": cid.0, "indices": sel, "ticks": v.max(0) - sum.start.0})),
        Field::Length => ("midi.notes_set", json!({"clip": cid.0, "indices": sel, "length": v.max(1)})),
        Field::Velocity => ("midi.notes_set", json!({"clip": cid.0, "indices": sel, "velocity": v.clamp(1, 127)})),
        Field::Channel => ("midi.notes_set", json!({"clip": cid.0, "indices": sel, "channel": v.clamp(1, 16) - 1})),
    };
    let unchanged = match f {
        Field::Pitch | Field::Start => sum.get(f).0 == v,
        _ => sum.get(f) == (v, v),
    };
    if unchanged {
        return Ok(());
    }
    if f == Field::Velocity {
        app.midi.last_velocity = v.clamp(1, 127) as u8;
    }
    run_notes(app, id, params, merge)
}

/// Sets `f` from typed text (as the note bar reads it).
pub fn apply_text(app: &mut SoundApp, cid: ClipId, f: Field, text: &str) -> Result<(), String> {
    let s = app.engine.session_arc();
    let notes = clip_notes(&s, cid);
    let at = Summary::of(&notes, &app.midi.selected).map_or(0, |m| m.start.0);
    let base = clip_base(&s, cid);
    let v = parse_field(&s, base, f, at, text).ok_or_else(|| format!("Can't read `{text}` as a {}", f.id()))?;
    apply_value(app, cid, f, v, None)
}

/// The clip's notes (none for another kind of clip).
pub fn clip_notes(s: &Session, cid: ClipId) -> Vec<Note> {
    match s.find_clip(cid).map(|(_, c)| &c.content) {
        Some(ClipContent::Midi { sequence }) => sequence.notes.clone(),
        _ => Vec::new(),
    }
}

/// The session tick the clip starts at.
pub fn clip_base(s: &Session, cid: ClipId) -> i64 {
    s.find_clip(cid).map_or(0, |(_, c)| s.tempo.samples_to_ticks(c.start, s.sample_rate))
}

/// The note bar across `rect`: the selection's pitch, start, length, velocity and channel, each
/// typed or dragged (see [`apply_value`]); a drag is one undo step.
pub fn note_bar(app: &mut SoundApp, ui: &mut Ui, cid: ClipId, rect: Rect, alt: bool) {
    let t = Tokens::current();
    ui.painter().rect_filled(rect, 0.0, t.panel_bg2);
    ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, t.border));
    let s = app.engine.session_arc();
    let notes = clip_notes(&s, cid);
    let base = clip_base(&s, cid);
    let sum = Summary::of(&notes, &app.midi.selected);
    let hint = "Space plays selection · Alt = no snap · Esc cancels drag";
    ui.painter().text(pos2(rect.right() - 8.0, rect.center().y), Align2::RIGHT_CENTER, hint, regular(10.5), t.text_dim);
    let mut bar =
        ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(8.0, 2.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    bar.spacing_mut().item_spacing.x = 5.0;
    let Some(sum) = sum else {
        bar.label(egui::RichText::new("No notes selected").font(regular(11.0)).color(t.text_dim));
        return;
    };
    bar.label(egui::RichText::new(format!("{} note{}", sum.count, if sum.count == 1 { "" } else { "s" })).font(regular(11.0)).color(t.text_dim));
    let grid = app.midi.grid.ticks(s.edit.grid);
    let mut rects = Vec::new();
    for f in Field::ALL {
        bar.label(egui::RichText::new(format!("· {}", f.label())).font(regular(10.5)).color(t.text_dim));
        let (lo, hi) = sum.get(f);
        let mut v = lo as f64;
        let (s2, s3) = (s.clone(), s.clone());
        let at = sum.start.0;
        let (range, speed) = match f {
            Field::Pitch => (0.0..=127.0, 0.08),
            Field::Start => (0.0..=1e12, (grid.max(1) as f64 / 6.0).max(1.0)),
            Field::Length => (1.0..=1e12, (grid.max(1) as f64 / 6.0).max(1.0)),
            Field::Velocity => (1.0..=127.0, 0.4),
            Field::Channel => (1.0..=16.0, 0.05),
        };
        let r = bar.add(
            egui::DragValue::new(&mut v)
                .range(range)
                .speed(speed)
                .custom_formatter(move |x, _| {
                    let x = x.round() as i64;
                    if x == lo && hi != lo {
                        format!("{}\u{2026}{}", value_text(&s2, base, f, lo, at), value_text(&s2, base, f, hi, at))
                    } else {
                        value_text(&s2, base, f, x, at)
                    }
                })
                .custom_parser(move |text| parse_field(&s3, base, f, at, text).map(|x| x as f64)),
        );
        rects.push((f, r.rect));
        if r.drag_started() || r.gained_focus() {
            app.midi.gesture = app.midi.gesture.wrapping_add(1);
        }
        if r.changed() {
            let mut nv = v.round() as i64;
            // A dragged start or length lands on the grid (Alt: free).
            if r.dragged() && !alt && matches!(f, Field::Start | Field::Length) {
                nv = match f {
                    Field::Start => snap_abs(&s.tempo, (base + nv) as f64, grid).saturating_sub(base).max(0),
                    _ => (((nv as f64) / grid.max(1) as f64).round() as i64 * grid.max(1)).max(grid.max(1)),
                };
            }
            let key = format!("notebar:{}:{}:{}", f.id(), cid.0, app.midi.gesture);
            let _ = apply_value(app, cid, f, nv, Some(&key));
        }
    }
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("soundcraft-midi-notebar"), rects));
}

/// The details box by the pointer: a note's (or a ghost's) details.
pub fn hover_box(ui: &Ui, at: egui::Pos2, text: &str, bounds: Rect) {
    let t = Tokens::current();
    let galley = ui.painter().layout_no_wrap(text.to_string(), regular(11.0), t.text);
    let size = galley.size() + vec2(14.0, 8.0);
    let mut min = at + vec2(12.0, 14.0);
    if min.x + size.x > bounds.right() {
        min.x = at.x - size.x - 8.0;
    }
    if min.y + size.y > bounds.bottom() {
        min.y = at.y - size.y - 8.0;
    }
    let r = Rect::from_min_size(min, size);
    let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("soundcraft-midi-hover")));
    p.rect(r, 4.0, t.panel_bg2, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    p.galley(r.min + vec2(7.0, 4.0), galley, t.text);
}

/// Points per tick the Edit window shows at session tick `tick`: the 1× zoom factor.
pub fn edit_px_per_tick(s: &Session, tick: i64) -> f32 {
    let bpm = s.tempo.tempo_at_tick(tick).max(1.0);
    let samples_per_tick = s.sample_rate.as_f64() * 60.0 / (bpm * TICKS_PER_QUARTER as f64);
    (samples_per_tick / s.edit.zoom.samples_per_px.max(1e-9)) as f32
}

/// "¼×", "½×", "1×", "2×", "4×".
pub fn zoom_label(f: f32) -> String {
    crate::vocal_editor::zoom_label(f)
}

/// The factor whose zoom the view has (within 1 %), if any.
pub fn factor_of(zoom: Option<f32>, fit: f32, edit: f32) -> Option<f32> {
    let z = zoom.unwrap_or(fit);
    ZOOM_FACTORS.into_iter().find(|f| (z / (edit * f).max(1e-9) - 1.0).abs() < 0.01)
}

/// Space over the editor: stops when playing; else plays the selected notes (first start to last
/// end) and stops there; else plays from the playhead.
pub fn space(app: &mut SoundApp, cid: ClipId) {
    if app.is_playing() {
        let _ = app.run("transport.stop", json!({}));
        return;
    }
    let s = app.engine.session_arc();
    let notes = clip_notes(&s, cid);
    let base = clip_base(&s, cid);
    let picked: Vec<&Note> = app.midi.selected.iter().filter_map(|i| notes.get(*i)).collect();
    let span = picked.iter().map(|n| n.start).min().zip(picked.iter().map(|n| n.end()).max());
    match span {
        Some((a, b)) => {
            let from = s.tempo.tick_to_samples(base.saturating_add(a), s.sample_rate);
            let to = s.tempo.tick_to_samples(base.saturating_add(b), s.sample_rate);
            app.start_play(from, Some(to), None);
            app.ui.status = "Playing the selected notes".into();
        }
        None => {
            let _ = app.run("transport.play", json!({}));
        }
    }
}

/// Select all: every note of the clip (ghosts never).
pub fn select_all(app: &mut SoundApp, cid: ClipId) {
    app.midi.selected = (0..clip_notes(app.engine.session(), cid).len()).collect();
}

/// Split: the selected notes at `at` (ticks from the clip start), else at the playhead.
pub fn split(app: &mut SoundApp, cid: ClipId, at: Option<i64>) -> Result<(), String> {
    if app.midi.selected.is_empty() {
        app.ui.status = "Split needs selected notes".into();
        return Err(app.ui.status.clone());
    }
    let at = at.unwrap_or_else(|| {
        let s = app.engine.session();
        s.tempo.samples_to_ticks(app.position(), s.sample_rate).saturating_sub(clip_base(s, cid))
    });
    let sel = app.midi.selected.clone();
    run_notes(app, "midi.notes_split", json!({"clip": cid.0, "indices": sel, "at": at}), None)
}

/// Join: the selected notes of one pitch into one note each.
pub fn join(app: &mut SoundApp, cid: ClipId) -> Result<(), String> {
    let sel = app.midi.selected.clone();
    run_notes(app, "midi.notes_join", json!({"clip": cid.0, "indices": sel}), None)
}

/// Cancels a drag in progress (Esc): nothing it did is kept.
pub fn cancel_drag(app: &mut SoundApp) -> bool {
    let m = &mut app.midi;
    let any = m.drag.is_some() || m.vel_drag.is_some() || m.draw.is_some() || m.band.is_some() || m.ctrl_drag.is_some();
    m.drag = None;
    m.vel_drag = None;
    m.draw = None;
    m.band = None;
    m.ctrl_drag = None;
    any
}

/// Whether a drag is in progress in the editor.
pub fn dragging(app: &SoundApp) -> bool {
    let m = &app.midi;
    m.drag.is_some() || m.vel_drag.is_some() || m.draw.is_some() || m.band.is_some() || m.ctrl_drag.is_some()
}

/// A piano key pressed: `pitch` sounds on the clip's track (the clip's copy holds only it,
/// soloed) until [`release_key`], at most [`KEY_HOLD_S`].
pub fn press_key(app: &mut SoundApp, cid: ClipId, pitch: u8) {
    app.midi.piano_key = Some(pitch);
    if app.engine.transport.recording || (app.is_playing() && app.midi.auditioning.is_none()) {
        return;
    }
    let s = app.engine.session_arc();
    let Some((_, clip)) = s.find_clip(cid) else { return };
    let base = s.tempo.samples_to_ticks(clip.start, s.sample_rate);
    let len = s.tempo.samples_to_ticks(clip.end(), s.sample_rate).saturating_sub(base).max(1);
    let note = Note { pitch: pitch.min(127), velocity: app.midi.last_velocity, release_velocity: 64, channel: 0, start: 0, length: len };
    let Some((copy, from, _)) = crate::midi_editor::audition_session(&s, cid, &[note]) else { return };
    let to = from.saturating_add((KEY_HOLD_S * s.sample_rate.as_f64()) as i64).min(clip.end());
    crate::midi_editor::start_audition(app, copy, from, to);
}

/// The piano key let go: its note stops.
pub fn release_key(app: &mut SoundApp) {
    if app.midi.piano_key.take().is_some() && app.midi.auditioning.is_some() {
        app.stop_play();
    }
}

/// The piano keys left of the roll: a press plays the key's note until the button goes up;
/// dragging along the keys glides from note to note.
pub fn piano_keys(app: &mut SoundApp, ui: &Ui, cid: ClipId, keys: Rect, pitch_at: &dyn Fn(f32) -> i32) {
    let resp = ui.interact(keys, ui.id().with(("midi_keys", cid.0)), Sense::click_and_drag());
    let down = resp.is_pointer_button_down_on() || resp.dragged();
    match (down, ui.ctx().pointer_latest_pos()) {
        (true, Some(p)) => {
            let pitch = pitch_at(p.y).clamp(0, 127) as u8;
            if app.midi.piano_key != Some(pitch) {
                press_key(app, cid, pitch);
            }
        }
        _ => {
            if app.midi.piano_key.is_some() {
                release_key(app);
            }
        }
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

/// Draws the held key lit, with its name.
pub fn lit_key(painter: &egui::Painter, key: Rect, pitch: i32) {
    let t = Tokens::current();
    painter.rect_filled(key, 1.0, t.accent);
    painter.text(
        pos2(key.max.x - 3.0, key.center().y),
        Align2::RIGHT_CENTER,
        soundcraft_midi::note_name(pitch.clamp(0, 127) as u8),
        bold(8.0),
        Color32::WHITE,
    );
}

/// The Grid menu: the Edit window's grid, 1/1 … 1/64, triplets, Off.
pub fn grid_menu(app: &mut SoundApp, ui: &mut Ui) {
    let edit = app.engine.session().edit.grid;
    let cur = app.midi.grid;
    if ui.radio(cur == MidiGrid::Edit, format!("Follow the Edit window ({})", MidiGrid::Edit.label(edit).trim_start_matches("Edit "))).clicked() {
        app.midi.grid = MidiGrid::Edit;
        ui.close();
    }
    ui.separator();
    let triplet = matches!(cur, MidiGrid::Note { triplet: true, .. });
    for value in NoteValue::ALL {
        let g = MidiGrid::Note { value, triplet };
        if ui.radio(cur == g, value_name(value)).clicked() {
            app.midi.grid = g;
            ui.close();
        }
    }
    ui.separator();
    let mut tri = triplet;
    if ui.checkbox(&mut tri, "Triplets").changed() {
        let value = match cur {
            MidiGrid::Note { value, .. } => value,
            _ => NoteValue::Eighth,
        };
        app.midi.grid = MidiGrid::Note { value, triplet: tri };
    }
    if ui.radio(cur == MidiGrid::Off, "Off").clicked() {
        app.midi.grid = MidiGrid::Off;
        ui.close();
    }
    ui.separator();
    ui.label(egui::RichText::new("Hold Alt while dragging to place freely").small().color(Tokens::current().text_dim));
}

/// The editor's state as `ui.midi_editor` and `ui.inspect` report it.
pub fn state(app: &SoundApp) -> Value {
    let s = app.engine.session();
    let cid = app.midi.clip.filter(|c| s.find_clip(*c).is_some());
    let note_bar = cid.and_then(|c| {
        let notes = clip_notes(s, c);
        let base = clip_base(s, c);
        Summary::of(&notes, &app.midi.selected).map(|m| {
            let mut o = serde_json::Map::new();
            for f in Field::ALL {
                o.insert(f.id().into(), json!(field_text(s, base, f, &m)));
            }
            Value::Object(o)
        })
    });
    let ghosts = cid.map_or(0, |c| crate::midi_ghosts::shown(app, s, c).len());
    json!({
        "clip": cid.map(|c| c.0),
        "selected": app.midi.selected,
        "grid": app.midi.grid.id(),
        "grid_label": app.midi.grid.label(s.edit.grid),
        "grid_ticks": app.midi.grid.ticks(s.edit.grid),
        "ghosts": app.midi.ghosts,
        "ghost_tracks": app.midi.ghost_tracks.as_ref().map_or(json!("all"), |v| json!(v.iter().map(|t| t.0).collect::<Vec<_>>())),
        "ghost_notes": ghosts,
        "zoom_factor": app.midi.zoom_factor,
        "dragging": dragging(app),
        "hover": app.midi.hover,
        "piano_key": app.midi.piano_key,
        "note_bar": note_bar,
    })
}

/// `ui.midi_editor {select_all?, select?: [n], split?: true | tick, join?, grid?: "1/16" | "1/8t" |
/// "off" | "edit", ghosts?: bool, ghost_tracks?: "all" | [track], zoom?: 0.25..4, fit?, cancel?,
/// set?: {pitch | start | length | velocity | channel: text}}`: drives the MIDI Editor on the clip
/// it shows (the selected MIDI clip) and returns its state ([`state`]).
pub fn command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    const ID: &str = "ui.midi_editor";
    let cid = crate::midi_editor::target_clip(app);
    if let Some(c) = cid
        && app.midi.clip != Some(c)
    {
        app.midi.clip = Some(c);
        app.midi.selected.clear();
    }
    let need = || cid.ok_or_else(|| "Select a MIDI clip first".to_string());
    if let Some(g) = p.get("grid") {
        let g = g.as_str().and_then(MidiGrid::parse).ok_or("`grid` is edit, off, 1/1 … 1/64, or one of those with t (triplet)")?;
        app.midi.grid = g;
    }
    if let Some(v) = p.get("ghosts") {
        app.midi.ghosts = v.as_bool().ok_or("`ghosts` must be a boolean")?;
    }
    if let Some(v) = p.get("ghost_tracks") {
        app.midi.ghost_tracks = match v {
            Value::String(s) if s == "all" => None,
            Value::Null => None,
            Value::Array(list) => {
                let mut ids = Vec::new();
                for x in list {
                    let t = soundcraft_engine::cmd::track_param(&app.engine, ID, &json!({"track": x}), "track").map_err(|e| e.to_string())?;
                    ids.extend(t);
                }
                Some(ids)
            }
            _ => return Err("`ghost_tracks` is \"all\" or a list of tracks".into()),
        };
    }
    if let Some(sel) = p.get("select") {
        let len = cid.map_or(0, |c| clip_notes(app.engine.session(), c).len());
        app.midi.selected = sel
            .as_array()
            .ok_or("`select` is a list of note indices")?
            .iter()
            .filter_map(Value::as_u64)
            .map(|v| v as usize)
            .filter(|i| *i < len)
            .collect();
    }
    if p.get("select_all").and_then(Value::as_bool) == Some(true) {
        select_all(app, need()?);
    }
    if let Some(v) = p.get("split") {
        let at = match v {
            Value::Bool(true) => None,
            Value::Number(n) => Some(n.as_i64().ok_or("`split` is true (the playhead) or a tick")?),
            _ => return Err("`split` is true (the playhead) or a tick from the clip start".into()),
        };
        split(app, need()?, at)?;
    }
    if p.get("join").and_then(Value::as_bool) == Some(true) {
        join(app, need()?)?;
    }
    if let Some(set) = p.get("set") {
        let c = need()?;
        let set = set.as_object().ok_or("`set` is {field: text}")?;
        for (k, v) in set {
            let f = Field::from_id(k).ok_or_else(|| format!("no note bar field `{k}`"))?;
            let text = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            apply_text(app, c, f, &text)?;
        }
    }
    if let Some(z) = p.get("zoom") {
        let z = z.as_f64().filter(|z| ZOOM_FACTORS.iter().any(|f| (f64::from(*f) - z).abs() < 1e-6)).ok_or("`zoom` is 0.25, 0.5, 1, 2 or 4")?;
        app.midi.pending_zoom = Some(z as f32);
    }
    if p.get("fit").and_then(Value::as_bool) == Some(true) {
        app.midi.pending_zoom = Some(0.0);
    }
    if p.get("cancel").and_then(Value::as_bool) == Some(true) {
        cancel_drag(app);
    }
    Ok(state(app))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_ids_round_trip() {
        for g in [
            MidiGrid::Edit,
            MidiGrid::Off,
            MidiGrid::Note { value: NoteValue::Sixteenth, triplet: false },
            MidiGrid::Note { value: NoteValue::Eighth, triplet: true },
        ] {
            assert_eq!(MidiGrid::parse(&g.id()), Some(g));
        }
        assert_eq!(MidiGrid::Note { value: NoteValue::Eighth, triplet: true }.ticks(GridValue::default()), 320); // 960 PPQ: eighth triplet = 1/3 beat
        assert_eq!(MidiGrid::Off.ticks(GridValue::default()), 1);
        assert_eq!(MidiGrid::Off.step(GridValue::default()), 240);
    }

    #[test]
    fn positions_and_lengths_read_back() {
        let map = TempoMap::default();
        assert_eq!(bbt(&map, 0), "1.1.1");
        assert_eq!(bbt(&map, 3840 * 33), "34.1.1");
        assert_eq!(parse_bbt(&map, "34.1.1"), Some(3840 * 33));
        assert_eq!(parse_bbt(&map, &bbt(&map, 1000)), Some(1000));
        assert_eq!(bbt_len(&map, 0, 3840), "1.0.0");
        assert_eq!(bbt_len(&map, 0, 960 + 260), "0.1.1+20");
        assert_eq!(parse_bbt_len(&map, 0, "0.1.1+20"), Some(1220));
    }

    #[test]
    fn pitch_names_read() {
        assert_eq!(parse_pitch("C3"), Some(60));
        assert_eq!(parse_pitch("F#3"), Some(66));
        assert_eq!(parse_pitch("Gb3"), Some(66));
        assert_eq!(parse_pitch("C-2"), Some(0));
        assert_eq!(parse_pitch("64"), Some(64));
        assert_eq!(parse_pitch("H3"), None);
        for p in 0..=127u8 {
            assert_eq!(parse_pitch(&soundcraft_midi::note_name(p)), Some(i64::from(p)));
        }
    }
}
