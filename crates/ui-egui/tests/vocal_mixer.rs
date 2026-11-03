//! The Mix window's PITCH section (plan 4 Task 2): the readout of the note under the playhead,
//! the Centre note / Flatten drift steps, the double-click into the editor, and the row lining
//! up across vocal and non-vocal strips. The compact pitch box: `pitch_compact.rs`.

mod vocal_support;

use egui::{Pos2, Rect, pos2, vec2};
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_engine::vocal::NoteRef;
use soundcraft_model::{ChannelFormat, Session, TrackId, TrackKind};
use soundcraft_time::{Range, Samples};
use soundcraft_ui_egui::vocal_mixer::pitch_window;
use soundcraft_ui_egui::{MainWindow, Services, SoundApp};
use vocal_support::*;

struct Mix {
    ctx: egui::Context,
    app: SoundApp,
    vox: TrackId,
    keys: TrackId,
}

/// Lead Vox (plan 2's two-note take at 0.25 s, analysed) beside a non-vocal "Keys" audio track,
/// the Mix window shown.
fn mix() -> Mix {
    let mut s = Session::default();
    let keys = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Keys"));
    let vox = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead Vox"));
    let clip = place(&mut s, vox, take2(), i64::from(RATE) / 4);
    let mut e = Engine::new(s);
    soundcraft_engine::vocal::run_until_idle(&mut e, &[clip]).unwrap();
    let mut app = SoundApp::new(e, None, Services::default());
    app.ui.window = MainWindow::Mix;
    // These tests drive the full Pitch panel (the expanded pitch box).
    app.run("mix.pitch_expanded", json!({"track": vox.0, "expanded": true})).unwrap();
    let ctx = egui::Context::default();
    let mut m = Mix { ctx, app, vox, keys };
    settle(&m.ctx, &mut m.app);
    m
}

fn at(m: &mut Mix, p: Samples) {
    m.app.engine.session_mut().edit.selection = Range::new(p, p);
    settle(&m.ctx, &mut m.app);
}

fn ctl(m: &Mix, t: TrackId, name: &str) -> Rect {
    control(&m.ctx, &format!("mix-{}-{name}", t.0)).unwrap_or_else(|| panic!("no {name} on track {}", t.0))
}

/// Clicks the Lead Vox strip's `name` button.
fn push(m: &mut Mix, name: &str) {
    let r = ctl(m, m.vox, name);
    click(&m.ctx, &mut m.app, r.center());
}

fn readout(m: &Mix) -> String {
    text(&m.ctx, &format!("mix-{}-readout", m.vox.0)).unwrap_or_default()
}

fn n1(m: &Mix) -> NoteRef {
    notes(&m.app, m.vox)[0]
}

fn mid(m: &Mix, r: NoteRef) -> Samples {
    let (a, b) = span(&m.app, r);
    (a + b) / 2
}

fn edit_of(m: &Mix, r: NoteRef) -> Option<soundcraft_vocal::NoteEdit> {
    let s = m.app.engine.session();
    s.find_clip(r.clip).and_then(|(_, c)| c.tune.as_ref()).and_then(|t| t.edits.get(&r.note).cloned())
}

#[test]
fn readout_names_the_note_under_the_playhead() {
    let mut m = mix();
    let p = mid(&m, n1(&m));
    at(&mut m, p);
    assert_eq!(readout(&m), "A3 \u{2212}30c");
}

#[test]
fn centre_note_is_one_40_percent_step() {
    let mut m = mix();
    let r = n1(&m);
    let p = mid(&m, r);
    at(&mut m, p);
    let before = m.app.engine.undo_history().len();
    push(&mut m, "centre");
    let e = edit_of(&m, r).expect("an edit");
    assert!((e.centre - 0.40).abs() < 1e-4, "{}", e.centre);
    assert_eq!(m.app.engine.undo_history().len(), before + 1);
    assert_eq!(m.app.engine.undo_label(), Some("Centre Note"));
    assert!(m.app.vocal.selected.is_empty(), "the editor's selection is left alone");
}

