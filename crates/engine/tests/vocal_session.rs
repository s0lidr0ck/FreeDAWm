//! Vocal tuning's note edit commands on a real (synthetic) take: `vocal.edit`, `vocal.macro`,
//! `vocal.step`, `vocal.reset`, `vocal.split`, `vocal.join` and the `vocal.notes` query, their
//! undo steps, the levelling reference, Snap to key, and tuning kept through clip edits.

use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::vocal;
use soundcraft_model::{ChannelFormat, Clip, ClipId, Session, SourceId, TrackId, TrackKind};
use soundcraft_time::Samples;
use soundcraft_vocal::{NoteId, SegOp};
use std::f64::consts::TAU;

const RATE: u32 = 48_000;
const FORMANTS: [f64; 3] = [700.0, 1200.0, 2600.0];
const BANDWIDTHS: [f64; 3] = [100.0, 110.0, 160.0];

/// `secs` of a harmonic series at `f0` shaped by formant peaks at 700/1200/2600 Hz, at an RMS
/// level of `db` dBFS.
fn vowel(f0: f64, secs: f64, db: f64) -> Vec<f32> {
    let n = (secs * f64::from(RATE)) as usize;
    let amp = |k: usize| {
        let f = f0 * k as f64;
        let res = |i: usize, a: f64| a / (1.0 + ((f - FORMANTS[i]) / BANDWIDTHS[i]).powi(2));
        res(0, 1.0) + res(1, 0.6) + res(2, 0.3) + 0.01 / k as f64
    };
    let ks: Vec<(usize, f64)> = (1..=60).filter(|k| f0 * (*k as f64) < f64::from(RATE) / 2.0).map(|k| (k, amp(k))).collect();
    let raw: Vec<f64> = (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            ks.iter().map(|(k, a)| a * (TAU * f0 * *k as f64 * t).sin()).sum::<f64>()
        })
        .collect();
    let rms = (raw.iter().map(|v| v * v).sum::<f64>() / n.max(1) as f64).sqrt();
    let g = 10f64.powf(db / 20.0) / rms.max(1e-12);
    raw.iter().map(|v| (v * g) as f32).collect()
}

fn cents(hz: f64, c: f64) -> f64 {
    hz * 2f64.powf(c / 1200.0)
}

fn gap(secs: f64) -> impl Iterator<Item = f32> {
    std::iter::repeat_n(0.0, (secs * f64::from(RATE)) as usize)
}

/// The Task 2 take: 0.5 s at 220 Hz −30 c, 0.2 s of silence, 0.5 s at 247 Hz +20 c.
fn take2() -> Vec<f32> {
    let mut x = vowel(cents(220.0, -30.0), 0.5, -21.0);
    x.extend(gap(0.2));
    x.extend(vowel(cents(247.0, 20.0), 0.5, -21.0));
    x
}

/// Three notes 0.2 s apart: F4 +40 c (6540) at −27 dB, 220 Hz −30 c at −21 dB, 247 Hz +20 c at −15 dB.
fn take3() -> Vec<f32> {
    let mut x = vowel(cents(349.228, 40.0), 0.5, -27.0);
    x.extend(gap(0.2));
    x.extend(vowel(cents(220.0, -30.0), 0.5, -21.0));
    x.extend(gap(0.2));
    x.extend(vowel(cents(247.0, 20.0), 0.5, -15.0));
    x
}

fn add_take(s: &mut Session, name: &str, take: Vec<f32>) -> SourceId {
    soundcraft_engine::io::add_source(s, name, AudioBuffer { sample_rate: RATE, channels: vec![take] }, None, FileFormat::Wav)
}

fn place(s: &mut Session, t: TrackId, src: SourceId, start: Samples, len: usize) -> ClipId {
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(s, t, Clip::audio(id, "Vox", src, 0, start, len as Samples));
    id
}

