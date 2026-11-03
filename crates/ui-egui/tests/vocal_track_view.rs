//! The track's "waveform + pitch" view (plan 4 Task 1): the pitch line per note, its clipping to
//! the clip window, hover and double-click, and the TUNED badge.

mod vocal_support;

use egui::{Event, Modifiers, Pos2, Rect, pos2};
use serde_json::json;
use soundcraft_model::{ClipId, TrackHeight};
use soundcraft_ui_egui::SoundApp;
use soundcraft_ui_egui::vocal_style::off_colour;
use soundcraft_ui_egui::vocal_track::{PitchSeg, pitch_line, target_for, tuned_for};
use vocal_support::*;

/// The Lead Vox track tall (240 px), in the pitch view, the editor closed.
fn pitch_setup(analyse: bool) -> Setup {
    let mut st = setup_with(vec![(take2(), i64::from(RATE) / 4)], analyse);
    st.app.ui.show_vocal_editor = false;
    if let Some(t) = st.app.engine.session_mut().track_mut(st.track) {
        t.height = TrackHeight::Custom(240);
    }
    st.app.run("track.view", json!({"track": st.track.0, "view": "pitch"})).unwrap();
    settle(&st.ctx, &mut st.app);
    st
}

fn segs(ctx: &egui::Context, clip: ClipId) -> Vec<PitchSeg> {
    ctx.data(|d| d.get_temp::<Vec<PitchSeg>>(egui::Id::new(format!("soundcraft-vocal-track-segs-{}", clip.0)))).unwrap_or_default()
}

/// The Lead Vox lane (the timeline across the track row).
fn lane(app: &SoundApp, st: &Setup) -> Rect {
    let row = app.edit_layout.rows.iter().find(|(id, _)| *id == st.track.0).map(|(_, r)| *r).expect("the track row");
    let tl = app.edit_layout.timeline;
    Rect::from_min_max(pos2(tl[0], row[1]), pos2(tl[2], row[3]))
}

/// The pure line of the first clip across a 1000 × 200 lane over 0–2 s.
fn line(app: &SoundApp, clip: ClipId) -> Vec<PitchSeg> {
    let s = app.engine.session();
    let (_, c) = s.find_clip(clip).unwrap();
    let a = soundcraft_engine::vocal::analysis_for(s, c).unwrap();
    let curves = soundcraft_engine::vocal::curves_for(s, c);
    let lane = Rect::from_min_max(Pos2::ZERO, pos2(1000.0, 200.0));
    pitch_line(lane, 0, 2 * i64::from(RATE), 5300.0, 6200.0, c, &a, curves.as_ref(), -21.0, &target_for(s, c), &tuned_for(s, c, &a))
}

#[test]
fn pitch_view_is_offered_and_set() {
    let mut st = setup();
    let track = st.app.engine.session().track(st.track).unwrap().clone();
    let opts = soundcraft_ui_egui::edit_window::view_options(&track);
    let i = opts.iter().position(|o| *o == "pitch").expect("pitch is offered");
    assert_eq!(opts.get(i.wrapping_sub(1)), Some(&"waveform"), "right after waveform");
    assert_eq!(soundcraft_ui_egui::edit_window::view_label(&track, "pitch"), "waveform + pitch");
    st.app.run("track.view", json!({"track": st.track.0, "view": "pitch"})).unwrap();
    assert_eq!(st.app.engine.session().track(st.track).unwrap().view, "pitch");
}

#[test]
fn one_coloured_segment_per_note_and_none_on_unvoiced() {
    let st = setup();
    let segs = line(&st.app, st.clips[0]);
    assert_eq!(segs.len(), 2, "{segs:?}");
    assert_eq!(segs[0].colour, off_colour(segs[0].offset));
    assert!((segs[0].offset + 30.0).abs() < 4.0, "{}", segs[0].offset);
    assert!((segs[1].offset - 20.0).abs() < 4.0, "{}", segs[1].offset);
    // The silent gap is 0.75 s..0.95 s (clip at 0.25 s); 1000 px over 2 s.
    for s in &segs {
        assert!(s.pts.iter().all(|p| !(p.x > 380.0 && p.x < 470.0)), "a point in the gap");
        assert!(s.width >= 2.0 && s.width <= 6.0);
    }
}

