//! The Vocal Editor's view: the dock tab and the ways in (Tune Vocal…, the clip menu, `open_at`),
//! the empty states, the worker's focus, and the roll's selection, keys, split/join and scoops.

mod vocal_support;

use serde_json::json;
use soundcraft_model::{ChannelFormat, Session, TrackKind};
use soundcraft_ui_egui::docking::Panel;
use soundcraft_ui_egui::vocal_editor::{self, focus_clip, open_at};
use soundcraft_ui_egui::{Services, SoundApp};
use vocal_support::*;

#[test]
fn tune_vocal_menu_opens_the_vocal_editor_on_the_clip() {
    let Setup { mut app, track, clips, .. } = setup();
    app.ui.show_vocal_editor = false;
    app.vocal.track = None;
    app.run("ui.vocal_editor", json!({"clip": clips[0].0})).unwrap();
    assert!(app.ui.show_vocal_editor);
    assert_eq!(app.vocal.track, Some(track));
    // A MIDI or missing clip is refused.
    assert!(app.run("ui.vocal_editor", json!({"clip": 9999})).is_err());
}

#[test]
fn the_dock_has_a_vocal_editor_tab_beside_midi_editor() {
    let Setup { ctx, mut app, .. } = setup();
    app.ui.show_midi_editor = true;
    settle(&ctx, &mut app);
    let layout = &app.ui.docking.layout;
    assert!(layout.contains(&Panel::Vocal) && layout.contains(&Panel::Midi));
    let mut nodes: Vec<_> = layout.root.iter().collect();
    let mut tabbed = false;
    while let Some(n) = nodes.pop() {
        match n {
            craft_ui::docking::Node::Split { first, second, .. } => {
                nodes.push(first);
                nodes.push(second);
            }
            craft_ui::docking::Node::Tabs { panels, .. } => tabbed |= panels.contains(&Panel::Vocal) && panels.contains(&Panel::Midi),
            _ => {}
        }
    }
    assert!(tabbed, "Vocal and MIDI editors share the bottom tab group: {layout:?}");
    let tab = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("soundcraft-dock-tab", "VOCAL EDITOR"))));
    let midi = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(("soundcraft-dock-tab", "MIDI EDITOR"))));
    assert!(tab.unwrap().left() < midi.unwrap().left(), "VOCAL EDITOR comes first in the lower dock strip");
    // Window › Vocal Editor toggles it.
    app.run("window.vocal_editor", json!({})).unwrap();
    settle(&ctx, &mut app);
    assert!(!app.ui.docking.layout.contains(&Panel::Vocal));
}

#[test]
fn open_at_selects_the_note_and_scrolls() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    assert_eq!(ns.len(), 2, "{ns:?}");
    // Scroll far away first.
    app.engine.session_mut().edit.zoom.scroll = 20 * i64::from(RATE);
    let (a, b) = span(&app, ns[1]);
    let at = (a + b) / 2;
    open_at(&mut app, track, at, Some(ns[1]));
    settle(&ctx, &mut app);
    assert_eq!(app.vocal.selected.iter().copied().collect::<Vec<_>>(), vec![ns[1]]);
    let (t0, t1) = vocal_editor::view_range(&app, 900.0);
    assert!(t0 <= at && at < t1, "{t0}..{t1} holds {at}");
    assert!(app.ui.show_vocal_editor && app.vocal.track == Some(track));
}

fn bare(setup: impl FnOnce(&mut Session)) -> (egui::Context, SoundApp) {
    let mut s = Session::default();
    setup(&mut s);
    let mut app = SoundApp::new(soundcraft_engine::Engine::new(s), None, Services::default());
    app.ui.show_vocal_editor = true;
    let ctx = egui::Context::default();
    settle(&ctx, &mut app);
    (ctx, app)
}

