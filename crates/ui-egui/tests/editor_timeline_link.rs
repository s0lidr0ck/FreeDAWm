//! The editors' timeline link (docs/superpowers/specs/2026-11-04-editor-timeline-link-design.md):
//! the 🔗 Link button (on by default), a linked editor following the playhead itself (Page,
//! Continuous, Centre) even with the Edit window hidden, a locate scrolling a linked editor and not
//! an unlinked one, Continuous and Centre on the main timeline, and the editor rulers drawn in the
//! main counter's format by the main ruler's code (tempo change and a 3/4 section included).

mod vocal_support;

use egui::pos2;
use serde_json::json;
use soundcraft_time::{BarBeat, Samples, TimeFormat};
use soundcraft_ui_egui::SoundApp;
use soundcraft_ui_egui::timeline_link;
use vocal_support::*;

const SEC: Samples = RATE as Samples;

/// Plays from `from` to `to`, moving the playhead `step` samples a frame (as playback does).
fn play_through(ctx: &egui::Context, app: &mut SoundApp, from: Samples, to: Samples, step: Samples) {
    if !app.is_playing() {
        app.run("transport.play", json!({"from": from})).unwrap();
    }
    let mut at = from;
    while at < to {
        app.run("transport.locate", json!({"at": at})).unwrap();
        frame(ctx, app, vec![]);
        at += step;
    }
}

fn stop(ctx: &egui::Context, app: &mut SoundApp) {
    app.run("transport.stop", json!({})).unwrap();
    settle(ctx, app);
}

/// Where the playhead sits across the roll (0 = left edge, 1 = right edge).
fn share(ctx: &egui::Context, app: &SoundApp) -> f64 {
    let v = view(ctx);
    (app.position() - v.t0) as f64 / (v.t1 - v.t0).max(1) as f64
}

fn locate(ctx: &egui::Context, app: &mut SoundApp, at: Samples) {
    app.run("edit.select", json!({"start": at})).unwrap();
    app.run("transport.locate", json!({"at": at})).unwrap();
    settle(ctx, app);
}

#[test]
fn the_link_button_is_on_by_default_and_toggles() {
    let Setup { ctx, mut app, .. } = setup();
    assert!(soundcraft_ui_egui::vocal_editor::linked(&app), "linked by default");
    assert_eq!(app.run("ui.vocal_editor", json!({"linked": false})).unwrap()["linked"], false);
    settle(&ctx, &mut app);
    assert!(!soundcraft_ui_egui::vocal_editor::linked(&app));
    click(&ctx, &mut app, control(&ctx, "link").expect("the Link button").center());
    assert!(soundcraft_ui_egui::vocal_editor::linked(&app), "the button links it again");
    // The MIDI Editor's is its own.
    assert_eq!(app.run("ui.midi_editor", json!({})).unwrap()["linked"], true);
    app.run("ui.midi_editor", json!({"linked": false})).unwrap();
    assert_eq!(app.run("ui.midi_editor", json!({})).unwrap()["linked"], false);
    assert!(soundcraft_ui_egui::vocal_editor::linked(&app));
}

#[test]
fn a_locate_far_away_scrolls_a_linked_editor_and_not_an_unlinked_one() {
    let Setup { ctx, mut app, .. } = setup();
    let far = 40 * SEC;
    locate(&ctx, &mut app, far);
    let v = view(&ctx);
    assert!(v.t0 <= far && far <= v.t1, "linked: the playhead shows: {}..{} holds {far}", v.t0, v.t1);
    // Unlinked: a locate leaves it where it is, and the Edit window's scroll does not move it.
    app.run("ui.vocal_editor", json!({"linked": false})).unwrap();
    locate(&ctx, &mut app, 0);
    let before = view(&ctx).t0;
    assert!(before > 0);
    locate(&ctx, &mut app, 90 * SEC);
    app.run("view.scroll", json!({"to": 10 * SEC})).unwrap();
    settle(&ctx, &mut app);
    assert_eq!(view(&ctx).t0, before, "unlinked: it stays where it was put");
}

