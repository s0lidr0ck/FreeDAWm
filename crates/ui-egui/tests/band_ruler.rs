//! The band in the Edit window: the Band ruler (its lanes, and in its name cell the band preset,
//! Natural / Classic, Feel, Take, Musicians and New Band), the Sections ruler's right-click (the
//! section definitions, Energy in words, Lay Factory Song), a track header's musician badge and a
//! musician's settings panel, Track ▸ Print Band to MIDI... and File ▸ Import ▸ MIDI Chords...
//! (These took over every control Window ▸ Studio had.)

use egui::{Event, Modifiers, PointerButton, Pos2, Rect, Shape, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_band::band_map::band_map;
use soundcraft_band::lanes::lanes_to_timeline;
use soundcraft_model::{ClipContent, Route, Session, TrackId};
use soundcraft_ui_egui::band::ruler;
use soundcraft_ui_egui::edit_window::{rect, x_of};
use soundcraft_ui_egui::{Services, SoundApp, menus};

fn blank() -> SoundApp {
    SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default())
}

fn app_with(tracks: &[&str]) -> SoundApp {
    let mut engine = soundcraft_engine::Engine::default();
    for name in tracks {
        engine.execute("track.new", &json!({"name": name, "kind": "instrument"})).unwrap();
    }
    SoundApp::new(engine, None, Services::default())
}

fn harness(app: SoundApp) -> Harness<'static, SoundApp> {
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

fn show_rulers(h: &mut Harness<SoundApp>, rulers: &[&str]) {
    for r in rulers {
        h.state_mut().run("view.ruler", json!({"ruler": r, "visible": true})).unwrap();
    }
    h.run_steps(4);
}

fn texts(h: &Harness<SoundApp>) -> Vec<String> {
    h.output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            Shape::Text(t) => Some(t.galley.job.text.clone()),
            _ => None,
        })
        .collect()
}

fn has_text(h: &Harness<SoundApp>, needle: &str) -> bool {
    texts(h).iter().any(|t| t.contains(needle))
}

fn lay(app: &mut SoundApp, name: &str) {
    app.run("band.song.factory", json!({"name": name})).unwrap();
}

fn track(app: &SoundApp, name: &str) -> TrackId {
    app.engine.session().track_by_name(name).unwrap().id
}

fn bar_ticks(s: &Session) -> i64 {
    2 * soundcraft_mix::half_ticks(s)
}

fn click_at(h: &mut Harness<SoundApp>, pos: Pos2, button: PointerButton) {
    let ev = |pressed| Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE };
    h.event(Event::PointerMoved(pos));
    h.run_steps(1);
    h.event(ev(true));
    h.run_steps(1);
    h.event(ev(false));
    h.run_steps(3);
}

/// Clicks segment `i` of `n` in a segmented switch.
fn click_segment(h: &mut Harness<SoundApp>, label: &str, i: usize, n: usize) {
    let r = h.get_by_label(label).rect();
    let w = r.width() / n as f32;
    click_at(h, pos2(r.min.x + w * (i as f32 + 0.5), r.center().y), PointerButton::Primary);
}

