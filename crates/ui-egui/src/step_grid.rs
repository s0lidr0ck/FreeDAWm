//! The step grid: a pattern clip's steps, in the Juke window's Steps view and in the MIDI Editor
//! (look approved 2026-11-06, `docs/juke.md` §5, `StepGrid.dc.html`).
//!
//! The pattern bar (steps per bar and the **New steps** velocity as segmented controls, the
//! Shuffle, Accent and Humanize dials, Fill every, Re-roll, Convert to MIDI) over the step
//! numbers and one row per lane: a header band in the lane's pad colour (its name, choke group,
//! rate, length and mute), then its steps as raised keys grouped by beat.
//!
//! * Click a step to turn it on or off; drag across steps to paint or clear a run (the first step
//!   decides which). New steps take the New steps velocity.
//! * Drag a lit step up or down for fine velocity; keys 1–4 over a step set Full, 75, 50 or 25 %.
//! * Right-click a step for its details, in a popover at the step: velocity, probability and
//!   nudge sliders, Roll 1–4, Accent and Flam, and when it plays. Right-click a lane's header for
//!   its rate, direction, note and removal.
//!
//! Every edit is a `pattern.*` command, so it undoes and scripts like any other.

use crate::SoundApp;
use crate::theme::{bold, regular};
use crate::widgets::{self, KnobStyle};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_dsp::{ParamInfo, Taper, Unit};
use soundcraft_midi::pattern::{Condition, Direction, Lane, Pattern, Rate, Step};
use soundcraft_model::{Clip, ClipId, DrumFamily, Insert, TrackId};

/// The New steps velocity tiers: Full, 75, 50 and 25 %.
pub const TIERS: [(&str, u8); 4] = [("Full", 127), ("75", 95), ("50", 64), ("25", 32)];

const HEADER_W: f32 = 196.0;
const BAR_H: f32 = 54.0;
const NUMBERS_H: f32 = 20.0;
const HINT_H: f32 = 18.0;
const CREAM: Color32 = Color32::from_rgb(0xF6, 0xEE, 0xDF);
const AMBER: Color32 = Color32::from_rgb(0xF0, 0xB2, 0x4A);
const DARK: Color32 = Color32::from_rgb(0x1E, 0x19, 0x15);
const GROUND: Color32 = Color32::from_rgb(0x2A, 0x24, 0x1F);
const BAR_BG: Color32 = Color32::from_rgb(0x22, 0x1D, 0x19);
const WELL: Color32 = Color32::from_rgb(0x15, 0x11, 0x0E);
const LABEL: Color32 = Color32::from_rgb(0xCD, 0xBF, 0xA8);
const DIM: Color32 = Color32::from_rgb(0x8A, 0x7B, 0x68);
const BUTTON: Color32 = Color32::from_rgb(0x3A, 0x32, 0x2B);

/// When a step plays, as the popover's pills offer it.
pub const PLAYS: [(Condition, &str); 6] = [
    (Condition::Always, "Always"),
    (Condition::Every { nth: 1, of: 2 }, "1st of 2"),
    (Condition::Every { nth: 2, of: 2 }, "2nd of 2"),
    (Condition::First, "First pass"),
    (Condition::Fill, "Fill bars"),
    (Condition::NotFill, "Not in fills"),
];

/// The step numbers and the keys: where each column of steps sits, with a wider gap between beats.
#[derive(Debug, Clone, Copy)]
struct Columns {
    x0: f32,
    w: f32,
    gap: f32,
    beat_gap: f32,
    beat: usize,
    n: usize,
}

impl Columns {
    fn new(r: Rect, n: usize, beat: usize) -> Columns {
        let n = n.max(1);
        let beat = beat.max(1);
        let groups = n.div_ceil(beat);
        let width = |gap: f32, beat_gap: f32| (r.width() - (n - 1) as f32 * gap - (groups - 1) as f32 * (beat_gap - gap)) / n as f32;
        let (mut gap, mut beat_gap) = (4.0, 12.0);
        if width(gap, beat_gap) < 14.0 {
            (gap, beat_gap) = (1.5, 5.0);
        }
        Columns { x0: r.min.x, w: width(gap, beat_gap).clamp(3.0, 60.0), gap, beat_gap, beat, n }
    }
    /// Column `col`'s left edge.
    fn x(&self, col: usize) -> f32 {
        self.x0 + col as f32 * (self.w + self.gap) + (col / self.beat) as f32 * (self.beat_gap - self.gap)
    }
    /// The column under `x` (a gap counts with the step before it), clamped to the grid.
    fn at(&self, x: f32) -> usize {
        (0..self.n).rev().find(|&c| self.x(c) <= x).unwrap_or(0)
    }
    fn cell(&self, col: usize, y: std::ops::RangeInclusive<f32>) -> Rect {
        Rect::from_min_max(pos2(self.x(col), *y.start()), pos2(self.x(col) + self.w, *y.end()))
    }
}

/// The velocity a key 1–4 sets.
pub fn tier_velocity(i: usize) -> u8 {
    TIERS.get(i).map_or(127, |t| t.1)
}

/// A condition as `pattern.step` takes it.
pub fn condition_arg(c: Condition) -> Value {
    match c {
        Condition::Always => json!("always"),
        Condition::First => json!("first"),
        Condition::NotFirst => json!("not_first"),
        Condition::Fill => json!("fill"),
        Condition::NotFill => json!("not_fill"),
        Condition::Every { nth, of } => json!(format!("{nth}:{of}")),
    }
}

/// A condition's short badge (`1:2`, `F`, `!F`, `1st`, `!1st`).
pub fn condition_badge(c: Condition) -> Option<String> {
    match c {
        Condition::Always => None,
        Condition::First => Some("1st".into()),
        Condition::NotFirst => Some("!1st".into()),
        Condition::Fill => Some("F".into()),
        Condition::NotFill => Some("!F".into()),
        Condition::Every { nth, of } => Some(format!("{nth}:{of}")),
    }
}

/// An in-progress gesture on the grid.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// Pressed on a step; not yet a paint or a velocity drag.
    Pressed { lane: usize, step: usize, on: bool },
    /// Painting steps `from..=to` (either order) on or off.
    Paint { lane: usize, from: usize, to: usize, on: bool },
    /// Dragging a step's velocity from `start`.
    Velocity { lane: usize, step: usize, start: u8 },
}

