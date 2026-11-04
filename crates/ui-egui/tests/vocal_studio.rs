//! The Vocal Studio (spec 2026-11-03-vocal-studio-design.md): every vocal track in one pitch
//! grid, the editing track editable and the others faint reference, the track list.

mod vocal_support;

use egui::{Id, Rect};
use serde_json::json;
use soundcraft_engine::vocal::NoteRef;
use soundcraft_model::{ChannelFormat, TrackId, TrackKind};
use vocal_support::*;

/// A BV a third under take2's notes (185 Hz, 196 Hz), 0.5 s each with 0.2 s between.
fn bv_take() -> Vec<f32> {
    let mut x = vowel(185.0, 0.5, -21.0);
    x.extend(gap(0.2));
    x.extend(vowel(196.0, 0.5, -21.0));
    x
}

/// Lead Vox (take2 at 0.25 s) and BV 1 (a third under it), analysed, the studio open on the
/// lead.
fn studio() -> (Setup, TrackId) {
    let mut st = setup();
    let s = st.app.engine.session_mut();
    let bv = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("BV 1"));
    let c = place(s, bv, bv_take(), i64::from(RATE) / 4);
    soundcraft_engine::vocal::run_until_idle(&mut st.app.engine, &[c]).unwrap();
    st.app.run("ui.vocal_studio", json!({"value": true})).unwrap();
    settle(&st.ctx, &mut st.app);
    (st, bv)
}

fn refs(ctx: &egui::Context) -> Vec<(TrackId, NoteRef, Rect)> {
    ctx.data(|d| d.get_temp(Id::new("soundcraft-vocal-studio-refs"))).unwrap_or_default()
}

fn rows(ctx: &egui::Context) -> Vec<(TrackId, Rect)> {
    ctx.data(|d| d.get_temp(Id::new("soundcraft-vocal-studio-rows"))).unwrap_or_default()
}

#[test]
fn every_vocal_track_shows_and_only_the_editing_one_is_editable() {
    let (Setup { ctx, mut app, track, .. }, bv) = studio();
    assert_eq!(rows(&ctx).iter().map(|r| r.0).collect::<Vec<_>>(), [track, bv], "both vocal tracks listed");
    let r = refs(&ctx);
    assert_eq!(r.len(), 2, "BV 1's two notes are reference: {r:?}");
    assert!(r.iter().all(|(t, _, _)| *t == bv));
    let edit_bars = shapes(&ctx);
    assert_eq!(edit_bars.len(), 2, "the lead's notes are the editable ones");
    let lead_notes = notes(&app, track);
    assert!(edit_bars.iter().all(|s| lead_notes.contains(&s.r)));
    // The BV's notes sit under the lead's (a third lower).
    assert!(r[0].2.center().y > edit_bars[0].rect.center().y);
    // A click on a reference note selects nothing of it.
    click(&ctx, &mut app, r[0].2.center());
    assert!(app.vocal.selected.iter().all(|n| lead_notes.contains(n)), "reference notes are not editable");
    // Rubber-band over everything selects the editing track's notes only.
    app.run("edit.select", json!({"tracks": [track.0]})).unwrap();
    let v = view(&ctx);
    drag(&ctx, &mut app, v.rect.left_top() + egui::vec2(4.0, 4.0), v.rect.right_bottom() - egui::vec2(4.0, 4.0), 6);
    assert_eq!(app.vocal.selected.len(), 2);
    assert!(app.vocal.selected.iter().all(|n| lead_notes.contains(n)));
}

#[test]
fn double_click_or_the_list_switches_the_editing_track() {
    let (Setup { ctx, mut app, track, .. }, bv) = studio();
    let r = refs(&ctx);
    let at = r[0].2.center();
    frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
    for _ in 0..2 {
        frame(&ctx, &mut app, vec![button(at, true, egui::Modifiers::NONE)]);
        frame(&ctx, &mut app, vec![button(at, false, egui::Modifiers::NONE)]);
    }
    settle(&ctx, &mut app);
    assert_eq!(app.vocal.track, Some(bv), "a double-click on BV 1's note edits BV 1");
    let bv_notes = notes(&app, bv);
    assert!(shapes(&ctx).iter().all(|s| bv_notes.contains(&s.r)));
    assert!(refs(&ctx).iter().all(|(t, _, _)| *t == track), "the lead is reference now");
    // The list: a click on the lead's row.
    let lead_row = rows(&ctx).into_iter().find(|(t, _)| *t == track).unwrap().1;
    click(&ctx, &mut app, lead_row.left_center() + egui::vec2(40.0, 0.0));
    assert_eq!(soundcraft_ui_egui::vocal_editor::editor_track(&app), Some(track));
}

