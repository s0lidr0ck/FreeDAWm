//! An index of a sequence's notes for playback: whether they are in start order and how long the
//! longest is, so the scheduler can look at the notes near the playhead only (a binary search on
//! their starts) instead of every note of a dense clip.
//!
//! It is derived data, made off the audio thread when the notes change (the engine refreshes it
//! with MIDI Cleanup's previews) and kept beside them. It is only used for the very notes it was
//! made from: [`NoteIndex::window`] checks their address and count, and any other notes (a copy, a
//! cleanup's output, notes edited since) are searched whole, as before.
//!
//! Made from a whole sequence ([`NoteIndex::of_sequence`]) it indexes the controllers too: whether
//! they are in tick order and the longest ramp (from a ramp point to the next point of its lane),
//! so the scheduler looks only at the controllers that can send something in the block
//! ([`NoteIndex::ctrl_window`]).

use crate::{CtrlEvent, CtrlKind, Note, Sequence};
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
    /// The controllers', when made from a whole sequence.
    ctrls: Option<CtrlPart>,
}

/// What a [`NoteIndex`] knows of the controllers it was made from.
#[derive(Debug, Clone)]
struct CtrlPart {
    ptr: usize,
    len: usize,
    /// A hash of their ticks, lanes and shapes.
    print: u64,
    /// Ticks never decrease (as `Sequence::sort` leaves them).
    sorted: bool,
    /// The longest span from a ramp point to the next point of its lane, in ticks.
    longest_ramp: i64,
}

fn lane_key(c: &CtrlEvent) -> (u8, u8, u8) {
    let (k, n) = match c.kind {
        CtrlKind::Cc(n) => (0, n),
        CtrlKind::PitchBend => (1, 0),
        CtrlKind::ChannelPressure => (2, 0),
        CtrlKind::PolyPressure(n) => (3, n),
        CtrlKind::Program => (4, 0),
    };
    (c.channel, k, n)
}

fn ctrl_print(ctrls: &[CtrlEvent]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for c in ctrls {
        (c.tick, lane_key(c), c.shape.is_step()).hash(&mut h);
    }
    h.finish()
}

impl CtrlPart {
    fn of(ctrls: &[CtrlEvent]) -> CtrlPart {
        // The tick of each lane's last point, when it is a ramp.
        let mut open: std::collections::HashMap<(u8, u8, u8), i64> = std::collections::HashMap::new();
        let mut longest_ramp = 0i64;
        for c in ctrls {
            let key = lane_key(c);
            if let Some(from) = open.remove(&key) {
                longest_ramp = longest_ramp.max(c.tick.saturating_sub(from));
            }
            if !c.shape.is_step() {
                open.insert(key, c.tick);
            }
        }
        CtrlPart {
            ptr: ctrls.as_ptr() as usize,
            len: ctrls.len(),
            print: ctrl_print(ctrls),
            sorted: ctrls.windows(2).all(|w| w.first().map(|a| a.tick) <= w.get(1).map(|b| b.tick)),
            longest_ramp: longest_ramp.max(0),
        }
    }
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
            ctrls: None,
        }
    }

    /// The index of a sequence's notes and controllers. Reads every event once.
    pub fn of_sequence(seq: &Sequence) -> NoteIndex {
        NoteIndex { ctrls: Some(CtrlPart::of(&seq.ctrls)), ..NoteIndex::of(&seq.notes) }
    }

    /// Whether this index was made by [`NoteIndex::of_sequence`] from `seq` as it is now.
    pub fn is_of_sequence(&self, seq: &Sequence) -> bool {
        self.is_of(&seq.notes)
            && self
                .ctrls
                .as_ref()
                .is_some_and(|c| c.ptr == seq.ctrls.as_ptr() as usize && c.len == seq.ctrls.len() && c.print == ctrl_print(&seq.ctrls))
    }

    /// The controllers of `ctrls` that can send something while ticks `from..=to` play: those
    /// whose tick lies there, and ramp points before it whose ramp can reach into it. Only when
    /// this index was made from them (with [`NoteIndex::of_sequence`]) and they are in tick order;
    /// every controller otherwise. No allocation.
    pub fn ctrl_window(index: Option<&NoteIndex>, ctrls: &[CtrlEvent], from: i64, to: i64) -> Range<usize> {
        match index.and_then(|ix| ix.ctrls.as_ref()) {
            Some(c) if c.sorted && c.ptr == ctrls.as_ptr() as usize && c.len == ctrls.len() => {
                let from = from.saturating_sub(c.longest_ramp);
                let a = ctrls.partition_point(|e| e.tick < from);
                let b = ctrls.partition_point(|e| e.tick <= to).max(a);
                a..b
            }
            _ => 0..ctrls.len(),
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

    #[test]
    fn the_controller_window_reaches_back_by_the_longest_ramp() {
        let c = |tick: i64, kind: CtrlKind, shape: crate::CtrlShape| CtrlEvent { tick, channel: 0, kind, value: 0, shape };
        let ramp = crate::CtrlShape::Ramp(0.0);
        let step = crate::CtrlShape::Step;
        let seq = Sequence {
            ctrls: vec![
                c(0, CtrlKind::Cc(1), ramp),
                c(100, CtrlKind::Cc(7), step),
                c(300, CtrlKind::Cc(1), step),
                c(400, CtrlKind::Cc(7), ramp),
                c(450, CtrlKind::Cc(7), step),
                c(1000, CtrlKind::PitchBend, ramp),
            ],
            ..Default::default()
        };
        let ix = NoteIndex::of_sequence(&seq);
        assert!(ix.is_of_sequence(&seq));
        // The longest ramp is 300 ticks (CC 1, 0 to 300); the bend's has no next point.
        assert_eq!(NoteIndex::ctrl_window(Some(&ix), &seq.ctrls, 310, 420), 1..4);
        assert_eq!(NoteIndex::ctrl_window(Some(&ix), &seq.ctrls, 900, 950), 5..5);
        assert_eq!(NoteIndex::ctrl_window(Some(&ix), &seq.ctrls, 600, 700), 2..5);
        let notes_only = NoteIndex::of(&seq.notes);
        assert_eq!(NoteIndex::ctrl_window(Some(&notes_only), &seq.ctrls, 600, 700), 0..6, "not indexed: all");
        assert!(!notes_only.is_of_sequence(&seq));
        let copy = seq.clone();
        assert_eq!(NoteIndex::ctrl_window(Some(&ix), &copy.ctrls, 600, 700), 0..6);
        let mut edited = seq;
        if let Some(x) = edited.ctrls.get_mut(2) {
            x.shape = ramp;
        }
        assert!(!ix.is_of_sequence(&edited), "a shape changed in place is seen");
    }
}
