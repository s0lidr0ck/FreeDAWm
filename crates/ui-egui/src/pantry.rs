//! Pantry - Multisampler's editor, in the Juke family's look (see [`crate::lift`]): a walnut
//! ground, tobacco cards, cream knobs and one amber accent.
//!
//! The page: a title bar (the Pantry wordmark, what is on the shelves, Load…, Save… and Clear); the
//! shelves (every sample as a block over the keys it plays, left to right, and the velocities it
//! answers, bottom to top, the part of a layer that blends with the one below shaded; press a
//! block to hear that key at that velocity on the Musical Typing port); and a side card with the
//! Dynamics, Envelope, Pitch and Output knobs. Every control runs `mix.instrument_param` or a
//! multisample command, so it undoes and scripts like any edit.

use crate::SoundApp;
use crate::lift::{AMBER, CREAM, DIM, INK, TEXT, TOBACCO_HI, WALNUT, WELL, WELL_GRID, button, card, paint_knob, section};
use crate::theme::{bold, regular};
use egui::{Align2, Color32, CornerRadius, CursorIcon, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::ParamInfo;
use soundcraft_dsp::plugins::multisampler::{ON_KEY_DOWN, PART_OFF_DB, PARTS, ZoneSpec, parse_sample_key};
use soundcraft_model::{Insert, TrackId};
use std::collections::BTreeMap;

/// The Multisampler's plugin id.
pub const PANTRY: &str = "multisampler";

/// The editor's size: the window opens at this width.
pub const W: f32 = 1000.0;
const H: f32 = 600.0;

fn info_of(id: &str) -> Option<&'static ParamInfo> {
    soundcraft_dsp::plugin_info(PANTRY).and_then(|i| i.param(id))
}

fn param(ins: &Insert, id: &str) -> f32 {
    ins.params.get(id).copied().unwrap_or_else(|| info_of(id).map_or(0.0, |p| p.default))
}

/// Sets one parameter (merged into one undo step while dragging).
fn set(app: &mut SoundApp, tid: TrackId, id: &str, value: f32) {
    if let Err(e) = app.engine.execute_merged(
        "mix.instrument_param",
        &json!({"track": tid.0, "param": id, "value": value}),
        &format!("instrument_param:{}:{id}", tid.0),
    ) {
        app.ui.status = e.to_string();
    }
}

/// A recorded note's velocity layers, softest first: the velocity each was played at and how
/// many takes it holds.
pub type Layers = Vec<(u8, usize)>;

/// What is on the shelves: each recorded note with its layers, lowest note first.
pub fn shelves(ins: &Insert) -> BTreeMap<u8, Layers> {
    let mut takes: BTreeMap<(u8, u8), usize> = BTreeMap::new();
    for (note, velocity, _) in ins.samples.keys().filter_map(|k| parse_sample_key(k)) {
        *takes.entry((note as u8, velocity)).or_default() += 1;
    }
    let mut out: BTreeMap<u8, Layers> = BTreeMap::new();
    for ((note, velocity), n) in takes {
        out.entry(note).or_default().push((velocity, n));
    }
    out
}

/// The keys each recorded note plays: the nearest one's, the lower of two as near (the
/// instrument's own rule).
pub fn key_spans(notes: &[u8]) -> Vec<(u8, u8, u8)> {
    notes
        .iter()
        .enumerate()
        .map(|(i, &n)| {
            let lo = if i == 0 { 0 } else { notes.get(i - 1).map_or(0, |&p| ((u16::from(p) + u16::from(n)) / 2 + 1) as u8) };
            let hi = notes.get(i + 1).map_or(127, |&next| ((u16::from(n) + u16::from(next)) / 2) as u8);
            (n, lo, hi)
        })
        .collect()
}

/// A block on the shelves: the samples that answer the same keys and velocities. `note` is the
/// one they were recorded at, `below` the velocity under their range (0 at the bottom), `top`
/// the highest they answer, and `takes` how many take turns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub note: u8,
    pub lokey: u8,
    pub hikey: u8,
    pub below: u8,
    pub top: u8,
    pub takes: usize,
}

