//! Task 4: edit maths (targets, decomposition, curves, levelling, transitions, selection summary,
//! factory macros).
//!
//! Tracks are built directly from cents vectors at 5 ms frames (48 kHz, hop 240); notes are made
//! with [`Note::new`] so their measures follow the crate's own rules.

use std::collections::BTreeMap;
use std::f32::consts::TAU;

use soundcraft_vocal::{
    EditCurves, Note, NoteEdit, NoteId, PitchClassSet, PitchFrame, PitchTrack, ScaleAt, Transition, UnvoicedRegion, VocalAnalysis, build_curves,
    decompose, factory_presets, join_notes, level_gain_db, moved_target, selection_summary, target_cents, tuned_centre, typical_level_db,
};

const LEVEL: f32 = -20.0;
const SILENT: f32 = -90.0;
const NO_SCALE: Option<PitchClassSet> = None;

/// Frame time (s) at 5 ms hops.
fn t(k: usize) -> f32 {
    k as f32 * 0.005
}

/// A track from `(cents, level)` frames.
fn track(frames: &[(Option<f32>, f32)]) -> PitchTrack {
    PitchTrack {
        sample_rate: 48_000,
        hop_samples: 240,
        frames: frames
            .iter()
            .map(|&(cents, level_db)| PitchFrame { cents, level_db, voiced_prob: if cents.is_some() { 0.9 } else { 0.0 } })
            .collect(),
    }
}

fn silence(n: usize) -> Vec<(Option<f32>, f32)> {
    vec![(None, SILENT); n]
}

fn voiced(cents: impl IntoIterator<Item = f32>) -> Vec<(Option<f32>, f32)> {
    cents.into_iter().map(|c| (Some(c), LEVEL)).collect()
}

/// An analysis of `frames` with notes `(start, end, scoop)` and unvoiced regions.
fn analysis(frames: &[(Option<f32>, f32)], notes: &[(u32, u32, Option<(u32, u32)>)], unvoiced: &[(u32, u32)]) -> VocalAnalysis {
    let track = track(frames);
    let notes = notes.iter().map(|&(s, e, sc)| Note::new(&track, s, e, sc)).collect();
    let unvoiced = unvoiced.iter().map(|&(start, end)| UnvoicedRegion { start, end }).collect();
    VocalAnalysis { track, notes, unvoiced }
}

fn edit(centre: f32) -> NoteEdit {
    NoteEdit { centre, ..NoteEdit::default() }
}

/// A single note of `cents` (one value per frame) between 10 silent frames on each side.
fn one_note(cents: Vec<f32>) -> VocalAnalysis {
    let n = cents.len() as u32;
    let mut f = silence(10);
    f.extend(voiced(cents));
    f.extend(silence(10));
    analysis(&f, &[(10, 10 + n, None)], &[])
}

fn curves_for(a: &VocalAnalysis, e: NoteEdit) -> EditCurves {
    let edits = BTreeMap::from([(a.notes[0].id, e)]);
    build_curves(a, &edits, &NO_SCALE)
}

/// Pitch after the shift over the first note's frames.
fn shifted_note(a: &VocalAnalysis, c: &EditCurves) -> (Vec<f32>, Vec<f32>) {
    let n = &a.notes[0];
    let x: Vec<f32> = (n.start..n.end).map(|i| a.track.frames[i as usize].cents.unwrap_or(0.0)).collect();
    let y: Vec<f32> = (n.start..n.end).map(|i| x[(i - n.start) as usize] + c.shift_cents[i as usize]).collect();
    (x, y)
}

fn std(xs: &[f32]) -> f32 {
    let m = xs.iter().sum::<f32>() / xs.len() as f32;
    (xs.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / xs.len() as f32).sqrt()
}

fn mean(xs: &[f32]) -> f32 {
    xs.iter().sum::<f32>() / xs.len() as f32
}

/// The middle 60 % of `xs`.
fn middle(xs: &[f32]) -> &[f32] {
    let k = xs.len() / 5;
    &xs[k..xs.len() - k]
}

/// `(slow std, fast std)` of `x`'s decomposition over its middle 60 %.
fn slow_fast_std(x: &[f32]) -> (f32, f32) {
    let (_, slow, fast) = decompose(x);
    (std(middle(&slow)), std(middle(&fast)))
}

