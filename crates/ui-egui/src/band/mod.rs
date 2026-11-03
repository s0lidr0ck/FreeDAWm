//! The Band in the UI: its timeline lanes ([`lanes`]); in the Mix window, the six musicians as
//! track inputs (the input menu's Musicians section), the Band key on a musician strip, and the
//! band drawer beside it ([`drawer`]); Window ▸ Studio, the band's song-wide controls ([`studio`]).
//!
//! No MIDI channel is shown anywhere: a track plays a musician, by number and name.

pub mod chord_picker;
pub mod drawer;
pub mod lanes;
pub mod studio;

pub use drawer::{BAND_ACCENT, BAND_DRAWER_W, DrawerRow, drawer, driver_rows, loaded_instrument, target_rows, target_slot};

use crate::SoundApp;
use crate::theme::Tokens;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, Rect, Ui, UiBuilder};
use serde_json::json;
use soundcraft_band::library::{INSTRUMENT_NAMES, LibraryData, PersonaData};
use soundcraft_model::{MusicianSlot, Route, Track, TrackId, TrackKind};

/// The musician (0-5) a track plays, when its input is one.
pub fn musician_of(track: &Track) -> Option<u8> {
    match track.mixer.input {
        Route::Musician(n) if n < 6 => Some(n),
        _ => None,
    }
}

/// Whether a track can play a musician: Instrument and MIDI tracks.
pub fn takes_musician(kind: TrackKind) -> bool {
    matches!(kind, TrackKind::Instrument | TrackKind::Midi)
}

/// An instrument's name (0-12).
pub fn instrument_name(i: u8) -> &'static str {
    INSTRUMENT_NAMES.get(usize::from(i)).copied().unwrap_or("Piano")
}

/// A musician's player, "Session" for the slot's own.
pub fn slot_player(m: &MusicianSlot) -> &str {
    if m.player.is_empty() { "Session" } else { &m.player }
}

/// A musician as the input menu lists it: `N · player (instrument)`.
pub fn slot_label(k: usize, m: &MusicianSlot) -> String {
    format!("{} · {} ({})", k + 1, slot_player(m), instrument_name(m.instrument))
}

/// The Library's persona of a player name, if it has one ("Session" and unknown names have none).
pub fn persona_named<'a>(lib: &'a LibraryData, name: &str) -> Option<&'a PersonaData> {
    let seed = lib.seed_for_name(name);
    if seed <= 0 { None } else { lib.persona(seed) }
}

/// A player's row in a list that picks one: `first` (the name, or the musician's line), and under it the
/// persona's `desc` ("Pop · ballad songwriter") in dim small text; the name alone when it has none.
pub fn player_row(ui: &Ui, first: &str, desc: &str, color: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let body = egui::TextStyle::Button.resolve(ui.style());
    job.append(first, 0.0, TextFormat { font_id: body.clone(), color, ..Default::default() });
    if !desc.trim().is_empty() {
        let small = egui::FontId::new((body.size * 0.85).max(8.0), body.family.clone());
        job.append(&format!("\n{}", desc.trim()), 0.0, TextFormat { font_id: small, color: Tokens::current().text_dim, ..Default::default() });
    }
    job
}

/// A persona's tags as a tooltip line ("pop · ballad"), empty when it has none.
pub fn tags_tip(p: &PersonaData) -> String {
    p.tags.join(" · ")
}

/// The input menu's Musicians section (Instrument and MIDI tracks): the six musicians, a
/// disabled one greyed. Picking one makes it the track's input.
pub(crate) fn input_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId) {
    let session = app.engine.session_arc();
    let Some(track) = session.track(id) else { return };
    if !takes_musician(track.kind) {
        return;
    }
    ui.separator();
    ui.strong("Musicians");
    let dim = Tokens::current().text_dim;
    let lib = app.engine.band_library();
    for (k, m) in session.band.slots.iter().enumerate() {
        let picked = track.mixer.input == Route::Musician(k as u8);
        let label = slot_label(k, m);
        let persona = persona_named(&lib, slot_player(m));
        let color = if m.enabled { ui.visuals().text_color() } else { dim };
        let text = player_row(ui, &label, persona.map_or("", |p| p.desc.as_str()), color);
        let resp = ui.add(egui::Button::selectable(picked, text));
        let tags = persona.map(tags_tip).filter(|t| !t.is_empty());
        let resp = if let Some(tags) = tags { resp.on_hover_text(tags) } else { resp };
        let resp = if m.enabled { resp } else { resp.on_hover_text("This musician is off: switch them on in the band drawer") };
        if resp.clicked() {
            if let Err(e) = app.run("track.input", json!({"track": id.0, "input": format!("musician:{}", k + 1)})) {
                app.ui.status = e;
            }
            ui.close();
        }
    }
}

