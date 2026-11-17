//! An index of a sequence's notes for playback: whether they are in start order and how long the
//! longest is, so the scheduler can look at the notes near the playhead only (a binary search on
//! their starts) instead of every note of a dense clip.
//!
//! It is derived data, made off the audio thread when the notes change (the engine refreshes it
//! with MIDI Cleanup's previews) and kept beside them. It is only used for the very notes it was
//! made from: [`NoteIndex::window`] checks their address and count, and any other notes (a copy, a
//! cleanup's output, notes edited since) are searched whole, as before.

use crate::Note;
use std::hash::{Hash, Hasher};
use std::ops::Range;

/// The notes of one sequence as the scheduler searches them.
#[derive(Debug, Clone)]
pub struct NoteIndex {
    /// Address and count of the notes it was made from.
    ptr: usize,
    len: usize,
    /// A hash of their starts and lengths, to see an edit that kept the address.
    print: u64,
    /// Starts never decrease (as `Sequence::sort` leaves them).
    sorted: bool,
    /// The longest note, in ticks.
    longest: i64,
}

/// Where a clip keeps its notes' index: derived data, never saved, and left out of comparisons
/// (two slots are always equal), so a session compares as its notes do, indexed or not.
#[derive(Debug, Clone, Default)]
pub struct NoteIndexSlot(pub Option<std::sync::Arc<NoteIndex>>);

impl PartialEq for NoteIndexSlot {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

fn print(notes: &[Note]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for n in notes {
        (n.start, n.length).hash(&mut h);
    }
    h.finish()
}

impl NoteIndex {
    /// The index of `notes`. Reads every note once; allocates nothing.
    pub fn of(notes: &[Note]) -> NoteIndex {
        NoteIndex {
            ptr: notes.as_ptr() as usize,
            len: notes.len(),
            print: print(notes),
            sorted: notes.windows(2).all(|w| w.first().map(|a| a.start) <= w.get(1).map(|b| b.start)),
            longest: notes.iter().map(|n| n.length).max().unwrap_or(0).max(0),
        }
    }

    /// Whether this index was made from `notes` as they are now (their address, count and
    /// starts and lengths). Reads every note: for refreshing, not for the audio thread.
    pub fn is_of(&self, notes: &[Note]) -> bool {
        self.ptr == notes.as_ptr() as usize && self.len == notes.len() && self.print == print(notes)
    }

    /// The longest note's length in ticks.
    pub fn longest(&self) -> i64 {
        self.longest
    }

    /// The notes of `notes` whose start lies in `from..=to` (ticks), when this index was made from
    /// them and they are in start order; every note otherwise. No allocation.
    pub fn window(index: Option<&NoteIndex>, notes: &[Note], from: i64, to: i64) -> Range<usize> {
        match index {
            Some(ix) if ix.sorted && ix.ptr == notes.as_ptr() as usize && ix.len == notes.len() => {
                let a = notes.partition_point(|n| n.start < from);
                let b = notes.partition_point(|n| n.start <= to).max(a);
                a..b
            }
            _ => 0..notes.len(),
        }
    }

    /// The index to use for `notes` (`None`: search them whole).
    pub fn usable<'a>(index: Option<&'a NoteIndex>, notes: &[Note]) -> Option<&'a NoteIndex> {
        index.filter(|ix| ix.sorted && ix.ptr == notes.as_ptr() as usize && ix.len == notes.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(start: i64, length: i64) -> Note {
        Note { probability: 100, pitch: 60, velocity: 100, muted: false, release_velocity: 64, channel: 0, start, length }
    }

    #[test]
    fn the_window_is_the_notes_starting_in_it_and_only_for_the_notes_indexed() {
        let notes = vec![n(0, 10), n(100, 500), n(200, 10), n(200, 20), n(900, 10)];
        let ix = NoteIndex::of(&notes);
        assert_eq!((ix.longest(), ix.is_of(&notes)), (500, true));
        assert_eq!(NoteIndex::window(Some(&ix), &notes, 100, 200), 1..4);
        assert_eq!(NoteIndex::window(Some(&ix), &notes, 201, 899), 4..4);
        let copy = notes.clone();
        assert_eq!(NoteIndex::window(Some(&ix), &copy, 100, 200), 0..5, "other notes: all of them");
        assert!(!ix.is_of(&copy));
        let unsorted = vec![n(50, 1), n(0, 1)];
        assert_eq!(NoteIndex::window(Some(&NoteIndex::of(&unsorted)), &unsorted, 0, 0), 0..2);
        let mut edited = notes;
        if let Some(x) = edited.get_mut(1) {
            x.length = 900;
        }
        assert!(!ix.is_of(&edited), "an edit in place is seen");
    }
}
