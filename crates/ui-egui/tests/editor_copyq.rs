//! CopyQ's wide editor in the Mix window: its size, picking and blending profiles from the
//! library, weights, the display's handles, the dials and switches, the link group, a user
//! profile on disk, hostile parameters and state, and what needs sound saying so.

mod editor_support;

use editor_support::*;
use egui::{Modifiers, Rect, Shape, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_dsp::plugins::fs::copyq::{CopyQState, Profile};
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_ui_egui::SoundApp;

const SIZE: egui::Vec2 = vec2(1040.0, 656.0);
/// The plot inside the display: x 36..702 (20 Hz..20 kHz), y 85..357.
const PLOT_LEFT: f32 = 36.0;
const PLOT_W: f32 = 666.0;
const PLOT_MID_Y: f32 = 221.0;
const PLOT_BOTTOM: f32 = 357.0;

fn text_shown(h: &Harness<SoundApp>, want: &str) -> bool {
    h.output().shapes.iter().any(|c| matches!(&c.shape, Shape::Text(tx) if tx.galley.job.text == want))
}

fn text_containing(h: &Harness<SoundApp>, want: &str) -> bool {
    h.output().shapes.iter().any(|c| matches!(&c.shape, Shape::Text(tx) if tx.galley.job.text.contains(want)))
}

/// The control labelled `label` inside the editor.
fn at(h: &Harness<SoundApp>, f: Rect, label: &str) -> Option<Rect> {
    h.query_all_by_label(label).map(|n| n.rect()).find(|r| f.expand(1.0).contains_rect(*r))
}

fn need(h: &Harness<SoundApp>, t: TrackId, m: ModuleId, label: &str) -> Rect {
    at(h, frame(h, t, m), label).unwrap_or_else(|| panic!("no `{label}` in the editor"))
}

fn press(h: &mut Harness<SoundApp>, t: TrackId, m: ModuleId, label: &str) {
    let c = need(h, t, m, label).center();
    click(h, c);
}

fn state(h: &Harness<SoundApp>, t: TrackId, m: ModuleId) -> CopyQState {
    let s = h.state().engine.session();
    let ins = s.track(t).unwrap().mixer.module(m).unwrap();
    ins.state_bytes().and_then(|b| CopyQState::read(&b)).unwrap_or_default()
}

fn steps(h: &Harness<SoundApp>) -> usize {
    h.state().engine.undo_history().len()
}

fn x_of(hz: f32) -> f32 {
    PLOT_LEFT + PLOT_W * (hz / 20.0).ln() / 1000f32.ln()
}

#[test]
fn copyq_editor_has_its_size_in_both_themes() {
    for theme in ["dark", "light"] {
        let (h, t, m) = open("fs_copyq", vec2(1800.0, 1000.0), theme);
        assert_eq!(frame(&h, t, m).size(), soundcraft_ui_egui::editors::copyq::SIZE);
        assert_eq!(frame(&h, t, m).size(), SIZE);
        for want in [
            "CopyQ",
            "Amount",
            "Detail",
            "Max Gain",
            "Dynamics",
            "Window",
            "Gate",
            "Low Mid",
            "Output",
            "100 %",
            "12.0 dB",
            "MATCH TO",
            "LIBRARY",
            "Nothing yet: pick a profile below",
            "No target yet: pick a profile",
            "Pink (-3 dB/oct)",
            "Send to CleanQ",
            "this track: live",
        ] {
            assert!(text_shown(&h, want), "{want}");
        }
    }
    assert!(soundcraft_ui_egui::editors::has_wide("fs_copyq"));
}

#[test]
fn copyq_profiles_are_picked_blended_weighted_and_dropped() {
    let (mut h, t, m) = open("fs_copyq", vec2(1800.0, 1000.0), "dark");
    let n = steps(&h);
    // A click on a profile matches to it alone.
    press(&mut h, t, m, "Profile Pink (-3 dB/oct)");
    let st = state(&h, t, m);
    assert_eq!(st.targets.len(), 1);
    assert_eq!((st.targets[0].1.name.as_str(), st.targets[0].0), ("Pink (-3 dB/oct)", 1.0));
    assert_eq!(steps(&h), n + 1, "one undo step");
    assert!(text_containing(&h, "Matching to"), "the foot says so");
    assert!(text_shown(&h, "Play to match"));
    // Another click replaces it; its + blends one in.
    press(&mut h, t, m, "Profile Dark (-6 dB/oct)");
    assert_eq!(state(&h, t, m).targets.len(), 1);
    press(&mut h, t, m, "Blend in Flat (white noise)");
    let st = state(&h, t, m);
    assert_eq!(st.targets.iter().map(|(_, p)| p.name.as_str()).collect::<Vec<_>>(), ["Dark (-6 dB/oct)", "Flat (white noise)"]);
    // With two, each has a weight: dragging the first one's slider to its left end is one step.
    let n = steps(&h);
    let drop = need(&h, t, m, "Remove target Dark (-6 dB/oct)");
    let slider_right = egui::pos2(drop.left() - 12.0, drop.center().y);
    drag(&mut h, slider_right, slider_right - vec2(40.0, 0.0), Modifiers::NONE);
    let w = state(&h, t, m).targets[0].0;
    assert!(w > 0.2 && w < 0.7, "{w}");
    assert_eq!(steps(&h), n + 1, "the whole drag is one undo step");
    assert_eq!(state(&h, t, m).targets[1].0, 1.0);
    // Dropping one leaves the other.
    press(&mut h, t, m, "Remove target Dark (-6 dB/oct)");
    let st = state(&h, t, m);
    assert_eq!(st.targets.len(), 1);
    assert_eq!(st.targets[0].1.name, "Flat (white noise)");
    undo(&mut h);
    assert_eq!(state(&h, t, m).targets.len(), 2);
    // A fifth profile takes the oldest one's place.
    for name in ["Pink (-3 dB/oct)", "Warm (-4.5 dB/oct)", "Bright (-1.5 dB/oct)"] {
        press(&mut h, t, m, &format!("Blend in {name}"));
    }
    let st = state(&h, t, m);
    assert_eq!(st.targets.len(), 4);
    assert!(st.targets.iter().all(|(_, p)| p.name != "Dark (-6 dB/oct)"));
    // The session holds it prepared for the audio thread.
    let s = h.state().engine.session();
    assert!(s.track(t).unwrap().mixer.module(m).unwrap().prepared.is_some());
}

#[test]
fn copyq_dials_go_through_commands_one_undo_step_each() {
    let (mut h, t, m) = open("fs_copyq", vec2(1800.0, 1000.0), "dark");
    for (label, id) in [("Amount", "amount"), ("Detail", "detail"), ("High Mid", "amtHighMid"), ("Output", "output")] {
        let n = steps(&h);
        let before = param(&h, t, m, id);
        let k = need(&h, t, m, label).center();
        drag(&mut h, k, k - vec2(0.0, 25.0), Modifiers::NONE);
        assert!(param(&h, t, m, id) > before + 0.1, "{label}");
        assert_eq!(steps(&h), n + 1, "{label}: one undo step");
        undo(&mut h);
        assert!((param(&h, t, m, id) - before).abs() < 1e-5, "{label}");
    }
    // Amount goes below zero: down from 100 % past the third of its travel that is negative.
    let k = need(&h, t, m, "Amount").center();
    drag(&mut h, k, k + vec2(0.0, 80.0), Modifiers::NONE);
    let v = raw(&h, t, m, "amount");
    assert!(v < 0.0, "{v}");
    assert!(text_shown(&h, &format!("\u{2212}{} %", (-v).round() as i32)));
}

#[test]
fn copyq_range_edges_and_splits_drag() {
    let (mut h, t, m) = open("fs_copyq", vec2(1800.0, 1000.0), "dark");
    let f = frame(&h, t, m);
    // The low edge from 20 Hz to 200 Hz.
    drag(&mut h, f.min + vec2(x_of(20.0), PLOT_MID_Y), f.min + vec2(x_of(200.0), PLOT_MID_Y), Modifiers::NONE);
    let lo = raw(&h, t, m, "rangeLo");
    assert!((lo - 200.0).abs() < 5.0, "{lo}");
    // The high edge dragged past the low one stops half an octave above it.
    drag(&mut h, f.min + vec2(x_of(20_000.0), PLOT_MID_Y), f.min + vec2(x_of(30.0), PLOT_MID_Y), Modifiers::NONE);
    let hi = raw(&h, t, m, "rangeHi");
    assert!((hi / lo - 2f32.sqrt()).abs() < 0.03, "{lo} .. {hi}");
    undo(&mut h);
    undo(&mut h);
    assert_eq!((raw(&h, t, m, "rangeLo"), raw(&h, t, m, "rangeHi")), (20.0, 20_000.0));
    // The middle split, by its handle at the bottom of the plot, from 1 kHz to 2 kHz.
    let y = PLOT_BOTTOM - 12.0;
    drag(&mut h, f.min + vec2(x_of(1_000.0), y), f.min + vec2(x_of(2_000.0), y), Modifiers::NONE);
    let split = raw(&h, t, m, "split2");
    assert!((split - 2_000.0).abs() < 40.0, "{split}");
    assert_eq!(raw(&h, t, m, "split1"), 150.0);
    // A double-click sends it home.
    double_click(&mut h, f.min + vec2(x_of(split), y));
    assert_eq!(raw(&h, t, m, "split2"), 1_000.0);
    // A range whose amount is not 100 % says so on the plot.
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "amtHigh", "value": 40.0})).unwrap();
    h.run_steps(3);
    assert!(text_shown(&h, "40 %"));
}

