//! The Band ruler (View ▸ Rulers ▸ Band): the band in the Edit window, where the song is written.
//!
//! - **The lanes:** one thin row per musician under the Sections and Chords rulers, bar by bar on
//!   the timeline, coloured by the Energy they play there ([`band_map`]); hatched where they sit
//!   out, outlined in gold on a kept take, dimmed for a musician no one hears (off, or no track
//!   plays them). Hover a bar for who plays it and how hard.
//! - **The name cell** holds the band's controls: the band preset (load, save, reload the
//!   Library), Natural / Classic, **Feel** (Tension, As a band and the four band switches, the
//!   song feel with its presets, who ignores it), the band's **Take**, **Musicians** (who is on,
//!   on which track and instrument, whether they are heard, and each one's settings) and **New
//!   Band**.
//! - A musician's settings open in a small panel ([`musician_panel`]): the band drawer the Mix
//!   strip's Band key opens, reached here from the Musicians list, the Edit track header's badge
//!   and its right-click menu (`window.musician`).
//! - Track ▸ **Print Band to MIDI...** ([`print_window`], `window.band_print`) and File ▸ Import ▸
//!   **MIDI Chords...** (`ui.band.import_chords`).
//!
//! Every edit is a `band.*` command (one undo step); a dial drag is one step until it ends.

use super::drawer::{BAND_ACCENT, palette};
use super::lanes::letter_color;
use super::{instrument_name, slot_player};
use crate::SoundApp;
use crate::editor_kit::VstPalette;
use crate::editor_kit::controls::{DialOpts, LabelLayout, PillLook, PillStyle, SelectionStyle, dial, pill_ex, segmented_ex};
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Color32, CornerRadius, Id, Painter, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_band::band_map::{BarCell, MARK_KEPT, band_map};
use soundcraft_band::lanes::lanes_to_timeline;
use soundcraft_band::song_feel::{FEEL_DIAL_COUNT, dial_name, dial_positions, dial_title, factory_feel_presets};
use soundcraft_model::{MusicianSlot, Route, Session};
use soundcraft_time::Samples;

/// The Band ruler's height: three rows of controls in its name cell, six lanes beside them.
pub const RULER_H: f32 = 58.0;
/// One musician's lane.
pub const LANE_H: f32 = 9.0;
/// A kept take's outline (MS `col::gold`).
const GOLD: Color32 = Color32::from_rgb(0xD9, 0xA4, 0x41);
const ROW_H: f32 = 17.0;

/// What Print… prints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrintScope {
    /// The whole song, from the lanes' start to its end.
    #[default]
    Song,
    /// One section block.
    Section,
    /// The edit selection.
    Selection,
}

/// Track ▸ Print Band to MIDI...
#[derive(Clone, Debug, Default)]
pub struct PrintUi {
    pub open: bool,
    pub scope: PrintScope,
    /// The section block printed with [`PrintScope::Section`].
    pub block: Option<u32>,
    /// The musicians to print; `None`: every one a track plays.
    pub slots: Option<[bool; 6]>,
}

/// The Library's lists as `band.list` gives them (read when a popover opens and after an edit
/// that changes them, never per frame: it reads the user's files).
#[derive(Clone, Debug, Default)]
pub struct Lists {
    /// Band presets: (name, factory).
    pub presets: Vec<(String, bool)>,
    /// Feel presets: (name, factory).
    pub feel: Vec<(String, bool)>,
    pub songs: Vec<String>,
    /// A user Band folder exists (saving is possible).
    pub user_folder: bool,
}

/// The band's UI state (not saved).
#[derive(Default)]
pub struct BandUi {
    /// The musician (0-5) whose settings panel is open.
    pub musician: Option<u8>,
    pub print: PrintUi,
    /// Names being typed: Save as… (band preset), the feel preset.
    pub preset_name: String,
    pub feel_name: String,
    /// Lay Factory Song: the song replaces what the lanes hold (else it is laid over it).
    pub lay_over: bool,
    lists: Option<Lists>,
}

// ---- shared helpers -----------------------------------------------------------------------------

/// The first track whose input is musician `k` (0-5).
pub fn track_of(s: &Session, k: usize) -> Option<&soundcraft_model::Track> {
    s.tracks.iter().find(|t| t.mixer.input == Route::Musician(k as u8))
}

/// `N · Instrument · Player`.
pub fn musician_name(k: usize, m: &MusicianSlot) -> String {
    format!("{} · {} · {}", k + 1, instrument_name(m.instrument), slot_player(m))
}

/// The band's name as its preset menu shows it.
pub fn band_name(s: &Session) -> &str {
    if s.band.band_name.is_empty() { "Factory Band" } else { s.band.band_name.as_str() }
}

/// The Library's lists, read once until an edit changes them.
pub fn lists(app: &mut SoundApp) -> Lists {
    if app.band_ui.lists.is_none() {
        app.band_ui.lists = Some(read_lists(app));
    }
    app.band_ui.lists.clone().unwrap_or_default()
}

fn read_lists(app: &mut SoundApp) -> Lists {
    let Ok(v) = app.engine.execute("band.list", &json!({})) else { return Lists::default() };
    let named = |key: &str| -> Vec<(String, bool)> {
        v.get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| Some((x.get("name")?.as_str()?.to_string(), x.get("factory").and_then(Value::as_bool).unwrap_or(false))))
                    .collect()
            })
            .unwrap_or_default()
    };
    let songs = v
        .get("factory_songs")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    Lists { presets: named("presets"), feel: named("feel_presets"), songs, user_folder: v.get("user_folder").is_some_and(|f| !f.is_null()) }
}

/// Runs a command; a refusal shows in the status bar.
fn run(app: &mut SoundApp, id: &str, p: Value) -> Option<Value> {
    match app.run(id, p) {
        Ok(v) => Some(v),
        Err(e) => {
            app.ui.status = e;
            None
        }
    }
}

