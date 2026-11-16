//! A module's automation is read where its input is on the timeline: behind a latent module
//! before it by that module's latency, as the fader's automation is behind the whole chain.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{add_delay, engine};
use soundcraft_audio_io::AudioBuffer;
use soundcraft_model::{AutoParam, ChannelFormat, Clip, Insert, Session, SourceAudio, SourceId, TrackKind};
use std::sync::Arc;

const BLOCK: usize = 64;

/// The left output of a mono DC track through `chain_delay` samples of latency and then a gain
/// module whose automation steps from 0 dB to off at sample 2000.
fn render(chain_delay: usize) -> Vec<f32> {
    let mut s = Session::default();
    s.edit.click = false;
    s.edit.values.insert("playback.auto_fade_ms".into(), 0.0);
    let n = 8192;
    s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; n]] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(cid, "dc", SourceId(1), 0, 0, n as i64));
    if chain_delay > 0 {
        add_delay(&mut s, t, chain_delay);
    }
    let mut gain = Insert::new("gain");
    gain.params.insert("gain".into(), 0.0);
    let m = s.track_mut(t).unwrap().mixer.add_module(gain, None).unwrap();
    let lane = s.track_mut(t).unwrap().lane_mut(&AutoParam::Plugin { module: m, param: "gain".into() });
    lane.set_point(0, 0.0);
    lane.set_point(1999, 0.0);
    lane.set_point(2000, -96.0);
    let mut eng = engine(&s, BLOCK);
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    let mut left = Vec::new();
    for b in 0..(4096 / BLOCK) {
        eng.render(&s, (b * BLOCK) as i64, BLOCK, &mut out);
        left.extend_from_slice(&out[0]);
    }
    left
}

/// Where the output first falls below its steady level.
fn falls_at(x: &[f32]) -> usize {
    let steady = x[1500];
    x.iter().enumerate().skip(1500).find(|(_, v)| **v < steady - 1e-4).map_or(usize::MAX, |(i, _)| i)
}

#[test]
fn a_module_reads_its_automation_where_its_audio_is() {
    let direct = falls_at(&render(0));
    assert!((2000..2000 + BLOCK).contains(&direct), "{direct}");
    // 300 samples of latency before it: the step lands on the same audio, 300 samples later at
    // the output (the audio itself is 300 samples late), not on audio 300 samples earlier.
    let late = falls_at(&render(300));
    assert!((2300..2300 + BLOCK).contains(&late), "{late}");
}