/// The instrument's parts that hold samples: each one's number (from 0) and name. A captured
/// instrument has none to show (it is all one part).
pub fn parts(ins: &Insert) -> Vec<(usize, String)> {
    let mut out: BTreeMap<usize, String> = BTreeMap::new();
    for z in ins.samples.keys().filter_map(|k| ZoneSpec::parse(k)) {
        out.entry(z.part).or_insert(z.part_name);
    }
    out.into_iter().map(|(i, name)| (i, if name.is_empty() { format!("Part {}", i + 1) } else { name })).collect()
}

/// The blocks to draw, and the part they are (`None`: a captured instrument's samples). Of a
/// library with several parts, the one with the most notes is drawn.
pub fn cells(ins: &Insert) -> (Vec<Cell>, Option<usize>) {
    let zones: Vec<ZoneSpec> = ins.samples.keys().filter_map(|k| ZoneSpec::parse(k)).filter(|z| z.trigger == ON_KEY_DOWN).collect();
    let main = (0..PARTS).max_by_key(|p| (zones.iter().filter(|z| z.part == *p).count(), PARTS - p)).filter(|_| !zones.is_empty());
    let mut out: Vec<Cell> = Vec::new();
    for z in zones.iter().filter(|z| Some(z.part) == main) {
        match out.iter_mut().find(|c| (c.lokey, c.hikey, c.below, c.top) == (z.lokey, z.hikey, z.lovel.saturating_sub(1), z.hivel)) {
            Some(cell) => cell.takes += usize::from(z.seq > 0),
            None => out.push(Cell { note: z.root, lokey: z.lokey, hikey: z.hikey, below: z.lovel.saturating_sub(1), top: z.hivel, takes: 1 }),
        }
    }
    if main.is_some() {
        return (out, main);
    }
    let shelves = shelves(ins);
    let recorded: Vec<u8> = shelves.keys().copied().collect();
    for (note, lokey, hikey) in key_spans(&recorded) {
        let mut below = 0;
        for &(velocity, takes) in shelves.get(&note).map(Vec::as_slice).unwrap_or(&[]) {
            out.push(Cell { note, lokey, hikey, below, top: velocity, takes });
            below = velocity;
        }
    }
    (out, None)
}

/// The whole editor.
pub fn editor(app: &mut SoundApp, ui: &mut Ui, tid: TrackId, ins: &Insert) {
    // A library's parts get a knob each under the others.
    let parts = parts(ins);
    let extra = if parts.is_empty() { 0.0 } else { 40.0 + parts.len().div_ceil(2) as f32 * 50.0 };
    let (outer, _) = ui.allocate_exact_size(vec2(W - 16.0, H + extra), Sense::hover());
    let p = ui.painter_at(outer);
    p.rect_filled(outer, CornerRadius::same(6), WALNUT);
    let (x0, x1) = (outer.min.x + 12.0, outer.max.x - 12.0);
    let title = Rect::from_min_max(pos2(x0, outer.min.y + 8.0), pos2(x1, outer.min.y + 50.0));
    let (cells, shown) = cells(ins);
    title_bar(app, ui, &p, title, tid, ins, &cells);
    let side_w = 300.0;
    let (top, bottom) = (title.max.y + 10.0, outer.max.y - 12.0);
    let main = Rect::from_min_max(pos2(x0, top), pos2(x1 - side_w - 12.0, bottom));
    let side = Rect::from_min_max(pos2(x1 - side_w, top), pos2(x1, bottom));
    let shown = shown.and_then(|i| parts.iter().find(|(n, _)| *n == i)).filter(|_| parts.len() > 1).map(|(_, name)| name.as_str());
    map_card(app, ui, &p, main, tid, ins, &cells, shown);
    side_card(app, ui, &p, side, tid, ins, &parts);
}