#[test]
fn target_is_the_nearest_semitone() {
    assert_eq!(target_cents(6759.0, None), 6800.0);
    assert_eq!(target_cents(6540.0, None), 6500.0);
}

#[test]
fn snap_to_key_targets_the_scale() {
    let e_major = PitchClassSet::from_pcs(&[4, 6, 8, 9, 11, 1, 3]);
    assert_eq!(target_cents(6540.0, Some(e_major)), 6600.0); // F4+40 -> F#4, not F4
    assert_eq!(target_cents(6759.0, Some(e_major)), 6800.0); // G#4 in key
    assert!(e_major.contains(4) && !e_major.contains(5) && !e_major.contains(12));
    // Ties go to the lower.
    let c_major = PitchClassSet::from_pcs(&[0, 2, 4, 5, 7, 9, 11]);
    assert_eq!(target_cents(6600.0, Some(c_major)), 6500.0); // F# out: F 6500 / G 6700 tie -> F
    assert_eq!(target_cents(6650.0, Some(c_major)), 6700.0);
    // An empty scale falls back to the nearest semitone.
    assert_eq!(target_cents(6540.0, Some(PitchClassSet(0))), 6500.0);
}

#[test]
fn centre_40_moves_g_sharp_minus_41_to_minus_24_6() {
    // Flat note at 6759 c (G#4 −41 c, mockup); Centre 0.40.
    let a = one_note(vec![6759.0; 100]);
    let e = edit(0.40);
    let tc = tuned_centre(&a, &a.notes[0], &e, &NO_SCALE);
    assert!((tc - 6775.4).abs() < 0.1, "tuned centre {tc}");
    let c = curves_for(&a, e);
    for i in 15..105 {
        assert!((c.shift_cents[i] - 16.4).abs() < 1e-3, "shift[{i}] = {}", c.shift_cents[i]);
    }
}

#[test]
fn centre_keeps_vibrato_shape() {
    let a = one_note((0..200).map(|k| 6759.0 + 30.0 * (TAU * 6.0 * t(k)).sin()).collect());
    let c = curves_for(&a, edit(1.0));
    let (x, y) = shifted_note(&a, &c);
    let (_, fx) = slow_fast_std(&x);
    let (_, fy) = slow_fast_std(&y);
    assert!((fy / fx - 1.0).abs() < 0.02, "fast std {fx} -> {fy}");
    assert!((mean(middle(&y)) - mean(middle(&x)) - 41.0).abs() < 1.0, "the centre moved to the target");
}

#[test]
fn drift_30_removes_30_percent_of_slow_wander() {
    let a = one_note((0..200).map(|k| 6759.0 + 20.0 * (TAU * 1.0 * t(k)).sin() + 30.0 * (TAU * 6.0 * t(k)).sin()).collect());
    let c = curves_for(&a, NoteEdit { drift: 0.30, ..NoteEdit::default() });
    let (x, y) = shifted_note(&a, &c);
    let (sx, fx) = slow_fast_std(&x);
    let (sy, fy) = slow_fast_std(&y);
    assert!((sy / sx - 0.70).abs() < 0.05, "slow std {sx} -> {sy}");
    assert!((fy / fx - 1.0).abs() < 0.05, "fast std {fx} -> {fy}");
}

#[test]
fn vibrato_minus_10_scales_fast_part_by_0_9() {
    let a = one_note((0..200).map(|k| 6759.0 + 30.0 * (TAU * 6.0 * t(k)).sin()).collect());
    let c = curves_for(&a, NoteEdit { vibrato: -0.10, ..NoteEdit::default() });
    let (x, y) = shifted_note(&a, &c);
    let (_, fx) = slow_fast_std(&x);
    let (_, fy) = slow_fast_std(&y);
    assert!((fy / fx - 0.90).abs() < 0.03, "fast std {fx} -> {fy}");
    assert!((mean(middle(&y)) - mean(middle(&x))).abs() < 0.5, "centre moved");
}

