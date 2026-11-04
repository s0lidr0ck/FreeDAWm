//! Musical note detection: synthetic sung lines with scoops, slides, falls and vibrato give one
//! note per target, and the Musical correction moves notes without stepping inside the glides
//! between them. Classic is checked to stay what it was (its own tests are in `segment.rs` and
//! `edit_maths.rs`).
//!
//! Tracks are built straight from cents at 5 ms frames (48 kHz, hop 240), level −20 dB unless a
//! test says otherwise.

use std::collections::BTreeMap;

use soundcraft_vocal::musical::{core_range, weighted_centre};
use soundcraft_vocal::{
    NoteEdit, NoteId, NoteMode, PitchClassSet, PitchFrame, PitchTrack, Transition, VocalAnalysis, apply_seg_ops_with, build_curves,
    build_curves_with, join_notes_with, segment, segment_with, selection_summary_with, split_note_with, tuned_centre_with,
};

const LEVEL: f32 = -20.0;

fn track_of(frames: &[(Option<f32>, f32)]) -> PitchTrack {
    PitchTrack {
        sample_rate: 48_000,
        hop_samples: 240,
        frames: frames
            .iter()
            .map(|&(cents, level_db)| PitchFrame { cents, level_db, voiced_prob: if cents.is_some() { 0.9 } else { 0.0 } })
            .collect(),
    }
}

fn track(cents: &[f32]) -> PitchTrack {
    track_of(&cents.iter().map(|&c| (Some(c), LEVEL)).collect::<Vec<_>>())
}

/// A line built piece by piece.
#[derive(Default)]
struct Line(Vec<f32>);

impl Line {
    /// `n` frames held at `c`, with vibrato of `depth` cents at 5.5 Hz.
    fn hold(mut self, c: f32, n: usize, depth: f32) -> Self {
        for k in 0..n {
            let t = k as f32 / 200.0;
            self.0.push(c + depth * (std::f32::consts::TAU * 5.5 * t).sin());
        }
        self
    }

    /// `n` frames gliding from `a` to `b` (an eased S-curve, as a voice moves).
    fn glide(mut self, a: f32, b: f32, n: usize) -> Self {
        for k in 0..n {
            let t = (k as f32 + 0.5) / n as f32;
            let s = t * t * (3.0 - 2.0 * t);
            self.0.push(a + (b - a) * s);
        }
        self
    }
}

fn analysis(t: PitchTrack, mode: NoteMode) -> VocalAnalysis {
    let (notes, unvoiced) = segment_with(&t, mode);
    VocalAnalysis { track: t, notes, unvoiced }
}

/// A loose legato line: C4 sung 35 c flat, a scoop that lingers 80 ms around D♯4 on its way into
/// E4 sung 30 c flat, a slide up to G4 sung 25 c sharp with wide vibrato, and a fall off its end.
/// Targets C4, E4, G4.
fn loose_line() -> Vec<f32> {
    Line::default()
        .hold(5965.0, 70, 0.0)
        .glide(5965.0, 6120.0, 8)
        .glide(6120.0, 6170.0, 16)
        .glide(6170.0, 6370.0, 6)
        .hold(6370.0, 80, 8.0)
        .glide(6370.0, 6725.0, 24)
        .hold(6725.0, 110, 45.0)
        .glide(6725.0, 6500.0, 20)
        .0
}

#[test]
fn a_loose_line_is_one_note_per_target() {
    let a = analysis(track(&loose_line()), NoteMode::Musical);
    let centres: Vec<f32> = a.notes.iter().map(|n| n.centre_cents).collect();
    assert_eq!(a.notes.len(), 3, "C4, E4, G4: {centres:?}");
    for (n, want) in a.notes.iter().zip([5965.0, 6370.0, 6725.0]) {
        assert!((n.centre_cents - want).abs() <= 12.0, "centre {} wants {want}", n.centre_cents);
        assert!(n.scoop.is_none(), "the approach is part of the note");
    }
    // The notes tile the line: the glides are inside them.
    assert_eq!(a.notes.first().map(|n| n.start), Some(0));
    assert_eq!(a.notes.last().map(|n| n.end), Some(loose_line().len() as u32));
    assert!(a.notes.windows(2).all(|w| w[0].end == w[1].start));
    // Classic makes notes of the scoop, the slide and the fall.
    let classic = analysis(track(&loose_line()), NoteMode::Classic);
    assert!(classic.notes.len() > a.notes.len(), "Classic: {} notes", classic.notes.len());
}