/// The grid's per-editor state.
#[derive(Debug, Clone, Copy, Default)]
struct State {
    /// Index into [`TIERS`].
    tier: usize,
    drag: Option<Drag>,
    /// The step whose details popover is open.
    details: Option<(usize, usize)>,
    /// Where that step was drawn: the popover hangs from it.
    anchor: Option<Rect>,
}

fn state_id(cid: ClipId) -> egui::Id {
    egui::Id::new(("step-grid", cid.0))
}

/// How a lane shows: the Juke pad its note triggers, or the General MIDI drum.
struct Look {
    name: String,
    color: Color32,
    /// The pad's choke group, when it is in one.
    choke: Option<u8>,
}

fn lane_look(juke: Option<&Insert>, note: u8) -> Look {
    if let Some(ins) = juke
        && let Some(pad) = crate::juke_window::pad_for_note(ins, note)
    {
        let layout = soundcraft_engine::cmd::juke::pad_layout(ins, pad);
        let choke = Some(crate::juke_window::pad_value(ins, pad, "choke").round().clamp(0.0, 8.0) as u8).filter(|c| *c > 0);
        return Look { name: layout.name, color: crate::juke_window::family_color(layout.family), choke };
    }
    let name = soundcraft_midi::gm_drum_name(note).map_or_else(|| soundcraft_midi::note_name(note), str::to_string);
    let family = match note {
        35 | 36 => DrumFamily::Kick,
        38 | 40 => DrumFamily::Snare,
        39 => DrumFamily::Clap,
        42 | 44 | 46 => DrumFamily::Hat,
        41 | 43 | 45 | 47 | 48 | 50 => DrumFamily::Tom,
        49 | 51 | 52 | 53 | 55 | 57 | 59 => DrumFamily::Cymbal,
        _ => DrumFamily::Percussion,
    };
    Look { name, color: crate::juke_window::family_color(family), choke: None }
}

/// The lane's second line: its choke group, rate and direction (`Choke 1 · ½ rate`).
pub fn lane_extra(choke: Option<u8>, lane: &Lane) -> String {
    let mut v = Vec::new();
    if let Some(c) = choke {
        v.push(format!("Choke {c}"));
    }
    match lane.rate {
        Rate::Half => v.push("½ rate".into()),
        Rate::Double => v.push("2× rate".into()),
        Rate::Normal => {}
    }
    match lane.direction {
        Direction::Reverse => v.push("Reverse".into()),
        Direction::PingPong => v.push("Ping-pong".into()),
        Direction::Random => v.push("Random".into()),
        Direction::Forward => {}
    }
    v.join(" · ")
}

