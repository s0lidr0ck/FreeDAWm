//! The MIDI Editor's navigating and editing (docs/superpowers/specs/
//! 2026-11-04-midi-editor-editing-design.md): Select all, Split and Join through `ui.midi_editor`,
//! Esc cancelling a drag, one undo step per gesture, the note bar's typed and relative edits on a
//! mixed selection, bar lines that follow the meter and the tempo, the piano keys, `owns_key` and
//! a focused text field, the zoom factors, the hover details and the ghost notes.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use serde_json::{Value, json};
use soundcraft_midi::Note;
use soundcraft_model::{ChannelFormat, ClipContent, ClipId, TrackKind};
use soundcraft_time::{GridValue, NoteValue, TICKS_PER_QUARTER, TempoMap};
use soundcraft_ui_egui::midi_edit_tools::{self as tools, LineKind};
use soundcraft_ui_egui::{Services, SoundApp, midi_editor, midi_ghosts};

/// A one-bar Keys clip from the session start (grid: eighths, 480 ticks; a quarter is 960) with
/// C3, E3 and G3 an eighth long on beats 1, 2 and 3, open in the MIDI Editor; the main counter
/// in Bars|Beats.
fn harness() -> Harness<'static, SoundApp> {
    harness_with(|_| {})
}

fn harness_with(extra: impl FnOnce(&mut SoundApp)) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let t = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let bar = bar_samples(&app);
    let clip = app.run("midi.new_clip", json!({"track": t.0, "start": 0, "end": bar})).unwrap()["clip"].as_u64().unwrap();
    app.engine.session_mut().edit.main_counter = soundcraft_time::TimeFormat::BarsBeats;
    for (pitch, start) in [(60, 0), (64, 960), (67, 1920)] {
        app.run("midi.note_add", json!({"clip": clip, "pitch": pitch, "start_ticks": start, "length_ticks": 480})).unwrap();
    }
    app.run("edit.select", json!({"clips": [clip]})).unwrap();
    app.engine.session_mut().edit.grid = GridValue::Note { value: NoteValue::Eighth, dotted: false, triplet: false };
    app.ui.show_midi_editor = true;
    app.midi.audition = false;
    extra(&mut app);
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    for _ in 0..3 {
        h.step();
    }
    h
}

/// One 4/4 bar in samples.
fn bar_samples(app: &SoundApp) -> i64 {
    let s = app.engine.session();
    s.tempo.tick_to_samples(4 * TICKS_PER_QUARTER, s.sample_rate)
}

fn clip(h: &Harness<'_, SoundApp>) -> ClipId {
    h.state().midi.clip.unwrap()
}

fn notes(h: &Harness<'_, SoundApp>) -> Vec<Note> {
    let s = h.state().engine.session();
    match &s.find_clip(clip(h)).unwrap().1.content {
        ClipContent::Midi { sequence } => sequence.notes.clone(),
        _ => panic!("not MIDI"),
    }
}

/// (pitch, start, length) of every note.
fn psl(h: &Harness<'_, SoundApp>) -> Vec<(u8, i64, i64)> {
    notes(h).iter().map(|n| (n.pitch, n.start, n.length)).collect()
}

/// `ui.midi_editor` through the control path (`run_sync`, as `engine.execute` runs it).
fn ui_cmd(h: &mut Harness<'_, SoundApp>, p: Value) -> Value {
    let r = h.state_mut().run_sync("ui.midi_editor", p).unwrap();
    h.step();
    r
}

fn undo_len(h: &Harness<'_, SoundApp>) -> usize {
    h.state().engine.undo_history().len()
}

fn at(h: &Harness<'_, SoundApp>, tick: i64, pitch: i32) -> Pos2 {
    let g = h.state().midi.drawn.unwrap();
    pos2(g.x_of(tick), g.y_of(pitch) + g.row_h / 2.0)
}

fn button(h: &mut Harness<'_, SoundApp>, p: Pos2, pressed: bool, modifiers: Modifiers) {
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers });
    h.step();
}

fn key(h: &mut Harness<'_, SoundApp>, key: Key, modifiers: Modifiers) {
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    h.step();
    h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    h.step();
}