#[test]
fn hide_show_all_and_the_window_command() {
    let (Setup { ctx, mut app, .. }, bv) = studio();
    let v = app.run("ui.vocal_studio", json!({"hide": bv.0})).unwrap();
    settle(&ctx, &mut app);
    assert!(refs(&ctx).is_empty(), "hidden: {v}");
    assert_eq!(v["tracks"][1]["hidden"], true);
    app.run("ui.vocal_studio", json!({"show": "BV 1", "all": false})).unwrap();
    settle(&ctx, &mut app);
    assert!(refs(&ctx).is_empty(), "All vocals off: only the editing track");
    app.run("ui.vocal_studio", json!({"all": true})).unwrap();
    settle(&ctx, &mut app);
    assert_eq!(refs(&ctx).len(), 2);
    assert_eq!(app.run("window.vocal_studio", json!({})).unwrap()["value"], false);
    settle(&ctx, &mut app);
    assert!(!app.vocal.studio.on);
    assert!(app.ui.show_vocal_editor, "the plain editor stays");
    assert_eq!(app.run("window.vocal_studio", json!({})).unwrap()["value"], true);
    assert!(app.run("ui.vocal_studio", json!({"edit": "Nope"})).is_err());
}

#[test]
fn status_chips_follow_the_mix_pitch_box() {
    let (Setup { mut app, track, .. }, bv) = studio();
    let v = app.run("ui.vocal_studio", json!({})).unwrap();
    let status = |t: TrackId| v["tracks"].as_array().unwrap().iter().find(|x| x["track"] == t.0).unwrap()["status"].clone();
    // take2 sings −30 c and +20 c: off; the BV (185, 196 Hz) is within a few cents.
    assert_eq!(status(track), "off");
    assert_eq!(status(bv), "in tune");
}

fn control_rect(ctx: &egui::Context, name: &str) -> Rect {
    control(ctx, name).unwrap_or_else(|| panic!("no control {name}"))
}

#[test]
fn formant_from_the_note_bar_and_the_panel() {
    let Setup { ctx, mut app, track, clips } = setup();
    let ns = notes(&app, track);
    app.vocal.selected = [ns[0]].into_iter().collect();
    settle(&ctx, &mut app);
    let undo = app.engine.undo_history().len();
    let f = control_rect(&ctx, "bar-formant");
    drag(&ctx, &mut app, f.center(), f.center() + egui::vec2(25.0, 0.0), 5);
    let edit =
        |app: &soundcraft_ui_egui::SoundApp| app.engine.session().find_clip(ns[0].clip).unwrap().1.tune_live().unwrap().edits[&ns[0].note].formant_st;
    assert!((edit(&app) - 2.5).abs() < 0.11, "formant {}", edit(&app));
    assert_eq!(app.engine.undo_history().len(), undo + 1, "one gesture, one undo step");
    // The panel's clip field: dragged, sent once on release.
    let c = control_rect(&ctx, "formant-clip");
    let undo = app.engine.undo_history().len();
    drag(&ctx, &mut app, c.center(), c.center() + egui::vec2(-40.0, 0.0), 8);
    let clip_st = app.engine.session().find_clip(clips[0]).unwrap().1.tune_live().unwrap().formant_st;
    assert!((-2.1..=-1.0).contains(&clip_st), "clip formant {clip_st} (40 px at 0.05 st a px, less the drag threshold)");
    assert_eq!(app.engine.undo_history().len(), undo + 1);
    assert_eq!(app.engine.undo_label(), Some("Vocal Formant"));
}

