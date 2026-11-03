//! CleanQ's slim editor (View > Mix Window Views > Slim Plugin Editors): its width, the bar on two
//! rows, the band chips and the docked card (the floating card's controls and commands), the
//! dynamics and the Ceiling strip only when they apply, More per module, and the full editor.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_ui_egui::SoundApp;

const W: f32 = soundcraft_ui_egui::module_drawer::DRAWER_W;

fn at(h: &Harness<SoundApp>, f: Rect, label: &str) -> Option<Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.expand(1.0).contains_rect(*r))
}

fn all(h: &Harness<SoundApp>, f: Rect, label: &str) -> Vec<Rect> {
    let mut v: Vec<Rect> = h.query_all_by_label(label).map(|n| n.rect()).filter(|r| f.expand(1.0).contains_rect(*r)).collect();
    v.sort_by(|a, b| a.left().total_cmp(&b.left()));
    v
}

fn has(h: &Harness<SoundApp>, f: Rect, label: &str) -> bool {
    at(h, f, label).is_some()
}

fn slim_with(params: serde_json::Value) -> (Harness<'static, SoundApp>, TrackId, ModuleId) {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t, m) = add(&mut app, "Pad", "fs_cleanq");
    app.run("mix.module_params", json!({"track": t.0, "module": m.0, "params": params})).unwrap();
    app.ui.drawers.insert(t, m);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1400.0, 1000.0), "dark");
    h.run_steps(6);
    (h, t, m)
}

/// Two bells: band 1 at the low mids, band 2 higher up.
fn two_bands() -> (Harness<'static, SoundApp>, TrackId, ModuleId) {
    slim_with(json!({"b01_on": 2.0, "b01_type": 0.0, "b01_freq": 0.3, "b02_on": 2.0, "b02_type": 0.0, "b02_freq": 0.6}))
}

fn steps(h: &Harness<SoundApp>) -> usize {
    h.state().engine.undo_history().len()
}

#[test]
fn the_bar_stays_inside_the_drawer() {
    let (h, t, m) = two_bands();
    let f = frame(&h, t, m);
    assert!((f.width() - W).abs() <= 1.0, "the classic drawer's width: {}", f.width());
    assert!(f.height() < 760.0, "about a strip tall: {}", f.height());
    let inner = f.shrink2(vec2(4.0, 0.0));
    for label in ["+ Band", "A", "Byp", "Fold the drawer", "Ceiling", "Band 1", "Band 2", "More"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label}"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
}

#[test]
fn add_band_makes_and_selects_one_in_one_step() {
    let (mut h, t, m) = slim_with(json!({}));
    let f = frame(&h, t, m);
    assert!(!has(&h, f, "Gain"), "no card without a band");
    let n = steps(&h);
    let add = at(&h, f, "+ Band").unwrap().center();
    click(&mut h, add);
    h.run_steps(2);
    assert_eq!(raw(&h, t, m, "b01_on"), 2.0, "band 1 is on");
    assert_eq!(steps(&h), n + 1, "one undo step");
    let f = frame(&h, t, m);
    assert!(has(&h, f, "Freq") && has(&h, f, "Gain") && has(&h, f, "Q"), "its card is docked under the graph");
}

#[test]
fn a_chip_selects_its_band_and_the_card_edits_it() {
    let (mut h, t, m) = two_bands();
    let f = frame(&h, t, m);
    let chip = at(&h, f, "Band 2").unwrap().center();
    click(&mut h, chip);
    h.run_steps(2);
    let f = frame(&h, t, m);
    // The Gain box: a drag is one undo step on band 2's gain.
    let n = steps(&h);
    let (g1, g2) = (raw(&h, t, m, "b01_gain"), raw(&h, t, m, "b02_gain"));
    let g = at(&h, f, "Gain").expect("the card's Gain").center();
    drag(&mut h, g, g - vec2(0.0, 30.0), Modifiers::NONE);
    assert_ne!(raw(&h, t, m, "b02_gain"), g2, "band 2's gain moved");
    assert_eq!(raw(&h, t, m, "b01_gain"), g1, "band 1's did not");
    assert_eq!(steps(&h), n + 1, "one undo step");
    // The type row: the fourth glyph is the low cut, and a cut has a slope in place of its gain.
    let types = all(&h, frame(&h, t, m), "Type");
    assert_eq!(types.len(), 7, "seven types");
    click(&mut h, types[3].center());
    h.run_steps(2);
    assert_eq!(raw(&h, t, m, "b02_type"), 3.0, "low cut");
    let f = frame(&h, t, m);
    assert!(has(&h, f, "Slope") && !has(&h, f, "Gain"), "the slope box in place of the gain");
}

