//! Regression: the clip context menu must stay open while the pointer moves onto it, so its
//! Delete item can be used. It used to vanish once the pointer left the lane, because the lane
//! returned early (no hover position) before the menu was drawn.

use egui::{Event, Modifiers, PointerButton, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundcraft_ui_egui::{Services, SoundApp};

/// One UI frame, with queued synthetic pointer input applied first (as the control channel does).
fn step(h: &mut Harness<'_, SoundApp>) {
    let mut raw = std::mem::take(h.input_mut());
    h.state_mut().raw_input_hook(&mut raw);
    *h.input_mut() = raw;
    h.step();
}

fn press(h: &mut Harness<'_, SoundApp>, x: f32, y: f32, button: PointerButton) {
    let pos = pos2(x, y);
    let s = &mut h.state_mut().synthetic;
    s.push(Event::PointerMoved(pos));
    s.push(Event::PointerButton { pos, button, pressed: true, modifiers: Modifiers::NONE });
    s.push(Event::PointerButton { pos, button, pressed: false, modifiers: Modifiers::NONE });
}

fn kick_clip_count(h: &Harness<'_, SoundApp>) -> usize {
    let s = h.state().engine.session();
    s.tracks.iter().find(|t| t.name == "Kick").map_or(0, |t| t.clips().len())
}

#[test]
fn clip_menu_stays_open_while_moving_onto_delete() {
    let engine = soundcraft_engine::demo::demo_engine();
    let app = SoundApp::new(engine, None, Services::default());
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).with_pixels_per_point(2.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..6 {
        step(&mut h);
    }
    let before = kick_clip_count(&h);
    assert!(before > 0, "the demo session should have a recorded Kick clip");

    // Right-click the Kick clip, then move the pointer into the menu and down to Delete.
    press(&mut h, 880.0, 240.0, PointerButton::Secondary);
    for _ in 0..10 {
        step(&mut h);
    }
    // The menu's Delete item, wherever the items above it put it.
    let delete = h.get_by_label("Delete").rect();
    let (x, y) = (delete.left() + 20.0, delete.center().y);
    // Move (no click) into the menu, as a real pointer would, so the lane loses its hover.
    for (x, y) in [(900.0, 300.0), (x, y)] {
        h.state_mut().synthetic.push(Event::PointerMoved(pos2(x, y)));
        for _ in 0..10 {
            step(&mut h);
        }
    }
    assert_eq!(h.get_by_label("Delete").rect(), delete, "the menu stayed open where it was");
    press(&mut h, x, y, PointerButton::Primary);
    for _ in 0..15 {
        step(&mut h);
    }

    assert_eq!(kick_clip_count(&h), before - 1, "clicking Delete in the clip menu should remove the clip");
}

fn app_harness() -> Harness<'static, SoundApp> {
    let engine = soundcraft_engine::demo::demo_engine();
    let app = SoundApp::new(engine, None, Services::default());
    Harness::builder().with_size(vec2(1600.0, 1000.0)).with_pixels_per_point(2.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    )
}

fn kick_clips(h: &Harness<'_, SoundApp>) -> Vec<soundcraft_model::ClipId> {
    let s = h.state().engine.session();
    s.tracks.iter().find(|t| t.name == "Kick").map(|t| t.clips().iter().map(|c| c.id).collect()).unwrap_or_default()
}

/// An audio clip's menu has the ARA submenu: greyed, with a Rescan item, while no ARA plug-in was
/// found.
#[test]
fn audio_clip_menu_has_the_ara_submenu() {
    let mut h = app_harness();
    for _ in 0..6 {
        step(&mut h);
    }
    press(&mut h, 880.0, 240.0, PointerButton::Secondary);
    for _ in 0..10 {
        step(&mut h);
    }
    // No ARA plug-in scanned in tests: the item is there, greyed, and a rescan is offered.
    assert!(h.query_by_label("ARA").is_some(), "the ARA item is in the audio clip's menu");
    assert!(h.query_by_label("Rescan plugins").is_some());
}

/// On a clip given to an ARA plug-in (here one this machine lacks): Open Editor, Bypass, Remove.
#[test]
fn an_ara_clip_menu_bypasses_and_removes() {
    let mut h = app_harness();
    for _ in 0..6 {
        step(&mut h);
    }
    // Every Kick clip, so the one under the pointer has it.
    let ids = kick_clips(&h);
    for id in &ids {
        h.state_mut().engine.session_mut().find_clip_mut(*id).unwrap().ara =
            Some(soundcraft_model::ClipAra { plugin: "vst3:00000000000000000000000000000001".into(), name: "Tuner".into(), ..Default::default() });
    }
    for _ in 0..4 {
        step(&mut h);
    }
    press(&mut h, 880.0, 240.0, PointerButton::Secondary);
    for _ in 0..10 {
        step(&mut h);
    }
    let ara = h.get(egui_kittest::kittest::By::new().label_contains("ARA")).rect();
    press(&mut h, ara.center().x, ara.center().y, PointerButton::Primary);
    for _ in 0..10 {
        step(&mut h);
    }
    assert!(h.query_by_label("Open Tuner Editor").is_some());
    assert!(h.query_by_label("Bypass Tuner").is_some());
    let remove = h.get_by_label("Remove Tuner").rect();
    press(&mut h, remove.center().x, remove.center().y, PointerButton::Primary);
    for _ in 0..10 {
        step(&mut h);
    }
    let left = ids.iter().filter(|id| h.state().engine.session().find_clip(**id).unwrap().1.ara.is_some()).count();
    assert_eq!(left, ids.len() - 1, "Remove took the clicked clip back");
}