#[test]
fn empty_states() {
    // Only a MIDI track, selected.
    let (ctx, mut app) = bare(|s| {
        let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Synth"));
        s.edit.selected_tracks = vec![t];
    });
    settle(&ctx, &mut app);
    assert_eq!(text(&ctx, "message").as_deref(), Some("Pick an audio track"));
    // An audio track not named like a vocal.
    let mut keys = None;
    let (ctx, mut app) = bare(|s| {
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Keys"));
        s.edit.selected_tracks = vec![t];
        keys = Some(t);
    });
    assert_eq!(text(&ctx, "message").as_deref(), Some("Not marked as a vocal track"));
    let mark = control(&ctx, "mark").expect("a Mark as vocal button");
    click(&ctx, &mut app, mark.center());
    assert!(app.engine.session().track(keys.unwrap()).unwrap().is_vocal());
    // A vocal track whose clip isn't analysed yet: "Analysing…" on the first frame, then the
    // background worker (the editor's clip first) analyses it.
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead Vox"));
    // A long take, so the analysis is still running after the first frames.
    let mut take = take2();
    take.extend(vocal_support::breath(20.0, -30.0));
    let clip = vocal_support::place(&mut s, t, take, 0);
    let mut app = SoundApp::new(soundcraft_engine::Engine::new(s), None, Services::default());
    app.ui.show_vocal_editor = true;
    let ctx = egui::Context::default();
    // The dock lays out over the first frames.
    for _ in 0..4 {
        frame(&ctx, &mut app, vec![]);
    }
    assert_eq!(text(&ctx, "message").as_deref(), Some("Analysing Lead Vox\u{2026}"));
    // The clip's waveform shows while it is analysed.
    let waves: Vec<(soundcraft_model::ClipId, egui::Rect)> = ctx.data(|d| d.get_temp(egui::Id::new("soundcraft-vocal-waves"))).unwrap_or_default();
    assert!(waves.iter().any(|(c, r)| *c == clip && r.width() > 10.0), "{waves:?}");
    soundcraft_engine::vocal::run_until_idle(&mut app.engine, &[clip]).unwrap();
    settle(&ctx, &mut app);
    assert_eq!(text(&ctx, "message"), None, "analysed: the roll");
    // Analysed: the roll, no message.
    let Setup { ctx, .. } = setup();
    assert_eq!(text(&ctx, "message"), None);
}

#[test]
fn worker_focus_is_the_editor_clip() {
    let Setup { mut app, track, clips, .. } = setup_with(vec![(take2(), 0), (take2(), 2 * i64::from(RATE))], true);
    let ns = notes(&app, track);
    let second = *ns.iter().find(|r| r.clip == clips[1]).unwrap();
    app.vocal.selected.insert(second);
    assert_eq!(focus_clip(&app), Some(clips[1]));
    app.vocal.selected.clear();
    assert_eq!(focus_clip(&app), Some(clips[0]), "the clip under the playhead");
}

fn selected(app: &SoundApp) -> Vec<soundcraft_engine::vocal::NoteRef> {
    app.vocal.selected.iter().copied().collect()
}

const SHIFT: egui::Modifiers = egui::Modifiers::SHIFT;
const CTRL: egui::Modifiers = egui::Modifiers::COMMAND;

#[test]
fn click_shift_click_and_rubber_band_select() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    let (b1, b2) = (bar(&ctx, ns[0]), bar(&ctx, ns[1]));
    click(&ctx, &mut app, b1.center());
    assert_eq!(selected(&app), vec![ns[0]]);
    vocal_support::click_mod(&ctx, &mut app, b2.center(), SHIFT);
    assert_eq!(selected(&app), vec![ns[0], ns[1]]);
    vocal_support::click_mod(&ctx, &mut app, b1.center(), egui::Modifiers { ctrl: true, command: true, ..Default::default() });
    assert_eq!(selected(&app), vec![ns[1]]);
    // A click on empty roll space clears; a band over both selects both.
    let v = view(&ctx);
    click(&ctx, &mut app, egui::pos2(b1.left() - 4.0, v.rect.top() + 4.0));
    assert!(app.vocal.selected.is_empty());
    let from = egui::pos2(b1.left() - 6.0, b1.top().min(b2.top()) - 20.0);
    let to = egui::pos2(b2.right() + 6.0, b1.bottom().max(b2.bottom()) + 20.0);
    drag(&ctx, &mut app, from, to, 6);
    assert_eq!(selected(&app), vec![ns[0], ns[1]]);
    // The tooltip names the note and its offsets.
    frame(&ctx, &mut app, vec![egui::Event::PointerMoved(b2.center())]);
    settle(&ctx, &mut app);
    let tip = text(&ctx, "tooltip").expect("a hover tooltip");
    assert!(tip.starts_with("B3 ") || tip.starts_with("C4 ") || tip.contains('c'), "{tip}");
    assert!(tip.contains("\u{2192}") && tip.contains("centre 0%"), "{tip}");
}