#[test]
fn levelling_25_25_matches_melodyne_fit() {
    let typ = typical_level_db(&[-27.0, -21.0, -15.0]);
    assert_eq!(typ, -21.0);
    let g: Vec<f32> = [-27.0, -21.0, -15.0].iter().map(|&l| level_gain_db(l, typ, 0.25, 0.25)).collect();
    assert_eq!(g, [1.5, 0.0, -1.5]); // dL = -0.25 (L - typical)
    assert_eq!(level_gain_db(-27.0, typ, 0.25, 0.0), 1.5);
    assert_eq!(level_gain_db(-15.0, typ, 0.25, 0.0), 0.0);
    assert_eq!(typical_level_db(&[]), -120.0);
}

#[test]
fn level_gain_is_clamped() {
    assert_eq!(level_gain_db(-80.0, -21.0, 1.0, 1.0), 12.0);
    assert_eq!(level_gain_db(10.0, -21.0, 1.0, 1.0), -12.0);
    assert_eq!(level_gain_db(f32::NAN, -21.0, 1.0, 1.0), 0.0);
}

/// Edited note A `[10, 70)`, unvoiced region `[70, 80)`, unedited note B `[80, 140)`.
fn two_notes() -> VocalAnalysis {
    let mut f = silence(10);
    f.extend(voiced(vec![6759.0; 60]));
    f.extend(vec![(None, -30.0); 10]);
    f.extend(voiced(vec![6540.0; 60]));
    f.extend(silence(10));
    analysis(&f, &[(10, 70, None), (80, 140, None)], &[(70, 80)])
}

fn level_edit() -> NoteEdit {
    // A's level is −20 dB: above typical −25 by 5 dB, so lower 0.25 gives −1.25 dB.
    NoteEdit { centre: 1.0, raise: 0.25, lower: 0.25, typical_db: Some(-25.0), ..NoteEdit::default() }
}

#[test]
fn default_typical_db_means_no_levelling() {
    // Raise 0.25 on a note 5 dB above where a 0.0 dB default would have put the target: no
    // typical level was given, so no gain at all.
    let a = two_notes();
    let e = NoteEdit { centre: 1.0, raise: 0.25, ..NoteEdit::default() };
    assert_eq!(e.typical_db, None);
    let c = build_curves(&a, &BTreeMap::from([(NoteId(10), e)]), &NO_SCALE);
    assert!(c.gain_db.iter().all(|&g| g == 0.0), "gain without a typical level: {:?}", c.gain_db.iter().find(|&&g| g != 0.0));
    assert!(c.shift_cents[40] > 40.0, "the pitch edit still applies");
}

/// Notes `[10, 70)` and `[80, 140)`, both at 6759 c, with a 10-frame unvoiced region `[70, 80)`
/// between them.
fn gapped_pair() -> VocalAnalysis {
    let mut f = silence(10);
    f.extend(voiced(vec![6759.0; 60]));
    f.extend(vec![(None, -30.0); 10]);
    f.extend(voiced(vec![6759.0; 60]));
    f.extend(silence(10));
    analysis(&f, &[(10, 70, None), (80, 140, None)], &[(70, 80)])
}

#[test]
fn joined_note_across_a_gap_gets_a_centre_shift() {
    let mut a = gapped_pair();
    let mut edits = BTreeMap::new();
    assert_eq!(join_notes(&mut a, &mut edits, NoteId(10), NoteId(80)), Some(NoteId(10)));
    assert_eq!((a.notes.len(), a.notes[0].start, a.notes[0].end), (1, 10, 140));
    let c = build_curves(&a, &BTreeMap::from([(NoteId(10), edit(1.0))]), &NO_SCALE);
    for i in (15..70).chain(80..135) {
        assert!((c.shift_cents[i] - 41.0).abs() < 1e-3, "voiced frame {i}: shift {}", c.shift_cents[i]);
    }
    for i in 70..80 {
        assert_eq!(c.shift_cents[i], 0.0, "unvoiced frame {i}");
    }
}

