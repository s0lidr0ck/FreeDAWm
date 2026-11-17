use soundcraft_midi::ops::*;
use soundcraft_midi::*;

fn n(pitch: u8, start: i64, length: i64) -> Note {
    Note { probability: 100, pitch, velocity: 100, muted: false, release_velocity: 64, channel: 0, start, length }
}

fn starts(notes: &[Note]) -> Vec<i64> {
    notes.iter().map(|n| n.start).collect()
}

#[test]
fn quantize_full_strength() {
    let mut notes = vec![n(60, 10, 200), n(60, 230, 100), n(60, 365, 50), n(60, -20, 40)];
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 240, ..Default::default() });
    assert_eq!(starts(&notes), vec![0, 240, 480, 0]);
    // Durations preserved by default.
    assert_eq!(notes.iter().map(|n| n.length).collect::<Vec<_>>(), vec![200, 100, 50, 40]);
}

#[test]
fn quantize_half_strength() {
    let mut notes = vec![n(60, 20, 100), n(60, 220, 100)];
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 240, strength: 0.5, ..Default::default() });
    assert_eq!(starts(&notes), vec![10, 230]);
}

#[test]
fn quantize_swing() {
    // 100% swing on a 1/8 (480) grid moves odd lines by 480/3 = 160 (triplet feel).
    let mut notes = vec![n(60, 0, 10), n(60, 470, 10), n(60, 650, 10), n(60, 950, 10)];
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 480, swing: 1.0, ..Default::default() });
    assert_eq!(starts(&notes), vec![0, 640, 640, 960]);
    let mut notes = vec![n(60, 480, 10)];
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 480, swing: 0.5, ..Default::default() });
    assert_eq!(starts(&notes), vec![560]);
}

#[test]
fn quantize_on_counts_the_grid_and_swing_from_each_span() {
    // A clip starting 100 ticks after a bar: the bar's lines are at -100, 140, 380 ... in the
    // clip's ticks, not at 0, 240, 480.
    let bar = |_: i64| (-100, 1820);
    let mut notes = vec![n(60, 0, 10), n(60, 150, 10), n(60, 370, 10)];
    quantize_on(&mut notes, &QuantizeOptions { grid_ticks: 240, ..Default::default() }, 1, bar);
    assert_eq!(starts(&notes), vec![-100, 140, 380]);
    // Swing delays the bar's second line, not the clip's.
    let mut notes = vec![n(60, 400, 10)];
    quantize_on(&mut notes, &QuantizeOptions { grid_ticks: 480, swing: 1.0, ..Default::default() }, 1, bar);
    assert_eq!(starts(&notes), vec![540]);
    // A 7/8 bar (3360 ticks) is not a whole number of quarters: the next bar's start is a line.
    let mut notes = vec![n(60, 3300, 10)];
    quantize_on(&mut notes, &QuantizeOptions { grid_ticks: 960, ..Default::default() }, 1, |_| (0, 3360));
    assert_eq!(starts(&notes), vec![3360]);
}

#[test]
fn quantize_include_exclude() {
    let opts = QuantizeOptions { grid_ticks: 240, include_within: 0.5, ..Default::default() };
    // Half grid = 120; include within 60 ticks.
    let mut notes = vec![n(60, 50, 10), n(60, 100, 10)];
    quantize(&mut notes, &opts);
    assert_eq!(starts(&notes), vec![0, 100]);

    let opts = QuantizeOptions { grid_ticks: 240, exclude_within: 0.25, ..Default::default() };
    // Exclude closer than 30 ticks.
    let mut notes = vec![n(60, 20, 10), n(60, 50, 10)];
    quantize(&mut notes, &opts);
    assert_eq!(starts(&notes), vec![20, 0]);
}

#[test]
fn quantize_ends_and_duration() {
    let mut notes = vec![n(60, 10, 220)];
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 240, quantize_ends: true, ..Default::default() });
    assert_eq!((notes[0].start, notes[0].length), (0, 240));

    let mut notes = vec![n(60, 10, 220)];
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 240, preserve_duration: false, ..Default::default() });
    assert_eq!((notes[0].start, notes[0].length), (0, 230));

    // Ends quantized onto the start line still keep a length of at least 1.
    let mut notes = vec![n(60, 10, 5)];
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 240, quantize_ends: true, ..Default::default() });
    assert_eq!(notes[0].length, 1);
}

