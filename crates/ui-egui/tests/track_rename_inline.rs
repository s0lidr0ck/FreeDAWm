//! Double-clicking a track's name (the Edit window's name plate, the Mix window's strip name)
//! renames it, keeping exactly what was typed: spaces and punctuation too.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use soundcraft_ui_egui::{Services, SoundApp};

const NAME: &str = "Lead Vox #2 (alt)";

fn harness() -> Harness<'static, SoundApp> {
    let app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..4 {
        h.step();
    }
    h
}

fn press(h: &mut Harness<'_, SoundApp>, at: Pos2, pressed: bool) {
    h.input_mut().events.push(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    h.step();
}

fn key(h: &mut Harness<'_, SoundApp>, key: Key, pressed: bool, modifiers: Modifiers) {
    h.input_mut().events.push(Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers });
}

/// The key a US keyboard types `c` with, and whether Shift is down.
fn key_for(c: char) -> Option<(Key, bool)> {
    match c {
        ' ' => Some((Key::Space, false)),
        '#' => Some((Key::Num3, true)),
        '(' => Some((Key::Num9, true)),
        ')' => Some((Key::Num0, true)),
        c => Key::from_name(&c.to_ascii_uppercase().to_string()).map(|k| (k, c.is_ascii_uppercase())),
    }
}

/// Double-clicks `at`, types [`NAME`] as a keyboard does (a key press, its text, the release),
/// then Enter.
fn double_click_and_type(h: &mut Harness<'_, SoundApp>, at: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(at));
    h.step();
    for _ in 0..2 {
        press(h, at, true);
        press(h, at, false);
    }
    for _ in 0..3 {
        h.step();
    }
    for c in NAME.chars() {
        let k = key_for(c);
        if let Some((k, shift)) = k {
            key(h, k, true, if shift { Modifiers::SHIFT } else { Modifiers::NONE });
        }
        h.input_mut().events.push(Event::Text(c.to_string()));
        h.step();
        if let Some((k, shift)) = k {
            key(h, k, false, if shift { Modifiers::SHIFT } else { Modifiers::NONE });
            h.step();
        }
    }
    key(h, Key::Enter, true, Modifiers::NONE);
    h.step();
    key(h, Key::Enter, false, Modifiers::NONE);
    for _ in 0..3 {
        h.step();
    }
}

#[test]
fn double_clicking_a_track_name_renames_it_as_typed() {
    let mut h = harness();
    let at = h.get_by_label("Kick name").rect().center();
    double_click_and_type(&mut h, at);
    let s = h.state().engine.session();
    assert!(s.track_by_name(NAME).is_some(), "{:?}", s.tracks.iter().map(|t| t.name.clone()).collect::<Vec<_>>());
    assert!(!h.state().is_playing(), "typing a space doesn't start the transport");
}

#[test]
fn double_clicking_a_mix_strip_name_renames_it_as_typed() {
    let mut h = harness();
    h.state_mut().run("window.mix", serde_json::json!({})).unwrap();
    for _ in 0..4 {
        h.step();
    }
    let at = h.get_by_label("Kick strip").rect().center();
    double_click_and_type(&mut h, at);
    let s = h.state().engine.session();
    assert!(s.track_by_name(NAME).is_some(), "{:?}", s.tracks.iter().map(|t| t.name.clone()).collect::<Vec<_>>());
}