#[test]
fn the_core_is_the_sung_middle() {
    let t = track(&loose_line());
    let a = analysis(t.clone(), NoteMode::Musical);
    // E4's note starts with the scoop (frames 70..100) and holds from frame 100.
    let e4 = &a.notes[1];
    let (c0, c1) = core_range(&t, e4);
    assert!(e4.start <= 84, "E4 takes in the scoop: starts at {}", e4.start);
    assert!((90..=106).contains(&c0), "E4's core starts where the voice settles: {c0}");
    assert!(c1 > c0 + 60 && c1 <= e4.end);
}

#[test]
fn wide_vibrato_across_a_semitone_line_is_one_note() {
    // ±70 c around 6450: the pitch crosses the 6400/6500 line every cycle.
    let c = Line::default().hold(6450.0, 220, 70.0).0;
    let m = analysis(track(&c), NoteMode::Musical);
    assert_eq!(m.notes.len(), 1, "{:?}", m.notes.iter().map(|n| (n.start, n.end)).collect::<Vec<_>>());
    assert!((m.notes[0].centre_cents - 6450.0).abs() < 20.0, "{}", m.notes[0].centre_cents);
}

#[test]
fn a_resung_syllable_on_one_pitch_is_two_notes() {
    // Two syllables on G4, joined by a voiced consonant (the level dips 14 dB for 40 ms).
    let mut frames: Vec<(Option<f32>, f32)> = Vec::new();
    frames.extend((0..80).map(|_| (Some(6700.0), LEVEL)));
    frames.extend((0..8).map(|_| (Some(6690.0), LEVEL - 14.0)));
    frames.extend((0..80).map(|_| (Some(6705.0), LEVEL)));
    let m = analysis(track_of(&frames), NoteMode::Musical);
    assert_eq!(m.notes.len(), 2, "{:?}", m.notes.iter().map(|n| (n.start, n.end)).collect::<Vec<_>>());
    let cut = m.notes[1].start;
    assert!((78..=90).contains(&cut), "cut at the dip: {cut}");
    // Classic keeps one note (no pitch change): unchanged behaviour.
    assert_eq!(analysis(track_of(&frames), NoteMode::Classic).notes.len(), 1);
}

#[test]
fn a_scoop_after_a_breath_folds_into_its_note() {
    let mut frames: Vec<(Option<f32>, f32)> = Vec::new();
    frames.extend(Line::default().hold(6000.0, 60, 0.0).0.into_iter().map(|c| (Some(c), LEVEL)));
    frames.extend((0..30).map(|_| (None, -70.0)));
    frames.extend(Line::default().glide(6050.0, 6400.0, 18).hold(6400.0, 90, 5.0).0.into_iter().map(|c| (Some(c), LEVEL)));
    let m = analysis(track_of(&frames), NoteMode::Musical);
    assert_eq!(m.notes.len(), 2);
    assert_eq!(m.notes[1].start, 90, "the scoop starts the note");
    assert!((m.notes[1].centre_cents - 6400.0).abs() < 8.0);
}

#[test]
fn classic_mode_is_segment_itself() {
    for line in [loose_line(), Line::default().hold(6450.0, 220, 70.0).0] {
        let t = track(&line);
        assert_eq!(segment_with(&t, NoteMode::Classic), segment(&t));
    }
    let a = analysis(track(&loose_line()), NoteMode::Classic);
    let edits: BTreeMap<NoteId, NoteEdit> = a.notes.iter().map(|n| (n.id, NoteEdit { centre: 1.0, drift: 0.5, ..NoteEdit::default() })).collect();
    let scale: Option<PitchClassSet> = None;
    assert_eq!(build_curves_with(&a, &edits, &scale, NoteMode::Classic), build_curves(&a, &edits, &scale));
}

fn full_edit() -> NoteEdit {
    NoteEdit { centre: 1.0, ..NoteEdit::default() }
}

