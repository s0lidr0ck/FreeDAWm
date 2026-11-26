//! Layers in the window: Add a layer, the 3-2-1, the recording screen while layering, Space,
//! the saved screen with its layers (mute, level, loop, throw one away), the ideas list; and
//! Open in FreeDAWm with every layer on its own track at the start.

mod common;

use common::{offline, ramp, temp_dir};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_recorder::files;
use soundcraft_recorder::layers::Sidecar;
use soundcraft_recorder::rig::InputChoice;
use soundcraft_recorder::settings::Prefs;
use soundcraft_recorder::ui::{COUNT_STEP, RecorderApp, Screen, daw_args, look};
use std::time::{Duration, Instant};

const MONO: InputChoice = InputChoice { on: true, channel: 0, stereo: false };

fn app(dir: &std::path::Path, count_in: bool) -> (RecorderApp, common::Offline) {
    let mut o = offline(1, &["Keys"], MONO);
    let rig = std::mem::replace(&mut o.rig, offline(0, &[], MONO).rig);
    let prefs = Prefs { ideas_folder: dir.to_string_lossy().into_owned(), count_in, ..Prefs::default() };
    (RecorderApp::with_rig(rig, prefs, None), o)
}

fn harness(app: RecorderApp) -> Harness<'static, RecorderApp> {
    let h = Harness::builder().with_size(egui::vec2(look::WIDTH, look::HEIGHT)).build_ui_state(|ui, app: &mut RecorderApp| app.ui(ui), app);
    look::apply(&h.ctx);
    h
}

fn has(h: &Harness<'_, RecorderApp>, text: &str) -> bool {
    h.query_by_label_contains(text).is_some()
}

fn play(h: &mut Harness<'_, RecorderApp>, o: &mut common::Offline, secs: f64) {
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        for _ in 0..4 {
            o.block(&|i, c| 0.5 * ramp(i, c));
        }
        h.step();
    }
}

/// Records a first idea of about `secs`, and leaves the saved screen showing.
fn first_idea(h: &mut Harness<'_, RecorderApp>, o: &mut common::Offline, secs: f64) {
    h.run_steps(2);
    h.state_mut().toggle_record();
    play(h, o, secs);
    h.state_mut().toggle_record();
    h.run_steps(2);
    assert_eq!(h.state().screen, Screen::Idea);
}

