//! MIDI clips on a track with the tick timebase keep their place in bars and beats when the tempo
//! map changes.
//!
//! A clip's start and length are stored in samples (as every clip's are) and its notes in ticks
//! from its start. When a command changes the tempo map, each MIDI clip on a tick-timebase track
//! that the command left where it was is moved from the ticks it sat on in the old map to the same
//! ticks in the new one: its start and its end follow the bar lines, so it stays on its bar and
//! its last notes stay inside it. The positions are converted in fractional ticks, so a clip that
//! starts between two ticks keeps its place, and a tempo change undone by another returns it to
//! the sample it started on.
//!
//! Audio clips on a tick-timebase track follow too. On a track that is not Elastic only the start
//! follows (the audio keeps its own speed, as a sample does). On an Elastic track the clip also
//! takes its new length: with Varispeed by its stretch ratio (the pitch moves with the speed), with
//! a pitch-preserving algorithm by a render from its original material at the new ratio
//! ([`follow_tempo`] returns those for the engine to render, as Elastic Properties does).
//! Edit- or time-locked audio clips stay where they are.
//!
//! A tick-timebase track's automation follows the same way: each lane the command left as it was
//! has its points moved from their ticks in the old map to the same ticks in the new one, so a
//! ride stays on its beat (the "automation follows tempo" of the four reference DAWs).
//!
//! What does not move: clips and automation on sample-timebase tracks; clips the command moved itself (Insert Time, Cut Time and Move Song Start place the clips
//! after the edit point as they shift the map). Undo and redo restore whole documents and a
//! loaded session is taken as saved, so neither moves anything: an old session opens and sounds
//! as it was saved, and its clips start to follow the tempo at the first tempo edit. A frozen MIDI
//! track refuses tempo edits (its render played the old tempo), so a frozen clip is never moved.

use soundcraft_model::{ClipContent, ClipId, Session};
use soundcraft_time::TempoMap;

/// What [`follow_tempo`] did: clips moved, and the audio clips of pitch-preserving Elastic tracks
/// that need their material rendered to a new length (clip, length now ÷ length before).
#[derive(Debug, Default, PartialEq)]
pub struct Followed {
    pub moved: usize,
    pub renders: Vec<(ClipId, f64)>,
}

