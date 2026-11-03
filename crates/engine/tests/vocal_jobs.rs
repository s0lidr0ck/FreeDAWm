//! Vocal tuning's background analysis: which clips are analysed and in what order, the session
//! scale, Clean Vocal chaining (a changed amount re-analyses and keeps the edits), stale results,
//! edits surviving a shifted re-analysis and re-sourced clips.

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::vocal::{self, VocalDone, VocalJob};
use soundcraft_model::{AnalysisKey, ChannelFormat, CleanAlt, Clip, ClipAnalysis, ClipId, Session, SourceId, TrackId, TrackKind, VocalTune};
use soundcraft_time::Samples;
use soundcraft_vocal::{NoteEdit, NoteId, PitchClassSet, ScaleAt, SegOp};
use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

const RATE: u32 = 48_000;
const FORMANTS: [f64; 3] = [700.0, 1200.0, 2600.0];
const BANDWIDTHS: [f64; 3] = [100.0, 110.0, 160.0];

/// `secs` of a harmonic series at `f0` shaped by formant peaks at 700/1200/2600 Hz.
fn vowel(f0: f64, secs: f64) -> Vec<f32> {
    let n = (secs * f64::from(RATE)) as usize;
    let amp = |k: usize| {
        let f = f0 * k as f64;
        let res = |i: usize, a: f64| a / (1.0 + ((f - FORMANTS[i]) / BANDWIDTHS[i]).powi(2));
        res(0, 1.0) + res(1, 0.6) + res(2, 0.3) + 0.01 / k as f64
    };
    let ks: Vec<(usize, f64)> = (1..=60).filter(|k| f0 * (*k as f64) < f64::from(RATE) / 2.0).map(|k| (k, amp(k))).collect();
    let norm: f64 = ks.iter().map(|(_, a)| a).sum();
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            (0.3 * ks.iter().map(|(k, a)| a * (TAU * f0 * *k as f64 * t).sin()).sum::<f64>() / norm) as f32
        })
        .collect()
}

fn cents(hz: f64, c: f64) -> f64 {
    hz * 2f64.powf(c / 1200.0)
}

/// 0.5 s at 220 Hz −30 c, 0.2 s of silence, 0.5 s at 247 Hz +20 c.
fn take() -> Vec<f32> {
    let mut x = vowel(cents(220.0, -30.0), 0.5);
    x.extend(std::iter::repeat_n(0.0, (0.2 * f64::from(RATE)) as usize));
    x.extend(vowel(cents(247.0, 20.0), 0.5));
    x
}

fn add_take(s: &mut Session, name: &str) -> SourceId {
    soundcraft_engine::io::add_source(s, name, AudioBuffer { sample_rate: RATE, channels: vec![take()] }, None, FileFormat::Wav)
}

/// Places a clip of the whole of `src` on `t` at `start`.
fn place(s: &mut Session, t: TrackId, src: SourceId, start: Samples) -> ClipId {
    let len = take().len() as Samples;
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(s, t, Clip::audio(id, "Vox", src, 0, start, len));
    id
}

/// A mono "Vox" track with one clip of the take at 0.
fn session() -> (Engine, TrackId, ClipId, SourceId) {
    let mut s = Session::default();
    assert_eq!(s.sample_rate.hz(), RATE);
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let src = add_take(&mut s, "Vox");
    let c = place(&mut s, t, src, 0);
    (Engine::new(s), t, c, src)
}

fn clip(e: &Engine, id: ClipId) -> Clip {
    e.session().find_clip(id).map(|(_, c)| c.clone()).unwrap()
}

fn analyse_key(job: &VocalJob) -> AnalysisKey {
    match job {
        VocalJob::Analyse { key, .. } => *key,
        #[allow(unreachable_patterns)]
        _ => panic!("expected an analysis job"),
    }
}

fn analyse_source(job: &VocalJob) -> SourceId {
    analyse_key(job).source
}

