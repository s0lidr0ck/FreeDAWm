//! The Juke window: it opens for a beat track's instrument, and its pad controls edit the kit
//! through commands (so they undo).

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_mix::SlotKey;
use soundcraft_model::TrackId;
use soundcraft_ui_egui::juke_window::Tab;
use soundcraft_ui_egui::{Services, SoundApp};

fn rig(editing: bool) -> (Harness<'static, SoundApp>, TrackId) {
    let mut app = SoundApp::new(soundcraft_engine::Engine::new(soundcraft_model::Session::default()), None, Services::default());
    let v = app.run("track.new_beat", json!({})).unwrap();
    let tid: TrackId = serde_json::from_value(v["track"].clone()).unwrap();
    // A new beat opens its window on the steps; these start on the kit.
    assert_eq!(app.ui.juke_tabs.get(&tid), Some(&Tab::Steps));
    assert!(app.ui.plugin_windows.contains(&(tid, SlotKey::Instrument)));
    app.ui.juke_tabs.insert(tid, Tab::Kit);
    let mut builder = Harness::builder().with_size(vec2(1280.0, 860.0)).with_step_dt(0.1);
    if proof_dir().is_some() {
        builder = builder.wgpu();
    }
    let mut h = builder.build_ui_state(
        move |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    if editing {
        h.get_by_label("Pad").click();
        h.run_steps(3);
    }
    (h, tid)
}

fn proof_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("SOUNDCRAFT_JUKE_PROOF_DIR").map(Into::into)
}

fn proof(h: &mut Harness<SoundApp>, name: &str) {
    if let Some(dir) = proof_dir() {
        std::fs::create_dir_all(&dir).unwrap();
        h.render().unwrap().save(dir.join(format!("{name}.png"))).unwrap();
    }
}

fn param(h: &Harness<SoundApp>, t: TrackId, id: &str) -> Option<f32> {
    h.state().engine.session().track(t).and_then(|t| t.instrument.as_ref()).and_then(|i| i.params.get(id).copied())
}

#[test]
fn the_kit_view_shows_the_kit_and_its_buses() {
    let (mut h, _) = rig(false);
    proof(&mut h, "juke-kit");
    // The pads in view have their Edit and Load buttons.
    assert!(h.get_all_by_label("Edit the pad").count() >= 6);
    assert!(h.get_all_by_label("Load a sample onto the pad").count() >= 6);
}

#[test]
fn the_choke_chip_steps_through_the_groups_and_undoes() {
    let (mut h, t) = rig(false);
    // The kick's choke starts off; clicking picks group 1.
    h.get_all_by_label("Choke –").next().unwrap().click();
    h.run_steps(2);
    assert_eq!(param(&h, t, "p1_choke"), Some(1.0));
    h.state_mut().run("edit.undo", json!({})).unwrap();
    assert_ne!(param(&h, t, "p1_choke"), Some(1.0));
}

#[test]
fn the_pad_view_shows_the_selected_pad() {
    let (mut h, _) = rig(true);
    proof(&mut h, "juke-pad");
    assert!(h.query_by_label("Edit the pad").is_none(), "the pad view replaces the cards");
    h.get_by_label("Back to the kit").click();
    h.run_steps(2);
    assert!(h.get_all_by_label("Edit the pad").count() >= 6);
}

