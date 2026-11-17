//! Keeps each MIDI clip's [`NoteIndex`] (`Clip::midi_index`) made from the notes it plays, so the
//! mix scheduler looks only at the notes (and controllers) near the playhead of a dense clip. Derived data, made
//! here off the audio thread: after a command or an undo step and when a session is opened. A
//! clip whose index is missing or out of date is searched whole by the scheduler, as before.

use soundcraft_midi::NoteIndex;
use soundcraft_model::{Clip, Session};
use std::sync::Arc;

fn stale(c: &Clip) -> bool {
    match c.midi_playing() {
        Some(seq) => c.midi_index.0.as_ref().is_none_or(|ix| !ix.is_of_sequence(seq)),
        None => c.midi_index.0.is_some(),
    }
}

/// Whether some clip's index is missing or was made from other notes (reads every note).
pub fn any_stale(s: &Session) -> bool {
    s.tracks.iter().flat_map(|t| t.playlists.iter()).flat_map(|p| p.clips.iter()).any(stale)
}

/// Makes every stale index anew from the notes as they are in `s` (after any copy of them).
pub fn refresh(s: &mut Session) {
    for c in s.tracks.iter_mut().flat_map(|t| t.playlists.iter_mut()).flat_map(|p| p.clips.iter_mut()) {
        if stale(c) {
            c.midi_index.0 = c.midi_playing().map(|seq| Arc::new(NoteIndex::of_sequence(seq)));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::{ChannelFormat, ClipId, TrackKind};

    fn fresh(e: &Engine, c: u64) -> bool {
        let clip = &e.session().find_clip(ClipId(c)).unwrap().1;
        let notes = &clip.midi_playing().unwrap().notes;
        clip.midi_index.0.as_ref().is_some_and(|ix| ix.is_of(notes) && ix.is_of_sequence(clip.midi_playing().unwrap()))
    }

    #[test]
    fn every_edit_leaves_the_index_made_from_the_notes() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let c = e.execute("midi.new_clip", &json!({"track": t.0, "start": 0, "end": 96_000})).unwrap()["clip"].as_u64().unwrap();
        for start in [0, 480, 960] {
            e.execute("midi.note_add", &json!({"clip": c, "pitch": 60, "start_ticks": start, "length_ticks": 240})).unwrap();
        }
        assert!(fresh(&e, c));
        // Kept by the player while edited: the edit copies the notes, the index follows.
        let playing = e.session_arc();
        e.execute("midi.note_edit", &json!({"clip": c, "index": 1, "length_ticks": 2000})).unwrap();
        assert!(fresh(&e, c));
        assert_eq!(e.session().find_clip(ClipId(c)).unwrap().1.midi_index.0.as_ref().unwrap().longest(), 2000);
        drop(playing);
        assert!(e.undo());
        assert!(fresh(&e, c), "undo brings back the step with its index");
        // A session opened has its indexes made.
        let s = e.session().clone();
        let mut e2 = Engine::default();
        e2.replace_session(s);
        assert!(fresh(&e2, c));
    }
}