#[test]
fn unvoiced_blocks_are_not_selectable() {
    let Setup { ctx, mut app, track, .. } = setup_with(vec![(take_breath(), 0)], true);
    let ns = notes(&app, track);
    click(&ctx, &mut app, bar(&ctx, ns[0]).center());
    assert_eq!(app.vocal.selected.len(), 1);
    let blocks: Vec<egui::Rect> = ctx.data(|d| d.get_temp(egui::Id::new("soundcraft-vocal-unvoiced"))).unwrap_or_default();
    let block = *blocks.first().expect("the breath is drawn as a hatched block");
    click(&ctx, &mut app, block.center());
    assert!(app.vocal.selected.is_empty(), "a click on a breath only clears the selection");
    assert_eq!(text(&ctx, "unvoiced-label").as_deref(), Some("breath"));
}

#[test]
fn ctrl_a_selects_the_whole_clips_shown_and_ctrl_shift_a_the_notes_in_view() {
    let rate = i64::from(RATE);
    let Setup { ctx, mut app, track, clips } = setup_with(vec![(take2(), rate / 10), (take2(), 3 * rate / 2)], true);
    let ns = notes(&app, track);
    assert_eq!(ns.len(), 4);
    let v = view(&ctx);
    press(&ctx, &mut app, v.rect.center(), egui::Key::A, CTRL);
    assert_eq!(selected(&app).len(), 4, "Ctrl+A: every note of both clips");
    assert!(app.vocal.whole_track);
    assert_eq!(text(&ctx, "selected-text").as_deref(), Some("Select all \u{b7} 4 notes"));
    press(&ctx, &mut app, v.rect.center(), egui::Key::Escape, egui::Modifiers::NONE);
    assert!(app.vocal.selected.is_empty());
    assert_eq!(text(&ctx, "selected-text").as_deref(), Some("0 selected"));
    // The view scrolled to 2.1 s: only the second clip's second note is in view.
    app.engine.session_mut().edit.zoom.scroll = 21 * rate / 10;
    settle(&ctx, &mut app);
    let v = view(&ctx);
    assert_eq!(v.t0, 21 * rate / 10);
    press(&ctx, &mut app, v.rect.center(), egui::Key::A, CTRL);
    let sel = selected(&app);
    assert_eq!(sel.len(), 2, "Ctrl+A: the whole clip shown, in view or not: {sel:?}");
    assert!(sel.iter().all(|r| r.clip == clips[1]), "{sel:?}");
    assert_eq!(view(&ctx).t0, 21 * rate / 10, "the roll does not scroll");
    assert_eq!(text(&ctx, "selected-text").as_deref(), Some("Select all \u{b7} 2 notes"));
    press(&ctx, &mut app, v.rect.center(), egui::Key::A, CTRL | SHIFT);
    let sel = selected(&app);
    assert_eq!(sel.len(), 1, "Ctrl+Shift+A: only the note in view: {sel:?}");
    assert_eq!(sel[0], ns[3]);
    assert!(app.vocal.whole_track, "levelling still refers to the track");
    // The Select all button does the same as Ctrl+A.
    press(&ctx, &mut app, v.rect.center(), egui::Key::Escape, egui::Modifiers::NONE);
    click(&ctx, &mut app, control(&ctx, "select-all").unwrap().center());
    assert_eq!(selected(&app).len(), 2);
}

