//! The band in the Mix window: musicians in the input menu, and the band drawer beside a
//! musician strip.

use egui::{Shape, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_band::library::Driver;
use soundcraft_model::{Route, TrackId};
use soundcraft_ui_egui::band::{BAND_DRAWER_W, DrawerRow, driver_rows};
use soundcraft_ui_egui::{Services, SoundApp};

fn app_with(tracks: &[&str]) -> SoundApp {
    let mut engine = soundcraft_engine::Engine::default();
    for name in tracks {
        engine.execute("track.new", &json!({"name": name, "kind": "instrument"})).unwrap();
    }
    let mut app = SoundApp::new(engine, None, Services::default());
    app.run("window.mix", json!({})).unwrap();
    app
}

fn track(app: &SoundApp, name: &str) -> TrackId {
    app.engine.session().track_by_name(name).unwrap().id
}

fn mix(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn texts(h: &Harness<SoundApp>) -> Vec<(String, egui::Rect)> {
    h.output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            Shape::Text(t) => Some((t.galley.job.text.clone(), t.visual_bounding_rect())),
            _ => None,
        })
        .collect()
}

fn shows(h: &Harness<SoundApp>, text: &str) -> bool {
    texts(h).iter().any(|(t, _)| t.eq_ignore_ascii_case(text))
}

/// A track whose input is musician `n` (1-6), with its band drawer open.
fn with_drawer(app: &mut SoundApp, name: &str, n: u8) -> TrackId {
    let id = track(app, name);
    app.run("track.input", json!({"track": id.0, "input": format!("musician:{n}")})).unwrap();
    app.ui.band_drawers.insert(id);
    id
}

#[test]
fn input_menu_lists_six_musicians_and_sets_the_input() {
    let app = app_with(&["Keys"]);
    let mut h = mix(app);
    h.get_by_label("Input").click();
    h.run_steps(3);
    assert!(shows(&h, "Musicians"), "the input menu has a Musicians section");
    // The factory band: slot by slot, `N · player (instrument)` with the player's style under it; slot 6 is
    // disabled (greyed).
    let band = h.state().engine.session().band.clone();
    let lib = h.state().engine.band_library();
    let row = |k: usize, m: &soundcraft_model::MusicianSlot| {
        let inst = soundcraft_band::library::INSTRUMENT_NAMES[usize::from(m.instrument)];
        let first = format!("{} · {} ({inst})", k + 1, m.player);
        match soundcraft_ui_egui::band::persona_named(&lib, &m.player) {
            Some(p) if !p.desc.is_empty() => format!("{first}\n{}", p.desc),
            _ => first,
        }
    };
    let mut styled = 0;
    for (k, m) in band.slots.iter().enumerate() {
        let label = row(k, m);
        assert!(h.query_by_label(&label).is_some(), "{label:?} listed");
        styled += usize::from(label.contains('\n'));
    }
    assert!(styled >= 5, "the factory players' styles are shown ({styled} of 6)");
    assert!(!band.slots[5].enabled && band.slots[0].enabled, "the factory band's sixth slot is off");
    let label = row(2, &band.slots[2]);
    assert!(label.starts_with(&format!("3 · {} (Bass)", band.slots[2].player)));
    h.get_by_label(&label).click();
    h.run_steps(3);
    let keys = h.state().engine.session().track_by_name("Keys").unwrap().mixer.input.clone();
    assert_eq!(keys, Route::Musician(2));
    // Undoable, as every track.input.
    h.state_mut().engine.undo();
    // Back on what a new instrument track takes: every MIDI input.
    assert_eq!(h.state().engine.session().track_by_name("Keys").unwrap().mixer.input, Route::Midi(soundcraft_model::MidiIn::all()));
}

