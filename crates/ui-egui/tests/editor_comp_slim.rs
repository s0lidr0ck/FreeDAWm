//! Comp's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, its header, its
//! slots, its More section and display toggle, the model menu, and the menu item and command.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_ui_egui::SoundApp;

const W: f32 = soundcraft_ui_egui::module_drawer::DRAWER_W;

/// The node labelled `label` inside `f`.
fn at(h: &Harness<SoundApp>, f: Rect, label: &str) -> Option<Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.expand(1.0).contains_rect(*r))
}

fn has(h: &Harness<SoundApp>, f: Rect, label: &str) -> bool {
    at(h, f, label).is_some()
}

/// Comp on "Pad" in the slim layout, its drawer open.
fn slim() -> (Harness<'static, SoundApp>, TrackId, ModuleId) {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t, m) = add(&mut app, "Pad", "fs_comp");
    app.ui.drawers.insert(t, m);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1400.0, 1000.0), "dark");
    h.run_steps(6);
    (h, t, m)
}

fn set(h: &mut Harness<SoundApp>, t: TrackId, m: ModuleId, params: serde_json::Value) {
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": params})).unwrap();
    h.run_steps(4);
}

fn steps(h: &Harness<SoundApp>) -> usize {
    h.state().engine.undo_history().len()
}

#[test]
fn the_header_stays_inside_the_drawer() {
    let (h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!((f.width() - W).abs() <= 1.0, "the classic drawer's width: {}", f.width());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["Stereo", "Undo", "1", "2", "Bypass", "Previous preset", "Next preset"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in the header"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
}

#[test]
fn a_slot_on_button_toggles_its_lane_as_one_undo_step() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    // Lane A starts on.
    assert_eq!(raw(&h, t, m, "aOn"), 1.0);
    let n = steps(&h);
    let at_ = at(&h, f, "A on").expect("A's On button").center();
    click(&mut h, at_);
    assert_eq!(raw(&h, t, m, "aOn"), 0.0, "A off");
    assert_eq!(steps(&h), n + 1, "one undo step");
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "aOn"), 1.0);
    let dry = raw(&h, t, m, "dryOn");
    let at_ = at(&h, f, "Dry on").expect("the dry On button").center();
    click(&mut h, at_);
    assert_ne!(raw(&h, t, m, "dryOn"), dry, "dry toggled");
    // In Serial a lane that is off reads BYP; the button still turns it on.
    set(&mut h, t, m, json!({"split": 2.0, "bOn": 0.0}));
    let at_ = at(&h, f, "B on").expect("B's On button").center();
    click(&mut h, at_);
    assert_eq!(raw(&h, t, m, "bOn"), 1.0);
}

#[test]
fn a_slot_drag_is_one_step_and_selects_its_lane() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let n = steps(&h);
    let level = raw(&h, t, m, "bLevel");
    let k = at(&h, f, "B level").expect("B's level knob").center();
    drag(&mut h, k, k - vec2(0.0, 20.0), Modifiers::NONE);
    assert!(raw(&h, t, m, "bLevel") > level + 0.1, "B's level went up");
    assert_eq!(steps(&h), n + 1, "the drag is one undo step");
    // The drag selected lane B: the big knobs now edit B.
    let b = raw(&h, t, m, "bAmount");
    let a = raw(&h, t, m, "amount");
    let f = frame(&h, t, m);
    let thr = at(&h, f, "Input").or_else(|| at(&h, f, "Threshold")).expect("lane B's amount knob").center();
    drag(&mut h, thr, thr - vec2(0.0, 25.0), Modifiers::NONE);
    assert_ne!(raw(&h, t, m, "bAmount"), b, "lane B's amount moved");
    assert_eq!(raw(&h, t, m, "amount"), a, "lane A's did not");
}

#[test]
fn a_slot_double_click_is_unity() {
    let (mut h, t, m) = slim();
    set(&mut h, t, m, json!({"aLevel": 0.3}));
    let f = frame(&h, t, m);
    let at_ = at(&h, f, "A level").unwrap().center();
    double_click(&mut h, at_);
    assert!((raw(&h, t, m, "aLevel") - 0.75).abs() < 1e-4, "unity: {}", raw(&h, t, m, "aLevel"));
}

#[test]
fn crossovers_show_only_in_multiband() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!(!has(&h, f, "Crossover"), "no crossovers in Parallel");
    set(&mut h, t, m, json!({"split": 1.0}));
    let f = frame(&h, t, m);
    assert!(has(&h, f, "Crossover"), "crossovers in Multiband");
}

