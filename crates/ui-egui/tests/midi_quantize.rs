//! The MIDI Editor's Quantize button snaps to the grid the editor shows.
use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

fn quantize_on(grid: &str) -> i64 {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("track.new", json!({"name": "Grid Check", "kind": "midi"})).unwrap();
    let clip = app.run("midi.new_clip", json!({"track": "Grid Check", "start": 0, "end": 96_000})).unwrap()["clip"].clone();
    app.run("midi.note_add", json!({"clip": clip, "pitch": 60, "start_ticks": 300, "length_ticks": 240})).unwrap();
    app.run("edit.select", json!({"clips": [clip]})).unwrap();
    app.run("view.grid", json!({"value": grid})).unwrap();
    app.ui.show_midi_editor = true;
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h.get_by_label("Quantize").click();
    h.run_steps(3);
    let notes = h.state_mut().run("midi.notes", json!({"clip": clip})).unwrap();
    assert_eq!(notes["notes"][0]["length"], 240, "{notes}");
    notes["notes"][0]["start"].as_i64().unwrap()
}

#[test]
fn quantize_follows_a_quarter_note_grid() {
    assert_eq!(quantize_on("1/4"), 0);
}

#[test]
fn quantize_follows_a_sixteenth_note_grid() {
    assert_eq!(quantize_on("1/16"), 240);
}

/// Bug 31: a note moved to the largest start (a script can do it) draws in the editor and on the
/// timeline without an overflow.
#[test]
fn a_note_at_the_largest_start_does_not_overflow_the_editor() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("track.new", json!({"name": "Far", "kind": "midi"})).unwrap();
    let clip = app.run("midi.new_clip", json!({"track": "Far", "start": 0, "end": 96_000})).unwrap()["clip"].clone();
    app.run("midi.note_add", json!({"clip": clip, "pitch": 60, "start_ticks": 0, "length_ticks": 480})).unwrap();
    app.run("midi.notes_move", json!({"clip": clip, "indices": [0], "ticks": i64::MAX})).unwrap();
    app.run("edit.select", json!({"clips": [clip]})).unwrap();
    app.ui.show_midi_editor = true;
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h.event(egui::Event::PointerMoved(egui::pos2(800.0, 700.0)));
    h.run_steps(2);
}

/// Bug 1: with notes selected, Quantize changes those notes only, and the selection stays on them.
#[test]
fn quantize_acts_on_the_selected_notes_only() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("track.new", json!({"name": "Sel", "kind": "midi"})).unwrap();
    let clip = app.run("midi.new_clip", json!({"track": "Sel", "start": 0, "end": 96_000})).unwrap()["clip"].clone();
    for start in [10, 1_000] {
        app.run("midi.note_add", json!({"clip": clip, "pitch": 60, "start_ticks": start, "length_ticks": 100})).unwrap();
    }
    app.run("edit.select", json!({"clips": [clip]})).unwrap();
    app.run("view.grid", json!({"value": "1/4"})).unwrap();
    app.ui.show_midi_editor = true;
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h.state_mut().midi.selected = vec![1];
    h.get_by_label("Quantize").click();
    h.run_steps(3);
    let notes = h.state_mut().run("midi.notes", json!({"clip": clip})).unwrap();
    assert_eq!(notes["notes"][0]["start"], 10, "{notes}");
    assert_eq!(notes["notes"][1]["start"], 960, "{notes}");
    assert_eq!(h.state().midi.selected, vec![1]);
}