/// A mono "Vox" track with one clip of `take` at 0, analysed with `vocal.analyse`.
fn session_of(take: Vec<f32>) -> (Engine, TrackId, ClipId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let len = take.len();
    let src = add_take(&mut s, "Vox", take);
    let c = place(&mut s, t, src, 0, len);
    let mut e = Engine::new(s);
    e.execute("vocal.analyse", &json!({"clip": c.0})).unwrap();
    (e, t, c)
}

fn clip(e: &Engine, id: ClipId) -> Clip {
    e.session().find_clip(id).map(|(_, c)| c.clone()).unwrap()
}

/// `vocal.notes` for one clip: its notes.
fn notes(e: &mut Engine, c: ClipId) -> Vec<Value> {
    e.execute("vocal.notes", &json!({"clip": c.0})).unwrap()["notes"].as_array().cloned().unwrap()
}

fn note_ref(c: ClipId, n: &Value) -> Value {
    json!({"clip": c.0, "note": n["note"]})
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"))
}

/// The analysis frames of `c`'s notes.
fn frames(e: &Engine, c: ClipId) -> Vec<(u32, u32)> {
    vocal::analysis_for(e.session(), &clip(e, c)).unwrap().notes.iter().map(|n| (n.start, n.end)).collect()
}

#[test]
fn edit_sets_values_and_undo_restores() {
    let (mut e, _, c) = session_of(take3());
    let ns = notes(&mut e, c);
    assert_eq!(ns.len(), 3, "{ns:?}");
    let undo = e.undo_history();
    e.execute("vocal.edit", &json!({"notes": [note_ref(c, &ns[0])], "centre": 40})).unwrap();
    assert_eq!(e.undo_history().len(), undo.len() + 1);
    assert_eq!(e.undo_label(), Some("Tune Vocal Edit"));
    let after = notes(&mut e, c);
    let (off, tuned) = (f(&after[0]["offset"]), f(&after[0]["tuned_offset"]));
    assert!(off.abs() > 20.0, "note 1 sits off pitch: {off}");
    assert!((tuned - off * 0.6).abs() <= 0.5, "tuned_offset {tuned} vs offset {off}");
    assert_eq!(after[0]["edit"]["centre"], json!(0.4f32));
    assert!(after[1]["edit"].is_null(), "the other notes are untouched");
    let cl = clip(&e, c);
    let tune = cl.tune.as_ref().unwrap();
    assert_eq!(tune.source, cl.source(), "a new tuning records its source");
    let id = NoteId(u32::try_from(ns[0]["note"].as_u64().unwrap()).unwrap());
    assert!(tune.note_spans.contains_key(&id), "the edited note's frames are kept");
    assert!(e.undo());
    assert!(clip(&e, c).tune.is_none());
}

#[test]
fn commit_with_missing_source_audio_is_refused() {
    let (mut e, _, c) = session_of(take3());
    let ns = notes(&mut e, c);
    e.execute("vocal.edit", &json!({"notes": [note_ref(c, &ns[0])], "centre": 40})).unwrap();
    let src = clip(&e, c).source().unwrap();
    e.session_mut().pool.remove(src);
    let err = e.execute("vocal.commit", &json!({"clip": c.0})).unwrap_err();
    assert!(err.to_string().contains("missing"), "{err}");
}

#[test]
fn a_drag_is_one_undo_step() {
    let (mut e, _, c) = session_of(take3());
    let ns = notes(&mut e, c);
    let refs = json!([note_ref(c, &ns[1])]);
    let undo = e.undo_history().len();
    for v in [10, 20, 30, 40, 50] {
        e.execute_merged("vocal.edit", &json!({"notes": refs, "centre": v}), "vocal-centre").unwrap();
    }
    e.execute_joined("vocal.edit", &json!({"notes": refs, "drift": 20})).unwrap();
    e.end_merge();
    let h = e.undo_history();
    assert_eq!(h.len(), undo + 1, "{h:?}");
    assert_eq!(h.last().map(String::as_str), Some("Tune Vocal Edit"));
    let edit = &notes(&mut e, c)[1]["edit"];
    assert_eq!((f(&edit["centre"]), f(&edit["drift"])), (0.5f32.into(), 0.2f32.into()));
    assert!(e.undo());
    assert!(clip(&e, c).tune.is_none(), "one Undo takes the whole drag back");
}

