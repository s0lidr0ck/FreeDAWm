//! An insert's wet/dry mix (built-in effects parity, "wet/dry mix on every effect"): the module's
//! output is blended with its input, the input delayed by the module's latency so a latent
//! effect at half mix does not comb, and a change glides instead of stepping.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{add_delay, engine};
use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::MixEngine;
use soundcraft_model::{ChannelFormat, Clip, Insert, ModuleId, Route, Session, SourceAudio, SourceId, TrackId, TrackKind};
use std::sync::Arc;

const BLOCK: usize = 256;
const LEN: usize = BLOCK * 170;

fn noise(len: usize) -> Vec<f32> {
    let mut seed = 0x8765_4321u32;
    (0..len)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * 0.25
        })
        .collect()
}

fn playback(s: &mut Session, name: &str, signal: &[f32]) -> TrackId {
    let src = SourceId(5000 + s.tracks.len() as u64);
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![signal.to_vec()] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
    let clip = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, name, src, 0, 0, signal.len() as i64));
    tr.mixer.output = Route::Main;
    t
}

fn set_mix(s: &mut Session, t: TrackId, m: ModuleId, mix: f32) {
    s.track_mut(t).unwrap().mixer.inserts.iter_mut().find(|i| i.id == m).unwrap().mix = mix;
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

/// A silencing Gain at half mix gives half the input; at no mix the input itself; back at full
/// mix silence. Each change glides over one block, never steps.
#[test]
fn half_mix_blends_the_output_with_the_input() {
    let sig = noise(LEN);
    let mut s = session();
    let a = playback(&mut s, "A", &sig);
    let mut g = Insert::new("gain");
    g.params.insert("gain".into(), -200.0);
    let m = s.track_mut(a).unwrap().mixer.add_module(g, None).unwrap();
    let mut eng = engine(&s, BLOCK);
    let mut out = Vec::new();
    // the gain first glides down from unity over its own 20 ms smoothing
    render(&mut eng, &s, 0..100, &mut out);
    assert!(out[BLOCK * 80..].iter().all(|v| v.abs() < 1e-6), "full wet: the gain's silence");
    set_mix(&mut s, a, m, 0.5);
    render(&mut eng, &s, 100..120, &mut out);
    set_mix(&mut s, a, m, 0.0);
    render(&mut eng, &s, 120..140, &mut out);
    set_mix(&mut s, a, m, 1.0);
    render(&mut eng, &s, 140..160, &mut out);
    // the mono track's pan law, read where the signal is loud
    let n0 = (BLOCK * 110..BLOCK * 120).max_by(|x, y| sig[*x].abs().total_cmp(&sig[*y].abs())).unwrap();
    let pan = out[n0] / (0.5 * sig[n0]);
    for n in BLOCK * 101..BLOCK * 120 {
        assert!((out[n] - pan * 0.5 * sig[n]).abs() < 1e-4, "half: {n}");
    }
    for n in BLOCK * 121..BLOCK * 140 {
        assert!((out[n] - pan * sig[n]).abs() < 1e-4, "dry: {n}");
    }
    assert!(out[BLOCK * 141..BLOCK * 160].iter().all(|v| v.abs() < 1e-6), "wet again");
    // the change to half glides across its block: no step bigger than the signal's own
    let ramp = &out[BLOCK * 100..BLOCK * 101];
    assert!(ramp.iter().zip(&sig[BLOCK * 100..]).all(|(o, x)| o.abs() <= 0.51 * pan.abs() * x.abs() + 1e-6));
}

/// A 200-sample delay at half mix, against the same signal phase-inverted on another track:
/// the input is delayed by the module's latency too, so the two still cancel.
#[test]
fn a_latent_insert_at_half_mix_does_not_comb() {
    let sig = noise(LEN);
    let mut s = session();
    let a = playback(&mut s, "A", &sig);
    let b = playback(&mut s, "B", &sig);
    s.track_mut(b).unwrap().mixer.phase_invert = true;
    let m = add_delay(&mut s, a, 200);
    set_mix(&mut s, a, m, 0.5);
    let mut eng = engine(&s, BLOCK);
    let mut out = Vec::new();
    render(&mut eng, &s, 0..60, &mut out);
    assert_eq!(eng.latency, 200);
    let rest = &out[BLOCK * 4..];
    assert!(rest.iter().all(|v| v.abs() < 1e-5), "{}", rest.iter().map(|v| v.abs()).fold(0.0, f32::max));
}

/// The mix's lane (`plugin:m<id>:@mix`) drives it as it plays, read where the module's input is;
/// the stored mix is left alone. A silencing Gain laned from full wet to dry halfway lets the
/// input through from there.
#[test]
fn the_mix_lane_moves_the_blend() {
    let sig = noise(LEN);
    let mut s = session();
    let a = playback(&mut s, "A", &sig);
    let mut g = Insert::new("gain");
    g.params.insert("gain".into(), -200.0);
    let m = s.track_mut(a).unwrap().mixer.add_module(g, None).unwrap();
    let mut lane = soundcraft_model::AutomationLane::new(soundcraft_model::AutoParam::wet_mix(m));
    lane.set_point(0, 1.0);
    lane.set_point((BLOCK * 100) as i64, 1.0);
    lane.set_point((BLOCK * 100 + 1) as i64, 0.0);
    s.track_mut(a).unwrap().automation.push(lane);
    let mut eng = engine(&s, BLOCK);
    let mut out = Vec::new();
    render(&mut eng, &s, 0..140, &mut out);
    assert!(out[BLOCK * 80..BLOCK * 100].iter().all(|v| v.abs() < 1e-6), "full wet by the lane");
    let n0 = (BLOCK * 110..BLOCK * 140).max_by(|x, y| sig[*x].abs().total_cmp(&sig[*y].abs())).unwrap();
    assert!(out[n0].abs() > 0.5 * sig[n0].abs(), "dry by the lane: {} for {}", out[n0], sig[n0]);
    assert_eq!(s.track(a).unwrap().mixer.module(m).unwrap().mix, 1.0, "the stored mix is left alone");
}
