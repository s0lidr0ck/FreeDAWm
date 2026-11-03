//! Low-latency monitoring (LLM, audio I/O Task 7): latent inserts on the live path are bypassed,
//! the compensation the live path would wait for is dropped, and turning it off gives back the
//! reference output.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{add_delay, engine};
use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::{LlmScope, MixEngine, llm_scope, render_range};
use soundcraft_model::{ChannelFormat, Clip, InputMonitor, OutputPath, Route, SendSlot, Session, SourceAudio, SourceId, TrackId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;

const BLOCK: usize = 256;
const LEN: usize = BLOCK * 64;
const LLM: &str = "options.low_latency_monitoring";

fn session(llm: bool) -> Session {
    let mut s = Session::default();
    s.edit.delay_compensation = true;
    s.edit.set_flag(LLM, llm);
    s
}

/// A mono playback track on the main mix playing `signal`.
fn playback(s: &mut Session, name: &str, signal: Vec<f32>) -> TrackId {
    let src = SourceId(3000 + s.tracks.len() as u64);
    let len = signal.len() as i64;
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![signal] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
    let clip = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, name, src, 0, 0, len));
    tr.mixer.output = Route::Main;
    t
}

fn impulse(at: usize, amp: f32) -> Vec<f32> {
    let mut v = vec![0.0; LEN];
    v[at] = amp;
    v
}

/// A record-armed mono track (Auto input monitoring) hearing device channel 1, on the main mix.
fn armed(s: &mut Session) -> TrackId {
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let tr = s.track_mut(t).unwrap();
    tr.mixer.record_arm = true;
    tr.mixer.output = Route::Main;
    t
}

/// Renders blocks `range` while recording, the device input at block `b` being `input(b)`;
/// returns the main mix's left channel.
fn run(eng: &mut MixEngine, s: &Session, blocks: std::ops::Range<usize>, input: impl Fn(usize) -> Vec<f32>) -> Vec<f32> {
    eng.recording = true;
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    let mut left = Vec::new();
    for b in blocks {
        eng.input = vec![input(b)];
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut out);
        left.extend_from_slice(&out[0]);
    }
    left
}

/// The device input: one impulse of `amp` at the very start.
fn first_impulse(amp: f32) -> impl Fn(usize) -> Vec<f32> {
    move |b| {
        let mut v = vec![0.0; BLOCK];
        if b == 0 {
            v[0] = amp;
        }
        v
    }
}

fn hits(x: &[f32]) -> Vec<usize> {
    x.iter().enumerate().filter(|(_, v)| v.abs() > 1e-4).map(|(i, _)| i).collect()
}

#[test]
fn llm_bypasses_latent_inserts_on_live_tracks_only() {
    for on in [false, true] {
        let mut s = session(on);
        let vox = armed(&mut s);
        add_delay(&mut s, vox, 64);
        let play = playback(&mut s, "Gtr", impulse(300, 0.5));
        add_delay(&mut s, play, 64);
        let mut eng = engine(&s, BLOCK);
        let out = run(&mut eng, &s, 0..4, first_impulse(0.8));
        if on {
            assert_eq!(hits(&out), vec![0, 364], "the input reaches main at once; the playback keeps its plugin");
            assert_eq!(eng.meters[&vox].llm_bypassed, 1);
        } else {
            assert_eq!(hits(&out), vec![64, 364]);
            assert_eq!(eng.meters[&vox].llm_bypassed, 0);
        }
        assert_eq!(eng.meters[&play].llm_bypassed, 0);
        assert_eq!(eng.latency, 64);
    }
}

#[test]
fn llm_drops_the_compensation_the_live_path_would_wait_for() {
    for on in [false, true] {
        let mut s = session(on);
        let vox = armed(&mut s);
        add_delay(&mut s, vox, 128);
        let a = playback(&mut s, "A", impulse(300, 0.5));
        add_delay(&mut s, a, 64);
        playback(&mut s, "B", impulse(600, 0.3));
        let mut eng = engine(&s, BLOCK);
        let out = run(&mut eng, &s, 0..5, first_impulse(0.8));
        if on {
            assert_eq!(hits(&out), vec![0, 364, 664], "B lines up with A, not with the live track");
            assert_eq!(eng.latency, 64);
        } else {
            assert_eq!(hits(&out), vec![128, 428, 728]);
            assert_eq!(eng.latency, 128);
        }
    }
}