#[test]
fn copyq_switches_and_the_link_group() {
    let (mut h, t, m) = open("fs_copyq", vec2(1800.0, 1000.0), "dark");
    // The segmented rows: a click in a segment picks it.
    let seg = need(&h, t, m, "Stereo|Mid/Side|Left/Right");
    click(&mut h, seg.center());
    assert_eq!(raw(&h, t, m, "channels"), 1.0);
    click(&mut h, seg.right_center() - vec2(10.0, 0.0));
    assert_eq!(raw(&h, t, m, "channels"), 2.0);
    let seg = need(&h, t, m, "Linear|Minimum");
    click(&mut h, seg.right_center() - vec2(10.0, 0.0));
    assert_eq!(raw(&h, t, m, "phase"), 1.0);
    // The pills flip their switch, one undo step each.
    for (label, id, was) in [("Match Level", "autoGain", 1.0), ("Delta", "delta", 0.0), ("Follow", "follow", 1.0), ("Capture", "capture", 0.0)] {
        assert_eq!(raw(&h, t, m, id), was, "{label}");
        press(&mut h, t, m, label);
        assert_eq!(raw(&h, t, m, id), 1.0 - was, "{label}");
        undo(&mut h);
        assert_eq!(raw(&h, t, m, id), was, "{label}");
    }
    press(&mut h, t, m, "Capture");
    assert!(text_shown(&h, "audio passes untouched") && text_shown(&h, "Capture is on: play to measure"));
    // Link: the role, then the group from its menu.
    let seg = need(&h, t, m, "Off|Send|Match|Group");
    click(&mut h, seg.left_center() + vec2(seg.width() * 0.375, 0.0));
    assert_eq!(raw(&h, t, m, "link"), 1.0, "Send");
    press(&mut h, t, m, "No group");
    h.run_steps(3);
    let pick = h.get_by_label("Group 1").rect().center();
    click(&mut h, pick);
    h.run_steps(3);
    let group = h.state().engine.session().track(t).unwrap().mixer.module(m).unwrap().link_group;
    assert_eq!(group, Some(1));
    assert!(at(&h, frame(&h, t, m), "Group 1").is_some(), "the button names the group");
}

