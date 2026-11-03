//! Window ▸ Studio: the band's song-wide controls, as MIDI Studio's Song tab and Band view hold
//! them.
//!
//! - Always on top: **Key** and **Scale**, shared by the whole song.
//! - **Song:** the Band view (bars × the six musicians from [`band_map`], coloured by Energy, the
//!   section strip above, the playhead while playing), the factory songs ("Lay onto lanes at bar
//!   N"), **Import MIDI chords…** and **Print…** (Song, a Section block or the Selection; which
//!   musicians).
//! - **Sections:** the definitions A–H: name, Energy shift, shape, role, take, who plays and each
//!   one's Energy, and the four feel dials a section can override (Time is the song's alone).
//! - **Feel:** Natural / Classic, Tension and the band feel switches (the Mod wheel waits for
//!   MIDI input); the song feel: On, its five dials, presets (Save, Delete) and who ignores it.
//! - **Band:** the band presets: Load, Save as… and Reload Library.
//!
//! Every edit is a `band.*` command; a dial or value-box drag is one undo step.

use super::drawer::{BAND_ACCENT, field, palette};
use super::lanes::{STUDIO_DEF, letter_color};
use super::{instrument_name, slot_player};
use crate::SoundApp;
use crate::editor_kit::VstPalette;
use crate::editor_kit::controls::{
    DialOpts, LabelLayout, PillLook, PillStyle, SelectionStyle, ValueBoxState, ValueSpec, dial, pill_ex, segmented_ex, value_box,
};
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Color32, CornerRadius, Id, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_band::band_map::{BarCell, MARK_KEPT, band_map};
use soundcraft_band::lanes::{SILENT_SECTION_ID, lanes_to_timeline};
use soundcraft_band::song_feel::{FEEL_DIAL_COUNT, dial_name, dial_positions, dial_title, factory_feel_presets};
use soundcraft_model::{MusicianSlot, Route, Session};
use soundcraft_time::Samples;

/// The Studio's tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StudioTab {
    #[default]
    Song,
    Sections,
    Feel,
    Band,
}

impl StudioTab {
    pub const ALL: [StudioTab; 4] = [StudioTab::Song, StudioTab::Sections, StudioTab::Feel, StudioTab::Band];

    pub fn name(self) -> &'static str {
        match self {
            StudioTab::Song => "Song",
            StudioTab::Sections => "Sections",
            StudioTab::Feel => "Feel",
            StudioTab::Band => "Band",
        }
    }
}

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

/// The Print… panel.
#[derive(Clone, Debug, Default)]
pub struct PrintUi {
    pub open: bool,
    pub scope: PrintScope,
    /// The section block printed with [`PrintScope::Section`].
    pub block: Option<u32>,
    /// The musicians to print; `None`: every one a track plays.
    pub slots: Option<[bool; 6]>,
}

/// The Library's lists as `band.list` gives them (read when the window opens and after an edit
/// that changes them, never per frame: it reads the user's files).
#[derive(Clone, Debug, Default)]
struct Lists {
    /// Band presets: (name, factory).
    presets: Vec<(String, bool)>,
    /// Feel presets: (name, factory).
    feel: Vec<(String, bool)>,
    songs: Vec<String>,
    /// A user Band folder exists (saving is possible).
    user_folder: bool,
}

/// The Studio window's state (not saved).
pub struct StudioUi {
    /// Window ▸ Studio.
    pub show: bool,
    pub tab: StudioTab,
    /// The section definition the Sections tab edits.
    pub def_editing: Option<char>,
    /// "Lay onto lanes at bar N" (and chord import): N, from 1.
    pub lay_bar: u32,
    /// A laid song replaces what the lanes hold (else it is laid over it).
    pub replace: bool,
    /// The factory song picked.
    pub song: Option<String>,
    pub print: PrintUi,
    /// The band preset picked.
    pub preset: Option<String>,
    /// Names being typed: Save as… (band preset), the feel preset, a definition's name.
    pub preset_name: String,
    pub feel_name: String,
    def_name: Option<(char, String)>,
    /// The last thing a command said (a refusal, what Print did).
    pub message: Option<String>,
    lists: Option<Lists>,
    boxes: ValueBoxState,
}

impl Default for StudioUi {
    fn default() -> Self {
        StudioUi {
            show: false,
            tab: StudioTab::Song,
            def_editing: None,
            lay_bar: 1,
            replace: true,
            song: None,
            print: PrintUi::default(),
            preset: None,
            preset_name: String::new(),
            feel_name: String::new(),
            def_name: None,
            message: None,
            lists: None,
            boxes: ValueBoxState::default(),
        }
    }
}

const KEYS: [&str; 12] = ["C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B"];
/// The Band view: the musician names' column, the section strip, a lane.
const LABEL_W: f32 = 120.0;
const STRIP_H: f32 = 14.0;
const LANE_H: f32 = 16.0;
/// A kept take's outline (MS `col::gold`).
const GOLD: Color32 = Color32::from_rgb(0xD9, 0xA4, 0x41);

