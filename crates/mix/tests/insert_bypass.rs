//! Insert bypass (built-in effects parity, bug 4): a bypassed insert crossfades instead of
//! switching, keeps its latency so the strip's delay compensation stays where it is, and a plugin
//! switched back on starts clean instead of playing what it held when it was bypassed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{add_delay, engine};
use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::MixEngine;
use soundcraft_model::{ChannelFormat, Clip, Insert, ModuleId, Route, Session, SourceAudio, SourceId, TrackId, TrackKind};
use std::sync::Arc;

const BLOCK: usize = 256;
const LEN: usize = BLOCK * 200;

fn noise(len: usize) -> Vec<f32> {
    let mut seed = 0x1234_5678u32;
    (0..len)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * 0.25
        })
        .collect()
}

/// A mono track on the main mix playing `signal`.
fn playback(s: &mut Session, name: &str, signal: &[f32]) -> TrackId {
    let src = SourceId(4000 + s.tracks.len() as u64);
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![signal.to_vec()] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
    let clip = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, name, src, 0, 0, signal.len() as i64));
    tr.mixer.output = Route::Main;
    t
}

fn bypass(s: &mut Session, t: TrackId, m: ModuleId, on: bool) {
    s.track_mut(t).unwrap().mixer.inserts.iter_mut().find(|i| i.id == m).unwrap().bypass = on;
}

fn render(eng: &mut MixEngine, s: &Session, blocks: std::ops::Range<usize>, out: &mut Vec<f32>) {
    let mut buf = vec![vec![0.0f32; BLOCK]; 2];
    for b in blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut buf);
        out.extend_from_slice(&buf[0]);
    }
}

fn session() -> Session {
    let mut s = Session::default();
    s.edit.delay_compensation = true;
    s.edit.values.insert("playback.auto_fade_ms".into(), 0.0);
    s
}

/// Track A runs a 200-sample delay; track B plays the same signal phase-inverted. Bypassing A's
/// delay and switching it back on while playing leaves the two in time: the mix stays silent and
/// the latency stays 200. (Before, bypass dropped the latency at once and the tracks slid apart.)
#[test]
fn bypass_keeps_the_latency_and_the_tracks_in_time() {
    let sig = noise(LEN);
    let mut s = session();
    let a = playback(&mut s, "A", &sig);
    let b = playback(&mut s, "B", &sig);
    s.track_mut(b).unwrap().mixer.phase_invert = true;
    let m = add_delay(&mut s, a, 200);
    let mut eng = engine(&s, BLOCK);
    let mut out = Vec::new();
    render(&mut eng, &s, 0..40, &mut out);
    assert_eq!(eng.latency, 200);
    bypass(&mut s, a, m, true);
    render(&mut eng, &s, 40..100, &mut out);
    assert_eq!(eng.latency, 200, "a bypassed insert keeps its latency");
    bypass(&mut s, a, m, false);
    render(&mut eng, &s, 100..160, &mut out);
    assert_eq!(eng.latency, 200);
    let worst = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(worst < 1e-5, "the tracks never leave each other: {worst}");
}

/// A gain of -40 dB bypassed while playing fades over the crossfade instead of jumping, and back.
#[test]
fn bypass_crossfades() {
    let sig = vec![0.5f32; LEN];
    let mut s = session();
    let a = playback(&mut s, "A", &sig);
    let mut ins = Insert::new("gain");
    ins.params.insert("gain".into(), -40.0);
    let m = s.track_mut(a).unwrap().mixer.add_module(ins, None).unwrap();
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    let mut out = Vec::new();
    render(&mut eng, &s, 0..20, &mut out);
    bypass(&mut s, a, m, true);
    render(&mut eng, &s, 20..40, &mut out);
    bypass(&mut s, a, m, false);
    render(&mut eng, &s, 40..60, &mut out);
    let jump = out.windows(2).skip(10 * BLOCK).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
    assert!(jump < 0.01, "no step: largest jump {jump}");
    let quiet = out[19 * BLOCK];
    let loud = out[39 * BLOCK];
    assert!(quiet.abs() < 0.01 && loud > 0.3, "{quiet} {loud}");
}

/// A delay switched back on after a bypass does not replay the audio it held when it was
/// bypassed: it starts from silence (reset) and is heard once its latency has passed.
#[test]
fn a_plugin_switched_back_on_starts_clean() {
    // A burst, then silence while bypassed, then silence when switched back on.
    let mut sig = vec![0.0f32; LEN];
    for (i, x) in sig.iter_mut().enumerate().take(40 * BLOCK).skip(39 * BLOCK) {
        *x = 0.5 * ((i as f32) * 0.05).sin();
    }
    let mut s = session();
    let a = playback(&mut s, "A", &sig);
    let m = add_delay(&mut s, a, 3000);
    let mut eng = engine(&s, BLOCK);
    let mut out = Vec::new();
    render(&mut eng, &s, 0..40, &mut out);
    // The burst is inside the delay when it is bypassed; the bypass line carries it out instead.
    bypass(&mut s, a, m, true);
    render(&mut eng, &s, 40..80, &mut out);
    bypass(&mut s, a, m, false);
    let at = out.len();
    render(&mut eng, &s, 80..120, &mut out);
    let late = out[at..].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(late < 1e-6, "nothing stale comes back: {late}");
    let heard = out[..at].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(heard > 0.3, "the burst is heard once, through the bypass: {heard}");
}