#[test]
fn flatten_drift_is_one_30_percent_step() {
    let mut m = mix();
    let r = n1(&m);
    let p = mid(&m, r);
    at(&mut m, p);
    push(&mut m, "drift");
    let e = edit_of(&m, r).expect("an edit");
    assert!((e.drift - 0.30).abs() < 1e-4, "{}", e.drift);
    assert!(e.centre.abs() < 1e-6);
}

#[test]
fn two_presses_compound() {
    let mut m = mix();
    let r = n1(&m);
    let p = mid(&m, r);
    at(&mut m, p);
    let before = m.app.engine.undo_history().len();
    push(&mut m, "centre");
    // The playhead moves a little within the note: the same note again.
    at(&mut m, p + i64::from(RATE) / 20);
    push(&mut m, "centre");
    let e = edit_of(&m, r).expect("an edit");
    assert!((e.centre - 0.64).abs() < 1e-4, "{}", e.centre);
    assert_eq!(m.app.engine.undo_history().len(), before + 2);
}

#[test]
fn buttons_disabled_without_a_note() {
    let mut m = mix();
    // Note 2 ends at 1.45 s: 2.2 s is more than 0.5 s after the last note.
    at(&mut m, (2.2 * f64::from(RATE)) as Samples);
    assert_eq!(readout(&m), "\u{2014}");
    let before = m.app.engine.undo_history().len();
    push(&mut m, "centre");
    push(&mut m, "drift");
    assert_eq!(m.app.engine.undo_history().len(), before);
    assert!(m.app.engine.session().find_clip(n1(&m).clip).unwrap().1.tune.is_none());
}

#[test]
fn double_click_opens_the_editor_at_that_spot() {
    let mut m = mix();
    let r = n1(&m);
    let p = (1.5 * f64::from(RATE)) as Samples;
    at(&mut m, p);
    let graph = ctl(&m, m.vox, "graph");
    let w = pitch_window(graph, m.app.engine.session(), m.vox, p).expect("a graph");
    let bar = w.bars.iter().find(|b| b.r == r).expect("note 1 in the window").rect;
    let pt = pos2(bar.center().x, graph.center().y);
    for _ in 0..2 {
        frame(&m.ctx, &mut m.app, vec![egui::Event::PointerMoved(pt), button(pt, true, egui::Modifiers::NONE)]);
        frame(&m.ctx, &mut m.app, vec![button(pt, false, egui::Modifiers::NONE)]);
    }
    settle(&m.ctx, &mut m.app);
    assert!(m.app.ui.show_vocal_editor);
    assert_eq!(m.app.vocal.selected.iter().copied().collect::<Vec<_>>(), vec![r]);
}

#[test]
fn rows_stay_lined_up() {
    let mut m = mix();
    let keys = ctl(&m, m.keys, "sends_ae");
    let vox = ctl(&m, m.vox, "sends_ae");
    assert!((keys.min.y - vox.min.y).abs() <= 0.5, "Keys {} vs Lead Vox {}", keys.min.y, vox.min.y);
    assert!(control(&m.ctx, &format!("mix-{}-graph", m.keys.0)).is_none(), "Keys draws nothing in the row");
    // With no vocal track shown the strips are as without the Pitch view.
    m.app.run("track.vocal", json!({"tracks": [m.vox.0], "value": false})).unwrap();
    settle(&m.ctx, &mut m.app);
    let with_view = ctl(&m, m.keys, "sends_ae").min.y;
    m.app.ui.mix_views.retain(|v| v != "pitch");
    settle(&m.ctx, &mut m.app);
    let without = ctl(&m, m.keys, "sends_ae").min.y;
    assert!((with_view - without).abs() <= 0.5, "{with_view} vs {without}");
    assert!(with_view < keys.min.y, "the row was reserved while the vocal strip showed");
}