/// Lead Vox (take2: A3, B3) and BV 1 singing A#3 (a minor 2nd against A3) then G3, the studio
/// editing BV 1.
fn clashing() -> (Setup, TrackId) {
    let mut st = setup();
    let s = st.app.engine.session_mut();
    let bv = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("BV 1"));
    let mut take = vowel(233.08, 0.5, -21.0);
    take.extend(gap(0.2));
    take.extend(vowel(196.0, 0.5, -21.0));
    let c = place(s, bv, take, i64::from(RATE) / 4);
    soundcraft_engine::vocal::run_until_idle(&mut st.app.engine, &[c]).unwrap();
    st.app.run("ui.vocal_studio", json!({"value": true, "edit": bv.0})).unwrap();
    settle(&st.ctx, &mut st.app);
    (st, bv)
}

fn markers(ctx: &egui::Context) -> Vec<(NoteRef, Rect, String)> {
    ctx.data(|d| d.get_temp(Id::new("soundcraft-vocal-studio-clashes"))).unwrap_or_default()
}

#[test]
fn clash_markers_explain_and_fit_to_lead_clears_them() {
    let (Setup { ctx, mut app, track, .. }, bv) = clashing();
    let m = markers(&ctx);
    assert_eq!(m.len(), 1, "{m:?}");
    assert_eq!(m[0].2, "minor 2nd against Lead Vox (A3)");
    let first = notes(&app, bv)[0];
    assert_eq!(m[0].0, first);
    // The marker sits right of its note.
    assert!(m[0].1.left() >= bar(&ctx, first).right());
    let v = app.run("ui.vocal_studio", json!({})).unwrap();
    assert_eq!(v["against"], track.0);
    assert_eq!(v["tracks"][1]["clashes"], 1);
    assert_eq!(v["clashes"][0]["kind"], "minor_second");
    // Hover explains.
    frame(&ctx, &mut app, vec![egui::Event::PointerMoved(m[0].1.center())]);
    settle(&ctx, &mut app);
    assert_eq!(text(&ctx, "tooltip").as_deref(), Some("minor 2nd against Lead Vox (A3)"));
    // Fit to lead (the bar's button): one undo step, no clash left.
    let undo = app.engine.undo_history().len();
    click(&ctx, &mut app, control(&ctx, "studio-fit").unwrap().center());
    assert!(markers(&ctx).is_empty(), "{}", app.ui.status);
    assert_eq!(app.engine.undo_history().len(), undo + 1);
    assert_eq!(app.engine.undo_label(), Some("Fit to Lead"));
    assert!(app.ui.status.starts_with("Fit to lead: moved 1"), "{}", app.ui.status);
    // Against nothing but the chord and key (here neither): no clash either way.
    assert!(app.engine.undo());
    settle(&ctx, &mut app);
    assert_eq!(markers(&ctx).len(), 1);
    app.run("ui.vocal_studio", json!({"against": bv.0})).unwrap();
    settle(&ctx, &mut app);
    assert!(markers(&ctx).is_empty(), "compared with itself: nothing");
}

#[test]
fn make_harmony_previews_then_creates_one_undo_step() {
    let (Setup { ctx, mut app, track, .. }, _bv) = studio();
    let v = app.run("ui.vocal_harmony", json!({"interval": "3rd", "direction": "up", "formant": 2.0})).unwrap();
    assert_eq!(v["source"], track.0, "from the lead");
    assert_eq!(v["notes"].as_array().unwrap().len(), 2);
    settle(&ctx, &mut app);
    let preview: Vec<Rect> = ctx.data(|d| d.get_temp(Id::new("soundcraft-vocal-harmony-preview-notes"))).unwrap_or_default();
    assert_eq!(preview.len(), 2, "dashed outlines where the harmony will sing");
    // Above the lead's notes (a third up).
    let lead_top = shapes(&ctx).iter().map(|s| s.rect.center().y).fold(f32::INFINITY, f32::min);
    assert!(preview.iter().all(|r| r.center().y < lead_top + 1.0), "{preview:?}");
    // Preview off hides them.
    click(&ctx, &mut app, control(&ctx, "harmony-preview").unwrap().center());
    let preview: Vec<Rect> = ctx.data(|d| d.get_temp(Id::new("soundcraft-vocal-harmony-preview-notes"))).unwrap_or_default();
    assert!(preview.is_empty());
    let tracks = app.engine.session().tracks.len();
    let undo = app.engine.undo_history().len();
    click(&ctx, &mut app, control(&ctx, "harmony-create").unwrap().center());
    assert_eq!(app.engine.session().tracks.len(), tracks + 1, "{}", app.ui.status);
    assert_eq!(app.engine.undo_history().len(), undo + 1);
    let made = app.vocal.track.unwrap();
    assert_eq!(app.engine.session().track(made).unwrap().name, "Harmony 3rd \u{2191}");
    assert!(app.vocal.studio.harmony.is_none(), "the form closes");
    assert!(rows(&ctx).iter().any(|(t, _)| *t == made), "listed, and edited");
    let clip = app.engine.session().track(made).unwrap().clips()[0].clone();
    assert_eq!(clip.tune_live().unwrap().formant_st, 2.0);
    assert!(app.engine.undo());
    assert_eq!(app.engine.session().tracks.len(), tracks);
    assert!(app.run("ui.vocal_harmony", json!({"interval": "2nd"})).is_err());
}

