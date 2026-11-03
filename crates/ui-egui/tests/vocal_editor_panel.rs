//! The Vocal Editor's Tuning panel: the summary, live sliders (one undo step per drag, across
//! clips), mixed values, the bipolar Vibrato, macros and user presets, Tuned/Original, Reset,
//! Remove and Commit, the key and chord lines, Snap to key.

mod vocal_support;

use egui::{Event, Modifiers, pos2};
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::vocal::NoteRef;
use soundcraft_model::{ChannelFormat, Clip, Session, TrackKind};
use soundcraft_ui_egui::vocal_editor::presets;
use soundcraft_ui_egui::{Services, SoundApp};
use soundcraft_vocal::NoteEdit;
use vocal_support::*;

fn edit(app: &SoundApp, r: NoteRef) -> NoteEdit {
    let s = app.engine.session();
    s.find_clip(r.clip).and_then(|(_, c)| c.tune_live()).and_then(|t| t.edits.get(&r.note)).cloned().unwrap_or_default()
}

fn select(ctx: &egui::Context, app: &mut SoundApp, refs: &[NoteRef]) {
    app.vocal.selected = refs.iter().copied().collect();
    settle(ctx, app);
}

/// The comp case: take2 in two clips, 0.25 s and 2 s.
fn comp() -> Setup {
    setup_with(vec![(take2(), i64::from(RATE) / 4), (take2(), 2 * i64::from(RATE))], true)
}

/// Drags the slider `name` from its left end to `frac` of its width over `steps` frames; calls
/// `mid` once halfway.
fn drag_slider(ctx: &egui::Context, app: &mut SoundApp, name: &str, from: f32, frac: f32, steps: usize, mut mid: impl FnMut(&SoundApp)) {
    let r = control(ctx, name).unwrap_or_else(|| panic!("no slider {name}"));
    let a = pos2(r.left() + r.width() * from, r.center().y);
    let b = pos2(r.left() + r.width() * frac, r.center().y);
    frame(ctx, app, vec![Event::PointerMoved(a)]);
    frame(ctx, app, vec![button(a, true, Modifiers::NONE)]);
    for k in 1..=steps {
        frame(ctx, app, vec![Event::PointerMoved(a.lerp(b, k as f32 / steps as f32))]);
        if k == steps / 2 {
            mid(app);
        }
    }
    frame(ctx, app, vec![button(b, false, Modifiers::NONE)]);
    settle(ctx, app);
}

fn click_at(ctx: &egui::Context, app: &mut SoundApp, name: &str) {
    let r = control(ctx, name).unwrap_or_else(|| panic!("no control {name}"));
    click(ctx, app, r.center());
}

#[test]
fn summary_matches_the_selection() {
    let Setup { ctx, mut app, track, .. } = setup();
    assert_eq!(text(&ctx, "summary").as_deref(), Some("No notes selected \u{b7} click or drag in the roll"));
    let ns = notes(&app, track);
    select(&ctx, &mut app, &ns);
    let line = text(&ctx, "summary").unwrap();
    assert!(line.starts_with("2 notes selected \u{b7} "), "{line}");
    assert!(line.ends_with(" s"), "{line}");
    let line2 = text(&ctx, "summary2").unwrap();
    assert!(line2.starts_with("avg 25c off \u{2192} now 25c \u{b7} spread "), "{line2}");
}

#[test]
fn centre_drag_previews_and_is_one_undo_step() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    select(&ctx, &mut app, &ns);
    let undo = app.engine.undo_history().len();
    let mut mid_value = 0.0;
    let mut mid_status = String::new();
    drag_slider(&ctx, &mut app, "centre", 0.0, 0.40, 10, |app| {
        mid_value = edit(app, ns[0]).centre;
        mid_status = app.ui.status.clone();
    });
    assert!(mid_value > 0.0, "the drag previews: centre {mid_value} mid-drag");
    assert_eq!(mid_status, format!("Centre {:.0}% on 2 notes", mid_value * 100.0));
    assert_eq!(app.engine.undo_history().len(), undo + 1, "one drag, one undo step");
    assert_eq!(app.engine.undo_label(), Some("Tune Vocal Edit"));
    for r in &ns {
        assert!((edit(&app, *r).centre - 0.40).abs() <= 0.01, "{}", edit(&app, *r).centre);
    }
    assert_eq!(text(&ctx, "value-centre").as_deref(), Some("40%"));
}