/// Runs the next job to completion and installs it.
fn step(e: &mut Engine) -> bool {
    let job = vocal::next_job(e, None, 0).expect("a job");
    let done = job.run(&AtomicBool::new(false)).unwrap();
    vocal::install(e, done)
}

#[test]
fn vocal_tracks_are_analysed_in_the_background() {
    let (mut e, _, c, src) = session();
    let job = vocal::next_job(&e, None, 0).expect("the Vox clip is analysed");
    assert!(matches!(job, VocalJob::Analyse { .. }));
    assert_eq!(analyse_source(&job), src);
    let rev = e.revision;
    let undo = e.undo_history();
    assert!(vocal::install(&mut e, job.run(&AtomicBool::new(false)).unwrap()));
    assert_ne!(e.revision, rev, "an install bumps the revision");
    assert_eq!(e.undo_history(), undo, "and is no undo step");
    assert!(vocal::next_job(&e, None, 0).is_none(), "nothing left to do");
    let a = vocal::analysis_for(e.session(), &clip(&e, c)).expect("analysed");
    assert_eq!(a.notes.len(), 2, "{:?}", a.notes.iter().map(|n| (n.start, n.end, n.centre_cents)).collect::<Vec<_>>());
    assert!(!e.is_dirty(), "a derived analysis does not dirty the session");
}

#[test]
fn non_vocal_tracks_are_not_analysed() {
    let (mut e, t, _, _) = session();
    e.session_mut().track_mut(t).unwrap().name = "Keys".into();
    assert!(vocal::next_job(&e, None, 0).is_none());
}

#[test]
fn focus_and_playhead_set_priority() {
    let (mut e, t, first, src0) = session();
    let s = e.session_mut();
    let src60 = add_take(s, "Vox 2");
    let late = place(s, t, src60, 60 * Samples::from(RATE));
    let job = vocal::next_job(&e, None, 60 * Samples::from(RATE)).unwrap();
    assert_eq!(analyse_source(&job), src60, "nearest the playhead first");
    let job = vocal::next_job(&e, None, 0).unwrap();
    assert_eq!(analyse_source(&job), src0);
    let job = vocal::next_job(&e, Some(first), 60 * Samples::from(RATE)).unwrap();
    assert_eq!(analyse_source(&job), src0, "the focused clip overrides the playhead");
    let job = vocal::next_job(&e, Some(late), 0).unwrap();
    assert_eq!(analyse_source(&job), src60);
}

#[test]
fn analyse_command_blocks_until_done() {
    let (mut e, _, c, _) = session();
    let undo = e.undo_history();
    let r = e.execute("vocal.analyse", &json!({"clip": c.0})).unwrap();
    let clips = r["clips"].as_array().unwrap();
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0]["clip"], json!(c.0));
    assert_eq!(clips[0]["notes"], json!(2));
    assert!(clips[0]["unvoiced"].is_u64());
    assert_eq!(e.undo_history(), undo, "undo history unchanged");
    assert!(vocal::analysis_for(e.session(), &clip(&e, c)).is_some());
    // Already analysed: still answers.
    let r = e.execute("vocal.analyse", &json!({"track": "Vox"})).unwrap();
    assert_eq!(r["clips"][0]["notes"], json!(2));
    assert!(e.execute("vocal.analyse", &json!({"clip": 9999})).is_err());
}