/// Every module in the session that sends MIDI (`PluginInfo::midi_out`), except those on track
/// `id` itself (a track never plays its own), in session order: what the "MIDI from" submenu
/// lists, with each entry's name (`Kick › Shale`).
pub fn midi_sources(s: &soundcraft_model::Session, id: TrackId) -> Vec<(soundcraft_model::ModuleRef, String)> {
    s.tracks
        .iter()
        .filter(|t| t.id != id)
        .flat_map(|t| {
            t.mixer.inserts.iter().filter(|ins| soundcraft_dsp::plugin_info(&ins.plugin).is_some_and(|p| p.midi_out)).map(move |ins| {
                let m = soundcraft_model::ModuleRef { track: t.id, module: ins.id };
                (m, crate::mix_window::module_route_name(s, m))
            })
        })
        .collect()
}

/// The input menu's "MIDI from" submenu (Instrument and MIDI tracks): a checkbox per module in
/// the session that sends MIDI. Checking several merges their events into the track; unchecking
/// the last one leaves the track with no input. Absent when no module sends MIDI.
pub(crate) fn module_input_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId) {
    let session = app.engine.session_arc();
    let Some(track) = session.track(id) else { return };
    if !takes_musician(track.kind) {
        return;
    }
    let sources = midi_sources(&session, id);
    if sources.is_empty() {
        return;
    }
    let current = track.mixer.input.module_sources();
    ui.menu_button("MIDI from", |ui| {
        let dim = Tokens::current().text_dim;
        for (m, name) in &sources {
            let mut on = current.contains(m);
            let sends =
                session.track(m.track).and_then(|t| t.mixer.module(m.module)).is_some_and(|ins| ins.params.get("midiOut").is_none_or(|v| *v >= 0.5));
            let text = if sends { egui::RichText::new(name) } else { egui::RichText::new(name).color(dim) };
            let resp = ui.checkbox(&mut on, text);
            let resp = if sends { resp } else { resp.on_hover_text("Its MIDI Out is off: nothing plays until it is on") };
            if resp.changed() {
                // The set as it will be: this module added or taken out, the rest in their order.
                let next: Vec<String> =
                    current.iter().filter(|x| *x != m).chain(on.then_some(m)).map(|x| format!("module:{}/#{}", x.track.0, x.module.0)).collect();
                let input = if next.is_empty() { json!("none") } else { json!(next) };
                if let Err(e) = app.run("track.input", json!({"track": id.0, "input": input})) {
                    app.ui.status = e;
                }
            }
        }
        if !current.is_empty() {
            ui.separator();
            let sel = session.edit.selection;
            let resp = ui.add_enabled(!sel.is_empty(), egui::Button::new("Print to a clip")).on_hover_text(
                "Renders the selection and writes what these modules sent as a MIDI clip on this track (Record does the same for an armed track)",
            );
            let resp = if sel.is_empty() { resp.on_disabled_hover_text("Select a range first") } else { resp };
            if resp.clicked() {
                match app.run("track.print_input", json!({"track": id.0})) {
                    Ok(_) => app.ui.status = "Printed the MIDI input".into(),
                    Err(e) => app.ui.status = e,
                }
                ui.close();
            }
        }
    });
}

/// The strip's Band key: opens and folds the band drawer.
pub(crate) fn strip_key(app: &mut SoundApp, ui: &mut Ui, id: TrackId, r: Rect, narrow: bool) {
    let on = app.ui.band_drawers.contains(&id);
    let mut c = ui.new_child(UiBuilder::new().id_salt(("band-key", id.0)).max_rect(r));
    let text = if narrow { "B" } else { "Band" };
    let resp = crate::widgets::text_toggle(&mut c, r.size(), text, on, BAND_ACCENT, "Band drawer: this musician's controls");
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, "Band drawer"));
    if resp.clicked() && !app.ui.band_drawers.remove(&id) {
        app.ui.band_drawers.insert(id);
    }
}

/// Folds the band drawers whose track is gone or no longer plays a musician.
pub(crate) fn prune(app: &mut SoundApp) {
    let s = app.engine.session_arc();
    app.ui.band_drawers.retain(|t| s.track(*t).and_then(musician_of).is_some());
}