#[test]
fn llm_reaches_feeding_busses_and_master_not_send_returns() {
    let mut s = session(true);
    let vox = armed(&mut s);
    let b1 = s.add_bus("Bus 1", ChannelFormat::Stereo);
    let b2 = s.add_bus("Verb", ChannelFormat::Stereo);
    let aux_a = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Vox Grp"));
    s.track_mut(aux_a).unwrap().mixer.input = Route::Bus(b1);
    add_delay(&mut s, aux_a, 64);
    let aux_b = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb Ret"));
    s.track_mut(aux_b).unwrap().mixer.input = Route::Bus(b2);
    add_delay(&mut s, aux_b, 64);
    let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
    add_delay(&mut s, master, 32);
    let tr = s.track_mut(vox).unwrap();
    tr.mixer.output = Route::Bus(b1);
    let mut send = SendSlot::new(Route::Bus(b2));
    send.level_db = 0.0;
    tr.mixer.sends = vec![Some(send)];
    let mut eng = engine(&s, BLOCK);
    let out = run(&mut eng, &s, 0..4, first_impulse(0.8));
    assert_eq!(eng.meters[&aux_a].llm_bypassed, 1, "the bus the live track feeds");
    assert_eq!(eng.meters[&aux_b].llm_bypassed, 0, "a send's effect return is left alone");
    assert_eq!(eng.meters[&master].llm_bypassed, 1, "the master's look-ahead");
    assert_eq!(hits(&out), vec![0, 64], "dry through the group at once; the reverb return keeps its latency");
    assert_eq!(scope_of(&s), vec![true, true, false, true]);
}

#[test]
fn off_input_monitoring_is_still_in_scope_when_armed() {
    let mut s = session(true);
    let vox = armed(&mut s);
    s.track_mut(vox).unwrap().mixer.input_monitor = InputMonitor::Off;
    add_delay(&mut s, vox, 64);
    let mon = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Talkback"));
    s.track_mut(mon).unwrap().mixer.input_monitor = InputMonitor::On;
    add_delay(&mut s, mon, 64);
    let idle = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Idle"));
    add_delay(&mut s, idle, 64);
    assert_eq!(scope_of(&s), vec![true, true, false]);
    let mut eng = engine(&s, BLOCK);
    run(&mut eng, &s, 0..2, first_impulse(0.0));
    assert_eq!(eng.meters[&vox].llm_bypassed, 1, "direct monitoring: armed is still live");
    assert_eq!(eng.meters[&mon].llm_bypassed, 1);
    assert_eq!(eng.meters[&idle].llm_bypassed, 0);
}

#[test]
fn nothing_live_leaves_the_master_alone() {
    let mut s = session(true);
    playback(&mut s, "Gtr", impulse(300, 0.5));
    let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
    add_delay(&mut s, master, 32);
    assert_eq!(scope_of(&s), vec![false, false]);
    let mut eng = engine(&s, BLOCK);
    let out = run(&mut eng, &s, 0..3, first_impulse(0.0));
    assert_eq!(hits(&out), vec![332]);
    assert_eq!(eng.meters[&master].llm_bypassed, 0);
}

/// A slow sine on the device input (continuous across blocks).
fn sine(b: usize) -> Vec<f32> {
    (0..BLOCK).map(|i| 0.5 * ((b * BLOCK + i) as f32 * 2.0 * std::f32::consts::PI * 200.0 / 48_000.0).sin()).collect()
}

fn max_step(x: &[f32]) -> f32 {
    x.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
}

fn toggle_session() -> (Session, TrackId) {
    let mut s = session(false);
    let vox = armed(&mut s);
    add_delay(&mut s, vox, 128);
    let a = playback(&mut s, "A", (0..LEN).map(|i| 0.3 * (i as f32 * 0.011).sin()).collect());
    add_delay(&mut s, a, 64);
    playback(&mut s, "B", (0..LEN).map(|i| 0.2 * (i as f32 * 0.007).cos()).collect());
    (s, vox)
}