#[test]
fn player_picker_shows_each_players_style() {
    let mut app = app_with(&["Keys"]);
    with_drawer(&mut app, "Keys", 1);
    let mut h = mix(app);
    h.get_by_label("Player").click();
    h.run_steps(3);
    assert!(shows(&h, "Piano"), "the browser groups players by instrument, the musician's own first");
    // Clay: `Pop · ballad songwriter` under the name (once per instrument he plays); a player with no desc shows
    // the name alone.
    assert!(h.query_all_by_label("Clay\nPop · ballad songwriter").next().is_some(), "Clay's style under the name");
    let lib = h.state().engine.band_library();
    let mut seen = 0;
    for seed in &lib.players[0] {
        let Some(p) = lib.persona(*seed) else { continue };
        let label = if p.desc.is_empty() { p.name.clone() } else { format!("{}\n{}", p.name, p.desc) };
        assert!(h.query_all_by_label(&label).next().is_some(), "{label:?} listed");
        seen += 1;
    }
    assert!(seen >= 10, "{seen} piano players listed");
    // Picking one sets the player.
    h.get_all_by_label("Clay\nPop · ballad songwriter").next().unwrap().click();
    h.run_steps(3);
    assert_eq!(h.state().engine.session().band.slots[0].player, "Clay");
}

#[test]
fn drawer_opens_beside_the_strip() {
    let mut app = app_with(&["Bass", "Drums"]);
    let bass = track(&app, "Bass");
    app.run("track.input", json!({"track": bass.0, "input": "musician:3"})).unwrap();
    let mut h = mix(app);
    let drums_before = h.get_by_label("Drums strip").rect();
    assert!(h.query_by_label("Band drawer").is_some(), "a musician strip has the Band key");
    h.get_by_label("Band drawer").click();
    h.run_steps(4);
    assert!(h.state().ui.band_drawers.contains(&bass));
    let drums_after = h.get_by_label("Drums strip").rect();
    assert!((drums_after.min.x - drums_before.min.x - (BAND_DRAWER_W + 1.0)).abs() < 2.0, "the next strip shifts right by the drawer");
    let bass_strip = h.get_by_label("Bass strip").rect();
    let (_, title) = texts(&h).into_iter().find(|(t, _)| t == "BASS · MUSICIAN 3").expect("the header names track and musician");
    assert!(title.min.x > bass_strip.max.x && title.max.x < drums_after.min.x, "the drawer sits between its strip and the next");
    for row in ["Enabled", "Mute", "Instrument", "Player", "Energy", "Swing", "Humanize", "Dynamics", "Fills", "Groove", "Respect chords"] {
        assert!(shows(&h, row), "{row} row shown");
    }
    for row in ["Octave", "Length", "Velocity", "Speed", "Voice", "Band take", "Target"] {
        assert!(shows(&h, row), "{row} row shown");
    }
    assert!(shows(&h, "Auto → Generic"), "no instrument loaded: Auto plays Generic");

    // Fold closes it.
    h.get_by_label("Fold the band drawer").click();
    h.run_steps(3);
    assert!(h.state().ui.band_drawers.is_empty());
    // A strip whose input is not a musician has no Band key and no drawer.
    assert!(h.query_by_label("Band drawer").is_some());
    h.state_mut().run("track.input", json!({"track": bass.0, "input": "none"})).unwrap();
    h.state_mut().ui.band_drawers.insert(bass);
    h.run_steps(3);
    assert!(h.query_by_label("Band drawer").is_none());
    assert!(h.state().ui.band_drawers.is_empty(), "pruned when the input stops being a musician");
}