fn drag(h: &mut Harness<'_, SoundApp>, a: Pos2, b: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(a));
    h.step();
    button(h, a, true, Modifiers::NONE);
    for k in 1..=6 {
        h.input_mut().events.push(Event::PointerMoved(a + (b - a) * (k as f32 / 6.0)));
        h.step();
    }
    button(h, b, false, Modifiers::NONE);
    h.step();
}

#[test]
fn select_all_split_and_join_through_the_control_protocol_each_one_undo_step() {
    let mut h = harness();
    let r = ui_cmd(&mut h, json!({"select_all": true}));
    assert_eq!(r["selected"], json!([0, 1, 2]));
    // Split C3 in the middle: two halves, selected; one undo step.
    ui_cmd(&mut h, json!({"select": [0]}));
    let steps = undo_len(&h);
    let r = ui_cmd(&mut h, json!({"split": 240}));
    assert_eq!(psl(&h), vec![(60, 0, 240), (60, 240, 240), (64, 960, 480), (67, 1920, 480)]);
    assert_eq!(r["selected"], json!([0, 1]));
    assert_eq!(undo_len(&h), steps + 1);
    // Join them again.
    let r = ui_cmd(&mut h, json!({"join": true}));
    assert_eq!(psl(&h), vec![(60, 0, 480), (64, 960, 480), (67, 1920, 480)]);
    assert_eq!(r["selected"], json!([0]));
    assert_eq!(undo_len(&h), steps + 2);
    // Split at the playhead (the insertion point, inside E3).
    let at = {
        let s = h.state().engine.session();
        s.tempo.tick_to_samples(1200, s.sample_rate)
    };
    h.state_mut().engine.session_mut().edit.selection = soundcraft_time::Range::new(at, at);
    ui_cmd(&mut h, json!({"select": [1]}));
    ui_cmd(&mut h, json!({"split": true}));
    assert_eq!(psl(&h), vec![(60, 0, 480), (64, 960, 240), (64, 1200, 240), (67, 1920, 480)]);
}

#[test]
fn ctrl_a_and_the_select_all_button_select_every_note() {
    let mut h = harness();
    let p = h.state().midi.drawn.unwrap().roll.center();
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    key(&mut h, Key::A, Modifiers::COMMAND);
    assert_eq!(h.state().midi.selected, vec![0, 1, 2]);
    assert!(h.state().engine.session().edit.selected_clips.contains(&clip(&h)), "the Edit window's Select All didn't run");
}

#[test]
fn esc_cancels_a_drag_in_progress_and_restores_the_notes() {
    let mut h = harness();
    let before = psl(&h);
    let steps = undo_len(&h);
    let a = at(&h, 240, 60);
    let b = at(&h, 240 + 1440, 64);
    h.input_mut().events.push(Event::PointerMoved(a));
    h.step();
    button(&mut h, a, true, Modifiers::NONE);
    for k in 1..=4 {
        h.input_mut().events.push(Event::PointerMoved(a + (b - a) * (k as f32 / 4.0)));
        h.step();
    }
    assert!(h.state().midi.drag.is_some(), "dragging");
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(h.state().midi.drag.is_none());
    button(&mut h, b, false, Modifiers::NONE);
    h.step();
    assert_eq!(psl(&h), before, "nothing moved");
    assert_eq!(undo_len(&h), steps, "no undo step");
}

