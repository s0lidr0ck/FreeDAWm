use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_engine::edit::{RELATIVE_GRID, SHUFFLE_LOCK, SNAP_TO_GRID};
use soundcraft_model::{Clip, ClipId, EditMode, Source, SourceId, TrackId};
use soundcraft_ui_egui::{Services, SoundApp, edit_window};

const SEC: i64 = 48_000;

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
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

fn key(key: Key) -> Vec<Event> {
    chord(key, Modifiers::NONE)
}

fn chord(key: Key, modifiers: Modifiers) -> Vec<Event> {
    [true, false].map(|pressed| key_event(key, pressed, modifiers)).to_vec()
}

fn key_event(key: Key, pressed: bool, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }
}

fn app(mode: &str, tool: &str, spans: &[(i64, i64)]) -> (SoundApp, egui::Context, TrackId, Vec<ClipId>) {
    let (app, ctx, tracks, ids) = tracks(mode, tool, &[spans]);
    (app, ctx, tracks[0], ids)
}

fn tracks(mode: &str, tool: &str, lanes: &[&[(i64, i64)]]) -> (SoundApp, egui::Context, Vec<TrackId>, Vec<ClipId>) {
    let mut e = Engine::default();
    let made = e.execute("track.new", &json!({"count": lanes.len()})).unwrap()["tracks"].clone();
    let tracks: Vec<TrackId> = (0..lanes.len()).map(|i| TrackId(made[i].as_u64().unwrap())).collect();
    let s = e.session_mut();
    s.sources.push(Source {
        id: SourceId(7),
        name: "src".into(),
        path: "src.wav".into(),
        channels: 1,
        frames: 20 * SEC as u64,
        sample_rate: 48_000,
        format: soundcraft_audio_io::FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
    });
    let mut ids = Vec::new();
    for (t, spans) in tracks.iter().zip(lanes) {
        for &(start, len) in *spans {
            let id = s.new_clip_id();
            s.track_mut(*t).and_then(|t| t.playlist_mut()).unwrap().clips.push(Clip::audio(id, "c", SourceId(7), start, start, len));
            ids.push(id);
        }
    }
    e.execute("edit.mode", &json!({"mode": mode})).unwrap();
    e.execute("edit.tool", &json!({"tool": tool})).unwrap();
    e.execute("view.zoom_set", &json!({"samples_per_px": 400.0})).unwrap();
    let mut app = SoundApp::new(e, None, Services::default());
    let ctx = egui::Context::default();
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    (app, ctx, tracks, ids)
}

fn point(app: &SoundApp, t: TrackId, at: i64) -> Pos2 {
    let s = app.engine.session();
    let [x0, y0, x1, y1] = app.edit_layout.timeline;
    let row = app.edit_layout.rows.iter().find(|(id, _)| *id == t.0).unwrap().1;
    let x = edit_window::x_of(s, Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)), at);
    pos2(x, row[1] + s.track(t).unwrap().height.points() * 0.75)
}

fn drag(ctx: &egui::Context, app: &mut SoundApp, t: TrackId, from: i64, to: i64) {
    let (from, to) = (point(app, t, from), point(app, t, to));
    drag_between(ctx, app, from, to);
}

fn drag_between(ctx: &egui::Context, app: &mut SoundApp, from: Pos2, to: Pos2) {
    frame(ctx, app, vec![Event::PointerMoved(from)]);
    frame(ctx, app, vec![button(from, true)]);
    let started = from + vec2(7.0_f32.copysign(to.x - from.x), 0.0);
    for k in 0..=5 {
        frame(ctx, app, vec![Event::PointerMoved(started + (to - started) * (k as f32 / 5.0))]);
    }
    frame(ctx, app, vec![button(to, false)]);
    frame(ctx, app, vec![]);
}