#[test]
fn levelling_reference_is_frozen_per_note() {
    let (mut e, t, c) = session_of(take3());
    let ns = notes(&mut e, c);
    let levels: Vec<f64> = ns.iter().map(|n| f(&n["level_db"])).collect();
    for (l, want) in levels.iter().zip([-27.0, -21.0, -15.0]) {
        assert!((l - want).abs() <= 1.0, "levels {levels:?}");
    }
    assert_eq!(vocal::track_notes(e.session(), t, None).len(), 3, "Select all");
    let all: Vec<Value> = ns.iter().map(|n| note_ref(c, n)).collect();
    e.execute("vocal.edit", &json!({"notes": all, "raise": 25, "lower": 25})).unwrap();
    let gains = |e: &Engine| {
        let curves = vocal::curves_for(e.session(), &clip(e, c)).unwrap();
        frames(e, c).iter().map(|(s, en)| f64::from(curves.gain_db[((s + en) / 2) as usize])).collect::<Vec<_>>()
    };
    let g = gains(&e);
    for (g, want) in g.iter().zip([1.5, 0.0, -1.5]) {
        assert!((g - want).abs() <= 0.05, "gains {g} vs {want} ({levels:?})");
    }
    let typical = |e: &Engine| -> Vec<Option<f32>> {
        let cl = clip(e, c);
        let tune = cl.tune.as_ref().unwrap();
        vocal::analysis_for(e.session(), &cl).unwrap().notes.iter().map(|n| tune.edits.get(&n.id).and_then(|x| x.typical_db)).collect()
    };
    let before = typical(&e);
    assert!(before.iter().all(|t| t.is_some_and(|t| (f64::from(t) - levels[1]).abs() < 1e-3)), "{before:?}");

    // Note 3 alone, reference selection: only its own reference moves.
    e.execute("vocal.edit", &json!({"notes": [note_ref(c, &ns[2])], "raise": 25, "lower": 25})).unwrap();
    let now = typical(&e);
    assert_eq!(now[..2], before[..2], "notes 1-2 keep their reference");
    assert!(now[2].is_some_and(|t| (f64::from(t) - levels[2]).abs() < 1e-3), "{now:?}");
    assert!(gains(&e)[2].abs() <= 0.05, "note 3 now levels to itself");

    // Reference track: the median of the track's notes again.
    e.execute("vocal.edit", &json!({"notes": [note_ref(c, &ns[2])], "lower": 25, "reference": "track"})).unwrap();
    assert!(typical(&e)[2].is_some_and(|t| (f64::from(t) - levels[1]).abs() < 1e-3));
    // Centre alone leaves the reference as it was.
    e.execute("vocal.edit", &json!({"notes": [note_ref(c, &ns[0])], "centre": 10})).unwrap();
    assert_eq!(typical(&e)[0], before[0]);
}