#[test]
fn a_linked_editor_follows_while_playing_with_the_edit_window_hidden() {
    let Setup { ctx, mut app, .. } = setup();
    app.run("options.scrolling", json!({"mode": "page"})).unwrap();
    // The editor in its own window, the Mix window in front: the Edit window is not drawn.
    app.run("window.vocal_separate", json!({})).unwrap();
    app.run("window.mix", json!({})).unwrap();
    settle(&ctx, &mut app);
    let edit_scroll = app.engine.session().edit.zoom.scroll;
    play_through(&ctx, &mut app, 0, 30 * SEC, SEC / 10);
    let v = view(&ctx);
    let pos = app.position();
    assert!(v.t0 <= pos && pos <= v.t1, "the playhead stays in view: {}..{} holds {pos}", v.t0, v.t1);
    let s = share(&ctx, &app);
    assert!((0.05..=0.95).contains(&s), "Page: between the turn and the landing: {s}");
    assert_eq!(app.engine.session().edit.zoom.scroll, edit_scroll, "the hidden Edit window did not scroll: the editor followed by itself");
    stop(&ctx, &mut app);
}

#[test]
fn page_continuous_and_centre_place_the_playhead_in_the_editor() {
    for (mode, lo, hi) in [("continuous", 0.30, 0.37), ("center", 0.47, 0.53)] {
        let Setup { ctx, mut app, .. } = setup();
        app.run("options.scrolling", json!({"mode": mode})).unwrap();
        play_through(&ctx, &mut app, 0, 20 * SEC, SEC / 20);
        frame(&ctx, &mut app, vec![]);
        let s = share(&ctx, &app);
        assert!((lo..=hi).contains(&s), "{mode}: the playhead at {s} of the roll");
        stop(&ctx, &mut app);
    }
    // Page: the playhead passes 90 % and lands at 10 %.
    let Setup { ctx, mut app, .. } = setup();
    app.run("options.scrolling", json!({"mode": "page"})).unwrap();
    let mut shares = Vec::new();
    app.run("transport.play", json!({"from": 0})).unwrap();
    let mut at = 0;
    while at < 20 * SEC {
        app.run("transport.locate", json!({"at": at})).unwrap();
        frame(&ctx, &mut app, vec![]);
        shares.push(share(&ctx, &app));
        at += SEC / 20;
    }
    assert!(shares.iter().all(|s| (0.0..=0.95).contains(s)), "never past the turn: {shares:?}");
    assert!(shares.iter().any(|s| (0.09..=0.13).contains(s)) && shares.iter().any(|s| *s > 0.85), "it turned pages: {shares:?}");
    stop(&ctx, &mut app);
    // None: it stays.
    let Setup { ctx, mut app, .. } = setup();
    app.run("options.scrolling", json!({"mode": "none"})).unwrap();
    let t0 = view(&ctx).t0;
    play_through(&ctx, &mut app, 0, 10 * SEC, SEC / 20);
    assert_eq!(view(&ctx).t0, t0, "no scrolling: the editor stays while playing");
}

#[test]
fn continuous_and_centre_scroll_the_main_timeline() {
    for (mode, lo, hi) in [("continuous", 0.30, 0.37), ("center", 0.47, 0.53)] {
        let Setup { ctx, mut app, .. } = setup();
        app.ui.show_vocal_editor = false;
        app.run("options.scrolling", json!({"mode": mode})).unwrap();
        play_through(&ctx, &mut app, 0, 20 * SEC, SEC / 20);
        frame(&ctx, &mut app, vec![]);
        let tl = app.edit_layout.timeline;
        let z = &app.engine.session().edit.zoom;
        let span = f64::from(tl[2] - tl[0]) * z.samples_per_px;
        let s = (app.position() - z.scroll) as f64 / span;
        assert!((lo..=hi).contains(&s), "{mode}: the playhead at {s} of the Edit window");
        stop(&ctx, &mut app);
    }
}