fn click(ctx: &egui::Context, app: &mut SoundApp, at: Pos2) {
    frame(ctx, app, vec![Event::PointerMoved(at)]);
    frame(ctx, app, vec![button(at, true)]);
    frame(ctx, app, vec![button(at, false)]);
}

fn starts(app: &SoundApp, ids: &[ClipId]) -> Vec<i64> {
    ids.iter().map(|id| app.engine.session().find_clip(*id).unwrap().1.start).collect()
}

#[test]
fn shuffle_drags_swap_clips() {
    let (mut app, ctx, t, ids) = app("shuffle", "grabber", &[(0, SEC), (SEC, SEC)]);
    drag(&ctx, &mut app, t, SEC * 3 / 2, SEC / 2 - 2000);
    assert_eq!(starts(&app, &ids), vec![SEC, 0]);
}

#[test]
fn grid_drags_snap_the_dragged_clip_not_the_one_under_the_pointer() {
    let (mut app, ctx, t, ids) = app("grid", "grabber", &[(1000, SEC), (3 * SEC + 777, SEC)]);
    let s = app.engine.session();
    let step = s.edit.grid.step_samples(0, &s.grid_ctx());
    drag(&ctx, &mut app, t, SEC / 2, 3 * SEC + SEC / 2);
    assert_eq!(starts(&app, &ids[..1])[0] % step, 0);
}

#[test]
fn spot_clicks_open_the_spot_dialog() {
    let (mut app, ctx, t, ids) = app("spot", "grabber", &[(SEC, SEC)]);
    let at = point(&app, t, SEC * 3 / 2);
    click(&ctx, &mut app, at);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.dialogs.open_name(), Some("spot"));
    frame(&ctx, &mut app, vec![Event::Text("4.5".into())]);
    frame(&ctx, &mut app, key(Key::Enter));
    assert!(app.dialogs.open.is_none());
    assert_eq!(starts(&app, &ids), vec![SEC * 9 / 2]);
}

#[test]
fn spot_drags_ask_where_before_moving() {
    let (mut app, ctx, t, ids) = app("spot", "grabber", &[(SEC, SEC)]);
    drag(&ctx, &mut app, t, SEC * 3 / 2, SEC * 5 / 2);
    assert_eq!(starts(&app, &ids), vec![SEC]);
    assert_eq!(app.dialogs.open_name(), Some("spot"));
    frame(&ctx, &mut app, key(Key::Enter));
    assert!((starts(&app, &ids)[0] - 2 * SEC).abs() < 4000);
}

#[test]
fn slip_trims_stop_at_the_next_clip() {
    let (mut app, ctx, t, ids) = app("slip", "trim", &[(0, SEC), (2 * SEC, SEC)]);
    drag(&ctx, &mut app, t, SEC * 3 / 4, 4 * SEC);
    let s = app.engine.session();
    assert_eq!(s.find_clip(ids[0]).unwrap().1.end(), 2 * SEC);
    assert_eq!(starts(&app, &ids), vec![0, 2 * SEC]);
}

#[test]
fn f4_toggles_absolute_and_relative_grid_and_comes_back_to_the_last_one() {
    let (mut app, ctx, _, _) = app("slip", "grabber", &[]);
    let mode = |app: &SoundApp| app.engine.session().edit.edit_mode;
    let mut seen = Vec::new();
    for k in [Key::F4, Key::F4, Key::F4, Key::F4, Key::F2, Key::F4, Key::F1, Key::Backtick, Key::Backtick, Key::Backtick] {
        frame(&ctx, &mut app, key(k));
        seen.push(mode(&app));
    }
    use EditMode::*;
    assert_eq!(seen, [Grid, GridRelative, Grid, GridRelative, Slip, GridRelative, Shuffle, Slip, Spot, GridRelative]);
    app.engine.execute("options.keyboard_lock", &json!({})).unwrap();
    frame(&ctx, &mut app, key(Key::F4));
    frame(&ctx, &mut app, chord(Key::Num4, Modifiers::ALT));
    assert_eq!(mode(&app), GridRelative);
}