#[test]
fn step_compounds() {
    let (mut e, t, c) = session_of(take3());
    let ns = notes(&mut e, c);
    let at = (ns[0]["start"].as_i64().unwrap() + ns[0]["end"].as_i64().unwrap()) / 2;
    let r = e.execute("vocal.step", &json!({"track": t.0, "at": at, "kind": "centre"})).unwrap();
    assert_eq!(r, note_ref(c, &ns[0]));
    assert_eq!(e.undo_label(), Some("Centre Note"));
    assert!((f(&notes(&mut e, c)[0]["edit"]["centre"]) - 0.4).abs() < 1e-6);
    e.execute("vocal.step", &json!({"track": t.0, "at": at, "kind": "centre"})).unwrap();
    assert!((f(&notes(&mut e, c)[0]["edit"]["centre"]) - 0.64).abs() < 1e-6, "1 − 0.6 × 0.6");
    e.execute("vocal.step", &json!({"track": "Vox", "at": at, "kind": "drift"})).unwrap();
    assert_eq!(e.undo_label(), Some("Flatten Drift"));
    assert!((f(&notes(&mut e, c)[0]["edit"]["drift"]) - 0.3).abs() < 1e-6);
    assert_eq!(vocal::note_at(e.session(), t, at).map(|n| (n.clip, n.note.0 as u64)), Some((c, ns[0]["note"].as_u64().unwrap())));
    // Between the notes there is nothing to step.
    let gap_at = (ns[0]["end"].as_i64().unwrap() + ns[1]["start"].as_i64().unwrap()) / 2;
    assert!(vocal::note_at(e.session(), t, gap_at).is_none());
    assert!(e.execute("vocal.step", &json!({"track": t.0, "at": gap_at, "kind": "centre"})).is_err());
    assert!(e.execute("vocal.step", &json!({"track": t.0, "at": at, "kind": "wobble"})).is_err());
}

#[test]
fn macro_first_pass_matches_the_mockup() {
    let (mut e, _, c) = session_of(take3());
    let ns = notes(&mut e, c);
    e.execute("vocal.macro", &json!({"notes": [note_ref(c, &ns[0]), note_ref(c, &ns[1])], "preset": "First pass (gentle)"})).unwrap();
    assert_eq!(e.undo_label(), Some("Tune Vocal Macro"));
    let after = notes(&mut e, c);
    let ed = &after[0]["edit"];
    let got: Vec<f64> = ["centre", "drift", "vibrato", "raise", "lower"].iter().map(|k| f(&ed[*k])).collect();
    let want = [0.40, 0.30, -0.10, 0.25, 0.25];
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() < 1e-6, "{got:?}");
    }
    // The levelling reference is set (the median of the two notes), or the macro would not level.
    let mid = (f(&ns[0]["level_db"]) + f(&ns[1]["level_db"])) / 2.0;
    assert!((f(&ed["typical_db"]) - mid).abs() < 1e-3, "{ed}");
    assert!(after[2]["edit"].is_null());
    assert!(e.execute("vocal.macro", &json!({"notes": [note_ref(c, &ns[0])], "preset": "Nope"})).is_err());
    // Values over a preset.
    e.execute("vocal.macro", &json!({"notes": [note_ref(c, &ns[2])], "preset": "Firm", "centre": 10})).unwrap();
    let ed = &notes(&mut e, c)[2]["edit"];
    assert!((f(&ed["centre"]) - 0.1).abs() < 1e-6 && (f(&ed["drift"]) - 0.6).abs() < 1e-6, "{ed}");
}

#[test]
fn reset_drops_an_identity_tuning() {
    let (mut e, _, c) = session_of(take3());
    let ns = notes(&mut e, c);
    e.execute("vocal.edit", &json!({"notes": [note_ref(c, &ns[0]), note_ref(c, &ns[1])], "centre": 50})).unwrap();
    e.execute("vocal.reset", &json!({"notes": [note_ref(c, &ns[0])]})).unwrap();
    assert_eq!(e.undo_label(), Some("Reset Tuning"));
    let cl = clip(&e, c);
    let tune = cl.tune.as_ref().unwrap();
    assert_eq!(tune.edits.len(), 1);
    assert_eq!(tune.note_spans.len(), 1, "the reset note's span goes too");
    e.execute("vocal.reset", &json!({"notes": [note_ref(c, &ns[1])]})).unwrap();
    assert!(clip(&e, c).tune.is_none(), "only identity edits and no seg_ops: tune dropped");
}

