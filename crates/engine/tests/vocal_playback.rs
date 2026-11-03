//! Vocal tuning's playback: the render cache read by playback and bounce. An untuned clip plays
//! exactly what it did; a tuned note plays shifted and everything outside the rendered spans is
//! bit-identical; Original, a missing render or a stale one (after Undo) play the heard audio.

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::vocal::{self, VocalJob};
use soundcraft_model::{ChannelFormat, CleanAlt, Clip, ClipId, Session, SourceId, TrackId, TrackKind};
use soundcraft_time::{Range, Samples};
use soundcraft_vocal::Note;
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

fn len() -> Samples {
    take().len() as Samples
}

/// A mono "Vox" track with one clip of the whole take at 0, `extra` samples longer than the
/// source (trimmed open past its end).
fn session_with(extra: Samples) -> (Engine, TrackId, ClipId, SourceId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let src = soundcraft_engine::io::add_source(&mut s, "Vox", AudioBuffer { sample_rate: RATE, channels: vec![take()] }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "Vox", src, 0, 0, len() + extra));
    (Engine::new(s), t, id, src)
}

fn session() -> (Engine, TrackId, ClipId, SourceId) {
    session_with(0)
}

fn clip(e: &Engine, id: ClipId) -> Clip {
    e.session().find_clip(id).map(|(_, c)| c.clone()).unwrap()
}

fn bounce_len(e: &Engine, t: TrackId, n: Samples) -> Vec<f32> {
    soundcraft_mix::render_clips(e.session(), t, Range::new(0, n)).into_iter().next().unwrap()
}

fn bounce(e: &Engine, t: TrackId) -> Vec<f32> {
    bounce_len(e, t, len())
}

/// Analyses (and renders) the clip, blocking.
fn analyse(e: &mut Engine, c: ClipId) {
    e.execute("vocal.analyse", &json!({"clip": c.0})).unwrap();
}

fn notes(e: &Engine, c: ClipId) -> Vec<Note> {
    vocal::analysis_for(e.session(), &clip(e, c)).unwrap().notes
}

fn edit(e: &mut Engine, c: ClipId, note: &Note, centre: f64) {
    e.execute("vocal.edit", &json!({"notes": [{"clip": c.0, "note": note.id.0}], "centre": centre})).unwrap();
}

/// Whether source position `at` lies in any of the clip's cached spans.
fn in_span(e: &Engine, c: ClipId, at: Samples) -> bool {
    e.session().vocal.renders.get(&c).is_some_and(|r| r.spans.iter().any(|s| s.start <= at && at < s.end()))
}

/// The centre (cents) of the re-tracked note of `audio` overlapping frames `[a, b)` most.
fn retracked_centre(audio: &[f32], a: u32, b: u32) -> f32 {
    let an = soundcraft_vocal::analyse(audio, RATE, &AtomicBool::new(false)).unwrap();
    an.notes.iter().max_by_key(|n| n.end.min(b).saturating_sub(n.start.max(a))).map(|n| n.centre_cents).unwrap()
}

#[test]
fn untuned_clip_plays_bit_identical() {
    let (mut e, t, c, _) = session();
    let before = bounce(&e, t);
    analyse(&mut e, c);
    assert!(vocal::analysis_for(e.session(), &clip(&e, c)).is_some());
    assert_eq!(bounce(&e, t), before, "an analysis alone changes nothing");
}

#[test]
fn tuned_note_plays_shifted_and_the_rest_is_untouched() {
    let (mut e, t, c, _) = session();
    analyse(&mut e, c);
    let untuned = bounce(&e, t);
    let ns = notes(&e, c);
    let (n1, n2) = (ns[0].clone(), ns[1].clone());
    edit(&mut e, c, &n1, 100.0);
    analyse(&mut e, c);
    let tuned = bounce(&e, t);
    assert_ne!(tuned, untuned);
    let before = retracked_centre(&untuned, n1.start, n1.end);
    let after = retracked_centre(&tuned, n1.start, n1.end);
    assert!((after - before - 30.0).abs() <= 2.0, "note 1 moved {} c (want +30)", after - before);
    // Outside every rendered span, the tuned bounce is the untuned one exactly.
    let mut outside_n2 = 0;
    let hop = 240;
    for (i, (a, b)) in tuned.iter().zip(&untuned).enumerate() {
        let at = i as Samples;
        if !in_span(&e, c, at) {
            assert_eq!(a.to_bits(), b.to_bits(), "sample {i} outside the spans");
            if at >= Samples::from(n2.start) * hop && at < Samples::from(n2.end) * hop {
                outside_n2 += 1;
            }
        }
    }
    assert!(outside_n2 > 10_000, "most of note 2 lies beyond the span dilation ({outside_n2})");
}

