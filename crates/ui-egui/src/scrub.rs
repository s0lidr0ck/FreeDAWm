//! Scrub audio: dragging the Scrubber over a track (or along a timebase ruler, for every audible
//! track) plays the audio the pointer passes over, forwards or backwards, at the speed of the
//! drag. Each frame the stretch swept since the last one is rendered through each scrubbed
//! track's strip as far as it can be away from the audio thread (clips, then the inserts when
//! they are all built in, then the fader; a muted track is silent), reversed when the drag goes
//! back, and sent to the player's preview voice at a rate that makes it last about a frame, so
//! its pitch follows the drag speed as tape does. The inserts start afresh each frame, so a
//! reverb's tail does not carry from one frame to the next.

use soundcraft_model::{Session, TrackId};
use soundcraft_time::{Range, Samples};

/// Track `t`'s audio over `range` as its strip makes it, as far as the UI thread can: its clips
/// through its inserts (pre-fader, [`soundcraft_mix::render_track_pre_fader`]) when every insert
/// is built in, else its clips alone, then its fader level. Nothing when it is muted.
pub fn strip_audio(s: &Session, t: TrackId, range: Range) -> Vec<Vec<f32>> {
    let Some(tr) = s.track(t) else { return Vec::new() };
    if tr.mixer.mute {
        return Vec::new();
    }
    let built_in = !tr.mixer.inserts.iter().any(|i| soundcraft_mix::is_third_party(&i.plugin));
    let mut audio = if built_in { soundcraft_mix::render_track_pre_fader(s, t, range) } else { soundcraft_mix::render_clips(s, t, range) };
    let db = if tr.mixer.volume_db.is_finite() { tr.mixer.volume_db.clamp(-144.0, 24.0) } else { 0.0 };
    let g = 10f32.powf(db / 20.0);
    audio.iter_mut().flatten().for_each(|v| *v *= g);
    audio
}

/// Every track a ruler scrub plays: those with clips that are not muted.
pub fn audible_tracks(s: &Session) -> Vec<TrackId> {
    s.tracks.iter().filter(|t| t.kind.has_playlist() && !t.kind.is_midi() && !t.mixer.mute).map(|t| t.id).collect()
}

/// The most audio one frame of a fast drag plays, in seconds of the session (a faster drag skips).
pub const MAX_SWEEP_SECONDS: f64 = 0.5;

/// The swept stretch from `from` to `to` on `tracks` as stereo audio and the rate that plays it
/// in `seconds`; `None` when nothing was swept or nothing plays there.
pub fn chunk(s: &Session, tracks: &[TrackId], from: Samples, to: Samples, seconds: f64) -> Option<(Vec<Vec<f32>>, u32)> {
    if from == to || tracks.is_empty() || !(seconds.is_finite() && seconds > 0.0) {
        return None;
    }
    let max = s.sample_rate.samples(MAX_SWEEP_SECONDS).max(1);
    let (a, b) = if to > from { (from.max(to - max), to) } else { (to, from.min(to.saturating_add(max))) };
    let range = Range::new(a.max(0), b.max(0));
    let len = usize::try_from(range.len()).ok().filter(|n| *n > 1)?;
    let mut out = vec![vec![0.0f32; len]; 2];
    let mut any = false;
    for t in tracks {
        let audio = strip_audio(s, *t, range);
        let n = audio.len();
        for (c, ch) in audio.iter().enumerate() {
            // Mono to both sides; stereo left and right; more channels fold in turn.
            for side in 0..2 {
                if (n == 1 || c % 2 == side)
                    && let Some(dst) = out.get_mut(side)
                {
                    for (d, v) in dst.iter_mut().zip(ch) {
                        *d += *v;
                        any |= *v != 0.0;
                    }
                }
            }
        }
    }
    if !any {
        return None;
    }
    if to < from {
        out.iter_mut().for_each(|ch| ch.reverse());
    }
    // Short ramps at both ends: chunks follow one another without clicks.
    let ramp = (len / 4).clamp(1, 96);
    for ch in &mut out {
        for i in 0..ramp {
            let g = i as f32 / ramp as f32;
            if let Some(x) = ch.get_mut(i) {
                *x *= g;
            }
            if let Some(x) = ch.get_mut(len - 1 - i) {
                *x *= g;
            }
        }
    }
    let rate = (len as f64 / seconds).clamp(4_000.0, 384_000.0) as u32;
    Some((out, rate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};

    fn session() -> (Session, TrackId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let ramp: Vec<f32> = (0..48_000).map(|i| i as f32 / 48_000.0).collect();
        let audio = SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![ramp] });
        s.pool.insert(SourceId(1), std::sync::Arc::new(audio));
        let id = s.new_clip_id();
        if let Some(p) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            p.clips = vec![Clip::audio(id, "ramp", SourceId(1), 0, 0, 48_000)];
        }
        (s, t)
    }

    #[test]
    fn a_drag_plays_what_it_passes_forwards_or_backwards_at_its_speed() {
        let (s, t) = session();
        let (fwd, rate) = chunk(&s, &[t], 10_000, 10_800, 1.0 / 60.0).unwrap();
        assert_eq!(fwd[0].len(), 800);
        assert_eq!(rate, 48_000, "800 samples in a 60th of a second: real speed");
        assert!(fwd[0][400] > 0.2 && fwd[0][400] < 0.22, "the material at 10_400: {}", fwd[0][400]);
        assert_eq!(fwd[0], fwd[1], "mono plays on both sides");
        let (back, _) = chunk(&s, &[t], 10_800, 10_000, 1.0 / 60.0).unwrap();
        assert!(back[0][300] > back[0][500], "backwards it plays the ramp down");
        // A slow drag plays slower (lower), a fast one is cut to half a second.
        let (_, slow) = chunk(&s, &[t], 10_000, 10_100, 1.0 / 60.0).unwrap();
        assert_eq!(slow, 6_000);
        let (fast, _) = chunk(&s, &[t], 0, 47_000, 1.0 / 60.0).unwrap();
        assert_eq!(fast[0].len(), 24_000);
        // Nothing moved, or nothing there: nothing plays.
        assert!(chunk(&s, &[t], 500, 500, 0.1).is_none());
        assert!(chunk(&s, &[t], 60_000, 61_000, 0.1).is_none());
    }

    #[test]
    fn a_scrub_is_heard_through_the_fader_and_mute() {
        let (mut s, t) = session();
        let (full, _) = chunk(&s, &[t], 10_000, 10_800, 1.0 / 60.0).unwrap();
        s.track_mut(t).unwrap().mixer.volume_db = -20.0;
        let (low, _) = chunk(&s, &[t], 10_000, 10_800, 1.0 / 60.0).unwrap();
        assert!((low[0][400] - full[0][400] * 0.1).abs() < 1e-3, "{} vs {}", low[0][400], full[0][400]);
        s.track_mut(t).unwrap().mixer.mute = true;
        assert!(chunk(&s, &[t], 10_000, 10_800, 1.0 / 60.0).is_none(), "a muted track is silent");
        assert!(audible_tracks(&s).is_empty());
    }
}
