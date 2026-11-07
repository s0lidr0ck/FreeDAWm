//! The step grid: a pattern clip in the MIDI Editor (look approved 2026-11-06, `docs/juke.md` §5).
//!
//! The pattern bar (steps per bar, the **New steps** velocity, shuffle, accent, humanize, fills,
//! Re-roll, Convert to MIDI) over one row per lane: a header band in the lane's pad colour, then
//! its steps as raised keys grouped by beat.
//!
//! * Click a step to turn it on or off; drag across steps to paint or clear a run (the first step
//!   decides which). New steps take the New steps velocity.
//! * Drag a lit step up or down for fine velocity; keys 1–4 over a step set Full, 75, 50 or 25 %.
//! * Right-click a step for its details (velocity, probability, nudge, roll, accent, flam, when it
//!   plays); right-click a lane's header for its rate, direction, note and removal.
//!
//! Every edit is a `pattern.*` command, so it undoes and scripts like any other.

use crate::SoundApp;
use crate::theme::{Tokens, bold, mono, regular};
use egui::{Align2, Color32, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_midi::pattern::{Condition, Direction, Lane, Pattern, Rate, Step};
use soundcraft_model::{Clip, ClipId, DrumFamily, Insert, TrackId};

/// The New steps velocity tiers: Full, 75, 50 and 25 %.
pub const TIERS: [(&str, u8); 4] = [("Full", 127), ("75", 95), ("50", 64), ("25", 32)];

const LANE_H: f32 = 26.0;
const HEADER_W: f32 = 150.0;
const BAR_H: f32 = 30.0;
const CREAM: Color32 = Color32::from_rgb(0xF6, 0xEE, 0xDF);
const AMBER: Color32 = Color32::from_rgb(0xF0, 0xB2, 0x4A);
const DARK: Color32 = Color32::from_rgb(0x1E, 0x19, 0x15);

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
}

fn state_id(cid: ClipId) -> egui::Id {
    egui::Id::new(("step-grid", cid.0))
}

