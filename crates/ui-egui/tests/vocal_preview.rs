//! The Preview toggle: a clicked or edited note plays once from its start to its end with the
//! track soloed, and the playhead and selection come back; off, nothing; never while the
//! transport plays on its own; an edit waits for the tuned render.

mod vocal_support;

use egui::{Key, Modifiers};
use serde_json::json;
use soundcraft_time::Range;
use soundcraft_ui_egui::SoundApp;
use vocal_support::*;

/// Runs frames until the transport stops (at most `max`).
fn run_until_stopped(ctx: &egui::Context, app: &mut SoundApp, max: usize) -> usize {
    for k in 0..max {
        if !app.is_playing() {
            return k;
        }
        frame(ctx, app, vec![]);
    }
    max
}

#[test]
fn a_click_previews_the_note_soloed_and_puts_the_playhead_back() {
    let Setup { ctx, mut app, track, .. } = setup();
    assert!(app.ui.vocal_preview, "on by default");
    let ns = notes(&app, track);
    app.run("transport.locate", json!({"at": 1000})).unwrap();
    settle(&ctx, &mut app);
    let sel = app.engine.session().edit.selection;
    assert_eq!(app.position(), 1000);
    let (start, end) = span(&app, ns[1]);
    click(&ctx, &mut app, bar(&ctx, ns[1]).center());
    let pv = app.vocal.preview.clone().expect("a preview after the click");
    assert_eq!((pv.note, pv.start, pv.end), (ns[1], start, end));
    assert!(!pv.pending(), "no edit: the render is current, it plays at once");
    assert!(app.is_playing(), "playing the note");
    assert_eq!(app.preview_solo, Some(track), "the track soloed for the player");
    let pos = app.engine.transport.position;
    assert!(pos >= start && pos <= end, "from the note's start: {pos} in {start}..{end}");
    let frames = run_until_stopped(&ctx, &mut app, 400);
    assert!(frames < 400, "it stops at the note's end");
    settle(&ctx, &mut app);
    assert!(app.vocal.preview.is_none(), "over");
    assert_eq!(app.preview_solo, None, "the solo goes");
    assert_eq!(app.position(), 1000, "the playhead is back");
    assert_eq!(app.engine.session().edit.selection, sel, "and the selection");
    assert_eq!(app.engine.transport.position, 1000);
    // A new gesture restarts it: a click on the other note previews that one.
    click(&ctx, &mut app, bar(&ctx, ns[1]).center());
    assert!(app.is_playing());
    frame(&ctx, &mut app, vec![]);
    click(&ctx, &mut app, bar(&ctx, ns[0]).center());
    let pv = app.vocal.preview.clone().expect("restarted on the other note");
    assert_eq!(pv.note, ns[0]);
    assert!(app.is_playing());
    let (s0, e0) = span(&app, ns[0]);
    let pos = app.engine.transport.position;
    assert!(pos >= s0 && pos <= e0, "{pos} in {s0}..{e0}");
    run_until_stopped(&ctx, &mut app, 400);
    settle(&ctx, &mut app);
    assert_eq!(app.position(), 1000);
}

#[test]
fn off_nothing_plays_and_the_toggle_is_in_the_header() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    let b = control(&ctx, "preview").expect("the Preview button");
    click(&ctx, &mut app, b.center());
    assert!(!app.ui.vocal_preview, "toggled off");
    click(&ctx, &mut app, bar(&ctx, ns[0]).center());
    assert!(app.vocal.preview.is_none() && !app.is_playing(), "off: nothing plays");
    assert_eq!(app.vocal.selected.len(), 1, "the click still selects");
    click(&ctx, &mut app, control(&ctx, "preview").unwrap().center());
    assert!(app.ui.vocal_preview, "and on again");
}

#[test]
fn never_while_the_transport_plays_on_its_own() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    app.run("transport.play", json!({})).unwrap();
    settle(&ctx, &mut app);
    assert!(app.is_playing());
    let pos = app.engine.transport.position;
    click(&ctx, &mut app, bar(&ctx, ns[0]).center());
    assert!(app.vocal.preview.is_none(), "no preview over the user's playback");
    assert!(app.is_playing() && app.engine.transport.position >= pos, "which goes on");
    assert_eq!(app.preview_solo, None);
    app.run("transport.stop", json!({})).unwrap();
    settle(&ctx, &mut app);
    assert!(!app.is_playing());
}

#[test]
fn an_edit_waits_for_the_tuned_render_then_plays_the_note() {
    let Setup { ctx, mut app, track, clips } = setup();
    let ns = notes(&app, track);
    app.vocal.selected = [ns[0]].into_iter().collect();
    settle(&ctx, &mut app);
    // Alt+Up moves the note: the render is stale, so the preview waits for it.
    press(&ctx, &mut app, view(&ctx).rect.center(), Key::ArrowUp, Modifiers::ALT);
    let pv = app.vocal.preview.clone().expect("a preview after the edit");
    assert_eq!(pv.note, ns[0]);
    if pv.pending() {
        assert!(!app.is_playing(), "not before the render lands");
        soundcraft_engine::vocal::run_until_idle(&mut app.engine, &clips).unwrap();
        settle(&ctx, &mut app);
    }
    assert!(app.vocal.preview.as_ref().is_some_and(|p| !p.pending()), "{:?}", app.vocal.preview);
    assert!(app.is_playing(), "plays once the render is current");
    run_until_stopped(&ctx, &mut app, 400);
    settle(&ctx, &mut app);
    assert!(app.vocal.preview.is_none());
    assert_eq!(app.engine.session().edit.selection, Range::point(0), "the selection is back");
}