/// A session with a tempo change (120 to 90 BPM at bar 3) and a 3/4 section from bar 5, the
/// Edit window showing every timebase ruler across 24 s, the Vocal Editor at 1×.
fn tempo_and_meter() -> Setup {
    let mut set = setup();
    let app = &mut set.app;
    app.run("event.tempo", json!({"bpm": 90.0, "at": 4 * SEC})).unwrap();
    let bar5 = {
        let s = app.engine.session();
        s.tempo.samples_at_bar_beat(BarBeat { bar: 5, beat: 1, tick: 0 }, s.sample_rate)
    };
    app.run("event.meter", json!({"numerator": 3, "denominator": 4, "at": bar5})).unwrap();
    let w = (app.edit_layout.timeline[2] - app.edit_layout.timeline[0]).max(100.0);
    let s = app.engine.session_mut();
    s.edit.zoom.samples_per_px = 24.0 * RATE as f64 / f64::from(w);
    s.edit.zoom.scroll = 0;
    s.edit.rulers = ["bars_beats", "min_secs", "timecode", "feet_frames", "samples"].iter().map(|r| r.to_string()).collect();
    soundcraft_ui_egui::vocal_editor::set_zoom_factor(app, 1.0);
    settle(&set.ctx, &mut set.app);
    set
}

fn labels(ctx: &egui::Context, key: &str) -> Vec<(Samples, String)> {
    ctx.data(|d| d.get_temp(egui::Id::new(key))).unwrap_or_default()
}

#[test]
fn editor_ruler_labels_equal_the_main_rulers_for_each_format() {
    let Setup { ctx, mut app, .. } = tempo_and_meter();
    for f in TimeFormat::ALL {
        app.run("view.main_counter", json!({"format": f.id()})).unwrap();
        settle(&ctx, &mut app);
        let main = labels(&ctx, &format!("soundcraft-ruler-edit-{f:?}"));
        let vocal = labels(&ctx, "soundcraft-ruler-vocal");
        assert!(vocal.len() >= 3, "{f:?}: the Vocal Editor's ruler labels: {vocal:?}");
        let end = main.last().map_or(0, |l| l.0).min(vocal.last().map_or(0, |l| l.0));
        let upto = |v: &[(Samples, String)]| v.iter().filter(|l| l.0 <= end).cloned().collect::<Vec<_>>();
        assert_eq!(upto(&vocal), upto(&main), "{f:?}: the Vocal Editor's ruler is the main ruler's");
    }
    // Bars|Beats crosses the tempo change and the 3/4 bars: bar n starts where the tempo map says.
    app.run("view.main_counter", json!({"format": "bars_beats"})).unwrap();
    settle(&ctx, &mut app);
    let vocal = labels(&ctx, "soundcraft-ruler-vocal");
    let s = app.engine.session();
    for (at, text) in &vocal {
        let bar: i64 = text.parse().expect("a bar number");
        assert_eq!(*at, s.tempo.samples_at_bar_beat(BarBeat { bar, beat: 1, tick: 0 }, s.sample_rate), "bar {bar}");
    }
    let bar = |n| s.tempo.samples_at_bar_beat(BarBeat { bar: n, beat: 1, tick: 0 }, s.sample_rate);
    assert_eq!(bar(4) - bar(3), 8 * SEC / 3, "4/4 at 90 BPM");
    assert_eq!(bar(6) - bar(5), 2 * SEC, "3/4 at 90 BPM");
    assert!(vocal.iter().any(|l| l.1 == "6"), "the ruler reaches the 3/4 section: {vocal:?}");
    // Min:Secs reads as the main counter does.
    app.run("view.main_counter", json!({"format": "min_secs"})).unwrap();
    settle(&ctx, &mut app);
    assert!(labels(&ctx, "soundcraft-ruler-vocal").iter().any(|l| l.1 == "0:02"), "Min:Secs labels");
}

#[test]
fn the_vocal_ruler_click_locates_in_any_format() {
    let Setup { ctx, mut app, .. } = tempo_and_meter();
    app.run("view.main_counter", json!({"format": "bars_beats"})).unwrap();
    settle(&ctx, &mut app);
    let ruler = control(&ctx, "ruler").unwrap();
    let v = view(&ctx);
    let at = 6 * SEC;
    click(&ctx, &mut app, pos2(v.x(at), ruler.center().y));
    assert!((app.position() - at).abs() < 2_000, "located: {}", app.position());
}

// ---- the MIDI Editor ---------------------------------------------------------------------------