/// `window.studio {value?, tab?: song|sections|feel|band, def?: A-H}`: shows, hides or toggles
/// the window, on a tab.
pub fn command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let show = p.get("value").and_then(Value::as_bool).unwrap_or(!app.studio.show);
    if let Some(t) = p.get("tab").and_then(Value::as_str) {
        app.studio.tab = StudioTab::ALL
            .into_iter()
            .find(|x| x.name().eq_ignore_ascii_case(t.trim()))
            .ok_or_else(|| format!("window.studio: `tab` is song, sections, feel or band (got `{t}`)"))?;
    }
    if let Some(d) = p.get("def").and_then(Value::as_str) {
        let mut c = d.trim().chars();
        match (c.next().map(|c| c.to_ascii_uppercase()), c.next()) {
            (Some(l @ 'A'..='H'), None) => {
                app.studio.def_editing = Some(l);
                app.studio.tab = StudioTab::Sections;
            }
            _ => return Err(format!("window.studio: `def` is a section A-H (got `{d}`)")),
        }
    }
    if show && !app.studio.show {
        app.studio.lists = None;
    }
    app.studio.show = show;
    Ok(json!({"value": show, "tab": app.studio.tab.name().to_lowercase()}))
}

/// The Band view's bars: [`band_map`] of the lanes' timeline, the enabled musicians at their own
/// Energy. `None` while the lanes are empty.
pub fn view_map(s: &Session) -> Option<Vec<BarCell>> {
    let tl = lanes_to_timeline(&soundcraft_mix::lane_song(s)?)?;
    let slots = &s.band.slots;
    let plays: [bool; 6] = std::array::from_fn(|k| slots.get(k).is_some_and(|m| m.enabled));
    let energies: [i32; 6] = std::array::from_fn(|k| slots.get(k).map_or(1, |m| i32::from(m.energy)));
    Some(band_map(&tl, &plays, &energies))
}

/// Bar `bar` of musician `lane` in the Band view whose cells fill `area`.
pub fn cell_rect(area: Rect, bars: usize, bar: usize, lane: usize) -> Rect {
    let cw = area.width() / bars.max(1) as f32;
    Rect::from_min_size(pos2(area.min.x + bar as f32 * cw + 0.5, area.min.y + lane as f32 * LANE_H + 1.0), vec2((cw - 1.0).max(1.0), LANE_H - 2.0))
}

/// A cell's colour: the band's accent, stronger with the Energy played there; a sat-out bar is
/// a hatched grey.
pub fn cell_fill(cell: &BarCell, lane: usize) -> Color32 {
    if cell.sat_out(lane) {
        return Tokens::current().border;
    }
    let e = cell.energy.get(lane).copied().unwrap_or(0).clamp(0, 4) as f32;
    BAND_ACCENT.gamma_multiply(0.12 + 0.17 * e)
}

/// Draws the window.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.studio.show {
        return;
    }
    // "Edit definition…" on the Sections ruler asks for a definition.
    if let Some(l) = ctx.data_mut(|m| m.remove_temp::<char>(Id::new(STUDIO_DEF))).filter(|l| l.is_ascii_uppercase()) {
        app.studio.tab = StudioTab::Sections;
        app.studio.def_editing = Some(l);
    }
    if app.studio.lists.is_none() {
        app.studio.lists = Some(read_lists(app));
    }
    let mut open = true;
    egui::Window::new("Studio").id(Id::new("band_studio")).open(&mut open).default_size(vec2(1000.0, 680.0)).min_size(vec2(780.0, 460.0)).show(
        ctx,
        |ui| {
            let pal = palette();
            header(app, ui, &pal);
            ui.add_space(6.0);
            tabs(app, ui);
            ui.add_space(4.0);
            ui.separator();
            egui::ScrollArea::vertical().id_salt("band_studio_body").auto_shrink([false, false]).show(ui, |ui| match app.studio.tab {
                StudioTab::Song => song_tab(app, ui, &pal),
                StudioTab::Sections => sections_tab(app, ui, &pal),
                StudioTab::Feel => feel_tab(app, ui, &pal),
                StudioTab::Band => band_tab(app, ui),
            });
        },
    );
    if !open {
        app.studio.show = false;
    }
}

// ---- helpers ------------------------------------------------------------------------------------

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

/// Runs a command; a refusal shows in the window and the status bar.
fn run(app: &mut SoundApp, id: &str, p: Value) -> Option<Value> {
    match app.run(id, p) {
        Ok(v) => Some(v),
        Err(e) => {
            app.studio.message = Some(e.clone());
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
        && let Err(e) = app.engine.execute_merged("band.settings", &p, &format!("studio:{key}"))
    {
        app.ui.status = e.to_string();
    }
    if ended {
        app.engine.end_merge();
    }
}

/// A titled card.
fn card<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::current();
    egui::Frame::NONE
        .fill(t.module_tile_bg)
        .stroke(Stroke::new(1.0, t.module_tile_border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).font(bold(10.5)).color(t.text_dim));
            ui.add_space(6.0);
            add(ui)
        })
        .inner
}

fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(regular(10.0)).color(Tokens::current().text_dim));
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
    let (r, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
    pill_ex(ui, r, salt, pal, text, on, PillStyle::Normal, PillLook { tracked: false, ..PillLook::default() })
}

/// A list row's text: white on the selection fill (the light theme's selection ink is too close
/// to its fill).
fn picked_text(t: RichText, on: bool) -> RichText {
    if on { t.color(Color32::WHITE) } else { t }
}

/// `N · Instrument · Player`.
fn musician_name(k: usize, m: &MusicianSlot) -> String {
    format!("{} · {} · {}", k + 1, instrument_name(m.instrument), slot_player(m))
}

