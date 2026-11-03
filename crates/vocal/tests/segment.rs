//! Task 3: notes, scoops, unvoiced regions, split/join and carrying edits across re-analysis.
//!
//! Every track here is built directly from a cents vector at 5 ms frames (48 kHz, hop 240) with
//! the level at −20 dB unless a test says otherwise.

use std::collections::BTreeMap;

use soundcraft_vocal::{Note, decompose, edit};
use soundcraft_vocal::{
    NoteEdit, NoteId, PitchFrame, PitchTrack, SegOp, UnvoicedRegion, VocalAnalysis, apply_seg_ops, carry_edits, join_notes, segment, split_note,
};

const LEVEL: f32 = -20.0;

fn track(cents: &[Option<f32>]) -> PitchTrack {
    track_with_levels(&cents.iter().map(|&c| (c, LEVEL)).collect::<Vec<_>>())
}

fn track_with_levels(frames: &[(Option<f32>, f32)]) -> PitchTrack {
    PitchTrack {
        sample_rate: 48_000,
        hop_samples: 240,
        frames: frames
            .iter()
            .map(|&(cents, level_db)| PitchFrame { cents, level_db, voiced_prob: if cents.is_some() { 0.9 } else { 0.0 } })
            .collect(),
    }
}

/// `n` frames at `c` cents.
fn held(c: f32, n: usize) -> Vec<Option<f32>> {
    vec![Some(c); n]
}

fn analysis(t: PitchTrack) -> VocalAnalysis {
    let (notes, unvoiced) = segment(&t);
    VocalAnalysis { track: t, notes, unvoiced }
}

fn ids(notes: &[Note]) -> Vec<u32> {
    notes.iter().map(|n| n.id.0).collect()
}

#[test]
fn two_held_notes_split_at_the_jump() {
    let mut c = held(6400.0, 60);
    c.extend(held(6700.0, 60));
    let (notes, unvoiced) = segment(&track(&c));
    assert_eq!(ids(&notes), vec![0, 60]);
    assert_eq!((notes[0].start, notes[0].end), (0, 60));
    assert_eq!((notes[1].start, notes[1].end), (60, 120));
    assert!((notes[0].centre_cents - 6400.0).abs() < 1e-3);
    assert!((notes[1].centre_cents - 6700.0).abs() < 1e-3);
    assert!(notes.iter().all(|n| n.scoop.is_none()));
    assert!(unvoiced.is_empty());
}

#[test]
fn a_short_blip_is_absorbed() {
    let mut c = held(6400.0, 60);
    c.extend(held(6600.0, 6));
    c.extend(held(6400.0, 60));
    let (notes, _) = segment(&track(&c));
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!((notes[0].start, notes[0].end), (0, 126));
}

#[test]
fn hysteresis_keeps_a_note_wobbling_by_60c() {
    let c: Vec<Option<f32>> = (0..100).map(|i| Some(6400.0 + 60.0 * (std::f32::consts::TAU * 5.0 * i as f32 * 0.005).sin())).collect();
    let (notes, _) = segment(&track(&c));
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!((notes[0].start, notes[0].end), (0, 100));
}

#[test]
fn a_short_rise_into_a_long_note_is_its_scoop() {
    // 60 ms rising 6200 -> 6380 c (11 voiced frames, then one unvoiced frame: a 5 ms gap, < 20 ms),
    // directly followed by 400 ms at 6400 c.
    let mut c: Vec<Option<f32>> = (0..11).map(|i| Some(6200.0 + 18.0 * i as f32)).collect();
    c.push(None);
    c.extend(held(6400.0, 80));
    let (notes, _) = segment(&track(&c));
    assert_eq!(notes.len(), 1, "no separate note for the rise: {notes:?}");
    let n = &notes[0];
    assert_eq!((n.start, n.end), (12, 92));
    assert!((n.centre_cents - 6400.0).abs() < 1e-3);
    assert_eq!(n.scoop, Some((n.start - 12, n.start)));
}

#[test]
fn unvoiced_gaps_with_level_are_unvoiced_regions() {
    let gap = |level: f32| {
        let mut f: Vec<(Option<f32>, f32)> = vec![(Some(6400.0), LEVEL); 60];
        f.extend(vec![(None, level); 20]);
        f.extend(vec![(Some(6700.0), LEVEL); 60]);
        segment(&track_with_levels(&f))
    };
    let (notes, unvoiced) = gap(-30.0);
    assert_eq!(ids(&notes), vec![0, 80]);
    assert_eq!(unvoiced, vec![UnvoicedRegion { start: 60, end: 80 }]);
    let (notes, unvoiced) = gap(-80.0);
    assert_eq!(ids(&notes), vec![0, 80]);
    assert!(unvoiced.is_empty());
}

