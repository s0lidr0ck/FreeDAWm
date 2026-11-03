//! One note selection, shared by the Vocal Editor and the track's "waveform + pitch" lane in the
//! Edit window: a note clicked on the lane is the editor's selection (its note bar targets it),
//! and the editor's selection shows on the lane.

mod vocal_support;

use egui::{Event, Id, Modifiers, Pos2};
use serde_json::json;
use soundcraft_engine::vocal::NoteRef;
use soundcraft_model::{ClipId, TrackHeight};
use soundcraft_ui_egui::vocal_track::PitchSeg;
use vocal_support::*;

/// The Lead Vox track 200 px tall in the pitch view, the editor open below.
fn shared_setup() -> Setup {
    let mut st = setup();
    if let Some(t) = st.app.engine.session_mut().track_mut(st.track) {
        t.height = TrackHeight::Custom(200);
    }
    st.app.run("track.view", json!({"track": st.track.0, "view": "pitch"})).unwrap();
    settle(&st.ctx, &mut st.app);
    st
}

fn segs(ctx: &egui::Context, clip: ClipId) -> Vec<PitchSeg> {
    ctx.data(|d| d.get_temp::<Vec<PitchSeg>>(Id::new(format!("soundcraft-vocal-track-segs-{}", clip.0)))).unwrap_or_default()
}

fn lane_selected(ctx: &egui::Context, clip: ClipId) -> Vec<NoteRef> {
    ctx.data(|d| d.get_temp::<Vec<NoteRef>>(Id::new(format!("soundcraft-vocal-track-selected-{}", clip.0)))).unwrap_or_default()
}

/// The middle of note `r`'s line on the lane.
fn on_lane(ctx: &egui::Context, clip: ClipId, r: NoteRef) -> Pos2 {
    let s = segs(ctx, clip);
    let seg = s.iter().find(|s| s.note == r).expect("the note's line on the lane");
    seg.pts[seg.pts.len() / 2]
}

#[test]
fn a_note_clicked_on_the_track_lane_is_the_editors_selection() {
    let Setup { ctx, mut app, track, clips } = shared_setup();
    let ns = notes(&app, track);
    assert_eq!(segs(&ctx, clips[0]).len(), 2, "both notes on the lane");
    let p = on_lane(&ctx, clips[0], ns[0]);
    assert!(!view(&ctx).rect.contains(p), "the lane is in the Edit window, not the roll");
    click(&ctx, &mut app, p);
    assert_eq!(app.vocal.selected.iter().copied().collect::<Vec<_>>(), vec![ns[0]], "the lane's click selects in the editor");
    assert_eq!(text(&ctx, "selected-text").as_deref(), Some("1 selected"));
    // The note bar targets it: under its bar in the roll.
    let nb = control(&ctx, "note-bar").expect("the note bar shows");
    let sh = bar(&ctx, ns[0]);
    assert!((nb.left() - sh.left().max(view(&ctx).rect.left())).abs() < 1.0, "{nb:?} under {sh:?}");
    // And the lane draws it selected.
    assert_eq!(lane_selected(&ctx, clips[0]), vec![ns[0]]);
    // Shift+click adds the second; a plain click on it alone selects it alone.
    click_mod(&ctx, &mut app, on_lane(&ctx, clips[0], ns[1]), Modifiers::SHIFT);
    assert_eq!(app.vocal.selected.len(), 2);
    assert_eq!(text(&ctx, "selected-text").as_deref(), Some("2 selected"));
    click(&ctx, &mut app, on_lane(&ctx, clips[0], ns[1]));
    assert_eq!(app.vocal.selected.iter().copied().collect::<Vec<_>>(), vec![ns[1]]);
}

#[test]
fn the_editors_selection_shows_on_the_track_lane() {
    let Setup { ctx, mut app, track, clips } = shared_setup();
    let ns = notes(&app, track);
    assert!(lane_selected(&ctx, clips[0]).is_empty());
    click(&ctx, &mut app, bar(&ctx, ns[1]).center());
    assert_eq!(app.vocal.selected.iter().copied().collect::<Vec<_>>(), vec![ns[1]]);
    assert_eq!(lane_selected(&ctx, clips[0]), vec![ns[1]], "the roll's click shows on the lane");
    // Escape in the editor clears both.
    frame(&ctx, &mut app, vec![Event::PointerMoved(view(&ctx).rect.center())]);
    frame(&ctx, &mut app, key(egui::Key::Escape, Modifiers::NONE));
    settle(&ctx, &mut app);
    assert!(lane_selected(&ctx, clips[0]).is_empty());
}
