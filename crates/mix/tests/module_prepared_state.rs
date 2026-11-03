//! A built-in module's prepared state (FireStation CleanQ's ceiling, `Insert::prepared`) reaches
//! the running plugin between blocks: set, changed and cleared on a live engine, and on a new
//! instance, without re-creating it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::MixEngine;
use soundcraft_model::{ChannelFormat, Clip, Insert, PreparedState, Session, SourceAudio, SourceId, TrackId, TrackKind};
use std::sync::Arc;

const LEN: usize = 48_000 * 3;
const BLOCK: usize = 512;

/// A tone over noise on one stereo track, through a CleanQ with its dynamic ceiling on.
fn session(prepared: Option<PreparedState>) -> (Session, TrackId) {
    let mut s = Session::default();
    let mut seed: u32 = 7;
    let signal: Vec<f32> = (0..LEN)
        .map(|i| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            0.05 * ((seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0) + 0.5 * (std::f32::consts::TAU * 2000.0 * i as f32 / 48_000.0).sin()
        })
        .collect();
    s.pool.insert(SourceId(9), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48000, channels: vec![signal.clone(), signal] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Tone"));
    let clip = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "tone", SourceId(9), 0, 0, LEN as i64));
    let mut cq = Insert::new("fs_cleanq");
    cq.params.insert("ceilOn".into(), 1.0);
    cq.prepared = prepared;
    s.track_mut(t).unwrap().mixer.add_module(cq, None).unwrap();
    (s, t)
}

fn ceiling(db: f64) -> PreparedState {
    let state = format!(
        "<ceiling>{}</ceiling>",
        (0..16).map(|k| format!("<point hz=\"{}\" db=\"{db}\"/>", 20.0 * 1000f64.powf(k as f64 / 15.0))).collect::<String>()
    );
    PreparedState::new(soundcraft_dsp::prepare_state("fs_cleanq", state.as_bytes()).unwrap())
}

/// Renders `sessions` one after another on one engine, each for `blocks` blocks: the left channel.
fn render(sessions: &[(&Session, usize)]) -> Vec<f32> {
    let mut e = MixEngine::new(48_000.0, BLOCK);
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    let mut left = Vec::new();
    let mut pos = 0i64;
    for (s, blocks) in sessions {
        for _ in 0..*blocks {
            e.render(s, pos, BLOCK, &mut out);
            left.extend_from_slice(&out[0]);
            pos += BLOCK as i64;
        }
    }
    left
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

#[test]
fn the_ceiling_reaches_the_plugin_and_follows_edits() {
    let blocks = LEN / BLOCK;
    let (plain, _) = session(None);
    let (low, _) = session(Some(ceiling(-60.0)));
    let free = render(&[(&plain, blocks)]);
    let cut = render(&[(&low, blocks)]);
    // Without a ceiling the dynamic ceiling only delays; against one 60 dB down it pulls the tone
    // down (by at most its 9 dB max cut).
    let (a, b) = (rms(&free[24_000..]), rms(&cut[24_000..]));
    assert!(b < a * 0.8, "cut {b} vs free {a}");

    // Live: the same engine, the ceiling set after one second, then cleared after two. The
    // instance is kept (no re-sync), so what it plays tracks the edits.
    let third = blocks / 3;
    let live = render(&[(&plain, third), (&low, third), (&plain, blocks - 2 * third)]);
    let s = third * BLOCK;
    assert_eq!(live[..s], free[..s], "nothing changes before the edit");
    assert!(rms(&live[s + 12_000..2 * s]) < rms(&free[s + 12_000..2 * s]) * 0.8, "the ceiling took hold");
    let back = 2 * s + 24_000;
    assert!((rms(&live[back..]) - rms(&free[back..])).abs() < 0.02 * rms(&free[back..]), "cleared: back to a pure delay");

    // A new preparation of the same numbers (an undo, say) is handed over again, harmlessly.
    let (again, _) = session(Some(ceiling(-60.0)));
    let twice = render(&[(&low, third), (&again, blocks - third)]);
    assert_eq!(twice, cut);
}
