//! Mixing parity (section 3), round three: strip features checked by rendering.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::MixEngine;
use soundcraft_model::{ChannelFormat, Clip, Route, Session, SourceAudio, SourceId, TrackId, TrackKind};
use std::sync::Arc;

const BLOCK: usize = 256;

fn session() -> Session {
    let mut s = Session::default();
    s.edit.values.insert("playback.auto_fade_ms".into(), 0.0);
    s
}

/// A stereo track on the main mix playing `l` and `r` (constant) for `len` samples.
fn stereo(s: &mut Session, l: f32, r: f32, len: usize) -> TrackId {
    let src = SourceId(7000 + s.tracks.len() as u64);
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![l; len], vec![r; len]] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("St"));
    let clip = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, "St", src, 0, 0, len as i64));
    tr.mixer.output = Route::Main;
    t
}

/// The main mix's last sample of each channel after `blocks` blocks from 0.
fn settle(s: &Session, blocks: usize) -> (f32, f32) {
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    let mut buf = vec![vec![0.0f32; BLOCK]; 2];
    for b in 0..blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut buf);
    }
    (buf[0][BLOCK - 1], buf[1][BLOCK - 1])
}

/// Polarity on one channel of a stereo track inverts that side only; the whole-track switch
/// inverts both.
#[test]
fn polarity_inverts_single_channels() {
    let mut s = session();
    let t = stereo(&mut s, 0.25, 0.5, BLOCK * 20);
    let (l0, r0) = settle(&s, 10);
    assert!(l0 > 0.1 && r0 > 0.2, "{l0} {r0}");
    s.track_mut(t).unwrap().mixer.phase_invert_channels = 0b10;
    let (l, r) = settle(&s, 10);
    assert!((l - l0).abs() < 1e-5 && (r + r0).abs() < 1e-5, "right only: {l} {r}");
    s.track_mut(t).unwrap().mixer.phase_invert_channels = 0;
    s.track_mut(t).unwrap().mixer.phase_invert = true;
    let (l, r) = settle(&s, 10);
    assert!((l + l0).abs() < 1e-5 && (r + r0).abs() < 1e-5, "both: {l} {r}");
}