/// The tuned pitch (sung + shift) per frame.
fn tuned(a: &VocalAnalysis, edits: &BTreeMap<NoteId, NoteEdit>) -> Vec<f64> {
    let scale: Option<PitchClassSet> = None;
    let cv = build_curves_with(a, edits, &scale, NoteMode::Musical);
    a.track.frames.iter().zip(&cv.shift_cents).map(|(f, s)| f64::from(f.cents.unwrap_or(0.0)) + f64::from(*s)).collect()
}

#[test]
fn correction_moves_whole_notes_and_keeps_the_glides_smooth() {
    let line = loose_line();
    let a = analysis(track(&line), NoteMode::Musical);
    let edits: BTreeMap<NoteId, NoteEdit> = a.notes.iter().map(|n| (n.id, full_edit())).collect();
    let y = tuned(&a, &edits);
    // Each core lands on its target.
    for (n, target) in a.notes.iter().zip([6000.0, 6400.0, 6700.0]) {
        let (c0, c1) = core_range(&a.track, n);
        let core: Vec<f64> = y[c0 as usize..c1 as usize].to_vec();
        let mut s = core.clone();
        s.sort_by(f64::total_cmp);
        let med = s[s.len() / 2];
        assert!((med - target).abs() < 6.0, "note at {} tuned to {med}, wants {target}", n.start);
    }
    // No step anywhere: the tuned pitch moves at most a little more per frame than the sung one
    // (the moves differ by tens of cents and are spread over the glides).
    let sung_step = line.windows(2).map(|w| (w[1] - w[0]).abs() as f64).fold(0.0, f64::max);
    let tuned_step = y.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f64::max);
    assert!(tuned_step <= sung_step + 6.0, "largest tuned step {tuned_step:.1} c, sung {sung_step:.1} c");
    // A rising glide stays rising: no stair inside the slide from E4 to G4 (frames 180..204).
    for k in 181..204 {
        assert!(y[k] >= y[k - 1] - 0.5, "the slide goes back down at frame {k}: {:.1} -> {:.1}", y[k - 1], y[k]);
    }
    // Classic on the same line: its scoop and slide notes are centred on semitones of their own.
    let c = analysis(track(&line), NoteMode::Classic);
    let ce: BTreeMap<NoteId, NoteEdit> = c.notes.iter().map(|n| (n.id, full_edit())).collect();
    let scale: Option<PitchClassSet> = None;
    let cv = build_curves(&c, &ce, &scale);
    let yc: Vec<f64> = line.iter().zip(&cv.shift_cents).map(|(p, s)| f64::from(*p) + f64::from(*s)).collect();
    let classic_step = yc.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f64::max);
    assert!(classic_step > tuned_step, "Classic's largest step {classic_step:.1} c vs Musical's {tuned_step:.1} c");
}

#[test]
fn an_unedited_neighbour_keeps_its_core() {
    let a = analysis(track(&loose_line()), NoteMode::Musical);
    let mut edits = BTreeMap::new();
    edits.insert(a.notes[1].id, full_edit());
    let scale: Option<PitchClassSet> = None;
    let cv = build_curves_with(&a, &edits, &scale, NoteMode::Musical);
    for n in [&a.notes[0], &a.notes[2]] {
        let (c0, c1) = core_range(&a.track, n);
        assert!(cv.shift_cents[c0 as usize..c1 as usize].iter().all(|&s| s == 0.0), "note at {} moved", n.start);
    }
    // The edited note moves by its whole offset (E4 sung 30 c flat).
    let (c0, c1) = core_range(&a.track, &a.notes[1]);
    let mid = (c0 + c1) as usize / 2;
    assert!((cv.shift_cents[mid] - 30.0).abs() < 2.0, "{}", cv.shift_cents[mid]);
}

#[test]
fn smooth_transition_draws_a_straight_glide() {
    let a = analysis(track(&loose_line()), NoteMode::Musical);
    let mut edits: BTreeMap<NoteId, NoteEdit> = a.notes.iter().map(|n| (n.id, full_edit())).collect();
    let g4 = a.notes[2].id;
    edits.insert(g4, NoteEdit { transition: Transition::Smooth, ..full_edit() });
    let y = tuned(&a, &edits);
    let (_, e4_end) = core_range(&a.track, &a.notes[1]);
    let (g4_start, _) = core_range(&a.track, &a.notes[2]);
    let (r0, r1) = (e4_end as usize, g4_start as usize);
    assert!(r1 > r0 + 4, "a glide between the cores");
    let steps: Vec<f64> = (r0 + 1..r1).map(|k| y[k] - y[k - 1]).collect();
    let mean = steps.iter().sum::<f64>() / steps.len() as f64;
    assert!(steps.iter().all(|s| (s - mean).abs() < 0.5), "a straight line: {steps:?}");
}