/// The sample position of bar `n` (from 1).
fn bar_position(s: &Session, n: u32) -> Samples {
    let bar = 2 * soundcraft_mix::half_ticks(s);
    s.tempo.tick_to_samples(i64::from(n.max(1) - 1).saturating_mul(bar), s.sample_rate)
}

// ---- the header: Key and Scale ------------------------------------------------------------------

fn header(app: &mut SoundApp, ui: &mut Ui, pal: &VstPalette) {
    let band = app.engine.session().band.clone();
    ui.horizontal(|ui| {
        caption(ui, "Key");
        let (r, _) = ui.allocate_exact_size(vec2(64.0, 22.0), Sense::hover());
        let resp = field(ui, r, KEYS.get(usize::from(band.key)).copied().unwrap_or("C"), "Key");
        egui::Popup::menu(&resp).show(|ui| {
            for (i, k) in KEYS.iter().enumerate() {
                if ui.selectable_label(usize::from(band.key) == i, *k).clicked() {
                    run(app, "band.settings", json!({"key": i}));
                    ui.close();
                }
            }
        });
        ui.add_space(12.0);
        caption(ui, "Scale");
        if let Some(i) = seg(ui, 140.0, "studio-scale", pal, &["Major", "Minor"], Some(usize::from(band.scale.min(1)))) {
            run(app, "band.settings", json!({"scale": i}));
        }
        ui.add_space(12.0);
        let t = Tokens::current();
        let name = if band.band_name.is_empty() { "Factory Band" } else { band.band_name.as_str() };
        ui.label(RichText::new(format!("Band: {name}")).font(regular(10.5)).color(t.text_dim));
        if let Some(m) = app.studio.message.clone() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("×").on_hover_text("Dismiss").clicked() {
                    app.studio.message = None;
                }
                ui.label(RichText::new(m).font(regular(10.5)).color(t.text));
            });
        }
    });
}

fn tabs(app: &mut SoundApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        for tab in StudioTab::ALL {
            let on = app.studio.tab == tab;
            let resp = ui.add(egui::Button::selectable(on, picked_text(RichText::new(tab.name()).font(bold(11.5)), on)));
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, format!("Studio: {}", tab.name())));
            if resp.clicked() {
                app.studio.tab = tab;
            }
        }
    });
}

// ---- Song: the Band view, factory songs, chord import, Print ------------------------------------

fn song_tab(app: &mut SoundApp, ui: &mut Ui, pal: &VstPalette) {
    card(ui, "BAND VIEW", |ui| band_view(app, ui));
    ui.add_space(8.0);
    ui.columns(3, |cols| {
        if let [a, b, c] = cols {
            card(a, "FACTORY SONGS", |ui| factory_songs(app, ui, pal));
            card(b, "IMPORT MIDI CHORDS", |ui| import_chords(app, ui));
            card(c, "PRINT", |ui| print_panel(app, ui, pal));
        }
    });
}

fn band_view(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let s = app.engine.session_arc();
    let Some(cells) = view_map(&s) else {
        ui.label(
            RichText::new("The song lanes are empty: lay a factory song below, or draw sections and chords on the Sections and Chords rulers.")
                .font(regular(11.0))
                .color(t.text_dim),
        );
        return;
    };
    let bars = cells.len();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), STRIP_H + 6.0 * LANE_H + 2.0), Sense::hover());
    let area = Rect::from_min_max(pos2(r.min.x + LABEL_W, r.min.y + STRIP_H), pos2(r.max.x, r.min.y + STRIP_H + 6.0 * LANE_H));
    let resp = ui.interact(area, ui.id().with("band-view"), Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Band view"));
    let p = ui.painter_at(r);
    let cw = area.width() / bars.max(1) as f32;

    // The section strip: the letter where a section starts, the Energy nudge as a tint.
    for (b, c) in cells.iter().enumerate() {
        let x = area.min.x + b as f32 * cw;
        if c.energy_add != 0 {
            let tint = if c.energy_add > 0 { BAND_ACCENT } else { GOLD };
            p.rect_filled(Rect::from_min_size(pos2(x, r.min.y), vec2(cw, STRIP_H - 3.0)), 0.0, tint.gamma_multiply(0.15 * c.energy_add.abs() as f32));
        }
        if b == 0 || cells.get(b - 1).is_some_and(|prev| prev.section != c.section) {
            p.rect_filled(Rect::from_min_size(pos2(x, r.min.y), vec2(1.0, area.max.y - r.min.y)), 0.0, t.border);
            let (letter, colour) = match c.section {
                '\0' | SILENT_SECTION_ID => ('-', t.text_dim),
                id => (id, letter_color(id)),
            };
            p.text(pos2(x + 3.0, r.min.y + STRIP_H * 0.5 - 1.0), Align2::LEFT_CENTER, letter.to_string(), bold(9.5), colour);
        }
    }
    // The lanes.
    for (k, m) in s.band.slots.iter().enumerate() {
        let y = area.min.y + k as f32 * LANE_H;
        let name_r = Rect::from_min_size(pos2(r.min.x, y), vec2(LABEL_W - 8.0, LANE_H));
        let ink = if m.enabled { t.text } else { t.text_dim.gamma_multiply(0.7) };
        p.with_clip_rect(name_r).text(pos2(name_r.min.x, name_r.center().y), Align2::LEFT_CENTER, musician_name(k, m), regular(9.5), ink);
        for (b, c) in cells.iter().enumerate() {
            let cell = cell_rect(area, bars, b, k);
            p.rect_filled(cell, 0.0, cell_fill(c, k));
            if c.sat_out(k) {
                // Hatched: sat out.
                let hatch = p.with_clip_rect(cell);
                let mut hx = cell.min.x - cell.height();
                while hx < cell.max.x {
                    hatch.line_segment(
                        [pos2(hx, cell.max.y), pos2(hx + cell.height(), cell.min.y)],
                        Stroke::new(0.6, t.text_dim.gamma_multiply(0.35)),
                    );
                    hx += 4.0;
                }
            } else if c.marks.get(k).is_some_and(|mk| mk & MARK_KEPT != 0) {
                p.rect_stroke(cell, 0.0, Stroke::new(1.0, GOLD), StrokeKind::Inside);
            }
        }
    }
    // The playhead, while playing.
    if app.engine.transport.playing {
        let bar = (2 * soundcraft_mix::half_ticks(&s)) as f64;
        let at = s.tempo.samples_to_ticks_f(app.engine.transport.position.max(0), s.sample_rate) / bar;
        if (0.0..bars as f64).contains(&at) {
            let x = area.min.x + at as f32 * cw;
            p.rect_filled(Rect::from_min_size(pos2(x, r.min.y), vec2(1.5, area.max.y - r.min.y)), 0.0, t.playhead);
            ui.ctx().request_repaint();
        }
    }
    ui.add_space(4.0);
    ui.label(
        RichText::new("Brighter = more Energy · hatched = sits out · gold = a kept take · the strip's tint is the arrangement's Energy nudge")
            .font(regular(9.5))
            .color(t.text_dim),
    );
}