#[test]
fn tuned_segments_follow_the_tuned_curve() {
    let mut st = setup();
    let n1 = notes(&st.app, st.track)[0];
    st.app.run("vocal.edit", json!({"notes": [{"clip": n1.clip.0, "note": n1.note.0}], "centre": 100})).unwrap();
    let segs = line(&st.app, st.clips[0]);
    let s1 = &segs[0];
    // 5300..6200 over 200 px: 5700 c (A3) at y = (6200 − 5700) / 900 · 200.
    let want = (6200.0 - 5700.0) / 900.0 * 200.0;
    let mid: Vec<f32> = s1.pts.iter().skip(s1.pts.len() / 4).take(s1.pts.len() / 2).map(|p| p.y).collect();
    let mean = mid.iter().sum::<f32>() / mid.len() as f32;
    assert!((mean - want).abs() <= 1.5, "mean y {mean} vs {want}");
    assert_eq!(s1.colour, off_colour(s1.offset));
    assert!(s1.offset.abs() < 1.0, "{}", s1.offset);
}

#[test]
fn line_is_clipped_to_the_clip_window() {
    let mut st = setup();
    // Trim the clip's end to the middle of note 2 (note 2 is 0.95 s..1.45 s).
    let cut = (1.2 * f64::from(RATE)) as i64;
    st.app.run("edit.trim_end_to_insertion", json!({"tracks": [st.track.0], "at": cut})).unwrap();
    assert_eq!(st.app.engine.session().find_clip(st.clips[0]).unwrap().1.end(), cut);
    let segs = line(&st.app, st.clips[0]);
    assert_eq!(segs.len(), 2);
    let end_x = cut as f32 / (2.0 * RATE as f32) * 1000.0;
    assert!(segs[1].pts.iter().all(|p| p.x <= end_x + 0.01), "past the clip end");
    assert!(segs[1].pts.iter().any(|p| p.x > end_x - 60.0), "drawn up to it");
}

#[test]
fn hover_and_double_click_open_the_editor_there() {
    let mut st = pitch_setup(true);
    let drawn = segs(&st.ctx, st.clips[0]);
    assert_eq!(drawn.len(), 2, "both notes drawn in the lane");
    let l = lane(&st.app, &st);
    let p = drawn[0].pts[drawn[0].pts.len() / 2];
    assert!(l.contains(p));
    frame(&st.ctx, &mut st.app, vec![Event::PointerMoved(p)]);
    frame(&st.ctx, &mut st.app, vec![]);
    assert_eq!(text(&st.ctx, "track-tip").as_deref(), Some("A3 \u{2212}30c \u{b7} 0:00.2"));
    let n1 = notes(&st.app, st.track)[0];
    for _ in 0..2 {
        frame(&st.ctx, &mut st.app, vec![button(p, true, Modifiers::NONE)]);
        frame(&st.ctx, &mut st.app, vec![button(p, false, Modifiers::NONE)]);
    }
    settle(&st.ctx, &mut st.app);
    assert!(st.app.ui.show_vocal_editor);
    assert_eq!(st.app.vocal.selected.iter().copied().collect::<Vec<_>>(), vec![n1]);
}

#[test]
fn tuned_badge_toggles() {
    let mut st = pitch_setup(true);
    let n1 = notes(&st.app, st.track)[0];
    st.app.run("vocal.edit", json!({"notes": [{"clip": n1.clip.0, "note": n1.note.0}], "centre": 100})).unwrap();
    settle(&st.ctx, &mut st.app);
    let l = lane(&st.app, &st);
    let tl = Rect::from_min_max(pos2(l.min.x, 0.0), pos2(l.max.x, 1000.0));
    let badge = |app: &SoundApp| {
        let s = app.engine.session();
        let (_, c) = s.find_clip(st.clips[0]).unwrap();
        soundcraft_ui_egui::vocal_track::tuned_badge(&st.ctx, s, tl, l, c)
    };
    let (r, label) = badge(&st.app).expect("a badge");
    assert_eq!(label, "TUNED");
    click(&st.ctx, &mut st.app, r.center());
    let s = st.app.engine.session();
    assert!(s.find_clip(st.clips[0]).unwrap().1.tune.as_ref().unwrap().original);
    assert_eq!(badge(&st.app).map(|b| b.1), Some("UNTUNED"), "not Clean Vocal's ORIGINAL");
}

#[test]
fn non_vocal_track_shows_the_waveform() {
    // The background worker analyses vocal tracks within a few frames: a track without the Vocal
    // flag is never analysed, and draws its waveform with the hint.
    let mut st = pitch_setup(false);
    st.app.run("track.vocal", json!({"tracks": [st.track.0], "value": false})).unwrap();
    settle(&st.ctx, &mut st.app);
    assert_eq!(text(&st.ctx, &format!("track-hint-{}", st.clips[0].0)).as_deref(), Some("Not a vocal track"));
    assert!(segs(&st.ctx, st.clips[0]).is_empty());
    let s = st.app.engine.session();
    let t = s.track(st.track).unwrap();
    assert!(!soundcraft_ui_egui::vocal_track::line_shown(s, t, &t.clips()[0]), "the waveform is not dimmed");
}

