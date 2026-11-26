//! The Gild panel (the Vocal Editor's right panel) and the Gild strip: the untimed-words message,
//! selecting words and turning effects on (gilding them), the cards, the marks on the strip and
//! the gold foot on gilded words, copy and paste, dragging a handle (one undo step), and Remove
//! All asking first.

mod vocal_support;

use egui::{Id, Rect};
use serde_json::json;
use soundcraft_model::{LyricLine, LyricWord, WordTiming};
use soundcraft_time::Samples;
use vocal_support::*;

const R: i64 = RATE as i64;

fn secs(x: f64) -> Samples {
    (x * R as f64).round() as Samples
}

/// [`setup`] with two confirmed words over its two notes, the Gild panel open.
fn with_words() -> Setup {
    let mut st = setup();
    let c = st.clips[0];
    {
        let s = st.app.engine.session_mut();
        let clip = s.find_clip_mut(c).unwrap();
        let mut a = LyricWord::new(0, secs(0.5), "Lord");
        a.timing = WordTiming::Confirmed;
        let mut b = LyricWord::new(secs(0.7), secs(1.2), "say");
        b.timing = WordTiming::Confirmed;
        clip.lyrics = vec![LyricLine { start: 0, end: secs(1.2), words: vec![a, b], sheet_line: None }];
    }
    st.app.run("ui.gild", json!({"value": true})).unwrap();
    settle(&st.ctx, &mut st.app);
    st
}

fn gild<T: Clone + Send + Sync + 'static>(ctx: &egui::Context, name: &str) -> Option<T> {
    ctx.data(|d| d.get_temp::<T>(Id::new(format!("soundcraft-gild-{name}"))))
}

fn word_box(ctx: &egui::Context, text: &str) -> Rect {
    let d: Vec<soundcraft_ui_egui::word_lane::Drawn> = ctx.data(|d| d.get_temp(Id::new("soundcraft-words-vocal-drawn"))).unwrap_or_default();
    d.into_iter().find(|d| d.text == text).unwrap().rect
}

fn effect_button(ctx: &egui::Context, kind: &str) -> Rect {
    let b: Vec<(String, Rect, bool)> = gild(ctx, "buttons").unwrap();
    b.into_iter().find(|b| b.0 == kind).unwrap().1
}

#[test]
fn untimed_tracks_say_to_time_the_words_first() {
    let Setup { ctx, mut app, .. } = setup();
    app.run("ui.gild", json!({"value": true})).unwrap();
    settle(&ctx, &mut app);
    assert!(gild::<Rect>(&ctx, "panel").is_some(), "the Gild panel shows in Tuning's place");
    let msg: String = gild(&ctx, "message").unwrap();
    assert!(msg.starts_with("Time the words first"), "{msg}");
}