/// What the grid asked for this frame.
#[derive(Default)]
struct Out {
    /// Commands, each optionally merged into one undo step by its key.
    cmds: Vec<(&'static str, Value, Option<String>)>,
}

impl Out {
    fn run(&mut self, id: &'static str, p: Value) {
        self.cmds.push((id, p, None));
    }
    fn merged(&mut self, id: &'static str, p: Value, key: String) {
        self.cmds.push((id, p, Some(key)));
    }
}

/// Draws clip `cid`'s step grid in `r`. `clip` must be a pattern clip.
pub fn show(app: &mut SoundApp, ui: &mut Ui, r: Rect, track: TrackId, cid: ClipId, clip: &Clip) {
    let Some(pc) = clip.pattern.as_ref() else { return };
    let pattern = &pc.pattern;
    let ctx = ui.ctx().clone();
    let mut st: State = ctx.data(|d| d.get_temp(state_id(cid))).unwrap_or_default();
    let mut out = Out::default();
    ui.painter().rect_filled(r, 0.0, GROUND);
    let juke = app.engine.session().track(track).and_then(|t| t.instrument.clone()).filter(|i| i.plugin == soundcraft_engine::cmd::juke::JUKE);
    let bar = Rect::from_min_size(r.min, vec2(r.width(), BAR_H));
    pattern_bar(ui, bar, cid, pattern, &mut st, &mut out);
    let playhead = playhead_steps(app, clip, pc.phase, pattern);
    let columns = pattern.lanes.iter().map(|l| l.steps.len()).max().unwrap_or(16).max(1);
    let beat = usize::from((pattern.steps_per_bar / 4).max(1));
    let inner = Rect::from_min_max(pos2(r.min.x + 10.0, bar.max.y + 4.0), pos2(r.max.x - 10.0, r.max.y - HINT_H));
    // Step numbers, the beat's first brighter, the playing step bold.
    let steps_x = inner.min.x + HEADER_W + 10.0;
    let numbers = Rect::from_min_max(pos2(steps_x, inner.min.y), pos2(inner.max.x - 12.0, inner.min.y + NUMBERS_H));
    let cols = Columns::new(numbers, columns, beat);
    let now = playhead.iter().flatten().next().copied();
    for c in 0..columns {
        if cols.w < 12.0 && c % beat != 0 {
            continue;
        }
        let (font, color) = if now == Some(c) {
            (bold(11.0), CREAM)
        } else if c % beat == 0 {
            (regular(11.0), LABEL)
        } else {
            (regular(11.0), DIM)
        };
        ui.painter().text(pos2(cols.x(c) + cols.w * 0.5, numbers.center().y), Align2::CENTER_CENTER, (c + 1).to_string(), font, color);
    }
    // The lanes.
    let body = Rect::from_min_max(pos2(inner.min.x, numbers.max.y + 2.0), inner.max);
    let n = pattern.lanes.len();
    let lane_h = ((body.height() - 34.0) / n.max(1) as f32).clamp(30.0, 44.0);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
    egui::ScrollArea::vertical().id_salt(("step-lanes", cid.0)).auto_shrink([false, false]).show(&mut child, |ui| {
        let h = n as f32 * lane_h + 34.0;
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
        let cols = Columns::new(Rect::from_min_max(pos2(steps_x, area.min.y), pos2(area.max.x - 12.0, area.max.y)), columns, beat);
        for (i, lane) in pattern.lanes.iter().enumerate() {
            let row = Rect::from_min_size(pos2(area.min.x, area.min.y + i as f32 * lane_h), vec2(area.width(), lane_h - 6.0));
            let look = lane_look(juke.as_ref(), lane.note);
            lane_header(ui, Rect::from_min_size(row.min, vec2(HEADER_W, row.height())), cid, i, lane, &look, &mut out);
            lane_steps(ui, row.y_range(), &cols, cid, i, lane, &look, playhead.get(i).copied().flatten(), &mut st, &mut out);
        }
        // Add a lane: the next pad's note not in the pattern yet.
        let add = Rect::from_min_size(pos2(area.min.x, area.min.y + n as f32 * lane_h), vec2(HEADER_W, 26.0));
        let used: Vec<u8> = pattern.lanes.iter().map(|l| l.note).collect();
        let next = match &juke {
            Some(ins) => (0..soundcraft_dsp::plugins::juke::PADS).map(|p| crate::juke_window::pad_note(ins, p)).find(|n| !used.contains(n)),
            None => (36..=81).find(|n| !used.contains(n)),
        };
        let resp = ui.put(add, egui::Button::new(egui::RichText::new("+ Lane").font(regular(12.0)).color(CREAM)).fill(BUTTON).corner_radius(7.0));
        if resp.on_hover_text("Add a lane for the next pad").clicked() {
            out.run("pattern.lane_add", json!({"clip": cid.0, "note": next.unwrap_or(36)}));
        }
    });
    ui.painter().text(
        pos2(r.min.x + 12.0, r.max.y - HINT_H * 0.5),
        Align2::LEFT_CENTER,
        "Click: on/off · drag across: paint · drag up/down: velocity · keys 1–4: Full/75/50/25 · right-click: the step's details",
        regular(10.5),
        DIM,
    );
    details_popover(ui, cid, pattern, juke.as_ref(), &mut st, &mut out);
    for (id, p, key) in out.cmds {
        let r = match key {
            Some(k) => app.engine.execute_merged(id, &p, &k).map_err(|e| e.to_string()),
            None => app.run(id, p),
        };
        if let Err(e) = r {
            app.ui.status = e;
        }
    }
    ctx.data_mut(|d| d.insert_temp(state_id(cid), st));
}

/// Per lane, the step the playhead is on while the transport plays inside the clip.
fn playhead_steps(app: &SoundApp, clip: &Clip, phase: i64, pattern: &Pattern) -> Vec<Option<usize>> {
    let n = pattern.lanes.len();
    if !app.is_playing() {
        return vec![None; n];
    }
    let s = app.engine.session();
    let pos = app.engine.transport.position;
    if !clip.range().contains(pos) {
        return vec![None; n];
    }
    let sr = s.sample_rate;
    let t0 = s.tempo.samples_to_ticks(clip.start, sr);
    let now = s.tempo.samples_to_ticks(pos, sr);
    let per_bar = s.tempo.meter_at_tick(t0).ticks_per_bar();
    let step_ticks = (per_bar as f64 / f64::from(pattern.steps_per_bar.max(1))).max(1.0);
    let at = (now - t0 + phase) as f64;
    pattern
        .lanes
        .iter()
        .map(|l| {
            let len = l.steps.len();
            if len == 0 || l.direction != Direction::Forward {
                return None;
            }
            let k = match l.rate {
                Rate::Half => 0.5,
                Rate::Normal => 1.0,
                Rate::Double => 2.0,
            };
            let i = (at / step_ticks * k).floor();
            (i >= 0.0).then(|| (i as usize) % len)
        })
        .collect()
}

/// A segmented control in `r`: returns the index clicked.
fn segmented(ui: &mut Ui, r: Rect, id: egui::Id, items: &[(String, f32)], selected: Option<usize>, tip: &str) -> Option<usize> {
    let p = ui.painter().clone();
    p.rect_filled(r, 8.0, WELL);
    let mut x = r.min.x + 2.0;
    let mut clicked = None;
    for (i, (label, w)) in items.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(x, r.min.y + 2.0), vec2(*w, r.height() - 4.0));
        x += w + 2.0;
        let resp = ui.interact(cell, id.with(i), Sense::click()).on_hover_text(tip);
        let on = selected == Some(i);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, ui.is_enabled(), on, label));
        let fill = if on {
            AMBER
        } else if resp.hovered() {
            Color32::from_white_alpha(14)
        } else {
            Color32::TRANSPARENT
        };
        p.rect_filled(cell, 6.0, fill);
        p.text(cell.center(), Align2::CENTER_CENTER, label, bold(12.0), if on { DARK } else { CREAM });
        if resp.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// A segmented control's width.
fn segmented_w(items: &[(String, f32)]) -> f32 {
    items.iter().map(|i| i.1 + 2.0).sum::<f32>() + 2.0
}

/// A caption over a control.
fn caption(ui: &Ui, x: f32, y: f32, s: &str) {
    ui.painter().text(pos2(x, y), Align2::LEFT_TOP, s, regular(11.0), LABEL);
}

/// A dial on the pattern bar: a cream knob with its name and value beside it. Returns its new value.
fn dial(ui: &mut Ui, at: egui::Pos2, info: &ParamInfo, v: f32, value: String) -> Option<f32> {
    let k = Rect::from_min_size(at, vec2(32.0, 32.0));
    let resp = ui.scope_builder(egui::UiBuilder::new().max_rect(k), |ui| widgets::knob(ui, info, v, 26.0, AMBER, KnobStyle::Cream));
    let p = ui.painter();
    p.text(pos2(k.max.x + 4.0, k.center().y - 1.0), Align2::LEFT_BOTTOM, info.name, regular(11.0), LABEL);
    p.text(pos2(k.max.x + 4.0, k.center().y + 1.0), Align2::LEFT_TOP, value, bold(12.0), CREAM);
    resp.inner.changed
}

const fn dial_info(id: &'static str, name: &'static str, max: f32, unit: Unit) -> ParamInfo {
    ParamInfo { id, name, min: 0.0, max, default: 0.0, unit, taper: Taper::Linear, choices: &[], display: None, group: "" }
}

const SHUFFLE: ParamInfo = dial_info("shuffle", "Shuffle", 100.0, Unit::Percent);
const ACCENT: ParamInfo = dial_info("accent", "Accent", 127.0, Unit::None);
const HUMANIZE: ParamInfo = dial_info("humanize", "Humanize", 100.0, Unit::Percent);

/// The fill choices: every 2, 4, 8 or 16 bars, or none.
const FILLS: [u8; 5] = [0, 2, 4, 8, 16];

fn fill_label(n: u8) -> String {
    match n {
        0 => "Off".into(),
        1 => "1 bar".into(),
        n => format!("{n} bars"),
    }
}