#[test]
fn add_a_layer_counts_in_records_over_the_loop_and_shows_the_layers() {
    let dir = temp_dir("ui-layer");
    let (a, mut o) = app(&dir, true);
    let mut h = harness(a);
    first_idea(&mut h, &mut o, 0.6);
    assert!(h.query_by_label("Add a layer").is_some() && h.query_by_label("Record another").is_some());
    assert!(h.query_by_label("Loop is off").is_some(), "one take plays once");
    h.get_by_label("Add a layer").click();
    h.run_steps(2);
    // The 3-2-1.
    assert!(h.state().counting.is_some() && !h.state().recording());
    assert!(has(&h, "Starting in 3"));
    assert!(has(&h, "Get ready to add a layer"));
    assert!(h.query_by_label("Cancel").is_some());
    play(&mut h, &mut o, 3.0 * COUNT_STEP + 0.2);
    assert!(h.state().recording(), "it starts when the count is over");
    // Recording a layer: the idea, the layer, the loop, the time round, the headphones hint once.
    assert!(has(&h, "Adding a layer to Idea"));
    assert!(has(&h, "Recording Layer 2"));
    assert!(has(&h, "of the way round the idea"));
    assert!(has(&h, "Round 1 · each time round is kept as its own layer"));
    assert!(has(&h, "Use headphones so the first part isn't recorded again"));
    assert!(h.state().prefs.layer_hint_seen);
    let len = h.state().idea.as_ref().unwrap().take.seconds.unwrap();
    play(&mut h, &mut o, len * 1.2);
    assert!(has(&h, "Round 2"), "the time round counts");
    // Space stops it, as the big button does.
    h.key_press(egui::Key::Space);
    h.run_steps(3);
    assert!(!h.state().recording());
    assert_eq!(h.state().screen, Screen::Idea);
    let idea = h.state().idea.clone().unwrap();
    assert_eq!(idea.made, vec![2, 3], "two times round, two layers");
    assert!(has(&h, "Layers 2 and 3 saved"));
    assert!(has(&h, "3 layers, playing together"));
    assert!(h.query_by_label("Mute Layer 2").is_some() && h.query_by_label("Layer 3 level").is_some());
    assert!(h.query_by_label("Loop is on").is_some(), "an idea with layers loops");
    assert_eq!(Sidecar::of(&idea.take).pending, None);
    // Mute one: remembered beside the files.
    h.get_by_label("Mute Layer 2").click();
    h.run_steps(2);
    assert!(h.query_by_label("Unmute Layer 2").is_some());
    let take = files::recent(&dir, 10).remove(0);
    assert!(Sidecar::of(&take).mix(2).muted);
    // Throw one layer away: it asks in its row.
    h.get_by_label("Throw Layer 3 away").click();
    h.run_steps(2);
    assert!(has(&h, "Throw layer 3 away?"));
    h.get_by_label("Yes, throw Layer 3 away").click();
    h.run_steps(2);
    assert!(!dir.join(format!("{} (layer 3).wav", idea.take.stem)).exists());
    assert!(dir.join(format!("{} (layer 2).wav", idea.take.stem)).exists());
    // The list: one row.
    // (With its layers listed the screen scrolls: All my ideas is further down.)
    h.state_mut().go(Screen::Main);
    h.run_steps(3);
    let rows: Vec<String> = h.query_all_by_role(egui::accesskit::Role::Label).map(|n| format!("{n:?}")).collect();
    assert!(has(&h, "Today · 2 layers · 0:0"), "one row for the idea and its layers: {rows:?}");
    // A fresh window remembers the mute.
    let (a2, _o2) = app(&dir, true);
    let mut h2 = harness(a2);
    h2.run_steps(2);
    let stem = h2.state().takes[0].stem.clone();
    h2.state_mut().open_idea(&stem);
    h2.run_steps(2);
    assert!(h2.state().idea.as_ref().unwrap().side.mix(2).muted);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn space_cancels_the_count_and_without_it_a_layer_starts_at_once() {
    let dir = temp_dir("ui-layer-space");
    let (a, mut o) = app(&dir, true);
    let mut h = harness(a);
    first_idea(&mut h, &mut o, 0.5);
    h.get_by_label("Add a layer").click();
    h.run_steps(2);
    assert!(h.state().counting.is_some());
    h.key_press(egui::Key::Space);
    h.run_steps(2);
    assert!(h.state().counting.is_none() && !h.state().recording(), "Space during the count: no layer");
    assert_eq!(h.state().screen, Screen::Idea);
    // The count turned off: it starts at once.
    h.state_mut().prefs.count_in = false;
    h.get_by_label("Add a layer").click();
    h.run_steps(2);
    assert!(h.state().recording());
    assert!(has(&h, "Recording Layer 2"));
    play(&mut h, &mut o, 0.3);
    h.get_by_label("Stop").click();
    h.run_steps(3);
    assert!(has(&h, "Layer 2 saved"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn nothing_played_adds_no_layer_and_says_so() {
    let dir = temp_dir("ui-layer-none");
    let (a, mut o) = app(&dir, false);
    let mut h = harness(a);
    first_idea(&mut h, &mut o, 0.5);
    h.get_by_label("Add a layer").click();
    h.run_steps(2);
    let end = Instant::now() + Duration::from_secs_f64(0.4);
    while Instant::now() < end {
        for _ in 0..4 {
            o.block(&|_, _| 0.0);
        }
        h.step();
    }
    h.state_mut().toggle_record();
    h.run_steps(3);
    assert_eq!(h.state().screen, Screen::Idea);
    assert!(has(&h, "Nothing was played, so no layer was added."));
    assert_eq!(files::recent(&dir, 10)[0].layer_count(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_settings_offer_the_count_the_nudge_and_line_up() {
    let dir = temp_dir("ui-layer-settings");
    let (a, _o) = app(&dir, true);
    // Tall enough that the whole settings screen shows (it scrolls in the window).
    let mut h = Harness::builder().with_size(egui::vec2(look::WIDTH, 1600.0)).build_ui_state(|ui, app: &mut RecorderApp| app.ui(ui), a);
    look::apply(&h.ctx);
    h.run_steps(2);
    h.state_mut().go(Screen::Settings);
    h.run_steps(2);
    assert!(h.query_by_label("Count 3-2-1 before a layer starts").is_some());
    assert!(has(&h, "Layers: Not moved"));
    h.get_by_label("Move layers 5 ms earlier").click();
    h.run_steps(2);
    assert_eq!(h.state().prefs.layer_nudge_ms, 5.0);
    assert!(has(&h, "Layers: 5 ms earlier"));
    assert_eq!(h.state().rig.as_ref().unwrap().nudge, 240, "5 ms at 48 kHz");
    assert!(h.query_by_label("Line up").is_some());
    assert!(has(&h, "Not lined up yet"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn open_in_freedawm_puts_every_layer_on_its_own_track_at_the_start() {
    let dir = temp_dir("daw-layers");
    let (a, mut o) = app(&dir, false);
    let mut h = harness(a);
    first_idea(&mut h, &mut o, 0.5);
    h.get_by_label("Add a layer").click();
    h.run_steps(2);
    play(&mut h, &mut o, 0.15);
    o.keys.send("Keys", &[0x90, 60, 100]);
    play(&mut h, &mut o, 0.1);
    o.keys.send("Keys", &[0x80, 60, 0]);
    play(&mut h, &mut o, 0.05);
    h.state_mut().toggle_record();
    h.run_steps(3);
    let take = h.state().idea.clone().unwrap().take;
    assert_eq!(take.layer_count(), 2, "{take:?}");
    let args: Vec<String> = daw_args(&take).iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let names: Vec<String> = args.iter().map(|a| std::path::Path::new(a).file_name().unwrap().to_string_lossy().into_owned()).collect();
    let stem = &take.stem;
    assert_eq!(names, vec![format!("{stem}.wav"), format!("{stem} (layer 2).wav"), format!("{stem} (layer 2).mid")], "every layer, in order");
    // FreeDAWm, as it opens the files named on its command line (`apps/soundcraft` `open_path`).
    let mut e = Engine::default();
    for p in &args {
        let at = e.session().edit.selection.start;
        let id = if p.ends_with(".mid") { "file.import_midi" } else { "file.import_audio" };
        e.execute(id, &json!({"path": p, "at": at})).unwrap();
    }
    let s = e.session();
    let tracks: Vec<(String, i64)> = s.tracks.iter().map(|t| (t.name.clone(), t.clips().first().map_or(-1, |c| c.start))).collect();
    assert_eq!(tracks.len(), 3, "each file on a track of its own: {tracks:?}");
    assert!(tracks.iter().all(|(_, at)| *at == 0), "all at the start: {tracks:?}");
    assert!(tracks[0].0.contains(stem.as_str()) && !tracks[0].0.contains("layer"), "the first take first: {tracks:?}");
    assert!(tracks[1].0.contains("layer 2") && tracks[2].0.contains("layer 2"), "then layer 2: {tracks:?}");
    let _ = std::fs::remove_dir_all(dir);
}