/// `band.settings` merged into the open drag under `key`: one undo step until it ends.
fn settings_gesture(app: &mut SoundApp, key: &str, p: Option<Value>, started: bool, ended: bool) {
    if started {
        app.engine.end_merge();
    }
    if let Some(p) = p
        && let Err(e) = app.engine.execute_merged("band.settings", &p, &format!("band-ruler:{key}"))
    {
        app.ui.status = e.to_string();
    }
    if ended {
        app.engine.end_merge();
    }
}

fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(regular(10.0)).color(Tokens::current().text_dim));
}

fn heading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(bold(10.5)).color(Tokens::current().text_dim));
}

/// A segmented switch `w` wide; the index picked, when it changed.
fn seg(
    ui: &mut Ui,
    w: f32,
    salt: impl std::hash::Hash + std::fmt::Debug,
    pal: &VstPalette,
    options: &[&str],
    selected: Option<usize>,
) -> Option<usize> {
    let (r, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
    segmented_ex(ui, r, salt, pal, options, 1, selected, SelectionStyle::Filled, false).filter(|i| Some(*i) != selected)
}

/// A pill `w` wide.
fn pill(ui: &mut Ui, w: f32, salt: impl std::hash::Hash + std::fmt::Debug, pal: &VstPalette, text: &str, on: bool) -> egui::Response {
    let (r, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
    pill_ex(ui, r, salt, pal, text, on, PillStyle::Normal, PillLook { tracked: false, ..PillLook::default() })
}

/// A list row's text: white on the selection fill (the light theme's selection ink is too close
/// to its fill).
fn picked_text(t: RichText, on: bool) -> RichText {
    if on { t.color(Color32::WHITE) } else { t }
}

/// A small field in the name cell: its text, a ▾ when it opens a popover, named `label` for
/// screen readers and tests.
fn chip(ui: &mut Ui, r: Rect, text: &str, label: &str, arrow: bool) -> egui::Response {
    let t = Tokens::current();
    let resp = ui.interact(r, ui.id().with(("band-chip", label)), Sense::click());
    let name = label.to_string();
    let kind = if arrow { egui::WidgetType::ComboBox } else { egui::WidgetType::Button };
    resp.widget_info(|| egui::WidgetInfo::labeled(kind, true, &name));
    let p = ui.painter();
    let fill = if resp.hovered() { t.graph_bg.lerp_to_gamma(t.module_tile_border, 0.5) } else { t.graph_bg };
    p.rect_filled(r, CornerRadius::same(4), fill);
    let edge = if resp.hovered() { BAND_ACCENT.gamma_multiply(0.7) } else { t.module_tile_border };
    p.rect_stroke(r, CornerRadius::same(4), Stroke::new(1.0, edge), StrokeKind::Inside);
    let text_r = if arrow { r.with_max_x(r.max.x - 9.0) } else { r };
    let at = if arrow { pos2(r.min.x + 4.0, r.center().y) } else { r.center() };
    let align = if arrow { Align2::LEFT_CENTER } else { Align2::CENTER_CENTER };
    p.with_clip_rect(text_r.shrink(1.0)).text(at, align, text, regular(9.5), t.text);
    if arrow {
        let a = pos2(r.max.x - 5.5, r.center().y);
        p.add(Shape::convex_polygon(vec![a + vec2(-3.0, -1.5), a + vec2(3.0, -1.5), a + vec2(0.0, 2.0)], t.text_dim, Stroke::NONE));
    }
    resp
}

// ---- the lanes ------------------------------------------------------------------------------------

/// The Band view's bars: [`band_map`] of the lanes' timeline, the enabled musicians at their own
/// Energy. `None` while the lanes are empty. Bar `b` is the song's bar `b + 1`.
pub fn view_map(s: &Session) -> Option<Vec<BarCell>> {
    let tl = lanes_to_timeline(&soundcraft_mix::lane_song(s)?)?;
    let slots = &s.band.slots;
    let plays: [bool; 6] = std::array::from_fn(|k| slots.get(k).is_some_and(|m| m.enabled));
    let energies: [i32; 6] = std::array::from_fn(|k| slots.get(k).map_or(1, |m| i32::from(m.energy)));
    Some(band_map(&tl, &plays, &energies))
}

/// Whether musician `k` is heard: on, and a track plays them.
pub fn heard(s: &Session, k: usize) -> bool {
    s.band.slots.get(k).is_some_and(|m| m.enabled) && track_of(s, k).is_some()
}

/// Musician `lane`'s cell of the bar from `x0` to `x1` in the ruler's `row`.
pub fn cell_rect(row: Rect, x0: f32, x1: f32, lane: usize) -> Rect {
    let y = row.min.y + 2.0 + lane as f32 * LANE_H;
    Rect::from_min_max(pos2(x0 + 0.5, y + 0.5), pos2((x1 - 0.5).max(x0 + 1.5), y + LANE_H - 0.5))
}

/// A cell's colour: the band's accent, stronger with the Energy played there; a sat-out bar is
/// the border grey; a musician no one hears is dimmed.
pub fn cell_fill(cell: &BarCell, lane: usize, heard: bool) -> Color32 {
    if cell.sat_out(lane) {
        return Tokens::current().border;
    }
    let e = cell.energy.get(lane).copied().unwrap_or(0).clamp(0, 4) as f32;
    let c = BAND_ACCENT.gamma_multiply(0.12 + 0.17 * e);
    if heard { c } else { c.gamma_multiply(0.35) }
}

/// The ticks of one bar (the lanes' grid).
fn bar_ticks(s: &Session) -> i64 {
    (2 * soundcraft_mix::half_ticks(s)).max(1)
}

/// The Band ruler's lanes in `row` of the timeline `tl`.
pub fn lanes(app: &mut SoundApp, ui: &mut Ui, painter: &Painter, s: &Session, tl: Rect, row: Rect) {
    let t = Tokens::current();
    painter.rect_filled(row, 0.0, t.marker_ruler);
    let resp = ui.interact(row, ui.id().with("band_ruler"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Band view"));
    let cells = view_map(s);
    let bar = bar_ticks(s);
    let x_of_tick = |tick: i64| crate::edit_window::x_of(s, tl, s.tempo.tick_to_samples(tick, s.sample_rate));
    match &cells {
        None => {
            let text = "The band plays nothing yet: lay a song (right-click the Sections ruler ▸ Lay Factory Song), or press New Band";
            painter.text(pos2(tl.min.x + 6.0, row.center().y), Align2::LEFT_CENTER, text, regular(10.5), t.text_dim);
        }
        Some(cells) => {
            let heard: [bool; 6] = std::array::from_fn(|k| heard(s, k));
            for (b, c) in cells.iter().enumerate() {
                let b = b as i64;
                let (x0, x1) = (x_of_tick(b.saturating_mul(bar)), x_of_tick(b.saturating_add(1).saturating_mul(bar)));
                if x1 < row.min.x || x0 > row.max.x {
                    continue;
                }
                for (k, on) in heard.iter().enumerate() {
                    let cell = cell_rect(row, x0, x1, k);
                    painter.rect_filled(cell, 0.0, cell_fill(c, k, *on));
                    if c.sat_out(k) {
                        let hatch = painter.with_clip_rect(cell.intersect(painter.clip_rect()));
                        let mut hx = cell.min.x - cell.height();
                        while hx < cell.max.x {
                            hatch.line_segment(
                                [pos2(hx, cell.max.y), pos2(hx + cell.height(), cell.min.y)],
                                Stroke::new(0.6, t.text_dim.gamma_multiply(0.35)),
                            );
                            hx += 4.0;
                        }
                    } else if c.marks.get(k).is_some_and(|mk| mk & MARK_KEPT != 0) {
                        painter.rect_stroke(cell, 0.0, Stroke::new(1.0, GOLD), StrokeKind::Inside);
                    }
                }
                // A section's first bar: a hairline in its letter's colour.
                if b == 0 || cells.get(b as usize - 1).is_some_and(|prev| prev.section != c.section) {
                    painter.rect_filled(Rect::from_min_size(pos2(x0, row.min.y), vec2(1.0, row.height())), 0.0, letter_color(c.section));
                }
            }
        }
    }
    // Hover: who plays the bar and how hard.
    if let (Some(cells), Some(p)) = (&cells, resp.hover_pos()) {
        let tick = s.tempo.samples_to_ticks(crate::edit_window::sample_at(s, tl, p.x).max(0), s.sample_rate);
        let lane = ((p.y - row.min.y - 2.0) / LANE_H).floor();
        if let (Some(c), Some(k)) = (usize::try_from(tick / bar).ok().and_then(|b| cells.get(b)), (0.0..6.0).contains(&lane).then_some(lane as usize))
            && let Some(m) = s.band.slots.get(k)
        {
            let what = if c.sat_out(k) {
                "sits out".to_string()
            } else {
                format!(
                    "Energy {}{}",
                    c.energy.get(k).copied().unwrap_or(0),
                    if c.energy_add != 0 { format!(" (section {:+})", c.energy_add) } else { String::new() }
                )
            };
            let heard_text = if heard(s, k) { "" } else { " · not heard (off, or no track plays them)" };
            resp.clone().on_hover_text(format!("Bar {} · {}: {what}{heard_text}", tick / bar + 1, musician_name(k, m)));
        }
    }
    resp.context_menu(|ui| {
        crate::menus::ruler_menu_head(app, ui, "band");
        ruler_menu(app, ui);
        crate::menus::rulers_submenu(app, ui);
    });
}

/// The Band ruler's own right-click items.
fn ruler_menu(app: &mut SoundApp, ui: &mut Ui) {
    if ui.button("New Band").on_hover_text("A track for each musician who is on and has none, playing them").clicked() {
        run(app, "track.new_band", json!({}));
        ui.close();
    }
    if ui.button("Print Band to MIDI...").clicked() {
        run(app, "window.band_print", json!({"value": true}));
        ui.close();
    }
}

// ---- the name cell --------------------------------------------------------------------------------

/// The Band ruler's name cell, `cell` (the ruler's row left of the timeline): the band preset,
/// Natural / Classic, Feel, Take, Musicians and New Band.
pub fn name_cell(app: &mut SoundApp, ui: &mut Ui, cell: Rect) {
    let t = Tokens::current();
    let s = app.engine.session_arc();
    let pal = palette();
    let inner = cell.shrink2(vec2(4.0, 2.0));
    let mut c = ui.new_child(UiBuilder::new().id_salt("band-ruler-name").max_rect(inner));
    let ui = &mut c;
    let row = |i: f32| Rect::from_min_size(pos2(inner.min.x, inner.min.y + i * (ROW_H + 2.0)), vec2(inner.width(), ROW_H));
    let gap = 3.0;

    // Row 1: the band preset.
    let r1 = row(0.0);
    let resp = chip(ui, r1, &format!("♪ {}", band_name(&s)), "Band preset", true)
        .on_hover_text("The band: load a band preset, save yours, reload the Library");
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| preset_popover(app, ui));

    // Row 2: Natural / Classic, Feel.
    let r2 = row(1.0);
    let feel_w = 42.0;
    let nat = Rect::from_min_max(r2.min, pos2(r2.max.x - feel_w - gap, r2.max.y));
    let natural = s.band.feel.natural;
    if let Some(i) =
        segmented_ex(ui, nat, "band-ruler-natural", &pal, &["Natural", "Classic"], 1, Some(usize::from(!natural)), SelectionStyle::Filled, false)
            .filter(|i| *i != usize::from(!natural))
    {
        run(app, "band.settings", json!({"feel": {"natural": i == 0}}));
    }
    let fr = Rect::from_min_max(pos2(r2.max.x - feel_w, r2.min.y), r2.max);
    let resp = chip(ui, fr, "Feel", "Band feel", true).on_hover_text("Tension, As a band, the band switches, the song feel and who ignores it");
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| feel_popover(app, ui, &pal));

    // Row 3: Take, Musicians, New Band.
    let r3 = row(2.0);
    let take_w = 38.0;
    let mus_w = 40.0;
    let tr = Rect::from_min_size(r3.min, vec2(take_w, ROW_H));
    let resp = chip(ui, tr, &format!("Take {}", s.band.take), "Band take", true).on_hover_text("The band's take: another take plays other ideas");
    egui::Popup::menu(&resp).show(|ui| {
        for k in 1u8..=16 {
            if ui.selectable_label(s.band.take == k, format!("Take {k}")).clicked() {
                run(app, "band.settings", json!({"take": k}));
                ui.close();
            }
        }
    });
    let mr = Rect::from_min_size(pos2(tr.max.x + gap, r3.min.y), vec2(mus_w, ROW_H));
    let on = s.band.slots.iter().filter(|m| m.enabled).count();
    let heard_n = (0..6).filter(|k| heard(&s, *k)).count();
    let resp = chip(ui, mr, &format!("{heard_n}/{on}"), "Musicians", true).on_hover_text(format!(
        "Musicians: {heard_n} of the {on} who are on are heard. Who plays, on which track and instrument, and their settings"
    ));
    if heard_n < on {
        ui.painter().circle_filled(pos2(mr.max.x - 2.0, mr.min.y + 2.0), 2.5, t.meter_yellow);
    }
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| musicians_popover(app, ui, &pal));
    let nr = Rect::from_min_max(pos2(mr.max.x + gap, r3.min.y), r3.max);
    let resp = chip(ui, nr, "+ Band", "New Band", false);
    let tip = if heard_n < on || on == 0 {
        "New Band: a track for each musician who is on and has none, on an instrument that sounds; with empty lanes it lays a song too"
    } else {
        "Every musician who is on has a track"
    };
    if resp.on_hover_text(tip).clicked() {
        run(app, "track.new_band", json!({}));
    }
}

// ---- band presets ---------------------------------------------------------------------------------

fn preset_popover(app: &mut SoundApp, ui: &mut Ui) {
    let lists = lists(app);
    let band_name = band_name(app.engine.session()).to_string();
    ui.set_min_width(260.0);
    heading(ui, "BAND PRESETS");
    caption(ui, "The six musicians, their instruments, players, targets and settings");
    egui::ScrollArea::vertical().id_salt("band-ruler-presets").max_height(280.0).show(ui, |ui| {
        for (name, factory) in &lists.presets {
            let loaded = *name == band_name;
            let resp = ui.add(egui::Button::selectable(loaded, picked_text(RichText::new(name.as_str()), loaded)));
            let resp = if *factory { resp.on_hover_text("Factory: click to load") } else { resp.on_hover_text("Click to load") };
            if resp.clicked() && !loaded {
                run(app, "band.preset.load", json!({"name": name}));
            }
        }
    });
    ui.separator();
    ui.horizontal(|ui| {
        caption(ui, "Save as…");
        ui.add(egui::TextEdit::singleline(&mut app.band_ui.preset_name).hint_text("Band name").desired_width(120.0));
        let name = app.band_ui.preset_name.trim().to_string();
        let exists = lists.presets.iter().any(|(n, f)| !*f && n.eq_ignore_ascii_case(&name));
        let label = if exists { "Replace" } else { "Save" };
        if ui
            .add_enabled(lists.user_folder && !name.is_empty(), egui::Button::new(label))
            .on_disabled_hover_text("Name it (needs the user Band folder)")
            .clicked()
            && run(app, "band.preset.save", json!({"name": name, "replace": exists})).is_some()
        {
            app.band_ui.lists = None;
            app.band_ui.preset_name.clear();
        }
    });
    if ui.button("Reload Library").on_hover_text("Read the players, targets and presets again").clicked()
        && let Some(v) = run(app, "band.library.reload", json!({}))
    {
        app.band_ui.lists = None;
        let errors = v.get("errors").and_then(Value::as_array).map_or(0, Vec::len);
        app.ui.status = if errors == 0 { "Library reloaded".into() } else { format!("Library reloaded: {errors} file(s) skipped") };
    }
}

// ---- feel -----------------------------------------------------------------------------------------

fn feel_popover(app: &mut SoundApp, ui: &mut Ui, pal: &VstPalette) {
    let band = app.engine.session().band.clone();
    let f = band.feel.clone();
    ui.set_min_width(520.0);
    heading(ui, "BAND FEEL");
    ui.horizontal(|ui| {
        if let Some(i) = seg(ui, 160.0, "band-feel-natural", pal, &["Natural", "Classic"], Some(usize::from(!f.natural))) {
            run(app, "band.settings", json!({"feel": {"natural": i == 0}}));
        }
        ui.add_space(8.0);
        if pill(ui, 80.0, "band-feel-tension", pal, "Tension", f.tension).on_hover_text("The band leans into the chords' tension").clicked() {
            run(app, "band.settings", json!({"feel": {"tension": !f.tension}}));
        }
        let tip = "Natural only: one pulse, phrases that breathe, fills the others answer";
        if pill(ui, 96.0, "band-feel-as-a-band", pal, "As a band", f.band && f.natural).on_hover_text(tip).clicked() {
            run(app, "band.settings", json!({"feel": {"band": !(f.band && f.natural), "natural": true}}));
        }
    });
    ui.add_space(4.0);
    caption(ui, "The band plays together");
    ui.horizontal(|ui| {
        let switches = [
            ("Bass lock", "bass_lock", f.bass_lock, "The bass locks to the kick"),
            ("Fills yield", "fills_yield", f.fills_yield, "One fill at a time: the others hold back"),
            ("Leave space", "leave_space", f.leave_space, "Musicians leave room for each other"),
            ("Dynamics together", "dynamics", f.dynamics, "The band rises and falls together"),
        ];
        for (label, key, on, tip) in switches {
            if pill(ui, 118.0, ("band-feel", key), pal, label, on).on_hover_text(tip).clicked() {
                run(app, "band.settings", json!({"feel": {key: !on}}));
            }
        }
    });
    ui.separator();
    heading(ui, "SONG FEEL");
    song_feel(app, ui, pal);
}

fn song_feel(app: &mut SoundApp, ui: &mut Ui, pal: &VstPalette) {
    let band = app.engine.session().band.clone();
    let sf = band.song_feel.clone();
    let dials = [sf.time, sf.density, sf.pocket, sf.groove, sf.touch];
    let lists = lists(app);
    ui.horizontal(|ui| {
        if pill(ui, 52.0, "band-sf-on", pal, "On", sf.on).on_hover_text("The song's feel shapes how the band plays").clicked() {
            run(app, "band.settings", json!({"song_feel": {"on": !sf.on}}));
        }
        ui.add_space(8.0);
        // Off, the dials don't play: no preset is in use.
        let current = factory_feel_presets()
            .iter()
            .find(|p| [p.time, p.density, p.pocket, p.groove, p.touch] == dials)
            .filter(|_| sf.on)
            .map_or_else(|| if sf.on { "Custom" } else { "None (off)" }.to_string(), |p| p.name.to_string());
        caption(ui, "Preset");
        egui::ComboBox::from_id_salt("band-feel-preset").selected_text(current.as_str()).width(170.0).show_ui(ui, |ui| {
            for (group, factory) in [("Factory", true), ("Yours", false)] {
                let names: Vec<&str> = lists.feel.iter().filter(|(_, f)| *f == factory).map(|(n, _)| n.as_str()).collect();
                if names.is_empty() {
                    continue;
                }
                ui.label(RichText::new(group).font(bold(10.0)).color(Tokens::current().text_dim));
                for n in names {
                    if ui.selectable_label(n == current, n).clicked() {
                        run(app, "band.settings", json!({"feel_preset": n}));
                        app.band_ui.feel_name = if factory { String::new() } else { n.to_string() };
                    }
                }
            }
        });
    });
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut app.band_ui.feel_name).hint_text("Preset name").desired_width(150.0));
        let name = app.band_ui.feel_name.trim().to_string();
        if ui
            .add_enabled(lists.user_folder && !name.is_empty(), egui::Button::new("Save"))
            .on_disabled_hover_text("Name it (needs the user Band folder)")
            .clicked()
            && run(app, "band.feel_preset.save", json!({"name": name})).is_some()
        {
            app.band_ui.lists = None;
        }
        let user = lists.feel.iter().any(|(n, f)| !*f && n.eq_ignore_ascii_case(&name));
        if ui.add_enabled(user, egui::Button::new("Delete")).on_disabled_hover_text("Type the name of one of your presets").clicked()
            && run(app, "band.feel_preset.delete", json!({"name": name})).is_some()
        {
            app.band_ui.lists = None;
            app.band_ui.feel_name.clear();
        }
    });
    ui.add_space(4.0);
    // The five dials, at their positions (MS `dialName`).
    let (r, _) = ui.allocate_exact_size(vec2(5.0 * 96.0, 78.0), Sense::hover());
    let cw = r.width() / 5.0;
    for d in 0..FEEL_DIAL_COUNT {
        let n = dial_positions(d).max(2);
        let cur = dials.get(d as usize).copied().map_or(0, i32::from).clamp(0, n - 1);
        let cell = Rect::from_min_size(pos2(r.min.x + cw * d as f32, r.min.y), vec2(cw, 76.0)).shrink2(vec2(10.0, 0.0));
        let o = DialOpts {
            label: dial_title(d),
            value_text: dial_name(d, cur),
            default: 0.0,
            layout: LabelLayout::Below,
            small: true,
            inactive: !sf.on,
            ..DialOpts::default()
        };
        let out = dial(ui, cell, ("band-sf-dial", d), pal, cur as f32 / (n - 1) as f32, &o);
        let pos = out.value.map(|v| (v * (n - 1) as f32).round().clamp(0.0, (n - 1) as f32) as i32).filter(|p| *p != cur);
        let p = pos.map(|p| json!({"song_feel": {dial_title(d): p}}));
        settings_gesture(app, &format!("dial{d}"), p, out.started, out.ended);
    }
    caption(ui, "Who ignores the feel (plays as set)");
    ui.horizontal_wrapped(|ui| {
        for (k, m) in band.slots.iter().enumerate() {
            let mut on = band.feel_ignore & (1 << k) != 0;
            if crate::widgets::checkbox(ui, &mut on, musician_name(k, m)).changed() {
                let mask = if on { band.feel_ignore | (1 << k) } else { band.feel_ignore & !(1 << k) };
                let list: Vec<usize> = (0..6).filter(|j| mask & (1 << j) != 0).map(|j| j + 1).collect();
                run(app, "band.settings", json!({"feel_ignore": list}));
            }
        }
    });
}