/// The top bar: the wordmark, what the instrument holds, and its buttons.
fn title_bar(app: &mut SoundApp, ui: &mut Ui, p: &Painter, r: Rect, tid: TrackId, ins: &Insert, cells: &[Cell]) {
    let g = p.layout_no_wrap("Pantry".into(), bold(30.0), AMBER);
    let w = g.size().x;
    p.galley(pos2(r.min.x, r.center().y - g.size().y * 0.5), g, AMBER);
    p.text(pos2(r.min.x + w + 10.0, r.center().y + 4.0), Align2::LEFT_CENTER, "Multisampler", regular(14.0), DIM);
    let s = app.engine.session();
    let (mut values, mut secs) = (0usize, 0.0f64);
    for a in ins.samples.values().filter_map(|id| s.pool.get(*id)) {
        values += a.frames() * a.num_channels();
        secs += a.frames() as f64 / f64::from(a.sample_rate().max(1));
    }
    let mut by_note: BTreeMap<u8, usize> = BTreeMap::new();
    for c in cells {
        *by_note.entry(c.note).or_default() += 1;
    }
    let layers = by_note.values().copied().max().unwrap_or(0);
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let (name, facts) = if ins.samples.is_empty() {
        ("Empty".to_string(), "capture a track, load one, or drop samples here".to_string())
    } else {
        (
            format!("{} sample{}", ins.samples.len(), plural(ins.samples.len())),
            format!(
                "{} note{} · {layers} layer{} · {:.0} s · {:.0} MB",
                by_note.len(),
                plural(by_note.len()),
                plural(layers),
                secs,
                values as f64 * 4.0 / 1.0e6
            ),
        )
    };
    let mid = Rect::from_center_size(pos2(r.center().x - 20.0, r.center().y), vec2(320.0, 44.0));
    p.rect_filled(mid, CornerRadius::same(22), Color32::from_rgb(0x22, 0x1D, 0x19));
    p.rect_stroke(mid, CornerRadius::same(22), Stroke::new(1.0, Color32::from_rgb(0x48, 0x3F, 0x37)), StrokeKind::Inside);
    p.text(pos2(mid.center().x, mid.min.y + 15.0), Align2::CENTER_CENTER, name, bold(15.0), TEXT);
    p.text(pos2(mid.center().x, mid.min.y + 32.0), Align2::CENTER_CENTER, facts, regular(11.5), DIM);
    let mut x = r.max.x;
    let mut next = |w: f32| {
        let b = Rect::from_min_max(pos2(x - w, r.center().y - 17.0), pos2(x, r.center().y + 17.0));
        x -= w + 8.0;
        b
    };
    let clear_r = next(70.0);
    if !ins.samples.is_empty()
        && button(ui, p, clear_r, "Clear", true, "pantry-clear").clicked()
        && let Err(e) = app.run("mix.multisample_clear", json!({"track": tid.0}))
    {
        app.ui.status = e;
    }
    let save_r = next(84.0);
    let can_save = !ins.samples.is_empty() && app.dialogs.dirs.pantry.is_some();
    let save = button(ui, p, save_r, "Save…", can_save, "pantry-save")
        .on_hover_text("Keep this instrument in the Library, for any session")
        .on_disabled_hover_text(if ins.samples.is_empty() { "Nothing on the shelves to save" } else { "The Library needs the desktop app" });
    if save.clicked() {
        let name = app.engine.session().track(tid).map_or_else(|| "Pantry".to_string(), |t| t.name.clone());
        app.dialogs.open = Some(crate::dialogs::Dialog::SavePantry { track: tid, name });
    }
    let load_r = next(84.0);
    if button(ui, p, load_r, "Load…", true, "pantry-load")
        .on_hover_text("An instrument from the Library, an SFZ file, or a folder of captured samples")
        .clicked()
    {
        app.dialogs.open =
            Some(crate::dialogs::Dialog::LoadPantry { track: tid, path: String::new(), saved: crate::user_files::pantry(&app.dialogs.dirs) });
    }
}

/// The status line after a multisample command returned `v` (`None`: not one that reports).
pub fn status(id: &str, v: &serde_json::Value) -> Option<String> {
    let n = |k: &str| v[k].as_u64().unwrap_or(0);
    let plural = |n: u64| if n == 1 { "" } else { "s" };
    let counted = |verb: &str| format!("{verb} {} sample{} of {} note{}", n("samples"), plural(n("samples")), n("notes"), plural(n("notes")));
    let mut s = match id {
        "mix.multisample_load" => counted("Loaded"),
        "mix.multisample_import" => counted("Imported"),
        "mix.multisample_save" => {
            return Some(format!(
                "Saved {} to the Library: {} sample{}, {:.0} MB",
                v["name"].as_str().unwrap_or("it"),
                n("samples"),
                plural(n("samples")),
                n("bytes") as f64 / 1.0e6
            ));
        }
        _ => return None,
    };
    if n("unnamed") > 0 {
        s.push_str(&format!("; {} had no note and velocity in the name", n("unnamed")));
    }
    if n("skipped") > 0 {
        s.push_str(&format!("; {} did not fit (16 layers a note, 4 takes a layer)", n("skipped")));
    }
    Some(s)
}

