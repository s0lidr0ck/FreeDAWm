//! Named inputs and hardware outputs in the live mix (audio I/O Task 5).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::{MixEngine, render_range};
use soundcraft_model::{ChannelFormat, Clip, InputMonitor, InputPath, OutputPath, Route, Session, SourceAudio, SourceId, TrackId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;

const BLOCK: usize = 256;
const LEN: usize = BLOCK * 200;
const G3: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// A track playing DC `level` (in its format) for `LEN` samples, routed to `output`.
fn dc_track(s: &mut Session, level: f32, fmt: ChannelFormat, name: &str, output: Route) -> TrackId {
    let src = SourceId(1000 + s.tracks.len() as u64);
    let channels = vec![vec![level; LEN]; fmt.channels()];
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels })));
    let t = s.add_track(TrackKind::Audio, fmt, Some(name));
    let clip = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, name, src, 0, 0, LEN as i64));
    tr.mixer.output = output;
    t
}

/// A monitored track (input monitoring On) on hardware input `input`, routed to the main mix.
fn monitored(s: &mut Session, fmt: ChannelFormat, input: &str) -> TrackId {
    let t = s.add_track(TrackKind::Audio, fmt, Some("Mic"));
    let tr = s.track_mut(t).unwrap();
    tr.mixer.input = Route::Hardware(input.into());
    tr.mixer.input_monitor = InputMonitor::On;
    tr.mixer.output = Route::Main;
    t
}

/// Device input with `n` channels, channel `c` at DC `0.1 * (c + 1)`.
fn device_input(n: usize) -> Vec<Vec<f32>> {
    (0..n).map(|c| vec![0.1 * (c + 1) as f32; BLOCK]).collect()
}

fn render_blocks(eng: &mut MixEngine, s: &Session, blocks: usize) -> Vec<Vec<f32>> {
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    for b in 0..blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut out);
    }
    out
}

fn silent(ch: &[Vec<f32>]) -> bool {
    ch.iter().all(|c| c.iter().all(|x| *x == 0.0))
}

fn mon_out(s: &mut Session) {
    s.outputs.push(OutputPath { name: "Mon 5-6".into(), first_channel: 4, format: ChannelFormat::Stereo });
}

#[test]
fn a_named_input_feeds_its_channels() {
    let mut s = Session::default();
    s.inputs.push(InputPath { name: "Vox".into(), first_channel: 4, format: ChannelFormat::Mono });
    s.inputs.push(InputPath { name: "Keys".into(), first_channel: 6, format: ChannelFormat::Stereo });
    let keys = monitored(&mut s, ChannelFormat::Stereo, "keys");
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    eng.input = device_input(8);
    eng.monitor_only = true;
    let out = render_blocks(&mut eng, &s, 2);
    let (l, r) = (out[0][100], out[1][100]);
    assert!(l > 0.1 && r > 0.1, "{l} {r}");
    assert!((l / r - 0.7 / 0.8).abs() < 1e-4, "Keys hears device channels 7 and 8 (0-based 6, 7): {l} {r}");

    // A mono track on a mono input hears that channel; a stereo track on it hears it twice.
    s.track_mut(keys).unwrap().mixer.input = Route::Hardware("Vox".into());
    let out = render_blocks(&mut eng, &s, 2);
    assert!(out[0][100] > 0.1 && (out[0][100] - out[1][100]).abs() < 1e-6, "{} {}", out[0][100], out[1][100]);
    let mut m = Session { inputs: s.inputs.clone(), ..Default::default() };
    monitored(&mut m, ChannelFormat::Mono, "Vox");
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    eng.input = device_input(8);
    eng.monitor_only = true;
    let out = render_blocks(&mut eng, &m, 2);
    assert!((out[0][100] - 0.5 * G3).abs() < 2e-3, "Vox is device channel 5 (0.5): {}", out[0][100]);
}

#[test]
fn inputs_and_outputs_beyond_the_device_are_silent_not_wrapped() {
    // An X32 session opened on a 2-channel device.
    let mut s = Session::default();
    s.inputs.push(InputPath { name: "X32 17".into(), first_channel: 16, format: ChannelFormat::Mono });
    monitored(&mut s, ChannelFormat::Stereo, "X32 17");
    monitored(&mut s, ChannelFormat::Mono, "In 5");
    monitored(&mut s, ChannelFormat::Mono, "No Such Input");
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    eng.input = device_input(2);
    eng.monitor_only = true;
    let out = render_blocks(&mut eng, &s, 2);
    assert!(silent(&out), "nothing wraps onto channels 1-2: {} {}", out[0][100], out[1][100]);

    // An input half on the device: the side beyond it is silent.
    let mut s = Session::default();
    s.inputs.push(InputPath { name: "Edge".into(), first_channel: 1, format: ChannelFormat::Stereo });
    let t = monitored(&mut s, ChannelFormat::Stereo, "Edge");
    s.track_mut(t).unwrap().mixer.pan = vec![-1.0, 1.0];
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    eng.input = device_input(2);
    eng.monitor_only = true;
    let out = render_blocks(&mut eng, &s, 2);
    assert!(out[0][100] > 0.1, "left hears device channel 2: {}", out[0][100]);
    assert_eq!(out[1][100], 0.0, "right (device channel 3) is not available");

    // An output on channels 31-32 still renders into its hardware buffer (the device mapping is
    // the player's job).
    let mut s = Session::default();
    s.outputs.push(OutputPath { name: "X32 Out 31-32".into(), first_channel: 30, format: ChannelFormat::Stereo });
    dc_track(&mut s, 0.5, ChannelFormat::Mono, "Gtr", Route::Hardware("X32 Out 31-32".into()));
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    let out = render_blocks(&mut eng, &s, 2);
    assert!(silent(&out));
    let hw = eng.hardware_out();
    assert_eq!(hw.len(), 2);
    assert!((hw[1][0][100] - 0.5 * G3).abs() < 2e-3, "{}", hw[1][0][100]);
}