#[test]
fn snap_to_grid_snaps_the_edit_cursor_while_slip_moves_stay_free() {
    let (mut app, ctx, t, ids) = app("slip", "grabber", &[(1000, SEC)]);
    app.engine.execute("edit.snap_to_grid", &json!({})).unwrap();
    let s = app.engine.session();
    let step = s.edit.grid.step_samples(0, &s.grid_ctx());
    drag(&ctx, &mut app, t, SEC / 2, SEC / 2 + 10_000);
    let moved = starts(&app, &ids)[0];
    assert!(moved > 1000 && moved % step != 0, "{moved}");
    app.engine.execute("edit.tool", &json!({"tool": "selector"})).unwrap();
    let at = point(&app, t, 3 * SEC + 1234);
    click(&ctx, &mut app, at);
    assert_eq!(app.engine.session().edit.selection.start % step, 0);
}

#[test]
fn the_selection_start_end_and_length_can_be_typed() {
    let (mut app, ctx, _, _) = app("slip", "selector", &[]);
    app.engine.execute("view.main_counter", &json!({"format": "bars_beats"})).unwrap();
    let sel = |app: &SoundApp| app.engine.session().edit.selection;
    for (field, text) in [("start", "3|1|000"), ("length", "1|2|000")] {
        app.dialogs.open_selection_field(field, "");
        frame(&ctx, &mut app, vec![]);
        frame(&ctx, &mut app, vec![Event::Text(text.into())]);
        frame(&ctx, &mut app, key(Key::Enter));
        assert!(app.dialogs.open.is_none(), "{field}");
    }
    // Bar 3 at 120 bpm in 4/4 is 4 s in; a bar and two beats is 3 s.
    assert_eq!((sel(&app).start, sel(&app).end), (SEC * 4, SEC * 7));
    app.dialogs.open_selection_field("end", "");
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![Event::Text("6|1|000".into())]);
    frame(&ctx, &mut app, key(Key::Enter));
    assert_eq!((sel(&app).start, sel(&app).end), (SEC * 4, SEC * 10));
}

#[test]
fn f6_with_f7_or_f7_with_f8_chooses_the_smart_tool() {
    let (mut app, ctx, _, _) = app("slip", "grabber", &[]);
    let tool = |app: &SoundApp| app.engine.session().edit.tool;
    let down = |k, pressed| key_event(k, pressed, Modifiers::NONE);
    for (held, pressed) in [(Key::F7, Key::F6), (Key::F6, Key::F7), (Key::F8, Key::F7), (Key::F7, Key::F8)] {
        frame(&ctx, &mut app, key(Key::F5));
        frame(&ctx, &mut app, vec![down(held, true)]);
        frame(&ctx, &mut app, vec![down(pressed, true)]);
        frame(&ctx, &mut app, vec![down(pressed, false), down(held, false)]);
        assert_eq!(format!("{:?}", tool(&app)), "Smart", "{held:?} then {pressed:?}");
    }
    // Alone, each key still chooses its own tool.
    frame(&ctx, &mut app, key(Key::F7));
    assert_eq!(format!("{:?}", tool(&app)), "Selector");
}

#[test]
fn the_suspend_modifier_lets_a_selection_go_off_the_grid() {
    let (mut app, ctx, t, _) = app("grid", "selector", &[]);
    let s = app.engine.session();
    let step = s.edit.grid.step_samples(0, &s.grid_ctx());
    let (from, to) = (point(&app, t, step), point(&app, t, 3 * step + step / 3));
    // Cmd (Ctrl off macOS) held through the drag.
    let held = |ctx: &egui::Context, app: &mut SoundApp, e: Event| {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))),
            events: vec![Event::ModifiersChanged(Modifiers::COMMAND), e],
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        })
        .textures_delta
        .clear();
    };
    held(&ctx, &mut app, Event::PointerMoved(from));
    held(&ctx, &mut app, button(from, true));
    for k in 1..=5 {
        held(&ctx, &mut app, Event::PointerMoved(from + (to - from) * (k as f32 / 5.0)));
    }
    let end = app.engine.session().edit.selection.end;
    held(&ctx, &mut app, button(to, false));
    frame(&ctx, &mut app, vec![Event::ModifiersChanged(Modifiers::NONE)]);
    assert_ne!(end % step, 0, "held: off the grid ({end})");
    // Without it the end snaps to the grid.
    drag_between(&ctx, &mut app, from, to);
    assert_eq!(app.engine.session().edit.selection.end % step, 0);
}