#[test]
fn graph_follows_the_playhead() {
    let m = mix();
    let s = m.app.engine.session();
    let graph = Rect::from_min_size(Pos2::ZERO, vec2(140.0, 72.0));
    let p = mid(&m, n1(&m));
    let a = pitch_window(graph, s, m.vox, p).unwrap();
    let b = pitch_window(graph, s, m.vox, p + i64::from(RATE)).unwrap();
    assert_eq!(b.t0 - a.t0, i64::from(RATE));
    let x = a.playhead_x(p);
    assert!((x - 0.87 * 140.0).abs() < 1.0, "{x}");
    assert_eq!(a.current.as_ref().map(|c| c.note), Some(n1(&m)));
    assert_eq!(b.current.as_ref().map(|c| c.note), Some(notes(&m.app, m.vox)[1]));
}

#[test]
fn pitch_view_toggles_from_the_menu() {
    let mut m = mix();
    let path = "View > Mix Window Views > Pitch";
    let id = soundcraft_ui_egui::menus::command_for_path(path, &soundcraft_ui_egui::menus::ui_aliases()).expect("the menu item");
    assert!(m.app.ui.mix_views.iter().any(|v| v == "pitch"), "on by default");
    soundcraft_ui_egui::menus::invoke_menu(&mut m.app, &id, path);
    assert!(!m.app.ui.mix_views.iter().any(|v| v == "pitch"));
    soundcraft_ui_egui::menus::invoke_menu(&mut m.app, &id, path);
    assert!(m.app.ui.mix_views.iter().any(|v| v == "pitch"));
}

#[test]
fn step_is_given_a_time_inside_the_note() {
    let m = mix();
    let r = n1(&m);
    let (a, b) = span(&m.app, r);
    let graph = Rect::from_min_size(Pos2::ZERO, vec2(140.0, 72.0));
    // Just inside the note's start: the step still aims at its middle, not at a frame edge.
    let w = pitch_window(graph, m.app.engine.session(), m.vox, a + 1).unwrap();
    let c = w.current.expect("the sounding note");
    assert_eq!(c.note, r);
    assert_eq!(c.at, (a + b) / 2);
}

#[test]
fn fader_moves_rebuild_no_analysis() {
    let mut m = mix();
    let p = mid(&m, n1(&m));
    at(&mut m, p);
    let before = soundcraft_ui_egui::vocal_editor::analysis_copies();
    for db in [-3.0, -4.0, -5.0] {
        m.app.run("mix.volume", json!({"track": m.vox.0, "db": db})).unwrap();
        frame(&m.ctx, &mut m.app, vec![]);
    }
    assert_eq!(soundcraft_ui_egui::vocal_editor::analysis_copies() - before, 0);
    assert_eq!(readout(&m), "A3 \u{2212}30c");
}

#[test]
fn stretched_clip_draws_no_graph_and_says_why() {
    let mut m = mix();
    let r = n1(&m);
    let p = mid(&m, r);
    m.app.engine.session_mut().find_clip_mut(r.clip).unwrap().stretch = 1.5;
    m.app.engine.revision += 1;
    at(&mut m, p);
    let graph = ctl(&m, m.vox, "graph");
    assert!(pitch_window(graph, m.app.engine.session(), m.vox, p).is_none(), "no bars of a stretched clip");
    assert_eq!(text(&m.ctx, &format!("mix-{}-why", m.vox.0)).as_deref(), Some("This clip is stretched: it plays untuned and can't be tuned"));
    let before = m.app.engine.undo_history().len();
    push(&mut m, "centre");
    assert_eq!(m.app.engine.undo_history().len(), before);
}

