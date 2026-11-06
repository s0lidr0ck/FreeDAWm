//! Juke pattern clips: keeping each one's notes the expansion of its pattern.
//!
//! A pattern clip is a MIDI clip with a [`PatternClip`] sidecar; its notes are
//! [`soundcraft_midi::pattern::expand`]ed from the pattern. Rather than teach every clip edit
//! about patterns, [`refresh`] runs after every change to the session (in the same undo step) and
//! brings each pattern clip up to date:
//!
//! * The phase follows the clip's start: a clip whose start moved while its end stayed (a trim or
//!   split at the start) keeps its steps where they were in time; a clip moved whole keeps its
//!   phase and takes the pattern with it.
//! * The notes are expanded again when the pattern, the clip's length, its bar length or the tempo
//!   map changed.
//! * Notes that differ from the last expansion while the clip's edges stayed put were edited by
//!   hand (the piano roll, quantize, …): the clip becomes a plain MIDI clip, keeping those notes,
//!   rather than lose the edit. Undo brings the pattern back with the step it undoes. (Edits that
//!   move an edge, such as a split, cut notes too: those clips re-expand.)
//!
//! The scan only reads; the session is copied only when a clip needs a change.

use std::sync::Arc;

use soundcraft_midi::Sequence;
use soundcraft_midi::pattern::{Expand, expand};
use soundcraft_model::{Clip, ClipContent, ClipId, PatternClip, Session};

/// Most notes one pattern clip expands to.
pub const MAX_PATTERN_NOTES: usize = 200_000;

/// FNV-1a over the notes: stable across runs and builds, so a saved session's fingerprint still
/// matches when it opens.
pub fn fingerprint(seq: &Sequence) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    let mut eat = |v: u64| {
        for b in v.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
    };
    eat(seq.notes.len() as u64);
    for n in &seq.notes {
        eat(n.start as u64);
        eat(n.length as u64);
        eat(u64::from(n.pitch) | u64::from(n.velocity) << 8 | u64::from(n.release_velocity) << 16 | u64::from(n.channel) << 24);
    }
    eat(seq.ctrls.len() as u64);
    h
}

/// Where the clip sits in ticks: its start, its length and the bar length there.
fn clip_ticks(s: &Session, start: i64, end: i64) -> (i64, i64, i64) {
    let sr = s.sample_rate;
    let t0 = s.tempo.samples_to_ticks(start, sr);
    let t1 = s.tempo.samples_to_ticks(end, sr);
    (t0, (t1 - t0).max(0), s.tempo.meter_at_tick(t0).ticks_per_bar())
}

/// The phase `pc` has for `clip`, following a change of the clip's start since it was last seen.
pub fn phase_for(s: &Session, clip: &Clip, pc: &PatternClip) -> i64 {
    let Some((s0, e0)) = pc.seen else { return pc.phase };
    let (start, end) = (clip.start, clip.end());
    let moved_whole = start - s0 == end - e0;
    if start == s0 || moved_whole {
        return pc.phase;
    }
    let sr = s.sample_rate;
    let delta = s.tempo.samples_to_ticks(start, sr) - s.tempo.samples_to_ticks(s0, sr);
    pc.phase.saturating_add(delta)
}

/// The notes `pc` expands to in `clip` at `phase`.
pub fn expansion(s: &Session, clip: &Clip, pc: &PatternClip, phase: i64) -> Sequence {
    let (_, length, ticks_per_bar) = clip_ticks(s, clip.start, clip.end());
    expand(&pc.pattern, &Expand { ticks_per_bar, length, phase, max_notes: MAX_PATTERN_NOTES })
}

/// What a pattern clip needs.
enum Fix {
    /// New notes, the phase they were expanded at and their fingerprint.
    Expand(Sequence, i64, u64),
    /// Its notes were edited by hand: it becomes a plain MIDI clip.
    Detach,
}

fn stale(s: &Session) -> Vec<(ClipId, Fix)> {
    let mut out = Vec::new();
    for clip in s.tracks.iter().flat_map(|t| t.clips()) {
        let (Some(pc), ClipContent::Midi { sequence }) = (&clip.pattern, &clip.content) else { continue };
        let fp = fingerprint(sequence);
        // Notes changed while the clip's edges did not: a hand edit. (A split, trim or move
        // changes the edges, and cuts or keeps notes as it goes: the pattern re-expands then.)
        if pc.seen == Some((clip.start, clip.end())) && fp != pc.expanded {
            out.push((clip.id, Fix::Detach));
            continue;
        }
        let phase = phase_for(s, clip, pc);
        let seq = expansion(s, clip, pc, phase);
        let new_fp = fingerprint(&seq);
        if new_fp != fp || phase != pc.phase || pc.seen != Some((clip.start, clip.end())) {
            out.push((clip.id, Fix::Expand(seq, phase, new_fp)));
        }
    }
    out
}

fn apply(s: &mut Session, fixes: Vec<(ClipId, Fix)>) {
    for (id, fix) in fixes {
        let Some(clip) = s.find_clip_mut(id) else { continue };
        let (start, end) = (clip.start, clip.end());
        match fix {
            Fix::Detach => clip.pattern = None,
            Fix::Expand(seq, phase, fp) => {
                clip.content = ClipContent::Midi { sequence: seq };
                if let Some(pc) = clip.pattern.as_mut() {
                    pc.phase = phase;
                    pc.seen = Some((start, end));
                    pc.expanded = fp;
                }
            }
        }
    }
}

/// Brings every pattern clip's notes up to date (see the module docs). Copies the session only
/// when one needs a change; returns whether one did.
pub(crate) fn refresh(doc: &mut Arc<Session>) -> bool {
    let fixes = stale(doc);
    if fixes.is_empty() {
        return false;
    }
    apply(Arc::make_mut(doc), fixes);
    true
}

/// [`refresh`] on a session not shared yet.
pub fn refresh_in(s: &mut Session) {
    let fixes = stale(s);
    apply(s, fixes);
}
