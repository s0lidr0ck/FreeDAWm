//! The Vocal Editor's time ruler (a click locates, a drag scrubs) and Space over the editor
//! (the selected notes' span; Shift+Space from the playhead; stops when playing).

mod vocal_support;

use egui::{Event, Key, Modifiers, pos2};
use serde_json::json;
use soundcraft_ui_egui::SoundApp;
use vocal_support::*;

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
fn a_click_in_the_ruler_locates_and_a_drag_scrubs() {
    let Setup { ctx, mut app, .. } = setup();
    let ruler = control(&ctx, "ruler").expect("the ruler's rect");
    let v = view(&ctx);
    let at = i64::from(RATE); // 1 s
    let x = v.x(at);
    click(&ctx, &mut app, pos2(x, ruler.center().y));
    let got = app.position();
    assert!((got - at).abs() < 200, "located at the click: {got} vs {at}");
    assert!(!app.is_playing(), "a click only locates");
    // A drag scrubs: the playhead follows the pointer.
    let to = pos2(v.x(2 * at), ruler.center().y);
    let from = pos2(x, ruler.center().y);
    frame(&ctx, &mut app, vec![Event::PointerMoved(from)]);
    frame(&ctx, &mut app, vec![button(from, true, Modifiers::NONE)]);
    let mut mid = None;
    for k in 1..=6 {
        frame(&ctx, &mut app, vec![Event::PointerMoved(from.lerp(to, k as f32 / 6.0))]);
        if k == 3 {
            mid = Some(app.position());
        }
    }
    frame(&ctx, &mut app, vec![button(to, false, Modifiers::NONE)]);
    settle(&ctx, &mut app);
    let mid = mid.unwrap();
    assert!(mid > at + 10_000 && mid < 2 * at - 10_000, "halfway through the drag: {mid}");
    assert!((app.position() - 2 * at).abs() < 200, "where the drag ended: {}", app.position());
    // The roll's playhead line follows (drawn at the located time).
    let v = view(&ctx);
    assert!((v.x(app.position()) - to.x).abs() < 2.0);
}

#[test]
fn space_plays_the_selected_notes_and_stops_at_their_end() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    app.run("transport.locate", json!({"at": 100})).unwrap();
    app.vocal.selected = ns.iter().copied().collect();
    settle(&ctx, &mut app);
    let (s0, _) = span(&app, ns[0]);
    let (_, e1) = span(&app, ns[1]);
    let over = view(&ctx).rect.center();
    press(&ctx, &mut app, over, Key::Space, Modifiers::NONE);
    assert!(app.is_playing(), "Space with notes selected plays");
    let pos = app.engine.transport.position;
    assert!(pos >= s0 && pos < e1, "from the first selected note's start: {pos} in {s0}..{e1}");
    let frames = run_until_stopped(&ctx, &mut app, 600);
    assert!(frames < 600, "stops at the last selected note's end");
    assert!(app.engine.transport.position >= e1 - 2000, "{}", app.engine.transport.position);
    // Space again while playing stops.
    press(&ctx, &mut app, over, Key::Space, Modifiers::NONE);
    assert!(app.is_playing());
    press(&ctx, &mut app, over, Key::Space, Modifiers::NONE);
    assert!(!app.is_playing(), "Space stops");
}