#[test]
fn drag_across_clips_is_one_undo_step() {
    let Setup { ctx, mut app, track, clips } = comp();
    let ns = notes(&app, track);
    assert_eq!(ns.len(), 4);
    assert!(ns.iter().any(|r| r.clip == clips[1]));
    select(&ctx, &mut app, &ns);
    let undo = app.engine.undo_history().len();
    drag_slider(&ctx, &mut app, "drift", 0.0, 0.5, 10, |_| {});
    assert_eq!(app.engine.undo_history().len(), undo + 1);
    for r in &ns {
        assert!((edit(&app, *r).drift - 0.5).abs() <= 0.01, "{r:?}");
    }
    assert!(app.engine.undo());
    for c in &clips {
        assert!(app.engine.session().find_clip(*c).unwrap().1.tune.is_none(), "a single undo clears both clips");
    }
}

#[test]
fn a_note_cut_by_a_clip_edge_is_tuned_in_both_clips() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead Vox"));
    let take = take2();
    let len = take.len() as i64;
    let src = soundcraft_engine::io::add_source(&mut s, "Vox", AudioBuffer { sample_rate: RATE, channels: vec![take] }, None, FileFormat::Wav);
    let cut = i64::from(RATE) / 4; // inside note 1 (0..0.5 s)
    let a = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(a, "Vox", src, 0, 0, cut));
    let b = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(b, "Vox", src, cut, cut, len - cut));
    let mut e = soundcraft_engine::Engine::new(s);
    soundcraft_engine::vocal::run_until_idle(&mut e, &[a, b]).unwrap();
    let mut app = SoundApp::new(e, None, Services::default());
    app.ui.show_vocal_editor = true;
    app.run("edit.select", json!({"tracks": [t.0]})).unwrap();
    let ctx = egui::Context::default();
    settle(&ctx, &mut app);
    let w = (app.edit_layout.timeline[2] - app.edit_layout.timeline[0]).max(100.0);
    app.engine.session_mut().edit.zoom.samples_per_px = 2.0 * f64::from(RATE) / f64::from(w);
    app.engine.session_mut().edit.zoom.scroll = 0;
    settle(&ctx, &mut app);
    let ns = notes(&app, t);
    let in_a = *ns.iter().find(|r| r.clip == a).unwrap();
    assert!(ns.iter().any(|r| r.clip == b && r.note == in_a.note), "the cut note shows in both clips: {ns:?}");
    click(&ctx, &mut app, bar(&ctx, in_a).center());
    assert_eq!(app.vocal.selected.len(), 2, "selecting it selects both halves");
    drag_slider(&ctx, &mut app, "centre", 0.0, 0.6, 8, |_| {});
    for c in [a, b] {
        assert!(edit(&app, NoteRef { clip: c, note: in_a.note }).centre > 0.5, "clip {c:?} tuned too");
    }
}

#[test]
fn mixed_values_show_mixed() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    app.run("vocal.edit", json!({"notes": [ns[0]], "centre": 20})).unwrap();
    app.run("vocal.edit", json!({"notes": [ns[1]], "centre": 50})).unwrap();
    select(&ctx, &mut app, &ns);
    assert_eq!(text(&ctx, "value-centre").as_deref(), Some("mixed (avg 35%)"));
    assert_eq!(text(&ctx, "value-drift").as_deref(), Some("0%"));
}

