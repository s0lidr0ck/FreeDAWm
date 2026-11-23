//! The song's key for audio effects that follow it (Auto Pitch): the Key ruler with its key
//! changes by timeline position, worked out off the audio thread and handed to each insert that
//! asks ([`soundcraft_dsp::Plugin::follows_song_key`]) when the mix syncs the session, as the
//! chord lane is handed to the MIDI Scale effect ([`crate::midi_fx::chord_table`]). The plugin
//! copies the table into fixed storage: reading it never allocates or locks.
//!
//! One key for the song (`soundcraft_engine::song_key`): the key at bar 1 is the band's key, so the
//! table starts with it from the very beginning; each key change on the ruler (one that parses)
//! takes over from its position.

use soundcraft_dsp::{MAX_SONG_KEYS, SongKey};
use soundcraft_model::{Session, parse_key};

/// The song's keys in time order (allocates: control thread).
pub fn song_key_table(s: &Session) -> Vec<SongKey> {
    let mut out = vec![SongKey { start: i64::MIN, tonic: s.band.key % 12, minor: s.band.scale == 1 }];
    let mut ruler: Vec<(i64, u8, u8)> = s.key_signatures.iter().filter_map(|(at, name)| parse_key(name).map(|(t, m)| (*at, t, m))).collect();
    ruler.sort_by_key(|k| k.0);
    for (at, tonic, scale) in ruler {
        let key = SongKey { start: if at <= 0 { i64::MIN } else { at }, tonic, minor: scale == 1 };
        match out.last_mut() {
            Some(last) if last.start == key.start => *last = key,
            _ => out.push(key),
        }
    }
    out.truncate(MAX_SONG_KEYS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_range;
    use soundcraft_audio_io::AudioBuffer;
    use soundcraft_model::{ChannelFormat, Clip, Insert, SourceAudio, SourceId, TrackId, TrackKind};
    use soundcraft_time::Range;
    use std::f64::consts::TAU;
    use std::sync::Arc;

    /// A harmonic tone at MIDI cents `cents`, `secs` long, peak about 0.3, 30 ms fades.
    fn tone(secs: f64, cents: f64) -> Vec<f32> {
        let n = (secs * 48_000.0) as usize;
        let f = 440.0 * 2f64.powf((cents - 6900.0) / 1200.0);
        (0..n)
            .map(|i| {
                let ph = TAU * f * i as f64 / 48_000.0;
                let e = (i.min(n - 1 - i) as f64 / 1440.0).min(1.0);
                ((1..=10).map(|k| (k as f64 * ph).sin() / k as f64).sum::<f64>() * 0.15 * e) as f32
            })
            .collect()
    }

    fn noise(n: usize) -> Vec<f32> {
        let mut r = 7u64;
        (0..n)
            .map(|_| {
                r ^= r << 13;
                r ^= r >> 7;
                r ^= r << 17;
                (((r >> 40) as f64 / (1u64 << 24) as f64) - 0.5) as f32 * 0.4
            })
            .collect()
    }

    /// A mono track playing `x` from 0, panned `pan`, with `fx` inserts.
    fn track(s: &mut Session, src: u64, x: &[f32], pan: f32, fx: &[Insert]) -> TrackId {
        s.pool.insert(SourceId(src), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![x.to_vec()] })));
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let id = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(id, "x", SourceId(src), 0, 0, x.len() as i64));
        tr.mixer.pan = vec![pan];
        for ins in fx {
            tr.mixer.add_module(ins.clone(), None).unwrap();
        }
        t
    }

    /// The average pitch (MIDI cents) of `y` over `[a, b)` seconds (autocorrelation peak).
    fn pitch(y: &[f32], a: f64, b: f64) -> f64 {
        let seg = &y[(a * 48_000.0) as usize..(b * 48_000.0) as usize];
        let ac = |lag: usize| -> f64 { seg.iter().zip(&seg[lag..]).map(|(p, q)| f64::from(*p) * f64::from(*q)).sum() };
        let best = (48..800).max_by(|p, q| ac(*p).total_cmp(&ac(*q))).unwrap();
        let (y0, y1, y2) = (ac(best - 1), ac(best), ac(best + 1));
        let lag = best as f64 + 0.5 * (y0 - y2) / (y0 - 2.0 * y1 + y2);
        6900.0 + 1200.0 * (48_000.0 / lag / 440.0).log2()
    }

    fn auto_pitch(set: &[(&str, f32)]) -> Insert {
        let mut ins = Insert::new("autopitch");
        for (id, v) in set {
            ins.params.insert((*id).into(), *v);
        }
        ins
    }

    #[test]
    fn a_key_change_on_the_key_ruler_mid_clip_moves_the_target_there() {
        // A sung 40 c flat for 2 s; C major from the start, Ab major (no A natural) from 1 s.
        let mut s = Session { key_signatures: vec![(0, "C major".into()), (48_000, "Ab major".into())], ..Session::default() };
        let x = tone(2.0, 5660.0);
        track(&mut s, 900, &x, 0.0, &[auto_pitch(&[("style", 1.0)])]);
        let out = render_range(&s, Range::new(0, 96_000), 512);
        let (before, after) = (pitch(&out[0], 0.5, 0.9), pitch(&out[0], 1.3, 1.8));
        assert!((before - 5700.0).abs() < 3.0, "C major: A, {before}");
        assert!((after - 5600.0).abs() < 3.0, "Ab major: Ab, {after}");
        // Its own key instead (key C, Major): A all through.
        let t = s.tracks.last().unwrap().id;
        let m = s.track(t).unwrap().mixer.inserts[0].id;
        s.track_mut(t).unwrap().module_mut(m).unwrap().params.insert("key".into(), 1.0);
        let out = render_range(&s, Range::new(0, 96_000), 512);
        assert!((pitch(&out[0], 1.3, 1.8) - 5700.0).abs() < 3.0);
    }

    #[test]
    fn the_mixer_lines_auto_pitch_up_with_the_other_tracks_and_its_bypass_too() {
        // The same noise (let through untouched, only delayed) on two tracks panned apart, one
        // through Auto Pitch: with delay compensation the two sides are identical, bypassed or
        // not. A voice at Mix 0 % likewise.
        for (x, set) in [(noise(48_000), vec![]), (tone(1.0, 5660.0), vec![("style", 1.0), ("mix", 0.0)])] {
            let mut s = Session::default();
            s.edit.values.insert("playback.auto_fade_ms".into(), 0.0);
            let fx = track(&mut s, 900, &x, -1.0, &[auto_pitch(&set)]);
            track(&mut s, 901, &x, 1.0, &[]);
            let out = render_range(&s, Range::new(0, 48_000), 512);
            let worst = out[0].iter().zip(&out[1]).map(|(l, r)| (l - r).abs()).fold(0.0f32, f32::max);
            assert!(worst < 1e-6, "aligned: {worst}");
            assert!(out[0].iter().map(|v| v.abs()).fold(0.0f32, f32::max) > 0.05);
            let m = s.track(fx).unwrap().mixer.inserts[0].id;
            s.track_mut(fx).unwrap().module_mut(m).unwrap().bypass = true;
            let out = render_range(&s, Range::new(0, 48_000), 512);
            let worst = out[0].iter().zip(&out[1]).map(|(l, r)| (l - r).abs()).fold(0.0f32, f32::max);
            assert!(worst < 1e-6, "bypassed, aligned: {worst}");
        }
    }

    #[test]
    fn auto_pitch_settings_survive_a_save_and_load() {
        let mut s = Session::default();
        let ins = auto_pitch(&[("style", 2.0), ("retune", 33.0), ("key", 7.0), ("scale", 12.0), ("note3", 1.0), ("formant", -2.5), ("mix", 80.0)]);
        let t = track(&mut s, 900, &tone(0.1, 6000.0), 0.0, &[ins]);
        let json = serde_json::to_string(&s).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        let a = &s.track(t).unwrap().mixer.inserts[0];
        let b = &back.track(t).unwrap().mixer.inserts[0];
        assert_eq!(a.params, b.params);
        assert_eq!(b.plugin, "autopitch");
    }

    #[test]
    fn the_band_key_holds_from_the_start_and_ruler_changes_take_over() {
        let mut s = Session::default();
        s.band.key = 2;
        s.band.scale = 1;
        assert_eq!(song_key_table(&s), vec![SongKey { start: i64::MIN, tonic: 2, minor: true }]);
        s.key_signatures = vec![(96_000, "F# major".into()), (0, "D minor".into()), (200_000, "nonsense".into())];
        assert_eq!(song_key_table(&s), vec![SongKey { start: i64::MIN, tonic: 2, minor: true }, SongKey { start: 96_000, tonic: 6, minor: false }]);
    }

    #[test]
    fn auto_pitchs_scales_are_the_midi_scale_effects() {
        let ours: Vec<(&str, u16)> = soundcraft_dsp::plugins::autopitch::SCALES.to_vec();
        let theirs: Vec<(&str, u16)> = soundcraft_model::SCALES.to_vec();
        assert_eq!(ours, theirs);
    }
}