#[test]
fn shift_space_and_no_selection_play_from_the_playhead() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    let ph = 7 * i64::from(RATE) / 10;
    app.run("transport.locate", json!({"at": ph})).unwrap();
    settle(&ctx, &mut app);
    let over = view(&ctx).rect.center();
    // No selection: from the playhead.
    press(&ctx, &mut app, over, Key::Space, Modifiers::NONE);
    assert!(app.is_playing());
    let pos = app.engine.transport.position;
    assert!(pos >= ph && pos < ph + 10_000, "{pos} from {ph}");
    press(&ctx, &mut app, over, Key::Space, Modifiers::NONE);
    assert!(!app.is_playing());
    // A selection, but Shift: still from the playhead.
    app.run("transport.locate", json!({"at": ph})).unwrap();
    app.vocal.selected = [ns[0]].into_iter().collect();
    settle(&ctx, &mut app);
    press(&ctx, &mut app, over, Key::Space, Modifiers::SHIFT);
    assert!(app.is_playing());
    let pos = app.engine.transport.position;
    assert!(pos >= ph && pos < ph + 10_000, "Shift+Space ignores the selection: {pos} from {ph}");
    press(&ctx, &mut app, over, Key::Space, Modifiers::NONE);
    assert!(!app.is_playing());
    // Not over the editor: Space is the app's (Play/Stop), untouched by the selection.
    app.run("transport.locate", json!({"at": ph})).unwrap();
    settle(&ctx, &mut app);
    press(&ctx, &mut app, pos2(5.0, 5.0), Key::Space, Modifiers::NONE);
    assert!(app.is_playing());
    let pos = app.engine.transport.position;
    assert!(pos >= ph && pos < ph + 10_000, "{pos} from {ph}");
    app.run("transport.stop", json!({})).unwrap();
    settle(&ctx, &mut app);
}

/// Twenty clicks at random x (and y) in the ruler all locate, also inside an Edit window
/// timeline selection (which `transport.locate` alone keeps, the playhead staying at its start).
#[test]
fn twenty_ruler_clicks_all_locate_even_inside_an_edit_selection() {
    let Setup { ctx, mut app, .. } = setup();
    let ruler = control(&ctx, "ruler").expect("the ruler's rect");
    app.run("edit.select", json!({"start": 0, "end": 3 * i64::from(RATE)})).unwrap();
    settle(&ctx, &mut app);
    let mut seed = 12345u64;
    let mut fails = vec![];
    for k in 0..20 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let fx = ((seed >> 33) % 1000) as f32 / 1000.0;
        let fy = ((seed >> 13) % 1000) as f32 / 1000.0;
        let p = pos2(ruler.left() + 2.0 + fx * (ruler.width() - 4.0), ruler.top() + 1.0 + fy * (ruler.height() - 2.0));
        let want = view(&ctx).at(p.x);
        if k % 2 == 0 {
            // The pointer comes up from the roll.
            frame(&ctx, &mut app, vec![Event::PointerMoved(pos2(p.x, ruler.bottom() + 30.0))]);
        }
        click(&ctx, &mut app, p);
        let got = app.position();
        if (got - want).abs() > 200 {
            fails.push((k, want, got));
        }
    }
    assert!(fails.is_empty(), "{} of 20 clicks didn't locate: {fails:?}", fails.len());
    let sel = app.engine.session().edit.selection;
    assert_eq!(sel.start, sel.end, "the click leaves an insertion point, not the old range");
}

/// A click on empty roll space locates there and clears the note selection; a drag on empty
/// space still rubber-bands (and doesn't locate).
#[test]
fn a_blank_click_locates_and_deselects_and_a_blank_drag_rubber_bands() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    app.vocal.selected = ns.iter().copied().collect();
    app.run("transport.locate", json!({"at": 0})).unwrap();
    settle(&ctx, &mut app);
    let v = view(&ctx);
    // Below every note, 2 s in (the take ends near 1.45 s).
    let lowest = shapes(&ctx).iter().map(|s| s.rect.bottom()).fold(f32::MIN, f32::max);
    let blank = pos2(v.x(2 * i64::from(RATE)), (lowest + 40.0).min(v.rect.bottom() - 4.0));
    assert!(shapes(&ctx).iter().all(|s| !s.rect.expand(4.0).contains(blank)));
    click(&ctx, &mut app, blank);
    assert!(app.vocal.selected.is_empty(), "the blank click deselects");
    let got = app.position();
    assert!((got - v.at(blank.x)).abs() < 200, "the blank click locates: {got}");
    // A drag from blank space across the notes rubber-bands them; the playhead stays.
    let before = app.position();
    let r0 = shapes(&ctx).iter().find(|s| s.r == ns[0]).unwrap().rect;
    let from = pos2(r0.left() - 10.0, r0.top() - 10.0);
    drag(&ctx, &mut app, from, r0.center() + egui::vec2(0.0, 5.0), 6);
    assert!(app.vocal.selected.contains(&ns[0]), "the drag picked the note");
    assert_eq!(app.position(), before, "a drag doesn't locate");
}
