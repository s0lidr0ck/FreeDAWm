//! The Vocal Editor's pure drawing maths: note colours, thickness and labels (`vocal_style`), and
//! the roll geometry (`vocal_editor::layout`): bars at the tuned centre with a ghost, curves
//! broken at unvoiced frames, culling to the visible range.

use egui::{Color32, Rect, pos2};
use soundcraft_model::{Clip, ClipId, SourceId};
use soundcraft_ui_egui::vocal_editor::layout::{RollView, curve_points, note_shapes};
use soundcraft_ui_egui::vocal_style::{fmt_cents, note_name, note_thickness, off_colour};
use soundcraft_vocal::{EditCurves, Note, NoteId, PitchFrame, PitchTrack, VocalAnalysis, target_cents};

const RATE: u32 = 48_000;
const HOP: u32 = 240;

fn track(n: usize, cents: impl Fn(usize) -> Option<f32>) -> PitchTrack {
    let frames = (0..n).map(|i| PitchFrame { cents: cents(i), level_db: -21.0, voiced_prob: if cents(i).is_some() { 0.9 } else { 0.0 } }).collect();
    PitchTrack { sample_rate: RATE, hop_samples: HOP, frames }
}

fn note(start: u32, end: u32, centre: f32, level: f32) -> Note {
    Note { id: NoteId(start), start, end, scoop: None, centre_cents: centre, level_db: level, drift_cents: 0.0, vibrato_cents: 0.0 }
}

fn clip(len_secs: i64) -> Clip {
    Clip::audio(ClipId(1), "Vox", SourceId(1), 0, 0, len_secs * i64::from(RATE))
}

fn view(t1: i64) -> RollView {
    RollView { rect: Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 400.0)), t0: 0, t1, top_cents: 7200.0, row_h: 20.0 }
}

fn target(n: &Note) -> f32 {
    target_cents(n.centre_cents, None)
}

#[test]
fn colour_stops_match_the_mockup() {
    assert_eq!(off_colour(0.0), Color32::from_rgb(63, 208, 176));
    assert_eq!(off_colour(-22.0), Color32::from_rgb(226, 196, 72));
    assert_eq!(off_colour(48.0), off_colour(120.0));
    assert_eq!(off_colour(48.0), Color32::from_rgb(236, 72, 72));
    assert_eq!(off_colour(5.0), Color32::from_rgb(80, 207, 158));
    assert_eq!(off_colour(f32::NAN), Color32::from_rgb(63, 208, 176));
}

#[test]
fn thickness_follows_loudness() {
    assert_eq!(note_thickness(-31.0, -21.0, 20.0), 6.0);
    assert_eq!(note_thickness(-15.0, -21.0, 20.0), 28.0);
    assert_eq!(note_thickness(-15.0, -21.0, 40.0), 56.0);
}

#[test]
fn labels() {
    assert_eq!(fmt_cents(-22.4), "\u{2212}22c");
    assert_eq!(fmt_cents(7.0), "+7c");
    assert_eq!(fmt_cents(0.2), "\u{b1}0c");
    assert_eq!(note_name(6759.0), "G#4");
    assert_eq!(note_name(6900.0), "A4");
}

#[test]
fn note_bar_sits_at_the_tuned_centre_with_a_ghost() {
    let tr = track(300, |i| (100..200).contains(&i).then_some(6759.0));
    let a = VocalAnalysis { track: tr, notes: vec![note(100, 200, 6759.0, -21.0)], unvoiced: vec![] };
    let shift: Vec<f32> = (0..300).map(|i| if (100..200).contains(&i) { 16.4 } else { 0.0 }).collect();
    let curves = EditCurves { shift_cents: shift, gain_db: vec![0.0; 300] };
    let v = view(i64::from(RATE) * 2);
    let shapes = note_shapes(&v, &clip(2), &a, Some(&curves), -21.0, target, |_| None);
    assert_eq!(shapes.len(), 1);
    let s = &shapes[0];
    assert!((s.rect.center().y - v.y(6775.4)).abs() <= 0.5, "bar at {} want {}", s.rect.center().y, v.y(6775.4));
    let g = s.ghost.expect("a moved note keeps a ghost");
    assert!((g.center().y - v.y(6759.0)).abs() <= 0.5);
    assert!((s.offset - -41.0).abs() < 0.01 && (s.tuned_offset - -24.6).abs() < 0.01, "{} {}", s.offset, s.tuned_offset);
    assert!((s.rect.height() - 19.75).abs() < 0.01, "typical level: 6 + 22·10/16 px");
    assert!((s.rect.left() - v.x(100 * i64::from(HOP))).abs() < 0.5);
    assert_eq!(s.colour, off_colour(s.tuned_offset));
    // Untuned: no ghost.
    let plain = note_shapes(&v, &clip(2), &a, None, -21.0, target, |_| None);
    assert!(plain[0].ghost.is_none());
    assert!((plain[0].rect.center().y - v.y(6759.0)).abs() <= 0.5);
}

#[test]
fn curves_break_at_unvoiced_frames() {
    let tr = track(20, |i| (!(10..12).contains(&i)).then_some(6000.0 + i as f32));
    let v = view(i64::from(RATE));
    let lines = curve_points(&v, &clip(1), &tr, None);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0].len(), 10);
    assert_eq!(lines[1].len(), 8);
    // A jump of more than 250 c breaks the line too, and a shift moves it.
    let jump = track(20, |i| Some(if i < 10 { 6000.0 } else { 6400.0 }));
    assert_eq!(curve_points(&v, &clip(1), &jump, None).len(), 2);
    let shift = vec![100.0; 20];
    let up = curve_points(&v, &clip(1), &tr, Some(&shift));
    assert!((up[0][0].y - v.y(6100.0)).abs() < 0.01);
}

#[test]
fn culls_to_visible_range() {
    // 5000 notes over 10 minutes; a 10 s view.
    let frames = 120_000usize;
    let tr = track(frames, |i| Some(6000.0 + (i % 7) as f32));
    let notes: Vec<Note> = (0..5000u32).map(|k| note(k * 24, k * 24 + 20, 6000.0, -21.0)).collect();
    let a = VocalAnalysis { track: tr, notes, unvoiced: vec![] };
    let rate = i64::from(RATE);
    let v = RollView { t0: 60 * rate, t1: 70 * rate, ..view(0) };
    let in_range = a.notes.iter().filter(|n| i64::from(n.end) * i64::from(HOP) > v.t0 && i64::from(n.start) * i64::from(HOP) < v.t1).count();
    let t = std::time::Instant::now();
    let shapes = note_shapes(&v, &clip(600), &a, None, -21.0, target, |_| None);
    let lines = curve_points(&v, &clip(600), &a.track, None);
    let took = t.elapsed();
    // Each bar names its note's index in the analysis (no search per bar to find the note).
    assert!(shapes.iter().all(|s| a.notes.get(s.index).is_some_and(|n| n.id == s.r.note)), "indices match the notes");
    assert!(shapes.len() <= in_range + 2 && !shapes.is_empty(), "{} shapes for {in_range} notes", shapes.len());
    let pts: usize = lines.iter().map(Vec::len).sum();
    assert!(pts <= 10 * 200 + 4, "{pts} curve points for a 10 s view");
    assert!(took.as_millis() < 5, "took {took:?}");
}