#[test]
fn mode_keys_add_snap_to_grid_and_cycle_past_a_locked_shuffle() {
    let (mut app, ctx, _, _) = app("slip", "grabber", &[]);
    let state = |app: &SoundApp| (app.engine.session().edit.edit_mode, app.engine.session().edit.flag(SNAP_TO_GRID));
    frame(&ctx, &mut app, chord(Key::F4, Modifiers::SHIFT));
    assert_eq!(state(&app), (EditMode::Slip, true));
    let f4 = |pressed| key_event(Key::F4, pressed, Modifiers::NONE);
    frame(&ctx, &mut app, vec![f4(true)]);
    assert_eq!(state(&app), (EditMode::Grid, false));
    frame(&ctx, &mut app, key(Key::F1));
    frame(&ctx, &mut app, vec![f4(false)]);
    assert_eq!(state(&app), (EditMode::Shuffle, true));
    frame(&ctx, &mut app, chord(Key::Num3, Modifiers::ALT));
    assert_eq!(state(&app), (EditMode::Spot, false));
    app.engine.execute("edit.shuffle_lock", &json!({})).unwrap();
    let mut seen = Vec::new();
    for _ in 0..4 {
        frame(&ctx, &mut app, key(Key::Backtick));
        seen.push(state(&app).0);
    }
    assert_eq!(seen, [EditMode::Grid, EditMode::Slip, EditMode::Spot, EditMode::Grid]);
    frame(&ctx, &mut app, key(Key::F1));
    assert_eq!(state(&app).0, EditMode::Grid);
}

#[test]
fn toolbar_clicks_add_snap_to_grid_lock_shuffle_and_switch_grid_modes_despite_the_keyboard_lock() {
    let mut e = Engine::default();
    e.execute("edit.mode", &json!({"mode": "slip"})).unwrap();
    let app = SoundApp::new(e, None, Services::default());
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    let click = |h: &mut egui_kittest::Harness<'_, SoundApp>, label: &str, modifiers: Modifiers| {
        h.get_by_label(label).click_modifiers(modifiers);
        h.step();
    };
    let state = |h: &egui_kittest::Harness<'_, SoundApp>| {
        let ed = &h.state().engine.session().edit;
        (ed.edit_mode, ed.flag(SNAP_TO_GRID), ed.flag(SHUFFLE_LOCK))
    };
    click(&mut h, "GRID", Modifiers::SHIFT);
    assert_eq!(state(&h), (EditMode::Slip, true, false));
    click(&mut h, "SPOT", Modifiers::NONE);
    assert_eq!(state(&h), (EditMode::Spot, false, false));
    click(&mut h, "SHUFFLE", Modifiers::COMMAND);
    click(&mut h, "SHUFFLE", Modifiers::NONE);
    assert_eq!(state(&h), (EditMode::Spot, false, true));
    assert!(h.state().ui.status.contains("locked"), "{}", h.state().ui.status);
    click(&mut h, "SLIP", Modifiers::SHIFT);
    assert_eq!(state(&h), (EditMode::Slip, true, true));
    h.state_mut().engine.execute("options.keyboard_lock", &json!({})).unwrap();
    for (label, want) in [("GRID", EditMode::Grid), ("GRID", EditMode::GridRelative), ("SLIP", EditMode::Slip), ("GRID", EditMode::GridRelative)] {
        click(&mut h, label, Modifiers::NONE);
        assert_eq!(state(&h).0, want, "{label}");
    }
}