#[test]
fn quantize_random_is_deterministic_and_bounded() {
    let opts = QuantizeOptions { grid_ticks: 240, random: 0.5, ..Default::default() };
    let mut a: Vec<Note> = (0..50).map(|i| n(60, i * 240 + 7, 10)).collect();
    let mut b = a.clone();
    quantize(&mut a, &opts);
    quantize(&mut b, &opts);
    assert_eq!(a, b);
    for (i, note) in a.iter().enumerate() {
        assert!((note.start - i as i64 * 240).abs() <= 60);
    }
    let mut c: Vec<Note> = (0..50).map(|i| n(60, i * 240 + 7, 10)).collect();
    quantize_with_seed(&mut c, &opts, 42);
    assert_ne!(a, c);
}

#[test]
fn quantize_degenerate_options() {
    let mut notes = vec![n(60, 17, 10)];
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 0, ..Default::default() });
    quantize(&mut notes, &QuantizeOptions { grid_ticks: -5, ..Default::default() });
    assert_eq!(notes[0].start, 17);
    quantize(&mut notes, &QuantizeOptions { grid_ticks: 240, strength: f32::NAN, swing: f32::INFINITY, ..Default::default() });
    assert_eq!(notes[0].start, 17);
    let mut extreme = vec![n(60, i64::MAX - 10, 1), n(60, i64::MIN + 10, 1)];
    quantize(&mut extreme, &QuantizeOptions { grid_ticks: i64::MAX, random: 1.0, ..Default::default() });
}

#[test]
fn transpose_clamps() {
    let mut notes = vec![n(0, 0, 1), n(60, 0, 1), n(127, 0, 1)];
    transpose(&mut notes, 12);
    assert_eq!(notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![12, 72, 127]);
    transpose(&mut notes, -100);
    assert_eq!(notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![0, 0, 27]);
    transpose(&mut notes, i32::MAX);
    assert!(notes.iter().all(|n| n.pitch == 127));
}

#[test]
fn reverse_half_time_strum_and_humanize() {
    let mut notes = vec![n(60, 0, 240), n(62, 480, 480)];
    reverse(&mut notes);
    assert_eq!(starts(&notes), vec![720, 0]);
    let mut notes = vec![n(60, 960, 240), n(62, 1440, 480)];
    scale_time(&mut notes, 2.0);
    assert_eq!(notes.iter().map(|x| (x.start, x.length)).collect::<Vec<_>>(), vec![(960, 480), (1920, 960)]);
    scale_time(&mut notes, 0.5);
    assert_eq!(notes.iter().map(|x| (x.start, x.length)).collect::<Vec<_>>(), vec![(960, 240), (1440, 480)]);
    // A C triad strummed up, 30 ticks a string; the ends stay.
    let mut notes = vec![n(67, 0, 960), n(60, 0, 960), n(64, 0, 960), n(72, 960, 10)];
    strum(&mut notes, 30, true);
    assert_eq!(
        notes.iter().map(|x| (x.pitch, x.start, x.end())).collect::<Vec<_>>(),
        vec![(67, 60, 960), (60, 0, 960), (64, 30, 960), (72, 960, 970)]
    );
    let mut a = vec![n(60, 1000, 100); 8];
    let mut b = a.clone();
    humanize(&mut a, 20, 10, 7);
    humanize(&mut b, 20, 10, 7);
    assert_eq!(a, b, "seeded");
    assert!(a.iter().all(|x| (980..=1020).contains(&x.start) && (90..=110).contains(&x.velocity)));
    assert!(a.iter().any(|x| x.start != 1000));
}

/// Bug 6: a chord moved past the top keeps its intervals.
#[test]
fn transpose_within_keeps_the_shape() {
    let mut notes = vec![n(100, 0, 1), n(104, 0, 1), n(120, 0, 1)];
    assert_eq!(transpose_within(&mut notes, 24), 7);
    assert_eq!(notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![107, 111, 127]);
    assert_eq!(transpose_within(&mut notes, i32::MIN), -107);
    assert_eq!(notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![0, 4, 20]);
    assert_eq!(fitting_shift(&[], 5), 0);
}