// ---- Edit All, Tune all, stacked unison notes (spec 2026-11-04-vocal-studio-edit-all-design.md) ----

/// The studio's track list as `ui.vocal_studio` reports it, for one track.
fn studio_track(app: &mut soundcraft_ui_egui::SoundApp, t: TrackId) -> serde_json::Value {
    let v = app.run("ui.vocal_studio", json!({})).unwrap();
    v["tracks"].as_array().unwrap().iter().find(|x| x["track"] == t.0).cloned().unwrap()
}

/// Each note's offset from its target on `t` (cents).
fn offsets(app: &soundcraft_ui_egui::SoundApp, t: TrackId) -> Vec<f32> {
    soundcraft_engine::vocal_studio::studio_notes(app.engine.session(), t).iter().map(|n| n.offset()).collect()
}

/// The stacks drawn last frame: notes, striped rect, mismatch.
fn stacks(ctx: &egui::Context) -> Vec<(Vec<NoteRef>, Rect, bool)> {
    ctx.data(|d| d.get_temp(Id::new("soundcraft-vocal-studio-stacks"))).unwrap_or_default()
}

/// Lead Vox (take2: A3 −30 c, B3 +20 c), Lead Double singing the same two notes with it (A3
/// +5 c: 35 c from the lead's, B3 +15 c: agreeing) and BV 2 a third under; the studio editing
/// the lead.
fn doubled() -> (Setup, TrackId, TrackId) {
    let mut st = setup();
    let s = st.app.engine.session_mut();
    let dbl = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead Double"));
    let mut take = vowel(cents(220.0, 5.0), 0.5, -21.0);
    take.extend(gap(0.2));
    take.extend(vowel(cents(247.0, 15.0), 0.5, -21.0));
    let c = place(s, dbl, take, i64::from(RATE) / 4);
    let bv = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("BV 2"));
    let b = place(s, bv, bv_take(), i64::from(RATE) / 4);
    soundcraft_engine::vocal::run_until_idle(&mut st.app.engine, &[c, b]).unwrap();
    st.app.run("ui.vocal_studio", json!({"value": true})).unwrap();
    settle(&st.ctx, &mut st.app);
    (st, dbl, bv)
}