// ---- musicians ------------------------------------------------------------------------------------

/// How a musician is heard, in a few words, and whether that is a problem the user should fix.
pub fn musician_status(s: &Session, lib: &soundcraft_band::library::LibraryData, k: usize) -> (String, bool) {
    let Some(m) = s.band.slots.get(k) else { return (String::new(), false) };
    if !m.enabled {
        return ("Off: not heard".into(), false);
    }
    let Some(track) = track_of(s, k) else { return ("No track plays them: not heard".into(), true) };
    if track.mixer.mute || m.mute {
        return ("Muted".into(), false);
    }
    let drums = matches!(m.instrument, 5 | 11);
    let plugin = track.instrument.as_ref().map(|i| i.plugin.as_str()).unwrap_or("");
    let drum_sound = matches!(plugin, "juke" | "drum_synth") || super::target_slot(lib, m, track).is_some_and(|t| t != 0);
    if drums && !drum_sound {
        return ("Drums on a pitched instrument: give the track Juke, Drum Synth or a drum plug-in".into(), true);
    }
    if super::loaded_instrument(track).is_empty() {
        return ("The track has no instrument: nothing is heard".into(), true);
    }
    ("Plays".into(), false)
}

/// The track's instrument as the musicians list shows it: its name, and the preset it loaded.
fn instrument_text(track: &soundcraft_model::Track) -> String {
    let name = super::loaded_instrument(track);
    match track.instrument.as_ref().map(|i| i.preset.as_str()).filter(|p| !p.is_empty() && !p.starts_with('<')) {
        Some(p) if !name.is_empty() => format!("{name} · {p}"),
        _ if name.is_empty() => "No instrument".into(),
        _ => name,
    }
}