#[test]
fn velocity_ops() {
    let base = vec![n(60, 0, 1), n(60, 100, 1), n(60, 200, 1)];
    let vels = |v: &[Note]| v.iter().map(|n| n.velocity).collect::<Vec<_>>();

    let mut v = base.clone();
    change_velocity(&mut v, VelocityOp::Set(0));
    assert_eq!(vels(&v), vec![1, 1, 1]);
    change_velocity(&mut v, VelocityOp::Set(200));
    assert_eq!(vels(&v), vec![127, 127, 127]);

    let mut v = base.clone();
    change_velocity(&mut v, VelocityOp::Add(50));
    assert_eq!(vels(&v), vec![127; 3]);
    change_velocity(&mut v, VelocityOp::Add(-500));
    assert_eq!(vels(&v), vec![1; 3]);

    let mut v = base.clone();
    change_velocity(&mut v, VelocityOp::Scale(0.5));
    assert_eq!(vels(&v), vec![50; 3]);
    change_velocity(&mut v, VelocityOp::Scale(f32::NAN));
    assert_eq!(vels(&v), vec![50; 3]);

    let mut v = vec![n(60, 0, 1), n(60, 0, 1)];
    v[0].velocity = 10;
    v[1].velocity = 120;
    change_velocity(&mut v, VelocityOp::Limit { min: 100, max: 30 });
    assert_eq!(vels(&v), vec![30, 100]);

    let mut v = base.clone();
    change_velocity(&mut v, VelocityOp::Ramp { from: 20, to: 120 });
    assert_eq!(vels(&v), vec![20, 70, 120]);

    let mut a = base.clone();
    let mut b = base.clone();
    change_velocity(&mut a, VelocityOp::Humanize(10));
    change_velocity(&mut b, VelocityOp::Humanize(10));
    assert_eq!(a, b);
    assert!(a.iter().all(|n| (90..=110).contains(&n.velocity)));
}

#[test]
fn duration_ops() {
    let lens = |v: &[Note]| v.iter().map(|n| n.length).collect::<Vec<_>>();
    let base = vec![n(60, 0, 100), n(62, 480, 100), n(64, 480, 50), n(60, 960, 100)];

    let mut v = base.clone();
    change_duration(&mut v, DurationOp::Set(0));
    assert_eq!(lens(&v), vec![1; 4]);

    let mut v = base.clone();
    change_duration(&mut v, DurationOp::Add(-75));
    assert_eq!(lens(&v), vec![25, 25, 1, 25]);

    let mut v = base.clone();
    change_duration(&mut v, DurationOp::Scale(2.0));
    assert_eq!(lens(&v), vec![200, 200, 100, 200]);

    let mut v = base.clone();
    change_duration(&mut v, DurationOp::Legato { gap: 10 });
    assert_eq!(lens(&v), vec![470, 470, 470, 100]);

    let mut v = vec![n(60, 0, 500), n(60, 200, 500), n(61, 100, 500), Note { channel: 1, ..n(60, 100, 500) }];
    change_duration(&mut v, DurationOp::RemoveOverlap);
    assert_eq!(lens(&v), vec![200, 500, 500, 500]);
}

#[test]
fn split_by_pitch() {
    let notes = vec![n(36, 0, 1), n(60, 1, 1), n(72, 2, 1), n(48, 3, 1)];
    let (inside, outside) = split_notes(&notes, 60, 40);
    assert_eq!(inside.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![60, 48]);
    assert_eq!(outside.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![36, 72]);
}

