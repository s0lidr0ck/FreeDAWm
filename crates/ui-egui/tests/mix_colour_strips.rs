//! Colour strips (`strip_tint`, the Mix window's Colour Off / Subtle / Medium / Strong): the
//! strip's background takes the track's colour, every label stays readable on it in both themes
//! for every colour, Off draws as before, and the amount is kept with the preferences.

use egui::{Color32, Shape, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_ui_egui::strip_tint::{MIN_CONTRAST, STEPS, contrast, look_with, step_of};
use soundcraft_ui_egui::theme::{ThemeMode, Tokens};
use soundcraft_ui_egui::{Services, SoundApp, UiState};

// The palette is process-wide, so tests in this file take turns.
static PALETTE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The track palette and the hard cases: pure primaries, yellow, white-ish, near-black.
fn colours() -> Vec<[u8; 3]> {
    let mut v = soundcraft_model::TRACK_COLORS.to_vec();
    v.extend([[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0], [255, 0, 255], [0, 255, 255], [250, 240, 200], [40, 10, 10], [140, 30, 30]]);
    v
}

#[test]
fn every_colour_at_every_amount_keeps_labels_readable_in_both_themes() {
    for (name, t) in [("dark", Tokens::DARK), ("light", Tokens::LIGHT)] {
        for c in colours() {
            for (amount, label) in STEPS.into_iter().skip(1) {
                for selected in [false, true] {
                    let look = look_with(&t, c, amount, selected).unwrap_or_else(|| panic!("{name} {c:?} {label}: tinted"));
                    for (surface, what) in [(look.bg, "strip"), (look.top, "strip top")] {
                        for (text, tname) in [(t.text, "text"), (t.text_dim, "dim text")] {
                            let k = contrast(text, surface);
                            assert!(k >= MIN_CONTRAST, "{name} {c:?} {label} selected={selected}: {tname} on {what} {surface:?} is {k:.2}:1");
                        }
                    }
                    for (text, tname) in [(t.section_label, "section label"), (t.text, "text"), (t.text_dim, "dim text")] {
                        let k = contrast(text, look.section);
                        assert!(k >= MIN_CONTRAST, "{name} {c:?} {label}: {tname} on a section {:?} is {k:.2}:1", look.section);
                    }
                    // The selection outline reads on the strip.
                    assert!(contrast(look.outline, look.bg) >= 3.0, "{name} {c:?} {label}: selection outline");
                }
            }
        }
    }
}

/// A coloured track's strip reads as its colour: its strongest channel leads at Strong, and the
/// tint grows with the amount (further from the untinted strip at each step).
#[test]
fn the_tint_is_the_tracks_colour_and_grows_with_the_amount() {
    let dist = |a: Color32, b: Color32| {
        let d = |x: u8, y: u8| (f32::from(x) - f32::from(y)).abs();
        d(a.r(), b.r()) + d(a.g(), b.g()) + d(a.b(), b.b())
    };
    for t in [Tokens::DARK, Tokens::LIGHT] {
        for (c, lead) in [([222, 104, 72], 0), ([104, 186, 98], 1), ([86, 132, 214], 2), ([196, 86, 168], 0)] {
            let strong = look_with(&t, c, 1.0, false).unwrap().bg;
            let ch = [strong.r(), strong.g(), strong.b()];
            let max = ch.iter().copied().max().unwrap();
            assert_eq!(ch[lead], max, "{c:?} → {strong:?}");
            let mut last = 0.0;
            for (amount, label) in STEPS.into_iter().skip(1) {
                let d = dist(look_with(&t, c, amount, false).unwrap().bg, t.strip_bg);
                assert!(d > last, "{c:?} {label}: {d} after {last}");
                last = d;
            }
        }
        // Grey stays neutral; Off, nonsense amounts too.
        assert_eq!(look_with(&t, [128, 128, 128], 1.0, false), None);
        assert_eq!(look_with(&t, [222, 104, 72], 0.0, false), None);
        assert_eq!(look_with(&t, [222, 104, 72], f32::NAN, false), None);
    }
    assert_eq!(step_of(0.351), Some(0.35));
    assert_eq!(step_of(0.5), None);
}

fn mix(mode: ThemeMode, amount: Option<f32>) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.engine.session_mut().edit.selected_tracks.clear();
    app.run("window.mix", json!({})).unwrap();
    app.run("ui.theme", json!({"mode": mode.id()})).unwrap();
    if let Some(a) = amount {
        app.run("ui.set", json!({"mix_strip_tint": a})).unwrap();
    }
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

/// Every colour painted by a mesh (the strips' gradients are meshes).
fn mesh_colours(h: &Harness<SoundApp>) -> Vec<Color32> {
    let mut out = Vec::new();
    for s in &h.output().shapes {
        if let Shape::Mesh(m) = &s.shape {
            out.extend(m.vertices.iter().map(|v| v.color));
        }
    }
    out
}

#[test]
fn the_strip_background_takes_the_amount_and_off_draws_as_before() {
    let _palette = PALETTE.lock().unwrap_or_else(|e| e.into_inner());
    for mode in [ThemeMode::Dark, ThemeMode::Light] {
        let t = if mode == ThemeMode::Light { Tokens::LIGHT } else { Tokens::DARK };
        let default = mix(mode, None);
        let kick = default.state().engine.session().track_by_name("Kick").unwrap().color;
        assert_eq!(default.state().ui.mix_strip_tint, 0.0, "Off by default");
        // Off: the very same shapes as never having touched it.
        let off = mix(mode, Some(0.0));
        assert_eq!(format!("{:?}", default.output().shapes), format!("{:?}", off.output().shapes), "{mode:?}: Off draws as before");
        assert!(mesh_colours(&off).contains(&t.strip_bg), "{mode:?}: untinted strips");
        for (amount, label) in STEPS.into_iter().skip(1) {
            let h = mix(mode, Some(amount));
            let want = look_with(&t, kick, amount, false).unwrap().bg;
            assert!(mesh_colours(&h).contains(&want), "{mode:?} {label}: Kick's strip is {want:?}");
            assert!(!mesh_colours(&off).contains(&want));
        }
    }
}

#[test]
fn the_bar_sets_the_amount_and_it_is_kept_with_the_preferences() {
    let _palette = PALETTE.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = mix(ThemeMode::Dark, None);
    h.get_by_label("Medium").click();
    h.run_steps(2);
    assert_eq!(h.state().ui.mix_strip_tint, 0.65);
    // Kept in the UI preferences (ui.json), not in the session.
    let saved = serde_json::to_value(&h.state().ui).unwrap();
    assert_eq!(saved["mix_strip_tint"], json!(0.65f32));
    let back: UiState = serde_json::from_value(saved).unwrap();
    assert_eq!(back.mix_strip_tint, 0.65);
    let session = serde_json::to_string(h.state().engine.session()).unwrap();
    assert!(!session.contains("mix_strip_tint"));
    // Preferences saved before it: Off.
    let old: UiState = serde_json::from_value(json!({"narrow_mix": true})).unwrap();
    assert_eq!(old.mix_strip_tint, 0.0);
    h.get_by_label("Off").click();
    h.run_steps(2);
    assert_eq!(h.state().ui.mix_strip_tint, 0.0);
}