#[test]
fn llm_off_restores_the_reference_output() {
    let (s, vox) = toggle_session();
    let mut reference = engine(&s, BLOCK);
    let want = run(&mut reference, &s, 0..30, sine);

    let mut on = s.clone();
    on.edit.set_flag(LLM, true);
    let mut eng = engine(&s, BLOCK);
    let mut got = run(&mut eng, &s, 0..8, sine);
    got.extend(run(&mut eng, &on, 8..16, sine));
    assert_eq!(eng.meters[&vox].llm_bypassed, 1);
    assert_eq!(eng.latency, 64);
    got.extend(run(&mut eng, &s, 16..30, sine));
    assert_eq!(eng.meters[&vox].llm_bypassed, 0, "zero when LLM is off");
    assert_eq!(eng.latency, 128);

    let tail = 24 * BLOCK;
    let worst = got[tail..].iter().zip(&want[tail..]).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
    assert!(worst < 1e-6, "once the crossfades are over the output is the reference's: {worst}");
    assert_eq!(&got[..8 * BLOCK], &want[..8 * BLOCK]);
    // No click switching either way: no sample step beyond what the music itself has.
    // (After the start, where B's cosine sets in at full level.)
    let settled = 2 * BLOCK;
    let limit = 1.5 * max_step(&want[settled..]);
    assert!(max_step(&got[settled..]) < limit, "{} vs {}", max_step(&got[settled..]), limit);
}

#[test]
fn arming_during_playback_realigns_without_a_click() {
    // Direct monitoring (Off): arming changes nothing on the track itself, but the other tracks
    // stop waiting for its 128-sample plugin, then wait again once it is disarmed.
    let (mut s, vox) = toggle_session();
    s.edit.set_flag(LLM, true);
    s.track_mut(vox).unwrap().mixer.record_arm = false;
    s.track_mut(vox).unwrap().mixer.input_monitor = InputMonitor::Off;
    let mut armed_s = s.clone();
    armed_s.track_mut(vox).unwrap().mixer.record_arm = true;
    let mut eng = engine(&s, BLOCK);
    let mut got = run(&mut eng, &s, 0..6, sine);
    assert_eq!(eng.latency, 128);
    got.extend(run(&mut eng, &armed_s, 6..14, sine));
    assert_eq!(eng.meters[&vox].llm_bypassed, 1);
    assert_eq!(eng.latency, 64);
    got.extend(run(&mut eng, &s, 14..24, sine));
    assert_eq!(eng.latency, 128);
    let mut reference = engine(&s, BLOCK);
    let want = run(&mut reference, &s, 0..24, sine);
    // (After the start, where B's cosine sets in at full level.)
    let settled = 2 * BLOCK;
    let limit = 1.5 * max_step(&want[settled..]);
    assert!(max_step(&got[settled..]) < limit, "{} vs {}", max_step(&got[settled..]), limit);
    let tail = 20 * BLOCK;
    let worst = got[tail..].iter().zip(&want[tail..]).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
    assert!(worst < 1e-6, "{worst}");
}

#[test]
fn hardware_outputs_drop_the_bypassed_master_latency() {
    let mut s = session(true);
    s.outputs.push(OutputPath { name: "Mon 5-6".into(), first_channel: 4, format: ChannelFormat::Stereo });
    armed(&mut s);
    playback(&mut s, "To Main", impulse(300, 0.5));
    let mon = playback(&mut s, "To Mon", impulse(300, 0.5));
    s.track_mut(mon).unwrap().mixer.output = Route::Hardware("Mon 5-6".into());
    let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
    add_delay(&mut s, master, 200);
    for on in [false, true] {
        s.edit.set_flag(LLM, on);
        let mut eng = engine(&s, BLOCK);
        eng.recording = true;
        let mut out = vec![vec![0.0f32; BLOCK]; 2];
        let (mut main, mut hw) = (Vec::new(), Vec::new());
        for b in 0..4 {
            eng.input = vec![vec![0.0; BLOCK]];
            eng.render(&s, (b * BLOCK) as i64, BLOCK, &mut out);
            main.extend_from_slice(&out[0]);
            hw.extend_from_slice(&eng.hardware_out()[1][0]);
        }
        let at = if on { 300 } else { 500 };
        assert_eq!(hits(&main), vec![at], "llm {on}");
        assert_eq!(hits(&hw), vec![at], "the hardware output lines up with main (llm {on})");
    }
}