#[test]
fn edit_all_edits_notes_on_several_tracks_and_one_undo_reverts_all() {
    let (Setup { ctx, mut app, track, .. }, dbl, bv) = doubled();
    let v = app.run("ui.vocal_studio", json!({"edit": "all"})).unwrap();
    settle(&ctx, &mut app);
    assert_eq!(v["edit"], "all");
    assert_eq!(v["editable"].as_array().unwrap().len(), 3, "{v}");
    // Every shown track's notes are editable bars; none is reference.
    let mut want: Vec<NoteRef> = [track, dbl, bv].iter().flat_map(|t| notes(&app, *t)).collect();
    want.sort();
    let mut drawn: Vec<NoteRef> = shapes(&ctx).iter().map(|s| s.r).collect();
    drawn.sort();
    assert_eq!(drawn, want);
    assert!(refs(&ctx).is_empty());
    // Rubber band over everything selects across tracks.
    let r = view(&ctx).rect;
    drag(&ctx, &mut app, r.left_top() + egui::vec2(4.0, 4.0), r.right_bottom() - egui::vec2(4.0, 4.0), 6);
    assert_eq!(app.vocal.selected.len(), 6, "{:?}", app.vocal.selected);
    // Centre note on them: one undo step, every track changed.
    let undo = app.engine.undo_history().len();
    click(&ctx, &mut app, control(&ctx, "studio-centre").unwrap().center());
    assert_eq!(app.engine.undo_history().len(), undo + 1, "{}", app.ui.status);
    let tuned = |app: &soundcraft_ui_egui::SoundApp, t: TrackId| app.engine.session().track(t).unwrap().clips()[0].tune_live().is_some();
    assert!(tuned(&app, track) && tuned(&app, dbl) && tuned(&app, bv));
    assert!(app.engine.undo());
    assert!(!tuned(&app, track) && !tuned(&app, dbl) && !tuned(&app, bv), "one undo reverts every track");
    // Dragging a BV note moves its pitch on its own track only.
    app.vocal.selected.clear();
    settle(&ctx, &mut app);
    let b0 = notes(&app, bv)[0];
    let from = bar(&ctx, b0).center();
    drag(&ctx, &mut app, from, from - egui::vec2(0.0, 40.0), 6);
    assert!(tuned(&app, bv) && !tuned(&app, track) && !tuned(&app, dbl));
    assert_eq!(app.engine.session().find_clip(b0.clip).unwrap().0, bv, "a note never changes track");
    // A name in the list: back to editing just that track.
    let row = rows(&ctx).into_iter().find(|(t, _)| *t == dbl).unwrap().1;
    click(&ctx, &mut app, row.left_center() + egui::vec2(40.0, 0.0));
    assert!(!app.vocal.studio.edit_all);
    assert_eq!(app.vocal.track, Some(dbl));
    assert!(shapes(&ctx).iter().all(|s| notes(&app, dbl).contains(&s.r)), "single-track editing as before");
}

#[test]
fn tune_all_skips_hidden_tracks_and_notes_inside_the_tolerance_in_one_step() {
    let (Setup { ctx, mut app, track, .. }, dbl, bv) = doubled();
    app.run("ui.vocal_studio", json!({"edit": "all", "hide": bv.0})).unwrap();
    settle(&ctx, &mut app);
    assert!(shapes(&ctx).iter().all(|s| !notes(&app, bv).contains(&s.r)), "hidden: not editable");
    let (l0, d0) = (offsets(&app, track), offsets(&app, dbl));
    assert!(d0[0].abs() <= 10.0 && d0[1].abs() > 10.0, "double {d0:?}");
    let undo = app.engine.undo_history().len();
    click(&ctx, &mut app, control(&ctx, "studio-tune-all").unwrap().center());
    assert_eq!(app.engine.undo_history().len(), undo + 1, "{}", app.ui.status);
    assert_eq!(app.engine.undo_label(), Some("Tune All"));
    assert!(app.ui.status.starts_with("Tune all: tuned 3 notes on 2 tracks"), "{}", app.ui.status);
    let (l1, d1) = (offsets(&app, track), offsets(&app, dbl));
    assert!(l1.iter().all(|o| o.abs() < 1.0), "lead to the centre: {l0:?} → {l1:?}");
    assert_eq!(d1[0], d0[0], "inside the tolerance: untouched");
    assert!(d1[1].abs() < 1.0);
    assert!(app.engine.session().track(bv).unwrap().clips()[0].tune.is_none(), "the hidden track is untouched");
    // The chips follow.
    assert_eq!(studio_track(&mut app, track)["status"], "in tune");
    assert!(app.engine.undo());
    assert_eq!(offsets(&app, track), l0);
    // Fix to Edge: the corrected notes stop at ±tolerance.
    let v = app.run("ui.vocal_studio", json!({"fix_to": "edge", "tolerance": 12, "tune_all": true})).unwrap();
    assert_eq!((v["fix_to"].as_str(), v["tolerance"].as_f64()), (Some("edge"), Some(12.0)));
    let l2 = offsets(&app, track);
    assert!((l2[0] + 12.0).abs() < 1.0 && (l2[1] - 12.0).abs() < 1.0, "{l2:?}");
}