#[test]
fn rows_follow_the_target_driver() {
    assert!(driver_rows(Driver::DrumMap).is_empty());
    assert_eq!(driver_rows(Driver::KeyswitchBass), [DrawerRow::KsOctave]);
    assert!(driver_rows(Driver::Swam).contains(&DrawerRow::SwamInstrument));
    assert_eq!(driver_rows(Driver::AmpleGuitar), [DrawerRow::Ample, DrawerRow::AmpleChords]);
    assert!(driver_rows(Driver::ArticulationKeys).contains(&DrawerRow::ArtKeys));
    assert_eq!(driver_rows(Driver::IndiginusStrings), [DrawerRow::PluckChords]);

    let mut app = app_with(&["Keys"]);
    with_drawer(&mut app, "Keys", 1);
    let mut h = mix(app);
    let labels = ["Keyswitch octave", "SWAM instrument", "Ample plays", "Ample chords", "Plucked chords"];
    let cases: [(&str, &[&str]); 4] = [
        ("MODO DRUM", &[]),
        ("MODO BASS 2", &["Keyswitch octave"]),
        // SWAM Strings uses SWAM's own keyswitch layout, so its octave shows too (as in MIDI Studio).
        ("SWAM Strings", &["Keyswitch octave", "SWAM instrument"]),
        ("Ample Guitar M 4", &["Ample plays", "Ample chords"]),
    ];
    for (target, want) in cases {
        h.state_mut().run("band.slot", json!({"slot": 1, "target": target})).unwrap();
        h.run_steps(3);
        assert!(shows(&h, target), "{target} shown as the target");
        for l in labels {
            assert_eq!(shows(&h, l), want.contains(&l), "{target}: {l}");
        }
        // Voice is SWAM's: greyed (with its note) for every other target.
        assert_eq!(shows(&h, "SWAM targets only"), target != "SWAM Strings", "{target}: Voice greyed");
    }
}

#[test]
fn a_knob_drag_is_one_undo_step() {
    let mut app = app_with(&["Keys"]);
    with_drawer(&mut app, "Keys", 1);
    let mut h = mix(app);
    let before = h.state().engine.session().band.slots[0].swing;
    let undo_before = h.state().engine.undo_history().len();
    let knob = h.get_by_label("Swing").rect().center();
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    h.event(egui::Event::PointerMoved(knob));
    h.run_steps(1);
    h.event(button(knob, true));
    h.run_steps(1);
    for dy in 1..=6 {
        h.event(egui::Event::PointerMoved(knob - vec2(0.0, dy as f32 * 6.0)));
        h.run_steps(1);
    }
    h.event(button(knob - vec2(0.0, 36.0), false));
    h.run_steps(3);
    let after = h.state().engine.session().band.slots[0].swing;
    assert!(after > before, "the drag turned Swing up ({before} → {after})");
    assert_eq!(h.state().engine.undo_history().len(), undo_before + 1, "one undo step");
    h.state_mut().engine.undo();
    assert_eq!(h.state().engine.session().band.slots[0].swing, before);
}

#[test]
fn no_channel_appears_anywhere() {
    let mut app = app_with(&["Keys", "Bass"]);
    with_drawer(&mut app, "Keys", 4);
    let mut h = mix(app);
    // Every driver's rows, then the input menu.
    for target in ["auto", "MODO BASS 2", "SWAM Strings", "Ample Guitar M 4", "Miroslav Philharmonik 2", "Indiginus The Mandolin"] {
        h.state_mut().run("band.slot", json!({"slot": 4, "target": target})).unwrap();
        h.run_steps(3);
        // (The Mix view's own "Channel width" bar is not the drawer's.)
        for (t, _) in texts(&h).into_iter().filter(|(t, _)| t != "Channel width") {
            assert!(!t.to_lowercase().contains("channel") && !t.contains("MIDI ch"), "`{t}` in the drawer ({target})");
        }
    }
    let bass_input = h.get_by_label("Bass strip").rect();
    let input = h
        .query_all_by_label("Input")
        .min_by(|a, b| (a.rect().center().x - bass_input.center().x).abs().total_cmp(&(b.rect().center().x - bass_input.center().x).abs()))
        .unwrap()
        .rect();
    h.event(egui::Event::PointerMoved(input.center()));
    h.run_steps(1);
    h.get_all_by_label("Input").find(|n| n.rect() == input).unwrap().click();
    h.run_steps(3);
    assert!(shows(&h, "Musicians"));
    // The hardware MIDI input's own channel submenu (Omni or 1-16) is not the band's.
    for (t, _) in texts(&h).into_iter().filter(|(t, _)| !t.starts_with("Channel: ") && t != "Channel width") {
        assert!(!t.to_lowercase().contains("channel") && !t.contains("MIDI ch"), "`{t}` in the menu");
    }
}