#[test]
fn bounces_ignore_llm() {
    let (mut s, vox) = toggle_session();
    // The armed track plays a take in a bounce (no input there).
    let src = SourceId(3999);
    let take: Vec<f32> = (0..LEN).map(|i| 0.25 * (i as f32 * 0.013).sin()).collect();
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![take] })));
    let clip = s.new_clip_id();
    s.track_mut(vox).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "take", src, 0, 0, LEN as i64));
    let mut on = s.clone();
    on.edit.set_flag(LLM, true);
    assert_eq!(render_range(&on, Range::new(0, 4096), BLOCK), render_range(&s, Range::new(0, 4096), BLOCK));
}

#[test]
fn llm_scope_does_not_allocate() {
    let (mut s, vox) = toggle_session();
    s.edit.set_flag(LLM, true);
    s.outputs.push(OutputPath { name: "Mon 5-6".into(), first_channel: 4, format: ChannelFormat::Stereo });
    let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
    add_delay(&mut s, master, 32);
    let mut scope = LlmScope::default();
    scope.reserve(&s);
    let id = (scope.tracks.as_ptr() as usize, scope.tracks.capacity());
    for _ in 0..10 {
        llm_scope(&s, &mut scope);
        assert_eq!((scope.tracks.as_ptr() as usize, scope.tracks.capacity()), id);
    }
    let mut off = s.clone();
    off.edit.set_flag(LLM, false);
    // LLM on in block 0, then toggled every 3 blocks from block 1: no warm-up.
    let pick = |b: usize| if b > 0 && ((b - 1) / 3).is_multiple_of(2) { &off } else { &s };
    let mut eng = engine(&s, BLOCK);
    run(&mut eng, pick(0), 0..1, sine);
    let module = s.track(vox).unwrap().mixer.inserts[0].id;
    let serial = eng.instance_serial(vox, module);
    assert!(serial.is_some());
    // Strip buffers include each strip's plugin list and every instance's serial: a re-sync or a
    // re-created slot shows.
    let ids = |e: &MixEngine| (s.tracks.iter().map(|t| e.strip_buffers(t.id)).collect::<Vec<_>>(), e.engine_buffers());
    let before = ids(&eng);
    assert!(before.1.len() > 4, "the hardware output's delay line is checked too");
    for b in 1..60 {
        run(&mut eng, pick(b), b..b + 1, sine);
        assert_eq!(ids(&eng), before, "block {b}");
        assert_eq!(eng.instance_serial(vox, module), serial);
    }
    let _ = vox;
}

fn scope_of(s: &Session) -> Vec<bool> {
    let mut scope = LlmScope::default();
    scope.reserve(s);
    llm_scope(s, &mut scope);
    scope.tracks
}

#[test]
fn inactive_tracks_are_not_on_the_live_path() {
    let mut s = session(true);
    let vox = armed(&mut s);
    let b = s.add_bus("Bus 1", ChannelFormat::Stereo);
    s.track_mut(vox).unwrap().mixer.output = Route::Bus(b);
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Grp"));
    s.track_mut(aux).unwrap().mixer.input = Route::Bus(b);
    s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
    assert_eq!(scope_of(&s), vec![true, true, true]);
    s.track_mut(aux).unwrap().inactive = true;
    assert_eq!(scope_of(&s), vec![true, false, true]);
    s.track_mut(vox).unwrap().inactive = true;
    assert_eq!(scope_of(&s), vec![false, false, false], "nothing live: the master is left alone too");
}