/// The demo's Keys clip in the MIDI Editor.
fn midi() -> (egui::Context, SoundApp) {
    let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, soundcraft_ui_egui::Services::default());
    app.ui.show_midi_editor = true;
    app.ui.show_vocal_editor = false;
    app.run("edit.select", json!({"tracks": ["Keys"]})).unwrap();
    let ctx = egui::Context::default();
    for _ in 0..5 {
        frame(&ctx, &mut app, vec![]);
    }
    (ctx, app)
}

fn midi_ruler(ctx: &egui::Context) -> egui::Rect {
    ctx.data(|d| d.get_temp(egui::Id::new("soundcraft-midi-ruler"))).expect("the MIDI ruler")
}

/// The clip's span and its first tick.
fn keys_clip(app: &SoundApp) -> (Samples, Samples, i64) {
    let s = app.engine.session();
    let cid = app.midi.clip.expect("a clip in the editor");
    let (_, c) = s.find_clip(cid).unwrap();
    (c.start, c.end(), s.tempo.samples_to_ticks(c.start, s.sample_rate))
}

/// The time under `x` in the MIDI roll.
fn midi_time(app: &SoundApp, x: f32) -> Samples {
    let geo = app.midi.drawn.unwrap();
    let (_, _, base) = keys_clip(app);
    let s = app.engine.session();
    s.tempo.tick_to_samples(base + geo.tick_at(x).round() as i64, s.sample_rate)
}

#[test]
fn the_midi_editor_has_a_ruler_in_the_main_format_that_locates() {
    let (ctx, mut app) = midi();
    for f in ["bars_beats", "min_secs", "timecode"] {
        app.run("view.main_counter", json!({"format": f})).unwrap();
        settle(&ctx, &mut app);
        let got = labels(&ctx, "soundcraft-ruler-midi");
        assert!(!got.is_empty(), "{f}: labels");
        let s = app.engine.session();
        for (at, text) in &got {
            assert_eq!(*text, timeline_link::label(s, *at, s.edit.main_counter, s.frame_rate), "{f}: the main ruler's label at {at}");
        }
    }
    // A click on the ruler locates; the editor keeps its clip.
    let cid = app.midi.clip;
    let r = midi_ruler(&ctx);
    let x = r.left() + r.width() * 0.6;
    let want = midi_time(&app, x);
    click(&ctx, &mut app, pos2(x, r.center().y));
    assert!((app.position() - want).abs() < 2_000, "located at the click: {} vs {want}", app.position());
    assert_eq!(app.midi.clip, cid, "still on the same clip");
}

#[test]
fn a_zoomed_linked_midi_editor_follows_the_playhead_and_an_unlinked_one_stays() {
    for linked in [true, false] {
        let (ctx, mut app) = midi();
        app.run("ui.midi_editor", json!({"linked": linked})).unwrap();
        app.run("options.scrolling", json!({"mode": "page"})).unwrap();
        // Zoomed in, at the clip's start.
        let roll = app.midi.drawn.unwrap().roll;
        for _ in 0..4 {
            frame(
                &ctx,
                &mut app,
                vec![
                    egui::Event::PointerMoved(pos2(roll.left() + 2.0, roll.center().y)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Line,
                        delta: egui::vec2(0.0, 3.0),
                        phase: egui::TouchPhase::Move,
                        modifiers: egui::Modifiers::COMMAND,
                    },
                ],
            );
        }
        settle(&ctx, &mut app);
        let before = app.midi.drawn.unwrap();
        assert!(before.px_per_tick > before.fit * 1.5, "zoomed in");
        let (start, end, _) = keys_clip(&app);
        let shown_end = midi_time(&app, roll.right());
        assert!(shown_end < end, "part of the clip shows");
        let target = (shown_end + (end - shown_end) / 2).min(end - 1);
        play_through(&ctx, &mut app, start, target, SEC / 20);
        let (a, b) = (midi_time(&app, roll.left()), midi_time(&app, roll.right()));
        let pos = app.position();
        if linked {
            assert!(a <= pos && pos <= b, "linked: the playhead shows: {a}..{b} holds {pos}");
        } else {
            assert_eq!(app.midi.drawn.unwrap().left, before.left, "unlinked: the view stays");
        }
        stop(&ctx, &mut app);
    }
}