#[test]
fn s_splits_and_j_joins() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    let b1 = bar(&ctx, ns[0]);
    let undo = app.engine.undo_history().len();
    press(&ctx, &mut app, b1.center(), egui::Key::S, egui::Modifiers::NONE);
    let after = notes(&app, track);
    assert_eq!(after.len(), 3, "S splits the note under the mouse");
    assert_eq!(app.engine.undo_history().len(), undo + 1);
    // The halves are selected; J joins them back.
    assert_eq!(selected(&app), vec![after[0], after[1]]);
    press(&ctx, &mut app, b1.center(), egui::Key::J, egui::Modifiers::NONE);
    assert_eq!(notes(&app, track).len(), 2);
    // J with notes that aren't neighbours says so.
    app.vocal.selected = [notes(&app, track)[0]].into_iter().collect();
    press(&ctx, &mut app, b1.center(), egui::Key::J, egui::Modifiers::NONE);
    assert_eq!(app.ui.status, "Join needs two neighbouring notes");
}

#[test]
fn scoop_popup_sets_the_transition() {
    let Setup { ctx, mut app, track, clips } = setup();
    // Scoops are Classic's (Musical notes take their approach in): new tunings are Musical.
    app.run("vocal.note_mode", json!({"clips": clips, "mode": "classic"})).unwrap();
    let ns = notes(&app, track);
    add_scoop(&mut app, ns[1], 12);
    settle(&ctx, &mut app);
    let handles: Vec<(soundcraft_engine::vocal::NoteRef, egui::Rect)> =
        ctx.data(|d| d.get_temp(egui::Id::new("soundcraft-vocal-scoops"))).unwrap_or_default();
    let (_, handle) = *handles.iter().find(|(r, _)| *r == ns[1]).expect("a scoop handle on note 2");
    click(&ctx, &mut app, handle.center());
    assert_eq!(app.vocal.popup, Some(ns[1]));
    let title = text(&ctx, "popup-title").unwrap();
    assert!(title.starts_with("Transition into "), "{title}");
    for name in ["keep", "smooth", "shorten"] {
        assert!(control(&ctx, name).is_some(), "{name}");
    }
    click(&ctx, &mut app, control(&ctx, "smooth").unwrap().center());
    let c = app.engine.session().find_clip(clips[0]).unwrap().1.clone();
    let e = c.tune_live().and_then(|t| t.edits.get(&ns[1].note)).cloned().expect("the note has an edit");
    assert_eq!(e.transition, soundcraft_vocal::Transition::Smooth);
    assert_eq!(app.vocal.popup, None);
}

#[test]
fn stale_selection_is_pruned() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    let b1 = bar(&ctx, ns[0]);
    press(&ctx, &mut app, b1.center(), egui::Key::S, egui::Modifiers::NONE);
    let halves = selected(&app);
    assert_eq!(halves.len(), 2);
    let second = halves[1];
    assert!(app.engine.undo());
    settle(&ctx, &mut app);
    assert!(!app.vocal.selected.contains(&second), "the split's second half is gone after Undo");
    assert!(app.vocal.selected.iter().all(|r| notes(&app, track).contains(r)));
}

#[test]
fn shortcuts_ignore_text_fields() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    let b1 = bar(&ctx, ns[0]);
    let mut typed = String::new();
    let mut run = |ctx: &egui::Context, app: &mut SoundApp, events: Vec<egui::Event>| {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))),
            events,
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
            egui::Area::new(egui::Id::new("test-field")).fixed_pos(egui::pos2(10.0, 10.0)).show(ui.ctx(), |ui| {
                let r = ui.text_edit_singleline(&mut typed);
                r.request_focus();
            });
        })
        .textures_delta
        .clear();
    };
    for _ in 0..3 {
        run(&ctx, &mut app, vec![egui::Event::PointerMoved(b1.center())]);
    }
    run(&ctx, &mut app, key(egui::Key::S, egui::Modifiers::NONE));
    run(&ctx, &mut app, vec![egui::Event::Text("s".into())]);
    run(&ctx, &mut app, vec![]);
    assert_eq!(notes(&app, track).len(), 2, "S typed into a text field doesn't split");
}