#[test]
fn a_track_routed_to_a_hardware_output_renders_there_not_in_main() {
    let mut s = Session::default();
    mon_out(&mut s);
    dc_track(&mut s, 0.5, ChannelFormat::Mono, "Gtr", Route::Hardware("mon 5-6".into()));
    dc_track(&mut s, 0.25, ChannelFormat::Mono, "Lost", Route::Hardware("Nowhere".into()));
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    let out = render_blocks(&mut eng, &s, 2);
    assert!(silent(&out), "neither reaches the main mix");
    let hw = eng.hardware_out();
    assert_eq!(hw.len(), 2);
    assert!(hw[0].is_empty(), "index 0 is the main");
    assert_eq!(hw[1].len(), 2);
    for c in &hw[1] {
        assert_eq!(c.len(), BLOCK);
        assert!((c[100] - 0.5 * G3).abs() < 2e-3, "panned centre into the stereo output: {}", c[100]);
    }
}

#[test]
fn an_aux_routed_to_a_hardware_output_renders_there() {
    let mut s = Session::default();
    mon_out(&mut s);
    let b = s.add_bus("Verb", ChannelFormat::Stereo);
    dc_track(&mut s, 0.5, ChannelFormat::Stereo, "Gtr", Route::Bus(b));
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb Ret"));
    let tr = s.track_mut(aux).unwrap();
    tr.mixer.input = Route::Bus(b);
    tr.mixer.output = Route::Hardware("Mon 5-6".into());
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    let out = render_blocks(&mut eng, &s, 2);
    assert!(silent(&out));
    let hw = eng.hardware_out();
    assert!(hw[1].iter().all(|c| c[100] > 0.1), "{:?}", hw[1].iter().map(|c| c[100]).collect::<Vec<_>>());
}

#[test]
fn the_main_name_routes_to_main() {
    let mut a = Session::default();
    dc_track(&mut a, 0.5, ChannelFormat::Mono, "Gtr", Route::Main);
    let mut b = Session::default();
    let main = b.outputs[0].name.to_uppercase();
    dc_track(&mut b, 0.5, ChannelFormat::Mono, "Gtr", Route::Hardware(main));
    let out_a = render_blocks(&mut MixEngine::new(48_000.0, BLOCK), &a, 2);
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    let out_b = render_blocks(&mut eng, &b, 2);
    assert!(out_a[0][100] > 0.1);
    assert_eq!(out_a, out_b);
    assert!(eng.hardware_out().iter().all(Vec::is_empty));
}

#[test]
fn hardware_buffers_do_not_allocate_in_steady_state() {
    let mut s = Session::default();
    mon_out(&mut s);
    s.outputs.push(OutputPath { name: "Sub".into(), first_channel: 7, format: ChannelFormat::Mono });
    dc_track(&mut s, 0.5, ChannelFormat::Mono, "Gtr", Route::Hardware("Mon 5-6".into()));
    dc_track(&mut s, 0.5, ChannelFormat::Stereo, "Bass", Route::Hardware("Sub".into()));
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    eng.render(&s, 0, BLOCK, &mut out);
    let ids = |e: &MixEngine| e.hardware_out().iter().flatten().map(|c| (c.as_ptr() as usize, c.capacity())).collect::<Vec<_>>();
    let before = ids(&eng);
    assert_eq!(before.len(), 3);
    for b in 1..101 {
        eng.render(&s, (b * BLOCK) as i64, BLOCK, &mut out);
        assert_eq!(ids(&eng), before, "block {b}");
    }
    assert!(eng.hardware_out()[2][0][100] > 0.1);
}

#[test]
fn bounces_ignore_hardware_outputs() {
    let mut s = Session::default();
    mon_out(&mut s);
    dc_track(&mut s, 0.5, ChannelFormat::Mono, "Gtr", Route::Main);
    let reference = render_range(&s, Range::new(0, 4096), BLOCK);
    dc_track(&mut s, 0.3, ChannelFormat::Mono, "Click", Route::Hardware("Mon 5-6".into()));
    assert_eq!(render_range(&s, Range::new(0, 4096), BLOCK), reference);
}