#[test]
fn selecting_a_word_and_turning_on_a_delay_gilds_it() {
    let Setup { ctx, mut app, track, .. } = with_words();
    let msg: String = gild(&ctx, "message").unwrap();
    assert!(msg.is_empty());
    click(&ctx, &mut app, word_box(&ctx, "say").center());
    let summary: String = gild(&ctx, "summary").unwrap();
    assert!(summary.contains("not gilded"), "{summary}");
    let steps = app.engine.undo_history().len();
    click(&ctx, &mut app, effect_button(&ctx, "delay").center());
    let c = &app.engine.session().track(track).unwrap().clips()[0];
    assert_eq!(c.gild.len(), 1);
    assert!(c.gild[0].fx.delay.is_some());
    assert_eq!(app.engine.undo_history().len(), steps + 1, "one undo step: {:?}", app.engine.undo_history());
    settle(&ctx, &mut app);
    // The card, the strip with its mark, the gold foot on the word.
    let cards: Vec<(String, Rect)> = gild(&ctx, "cards").unwrap();
    assert!(cards.iter().any(|c| c.0 == "delay"));
    let bars: Vec<(u32, Rect, String)> = gild(&ctx, "bars").unwrap();
    assert_eq!(bars.len(), 1);
    assert_eq!(bars[0].2, "Dl");
    let feet: Vec<Rect> = ctx.data(|d| d.get_temp(Id::new("soundcraft-words-vocal-gilded"))).unwrap();
    assert_eq!(feet.len(), 1);
    assert!((feet[0].center().x - word_box(&ctx, "say").center().x).abs() < 1.0);
    // A second effect stacks; turning it off again leaves the delay.
    click(&ctx, &mut app, effect_button(&ctx, "reverb").center());
    assert_eq!(app.engine.session().track(track).unwrap().clips()[0].gild[0].fx.kinds(), vec!["delay", "reverb"]);
    let bars: Vec<(u32, Rect, String)> = gild(&ctx, "bars").unwrap();
    assert_eq!(bars[0].2, "Dl Rv");
    click(&ctx, &mut app, effect_button(&ctx, "reverb").center());
    assert_eq!(app.engine.session().track(track).unwrap().clips()[0].gild[0].fx.kinds(), vec!["delay"]);
    // Off on the last effect: the word is no longer gilded.
    click(&ctx, &mut app, effect_button(&ctx, "delay").center());
    assert!(app.engine.session().track(track).unwrap().clips()[0].gild.is_empty());
}

#[test]
fn copy_paste_and_a_handle_drag_in_one_step() {
    let Setup { ctx, mut app, track, clips } = with_words();
    let c = clips[0];
    app.run("gild.add", json!({"words": [{"clip": c.0, "line": 0, "word": 1}], "fx": {"double": true, "delay": {"note": "1/8"}}})).unwrap();
    app.run("gild.copy", json!({"clip": c.0, "id": 1})).unwrap();
    settle(&ctx, &mut app);
    click(&ctx, &mut app, word_box(&ctx, "Lord").center());
    click(&ctx, &mut app, gild::<Rect>(&ctx, "paste").unwrap().center());
    let gs = &app.engine.session().track(track).unwrap().clips()[0].gild;
    assert_eq!(gs.len(), 2);
    assert_eq!(gs[0].fx, gs[1].fx, "pasted");
    // One gilding selected: drag its end handle right on the strip.
    let bars: Vec<(u32, Rect, String)> = gild(&ctx, "bars").unwrap();
    let lord = bars.iter().find(|b| b.0 == 2).unwrap().1;
    let before = app.engine.session().track(track).unwrap().clips()[0].gild.iter().find(|g| g.id == 2).unwrap().end;
    let steps = app.engine.undo_history().len();
    drag(&ctx, &mut app, lord.right_center(), lord.right_center() + egui::vec2(20.0, 0.0), 5);
    let g = app.engine.session().track(track).unwrap().clips()[0].gild.iter().find(|g| g.id == 2).unwrap().clone();
    assert!(g.end > before, "the end handle moved");
    assert!(g.held, "moved by hand: it keeps its own times");
    assert_eq!(app.engine.undo_history().len(), steps + 1, "one undo step");
}

#[test]
fn remove_all_asks_first() {
    let Setup { ctx, mut app, track, clips } = with_words();
    app.run("gild.add", json!({"words": [{"clip": clips[0].0, "line": 0, "word": 0}], "fx": {"reverb": true}})).unwrap();
    settle(&ctx, &mut app);
    click(&ctx, &mut app, gild::<Rect>(&ctx, "remove-all").unwrap().center());
    assert_eq!(gild::<usize>(&ctx, "confirm"), Some(1), "it asks, naming how many");
    assert_eq!(app.engine.session().track(track).unwrap().clips()[0].gild.len(), 1, "nothing removed yet");
    app.vocal.gild_confirm = false;
    // The menu's Remove All Gild on Track… opens the same question.
    app.run("ui.gild_clear", json!({})).unwrap();
    assert!(app.vocal.gild_confirm);
}