#[test]
fn a_playback_track_on_a_live_bus_stays_aligned_with_the_others() {
    // P plays into the group the armed track feeds (on the live path), Q into the main mix; an
    // off-path return with a 100-sample plugin sets the aux compensation.
    let mut s = session(true);
    let vox = armed(&mut s);
    let b1 = s.add_bus("Bus 1", ChannelFormat::Stereo);
    let b2 = s.add_bus("Verb", ChannelFormat::Stereo);
    s.track_mut(vox).unwrap().mixer.output = Route::Bus(b1);
    let p = playback(&mut s, "P", impulse(300, 0.5));
    s.track_mut(p).unwrap().mixer.output = Route::Bus(b1);
    playback(&mut s, "Q", impulse(1000, 0.5));
    let grp = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Grp"));
    s.track_mut(grp).unwrap().mixer.input = Route::Bus(b1);
    let ret = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb Ret"));
    s.track_mut(ret).unwrap().mixer.input = Route::Bus(b2);
    add_delay(&mut s, ret, 100);
    assert_eq!(scope_of(&s), vec![true, false, false, true, false]);
    let mut eng = engine(&s, BLOCK);
    let out = run(&mut eng, &s, 0..6, first_impulse(0.0));
    assert_eq!(hits(&out), vec![400, 1100], "P waits for the returns as Q does");
}

#[test]
fn a_sync_while_llm_is_on_reserves_for_llm_off() {
    // At 8 kHz the lines' headroom (0.25 s) is 2000 samples: less than the live track's 4000.
    let mut s = session(true);
    let vox = armed(&mut s);
    add_delay(&mut s, vox, 4000);
    playback(&mut s, "B", (0..LEN).map(|i| 0.2 * (i as f32 * 0.007).cos()).collect());
    let mut eng = common::engine_at(&s, BLOCK, 8_000.0);
    run(&mut eng, &s, 0..2, sine);
    // An edit while tracking re-syncs with LLM on.
    eng.sync(&s);
    run(&mut eng, &s, 2..3, sine);
    let ids = |e: &MixEngine| s.tracks.iter().map(|t| e.delay_buffers(t.id)).collect::<Vec<_>>();
    let before = ids(&eng);
    let mut off = s.clone();
    off.edit.set_flag(LLM, false);
    for b in 3..12 {
        run(&mut eng, &off, b..b + 1, sine);
        assert_eq!(ids(&eng), before, "block {b}: LLM off fits the lines reserved with it on");
    }
    assert_eq!(eng.latency, 4000);
}

#[test]
fn a_track_feeding_a_live_group_through_an_off_path_aux_stays_aligned() {
    // T -> Bus X -> aux A (off the path) -> Bus Y -> group G (on the path, the armed track feeds
    // it). An off-path return with a 100-sample plugin sets the aux compensation; Q plays to main.
    let mut s = session(true);
    let vox = armed(&mut s);
    let x = s.add_bus("X", ChannelFormat::Stereo);
    let y = s.add_bus("Y", ChannelFormat::Stereo);
    let z = s.add_bus("Verb", ChannelFormat::Stereo);
    s.track_mut(vox).unwrap().mixer.output = Route::Bus(y);
    let tt = playback(&mut s, "T", impulse(300, 0.5));
    s.track_mut(tt).unwrap().mixer.output = Route::Bus(x);
    playback(&mut s, "Q", impulse(1000, 0.5));
    let a = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("A"));
    s.track_mut(a).unwrap().mixer.input = Route::Bus(x);
    s.track_mut(a).unwrap().mixer.output = Route::Bus(y);
    let g = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("G"));
    s.track_mut(g).unwrap().mixer.input = Route::Bus(y);
    let ret = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb Ret"));
    s.track_mut(ret).unwrap().mixer.input = Route::Bus(z);
    add_delay(&mut s, ret, 100);
    assert_eq!(scope_of(&s), vec![true, false, false, false, true, false]);
    let mut eng = engine(&s, BLOCK);
    let out = run(&mut eng, &s, 0..6, first_impulse(0.0));
    assert_eq!(hits(&out), vec![400, 1100], "T waits for the returns as Q does");
}