#[test]
fn every_gesture_is_one_undo_step() {
    let mut h = harness();
    // A move.
    let steps = undo_len(&h);
    let (a, b) = (at(&h, 240, 60), at(&h, 240 + 480, 62));
    drag(&mut h, a, b);
    assert_eq!(undo_len(&h), steps + 1);
    // A resize.
    let (a, b) = (at(&h, 960 + 478, 64) - vec2(1.0, 0.0), at(&h, 1920, 64));
    drag(&mut h, a, b);
    assert_eq!(undo_len(&h), steps + 2);
    // A velocity drag across several frames.
    let vel = h.ctx.data(|d| d.get_temp::<Rect>(egui::Id::new("soundcraft-midi-vel"))).unwrap();
    // (E3, the selected note after the resize: its stem.)
    let x = h.state().midi.drawn.unwrap().x_of(960) + 1.0;
    drag(&mut h, pos2(x, vel.bottom() - 4.0), pos2(x, vel.top() + 6.0));
    assert_eq!(undo_len(&h), steps + 3);
    // A note-bar drag (velocity field) on two notes.
    ui_cmd(&mut h, json!({"select": [0, 1]}));
    let rects: Vec<(tools::Field, Rect)> = h.ctx.data(|d| d.get_temp(egui::Id::new("soundcraft-midi-notebar"))).unwrap();
    let r = rects.iter().find(|(f, _)| *f == tools::Field::Velocity).unwrap().1;
    drag(&mut h, r.center(), r.center() + vec2(60.0, 0.0));
    let v: Vec<u8> = notes(&h).iter().map(|n| n.velocity).collect();
    assert!(v[0] > 100 && v[0] == v[1], "{v:?}");
    assert_eq!(undo_len(&h), steps + 4, "the whole drag is one step");
}

#[test]
fn the_note_bar_shows_a_range_and_edits_pitch_and_start_relatively() {
    let mut h = harness();
    let r = ui_cmd(&mut h, json!({"select": [0, 1]}));
    assert_eq!(r["note_bar"]["pitch"], "C3\u{2026}E3");
    assert_eq!(r["note_bar"]["start"], "1.1.1\u{2026}1.2.1");
    assert_eq!(r["note_bar"]["length"], "0.0.2");
    assert_eq!(r["note_bar"]["velocity"], "100");
    assert_eq!(r["note_bar"]["channel"], "1");
    // Pitch typed: the lowest goes to D3, the other keeps its distance.
    let steps = undo_len(&h);
    ui_cmd(&mut h, json!({"set": {"pitch": "D3"}}));
    assert_eq!(psl(&h), vec![(62, 0, 480), (66, 960, 480), (67, 1920, 480)]);
    assert_eq!(undo_len(&h), steps + 1);
    // Start typed as a position: both move by the same amount.
    let r = ui_cmd(&mut h, json!({"set": {"start": "1.1.3"}}));
    assert_eq!(psl(&h), vec![(62, 480, 480), (66, 1440, 480), (67, 1920, 480)]);
    assert_eq!(r["selected"], json!([0, 1]));
    // Length, velocity and channel are absolute.
    ui_cmd(&mut h, json!({"set": {"length": "0.0.1", "velocity": 50, "channel": 3}}));
    let n = notes(&h);
    assert_eq!((n[0].length, n[0].velocity, n[0].channel), (240, 50, 2));
    assert_eq!((n[1].length, n[1].velocity, n[1].channel), (240, 50, 2));
    assert_eq!((n[2].length, n[2].velocity, n[2].channel), (480, 100, 0), "unselected notes stay");
    // Bad text is refused and changes nothing.
    let before = psl(&h);
    assert!(h.state_mut().run_sync("ui.midi_editor", json!({"set": {"pitch": "H9"}})).is_err());
    assert_eq!(psl(&h), before);
}

#[test]
fn typing_into_the_note_bar_moves_the_selection() {
    let mut h = harness();
    ui_cmd(&mut h, json!({"select": [0, 1]}));
    let rects: Vec<(tools::Field, Rect)> = h.ctx.data(|d| d.get_temp(egui::Id::new("soundcraft-midi-notebar"))).unwrap();
    let r = rects.iter().find(|(f, _)| *f == tools::Field::Pitch).unwrap().1;
    h.input_mut().events.push(Event::PointerMoved(r.center()));
    h.step();
    button(&mut h, r.center(), true, Modifiers::NONE);
    button(&mut h, r.center(), false, Modifiers::NONE);
    assert!(h.ctx.egui_wants_keyboard_input(), "the field took the keyboard");
    // While it has the keyboard, the editor takes no keys.
    {
        let roll = h.state().midi.drawn.unwrap().roll;
        h.input_mut().events.push(Event::PointerMoved(roll.center()));
        h.step();
        let ctx = h.ctx.clone();
        for (k, m) in [(Key::A, Modifiers::COMMAND), (Key::Space, Modifiers::NONE), (Key::F, Modifiers::NONE)] {
            assert!(!midi_editor::owns_key(h.state(), &ctx, k, m), "{k:?} taken while a field is focused");
        }
    }
    key(&mut h, Key::A, Modifiers::COMMAND);
    h.input_mut().events.push(Event::Text("E3".into()));
    h.step();
    key(&mut h, Key::Enter, Modifiers::NONE);
    h.step();
    assert_eq!(psl(&h), vec![(64, 0, 480), (68, 960, 480), (67, 1920, 480)]);
}