#[test]
fn gain_is_zero_on_an_unvoiced_region_inside_an_edited_note() {
    let a = gapped_pair();
    let a = VocalAnalysis { notes: vec![Note::new(&a.track, 10, 140, None)], ..a };
    let c = build_curves(&a, &BTreeMap::from([(NoteId(10), level_edit())]), &NO_SCALE);
    let g = c.gain_db[40];
    assert!(g < -0.5, "the note is levelled: {g}");
    for i in 70..80 {
        assert_eq!(c.gain_db[i], 0.0, "gain inside the unvoiced region at {i}");
    }
    // 3-frame raised-cosine ramps at the region's edges, inside the note.
    for (i, j) in [(69, 80), (68, 81), (67, 82)] {
        assert!(c.gain_db[i] < 0.0 && c.gain_db[i] > g, "ramp out gain[{i}] = {}", c.gain_db[i]);
        assert!((c.gain_db[i] - c.gain_db[j]).abs() < 1e-6, "symmetric ramps {i}/{j}");
    }
    assert!(c.gain_db[69] > c.gain_db[68] && c.gain_db[68] > c.gain_db[67]);
    assert!((c.gain_db[66] - g).abs() < 1e-6 && (c.gain_db[83] - g).abs() < 1e-6);
}

#[test]
fn manual_gain_is_a_flat_gain_over_the_note() {
    let a = two_notes();
    let e = NoteEdit { gain_db: 3.0, ..NoteEdit::default() };
    assert!(!e.is_identity(), "a gain is an edit");
    assert!(NoteEdit::default().is_identity(), "0 dB is the identity");
    let c = build_curves(&a, &BTreeMap::from([(NoteId(10), e)]), &NO_SCALE);
    for i in 13..67 {
        assert!((c.gain_db[i] - 3.0).abs() < 1e-6, "flat gain[{i}] = {}", c.gain_db[i]);
    }
    for i in (10..13).chain(67..70) {
        assert!(c.gain_db[i] > 0.0 && c.gain_db[i] < 3.0, "ramp gain[{i}] = {}", c.gain_db[i]);
    }
    for i in (7..10).chain(70..150) {
        assert_eq!(c.gain_db[i], 0.0, "outside gain[{i}]");
    }
    assert!(c.shift_cents.iter().all(|&v| v == 0.0), "no pitch change");
    // It joins the levelling gain (−1.25 dB here) additively, in dB.
    let e = NoteEdit { gain_db: 2.0, ..level_edit() };
    let c = build_curves(&a, &BTreeMap::from([(NoteId(10), e)]), &NO_SCALE);
    assert!((c.gain_db[40] - 0.75).abs() < 1e-6, "levelling + gain: {}", c.gain_db[40]);
    // Clamped to ±12 dB; a non-finite value becomes 0.
    assert_eq!(NoteEdit { gain_db: 40.0, ..NoteEdit::default() }.clamped().gain_db, 12.0);
    assert_eq!(NoteEdit { gain_db: -40.0, ..NoteEdit::default() }.clamped().gain_db, -12.0);
    assert_eq!(NoteEdit { gain_db: f32::NAN, ..NoteEdit::default() }.clamped().gain_db, 0.0);
}

#[test]
fn note_edit_deserialises_from_a_partial_map() {
    // The equivalent of `{"centre":0.4}`: every other field takes its default.
    use serde::Deserialize;
    use serde::de::value::{Error, MapDeserializer};
    let map = MapDeserializer::<_, Error>::new([("centre", 0.4f32)].into_iter());
    let e = NoteEdit::deserialize(map).expect("partial NoteEdit");
    assert_eq!(e, NoteEdit { centre: 0.4, ..NoteEdit::default() });
    assert_eq!((e.typical_db, e.transition), (None, Transition::Keep));
    assert_eq!((e.offset_cents, e.gain_db), (0.0, 0.0), "fields added later default to 0");
}

#[test]
fn curves_are_zero_outside_edited_notes_and_unvoiced() {
    let a = two_notes();
    let edits = BTreeMap::from([(NoteId(10), level_edit()), (NoteId(80), NoteEdit::default())]);
    let c = build_curves(&a, &edits, &NO_SCALE);
    assert_eq!((c.shift_cents.len(), c.gain_db.len()), (150, 150));
    for i in (0..10).chain(70..150) {
        assert!(c.shift_cents[i] == 0.0 && c.gain_db[i] == 0.0, "frame {i}: shift {} gain {}", c.shift_cents[i], c.gain_db[i]);
    }
    assert!(c.shift_cents[40] > 40.0, "edited note moved: {}", c.shift_cents[40]);
    // No edits at all: both curves are all zero.
    let c = build_curves(&a, &BTreeMap::new(), &NO_SCALE);
    assert!(c.shift_cents.iter().chain(&c.gain_db).all(|&v| v == 0.0));
}

