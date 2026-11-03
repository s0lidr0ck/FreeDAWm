//! The Band's two lanes in the Edit window's ruler area.
//!
//! - **Sections:** blocks of the song's section definitions (A–H), drawn `<letter> · <name>` in
//!   the letter's colour. A drag on empty space draws a block with the last letter picked (A at
//!   first); a drag on a body moves it (Alt copies it) and a drag on an edge resizes it. Right-click
//!   picks the letter, the arrangement chips (Energy, Take), opens the definition or deletes.
//! - **Chords** (id `band_chords`): the chord lane, named by `chord_theory::name_for`. A
//!   double-click opens the chord picker (`chord_picker`) on empty space or on a chord; a drag on a
//!   body or an edge moves or resizes it (Alt: off the grid); a click picks it for Delete.
//!   Right-click edits, deletes (one, those in the selection, or all: `ruler_items`) or picks the
//!   bass.
//!
//! Every edit is a `band.*` command. Positions go out in samples and the engine snaps them to the
//! half-bar grid ([`soundcraft_mix::half_ticks`]); the previews here snap the same way. An
//! Alt-drag of a chord sends `snap: false` and keeps the tick under the pointer.

use crate::SoundApp;
use crate::edit_window::{sample_at, x_of};
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Color32, CursorIcon, Id, Painter, Rect, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_band::chord_theory::{name_for, note_name};
use soundcraft_model::Session;
use soundcraft_time::Samples;

/// The height of both lanes.
pub const LANE_H: f32 = 22.0;
/// The definition "Edit definition…" asks the Studio window to show (a `char` in egui's temporary
/// memory under `Id::new(STUDIO_DEF)`).
pub const STUDIO_DEF: &str = "band.studio.def";
/// Within this many points of a block's end a press grabs the edge (resize), not the body.
const EDGE_PX: f32 = 5.0;
/// How long "≈ nearest in key" shows after a typed chord the key doesn't hold.
const FLASH_SECS: f64 = 2.0;
/// The letter the next drawn block takes.
const LETTER: &str = "band.lanes.letter";
const SECTIONS: &str = "band.lanes.sections";
const CHORDS: &str = "band.lanes.chords";
const MENU: &str = "band.lanes.menu";
const FLASH: &str = "band.lanes.flash";

/// Section colours, A to H.
const LETTER_COLORS: [Color32; 8] = [
    Color32::from_rgb(58, 112, 184),
    Color32::from_rgb(196, 106, 46),
    Color32::from_rgb(62, 140, 84),
    Color32::from_rgb(150, 74, 152),
    Color32::from_rgb(184, 64, 70),
    Color32::from_rgb(40, 138, 142),
    Color32::from_rgb(158, 136, 34),
    Color32::from_rgb(98, 96, 176),
];

/// A section letter's colour.
pub fn letter_color(letter: char) -> Color32 {
    let i = u32::from(letter).wrapping_sub(u32::from('A')) as usize;
    LETTER_COLORS.get(i).copied().unwrap_or(Color32::from_rgb(110, 110, 114))
}

/// A lane item: its id and `[start, end)` in ticks.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Item {
    id: u32,
    start: i64,
    end: i64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    Body,
    Start,
    End,
}

/// A drag in progress on a lane.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Drag {
    /// Drawing a new item from this tick.
    New(i64),
    /// An item grabbed by its body or an edge; `press` is the tick under the press.
    Item { item: Item, grab: Grab, press: i64 },
}

/// What a drag makes, snapped: the item (`None` for a new one) and its span.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ghost {
    id: Option<u32>,
    start: i64,
    end: i64,
    copy: bool,
    resize: bool,
}

/// Ticks to points and back, and the half-bar grid.
struct Grid<'a> {
    s: &'a Session,
    tl: Rect,
    half: i64,
}

impl Grid<'_> {
    fn new(s: &Session, tl: Rect) -> Grid<'_> {
        Grid { s, tl, half: soundcraft_mix::half_ticks(s).max(1) }
    }

    /// The nearest half bar (as the engine snaps).
    fn snap(&self, tick: i64) -> i64 {
        tick.max(0).saturating_add(self.half / 2).div_euclid(self.half).saturating_mul(self.half)
    }

    fn tick_at(&self, x: f32) -> i64 {
        self.s.tempo.samples_to_ticks(sample_at(self.s, self.tl, x).max(0), self.s.sample_rate).max(0)
    }

    fn samples(&self, tick: i64) -> Samples {
        self.s.tempo.tick_to_samples(tick, self.s.sample_rate)
    }

    fn x(&self, tick: i64) -> f32 {
        x_of(self.s, self.tl, self.samples(tick))
    }
}

