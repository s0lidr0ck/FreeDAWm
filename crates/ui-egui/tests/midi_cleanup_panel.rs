//! The MIDI editor's Cleanup panel: sliders, the Cleaned/Original switch, Commit and the
//! no-chords state, driven through real egui input on the demo session's Keys clip.

use egui::{Event, Id, Key, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use serde_json::json;
use soundcraft_model::{Clip, ClipContent, ClipId};
use soundcraft_ui_egui::{Services, SoundApp};

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

fn control(ctx: &egui::Context, name: &str) -> Option<Rect> {
    ctx.data(|d| d.get_temp::<Rect>(Id::new(format!("soundcraft-midi-cleanup-{name}"))))
}

fn summary(ctx: &egui::Context) -> Option<String> {
    ctx.data(|d| d.get_temp::<String>(Id::new("soundcraft-midi-cleanup-summary")))
}

/// The demo session with the MIDI editor open on the Keys clip; `chords` lays a C chord over it.
fn setup(chords: bool) -> (egui::Context, SoundApp, ClipId) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
    app.ui.show_midi_editor = true;
    let (cid, start, end) = {
        let s = app.engine.session();
        let c = s.tracks.iter().find(|t| t.name == "Keys").and_then(|t| t.clips().first()).unwrap();
        (c.id, c.start, c.end())
    };
    app.run("edit.select", json!({"clips": [cid.0]})).unwrap();
    if chords {
        app.run("band.chord.add", json!({"at": start, "end": end, "chord": "C"})).unwrap();
    }
    let ctx = egui::Context::default();
    for _ in 0..6 {
        frame(&ctx, &mut app, vec![]);
    }
    (ctx, app, cid)
}

fn clip(app: &SoundApp, cid: ClipId) -> Clip {
    app.engine.session().find_clip(cid).map(|(_, c)| c.clone()).unwrap()
}

fn drag(ctx: &egui::Context, app: &mut SoundApp, from: Pos2, to: Pos2) {
    frame(ctx, app, vec![Event::PointerMoved(from)]);
    frame(ctx, app, vec![button(from, true)]);
    for step in 1..=8 {
        frame(ctx, app, vec![Event::PointerMoved(from.lerp(to, step as f32 / 8.0))]);
    }
    frame(ctx, app, vec![button(to, false)]);
    for _ in 0..3 {
        frame(ctx, app, vec![]);
    }
}

fn click(ctx: &egui::Context, app: &mut SoundApp, at: Pos2) {
    frame(ctx, app, vec![Event::PointerMoved(at)]);
    frame(ctx, app, vec![button(at, true)]);
    frame(ctx, app, vec![button(at, false)]);
    for _ in 0..3 {
        frame(ctx, app, vec![]);
    }
}

fn across(r: Rect) -> (Pos2, Pos2) {
    (pos2(r.left(), r.center().y), pos2(r.right(), r.center().y))
}

#[test]
fn panel_shows_and_slider_changes_preview() {
    let (ctx, mut app, cid) = setup(true);
    assert!(clip(&app, cid).midi_cleanup.is_none(), "showing the panel creates nothing");
    assert!(control(&ctx, "detect").is_none(), "chords overlap the clip");
    assert!(summary(&ctx).is_none(), "no counts before the clip has a cleanup");
    let steps = app.engine.undo_history().len();
    let (a, b) = across(control(&ctx, "wrong").expect("the wrong-notes slider"));
    drag(&ctx, &mut app, a, b);
    let c = clip(&app, cid);
    let m = c.midi_cleanup.as_ref().expect("the first change creates settings");
    assert_eq!(m.settings.wrong, 100);
    assert_eq!(m.settings.held, 70, "the other settings keep their defaults");
    assert_eq!(app.engine.undo_history().len(), steps + 1, "the whole drag is one undo step");
    let report = m.preview.as_ref().expect("a preview").out.report;
    assert_eq!(summary(&ctx).unwrap(), report.summary());
    // One undo takes the clip back to no cleanup at all.
    assert!(app.engine.undo());
    assert!(clip(&app, cid).midi_cleanup.is_none());
}