#[test]
fn level_lane_shows_levels_and_the_typical_line() {
    use soundcraft_ui_egui::vocal_editor::level_lane::lane_y;
    let Setup { ctx, mut app, track, clips } = setup_with(vec![(take3(), i64::from(RATE) / 4)], true);
    let ns = notes(&app, track);
    assert_eq!(ns.len(), 3, "{ns:?}");
    app.run("vocal.edit", json!({"notes": ns, "raise": 25, "lower": 25})).unwrap();
    settle(&ctx, &mut app);
    let (_, c) = app.engine.session().find_clip(clips[0]).map(|(t, c)| (t, c.clone())).unwrap();
    let a = soundcraft_engine::vocal::analysis_for(app.engine.session(), &c).unwrap();
    let levels: Vec<f32> = a.notes.iter().map(|n| n.level_db).collect();
    let typical = soundcraft_vocal::typical_level_db(&levels);
    assert!((typical + 21.0).abs() < 1.0, "{levels:?}");
    let lane: egui::Rect = control(&ctx, "lane").expect("the Level lane");
    let bars: Vec<(soundcraft_engine::vocal::NoteRef, egui::Rect)> =
        ctx.data(|d| d.get_temp(egui::Id::new("soundcraft-vocal-level-bars"))).unwrap_or_default();
    let ghosts: Vec<(soundcraft_engine::vocal::NoteRef, egui::Rect)> =
        ctx.data(|d| d.get_temp(egui::Id::new("soundcraft-vocal-level-ghosts"))).unwrap_or_default();
    let l1 = levels[0];
    let moved = l1 + soundcraft_vocal::level_gain_db(l1, typical, 0.25, 0.25);
    assert!((moved - l1 - 0.25 * (typical - l1)).abs() < 0.2, "raise 25 % of the shortfall: {l1} -> {moved}");
    let b1 = bars.iter().find(|(r, _)| *r == ns[0]).unwrap().1;
    assert!((b1.center().y - lane_y(lane, typical, moved)).abs() <= 1.0, "bar {} want {}", b1.center().y, lane_y(lane, typical, moved));
    let g1 = ghosts.iter().find(|(r, _)| *r == ns[0]).expect("a moved note keeps its original level as a ghost").1;
    assert!((g1.center().y - lane_y(lane, typical, l1)).abs() <= 1.0);
    assert_eq!(text(&ctx, "typical").as_deref(), Some("typical \u{2212}21 dB"));
    let b3 = bars.iter().find(|(r, _)| *r == ns[2]).unwrap().1;
    click(&ctx, &mut app, b3.center());
    assert_eq!(app.vocal.selected.iter().copied().collect::<Vec<_>>(), vec![ns[2]]);
}

/// One source of five notes, comped as ten clips of the whole take.
fn ten_clips_of_one_take() -> (egui::Context, SoundApp, soundcraft_model::TrackId) {
    use soundcraft_audio_io::{AudioBuffer, FileFormat};
    let mut take = Vec::new();
    for k in 0..5 {
        take.extend(vowel(220.0 * 2f64.powf(f64::from(k) / 12.0), 0.3, -21.0));
        take.extend(gap(0.15));
    }
    let len = take.len() as i64;
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead Vox"));
    let src = soundcraft_engine::io::add_source(&mut s, "Vox", AudioBuffer { sample_rate: RATE, channels: vec![take] }, None, FileFormat::Wav);
    let clips: Vec<_> = (0..10)
        .map(|i| {
            let id = s.new_clip_id();
            soundcraft_engine::edit::place_clip(&mut s, t, soundcraft_model::Clip::audio(id, "Vox", src, 0, i * (len + i64::from(RATE) / 10), len));
            id
        })
        .collect();
    let mut e = soundcraft_engine::Engine::new(s);
    soundcraft_engine::vocal::run_until_idle(&mut e, &clips).unwrap();
    let mut app = SoundApp::new(e, None, Services::default());
    app.ui.show_vocal_editor = true;
    app.run("edit.select", json!({"tracks": [t.0]})).unwrap();
    let ctx = egui::Context::default();
    settle(&ctx, &mut app);
    let w = (app.edit_layout.timeline[2] - app.edit_layout.timeline[0]).max(100.0);
    app.engine.session_mut().edit.zoom.samples_per_px = 30.0 * f64::from(RATE) / f64::from(w);
    app.engine.session_mut().edit.zoom.scroll = 0;
    settle(&ctx, &mut app);
    (ctx, app, t)
}