fn lay_bar_box(app: &mut SoundApp, ui: &mut Ui, pal: &VstPalette, salt: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(80.0, 36.0), Sense::hover());
    let spec = ValueSpec { label: "Bar", lo: 1.0, hi: 9999.0, log: false, fine: 0.1, decimals: 0, unit: "", default: 1.0, fmt: None };
    let out = value_box(ui, &mut app.studio.boxes, r, salt, pal, &spec, f64::from(app.studio.lay_bar), false);
    if let Some(v) = out.changed {
        app.studio.lay_bar = v.round().clamp(1.0, 9999.0) as u32;
    }
}

fn factory_songs(app: &mut SoundApp, ui: &mut Ui, pal: &VstPalette) {
    let songs = app.studio.lists.as_ref().map(|l| l.songs.clone()).unwrap_or_default();
    if app.studio.song.is_none() {
        app.studio.song = songs.first().cloned();
    }
    for name in &songs {
        let on = app.studio.song.as_deref() == Some(name.as_str());
        if ui.selectable_label(on, picked_text(RichText::new(name.as_str()), on)).clicked() {
            app.studio.song = Some(name.clone());
        }
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        lay_bar_box(app, ui, pal, "studio-lay-bar");
        ui.add_space(6.0);
        crate::widgets::checkbox(ui, &mut app.studio.replace, "Replace the lanes").on_hover_text("Off: the song is laid over what the lanes hold");
    });
    ui.add_space(4.0);
    let n = app.studio.lay_bar;
    let label = format!("Lay onto lanes at bar {n}");
    let tip = "Its sections, chords, key, scale, take and feel (not the players)";
    if ui.add_enabled(app.studio.song.is_some(), egui::Button::new(label)).on_hover_text(tip).clicked()
        && let Some(name) = app.studio.song.clone()
    {
        let at = bar_position(app.engine.session(), n);
        let replace = app.studio.replace;
        if run(app, "band.song.factory", json!({"name": name, "at": at, "replace": replace})).is_some() {
            app.studio.message = Some(format!("{name} laid from bar {n}"));
        }
    }
}

fn import_chords(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    ui.label(
        RichText::new(format!("Chords from a MIDI file, read in the band's key, onto the chord lane from bar {}.", app.studio.lay_bar))
            .font(regular(10.5))
            .color(t.text_dim),
    );
    ui.add_space(6.0);
    let can = app.services.pick_open.is_some();
    let resp = ui.add_enabled(can, egui::Button::new("Import MIDI chords…")).on_disabled_hover_text("No file dialog in this build");
    if resp.clicked() {
        let path = app.services.pick_open.as_ref().and_then(|open| open("band.chords.import", &["mid", "midi"]));
        if let Some(path) = path {
            let at = bar_position(app.engine.session(), app.studio.lay_bar);
            if let Some(v) = run(app, "band.chords.import", json!({"path": path, "at": at})) {
                let n = v.get("chords").and_then(Value::as_u64).unwrap_or(0);
                app.studio.message = Some(format!("{n} chord(s) imported"));
            }
        }
    }
}