#[test]
fn clean_vocal_amount_change_reanalyses_and_keeps_edits() {
    let (mut e, _, c, src) = session();
    let s = e.session_mut();
    let cleaned: Vec<f32> = take().iter().map(|v| v * 0.5).collect();
    let k = soundcraft_engine::io::add_source(s, "Vox-clean", AudioBuffer { sample_rate: RATE, channels: vec![cleaned] }, None, FileFormat::Wav);
    s.find_clip_mut(c).unwrap().clean = Some(CleanAlt { source: k, original: src, origin: 0, mix: 1.0, bypass: false });
    assert!(step(&mut e));
    let a = vocal::analysis_for(e.session(), &clip(&e, c)).unwrap();
    let first = a.notes[0].clone();
    let mut tune = VocalTune::default();
    tune.edits.insert(first.id, NoteEdit { centre: 0.4, ..NoteEdit::default() });
    e.session_mut().find_clip_mut(c).unwrap().tune = Some(tune);

    e.execute("clip.clean_mix", &json!({"clip": c.0, "mix": 60})).unwrap();
    let job = vocal::next_job(&e, None, 0).expect("a changed amount re-analyses");
    let key = analyse_key(&job);
    assert_eq!(Some(key), AnalysisKey::of(e.session(), &clip(&e, c)));
    assert!(vocal::install(&mut e, job.run(&AtomicBool::new(false)).unwrap()));

    let cl = clip(&e, c);
    let a = vocal::analysis_for(e.session(), &cl).unwrap();
    let tune = cl.tune_live().unwrap();
    assert_eq!(tune.analysed, key.hash());
    let on = a.notes.iter().find(|n| n.end.min(first.end).saturating_sub(n.start.max(first.start)) * 2 >= n.end - n.start).unwrap();
    assert_eq!(tune.edits.get(&on.id).map(|e| e.centre), Some(0.4), "the edit sits on the note covering the same frames");
    assert_eq!(tune.note_spans.get(&on.id), Some(&(on.start, on.end)));
    assert_eq!(e.session().vocal.analyses.len(), 1, "the old amount's analysis is dropped");
}

#[test]
fn scale_at_reads_key_changes_then_the_band_key() {
    let (mut e, _, c, _) = session();
    let e_major = PitchClassSet::from_pcs(&[4, 6, 8, 9, 11, 1, 3]);
    let d_major = PitchClassSet::from_pcs(&[2, 4, 6, 7, 9, 11, 1]);
    {
        let s = e.session_mut();
        s.band.key = 4;
        s.band.scale = 0;
    }
    // 200 frames a second (5 ms hops).
    let sc = vocal::scale_at(e.session(), &clip(&e, c));
    assert_eq!(sc.scale_at(0), Some(e_major));
    assert_eq!(sc.scale_at(300), Some(e_major));
    e.execute("event.add_key_change", &json!({"key": "D major", "at": {"seconds": 1.0}})).unwrap();
    let sc = vocal::scale_at(e.session(), &clip(&e, c));
    assert_eq!(sc.scale_at(100), Some(e_major), "before the key change: the band key");
    assert_eq!(sc.scale_at(300), Some(d_major), "after it: D major");
    // A clip placed later reads the key at its own session time.
    let mut moved = clip(&e, c);
    moved.start = Samples::from(RATE);
    assert_eq!(vocal::scale_at(e.session(), &moved).scale_at(0), Some(d_major));
    // The command refuses it now; a session saved before that may still hold one.
    {
        let s = e.session_mut();
        let at = s.key_signatures[0].0;
        s.key_signatures[0] = (at, "Q blorp".into());
    }
    let sc = vocal::scale_at(e.session(), &clip(&e, c));
    assert_eq!(sc.scale_at(300), Some(e_major), "an unparsable key falls through to the band key");
    // Minor keys add the raised 7th, as MIDI Cleanup reads them.
    e.execute("event.add_key_change", &json!({"key": "A minor", "at": {"seconds": 1.0}})).unwrap();
    let sc = vocal::scale_at(e.session(), &clip(&e, c));
    assert_eq!(sc.scale_at(300), Some(PitchClassSet::from_pcs(&[9, 11, 0, 2, 4, 5, 7, 8])));
}

#[test]
fn key_names_parse() {
    assert_eq!(vocal::parse_key("C major"), Some((0, 0)));
    assert_eq!(vocal::parse_key("F# minor"), Some((6, 1)));
    assert_eq!(vocal::parse_key("Bb"), Some((10, 0)));
    assert_eq!(vocal::parse_key("ebm"), Some((3, 1)));
    assert_eq!(vocal::parse_key("  a  Minor "), Some((9, 1)));
    assert_eq!(vocal::parse_key("Cb major"), Some((11, 0)));
    assert_eq!(vocal::parse_key("Q blorp"), None);
    assert_eq!(vocal::parse_key("C dorian"), None);
    assert_eq!(vocal::parse_key(""), None);
}

