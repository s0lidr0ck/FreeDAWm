//! Sift's wide editor in the Mix window: its size, gestures through commands (dials, Mode,
//! Listen, Bypass), presets, hostile parameters and undo made elsewhere.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Pos2, Rect, Shape, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::SoundApp;

const SIZE: egui::Vec2 = vec2(860.0, 536.0);

fn text_shown(h: &Harness<SoundApp>, want: &str) -> bool {
    h.output().shapes.iter().any(|c| matches!(&c.shape, Shape::Text(tx) if tx.galley.job.text == want))
}

/// The controls called `label` inside the editor, left to right.
fn all(h: &Harness<SoundApp>, f: Rect, label: &str) -> Vec<Rect> {
    let mut v: Vec<Rect> = h.query_all_by_label(label).map(|n| n.rect()).filter(|r| f.expand(1.0).contains_rect(*r)).collect();
    v.sort_by(|a, b| a.left().total_cmp(&b.left()));
    v
}

fn one(h: &Harness<SoundApp>, f: Rect, label: &str) -> Rect {
    *all(h, f, label).first().unwrap_or_else(|| panic!("{label}"))
}

/// The middle of segment `i` of `n` in a segmented control.
fn segment(r: Rect, i: usize, n: usize) -> Pos2 {
    pos2(r.left() + r.width() * (i as f32 + 0.5) / n as f32, r.center().y)
}

#[test]
fn sift_editor_has_its_size_in_both_themes() {
    for theme in ["dark", "light"] {
        let (h, t, m) = open("fs_sift", vec2(1800.0, 1000.0), theme);
        assert_eq!(frame(&h, t, m).size(), soundcraft_ui_egui::editors::sift::SIZE);
        assert_eq!(frame(&h, t, m).size(), SIZE);
        for want in
            ["Sift", "Voice", "Sibilance", "Plosives", "Amount", "Range", "Low", "High", "Ceiling", "Mode", "Listen", "50 %", "the treated voice"]
        {
            assert!(text_shown(&h, want), "{want}");
        }
    }
    assert!(soundcraft_ui_egui::editors::has_wide("fs_sift"));
}

#[test]
fn sift_dials_go_through_commands_one_undo_step_each() {
    let (mut h, t, m) = open("fs_sift", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    let amounts = all(&h, f, "Amount");
    assert_eq!(amounts.len(), 2, "one Amount for sibilance and one for plosives");
    for (r, id) in amounts.into_iter().zip(["sibAmount", "ploAmount"]) {
        let before = param(&h, t, m, id);
        drag(&mut h, r.center(), r.center() - vec2(0.0, 25.0), Modifiers::NONE);
        let after = param(&h, t, m, id);
        assert!(after > before + 0.1, "{id}: {before} -> {after}");
        undo(&mut h);
        assert!((param(&h, t, m, id) - before).abs() < 1e-6, "{id}: one undo step");
    }
    let c = one(&h, f, "Ceiling").center();
    drag(&mut h, c, c + vec2(0.0, 25.0), Modifiers::NONE);
    assert!(raw(&h, t, m, "ploHigh") < 180.0);
    let lo = one(&h, f, "Low").center();
    drag(&mut h, lo, lo - vec2(0.0, 25.0), Modifiers::NONE);
    assert!(raw(&h, t, m, "sibLow") > 4_500.0);
}

#[test]
fn sift_mode_listen_and_bypass_are_switches() {
    let (mut h, t, m) = open("fs_sift", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    let mode = one(&h, f, "Split|Wide");
    click(&mut h, segment(mode, 1, 2));
    assert_eq!(raw(&h, t, m, "mode"), 1.0);
    undo(&mut h);
    assert_eq!(raw(&h, t, m, "mode"), 0.0);

    let listen = one(&h, f, "Off|Sibilance|Plosives|Removed");
    for (i, note) in
        [(1, "only what it takes for sibilance"), (2, "only what it takes for a plosive"), (3, "only what it removes"), (0, "the treated voice")]
    {
        click(&mut h, segment(listen, i, 4));
        assert_eq!(raw(&h, t, m, "listen"), i as f32);
        assert!(text_shown(&h, note), "{note}");
    }

    let b = one(&h, f, "Bypass").center();
    click(&mut h, b);
    assert_eq!(raw(&h, t, m, "bypass"), 1.0);
}

#[test]
fn sift_editor_survives_hostile_params_and_follows_undo() {
    let (mut h, t, m) = open("fs_sift", vec2(1800.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(6);
    let f = frame(&h, t, m);
    assert_eq!(f.size(), SIZE);
    let k = one(&h, f, "Amount").center();
    drag(&mut h, k, k + vec2(0.0, 20.0), Modifiers::NONE);
    let v = raw(&h, t, m, "sibAmount");
    assert!(v.is_finite() && (0.0..=100.0).contains(&v), "{v}");

    let (mut h, t, m) = open("fs_sift", vec2(1800.0, 1000.0), "dark");
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "ploAmount", "value": 80.0})).unwrap();
    h.run_steps(3);
    assert!(text_shown(&h, "80 %"));
    undo(&mut h);
    assert!(!text_shown(&h, "80 %"));
}

#[test]
fn sift_presets_step_from_the_header_and_leave_listen_alone() {
    let (mut h, t, m) = open("fs_sift", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    let info = soundcraft_dsp::plugin_info("fs_sift").unwrap();
    let groups = soundcraft_ui_egui::fs_presets::factory_groups(info);
    assert_eq!(groups.iter().map(|g| g.1.len()).sum::<usize>(), 8);
    let preset = |h: &Harness<SoundApp>| h.state().engine.session().track(t).unwrap().mixer.module(m).unwrap().preset.clone();

    let listen = one(&h, f, "Off|Sibilance|Plosives|Removed");
    click(&mut h, segment(listen, 1, 4));
    // The arrows either side of the preset name, in the header's row.
    let (prev, next) = (f.min + vec2(189.0, 15.0 + 13.0), f.min + vec2(454.0, 15.0 + 13.0));
    click(&mut h, next);
    assert_eq!(preset(&h), groups.first().unwrap().1.first().unwrap().0, "the right arrow steps forward from Init");
    assert_eq!(raw(&h, t, m, "listen"), 1.0, "a preset is a sound, not what you are auditioning");
    assert_eq!(raw(&h, t, m, "bypass"), 0.0);
    click(&mut h, prev);
    assert_eq!(preset(&h), groups.last().unwrap().1.last().unwrap().0, "round to the last");
}
