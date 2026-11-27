//! The Spectrum editor's taps in the mix engine: they read what each track sends into the mix
//! (after its fader, mute and pan), cost nothing while the editor is closed or a curve hidden, and
//! allocate nothing while rendering.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::MixEngine;
use soundcraft_mix::spectrum_tap::{MASTER_KEY, SpectrumTaps, track_key};
use soundcraft_model::{ChannelFormat, Clip, Session, SourceAudio, SourceId, TrackId, TrackKind};
use soundcraft_time::SampleRate;
use std::sync::Arc;

const BLOCK: usize = 512;
const RATE: u32 = 48_000;

fn sine(len: usize, amp: f32, hz: f32) -> Vec<f32> {
    (0..len).map(|n| amp * (std::f32::consts::TAU * hz * n as f32 / RATE as f32).sin()).collect()
}

/// A session of `n` mono tracks, each playing the same 1 kHz sine at -12 dBFS, into the main mix.
fn session(n: usize, secs: f64) -> (Session, Vec<TrackId>) {
    let mut s = Session::new("Spectrum", SampleRate::new(RATE).unwrap());
    let len = (secs * f64::from(RATE)) as usize;
    s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: RATE, channels: vec![sine(len, 0.25, 1000.0)] })));
    let mut ids = Vec::new();
    for i in 0..n {
        let fmt = if i % 2 == 0 { ChannelFormat::Mono } else { ChannelFormat::Stereo };
        let t = s.add_track(TrackKind::Audio, fmt, Some(&format!("T{i}")));
        let clip = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "sine", SourceId(1), 0, 0, len as i64));
        ids.push(t);
    }
    (s, ids)
}

fn taps() -> &'static SpectrumTaps {
    Box::leak(Box::new(SpectrumTaps::new()))
}

fn engine(taps: Option<&'static SpectrumTaps>) -> MixEngine {
    let mut eng = MixEngine::new(RATE as f32, BLOCK);
    eng.set_spectrum_taps(taps);
    eng
}

fn render(eng: &mut MixEngine, s: &Session, blocks: std::ops::Range<usize>) {
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    for b in blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut out);
    }
}

/// Claims and switches on a slot for `key`.
fn shown(taps: &SpectrumTaps, key: u64) -> usize {
    let i = taps.claim(key).unwrap();
    taps.set_on(i, true);
    i
}

