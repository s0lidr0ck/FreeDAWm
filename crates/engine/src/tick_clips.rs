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
//! What does not move: clips on sample-timebase tracks; audio clips (their length is their audio);
//! clips the command moved itself (Insert Time, Cut Time and Move Song Start place the clips
//! after the edit point as they shift the map). Undo and redo restore whole documents and a
//! loaded session is taken as saved, so neither moves anything: an old session opens and sounds
//! as it was saved, and its clips start to follow the tempo at the first tempo edit. A frozen MIDI
//! track refuses tempo edits (its render played the old tempo), so a frozen clip is never moved.

use soundcraft_model::{ClipContent, Session};
use soundcraft_time::TempoMap;

/// Moves the MIDI clips of tick-timebase tracks in `after` that sit where they sat in `before`
/// from their ticks in `before`'s tempo map to the same ticks in `after`'s. Returns how many moved.
pub fn follow_tempo(before: &Session, after: &mut Session) -> usize {
    if before.tempo == after.tempo || before.sample_rate != after.sample_rate {
        return 0;
    }
    let (old, new): (&TempoMap, TempoMap) = (&before.tempo, after.tempo.clone());
    let sr = after.sample_rate;
    let place = |s: i64| new.ticks_f_to_samples_f(old.samples_to_ticks_f(s, sr), sr).round();
    let mut moved = 0;
    for tr in after.tracks.iter_mut().filter(|t| t.ticks_timebase) {
        let Some(was) = before.track(tr.id) else { continue };
        for (i, pl) in tr.playlists.iter_mut().enumerate() {
            let Some(old_pl) = was.playlists.get(i) else { continue };
            let mut any = false;
            for c in pl.clips.iter_mut().filter(|c| matches!(c.content, ClipContent::Midi { .. })) {
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
                    moved += 1;
                }
            }
            if any {
                pl.sort();
            }
        }
    }
    moved
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