fn print_panel(app: &mut SoundApp, ui: &mut Ui, pal: &VstPalette) {
    let t = Tokens::current();
    let s = app.engine.session_arc();
    let open = app.studio.print.open;
    let resp = ui
        .add(egui::Button::selectable(open, picked_text(RichText::new("Print…"), open)))
        .on_hover_text("The band into MIDI clips on the tracks that play it");
    if resp.clicked() {
        app.studio.print.open = !open;
    }
    if !app.studio.print.open {
        ui.label(
            RichText::new("Writes what the band plays as ordinary MIDI clips; those tracks then play the clips.")
                .font(regular(10.0))
                .color(t.text_dim),
        );
        return;
    }
    ui.add_space(6.0);
    let scopes = [PrintScope::Song, PrintScope::Section, PrintScope::Selection];
    let cur = scopes.iter().position(|x| *x == app.studio.print.scope);
    let w = ui.available_width().min(260.0);
    if let Some(i) = seg(ui, w, "studio-print-scope", pal, &["Song", "Section", "Selection"], cur) {
        app.studio.print.scope = scopes.get(i).copied().unwrap_or_default();
    }
    ui.add_space(4.0);
    let half = 2 * soundcraft_mix::half_ticks(&s);
    let bar_of = |tick: i64| tick.div_euclid(half.max(1)) + 1;
    let block_text = |id: u32| {
        s.band.sections.iter().find(|b| b.id == id).map(|b| {
            let name = s.band.def(b.section).map(|d| d.name.as_str()).unwrap_or("");
            format!("{} · {name} · bars {}–{}", b.section, bar_of(b.start_tick), bar_of(b.start_tick.saturating_add(b.length_ticks) - 1))
        })
    };
    let ready = match app.studio.print.scope {
        PrintScope::Song => true,
        PrintScope::Section => {
            if app.studio.print.block.and_then(block_text).is_none() {
                app.studio.print.block = s.band.sections.iter().min_by_key(|b| b.start_tick).map(|b| b.id);
            }
            let (r, _) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
            let shown = app.studio.print.block.and_then(block_text).unwrap_or_else(|| "No section blocks".into());
            let resp = field(ui, r, &shown, "Section block");
            let mut blocks: Vec<_> = s.band.sections.iter().collect();
            blocks.sort_by_key(|b| b.start_tick);
            egui::Popup::menu(&resp).show(|ui| {
                for b in blocks {
                    if ui.selectable_label(app.studio.print.block == Some(b.id), block_text(b.id).unwrap_or_default()).clicked() {
                        app.studio.print.block = Some(b.id);
                        ui.close();
                    }
                }
            });
            app.studio.print.block.is_some()
        }
        PrintScope::Selection => {
            let sel = s.edit.selection;
            let ok = sel.end > sel.start;
            let text = if ok {
                let tick = |x: Samples| s.tempo.samples_to_ticks(x, s.sample_rate);
                format!("Bars {}–{} of the edit selection", bar_of(tick(sel.start)), bar_of(tick(sel.end) - 1))
            } else {
                "Select a range in the Edit window first".into()
            };
            ui.label(RichText::new(text).font(regular(10.5)).color(t.text_dim));
            ok
        }
    };
    ui.add_space(4.0);
    caption(ui, "Musicians");
    let played: [bool; 6] = std::array::from_fn(|k| s.tracks.iter().any(|tr| tr.mixer.input == Route::Musician(k as u8)));
    let mut slots = app.studio.print.slots.unwrap_or(played);
    ui.horizontal_wrapped(|ui| {
        for (k, m) in s.band.slots.iter().enumerate() {
            let on = slots.get(k).copied().unwrap_or(false);
            let resp = pill(ui, 92.0, ("studio-print-slot", k), pal, &format!("{} · {}", k + 1, slot_player(m)), on);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, format!("Print musician {}", k + 1)));
            let resp = if played.get(k).copied().unwrap_or(false) { resp } else { resp.on_hover_text("No track plays this musician") };
            if resp.clicked()
                && let Some(x) = slots.get_mut(k)
            {
                *x = !*x;
                app.studio.print.slots = Some(slots);
            }
        }
    });
    ui.add_space(6.0);
    let chosen: Vec<usize> = (0..6).filter(|k| slots.get(*k).copied().unwrap_or(false)).map(|k| k + 1).collect();
    let go = ui.add_enabled(ready && !chosen.is_empty(), egui::Button::new("Print"));
    if go.clicked() {
        let p = match app.studio.print.scope {
            PrintScope::Song => json!({"song": true}),
            PrintScope::Section => json!({"block": app.studio.print.block}),
            PrintScope::Selection => json!({"range": {"start": s.edit.selection.start, "end": s.edit.selection.end}}),
        };
        if let Some(v) = run(app, "band.print", with(p, "slots", json!(chosen))) {
            let clips = v.get("clips").and_then(Value::as_array).map_or(0, Vec::len);
            let replaced = v.get("replaced").and_then(Value::as_array).map_or(0, Vec::len);
            let msg = if replaced > 0 {
                format!("Printed {clips} clip(s); {replaced} clip(s) under them replaced")
            } else {
                format!("Printed {clips} clip(s)")
            };
            app.studio.message = Some(msg.clone());
            app.ui.status = msg;
            app.studio.print = PrintUi::default();
        }
    }
}

// ---- Sections: the definitions A-H --------------------------------------------------------------

