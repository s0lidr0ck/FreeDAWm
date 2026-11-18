//! Parity section 7, the MIDI Event List: every kind of event in time order, filtered by kind
//! and channel, an Insert row, and typed values.

use egui::vec2;
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use soundcraft_model::{ChannelFormat, TrackKind};
use soundcraft_ui_egui::midi_event_list::{EventListState, Row, kind_text, rows};
use soundcraft_ui_egui::{Services, SoundApp};

#[test]
fn rows_hold_every_kind_in_time_order_and_filter() {
    let n = |start, channel| Note { probability: 100, pitch: 60, velocity: 100, muted: false, release_velocity: 64, channel, start, length: 10 };
    let c = |tick, kind, channel| CtrlEvent { tick, channel, kind, value: 5, shape: Default::default() };
    let seq = Sequence {
        notes: vec![n(-10, 0), n(0, 0), n(480, 1)],
        ctrls: vec![c(0, CtrlKind::Program, 0), c(100, CtrlKind::Cc(1), 0), c(200, CtrlKind::PitchBend, 1), c(300, CtrlKind::PolyPressure(60), 0)],
        sysex: Vec::new(),
    };
    let all = rows(&seq, &EventListState::default());
    assert_eq!(
        all.iter().map(|x| x.1).collect::<Vec<_>>(),
        vec![Row::Note(1), Row::Ctrl(0), Row::Ctrl(1), Row::Ctrl(2), Row::Ctrl(3), Row::Note(2)],
        "hidden events left out, notes first at one tick"
    );
    let only_ch2 = rows(&seq, &EventListState { channel: Some(1), ..EventListState::default() });
    assert_eq!(only_ch2.iter().map(|x| x.1).collect::<Vec<_>>(), vec![Row::Ctrl(2), Row::Note(2)]);
    let no_notes = rows(&seq, &EventListState { notes: false, bend: false, ..EventListState::default() });
    assert_eq!(no_notes.iter().map(|x| x.1).collect::<Vec<_>>(), vec![Row::Ctrl(0), Row::Ctrl(1), Row::Ctrl(3)]);
    assert_eq!(kind_text(CtrlKind::PolyPressure(60)), "Poly Pressure C3");
}

#[test]
fn the_insert_row_adds_an_event_of_its_kind_and_channel() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let t = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let clip = app.run("midi.new_clip", json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
    app.run("edit.select", json!({"clips": [clip]})).unwrap();
    app.ui.show_event_list = true;
    app.midi.event_list.insert_kind = "cc".into();
    app.midi.event_list.insert_number = 11;
    app.midi.event_list.insert_channel = 3;
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h.get_by_label("Insert").click();
    h.run_steps(3);
    let r = h.state_mut().run("midi.ctrls", json!({"clip": clip, "lane": "cc11"})).unwrap();
    assert_eq!(r["events"][0]["channel"], 3, "{r}");
    // A note through it too.
    h.state_mut().midi.event_list.insert_kind = "note".into();
    h.run_steps(2);
    h.get_by_label("Insert").click();
    h.run_steps(3);
    let n = h.state_mut().run("midi.notes", json!({"clip": clip})).unwrap();
    assert_eq!(n["notes"].as_array().unwrap().len(), 1);
    // A controller's channel can be typed (the command the list runs).
    h.state_mut().run("midi.ctrl_edit", json!({"clip": clip, "index": 0, "channel": 5})).unwrap();
    let r = h.state_mut().run("midi.ctrls", json!({"clip": clip})).unwrap();
    assert_eq!(r["events"][0]["channel"], 5);
}

/// SysEx shows in the list (first at its tick, as it plays first; the channel filter leaves it
/// in), is inserted from the Insert row and deleted from its row.
#[test]
fn sysex_is_listed_inserted_and_deleted() {
    let seq = Sequence {
        notes: vec![Note { probability: 100, pitch: 60, velocity: 100, muted: false, release_velocity: 64, channel: 0, start: 0, length: 10 }],
        ctrls: Vec::new(),
        sysex: vec![soundcraft_midi::SysEx::new(0, &[0x43, 0x10]).unwrap(), soundcraft_midi::SysEx::new(-5, &[0x41]).unwrap()],
    };
    let all = rows(&seq, &EventListState { channel: Some(3), ..EventListState::default() });
    assert_eq!(all.iter().map(|x| x.1).collect::<Vec<_>>(), vec![Row::SysEx(0)], "the channel filter keeps it; hidden ones out");
    let both = rows(&seq, &EventListState::default());
    assert_eq!(both.iter().map(|x| x.1).collect::<Vec<_>>(), vec![Row::SysEx(0), Row::Note(0)], "before the note at its tick");
    assert!(rows(&seq, &EventListState { sysex: false, ..EventListState::default() }).iter().all(|x| !matches!(x.1, Row::SysEx(_))));

    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let t = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Synth"));
    let clip = app.run("midi.new_clip", json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
    app.run("edit.select", json!({"clips": [clip]})).unwrap();
    app.ui.show_event_list = true;
    app.midi.event_list.insert_kind = "sysex".into();
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h.get_by_label("Insert").click();
    h.run_steps(3);
    let r = h.state_mut().run("midi.sysex", json!({"clip": clip})).unwrap();
    assert_eq!(r["events"][0]["bytes"], "F0 7E 7F 09 01 F7", "{r}");
    h.run_steps(2);
    let _ = h.get_by_label("6 bytes");
    h.get_by_label("✕").click();
    h.run_steps(3);
    let r = h.state_mut().run("midi.sysex", json!({"clip": clip})).unwrap();
    assert!(r["events"].as_array().unwrap().is_empty(), "{r}");
}
