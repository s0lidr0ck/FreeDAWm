//! Entering and editing notes in the Score Editor: a click on the staff adds a note of the chosen
//! value where it lands, a click on a head selects it, Delete and the arrows edit it, A–G enter at
//! the cursor; each is one undo step, and the key on the Key ruler spells what is entered.

use egui::{Event, Key, Modifiers, PointerButton, Pos2, pos2, vec2};
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_model::{ChannelFormat, ClipContent, ClipId, TrackKind};
use soundcraft_ui_egui::{Services, SoundApp};

/// An empty two-bar Keys clip, selected, with the Score Editor open.
fn harness(key: Option<&str>) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    let t = app.engine.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let clip = app.run("midi.new_clip", json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
    app.run("edit.select", json!({"clips": [clip]})).unwrap();
    if let Some(k) = key {
        app.engine.session_mut().key_signatures.push((0, k.into()));
    }
    app.ui.show_score = true;
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
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

fn clip(h: &Harness<'_, SoundApp>) -> ClipId {
    h.state().engine.session().edit.selected_clips[0]
}

/// (pitch, start, length) of every note.
fn psl(h: &Harness<'_, SoundApp>) -> Vec<(u8, i64, i64)> {
    match &h.state().engine.session().find_clip(clip(h)).unwrap().1.content {
        ClipContent::Midi { sequence } => sequence.notes.iter().map(|n| (n.pitch, n.start, n.length)).collect(),
        _ => panic!("not MIDI"),
    }
}

/// The point on the staff at `tick` (from the clip's start) on diatonic `step` above middle C.
fn at(h: &Harness<'_, SoundApp>, tick: i64, step: i32) -> Pos2 {
    let g = h.state().score.drawn.expect("the staff is drawn");
    pos2(g.x_of(tick), g.y_of(step))
}

fn click(h: &mut Harness<'_, SoundApp>, p: Pos2, modifiers: Modifiers) {
    h.event(Event::ModifiersChanged(modifiers));
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers });
    h.step();
    h.input_mut().events.push(Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers });
    h.step();
    h.event(Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

fn key(h: &mut Harness<'_, SoundApp>, k: Key, modifiers: Modifiers) {
    h.input_mut().events.push(Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers });
    h.step();
    h.input_mut().events.push(Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers });
    h.step();
}

fn undo(h: &mut Harness<'_, SoundApp>) {
    h.state_mut().run("edit.undo", json!({})).unwrap();
    h.step();
}

#[test]
fn a_click_adds_a_quarter_where_it_lands_and_the_keys_edit_it() {
    let mut h = harness(None);
    assert_eq!(h.state().score.value, 2, "a quarter by default");
    // The treble G line (step 4) at beat 2, a little off the beat: snapped onto it.
    let p = at(&h, 960, 4) + vec2(3.0, 1.0);
    click(&mut h, p, Modifiers::NONE);
    assert_eq!(psl(&h), vec![(67, 960, 960)]);
    assert_eq!(h.state().midi.selected, vec![0], "the new note is selected");
    // Up: a scale step (A); Alt+Down: a semitone (G#); Delete removes it.
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(psl(&h), vec![(69, 960, 960)]);
    key(&mut h, Key::ArrowDown, Modifiers::ALT);
    assert_eq!(psl(&h), vec![(68, 960, 960)]);
    // A value key with the note selected sets its length.
    key(&mut h, Key::Num4, Modifiers::NONE);
    assert_eq!(psl(&h), vec![(68, 960, 480)]);
    key(&mut h, Key::Delete, Modifiers::NONE);
    assert!(psl(&h).is_empty());
    // Each was one undo step.
    undo(&mut h);
    assert_eq!(psl(&h), vec![(68, 960, 480)]);
    undo(&mut h);
    assert_eq!(psl(&h), vec![(68, 960, 960)]);
    undo(&mut h);
    assert_eq!(psl(&h), vec![(69, 960, 960)]);
    undo(&mut h);
    assert_eq!(psl(&h), vec![(67, 960, 960)]);
    undo(&mut h);
    assert!(psl(&h).is_empty());
}

#[test]
fn the_key_spells_what_is_entered_and_a_click_on_a_head_selects_it() {
    let mut h = harness(Some("G major"));
    // The top line of the treble staff is F: F# in G major.
    let p = at(&h, 0, 10);
    click(&mut h, p, Modifiers::NONE);
    assert_eq!(psl(&h), vec![(78, 0, 960)]);
    // A chosen natural wins over the key, for that note only.
    h.state_mut().score.accidental = Some(0);
    let p = at(&h, 960, 3);
    click(&mut h, p, Modifiers::NONE);
    assert_eq!(psl(&h), vec![(78, 0, 960), (65, 960, 960)]);
    assert_eq!(h.state().score.accidental, None);
    // A click on the first head selects it; Shift adds the second.
    let p = at(&h, 0, 10);
    click(&mut h, p, Modifiers::NONE);
    assert_eq!(h.state().midi.selected, vec![0]);
    let p = at(&h, 960, 3);
    click(&mut h, p, Modifiers::SHIFT);
    assert_eq!(h.state().midi.selected, vec![0, 1]);
    assert_eq!(psl(&h).len(), 2, "clicks on heads add nothing");
    // Up a scale step in G: F#5 to G5, F4 (not in the key) to G4.
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(psl(&h), vec![(79, 0, 960), (67, 960, 960)]);
    undo(&mut h);
    assert_eq!(psl(&h), vec![(78, 0, 960), (65, 960, 960)], "one undo step for both");
}

#[test]
fn letters_enter_at_the_cursor_and_r_rests() {
    let mut h = harness(None);
    h.state_mut().score.value = 3; // eighths
    let p = at(&h, 0, 4);
    h.input_mut().events.push(Event::PointerMoved(p));
    h.step();
    // G (nearest the default G4), then a rest, then E nearest it, then C.
    for k in [Key::G, Key::R, Key::E, Key::C] {
        key(&mut h, k, Modifiers::NONE);
    }
    assert_eq!(psl(&h), vec![(67, 0, 480), (64, 960, 480), (60, 1440, 480)]);
    assert_eq!(h.state().score.cursor, 1920);
}