#[test]
fn gain_ramps_over_15_ms() {
    let a = two_notes();
    let c = build_curves(&a, &BTreeMap::from([(NoteId(10), level_edit())]), &NO_SCALE);
    let g = -1.25;
    for i in 13..67 {
        assert!((c.gain_db[i] - g).abs() < 1e-6, "gain[{i}] = {}", c.gain_db[i]);
    }
    for i in (10..13).chain(67..70) {
        assert!(c.gain_db[i] < 0.0 && c.gain_db[i] > g, "ramp gain[{i}] = {}", c.gain_db[i]);
    }
    assert!(c.gain_db[10] > c.gain_db[11] && c.gain_db[11] > c.gain_db[12], "ramp in");
    for i in (7..10).chain(70..73) {
        assert_eq!(c.gain_db[i], 0.0, "outside gain[{i}]");
    }
}

/// A 16-frame scoop `[20, 36)` gliding 6500 → 6725 straight into a note at 6759 `[36, 136)`.
/// With `gap`, the scoop's last two frames are an unvoiced gap (also an [`UnvoicedRegion`]).
fn scooped(gap: bool) -> VocalAnalysis {
    let mut f = silence(20);
    f.extend(voiced((0..16).map(|k| 6500.0 + 15.0 * k as f32)));
    if gap {
        f[34] = (None, -30.0);
        f[35] = (None, -30.0);
    }
    f.extend(voiced(vec![6759.0; 100]));
    f.extend(silence(20));
    let unvoiced: &[(u32, u32)] = if gap { &[(34, 36)] } else { &[] };
    analysis(&f, &[(36, 136, Some((20, 36)))], unvoiced)
}

fn scoop_curves(a: &VocalAnalysis, transition: Transition) -> EditCurves {
    build_curves(a, &BTreeMap::from([(NoteId(36), NoteEdit { centre: 1.0, transition, ..NoteEdit::default() })]), &NO_SCALE)
}

#[test]
fn scoop_keep_leaves_the_approach_alone() {
    let a = scooped(false);
    let c = scoop_curves(&a, Transition::Keep);
    assert_eq!(c.shift_cents[20], 0.0, "first scoop frame");
    for i in 20..=32 {
        assert_eq!(c.shift_cents[i], 0.0, "scoop frame {i}");
    }
    for i in 33..40 {
        assert!(c.shift_cents[i] >= c.shift_cents[i - 1], "ramp at {i}");
    }
    assert!((c.shift_cents[60] - 41.0).abs() < 1e-3);

    // Unvoiced always wins, even inside the scoop span and for every transition.
    let a = scooped(true);
    for tr in [Transition::Keep, Transition::Smooth, Transition::Shorten] {
        let c = scoop_curves(&a, tr);
        assert!(c.shift_cents[34] == 0.0 && c.shift_cents[35] == 0.0, "{tr:?}: gap {} {}", c.shift_cents[34], c.shift_cents[35]);
        assert!(c.shift_cents.iter().all(|v| v.is_finite()));
    }
}

#[test]
fn scoop_smooth_and_shorten() {
    let a = scooped(false);
    let p = |k: usize| 6500.0 + 15.0 * k as f32;

    // Smooth: pitch + shift follows the straight line from the scoop's first pitch (6500) to the
    // note's tuned start pitch (6800) over the 16 scoop frames.
    let c = scoop_curves(&a, Transition::Smooth);
    for k in 0..16 {
        let want = 6500.0 + 300.0 * k as f32 / 16.0;
        let got = p(k) + c.shift_cents[20 + k];
        assert!((got - want).abs() < 1.0, "smooth k {k}: {got} want {want}");
    }

    // Shorten: the glide runs at double speed and holds the tuned pitch from the midpoint on.
    let c = scoop_curves(&a, Transition::Shorten);
    for k in 8..16 {
        let got = p(k) + c.shift_cents[20 + k];
        assert!((got - 6800.0).abs() < 2.0, "shorten k {k}: {got}");
    }
    for i in 36..60 {
        assert!((6759.0 + c.shift_cents[i] - 6800.0).abs() < 2.0, "note frame {i}");
    }
    for k in 1..4 {
        let got = p(k) + c.shift_cents[20 + k];
        assert!((got - p(2 * k)).abs() < 2.0, "double-speed glide k {k}: {got} want {}", p(2 * k));
    }
}