/// The centre line of the ruler labelled `label` (its name drawn in the name cell).
fn ruler_y(h: &Harness<SoundApp>, label: &str) -> f32 {
    h.output()
        .shapes
        .iter()
        .find_map(|c| match &c.shape {
            Shape::Text(t) if t.galley.text() == label => Some(t.galley.rect.translate(t.pos.to_vec2()).center().y),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {label} ruler"))
}

/// Where `tick` sits on the timeline.
fn tick_x(h: &Harness<SoundApp>, tick: i64) -> f32 {
    let s = h.state().engine.session();
    x_of(s, rect(h.state().edit_layout.timeline), s.tempo.tick_to_samples(tick, s.sample_rate))
}

/// Right-clicks the Sections ruler at `tick`: its menu opens.
fn sections_menu(h: &mut Harness<SoundApp>, tick: i64) {
    let at = pos2(tick_x(h, tick), ruler_y(h, "Sections"));
    click_at(h, at, PointerButton::Secondary);
}

/// Opens a submenu of an open menu by hovering and clicking its button.
fn open_submenu(h: &mut Harness<SoundApp>, label: &str) {
    let label = format!("{label} ⏵");
    let r = h.get_by_label(&label).rect();
    h.event(Event::PointerMoved(r.center()));
    h.run_steps(4);
}

// ---- the ruler ------------------------------------------------------------------------------------

#[test]
fn the_band_ruler_is_in_view_rulers_and_holds_the_band_controls() {
    assert!(menus::tree().iter().any(|n| n.label == "View" && n.children.iter().any(|c| c.label == "Rulers")));
    assert_eq!(menus::command_for_path("View > Rulers > Band", &menus::ui_aliases()).as_deref(), Some("view.ruler"));
    assert!(menus::command_for_path("Window > Studio", &menus::ui_aliases()).is_none(), "the Studio window is gone");
    let mut h = harness(blank());
    assert!(h.query_by_label("Band view").is_none());
    show_rulers(&mut h, &["band"]);
    assert!(h.query_by_label("Band view").is_some(), "the Band ruler is up");
    for label in ["Band preset", "Natural|Classic", "Band feel", "Band take", "Musicians", "New Band"] {
        assert!(h.query_by_label(label).is_some(), "{label} in the name cell");
    }
    assert!(has_text(&h, "♪ Factory Band"));
    assert!(has_text(&h, "The band plays nothing yet"), "empty lanes say what to do");
    h.state_mut().run("view.ruler", json!({"ruler": "band", "visible": false})).unwrap();
    h.run_steps(4);
    assert!(h.query_by_label("Band view").is_none());
}

#[test]
fn band_view_matches_band_map() {
    let mut app = app_with(&["Keys"]);
    lay(&mut app, "Ballad with Intro and Ending");
    app.run("band.slot", json!({"slot": 2, "energy": 4})).unwrap();
    let keys = track(&app, "Keys");
    app.run("track.input", json!({"track": keys.0, "input": "musician:1"})).unwrap();
    // A few bars to a screen, so each bar's cells are wide.
    app.run("view.zoom_set", json!({"samples_per_px": 400.0})).unwrap();
    let mut h = harness(app);
    show_rulers(&mut h, &["band"]);
    let s = h.state().engine.session_arc();
    let tl = lanes_to_timeline(&soundcraft_mix::lane_song(&s).unwrap()).unwrap();
    let plays = std::array::from_fn(|k| s.band.slots[k].enabled);
    let energies = std::array::from_fn(|k| i32::from(s.band.slots[k].energy));
    let want = band_map(&tl, &plays, &energies);
    assert_eq!(ruler::view_map(&s).as_deref(), Some(want.as_slice()));
    let row = h.get_by_label("Band view").rect();
    let rects: Vec<(Rect, egui::Color32)> = h
        .output()
        .shapes
        .iter()
        .filter_map(|c| match &c.shape {
            Shape::Rect(r) => Some((r.rect, r.fill)),
            _ => None,
        })
        .collect();
    let bar = bar_ticks(&s);
    let mut seen = 0;
    for (b, cell) in want.iter().enumerate() {
        let (x0, x1) = (tick_x(&h, b as i64 * bar), tick_x(&h, (b as i64 + 1) * bar));
        if x1 > row.max.x {
            break;
        }
        for k in 0..6 {
            let r = ruler::cell_rect(row, x0, x1, k);
            // Musician 1 is heard (a track plays them); the others are dimmed.
            let fill = ruler::cell_fill(cell, k, k == 0);
            let found = rects.iter().any(|(x, f)| (x.min - r.min).length() < 0.6 && (x.max - r.max).length() < 0.6 && *f == fill);
            assert!(found, "bar {b}, musician {k}: {fill:?} at {r:?}");
            seen += 1;
        }
    }
    assert!(seen >= 12, "at least two bars on screen ({seen} cells)");
    assert!(ruler::heard(&s, 0) && !ruler::heard(&s, 1));
}

#[test]
fn natural_classic_feel_and_take_run_band_settings() {
    let mut h = harness(blank());
    show_rulers(&mut h, &["band"]);
    assert!(h.state().engine.session().band.feel.natural);
    click_segment(&mut h, "Natural|Classic", 1, 2);
    assert!(!h.state().engine.session().band.feel.natural, "Classic");
    assert_eq!(h.state().engine.undo_label(), Some("Band Settings"));
    h.state_mut().engine.undo();
    h.run_steps(3);
    assert!(h.state().engine.session().band.feel.natural);
    // The Feel popover: Tension and the band switches; the Mod wheel isn't there (no MIDI input yet).
    h.get_by_label("Band feel").click();
    h.run_steps(4);
    for label in ["Tension", "As a band", "Bass lock", "Fills yield", "Leave space", "Dynamics together"] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    assert!(h.query_by_label("Mod wheel").is_none());
    assert!(has_text(&h, "SONG FEEL") && has_text(&h, "Who ignores the feel"));
    h.get_by_label("Bass lock").click();
    h.run_steps(3);
    assert!(h.state().engine.session().band.feel.bass_lock);
    // "As a band" (on in a new session) switches off and on, one undo step each.
    assert!(h.state().engine.session().band.feel.band);
    h.get_by_label("As a band").click();
    h.run_steps(3);
    assert!(!h.state().engine.session().band.feel.band);
    h.state_mut().engine.undo();
    assert!(h.state().engine.session().band.feel.band);
    // Tension flips.
    let tension = h.state().engine.session().band.feel.tension;
    h.get_by_label("Tension").click();
    h.run_steps(3);
    assert_eq!(h.state().engine.session().band.feel.tension, !tension);
    // The band's take.
    h.state_mut().run("band.settings", json!({"take": 3})).unwrap();
    h.run_steps(3);
    assert!(has_text(&h, "Take 3"));
}

#[test]
fn the_preset_popover_loads_a_band() {
    let mut h = harness(blank());
    show_rulers(&mut h, &["band"]);
    h.get_by_label("Band preset").click();
    h.run_steps(4);
    assert!(has_text(&h, "BAND PRESETS"));
    assert!(h.query_by_label("Reload Library").is_some());
    let other = ["Funk Machine", "Delta Blues", "Acoustic Worship"].into_iter().find(|n| h.query_by_label(n).is_some()).expect("a factory band");
    h.get_by_label(other).click();
    h.run_steps(4);
    assert_eq!(h.state().engine.session().band.band_name, other);
    h.state_mut().engine.undo();
    assert_eq!(h.state().engine.session().band.band_name, "Factory Band");
}

// ---- New Band and the Musicians list ---------------------------------------------------------------

#[test]
fn track_new_band_is_in_the_track_menu() {
    assert_eq!(menus::command_for_path("Track > New Band", &menus::ui_aliases()).as_deref(), Some("track.new_band"));
    assert!(menus::tree().iter().any(|n| n.label == "Track" && n.children.iter().any(|c| c.path == "Track > New Band")));
}

#[test]
fn the_empty_musicians_list_says_what_to_do_and_new_band_does_it_in_one_undo_step() {
    let mut h = harness(blank());
    show_rulers(&mut h, &["band"]);
    h.get_by_label("Musicians").click();
    h.run_steps(4);
    assert!(has_text(&h, "No track plays the band yet."), "the empty state: {:?}", texts(&h));
    // The popover's New Band (the name cell's + Band chip is "New Band" too).
    let buttons: Vec<_> = h.query_all_by_label("New Band").collect();
    assert_eq!(buttons.len(), 2);
    buttons[1].click();
    h.run_steps(6);
    let s = h.state().engine.session_arc();
    assert_eq!(s.tracks.len(), 5, "a track per musician who is on");
    assert!(s.tracks.iter().enumerate().all(|(k, t)| t.mixer.input == Route::Musician(k as u8)));
    assert!(!s.band.chords.is_empty(), "a song is laid on the empty lanes");
    for r in ["band", "sections", "band_chords"] {
        assert!(s.edit.rulers.iter().any(|x| x == r), "{r} shows");
    }
    assert!(h.state().ui.status.starts_with("New Band: 5 track(s)") && h.state().ui.status.contains("Press Play"), "{}", h.state().ui.status);
    // The musicians list replaces the empty state (the popover stays open): each one's track and
    // instrument.
    if !has_text(&h, "MUSICIANS") {
        h.get_by_label("Musicians").click();
        h.run_steps(4);
    }
    assert!(!has_text(&h, "No track plays the band yet."));
    assert!(has_text(&h, "MUSICIANS"));
    assert!(has_text(&h, "Juke — Drum Machine"), "the drummer's instrument is shown: {:?}", texts(&h));
    assert!(has_text(&h, "Round Bass"), "with its preset");
    assert!(has_text(&h, "Off: not heard"), "the sixth musician is off");
    assert!(has_text(&h, "Instrument plug-in"), "the column says what the target is");
    // One undo step.
    assert_eq!(h.state().engine.undo_label(), Some("New Band"));
    h.state_mut().run("edit.undo", json!({})).unwrap();
    h.run_steps(4);
    assert!(h.state().engine.session().tracks.is_empty());
}

#[test]
fn the_list_says_who_is_not_heard_and_why() {
    let mut app = blank();
    app.run("track.new", json!({"kind": "instrument", "name": "Beats"})).unwrap();
    app.run("track.input", json!({"track": "Beats", "input": "musician:2"})).unwrap();
    let mut h = harness(app);
    show_rulers(&mut h, &["band"]);
    let s = h.state().engine.session_arc();
    let lib = h.state().engine.band_library();
    let (status, problem) = ruler::musician_status(&s, &lib, 1);
    assert!(problem && status.starts_with("Drums on a pitched instrument"), "{status}");
    let (status, problem) = ruler::musician_status(&s, &lib, 0);
    assert!(problem && status.starts_with("No track plays them"), "{status}");
    assert!(has_text(&h, "1/5"), "one of five heard, on the ruler: {:?}", texts(&h));
    h.get_by_label("Musicians").click();
    h.run_steps(4);
    assert!(has_text(&h, "4 musician(s) who are on have no track"), "{:?}", texts(&h));
    // Add their tracks: the four, the drummer's own track kept.
    h.get_by_label("Add their tracks").click();
    h.run_steps(4);
    assert_eq!(h.state().engine.session().tracks.len(), 5);
    assert!(has_text(&h, "5/5"));
}

#[test]
fn a_musicians_settings_open_from_the_list_the_badge_and_the_track_menu() {
    let mut h = harness(blank());
    h.state_mut().run("track.new_band", json!({"installed": []})).unwrap();
    h.run_steps(4);
    assert!(h.query_by_label("Fold the band drawer").is_none());
    // The Edit track header's badge: "♪ 2 · Benny".
    assert!(has_text(&h, &ruler::badge_text(1, "Benny")), "{:?}", texts(&h));
    h.get_by_label("Drums musician 2").click();
    h.run_steps(4);
    assert_eq!(h.state().band_ui.musician, Some(1));
    assert!(has_text(&h, "MUSICIAN 2"), "the drummer's drawer is open");
    h.get_by_label("Fold the band drawer").click();
    h.run_steps(4);
    assert_eq!(h.state().band_ui.musician, None);
    assert!(h.state().ui.band_drawers.is_empty(), "the Mix window's drawers are untouched");
    // From the Musicians list.
    h.get_by_label("Musicians").click();
    h.run_steps(4);
    h.get_by_label("Musician 3 settings").click();
    h.run_steps(4);
    assert_eq!(h.state().band_ui.musician, Some(2));
    // Musician Settings… in the track menu runs window.musician.
    h.state_mut().run("window.musician", json!({"value": false})).unwrap();
    h.state_mut().run("window.musician", json!({"value": true, "musician": 3})).unwrap();
    h.run_steps(4);
    assert_eq!(h.state().band_ui.musician, Some(2));
    assert!(has_text(&h, "MUSICIAN 3"));
    assert!(h.state_mut().run("window.musician", json!({"musician": 7})).is_err());
}

// ---- the Sections ruler's right-click --------------------------------------------------------------

/// The first block of `letter`'s middle tick.
fn block_mid(s: &Session, letter: char) -> i64 {
    let b = s.band.sections.iter().filter(|b| b.section == letter).min_by_key(|b| b.start_tick).unwrap();
    b.start_tick + b.length_ticks / 2
}

#[test]
fn lay_factory_song_from_the_sections_ruler() {
    let mut h = harness(blank());
    show_rulers(&mut h, &["sections"]);
    let bar = bar_ticks(h.state().engine.session());
    sections_menu(&mut h, 4 * bar + bar / 3);
    open_submenu(&mut h, "Lay Factory Song");
    assert!(has_text(&h, "From bar 5"), "{:?}", texts(&h));
    h.get_by_label("Jazz ii-V-I").click();
    h.run_steps(4);
    let s = h.state().engine.session_arc();
    assert!(!s.band.sections.is_empty() && !s.band.chords.is_empty(), "the lanes hold the song");
    let first = s.band.sections.iter().map(|b| b.start_tick).min().unwrap();
    assert_eq!(first, 4 * bar, "laid from bar 5");
    // One undo step takes it off again.
    h.state_mut().engine.undo();
    assert!(h.state().engine.session().band.sections.is_empty());
}

#[test]
fn energy_says_bigger_and_smaller_and_a_definition_edit_changes_every_copy() {
    let mut app = blank();
    lay(&mut app, "Pop Verse Chorus Bridge");
    let s = app.engine.session_arc();
    assert!(s.band.sections.iter().filter(|b| b.section == 'A').count() >= 2, "the song plays A more than once");
    assert_eq!(s.band.def('A').unwrap().energy_shift, 0);
    let mut h = harness(app);
    show_rulers(&mut h, &["sections"]);
    let mid = block_mid(&s, 'A');
    sections_menu(&mut h, mid);
    assert!(h.query_by_label("Energy (+ bigger, − smaller)").is_some(), "Energy in words");
    for label in ["Shape", "Role", "Who Plays ⏵", "Feel Here ⏵", "Rename...", "Lay Factory Song ⏵"] {
        assert!(h.query_by_label(label).is_some(), "{label} in the menu");
    }
    // The second "+2": the definition's row (every A block).
    let nodes: Vec<_> = h.query_all_by_label("+2").collect();
    assert_eq!(nodes.len(), 2, "this block's Energy and every block's");
    let r = nodes[1].rect();
    click_at(&mut h, r.center(), PointerButton::Primary);
    let s = h.state().engine.session_arc();
    assert_eq!(s.band.def('A').unwrap().energy_shift, 2);
    // Every block of A plays it: the Band view's bars of A all carry the shift.
    let cells = ruler::view_map(&s).unwrap();
    let def = s.band.def('A').unwrap();
    let mut seen = 0;
    for c in cells.iter().filter(|c| c.section == 'A') {
        for (k, m) in s.band.slots.iter().enumerate() {
            if !m.enabled || !def.musicians[k].plays || def.musicians[k].energy > 0 {
                continue;
            }
            let want = (i32::from(m.energy) + 2 + c.energy_add).clamp(1, 4);
            assert_eq!(c.energy[k], want, "musician {k} in a bar of A");
            seen += 1;
        }
    }
    assert!(seen > 8, "bars of A checked");
    h.state_mut().engine.undo();
    assert_eq!(h.state().engine.session().band.def('A').unwrap().energy_shift, 0);
    // A block's own Energy: this block only.
    let b = s.band.sections.iter().filter(|b| b.section == 'A').min_by_key(|b| b.start_tick).unwrap().id;
    h.run_steps(30);
    sections_menu(&mut h, mid);
    let nodes: Vec<_> = h.query_all_by_label("−1").collect();
    let r = nodes[0].rect();
    click_at(&mut h, r.center(), PointerButton::Primary);
    let s = h.state().engine.session_arc();
    assert_eq!(s.band.sections.iter().find(|x| x.id == b).unwrap().energy, -1);
    assert_eq!(s.band.sections.iter().filter(|x| x.energy != 0).count(), 1, "only that block");
}

#[test]
fn shape_role_who_plays_feel_and_rename_edit_the_definition() {
    let mut app = blank();
    lay(&mut app, "Pop Verse Chorus Bridge");
    let mid = block_mid(app.engine.session(), 'B');
    let mut h = harness(app);
    show_rulers(&mut h, &["sections"]);
    // Shape: Build.
    sections_menu(&mut h, mid);
    h.get_by_label("Build").click();
    h.run_steps(4);
    assert_eq!(h.state().engine.session().band.def('B').unwrap().shape, 1);
    assert_eq!(h.state().engine.undo_label(), Some("Section Definition"));
    // Role: Ending.
    h.run_steps(30);
    sections_menu(&mut h, mid);
    h.get_by_label("Ending").click();
    h.run_steps(4);
    assert_eq!(h.state().engine.session().band.def('B').unwrap().role, 2);
    // Who Plays: the drummer sits B out.
    h.run_steps(30);
    sections_menu(&mut h, mid);
    open_submenu(&mut h, "Who Plays");
    let drummer = h.state().engine.session().band.slots[1].clone();
    let label = ruler::musician_name(1, &drummer);
    h.get_by_label(&label).click();
    h.run_steps(4);
    assert!(!h.state().engine.session().band.def('B').unwrap().musicians[1].plays, "the drummer sits out");
    // Feel Here: Density's first position.
    h.run_steps(30);
    sections_menu(&mut h, mid);
    open_submenu(&mut h, "Feel Here");
    assert!(has_text(&h, "Time is always the song's"));
    let density = soundcraft_band::song_feel::dial_name(1, 0);
    h.get_by_label(density).click();
    h.run_steps(4);
    assert_eq!(h.state().engine.session().band.def('B').unwrap().feel[1], 0);
    // Rename…: a dialog; every B block takes the name.
    h.run_steps(30);
    sections_menu(&mut h, mid);
    h.get_by_label("Rename...").click();
    h.run_steps(4);
    assert_eq!(h.state().dialogs.open_name(), Some("rename_section"));
    if let Some(soundcraft_ui_egui::dialogs::Dialog::RenameSection { name, .. }) = h.state_mut().dialogs.open.as_mut() {
        *name = "Big Chorus".into();
    }
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(h.state().engine.session().band.def('B').unwrap().name, "Big Chorus");
    assert!(has_text(&h, "B · Big Chorus"));
}

// ---- Print and Import ------------------------------------------------------------------------------

#[test]
fn print_band_to_midi_is_a_track_menu_item_that_runs_band_print() {
    assert_eq!(menus::command_for_path("Track > Print Band to MIDI...", &menus::ui_aliases()).as_deref(), Some("window.band_print"));
    let mut app = app_with(&["Keys"]);
    let keys = track(&app, "Keys");
    app.run("track.input", json!({"track": keys.0, "input": "musician:1"})).unwrap();
    lay(&mut app, "12-bar Blues");
    let mut h = harness(app);
    h.state_mut().run("window.band_print", json!({})).unwrap();
    h.run_steps(4);
    assert_eq!(menus::item_state(h.state(), "Track > Print Band to MIDI...", Some("window.band_print")), (true, true), "ticked while open");
    assert!(h.query_by_label("Song|Section|Selection").is_some(), "the scope switch");
    for k in 1..=6 {
        assert!(h.query_by_label(&format!("Print musician {k}")).is_some());
    }
    h.get_by_label("Print").click();
    h.run_steps(4);
    let s = h.state().engine.session_arc();
    let t = s.track(keys).unwrap();
    assert_eq!(t.mixer.input, Route::None, "printing hands the track to its clips");
    let clip = t.clips().first().expect("a printed clip");
    let ClipContent::Midi { sequence } = &clip.content else { panic!("a MIDI clip") };
    assert!(!sequence.notes.is_empty());
    assert!(!h.state().band_ui.print.open, "the window closes");
    assert!(h.state().ui.status.starts_with("Printed"), "{}", h.state().ui.status);
}

#[test]
fn import_midi_chords_is_a_file_import_item() {
    assert_eq!(menus::command_for_path("File > Import > MIDI Chords...", &menus::ui_aliases()).as_deref(), Some("ui.band.import_chords"));
    // Without a file dialog it is greyed out and says why.
    let app = blank();
    assert_eq!(menus::availability(&app, "ui.band.import_chords"), Err("No file dialog in this build".to_string()));
}

#[test]
fn import_midi_chords_reads_the_picked_file_at_the_insertion() {
    let dir = std::env::temp_dir().join(format!("band-ruler-import-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("chords.mid");
    // C major then F major, a bar each, at 480 ticks a quarter.
    let mut track = Vec::new();
    let mut ev = |delta: u32, bytes: &[u8]| {
        let mut d = Vec::new();
        let mut v = delta;
        d.push((v & 0x7f) as u8);
        v >>= 7;
        while v > 0 {
            d.insert(0, ((v & 0x7f) as u8) | 0x80);
            v >>= 7;
        }
        track.extend(d);
        track.extend_from_slice(bytes);
    };
    for chord in [[60u8, 64, 67], [65, 69, 72]] {
        for n in &chord {
            ev(0, &[0x90, *n, 90]);
        }
        for (j, n) in chord.iter().enumerate() {
            ev(if j == 0 { 1920 } else { 0 }, &[0x80, *n, 0]);
        }
    }
    ev(0, &[0xff, 0x2f, 0]);
    let mut file = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk".to_vec();
    file.extend((track.len() as u32).to_be_bytes());
    file.extend(track);
    std::fs::write(&path, file).unwrap();
    let picked = path.to_string_lossy().to_string();
    let services = Services { pick_open: Some(Box::new(move |_, _| Some(picked.clone()))), ..Services::default() };
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, services);
    assert_eq!(menus::availability(&app, "ui.band.import_chords"), Ok(()));
    app.run("ui.band.import_chords", json!({})).unwrap();
    let s = app.engine.session_arc();
    assert_eq!(s.band.chords.len(), 2, "two chords on the Chords ruler");
    assert!(app.ui.status.contains("2 chord(s) imported"), "{}", app.ui.status);
    assert!(s.edit.rulers.iter().any(|r| r == "band_chords"));
    let _ = std::fs::remove_dir_all(&dir);
}