/// The item under `x` and the part grabbed (the last drawn wins).
fn hit(items: &[Item], g: &Grid, x: f32) -> Option<(Item, Grab)> {
    items.iter().rev().find_map(|it| {
        let (a, b) = (g.x(it.start), g.x(it.end));
        if x < a || x >= b {
            return None;
        }
        let grab = if b - a < 3.0 * EDGE_PX {
            Grab::Body
        } else if x - a <= EDGE_PX {
            Grab::Start
        } else if b - x <= EDGE_PX {
            Grab::End
        } else {
            Grab::Body
        };
        Some((*it, grab))
    })
}

/// What `d` makes with the pointer at `now`. `free` (an Alt-drag on the Chords ruler) keeps the
/// ticks off the grid, at least an eighth of a bar long.
fn ghost(d: Drag, now: i64, g: &Grid, copy: bool, free: bool) -> Ghost {
    let snap = |t: i64| if free { t.max(0) } else { g.snap(t) };
    let half = if free { (g.half / 4).max(1) } else { g.half };
    match d {
        Drag::New(from) => {
            let (a, b) = (snap(from.min(now)), snap(from.max(now)));
            Ghost { id: None, start: a, end: if b > a { b } else { a.saturating_add(half) }, copy: false, resize: false }
        }
        Drag::Item { item, grab: Grab::Body, press } => {
            let a = snap(item.start.saturating_add(now.saturating_sub(press)));
            Ghost { id: Some(item.id), start: a, end: a.saturating_add(item.end.saturating_sub(item.start)), copy, resize: false }
        }
        Drag::Item { item, grab: Grab::Start, .. } => {
            let a = snap(now).min(item.end.saturating_sub(half)).max(0);
            Ghost { id: Some(item.id), start: a, end: item.end, copy: false, resize: true }
        }
        Drag::Item { item, grab: Grab::End, .. } => {
            let b = snap(now).max(item.start.saturating_add(half));
            Ghost { id: Some(item.id), start: item.start, end: b, copy: false, resize: true }
        }
    }
}

/// Runs a lane's drag: the ghost while dragging, and the finished one on release. Alt copies on a
/// lane that `draws` (Sections) and frees the grid on one that doesn't (Chords).
fn drag(ui: &Ui, resp: &Response, g: &Grid, items: &[Item], key: Id, draws: bool) -> (Option<Ghost>, Option<Ghost>) {
    if resp.drag_started() {
        let origin = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
        let d = origin.and_then(|p| match hit(items, g, p.x) {
            Some((item, grab)) => Some(Drag::Item { item, grab, press: g.tick_at(p.x) }),
            None if draws => Some(Drag::New(g.tick_at(p.x))),
            None => None,
        });
        ui.ctx().data_mut(|m| {
            if let Some(d) = d {
                m.insert_temp(key, d);
            } else {
                m.remove::<Drag>(key);
            }
        });
    }
    let Some(d) = ui.ctx().data(|m| m.get_temp::<Drag>(key)) else { return (None, None) };
    let pointer = resp.interact_pointer_pos().or_else(|| ui.input(|i| i.pointer.latest_pos()));
    let (stopped, going) = (resp.drag_stopped(), resp.dragged());
    if stopped || !going {
        ui.ctx().data_mut(|m| m.remove::<Drag>(key));
    }
    let Some(p) = pointer else { return (None, None) };
    let alt = ui.input(|i| i.modifiers.alt);
    let gh = ghost(d, g.tick_at(p.x), g, alt && draws, alt && !draws);
    if stopped {
        (None, Some(gh))
    } else if going {
        (Some(gh), None)
    } else {
        (None, None)
    }
}