#[test]
fn stale_analysis_is_dropped() {
    let (mut e, _, c, src) = session();
    let s = e.session_mut();
    let k = soundcraft_engine::io::add_source(s, "Vox-clean", AudioBuffer { sample_rate: RATE, channels: vec![take()] }, None, FileFormat::Wav);
    s.find_clip_mut(c).unwrap().clean = Some(CleanAlt { source: k, original: src, origin: 0, mix: 1.0, bypass: false });
    let job = vocal::next_job(&e, None, 0).unwrap();
    let done = job.run(&AtomicBool::new(false)).unwrap();
    e.execute("clip.clean_remove", &json!({"clip": c.0})).unwrap();
    assert!(!vocal::install(&mut e, done), "the result matches no clip any more");
    assert!(e.session().vocal.analyses.is_empty(), "no unused keys cached");
}

/// The analysis under `key`, every note moved `by` frames later (ids follow the starts).
fn shifted(a: &ClipAnalysis, by: u32) -> ClipAnalysis {
    let mut b = a.clone();
    for n in &mut b.analysis.notes {
        n.start += by;
        n.end += by;
        n.id = NoteId(n.start);
        n.scoop = None;
    }
    b
}

#[test]
fn edits_survive_a_shifted_reanalysis() {
    let (mut e, _, c, _) = session();
    assert!(step(&mut e));
    let key = AnalysisKey::of(e.session(), &clip(&e, c)).unwrap();
    let a = Arc::clone(e.session().vocal.analyses.get(&key.hash()).unwrap());
    let (n0, n1) = (a.analysis.notes[0].clone(), a.analysis.notes[1].clone());
    let mut tune = VocalTune::default();
    tune.edits.insert(n0.id, NoteEdit { centre: 0.4, ..NoteEdit::default() });
    tune.edits.insert(n1.id, NoteEdit { drift: 0.3, ..NoteEdit::default() });
    e.session_mut().find_clip_mut(c).unwrap().tune = Some(tune);

    // The same key analyses slightly differently (a reload on another build): starts move 2 frames.
    let b = shifted(&a, 2);
    assert!(vocal::install(&mut e, VocalDone::Analysed(Arc::new(b.clone()), None)));
    let cl = clip(&e, c);
    let tune = cl.tune_live().unwrap();
    let (m0, m1) = (&b.analysis.notes[0], &b.analysis.notes[1]);
    assert_eq!(tune.edits.get(&m0.id).map(|x| x.centre), Some(0.4), "{:?}", tune.edits.keys().collect::<Vec<_>>());
    assert_eq!(tune.edits.get(&m1.id).map(|x| x.drift), Some(0.3));
    assert_eq!(tune.edits.len(), 2);
    assert_eq!(tune.note_spans.get(&m0.id), Some(&(m0.start, m0.end)), "spans follow the new notes");
    assert_eq!(tune.analysed, key.hash());

    // And once more, 1 frame back: matched again from the saved spans, not the ids.
    let b2 = shifted(&b, 1);
    assert!(vocal::install(&mut e, VocalDone::Analysed(Arc::new(b2.clone()), None)));
    let cl = clip(&e, c);
    let tune = cl.tune_live().unwrap();
    assert_eq!(tune.edits.get(&b2.analysis.notes[0].id).map(|x| x.centre), Some(0.4));
    assert_eq!(tune.edits.get(&b2.analysis.notes[1].id).map(|x| x.drift), Some(0.3));
}