/// A layer's colour: warmer and brighter the harder it was played.
fn layer_colour(velocity: u8) -> Color32 {
    let t = f32::from(velocity) / 127.0;
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t) as u8;
    let dark = Color32::from_rgb(0x6A, 0x52, 0x34);
    Color32::from_rgb(mix(dark.r(), AMBER.r()), mix(dark.g(), AMBER.g()), mix(dark.b(), AMBER.b()))
}

/// The shelves: every sample over its keys and velocities, with a strip of keys underneath.
fn map_card(app: &mut SoundApp, ui: &mut Ui, p: &Painter, r: Rect, tid: TrackId, ins: &Insert, cells: &[Cell], shown: Option<&str>) {
    card(p, r);
    section(p, pos2(r.min.x + 16.0, r.min.y + 22.0), "Shelves");
    p.text(
        pos2(r.min.x + 92.0, r.min.y + 23.0),
        Align2::LEFT_CENTER,
        match shown {
            Some(part) => format!("the {part} part · keys left to right, velocity bottom to top · press a block to hear it"),
            None => "keys left to right, velocity bottom to top · press a block to hear it".to_string(),
        },
        regular(12.0),
        DIM,
    );
    let gutter = 30.0;
    let keys_h = 34.0;
    let map = Rect::from_min_max(pos2(r.min.x + 16.0 + gutter, r.min.y + 44.0), pos2(r.max.x - 16.0, r.max.y - 16.0 - keys_h - 6.0));
    p.rect_filled(map, CornerRadius::same(6), WELL);
    let notes: Vec<u8> = cells.iter().map(|c| c.note).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    // The keys shown: the recorded notes with a little room each side (a piano's, when empty).
    let (first, last) = match (notes.first(), notes.last()) {
        (Some(&a), Some(&b)) => (a.saturating_sub(6), b.saturating_add(6).min(127)),
        _ => (21, 108),
    };
    let (first, last) = if last - first < 24 { (first.saturating_sub(12), last.saturating_add(12).min(127)) } else { (first, last) };
    let key_w = map.width() / f32::from(last - first + 1);
    let key_x = |k: u8| map.min.x + f32::from(k.clamp(first, last) - first) * key_w;
    let vel_y = |v: f32| map.max.y - (v / 127.0).clamp(0.0, 1.0) * map.height();
    for v in [1u8, 32, 64, 96, 127] {
        let y = vel_y(f32::from(v));
        p.line_segment([pos2(map.min.x, y), pos2(map.max.x, y)], Stroke::new(1.0, WELL_GRID));
        p.text(pos2(map.min.x - 6.0, y.clamp(map.min.y + 6.0, map.max.y - 6.0)), Align2::RIGHT_CENTER, v.to_string(), regular(10.5), DIM);
    }
    let blend = param(ins, "layer_blend") / 100.0;
    let pointer = ui.input(|i| i.pointer.latest_pos()).filter(|pos| map.contains(*pos));
    let mut hovered: Option<String> = None;
    for c in cells.iter().filter(|c| c.hikey >= first && c.lokey <= last) {
        let (xa, xb) = (key_x(c.lokey.max(first)), key_x(c.hikey.min(last)) + key_w);
        let cell = Rect::from_min_max(pos2(xa + 0.5, vel_y(f32::from(c.top)) + 0.5), pos2(xb - 0.5, vel_y(f32::from(c.below)) - 0.5));
        p.rect_filled(cell, CornerRadius::same(2), layer_colour(c.top));
        // The bottom of a layer's range mixes with the layer below it.
        if c.below > 0 && blend > 0.0 {
            let mixed = Rect::from_min_max(pos2(cell.min.x, cell.max.y - cell.height() * blend), cell.max);
            p.rect_filled(mixed, CornerRadius::same(2), layer_colour(c.below).gamma_multiply(0.55));
        }
        if c.takes > 1 && cell.width() >= 8.0 && cell.height() >= 8.0 {
            for t in 0..c.takes.min(4) {
                p.circle_filled(pos2(cell.min.x + 4.0 + t as f32 * 5.0, cell.min.y + 4.0), 1.5, INK);
            }
        }
        if pointer.is_some_and(|pos| cell.contains(pos)) {
            p.rect_stroke(cell, CornerRadius::same(2), Stroke::new(1.5, CREAM), StrokeKind::Inside);
            let name = soundcraft_midi::note_name;
            hovered = Some(format!(
                "{} · keys {} to {} · velocity {} to {} · {} take{}",
                name(c.note),
                name(c.lokey),
                name(c.hikey),
                u16::from(c.below) + 1,
                c.top,
                c.takes,
                if c.takes == 1 { "" } else { "s" }
            ));
        }
    }
    p.rect_stroke(map, CornerRadius::same(6), Stroke::new(1.0, Color32::from_black_alpha(120)), StrokeKind::Inside);
    if notes.is_empty() {
        p.text(map.center() - vec2(0.0, 10.0), Align2::CENTER_CENTER, "Nothing on the shelves yet", bold(15.0), TEXT);
        p.text(
            map.center() + vec2(0.0, 12.0),
            Align2::CENTER_CENTER,
            "Capture a track (Track › Capture to Pantry…), load an instrument, or drop captured samples here",
            regular(12.5),
            DIM,
        );
    }
    // The keys, a column each: white keys cream, black keys dark, recorded notes amber.
    let strip = Rect::from_min_max(pos2(map.min.x, map.max.y + 6.0), pos2(map.max.x, map.max.y + 6.0 + keys_h));
    let black = |n: u8| matches!(n % 12, 1 | 3 | 6 | 8 | 10);
    for k in first..=last {
        let x = key_x(k);
        let recorded = notes.contains(&k);
        let h = if black(k) { keys_h * 0.62 } else { keys_h };
        let fill = if recorded {
            AMBER
        } else if black(k) {
            Color32::from_rgb(0x1A, 0x15, 0x11)
        } else {
            CREAM
        };
        p.rect_filled(
            Rect::from_min_size(pos2(x + 0.5, strip.min.y), vec2((key_w - 1.0).max(1.0), h)),
            CornerRadius { nw: 0, ne: 0, sw: 2, se: 2 },
            fill,
        );
        if k % 12 == 0 && key_w * 12.0 >= 30.0 {
            p.text(
                pos2(x + key_w * 0.5, strip.max.y - 2.0),
                Align2::CENTER_BOTTOM,
                soundcraft_midi::note_name(k),
                regular(9.0),
                INK.gamma_multiply(0.8),
            );
        }
    }
    // Pressing the map plays the key under the pointer at the velocity under it, until let go.
    let resp = ui.interact(map, ui.id().with(("pantry-map", tid.0)), Sense::click_and_drag());
    let held_id = egui::Id::new(("pantry-held", tid.0));
    let held: Option<u8> = ui.ctx().data(|d| d.get_temp(held_id));
    let at = resp.interact_pointer_pos().or(pointer);
    let under = at.map(|pos| {
        let key = first.saturating_add(((pos.x - map.min.x) / key_w).floor().clamp(0.0, f32::from(last - first)) as u8);
        let velocity = (((map.max.y - pos.y) / map.height()) * 127.0).round().clamp(1.0, 127.0) as u8;
        (key, velocity)
    });
    let down = resp.is_pointer_button_down_on();
    match (held, under.filter(|_| down && !notes.is_empty())) {
        (None, Some((key, velocity))) => {
            send(app, [0x90, key, velocity]);
            ui.ctx().data_mut(|d| d.insert_temp(held_id, key));
        }
        (Some(key), None) => {
            send(app, [0x80, key, 0]);
            ui.ctx().data_mut(|d| d.remove::<u8>(held_id));
        }
        _ => {}
    }
    if resp.hovered() && !notes.is_empty() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    if let Some(text) = hovered {
        resp.on_hover_text_at_pointer(text);
    }
}