#[test]
fn panel_without_chords_disables_harmonic_sliders() {
    let (ctx, mut app, cid) = setup(false);
    assert!(control(&ctx, "detect").is_some(), "a Detect chords button");
    let (a, b) = across(control(&ctx, "wrong").unwrap());
    drag(&ctx, &mut app, a, b);
    assert!(clip(&app, cid).midi_cleanup.is_none(), "the disabled slider changes nothing");
    let (a, b) = across(control(&ctx, "legato").unwrap());
    drag(&ctx, &mut app, a, b);
    let m = clip(&app, cid).midi_cleanup.unwrap();
    assert_eq!(m.settings.legato, 100);
    assert_eq!(m.settings.wrong, 40);
}

#[test]
fn commit_button_commits() {
    let (ctx, mut app, cid) = setup(true);
    app.run("midi.cleanup", json!({"clip": cid.0, "wrong": 100, "held": 100})).unwrap();
    frame(&ctx, &mut app, vec![]);
    let cleaned = clip(&app, cid).midi_cleanup.unwrap().preview.unwrap().out.sequence.notes.clone();
    click(&ctx, &mut app, control(&ctx, "commit").unwrap().center());
    let c = clip(&app, cid);
    assert!(c.midi_cleanup.is_none());
    let ClipContent::Midi { sequence } = &c.content else { panic!("not MIDI") };
    assert_eq!(sequence.notes, cleaned);
}

#[test]
fn switch_and_reset_buttons() {
    let (ctx, mut app, cid) = setup(true);
    app.run("midi.cleanup", json!({"clip": cid.0, "wrong": 90})).unwrap();
    frame(&ctx, &mut app, vec![]);
    let sw = control(&ctx, "switch").unwrap();
    // The right half is "Original".
    click(&ctx, &mut app, pos2(sw.right() - 8.0, sw.center().y));
    assert!(clip(&app, cid).midi_cleanup.unwrap().original);
    click(&ctx, &mut app, pos2(sw.left() + 8.0, sw.center().y));
    assert!(!clip(&app, cid).midi_cleanup.unwrap().original);
    click(&ctx, &mut app, control(&ctx, "reset").unwrap().center());
    assert_eq!(clip(&app, cid).midi_cleanup.unwrap().settings.wrong, 40);
}

#[test]
fn note_edit_in_the_editor_refreshes_the_preview() {
    let (ctx, mut app, cid) = setup(true);
    app.run("midi.cleanup", json!({"clip": cid.0})).unwrap();
    frame(&ctx, &mut app, vec![]);
    let before = clip(&app, cid).midi_cleanup.unwrap().preview.unwrap().out.report.total;
    app.midi.selected = vec![0];
    let roll = ctx.data(|d| d.get_temp::<Rect>(Id::new("soundcraft-midi-roll"))).unwrap();
    frame(&ctx, &mut app, vec![Event::PointerMoved(roll.center()), delete()]);
    frame(&ctx, &mut app, vec![]);
    let c = clip(&app, cid);
    let ClipContent::Midi { sequence } = &c.content else { panic!("not MIDI") };
    let total = c.midi_cleanup.unwrap().preview.unwrap().out.report.total;
    assert_eq!(total, sequence.notes.len());
    assert_eq!(total + 1, before, "the preview follows the deleted note");
}

fn delete() -> Event {
    Event::Key { key: Key::Delete, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }
}

