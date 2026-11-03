//! Window ▸ Studio: the band's song-wide controls (key, feel, song feel, band presets), the Band
//! view, the section definitions, factory songs, chord import and Print.

use egui::{Pos2, Rect, Shape, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_band::band_map::band_map;
use soundcraft_band::lanes::lanes_to_timeline;
use soundcraft_model::{ClipContent, Route, Session, TrackId};
use soundcraft_ui_egui::band::studio::{self, StudioTab};
use soundcraft_ui_egui::{Services, SoundApp, menus};

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

fn click_at(h: &mut Harness<SoundApp>, pos: Pos2) {
    let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    h.event(egui::Event::PointerMoved(pos));
    h.run_steps(1);
    h.event(button(true));
    h.run_steps(1);
    h.event(button(false));
    h.run_steps(3);
}

/// Clicks segment `i` of `n` in a segmented switch.
fn click_segment(h: &mut Harness<SoundApp>, label: &str, i: usize, n: usize) {
    let r = h.get_by_label(label).rect();
    let w = r.width() / n as f32;
    click_at(h, egui::pos2(r.min.x + w * (i as f32 + 0.5), r.center().y));
}

fn open(h: &mut Harness<SoundApp>, tab: StudioTab) {
    h.state_mut().run("window.studio", json!({"value": true})).unwrap();
    h.state_mut().studio.tab = tab;
    h.run_steps(4);
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

#[test]
fn window_studio_toggles() {
    let app = app_with(&[]);
    assert!(menus::tree().iter().any(|n| n.label == "Window" && n.children.iter().any(|c| c.path == "Window > Studio")));
    assert_eq!(menus::command_for_path("Window > Studio", &menus::ui_aliases()).as_deref(), Some("window.studio"));
    let mut h = harness(app);
    assert!(!h.state().studio.show);
    assert!(!texts(&h).iter().any(|t| t == "Studio"));
    h.state_mut().run("window.studio", json!({})).unwrap();
    h.run_steps(4);
    assert!(h.state().studio.show);
    assert!(texts(&h).iter().any(|t| t == "Studio"), "the window is up");
    assert_eq!(menus::item_state(h.state(), "Window > Studio", Some("window.studio")), (true, true), "the menu item is ticked");
    for tab in ["Song", "Sections", "Feel", "Band"] {
        assert!(h.query_by_label(&format!("Studio: {tab}")).is_some(), "{tab} tab");
    }
    h.state_mut().run("window.studio", json!({})).unwrap();
    h.run_steps(4);
    assert!(!h.state().studio.show);
    assert!(!texts(&h).iter().any(|t| t == "Studio"));
}

#[test]
fn edit_definition_opens_the_studio_at_that_definition() {
    let mut app = app_with(&[]);
    lay(&mut app, "Pop Verse Chorus Bridge");
    let mut h = harness(app);
    h.state_mut().run("window.studio", json!({"value": true})).unwrap();
    h.run_steps(1);
    h.ctx.data_mut(|m| m.insert_temp(egui::Id::new(soundcraft_ui_egui::band::lanes::STUDIO_DEF), 'B'));
    h.run_steps(4);
    assert_eq!(h.state().studio.tab, StudioTab::Sections);
    assert_eq!(h.state().studio.def_editing, Some('B'));
}

#[test]
fn lay_factory_song_from_the_panel() {
    let app = app_with(&[]);
    let mut h = harness(app);
    open(&mut h, StudioTab::Song);
    h.get_by_label("Jazz ii-V-I").click();
    h.run_steps(3);
    h.state_mut().studio.lay_bar = 5;
    h.run_steps(3);
    h.get_by_label("Lay onto lanes at bar 5").click();
    h.run_steps(4);
    let s = h.state().engine.session_arc();
    assert!(!s.band.sections.is_empty() && !s.band.chords.is_empty(), "the lanes hold the song");
    let first = s.band.sections.iter().map(|b| b.start_tick).min().unwrap();
    assert_eq!(first, 4 * bar_ticks(&s), "laid from bar 5");
    // One undo step takes it off again.
    h.state_mut().engine.undo();
    assert!(h.state().engine.session().band.sections.is_empty());
}

#[test]
fn natural_classic_switch_runs_band_settings() {
    let app = app_with(&[]);
    let mut h = harness(app);
    open(&mut h, StudioTab::Feel);
    assert!(h.state().engine.session().band.feel.natural);
    click_segment(&mut h, "Natural|Classic", 1, 2);
    assert!(!h.state().engine.session().band.feel.natural, "Classic");
    assert_eq!(h.state().engine.undo_label(), Some("Band Settings"));
    h.state_mut().engine.undo();
    assert!(h.state().engine.session().band.feel.natural);
    // The band feel switches and Tension are there; the Mod wheel isn't (no MIDI input yet).
    for label in ["Tension", "Bass lock", "Fills yield", "Leave space", "Dynamics together"] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    assert!(h.query_by_label("Mod wheel").is_none());
    h.get_by_label("Bass lock").click();
    h.run_steps(3);
    assert!(h.state().engine.session().band.feel.bass_lock);
}

#[test]
fn def_edit_changes_every_copy() {
    let mut app = app_with(&[]);
    lay(&mut app, "Pop Verse Chorus Bridge");
    let s = app.engine.session_arc();
    assert!(s.band.sections.iter().filter(|b| b.section == 'A').count() >= 2, "the song plays A more than once");
    let before = s.band.def('A').unwrap().energy_shift;
    assert_eq!(before, 0);
    let mut h = harness(app);
    open(&mut h, StudioTab::Sections);
    h.state_mut().studio.def_editing = Some('A');
    h.run_steps(3);
    // -3 … +3: index 5 is +2.
    click_segment(&mut h, "-3|-2|-1|0|+1|+2|+3", 5, 7);
    let s = h.state().engine.session_arc();
    assert_eq!(s.band.def('A').unwrap().energy_shift, 2);
    // Every block of A plays it: the Band view's bars of A all carry the shift.
    let cells = studio::view_map(&s).unwrap();
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
    // Other definitions keep theirs.
    assert_eq!(s.band.def('B').unwrap().energy_shift, h.state().engine.session().band.def('B').unwrap().energy_shift);
    h.state_mut().engine.undo();
    assert_eq!(h.state().engine.session().band.def('A').unwrap().energy_shift, 0);
}

#[test]
fn print_dialog_runs_band_print() {
    let mut app = app_with(&["Keys"]);
    let keys = track(&app, "Keys");
    app.run("track.input", json!({"track": keys.0, "input": "musician:1"})).unwrap();
    lay(&mut app, "12-bar Blues");
    let mut h = harness(app);
    open(&mut h, StudioTab::Song);
    h.get_by_label("Print…").click();
    h.run_steps(3);
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
    assert!(!h.state().studio.print.open, "the print panel closes");
}

#[test]
fn band_view_matches_band_map() {
    let mut app = app_with(&[]);
    lay(&mut app, "Ballad with Intro and Ending");
    app.run("band.slot", json!({"slot": 2, "energy": 4})).unwrap();
    let mut h = harness(app);
    open(&mut h, StudioTab::Song);
    let s = h.state().engine.session_arc();
    let tl = lanes_to_timeline(&soundcraft_mix::lane_song(&s).unwrap()).unwrap();
    let plays = std::array::from_fn(|k| s.band.slots[k].enabled);
    let energies = std::array::from_fn(|k| i32::from(s.band.slots[k].energy));
    let want = band_map(&tl, &plays, &energies);
    assert_eq!(studio::view_map(&s).as_deref(), Some(want.as_slice()));
    let area = h.get_by_label("Band view").rect();
    let rects: Vec<(Rect, egui::Color32)> = h
        .output()
        .shapes
        .iter()
        .filter_map(|c| match &c.shape {
            Shape::Rect(r) => Some((r.rect, r.fill)),
            _ => None,
        })
        .collect();
    for (b, cell) in want.iter().enumerate() {
        for k in 0..6 {
            let r = studio::cell_rect(area, want.len(), b, k);
            let fill = studio::cell_fill(cell, k);
            let found = rects.iter().any(|(x, f)| (x.min - r.min).length() < 0.6 && (x.max - r.max).length() < 0.6 && *f == fill);
            assert!(found, "bar {b}, musician {k}: {fill:?} at {r:?}");
        }
    }
    // Section letters along the top, where each section starts.
    let shown = texts(&h);
    for id in ['A', 'B'] {
        if want.iter().any(|c| c.section == id) {
            assert!(shown.iter().any(|t| t == &id.to_string()), "{id} on the strip");
        }
    }
}