#[test]
fn more_opens_and_closes() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    assert!(!has(&h, f, "Stereo link"), "More starts closed");
    let h0 = f.height();
    let at_ = at(&h, f, "More").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    let f = frame(&h, t, m);
    assert!(has(&h, f, "Stereo link"), "the small knobs are in More");
    assert!(has(&h, f, "Auto gain"));
    assert!(f.height() > h0, "the editor grew");
    let at_ = at(&h, f, "More").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    assert!(!has(&h, frame(&h, t, m), "Stereo link"), "closed again");
}

#[test]
fn two_comps_keep_their_own_display_and_more() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t1, m1) = add(&mut app, "Pad", "fs_comp");
    let (t2, m2) = add(&mut app, "Bass", "fs_comp");
    app.ui.drawers.insert(t1, m1);
    app.ui.drawers.insert(t2, m2);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1800.0, 1000.0), "dark");
    h.run_steps(6);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(has(&h, f1, "Gain reduction meter") && has(&h, f2, "Gain reduction meter"));
    // The first one shows its curve and opens More; the second stays as it was.
    let tog = at(&h, f1, "GR|Curve").unwrap();
    click(&mut h, egui::pos2(tog.right() - tog.width() / 4.0, tog.center().y));
    let at_ = at(&h, f1, "More").unwrap().center();
    click(&mut h, at_);
    h.run_steps(2);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(has(&h, f1, "Transfer curve"), "the first shows its curve");
    assert!(has(&h, f2, "Gain reduction meter"), "the second still shows its meter");
    assert!(has(&h, f1, "Stereo link") && !has(&h, f2, "Stereo link"), "More is per module");
}

#[test]
fn the_model_menu_picks_a_circuit_with_the_editors_undo() {
    let (mut h, t, m) = slim();
    let f = frame(&h, t, m);
    let at_ = at(&h, f, "Model").unwrap().center();
    click(&mut h, at_);
    let gbus = h.get_by_label("G Bus SSL").rect().center();
    click(&mut h, gbus);
    assert_eq!(raw(&h, t, m, "style"), 4.0, "G Bus");
    // The VST's Undo (pick_style pushed a snapshot) puts Clean back.
    let f = frame(&h, t, m);
    let at_ = at(&h, f, "Undo").unwrap().center();
    click(&mut h, at_);
    assert_eq!(raw(&h, t, m, "style"), 0.0, "Clean again");
}

#[test]
fn the_slim_flag_is_a_command_and_a_menu_item() {
    let (mut h, t, m) = slim();
    // serde default: settings saved before the flag load with it off
    let mut v = serde_json::to_value(soundcraft_ui_egui::UiState::default()).unwrap();
    v.as_object_mut().unwrap().remove("slim_editors");
    let loaded: soundcraft_ui_egui::UiState = serde_json::from_value(v).unwrap();
    assert!(!loaded.slim_editors);
    assert!(!soundcraft_ui_egui::UiState::default().slim_editors, "off by default");

    assert_eq!(h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap(), json!({"on": false}));
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), vec2(960.0, 620.0), "the full editor");
    assert_eq!(h.state_mut().run("ui.mix.slim_editors", json!({})).unwrap(), json!({"on": true}), "flips");
    assert!(h.state_mut().run("ui.mix.slim_editors", json!({"on": 3})).is_err());

    use soundcraft_ui_egui::menus;
    let path = "View > Mix Window Views > Slim Plugin Editors";
    let view = menus::tree().into_iter().find(|n| n.label == "View").unwrap();
    let views = view.children.iter().find(|n| n.label == "Mix Window Views").unwrap();
    assert!(views.children.iter().any(|n| n.path == path));
    let id = menus::command_for_path(path, &menus::ui_aliases()).unwrap();
    assert_eq!(id, "ui.mix.slim_editors");
    assert_eq!(menus::item_state(h.state(), path, Some(&id)), (true, true), "enabled, checked");
    menus::invoke_menu(h.state_mut(), &id, path);
    assert_eq!(menus::item_state(h.state(), path, Some(&id)), (true, false), "unchecked");
    // Greyed out while the VST-faithful editors are off.
    h.state_mut().run("ui.mix.wide_editors", json!({"on": false})).unwrap();
    assert!(!menus::item_state(h.state(), path, Some(&id)).0, "disabled without wide editors");
}

#[test]
fn the_slim_tabs_do_nothing_in_the_full_editor() {
    let (mut h, t, m) = slim();
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(3);
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "More"})).unwrap();
    h.run_steps(3);
    h.state_mut().run("ui.mix.slim_editors", json!({"on": true})).unwrap();
    h.run_steps(4);
    assert!(!has(&h, frame(&h, t, m), "Stereo link"), "a More tab sent to the full editor is not kept for the slim one");
}