#[test]
fn original_switch_plays_the_untuned_audio() {
    let (mut e, t, c, _) = session();
    analyse(&mut e, c);
    let untuned = bounce(&e, t);
    let n1 = notes(&e, c)[0].clone();
    edit(&mut e, c, &n1, 100.0);
    analyse(&mut e, c);
    let tuned = bounce(&e, t);
    assert_ne!(tuned, untuned);
    e.session_mut().find_clip_mut(c).unwrap().tune.as_mut().unwrap().original = true;
    assert_eq!(bounce(&e, t), untuned, "Original plays the untuned clip");
    e.session_mut().find_clip_mut(c).unwrap().tune.as_mut().unwrap().original = false;
    assert_eq!(bounce(&e, t), tuned, "and back to Tuned");
}

#[test]
fn missing_render_plays_heard_audio() {
    let (mut e, t, c, _) = session();
    analyse(&mut e, c);
    let untuned = bounce(&e, t);
    let n1 = notes(&e, c)[0].clone();
    edit(&mut e, c, &n1, 100.0);
    assert!(matches!(vocal::next_job(&e, None, 0), Some(VocalJob::Render(_))), "a render is due");
    assert_eq!(bounce(&e, t), untuned, "until it is in, the heard audio plays");
}

#[test]
fn tuning_follows_clean_vocal() {
    let (mut e, t, c, src) = session();
    let s = e.session_mut();
    // The cleaned alternate: the same take a little quieter and lightly distorted.
    let cleaned: Vec<f32> = take().iter().map(|v| 0.5 * v + 0.2 * v * v.abs()).collect();
    let k = soundcraft_engine::io::add_source(s, "Vox-clean", AudioBuffer { sample_rate: RATE, channels: vec![cleaned] }, None, FileFormat::Wav);
    s.find_clip_mut(c).unwrap().clean = Some(CleanAlt { source: k, original: src, origin: 0, mix: 0.6, bypass: false });
    analyse(&mut e, c);
    let blend = bounce(&e, t);
    let n1 = notes(&e, c)[0].clone();
    edit(&mut e, c, &n1, 100.0);
    analyse(&mut e, c);
    let tuned = bounce(&e, t);
    let moved = retracked_centre(&tuned, n1.start, n1.end) - retracked_centre(&blend, n1.start, n1.end);
    assert!((moved - 30.0).abs() <= 2.0, "the 60 % blend shifted {moved} c (want +30)");
    for (i, (a, b)) in tuned.iter().zip(&blend).enumerate() {
        if !in_span(&e, c, i as Samples) {
            assert_eq!(a.to_bits(), b.to_bits(), "outside note 1 the clean blend plays exactly (sample {i})");
        }
    }
}

#[test]
fn window_past_source_end_is_safe() {
    let extra = 24_000;
    let (mut e, t, c, _) = session_with(extra);
    analyse(&mut e, c);
    let n1 = notes(&e, c)[0].clone();
    edit(&mut e, c, &n1, 100.0);
    analyse(&mut e, c);
    assert!(e.session().vocal.renders.get(&c).is_some_and(|r| !r.spans.is_empty()));
    let out = bounce_len(&e, t, len() + extra);
    assert_eq!(out.len() as Samples, len() + extra, "the clip's length");
    assert!(out[len() as usize..].iter().all(|v| *v == 0.0), "silence past the source");
    let w = soundcraft_mix::clip_source_window(e.session(), &clip(&e, c), 0, (len() + extra) as usize).unwrap();
    assert!(w[0][len() as usize..].iter().all(|v| *v == 0.0));
}

#[test]
fn changing_one_note_rerenders_only_that_note() {
    let (mut e, _, c, _) = session();
    analyse(&mut e, c);
    let ns = notes(&e, c);
    let (n1, n2) = (ns[0].clone(), ns[1].clone());
    // Notes 200 ms apart: their spans never touch.
    edit(&mut e, c, &n1, 100.0);
    edit(&mut e, c, &n2, 50.0);
    analyse(&mut e, c);
    let full = Arc::clone(e.session().vocal.renders.get(&c).unwrap());
    let n1_end = Samples::from(n1.end) * 240;
    let first = full.spans.iter().find(|s| s.start < n1_end).cloned().expect("a span over note 1");

    edit(&mut e, c, &n2, 100.0);
    let Some(VocalJob::Render(job)) = vocal::next_job(&e, None, 0) else { panic!("a render job") };
    assert!(job.base.is_some(), "spliced over the cached render");
    assert!(!job.frames.is_empty());
    for f in &job.frames {
        assert!(f.start + 4 >= n2.start && f.end <= n2.end + 4, "frames {f:?} lie inside note 2 ({}..{}) ± 4", n2.start, n2.end);
    }
    assert!(vocal::install(&mut e, VocalJob::Render(job).run(&AtomicBool::new(false)).unwrap()));
    let after = e.session().vocal.renders.get(&c).unwrap();
    let kept = after.spans.iter().find(|s| s.start == first.start).expect("note 1's span kept");
    assert!(Arc::ptr_eq(&kept.channels[0], &first.channels[0]), "note 1's span is the same data");
}