/// A new beat's pattern clip, selected, with the MIDI Editor showing its step grid.
fn grid() -> (Harness<'static, SoundApp>, soundcraft_model::ClipId) {
    let mut app = SoundApp::new(soundcraft_engine::Engine::new(soundcraft_model::Session::default()), None, Services::default());
    let v = app.run("track.new_beat", json!({})).unwrap();
    let clip: soundcraft_model::ClipId = serde_json::from_value(v["clip"].clone()).unwrap();
    app.run("edit.select", json!({"clips": [clip.0]})).unwrap();
    app.ui.show_midi_editor = true;
    app.ui.plugin_windows.clear();
    let mut builder = Harness::builder().with_size(vec2(1280.0, 860.0)).with_step_dt(0.1);
    if proof_dir().is_some() {
        builder = builder.wgpu();
    }
    let mut h = builder.build_ui_state(
        move |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    (h, clip)
}

fn pattern(h: &Harness<SoundApp>, clip: soundcraft_model::ClipId) -> soundcraft_midi::pattern::Pattern {
    let s = h.state().engine.session();
    s.find_clip(clip).and_then(|(_, c)| c.pattern.clone()).unwrap().pattern
}

#[test]
fn the_step_grid_shows_a_pattern_clip_and_its_bar_edits_it() {
    let (mut h, clip) = grid();
    // A groove to draw: four on the floor, a backbeat, an accent, a chance, a roll and a flam.
    {
        let app = h.state_mut();
        for s in [0, 4, 8, 12] {
            app.run("pattern.step", json!({"clip": clip.0, "lane": 0, "step": s, "on": true, "velocity": 127})).unwrap();
        }
        app.run("pattern.step", json!({"clip": clip.0, "lane": 1, "step": 4, "on": true, "accent": true})).unwrap();
        app.run("pattern.step", json!({"clip": clip.0, "lane": 1, "step": 12, "on": true, "flam": true, "condition": "1:2"})).unwrap();
        app.run("pattern.paint", json!({"clip": clip.0, "lane": 2, "from": 0, "to": 15, "on": true, "velocity": 64})).unwrap();
        app.run("pattern.step", json!({"clip": clip.0, "lane": 2, "step": 6, "probability": 40, "roll": 3})).unwrap();
        app.run("pattern.lane", json!({"clip": clip.0, "lane": 3, "length": 12})).unwrap();
        app.run("pattern.step", json!({"clip": clip.0, "lane": 3, "step": 2, "on": true, "velocity": 90})).unwrap();
    }
    h.run_steps(2);
    proof(&mut h, "juke-steps");
    // The New steps tiers and the lane mutes are there.
    h.get_by_label("75").click();
    h.run_steps(2);
    assert!(h.get_all_by_label("Mute the lane").count() >= 4);
    h.get_all_by_label("Mute the lane").next().unwrap().click();
    h.run_steps(2);
    assert!(pattern(&h, clip).lanes[0].mute);
    h.get_by_label("Re-roll").click();
    h.run_steps(2);
    h.get_by_label("Convert to MIDI").click();
    h.run_steps(2);
    assert!(h.state().engine.session().find_clip(clip).is_some_and(|(_, c)| c.pattern.is_none()));
}

#[test]
fn a_bus_effect_opens_its_full_editor_and_edits_through_it() {
    let (mut h, t) = rig(false);
    h.state_mut().run("mix.kit_fx", json!({"track": t.0, "bus": "a", "slot": 1, "plugin": "fs_reverb"})).unwrap();
    h.run_steps(2);
    h.get_by_label("Bus a effect 1").click();
    h.run_steps(4);
    let module = soundcraft_model::KitFx::module_id(soundcraft_model::KitFx::index(soundcraft_model::KitBus::A, 0));
    let app = h.state();
    let opened = app.ui.full_editors.contains(&(t, module)) || app.ui.plugin_windows.contains(&(t, SlotKey::Module(module)));
    assert!(opened, "the effect's editor opens");
    proof(&mut h, "juke-fx-editor");
    // The empty slots still offer effects.
    assert!(h.query_by_label("Add effect on bus b 1").is_some());
}

#[test]
fn the_kit_arrows_step_through_the_starter_kits() {
    let (mut h, t) = rig(false);
    h.get_by_label("Next kit").click();
    h.run_steps(3);
    let name = |h: &Harness<SoundApp>| {
        h.state().engine.session().track(t).and_then(|t| t.instrument.as_ref()).and_then(|i| i.kit.as_ref()).map(|k| k.name.clone())
    };
    assert_eq!(name(&h).as_deref(), Some(soundcraft_engine::juke_kits::KITS[1].name));
    proof(&mut h, "juke-kit-rendered");
    h.get_by_label("Previous kit").click();
    h.run_steps(3);
    assert_eq!(name(&h).as_deref(), Some("Porch Light"));
}

/// A 16-bit mono WAV of a short click.
fn click_wav() -> Vec<u8> {
    let samples: Vec<i16> = (0..4800).map(|i| if i < 200 { if i % 2 == 0 { 20_000 } else { -20_000 } } else { 0 }).collect();
    let data_len = (samples.len() * 2) as u32;
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&48_000u32.to_le_bytes());
    b.extend_from_slice(&96_000u32.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    b
}

#[test]
fn an_installed_recorded_kit_loads_from_the_kits_menu() {
    let d = std::env::temp_dir().join(format!("juke-ui-kits-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let kit = d.join("porch_test");
    std::fs::create_dir_all(&kit).unwrap();
    std::fs::write(kit.join("k.wav"), click_wav()).unwrap();
    std::fs::write(
        kit.join("kit.json"),
        r#"{"name":"Porch Test","license":"CC0","pads":[{"name":"Stomp","family":"kick","samples":{"v1.r1":"k.wav"}}]}"#,
    )
    .unwrap();
    let (mut h, t) = rig(false);
    h.state_mut().juke_kits.dir = Some(d.clone());
    h.run_steps(2);
    h.get_by_label("Kits").click();
    h.run_steps(2);
    h.get_all_by_label_contains("Porch Test").next().unwrap().click();
    h.run_steps(2);
    h.get_by_label("As recorded").click();
    h.run_steps(3);
    let ins = h.state().engine.session().track(t).and_then(|t| t.instrument.clone()).unwrap();
    assert_eq!(ins.kit.as_ref().map(|k| k.name.as_str()), Some("Porch Test"));
    assert!(ins.samples.contains_key("p1.v1.r1"));
    // With a character laid over it.
    h.get_by_label("Kits").click();
    h.run_steps(2);
    h.get_all_by_label_contains("Porch Test ✓").next().unwrap().click();
    h.run_steps(2);
    h.get_by_label("Big Room").click();
    h.run_steps(3);
    let ins = h.state().engine.session().track(t).and_then(|t| t.instrument.clone()).unwrap();
    assert_eq!(ins.kit.as_ref().map(|k| k.name.as_str()), Some("Porch Test · Big Room"));
    assert!(ins.samples.contains_key("p1.v1.r1"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_steps_tab_shows_the_tracks_patterns_in_the_juke_window() {
    let (mut h, t) = rig(false);
    h.get_by_label("Steps").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.ui.juke_tabs.get(&t), Some(&Tab::Steps));
    assert!(!app.ui.show_midi_editor, "the steps are in the Juke window, not the MIDI Editor");
    let first = *app.ui.juke_clips.get(&t).expect("the Steps view edits a pattern clip");
    assert!(app.engine.session().find_clip(first).is_some_and(|(track, c)| track == t && c.pattern.is_some()));
    assert!(h.get_all_by_label("Mute the lane").count() >= 4);
    // A groove from the Grooves menu's command lands on the clip shown.
    let v = h.state_mut().run("pattern.groove", json!({"clip": first.0, "style": "Gospel / Shout"})).unwrap();
    assert_eq!(v["clip"], first.0);
    h.run_steps(3);
    proof(&mut h, "juke-steps-tab");
    // + Pattern makes a second clip and shows it; the first one's tab brings it back.
    h.get_by_label("+ Pattern").click();
    h.run_steps(3);
    let second = *h.state().ui.juke_clips.get(&t).unwrap();
    assert_ne!(second, first);
    let name = h.state().engine.session().find_clip(first).unwrap().1.name.clone();
    h.get_all_by_label_contains(&format!("{name} · ")).next().expect("the clips show as tabs").click();
    h.run_steps(2);
    assert_eq!(h.state().ui.juke_clips.get(&t), Some(&first));
    // Kit goes back to the pads.
    h.get_by_label("Kit").click();
    h.run_steps(2);
    assert!(h.get_all_by_label("Edit the pad").count() >= 6);
}

#[test]
fn a_right_clicked_step_opens_its_details_at_the_step() {
    let (mut h, t) = rig(false);
    h.get_by_label("Steps").click();
    h.run_steps(3);
    let clip = *h.state().ui.juke_clips.get(&t).unwrap();
    let lane = pattern(&h, clip).lanes.iter().position(|_| true).unwrap();
    let lanes: Vec<_> = h.get_all_by_label_contains(" steps").filter(|n| n.rect().width() > 300.0).collect();
    lanes[0].click_secondary();
    h.run_steps(3);
    proof(&mut h, "juke-step-popover");
    for pill in ["Always", "1st of 2", "2nd of 2", "First pass", "Fill bars", "Not in fills", "Accent", "Flam"] {
        assert!(h.query_by_label(pill).is_some(), "{pill}");
    }
    h.get_by_label("Accent").click();
    h.run_steps(2);
    h.get_by_label("Fill bars").click();
    h.run_steps(2);
    let p = pattern(&h, clip);
    let step = p.lanes[lane].steps.iter().find(|s| s.accent).expect("the step took the accent");
    assert_eq!(step.condition, soundcraft_midi::pattern::Condition::Fill);
    h.get_by_label("Close the step details").click();
    h.run_steps(2);
    assert!(h.query_by_label("Fill bars").is_none());
}

#[test]
fn randomize_presets_and_save_preset_work_from_the_top_bar() {
    let d = std::env::temp_dir().join(format!("juke-ui-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let (mut h, t) = rig(true);
    h.state_mut().juke_kits.dir = Some(d.join("kits"));
    proof(&mut h, "juke-pad-editor");
    // On the pad view, Randomize takes only that pad.
    let before = h.state().engine.session().track(t).and_then(|t| t.instrument.clone()).unwrap().params;
    h.get_by_label("Randomize").click();
    h.run_steps(2);
    let after = h.state().engine.session().track(t).and_then(|t| t.instrument.clone()).unwrap().params;
    assert_ne!(before, after);
    assert_eq!(before.get("p2_tone"), after.get("p2_tone"), "the other pads keep their sound");
    // A built-in preset from the Presets menu: its sound, effects and name.
    h.get_by_label("Presets").click();
    h.run_steps(2);
    h.get_all_by_label_contains("Drum Machines").next().unwrap().click();
    h.run_steps(2);
    proof(&mut h, "juke-presets-menu");
    assert!(h.query_all_by_label_contains("(download)").next().is_none(), "the drum machines need no download");
    h.get_by_label("Analog 909").click();
    h.run_steps(3);
    let ins = h.state().engine.session().track(t).and_then(|t| t.instrument.clone()).unwrap();
    assert_eq!(ins.kit.as_ref().map(|k| k.name.as_str()), Some("Analog 909"));
    assert_eq!(ins.params.get("p1_decay"), Some(&450.0));
    let library_fx = soundcraft_engine::juke_presets::find("Analog 909").and_then(|p| p.fx.as_ref()).map_or(0, Vec::len);
    assert_eq!(ins.kit_fx.len(), library_fx);
    // Recorded presets say they need a download until the kit is installed.
    h.get_by_label("Presets").click();
    h.run_steps(2);
    h.get_all_by_label_contains("Acoustic").next().unwrap().hover();
    h.run_steps(3);
    proof(&mut h, "juke-presets-acoustic");
    assert!(h.get_all_by_label_contains("Arena (download)").next().is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    // Save preset: it is listed under My Presets and loads back.
    h.get_by_label("Save preset").click();
    h.run_steps(3);
    let saved = d.join("juke_presets").join("my_analog_909.json");
    assert!(saved.is_file(), "{}", h.state().ui.status);
    h.state_mut().run("mix.kit_load", serde_json::json!({"track": t.0, "kit": "Tin Roof"})).unwrap();
    h.run_steps(2);
    h.get_by_label("Presets").click();
    h.run_steps(2);
    h.get_all_by_label_contains("My Presets").next().unwrap().hover();
    h.run_steps(3);
    h.get_all_by_label("Analog 909").last().unwrap().click();
    h.run_steps(3);
    let back = h.state().engine.session().track(t).and_then(|t| t.instrument.clone()).unwrap();
    assert_eq!(back.params, ins.params);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_pad_editor_has_its_humanize_layers_and_envelope_lamps() {
    let (mut h, t) = rig(true);
    for label in ["Hard layer, round-robin 1", "Ghost layer, round-robin 4", "Humanize Level", "Humanize Timing", "One-shot", "Gate", "Reverse"] {
        assert!(h.query_all_by_label(label).next().is_some(), "{label}");
    }
    // The filter's lamp switches the filter on (low-pass) and off.
    h.get_by_label("Filter envelope on").click();
    h.run_steps(2);
    assert_eq!(param(&h, t, "p1_filter"), Some(1.0));
    h.get_by_label("Pitch envelope on").click();
    h.run_steps(2);
    assert_eq!(param(&h, t, "p1_pitch_amount").map(|v| v.abs() > 0.0), Some(true));
    h.get_by_label("Gate").click();
    h.run_steps(2);
    assert_eq!(param(&h, t, "p1_mode"), Some(1.0));
}