#[test]
fn owns_key_takes_the_editor_keys_over_the_roll_only() {
    let mut h = harness();
    let roll = h.state().midi.drawn.unwrap().roll;
    h.input_mut().events.push(Event::PointerMoved(roll.center()));
    h.step();
    let ctx = h.ctx.clone();
    assert!(midi_editor::owns_key(h.state(), &ctx, Key::Space, Modifiers::NONE));
    assert!(midi_editor::owns_key(h.state(), &ctx, Key::A, Modifiers::COMMAND));
    assert!(!midi_editor::owns_key(h.state(), &ctx, Key::Escape, Modifiers::NONE), "Esc only while dragging");
    h.input_mut().events.push(Event::PointerMoved(pos2(5.0, 5.0)));
    h.step();
    assert!(!midi_editor::owns_key(h.state(), &ctx, Key::Space, Modifiers::NONE), "not over the roll");
}

#[test]
fn bar_lines_follow_a_three_four_section() {
    // 4/4 for two bars (3840 ticks each), then 3/4 (2880).
    let q = TICKS_PER_QUARTER;
    let mut map = TempoMap::default();
    map.set_meter(8 * q, 3, 4).unwrap();
    let lines = tools::grid_lines(&map, 0, 0, 14 * q, true, 0);
    let bars: Vec<i64> = lines.iter().filter(|(_, k)| *k == LineKind::Bar).map(|(t, _)| *t).collect();
    assert_eq!(bars, vec![0, 4 * q, 8 * q, 11 * q, 14 * q]);
    let beats_in_34: Vec<i64> = lines.iter().filter(|(t, k)| *k == LineKind::Beat && (8 * q..11 * q).contains(t)).map(|(t, _)| *t).collect();
    assert_eq!(beats_in_34, vec![9 * q, 10 * q]);
    // A clip starting on the 3/4 bar's last beat sees the next bars at their own ticks.
    let rel: Vec<i64> = tools::grid_lines(&map, 10 * q, 0, 4 * q, false, 0).iter().map(|(t, _)| *t).collect();
    assert_eq!(rel, vec![q, 4 * q]);
    // Grid lines count from each bar's start and leave out those on beats.
    let grid: Vec<i64> =
        tools::grid_lines(&map, 0, 8 * q, 11 * q - 1, true, q / 2).iter().filter(|(_, k)| *k == LineKind::Grid).map(|(t, _)| *t).collect();
    assert_eq!(grid, vec![8 * q + q / 2, 9 * q + q / 2, 10 * q + q / 2]);
}

#[test]
fn bar_lines_after_a_tempo_change_stay_on_the_bars() {
    let q = TICKS_PER_QUARTER;
    let mut h = harness_with(|app| {
        let s = app.engine.session_mut();
        s.tempo.set_tempo(4 * q, 90.0).unwrap();
        s.tempo.set_meter(8 * q, 3, 4).unwrap();
    });
    // A clip from bar 3 (the 3/4 bar, after the tempo change) to two bars on.
    let t = h.state().engine.session().tracks[0].id;
    let (start, end) = {
        let s = h.state().engine.session();
        (s.tempo.tick_to_samples(8 * q, s.sample_rate), s.tempo.tick_to_samples(14 * q, s.sample_rate))
    };
    let c = h.state_mut().run("midi.new_clip", json!({"track": t.0, "start": start, "end": end})).unwrap()["clip"].as_u64().unwrap();
    h.state_mut().run("edit.select", json!({"clips": [c]})).unwrap();
    h.step();
    h.step();
    let s = h.state().engine.session_arc();
    let base = tools::clip_base(&s, ClipId(c));
    assert_eq!(base, 8 * q);
    let bars: Vec<i64> = tools::grid_lines(&s.tempo, base, 0, 6 * q, true, 0).iter().filter(|(_, k)| *k == LineKind::Bar).map(|(t, _)| *t).collect();
    assert_eq!(bars, vec![0, 3 * q, 6 * q]);
}