#[test]
fn note_measures_follow_the_spike() {
    // 50 frames: the outer 10 at each end are far off; the middle 30 (60 %) hold 6400..6429.
    let mut frames: Vec<(Option<f32>, f32)> = Vec::new();
    for i in 0..50 {
        let c = if i < 10 {
            6300.0
        } else if i >= 40 {
            6500.0
        } else {
            6400.0 + (i - 10) as f32
        };
        let level = if i % 2 == 0 { -10.0 } else { -30.0 };
        frames.push((Some(c), level));
    }
    let t = track_with_levels(&frames);
    let (centre, level, drift, vibrato) = Note::measures(&t, 0, 50);
    // note_core(trim 0.2): k = 10, core = frames 10..40, median of 6400..6429 = 6414.5.
    assert!((centre - 6414.5).abs() < 1e-3, "centre {centre}");
    // level = 10·log10(mean(10^(L/10))) = 10·log10((0.1 + 0.001) / 2).
    let want = 10.0 * ((0.1f64 + 0.001) / 2.0).log10();
    assert!((f64::from(level) - want).abs() < 1e-3, "level {level} want {want}");
    assert!(drift.is_finite() && vibrato.is_finite());

    // Drift and vibrato: the std of decompose()'s slow and fast parts. A 200-frame note with a
    // 1 Hz ±20 c wander and a 6 Hz ±30 c vibrato: slow ≈ 20/√2, fast ≈ 30/√2.
    let wave: Vec<f32> = (0..200)
        .map(|k| {
            let t = k as f32 * 0.005;
            6400.0 + 20.0 * (std::f32::consts::TAU * t).sin() + 30.0 * (std::f32::consts::TAU * 6.0 * t).sin()
        })
        .collect();
    let t = track(&wave.iter().map(|&c| Some(c)).collect::<Vec<_>>());
    let (_, _, drift, vibrato) = Note::measures(&t, 0, 200);
    let (_, slow, fast) = decompose(&wave);
    assert!((drift - pop_std(&slow)).abs() < 1e-3 && (vibrato - pop_std(&fast)).abs() < 1e-3, "drift {drift} vibrato {vibrato}");
    assert!((drift - 14.14).abs() < 1.5, "drift {drift}");
    assert!((vibrato - 21.21).abs() < 1.5, "vibrato {vibrato}");

    // A short note (n − 2k < 4) uses all its frames.
    let t = track(&[Some(6000.0), Some(6100.0), Some(6200.0), Some(6900.0)]);
    let (centre, level, _, _) = Note::measures(&t, 0, 4);
    assert!((centre - 6150.0).abs() < 1e-3, "centre {centre}");
    assert!((level - LEVEL).abs() < 1e-3);

    // segment() fills notes from the same measures.
    let mut c = held(6400.0, 60);
    c.extend(held(6700.0, 60));
    let t = track(&c);
    let (notes, _) = segment(&t);
    let (centre, level, drift, vibrato) = Note::measures(&t, notes[1].start, notes[1].end);
    assert_eq!((notes[1].centre_cents, notes[1].level_db, notes[1].drift_cents, notes[1].vibrato_cents), (centre, level, drift, vibrato));
}

fn edit_marker() -> NoteEdit {
    // A non-default edit, so carrying it is distinguishable from a fresh default.
    edit::NoteEdit { centre: 0.4, drift: 0.3, ..edit::NoteEdit::default() }
}