/// A DC step of 0.5 from `STEP` on, on a mono track routed to `output`.
const STEP: usize = 1000;
fn step_track(s: &mut Session, name: &str, output: Route) -> TrackId {
    let src = SourceId(2000 + s.tracks.len() as u64);
    let mut ch = vec![0.0f32; LEN];
    ch[STEP..].iter_mut().for_each(|x| *x = 0.5);
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![ch] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
    let clip = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, name, src, 0, 0, LEN as i64));
    tr.mixer.output = output;
    t
}

/// Renders `blocks` and returns the first sample index above 0.01 in the main mix's left
/// channel and in hardware output 1's left channel.
fn onsets(eng: &mut MixEngine, s: &Session, blocks: usize) -> (Option<usize>, Option<usize>) {
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    let (mut main, mut hw) = (None, None);
    for b in 0..blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut out);
        let first = |c: &[f32]| c.iter().position(|x| x.abs() > 0.01).map(|i| b * BLOCK + i);
        main = main.or(first(&out[0]));
        hw = hw.or(first(&eng.hardware_out()[1][0]));
    }
    (main, hw)
}

/// Two identical step tracks, one to the main mix, one to "Mon 5-6".
fn main_and_monitor() -> Session {
    let mut s = Session::default();
    s.edit.delay_compensation = true;
    mon_out(&mut s);
    step_track(&mut s, "To Main", Route::Main);
    step_track(&mut s, "To Mon", Route::Hardware("Mon 5-6".into()));
    s
}

#[test]
fn a_hardware_output_waits_for_the_aux_returns_as_main_does() {
    let mut s = main_and_monitor();
    let b = s.add_bus("Verb", ChannelFormat::Stereo);
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb Ret"));
    s.track_mut(aux).unwrap().mixer.input = Route::Bus(b);
    common::add_delay(&mut s, aux, 100);
    let mut eng = common::engine(&s, BLOCK);
    let (main, hw) = onsets(&mut eng, &s, 12);
    assert_eq!(main, Some(STEP + 100), "the main mix waits for the aux return");
    assert_eq!(hw, main, "the hardware output lines up with the main mix");
}

#[test]
fn a_hardware_output_waits_for_the_masters_insert_latency() {
    let mut s = main_and_monitor();
    let m = s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
    common::add_delay(&mut s, m, 200);
    let mut eng = common::engine(&s, BLOCK);
    let (main, hw) = onsets(&mut eng, &s, 12);
    assert_eq!(main, Some(STEP + 200), "the master's insert delays the main mix");
    assert_eq!(hw, main, "the hardware output lines up with the main mix");
    assert_eq!(eng.latency, 200);
}

#[test]
fn renamed_inputs_and_outputs_take_effect_on_the_next_block() {
    let mut s = Session::default();
    mon_out(&mut s);
    dc_track(&mut s, 0.5, ChannelFormat::Mono, "Gtr", Route::Hardware("Mon 5-6".into()));
    s.inputs.push(InputPath { name: "Vox".into(), first_channel: 0, format: ChannelFormat::Mono });
    let mic = monitored(&mut s, ChannelFormat::Mono, "Vox");
    s.track_mut(mic).unwrap().mixer.output = Route::Hardware("Mon 5-6".into());
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    eng.input = device_input(4);
    eng.recording = true;
    render_blocks(&mut eng, &s, 2);
    let mon = |e: &MixEngine| e.hardware_out()[1][0][100];
    let before = mon(&eng);
    assert!((before - (0.5 + 0.1) * G3).abs() < 2e-3, "{before}");
    // The input moves to device channel 3: the cached span follows.
    s.inputs[0].first_channel = 2;
    render_blocks(&mut eng, &s, 2);
    assert!((mon(&eng) - (0.5 + 0.3) * G3).abs() < 2e-3, "{}", mon(&eng));
    // The output is renamed: both routes now go nowhere.
    s.outputs[1].name = "Cue".into();
    render_blocks(&mut eng, &s, 2);
    assert!(eng.hardware_out()[1].iter().all(|c| c[100] == 0.0));
}

#[test]
fn no_input_route_reads_device_channel_one_on_both_sides() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Mic"));
    let tr = s.track_mut(t).unwrap();
    tr.mixer.input = Route::None;
    tr.mixer.input_monitor = InputMonitor::On;
    tr.mixer.output = Route::Main;
    tr.mixer.pan = vec![-1.0, 1.0];
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    eng.input = device_input(2);
    eng.monitor_only = true;
    let out = render_blocks(&mut eng, &s, 2);
    assert!(out[0][100] > 0.05 && (out[0][100] - out[1][100]).abs() < 1e-6, "{} {}", out[0][100], out[1][100]);
}