#[test]
fn options_overlap_layer_keeps_the_clip_a_drag_lands_on_and_the_top_one_takes_clicks() {
    let (mut app, ctx, t, ids) = app("slip", "grabber", &[(0, 4 * SEC), (6 * SEC, SEC)]);
    let path = "Options > Overlap > Layer";
    let id = soundcraft_ui_egui::menus::command_for_path(path, &soundcraft_ui_egui::menus::ui_aliases()).unwrap();
    soundcraft_ui_egui::menus::invoke_menu(&mut app, &id, path);
    assert!(soundcraft_ui_egui::menus::item_state(&app, path, Some(&id)).1, "Layer is checked");
    assert!(!soundcraft_ui_egui::menus::item_state(&app, "Options > Overlap > Overwrite", Some(&id)).1);
    // The second clip dragged onto the middle of the first: both stay whole.
    drag(&ctx, &mut app, t, 6 * SEC + SEC / 2, 2 * SEC);
    let s = app.engine.session();
    let (a, b) = (s.find_clip(ids[0]).unwrap().1.clone(), s.find_clip(ids[1]).unwrap().1.clone());
    assert_eq!((a.start, a.length), (0, 4 * SEC), "the clip underneath is not cut");
    assert_eq!(b.layer, a.layer + 1, "the dropped clip is on top");
    assert!((b.start - (1.5 * SEC as f64) as i64).abs() < SEC / 10, "{}", b.start);
    // One undo puts the drag back.
    let moved = starts(&app, &ids);
    app.engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(starts(&app, &ids), vec![0, 6 * SEC]);
    app.engine.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(starts(&app, &ids), moved);
    // A click where they overlap picks the top clip.
    frame(&ctx, &mut app, vec![]);
    let at = point(&app, t, 2 * SEC);
    click(&ctx, &mut app, at);
    assert_eq!(app.engine.session().edit.selected_clips, vec![ids[1]]);
}

#[test]
fn a_grabber_drag_on_an_edit_grouped_track_moves_the_group_and_trims_follow() {
    let (mut app, ctx, t, ids) = tracks("slip", "grabber", &[&[(SEC, 2 * SEC)], &[(SEC, 2 * SEC)], &[(SEC, 2 * SEC)]]);
    app.engine.execute("track.group", &json!({"tracks": [t[0].0, t[1].0], "name": "Drums"})).unwrap();
    drag(&ctx, &mut app, t[0], 2 * SEC, 3 * SEC);
    let moved = starts(&app, &ids);
    assert!((moved[0] - 2 * SEC).abs() < SEC / 10, "{moved:?}");
    assert_eq!(moved[1], moved[0], "the grouped track's clip moves as far");
    assert_eq!(moved[2], SEC, "the track outside the group stays");
    // One undo step for both.
    app.engine.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(starts(&app, &ids), vec![SEC, SEC, SEC]);
    // The Trim tool on one grouped clip's end trims the other's too.
    app.engine.execute("edit.tool", &json!({"tool": "trim"})).unwrap();
    frame(&ctx, &mut app, vec![]);
    drag(&ctx, &mut app, t[1], 3 * SEC - 200, 2 * SEC);
    let len = |i: usize| app.engine.session().find_clip(ids[i]).unwrap().1.length;
    assert!(len(1) < 2 * SEC - SEC / 2, "{}", len(1));
    assert_eq!(len(0), len(1));
    assert_eq!(len(2), 2 * SEC);
}