/// Moves the clips of tick-timebase tracks in `after` that sit where they sat in `before` from
/// their ticks in `before`'s tempo map to the same ticks in `after`'s (audio as the module says).
pub fn follow_tempo(before: &Session, after: &mut Session) -> Followed {
    let mut out = Followed::default();
    if before.tempo == after.tempo || before.sample_rate != after.sample_rate {
        return out;
    }
    let (old, new): (&TempoMap, TempoMap) = (&before.tempo, after.tempo.clone());
    let sr = after.sample_rate;
    let place = |s: i64| new.ticks_f_to_samples_f(old.samples_to_ticks_f(s, sr), sr).round();
    for tr in after.tracks.iter_mut().filter(|t| t.ticks_timebase) {
        let Some(was) = before.track(tr.id) else { continue };
        let elastic = tr.elastic.clone();
        for (i, pl) in tr.playlists.iter_mut().enumerate() {
            let Some(old_pl) = was.playlists.get(i) else { continue };
            let mut any = false;
            for c in pl.clips.iter_mut() {
                if matches!(c.content, ClipContent::Audio { .. }) {
                    if c.edit_locked || c.time_locked || !old_pl.clips.iter().any(|o| o.id == c.id && o.start == c.start && o.length == c.length) {
                        continue;
                    }
                    let (a, b) = (place(c.start), place(c.end()));
                    if !a.is_finite() || !b.is_finite() || a > i64::MAX as f64 / 2.0 || b > i64::MAX as f64 / 2.0 {
                        continue;
                    }
                    let start = (a as i64).max(0);
                    let len = ((b - a).round() as i64).max(1);
                    let was_len = c.length;
                    if start != c.start {
                        c.start = start;
                        any = true;
                        out.moved += 1;
                    }
                    match elastic.as_deref() {
                        Some(_) if len == was_len => {}
                        Some("varispeed") => {
                            let r = len as f64 / was_len.max(1) as f64;
                            let st = if c.stretch.is_finite() && c.stretch > 0.0 { c.stretch } else { 1.0 };
                            c.stretch = st * r;
                            c.length = len;
                            c.clamp_fades();
                            any = true;
                        }
                        Some(_) => out.renders.push((c.id, len as f64 / was_len.max(1) as f64)),
                        None => {}
                    }
                    continue;
                }
                if !matches!(c.content, ClipContent::Midi { .. }) {
                    continue;
                }
                // Only a clip the command left in place: one it moved or trimmed is where it put it.
                if !old_pl.clips.iter().any(|o| o.id == c.id && o.start == c.start && o.length == c.length) {
                    continue;
                }
                let (a, b) = (place(c.start), place(c.end()));
                if !a.is_finite() || !b.is_finite() || a > i64::MAX as f64 / 2.0 || b > i64::MAX as f64 / 2.0 {
                    continue;
                }
                let start = (a as i64).max(0);
                let end = (b as i64).max(start.saturating_add(1));
                if start != c.start || end != c.end() {
                    c.start = start;
                    c.length = end - start;
                    any = true;
                    out.moved += 1;
                }
            }
            if any {
                pl.sort();
            }
        }
        // Its automation: each lane the command left as it was keeps its points on their ticks
        // (the map is monotonic, so their order holds).
        for lane in tr.automation.iter_mut() {
            if !was.automation.iter().any(|l| l.param == lane.param && l.points == lane.points) {
                continue;
            }
            for p in lane.points.iter_mut() {
                let a = place(p.at);
                if a.is_finite() && a <= i64::MAX as f64 / 2.0 {
                    p.at = (a as i64).max(0);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::{ChannelFormat, ClipId, TrackKind};

    fn clip_at(e: &Engine, id: u64) -> (i64, i64) {
        let c = e.session().find_clip(ClipId(id)).unwrap().1;
        (c.start, c.length)
    }

    /// Bug 3: at 120 bpm a bar is 96 000 samples at 48 kHz. Halving the tempo from bar 2 moves a
    /// clip on bar 3 to where bar 3 now is and doubles its length; the notes stay inside it.
    #[test]
    fn midi_clips_on_tick_tracks_stay_on_their_bars_when_the_tempo_changes() {
        let mut e = Engine::default();
        let midi = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        assert!(e.session().track(midi).unwrap().ticks_timebase);
        let clip = e.execute("midi.new_clip", &json!({"track": midi.0, "start": 192_000, "end": 288_000})).unwrap()["clip"].as_u64().unwrap();
        e.execute("midi.note_add", &json!({"clip": clip, "pitch": 60, "start_ticks": 3_000, "length_ticks": 800})).unwrap();
        // The same clip on a track kept in samples does not move.
        let fixed = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Fixed"));
        e.session_mut().track_mut(fixed).unwrap().ticks_timebase = false;
        let other = e.execute("midi.new_clip", &json!({"track": fixed.0, "start": 192_000, "end": 288_000})).unwrap()["clip"].as_u64().unwrap();
        e.execute("event.tempo", &json!({"bpm": 60.0, "at": 96_000})).unwrap();
        assert_eq!(clip_at(&e, clip), (96_000 + 192_000, 192_000));
        assert_eq!(clip_at(&e, other), (192_000, 96_000));
        // The last note still ends inside the clip.
        let s = e.session();
        let c = s.find_clip(ClipId(clip)).unwrap().1;
        let base = s.tempo.samples_to_ticks(c.start, s.sample_rate);
        assert!(s.tempo.tick_to_samples(base + 3_800, s.sample_rate) <= c.end());
        // Undo puts the clip and the tempo back in one step; redo moves it again.
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(clip_at(&e, clip), (192_000, 96_000));
        e.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(clip_at(&e, clip), (288_000, 192_000));
        // Back to 120 bpm: back to the sample it started on.
        e.execute("event.tempo_remove", &json!({"at": 96_000})).unwrap();
        assert_eq!(clip_at(&e, clip), (192_000, 96_000));
    }

    /// A tick-timebase track's automation stays on its bars when the tempo changes; a sample
    /// track's stays on its samples.
    #[test]
    fn automation_on_tick_tracks_follows_the_tempo() {
        let mut e = Engine::default();
        let keys = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let fixed = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Fixed"));
        e.session_mut().track_mut(fixed).unwrap().ticks_timebase = false;
        for t in [keys, fixed] {
            e.execute("automation.set_point", &json!({"track": t.0, "param": "volume", "at": 192_000, "value": -6.0})).unwrap();
        }
        let at = |e: &Engine, t| e.session().track(t).unwrap().lane(&soundcraft_model::AutoParam::Volume).unwrap().points[0].at;
        e.execute("event.tempo", &json!({"bpm": 60.0, "at": 96_000})).unwrap();
        assert_eq!(at(&e, keys), 96_000 + 192_000, "bar 3 moved with the tempo");
        assert_eq!(at(&e, fixed), 192_000);
        e.execute("event.tempo_remove", &json!({"at": 96_000})).unwrap();
        assert_eq!(at(&e, keys), 192_000, "and back");
    }

    /// An audio track kept in bars and beats: a plain track's clip moves to its bar and keeps its
    /// length; on a Varispeed track it also takes the new length by its stretch; on a
    /// pitch-preserving Elastic track it is rendered to the new length; a sample track's stays.
    #[test]
    fn audio_clips_on_tick_tracks_follow_the_tempo() {
        use soundcraft_audio_io::{AudioBuffer, FileFormat};
        use soundcraft_model::Clip;
        let mut e = Engine::default();
        let src = crate::io::add_source(
            e.session_mut(),
            "tone",
            AudioBuffer { sample_rate: 48_000, channels: vec![(0..96_000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect()] },
            None,
            FileFormat::Wav,
        );
        let mut clips = Vec::new();
        for (name, ticks, elastic) in
            [("Plain", true, None), ("Vari", true, Some("varispeed")), ("Poly", true, Some("polyphonic")), ("Fixed", false, None)]
        {
            let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
            let s = e.session_mut();
            let id = s.new_clip_id();
            let tr = s.track_mut(t).unwrap();
            tr.ticks_timebase = ticks;
            tr.elastic = elastic.map(str::to_string);
            tr.playlist_mut().unwrap().clips = vec![Clip::audio(id, name, src, 0, 192_000, 96_000)];
            clips.push(id.0);
        }
        // 60 bpm from bar 2: bar 3 moves from 192 000 to 288 000 and a bar is twice as long.
        e.execute("event.tempo", &json!({"bpm": 60.0, "at": 96_000})).unwrap();
        assert_eq!(clip_at(&e, clips[0]), (288_000, 96_000), "plain: the start follows, the audio keeps its speed");
        assert_eq!(clip_at(&e, clips[1]), (288_000, 192_000), "varispeed: the length follows");
        assert!((e.session().find_clip(ClipId(clips[1])).unwrap().1.stretch - 2.0).abs() < 1e-9);
        let poly = e.session().find_clip(ClipId(clips[2])).unwrap().1.clone();
        assert_eq!((poly.start, poly.length), (288_000, 192_000), "elastic: rendered to the new length");
        assert_eq!(poly.stretch, 1.0, "rendered, not varispeed");
        assert_eq!(clip_at(&e, clips[3]), (192_000, 96_000), "a sample track's clip stays");
        // Undo puts everything back in one step.
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(clip_at(&e, clips[2]), (192_000, 96_000));
    }

    /// A clip that starts between two ticks keeps its offset through a tempo change and back.
    #[test]
    fn a_clip_off_the_tick_grid_returns_to_its_sample() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let clip = e.execute("midi.new_clip", &json!({"track": t.0, "start": 100_013, "end": 150_007})).unwrap()["clip"].as_u64().unwrap();
        e.execute("event.tempo", &json!({"bpm": 97.0, "at": 0})).unwrap();
        assert_ne!(clip_at(&e, clip).0, 100_013);
        e.execute("event.tempo", &json!({"bpm": 120.0, "at": 0})).unwrap();
        assert_eq!(clip_at(&e, clip), (100_013, 49_994));
    }
}