#[test]
fn fader_moves_rebuild_no_analysis() {
    // The editor open beside the pitch view: a fader move bumps the revision but changes nothing
    // the vocal data derives from, so no analysis is copied again.
    let mut st = pitch_setup(true);
    st.app.ui.show_vocal_editor = true;
    settle(&st.ctx, &mut st.app);
    assert_eq!(segs(&st.ctx, st.clips[0]).len(), 2);
    let before = soundcraft_ui_egui::vocal_editor::analysis_copies();
    for db in [-3.0, -4.0, -5.0] {
        st.app.run("mix.volume", json!({"track": st.track.0, "db": db})).unwrap();
        frame(&st.ctx, &mut st.app, vec![]);
    }
    let copies = soundcraft_ui_egui::vocal_editor::analysis_copies() - before;
    assert_eq!(copies, 0, "{copies} analysis copies after fader moves");
    assert_eq!(segs(&st.ctx, st.clips[0]).len(), 2, "the line still draws");
}

#[test]
fn line_is_decimated_to_the_pixel_width() {
    let st = setup();
    let s = st.app.engine.session();
    let (_, c) = s.find_clip(st.clips[0]).unwrap();
    let a = soundcraft_engine::vocal::analysis_for(s, c).unwrap();
    // 40 px over 2 s: a 0.5 s note (100 frames) spans 10 px.
    let lane = Rect::from_min_max(Pos2::ZERO, pos2(40.0, 200.0));
    let segs = pitch_line(lane, 0, 2 * i64::from(RATE), 5300.0, 6200.0, c, &a, None, -21.0, &target_for(s, c), &tuned_for(s, c, &a));
    assert_eq!(segs.len(), 2);
    for sg in &segs {
        let (x0, x1) = sg.pts.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), p| (a.min(p.x), b.max(p.x)));
        let cols = (x1.floor() - x0.floor()) as usize + 1;
        assert!(sg.pts.len() <= 2 * cols, "{} points over {cols} pixel columns", sg.pts.len());
        assert!(sg.pts.len() >= cols, "at least one point per column");
    }
}

#[test]
fn unanalysed_vocal_clip_says_no_pitch_yet() {
    // A vocal track whose clip has no analysis yet (no frame has run, so the worker has not).
    let mut s = soundcraft_model::Session::default();
    let t = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, Some("Lead Vox"));
    let clip = place(&mut s, t, take2(), 0);
    let track = s.track(t).unwrap();
    let c = s.find_clip(clip).unwrap().1;
    assert!(track.is_vocal());
    assert_eq!(soundcraft_ui_egui::vocal_track::lane_hint(&s, track, c), Some("No pitch yet"));
    assert!(!soundcraft_ui_egui::vocal_track::line_shown(&s, track, c));
}

#[test]
fn stretched_clip_draws_no_line_and_says_why() {
    let mut st = pitch_setup(true);
    st.app.engine.session_mut().find_clip_mut(st.clips[0]).unwrap().stretch = 1.5;
    st.app.engine.revision += 1;
    settle(&st.ctx, &mut st.app);
    assert_eq!(text(&st.ctx, &format!("track-hint-{}", st.clips[0].0)).as_deref(), Some("Stretched: plays untuned"));
    assert!(segs(&st.ctx, st.clips[0]).is_empty());
    assert!(line(&st.app, st.clips[0]).is_empty(), "pitch_line draws nothing for a stretched clip");
}

#[test]
fn range_and_lines_are_worked_out_once_per_view() {
    // Two clips on the track: one pitch range for the track, one line per clip, and nothing again
    // while nothing changes.
    let mut st = setup_with(vec![(take2(), i64::from(RATE) / 4), (take2(), 2 * i64::from(RATE))], true);
    st.app.ui.show_vocal_editor = false;
    st.app.engine.session_mut().track_mut(st.track).unwrap().height = TrackHeight::Custom(240);
    st.app.run("track.view", json!({"track": st.track.0, "view": "pitch"})).unwrap();
    st.app.engine.session_mut().edit.zoom.samples_per_px *= 1.5;
    st.app.engine.revision += 1;
    let (r0, l0) = soundcraft_ui_egui::vocal_track::lane_builds();
    frame(&st.ctx, &mut st.app, vec![]);
    let (r1, l1) = soundcraft_ui_egui::vocal_track::lane_builds();
    assert_eq!(r1 - r0, 1, "one range for the track, not one per clip");
    assert_eq!(l1 - l0, 2, "one line per clip");
    for _ in 0..5 {
        frame(&st.ctx, &mut st.app, vec![]);
    }
    assert_eq!(soundcraft_ui_egui::vocal_track::lane_builds(), (r1, l1), "nothing rebuilt while nothing changes");
    // A fader move changes nothing they derive from.
    st.app.run("mix.volume", json!({"track": st.track.0, "db": -6.0})).unwrap();
    frame(&st.ctx, &mut st.app, vec![]);
    assert_eq!(soundcraft_ui_egui::vocal_track::lane_builds(), (r1, l1), "a fader move rebuilds nothing");
    // A tuning edit does.
    let n1 = notes(&st.app, st.track)[0];
    st.app.run("vocal.edit", json!({"notes": [{"clip": n1.clip.0, "note": n1.note.0}], "centre": 100})).unwrap();
    frame(&st.ctx, &mut st.app, vec![]);
    let (r2, l2) = soundcraft_ui_egui::vocal_track::lane_builds();
    assert_eq!((r2 - r1, l2 - l1), (1, 2), "the edit is drawn");
}