#[test]
fn fifty_selected_notes_over_ten_clips_copy_no_analysis_per_frame() {
    let (ctx, mut app, track) = ten_clips_of_one_take();
    let ns = notes(&app, track);
    assert_eq!(ns.len(), 50, "{ns:?}");
    app.vocal.selected = ns.iter().copied().collect();
    settle(&ctx, &mut app);
    assert_eq!(text(&ctx, "summary").map(|s| s.starts_with("50 notes selected")), Some(true), "{:?}", text(&ctx, "summary"));
    let before = vocal_editor::analysis_copies();
    for _ in 0..10 {
        frame(&ctx, &mut app, vec![]);
    }
    let copies = vocal_editor::analysis_copies() - before;
    assert_eq!(copies, 0, "no analysis is copied per frame while nothing changes ({copies} in 10 frames)");
    // Twins still work: one note selects its ten copies.
    app.vocal.selected.clear();
    settle(&ctx, &mut app);
    click(&ctx, &mut app, bar(&ctx, ns[0]).center());
    assert_eq!(app.vocal.selected.len(), 10, "{:?}", app.vocal.selected);
}

#[test]
fn keys_reach_the_app_when_the_editor_is_not_drawn() {
    let Setup { ctx, mut app, .. } = setup();
    let over = view(&ctx).rect.center();
    frame(&ctx, &mut app, vec![egui::Event::PointerMoved(over)]);
    let none = egui::Modifiers::NONE;
    assert!(vocal_editor::owns_key(&app, &ctx, egui::Key::S, none), "drawn under the pointer: the editor takes S");
    assert!(!vocal_editor::owns_key(&app, &ctx, egui::Key::S, SHIFT), "Shift+S stays the app's");
    assert!(!vocal_editor::owns_key(&app, &ctx, egui::Key::J, SHIFT), "Shift+J stays the app's");
    // Frames where the editor isn't drawn (another window or tab in front): its old rect is stale.
    for _ in 0..2 {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1200.0))),
            events: vec![egui::Event::PointerMoved(over)],
            ..Default::default()
        };
        ctx.run_ui(raw, |_| {}).textures_delta.clear();
    }
    assert!(app.ui.show_vocal_editor);
    assert!(!vocal_editor::owns_key(&app, &ctx, egui::Key::S, none), "S reaches the normal handler");
    assert!(!vocal_editor::owns_key(&app, &ctx, egui::Key::A, CTRL), "Ctrl+A reaches the normal handler");
}

#[test]
fn an_unanalysed_clip_beside_analysed_ones_shows_its_waveform() {
    let mut long = take2();
    long.extend(vocal_support::breath(20.0, -30.0));
    let Setup { ctx, mut app, clips, .. } = setup_with(vec![(take2(), 0), (long, 3 * i64::from(RATE) / 2)], false);
    soundcraft_engine::vocal::run_until_idle(&mut app.engine, &clips[..1]).unwrap();
    frame(&ctx, &mut app, vec![]);
    let waves: Vec<(soundcraft_model::ClipId, egui::Rect)> = ctx.data(|d| d.get_temp(egui::Id::new("soundcraft-vocal-waves"))).unwrap_or_default();
    assert_eq!(text(&ctx, "message"), None, "the analysed clip shows the roll");
    assert!(waves.iter().any(|(c, r)| *c == clips[1] && r.width() > 10.0), "the unanalysed clip's waveform: {waves:?}");
    assert!(!waves.iter().any(|(c, _)| *c == clips[0]), "not over the analysed clip's notes");
}

#[test]
fn stretched_clips_are_left_out_of_twins_and_whole_track_apply() {
    let (ctx, mut app, track) = ten_clips_of_one_take();
    let ns = notes(&app, track);
    let first = ns[0].clip;
    let others: Vec<_> = ns.iter().map(|r| r.clip).filter(|c| *c != first).collect();
    for c in &others {
        app.engine.session_mut().find_clip_mut(*c).unwrap().stretch = 1.5;
    }
    app.engine.revision += 1;
    settle(&ctx, &mut app);
    click(&ctx, &mut app, bar(&ctx, ns[0]).center());
    assert_eq!(app.vocal.selected.iter().copied().collect::<Vec<_>>(), vec![ns[0]], "no twins in stretched clips");
    // Apply with nothing selected tunes the unstretched clip's notes, not refused.
    app.vocal.selected.clear();
    settle(&ctx, &mut app);
    let apply = control(&ctx, "apply").unwrap();
    app.ui.status.clear();
    click(&ctx, &mut app, apply.center());
    assert!(app.ui.status.is_empty(), "{}", app.ui.status);
    let s = app.engine.session();
    let tuned = s.find_clip(first).unwrap().1.tune_live().map_or(0, |t| t.edits.len());
    assert_eq!(tuned, 5, "the unstretched clip's notes are tuned");
    assert!(others.iter().all(|c| s.find_clip(*c).unwrap().1.tune.is_none()), "stretched clips untouched");
}