fn pattern_bar(ui: &mut Ui, r: Rect, cid: ClipId, pattern: &Pattern, st: &mut State, out: &mut Out) {
    ui.painter().rect_filled(r, 0.0, BAR_BG);
    ui.painter().hline(r.x_range(), r.max.y - 0.5, Stroke::new(1.0, WELL));
    let cap_y = r.min.y + 6.0;
    let row = Rect::from_min_max(pos2(r.min.x, cap_y + 16.0), pos2(r.max.x, cap_y + 44.0));
    let mut x = r.min.x + 12.0;
    // Steps per bar.
    caption(ui, x, cap_y, "Steps per bar");
    let items: Vec<(String, f32)> = soundcraft_midi::pattern::STEPS_PER_BAR.iter().map(|n| (n.to_string(), 28.0)).collect();
    let w = segmented_w(&items);
    let sel = soundcraft_midi::pattern::STEPS_PER_BAR.iter().position(|n| *n == pattern.steps_per_bar);
    let seg = Rect::from_min_size(pos2(x, row.min.y), vec2(w, row.height()));
    if let Some(i) = segmented(ui, seg, egui::Id::new(("spb", cid.0)), &items, sel, "Steps in each bar")
        && let Some(n) = soundcraft_midi::pattern::STEPS_PER_BAR.get(i)
        && *n != pattern.steps_per_bar
    {
        out.run("pattern.settings", json!({"clip": cid.0, "steps_per_bar": n}));
    }
    x += w + 14.0;
    // New steps velocity.
    caption(ui, x, cap_y, "New steps");
    let items: Vec<(String, f32)> = TIERS.iter().enumerate().map(|(i, (n, _))| ((*n).to_string(), if i == 0 { 38.0 } else { 28.0 })).collect();
    let w = segmented_w(&items);
    let seg = Rect::from_min_size(pos2(x, row.min.y), vec2(w, row.height()));
    if let Some(i) = segmented(
        ui,
        seg,
        egui::Id::new(("tier", cid.0)),
        &items,
        Some(st.tier),
        "The velocity a step takes when it is turned on (keys 1–4 set a step's)",
    ) {
        st.tier = i;
    }
    x += w + 16.0;
    // Shuffle, accent, humanize: each drag one undo step.
    let dials = [
        (&SHUFFLE, f32::from(pattern.shuffle), format!("{}%", pattern.shuffle)),
        (&ACCENT, f32::from(pattern.accent), format!("+{}", pattern.accent)),
        (&HUMANIZE, f32::from(pattern.humanize), format!("{}%", pattern.humanize)),
    ];
    for (info, v, shown) in dials {
        if let Some(nv) = dial(ui, pos2(x, r.center().y - 16.0), info, v, shown) {
            let nv = nv.round().clamp(0.0, info.max) as u8;
            let mut p = json!({"clip": cid.0});
            p[info.id] = json!(nv);
            out.merged("pattern.settings", p, format!("pattern_settings:{}:{}", cid.0, info.id));
        }
        x += 96.0;
    }
    // Fill every.
    caption(ui, x, cap_y, "Fill every");
    let fill_r = Rect::from_min_size(pos2(x, row.min.y), vec2(84.0, row.height()));
    let mut fill = pattern.fill_every;
    ui.scope_builder(egui::UiBuilder::new().max_rect(fill_r), |ui| {
        egui::ComboBox::from_id_salt(("fill-every", cid.0)).width(76.0).selected_text(fill_label(fill)).show_ui(ui, |ui| {
            for n in FILLS {
                ui.selectable_value(&mut fill, n, fill_label(n));
            }
        });
    });
    if fill != pattern.fill_every {
        out.run("pattern.settings", json!({"clip": cid.0, "fill_every": fill}));
    }
    // Re-roll and Convert to MIDI, on the right.
    let button = |s: &str| egui::Button::new(egui::RichText::new(s).font(regular(12.0)).color(CREAM)).fill(BUTTON).corner_radius(8.0);
    let convert = Rect::from_min_size(pos2(r.max.x - 12.0 - 118.0, r.center().y - 15.0), vec2(118.0, 30.0));
    let reroll = Rect::from_min_size(pos2(convert.min.x - 8.0 - 76.0, convert.min.y), vec2(76.0, 30.0));
    if reroll.min.x > fill_r.max.x + 8.0 {
        if ui.put(reroll, button("Re-roll")).on_hover_text("Draw every chance step again").clicked() {
            out.run("pattern.reroll", json!({"clip": cid.0}));
        }
        if ui.put(convert, button("Convert to MIDI")).on_hover_text("Keep the notes as a plain MIDI clip, to edit them by hand").clicked() {
            out.run("pattern.to_midi", json!({"clip": cid.0}));
        }
    } else {
        // Too narrow: both in a menu.
        let more = Rect::from_min_size(pos2(r.max.x - 12.0 - 34.0, convert.min.y), vec2(34.0, 30.0));
        let resp = ui.put(more, button("…"));
        egui::Popup::menu(&resp).show(|ui| {
            if ui.button("Re-roll").clicked() {
                out.run("pattern.reroll", json!({"clip": cid.0}));
                ui.close();
            }
            if ui.button("Convert to MIDI").clicked() {
                out.run("pattern.to_midi", json!({"clip": cid.0}));
                ui.close();
            }
        });
    }
}