#[test]
fn a_piano_key_sounds_while_held_and_stops_on_release() {
    let mut h = harness();
    let g = h.state().midi.drawn.unwrap();
    let p = pos2(g.roll.min.x - 20.0, g.y_of(64) + g.row_h / 2.0);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    button(&mut h, p, true, Modifiers::NONE);
    h.step();
    assert_eq!(h.state().midi.piano_key, Some(64));
    let heard: Vec<u8> = {
        let a = h.state().audition_session.clone().expect("note on: the key plays");
        a.find_clip(clip(&h)).and_then(|(_, c)| c.midi_playing()).map(|q| q.notes.iter().map(|n| n.pitch).collect()).unwrap_or_default()
    };
    assert_eq!(heard, vec![64]);
    assert!(h.state().is_playing());
    // Gliding to the next key plays it instead.
    let q = pos2(p.x, g.y_of(65) + g.row_h / 2.0);
    h.input_mut().events.push(Event::PointerMoved(q));
    h.step();
    h.step();
    assert_eq!(h.state().midi.piano_key, Some(65));
    // Note off on release.
    button(&mut h, q, false, Modifiers::NONE);
    h.step();
    h.step();
    assert_eq!(h.state().midi.piano_key, None);
    assert!(!h.state().is_playing(), "the note stopped");
    assert!(h.state().audition_session.is_none());
}

#[test]
fn space_plays_the_selected_notes_and_stops() {
    let mut h = harness();
    ui_cmd(&mut h, json!({"select": [1, 2]}));
    let roll = h.state().midi.drawn.unwrap().roll;
    h.input_mut().events.push(Event::PointerMoved(roll.center()));
    h.step();
    key(&mut h, Key::Space, Modifiers::NONE);
    assert!(h.state().is_playing());
    // It plays from E3's start (the simulated playhead has moved on since) to G3's end.
    let (from, to, _) = h.state().sim_play().expect("the simulated transport plays");
    let s = h.state().engine.session_arc();
    assert!((s.tempo.tick_to_samples(960, s.sample_rate)..s.tempo.tick_to_samples(2400, s.sample_rate)).contains(&from), "{from}");
    assert_eq!(to, Some(s.tempo.tick_to_samples(2400, s.sample_rate)));
    assert_eq!(h.state().ui.status, "Playing the selected notes");
    key(&mut h, Key::Space, Modifiers::NONE);
    assert!(!h.state().is_playing());
}

#[test]
fn the_zoom_factors_and_fit() {
    let mut h = harness();
    // A 16-bar clip, the Edit window at 0.1 points per tick.
    let t = h.state().engine.session().track_by_name("Keys").unwrap().id;
    let end = 16 * bar_samples(h.state());
    let c = h.state_mut().run("midi.new_clip", json!({"track": t.0, "start": end, "end": end * 2})).unwrap()["clip"].as_u64().unwrap();
    h.state_mut().run("edit.select", json!({"clips": [c]})).unwrap();
    let spp = {
        let s = h.state().engine.session();
        f64::from(s.tempo.tick_to_samples(4 * TICKS_PER_QUARTER, s.sample_rate) as i32) / f64::from(4 * TICKS_PER_QUARTER as i32) / 0.1
    };
    h.state_mut().engine.session_mut().edit.zoom.samples_per_px = spp;
    h.step();
    h.step();
    ui_cmd(&mut h, json!({"zoom": 2}));
    h.step();
    let st = tools::state(h.state());
    assert_eq!(st["zoom_factor"], json!(2.0));
    let g = h.state().midi.drawn.unwrap();
    assert!((g.px_per_tick - 0.2).abs() < 0.002, "{}", g.px_per_tick);
    ui_cmd(&mut h, json!({"fit": true}));
    h.step();
    let g = h.state().midi.drawn.unwrap();
    assert_eq!(g.px_per_tick, g.fit);
}