#[test]
fn overlapping_notes_stripe_full_height_and_the_picker_picks() {
    let (Setup { ctx, mut app, track, .. }, dbl, _bv) = doubled();
    app.run("ui.vocal_studio", json!({"edit": "all"})).unwrap();
    settle(&ctx, &mut app);
    let st = stacks(&ctx);
    assert_eq!(st.len(), 2, "two unison pairs: {st:?}");
    let (ln, dn) = (notes(&app, track), notes(&app, dbl));
    assert!(st[0].0.contains(&ln[0]) && st[0].0.contains(&dn[0]));
    // Full height: the striped spot covers both bars' height, not half a row each.
    for r in [ln[0], dn[0]] {
        let b = bar(&ctx, r);
        assert!(st[0].1.top() <= b.top() + 0.5 && st[0].1.bottom() >= b.bottom() - 0.5, "{:?} vs {b:?}", st[0].1);
    }
    // ≠ only above the tolerance: 35 c apart is, 5 c isn't.
    assert!(st[0].2 && !st[1].2);
    assert_eq!(studio_track(&mut app, dbl)["chip"], "1 \u{2260}");
    // A click opens the pick list; its second row selects the double's note.
    click(&ctx, &mut app, st[0].1.center());
    assert!(control(&ctx, "picker").is_some(), "the pick list is open");
    assert_eq!(text(&ctx, "picker-title").as_deref(), Some("A3 \u{b7} 2 notes here \u{2014} pick one to edit"));
    let row = control(&ctx, "picker-row-1").unwrap();
    click(&ctx, &mut app, row.center());
    assert_eq!(app.vocal.selected.iter().copied().collect::<Vec<_>>(), [dn[0]]);
    assert!(app.vocal.studio.picker.is_none());
    // Both selects both.
    click(&ctx, &mut app, st[0].1.center());
    click(&ctx, &mut app, control(&ctx, "picker-both").unwrap().center());
    assert_eq!(app.vocal.selected.len(), 2);
    // Match the double to the lead: one step, the pitches equal, the ≠ gone.
    click(&ctx, &mut app, st[0].1.center());
    let undo = app.engine.undo_history().len();
    click(&ctx, &mut app, control(&ctx, "picker-match").unwrap().center());
    assert_eq!(app.engine.undo_history().len(), undo + 1, "{}", app.ui.status);
    let pitch = |app: &soundcraft_ui_egui::SoundApp, t: TrackId| soundcraft_engine::vocal_studio::studio_notes(app.engine.session(), t)[0].pitch;
    assert!((pitch(&app, track) - pitch(&app, dbl)).abs() < 0.5);
    assert!(!stacks(&ctx)[0].2, "matched: no ≠");
    // Esc closes the list.
    click(&ctx, &mut app, stacks(&ctx)[0].1.center());
    press(&ctx, &mut app, stacks(&ctx)[0].1.center(), egui::Key::Escape, egui::Modifiers::NONE);
    assert!(app.vocal.studio.picker.is_none());
}

#[test]
fn single_track_editing_stripes_a_reference_under_the_note_too() {
    let (Setup { ctx, mut app, track, .. }, dbl, _bv) = doubled();
    assert_eq!(soundcraft_ui_egui::vocal_editor::editor_track(&app), Some(track));
    assert!(!app.vocal.studio.edit_all);
    let st = stacks(&ctx);
    assert_eq!(st.len(), 2, "the double under the lead's notes: {st:?}");
    // Editable notes are the lead's only, as before.
    assert!(shapes(&ctx).iter().all(|s| notes(&app, track).contains(&s.r)));
    // The list offers no Both (the double isn't editable); picking the double edits its track.
    click(&ctx, &mut app, st[0].1.center());
    assert!(control(&ctx, "picker").is_some());
    assert!(ctx.data(|d| d.get_temp::<Rect>(Id::new("soundcraft-vocal-picker-both"))).is_none());
    click(&ctx, &mut app, control(&ctx, "picker-row-1").unwrap().center());
    assert_eq!(app.vocal.track, Some(dbl));
    assert_eq!(app.vocal.selected.iter().copied().collect::<Vec<_>>(), [notes(&app, dbl)[0]]);
}