#[test]
fn tuned_curves_draw_over_the_note_bars() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    app.run("vocal.edit", json!({"notes": ns, "centre": 60})).unwrap();
    settle(&ctx, &mut app);
    let layers: Vec<&'static str> = ctx.data(|d| d.get_temp(egui::Id::new("soundcraft-vocal-layers"))).unwrap_or_default();
    assert_eq!(layers, ["notes", "curves"], "the bars first, the tuned and ghost curves over them");
}

#[test]
fn open_at_and_join_leave_whole_track_and_grow_the_dock() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    // Shrink the lower dock, then Tune Vocal… grows it back.
    let mut path = Vec::new();
    fn find(node: &craft_ui::docking::Node<Panel>, path: &mut Vec<bool>) -> Option<craft_ui::layout::SplitSize> {
        let craft_ui::docking::Node::Split { first, second, size, .. } = node else { return None };
        if matches!(&**second, craft_ui::docking::Node::Tabs { panels, .. } if panels.contains(&Panel::Vocal)) {
            return Some(*size);
        }
        for (side, child) in [(false, first), (true, second)] {
            path.push(side);
            if let Some(s) = find(child, path) {
                return Some(s);
            }
            path.pop();
        }
        None
    }
    find(app.ui.docking.layout.root.as_ref().unwrap(), &mut path).unwrap();
    app.run("ui.dock", json!({"operation": "resizeSplit", "path": path, "size": {"FixedSecond": 300.0}})).unwrap();
    settle(&ctx, &mut app);
    app.vocal.whole_track = true;
    open_at(&mut app, track, 0, Some(ns[0]));
    assert!(!app.vocal.whole_track, "open_at selects one note: not the whole track");
    let size = find(app.ui.docking.layout.root.as_ref().unwrap(), &mut Vec::new()).unwrap();
    assert!(matches!(size, craft_ui::layout::SplitSize::FixedSecond(h) if h >= vocal_editor::DOCK_H), "{size:?}");
    settle(&ctx, &mut app);
    // Split then join: the joined note alone, not the whole track.
    press(&ctx, &mut app, bar(&ctx, ns[0]).center(), egui::Key::S, egui::Modifiers::NONE);
    app.vocal.whole_track = true;
    press(&ctx, &mut app, view(&ctx).rect.center(), egui::Key::J, egui::Modifiers::NONE);
    assert_eq!(app.vocal.selected.len(), 1);
    assert!(!app.vocal.whole_track);
}

#[test]
fn level_line_and_caption_agree() {
    let Setup { ctx, mut app, track, .. } = setup_with(vec![(take3(), i64::from(RATE) / 4)], true);
    let ns = notes(&app, track);
    app.vocal.selected = [ns[0]].into_iter().collect();
    settle(&ctx, &mut app);
    let line = text(&ctx, "typical").unwrap();
    let caption = text(&ctx, "loud-caption").unwrap();
    assert!(caption.starts_with("towards selection's typical level"), "{caption}");
    assert!(caption.ends_with(line.trim_start_matches("typical ")), "{line} / {caption}");
    // Ctrl+A: the track's typical level in both.
    press(&ctx, &mut app, view(&ctx).rect.center(), egui::Key::A, CTRL);
    let line = text(&ctx, "typical").unwrap();
    let caption = text(&ctx, "loud-caption").unwrap();
    assert_eq!(line, "typical \u{2212}21 dB");
    assert_eq!(caption, "towards the track's typical level \u{2212}21 dB");
}

