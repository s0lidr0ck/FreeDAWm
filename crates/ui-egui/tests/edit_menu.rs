use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, menus};

#[test]
fn undo_and_redo_name_the_step_they_would_take() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let label = |app: &SoundApp, id| menus::live_label(app, Some(id));
    assert_eq!(label(&app, "edit.undo").as_deref(), Some("Can't Undo"));
    app.run("mix.mute", json!({"track": "Kick"})).unwrap();
    assert_eq!(label(&app, "edit.undo").as_deref(), Some("Undo Mute"));
    assert_eq!(label(&app, "edit.redo").as_deref(), Some("Can't Redo"));
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(label(&app, "edit.redo").as_deref(), Some("Redo Mute"));
    assert_eq!(label(&app, "edit.copy"), None);
}

/// Copy Special › Clip Gain and Clip Effects, then Edit › Paste from the menu: the copied gain
/// curve or effect settings land on the clips the paste targets, in one undo step named Paste
/// (they used to fill a clipboard only unreachable commands read, and Paste was greyed out).
#[test]
fn copy_special_clip_gain_and_clip_effects_paste_back_from_the_menu() {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let s = app.engine.session();
    let kick = s.track_by_name("Kick").unwrap();
    let (kt, a, b) = (kick.id, kick.clips()[0].clone(), kick.clips()[1].clone());
    let paste_enabled = |app: &SoundApp| menus::item_state(app, "Edit > Paste", Some("edit.paste")).0;
    let mid_gain = |app: &SoundApp| {
        let c = app.engine.session().find_clip(b.id).unwrap().1.clone();
        c.gain_at(c.length / 2)
    };
    app.run("clip.gain", json!({"clip": a.id.0, "db": -6.0})).unwrap();
    app.run("edit.select", json!({"tracks": [kt.0], "start": a.start, "end": a.end()})).unwrap();
    menus::invoke_menu(&mut app, "edit.copy_clip_gain", "Edit > Copy Special > Clip Gain");
    assert!(paste_enabled(&app), "Paste takes the copied clip gain");
    app.run("edit.select", json!({"tracks": [kt.0], "start": b.start, "end": b.end()})).unwrap();
    assert!((mid_gain(&app) - 1.0).abs() < 1e-3, "the second clip starts at 0 dB");
    let steps = app.engine.undo_history().len();
    menus::invoke_menu(&mut app, "edit.paste", "Edit > Paste");
    let want = 10f32.powf(-6.0 / 20.0);
    assert!((mid_gain(&app) - want).abs() < 1e-3, "pasted -6 dB: {}", mid_gain(&app));
    assert_eq!(app.engine.undo_history().len(), steps + 1, "one undo step");
    assert_eq!(app.engine.undo_label(), Some("Paste"));

    // Clip effects: copied from one clip, pasted onto another selected clip.
    app.run("clip.effects_set", json!({"clip": a.id.0, "params": {"eq_gain_db": 3.0}})).unwrap();
    app.run("edit.select", json!({"tracks": [kt.0], "clips": [a.id.0], "start": a.start, "end": a.end()})).unwrap();
    menus::invoke_menu(&mut app, "edit.copy_clip_effects", "Edit > Copy Special > Clip Effects");
    assert!(paste_enabled(&app), "Paste takes the copied clip effects");
    app.run("edit.select", json!({"tracks": [kt.0], "clips": [b.id.0], "start": b.start, "end": b.end()})).unwrap();
    let steps = app.engine.undo_history().len();
    menus::invoke_menu(&mut app, "edit.paste", "Edit > Paste");
    let effects = |app: &SoundApp, id| app.engine.session().find_clip(id).unwrap().1.effects.clone();
    assert_eq!(effects(&app, b.id), vec![("eq_gain_db".to_string(), 3.0)]);
    assert_eq!(app.engine.undo_history().len(), steps + 1, "one undo step");
}
