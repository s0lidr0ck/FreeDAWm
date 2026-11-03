//! The MIDI Editor's controller lanes: a clip with CC data opens with its lanes shown; a click
//! adds a point, a drag moves points, Alt-click deletes, Delete removes the selected points (and
//! leaves the selected notes), the Pencil draws a line, and the sustain lane is on/off.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_midi::CtrlKind;
use soundcraft_model::{ChannelFormat, TrackKind};
use soundcraft_time::{GridValue, NoteValue};
use soundcraft_ui_egui::midi_lanes::{self, LaneGeometry};
use soundcraft_ui_egui::{Services, SoundApp};

/// A one-bar Keys clip (grid: quarters, 960 ticks) with two notes and mod wheel at 0 and 64 on
/// beats 1 and 3, open in the MIDI Editor.
fn harness() -> (Harness<'static, SoundApp>, u64) {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let t = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let clip = app.run("midi.new_clip", json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
    for (pitch, start) in [(60, 0), (64, 960)] {
        app.run("midi.note_add", json!({"clip": clip, "pitch": pitch, "start_ticks": start})).unwrap();
    }
    for (tick, value) in [(0, 0), (1920, 64)] {
        app.run("midi.ctrl_add", json!({"clip": clip, "lane": "cc1", "tick": tick, "value": value})).unwrap();
    }
    app.run("edit.select", json!({"clips": [clip]})).unwrap();
    app.engine.session_mut().edit.grid = GridValue::Note { value: NoteValue::Quarter, dotted: false, triplet: false };
    app.ui.show_midi_editor = true;
    app.midi.audition = false;
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
    (h, clip)
}

fn lane_of(h: &Harness<'_, SoundApp>, kind: CtrlKind) -> LaneGeometry {
    *h.state().midi.drawn_lanes.iter().find(|l| l.kind == kind).expect("the lane is shown")
}

/// Where `value` at `tick` sits in the lane.
fn at(h: &Harness<'_, SoundApp>, kind: CtrlKind, tick: i64, value: i32) -> Pos2 {
    let g = h.state().midi.drawn.unwrap();
    pos2(g.x_of(tick), lane_of(h, kind).y_of(value))
}

fn events(h: &mut Harness<'_, SoundApp>, clip: u64, lane: &str) -> Vec<(i64, i64)> {
    let r = h.state_mut().run("midi.ctrls", json!({"clip": clip, "lane": lane})).unwrap();
    r["events"].as_array().unwrap().iter().map(|x| (x["tick"].as_i64().unwrap(), x["value"].as_i64().unwrap())).collect()
}

fn button(h: &mut Harness<'_, SoundApp>, p: Pos2, pressed: bool, modifiers: Modifiers) {
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers });
    h.step();
}

fn click_with(h: &mut Harness<'_, SoundApp>, p: Pos2, modifiers: Modifiers) {
    for _ in 0..40 {
        h.step();
    }
    h.event(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    button(h, p, true, modifiers);
    button(h, p, false, modifiers);
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

fn drag(h: &mut Harness<'_, SoundApp>, a: Pos2, b: Pos2) {
    h.input_mut().events.push(Event::PointerMoved(a));
    h.step();
    button(h, a, true, Modifiers::NONE);
    for k in 1..=8 {
        h.input_mut().events.push(Event::PointerMoved(a + (b - a) * (k as f32 / 8.0)));
        h.step();
    }
    button(h, b, false, Modifiers::NONE);
    h.step();
}

#[test]
fn a_clip_with_mod_wheel_data_opens_with_its_lane() {
    let (h, _) = harness();
    let lanes = &h.state().midi.lanes;
    assert_eq!(lanes, &vec![CtrlKind::Cc(1)]);
    // Under the velocity lane.
    let vel = h.ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("soundcraft-midi-vel"))).unwrap();
    assert!(lane_of(&h, CtrlKind::Cc(1)).rect.min.y > vel.max.y);
}

#[test]
fn a_click_adds_a_point_on_the_grid_and_a_drag_moves_it() {
    let (mut h, clip) = harness();
    let p = at(&h, CtrlKind::Cc(1), 1000, 100);
    click_with(&mut h, p, Modifiers::NONE);
    let e = events(&mut h, clip, "cc1");
    assert_eq!(e.len(), 3);
    let (tick, v) = e[1];
    assert_eq!(tick, 960, "on the grid");
    assert!((97..=103).contains(&v), "{e:?}");
    // Drag it down to about 40: one undo step.
    let a = at(&h, CtrlKind::Cc(1), 960, v as i32);
    let b = at(&h, CtrlKind::Cc(1), 960, 40);
    drag(&mut h, a, b);
    let e = events(&mut h, clip, "cc1");
    assert_eq!(e[1].0, 960);
    assert!((36..=44).contains(&e[1].1), "{e:?}");
    assert_eq!(h.state().engine.undo_label(), Some("Move Controller Events"));
}

