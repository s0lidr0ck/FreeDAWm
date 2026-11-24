//! The word lanes (the Vocal Editor's under its ruler, the Edit window's strip under a track):
//! dragging an edge (one undo step), split and merge from the menu's commands, retyping by
//! double-click, the word being sung lit while playing, guesses drawn apart from confirmed words
//! with a count, tap mode (taps become a line's words), and the AI menu items.

mod vocal_support;

use egui::{Event, Id, Key, Modifiers, Rect};
use serde_json::json;
use soundcraft_model::{LyricLine, LyricWord, WordTiming};
use soundcraft_time::Samples;
use soundcraft_ui_egui::SoundApp;
use soundcraft_ui_egui::word_lane::Drawn;
use vocal_support::*;

const R: i64 = RATE as i64;

fn secs(x: f64) -> Samples {
    (x * R as f64).round() as Samples
}

/// [`setup`] (two notes: 0.25–0.75 s and 0.95–1.45 s on the timeline) with two words on its
/// clip: "Lord" (a guess) over the first note and "say" (confirmed) over the second.
fn with_words() -> Setup {
    let mut st = setup();
    let c = st.clips[0];
    {
        let s = st.app.engine.session_mut();
        let clip = s.find_clip_mut(c).unwrap();
        // The clip starts at 0.25 s; its source time starts at 0.
        let mut lord = LyricWord::new(0, secs(0.5), "Lord");
        lord.timing = WordTiming::Guess;
        let mut say = LyricWord::new(secs(0.7), secs(1.2), "say");
        say.timing = WordTiming::Confirmed;
        clip.lyrics = vec![LyricLine { start: 0, end: secs(1.2), words: vec![lord, say], sheet_line: None }];
    }
    settle(&st.ctx, &mut st.app);
    st
}

fn drawn(ctx: &egui::Context, lane: &str) -> Vec<Drawn> {
    ctx.data(|d| d.get_temp::<Vec<Drawn>>(Id::new(format!("soundcraft-words-{lane}-drawn")))).unwrap_or_default()
}

fn word_box(ctx: &egui::Context, lane: &str, text: &str) -> Rect {
    drawn(ctx, lane).into_iter().find(|d| d.text == text).map(|d| d.rect).unwrap_or_else(|| panic!("no `{text}` in {lane}"))
}

fn words(app: &SoundApp) -> Vec<(String, Samples, Samples, WordTiming)> {
    let t = app.engine.session().tracks[0].id;
    soundcraft_engine::word_time::track_words(app.engine.session(), t).into_iter().map(|w| (w.text, w.start, w.end, w.timing)).collect()
}

#[test]
fn the_vocal_editor_shows_a_word_lane_only_with_words() {
    let Setup { ctx, mut app, .. } = setup();
    assert!(control(&ctx, "words").is_none(), "no words: no lane, the roll keeps its place");
    let roll_top = view(&ctx).rect.top();
    let Setup { ctx, .. } = with_words();
    let lane = control(&ctx, "words").expect("the word lane");
    assert_eq!(lane.height(), soundcraft_ui_egui::word_lane::LANE_H);
    assert_eq!(view(&ctx).rect.top(), lane.bottom(), "the roll starts under the lane");
    assert!(view(&ctx).rect.top() > roll_top);
    let d = drawn(&ctx, "vocal");
    assert_eq!(d.iter().map(|d| d.text.as_str()).collect::<Vec<_>>(), ["Lord", "say"]);
    // Lined up with the notes: "Lord" starts where the first note's bar does.
    let notes = shapes(&ctx);
    assert!((d[0].rect.left() - notes[0].rect.left()).abs() < 2.0, "{:?} vs {:?}", d[0].rect, notes[0].rect);
    let _ = &mut app;
}

#[test]
fn guesses_are_drawn_apart_and_counted() {
    let Setup { ctx, .. } = with_words();
    let d = drawn(&ctx, "vocal");
    assert_eq!((d[0].timing, d[1].timing), (WordTiming::Guess, WordTiming::Confirmed));
    let caption: String = ctx.data(|x| x.get_temp(Id::new("soundcraft-words-vocal-guessed"))).unwrap();
    assert_eq!(caption, "1 word still guessed");
}