#[test]
fn the_dynamics_show_only_when_dyn_is_on() {
    let (mut h, t, m) = two_bands();
    let chip = at(&h, frame(&h, t, m), "Band 1").unwrap().center();
    click(&mut h, chip);
    h.run_steps(2);
    let f = frame(&h, t, m);
    assert!(!has(&h, f, "Thresh"), "no dynamics yet");
    let h0 = f.height();
    let d = at(&h, f, "Dyn").unwrap().center();
    click(&mut h, d);
    h.run_steps(3);
    assert_eq!(raw(&h, t, m, "b01_dyn"), 1.0);
    let f = frame(&h, t, m);
    assert!(has(&h, f, "Thresh") && has(&h, f, "Release"), "the dynamics");
    assert!(f.height() > h0, "the editor grew");
}

#[test]
fn the_ceiling_strip_shows_only_while_open_and_wraps_inside() {
    let (mut h, t, m) = two_bands();
    let f = frame(&h, t, m);
    assert!(!has(&h, f, "Conform"), "closed");
    let h0 = f.height();
    let c = at(&h, f, "Ceiling").unwrap().center();
    click(&mut h, c);
    h.run_steps(4);
    let f = frame(&h, t, m);
    let inner = f.shrink2(vec2(2.0, 0.0));
    for label in ["Conform", "Auto Generate", "Cuts only", "Delta", "Pin", "Violet"] {
        let r = at(&h, f, label).unwrap_or_else(|| panic!("{label} in the strip"));
        assert!(inner.contains_rect(r), "{label} at {r:?} leaves the drawer {f:?}");
    }
    assert!(f.height() > h0 + 200.0, "the strip's rows wrapped under the graph");
    // Its controls are the full strip's: Cuts only writes the module state as one undo step.
    let n = steps(&h);
    let cuts = at(&h, f, "Cuts only").unwrap().center();
    click(&mut h, cuts);
    assert_eq!(steps(&h), n + 1);
}

#[test]
fn two_cleanqs_keep_their_own_more() {
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t1, m1) = add(&mut app, "Pad", "fs_cleanq");
    let (t2, m2) = add(&mut app, "Bass", "fs_cleanq");
    app.ui.drawers.insert(t1, m1);
    app.ui.drawers.insert(t2, m2);
    app.ui.slim_editors = true;
    let mut h = harness(app, vec2(1800.0, 1000.0), "dark");
    h.run_steps(6);
    let f1 = frame(&h, t1, m1);
    assert!(!has(&h, f1, "Copy"), "More starts closed");
    let mo = at(&h, f1, "More").unwrap().center();
    click(&mut h, mo);
    h.run_steps(2);
    let (f1, f2) = (frame(&h, t1, m1), frame(&h, t2, m2));
    assert!(has(&h, f1, "Copy") && has(&h, f1, "Hold") && has(&h, f1, "Auto"), "the rest of the bar is in More");
    assert!(!has(&h, f2, "Copy"), "More is per module");
    // Auto is the full bar's: one step.
    let n = steps(&h);
    let a = at(&h, f1, "Auto").unwrap().center();
    click(&mut h, a);
    assert_eq!(raw(&h, t1, m1, "autoGain"), 1.0);
    assert_eq!(steps(&h), n + 1);
}

#[test]
fn the_full_editor_is_unchanged_without_the_flag() {
    let (mut h, t, m) = two_bands();
    h.state_mut().run("ui.mix.slim_editors", json!({"on": false})).unwrap();
    h.run_steps(4);
    assert_eq!(frame(&h, t, m).size(), vec2(1010.0, 560.0), "the full editor");
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "More"})).unwrap();
    h.run_steps(3);
    h.state_mut().run("ui.mix.slim_editors", json!({"on": true})).unwrap();
    h.run_steps(4);
    assert!(!has(&h, frame(&h, t, m), "Copy"), "a More tab sent to the full editor is not kept for the slim one");
}
