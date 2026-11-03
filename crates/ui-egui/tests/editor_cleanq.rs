//! CleanQ's VST editor in the Mix window: its size, guides in the module state, the curve cache
//! following undo, and FireStation's preset browser.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Pos2, Rect, Shape, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_dsp::plugins::fs::cleanq::clean_q_ceiling_state::CeilingState;
use soundcraft_dsp::plugins::fs::cleanq::clean_q_guides::Guide;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_ui_egui::SoundApp;

fn state(h: &Harness<SoundApp>, t: TrackId, m: ModuleId) -> CeilingState {
    let s = h.state().engine.session();
    s.track(t).unwrap().mixer.module(m).unwrap().state_bytes().and_then(|b| CeilingState::read(&b)).unwrap_or_default()
}

/// The plot of an editor at `f` with no Ceiling strip open, and its +-18 dB / 10 Hz..30 kHz map.
fn plot(f: Rect) -> Rect {
    Rect::from_min_max(pos2(f.left(), f.top() + 40.0 + 9.0), pos2(f.right(), f.bottom() - 14.0 - 9.0))
}
fn gy(f: Rect, db: f32) -> f32 {
    let p = plot(f);
    p.top() + (0.5 - db / 36.0) * p.height()
}
fn fx(f: Rect, hz: f32) -> f32 {
    let p = plot(f);
    p.left() + ((hz / 10.0).ln() / 3000f32.ln()) * p.width()
}