#[test]
fn vibrato_is_bipolar() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    select(&ctx, &mut app, &ns);
    drag_slider(&ctx, &mut app, "vibrato", 0.5, 0.3, 8, |_| {});
    let v = edit(&app, ns[0]).vibrato;
    assert!(v < -0.3 && v > -0.5, "{v}");
    let shown = text(&ctx, "value-vibrato").unwrap();
    assert!(shown.starts_with('\u{2212}'), "{shown}");
}

#[test]
fn apply_macro_with_no_selection_tunes_the_whole_track() {
    let Setup { ctx, mut app, track, .. } = setup();
    assert!(app.vocal.selected.is_empty());
    assert_eq!(text(&ctx, "preset").as_deref(), Some("First pass (gentle)"));
    assert_eq!(text(&ctx, "preset-values").as_deref(), Some("Centre 40 \u{b7} Drift 30 \u{b7} Vib \u{2212}10 \u{b7} Loud 25/25"));
    click_at(&ctx, &mut app, "apply");
    let ns = notes(&app, track);
    let levels: Vec<f32> = {
        let s = app.engine.session();
        let c = s.find_clip(ns[0].clip).unwrap().1;
        soundcraft_engine::vocal::analysis_for(s, c).unwrap().notes.iter().map(|n| n.level_db).collect()
    };
    let typical = soundcraft_vocal::typical_level_db(&levels);
    for r in &ns {
        let e = edit(&app, *r);
        assert_eq!((e.centre, e.drift, e.vibrato, e.raise, e.lower), (0.40, 0.30, -0.10, 0.25, 0.25), "{r:?}");
        assert!((e.typical_db.unwrap() - typical).abs() < 0.01, "the track's typical level");
    }
}