#[test]
fn join_ops_follow_a_shifted_reanalysis() {
    let (mut e, _, c, _) = session();
    assert!(step(&mut e));
    let key = AnalysisKey::of(e.session(), &clip(&e, c)).unwrap();
    let a = Arc::clone(e.session().vocal.analyses.get(&key.hash()).unwrap());
    let (n0, n1) = (a.analysis.notes[0].clone(), a.analysis.notes[1].clone());
    let mut tune = VocalTune::default();
    tune.seg_ops.push(SegOp::Join { at: n1.id.0 });
    tune.edits.insert(n0.id, NoteEdit { centre: 0.5, ..NoteEdit::default() });
    e.session_mut().find_clip_mut(c).unwrap().tune = Some(tune);
    assert_eq!(vocal::analysis_for(e.session(), &clip(&e, c)).unwrap().notes.len(), 1, "joined");

    let b = shifted(&a, 2);
    assert!(vocal::install(&mut e, VocalDone::Analysed(Arc::new(b.clone()), None)));
    let cl = clip(&e, c);
    let tune = cl.tune_live().unwrap();
    assert_eq!(tune.seg_ops, vec![SegOp::Join { at: b.analysis.notes[1].id.0 }], "the join names the new note");
    let joined = vocal::analysis_for(e.session(), &cl).unwrap();
    assert_eq!(joined.notes.len(), 1, "still joined");
    assert_eq!(tune.edits.get(&joined.notes[0].id).map(|x| x.centre), Some(0.5));
}

#[test]
fn resourced_clip_ignores_its_tuning() {
    let (mut e, _, c, _) = session();
    assert!(step(&mut e));
    let key = AnalysisKey::of(e.session(), &clip(&e, c)).unwrap();
    let a = vocal::analysis_for(e.session(), &clip(&e, c)).unwrap();
    let mut tune = VocalTune::default();
    tune.edits.insert(a.notes[0].id, NoteEdit { centre: 1.0, ..NoteEdit::default() });
    e.session_mut().find_clip_mut(c).unwrap().tune = Some(tune);
    // An install records the source the tuning was made against.
    let same = Arc::clone(e.session().vocal.analyses.get(&key.hash()).unwrap());
    assert!(vocal::install(&mut e, VocalDone::Analysed(same, None)));
    assert!(clip(&e, c).tune_live().is_some());

    // Consolidate/Process give the clip a new source: the tuning goes inert.
    let s = e.session_mut();
    let fresh = add_take(s, "Vox-consolidated");
    if let soundcraft_model::ClipContent::Audio { source, .. } = &mut s.find_clip_mut(c).unwrap().content {
        *source = fresh;
    }
    let cl = clip(&e, c);
    assert!(cl.tune.is_some(), "kept (Undo brings the old source back)");
    assert!(cl.tune_live().is_none(), "but ignored");
    let before = cl.tune.clone();
    vocal::run_until_idle(&mut e, &[c]).unwrap();
    assert_eq!(clip(&e, c).tune, before, "an install leaves an inert tuning alone");
    assert!(vocal::analysis_for(e.session(), &clip(&e, c)).is_some(), "the new source is still analysed (Vox track)");
}

#[test]
fn focused_clip_on_a_non_vocal_track_is_analysed() {
    let (mut e, t, c, src) = session();
    e.session_mut().track_mut(t).unwrap().name = "Audio 3".into();
    assert!(vocal::next_job(&e, None, 0).is_none(), "not a Vocal track");
    let job = vocal::next_job(&e, Some(c), 0).expect("the focused clip is analysed anyway");
    assert!(matches!(job, VocalJob::Analyse { .. }));
    assert_eq!(analyse_source(&job), src);
}