/// The lane's name and colour: the Juke pad its note triggers, or the General MIDI drum.
fn lane_look(juke: Option<&Insert>, note: u8) -> (String, Color32) {
    if let Some(ins) = juke
        && let Some(pad) = crate::juke_window::pad_for_note(ins, note)
    {
        let layout = soundcraft_engine::cmd::juke::pad_layout(ins, pad);
        return (layout.name, crate::juke_window::family_color(layout.family));
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
    (name, crate::juke_window::family_color(family))
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

/// Draws clip `cid`'s step grid in `r` (the MIDI Editor's body). `clip` must be a pattern clip.
pub fn show(app: &mut SoundApp, ui: &mut Ui, r: Rect, track: TrackId, cid: ClipId, clip: &Clip) {
    let Some(pc) = clip.pattern.as_ref() else { return };
    let pattern = &pc.pattern;
    let ctx = ui.ctx().clone();
    let mut st: State = ctx.data(|d| d.get_temp(state_id(cid))).unwrap_or_default();
    let mut out = Out::default();
    let t = Tokens::current();
    ui.painter().rect_filled(r, 0.0, t.panel_bg);
    let juke = app.engine.session().track(track).and_then(|t| t.instrument.clone()).filter(|i| i.plugin == soundcraft_engine::cmd::juke::JUKE);
    let bar = Rect::from_min_size(r.min, vec2(r.width(), BAR_H));
    pattern_bar(ui, bar, cid, clip, pattern, &mut st, &mut out);
    let playhead = playhead_steps(app, clip, pc.phase, pattern);
    let body = Rect::from_min_max(pos2(r.min.x, bar.max.y + 4.0), r.max);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
    egui::ScrollArea::vertical().id_salt(("step-lanes", cid.0)).auto_shrink([false, false]).show(&mut child, |ui| {
        let columns = pattern.lanes.iter().map(|l| l.steps.len()).max().unwrap_or(16).max(1);
        let h = (pattern.lanes.len() as f32 + 1.0) * LANE_H + 4.0;
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
        for (i, lane) in pattern.lanes.iter().enumerate() {
            let row = Rect::from_min_size(pos2(area.min.x, area.min.y + i as f32 * LANE_H), vec2(area.width(), LANE_H - 3.0));
            let look = lane_look(juke.as_ref(), lane.note);
            lane_header(ui, Rect::from_min_size(row.min, vec2(HEADER_W, row.height())), cid, i, lane, &look, &mut out);
            let steps = Rect::from_min_max(pos2(row.min.x + HEADER_W + 6.0, row.min.y), row.max);
            lane_steps(ui, steps, cid, i, lane, columns, pattern.steps_per_bar, look.1, playhead.get(i).copied().flatten(), &mut st, &mut out);
        }
        // Add a lane: the next pad's note not in the pattern yet.
        let add = Rect::from_min_size(pos2(area.min.x, area.min.y + pattern.lanes.len() as f32 * LANE_H), vec2(HEADER_W, LANE_H - 3.0));
        let used: Vec<u8> = pattern.lanes.iter().map(|l| l.note).collect();
        let next = match &juke {
            Some(ins) => (0..soundcraft_dsp::plugins::juke::PADS).map(|p| crate::juke_window::pad_note(ins, p)).find(|n| !used.contains(n)),
            None => (36..=81).find(|n| !used.contains(n)),
        };
        let resp = ui.put(add, egui::Button::new("+ Lane"));
        if resp.clicked() {
            out.run("pattern.lane_add", json!({"clip": cid.0, "note": next.unwrap_or(36)}));
        }
    });
    details_popover(ui, cid, pattern, &mut st, &mut out);
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

#[allow(clippy::too_many_arguments)]
fn pattern_bar(ui: &mut Ui, r: Rect, cid: ClipId, clip: &Clip, pattern: &Pattern, st: &mut State, out: &mut Out) {
    let t = Tokens::current();
    ui.painter().rect_filled(r, 0.0, t.panel_bg2);
    let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(vec2(8.0, 3.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let ui = &mut bar;
    ui.spacing_mut().item_spacing.x = 6.0;
    ui.label(egui::RichText::new("PATTERN").font(bold(11.0)).color(AMBER));
    ui.label(egui::RichText::new(&clip.name).font(regular(11.0)));
    ui.separator();
    // Steps per bar.
    let mut spb = pattern.steps_per_bar;
    egui::ComboBox::from_id_salt(("spb", cid.0)).width(70.0).selected_text(format!("{spb} / bar")).show_ui(ui, |ui| {
        for n in [8u8, 12, 16, 24, 32] {
            ui.selectable_value(&mut spb, n, format!("{n} steps"));
        }
    });
    if spb != pattern.steps_per_bar {
        out.run("pattern.settings", json!({"clip": cid.0, "steps_per_bar": spb}));
    }
    ui.separator();
    // New steps velocity.
    ui.label(egui::RichText::new("New steps").font(regular(11.0)).color(t.text_dim));
    for (i, (name, _)) in TIERS.iter().enumerate() {
        let label = if i == 0 { (*name).to_string() } else { format!("{name}%") };
        if ui.selectable_label(st.tier == i, label).on_hover_text("The velocity a step takes when it is turned on (keys 1–4 set a step's)").clicked()
        {
            st.tier = i;
        }
    }
    ui.separator();
    // Shuffle, accent, humanize, fills: drag values, each edit one undo step per drag.
    let mut field = |ui: &mut Ui, label: &str, key: &str, v: u8, max: u8, suffix: &str| {
        ui.label(egui::RichText::new(label).font(regular(11.0)).color(t.text_dim));
        let mut x = i32::from(v);
        let resp = ui.add(egui::DragValue::new(&mut x).range(0..=i32::from(max)).suffix(suffix));
        if resp.changed() && x != i32::from(v) {
            let mut p = json!({"clip": cid.0});
            p[key] = json!(x);
            out.merged("pattern.settings", p, format!("pattern_settings:{}:{key}", cid.0));
        }
    };
    field(ui, "Shuffle", "shuffle", pattern.shuffle, 100, "%");
    field(ui, "Accent", "accent", pattern.accent, 127, "");
    field(ui, "Humanize", "humanize", pattern.humanize, 100, "%");
    field(ui, "Fill every", "fill_every", pattern.fill_every, 16, " bars");
    ui.separator();
    if ui.button("Re-roll").on_hover_text("Draw every chance step again").clicked() {
        out.run("pattern.reroll", json!({"clip": cid.0}));
    }
    if ui.button("Convert to MIDI").on_hover_text("Keep the notes as a plain MIDI clip, to edit them by hand").clicked() {
        out.run("pattern.to_midi", json!({"clip": cid.0}));
    }
}

fn lane_header(ui: &mut Ui, r: Rect, cid: ClipId, i: usize, lane: &Lane, (name, color): &(String, Color32), out: &mut Out) {
    let p = ui.painter().clone();
    let c = if lane.mute { color.gamma_multiply(0.45) } else { *color };
    p.rect_filled(r, 3.0, c);
    p.text(pos2(r.min.x + 6.0, r.center().y), Align2::LEFT_CENTER, name, bold(11.0), DARK);
    // Length, as a drag value; mute as a chip.
    let len_r = Rect::from_min_size(pos2(r.max.x - 66.0, r.min.y + 3.0), vec2(36.0, r.height() - 6.0));
    let mut len = lane.steps.len() as i32;
    let resp = ui.put(len_r, egui::DragValue::new(&mut len).range(1..=64)).on_hover_text("The lane's length in steps");
    if resp.changed() && len != lane.steps.len() as i32 {
        out.merged("pattern.lane", json!({"clip": cid.0, "lane": i, "length": len}), format!("pattern_lane_len:{}:{i}", cid.0));
    }
    let mute_r = Rect::from_min_size(pos2(r.max.x - 26.0, r.min.y + 3.0), vec2(22.0, r.height() - 6.0));
    let resp = ui.interact(mute_r, egui::Id::new(("lane-mute", cid.0, i)), Sense::click()).on_hover_text("Mute the lane");
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, lane.mute, "Mute the lane"));
    p.rect_filled(mute_r, 3.0, if lane.mute { DARK } else { Color32::from_black_alpha(40) });
    p.text(mute_r.center(), Align2::CENTER_CENTER, "M", bold(10.0), if lane.mute { *color } else { DARK });
    if resp.clicked() {
        out.run("pattern.lane", json!({"clip": cid.0, "lane": i, "mute": !lane.mute}));
    }
    // Rate and direction marks.
    let mut marks = Vec::new();
    match lane.rate {
        Rate::Half => marks.push("½×"),
        Rate::Double => marks.push("2×"),
        Rate::Normal => {}
    }
    match lane.direction {
        Direction::Reverse => marks.push("◂"),
        Direction::PingPong => marks.push("↔"),
        Direction::Random => marks.push("?"),
        Direction::Forward => {}
    }
    if !marks.is_empty() {
        p.text(pos2(len_r.min.x - 4.0, r.center().y), Align2::RIGHT_CENTER, marks.join(" "), regular(10.0), DARK);
    }
    let head = ui.interact(Rect::from_min_max(r.min, pos2(len_r.min.x - 2.0, r.max.y)), egui::Id::new(("lane-head", cid.0, i)), Sense::click());
    head.context_menu(|ui| {
        ui.label(egui::RichText::new(format!("{name} · {}", soundcraft_midi::note_name(lane.note))).strong());
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

/// Which step column the x position falls in.
fn column_at(r: Rect, columns: usize, x: f32) -> usize {
    let w = r.width() / columns.max(1) as f32;
    (((x - r.min.x) / w).floor().max(0.0) as usize).min(columns.saturating_sub(1))
}

#[allow(clippy::too_many_arguments)]
fn lane_steps(
    ui: &mut Ui,
    r: Rect,
    cid: ClipId,
    lane_i: usize,
    lane: &Lane,
    columns: usize,
    steps_per_bar: u8,
    color: Color32,
    playhead: Option<usize>,
    st: &mut State,
    out: &mut Out,
) {
    let len = lane.steps.len().max(1);
    let w = r.width() / columns.max(1) as f32;
    let beat = usize::from((steps_per_bar / 4).max(1));
    let p = ui.painter().clone();
    let resp = ui.interact(r, egui::Id::new(("lane-steps", cid.0, lane_i)), Sense::click_and_drag());
    let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
    // The painted run, while painting.
    let painting = match st.drag {
        Some(Drag::Paint { lane: l, from, to, on }) if l == lane_i => Some((from.min(to), from.max(to), on)),
        _ => None,
    };
    for col in 0..columns {
        let step_i = col % len;
        let ghost = col >= len;
        let Some(step) = lane.steps.get(step_i) else { continue };
        let group = (col / beat) % 2 == 1;
        let cell = Rect::from_min_size(pos2(r.min.x + col as f32 * w, r.min.y), vec2(w - 2.0, r.height())).shrink2(vec2(0.0, 1.0));
        let on = match painting {
            Some((a, b, on)) if !ghost && (a..=b).contains(&col) => on,
            _ => step.on,
        };
        draw_step(&p, cell, step, on, color, group, ghost);
        if !ghost && playhead == Some(step_i) {
            p.rect_stroke(cell.expand(1.0), 3.0, Stroke::new(1.5, CREAM), StrokeKind::Outside);
        }
    }
    // Gestures.
    if resp.drag_started()
        && let Some(pos) = pointer
    {
        let col = column_at(r, columns, pos.x);
        if col < len {
            st.drag = Some(Drag::Pressed { lane: lane_i, step: col, on: !lane.steps[col].on });
        }
    }
    if resp.dragged()
        && let Some(pos) = pointer
    {
        let col = column_at(r, columns, pos.x).min(len - 1);
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
        let col = column_at(r, columns, pos.x);
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
        let col = column_at(r, columns, pos.x);
        if col < len {
            st.details = Some((lane_i, col));
        }
    }
    // Keys 1–4 over a step set its velocity tier (and turn it on).
    if let Some(pos) = resp.hover_pos() {
        let col = column_at(r, columns, pos.x);
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

/// A step as a raised key: its fill height the velocity in the lane's colour, a cream cap for an
/// accent, hatching and a percentage for a chance, ticks for a roll, ◂ for a flam, a dark badge for
/// a condition. Ghosted where a shorter lane repeats.
fn draw_step(p: &egui::Painter, cell: Rect, step: &Step, on: bool, color: Color32, group: bool, ghost: bool) {
    let base = if group { Color32::from_rgb(0x3A, 0x33, 0x2C) } else { Color32::from_rgb(0x46, 0x3D, 0x35) };
    let a = if ghost { 0.35 } else { 1.0 };
    p.rect_filled(cell.translate(vec2(0.0, 1.0)), 3.0, Color32::from_black_alpha(90).gamma_multiply(a));
    p.rect_filled(cell, 3.0, base.gamma_multiply(a));
    p.hline(cell.min.x + 2.0..=cell.max.x - 2.0, cell.min.y + 1.0, Stroke::new(1.0, Color32::from_white_alpha(22).gamma_multiply(a)));
    if !on {
        return;
    }
    let fill_h = (cell.height() - 4.0) * (f32::from(step.velocity) / 127.0).clamp(0.08, 1.0);
    let fill = Rect::from_min_max(pos2(cell.min.x + 2.0, cell.max.y - 2.0 - fill_h), pos2(cell.max.x - 2.0, cell.max.y - 2.0));
    p.rect_filled(fill, 2.0, color.gamma_multiply(a));
    if step.accent {
        p.rect_filled(Rect::from_min_size(pos2(cell.min.x + 2.0, cell.min.y + 2.0), vec2(cell.width() - 4.0, 3.0)), 1.0, CREAM.gamma_multiply(a));
    }
    if step.probability < 100 {
        let mut x = fill.min.x - fill.height();
        while x < fill.max.x {
            let a0 = pos2(x.max(fill.min.x), fill.max.y - (x.max(fill.min.x) - x));
            let x1 = (x + fill.height()).min(fill.max.x);
            let a1 = pos2(x1, fill.max.y - (x1 - x));
            p.line_segment([a0, a1], Stroke::new(1.0, DARK.gamma_multiply(0.5)));
            x += 4.0;
        }
        if cell.width() >= 22.0 {
            p.text(cell.center(), Align2::CENTER_CENTER, format!("{}%", step.probability), mono(8.5), DARK);
        }
    }
    if step.roll > 1 {
        for k in 0..step.roll {
            let x = cell.min.x + 3.0 + f32::from(k) * 3.0;
            p.vline(x, cell.min.y + 6.0..=cell.min.y + 10.0, Stroke::new(1.0, CREAM));
        }
    }
    if step.flam {
        p.add(Shape::convex_polygon(
            vec![
                pos2(cell.min.x + 2.0, cell.center().y),
                pos2(cell.min.x + 6.0, cell.center().y - 3.0),
                pos2(cell.min.x + 6.0, cell.center().y + 3.0),
            ],
            DARK,
            Stroke::NONE,
        ));
    }
    if let Some(b) = condition_badge(step.condition) {
        let br = Rect::from_min_size(pos2(cell.max.x - 16.0, cell.min.y + 1.0), vec2(15.0, 9.0));
        p.rect_filled(br, 2.0, DARK);
        p.text(br.center(), Align2::CENTER_CENTER, b, regular(7.0), CREAM);
    }
}

/// The open step's details: velocity, probability, nudge, roll, accent, flam and when it plays.
fn details_popover(ui: &mut Ui, cid: ClipId, pattern: &Pattern, st: &mut State, out: &mut Out) {
    let Some((lane_i, step_i)) = st.details else { return };
    let Some(step) = pattern.lanes.get(lane_i).and_then(|l| l.steps.get(step_i)).copied() else {
        st.details = None;
        return;
    };
    let mut open = true;
    let key = |what: &str| format!("pattern_step:{}:{lane_i}:{step_i}:{what}", cid.0);
    let base = json!({"clip": cid.0, "lane": lane_i, "step": step_i});
    let with = |k: &str, v: Value| {
        let mut p = base.clone();
        p[k] = v;
        p
    };
    egui::Window::new(format!("Step {} · lane {}", step_i + 1, lane_i + 1))
        .id(egui::Id::new(("step-details", cid.0)))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_pos(ui.ctx().pointer_latest_pos().unwrap_or_default())
        .show(ui.ctx(), |ui| {
            egui::Grid::new(("step-details-grid", cid.0)).num_columns(2).show(ui, |ui| {
                ui.label("On");
                let mut on = step.on;
                if ui.checkbox(&mut on, "").changed() {
                    out.run("pattern.step", with("on", json!(on)));
                }
                ui.end_row();
                let mut slider = |ui: &mut Ui, label: &str, k: &str, v: i32, range: std::ops::RangeInclusive<i32>, suffix: &str| {
                    ui.label(label);
                    let mut x = v;
                    if ui.add(egui::Slider::new(&mut x, range).suffix(suffix)).changed() && x != v {
                        out.merged("pattern.step", with(k, json!(x)), key(k));
                    }
                    ui.end_row();
                };
                slider(ui, "Velocity", "velocity", i32::from(step.velocity), 1..=127, "");
                slider(ui, "Chance", "probability", i32::from(step.probability), 0..=100, "%");
                slider(ui, "Nudge", "offset", i32::from(step.offset), -50..=50, "%");
                slider(ui, "Roll", "roll", i32::from(step.roll), 1..=4, "×");
                ui.label("Accent");
                let mut accent = step.accent;
                if ui.checkbox(&mut accent, "").changed() {
                    out.run("pattern.step", with("accent", json!(accent)));
                }
                ui.end_row();
                ui.label("Flam");
                let mut flam = step.flam;
                if ui.checkbox(&mut flam, "").changed() {
                    out.run("pattern.step", with("flam", json!(flam)));
                }
                ui.end_row();
                ui.label("Plays");
                let choices = [
                    (Condition::Always, "Always"),
                    (Condition::First, "First time only"),
                    (Condition::NotFirst, "Not the first time"),
                    (Condition::Fill, "In fills"),
                    (Condition::NotFill, "Not in fills"),
                    (Condition::Every { nth: 1, of: 2 }, "1st of every 2"),
                    (Condition::Every { nth: 2, of: 2 }, "2nd of every 2"),
                    (Condition::Every { nth: 1, of: 4 }, "1st of every 4"),
                    (Condition::Every { nth: 4, of: 4 }, "4th of every 4"),
                ];
                let shown = choices
                    .iter()
                    .find(|c| c.0 == step.condition)
                    .map_or_else(|| condition_badge(step.condition).unwrap_or_default(), |c| c.1.to_string());
                egui::ComboBox::from_id_salt(("step-cond", cid.0)).selected_text(shown).show_ui(ui, |ui| {
                    for (c, label) in choices {
                        if ui.selectable_label(step.condition == c, label).clicked() {
                            out.run("pattern.step", with("condition", condition_arg(c)));
                        }
                    }
                });
                ui.end_row();
            });
        });
    if !open {
        st.details = None;
    }
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
    fn columns_clamp_to_the_grid() {
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(160.0, 20.0));
        assert_eq!(column_at(r, 16, -5.0), 0);
        assert_eq!(column_at(r, 16, 15.0), 1);
        assert_eq!(column_at(r, 16, 500.0), 15);
    }
}