#[test]
fn missing_audio_says_it_cant_be_analysed_not_analysing() {
    // A vocal track whose clip's audio is gone from the pool: the worker never offers it, so
    // the editor must not wait for it.
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead Vox"));
    let clip = vocal_support::place(&mut s, t, take2(), 0);
    let src = s.find_clip(clip).and_then(|(_, c)| c.source()).unwrap();
    s.pool.remove(src);
    let mut app = SoundApp::new(soundcraft_engine::Engine::new(s), None, Services::default());
    app.ui.show_vocal_editor = true;
    let ctx = egui::Context::default();
    for _ in 0..4 {
        frame(&ctx, &mut app, vec![]);
    }
    assert_eq!(
        text(&ctx, "message").as_deref(),
        Some("Lead Vox can't be analysed (missing audio, or longer than 30 min: Edit › Split into Files cuts a long recording into songs)")
    );
}

#[test]
fn tune_vocal_grows_the_dock_to_at_most_60_percent_of_the_space() {
    let Setup { ctx, mut app, track, .. } = setup();
    fn size_of(node: &craft_ui::docking::Node<Panel>, path: &mut Vec<bool>) -> Option<(Vec<bool>, f32)> {
        let craft_ui::docking::Node::Split { first, second, size, .. } = node else { return None };
        if matches!(&**second, craft_ui::docking::Node::Tabs { panels, .. } if panels.contains(&Panel::Vocal)) {
            let craft_ui::layout::SplitSize::FixedSecond(h) = size else { return None };
            return Some((path.clone(), *h));
        }
        for (side, child) in [(false, first), (true, second)] {
            path.push(side);
            if let Some(s) = size_of(child, path) {
                return Some(s);
            }
            path.pop();
        }
        None
    }
    // A short window: 620 px would leave the Edit window almost nothing.
    let (path, _) = size_of(app.ui.docking.layout.root.as_ref().unwrap(), &mut Vec::new()).unwrap();
    app.run("ui.dock", json!({"operation": "resizeSplit", "path": path, "size": {"FixedSecond": 200.0}})).unwrap();
    let short = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 700.0));
    for _ in 0..3 {
        let raw = egui::RawInput { screen_rect: Some(short), ..Default::default() };
        ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        })
        .textures_delta
        .clear();
    }
    open_at(&mut app, track, 0, None);
    let (_, h) = size_of(app.ui.docking.layout.root.as_ref().unwrap(), &mut Vec::new()).unwrap();
    assert!(h > 200.0, "grown: {h}");
    assert!(h <= 0.6 * 700.0, "at most 60 % of a 700 px window: {h}");
}

#[test]
fn rows_are_shaded_by_the_piano_and_hatched_when_out_of_key() {
    use soundcraft_ui_egui::vocal_editor::roll::RowFill;
    let fills = |ctx: &egui::Context| -> Vec<RowFill> { ctx.data(|d| d.get_temp(egui::Id::new("soundcraft-vocal-row-fills"))).unwrap_or_default() };
    let Setup { ctx, mut app, .. } = setup();
    let rows = fills(&ctx);
    assert!(rows.len() > 12, "{} rows drawn", rows.len());
    for r in &rows {
        let black = matches!(r.midi.rem_euclid(12), 1 | 3 | 6 | 8 | 10);
        assert_eq!(r.lit, !black, "no key: white-key rows light, black-key rows dark: {r:?}");
        assert!(!r.hatched, "no key: no hatch: {r:?}");
    }
    // E major: the fills stay the piano's, and C, D, F, G and A# rows (out of key) are hatched.
    app.engine.session_mut().band.key = 4;
    app.engine.session_mut().band.scale = 0;
    settle(&ctx, &mut app);
    let rows = fills(&ctx);
    let e_major = [4, 6, 8, 9, 11, 1, 3];
    for r in &rows {
        let pc = r.midi.rem_euclid(12);
        let black = matches!(pc, 1 | 3 | 6 | 8 | 10);
        assert_eq!(r.lit, !black, "the fill still follows the piano: {r:?}");
        assert_eq!(r.hatched, !e_major.contains(&pc), "hatched only out of key: {r:?}");
    }
    assert!(rows.iter().any(|r| r.hatched) && rows.iter().any(|r| !r.hatched));
}