#[test]
fn disabled_buttons_say_why() {
    let mut m = mix();
    at(&mut m, (2.2 * f64::from(RATE)) as Samples);
    assert_eq!(text(&m.ctx, &format!("mix-{}-why", m.vox.0)).as_deref(), Some("no note"));
    let p = mid(&m, n1(&m));
    at(&mut m, p);
    assert_eq!(text(&m.ctx, &format!("mix-{}-why", m.vox.0)).as_deref(), Some(""));
}

#[test]
fn analysing_only_when_an_unanalysed_clip_meets_the_window() {
    let mut s = Session::default();
    let vox = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead Vox"));
    // Unanalysed, at 10 s.
    place(&mut s, vox, take2(), 10 * i64::from(RATE));
    assert!(!soundcraft_ui_egui::vocal_mixer::waiting(&s, vox, i64::from(RATE)), "far before the clip");
    assert!(soundcraft_ui_egui::vocal_mixer::waiting(&s, vox, 10 * i64::from(RATE) + 100), "over it");
    assert!(!soundcraft_ui_egui::vocal_mixer::waiting(&s, vox, 30 * i64::from(RATE)), "long after it");
    use soundcraft_ui_egui::vocal_mixer::disabled_reason;
    assert_eq!(disabled_reason(None, false, true, false), Some("Analysing\u{2026}"));
    assert_eq!(disabled_reason(None, false, false, false), Some("no note"));
    assert_eq!(disabled_reason(None, true, false, false), Some("This clip is stretched: it plays untuned and can't be tuned"));
    assert_eq!(disabled_reason(None, false, false, true), Some("Can't be analysed (missing audio, or longer than 30 min)"));
}

#[test]
fn missing_audio_is_not_analysing_forever() {
    let mut s = Session::default();
    let vox = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead Vox"));
    let clip = place(&mut s, vox, take2(), i64::from(RATE));
    let src = s.find_clip(clip).and_then(|(_, c)| c.source()).unwrap();
    s.pool.remove(src);
    let over = i64::from(RATE) + 100;
    assert!(!soundcraft_ui_egui::vocal_mixer::waiting(&s, vox, over), "the worker never offers it");
    assert!(soundcraft_ui_egui::vocal_mixer::cant_analyse(&s, vox, over));
    let mut app = SoundApp::new(Engine::new(s), None, Services::default());
    app.ui.window = MainWindow::Mix;
    app.run("mix.pitch_expanded", json!({"track": vox.0, "expanded": true})).unwrap();
    let ctx = egui::Context::default();
    app.engine.session_mut().edit.selection = Range::new(over, over);
    settle(&ctx, &mut app);
    assert_eq!(text(&ctx, &format!("mix-{}-why", vox.0)).as_deref(), Some("Can't be analysed (missing audio, or longer than 30 min)"));
}

#[test]
fn a_stretched_neighbour_does_not_disable_the_current_notes_buttons() {
    let mut m = mix();
    let r = notes(&m.app, m.vox)[1];
    let (_, end) = span(&m.app, r);
    let first = r.clip;
    let clip_end = m.app.engine.session().find_clip(first).unwrap().1.end();
    // A stretched clip right after the take: the playhead over it, the take's last note just
    // ended (the current note).
    let s = m.app.engine.session_mut();
    let other = place(s, m.vox, take2(), clip_end);
    s.find_clip_mut(other).unwrap().stretch = 1.5;
    m.app.engine.revision += 1;
    let p = clip_end + i64::from(RATE) / 10;
    assert!(p - end < i64::from(RATE) / 2, "the note ended within 0.5 s");
    at(&mut m, p);
    assert_eq!(text(&m.ctx, &format!("mix-{}-why", m.vox.0)).as_deref(), Some(""), "the current note is not on the stretched clip");
    let before = m.app.engine.undo_history().len();
    push(&mut m, "centre");
    assert_eq!(m.app.engine.undo_history().len(), before + 1, "Centre note acts on it");
    assert!(edit_of(&m, r).is_some_and(|e| e.centre > 0.0));
}
