//! How a clip's audio is read: varispeed clips through the band-limited reader.

use soundcraft_model::{ChannelFormat, Clip, Session, SourceAudio, SourceId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;

/// A mono session with one clip of `samples` (at 48 kHz) at 0, `stretch` as its varispeed ratio.
fn session(samples: Vec<f32>, stretch: f64) -> (Session, soundcraft_model::TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("A"));
    let len = samples.len() as i64;
    s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![samples] })));
    let id = s.new_clip_id();
    let mut c = Clip::audio(id, "c", SourceId(1), 0, 0, (len as f64 * stretch) as i64);
    c.stretch = stretch;
    if let Some(pl) = s.track_mut(t).and_then(|x| x.playlist_mut()) {
        pl.clips.push(c);
    }
    (s, t)
}

fn rms(v: &[f32]) -> f32 {
    (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
}

/// A 15 kHz tone played at twice its speed would be 30 kHz: above 24 kHz it can only fold back
/// as aliasing at 18 kHz. The band-limited reader filters it out instead.
#[test]
fn a_sped_up_clip_does_not_alias() {
    let tone: Vec<f32> = (0..96_000).map(|i| (std::f32::consts::TAU * 15_000.0 * i as f32 / 48_000.0).sin() * 0.5).collect();
    let (s, t) = session(tone, 0.5);
    let out = soundcraft_mix::render_clips(&s, t, Range { start: 1000, end: 40_000 });
    let level = rms(&out[0]);
    assert!(
        level < 0.5 * std::f32::consts::FRAC_1_SQRT_2 * 0.03,
        "aliasing left at {level} (−30 dB would be {})",
        0.5 * std::f32::consts::FRAC_1_SQRT_2 * 0.03
    );
}

/// A tone well inside the band plays through a stretched read at its level.
#[test]
fn a_stretched_clip_keeps_a_low_tone() {
    let tone: Vec<f32> = (0..96_000).map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin() * 0.5).collect();
    for stretch in [0.5, 1.5] {
        let (s, t) = session(tone.clone(), stretch);
        let out = soundcraft_mix::render_clips(&s, t, Range { start: 1000, end: 40_000 });
        let level = rms(&out[0]);
        assert!((level - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01, "{stretch}: {level}");
    }
}