fn lane_header(ui: &mut Ui, r: Rect, cid: ClipId, i: usize, lane: &Lane, look: &Look, out: &mut Out) {
    let p = ui.painter().clone();
    let c = if lane.mute { look.color.gamma_multiply(0.45) } else { look.color };
    p.rect_filled(r, 8.0, c);
    p.hline(r.min.x + 6.0..=r.max.x - 6.0, r.min.y + 1.0, Stroke::new(1.0, Color32::from_white_alpha(70)));
    let extra = lane_extra(look.choke, lane);
    let name_r = Rect::from_min_max(r.min, pos2(r.max.x - 96.0, r.max.y));
    let clip = p.with_clip_rect(name_r);
    if extra.is_empty() {
        clip.text(pos2(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, &look.name, bold(12.0), DARK);
    } else {
        clip.text(pos2(r.min.x + 10.0, r.center().y + 1.0), Align2::LEFT_BOTTOM, &look.name, bold(12.0), DARK);
        clip.text(pos2(r.min.x + 10.0, r.center().y + 1.0), Align2::LEFT_TOP, &extra, regular(10.0), DARK.gamma_multiply(0.75));
    }
    // Length, as a drag value; mute as a chip.
    let len_r = Rect::from_min_size(pos2(r.max.x - 92.0, r.center().y - 10.0), vec2(58.0, 20.0));
    let mut len = lane.steps.len() as i32;
    let resp = ui
        .put(len_r, egui::DragValue::new(&mut len).range(1..=soundcraft_midi::pattern::MAX_STEPS as i32).suffix(" steps"))
        .on_hover_text("The lane's length in steps: shorter lanes loop on their own (a polymeter)");
    if resp.changed() && len != lane.steps.len() as i32 {
        out.merged("pattern.lane", json!({"clip": cid.0, "lane": i, "length": len}), format!("pattern_lane_len:{}:{i}", cid.0));
    }
    let mute_r = Rect::from_min_size(pos2(r.max.x - 30.0, r.center().y - 11.0), vec2(26.0, 22.0));
    let resp = ui.interact(mute_r, egui::Id::new(("lane-mute", cid.0, i)), Sense::click()).on_hover_text("Mute the lane");
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, lane.mute, "Mute the lane"));
    p.rect_filled(mute_r, 6.0, if lane.mute { DARK } else { Color32::from_black_alpha(22) });
    p.rect_stroke(mute_r, 6.0, Stroke::new(1.0, DARK.gamma_multiply(0.25)), StrokeKind::Inside);
    p.text(mute_r.center(), Align2::CENTER_CENTER, "M", bold(11.0), if lane.mute { look.color } else { DARK });
    if resp.clicked() {
        out.run("pattern.lane", json!({"clip": cid.0, "lane": i, "mute": !lane.mute}));
    }
    let head = ui
        .interact(name_r, egui::Id::new(("lane-head", cid.0, i)), Sense::click())
        .on_hover_text("Right-click for the lane's rate, direction and note");
    head.context_menu(|ui| {
        ui.label(egui::RichText::new(format!("{} · {}", look.name, soundcraft_midi::note_name(lane.note))).strong());
        ui.menu_button("Rate", |ui| {
            for (rate, label, arg) in
                [(Rate::Half, "Half speed", "half"), (Rate::Normal, "Normal", "normal"), (Rate::Double, "Double speed", "double")]
            {
                if ui.selectable_label(lane.rate == rate, label).clicked() {
                    out.run("pattern.lane", json!({"clip": cid.0, "lane": i, "rate": arg}));
                    ui.close();
                }
            }
        });
        ui.menu_button("Direction", |ui| {
            for (d, label, arg) in [
                (Direction::Forward, "Forward", "forward"),
                (Direction::Reverse, "Reverse", "reverse"),
                (Direction::PingPong, "Ping-pong", "ping_pong"),
                (Direction::Random, "Random", "random"),
            ] {
                if ui.selectable_label(lane.direction == d, label).clicked() {
                    out.run("pattern.lane", json!({"clip": cid.0, "lane": i, "direction": arg}));
                    ui.close();
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Note");
            let mut n = i32::from(lane.note);
            if ui
                .add(
                    egui::DragValue::new(&mut n)
                        .range(0..=127)
                        .custom_formatter(|v, _| soundcraft_midi::note_name(v.round().clamp(0.0, 127.0) as u8)),
                )
                .changed()
            {
                out.merged("pattern.lane", json!({"clip": cid.0, "lane": i, "note": n}), format!("pattern_lane_note:{}:{i}", cid.0));
            }
        });
        ui.separator();
        if ui.button("Remove Lane").clicked() {
            out.run("pattern.lane_remove", json!({"clip": cid.0, "lane": i}));
            ui.close();
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn lane_steps(
    ui: &mut Ui,
    y: egui::Rangef,
    cols: &Columns,
    cid: ClipId,
    lane_i: usize,
    lane: &Lane,
    look: &Look,
    playhead: Option<usize>,
    st: &mut State,
    out: &mut Out,
) {
    let len = lane.steps.len().max(1);
    let r = Rect::from_min_max(pos2(cols.x(0), y.min), pos2(cols.x(cols.n - 1) + cols.w, y.max));
    let p = ui.painter().clone();
    let color = look.color;
    let resp = ui.interact(r, egui::Id::new(("lane-steps", cid.0, lane_i)), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, format!("{} steps", look.name)));
    let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
    // The painted run, while painting.
    let painting = match st.drag {
        Some(Drag::Paint { lane: l, from, to, on }) if l == lane_i => Some((from.min(to), from.max(to), on)),
        _ => None,
    };
    for col in 0..cols.n {
        let step_i = col % len;
        let ghost = col >= len;
        let Some(step) = lane.steps.get(step_i) else { continue };
        let cell = cols.cell(col, y.min..=y.max);
        let on = match painting {
            Some((a, b, on)) if !ghost && (a..=b).contains(&col) => on,
            _ => step.on,
        };
        draw_step(&p, cell, step, on, color, ghost);
        if !ghost && playhead == Some(step_i) {
            p.rect_stroke(cell.expand(1.5), 6.0, Stroke::new(2.0, CREAM), StrokeKind::Outside);
        }
        if !ghost && st.details == Some((lane_i, col)) {
            st.anchor = Some(cell);
            p.rect_stroke(cell.expand(1.5), 6.0, Stroke::new(2.0, AMBER), StrokeKind::Outside);
        }
    }
    let column = |x: f32| cols.at(x);
    // Gestures.
    if resp.drag_started()
        && let Some(pos) = pointer
    {
        let col = column(pos.x);
        if col < len {
            st.drag = Some(Drag::Pressed { lane: lane_i, step: col, on: !lane.steps[col].on });
        }
    }
    if resp.dragged()
        && let Some(pos) = pointer
    {
        let col = column(pos.x).min(len - 1);
        let total = resp.interact_pointer_pos().zip(ui.input(|i| i.pointer.press_origin())).map(|(a, b)| a - b).unwrap_or_default();
        match st.drag {
            Some(Drag::Pressed { lane: l, step, on }) if l == lane_i => {
                if col != step {
                    st.drag = Some(Drag::Paint { lane: l, from: step, to: col, on });
                } else if total.y.abs() > 6.0
                    && let Some(s) = lane.steps.get(step).filter(|s| s.on)
                {
                    st.drag = Some(Drag::Velocity { lane: l, step, start: s.velocity });
                }
            }
            Some(Drag::Paint { lane: l, from, on, .. }) if l == lane_i => st.drag = Some(Drag::Paint { lane: l, from, to: col, on }),
            Some(Drag::Velocity { lane: l, step, start }) if l == lane_i => {
                let v = (f32::from(start) - total.y * 0.5).round().clamp(1.0, 127.0) as u8;
                if lane.steps.get(step).is_some_and(|s| s.velocity != v) {
                    out.merged(
                        "pattern.step",
                        json!({"clip": cid.0, "lane": lane_i, "step": step, "velocity": v}),
                        format!("pattern_vel:{}:{lane_i}:{step}", cid.0),
                    );
                }
            }
            _ => {}
        }
    }
    let velocity = tier_velocity(st.tier);
    if resp.drag_stopped() {
        match st.drag.take() {
            Some(Drag::Paint { lane: l, from, to, on }) if l == lane_i => {
                out.run(
                    "pattern.paint",
                    json!({"clip": cid.0, "lane": lane_i, "from": from.min(to), "to": from.max(to), "on": on, "velocity": velocity}),
                );
            }
            Some(Drag::Pressed { lane: l, step, on }) if l == lane_i => toggle(out, cid, lane_i, step, on, velocity),
            other => st.drag = other.filter(|d| !matches!(d, Drag::Velocity { lane: l, .. } if *l == lane_i)),
        }
    }
    if resp.clicked()
        && let Some(pos) = pointer
    {
        let col = column(pos.x);
        if col < len {
            let modifier = ui.input(|i| i.modifiers.command || i.modifiers.alt);
            if modifier {
                st.details = Some((lane_i, col));
            } else {
                toggle(out, cid, lane_i, col, !lane.steps[col].on, velocity);
            }
        }
    }
    if resp.secondary_clicked()
        && let Some(pos) = pointer
    {
        let col = column(pos.x);
        if col < len {
            st.details = Some((lane_i, col));
            st.anchor = Some(cols.cell(col, y.min..=y.max));
        }
    }
    // Keys 1–4 over a step set its velocity tier (and turn it on).
    if let Some(pos) = resp.hover_pos() {
        let col = column(pos.x);
        let keys = [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4];
        for (k, key) in keys.into_iter().enumerate() {
            if col < len && ui.input(|i| i.key_pressed(key) && i.modifiers.is_none()) {
                out.run("pattern.step", json!({"clip": cid.0, "lane": lane_i, "step": col, "on": true, "velocity": tier_velocity(k)}));
            }
        }
    }
}

fn toggle(out: &mut Out, cid: ClipId, lane: usize, step: usize, on: bool, velocity: u8) {
    if on {
        out.run("pattern.step", json!({"clip": cid.0, "lane": lane, "step": step, "on": true, "velocity": velocity}));
    } else {
        out.run("pattern.step", json!({"clip": cid.0, "lane": lane, "step": step, "on": false}));
    }
}

/// Diagonal hatching over `r`, `every` px apart.
fn hatch(p: &egui::Painter, r: Rect, every: f32, c: Color32) {
    let p = p.with_clip_rect(r.intersect(p.clip_rect()));
    let mut x = r.min.x - r.height();
    while x < r.max.x {
        p.line_segment([pos2(x, r.max.y), pos2(x + r.height(), r.min.y)], Stroke::new(1.5, c));
        x += every;
    }
}

/// A step as a raised key: its fill height the velocity in the lane's colour, a cream cap for an
/// accent, hatching and a percentage for a chance, ticks for a roll, ◂ for a flam, a dark badge for
/// a condition. Hatched and dim where a shorter lane repeats.
fn draw_step(p: &egui::Painter, cell: Rect, step: &Step, on: bool, color: Color32, ghost: bool) {
    if ghost {
        p.rect_filled(cell, 6.0, Color32::from_rgb(0x2F, 0x28, 0x22));
        hatch(p, cell.shrink(1.0), 8.0, Color32::from_black_alpha(38));
    } else {
        p.rect_filled(cell.translate(vec2(0.0, 1.0)), 6.0, Color32::from_black_alpha(90));
        let top = Rect::from_min_max(cell.min, pos2(cell.max.x, cell.center().y));
        p.rect_filled(cell, 6.0, Color32::from_rgb(0x46, 0x3C, 0x33));
        p.rect_filled(top, egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, Color32::from_rgb(0x50, 0x45, 0x39));
        p.hline(cell.min.x + 3.0..=cell.max.x - 3.0, cell.min.y + 1.0, Stroke::new(1.0, Color32::from_white_alpha(20)));
    }
    p.rect_stroke(cell, 6.0, Stroke::new(1.0, DARK), StrokeKind::Inside);
    if !on {
        return;
    }
    let a = if ghost { 0.4 } else { 1.0 } * if step.probability < 100 { 0.75 } else { 1.0 };
    let fill_h = ((cell.height() - 6.0) * (f32::from(step.velocity) / 127.0)).max(6.0);
    let fill = Rect::from_min_max(pos2(cell.min.x + 3.0, cell.max.y - 3.0 - fill_h), pos2(cell.max.x - 3.0, cell.max.y - 3.0));
    p.rect_filled(fill, 4.0, color.gamma_multiply(a));
    if step.accent {
        p.rect_filled(
            Rect::from_min_size(fill.min, vec2(fill.width(), 3.0)),
            egui::CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 },
            CREAM.gamma_multiply(a),
        );
    } else {
        p.hline(fill.min.x + 2.0..=fill.max.x - 2.0, fill.min.y + 0.5, Stroke::new(1.0, Color32::from_white_alpha(77).gamma_multiply(a)));
    }
    if step.probability < 100 {
        hatch(p, fill, 6.0, DARK.gamma_multiply(0.25));
    }
    if step.roll > 1 {
        let n = f32::from(step.roll);
        for k in 0..step.roll {
            let x = cell.min.x + cell.width() * (f32::from(k) + 0.5) / n;
            p.vline(x, cell.max.y - 12.0..=cell.max.y - 6.0, Stroke::new(1.5, DARK));
        }
    }
    // The badge: a condition (dark, in the lane's colour), else a chance, else a flam mark.
    let badge = condition_badge(step.condition)
        .map(|b| (b, true))
        .or_else(|| (step.probability < 100).then(|| (format!("{}%", step.probability), false)))
        .or_else(|| step.flam.then(|| ("◂".to_string(), false)));
    if let Some((b, dark)) = badge
        && cell.width() >= 18.0
    {
        let galley = p.layout_no_wrap(b, bold(10.0), if dark { color } else { DARK });
        let br = Rect::from_min_size(pos2(cell.min.x + 3.0, cell.min.y + 3.0), galley.size() + vec2(6.0, 3.0));
        if dark {
            p.rect_filled(br, 3.0, DARK);
        }
        p.with_clip_rect(cell).galley(br.min + vec2(3.0, 1.5), galley, CREAM);
    }
}

/// The open step's details, in a popover hanging from the step: velocity, probability and nudge
/// sliders, Roll, Accent and Flam, and when the step plays.
fn details_popover(ui: &mut Ui, cid: ClipId, pattern: &Pattern, juke: Option<&Insert>, st: &mut State, out: &mut Out) {
    let Some((lane_i, step_i)) = st.details else { return };
    let Some((lane, step)) = pattern.lanes.get(lane_i).and_then(|l| l.steps.get(step_i).map(|s| (l, *s))) else {
        st.details = None;
        return;
    };
    let Some(anchor) = st.anchor else { return };
    let look = lane_look(juke, lane.note);
    let key = |what: &str| format!("pattern_step:{}:{lane_i}:{step_i}:{what}", cid.0);
    let base = json!({"clip": cid.0, "lane": lane_i, "step": step_i});
    let with = |k: &str, v: Value| {
        let mut p = base.clone();
        p[k] = v;
        p
    };
    let spb = usize::from(pattern.steps_per_bar.max(1));
    let beat = (spb / 4).max(1);
    let (bar, beat_n) = (step_i / spb + 1, (step_i % spb) / beat + 1);
    let screen = ui.ctx().content_rect();
    const POP: Vec2 = vec2(300.0, 290.0);
    let below = anchor.max.y + 6.0 + POP.y < screen.max.y;
    let (pivot, at) =
        if below { (Align2::LEFT_TOP, anchor.left_bottom() + vec2(0.0, 6.0)) } else { (Align2::LEFT_BOTTOM, anchor.left_top() - vec2(0.0, 6.0)) };
    let mut close = ui.input(|i| i.key_pressed(egui::Key::Escape));
    let area =
        egui::Area::new(egui::Id::new(("step-popover", cid.0))).order(egui::Order::Foreground).pivot(pivot).fixed_pos(at).constrain(true).show(
            ui.ctx(),
            |ui| {
                egui::Frame::new()
                    .fill(GROUND)
                    .stroke(Stroke::new(1.0, WELL))
                    .corner_radius(10.0)
                    .shadow(egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(140) })
                    .show(ui, |ui| {
                        ui.set_width(POP.x);
                        ui.spacing_mut().item_spacing = vec2(6.0, 8.0);
                        // The band: lane and step, on/off, close.
                        let (band, _) = ui.allocate_exact_size(vec2(POP.x, 36.0), Sense::hover());
                        let p = ui.painter();
                        p.rect_filled(band, egui::CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 }, look.color);
                        let t = p.text(
                            pos2(band.min.x + 12.0, band.center().y),
                            Align2::LEFT_CENTER,
                            format!("{} · step {}", look.name, step_i + 1),
                            bold(13.0),
                            DARK,
                        );
                        p.text(
                            pos2(t.max.x + 8.0, band.center().y),
                            Align2::LEFT_CENTER,
                            format!("bar {bar}, beat {beat_n}"),
                            regular(11.0),
                            DARK.gamma_multiply(0.7),
                        );
                        let x_r = Rect::from_center_size(pos2(band.max.x - 18.0, band.center().y), vec2(22.0, 22.0));
                        let resp = ui.interact(x_r, egui::Id::new(("step-popover-close", cid.0)), Sense::click()).on_hover_text("Close (Esc)");
                        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close the step details"));
                        let p = ui.painter();
                        if resp.hovered() {
                            p.rect_filled(x_r, 4.0, Color32::from_black_alpha(30));
                        }
                        let c = x_r.center();
                        for d in [vec2(5.0, 5.0), vec2(5.0, -5.0)] {
                            p.line_segment([c - d, c + d], Stroke::new(1.8, DARK));
                        }
                        if resp.clicked() {
                            close = true;
                        }
                        let on_r = Rect::from_min_size(pos2(x_r.min.x - 46.0, band.center().y - 11.0), vec2(40.0, 22.0));
                        let label = if step.on { "On" } else { "Off" };
                        let resp = ui.put(
                            on_r,
                            egui::Button::new(egui::RichText::new(label).font(bold(11.0)).color(if step.on { CREAM } else { DARK }))
                                .fill(if step.on { DARK } else { Color32::from_black_alpha(30) })
                                .corner_radius(11.0),
                        );
                        if resp.on_hover_text("Turn the step on or off").clicked() {
                            out.run("pattern.step", with("on", json!(!step.on)));
                        }
                        ui.add_space(2.0);
                        egui::Frame::new().inner_margin(egui::Margin { left: 12, right: 12, top: 0, bottom: 12 }).show(ui, |ui| {
                            let sliders: [(&str, &str, i32, i32, i32, i32, String); 3] = [
                                ("Velocity", "velocity", i32::from(step.velocity), 1, 127, 1, step.velocity.to_string()),
                                ("Probability", "probability", i32::from(step.probability), 0, 100, 0, format!("{}%", step.probability)),
                                ("Nudge", "offset", i32::from(step.offset), -50, 50, 0, format!("{:+}%", step.offset)),
                            ];
                            for (label, k, v, lo, hi, from, shown) in sliders {
                                if let Some(nv) = slider_row(ui, label, v, lo, hi, from, &shown) {
                                    let mut p = with(k, json!(nv));
                                    if !step.on {
                                        p["on"] = json!(true);
                                    }
                                    out.merged("pattern.step", p, key(k));
                                }
                            }
                            ui.horizontal(|ui| {
                                row_label(ui, "Roll");
                                let items: Vec<(String, f32)> = (1..=4).map(|n| (n.to_string(), 30.0)).collect();
                                let (r, _) = ui.allocate_exact_size(vec2(segmented_w(&items), 28.0), Sense::hover());
                                if let Some(i) = segmented(
                                    ui,
                                    r,
                                    egui::Id::new(("roll", cid.0)),
                                    &items,
                                    Some(usize::from(step.roll.clamp(1, 4) - 1)),
                                    "Hits the step plays, evenly spaced",
                                ) {
                                    out.run("pattern.step", with("roll", json!(i + 1)));
                                }
                            });
                            ui.horizontal(|ui| {
                                row_label(ui, "Hit");
                                if pill(ui, "Accent", step.accent).on_hover_text("Adds the pattern's accent to the velocity").clicked() {
                                    out.run("pattern.step", with("accent", json!(!step.accent)));
                                }
                                if pill(ui, "Flam", step.flam).on_hover_text("A quieter grace hit just before").clicked() {
                                    out.run("pattern.step", with("flam", json!(!step.flam)));
                                }
                            });
                            ui.horizontal_top(|ui| {
                                row_label(ui, "Plays");
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                                    let mut choices: Vec<(Condition, String)> = PLAYS.iter().map(|(c, l)| (*c, (*l).to_string())).collect();
                                    if !PLAYS.iter().any(|(c, _)| *c == step.condition) {
                                        choices.push((step.condition, condition_badge(step.condition).unwrap_or_default()));
                                    }
                                    ui.horizontal_wrapped(|ui| {
                                        for (c, label) in choices {
                                            if pill(ui, &label, step.condition == c).clicked() && step.condition != c {
                                                out.run("pattern.step", with("condition", condition_arg(c)));
                                            }
                                        }
                                    });
                                });
                            });
                        });
                    });
            },
        );
    let outside =
        ui.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p) && !anchor.contains(p)));
    if close || outside {
        st.details = None;
        st.anchor = None;
    }
}