/// The resize cursor over an edge (and while resizing).
fn edge_cursor(ui: &Ui, resp: &Response, g: &Grid, items: &[Item], live: Option<Ghost>) {
    let over_edge = resp.hover_pos().and_then(|p| hit(items, g, p.x)).is_some_and(|(_, grab)| grab != Grab::Body);
    if over_edge || live.is_some_and(|gh| gh.resize) {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
}

/// The item a right-click opened the lane's menu on.
fn menu_target(ui: &Ui, resp: &Response, g: &Grid, items: &[Item], key: Id) -> Option<u32> {
    if resp.secondary_clicked() {
        let id = resp.interact_pointer_pos().and_then(|p| hit(items, g, p.x)).map(|(it, _)| it.id);
        ui.ctx().data_mut(|m| m.insert_temp(key, id));
    }
    ui.ctx().data(|m| m.get_temp::<Option<u32>>(key)).flatten()
}

fn span_rect(g: &Grid, row: Rect, start: i64, end: i64) -> Rect {
    Rect::from_min_max(pos2(g.x(start), row.min.y + 2.0), pos2(g.x(end), row.max.y - 2.0))
}

/// A block with its label clipped to it (the label only where it has room).
fn block(painter: &Painter, r: Rect, fill: Color32, stroke: Color32, label: impl FnOnce() -> String, text: Color32) {
    painter.rect(r, 2.0, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    if r.width() > 16.0 {
        let clip = r.shrink(1.0).intersect(painter.clip_rect());
        painter.with_clip_rect(clip).text(pos2(r.min.x + 5.0, r.center().y), Align2::LEFT_CENTER, label(), bold(11.0), text);
    }
}

fn draw_ghost(painter: &Painter, g: &Grid, row: Rect, gh: Ghost, color: Color32) {
    let r = span_rect(g, row, gh.start, gh.end);
    painter.rect(r, 2.0, color.gamma_multiply(0.35), Stroke::new(1.5, color), StrokeKind::Inside);
}

/// Runs `id` (one undo step), first making the section definition `letter` when it is missing.
fn run_with_def(app: &mut SoundApp, letter: char, id: &str, p: Value) {
    if app.engine.session().band.def(letter).is_some() {
        let _ = app.run(id, p);
        return;
    }
    let key = format!("band.def+{id}");
    let r = app.engine.execute_merged("band.def", &json!({"id": letter.to_string()}), &key).and_then(|_| app.engine.execute_merged(id, &p, &key));
    app.engine.end_merge();
    if let Err(e) = r {
        log::warn!("{id}: {e}");
        app.ui.status = e.to_string();
    }
}

fn letter(ui: &Ui) -> char {
    ui.ctx().data(|m| m.get_temp::<char>(Id::new(LETTER))).unwrap_or('A')
}

fn set_letter(ui: &Ui, l: char) {
    ui.ctx().data_mut(|m| m.insert_temp(Id::new(LETTER), l));
}

// ---- Sections -----------------------------------------------------------------------------------

/// The Sections ruler: the song's section blocks.
pub fn sections_ruler(app: &mut SoundApp, ui: &mut Ui, painter: &Painter, s: &Session, tl: Rect, row: Rect) {
    let t = Tokens::current();
    let g = Grid::new(s, tl);
    painter.rect_filled(row, 0.0, t.marker_ruler);
    let band = &s.band;
    let items: Vec<Item> =
        band.sections.iter().map(|b| Item { id: b.id, start: b.start_tick, end: b.start_tick.saturating_add(b.length_ticks) }).collect();
    let key = ui.id().with(SECTIONS);
    let resp = ui.interact(row, key, Sense::click_and_drag());
    let (live, done) = drag(ui, &resp, &g, &items, key, true);
    let moving = live.filter(|gh| !gh.copy).and_then(|gh| gh.id);
    for b in &band.sections {
        let r = span_rect(&g, row, b.start_tick, b.start_tick.saturating_add(b.length_ticks));
        if r.max.x < row.min.x || r.min.x > row.max.x {
            continue;
        }
        let fill = letter_color(b.section);
        let fill = if moving == Some(b.id) { fill.gamma_multiply(0.4) } else { fill };
        let name = band.def(b.section).map(|d| d.name.as_str()).unwrap_or("");
        let label = || if name.is_empty() { b.section.to_string() } else { format!("{} · {name}", b.section) };
        block(painter, r, fill, fill.gamma_multiply(0.6), label, Color32::WHITE);
        // The arrangement chips, at the block's end when set.
        let chips = match (b.energy, b.take) {
            (0, 0) => String::new(),
            (e, 0) => format!("{e:+}"),
            (0, k) => format!("T{k}"),
            (e, k) => format!("{e:+} T{k}"),
        };
        if !chips.is_empty() && r.width() > 90.0 {
            painter.text(pos2(r.max.x - 5.0, r.center().y), Align2::RIGHT_CENTER, chips, regular(10.0), Color32::from_white_alpha(220));
        }
    }
    if let Some(gh) = live {
        draw_ghost(painter, &g, row, gh, if gh.id.is_none() { letter_color(letter(ui)) } else { t.accent });
    }
    edge_cursor(ui, &resp, &g, &items, live);
    let target = menu_target(ui, &resp, &g, &items, ui.id().with(MENU).with(SECTIONS));
    resp.context_menu(|ui| {
        crate::menus::ruler_menu_head(app, ui, "sections");
        section_menu(app, ui, target);
        crate::menus::rulers_submenu(app, ui);
    });
    if let Some(gh) = done {
        commit_section(app, ui, &g, gh, &items);
    }
}

fn commit_section(app: &mut SoundApp, ui: &Ui, g: &Grid, gh: Ghost, items: &[Item]) {
    let (a, b) = (g.samples(gh.start), g.samples(gh.end));
    let Some(id) = gh.id else {
        let l = letter(ui);
        run_with_def(app, l, "band.section.add", json!({"section": l.to_string(), "start": a, "end": b}));
        return;
    };
    let unmoved = items.iter().any(|it| it.id == id && it.start == gh.start && it.end == gh.end);
    if unmoved {
        return;
    }
    let _ = match (gh.copy, gh.resize) {
        (true, _) => app.run("band.section.copy", json!({"block": id, "to": a})),
        (false, true) => app.run("band.section.set", json!({"block": id, "start": a, "end": b})),
        (false, false) => app.run("band.section.set", json!({"block": id, "start": a})),
    };
}

/// A command a menu picked, run once the menu is drawn.
enum Pick {
    Run(&'static str, Value),
    WithDef(char, &'static str, Value),
    Studio(char),
}

fn section_menu(app: &mut SoundApp, ui: &mut Ui, target: Option<u32>) {
    let block = target.and_then(|id| app.engine.session().band.sections.iter().find(|b| b.id == id).cloned());
    let Some(b) = block else {
        // On empty space: the letter drawn blocks take.
        ui.label("Draw sections as");
        let current = letter(ui);
        ui.horizontal(|ui| {
            for l in 'A'..='H' {
                if ui.selectable_label(l == current, l.to_string()).clicked() {
                    set_letter(ui, l);
                    ui.close();
                }
            }
        });
        return;
    };
    let mut pick = None;
    ui.horizontal(|ui| {
        ui.label("Section");
        for l in 'A'..='H' {
            if ui.selectable_label(b.section == l, l.to_string()).clicked() {
                set_letter(ui, l);
                pick = Some(Pick::WithDef(l, "band.section.set", json!({"block": b.id, "section": l.to_string()})));
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label("Energy");
        for e in -3i8..=3 {
            if ui.selectable_label(b.energy == e, format!("{e:+}")).clicked() {
                pick = Some(Pick::Run("band.section.set", json!({"block": b.id, "energy": e})));
            }
        }
    });
    let take = if b.take == 0 { "Section's".to_string() } else { b.take.to_string() };
    ui.menu_button(format!("Take: {take}"), |ui| {
        if ui.selectable_label(b.take == 0, "Section's own").clicked() {
            pick = Some(Pick::Run("band.section.set", json!({"block": b.id, "take": 0})));
        }
        for k in 1u8..=16 {
            if ui.selectable_label(b.take == k, k.to_string()).clicked() {
                pick = Some(Pick::Run("band.section.set", json!({"block": b.id, "take": k})));
            }
        }
    });
    ui.separator();
    let studio = crate::menus::UI_COMMANDS.iter().any(|c| c.0 == "window.studio");
    if ui.add_enabled(studio, egui::Button::new("Edit definition…")).on_disabled_hover_text("Needs the Studio window").clicked() {
        pick = Some(Pick::Studio(b.section));
    }
    if ui.button("Delete").clicked() {
        pick = Some(Pick::Run("band.section.remove", json!({"block": b.id})));
    }
    let Some(pick) = pick else { return };
    ui.close();
    match pick {
        Pick::Run(id, p) => {
            let _ = app.run(id, p);
        }
        Pick::WithDef(l, id, p) => run_with_def(app, l, id, p),
        Pick::Studio(l) => {
            ui.ctx().data_mut(|m| m.insert_temp(Id::new(STUDIO_DEF), l));
            let _ = app.run("window.studio", json!({"value": true}));
        }
    }
}

// ---- Chords -------------------------------------------------------------------------------------

/// The Chords ruler (`band_chords`): the chord lane.
pub fn chords_ruler(app: &mut SoundApp, ui: &mut Ui, painter: &Painter, s: &Session, tl: Rect, row: Rect) {
    let t = Tokens::current();
    let g = Grid::new(s, tl);
    painter.rect_filled(row, 0.0, t.marker_ruler);
    let band = &s.band;
    let (key, scale) = (i32::from(band.key), i32::from(band.scale));
    let items: Vec<Item> = band.chords.iter().map(|c| Item { id: c.id, start: c.tick, end: c.tick.saturating_add(c.length_ticks) }).collect();
    let id = ui.id().with(CHORDS);
    let resp = ui.interact(row, id, Sense::click_and_drag());
    let (live, done) = drag(ui, &resp, &g, &items, id, false);
    let moving = live.and_then(|gh| gh.id);
    let (fill, stroke) = (t.button, t.border);
    // Chords narrower than this are drawn as one run each (thousands fit in a screen).
    let mut run: Option<Rect> = None;
    for c in &band.chords {
        let r = span_rect(&g, row, c.tick, c.tick.saturating_add(c.length_ticks));
        if r.max.x < row.min.x || r.min.x > row.max.x {
            continue;
        }
        if r.width() < 3.0 {
            run = Some(match run {
                Some(acc) if r.min.x <= acc.max.x + 1.0 => acc.union(r),
                Some(acc) => {
                    painter.rect_filled(acc, 0.0, fill);
                    r
                }
                None => r,
            });
            continue;
        }
        let f = if moving == Some(c.id) { fill.gamma_multiply(0.4) } else { fill };
        let picked = app.ui.ruler_item == Some(crate::ruler_items::RulerItem::Chord(c.id));
        block(painter, r, f, if picked { t.accent } else { stroke }, || name_for(i32::from(c.slot), key, scale, i32::from(c.bass)), t.text);
        if picked {
            painter.rect_stroke(r, 2.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
        }
    }
    if let Some(acc) = run {
        painter.rect_filled(acc, 0.0, fill);
    }
    if let Some(gh) = live {
        draw_ghost(painter, &g, row, gh, t.accent);
    }
    flash(ui, painter, &g, row, s);
    edge_cursor(ui, &resp, &g, &items, live);
    // A click picks a chord (Delete removes it).
    if resp.clicked()
        && let Some((it, _)) = resp.interact_pointer_pos().and_then(|p| hit(&items, &g, p.x))
    {
        crate::ruler_items::pick(app, crate::ruler_items::RulerItem::Chord(it.id));
    }
    if resp.double_clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        match hit(&items, &g, p.x) {
            Some((it, _)) => super::chord_picker::open(app, it.start, Some(it.id)),
            None => super::chord_picker::open(app, free_start(&items, g.tick_at(p.x), g.snap(g.tick_at(p.x))), None),
        }
    }
    let target = menu_target(ui, &resp, &g, &items, ui.id().with(MENU).with(CHORDS));
    if resp.secondary_clicked()
        && let Some(id) = target
    {
        crate::ruler_items::pick(app, crate::ruler_items::RulerItem::Chord(id));
    }
    resp.context_menu(|ui| {
        chord_menu(app, ui, target);
        ui.separator();
        crate::menus::ruler_menu_head(app, ui, "band_chords");
        crate::menus::rulers_submenu(app, ui);
    });
    if let Some(gh) = done
        && let Some(cid) = gh.id
        && !items.iter().any(|it| it.id == cid && it.start == gh.start && it.end == gh.end)
    {
        let (a, b) = (g.samples(gh.start), g.samples(gh.end));
        // The ghost is already where the chord goes (snapped, or off the grid with Alt): the engine
        // keeps it exactly, so a resize moves only the edge dragged.
        let p = if gh.resize { json!({"chord": cid, "at": a, "end": b, "snap": false}) } else { json!({"chord": cid, "at": a, "snap": false}) };
        let _ = app.run("band.chord.set", p);
    }
}

/// Where a double-click on empty Chords ruler space at `tick` starts a chord: the half bar
/// `snapped`, kept out of the chords either side (off-grid ones, from Detect Chords "As played",
/// stay as they are): no earlier than the end of the chord before, no later than the next chord's
/// start (else the tick itself).
fn free_start(items: &[Item], tick: i64, snapped: i64) -> i64 {
    let prev_end = items.iter().filter(|it| it.start <= tick).map(|it| it.end).max().unwrap_or(0);
    let next = items.iter().map(|it| it.start).filter(|t| *t > tick).min();
    let at = snapped.max(prev_end);
    match next {
        Some(n) if at >= n => tick.max(prev_end),
        _ => at,
    }
}

/// "≈ nearest in key" beside a typed chord the key doesn't hold, for a moment.
fn flash(ui: &Ui, painter: &Painter, g: &Grid, row: Rect, s: &Session) {
    let fid = ui.id().with(FLASH);
    let request = egui::Id::new(super::chord_picker::FLASH_REQUEST);
    if let Some(id) = ui.ctx().data_mut(|m| m.remove_temp::<u32>(request)) {
        let until = ui.input(|i| i.time) + FLASH_SECS;
        ui.ctx().data_mut(|m| m.insert_temp(fid, (id, until)));
    }
    let Some((chord, until)) = ui.ctx().data(|m| m.get_temp::<(u32, f64)>(fid)) else { return };
    let now = ui.input(|i| i.time);
    let Some(c) = s.band.chords.iter().find(|c| c.id == chord).filter(|_| now < until) else {
        ui.ctx().data_mut(|m| m.remove::<(u32, f64)>(fid));
        return;
    };
    let t = Tokens::current();
    let at = pos2(g.x(c.tick.saturating_add(c.length_ticks)) + 4.0, row.center().y);
    let galley = painter.layout_no_wrap("≈ nearest in key".into(), regular(10.0), t.text);
    let r = Rect::from_min_size(pos2(at.x, row.min.y + 2.0), vec2(galley.size().x + 10.0, row.height() - 4.0));
    painter.rect(r, 3.0, t.accent.gamma_multiply(0.85), Stroke::NONE, StrokeKind::Inside);
    painter.galley(pos2(r.min.x + 5.0, r.center().y - galley.size().y / 2.0), galley, t.text);
    ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((until - now).max(0.0)));
}

/// A chord's right-click menu (Edit Chord…, Delete Chord, the clear items, then Bass); on
/// empty space, the clear items and a hint.
fn chord_menu(app: &mut SoundApp, ui: &mut Ui, target: Option<u32>) {
    let band = &app.engine.session().band;
    let Some(c) = target.and_then(|id| band.chords.iter().find(|c| c.id == id).cloned()) else {
        ui.label("Double-click to add a chord");
        ui.separator();
        crate::ruler_items::chord_clear_items(app, ui);
        return;
    };
    let key = i32::from(band.key);
    let mut pick = None;
    if ui.button("Edit Chord…").clicked() {
        // The same picker a double-click opens, on this chord.
        super::chord_picker::open(app, c.tick, Some(c.id));
        ui.close();
    }
    let delete = ui.add(egui::Button::new("Delete Chord").shortcut_text("Del")).clicked();
    ui.separator();
    crate::ruler_items::chord_clear_items(app, ui);
    ui.separator();
    ui.menu_button("Bass", |ui| {
        if ui.selectable_label(c.bass < 0, "Root").clicked() {
            pick = Some(json!({"chord": c.id, "bass": -1}));
        }
        for b in 0i8..12 {
            if ui.selectable_label(c.bass == b, note_name(key + i32::from(b))).clicked() {
                pick = Some(json!({"chord": c.id, "bass": b}));
            }
        }
    });
    if delete {
        ui.close();
        crate::ruler_items::unpick(app);
        let _ = app.run("band.chord.remove", json!({"chord": c.id}));
    } else if let Some(p) = pick {
        ui.close();
        let _ = app.run("band.chord.set", p);
    }
}