fn sections_tab(app: &mut SoundApp, ui: &mut Ui, pal: &VstPalette) {
    let s = app.engine.session_arc();
    let defs = &s.band.defs;
    if app.studio.def_editing.is_none_or(|l| s.band.def(l).is_none()) {
        app.studio.def_editing = defs.first().map(|d| d.id);
    }
    // The definitions as chips, and New.
    ui.horizontal_wrapped(|ui| {
        for d in defs {
            let on = app.studio.def_editing == Some(d.id);
            let uses = s.band.sections.iter().filter(|b| b.section == d.id).count();
            let text = RichText::new(format!("{} · {}", d.id, d.name)).font(bold(11.0)).color(if on { Color32::WHITE } else { letter_color(d.id) });
            let mut b = egui::Button::new(text).corner_radius(CornerRadius::same(10));
            if on {
                b = b.fill(letter_color(d.id));
            }
            if ui.add(b).on_hover_text(format!("{uses} block(s) on the Sections ruler")).clicked() {
                app.studio.def_editing = Some(d.id);
            }
        }
        let free = ('A'..='H').find(|l| s.band.def(*l).is_none());
        if ui.add_enabled(free.is_some(), egui::Button::new("+ New")).on_hover_text("A new section definition").clicked()
            && let Some(l) = free
            && run(app, "band.def", json!({"id": l.to_string()})).is_some()
        {
            app.studio.def_editing = Some(l);
        }
    });
    ui.add_space(8.0);
    let Some(d) = app.studio.def_editing.and_then(|l| s.band.def(l)).cloned() else { return };
    let id = d.id.to_string();
    let def = |app: &mut SoundApp, p: Value| {
        run(app, "band.def", with(p, "id", json!(id)));
    };
    let uses = s.band.sections.iter().filter(|b| b.section == d.id).count();
    card(ui, &format!("SECTION {} · USED BY {uses} BLOCK(S)", d.id), |ui| {
        ui.horizontal(|ui| {
            caption(ui, "Name");
            let draft = match &app.studio.def_name {
                Some((l, n)) if *l == d.id => n.clone(),
                _ => d.name.clone(),
            };
            let mut text = draft;
            let resp = ui.add(egui::TextEdit::singleline(&mut text).desired_width(180.0));
            app.studio.def_name = Some((d.id, text.clone()));
            if resp.lost_focus() {
                let name = text.trim().to_string();
                app.studio.def_name = None;
                if !name.is_empty() && name != d.name {
                    def(app, json!({"name": name}));
                }
            }
            ui.add_space(16.0);
            let used = uses > 0 || s.band.defs.len() <= 1;
            let tip = if uses > 0 { "Remove its blocks from the Sections ruler first" } else { "The band keeps at least one definition" };
            if ui.add_enabled(!used, egui::Button::new("Delete definition")).on_disabled_hover_text(tip).clicked() {
                run(app, "band.def.delete", json!({"id": d.id.to_string()}));
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                caption(ui, "Energy shift");
                let shifts = ["-3", "-2", "-1", "0", "+1", "+2", "+3"];
                let cur = usize::try_from(i32::from(d.energy_shift) + 3).ok();
                if let Some(i) = seg(ui, 250.0, "studio-def-shift", pal, &shifts, cur) {
                    def(app, json!({"energy_shift": i as i64 - 3}));
                }
            });
            ui.add_space(12.0);
            ui.vertical(|ui| {
                caption(ui, "Shape");
                if let Some(i) = seg(ui, 170.0, "studio-def-shape", pal, &["None", "Build", "Drop"], Some(usize::from(d.shape.min(2)))) {
                    def(app, json!({"shape": i}));
                }
            });
            ui.add_space(12.0);
            ui.vertical(|ui| {
                caption(ui, "Role");
                if let Some(i) = seg(ui, 180.0, "studio-def-role", pal, &["Normal", "Intro", "Ending"], Some(usize::from(d.role.min(2)))) {
                    def(app, json!({"role": i}));
                }
            });
            ui.add_space(12.0);
            let (r, _) = ui.allocate_exact_size(vec2(80.0, 36.0), Sense::hover());
            let spec =
                ValueSpec { label: "Take", lo: 0.0, hi: 16.0, log: false, fine: 0.1, decimals: 0, unit: "", default: 0.0, fmt: Some(take_text) };
            let out = value_box(ui, &mut app.studio.boxes, r, ("studio-def-take", d.id), pal, &spec, f64::from(d.take), false);
            if let Some(v) = out.changed {
                def(app, json!({"take": v.round() as i64}));
            }
        });
        ui.add_space(10.0);
        caption(ui, "Who plays, and at what Energy (Own: the musician's, moved by the shift)");
        ui.add_space(2.0);
        for (k, m) in s.band.slots.iter().enumerate() {
            let Some(sm) = d.musicians.get(k).copied() else { continue };
            ui.horizontal(|ui| {
                let t = Tokens::current();
                let (nr, _) = ui.allocate_exact_size(vec2(170.0, 22.0), Sense::hover());
                let ink = if m.enabled { t.text } else { t.text_dim };
                ui.painter().with_clip_rect(nr).text(pos2(nr.min.x, nr.center().y), Align2::LEFT_CENTER, musician_name(k, m), regular(10.5), ink);
                if pill(ui, 70.0, ("studio-def-plays", k), pal, "Plays", sm.plays).clicked() {
                    def(app, json!({"musicians": one_musician(k, json!({"plays": !sm.plays}))}));
                }
                ui.add_space(8.0);
                let cur = Some(usize::from(sm.energy.min(4)));
                if let Some(i) = seg(ui, 200.0, ("studio-def-energy", k), pal, &["Own", "1", "2", "3", "4"], cur) {
                    def(app, json!({"musicians": one_musician(k, json!({"energy": i}))}));
                }
            });
        }
        ui.add_space(10.0);
        caption(ui, "Feel here (Song's: the song's own dial; Time is always the song's)");
        ui.add_space(2.0);
        for dd in 1..FEEL_DIAL_COUNT {
            ui.horizontal(|ui| {
                let t = Tokens::current();
                let (nr, _) = ui.allocate_exact_size(vec2(70.0, 22.0), Sense::hover());
                ui.painter().text(pos2(nr.min.x, nr.center().y), Align2::LEFT_CENTER, dial_title(dd), regular(10.5), t.text);
                let mut options = vec!["Song's"];
                options.extend((0..dial_positions(dd)).map(|i| dial_name(dd, i)));
                let cur = d.feel.get(dd as usize).copied().unwrap_or(-1);
                let sel = usize::try_from(i32::from(cur) + 1).ok();
                if let Some(i) = seg(ui, 90.0 * options.len() as f32, ("studio-def-feel", dd), pal, &options, sel) {
                    let mut feel = d.feel.map(i64::from);
                    if let Some(time) = feel.first_mut() {
                        *time = -1;
                    }
                    if let Some(f) = feel.get_mut(dd as usize) {
                        *f = i as i64 - 1;
                    }
                    def(app, json!({"feel": feel}));
                }
            });
        }
    });
}