#[test]
fn dragging_an_edge_retimes_the_word_in_one_undo_step() {
    let Setup { ctx, mut app, .. } = with_words();
    let r = word_box(&ctx, "vocal", "say");
    let before = words(&app);
    let steps = app.engine.undo_history().len();
    // Drag "say"'s end 40 px left, snapping off (Ctrl held).
    let from = r.right_center() - egui::vec2(1.0, 0.0);
    let to = from - egui::vec2(40.0, 0.0);
    let m = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    frame_m(&ctx, &mut app, vec![Event::PointerMoved(from)], m);
    frame_m(&ctx, &mut app, vec![button(from, true, m)], m);
    for k in 1..=6 {
        frame_m(&ctx, &mut app, vec![Event::PointerMoved(from.lerp(to, k as f32 / 6.0))], m);
    }
    frame_m(&ctx, &mut app, vec![button(to, false, m)], m);
    settle(&ctx, &mut app);
    let after = words(&app);
    assert!(after[1].2 < before[1].2 - secs(0.05), "{before:?} → {after:?}");
    assert_eq!(after[1].1, before[1].1, "the start stays");
    assert_eq!(app.engine.undo_history().len(), steps + 1, "one undo step");
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(words(&app), before);
}

#[test]
fn split_merge_and_retype_by_double_click() {
    let Setup { ctx, mut app, .. } = with_words();
    let t = app.engine.session().tracks[0].id;
    let c = app.engine.session().tracks[0].clips()[0].id;
    // Split "say" (what the menu's Split runs), then merge it back.
    app.run("lyrics.word_split", json!({"clip": c.0, "line": 0, "word": 1})).unwrap();
    settle(&ctx, &mut app);
    assert_eq!(drawn(&ctx, "vocal").len(), 3);
    app.run("lyrics.word_merge", json!({"clip": c.0, "line": 0, "word": 1})).unwrap();
    settle(&ctx, &mut app);
    assert_eq!(drawn(&ctx, "vocal").iter().map(|d| d.text.clone()).collect::<Vec<_>>(), ["Lord", "say"]);
    // Double-click "Lord", type over it, Enter.
    let at = word_box(&ctx, "vocal", "Lord").center();
    frame(&ctx, &mut app, vec![Event::PointerMoved(at)]);
    for _ in 0..2 {
        frame(&ctx, &mut app, vec![button(at, true, Modifiers::NONE)]);
        frame(&ctx, &mut app, vec![button(at, false, Modifiers::NONE)]);
    }
    settle(&ctx, &mut app);
    assert!(app.words.editing.is_some(), "retyping");
    frame(&ctx, &mut app, vec![Event::Text("Lord,".into())]);
    frame(&ctx, &mut app, key(Key::Enter, Modifiers::NONE));
    settle(&ctx, &mut app);
    let w = soundcraft_engine::word_time::track_words(app.engine.session(), t);
    assert_eq!(w[0].text, "Lord,");
    assert_eq!(w[0].timing, WordTiming::Confirmed, "retyped: confirmed");
}

#[test]
fn a_click_selects_the_word_its_notes_and_puts_the_playhead_there() {
    let Setup { ctx, mut app, .. } = with_words();
    click(&ctx, &mut app, word_box(&ctx, "vocal", "say").center());
    let s = app.engine.session();
    assert_eq!(s.edit.selection, soundcraft_time::Range::new(R / 4 + secs(0.7), R / 4 + secs(1.2)));
    assert_eq!(s.edit.playhead, R / 4 + secs(0.7));
    assert_eq!(app.vocal.selected.len(), 1, "the note under it");
    assert!(drawn(&ctx, "vocal")[1].selected);
}

#[test]
fn the_word_being_sung_lights_up() {
    let Setup { ctx, mut app, .. } = with_words();
    app.engine.transport.playing = true;
    app.engine.transport.position = R / 4 + secs(0.9);
    frame(&ctx, &mut app, vec![]);
    let d = drawn(&ctx, "vocal");
    assert!(app.engine.transport.playing, "no player here resets it");
    assert_eq!(d.iter().map(|d| d.lit).collect::<Vec<_>>(), [false, true]);
    app.engine.transport.playing = false;
    frame(&ctx, &mut app, vec![]);
    assert!(drawn(&ctx, "vocal").iter().all(|d| !d.lit));
}