#[test]
fn alt_click_deletes_and_delete_removes_selected_points_not_notes() {
    let (mut h, clip) = harness();
    let p = at(&h, CtrlKind::Cc(1), 1920, 64);
    click_with(&mut h, p, Modifiers::ALT);
    assert_eq!(events(&mut h, clip, "cc1"), vec![(0, 0)]);
    // Select the remaining point and a note; Delete over the lane removes only the point.
    let p = at(&h, CtrlKind::Cc(1), 0, 0) + vec2(1.0, -1.0);
    click_with(&mut h, p, Modifiers::NONE);
    h.state_mut().midi.selected = vec![0];
    h.input_mut().events.push(Event::Key { key: Key::Delete, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
    h.step();
    assert_eq!(events(&mut h, clip, "cc1"), vec![]);
    let n = h.state_mut().run("midi.notes", json!({"clip": clip})).unwrap();
    assert_eq!(n["notes"].as_array().map(Vec::len), Some(2));
}

#[test]
fn the_pencil_draws_a_line_and_sustain_is_on_or_off() {
    let (mut h, clip) = harness();
    h.state_mut().engine.execute("edit.tool", &json!({"tool": "pencil"})).unwrap();
    midi_lanes::toggle(h.state_mut(), CtrlKind::Cc(64), true);
    h.step();
    h.step();
    // A ramp across the mod wheel lane, beat 2 to beat 4.
    let a = at(&h, CtrlKind::Cc(1), 960, 10);
    let b = at(&h, CtrlKind::Cc(1), 2880, 120);
    drag(&mut h, a, b);
    let e = events(&mut h, clip, "cc1");
    let drawn: Vec<_> = e.iter().filter(|(t, _)| (960..=2880).contains(t)).collect();
    assert!(drawn.len() >= 4, "{e:?}");
    assert!(drawn.windows(2).all(|w| w[1].1 >= w[0].1), "rising: {e:?}");
    // The point at 1920 under the line was replaced; the one at 0 stays.
    assert_eq!(e[0], (0, 0));
    assert!(!e.contains(&(1920, 64)), "{e:?}");
    // A Pencil click in the sustain lane's upper half is on, lower half off.
    let p = at(&h, CtrlKind::Cc(64), 0, 90);
    click_with(&mut h, p, Modifiers::NONE);
    let p = at(&h, CtrlKind::Cc(64), 1920, 30);
    click_with(&mut h, p, Modifiers::NONE);
    assert_eq!(events(&mut h, clip, "cc64"), vec![(0, 127), (1920, 0)]);
}

#[test]
fn right_click_on_a_point_selects_it_for_its_menu() {
    let (mut h, _) = harness();
    let p = at(&h, CtrlKind::Cc(1), 1920, 64);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.step();
    assert_eq!(h.state().midi.ctrl_menu, Some(1));
    assert_eq!(h.state().midi.ctrl_sel, Some((CtrlKind::Cc(1), vec![1])));
}

#[test]
fn lane_helpers() {
    assert_eq!(midi_lanes::lane_label(CtrlKind::Cc(64)), "Sustain (CC64)");
    assert_eq!(midi_lanes::lane_label(CtrlKind::Cc(74)), "CC74");
    assert_eq!(midi_lanes::lane_short(CtrlKind::Cc(1)), ("Mod".to_string(), "CC1".to_string()));
    assert_eq!(midi_lanes::lane_short(CtrlKind::Cc(74)), ("CC74".to_string(), String::new()));
    assert!(midi_lanes::is_switch(CtrlKind::Cc(64)) && !midi_lanes::is_switch(CtrlKind::Cc(1)));
    let seq = soundcraft_midi::Sequence {
        notes: vec![],
        ctrls: [CtrlKind::PitchBend, CtrlKind::Cc(11), CtrlKind::Program, CtrlKind::Cc(1), CtrlKind::Cc(11)]
            .iter()
            .map(|k| soundcraft_midi::CtrlEvent { tick: 0, channel: 0, kind: *k, value: 0 })
            .collect(),
    };
    assert_eq!(midi_lanes::lanes_with_data(&seq), vec![CtrlKind::Cc(1), CtrlKind::Cc(11), CtrlKind::PitchBend]);
    assert_eq!(midi_lanes::lane_height(2, 400.0), midi_lanes::LANE_H);
    assert_eq!(midi_lanes::lane_height(4, 40.0), 18.0);
    let lg = LaneGeometry { kind: CtrlKind::PitchBend, rect: egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 56.0)) };
    assert_eq!(lg.value_at(lg.y_of(0)), 0);
    assert_eq!(lg.value_at(-100.0), 8191);
}