/// Sends a live message on the Musical Typing port (nothing where there is no MIDI hub).
fn send(app: &SoundApp, msg: [u8; 3]) {
    if let Some(h) = &app.midi_hub {
        h.typing().send(&msg);
    }
}

/// Parameters whose knob lights its ring from the centre.
fn bipolar(id: &str) -> bool {
    matches!(id, "transpose" | "fine_tune" | "pan")
}

fn value_text(ins: &Insert, id: &str) -> String {
    let Some(info) = info_of(id) else { return String::new() };
    let v = param(ins, id);
    if info.unit == soundcraft_dsp::Unit::Ms && v >= 1000.0 {
        return format!("{:.2} s", v / 1000.0);
    }
    if info.unit == soundcraft_dsp::Unit::Ms && v >= 100.0 {
        return format!("{v:.0} ms");
    }
    if id == "velocity_curve" {
        return format!("{v:.2}");
    }
    if PART_LEVELS.contains(&id) && v <= PART_OFF_DB {
        return "Off".to_string();
    }
    info.format(v)
}

/// A small knob with its name and value to its right: drag up and down (Shift for fine), wheel,
/// double-click resets.
fn knob(app: &mut SoundApp, ui: &mut Ui, p: &Painter, at: Pos2, tid: TrackId, ins: &Insert, id: &str, name: &str) {
    let Some(info) = info_of(id) else { return };
    let rad = 15.0;
    let c = pos2(at.x + rad + 6.0, at.y + 22.0);
    let v = param(ins, id);
    let resp = ui.interact(
        Rect::from_center_size(c, vec2(rad * 2.0 + 10.0, rad * 2.0 + 10.0)),
        ui.id().with(("pantry-knob", tid.0, id)),
        Sense::click_and_drag(),
    );
    resp.widget_info(|| egui::WidgetInfo::slider(true, f64::from(v), name));
    let hot = resp.hovered() || resp.dragged();
    if hot {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }
    let fine = ui.input(|i| i.modifiers.shift);
    if resp.double_clicked() {
        set(app, tid, id, info.default);
    } else if resp.dragged() {
        let dy = resp.drag_delta().y;
        if dy != 0.0 {
            set(app, tid, id, crate::widgets::knob_drag(info, v, dy, fine));
        }
    } else if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.5 {
            set(app, tid, id, crate::widgets::knob_scroll(info, v, scroll / 50.0));
        }
    }
    paint_knob(p, c, rad, info.to_normalized(v), false, bipolar(id), hot);
    p.text(pos2(c.x + rad + 10.0, c.y - 8.0), Align2::LEFT_CENTER, name, regular(12.0), DIM);
    p.text(pos2(c.x + rad + 10.0, c.y + 8.0), Align2::LEFT_CENTER, value_text(ins, id), bold(13.5), TEXT);
}

