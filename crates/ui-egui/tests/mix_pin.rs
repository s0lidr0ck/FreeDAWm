//! Track › Pin in the Mix window: pinned strips stay at the left while the others scroll.

use egui::{Shape, vec2};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp};

/// Where the shown text `name` starts on its strip: the rightmost of its shown copies (the
/// tracks list at the window's left shows the names too).
fn text_x(h: &egui_kittest::Harness<'_, SoundApp>, name: &str) -> Option<f32> {
    h.output()
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            // Shown: inside its clip (a scrolled-away strip is still painted, clipped out).
            Shape::Text(t) if t.galley.job.text == name && s.clip_rect.intersects(t.visual_bounding_rect()) => Some(t.visual_bounding_rect().min.x),
            _ => None,
        })
        .reduce(f32::max)
}

#[test]
fn pinned_strips_stay_at_the_left_while_the_others_scroll() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    for i in 0..30 {
        app.run("track.new", json!({"name": format!("Trk{i:02}")})).unwrap();
    }
    let last = app.engine.session().tracks.iter().find(|t| t.name == "Trk29").map(|t| t.id.0).unwrap();
    app.run("window.mix", json!({})).unwrap();
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    for _ in 0..3 {
        h.step();
    }
    assert!(text_x(&h, "Trk29").is_none(), "the last strip is off to the right before it is pinned");
    h.state_mut().run("track.pin", json!({"tracks": [last], "value": true})).unwrap();
    for _ in 0..3 {
        h.step();
    }
    let pinned = text_x(&h, "Trk29").expect("the pinned strip is in view");
    let first = text_x(&h, "Trk00").expect("the first strip follows it");
    assert!(pinned < first, "pinned at the left: {pinned} vs {first}");
    h.state_mut().run("track.pin", json!({"tracks": [last], "value": false})).unwrap();
    for _ in 0..3 {
        h.step();
    }
    assert!(text_x(&h, "Trk29").is_none(), "unpinned, it goes back to its place");
}