#[test]
fn selection_summary_reports_before_and_after() {
    // 4 notes off by −41, +23, +39, −5 c, 60 frames each with 10 silent frames around them.
    let centres = [6759.0, 6523.0, 6439.0, 6595.0];
    let mut f = silence(10);
    let mut notes = Vec::new();
    for &c in &centres {
        let s = f.len() as u32;
        f.extend(voiced(vec![c; 60]));
        notes.push((s, s + 60, None));
        f.extend(silence(10));
    }
    let a = analysis(&f, &notes, &[]);
    let ids: Vec<NoteId> = a.notes.iter().map(|n| n.id).collect();
    let edits: BTreeMap<NoteId, NoteEdit> = ids.iter().map(|&id| (id, edit(0.40))).collect();
    let s = selection_summary(&a, &edits, &ids, &NO_SCALE).expect("summary");
    assert_eq!(s.count, 4);
    assert!((s.avg_off_before - 27.0).abs() < 0.1, "before {}", s.avg_off_before);
    assert!((s.avg_off_after - 16.2).abs() < 0.1, "after {}", s.avg_off_after);
    assert_eq!((s.start_frame, s.end_frame), (10, 280));
    assert_eq!(s.level_spread_db, 0.0);
    assert!((s.typical_db - LEVEL).abs() < 1e-4);
    assert!(selection_summary(&a, &edits, &[], &NO_SCALE).is_none());
}

#[test]
fn factory_first_pass_matches_the_mockup() {
    let p = &factory_presets()[0];
    assert_eq!((p.name.as_str(), p.centre, p.drift, p.vibrato, p.raise, p.lower), ("First pass (gentle)", 0.40, 0.30, -0.10, 0.25, 0.25));
}

#[test]
fn note_edit_identity_and_clamping() {
    assert!(NoteEdit::default().is_identity());
    assert!(!edit(0.1).is_identity());
    assert!(!NoteEdit { transition: Transition::Smooth, ..NoteEdit::default() }.is_identity());
    let e = NoteEdit {
        centre: 2.0,
        drift: -1.0,
        vibrato: f32::NAN,
        raise: f32::INFINITY,
        lower: 0.5,
        typical_db: Some(f32::NEG_INFINITY),
        ..NoteEdit::default()
    }
    .clamped();
    assert_eq!((e.centre, e.drift, e.vibrato, e.raise, e.lower, e.typical_db), (1.0, 0.0, 0.0, 0.0, 0.5, None));
    assert_eq!(NoteEdit { typical_db: Some(-21.0), ..NoteEdit::default() }.clamped().typical_db, Some(-21.0));
    assert_eq!(NoteEdit { vibrato: -3.0, ..NoteEdit::default() }.clamped().vibrato, -1.0);
    // ScaleAt for a constant scale.
    let s: &dyn ScaleAt = &Some(PitchClassSet::from_pcs(&[0]));
    assert_eq!(s.scale_at(123), Some(PitchClassSet(1)));
}