/// After `track.new_band` (menu, button or control): the status says how many tracks were made and
/// how many play a built-in fallback (the engine has shown the Band, Sections and Chords rulers).
pub fn after_new_band(app: &mut SoundApp, v: &Value) {
    let list = v.get("musicians").and_then(Value::as_array).cloned().unwrap_or_default();
    let built_in = list.iter().filter(|m| m.get("installed").and_then(Value::as_bool) == Some(false)).count();
    let song = v.get("song").and_then(Value::as_str).map(|s| format!("; laid “{s}”")).unwrap_or_default();
    app.ui.status = if built_in == 0 {
        format!("New Band: {} track(s) on your plug-ins{song}. Press Play.", list.len())
    } else {
        format!(
            "New Band: {} track(s), {built_in} on built-in instruments (no installed plug-in the Library knows was found){song}. Press Play.",
            list.len()
        )
    };
}

/// The Musicians popover: with no track playing the band, what to do first; else every musician,
/// whether they are heard, on which track and instrument, and the way to their settings.
fn musicians_popover(app: &mut SoundApp, ui: &mut Ui, pal: &VstPalette) {
    let s = app.engine.session_arc();
    let lib = app.engine.band_library();
    let t = Tokens::current();
    heading(ui, "MUSICIANS");
    if !(0..6).any(|k| track_of(&s, k).is_some()) {
        ui.set_max_width(420.0);
        ui.label(RichText::new("No track plays the band yet.").font(bold(13.0)).color(t.text));
        ui.label(
            RichText::new(
                "New Band makes a track for each musician who is on, with an instrument that sounds: your MODO DRUM, MODO BASS, \
                 Pianoteq… when the plug-in scan found them, else a built-in one. When the Sections and Chords rulers are empty it \
                 lays a verse–chorus song on them too. Then press Play. One Undo takes it all back.",
            )
            .font(regular(11.0))
            .color(t.text_dim),
        );
        if ui.add(egui::Button::new(RichText::new("New Band").font(bold(12.0))).min_size(vec2(140.0, 26.0))).clicked() {
            run(app, "track.new_band", json!({}));
        }
        caption(ui, "Or give any instrument track a musician yourself: its input menu ▸ Musicians.");
        return;
    }
    let unheard = (0..6).filter(|k| s.band.slots.get(*k).is_some_and(|m| m.enabled) && track_of(&s, *k).is_none()).count();
    if unheard > 0 {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{unheard} musician(s) who are on have no track and are not heard.")).font(regular(11.0)).color(t.text));
            if ui.button("Add their tracks").on_hover_text("New Band: a track for each musician who is on and has none").clicked() {
                run(app, "track.new_band", json!({}));
            }
        });
    }
    egui::Grid::new("band-ruler-musicians").num_columns(6).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
        for h in ["", "Musician", "Track", "Instrument", "Instrument plug-in", ""] {
            caption(ui, h);
        }
        ui.end_row();
        for (k, m) in s.band.slots.iter().enumerate() {
            let on = m.enabled;
            let resp = pill(ui, 50.0, ("band-ruler-on", k), pal, if on { "On" } else { "Off" }, on);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, format!("Musician {} on", k + 1)));
            if resp.on_hover_text("Switch this musician on or off").clicked() {
                run(app, "band.slot", json!({"slot": k + 1, "enabled": !on}));
            }
            let (status, problem) = musician_status(&s, &lib, k);
            ui.vertical(|ui| {
                ui.add(egui::Label::new(RichText::new(musician_name(k, m)).font(bold(11.0)).color(if on { t.text } else { t.text_dim })).extend());
                ui.add(egui::Label::new(RichText::new(&status).font(regular(10.0)).color(if problem { t.meter_yellow } else { t.text_dim })).extend());
            });
            let track = track_of(&s, k);
            match track {
                Some(tr) => {
                    ui.label(RichText::new(&tr.name).font(regular(11.0)));
                    ui.label(RichText::new(instrument_text(tr)).font(regular(11.0)));
                    ui.label(RichText::new(super::drawer::target_text(&lib, m, tr)).font(regular(11.0)).color(t.text_dim)).on_hover_text(
                        "The plug-in the musician's notes are shaped for: Auto picks it by the track's instrument; Generic sends plain General MIDI notes",
                    );
                }
                None => {
                    for _ in 0..3 {
                        ui.label(RichText::new("—").color(t.text_dim));
                    }
                }
            }
            let open = app.band_ui.musician == Some(k as u8);
            let b = ui.add_enabled(track.is_some(), egui::Button::selectable(open, picked_text(RichText::new("Settings"), open)));
            let b = b.on_hover_text("This musician's player, Energy, feel dials and instrument plug-in").on_disabled_hover_text("No track plays this musician: add their track first");
            b.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Musician {} settings", k + 1)));
            if b.clicked() {
                app.band_ui.musician = if open { None } else { Some(k as u8) };
            }
            ui.end_row();
        }
    });
}

