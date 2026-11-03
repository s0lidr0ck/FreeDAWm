//! Shared harness helpers for the VST-faithful editor tests (`mod editor_support;`).
#![allow(dead_code)]

use egui::{Modifiers, PointerButton, Pos2, Rect, Vec2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_ui_egui::{Services, SoundApp};

/// The skip list's VST3 tests are not built here; nothing to skip.
pub fn app_with_tracks(names: &[&str]) -> SoundApp {
    let mut engine = soundcraft_engine::Engine::default();
    for n in names {
        engine.execute("track.new", &json!({"name": n})).unwrap();
    }
    SoundApp::new(engine, None, Services::default())
}

/// Adds `plugin` on `track` and returns its ids.
pub fn add(app: &mut SoundApp, track: &str, plugin: &str) -> (TrackId, ModuleId) {
    app.engine.execute("mix.module_add", &json!({"track": track, "plugin": plugin})).unwrap();
    let t = app.engine.session().track_by_name(track).unwrap();
    (t.id, t.mixer.inserts.iter().rev().find(|i| i.plugin == plugin).unwrap().id)
}

/// A Mix window harness of `size` over `app`.
pub fn harness(mut app: SoundApp, size: Vec2, theme: &str) -> Harness<'static, SoundApp> {
    app.run("ui.set", json!({"theme": theme})).unwrap();
    app.run("window.mix", json!({})).unwrap();
    // Real-time-ish steps, so two clicks a step apart are a double-click (egui: within 0.3 s).
    let mut h = Harness::builder().with_size(size).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

/// `plugin` added to "Pad" (with "Bass" next to it), its drawer open, a few frames run.
pub fn open(plugin: &str, size: Vec2, theme: &str) -> (Harness<'static, SoundApp>, TrackId, ModuleId) {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t, m) = add(&mut app, "Pad", plugin);
    app.ui.drawers.insert(t, m);
    let mut h = harness(app, size, theme);
    h.run_steps(4);
    (h, t, m)
}

/// The editor's frame as last drawn.
pub fn frame(h: &Harness<SoundApp>, t: TrackId, m: ModuleId) -> Rect {
    soundcraft_ui_egui::editor_kit::frame_rect(&h.ctx, t, m).expect("the wide editor was drawn")
}

/// A parameter, normalised 0..1 over its range.
pub fn param(h: &Harness<SoundApp>, t: TrackId, m: ModuleId, id: &str) -> f32 {
    let s = h.state().engine.session();
    let ins = s.track(t).unwrap().mixer.module(m).unwrap();
    let info = soundcraft_dsp::plugin_info(&ins.plugin).unwrap();
    let p = info.params.iter().find(|p| p.id == id).unwrap();
    p.to_normalized(p.clamp(ins.params.get(id).copied().unwrap_or(p.default)))
}

/// A parameter's raw value.
pub fn raw(h: &Harness<SoundApp>, t: TrackId, m: ModuleId, id: &str) -> f32 {
    let s = h.state().engine.session();
    let ins = s.track(t).unwrap().mixer.module(m).unwrap();
    let info = soundcraft_dsp::plugin_info(&ins.plugin).unwrap();
    ins.params.get(id).copied().unwrap_or_else(|| info.params.iter().find(|p| p.id == id).unwrap().default)
}

fn pointer(h: &mut Harness<SoundApp>, pos: Pos2, button: PointerButton, pressed: Option<bool>, mods: Modifiers) {
    h.input_mut().events.push(egui::Event::ModifiersChanged(mods));
    h.input_mut().events.push(egui::Event::PointerMoved(pos));
    if let Some(pressed) = pressed {
        h.input_mut().events.push(egui::Event::PointerButton { pos, button, pressed, modifiers: mods });
    }
    h.step();
}

/// Moves the pointer to `at` (hover).
pub fn hover(h: &mut Harness<SoundApp>, at: Pos2) {
    pointer(h, at, PointerButton::Primary, None, Modifiers::NONE);
    h.step();
}

/// Press at `from`, move there in steps of at most 8 px, release at `to`.
pub fn drag(h: &mut Harness<SoundApp>, from: Pos2, to: Pos2, mods: Modifiers) {
    pointer(h, from, PointerButton::Primary, None, mods);
    pointer(h, from, PointerButton::Primary, Some(true), mods);
    let d = to - from;
    let n = (d.length() / 8.0).ceil().max(1.0) as usize;
    for k in 1..=n {
        pointer(h, from + d * (k as f32 / n as f32), PointerButton::Primary, None, mods);
    }
    pointer(h, to, PointerButton::Primary, Some(false), mods);
    h.input_mut().events.push(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

/// `notches` wheel notches (positive: up) at `at`.
pub fn wheel_notches(h: &mut Harness<SoundApp>, at: Pos2, notches: f32) {
    pointer(h, at, PointerButton::Primary, None, Modifiers::NONE);
    h.input_mut().events.push(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: vec2(0.0, notches),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(3);
}

/// One click.
pub fn click(h: &mut Harness<SoundApp>, at: Pos2) {
    pointer(h, at, PointerButton::Primary, None, Modifiers::NONE);
    pointer(h, at, PointerButton::Primary, Some(true), Modifiers::NONE);
    pointer(h, at, PointerButton::Primary, Some(false), Modifiers::NONE);
    h.run_steps(2);
}

/// Two clicks in quick succession.
pub fn double_click(h: &mut Harness<SoundApp>, at: Pos2) {
    pointer(h, at, PointerButton::Primary, None, Modifiers::NONE);
    for _ in 0..2 {
        pointer(h, at, PointerButton::Primary, Some(true), Modifiers::NONE);
        pointer(h, at, PointerButton::Primary, Some(false), Modifiers::NONE);
    }
    h.run_steps(2);
}

/// A secondary click.
pub fn right_click(h: &mut Harness<SoundApp>, at: Pos2) {
    pointer(h, at, PointerButton::Secondary, None, Modifiers::NONE);
    pointer(h, at, PointerButton::Secondary, Some(true), Modifiers::NONE);
    pointer(h, at, PointerButton::Secondary, Some(false), Modifiers::NONE);
    h.run_steps(2);
}

/// A key press (and release) with `mods`.
pub fn key(h: &mut Harness<SoundApp>, key: egui::Key, mods: Modifiers) {
    h.input_mut().events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: mods });
    h.step();
    h.input_mut().events.push(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: mods });
    h.run_steps(2);
}

/// Typed text (into whatever has focus).
pub fn type_text(h: &mut Harness<SoundApp>, text: &str) {
    h.input_mut().events.push(egui::Event::Text(text.to_string()));
    h.run_steps(2);
}

/// `edit.undo`.
pub fn undo(h: &mut Harness<SoundApp>) {
    h.state_mut().run("edit.undo", json!({})).unwrap();
    h.run_steps(3);
}

/// Every parameter NaN or +-1e30 (written into the model directly, as a corrupt file would),
/// and the module state 4 KiB of LCG bytes.
pub fn set_hostile(h: &mut Harness<SoundApp>, t: TrackId, m: ModuleId) {
    let mut seed = 0x1234_5678u32;
    let bytes: Vec<u8> = (0..4096)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 24) as u8
        })
        .collect();
    let s = h.state_mut().engine.session_mut();
    let ins = s.track_mut(t).unwrap().mixer.module_mut(m).unwrap();
    let info = soundcraft_dsp::plugin_info(&ins.plugin).unwrap();
    for (i, p) in info.params.iter().enumerate() {
        let v = match i % 3 {
            0 => f32::NAN,
            1 => 1e30,
            _ => -1e30,
        };
        ins.params.insert(p.id.to_string(), v);
    }
    ins.state = Some(soundcraft_model::b64::encode(&bytes));
}