#[test]
fn hovering_a_note_shows_its_details() {
    let mut h = harness();
    let p = at(&h, 1100, 64);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    h.step();
    assert_eq!(h.state().midi.hover.as_deref(), Some("E3 · 1.2.1 · len 0.0.2 · vel 100 · ch 1"));
    // In another counter format, the position follows it.
    h.state_mut().engine.session_mut().edit.main_counter = soundcraft_time::TimeFormat::Samples;
    h.step();
    h.step();
    let s = h.state().engine.session_arc();
    let want = format!("E3 · {} · len 0.0.2 · vel 100 · ch 1", s.tempo.tick_to_samples(960, s.sample_rate));
    assert_eq!(h.state().midi.hover.as_deref(), Some(want.as_str()));
}

#[test]
fn the_grid_menu_choice_snaps_drags_and_alt_drags_freely() {
    let mut h = harness();
    let r = ui_cmd(&mut h, json!({"grid": "1/4"}));
    assert_eq!(r["grid_ticks"], 960);
    assert_eq!(
        h.state().engine.session().edit.grid,
        GridValue::Note { value: NoteValue::Eighth, dotted: false, triplet: false },
        "the Edit grid stays"
    );
    // A move by about 1.3 quarters lands on one quarter.
    let (a, b) = (at(&h, 100, 60), at(&h, 100 + 1240, 60));
    drag(&mut h, a, b);
    assert_eq!(psl(&h)[0], (60, 960, 480));
    // Alt held during the drag (not at the press): no snap, and no copy.
    let a = at(&h, 1200, 60);
    let b = at(&h, 1200 + 130, 60);
    h.input_mut().events.push(Event::PointerMoved(a));
    h.step();
    button(&mut h, a, true, Modifiers::NONE);
    for k in 1..=6 {
        if k == 3 {
            // Alt goes down once the drag is under way.
            h.event(Event::ModifiersChanged(Modifiers::ALT));
        }
        h.input_mut().events.push(Event::PointerMoved(a + (b - a) * (k as f32 / 6.0)));
        h.step();
    }
    button(&mut h, b, false, Modifiers::ALT);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
    let n = psl(&h);
    assert_eq!(n.len(), 3, "no copy");
    let c3 = n.iter().find(|x| x.0 == 60).unwrap();
    assert!((c3.1 - 1090).abs() <= 4, "free placement: {c3:?}");
}

/// Keys (edited), Bass (a clip over the same bar), Pad (muted) and a second Keys clip later.
fn with_ghosts(app: &mut SoundApp) {
    let bass = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Bass"));
    let pad = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Pad"));
    let bar = bar_samples(app);
    let b = app.run("midi.new_clip", json!({"track": bass.0, "start": 0, "end": bar})).unwrap()["clip"].as_u64().unwrap();
    app.run("midi.note_add", json!({"clip": b, "pitch": 55, "start_ticks": 0, "length_ticks": 960})).unwrap();
    app.run("midi.note_add", json!({"clip": b, "pitch": 57, "start_ticks": 960, "length_ticks": 960})).unwrap();
    let p = app.run("midi.new_clip", json!({"track": pad.0, "start": 0, "end": bar})).unwrap()["clip"].as_u64().unwrap();
    app.run("midi.note_add", json!({"clip": p, "pitch": 72, "start_ticks": 0, "length_ticks": 960})).unwrap();
    app.engine.session_mut().track_mut(pad).unwrap().mixer.mute = true;
    // The edited clip is the Keys clip: select it again.
    let keys = app.engine.session().track_by_name("Keys").unwrap().id;
    let first = app.engine.session().track(keys).unwrap().clips()[0].id;
    app.run("edit.select", json!({"clips": [first.0]})).unwrap();
}