#[test]
fn dragging_clips_on_two_tracks_moves_both_down_a_track() {
    let (mut app, ctx, t, ids) = tracks("slip", "grabber", &[&[(0, SEC)], &[(2 * SEC, SEC)], &[]]);
    app.engine.execute("edit.select", &json!({"clips": [ids[0].0, ids[1].0]})).unwrap();
    let (from, to) = (point(&app, t[0], SEC / 2), point(&app, t[1], SEC / 2));
    drag_between(&ctx, &mut app, from, to);
    let s = app.engine.session();
    assert_eq!(s.find_clip(ids[0]).map(|(t, _)| t), Some(t[1]));
    assert_eq!(s.find_clip(ids[1]).map(|(t, _)| t), Some(t[2]));
}

#[test]
fn shuffle_drags_of_clips_on_two_tracks_land_on_the_grabbed_tracks_boundaries() {
    let lanes: [&[(i64, i64)]; 2] = [&[(4 * SEC, SEC), (5 * SEC, 3 * SEC), (17 * SEC / 2, SEC / 2)], &[(0, SEC), (SEC, 6 * SEC)]];
    let (mut app, ctx, t, ids) = tracks("shuffle", "grabber", &lanes);
    app.engine.execute("edit.select", &json!({"clips": [ids[0].0, ids[3].0]})).unwrap();
    let before = starts(&app, &ids);
    drag(&ctx, &mut app, t[0], 9 * SEC / 2, 9 * SEC / 2 + 3 * SEC / 10);
    assert_eq!(starts(&app, &ids), before);
}

#[test]
fn shuffle_drags_leave_locked_clips_in_the_selection_out_of_it() {
    let (mut app, ctx, t, ids) = app("shuffle", "grabber", &[(0, SEC), (3 * SEC, SEC)]);
    app.engine.session_mut().find_clip_mut(ids[0]).unwrap().time_locked = true;
    app.engine.execute("edit.select", &json!({"clips": [ids[0].0, ids[1].0]})).unwrap();
    drag(&ctx, &mut app, t, 7 * SEC / 2, 2 * SEC);
    assert_eq!(starts(&app, &ids), vec![0, SEC]);
}

#[test]
fn f4_pressed_before_f1_leaves_the_kind_of_grid_alone() {
    let (mut app, ctx, _, _) = app("grid", "grabber", &[]);
    let f4 = |pressed| key_event(Key::F4, pressed, Modifiers::NONE);
    frame(&ctx, &mut app, vec![f4(true)]);
    frame(&ctx, &mut app, key(Key::F1));
    frame(&ctx, &mut app, vec![f4(false)]);
    let ed = &app.engine.session().edit;
    assert_eq!((ed.edit_mode, ed.flag(SNAP_TO_GRID), ed.flag(RELATIVE_GRID)), (EditMode::Shuffle, true, false));
}

/// The Scissors tool separates the clip under a click there; the Mute tool mutes it and a
/// second click unmutes it. Clicks on empty space do nothing.
#[test]
fn scissors_and_mute_tools_act_on_the_clicked_clip() {
    let (mut app, ctx, t, _) = app("slip", "scissors", &[(0, 4 * SEC)]);
    let at = point(&app, t, 2 * SEC);
    click(&ctx, &mut app, at);
    let clips = app.engine.session().track(t).unwrap().clips().to_vec();
    assert_eq!(clips.len(), 2, "one clip split in two");
    let cut = clips[1].start;
    assert!((cut - 2 * SEC).abs() < 1000, "split where clicked: {cut}");
    let empty = point(&app, t, 6 * SEC);
    click(&ctx, &mut app, empty);
    assert_eq!(app.engine.session().track(t).unwrap().clips().len(), 2, "a click on no clip cuts nothing");

    app.run("edit.tool", json!({"tool": "mute"})).unwrap();
    let left = point(&app, t, SEC / 2);
    click(&ctx, &mut app, left);
    let muted: Vec<bool> = app.engine.session().track(t).unwrap().clips().iter().map(|c| c.muted).collect();
    assert_eq!(muted, vec![true, false], "only the clicked clip mutes");
    click(&ctx, &mut app, left);
    assert!(app.engine.session().track(t).unwrap().clips().iter().all(|c| !c.muted), "a second click unmutes");
}

