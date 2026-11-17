//! Modulators (built-in effects parity, "modulators for any parameter"): an LFO on a track
//! moves a plug-in parameter around its own value; taken off, the parameter returns to it. An
//! envelope follower moves it with the track's level.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::MixEngine;
use soundcraft_model::{
    ChannelFormat, Clip, Insert, ModTarget, Modulator, ModulatorKind, ModuleId, Route, Session, SourceAudio, SourceId, TrackId, TrackKind,
};
use std::sync::Arc;

const BLOCK: usize = 256;
const RATE: usize = 48_000;

fn tone(len: usize) -> Vec<f32> {
    (0..len).map(|n| 0.5 * (std::f32::consts::TAU * 440.0 * n as f32 / RATE as f32).sin()).collect()
}

fn session(signal: &[f32]) -> (Session, TrackId, ModuleId) {
    let mut s = Session::default();
    s.edit.values.insert("playback.auto_fade_ms".into(), 0.0);
    s.pool.insert(SourceId(9), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: RATE as u32, channels: vec![signal.to_vec()] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Tone"));
    let clip = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, "tone", SourceId(9), 0, 0, signal.len() as i64));
    tr.mixer.output = Route::Main;
    let mut g = Insert::new("gain");
    g.params.insert("gain".into(), -12.0);
    let m = tr.mixer.add_module(g, None).unwrap();
    (s, t, m)
}

fn render(eng: &mut MixEngine, s: &Session, blocks: std::ops::Range<usize>, out: &mut Vec<f32>) {
    let mut buf = vec![vec![0.0f32; BLOCK]; 2];
    for b in blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut buf);
        out.extend_from_slice(&buf[0]);
    }
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

/// A 1 Hz square LFO on Gain's gain: louder for the first half of each second than the second;
/// removed, the gain is back where it was set.
#[test]
fn an_lfo_moves_a_parameter_around_its_value_and_lets_it_go() {
    let sig = tone(RATE * 4);
    let (mut s, t, g) = session(&sig);
    let mut lfo = Modulator::new(1, ModulatorKind::Lfo);
    lfo.shape = 3;
    lfo.rate_hz = 1.0;
    lfo.targets.push(ModTarget { module: g, param: "gain".into(), depth: 0.2 });
    let mut eng = MixEngine::new(RATE as f32, BLOCK);
    let mut plain = Vec::new();
    render(&mut eng, &s, 0..200, &mut plain);
    let set = rms(&plain[RATE / 2..RATE * 9 / 10]);
    s.track_mut(t).unwrap().modulators.push(lfo);
    let blocks = RATE / BLOCK;
    let mut out = Vec::new();
    render(&mut eng, &s, 200..200 + blocks, &mut out);
    let (up, down) = (rms(&out[RATE / 10..RATE * 4 / 10]), rms(&out[RATE * 6 / 10..RATE * 9 / 10]));
    assert!(up > 1.5 * set && down < set / 1.5, "set {set}, up {up}, down {down}");
    // off again: back to the set value
    s.track_mut(t).unwrap().modulators.clear();
    let mut back = Vec::new();
    render(&mut eng, &s, 200 + blocks..200 + 2 * blocks, &mut back);
    let after = rms(&back[RATE / 2..RATE * 9 / 10]);
    assert!((after / set - 1.0).abs() < 0.01, "set {set}, after {after}");
}

/// An envelope follower opening Gain as the track gets louder: silence keeps it at its value, a
/// loud passage turns it up.
#[test]
fn an_envelope_follows_the_tracks_level() {
    let mut sig = vec![0.0f32; RATE];
    sig.extend(tone(RATE));
    let (mut s, t, g) = session(&sig);
    let mut env = Modulator::new(1, ModulatorKind::Envelope);
    env.targets.push(ModTarget { module: g, param: "gain".into(), depth: 0.3 });
    s.track_mut(t).unwrap().modulators.push(env);
    let mut eng = MixEngine::new(RATE as f32, BLOCK);
    let mut out = Vec::new();
    render(&mut eng, &s, 0..(2 * RATE / BLOCK), &mut out);
    let (plain_s, _, _) = session(&sig);
    let mut eng2 = MixEngine::new(RATE as f32, BLOCK);
    let mut plain = Vec::new();
    render(&mut eng2, &plain_s, 0..(2 * RATE / BLOCK), &mut plain);
    let loud = RATE + RATE / 2..RATE * 2 - 1000;
    assert!(rms(&out[loud.clone()]) > 1.5 * rms(&plain[loud]), "the envelope turned it up");
    assert!(out[..RATE].iter().all(|v| v.abs() < 1e-6), "silence stays silent");
}
