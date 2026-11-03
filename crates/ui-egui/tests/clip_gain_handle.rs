//! The clip gain handle at the top centre of a selected audio clip in the waveform and
//! waveform + pitch views: dragging it changes `gain_db` by the clip fader's mapping and leaves
//! the clip selected; unselected clips have no handle (automation editing spec, §6).

use egui::{Event, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, edit_window, widgets};

fn frame(ctx: &egui::Context, app: &mut SoundApp, events: Vec<Event>) {
    let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))), events, ..Default::default() };
    ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    })
    .textures_delta
    .clear();
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Default::default() }
}

fn handle_of(app: &SoundApp, track: &str) -> Option<Rect> {
    let s = app.engine.session();
    let t = s.track_by_name(track).unwrap();
    let clip = &t.clips()[0];
    let [x0, y0, x1, y1] = app.edit_layout.timeline;
    let tl = Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
    let row = app.edit_layout.rows.iter().find(|(id, _)| *id == t.id.0).unwrap().1;
    let lane = Rect::from_min_max(pos2(tl.min.x, row[1]), pos2(tl.max.x, row[1] + t.height.points()));
    edit_window::clip_gain_handle(s, tl, lane, t, clip)
}

#[test]
fn the_handle_shows_on_selected_clips_only_and_drags_gain_by_the_fader_mapping() {
    for view in ["waveform", "pitch"] {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.engine.execute("track.view", &json!({"track": "Kick", "view": view})).unwrap();
        let ctx = egui::Context::default();
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, vec![]);
        assert!(handle_of(&app, "Kick").is_none(), "{view}: no handle on an unselected clip");
        let clip = app.engine.session().track_by_name("Kick").unwrap().clips()[0].clone();
        app.engine.execute("edit.select", &json!({"clips": [clip.id.0]})).unwrap();
        frame(&ctx, &mut app, vec![]);
        let handle = handle_of(&app, "Kick").unwrap_or_else(|| panic!("{view}: a selected audio clip has a handle"));
        assert_eq!(handle.size(), vec2(10.0, 10.0));
        let at = handle.center();
        let gain = |app: &SoundApp| app.engine.session().find_clip(clip.id).unwrap().1.gain_db;
        let up = |dy: f32| widgets::clip_gain_from_pos(widgets::clip_gain_to_pos(0.0) + dy / edit_window::CLIP_FADER_TRAVEL);

        frame(&ctx, &mut app, vec![Event::PointerMoved(at)]);
        frame(&ctx, &mut app, vec![button(at, true)]);
        for dy in [3.0, 6.0, 10.0, 40.0] {
            frame(&ctx, &mut app, vec![Event::PointerMoved(at - vec2(0.0, dy))]);
        }
        assert!((gain(&app) - up(40.0)).abs() < 0.01, "{view}: {} vs {}", gain(&app), up(40.0));
        frame(&ctx, &mut app, vec![button(at - vec2(0.0, 40.0), false)]);
        frame(&ctx, &mut app, vec![]);
        assert!((gain(&app) - up(40.0)).abs() < 0.01, "{view}: the release keeps the gain");
        assert!(app.engine.session().edit.selected_clips.contains(&clip.id), "{view}: the clip stays selected");
        assert!(handle_of(&app, "Kick").is_some(), "{view}: the handle stays");
        app.engine.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(gain(&app), 0.0, "{view}: one undo step");
        // Other views have no handle.
        app.engine.execute("track.view", &json!({"track": "Kick", "view": "blocks"})).unwrap();
        frame(&ctx, &mut app, vec![]);
        assert!(handle_of(&app, "Kick").is_none(), "{view}: blocks view");
    }
}