/// Each part's level parameter.
const PART_LEVELS: [&str; PARTS] =
    ["part1_level", "part2_level", "part3_level", "part4_level", "part5_level", "part6_level", "part7_level", "part8_level"];

/// The side card's sections, top to bottom: a name, a note, and its knobs (parameter, label).
const SECTIONS: [(&str, &str, &[(&str, &str)]); 4] = [
    ("Dynamics", "how the layers meet", &[("velocity_curve", "Velocity Curve"), ("layer_blend", "Layer Blend")]),
    ("Envelope", "", &[("attack", "Attack"), ("decay", "Decay"), ("sustain", "Sustain"), ("release", "Release")]),
    ("Pitch", "", &[("transpose", "Transpose"), ("fine_tune", "Fine Tune")]),
    ("Output", "", &[("level", "Level"), ("pan", "Pan")]),
];

/// The side card: Dynamics, Envelope, Pitch and Output.
fn side_card(app: &mut SoundApp, ui: &mut Ui, p: &Painter, r: Rect, tid: TrackId, ins: &Insert, parts: &[(usize, String)]) {
    card(p, r);
    let col = (r.width() - 24.0) / 2.0;
    let mut y = r.min.y + 22.0;
    for (i, (name, note, knobs)) in SECTIONS.iter().enumerate() {
        if i > 0 {
            p.line_segment([pos2(r.min.x + 12.0, y - 14.0), pos2(r.max.x - 12.0, y - 14.0)], Stroke::new(1.0, TOBACCO_HI));
        }
        section(p, pos2(r.min.x + 16.0, y), name);
        if !note.is_empty() {
            p.text(pos2(r.max.x - 16.0, y + 1.0), Align2::RIGHT_CENTER, *note, regular(12.0), DIM);
        }
        y += 14.0;
        for (k, (id, label)) in knobs.iter().enumerate() {
            let at = pos2(r.min.x + 12.0 + (k % 2) as f32 * col, y + (k / 2) as f32 * 50.0);
            knob(app, ui, p, at, tid, ins, id, label);
        }
        y += knobs.len().div_ceil(2) as f32 * 50.0 + 26.0;
    }
    // A library's parts sound together, each at its own level (all the way down: off).
    if !parts.is_empty() {
        p.line_segment([pos2(r.min.x + 12.0, y - 14.0), pos2(r.max.x - 12.0, y - 14.0)], Stroke::new(1.0, TOBACCO_HI));
        section(p, pos2(r.min.x + 16.0, y), "Parts");
        p.text(pos2(r.max.x - 16.0, y + 1.0), Align2::RIGHT_CENTER, "sound together", regular(12.0), DIM);
        y += 14.0;
        for (k, (index, name)) in parts.iter().enumerate() {
            let at = pos2(r.min.x + 12.0 + (k % 2) as f32 * col, y + (k / 2) as f32 * 50.0);
            if let Some(id) = PART_LEVELS.get(*index) {
                knob(app, ui, p, at, tid, ins, id, name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shelves_list_each_notes_layers_and_takes() {
        let mut ins = Insert::new(PANTRY);
        for key in ["n60.v127.r1", "n60.v64.r1", "n60.v64.r2", "n48.v100.r1", "not a key"] {
            ins.samples.insert(key.into(), soundcraft_model::SourceId(1));
        }
        let s = shelves(&ins);
        assert_eq!(s.keys().copied().collect::<Vec<_>>(), [48, 60]);
        assert_eq!((s[&48].as_slice(), s[&60].as_slice()), (&[(100, 1)][..], &[(64, 2), (127, 1)][..]));
    }

    #[test]
    fn a_library_shows_its_biggest_part_and_names_them_all() {
        let mut ins = Insert::new(PANTRY);
        let mut add = |index: usize, part: usize, name: &str, set: &dyn Fn(&mut ZoneSpec)| {
            let mut z = ZoneSpec { index, part, part_name: name.into(), ..ZoneSpec::default() };
            set(&mut z);
            ins.samples.insert(z.key(), soundcraft_model::SourceId(index as u64 + 1));
        };
        add(0, 0, "pad", &|z| (z.lokey, z.hikey, z.root) = (21, 60, 48));
        add(1, 1, "notes", &|z| (z.lokey, z.hikey, z.root, z.hivel) = (30, 34, 34, 70));
        add(2, 1, "notes", &|z| (z.lokey, z.hikey, z.root, z.lovel) = (30, 34, 34, 71));
        add(3, 1, "notes", &|z| (z.lokey, z.hikey, z.root, z.lovel, z.seq) = (35, 39, 39, 1, 1));
        add(4, 1, "notes", &|z| (z.lokey, z.hikey, z.root, z.lovel, z.seq) = (35, 39, 39, 1, 2));
        add(5, 2, "", &|z| z.trigger = soundcraft_dsp::plugins::multisampler::ON_KEY_UP);
        assert_eq!(parts(&ins), [(0, "pad".to_string()), (1, "notes".to_string()), (2, "Part 3".to_string())]);
        let (cells, shown) = cells(&ins);
        assert_eq!(shown, Some(1));
        assert_eq!(
            cells,
            [
                Cell { note: 34, lokey: 30, hikey: 34, below: 0, top: 70, takes: 1 },
                Cell { note: 34, lokey: 30, hikey: 34, below: 70, top: 127, takes: 1 },
                Cell { note: 39, lokey: 35, hikey: 39, below: 0, top: 127, takes: 2 },
            ]
        );
        // A captured instrument: its notes over the keys nearest them, no parts to show.
        let mut ins = Insert::new(PANTRY);
        for key in ["n60.v127.r1", "n60.v64.r1", "n60.v64.r2"] {
            ins.samples.insert(key.into(), soundcraft_model::SourceId(1));
        }
        assert!(parts(&ins).is_empty());
        assert_eq!(
            super::cells(&ins),
            (
                vec![
                    Cell { note: 60, lokey: 0, hikey: 127, below: 0, top: 64, takes: 2 },
                    Cell { note: 60, lokey: 0, hikey: 127, below: 64, top: 127, takes: 1 }
                ],
                None
            )
        );
    }

    #[test]
    fn each_note_takes_the_keys_nearest_it() {
        assert_eq!(key_spans(&[48, 51, 60]), [(48, 0, 49), (51, 50, 55), (60, 56, 127)]);
        // Halfway between two goes to the lower one, as the instrument plays it.
        assert_eq!(key_spans(&[48, 50]), [(48, 0, 49), (50, 50, 127)]);
        assert_eq!(key_spans(&[60]), [(60, 0, 127)]);
        assert!(key_spans(&[]).is_empty());
    }

    #[test]
    fn an_instrument_goes_to_the_library_and_comes_back_in_another_track() {
        use serde_json::json;
        let dir = std::env::temp_dir().join(format!("sc-pantry-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = soundcraft_engine::Engine::default();
        let new = |e: &mut soundcraft_engine::Engine, plugin: &str| {
            e.execute("track.new", &json!({"kind": "instrument", "instrument": plugin})).unwrap()["tracks"][0].as_u64().unwrap()
        };
        let synth = new(&mut e, "subtractive_synth");
        let other = new(&mut e, PANTRY);
        let mut app = SoundApp::new(e, None, crate::Services::default());
        app.dialogs.dirs = crate::user_files::UserDirs::under(&dir);
        let made = app
            .run_sync("track.capture_multisample", json!({"track": synth, "low": 60, "high": 63, "step": 3, "layers": 2, "hold": 0.2, "tail": 0.4}))
            .unwrap();
        // What Save… sends: a file in the Library's Pantry folder.
        let file = app.dialogs.dirs.pantry.clone().unwrap().join("Grand.sfz");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        app.run("mix.multisample_save", json!({"track": made["track"], "path": file.to_string_lossy()})).unwrap();
        assert!(app.ui.status.starts_with("Saved Grand to the Library: 4 samples"), "{}", app.ui.status);
        // Load… lists it, and loading it fills the other track's shelves.
        let saved = crate::user_files::pantry(&app.dialogs.dirs);
        assert_eq!(saved.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Grand"]);
        app.run("mix.multisample_load", json!({"track": other, "path": saved[0].path.to_string_lossy()})).unwrap();
        assert_eq!(app.ui.status, "Loaded 4 samples of 2 notes");
        let ins = app.engine.session().track(TrackId(other)).and_then(|t| t.instrument.clone()).unwrap();
        assert_eq!(shelves(&ins).keys().copied().collect::<Vec<_>>(), [60, 63]);
        // The saved samples as loose files import by their names.
        app.run("mix.multisample_clear", json!({"track": other})).unwrap();
        app.run("mix.multisample_import", json!({"track": other, "folder": file.with_extension("").to_string_lossy()})).unwrap();
        assert_eq!(app.ui.status, "Imported 4 samples of 2 notes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_status_line_says_what_a_command_did() {
        use serde_json::json;
        assert_eq!(
            status("mix.multisample_load", &json!({"samples": 360, "notes": 30, "skipped": 0})).as_deref(),
            Some("Loaded 360 samples of 30 notes")
        );
        assert_eq!(
            status("mix.multisample_import", &json!({"samples": 1, "notes": 1, "skipped": 2, "unnamed": 3})).as_deref(),
            Some("Imported 1 sample of 1 note; 3 had no note and velocity in the name; 2 did not fit (16 layers a note, 4 takes a layer)")
        );
        assert_eq!(
            status("mix.multisample_save", &json!({"name": "Grand", "samples": 360, "bytes": 912_000_000})).as_deref(),
            Some("Saved Grand to the Library: 360 samples, 912 MB")
        );
        assert_eq!(status("mix.multisample_clear", &json!({})), None);
    }

    #[test]
    fn every_knob_is_a_parameter_of_the_instrument() {
        let info = soundcraft_dsp::plugin_info(PANTRY).unwrap();
        let shown: Vec<&str> = SECTIONS.iter().flat_map(|s| s.2.iter().map(|k| k.0)).chain(PART_LEVELS).collect();
        assert_eq!(shown.len(), info.params.len(), "every parameter has a knob");
        assert!(shown.iter().all(|id| info.param(id).is_some()));
    }
}
