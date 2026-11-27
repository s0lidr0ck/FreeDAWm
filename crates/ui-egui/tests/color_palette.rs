//! Window › Color Palette with the 64-colour palette: every palette colour keeps a track
//! header's text, a clip's name and a graph line readable in both themes; the window colours
//! the selected tracks from the grid, the originals and custom colours (Shift: the clips), rings
//! the selected track's colour, and keeps the last custom colours with the preferences.

use egui::{Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::TRACK_COLORS;
use soundcraft_model::palette::{self, GRID, MAIN_SHADE, PALETTE_LEN, palette_color};
use soundcraft_ui_egui::palette_window::{MAX_CUSTOM, swatch_label};
use soundcraft_ui_egui::strip_tint::{MIN_CONTRAST, contrast};
use soundcraft_ui_egui::theme::{Tokens, clip_colors, clip_name_ink, rgb, track_backgrounds_with};
use soundcraft_ui_egui::{Services, SoundApp, UiState};

fn palette() -> Vec<[u8; 3]> {
    (0..PALETTE_LEN).map(|i| palette_color(i).unwrap()).collect()
}

/// Custom colours at the edges: white, black, pure primaries, a pale yellow.
const HARD: [[u8; 3]; 7] = [[255, 255, 255], [0, 0, 0], [255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0], [250, 240, 200]];

#[test]
fn every_colour_keeps_the_track_header_text_readable_in_both_themes() {
    for (name, t) in [("dark", Tokens::DARK), ("light", Tokens::LIGHT)] {
        for c in palette().into_iter().chain(HARD) {
            for tint in [0.25, 0.5, 0.75, 1.0] {
                for selected in [false, true] {
                    let (head, _) = track_backgrounds_with(&t, c, selected, tint);
                    for (text, what) in [(t.text, "name"), (t.text_dim, "dim label")] {
                        let k = contrast(text, head);
                        assert!(k >= MIN_CONTRAST, "{name} {c:?} tint {tint} selected={selected}: {what} on {head:?} is {k:.2}:1");
                    }
                }
            }
        }
        // No tint: the header as it always was.
        assert_eq!(track_backgrounds_with(&t, [222, 196, 72], false, 0.0).0, t.panel_bg2);
        assert_eq!(track_backgrounds_with(&t, [222, 196, 72], false, f32::NAN).0, t.panel_bg2);
        // Tinted, it still takes the track's colour: a blue header is bluer than a red one.
        let blue = track_backgrounds_with(&t, GRID[MAIN_SHADE][8], false, 1.0).0;
        let red = track_backgrounds_with(&t, GRID[MAIN_SHADE][0], false, 1.0).0;
        assert!(blue.b() > blue.r() && red.r() > red.b(), "{name}: {blue:?} {red:?}");
    }
}

#[test]
fn clip_names_and_graph_lines_read_on_every_palette_colour() {
    let dark_text = egui::Color32::from_rgb(12, 12, 12);
    for (i, c) in palette().into_iter().enumerate() {
        // The clip's name bar is the same in both themes.
        let bar = clip_colors(c, false).1;
        let ink = clip_name_ink(bar);
        if i < TRACK_COLORS.len() {
            // The originals keep the dark name they always had.
            assert_eq!(ink, dark_text, "original {i}");
            assert!(contrast(ink, bar) >= 3.5, "original {i}");
        } else {
            let k = contrast(ink, bar);
            assert!(k >= MIN_CONTRAST, "{}: clip name {k:.2}:1", palette::palette_name(i).unwrap());
        }
        // A line in the track's colour on the dark graph background (meters, spectrum, curves).
        let k = contrast(rgb(c), Tokens::DARK.graph_bg);
        assert!(k >= 3.0, "{}: on the dark graph {k:.2}:1", palette::palette_name(i).unwrap());
    }
    // Only the Dark row takes light text; a black custom colour too.
    let light_inked: Vec<usize> = (0..PALETTE_LEN).filter(|i| clip_name_ink(clip_colors(palette_color(*i).unwrap(), false).1) != dark_text).collect();
    assert_eq!(light_inked, (palette::grid_index(3, 0)..PALETTE_LEN).collect::<Vec<_>>());
    assert_ne!(clip_name_ink(clip_colors([0, 0, 0], false).1), dark_text);
}

fn harness() -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    let kick = app.engine.session().track_by_name("Kick").unwrap().id;
    app.engine.session_mut().edit.selected_tracks = vec![kick];
    app.engine.session_mut().edit.selected_clips.clear();
    app.run("window.color_palette", json!({})).unwrap();
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

fn kick(h: &Harness<SoundApp>) -> [u8; 3] {
    h.state().engine.session().track_by_name("Kick").unwrap().color
}

#[test]
fn the_window_colours_the_selected_track_and_rings_its_swatch() {
    let mut h = harness();
    assert!(h.state().ui.show_color_palette);
    // Every grid colour and every original is in the window, by name.
    // (Kick's own colour, an original, is marked as the selected track's.)
    let shown = |h: &Harness<SoundApp>, name: &str| [false, true].iter().any(|c| h.query_by_label(&swatch_label(name, *c)).is_some());
    for shade in ["light", "main", "deep", "dark"] {
        for hue in palette::HUE_NAMES {
            assert!(shown(&h, &format!("{hue}, {shade}")), "{hue}, {shade}");
        }
    }
    for i in 1..=16 {
        assert!(shown(&h, &format!("Original {i}")), "Original {i}");
    }
    let before = palette::palette_index_of(kick(&h)).and_then(palette::palette_name).unwrap();
    let _ = h.get_by_label(&swatch_label(&before, true));
    let _ = h.get_by_label("Custom...");
    // A grid swatch colours the selected track; its swatch is ringed and the window says so.
    let undo = h.state().engine.undo_history().len();
    h.get_by_label("Pink, main").click();
    h.run_steps(2);
    assert_eq!(kick(&h), GRID[MAIN_SHADE][11]);
    assert_eq!(h.state().engine.undo_history().len(), undo + 1, "one undo step");
    let _ = h.get_by_label(&swatch_label("Pink, main", true));
    let _ = h.get_by_label("Kick: Pink, main");
    // An original keeps its old index: Original 1 is TRACK_COLORS[0].
    h.get_by_label("Original 1").click();
    h.run_steps(2);
    assert_eq!(kick(&h), TRACK_COLORS[0]);
    let _ = h.get_by_label(&swatch_label("Original 1", true));
    assert!(h.query_by_label(&swatch_label("Pink, main", true)).is_none(), "only the current colour is ringed");
}

#[test]
fn a_custom_colour_applies_and_is_kept_as_a_swatch() {
    let mut h = harness();
    h.get_by_label("Custom...").click();
    h.run_steps(2);
    assert!(h.state().ui.custom_pick.is_some(), "the picker opened");
    // The picker's colour (set here as a drag in the picker would) applied with Apply.
    h.state_mut().ui.custom_pick = Some([10, 120, 90]);
    h.run_steps(1);
    h.state_mut().ui.custom_pick = Some([10, 120, 90]);
    h.get_by_label("Apply").click();
    h.run_steps(2);
    assert_eq!(kick(&h), [10, 120, 90]);
    assert_eq!(h.state().ui.custom_colors, vec![[10, 120, 90]]);
    let _ = h.get_by_label("Kick: custom #0a785a");
    // Kept as a swatch: another colour, then the kept one back from its swatch.
    h.get_by_label("Close").click();
    h.run_steps(2);
    assert!(h.state().ui.custom_pick.is_none());
    h.get_by_label("Teal, main").click();
    h.run_steps(2);
    h.get_by_label("Custom #0a785a").click();
    h.run_steps(2);
    assert_eq!(kick(&h), [10, 120, 90]);
    // At most eight, newest first, with the preferences (ui.json) and not the session.
    for i in 0..10u8 {
        h.state_mut().ui.custom_colors.insert(0, [i, i, 200]);
    }
    h.run_steps(1);
    assert!(h.query_by_label("Custom #0909c8").is_some());
    for current in [false, true] {
        assert!(h.query_by_label(&swatch_label("Custom #0a785a", current)).is_none(), "past the eighth: not shown");
    }
    soundcraft_ui_egui::palette_window::remember_custom(&mut h.state_mut().ui.custom_colors, [1, 2, 3]);
    assert_eq!(h.state().ui.custom_colors.len(), MAX_CUSTOM);
    let saved = serde_json::to_value(&h.state().ui).unwrap();
    assert_eq!(saved["custom_colors"][0], json!([1, 2, 3]));
    let back: UiState = serde_json::from_value(saved).unwrap();
    assert_eq!(back.custom_colors.len(), MAX_CUSTOM);
    assert!(back.custom_pick.is_none());
    assert!(!serde_json::to_string(h.state().engine.session()).unwrap().contains("custom_colors"));
    // Preferences saved before custom colours: none.
    let old: UiState = serde_json::from_value(json!({"narrow_mix": true})).unwrap();
    assert!(old.custom_colors.is_empty());
}

#[test]
fn shift_click_colours_the_selected_clips_not_the_track() {
    let mut h = harness();
    let (track, clip) = {
        let s = h.state().engine.session();
        let t = s.track_by_name("Kick").unwrap();
        (t.id, t.clips().first().unwrap().id)
    };
    h.state_mut().engine.session_mut().edit.selected_clips = vec![clip];
    h.run_steps(1);
    let before = kick(&h);
    h.get_by_label("Violet, deep").click_modifiers(Modifiers::SHIFT);
    h.run_steps(2);
    let s = h.state().engine.session();
    assert_eq!(s.track(track).unwrap().color, before, "the track keeps its colour");
    let c = s.track(track).unwrap().clips().iter().find(|c| c.id == clip).unwrap().color;
    assert_eq!(c, Some(GRID[2][9]));
}