/// RMS of the newest 4096 samples of slot `i`.
fn rms(taps: &SpectrumTaps, i: usize) -> f32 {
    let mut x = vec![0.0f32; 4096];
    taps.ring(i).unwrap().latest(&mut x);
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

#[test]
fn the_tap_reads_after_the_fader_and_the_mute() {
    let (mut s, ids) = session(4, 2.0);
    s.track_mut(ids[1]).unwrap().mixer.volume_db = -20.0;
    s.track_mut(ids[2]).unwrap().mixer.mute = true;
    let taps = taps();
    let slots: Vec<usize> = ids.iter().map(|t| shown(taps, track_key(*t))).collect();
    let master = shown(taps, MASTER_KEY);
    taps.set_active(true);
    let mut eng = engine(Some(taps));
    render(&mut eng, &s, 0..40);
    let level: Vec<f32> = slots.iter().map(|i| db(rms(taps, *i))).collect();
    // A -12 dBFS sine reads -15 dB RMS at unity (a centred mono track: the pan law's -3 dB per
    // side is one power gain of 1).
    assert!((level[0] - -15.05).abs() < 0.3, "unity: {level:?}");
    assert!((level[1] - level[0] - -20.0).abs() < 0.3, "the fader at -20 dB reads 20 dB lower: {level:?}");
    assert!(level[2] < -120.0, "a muted track reads silence: {level:?}");
    // A stereo track of the same audio reads the same as the mono one.
    assert!((level[3] - level[0]).abs() < 0.3, "stereo and mono agree: {level:?}");
    assert!(db(rms(taps, master)) > level[0], "the master holds the sum");
}

#[test]
fn closed_or_hidden_costs_no_writes() {
    let (s, ids) = session(3, 1.0);
    let taps = taps();
    let a = shown(taps, track_key(ids[0]));
    // Claimed but hidden.
    let b = taps.claim(track_key(ids[1])).unwrap();
    let mut eng = engine(Some(taps));
    // The editor closed: nothing is written at all.
    render(&mut eng, &s, 0..10);
    assert_eq!((taps.ring(a).unwrap().written(), taps.ring(b).unwrap().written()), (0, 0));
    // Open: the shown curve is fed, the hidden one still not.
    taps.set_active(true);
    render(&mut eng, &s, 10..20);
    assert_eq!(taps.ring(a).unwrap().written(), 10 * BLOCK);
    assert_eq!(taps.ring(b).unwrap().written(), 0);
    // Released (the curve went away): no longer fed.
    taps.release(a);
    render(&mut eng, &s, 20..30);
    assert_eq!(taps.ring(a).unwrap().written(), 10 * BLOCK);
}

#[test]
fn an_engine_without_taps_never_touches_them() {
    let (s, ids) = session(2, 1.0);
    let taps = taps();
    let a = shown(taps, track_key(ids[0]));
    taps.set_active(true);
    // An offline render's engine: no taps attached.
    let mut eng = engine(None);
    render(&mut eng, &s, 0..10);
    assert_eq!(taps.ring(a).unwrap().written(), 0);
}

#[test]
fn forty_four_shown_tracks_render_without_allocating() {
    let (s, ids) = session(44, 1.0);
    let taps = taps();
    for t in &ids {
        shown(taps, track_key(*t));
    }
    shown(taps, MASTER_KEY);
    taps.set_active(true);
    let mut eng = engine(Some(taps));
    render(&mut eng, &s, 0..4);
    let before = (taps.buffers(), eng.engine_buffers(), ids.iter().map(|t| eng.strip_buffers(*t)).collect::<Vec<_>>());
    render(&mut eng, &s, 4..90);
    let after = (taps.buffers(), eng.engine_buffers(), ids.iter().map(|t| eng.strip_buffers(*t)).collect::<Vec<_>>());
    assert_eq!(before, after, "nothing the audio thread writes into moved or grew");
    // And a claim made while rendering is picked up without anything new on the audio side.
    let late = (s.tracks.len() + 10) as u64;
    assert!(taps.claim(track_key(TrackId(late))).is_some());
    render(&mut eng, &s, 90..92);
    assert_eq!(eng.engine_buffers(), before.1);
}

/// Not a check: prints what the taps cost a 40-track render with 8, 12, 24 and 40 curves shown
/// (run it by hand with `cargo test -p soundcraft-mix --test spectrum_taps -- --ignored --nocapture`).
#[test]
#[ignore = "a timing measurement, run by hand"]
fn measure_the_cost() {
    let (s, ids) = session(40, 12.0);
    let blocks = 1000;
    let time = |taps: Option<&'static SpectrumTaps>| {
        let mut eng = engine(taps);
        render(&mut eng, &s, 0..20);
        let mut best = f64::MAX;
        for _ in 0..7 {
            let t0 = std::time::Instant::now();
            render(&mut eng, &s, 20..20 + blocks);
            best = best.min(t0.elapsed().as_secs_f64());
        }
        best / blocks as f64 * 1e6
    };
    let none = time(None);
    let closed_taps = taps();
    for t in &ids {
        shown(closed_taps, track_key(*t));
    }
    let closed = time(Some(closed_taps));
    println!(
        "40 tracks, {BLOCK}-frame block ({:.0} µs of audio): no taps {none:.1} µs, editor closed {closed:.1} µs",
        BLOCK as f64 / f64::from(RATE) * 1e6
    );
    for n in [8usize, 12, 24, 40] {
        let open_taps = taps();
        for t in ids.iter().take(n) {
            shown(open_taps, track_key(*t));
        }
        open_taps.set_active(true);
        let open = time(Some(open_taps));
        println!("  {n} shown: {open:.1} µs (+{:.1} µs over no taps)", open - none);
    }
}