#[test]
fn smooth_and_shorten_output_pitch_is_continuous() {
    // Voiced lead-in at 6600 [16, 20), a steep 16-frame scoop [20, 36) rising 6600 -> 6750 at
    // 10 c per frame (real-take scoops cover 1-2 semitones in 14-18 frames), then a note around
    // 6759 (tuned to 6800) with a 1 Hz ±10 c wander and a 6 Hz ±20 c vibrato.
    let mut f = silence(16);
    f.extend(voiced(vec![6600.0; 4]));
    f.extend(voiced((0..16).map(|k| 6600.0 + 10.0 * k as f32)));
    f.extend(voiced((0..100).map(|k| 6759.0 + 10.0 * (TAU * t(k)).sin() + 20.0 * (TAU * 6.0 * t(k)).sin())));
    f.extend(silence(20));
    let a = analysis(&f, &[(36, 136, Some((20, 36)))], &[]);
    let pitch = |i: usize| f[i].0.unwrap_or(0.0);
    let range = 16..44; // s0 − 4 through start + 8
    let natural = range.clone().map(|i| (pitch(i + 1) - pitch(i)).abs()).filter(|&d| d < 30.0).fold(0.0f32, f32::max);
    // Shorten's double-speed glide moves 2× the natural motion by design; 25 c covers the 4-frame
    // raised-cosine blend riding on it (its largest step here is about 41 c). The old unblended
    // Shorten handed over from p[14] = 6740 straight to ~6800 in one frame (a ~60 c step here,
    // ~90 c on the 15 c/frame fixture above), which this bound rejects.
    let bound = 2.0 * natural + 25.0;
    assert!((bound - 45.0).abs() < 0.5, "natural motion {natural}");
    for transition in [Transition::Smooth, Transition::Shorten] {
        let e = NoteEdit { centre: 1.0, drift: 0.3, vibrato: -0.1, transition, ..NoteEdit::default() };
        let c = build_curves(&a, &BTreeMap::from([(NoteId(36), e)]), &NO_SCALE);
        let out = |i: usize| pitch(i) + c.shift_cents[i];
        let mut max_step = 0.0f32;
        for i in range.clone() {
            let step = (out(i + 1) - out(i)).abs();
            max_step = max_step.max(step);
            assert!(step <= bound, "{transition:?}: step {step} c between frames {i} and {}", i + 1);
        }
        println!("{transition:?}: largest step {max_step:.1} c (bound {bound:.1})");
    }
}

// ---- manual offset (dragging a note up or down) ------------------------------------------

fn moved(offset_cents: f32) -> NoteEdit {
    NoteEdit { offset_cents, ..NoteEdit::default() }
}

#[test]
fn offset_moves_the_whole_note() {
    let a = one_note(vec![6759.0; 100]);
    let c = curves_for(&a, moved(100.0));
    for i in 15..105 {
        assert!((c.shift_cents[i] - 100.0).abs() < 1e-3, "shift[{i}] = {}", c.shift_cents[i]);
    }
    assert_eq!(c.shift_cents[0], 0.0, "silence stays untouched");
    assert!((tuned_centre(&a, &a.notes[0], &moved(100.0), &NO_SCALE) - 6859.0).abs() < 0.01);
}

#[test]
fn offset_adds_to_centre_and_passes_the_250_cent_clamp() {
    let a = one_note(vec![6759.0; 100]);
    let e = NoteEdit { centre: 0.4, offset_cents: 1200.0, ..NoteEdit::default() };
    let c = curves_for(&a, e.clone());
    for i in 15..105 {
        assert!((c.shift_cents[i] - 1216.4).abs() < 1e-2, "shift[{i}] = {}", c.shift_cents[i]);
    }
    assert!((tuned_centre(&a, &a.notes[0], &e, &NO_SCALE) - 7975.4).abs() < 0.1);
    // The ±250 c clamp still holds beyond the offset.
    let wild = NoteEdit { vibrato: 1.0, offset_cents: -300.0, ..NoteEdit::default() };
    let b = one_note((0..200).map(|k| 6759.0 + 400.0 * (TAU * 6.0 * t(k)).sin()).collect());
    let cb = curves_for(&b, wild);
    assert!(cb.shift_cents.iter().all(|v| v.abs() <= 550.0 + 1e-3));
    assert!(cb.shift_cents.iter().any(|v| (v.abs() - 550.0).abs() < 1e-3), "the clamp is reached");
}

#[test]
fn offset_scoop_follows_the_transition() {
    let a = scooped(false);
    let pitch = |i: usize| a.track.frames[i].cents.unwrap_or(0.0);
    for transition in [Transition::Keep, Transition::Smooth, Transition::Shorten] {
        let e = NoteEdit { centre: 1.0, offset_cents: 200.0, transition, ..NoteEdit::default() };
        let c = build_curves(&a, &BTreeMap::from([(NoteId(36), e)]), &NO_SCALE);
        let out = |i: usize| pitch(i) + c.shift_cents[i];
        // The note body sits at its target moved up two semitones.
        assert!((out(80) - 7000.0).abs() < 1e-2, "{transition:?}: body {}", out(80));
        match transition {
            // As sung.
            Transition::Keep => (20..=32).for_each(|i| assert_eq!(c.shift_cents[i], 0.0, "scoop frame {i}")),
            // Aims at the moved note.
            Transition::Smooth => assert!((out(35) - (6500.0 + (7000.0 - 6500.0) * 15.0 / 16.0)).abs() < 1e-2, "{}", out(35)),
            Transition::Shorten => (28..36).for_each(|i| assert!((out(i) - 7000.0).abs() < 1e-2, "scoop frame {i}: {}", out(i))),
        }
    }
}