/// The text of a track header's musician badge: `♪ 2 · Benny`.
pub fn badge_text(k: u8, player: &str) -> String {
    format!("♪ {} · {player}", k + 1)
}

/// The badge as the header draws it, compact for the name plate: `♪2 Benny`.
pub fn badge_short(k: u8, player: &str) -> String {
    format!("♪{} {player}", k + 1)
}

/// The musician badge at the end of an Edit track header's name plate `name_r` (musician `k`, 0-5):
/// a click opens their settings, a right-click the track's menu. Returns what is left of the name
/// plate (at least half of it: a long player name is clipped).
pub fn header_badge(app: &mut SoundApp, ui: &mut Ui, track: &soundcraft_model::Track, name_r: Rect, k: u8, player: &str) -> Rect {
    let t = Tokens::current();
    // Dark ink on the solid accent: readable on a selected (light) name plate and on a plain one.
    let ink = Color32::from_rgb(0x1A, 0x14, 0x33);
    let galley = ui.painter().layout_no_wrap(badge_short(k, player), bold(9.0), ink);
    let w = (galley.size().x + 8.0).min(name_r.width() * 0.55);
    let r = Rect::from_min_max(pos2(name_r.max.x - w, name_r.min.y + 1.0), pos2(name_r.max.x, name_r.max.y - 1.0));
    let resp = ui.interact(r, ui.id().with(("band-badge", track.id.0)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{} musician {}", track.name, k + 1)));
    let on = app.band_ui.musician == Some(k);
    let fill = if on || resp.hovered() { BAND_ACCENT } else { BAND_ACCENT.lerp_to_gamma(Color32::WHITE, 0.25) };
    ui.painter().rect(r, CornerRadius::same(7), fill, Stroke::new(1.0, BAND_ACCENT.lerp_to_gamma(t.text_dark, 0.3)), StrokeKind::Inside);
    ui.painter().with_clip_rect(r.shrink(1.0)).galley(pos2(r.min.x + 4.0, r.center().y - galley.size().y / 2.0), galley, ink);
    let resp =
        resp.on_hover_text(format!("Plays musician {}: click for their settings (player, Energy, feel, instrument plug-in)", badge_text(k, player)));
    if resp.clicked() {
        let p = if on { json!({"value": false}) } else { json!({"value": true, "musician": k + 1}) };
        run(app, "window.musician", p);
    }
    resp.context_menu(|ui| crate::edit_window::track_context_menu(app, ui, track.id, crate::MainWindow::Edit));
    Rect::from_min_max(name_r.min, pos2(r.min.x - 2.0, name_r.max.y))
}

// ---- windows: a musician's settings, Print ---------------------------------------------------------

/// `window.musician {musician?: 1-6, value?}`: opens (or with `value: false` closes) musician
/// N's settings panel; without `musician`, toggles the panel that is open.
pub fn musician_command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let k = match p.get("musician") {
        None => app.band_ui.musician,
        Some(v) => Some(
            v.as_u64().filter(|n| (1..=6).contains(n)).map(|n| n as u8 - 1).ok_or_else(|| format!("window.musician: `musician` is 1-6 (got {v})"))?,
        ),
    };
    let show = p.get("value").and_then(Value::as_bool).unwrap_or(p.get("musician").is_some() || app.band_ui.musician.is_none());
    app.band_ui.musician = if show { k } else { None };
    Ok(json!({"value": app.band_ui.musician.is_some(), "musician": app.band_ui.musician.map(|k| k + 1)}))
}

/// `window.band_print {value?}`: shows, hides or toggles Print Band to MIDI.
pub fn print_command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let show = p.get("value").and_then(Value::as_bool).unwrap_or(!app.band_ui.print.open);
    if show && !app.band_ui.print.open {
        app.band_ui.print = PrintUi { open: true, ..PrintUi::default() };
    }
    app.band_ui.print.open = show;
    Ok(json!({"value": show}))
}

