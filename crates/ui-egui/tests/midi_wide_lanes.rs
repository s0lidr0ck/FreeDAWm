//! The MIDI Editor's 14-bit and RPN lanes: a clip with a CC1/CC33 pair opens with one 14-bit lane
//! (not two 7-bit ones as its only view), a click there sets both controllers, Alt-click removes
//! them, and a pitch-bend-range lane writes the RPN select, data entry and null.

use egui::{Event, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_engine::cmd::midi_wide::WideLane;
use soundcraft_model::{ChannelFormat, TrackKind};
use soundcraft_time::{GridValue, NoteValue};
use soundcraft_ui_egui::{Services, SoundApp};

fn harness() -> (Harness<'static, SoundApp>, u64) {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let t = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let clip = app.run("midi.new_clip", json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
    app.run("midi.note_add", json!({"clip": clip, "pitch": 60, "start_ticks": 0})).unwrap();
    app.run("midi.ctrl14_draw", json!({"clip": clip, "lane": "cc14:1", "points": [[0, 4000]]})).unwrap();
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

fn lane(h: &Harness<'_, SoundApp>, l: WideLane) -> Rect {
    h.state().midi.drawn_wide.iter().find(|w| w.0 == l).expect("the lane is shown").1
}

/// Where `value` (0..16383) at `tick` sits in the lane.
fn at(h: &Harness<'_, SoundApp>, l: WideLane, tick: i64, value: f32) -> Pos2 {
    let g = h.state().midi.drawn.unwrap();
    let r = lane(h, l);
    pos2(g.x_of(tick), r.max.y - 3.0 - (r.height() - 6.0) * value / 16383.0)
}

fn click_with(h: &mut Harness<'_, SoundApp>, p: Pos2, modifiers: Modifiers) {
    for _ in 0..40 {
        h.step();
    }
    h.event(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    for pressed in [true, false] {
        h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers });
        h.step();
    }
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

fn values(h: &mut Harness<'_, SoundApp>, clip: u64, lane: &str) -> Vec<(i64, i64)> {
    let r = h.state_mut().run("midi.ctrl14s", json!({"clip": clip, "lane": lane})).unwrap();
    r["points"].as_array().unwrap().iter().map(|x| (x["tick"].as_i64().unwrap(), x["value"].as_i64().unwrap())).collect()
}

#[test]
fn a_14_bit_pair_opens_as_one_lane_and_a_click_sets_both_controllers() {
    let (mut h, clip) = harness();
    assert_eq!(h.state().midi.wide_lanes, vec![WideLane::Cc14(1)]);
    let p = at(&h, WideLane::Cc14(1), 1000, 12000.0);
    click_with(&mut h, p, Modifiers::NONE);
    let v = values(&mut h, clip, "cc14:1");
    assert_eq!(v.len(), 2, "{v:?}");
    assert_eq!(v[1].0, 960, "on the grid");
    assert!((11_700..=12_300).contains(&v[1].1), "{v:?}");
    let cc = h.state_mut().run("midi.ctrls", json!({"clip": clip, "lane": "cc33"})).unwrap();
    assert_eq!(cc["events"].as_array().unwrap().len(), 2, "the low controller is written too");
    // Alt-click on the point removes both controllers there.
    let p = at(&h, WideLane::Cc14(1), 960, v[1].1 as f32);
    click_with(&mut h, p, Modifiers::ALT);
    assert_eq!(values(&mut h, clip, "cc14:1"), vec![(0, 4000)]);
}

#[test]
fn a_pitch_bend_range_lane_writes_the_rpn() {
    let (mut h, clip) = harness();
    soundcraft_ui_egui::midi_wide_lanes::toggle(h.state_mut(), WideLane::Rpn(0), true);
    for _ in 0..3 {
        h.step();
    }
    // 12 semitones: 12 × 128.
    let p = at(&h, WideLane::Rpn(0), 0, 1536.0);
    click_with(&mut h, p, Modifiers::NONE);
    let v = values(&mut h, clip, "rpn:0");
    assert_eq!(v.len(), 1, "{v:?}");
    assert!((v[0].1 - 1536).abs() < 200, "{v:?}");
    let sel = h.state_mut().run("midi.ctrls", json!({"clip": clip, "lane": "cc101"})).unwrap();
    let sel: Vec<i64> = sel["events"].as_array().unwrap().iter().map(|e| e["value"].as_i64().unwrap()).collect();
    assert_eq!(sel, vec![0, 127], "selected, then the null RPN");
}