#[test]
fn split_gives_both_halves_the_parent_edit_and_join_keeps_the_first() {
    let mut c = held(6400.0, 60);
    c.extend(held(6700.0, 60));
    let mut a = analysis(track(&c));
    let mut edits = BTreeMap::new();
    edits.insert(NoteId(0), edit_marker());

    let halves = split_note(&mut a, &mut edits, NoteId(0), 30);
    assert_eq!(halves, Some((NoteId(0), NoteId(30))));
    assert_eq!(ids(&a.notes), vec![0, 30, 60]);
    assert_eq!((a.notes[0].start, a.notes[0].end), (0, 30));
    assert_eq!((a.notes[1].start, a.notes[1].end), (30, 60));
    assert!(edits.contains_key(&NoteId(0)) && edits.contains_key(&NoteId(30)));
    assert!(!edits.contains_key(&NoteId(60)));
    let (centre, level, _, _) = Note::measures(&a.track, 30, 60);
    assert_eq!((a.notes[1].centre_cents, a.notes[1].level_db), (centre, level));

    // Out-of-range or unknown splits do nothing.
    assert_eq!(split_note(&mut a, &mut edits, NoteId(30), 30), None);
    assert_eq!(split_note(&mut a, &mut edits, NoteId(30), 60), None);
    assert_eq!(split_note(&mut a, &mut edits, NoteId(7), 10), None);
    assert_eq!(ids(&a.notes), vec![0, 30, 60]);

    // Join the edited second half with the unedited note 60: the joined note keeps the first's edit.
    let joined = join_notes(&mut a, &mut edits, NoteId(30), NoteId(60));
    assert_eq!(joined, Some(NoteId(30)));
    assert_eq!(ids(&a.notes), vec![0, 30]);
    assert_eq!((a.notes[1].start, a.notes[1].end), (30, 120));
    assert!(edits.contains_key(&NoteId(30)));
    assert!(!edits.contains_key(&NoteId(60)));

    // Join an unedited first with an edited second: the joined note has no edit.
    let mut edits2 = BTreeMap::new();
    edits2.insert(NoteId(30), edit_marker());
    assert_eq!(join_notes(&mut a, &mut edits2, NoteId(0), NoteId(30)), Some(NoteId(0)));
    assert!(edits2.is_empty());
    assert_eq!(ids(&a.notes), vec![0]);
    assert_eq!((a.notes[0].start, a.notes[0].end), (0, 120));
    let (centre, level, _, _) = Note::measures(&a.track, 0, 120);
    assert_eq!((a.notes[0].centre_cents, a.notes[0].level_db), (centre, level));

    // Non-adjacent or unknown joins do nothing.
    assert_eq!(join_notes(&mut a, &mut edits, NoteId(0), NoteId(99)), None);
    assert_eq!(join_notes(&mut a, &mut edits, NoteId(0), NoteId(0)), None);
}

#[test]
fn seg_ops_replay_after_reanalysis_and_stale_ops_are_skipped() {
    let mut c = held(6400.0, 60);
    c.extend(held(6700.0, 60));
    let t = track(&c);

    let mut first = analysis(t.clone());
    let mut scratch = BTreeMap::new();
    assert!(split_note(&mut first, &mut scratch, NoteId(0), 30).is_some());

    let ops = [SegOp::Split { at: 30 }];
    let mut again = analysis(t.clone());
    apply_seg_ops(&mut again, &ops);
    assert_eq!(again, first);

    // Stale ops: a join at a missing id, a split outside any note, a join with no predecessor.
    let before = again.clone();
    apply_seg_ops(&mut again, &[SegOp::Join { at: 45 }, SegOp::Split { at: 500 }, SegOp::Join { at: 0 }, SegOp::Split { at: 30 }]);
    assert_eq!(again, before);

    // A replayed join undoes the split.
    apply_seg_ops(&mut again, &[SegOp::Join { at: 30 }]);
    assert_eq!(again, analysis(t));
}

fn note(start: u32, end: u32) -> Note {
    Note { id: NoteId(start), start, end, scoop: None, centre_cents: 6400.0, level_db: LEVEL, drift_cents: 0.0, vibrato_cents: 0.0 }
}

#[test]
fn carry_edits_moves_edits_by_overlap() {
    let old = vec![note(0, 100), note(100, 200)];
    let mut edits = BTreeMap::new();
    edits.insert(NoteId(0), edit_marker());
    let new = vec![note(5, 95), note(95, 140)];
    let carried = carry_edits(&old, &edits, &new);
    assert!(carried.contains_key(&NoteId(5)));
    assert!(!carried.contains_key(&NoteId(95)));
    assert_eq!(carried.len(), 1);

    // Exactly 50 % overlap is enough.
    let carried = carry_edits(&old, &edits, &[note(50, 150)]);
    assert!(carried.contains_key(&NoteId(50)));
}

/// Population standard deviation.
fn pop_std(xs: &[f32]) -> f32 {
    let m = xs.iter().sum::<f32>() / xs.len() as f32;
    (xs.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / xs.len() as f32).sqrt()
}