/// `ui.band.import_chords`: File ▸ Import ▸ MIDI Chords...: chords from a MIDI file, read in the
/// song's key, onto the Chords ruler from the insertion point (one undo step).
pub fn import_chords(app: &mut SoundApp) -> Result<Value, String> {
    let Some(path) = app.services.pick_open.as_ref().and_then(|open| open("band.chords.import", &["mid", "midi"])) else {
        return Ok(json!({"chords": 0}));
    };
    let at = app.engine.session().edit.selection.start.max(0);
    let v = app.run("band.chords.import", json!({"path": path, "at": at}))?;
    let n = v.get("chords").and_then(Value::as_u64).unwrap_or(0);
    app.ui.status = format!("{n} chord(s) imported onto the Chords ruler");
    Ok(v)
}

/// The windows the band opens: a musician's settings and Print Band to MIDI.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    musician_panel(app, ctx);
    print_window(app, ctx);
}

/// A musician's settings: the band drawer, in a small panel.
fn musician_panel(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(k) = app.band_ui.musician else { return };
    let s = app.engine.session_arc();
    let Some(track) = track_of(&s, usize::from(k)) else {
        app.band_ui.musician = None;
        return;
    };
    let title =
        s.band.slots.get(usize::from(k)).map_or_else(|| format!("Musician {}", k + 1), |m| format!("Musician {}", musician_name(usize::from(k), m)));
    let mut open = true;
    egui::Window::new(title)
        .id(Id::new("band_musician_panel"))
        .open(&mut open)
        .default_size(vec2(super::BAND_DRAWER_W + 16.0, 620.0))
        .resizable(false)
        .show(ctx, |ui| super::drawer(app, ui, track, k, true));
    if !open {
        app.band_ui.musician = None;
    }
}