#[test]
fn split_then_join_round_trips_and_survives_reanalysis() {
    let (mut e, _, c) = session_of(take2());
    let ns = notes(&mut e, c);
    assert_eq!(ns.len(), 2);
    let (s0, e0) = (ns[0]["start"].as_i64().unwrap(), ns[0]["end"].as_i64().unwrap());
    let at = (s0 + e0) / 2;
    e.execute("vocal.edit", &json!({"notes": [note_ref(c, &ns[0])], "centre": 30})).unwrap();
    let r = e.execute("vocal.split", &json!({"clip": c.0, "note": ns[0]["note"], "at": at})).unwrap();
    assert_eq!(e.undo_label(), Some("Split Vocal Note"));
    let split = notes(&mut e, c);
    assert_eq!(split.len(), 3, "{split:?}");
    assert_eq!(r["notes"].as_array().map(Vec::len), Some(2));
    assert!(split[0]["edit"]["centre"] == json!(0.3f32) && split[1]["edit"]["centre"] == json!(0.3f32), "both halves keep the edit");
    let tune = clip(&e, c).tune.unwrap();
    assert!(matches!(tune.seg_ops.as_slice(), [SegOp::Split { .. }]));
    assert_eq!(tune.note_spans.len(), 2, "both halves' frames kept");

    // A fresh analysis (a reload) replays the split.
    e.session_mut().vocal = Default::default();
    e.execute("vocal.analyse", &json!({"clip": c.0})).unwrap();
    let again = notes(&mut e, c);
    assert_eq!(again.len(), 3, "seg_ops replayed");
    assert_eq!(again[1]["edit"]["centre"], json!(0.3f32));

    assert!(e.execute("vocal.join", &json!({"clip": c.0, "first": again[0]["note"], "second": again[2]["note"]})).is_err(), "adjacent only");
    e.execute("vocal.join", &json!({"clip": c.0, "first": again[0]["note"], "second": again[1]["note"]})).unwrap();
    assert_eq!(e.undo_label(), Some("Join Vocal Notes"));
    let joined = notes(&mut e, c);
    assert_eq!(joined.len(), 2);
    assert_eq!(joined[0]["note"], ns[0]["note"]);
    assert_eq!(clip(&e, c).tune.unwrap().seg_ops.len(), 2);
}

#[test]
fn snap_to_key_uses_the_session_key() {
    let (mut e, _, c) = session_of(take3());
    {
        let s = e.session_mut();
        s.band.key = 4;
        s.band.scale = 0;
    }
    let ns = notes(&mut e, c);
    assert!((f(&ns[0]["centre"]) - 6540.0).abs() < 10.0, "{}", ns[0]);
    assert_eq!(f(&ns[0]["target"]), 6500.0, "snap off: the nearest semitone");
    e.execute("vocal.edit", &json!({"notes": [note_ref(c, &ns[0])], "snap_to_key": true, "centre": 100})).unwrap();
    let after = notes(&mut e, c);
    assert_eq!(f(&after[0]["target"]), 6600.0, "snap on: F# in E major");
    assert!(f(&after[0]["tuned_offset"]).abs() < 0.5);
}

/// Every clip on `t`, in time order.
fn clips_on(e: &Engine, t: TrackId) -> Vec<Clip> {
    e.session().track(t).unwrap().clips().to_vec()
}

