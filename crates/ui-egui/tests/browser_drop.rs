//! The plugin browser's drag and drop, double-click and card menu: real pointer drags from a card
//! onto the Edit and Mix windows' tracks, slots and the space below the tracks.
mod editor_support;

use editor_support::{app_with_tracks, double_click, drag, right_click};
use egui::accesskit::Role;
use egui::{Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_model::{TrackId, TrackKind};
use soundcraft_ui_egui::SoundApp;
use soundcraft_ui_egui::browser::BrowserTab;
use soundcraft_ui_egui::browser::catalog::build_catalog;
use soundcraft_ui_egui::browser::drop::DropTarget;

/// "Voice" (audio) and "Keys" (an instrument track playing the Subtractive Synth).
fn app() -> SoundApp {
    let mut app = app_with_tracks(&["Voice"]);
    app.engine.execute("track.new", &json!({"kind": "instrument", "name": "Keys"})).unwrap();
    app.engine.execute("view.edit_inserts_ae", &json!({"value": true})).unwrap();
    app.engine.execute("view.edit_instruments", &json!({"value": true})).unwrap();
    app
}

/// An Edit window harness with the browser shown on `tab` and the built-in catalog.
fn browser(mut app: SoundApp, tab: BrowserTab) -> Harness<'static, SoundApp> {
    app.browser_catalog = Some(build_catalog(soundcraft_dsp::plugins(), &[], &[], &[], Vec::new()));
    app.ui.browser.tab = tab;
    app.run("ui.set", json!({"theme": "dark"})).unwrap();
    app.run("window.plugin_browser", json!({})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1700.0, 1000.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h
}

fn track(h: &Harness<SoundApp>, name: &str) -> TrackId {
    h.state().engine.session().track_by_name(name).unwrap().id
}

/// The card of `name`, searched for so it is in view (the largest button of that name: a chip
/// may share it).
fn card(h: &mut Harness<SoundApp>, name: &str) -> Pos2 {
    h.state_mut().ui.browser.text = name.into();
    h.run_steps(3);
    let all: Vec<Rect> = h.get_all_by_role_and_label(Role::Button, name).map(|n| n.rect()).collect();
    all.into_iter().max_by(|a, b| a.area().total_cmp(&b.area())).unwrap().center()
}

/// Lets egui's double-click window pass, so the next two clicks are a new double-click.
fn pause(h: &mut Harness<SoundApp>) {
    h.run_steps(40);
}

/// The Edit window row of `name` (header and lane), as last drawn.
fn row(h: &Harness<SoundApp>, name: &str) -> Rect {
    let id = track(h, name).0;
    let r = h.state().edit_layout.rows.iter().find(|(t, _)| *t == id).unwrap().1;
    Rect::from_min_max(pos2(r[0], r[1]), pos2(r[2], r[3]))
}

/// A point on the row's track header, left of the columns.
fn header(h: &Harness<SoundApp>, name: &str) -> Pos2 {
    let r = row(h, name);
    pos2(r.min.x + 40.0, r.center().y)
}

fn undo_len(h: &Harness<SoundApp>) -> usize {
    h.state().engine.undo_history().len()
}

fn inserts(h: &Harness<SoundApp>, name: &str) -> Vec<String> {
    h.state().engine.session().track_by_name(name).unwrap().mixer.inserts.iter().map(|i| i.plugin.clone()).collect()
}

/// The `n`th button labelled `label` from the top.
fn nth(h: &Harness<SoundApp>, label: &str, n: usize) -> Pos2 {
    let mut all: Vec<Rect> = h.get_all_by_role_and_label(Role::Button, label).map(|n| n.rect()).collect();
    all.sort_by(|a, b| a.min.y.total_cmp(&b.min.y));
    all[n].center()
}

#[test]
fn drop_instrument_on_instrument_track_sets_it() {
    let mut h = browser(app(), BrowserTab::Instruments);
    let from = card(&mut h, "Drum Synth");
    let to = row(&h, "Keys").center();
    drag(&mut h, from, to, Modifiers::NONE);
    let keys = h.state().engine.session().track_by_name("Keys").unwrap().instrument.clone().unwrap();
    assert_eq!(keys.plugin, "drum_synth");
    // And on the Instrument column's slot.
    let from = card(&mut h, "Sampler");
    let slot = h.get_by_role_and_label(Role::Button, "Instrument").rect().center();
    drag(&mut h, from, slot, Modifiers::NONE);
    let keys = h.state().engine.session().track_by_name("Keys").unwrap().instrument.clone().unwrap();
    assert_eq!(keys.plugin, "sampler");
}

#[test]
fn drop_instrument_on_audio_track_is_refused() {
    let mut h = browser(app(), BrowserTab::Instruments);
    let session = h.state().engine.session().clone();
    let undo = undo_len(&h);
    let from = card(&mut h, "Drum Synth");
    let to = header(&h, "Voice");
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(h.state().ui.status, "Drum Synth is an instrument: drop it on an instrument track or below the tracks");
    assert_eq!(h.state().engine.session().tracks, session.tracks);
    assert_eq!(undo_len(&h), undo);
}

#[test]
fn drop_effect_on_empty_slot_inserts_and_on_full_slot_replaces() {
    let mut h = browser(app(), BrowserTab::Effects);
    let undo = undo_len(&h);
    let from = card(&mut h, "Time Shift");
    // Voice's "Add insert" row (Voice is the top track).
    let to = nth(&h, "Add insert", 0);
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(inserts(&h, "Voice"), ["time_shift"]);
    assert_eq!(undo_len(&h), undo + 1);
    // Dropped on the Time Shift row, Invert replaces it.
    let from = card(&mut h, "Invert");
    let room = h.state().browser_catalog.as_ref().unwrap().items.iter().find(|i| i.name == "Invert").unwrap().id.clone();
    let eq = soundcraft_dsp::plugin_info("time_shift").unwrap().short_name;
    let to = h.get_by_role_and_label(Role::Button, &format!("{eq} insert")).rect().center();
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(inserts(&h, "Voice"), [room]);
    assert_eq!(undo_len(&h), undo + 2);
    editor_support::undo(&mut h);
    assert_eq!(inserts(&h, "Voice"), ["time_shift"]);
}

#[test]
fn drop_effect_on_track_header_appends() {
    let mut h = browser(app(), BrowserTab::Effects);
    let from = card(&mut h, "Time Shift");
    let to = header(&h, "Voice");
    drag(&mut h, from, to, Modifiers::NONE);
    let from = card(&mut h, "DC Offset Removal");
    let to = header(&h, "Voice");
    drag(&mut h, from, to, Modifiers::NONE);
    let one = h.state().browser_catalog.as_ref().unwrap().items.iter().find(|i| i.name == "DC Offset Removal").unwrap().id.clone();
    assert_eq!(inserts(&h, "Voice"), ["time_shift".to_string(), one]);
    // In the Mix window, the end of Keys' chain (its "Add module" row; Keys is the second strip).
    h.state_mut().run("window.mix", json!({})).unwrap();
    h.run_steps(5);
    let from = card(&mut h, "Time Shift");
    let mut adds: Vec<Rect> = h.get_all_by_role_and_label(Role::Button, "Add module").map(|n| n.rect()).collect();
    adds.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
    drag(&mut h, from, adds[1].center(), Modifiers::NONE);
    assert_eq!(inserts(&h, "Keys"), ["time_shift"]);
}

#[test]
fn drop_below_tracks_creates_track() {
    let mut h = browser(app(), BrowserTab::Instruments);
    let below = |h: &Harness<SoundApp>| pos2(row(h, "Keys").center().x, row(h, "Keys").max.y + 120.0);
    let tracks = h.state().engine.session().tracks.len();
    let undo = undo_len(&h);
    let from = card(&mut h, "Drum Synth");
    let to = below(&h);
    drag(&mut h, from, to, Modifiers::NONE);
    let s = h.state().engine.session();
    assert_eq!(s.tracks.len(), tracks + 1);
    let new = s.tracks.last().unwrap();
    assert_eq!(new.kind, TrackKind::Instrument);
    assert_eq!(new.instrument.as_ref().unwrap().plugin, "drum_synth");
    assert_eq!(s.edit.selected_tracks, [new.id], "the new track is selected");
    assert_eq!(undo_len(&h), undo + 1);
    editor_support::undo(&mut h);
    assert_eq!(h.state().engine.session().tracks.len(), tracks);
    // An effect: a new audio track with it, one undo step.
    h.get_by_role_and_label(Role::RadioButton, "Effects").click();
    h.run_steps(3);
    let undo = undo_len(&h);
    let from = card(&mut h, "Time Shift");
    let to = below(&h);
    drag(&mut h, from, to, Modifiers::NONE);
    let s = h.state().engine.session();
    assert_eq!(s.tracks.len(), tracks + 1);
    let new = s.tracks.last().unwrap();
    assert_eq!(new.kind, TrackKind::Audio);
    assert_eq!(new.mixer.inserts.iter().map(|i| i.plugin.as_str()).collect::<Vec<_>>(), ["time_shift"]);
    assert_eq!(s.edit.selected_tracks, [new.id], "the new track is selected");
    assert_eq!(undo_len(&h), undo + 1);
    editor_support::undo(&mut h);
    assert_eq!(h.state().engine.session().tracks.len(), tracks);
}

#[test]
fn double_click_uses_selected_track() {
    let mut h = browser(app(), BrowserTab::Effects);
    let at = card(&mut h, "Time Shift");
    // No selection: nothing happens but the hint.
    h.state_mut().run("edit.select_none", json!({})).unwrap();
    h.run_steps(2);
    double_click(&mut h, at);
    assert_eq!(h.state().ui.status, "Select a track first");
    assert!(inserts(&h, "Voice").is_empty() && inserts(&h, "Keys").is_empty());
    let keys = track(&h, "Keys");
    h.state_mut().run("edit.select", json!({"tracks": [keys.0]})).unwrap();
    pause(&mut h);
    double_click(&mut h, at);
    assert_eq!(inserts(&h, "Keys"), ["time_shift"]);
    // An instrument goes to an instrument track's instrument slot.
    h.get_by_role_and_label(Role::RadioButton, "Instruments").click();
    h.run_steps(3);
    let at = card(&mut h, "Drum Synth");
    // Not onto an audio track: it says what would work, and nothing changes.
    let voice = track(&h, "Voice");
    h.state_mut().run("edit.select", json!({"tracks": [voice.0]})).unwrap();
    let undo = undo_len(&h);
    pause(&mut h);
    double_click(&mut h, at);
    assert_eq!(h.state().ui.status, "Select an instrument track, or drag Drum Synth below the tracks");
    assert_eq!(undo_len(&h), undo);
    h.state_mut().run("edit.select", json!({"tracks": [keys.0]})).unwrap();
    pause(&mut h);
    double_click(&mut h, at);
    assert_eq!(h.state().engine.session().track(keys).unwrap().instrument.as_ref().unwrap().plugin, "drum_synth");
    // The card menu's "Insert on selected track" does the same.
    let at = card(&mut h, "Sampler");
    right_click(&mut h, at);
    h.get_by_label("Insert on selected track").click();
    h.run_steps(3);
    assert_eq!(h.state().engine.session().track(keys).unwrap().instrument.as_ref().unwrap().plugin, "sampler");
}

#[test]
fn recent_updates_after_drop() {
    let mut h = browser(app(), BrowserTab::Effects);
    let from = card(&mut h, "Time Shift");
    let to = header(&h, "Voice");
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(h.state().ui.browser.recent.first().map(String::as_str), Some("time_shift"));
    // A new track made by a drop counts as well.
    let from = card(&mut h, "DC Offset Removal");
    let to = pos2(row(&h, "Keys").center().x, row(&h, "Keys").max.y + 120.0);
    drag(&mut h, from, to, Modifiers::NONE);
    let one = h.state().browser_catalog.as_ref().unwrap().items.iter().find(|i| i.name == "DC Offset Removal").unwrap().id.clone();
    assert_eq!(h.state().ui.browser.recent.first(), Some(&one));
}

/// Press on `from` and move to `to` without releasing.
fn drag_to(h: &mut Harness<SoundApp>, from: Pos2, to: Pos2) {
    let step = |h: &mut Harness<SoundApp>, pos: Pos2, pressed: Option<bool>| {
        h.input_mut().events.push(egui::Event::PointerMoved(pos));
        if let Some(pressed) = pressed {
            h.input_mut().events.push(egui::Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        }
        h.step();
    };
    step(h, from, None);
    step(h, from, Some(true));
    let d = to - from;
    let n = (d.length() / 8.0).ceil().max(1.0) as usize;
    for k in 1..=n {
        step(h, from + d * (k as f32 / n as f32), None);
    }
    h.run_steps(2);
}

#[test]
fn hover_outlines_target_while_dragging() {
    let mut h = browser(app(), BrowserTab::Effects);
    let from = card(&mut h, "Time Shift");
    let to = nth(&h, "Add insert", 1);
    drag_to(&mut h, from, to);
    let keys = track(&h, "Keys");
    assert_eq!(h.state().browser_debug.drop_hover, Some(DropTarget::InsertSlot(keys, 0)));
    if let Ok(dir) = std::env::var("BROWSER_SHOTS") {
        std::fs::create_dir_all(&dir).unwrap();
        h.render().unwrap().save(format!("{dir}/t4-drag-insert.png")).unwrap();
    }
    // Over a track's header: the whole track.
    let at = header(&h, "Voice");
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    h.run_steps(2);
    assert_eq!(h.state().browser_debug.drop_hover, Some(DropTarget::Track(track(&h, "Voice"))));
    let below = pos2(at.x, row(&h, "Keys").max.y + 120.0);
    h.input_mut().events.push(egui::Event::PointerMoved(below));
    h.run_steps(2);
    assert_eq!(h.state().browser_debug.drop_hover, Some(DropTarget::NewTrack));
    if let Ok(dir) = std::env::var("BROWSER_SHOTS") {
        h.render().unwrap().save(format!("{dir}/t4-drag-new-track.png")).unwrap();
    }
    // Escape-free release over nothing droppable (the browser itself) changes nothing.
    let tracks = h.state().engine.session().tracks.len();
    h.input_mut().events.push(egui::Event::PointerMoved(from));
    h.input_mut().events.push(egui::Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert_eq!(h.state().engine.session().tracks.len(), tracks);
    assert_eq!(h.state().browser_debug.drop_hover, None);
}