#[test]
fn delete_over_the_panel_keeps_the_selected_notes() {
    let (ctx, mut app, cid) = setup(true);
    let count = |app: &SoundApp| match &clip(app, cid).content {
        ClipContent::Midi { sequence } => sequence.notes.len(),
        _ => 0,
    };
    let before = count(&app);
    app.midi.selected = vec![0];
    let over = control(&ctx, "legato").unwrap().center();
    frame(&ctx, &mut app, vec![Event::PointerMoved(over), delete()]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(count(&app), before, "Delete over the panel must not delete notes");
    assert_eq!(app.midi.selected, vec![0]);
}

#[test]
fn delete_with_the_panel_hidden_works_off_the_roll() {
    let (ctx, mut app, cid) = setup(true);
    app.midi.show_cleanup = false;
    frame(&ctx, &mut app, vec![]);
    let count = |app: &SoundApp| match &clip(app, cid).content {
        ClipContent::Midi { sequence } => sequence.notes.len(),
        _ => 0,
    };
    let before = count(&app);
    app.midi.selected = vec![0];
    // Over the velocity lane, below the roll.
    let roll = ctx.data(|d| d.get_temp::<Rect>(Id::new("soundcraft-midi-roll"))).unwrap();
    frame(&ctx, &mut app, vec![Event::PointerMoved(pos2(roll.center().x, roll.bottom() + 20.0)), delete()]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(count(&app), before - 1, "with no panel, Delete works as it always did");
}

#[test]
fn original_on_a_clip_without_cleanup_creates_nothing() {
    let (ctx, mut app, cid) = setup(true);
    let sw = control(&ctx, "switch").unwrap();
    click(&ctx, &mut app, pos2(sw.right() - 8.0, sw.center().y));
    assert!(clip(&app, cid).midi_cleanup.is_none(), "the take already plays: nothing to switch");
}

#[test]
fn cleaned_applies_the_defaults_at_once() {
    let (ctx, mut app, cid) = setup(true);
    let sw = control(&ctx, "switch").unwrap();
    click(&ctx, &mut app, pos2(sw.left() + 8.0, sw.center().y));
    let m = clip(&app, cid).midi_cleanup.expect("Cleaned turns the cleanup on");
    assert_eq!(m.settings, soundcraft_midi::cleanup::CleanupSettings::default());
    assert!(!m.original);
    assert_eq!(summary(&ctx).unwrap(), m.preview.expect("a preview").out.report.summary());
}

#[test]
fn a_click_on_a_slider_at_its_value_applies_the_defaults() {
    let (ctx, mut app, cid) = setup(true);
    let r = control(&ctx, "wrong").unwrap();
    click(&ctx, &mut app, pos2(r.left() + r.width() * 0.4, r.center().y));
    let m = clip(&app, cid).midi_cleanup.expect("the click turns the cleanup on");
    assert_eq!(m.settings, soundcraft_midi::cleanup::CleanupSettings::default());
    assert!(m.preview.is_some());
}

#[test]
fn the_preview_follows_a_drag_before_release_in_one_undo_step() {
    let (ctx, mut app, cid) = setup(true);
    app.run("midi.cleanup", json!({"clip": cid.0})).unwrap();
    frame(&ctx, &mut app, vec![]);
    let steps = app.engine.undo_history().len();
    let at_default = clip(&app, cid).midi_cleanup.unwrap().preview.unwrap();
    let (a, b) = across(control(&ctx, "wrong").unwrap());
    let from = pos2(a.x + (b.x - a.x) * 0.4, a.y);
    frame(&ctx, &mut app, vec![Event::PointerMoved(from)]);
    frame(&ctx, &mut app, vec![button(from, true)]);
    // Drag to 100 % and hold the button, past the drag throttle.
    for step in 1..=8 {
        frame(&ctx, &mut app, vec![Event::PointerMoved(from.lerp(b, step as f32 / 8.0))]);
    }
    std::thread::sleep(std::time::Duration::from_millis(60));
    frame(&ctx, &mut app, vec![Event::PointerMoved(b)]);
    frame(&ctx, &mut app, vec![]);
    let c = clip(&app, cid);
    let m = c.midi_cleanup.as_ref().unwrap();
    assert_eq!(m.settings.wrong, 100, "the setting follows the drag before release");
    let live = m.preview.clone().expect("a preview");
    assert_ne!(live.inputs, at_default.inputs, "the preview was re-derived for 100 %");
    assert_eq!(c.midi_playing(), Some(&live.out.sequence), "playback plays the live preview");
    assert_eq!(summary(&ctx).unwrap(), live.out.report.summary(), "the counts follow the drag");
    assert!(app.engine.undo_history().len() <= steps + 1);
    frame(&ctx, &mut app, vec![button(b, false)]);
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    assert_eq!(app.engine.undo_history().len(), steps + 1, "the whole drag is one undo step");
    assert!(app.engine.undo());
    assert_eq!(clip(&app, cid).midi_cleanup.unwrap().settings.wrong, 40, "one undo undoes the whole drag");
}