#[test]
fn copyq_says_what_needs_sound_and_changes_nothing() {
    let (mut h, t, m) = open("fs_copyq", vec2(1800.0, 1000.0), "dark");
    let n = steps(&h);
    let modules = |h: &Harness<SoundApp>| h.state().engine.session().track(t).unwrap().mixer.inserts.len();
    // Nothing plays in a test: Save and Freeze have nothing to work from and are off.
    press(&mut h, t, m, "Save");
    press(&mut h, t, m, "Freeze");
    assert!(state(&h, t, m).source.is_none());
    // Send to CleanQ with nothing matched adds nothing and says why.
    press(&mut h, t, m, "Send to CleanQ");
    assert_eq!(modules(&h), 1);
    assert!(text_containing(&h, "Nothing is being matched yet"));
    // From Selection with no selection is refused in words.
    press(&mut h, t, m, "From Selection");
    assert!(state(&h, t, m).targets.is_empty());
    // Reset is an action, not an edit.
    press(&mut h, t, m, "Reset");
    assert_eq!(steps(&h), n, "none of that is an undo step");
    // A frozen source (set by command, as Freeze does) shows as frozen and Unfreeze drops it.
    let pink = h.state_mut().run("mix.copyq_measure", json!({"factory": "Pink (-3 dB/oct)"})).unwrap()["profile"].clone();
    h.state_mut().run("mix.module_copyq", json!({"track": t.0, "module": m.0, "source": pink})).unwrap();
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "follow", "value": 0.0})).unwrap();
    h.run_steps(3);
    assert!(text_shown(&h, "this track: frozen"));
    let n = steps(&h);
    press(&mut h, t, m, "Unfreeze");
    assert!(state(&h, t, m).source.is_none());
    assert_eq!(raw(&h, t, m, "follow"), 1.0);
    assert_eq!(steps(&h), n + 1, "the source and Follow together are one undo step");
    undo(&mut h);
    assert!(state(&h, t, m).source.is_some() && raw(&h, t, m, "follow") == 0.0);
}