/// `band.def`'s `musicians` list that changes musician `k` only.
fn one_musician(k: usize, m: Value) -> Vec<Value> {
    let mut list = vec![json!({}); k];
    list.push(m);
    list
}

/// `p` (an object) with `key` set.
fn with(mut p: Value, key: &str, v: Value) -> Value {
    if let Some(o) = p.as_object_mut() {
        o.insert(key.to_string(), v);
    }
    p
}

fn take_text(v: f64) -> String {
    let n = v.round() as i64;
    if n <= 0 { "Song's".into() } else { n.to_string() }
}

// ---- Feel: the band feel and the song feel ------------------------------------------------------

fn feel_tab(app: &mut SoundApp, ui: &mut Ui, pal: &VstPalette) {
    let band = app.engine.session().band.clone();
    let f = band.feel.clone();
    card(ui, "FEEL", |ui| {
        ui.horizontal(|ui| {
            if let Some(i) = seg(ui, 200.0, "studio-natural", pal, &["Natural", "Classic"], Some(usize::from(!f.natural))) {
                run(app, "band.settings", json!({"feel": {"natural": i == 0}}));
            }
            ui.add_space(10.0);
            let tip = "The band leans into the chords' tension";
            if pill(ui, 90.0, "studio-tension", pal, "Tension", f.tension).on_hover_text(tip).clicked() {
                run(app, "band.settings", json!({"feel": {"tension": !f.tension}}));
            }
        });
        ui.add_space(8.0);
        caption(ui, "The band plays together");
        ui.horizontal(|ui| {
            let switches = [
                ("Bass lock", "bass_lock", f.bass_lock, "The bass locks to the kick"),
                ("Fills yield", "fills_yield", f.fills_yield, "One fill at a time: the others hold back"),
                ("Leave space", "leave_space", f.leave_space, "Musicians leave room for each other"),
                ("Dynamics together", "dynamics", f.dynamics, "The band rises and falls together"),
            ];
            for (label, key, on, tip) in switches {
                if pill(ui, 130.0, ("studio-feel", key), pal, label, on).on_hover_text(tip).clicked() {
                    run(app, "band.settings", json!({"feel": {key: !on}}));
                }
                ui.add_space(4.0);
            }
        });
    });
    ui.add_space(8.0);
    card(ui, "SONG FEEL", |ui| song_feel(app, ui, pal));
}