#[test]
fn save_preset_round_trips() {
    let Setup { ctx, mut app, .. } = setup();
    // No preset folder: Save… is greyed out.
    assert_eq!(app.preset_dir, None);
    let save = control(&ctx, "save").unwrap();
    click(&ctx, &mut app, save.center());
    assert_eq!(app.vocal.save_name, None, "Save… is disabled without a preset folder");
    let dir = std::env::temp_dir().join(format!("sc-vocal-presets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    app.preset_dir = Some(dir.clone());
    app.run("ui.vocal_preset_save", json!({"name": "Mine", "centre": 55, "drift": 20, "vibrato": -30, "raise": 10, "lower": 40})).unwrap();
    assert!(dir.join("Vocal Tuning").join("Mine.json").is_file());
    let all = presets::all(&app);
    let names: Vec<&str> = all.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["First pass (gentle)", "Like Melodyne 70 %", "Firm", "Mine"]);
    let mine = all.iter().find(|p| p.name == "Mine").unwrap();
    assert_eq!((mine.centre, mine.drift, mine.vibrato, mine.raise, mine.lower), (0.55, 0.20, -0.30, 0.10, 0.40));
    // Save… now opens the name field.
    settle(&ctx, &mut app);
    click_at(&ctx, &mut app, "save");
    assert!(app.vocal.save_name.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tuned_original_switch_reset_remove_commit() {
    let Setup { ctx, mut app, track, clips } = setup();
    let ns = notes(&app, track);
    select(&ctx, &mut app, &ns);
    drag_slider(&ctx, &mut app, "centre", 0.0, 0.5, 6, |_| {});
    let tune = |app: &SoundApp| app.engine.session().find_clip(clips[0]).unwrap().1.tune.clone();
    assert!(tune(&app).is_some());
    click_at(&ctx, &mut app, "switch-original");
    assert!(tune(&app).unwrap().original, "Original plays the clip untuned");
    click_at(&ctx, &mut app, "switch-tuned");
    assert!(!tune(&app).unwrap().original);
    // Reset clears the selection's edits (the clip drops its tuning).
    select(&ctx, &mut app, &[ns[0]]);
    click_at(&ctx, &mut app, "reset");
    assert_eq!(edit(&app, ns[0]), NoteEdit::default());
    assert!(edit(&app, ns[1]).centre > 0.4, "only the selection");
    click_at(&ctx, &mut app, "remove");
    assert!(tune(&app).is_none(), "Remove drops the tuning");
    assert!(app.engine.undo());
    assert!(tune(&app).is_some());
    settle(&ctx, &mut app);
    click_at(&ctx, &mut app, "commit");
    let (_, c) = app.engine.session().find_clip(clips[0]).map(|(t, c)| (t, c.clone())).unwrap();
    assert!(c.tune.is_none(), "Commit drops the tuning");
    let src = c.source().and_then(|id| app.engine.session().sources.iter().find(|x| x.id == id)).map(|x| x.name.clone()).unwrap_or_default();
    assert!(src.ends_with("-tuned"), "{src}");
}

#[test]
fn key_and_chord_lines_read_the_session() {
    let Setup { ctx, mut app, .. } = setup();
    app.engine.session_mut().band.key = 4;
    app.engine.session_mut().band.scale = 0;
    let end = 4 * i64::from(RATE);
    app.run("band.chord.add", json!({"at": 0, "end": end, "chord": "E"})).unwrap();
    settle(&ctx, &mut app);
    assert_eq!(text(&ctx, "key-line").as_deref(), Some("Key E major \u{b7} from session"));
    assert_eq!(text(&ctx, "chord-line").as_deref(), Some("Chord at playhead E \u{b7} from chord track"));
}

#[test]
fn no_key_set_greys_out_snap_and_judges_no_note_off_key() {
    let Setup { ctx, mut app, track, .. } = setup();
    // A new session sets no key: no key change, no band chords or sections, the default key.
    assert_eq!(text(&ctx, "key-line").as_deref(), Some("No key set"));
    assert_eq!(text(&ctx, "key-strip").as_deref(), Some("No key set \u{b7} set key & chords in the timeline, not here"));
    assert_eq!(text(&ctx, "chord-line").as_deref(), Some("No chord track"));
    let ns = notes(&app, track);
    select(&ctx, &mut app, &ns);
    assert_eq!(text(&ctx, "snap-why").as_deref(), Some("Set a key in the timeline first"));
    click_at(&ctx, &mut app, "snap");
    assert!(ns.iter().all(|r| !edit(&app, *r).snap_to_key), "Snap to key is greyed out without a key");
    let off: Vec<NoteRef> = ctx.data(|d| d.get_temp(egui::Id::new("soundcraft-vocal-off-key"))).unwrap_or_default();
    assert!(off.is_empty(), "no note is off key without a key: {off:?}");
}

#[test]
fn a_key_set_enables_snap_and_off_key_outlines() {
    let Setup { ctx, mut app, track, .. } = setup();
    // C# major: the take's A and B are outside it.
    app.engine.session_mut().band.key = 1;
    settle(&ctx, &mut app);
    assert_eq!(text(&ctx, "key-line").as_deref(), Some("Key C# major \u{b7} from session"));
    let strip = text(&ctx, "key-strip").unwrap_or_default();
    assert!(strip.starts_with("C# major"), "{strip}");
    let off: Vec<NoteRef> = ctx.data(|d| d.get_temp(egui::Id::new("soundcraft-vocal-off-key"))).unwrap_or_default();
    assert!(!off.is_empty(), "notes outside C# major are outlined");
    let ns = notes(&app, track);
    select(&ctx, &mut app, &ns);
    assert_eq!(text(&ctx, "snap-why").as_deref(), Some(""));
    click_at(&ctx, &mut app, "snap");
    assert!(ns.iter().all(|r| edit(&app, *r).snap_to_key));
}

#[test]
fn a_key_counts_as_set_by_a_key_change_chords_sections_or_band_key() {
    use soundcraft_ui_egui::vocal_editor::roll::has_key;
    let mut s = Session::default();
    assert!(!has_key(&s));
    s.key_signatures.push((0, "nonsense".into()));
    assert!(!has_key(&s), "an unparsable key change sets nothing");
    s.key_signatures.push((0, "E minor".into()));
    assert!(has_key(&s));
    let mut s = Session::default();
    s.band.scale = 1;
    assert!(has_key(&s), "A band key other than the default");
    let mut e = soundcraft_engine::Engine::new(Session::default());
    e.execute("band.chord.add", &json!({"at": 0, "end": 48_000, "chord": "E"})).unwrap();
    assert!(has_key(e.session()), "band chords");
    let mut s = Session::default();
    s.band.sections.push(Default::default());
    assert!(has_key(&s), "band sections");
}

#[test]
fn a_user_preset_cannot_take_a_factory_name() {
    let Setup { mut app, .. } = setup();
    let dir = std::env::temp_dir().join(format!("sc-vocal-presets-factory-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    app.preset_dir = Some(dir.clone());
    let e = app.run("ui.vocal_preset_save", json!({"name": "firm", "centre": 10})).unwrap_err();
    assert!(e.contains("name in use"), "{e}");
    assert!(app.run("ui.vocal_preset_save", json!({"name": "  "})).is_err(), "an empty name is still refused");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_refused_slider_send_is_reported_in_the_status_bar() {
    // MIDI Cleanup's and the Tuning panel's sliders share `slider_drag::drive`: a send the engine
    // refuses goes through the same report as their other controls.
    use soundcraft_ui_egui::slider_drag;
    let mut app = SoundApp::new(soundcraft_engine::Engine::new(Session::default()), None, Services::default());
    let ctx = egui::Context::default();
    let r = egui::Rect::from_min_size(pos2(100.0, 100.0), egui::vec2(200.0, 20.0));
    let run = |app: &mut SoundApp, events: Vec<Event>| {
        let raw =
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), events, ..Default::default() };
        ctx.run_ui(raw, |ui| {
            let resp = ui.interact(r, egui::Id::new("test-slider"), egui::Sense::click_and_drag());
            slider_drag::drive(
                app,
                ui,
                &resp,
                r,
                egui::Id::new("test-slider-pending"),
                |k| k,
                |_, _, _| true,
                |_, _, _| Err("refused here".to_string()),
            );
        })
        .textures_delta
        .clear();
    };
    let a = pos2(105.0, 110.0);
    run(&mut app, vec![Event::PointerMoved(a)]);
    run(&mut app, vec![button(a, true, Modifiers::NONE)]);
    for k in 1..=5 {
        run(&mut app, vec![Event::PointerMoved(a + egui::vec2(20.0 * k as f32, 0.0))]);
    }
    run(&mut app, vec![button(a + egui::vec2(100.0, 0.0), false, Modifiers::NONE)]);
    assert_eq!(app.ui.status, "refused here");
}

#[test]
fn a_drag_that_loses_its_selection_never_says_0_notes() {
    let Setup { ctx, mut app, track, .. } = setup();
    let ns = notes(&app, track);
    select(&ctx, &mut app, &ns);
    let r = control(&ctx, "centre").unwrap();
    let a = pos2(r.left() + 2.0, r.center().y);
    frame(&ctx, &mut app, vec![Event::PointerMoved(a)]);
    frame(&ctx, &mut app, vec![button(a, true, Modifiers::NONE)]);
    frame(&ctx, &mut app, vec![Event::PointerMoved(a + egui::vec2(20.0, 0.0))]);
    app.vocal.selected.clear();
    let mut seen = Vec::new();
    for k in 2..6 {
        frame(&ctx, &mut app, vec![Event::PointerMoved(a + egui::vec2(20.0 * k as f32, 0.0))]);
        seen.push(app.ui.status.clone());
    }
    frame(&ctx, &mut app, vec![button(a + egui::vec2(100.0, 0.0), false, Modifiers::NONE)]);
    seen.push(app.ui.status.clone());
    assert!(seen.iter().all(|s| !s.contains("on 0 notes")), "{seen:?}");
}