#[test]
fn failed_analysis_is_not_offered_again() {
    let (mut e, _, c, _) = session();
    let job = vocal::next_job(&e, None, 0).expect("offered");
    let key = analyse_key(&job);
    // A job the tracker refuses (here: an unsupported rate) fails.
    #[allow(irrefutable_let_patterns)]
    let VocalJob::Analyse { session, clip, .. } = job else { panic!("expected an analysis job") };
    let bad = VocalJob::Analyse { key, session, clip, rate: 1_000 };
    assert!(bad.run(&AtomicBool::new(false)).is_err());
    let undo = e.undo_history();
    vocal::mark_failed(&mut e, key.hash());
    assert!(vocal::next_job(&e, None, 0).is_none(), "a failed key is not offered again");
    assert!(vocal::next_job(&e, Some(c), 0).is_none(), "not even for the focused clip");
    assert_eq!(e.undo_history(), undo, "no undo step");
    assert!(!e.is_dirty(), "derived data");
    // Kept across Undo of a later edit.
    e.execute("track.vocal", &json!({"tracks": ["Vox"], "value": true})).unwrap();
    assert!(e.undo());
    assert!(e.session().vocal.failed.contains(&key.hash()));
    assert!(vocal::next_job(&e, None, 0).is_none());
}

#[test]
fn stale_render_is_dropped_after_undo() {
    let (mut e, _, c, _) = session();
    e.execute("vocal.analyse", &json!({"clip": c.0})).unwrap();
    let n = vocal::analysis_for(e.session(), &clip(&e, c)).unwrap().notes[0].clone();
    e.execute("vocal.edit", &json!({"notes": [{"clip": c.0, "note": n.id.0}], "centre": 100})).unwrap();
    let job = vocal::next_job(&e, None, 0).expect("a render job");
    assert!(matches!(job, VocalJob::Render(_)));
    assert!(e.undo());
    let renders = Arc::clone(&e.session().vocal.renders);
    let done = job.run(&AtomicBool::new(false)).unwrap();
    assert!(!vocal::install(&mut e, done), "the edit it was made for is gone");
    assert!(Arc::ptr_eq(&e.session().vocal.renders, &renders), "renders map unchanged");
}

/// `secs` of 1.5 s notes (each a different pitch) with 0.2 s gaps.
fn long_take(secs: f64) -> Vec<f32> {
    let mut x = Vec::new();
    let pitches = [220.0, 247.0, 262.0, 294.0, 330.0, 294.0, 262.0, 247.0];
    let mut i = 0;
    while (x.len() as f64) < secs * f64::from(RATE) {
        x.extend(vowel(cents(pitches[i % pitches.len()], 25.0), 1.5));
        x.extend(std::iter::repeat_n(0.0, (0.2 * f64::from(RATE)) as usize));
        i += 1;
    }
    x.truncate((secs * f64::from(RATE)) as usize);
    x
}

#[cfg_attr(debug_assertions, ignore = "timing: run with --release")]
#[test]
fn a_slider_step_is_heard_within_a_quarter_second() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let audio = long_take(13.0);
    let n = audio.len() as Samples;
    let src = soundcraft_engine::io::add_source(&mut s, "Vox", AudioBuffer { sample_rate: RATE, channels: vec![audio] }, None, FileFormat::Wav);
    let c = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(c, "Vox", src, 0, 0, n));
    let mut e = Engine::new(s);
    e.execute("vocal.analyse", &json!({"clip": c.0})).unwrap();
    let notes = vocal::analysis_for(e.session(), &clip(&e, c)).unwrap().notes;
    let long = notes.iter().filter(|n| n.end - n.start >= 250).nth(3).expect("a 1.5 s note").clone();
    let ed = |e: &mut Engine, centre: u32| {
        e.execute_merged("vocal.edit", &json!({"notes": [{"clip": c.0, "note": long.id.0}], "centre": centre}), "slider").unwrap();
    };
    ed(&mut e, 40);
    e.execute("vocal.analyse", &json!({"clip": c.0})).unwrap();
    ed(&mut e, 50);
    let t0 = std::time::Instant::now();
    let job = vocal::next_job(&e, None, 0).expect("a render job");
    assert!(matches!(job, VocalJob::Render(_)));
    let done = job.run(&AtomicBool::new(false)).unwrap();
    assert!(vocal::install(&mut e, done));
    let took = t0.elapsed();
    assert!(took.as_millis() < 250, "a slider step took {took:?}");
}

