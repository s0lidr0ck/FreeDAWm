//! A Locate (or Play/Stop) resets the plugins at the new position: each module takes its
//! automation there first, so a smoothed parameter is seated at the automated value instead of
//! gliding from the value at the old position (`MixEngine::reset_at`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::MixEngine;
use soundcraft_model::{AutoParam, ChannelFormat, Clip, Insert, ModuleId, Route, Session, SourceAudio, SourceId, TrackId, TrackKind};
use std::sync::Arc;

const BLOCK: usize = 512;
const LEN: usize = BLOCK * 24;
/// Where the lane jumps.
const JUMP: i64 = (BLOCK * 8) as i64;
/// Where the transport is located to (inside the jumped-to region).
const AT: i64 = (BLOCK * 16) as i64;

/// A mono sine through `plugin` (with `params`), its `param` automated by `points`.
fn session(plugin: &str, params: &[(&str, f32)], param: &str, points: &[(i64, f32)]) -> (Session, TrackId, ModuleId) {
    let mut s = Session::default();
    let sine: Vec<f32> = (0..LEN).map(|n| 0.3 * (std::f32::consts::TAU * 220.0 * n as f32 / 48_000.0).sin()).collect();
    s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![sine] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Sine"));
    let clip = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, "sine", SourceId(5), 0, 0, LEN as i64));
    tr.mixer.output = Route::Main;
    let mut ins = Insert::new(plugin);
    // A lane drives a stored param (the engine stores them all when it adds a module).
    ins.params.insert(param.into(), 0.5);
    for (k, v) in params {
        ins.params.insert((*k).into(), *v);
    }
    let m = tr.mixer.add_module(ins, None).unwrap();
    let lane = tr.lane_mut(&AutoParam::Plugin { module: m, param: param.into() });
    for (at, v) in points {
        lane.set_point(*at, *v);
    }
    (s, t, m)
}

fn block(eng: &mut MixEngine, s: &Session, pos: i64) -> Vec<f32> {
    let mut buf = vec![vec![0.0f32; BLOCK]; 2];
    eng.render(s, pos, BLOCK, &mut buf);
    buf.swap_remove(0)
}

/// The first block after locating to `AT`, having played the start first; `seat` resets with the
/// new position when true (`reset_at`), with the old plain `reset` when false.
fn first_block_after_locate(s: &Session, seat: bool) -> Vec<f32> {
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    for b in 0..4 {
        block(&mut eng, s, (b * BLOCK) as i64);
    }
    if seat {
        eng.reset_at(s, AT);
    } else {
        eng.reset();
    }
    block(&mut eng, s, AT)
}

/// The first two blocks after locating to `AT` (seated), back to back.
fn two_blocks_after_locate(s: &Session) -> Vec<f32> {
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    for b in 0..4 {
        block(&mut eng, s, (b * BLOCK) as i64);
    }
    eng.reset_at(s, AT);
    let mut out = block(&mut eng, s, AT);
    out.extend(block(&mut eng, s, AT + BLOCK as i64));
    out
}

fn check(plugin: &str, params: &[(&str, f32)], param: &str) {
    let (jumps, ..) = session(plugin, params, param, &[(0, 0.2), (JUMP - 1, 0.2), (JUMP, 0.8), (LEN as i64, 0.8)]);
    let (steady, ..) = session(plugin, params, param, &[(0, 0.8), (LEN as i64, 0.8)]);
    let want = first_block_after_locate(&steady, true);
    let got = first_block_after_locate(&jumps, true);
    assert!(got.iter().all(|x| x.is_finite()));
    assert!(want.iter().any(|x| x.abs() > 1e-3), "{plugin}: the reference block has signal");
    let worst = got.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst <= 1e-6, "{plugin}: the first block after Locate is at the automated 0.8 from its first sample (worst {worst})");
    // Without the new position's values first, the plugin seats at 0.2 and glides.
    let glide = first_block_after_locate(&jumps, false);
    let worst = glide.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst > 1e-3, "{plugin}: a plain reset glides from the old value ({worst})");
}

#[test]
fn locate_seats_smoothed_params_at_the_automated_value() {
    check("test_link", &[], "value");
    check("fs_transient", &[], "output");
    check("fs_sat", &[("autoGain", 0.0)], "output");
}

/// An offline render (bounce) starting inside the 0.8 region is seated there from its first
/// sample, as a Locate is: its first block is the one a live engine plays after locating there.
fn check_bounce(plugin: &str, params: &[(&str, f32)], param: &str) {
    let (jumps, ..) = session(plugin, params, param, &[(0, 0.2), (JUMP - 1, 0.2), (JUMP, 0.8), (LEN as i64, 0.8)]);
    let (steady, ..) = session(plugin, params, param, &[(0, 0.8), (LEN as i64, 0.8)]);
    let got = soundcraft_mix::render_range(&jumps, soundcraft_time::Range::new(AT, AT + BLOCK as i64), BLOCK);
    // A plugin with latency (fs_transient's 3 ms lookahead): the bounce has the delay
    // compensation removed, the live playback has not, so the bounce's sample i is the live
    // sample i + latency. Two live blocks, so the whole bounce block is compared.
    let mut probe = soundcraft_dsp::create(plugin).unwrap();
    probe.prepare(48_000.0, BLOCK, 1);
    let lat = probe.latency();
    assert!(lat < BLOCK, "{plugin}: latency {lat}");
    let live = two_blocks_after_locate(&steady);
    let want = &live[lat..lat + BLOCK];
    assert!(want.iter().any(|x| x.abs() > 1e-3), "{plugin}: the reference has signal");
    assert_eq!(got[0].len(), BLOCK);
    let worst = got[0].iter().zip(want).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst <= 1e-6, "{plugin}: the bounce starts at the automated 0.8 (worst {worst})");
}

#[test]
fn a_bounce_starts_seated_at_the_automated_value() {
    // the probe glides its Value gain from wherever it was unless a reset seats it
    check_bounce("test_link", &[], "value");
    check_bounce("fs_transient", &[], "output");
    check_bounce("fs_sat", &[("autoGain", 0.0)], "output");
}