#[test]
fn sequence_helpers() {
    let mut seq = Sequence {
        notes: vec![n(64, 100, 50), n(60, 100, 10), n(60, 100, 30), n(60, 0, 10), Note { channel: 2, ..n(60, 0, 10) }],
        ctrls: vec![
            CtrlEvent { tick: 500, channel: 0, kind: CtrlKind::Cc(1), value: 1, shape: Default::default() },
            CtrlEvent { tick: 5, channel: 0, kind: CtrlKind::Cc(1), value: 2, shape: Default::default() },
            CtrlEvent { tick: 500, channel: 0, kind: CtrlKind::Cc(1), value: 3, shape: Default::default() },
            CtrlEvent { tick: 500, channel: 0, kind: CtrlKind::Cc(2), value: 4, shape: Default::default() },
        ],
    };
    assert_eq!(seq.end_tick(), 500);
    seq.sort();
    assert_eq!(starts(&seq.notes), vec![0, 0, 100, 100, 100]);
    assert_eq!(seq.notes[2].pitch, 60);
    assert_eq!(seq.notes_in(0, 100).count(), 2);
    assert_eq!(seq.notes_overlapping(105, 106).count(), 3);

    let removed = seq.remove_duplicates();
    assert_eq!(removed, 2);
    assert_eq!(seq.notes.len(), 4);
    let kept = seq.notes.iter().find(|n| n.start == 100 && n.pitch == 60).unwrap();
    assert_eq!(kept.length, 30);
    assert_eq!(seq.ctrls.iter().map(|c| c.value).collect::<Vec<_>>(), vec![2, 3, 4]);
    assert_eq!(seq.remove_duplicates(), 0);
    assert_eq!(Sequence::default().end_tick(), 0);
}

#[test]
fn sequence_serde_round_trip() {
    let seq = Sequence {
        notes: vec![n(60, 0, 1)],
        ctrls: vec![CtrlEvent { tick: 1, channel: 0, kind: CtrlKind::PolyPressure(60), value: 9, shape: Default::default() }],
    };
    let json = serde_json::to_string(&seq).unwrap();
    assert_eq!(serde_json::from_str::<Sequence>(&json).unwrap(), seq);
}

#[test]
fn grooves_are_built_in_and_followed() {
    let all = builtin_grooves();
    let swing = all.iter().find(|g| g.name == "Swing 8ths 66%").unwrap();
    assert_eq!((swing.period, swing.points.clone()), (960, vec![0, 633]));
    // Counted from the bar (at 100): the off-beat of beat 2 is 100 + 960 + 633.
    assert_eq!(swing.nearest(100 + 960 + 500, 100), 100 + 960 + 633);
    assert_eq!(swing.nearest(100 + 960 + 900, 100), 100 + 1920, "the next period's first point");
    assert!(Groove::new("x", 0, &[1]).is_none() && Groove::new("x", 10, &[]).is_none());
    let mut v = vec![n(60, 470, 100), n(62, 100, 100)];
    let opts = QuantizeOptions { grid_ticks: 480, ..QuantizeOptions::default() };
    quantize_to_groove(&mut v, swing, &opts, DEFAULT_SEED, |_| 0);
    assert_eq!(starts(&v), vec![633, 0]);
}

#[test]
fn a_groove_is_taken_from_notes() {
    // 16ths, each off-beat 16th 40 ticks late, one bar of 4/4.
    let v: Vec<Note> = (0..16).map(|k| n(60, k * 240 + if k % 2 == 1 { 40 } else { 0 }, 60)).collect();
    let g = groove_from_notes("feel", &v, 3840, 240, |_| 0).unwrap();
    assert_eq!(g.points.len(), 16);
    assert_eq!(&g.points[..4], &[0, 280, 480, 760]);
    assert!(groove_from_notes("none", &[], 3840, 240, |_| 0).is_none());
}

#[test]
fn quantize_window_ends_and_playback_quantize() {
    let opts = QuantizeOptions { grid_ticks: 240, include_within: 0.5, ..QuantizeOptions::default() };
    // Within half of half a step (60 ticks) moves; 100 off does not.
    let mut v = vec![n(60, 50, 100), n(60, 340, 100)];
    quantize(&mut v, &opts);
    assert_eq!(starts(&v), vec![0, 340]);
    let opts = QuantizeOptions { grid_ticks: 240, quantize_ends: true, ..QuantizeOptions::default() };
    let mut v = vec![n(60, 10, 200)];
    quantize(&mut v, &opts);
    assert_eq!((v[0].start, v[0].length), (0, 240));
    let opts = QuantizeOptions { grid_ticks: 240, preserve_duration: false, ..QuantizeOptions::default() };
    let mut v = vec![n(60, 10, 200)];
    quantize(&mut v, &opts);
    assert_eq!((v[0].start, v[0].length), (0, 210), "the end stays put");
    let q = PlayQuantize { grid: 240, strength: 0.5, swing: 0.0, groove: None };
    assert_eq!(q.start(100, (0, 3840)), 50);
    let off = PlayQuantize { grid: 0, strength: 1.0, swing: 0.0, groove: None };
    assert_eq!(off.start(77, (0, 3840)), 77);
}