#[test]
fn the_analysis_seeds_the_heard_audio_for_the_first_render() {
    let (mut e, _, c, _) = session();
    // A tune must exist before the analysis: only tuned clips keep their heard audio.
    e.session_mut().find_clip_mut(c).unwrap().tune = Some(VocalTune::default());
    assert!(step(&mut e));
    let key = AnalysisKey::of(e.session(), &clip(&e, c)).unwrap().hash();
    let heard = Arc::clone(e.session().vocal.heard.get(&key).expect("the analysis cached its heard audio"));
    let n = vocal::analysis_for(e.session(), &clip(&e, c)).unwrap().notes[0].id;
    e.execute("vocal.edit", &json!({"notes": [{"clip": c.0, "note": n.0}], "centre": 100})).unwrap();
    let Some(VocalJob::Render(job)) = vocal::next_job(&e, None, 0) else { panic!("a render job") };
    assert!(job.audio.as_ref().is_some_and(|a| Arc::ptr_eq(a, &heard)), "the first render reuses it");
}

#[test]
fn untuned_clips_do_not_keep_heard_audio() {
    let (mut e, _, c, _) = session();
    assert!(step(&mut e));
    let cl = clip(&e, c);
    assert!(vocal::analysis_for(e.session(), &cl).is_some(), "analysed");
    let key = AnalysisKey::of(e.session(), &cl).unwrap().hash();
    assert!(!e.session().vocal.heard.contains_key(&key), "an untuned clip keeps no heard audio");
}

#[test]
fn clip_with_missing_source_is_not_offered() {
    let (mut e, _, _, src) = session();
    e.session_mut().pool.remove(src);
    assert!(vocal::next_job(&e, None, 0).is_none(), "no job for a clip whose audio is gone");
}

#[test]
fn clip_with_missing_source_is_not_analysable_and_the_note_commands_say_so() {
    let (mut e, t, c, src) = session();
    assert!(vocal::analysable(e.session(), &clip(&e, c)), "audio in the pool");
    e.session_mut().pool.remove(src);
    assert!(!vocal::analysable(e.session(), &clip(&e, c)), "the audio is gone");
    assert!(vocal::cannot_analyse(e.session(), &clip(&e, c)));
    let want = "Vox can't be analysed (missing audio, or longer than 30 min: Edit › Split into Files cuts a long recording into songs)";
    let err = e.execute("vocal.notes", &json!({"clip": c.0})).unwrap_err().to_string();
    assert!(err.contains(want), "{err}");
    let err = e.execute("vocal.step", &json!({"track": t.0, "at": 1000, "kind": "centre"})).unwrap_err().to_string();
    assert!(err.contains(want), "{err}");
    let err = e.execute("vocal.split", &json!({"clip": c.0, "note": 0, "at": 10})).unwrap_err().to_string();
    assert!(err.contains(want), "{err}");
}

#[test]
fn next_job_is_cheap_on_a_long_stereo_source() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Vox"));
    let n = 4 * 60 * RATE as usize;
    let l: Vec<f32> = (0..n).map(|i| ((i % 480) as f32 / 480.0 - 0.5) * 0.2).collect();
    let r: Vec<f32> = l.iter().map(|v| v * 0.7).collect();
    let src = soundcraft_engine::io::add_source(&mut s, "Vox", AudioBuffer { sample_rate: RATE, channels: vec![l, r] }, None, FileFormat::Wav);
    let c = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(c, "Vox", src, 0, 0, n as Samples));
    let e = Engine::new(s);
    let t0 = std::time::Instant::now();
    let job = vocal::next_job(&e, None, 0).expect("an analysis job");
    let took = t0.elapsed();
    assert!(matches!(job, VocalJob::Analyse { .. }));
    assert!(took.as_millis() < 5, "next_job took {took:?}: the heard audio is built on the worker");
}