fn song_feel(app: &mut SoundApp, ui: &mut Ui, pal: &VstPalette) {
    let band = app.engine.session().band.clone();
    let sf = band.song_feel.clone();
    let dials = [sf.time, sf.density, sf.pocket, sf.groove, sf.touch];
    ui.horizontal(|ui| {
        if pill(ui, 60.0, "studio-sf-on", pal, "On", sf.on).on_hover_text("The song's feel shapes how the band plays").clicked() {
            run(app, "band.settings", json!({"song_feel": {"on": !sf.on}}));
        }
        ui.add_space(12.0);
        // The preset menu: the factory presets, then the user's.
        let lists = app.studio.lists.clone().unwrap_or_default();
        // Off, the dials don't play: no preset is in use.
        let current = factory_feel_presets()
            .iter()
            .find(|p| [p.time, p.density, p.pocket, p.groove, p.touch] == dials)
            .filter(|_| sf.on)
            .map_or_else(|| if sf.on { "Custom" } else { "None (off)" }.to_string(), |p| p.name.to_string());
        caption(ui, "Preset");
        let (r, _) = ui.allocate_exact_size(vec2(220.0, 22.0), Sense::hover());
        let resp = field(ui, r, &current, "Feel preset");
        egui::Popup::menu(&resp).show(|ui| {
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for (group, factory) in [("Factory", true), ("Yours", false)] {
                    let names: Vec<&str> = lists.feel.iter().filter(|(_, f)| *f == factory).map(|(n, _)| n.as_str()).collect();
                    if names.is_empty() {
                        continue;
                    }
                    ui.label(RichText::new(group).font(bold(10.0)).color(Tokens::current().text_dim));
                    for n in names {
                        if ui.selectable_label(n == current, n).clicked() {
                            run(app, "band.settings", json!({"feel_preset": n}));
                            app.studio.feel_name = if factory { String::new() } else { n.to_string() };
                            ui.close();
                        }
                    }
                }
            });
        });
        ui.add_space(8.0);
        ui.add(egui::TextEdit::singleline(&mut app.studio.feel_name).hint_text("Preset name").desired_width(150.0));
        let name = app.studio.feel_name.trim().to_string();
        let can_save = lists.user_folder && !name.is_empty();
        if ui.add_enabled(can_save, egui::Button::new("Save")).on_disabled_hover_text("Name it (needs the user Band folder)").clicked()
            && run(app, "band.feel_preset.save", json!({"name": name})).is_some()
        {
            app.studio.lists = None;
        }
        let user = lists.feel.iter().any(|(n, f)| !*f && n.eq_ignore_ascii_case(&name));
        if ui.add_enabled(user, egui::Button::new("Delete")).on_disabled_hover_text("Type the name of one of your presets").clicked()
            && run(app, "band.feel_preset.delete", json!({"name": name})).is_some()
        {
            app.studio.lists = None;
            app.studio.feel_name.clear();
        }
    });
    ui.add_space(8.0);
    // The five dials, at their positions (MS `dialName`).
    let (r, _) = ui.allocate_exact_size(vec2((5.0 * 96.0f32).min(ui.available_width()), 78.0), Sense::hover());
    let cw = r.width() / 5.0;
    for d in 0..FEEL_DIAL_COUNT {
        let n = dial_positions(d).max(2);
        let cur = dials.get(d as usize).copied().map_or(0, i32::from).clamp(0, n - 1);
        let cell = Rect::from_min_size(pos2(r.min.x + cw * d as f32, r.min.y), vec2(cw, 76.0)).shrink2(vec2(10.0, 0.0));
        let text = dial_name(d, cur);
        let o = DialOpts {
            label: dial_title(d),
            value_text: text,
            default: 0.0,
            layout: LabelLayout::Below,
            small: true,
            inactive: !sf.on,
            ..DialOpts::default()
        };
        let out = dial(ui, cell, ("studio-sf-dial", d), pal, cur as f32 / (n - 1) as f32, &o);
        let pos = out.value.map(|v| (v * (n - 1) as f32).round().clamp(0.0, (n - 1) as f32) as i32).filter(|p| *p != cur);
        let p = pos.map(|p| json!({"song_feel": {dial_title(d): p}}));
        settings_gesture(app, &format!("dial{d}"), p, out.started, out.ended);
    }
    ui.add_space(8.0);
    caption(ui, "Who ignores the feel (plays as set)");
    ui.horizontal_wrapped(|ui| {
        for (k, m) in band.slots.iter().enumerate() {
            let mut on = band.feel_ignore & (1 << k) != 0;
            if crate::widgets::checkbox(ui, &mut on, musician_name(k, m)).changed() {
                let mask = if on { band.feel_ignore | (1 << k) } else { band.feel_ignore & !(1 << k) };
                let list: Vec<usize> = (0..6).filter(|j| mask & (1 << j) != 0).map(|j| j + 1).collect();
                run(app, "band.settings", json!({"feel_ignore": list}));
            }
            ui.add_space(6.0);
        }
    });
}

// ---- Band: the band presets ---------------------------------------------------------------------

fn band_tab(app: &mut SoundApp, ui: &mut Ui) {
    let lists = app.studio.lists.clone().unwrap_or_default();
    let band_name = app.engine.session().band.band_name.clone();
    card(ui, "BAND PRESETS", |ui| {
        let t = Tokens::current();
        ui.label(RichText::new("The six musicians, their instruments, players, targets and settings").font(regular(10.5)).color(t.text_dim));
        ui.add_space(6.0);
        egui::ScrollArea::vertical().id_salt("studio-presets").max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
            for (name, factory) in &lists.presets {
                let picked = app.studio.preset.as_deref() == Some(name.as_str());
                let text = if *name == band_name { format!("{name}  ·  loaded") } else { name.clone() };
                let resp = ui.selectable_label(picked, picked_text(RichText::new(text), picked));
                let resp = if *factory { resp.on_hover_text("Factory") } else { resp };
                if resp.clicked() {
                    app.studio.preset = Some(name.clone());
                }
                if resp.double_clicked() {
                    run(app, "band.preset.load", json!({"name": name}));
                }
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let picked = app.studio.preset.clone();
            if ui.add_enabled(picked.is_some(), egui::Button::new("Load")).clicked()
                && let Some(name) = picked
            {
                run(app, "band.preset.load", json!({"name": name}));
            }
            ui.add_space(16.0);
            caption(ui, "Save as…");
            ui.add(egui::TextEdit::singleline(&mut app.studio.preset_name).hint_text("Band name").desired_width(170.0));
            let name = app.studio.preset_name.trim().to_string();
            let exists = lists.presets.iter().any(|(n, f)| !*f && n.eq_ignore_ascii_case(&name));
            let label = if exists { "Replace" } else { "Save" };
            if ui
                .add_enabled(lists.user_folder && !name.is_empty(), egui::Button::new(label))
                .on_disabled_hover_text("Name it (needs the user Band folder)")
                .clicked()
                && run(app, "band.preset.save", json!({"name": name, "replace": exists})).is_some()
            {
                app.studio.lists = None;
                app.studio.preset = Some(name);
                app.studio.preset_name.clear();
            }
            ui.add_space(16.0);
            if ui.button("Reload Library").on_hover_text("Read the players, targets and presets again").clicked()
                && let Some(v) = run(app, "band.library.reload", json!({}))
            {
                app.studio.lists = None;
                let errors = v.get("errors").and_then(Value::as_array).map_or(0, Vec::len);
                app.studio.message =
                    Some(if errors == 0 { "Library reloaded".into() } else { format!("Library reloaded: {errors} file(s) skipped") });
            }
        });
    });
}