#[test]
fn cleanq_editor_keeps_its_vst_size() {
    let (h, t, m) = open("fs_cleanq", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    assert_eq!(f.size(), vec2(1010.0, 560.0), "kDefaultWidth x kDefaultHeight");
    // The strips' top: the highest shape painted in Pad's column (under the Mix top bar and the
    // Channel width bar).
    let pad = h.get_by_label("Pad strip").rect();
    let strip_top = h
        .output()
        .shapes
        .iter()
        .map(|c| c.shape.visual_bounding_rect().intersect(c.clip_rect))
        .filter(|r| {
            r.is_positive()
                && r.min.x >= pad.left() - 2.0
                && r.max.x <= pad.right() + 2.0
                && r.min.y > 20.0 + soundcraft_ui_egui::toolbar::MIX_HEIGHT + soundcraft_ui_egui::channel_width::BAR_H
        })
        .map(|r| r.min.y)
        .fold(f32::INFINITY, f32::min);
    assert!((f.top() - strip_top).abs() < 5.0, "the editor starts at the strips' top ({} vs {strip_top})", f.top());
    let bass = h.get_by_label("Bass strip").rect();
    assert!((bass.left() - (f.right() + 1.0)).abs() < 5.0, "the next strip follows the editor: {} vs {}", bass.left(), f.right());
    // Nothing painted below the editor in its column: the space is blank.
    for c in &h.output().shapes {
        let r = c.shape.visual_bounding_rect().intersect(c.clip_rect);
        if r.is_positive() && r.min.x >= f.left() + 1.0 && r.max.x <= f.right() - 1.0 {
            assert!(r.min.y <= f.bottom() + 0.5, "a shape below the editor: {r:?}");
        }
    }
}

#[test]
fn guides_persist_and_undo() {
    let (mut h, t, m) = open("fs_cleanq", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    // Right-click the empty plot: the guide card for a new guide, its note focused.
    right_click(&mut h, pos2(fx(f, 500.0), gy(f, -6.0)));
    h.run_steps(2);
    type_text(&mut h, "boxy here");
    let save = h.get_by_label("Save").rect().center();
    click(&mut h, save);
    let s = state(&h, t, m);
    assert_eq!(s.guides.list.len(), 1, "the guide is in the module state");
    assert_eq!(s.guides.list[0].note, "boxy here");
    assert!((s.guides.list[0].freq_hz - 500.0).abs() < 20.0);
    assert!((s.guides.list[0].gain_db - -6.0).abs() < 0.6);

    // The session keeps it (saved and read back).
    let session = (*h.state().engine.session_arc()).clone();
    let text = serde_json::to_string(&session).unwrap();
    let back: soundcraft_model::Session = serde_json::from_str(&text).unwrap();
    let ins = back.track(t).unwrap().mixer.module(m).unwrap();
    assert_eq!(CeilingState::read(&ins.state_bytes().unwrap()).unwrap().guides.list.len(), 1);

    undo(&mut h);
    assert!(state(&h, t, m).guides.list.is_empty(), "undo takes the guide away");
}

/// The band 1 node as drawn: the 8 px circle in band 1's colour.
fn node(h: &Harness<SoundApp>) -> Option<Pos2> {
    let col = egui::Color32::from_rgb(0x5E, 0xD1, 0xFF);
    h.output().shapes.iter().find_map(|c| match &c.shape {
        Shape::Circle(cs) if (cs.radius - 8.0).abs() < 0.01 && cs.fill == col => Some(cs.center),
        _ => None,
    })
}

#[test]
fn cleanq_curve_cache_follows_undo() {
    let (mut h, t, m) = open("fs_cleanq", vec2(1800.0, 1000.0), "dark");
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"b01_on": 2.0, "b01_type": 0.0}})).unwrap();
    h.run_steps(3);
    let f = frame(&h, t, m);
    let at = node(&h).unwrap();
    assert!((at.y - gy(f, 0.0)).abs() < 1.0, "a bell at 0 dB: {at:?}");
    drag(&mut h, at, at - vec2(0.0, gy(f, 0.0) - gy(f, 6.0)), Modifiers::NONE);
    let moved = node(&h).unwrap();
    assert!((moved.y - gy(f, 6.0)).abs() < 1.5, "dragged to +6 dB: {moved:?}");
    undo(&mut h);
    let back = node(&h).unwrap();
    assert!((back.y - gy(f, 0.0)).abs() < 0.5, "undo puts the node back at 0 dB on the next frame: {back:?}");
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("sc-editor-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn cleanq_presets_browse_recall_and_save() {
    let dir = temp_dir("presets");
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    app.preset_dir = Some(dir.clone());
    let (t, m) = add(&mut app, "Pad", "fs_cleanq");
    app.ui.drawers.insert(t, m);
    let mut h = harness(app, vec2(1800.0, 1000.0), "dark");
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "Presets"})).unwrap();
    h.run_steps(4);

    // The categories: Favorites, All, the factory groups in order, User.
    let info = soundcraft_dsp::plugin_info("fs_cleanq").unwrap();
    let mut want: Vec<String> = vec!["Favorites".into(), "All".into()];
    want.extend(soundcraft_ui_egui::fs_presets::factory_groups(info).iter().map(|(g, _)| g.clone()));
    want.push("User".into());
    for c in &want {
        assert!(h.query_by_label(&format!("Category {c}")).is_some(), "category {c}");
    }

    // The first card recalls through mix.module_params with its name, one undo step.
    let (_, list) = &soundcraft_ui_egui::fs_presets::factory_groups(info)[0];
    let first = list[0].0;
    let steps = h.state().engine.undo_history().len();
    let card = h.get_by_label(&format!("Preset card {first}")).rect().center();
    click(&mut h, card);
    let ins_preset = h.state().engine.session().track(t).unwrap().mixer.module(m).unwrap().preset.clone();
    assert_eq!(ins_preset, first);
    assert_eq!(h.state().engine.undo_history().len(), steps + 1);

    // A guide, then Save under a new name: the file carries the guides under "cleanq".
    let mut st = state(&h, t, m);
    st.guides.add(Guide::around(400.0, -4.0));
    let b64 = soundcraft_model::b64::encode(&st.write());
    h.state_mut().run("mix.module_state", json!({"track": t.0, "module": m.0, "state": b64})).unwrap();
    h.run_steps(2);
    let f = frame(&h, t, m);
    click(&mut h, pos2(f.left() + 60.0, f.bottom() - 14.0 - 13.0));
    key(&mut h, egui::Key::A, Modifiers::COMMAND);
    type_text(&mut h, "My EQ");
    key(&mut h, egui::Key::Enter, Modifiers::NONE);
    h.run_steps(3);
    let file = dir.join(soundcraft_ui_egui::preset_folder_name("fs_cleanq")).join("My EQ.json");
    let text = std::fs::read_to_string(&file).expect("saved into the user preset folder");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["cleanq"]["guides"].as_array().map(Vec::len), Some(1));
    assert_eq!(h.state().engine.session().track(t).unwrap().mixer.module(m).unwrap().preset, "My EQ");

    // Recalled with the guides gone: they come back with it, in the same undo step.
    let empty = soundcraft_model::b64::encode(&CeilingState::default().write());
    h.state_mut().run("mix.module_state", json!({"track": t.0, "module": m.0, "state": empty})).unwrap();
    h.run_steps(2);
    assert!(state(&h, t, m).guides.list.is_empty());
    let user_cat = h.get_by_label("Category User").rect().center();
    click(&mut h, user_cat);
    let steps = h.state().engine.undo_history().len();
    let user = h.get_by_label("Preset card My EQ").rect().center();
    click(&mut h, user);
    assert_eq!(state(&h, t, m).guides.list.len(), 1, "the preset's guides");
    assert_eq!(h.state().engine.undo_history().len(), steps + 1, "params and guides: one undo step");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn scout_guides_draw_but_never_change_the_mixer() {
    let (mut h, t, _m) = open("fs_cleanq", vec2(1800.0, 1000.0), "light");
    let turn: soundcraft_model::ScoutTurn = serde_json::from_value(json!({
        "id": 7,
        "guides": [
            {"kind": "eq", "track": "Pad", "lo_hz": 300.0, "hi_hz": 500.0, "gain_lo_db": -4.0, "gain_hi_db": -2.0, "label": "boxy"},
            {"kind": "eq", "track": "Bass", "lo_hz": 60.0, "hi_hz": 90.0, "gain_lo_db": -3.0, "gain_hi_db": -1.0, "label": "boom"}
        ]
    }))
    .unwrap();
    {
        let s = h.state_mut().engine.session_mut();
        s.scout.turns.push(turn);
        s.scout.guides_shown = Some(7);
    }
    h.run_steps(3);
    let text_at = |h: &Harness<SoundApp>, want: &str| {
        h.output().shapes.iter().find_map(|c| match &c.shape {
            Shape::Text(tx) if tx.galley.job.text == want => Some(tx.visual_bounding_rect()),
            _ => None,
        })
    };
    let chip = text_at(&h, "1  boxy").expect("Scout's guide for Pad is drawn");
    assert!(text_at(&h, "SCOUT").is_some(), "marked as Scout's");
    assert!(text_at(&h, "2  boom").is_none(), "Bass's guide is not on Pad's editor");
    // It takes no gesture: the session's tracks stay byte for byte, and no guide card opens.
    let before = serde_json::to_string(&h.state().engine.session().tracks).unwrap();
    let at = chip.center();
    click(&mut h, at);
    double_click(&mut h, at);
    right_click(&mut h, at);
    hover(&mut h, at);
    key(&mut h, egui::Key::Delete, Modifiers::NONE);
    h.run_steps(3);
    assert_eq!(serde_json::to_string(&h.state().engine.session().tracks).unwrap(), before);
    assert!(h.query_by_label("Save").is_none(), "no guide card");
    let _ = t;
}