#[test]
fn controls_keep_their_meaning() {
    let a = analysis(track(&loose_line()), NoteMode::Musical);
    let scale: Option<PitchClassSet> = None;
    let g4 = &a.notes[2];
    // Centre: 60 % of the way; the offset adds on top.
    let e = NoteEdit { centre: 0.6, offset_cents: 100.0, ..NoteEdit::default() };
    let tc = tuned_centre_with(&a, g4, &e, &scale, NoteMode::Musical);
    assert!((tc - (6725.0 - 0.6 * 25.0 + 100.0)).abs() < 3.0, "{tc}");
    // Vibrato −100 % flattens the core's vibrato but not the glides.
    let mut edits = BTreeMap::new();
    edits.insert(g4.id, NoteEdit { vibrato: -1.0, ..NoteEdit::default() });
    let y = tuned(&a, &edits);
    // G4 is held over frames 204..314 (its core also takes in the slide's top and the fall's start,
    // whose slow sag Vibrato leaves alone).
    let core = &y[214..284];
    let spread = core.iter().copied().fold(f64::MIN, f64::max) - core.iter().copied().fold(f64::MAX, f64::min);
    assert!(spread < 20.0, "vibrato left: {spread:.1} c (sung ±45 c)");
    // The summary judges notes by their stable middle.
    let ids: Vec<NoteId> = a.notes.iter().map(|n| n.id).collect();
    let sm = selection_summary_with(&a, &BTreeMap::new(), &ids, &scale, NoteMode::Musical).unwrap();
    assert!((sm.avg_off_before - 30.0).abs() < 6.0, "{}", sm.avg_off_before);
}

#[test]
fn split_and_join_measure_musical_notes() {
    let t = track(&loose_line());
    let mut a = analysis(t.clone(), NoteMode::Musical);
    let mut edits = BTreeMap::new();
    let g4 = a.notes[2].id;
    let at = a.notes[2].start + 70;
    let (_, second) = split_note_with(&mut a, &mut edits, g4, at, NoteMode::Musical).unwrap();
    assert_eq!(a.notes.len(), 4);
    let first = a.notes.iter().find(|n| n.id == g4).unwrap();
    let xs: Vec<f64> = t.frames[first.start as usize..first.end as usize].iter().map(|f| f64::from(f.cents.unwrap())).collect();
    assert!((f64::from(first.centre_cents) - weighted_centre(&xs).unwrap()).abs() < 1e-3, "remeasured the Musical way");
    join_notes_with(&mut a, &mut edits, g4, second, NoteMode::Musical).unwrap();
    assert_eq!(a, analysis(t.clone(), NoteMode::Musical), "split then join is the original cut");
    // Replayed ops do the same.
    let mut b = analysis(t, NoteMode::Musical);
    apply_seg_ops_with(&mut b, &[soundcraft_vocal::SegOp::Split { at }], NoteMode::Musical);
    assert_eq!(b.notes.len(), 4);
}

#[test]
fn hostile_tracks_do_not_panic() {
    let scale: Option<PitchClassSet> = None;
    for frames in [
        vec![],
        vec![(Some(6000.0), LEVEL)],
        vec![(Some(f32::NAN), LEVEL); 40],
        vec![(Some(6000.0), f32::NEG_INFINITY); 40],
        (0..300).map(|k| (if k % 3 == 0 { None } else { Some(5000.0 + (k * 37 % 900) as f32) }, LEVEL)).collect(),
    ] {
        let a = analysis(track_of(&frames), NoteMode::Musical);
        let edits: BTreeMap<NoteId, NoteEdit> =
            a.notes.iter().map(|n| (n.id, NoteEdit { centre: 1.0, drift: 1.0, transition: Transition::Shorten, ..NoteEdit::default() })).collect();
        let cv = build_curves_with(&a, &edits, &scale, NoteMode::Musical);
        assert_eq!(cv.shift_cents.len(), frames.len());
        assert!(cv.shift_cents.iter().all(|v| v.is_finite()));
    }
}