#[test]
fn tuning_survives_split_trim_move_and_copy() {
    let (mut e, t, c) = session_of(take2());
    let ns = notes(&mut e, c);
    let n2 = NoteId(u32::try_from(ns[1]["note"].as_u64().unwrap()).unwrap());
    e.execute("vocal.edit", &json!({"notes": [note_ref(c, &ns[1])], "centre": 50})).unwrap();
    let sec = |x: f64| (x * f64::from(RATE)) as i64;
    // Split the clip in the gap between the notes, trim the left part, move the right part.
    e.execute("edit.separate", &json!({"tracks": [t.0], "at": sec(0.6)})).unwrap();
    let parts = clips_on(&e, t);
    assert_eq!(parts.len(), 2);
    let (left, right) = (parts[0].id, parts[1].id);
    e.execute("edit.trim_clip", &json!({"clip": left.0, "end": sec(0.55)})).unwrap();
    e.execute("edit.move_clips", &json!({"clips": [right.0], "by": sec(1.0)})).unwrap();
    let r = clip(&e, right);
    e.execute("edit.copy", &json!({"tracks": [t.0], "start": r.start, "end": r.end()})).unwrap();
    e.execute("edit.paste", &json!({"tracks": [t.0], "at": sec(5.0)})).unwrap();
    let all = clips_on(&e, t);
    assert_eq!(all.len(), 3, "left, right and the copy");
    for cl in &all {
        let edit = cl.tune.as_ref().and_then(|tu| tu.edits.get(&n2));
        assert_eq!(edit.map(|x| x.centre), Some(0.5), "clip {} keeps note 2's edit", cl.id.0);
    }
    for cl in &all {
        let listed = notes(&mut e, cl.id);
        let ids: Vec<u64> = listed.iter().map(|n| n["note"].as_u64().unwrap()).collect();
        let want = if cl.id == left { ns[0]["note"].as_u64() } else { ns[1]["note"].as_u64() };
        assert_eq!(ids, vec![want.unwrap()], "clip {} lists only the notes in its window", cl.id.0);
        // Session times follow the clip.
        let n = &listed[0];
        assert!(
            n["start"].as_i64().unwrap() >= cl.start.saturating_sub(sec(0.05)) && n["end"].as_i64().unwrap() <= cl.end() + sec(0.05),
            "{n} in {}..{}",
            cl.start,
            cl.end()
        );
    }
    // The copy's curves are the original's: same source frames.
    let curves: Vec<_> = all.iter().skip(1).map(|cl| vocal::curves_for(e.session(), cl).unwrap()).collect();
    assert_eq!(curves[0], curves[1]);
}

#[test]
fn vocal_flag_off_keeps_existing_tuning() {
    let (mut e, t, c) = session_of(take2());
    let ns = notes(&mut e, c);
    e.execute("vocal.edit", &json!({"notes": [note_ref(c, &ns[0])], "centre": 60})).unwrap();
    let curves = vocal::curves_for(e.session(), &clip(&e, c)).unwrap();
    // An untuned clip of another take on the same track.
    let s = e.session_mut();
    let other: Vec<f32> = take2().iter().map(|v| v * 0.5).collect();
    let len = other.len();
    let src = add_take(s, "Vox 2", other);
    place(s, t, src, 10 * Samples::from(RATE), len);
    assert!(vocal::next_job(&e, None, 0).is_some(), "on a Vocal track it would be analysed");
    e.execute("track.vocal", &json!({"tracks": [t.0], "value": false})).unwrap();
    assert!(clip(&e, c).tune.is_some(), "tuning kept");
    assert_eq!(vocal::curves_for(e.session(), &clip(&e, c)), Some(curves), "and plays the same");
    // The tuned clip's render is still due; the untuned clip is not analysed.
    assert!(!matches!(vocal::next_job(&e, None, 0), Some(vocal::VocalJob::Analyse { .. })), "no analysis for untuned clips there");
}