#[test]
fn an_octave_move_spills_no_more_than_the_clamp_onto_a_legato_neighbour() {
    // A `[10, 70)` moved an octave, B `[70, 130)` unedited and starting where A ends.
    let mut f = silence(10);
    f.extend(voiced(vec![6759.0; 60]));
    f.extend(voiced(vec![6540.0; 60]));
    f.extend(silence(10));
    let a = analysis(&f, &[(10, 70, None), (70, 130, None)], &[]);
    let c = build_curves(&a, &BTreeMap::from([(a.notes[0].id, moved(1200.0))]), &NO_SCALE);
    assert!((c.shift_cents[40] - 1200.0).abs() < 1e-2, "A's body: {}", c.shift_cents[40]);
    // B's first frames carry the smoothing spill, within the plain ±250 c clamp, then nothing.
    for i in 70..74 {
        assert!(c.shift_cents[i].abs() <= 250.0 + 1e-3, "B's frame {i}: {}", c.shift_cents[i]);
    }
    assert!(c.shift_cents[70] > 0.0, "the spill still eases the join");
    assert!(c.shift_cents[74..130].iter().all(|v| *v == 0.0), "the rest of B is as sung");
}

#[test]
fn offset_leaves_unvoiced_frames_alone() {
    let a = scooped(true);
    for transition in [Transition::Keep, Transition::Smooth, Transition::Shorten] {
        let e = NoteEdit { offset_cents: 300.0, transition, ..NoteEdit::default() };
        let c = build_curves(&a, &BTreeMap::from([(NoteId(36), e)]), &NO_SCALE);
        assert_eq!((c.shift_cents[34], c.shift_cents[35]), (0.0, 0.0), "{transition:?}");
        assert!(c.shift_cents[..20].iter().all(|v| *v == 0.0), "silence before the scoop");
    }
}

#[test]
fn offset_identity_clamp_and_serde() {
    assert!(!moved(1.0).is_identity());
    assert_eq!(moved(5000.0).clamped().offset_cents, 2400.0);
    assert_eq!(moved(-3000.0).clamped().offset_cents, -2400.0);
    assert_eq!(moved(f32::NAN).clamped().offset_cents, 0.0);
    assert_eq!(moved(f32::INFINITY).clamped().offset_cents, 0.0);
    // Edits saved before the field load with no offset; a set one round-trips.
    let old: NoteEdit = serde_json::from_str(r#"{"centre":0.4,"drift":0.3}"#).unwrap();
    assert_eq!(old, NoteEdit { centre: 0.4, drift: 0.3, ..NoteEdit::default() });
    let e = NoteEdit { centre: 0.4, offset_cents: -35.0, ..NoteEdit::default() };
    assert_eq!(serde_json::from_str::<NoteEdit>(&serde_json::to_string(&e).unwrap()).unwrap(), e);
}

#[test]
fn moved_notes_are_judged_against_the_moved_target() {
    assert_eq!(moved_target(6800.0, 0.0), 6800.0);
    assert_eq!(moved_target(6800.0, 241.0), 7000.0);
    assert_eq!(moved_target(6800.0, -35.0), 6800.0);
    assert_eq!(moved_target(6800.0, -60.0), 6700.0);
    assert_eq!(moved_target(6800.0, f32::NAN), 6800.0);
    let a = one_note(vec![6759.0; 100]);
    let ids = [a.notes[0].id];
    // −41 c dragged up two rows, snapped: on the semitone two up.
    let sm = selection_summary(&a, &BTreeMap::from([(ids[0], moved(241.0))]), &ids, &NO_SCALE).unwrap();
    assert!((sm.avg_off_before - 41.0).abs() < 0.01 && sm.avg_off_after.abs() < 0.01, "{sm:?}");
    // Moved up a semitone without snapping: still 41 c flat of the new target.
    let sm = selection_summary(&a, &BTreeMap::from([(ids[0], moved(100.0))]), &ids, &NO_SCALE).unwrap();
    assert!((sm.avg_off_after - 41.0).abs() < 0.01, "{sm:?}");
}