#[test]
fn undo_does_not_play_a_newer_render() {
    let (mut e, t, c, _) = session();
    analyse(&mut e, c);
    let untuned = bounce(&e, t);
    let n1 = notes(&e, c)[0].clone();
    edit(&mut e, c, &n1, 50.0);
    analyse(&mut e, c);
    let first = bounce(&e, t);
    e.end_merge();
    edit(&mut e, c, &n1, 100.0);
    analyse(&mut e, c);
    let second = bounce(&e, t);
    assert_ne!(first, second);
    assert!(e.undo());
    let now = bounce(&e, t);
    assert_ne!(now, second, "the newer render is not played after Undo");
    assert_eq!(now, untuned, "the heard audio plays until the first edit is rendered again");
    // Rendering again plays the first edit.
    analyse(&mut e, c);
    let again = bounce(&e, t);
    assert_ne!(again, second);
    assert!(e.redo());
    assert_eq!(bounce(&e, t), untuned, "Redo plays the heard audio, never the first edit's render");
}

#[test]
fn stereo_channels_that_differ_are_each_shifted() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Vox"));
    let l = take();
    let r: Vec<f32> = l.iter().map(|v| v * 0.7).collect();
    let src = soundcraft_engine::io::add_source(&mut s, "Vox", AudioBuffer { sample_rate: RATE, channels: vec![l, r] }, None, FileFormat::Wav);
    let c = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(c, "Vox", src, 0, 0, len()));
    let mut e = Engine::new(s);
    analyse(&mut e, c);
    let untuned = soundcraft_mix::render_clips(e.session(), t, Range::new(0, len()));
    let n1 = notes(&e, c)[0].clone();
    edit(&mut e, c, &n1, 100.0);
    analyse(&mut e, c);
    let spans = Arc::clone(e.session().vocal.renders.get(&c).unwrap());
    assert!(spans.spans.iter().all(|s| s.channels.len() == 2 && !Arc::ptr_eq(&s.channels[0], &s.channels[1])), "rendered per channel");
    let tuned = soundcraft_mix::render_clips(e.session(), t, Range::new(0, len()));
    assert_eq!(tuned.len(), 2);
    for ch in 0..2 {
        let moved = retracked_centre(&tuned[ch], n1.start, n1.end) - retracked_centre(&untuned[ch], n1.start, n1.end);
        assert!((moved - 30.0).abs() <= 2.0, "channel {ch} moved {moved} c (want +30)");
    }
}

#[test]
fn a_failed_render_is_reported_refused_and_skipped() {
    let (mut e, t, c, _) = session();
    analyse(&mut e, c);
    let untuned = bounce(&e, t);
    let n1 = notes(&e, c)[0].clone();
    edit(&mut e, c, &n1, 100.0);
    let inputs = vocal::render_inputs(e.session(), &clip(&e, c)).unwrap();
    vocal::mark_failed(&mut e, inputs);
    assert!(vocal::render_failed(e.session(), &clip(&e, c)));
    assert!(vocal::next_job(&e, None, 0).is_none(), "the failed render is not offered again");
    let r = e.execute("vocal.notes", &json!({"clip": c.0})).unwrap();
    assert_eq!(r["messages"][0]["clip"], json!(c.0), "{r}");
    assert!(r["messages"][0]["message"].as_str().unwrap().contains("can't be rendered"));
    let r = e.execute("vocal.analyse", &json!({"clip": c.0})).unwrap();
    assert!(r["clips"][0]["message"].as_str().unwrap().contains("plays untuned"), "{r}");
    let err = e.execute("vocal.commit", &json!({"clip": c.0})).unwrap_err();
    assert!(err.to_string().contains("can't be rendered"), "{err}");
    assert_eq!(bounce(&e, t), untuned, "the clip plays untuned");
    // An over-long phrase reads with the breath wording.
    vocal::mark_failed_because(&mut e, inputs, "unsupported: a 61.0 s render span: no breath for over 60 s");
    let r = e.execute("vocal.notes", &json!({"clip": c.0})).unwrap();
    assert_eq!(r["messages"][0]["message"], json!(vocal::RENDER_FAILED));
}