/// A row's name in the popover, left of its controls.
fn row_label(ui: &mut Ui, s: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(64.0, 26.0), Sense::hover());
    ui.painter().text(pos2(r.min.x, r.center().y), Align2::LEFT_CENTER, s, regular(11.0), LABEL);
}

/// A pill toggle.
fn pill(ui: &mut Ui, label: &str, on: bool) -> egui::Response {
    let text = egui::RichText::new(label).font(bold(12.0)).color(if on { DARK } else { CREAM });
    let resp = ui.add(
        egui::Button::new(text)
            .fill(if on { AMBER } else { Color32::from_black_alpha(38) })
            .stroke(Stroke::new(1.0, Color32::from_black_alpha(89)))
            .corner_radius(13.0)
            .min_size(vec2(0.0, 26.0)),
    );
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, label));
    resp
}

/// A labelled slider row: an amber run from `from` to the value under a cream cap. Returns the new
/// value while dragged.
fn slider_row(ui: &mut Ui, label: &str, v: i32, lo: i32, hi: i32, from: i32, shown: &str) -> Option<i32> {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let p = ui.painter().clone();
    p.text(pos2(row.min.x, row.center().y), Align2::LEFT_CENTER, label, regular(11.0), LABEL);
    p.text(pos2(row.max.x, row.center().y), Align2::RIGHT_CENTER, shown, bold(12.0), CREAM);
    let track = Rect::from_min_max(pos2(row.min.x + 70.0, row.center().y - 3.0), pos2(row.max.x - 46.0, row.center().y + 3.0));
    let span = (hi - lo).max(1) as f32;
    let f = |x: i32| ((x - lo) as f32 / span).clamp(0.0, 1.0);
    let at = |t: f32| track.min.x + track.width() * t;
    p.rect_filled(track, 3.0, WELL);
    let (a, b) = (at(f(from).min(f(v))), at(f(from).max(f(v))));
    p.rect_filled(Rect::from_min_max(pos2(a, track.min.y), pos2(b, track.max.y)), 3.0, AMBER.gamma_multiply(0.7));
    let cap = Rect::from_center_size(pos2(at(f(v)), track.center().y), vec2(12.0, 18.0));
    p.rect_filled(cap.translate(vec2(0.0, 2.0)), 3.0, Color32::from_black_alpha(90));
    p.rect_filled(cap, 3.0, Color32::from_rgb(0xE6, 0xD9, 0xC2));
    p.rect_stroke(cap, 3.0, Stroke::new(1.0, Color32::from_rgb(0x3A, 0x2E, 0x22)), StrokeKind::Inside);
    let hit = Rect::from_min_max(pos2(track.min.x - 6.0, row.min.y), pos2(track.max.x + 6.0, row.max.y));
    let resp = ui.interact(hit, ui.id().with(("slider", label)), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::slider(true, f64::from(v), label));
    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let t = ((pos.x - track.min.x) / track.width().max(1.0)).clamp(0.0, 1.0);
        let nv = (lo as f32 + t * span).round() as i32;
        return (nv != v).then_some(nv);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_round_trip_through_their_command_form() {
        for c in [Condition::Always, Condition::First, Condition::NotFirst, Condition::Fill, Condition::NotFill, Condition::Every { nth: 3, of: 4 }] {
            let arg = condition_arg(c);
            assert!(arg.is_string(), "{arg}");
        }
        assert_eq!(condition_arg(Condition::Every { nth: 3, of: 4 }), json!("3:4"));
        assert_eq!(condition_badge(Condition::Always), None);
    }

    #[test]
    fn tiers_run_full_to_quarter() {
        assert_eq!((0..4).map(tier_velocity).collect::<Vec<_>>(), vec![127, 95, 64, 32]);
    }

    #[test]
    fn columns_clamp_to_the_grid_and_gap_between_beats() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 20.0));
        let c = Columns::new(r, 16, 4);
        assert_eq!(c.at(-5.0), 0);
        assert_eq!(c.at(c.x(1) + 1.0), 1);
        assert_eq!(c.at(500.0), 15);
        // The gap after a beat is wider than the gap inside one.
        let inside = c.x(2) - (c.x(1) + c.w);
        let between = c.x(4) - (c.x(3) + c.w);
        assert!(between > inside, "{between} > {inside}");
        assert!(c.x(15) + c.w <= r.max.x + 0.01);
    }

    #[test]
    fn lane_extras_name_choke_rate_and_direction() {
        let mut lane = Lane::new(42, 16);
        assert_eq!(lane_extra(None, &lane), "");
        lane.rate = Rate::Half;
        assert_eq!(lane_extra(Some(1), &lane), "Choke 1 · ½ rate");
        lane.direction = Direction::PingPong;
        assert_eq!(lane_extra(None, &lane), "½ rate · Ping-pong");
    }

    #[test]
    fn the_plays_pills_are_the_mockups() {
        let names: Vec<_> = PLAYS.iter().map(|p| p.1).collect();
        assert_eq!(names, ["Always", "1st of 2", "2nd of 2", "First pass", "Fill bars", "Not in fills"]);
    }
}