#[test]
fn ghost_notes_show_other_tracks_but_not_muted_ones_and_never_get_selected() {
    let mut h = harness_with(with_ghosts);
    let st = ui_cmd(&mut h, json!({}));
    assert_eq!(st["ghosts"], true);
    let s = h.state().engine.session_arc();
    let ghosts = midi_ghosts::ghost_notes(&s, clip(&h), None);
    let mut pitches: Vec<u8> = ghosts.iter().map(|g| g.note.pitch).collect();
    pitches.sort_unstable();
    assert_eq!(pitches, vec![55, 57], "Bass shows, the muted Pad doesn't");
    assert!(ghosts.iter().all(|g| g.track_name == "Bass"));
    assert_eq!(st["ghost_notes"], 2);
    // Select all takes this clip's notes only.
    let r = ui_cmd(&mut h, json!({"select_all": true}));
    assert_eq!(r["selected"], json!([0, 1, 2]));
    // A rubber band over a ghost selects nothing.
    ui_cmd(&mut h, json!({"select": []}));
    let (a, b) = (at(&h, 1500, 58), at(&h, 1800, 56));
    drag(&mut h, a, b);
    assert!(h.state().midi.selected.is_empty());
    // Toggle off: none.
    let r = ui_cmd(&mut h, json!({"ghosts": false}));
    assert_eq!(r["ghost_notes"], 0);
    // Only chosen tracks.
    ui_cmd(&mut h, json!({"ghosts": true}));
    let keys = h.state().engine.session().track_by_name("Keys").unwrap().id;
    let r = ui_cmd(&mut h, json!({"ghost_tracks": [keys.0]}));
    assert_eq!(r["ghost_notes"], 0);
    let r = ui_cmd(&mut h, json!({"ghost_tracks": "all"}));
    assert_eq!(r["ghost_notes"], 2);
}

#[test]
fn a_ghost_names_itself_on_hover_and_a_double_click_opens_its_clip() {
    let mut h = harness_with(with_ghosts);
    let own = clip(&h);
    let p = at(&h, 1400, 57);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    h.step();
    assert_eq!(h.state().midi.hover.as_deref(), Some("Ghost · Bass · A2 · 1.2.1"));
    // Both clicks in one frame, as `ui.click {count: 2}` sends them.
    for pressed in [true, false, true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.step();
    h.step();
    h.step();
    assert_ne!(clip(&h), own, "the editor shows the Bass clip");
    let s = h.state().engine.session_arc();
    assert_eq!(s.find_clip(clip(&h)).map(|(t, _)| s.track(t).unwrap().name.clone()).as_deref(), Some("Bass"));
}

#[test]
fn another_clip_on_the_same_track_shows_as_ghosts_and_a_muted_clip_does_not() {
    let h = harness();
    let own = clip(&h);
    let mut s = (*h.state().engine.session_arc()).clone();
    let keys = s.track_by_name("Keys").unwrap().id;
    // Two more Keys clips over the same bar (as on another layer), one of them muted.
    let (_, c) = s.find_clip(own).unwrap();
    let mut other = c.clone();
    other.id = s.new_clip_id();
    other.content = ClipContent::Midi {
        sequence: soundcraft_midi::Sequence {
            notes: vec![Note { pitch: 50, velocity: 90, release_velocity: 64, channel: 0, start: 480, length: 480 }],
            ctrls: Vec::new(),
        },
    };
    let mut muted = other.clone();
    muted.id = s.new_clip_id();
    muted.muted = true;
    let t = s.track_mut(keys).unwrap();
    t.playlists[0].clips.push(other);
    t.playlists[0].clips.push(muted);
    let ghosts = midi_ghosts::ghost_notes(&s, own, None);
    assert_eq!(ghosts.iter().map(|g| (g.note.pitch, g.note.start, g.track_name.as_str())).collect::<Vec<_>>(), vec![(50, 480, "Keys")]);
}

#[test]
fn ui_midi_editor_keeps_the_link_and_the_tools_in_one_command() {
    let mut h = harness();
    let r = ui_cmd(&mut h, json!({"linked": false, "select_all": true}));
    assert_eq!(r["linked"], false);
    assert_eq!(r["selected"], json!([0, 1, 2]));
    let r = ui_cmd(&mut h, json!({"linked": true}));
    assert_eq!(r["linked"], true);
}
