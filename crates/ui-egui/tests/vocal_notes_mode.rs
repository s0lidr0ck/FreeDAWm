//! The Tuning panel's Notes selector (Classic | Musical): it shows the track's mode, a click runs
//! `vocal.note_mode` on the track's audio clips as one undo step, and the roll then shows the
//! clips' notes cut that way.

mod vocal_support;

use soundcraft_vocal::NoteMode;
use vocal_support::*;

fn click_at(ctx: &egui::Context, app: &mut soundcraft_ui_egui::SoundApp, name: &str) {
    let r = control(ctx, name).unwrap_or_else(|| panic!("no control {name}"));
    click(ctx, app, r.center());
}

#[test]
fn the_notes_selector_switches_the_tracks_clips() {
    let Setup { ctx, mut app, clips, .. } = setup();
    let mode = |app: &soundcraft_ui_egui::SoundApp| soundcraft_engine::vocal::note_mode(app.engine.session().find_clip(clips[0]).unwrap().1);
    // New tunings (and untuned clips) are Musical.
    assert_eq!(text(&ctx, "notes-mode").as_deref(), Some("musical"));
    assert_eq!(mode(&app), NoteMode::Musical);
    let undo = app.engine.undo_history().len();
    click_at(&ctx, &mut app, "notes-classic");
    settle(&ctx, &mut app);
    assert_eq!(mode(&app), NoteMode::Classic);
    assert_eq!(text(&ctx, "notes-mode").as_deref(), Some("classic"));
    assert_eq!(app.engine.undo_history().len(), undo + 1, "one undo step");
    // Clicking the lit side again changes nothing.
    click_at(&ctx, &mut app, "notes-classic");
    settle(&ctx, &mut app);
    assert_eq!(app.engine.undo_history().len(), undo + 1);
    click_at(&ctx, &mut app, "notes-musical");
    settle(&ctx, &mut app);
    assert_eq!(mode(&app), NoteMode::Musical);
    assert!(app.engine.session().find_clip(clips[0]).unwrap().1.tune.is_none(), "back to Musical with nothing else: untuned");
}