#[test]
fn tap_mode_turns_taps_into_the_lines_words() {
    let Setup { ctx, mut app, .. } = setup();
    app.run("lyrics.sheet_set", json!({"text": "Lord say"})).unwrap();
    app.run("ui.lyrics_tap", json!({})).unwrap();
    assert!(app.words.tap.on && app.ui.show_lyrics);
    settle(&ctx, &mut app);
    let word: String = ctx.data(|d| d.get_temp(Id::new("soundcraft-tap-word"))).unwrap_or_default();
    assert_eq!(word, "Lord", "the word to tap, large");
    // Two taps (the transport where each lands), no latency, raw.
    app.words.tap.latency_ms = 0.0;
    app.words.tap.snap_ms = 0.0;
    for at in [R / 4 + secs(0.01), R / 4 + secs(0.72)] {
        app.engine.transport.position = at;
        app.engine.transport.playing = true;
        let t = ctx.input(|i| i.time);
        soundcraft_ui_egui::lyrics_tap::tap_down(&mut app, at, t);
        soundcraft_ui_egui::lyrics_tap::tap_up(&mut app, at, t);
    }
    app.engine.transport.playing = false;
    let w = words(&app);
    assert_eq!(
        w.iter().map(|w| (w.0.as_str(), w.1, w.3)).collect::<Vec<_>>(),
        [("Lord", R / 4 + secs(0.01), WordTiming::Confirmed), ("say", R / 4 + secs(0.72), WordTiming::Confirmed)]
    );
    assert!(w[0].2 <= w[1].1, "an end before the next start");
    // The J key itself: tap mode takes it before any shortcut.
    app.run("lyrics.sheet_set", json!({"text": "Lord say\nagain"})).unwrap();
    assert_eq!(app.words.tap.line, 1);
    app.engine.transport.position = R / 4 + secs(1.0);
    frame(&ctx, &mut app, key(soundcraft_ui_egui::lyrics_tap::TAP_KEY, Modifiers::NONE));
    assert_eq!(app.words.tap.line, 2, "the one-word line is tapped and sent");
    frame(&ctx, &mut app, key(Key::Escape, Modifiers::NONE));
    assert!(!app.words.tap.on);
}

#[test]
fn the_edit_window_strip_shows_and_selects_words() {
    let Setup { ctx, mut app, track, .. } = with_words();
    app.run("track.height", json!({"tracks": [track.0], "height": "large"})).ok();
    settle(&ctx, &mut app);
    let name = format!("strip-{}", track.0);
    let d = drawn(&ctx, &name);
    assert!(!d.is_empty(), "a large lane holds the strip");
    assert_eq!(d.iter().map(|d| d.text.as_str()).collect::<Vec<_>>(), ["Lord", "say"]);
    click(&ctx, &mut app, d[0].rect.center());
    assert_eq!(app.engine.session().edit.playhead, R / 4);
}

#[test]
fn the_ai_menu_times_and_taps() {
    let Setup { mut app, .. } = with_words();
    let tree = soundcraft_ui_egui::menus::tree();
    let ai = tree.iter().find(|n| n.label == "AI").unwrap();
    let extra = soundcraft_ui_egui::menus::ui_aliases();
    let ids: Vec<String> = ai.children.iter().filter_map(|c| soundcraft_ui_egui::menus::command_for_path(&c.path, &extra)).collect();
    assert!(ids.iter().any(|i| i == "lyrics.time_words") && ids.iter().any(|i| i == "ui.lyrics_tap"), "{ids:?}");
    // Greyed with a reason until lyrics are typed.
    assert!(soundcraft_ui_egui::menus::availability(&app, "ui.lyrics_tap").is_err());
    assert!(soundcraft_ui_egui::menus::availability(&app, "lyrics.time_words").is_err());
    app.run("lyrics.sheet_set", json!({"text": "Lord say"})).unwrap();
    assert!(soundcraft_ui_egui::menus::availability(&app, "ui.lyrics_tap").is_ok());
}
