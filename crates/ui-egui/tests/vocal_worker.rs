//! The app's vocal worker: analyses a Vocal track in the background without blocking a frame,
//! and a render superseded by a newer edit is cancelled so the newest edit is what plays.

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::vocal;
use soundcraft_model::{ChannelFormat, Clip, ClipId, Session, TrackKind};
use soundcraft_time::Samples;
use soundcraft_ui_egui::vocal_worker::VocalWorker;
use std::f64::consts::TAU;
use std::time::{Duration, Instant};

const RATE: u32 = 48_000;

/// `secs` of a formant-shaped harmonic series at `f0`.
fn vowel(f0: f64, secs: f64) -> Vec<f32> {
    let n = (secs * f64::from(RATE)) as usize;
    let amp = |k: usize| {
        let f = f0 * k as f64;
        let res = |c: f64, b: f64, a: f64| a / (1.0 + ((f - c) / b).powi(2));
        res(700.0, 100.0, 1.0) + res(1200.0, 110.0, 0.6) + res(2600.0, 160.0, 0.3) + 0.01 / k as f64
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

/// A "Lead Vox" track holding one clip: 0.5 s at 220 Hz −30 c, 0.2 s of silence, 0.5 s at
/// 247 Hz +20 c.
fn engine() -> (Engine, ClipId) {
    let mut x = vowel(220.0 * 2f64.powf(-30.0 / 1200.0), 0.5);
    x.extend(std::iter::repeat_n(0.0, 9_600));
    x.extend(vowel(247.0 * 2f64.powf(20.0 / 1200.0), 0.5));
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead Vox"));
    let n = x.len() as Samples;
    let src = soundcraft_engine::io::add_source(&mut s, "Vox", AudioBuffer { sample_rate: RATE, channels: vec![x] }, None, FileFormat::Wav);
    let c = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(c, "Vox", src, 0, 0, n));
    (Engine::new(s), c)
}

fn clip(e: &Engine, id: ClipId) -> Clip {
    e.session().find_clip(id).map(|(_, c)| c.clone()).unwrap()
}

/// Polls like the app's frames do until `done` holds (30 s at most); every poll must return
/// within 20 ms. Returns whether `done` held.
fn poll_until(w: &mut VocalWorker, e: &mut Engine, mut done: impl FnMut(&Engine, &VocalWorker) -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        let t = Instant::now();
        w.poll(e, None, 0);
        let took = t.elapsed();
        assert!(took < Duration::from_millis(20), "a poll took {took:?}");
        if done(e, w) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn worker_analyses_a_vocal_track_without_blocking() {
    let (mut e, c) = engine();
    let mut w = VocalWorker::new();
    assert!(w.busy().is_none());
    w.poll(&mut e, None, 0);
    assert_eq!(w.busy().as_deref(), Some("Analysing Lead Vox…"), "the status bar names the track");
    let ok = poll_until(&mut w, &mut e, |e, _| vocal::analysis_for(e.session(), &clip(e, c)).is_some());
    assert!(ok, "analysed in the background");
    assert!(e.undo_history().is_empty(), "no undo step");
}

#[test]
fn worker_cancels_a_render_superseded_by_an_edit() {
    let (mut e, c) = engine();
    e.execute("vocal.analyse", &json!({"clip": c.0})).unwrap();
    let note = vocal::analysis_for(e.session(), &clip(&e, c)).unwrap().notes[0].id;
    let edit = |e: &mut Engine, centre: u32| {
        e.execute_merged("vocal.edit", &json!({"notes": [{"clip": c.0, "note": note.0}], "centre": centre}), "drag").unwrap();
    };
    let mut w = VocalWorker::new();
    edit(&mut e, 50);
    w.poll(&mut e, None, 0);
    assert_eq!(w.busy().as_deref(), Some("Tuning Lead Vox…"), "a render started");
    edit(&mut e, 100);
    let latest = vocal::render_inputs(e.session(), &clip(&e, c)).unwrap();
    let ok = poll_until(&mut w, &mut e, |e, w| !w.running() && vocal::next_job(e, None, 0).is_none());
    assert!(ok, "the worker went idle");
    let r = e.session().vocal.renders.get(&c).expect("a render is installed");
    assert_eq!(r.inputs, latest, "the newest edit's render is the one kept");
    assert_eq!(clip(&e, c).tune.unwrap().fingerprint, latest, "and it plays");
}

#[test]
fn an_analysis_superseded_by_a_clean_amount_comes_back_after_undo() {
    let (mut e, c) = engine();
    // Clean Vocal on the clip: a quieter copy of the take, fully cleaned.
    let s = e.session_mut();
    let src = clip_of_session(s, c).source().unwrap();
    let cleaned: Vec<f32> = s.pool.get(src).unwrap().buffer().channels[0].iter().map(|v| 0.5 * v).collect();
    let k = soundcraft_engine::io::add_source(s, "Vox-clean", AudioBuffer { sample_rate: RATE, channels: vec![cleaned] }, None, FileFormat::Wav);
    s.find_clip_mut(c).unwrap().clean = Some(soundcraft_model::CleanAlt { source: k, original: src, origin: 0, mix: 1.0, bypass: false });
    let mut w = VocalWorker::new();
    w.poll(&mut e, None, 0);
    assert!(w.running(), "the analysis at 100 % started");
    let first = soundcraft_model::AnalysisKey::of(e.session(), &clip(&e, c)).unwrap().hash();
    e.execute("clip.clean_mix", &json!({"clip": c.0, "mix": 60})).unwrap();
    let ok = poll_until(&mut w, &mut e, |e, w| !w.running() && vocal::next_job(e, None, 0).is_none());
    assert!(ok, "idle at 60 %");
    assert!(!e.session().vocal.failed.contains(&first), "the unused 100 % analysis is not marked failed");
    assert!(e.undo());
    let ok = poll_until(&mut w, &mut e, |e, _| vocal::analysis_for(e.session(), &clip(e, c)).is_some());
    assert!(ok, "after Undo the clip is analysed again at 100 %");
}

fn clip_of_session(s: &Session, id: ClipId) -> Clip {
    s.find_clip(id).map(|(_, c)| c.clone()).unwrap()
}

#[test]
fn an_idle_worker_does_not_rescan_until_something_changes() {
    let (mut e, c) = engine();
    let mut w = VocalWorker::new();
    let ok = poll_until(&mut w, &mut e, |e, w| !w.running() && vocal::analysis_for(e.session(), &clip(e, c)).is_some());
    assert!(ok, "analysed");
    // One more poll finds nothing to do; then idle frames ask the engine nothing.
    w.poll(&mut e, None, 0);
    let scans = w.scans();
    for _ in 0..20 {
        w.poll(&mut e, None, 0);
    }
    assert_eq!(w.scans(), scans, "an idle worker doesn't re-scan the session every frame");
    // An edit (a new revision) is looked at once.
    let note = vocal::analysis_for(e.session(), &clip(&e, c)).unwrap().notes[0].id;
    e.execute("vocal.edit", &json!({"notes": [{"clip": c.0, "note": note.0}], "centre": 50})).unwrap();
    w.poll(&mut e, None, 0);
    assert_eq!(w.scans(), scans + 1, "a changed session is scanned again");
    assert!(w.running(), "and its render starts");
}