#[test]
fn copyq_lists_uses_and_deletes_a_profile_on_disk() {
    let dir = std::env::temp_dir().join(format!("copyq-editor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let lib = dir.join("CopyQ Profiles");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("My Snare.json"), Profile::tilt("anything", -2.0).to_json()).unwrap();
    std::fs::write(lib.join("junk.json"), "{").unwrap();
    let mut app = app_with_tracks(&["Pad", "Bass"]);
    let (t, m) = add(&mut app, "Pad", "fs_copyq");
    app.preset_dir = Some(dir.clone());
    app.ui.drawers.insert(t, m);
    let mut h = harness(app, vec2(1800.0, 1000.0), "dark");
    h.run_steps(4);
    assert!(at(&h, frame(&h, t, m), "Profile junk").is_none(), "what is not a profile is not listed");
    press(&mut h, t, m, "Profile My Snare");
    let st = state(&h, t, m);
    assert_eq!(st.targets[0].1.name, "My Snare", "a profile is named after its file");
    assert!(at(&h, frame(&h, t, m), "Delete profile Pink (-3 dB/oct)").is_none(), "factory profiles stay");
    press(&mut h, t, m, "Delete profile My Snare");
    assert!(!lib.join("My Snare.json").exists());
    assert!(at(&h, frame(&h, t, m), "Profile My Snare").is_none());
    assert_eq!(state(&h, t, m).targets.len(), 1, "the module keeps its copy");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn copyq_editor_survives_hostile_params_and_state_and_follows_undo() {
    let (mut h, t, m) = open("fs_copyq", vec2(1800.0, 1000.0), "dark");
    set_hostile(&mut h, t, m);
    h.run_steps(6);
    assert_eq!(frame(&h, t, m).size(), SIZE);
    assert!(state(&h, t, m).targets.is_empty(), "junk state reads as no target");
    let k = need(&h, t, m, "Amount").center();
    drag(&mut h, k, k + vec2(0.0, 20.0), Modifiers::NONE);
    let v = raw(&h, t, m, "amount");
    assert!(v.is_finite() && (-100.0..=200.0).contains(&v), "{v}");
    // A profile picked over junk state replaces it.
    press(&mut h, t, m, "Profile Pink (-3 dB/oct)");
    assert_eq!(state(&h, t, m).targets.len(), 1);

    let (mut h, t, m) = open("fs_copyq", vec2(1800.0, 1000.0), "dark");
    h.state_mut().run("mix.module_param", json!({"track": t.0, "module": m.0, "param": "detail", "value": 83.0})).unwrap();
    h.run_steps(3);
    assert!(text_shown(&h, "83 %"));
    undo(&mut h);
    assert!(!text_shown(&h, "83 %"));
}

#[test]
fn copyq_presets_step_from_the_header_and_leave_capture_and_link_alone() {
    let (mut h, t, m) = open("fs_copyq", vec2(1800.0, 1000.0), "dark");
    let info = soundcraft_dsp::plugin_info("fs_copyq").unwrap();
    let groups = soundcraft_ui_egui::fs_presets::factory_groups(info);
    assert_eq!(groups.iter().map(|g| g.1.len()).sum::<usize>(), 12);
    h.state_mut().run("mix.module_params", json!({"track": t.0, "module": m.0, "params": {"capture": 1.0, "link": 2.0}})).unwrap();
    h.run_steps(3);
    let preset = |h: &Harness<SoundApp>| h.state().engine.session().track(t).unwrap().mixer.module(m).unwrap().preset.clone();
    // The browser's first card.
    h.state_mut().run("ui.mix.drawer", json!({"track": t.0, "module": m.0, "tab": "Presets"})).unwrap();
    h.run_steps(4);
    let first = groups[0].1[0].0;
    let card = h.get_by_label(&format!("Preset card {first}")).rect().center();
    click(&mut h, card);
    h.run_steps(3);
    assert_eq!(preset(&h), first);
    assert_eq!((raw(&h, t, m, "capture"), raw(&h, t, m, "link"), raw(&h, t, m, "bypass")), (1.0, 2.0, 0.0));
}