#[test]
fn double_click_on_the_line_does_not_select_the_clip() {
    // With the Selector a click places the insertion and a double-click on a clip selects it:
    // on the pitch line the double-click opens the editor instead.
    let mut st = pitch_setup(true);
    st.app.engine.session_mut().edit.tool = soundcraft_model::Tool::Selector;
    st.app.engine.revision += 1;
    settle(&st.ctx, &mut st.app);
    let drawn = segs(&st.ctx, st.clips[0]);
    let p = drawn[0].pts[drawn[0].pts.len() / 2];
    frame(&st.ctx, &mut st.app, vec![Event::PointerMoved(p)]);
    for _ in 0..2 {
        frame(&st.ctx, &mut st.app, vec![button(p, true, Modifiers::NONE)]);
        frame(&st.ctx, &mut st.app, vec![button(p, false, Modifiers::NONE)]);
    }
    settle(&st.ctx, &mut st.app);
    assert!(st.app.ui.show_vocal_editor);
    assert!(!st.app.engine.session().edit.selected_clips.contains(&st.clips[0]), "the double-click was the line's");
}

#[test]
fn badge_sits_after_the_measured_name() {
    let mut st = pitch_setup(true);
    st.app.run("clip.rename", json!({"clip": st.clips[0].0, "name": "WWWW WWWW"})).unwrap();
    let n1 = notes(&st.app, st.track)[0];
    st.app.run("vocal.edit", json!({"notes": [{"clip": n1.clip.0, "note": n1.note.0}], "centre": 100})).unwrap();
    settle(&st.ctx, &mut st.app);
    let l = lane(&st.app, &st);
    let tl = Rect::from_min_max(pos2(l.min.x, 0.0), pos2(l.max.x, 1000.0));
    let s = st.app.engine.session();
    let (_, c) = s.find_clip(st.clips[0]).unwrap();
    let (r, _) = soundcraft_ui_egui::vocal_track::tuned_badge(&st.ctx, s, tl, l, c).expect("a badge");
    let name_w = st.ctx.fonts_mut(|f| f.layout_no_wrap(c.name.clone(), soundcraft_ui_egui::theme::bold(10.5), egui::Color32::WHITE).size().x);
    let x0 = soundcraft_ui_egui::edit_window::x_of(s, tl, c.start).max(l.min.x);
    assert!(r.min.x >= x0 + 4.0 + name_w, "badge {} over the name ending at {}", r.min.x, x0 + 4.0 + name_w);
    assert!(r.min.x <= x0 + 4.0 + name_w + 12.0, "right after it");
}

#[test]
fn tuned_line_shows_on_tall_headers_only() {
    let mut st = pitch_setup(true);
    let n1 = notes(&st.app, st.track)[0];
    st.app.run("vocal.edit", json!({"notes": [{"clip": n1.clip.0, "note": n1.note.0}], "centre": 100})).unwrap();
    settle(&st.ctx, &mut st.app);
    let key = format!("soundcraft-vocal-tuned-line-{}", st.track.0);
    let shown = |ctx: &egui::Context| ctx.data(|d| d.get_temp::<bool>(egui::Id::new(key.clone())));
    assert_eq!(shown(&st.ctx), Some(true));
    st.app.engine.session_mut().track_mut(st.track).unwrap().height = TrackHeight::Custom(100);
    st.app.engine.revision += 1;
    settle(&st.ctx, &mut st.app);
    assert_eq!(shown(&st.ctx), Some(false), "under 112 px the rows below keep their room");
}

#[test]
fn vocal_clip_with_missing_audio_says_it_cant_be_analysed() {
    let mut s = soundcraft_model::Session::default();
    let t = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, Some("Lead Vox"));
    let clip = place(&mut s, t, take2(), 0);
    let src = s.find_clip(clip).and_then(|(_, c)| c.source()).unwrap();
    s.pool.remove(src);
    let track = s.track(t).unwrap();
    let c = s.find_clip(clip).unwrap().1;
    assert_eq!(soundcraft_ui_egui::vocal_track::lane_hint(&s, track, c), Some("Can't be analysed"));
}