#[test]
fn notes_are_picked_by_rule() {
    let mut v = vec![n(60, 0, 240), n(62, 960, 480), n(64, 3840 + 960, 240), n(72, 4000, 960)];
    v[1].velocity = 40;
    let bar = |t: i64| t.div_euclid(3840) * 3840;
    assert_eq!(select_matching(&v, &NoteRule::default(), bar), vec![0, 1, 2, 3]);
    let r = NoteRule { pitch: (65, 60), ..NoteRule::default() };
    assert_eq!(select_matching(&v, &r, bar), vec![0, 1, 2]);
    let r = NoteRule { velocity: (80, 127), length: (1, 480), ..NoteRule::default() };
    assert_eq!(select_matching(&v, &r, bar), vec![0, 2]);
    // Beat 2 of any bar.
    let r = NoteRule { in_bar: Some((960, 1919)), ..NoteRule::default() };
    assert_eq!(select_matching(&v, &r, bar), vec![1, 2]);
    let r = NoteRule { start: (900, 4000), ..NoteRule::default() };
    assert_eq!(select_matching(&v, &r, bar), vec![1, 3]);
}

#[test]
fn a_muted_note_round_trips_and_old_notes_load_unmuted() {
    let mut a = n(60, 0, 10);
    let json = serde_json::to_string(&a).unwrap();
    assert!(!json.contains("muted"), "an unmuted note saves as before: {json}");
    a.muted = true;
    let back: Note = serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
    assert!(back.muted);
    let old: Note = serde_json::from_str(r#"{"pitch":60,"velocity":100,"release_velocity":64,"channel":0,"start":0,"length":10}"#).unwrap();
    assert!(!old.muted);
}

#[test]
fn a_groove_from_notes_keeps_their_accents() {
    // 8ths over one bar: on-beats played at 110, off-beats at 70 (the average 90).
    let v: Vec<Note> = (0..8).map(|k| Note { velocity: if k % 2 == 0 { 110 } else { 70 }, ..n(60, k * 480, 60) }).collect();
    let g = groove_from_notes("accents", &v, 3840, 480, |_| 0).unwrap();
    assert_eq!(g.accents, vec![20, -20, 20, -20, 20, -20, 20, -20]);
    assert_eq!((g.accent(10, 0), g.accent(470, 0)), (20, -20), "the nearest point's accent");
    // Quantizing flat notes to it gives them the accents, by the strength.
    let mut flat: Vec<Note> = (0..4).map(|k| n(60, k * 480 + 15, 60)).collect();
    let opts = QuantizeOptions { grid_ticks: 480, strength: 0.5, ..QuantizeOptions::default() };
    quantize_to_groove(&mut flat, &g, &opts, DEFAULT_SEED, |_| 0);
    assert_eq!(flat.iter().map(|n| n.velocity).collect::<Vec<_>>(), vec![110, 90, 110, 90]);
    // Played: the same, the notes left alone.
    let q = PlayQuantize { grid: 480, strength: 1.0, swing: 0.0, groove: Some(g.clone()) };
    assert!(q.accents());
    assert_eq!((q.velocity(15, (0, 3840), 100), q.velocity(495, (0, 3840), 100)), (120, 80));
    // Timing-only grooves (the built-in ones, older ones) change no velocity, and save as before.
    let swing = builtin_grooves().into_iter().next().unwrap();
    assert!(swing.accents.is_empty() && !serde_json::to_string(&swing).unwrap().contains("accents"));
    let old: Groove = serde_json::from_str(r#"{"name":"x","period":960,"points":[0,600]}"#).unwrap();
    assert_eq!(old.accent(600, 0), 0);
}