#[test]
fn bad_params_are_refused() {
    let (mut e, t, c) = session_of(take3());
    let ns = notes(&mut e, c);
    let one = json!([note_ref(c, &ns[0])]);
    let undo = e.undo_history();
    let rev_doc = serde_json::to_string(e.session()).unwrap();
    let refused = [
        ("vocal.edit", json!({"notes": one, "centre": 101})),
        ("vocal.edit", json!({"notes": one, "vibrato": -101})),
        ("vocal.edit", json!({"notes": one, "centre": f64::NAN})),
        ("vocal.edit", json!({"notes": one, "drift": "lots"})),
        ("vocal.edit", json!({"notes": one, "transition": "wobble"})),
        ("vocal.edit", json!({"notes": one, "reference": "band", "raise": 10})),
        ("vocal.edit", json!({"notes": one})),
        ("vocal.edit", json!({"notes": [], "centre": 10})),
        ("vocal.edit", json!({"notes": [{"clip": c.0, "note": 999_999}], "centre": 10})),
        ("vocal.edit", json!({"notes": [{"clip": 424_242, "note": 0}], "centre": 10})),
        ("vocal.edit", json!({"notes": [{"clip": c.0}], "centre": 10})),
        ("vocal.macro", json!({"notes": one, "raise": 120})),
        ("vocal.reset", json!({"notes": [{"clip": c.0, "note": 999_999}]})),
        ("vocal.split", json!({"clip": c.0, "note": ns[0]["note"], "at": -5})),
        ("vocal.split", json!({"clip": c.0, "note": 999_999, "at": 100})),
        ("vocal.join", json!({"clip": c.0, "first": ns[1]["note"], "second": ns[0]["note"]})),
        ("vocal.step", json!({"track": t.0, "at": "never"})),
        ("vocal.notes", json!({"clip": 424_242})),
    ];
    for (id, p) in refused {
        assert!(e.execute(id, &p).is_err(), "{id} {p} was accepted");
    }
    assert_eq!(e.undo_history(), undo);
    assert_eq!(serde_json::to_string(e.session()).unwrap(), rev_doc, "document untouched");

    // A clip not analysed yet: try again in a moment.
    let s = e.session_mut();
    let fresh = take2();
    let len = fresh.len();
    let src = add_take(s, "Vox 2", fresh);
    let late = place(s, t, src, 20 * Samples::from(RATE), len);
    let err = e.execute("vocal.edit", &json!({"notes": [{"clip": late.0, "note": 0}], "centre": 10})).unwrap_err();
    assert!(err.to_string().contains("analysing — try again in a moment"), "{err}");
}

#[test]
fn tuning_saves_and_reloads() {
    let (mut e, t, c) = session_of(take2());
    let ns = notes(&mut e, c);
    e.execute("vocal.edit", &json!({"notes": [note_ref(c, &ns[0])], "centre": 100, "drift": 50})).unwrap();
    e.execute("vocal.edit", &json!({"notes": [note_ref(c, &ns[1])], "snap_to_key": true})).unwrap();
    e.execute("vocal.analyse", &json!({"clip": c.0})).unwrap();
    let len = clip(&e, c).length;
    let bounce = |e: &Engine| soundcraft_mix::render_clips(e.session(), t, soundcraft_time::Range::new(0, len)).into_iter().next().unwrap();
    let before = bounce(&e);
    let ids: Vec<Value> = ns.iter().map(|n| n["note"].clone()).collect();
    let tune = clip(&e, c).tune;

    let dir = std::env::temp_dir().join(format!("soundcraft-vocal-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Tuned.scraft");
    soundcraft_engine::io::save_session(&mut e, path.to_str().unwrap()).unwrap();

    let mut back = Engine::default();
    soundcraft_engine::io::open_session(&mut back, path.to_str().unwrap()).unwrap();
    assert!(back.session().vocal.analyses.is_empty(), "analyses are derived, not saved");
    assert_eq!(clip(&back, c).tune, tune, "the tuning is saved");
    vocal::run_until_idle(&mut back, &[c]).unwrap();
    let again: Vec<Value> = notes(&mut back, c).iter().map(|n| n["note"].clone()).collect();
    assert_eq!(again, ids, "the same notes (ids)");
    assert_eq!(clip(&back, c).tune, tune, "the edits found their notes again");
    let after = bounce(&back);
    assert!(before.len() == after.len() && before.iter().zip(&after).all(|(a, b)| a.to_bits() == b.to_bits()), "the bounce, bit for bit");
    let _ = std::fs::remove_dir_all(&dir);
}