/// The Smart Tool's lower corner where two clips meet draws a crossfade centred on the join,
/// as wide as the drag either way (one undo step).
#[test]
fn smart_tool_drag_at_a_join_draws_a_crossfade() {
    let (mut app, ctx, t, ids) = app("slip", "smart", &[(0, 2 * SEC), (2 * SEC, 2 * SEC)]);
    let low = |app: &SoundApp, at: i64| {
        let p = point(app, t, at);
        let row = app.edit_layout.rows.iter().find(|(id, _)| *id == t.0).unwrap().1;
        pos2(p.x, row[1] + app.engine.session().track(t).unwrap().height.points() * 0.95)
    };
    let (from, to) = (low(&app, 2 * SEC), low(&app, 2 * SEC + SEC / 4));
    drag_between(&ctx, &mut app, from, to);
    let s = app.engine.session();
    let (a, b) = (s.find_clip(ids[0]).unwrap().1, s.find_clip(ids[1]).unwrap().1);
    assert!(a.fade_out.len > SEC / 3 && b.fade_in.len > SEC / 3, "a crossfade about half a second wide: {} {}", a.fade_out.len, b.fade_in.len);
    assert!(a.end() > 2 * SEC && b.start < 2 * SEC, "the clips overlap across the join");
    assert_eq!(app.engine.undo_label(), Some("Create..."));
}

/// Relative Grid: a trim moves the clip's edge by whole grid steps and keeps its offset from
/// the grid (absolute Grid puts it on a line).
#[test]
fn relative_grid_trims_move_by_whole_steps() {
    // 120 bpm at 48 kHz: a 16th (the default grid) is 6,000 samples.
    let (mut app, ctx, t, ids) = app("relative_grid", "trim", &[(1000, 4 * SEC)]);
    let end = 1000 + 4 * SEC;
    drag(&ctx, &mut app, t, end - 100, end + 13_000);
    let c = app.engine.session().find_clip(ids[0]).unwrap().1.clone();
    assert_eq!((c.end() - end) % 6000, 0, "whole steps: {}", c.end() - end);
    assert_eq!(c.end() - end, 12_000, "two steps");
    let (mut app, ctx, t, ids) = app_abs();
    drag(&ctx, &mut app, t, end - 100, end + 13_000);
    assert_eq!(app.engine.session().find_clip(ids[0]).unwrap().1.end() % 6000, 0, "absolute: on a line");
}

fn app_abs() -> (SoundApp, egui::Context, TrackId, Vec<ClipId>) {
    app("grid", "trim", &[(1000, 4 * SEC)])
}

/// Separation grab: a Grabber drag inside a range selection cuts the range out of the clip
/// and moves only that piece.
#[test]
fn a_grabber_drag_inside_a_range_moves_the_range_alone() {
    let (mut app, ctx, t, _) = app("slip", "grabber", &[(0, 6 * SEC)]);
    app.run("edit.select", json!({"tracks": [t.0], "start": SEC, "end": 2 * SEC})).unwrap();
    frame(&ctx, &mut app, vec![]);
    drag(&ctx, &mut app, t, SEC * 3 / 2, SEC * 7 / 2);
    let spans: Vec<(i64, i64)> = app.engine.session().track(t).unwrap().clips().iter().map(|c| (c.start, c.end())).collect();
    let moved = spans.iter().find(|(a, b)| b - a == SEC && *a > 2 * SEC).copied();
    let (a, _) = moved.unwrap_or_else(|| panic!("the range's piece moved: {spans:?}"));
    assert!((a - 3 * SEC).abs() < SEC / 20, "by the drag: {spans:?}");
    assert!(spans.iter().any(|&(a, b)| a == 0 && b == SEC), "what was before the range stays: {spans:?}");
}
