//! MIDI clips that follow the chord lane (opt-in per clip, `clip.follow_chords`).
//!
//! Design: a following clip keeps the chords it was written against (`Clip::follow_chords`, in
//! ticks from its start, as MIDI Cleanup reads the lane). After any command that changes the
//! chord lane or the band key, each following clip's notes are moved from the chord they were
//! under to the chord now there: a chord tone goes to the nearest tone of the new chord (common
//! tones stay, the lower on a tie), any other note moves as the root does, the shortest way (up
//! to a tritone). The new chords become
//! the clip's reference, and the change is one undo step with the chord edit. Notes outside any
//! chord stay. The Band and its players are not touched: they read the lane as before.

use soundcraft_midi::ops::FollowChord;
use soundcraft_model::{ClipContent, ClipId, Session};

/// The chord lane over `clip` as following clips keep it.
pub fn snapshot(s: &Session, clip: &soundcraft_model::Clip) -> Vec<FollowChord> {
    crate::midi_cleanup::chord_spans(s, clip)
        .into_iter()
        .map(|c| FollowChord {
            start: c.start,
            end: c.end,
            root: c.root,
            tones: c.tones.iter().enumerate().fold(0u16, |m, (i, on)| if *on { m | (1 << i) } else { m }),
        })
        .collect()
}

/// Moves every following clip's notes from its reference chords to the lane's chords now, and
/// keeps those as its reference. Returns how many notes moved.
pub fn follow(s: &mut Session) -> usize {
    let ids: Vec<ClipId> = s.tracks.iter().flat_map(|t| t.clips().iter()).filter(|c| c.follow_chords.is_some()).map(|c| c.id).collect();
    let mut moved = 0;
    for id in ids {
        let Some(clip) = s.find_clip(id).map(|(_, c)| c.clone()) else { continue };
        let Some(old) = clip.follow_chords.as_ref() else { continue };
        let new = snapshot(s, &clip);
        let ClipContent::Midi { sequence } = &clip.content else { continue };
        let mut seq = sequence.clone();
        let at = |list: &[FollowChord], t: i64| list.iter().rev().find(|c| c.start <= t && t < c.end).copied();
        for n in seq.notes.iter_mut().filter(|n| n.start >= 0) {
            if let (Some(a), Some(b)) = (at(old, n.start), at(&new, n.start)) {
                let p = soundcraft_midi::ops::follow_chord_pitch(n.pitch, &a, &b);
                if p != n.pitch {
                    n.pitch = p;
                    moved += 1;
                }
            }
        }
        seq.sort();
        if let Some(c) = s.find_clip_mut(id) {
            c.content = ClipContent::Midi { sequence: seq };
            c.follow_chords = Some(new);
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::{ChannelFormat, ClipContent, ClipId, TrackKind};

    fn pitches(e: &Engine, c: u64) -> Vec<u8> {
        match &e.session().find_clip(ClipId(c)).unwrap().1.content {
            ClipContent::Midi { sequence } => sequence.notes.iter().map(|n| n.pitch).collect(),
            _ => panic!("a MIDI clip"),
        }
    }

    #[test]
    fn a_following_clip_moves_with_the_chord_lane_and_others_stay() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let chord = e.execute("band.chord.add", &json!({"at": 0, "end": 96_000, "chord": "C"})).unwrap();
        let id = chord["chord"].clone();
        let a = e.execute("midi.new_clip", &json!({"track": t.0, "start": 0, "end": 96_000})).unwrap()["clip"].as_u64().unwrap();
        let b = e.execute("midi.new_clip", &json!({"track": t.0, "start": 96_000, "end": 192_000})).unwrap()["clip"].as_u64().unwrap();
        for c in [a, b] {
            // C E G and a D passing tone.
            for (p, at) in [(60, 0), (64, 0), (67, 0), (62, 960)] {
                e.execute("midi.note_add", &json!({"clip": c, "pitch": p, "start_ticks": at})).unwrap();
            }
        }
        e.execute("clip.follow_chords", &json!({"clips": [a], "on": true})).unwrap();
        // The chord under clip a becomes A minor: C E G -> C E A (common tones stay), D -> B.
        e.execute("band.chord.set", &json!({"chord": id, "chord_text": "Am"})).unwrap();
        assert_eq!(pitches(&e, a), vec![60, 64, 69, 59]);
        assert_eq!(pitches(&e, b), vec![60, 64, 67, 62], "a clip not following stays");
        // One undo step takes the chord and the notes back.
        assert!(e.undo());
        assert_eq!(pitches(&e, a), vec![60, 64, 67, 62]);
        // Off: the lane no longer moves it.
        e.execute("clip.follow_chords", &json!({"clips": [a], "on": false})).unwrap();
        e.execute("band.chord.set", &json!({"chord": id, "chord_text": "F"})).unwrap();
        assert_eq!(pitches(&e, a), vec![60, 64, 67, 62]);
    }
}