/// The bar (from 1) holding tick `tick`.
fn bar_of_tick(s: &Session, tick: i64) -> i64 {
    tick.div_euclid(bar_ticks(s)) + 1
}

fn print_window(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.band_ui.print.open {
        return;
    }
    let mut open = true;
    egui::Window::new("Print Band to MIDI").id(Id::new("band_print")).open(&mut open).resizable(false).default_width(420.0).show(ctx, |ui| {
        print_panel(app, ui, &palette());
    });
    if !open {
        app.band_ui.print.open = false;
    }
}

fn print_panel(app: &mut SoundApp, ui: &mut Ui, pal: &VstPalette) {
    let t = Tokens::current();
    let s = app.engine.session_arc();
    ui.label(
        RichText::new("Writes what the band plays as ordinary MIDI clips on the tracks that play it; those tracks then play the clips.")
            .font(regular(10.5))
            .color(t.text_dim),
    );
    ui.add_space(6.0);
    let scopes = [PrintScope::Song, PrintScope::Section, PrintScope::Selection];
    let cur = scopes.iter().position(|x| *x == app.band_ui.print.scope);
    let w = 300.0;
    if let Some(i) = seg(ui, w, "band-print-scope", pal, &["Song", "Section", "Selection"], cur) {
        app.band_ui.print.scope = scopes.get(i).copied().unwrap_or_default();
    }
    ui.add_space(4.0);
    let block_text = |id: u32| {
        s.band.sections.iter().find(|b| b.id == id).map(|b| {
            let name = s.band.def(b.section).map(|d| d.name.as_str()).unwrap_or("");
            format!(
                "{} · {name} · bars {}–{}",
                b.section,
                bar_of_tick(&s, b.start_tick),
                bar_of_tick(&s, b.start_tick.saturating_add(b.length_ticks) - 1)
            )
        })
    };
    let ready = match app.band_ui.print.scope {
        PrintScope::Song => true,
        PrintScope::Section => {
            if app.band_ui.print.block.and_then(block_text).is_none() {
                app.band_ui.print.block = s.band.sections.iter().min_by_key(|b| b.start_tick).map(|b| b.id);
            }
            let shown = app.band_ui.print.block.and_then(block_text).unwrap_or_else(|| "No section blocks".into());
            let mut blocks: Vec<_> = s.band.sections.iter().collect();
            blocks.sort_by_key(|b| b.start_tick);
            egui::ComboBox::from_id_salt("band-print-block").selected_text(shown).width(w).show_ui(ui, |ui| {
                for b in blocks {
                    if ui.selectable_label(app.band_ui.print.block == Some(b.id), block_text(b.id).unwrap_or_default()).clicked() {
                        app.band_ui.print.block = Some(b.id);
                    }
                }
            });
            app.band_ui.print.block.is_some()
        }
        PrintScope::Selection => {
            let sel = s.edit.selection;
            let ok = sel.end > sel.start;
            let text = if ok {
                let tick = |x: Samples| s.tempo.samples_to_ticks(x, s.sample_rate);
                format!("Bars {}–{} of the edit selection", bar_of_tick(&s, tick(sel.start)), bar_of_tick(&s, tick(sel.end) - 1))
            } else {
                "Select a range in the Edit window first".into()
            };
            ui.label(RichText::new(text).font(regular(10.5)).color(t.text_dim));
            ok
        }
    };
    ui.add_space(4.0);
    caption(ui, "Musicians");
    let played: [bool; 6] = std::array::from_fn(|k| track_of(&s, k).is_some());
    let mut slots = app.band_ui.print.slots.unwrap_or(played);
    ui.horizontal_wrapped(|ui| {
        for (k, m) in s.band.slots.iter().enumerate() {
            let on = slots.get(k).copied().unwrap_or(false);
            let resp = pill(ui, 92.0, ("band-print-slot", k), pal, &format!("{} · {}", k + 1, slot_player(m)), on);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, format!("Print musician {}", k + 1)));
            let resp = if played.get(k).copied().unwrap_or(false) { resp } else { resp.on_hover_text("No track plays this musician") };
            if resp.clicked()
                && let Some(x) = slots.get_mut(k)
            {
                *x = !*x;
                app.band_ui.print.slots = Some(slots);
            }
        }
    });
    ui.add_space(6.0);
    let chosen: Vec<usize> = (0..6).filter(|k| slots.get(*k).copied().unwrap_or(false)).map(|k| k + 1).collect();
    if ui.add_enabled(ready && !chosen.is_empty(), egui::Button::new("Print")).clicked() {
        let mut p = match app.band_ui.print.scope {
            PrintScope::Song => json!({"song": true}),
            PrintScope::Section => json!({"block": app.band_ui.print.block}),
            PrintScope::Selection => json!({"range": {"start": s.edit.selection.start, "end": s.edit.selection.end}}),
        };
        if let Some(o) = p.as_object_mut() {
            o.insert("slots".into(), json!(chosen));
        }
        if let Some(v) = run(app, "band.print", p) {
            let clips = v.get("clips").and_then(Value::as_array).map_or(0, Vec::len);
            let replaced = v.get("replaced").and_then(Value::as_array).map_or(0, Vec::len);
            app.ui.status = if replaced > 0 {
                format!("Printed {clips} clip(s); {replaced} clip(s) under them replaced")
            } else {
                format!("Printed {clips} clip(s)")
            };
            app.band_ui.print = PrintUi::default();
        }
    }
}
