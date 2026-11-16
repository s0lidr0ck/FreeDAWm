//! Shared (alias) MIDI clips: clips with the same [`Clip::shared`] id play the same notes and
//! controllers. After each command, [`follow`] keeps them so:
//!
//! - A note edit (a `midi.*`, `event.*`, `clip.midi_reverse` or `clip.play_quantize` command)
//!   that changed one member's notes gives every other member of its group the same notes, in the
//!   same undo step.
//! - Any other command that changed a member's notes (a start trim that hid notes, a split, a
//!   consolidate, a cleanup commit) leaves that member on its own: it is no longer shared.
//! - A clip that joins a group (a Duplicate or paste of a shared clip, Duplicate as Shared Copy)
//!   stays in it when it holds the group's notes; otherwise it is on its own.
//! - A group left with one member is no group: that clip is independent again.
//!
//! Each member keeps its own place, length, mute, transpose and the rest: only the notes and
//! controllers are shared.

use soundcraft_midi::Sequence;
use soundcraft_model::{ClipContent, ClipId, Session};
use std::collections::HashMap;

/// Whether any clip of `s` is shared (the check is skipped when none is).
pub fn any(s: &Session) -> bool {
    s.tracks.iter().any(|t| t.clips().iter().any(|c| c.shared.is_some()))
}

/// Whether command `id` edits notes in place (its change to a shared clip is made in all).
pub fn edits_notes(id: &str) -> bool {
    id.starts_with("midi.") || id.starts_with("event.") || matches!(id, "clip.midi_reverse" | "clip.play_quantize")
}

fn seq_of(c: &soundcraft_model::Clip) -> Option<&Sequence> {
    match &c.content {
        ClipContent::Midi { sequence } => Some(sequence),
        _ => None,
    }
}

/// Brings the shared clips of `s` (the session after command `id`) back in step: see the module
/// docs. `before` is the session before the command.
pub fn follow(id: &str, before: &Session, s: &mut Session) {
    let editing = edits_notes(id);
    // Before: each shared clip's group and notes, and a member's notes per group.
    let mut old: HashMap<ClipId, (u64, &Sequence)> = HashMap::new();
    let mut rep: HashMap<u64, &Sequence> = HashMap::new();
    for c in before.tracks.iter().flat_map(|t| t.clips()) {
        if let (Some(g), Some(q)) = (c.shared, seq_of(c)) {
            old.insert(c.id, (g, q));
            rep.entry(g).or_insert(q);
        }
    }
    let mut source: HashMap<u64, Sequence> = HashMap::new();
    let mut alone: Vec<ClipId> = Vec::new();
    for c in s.tracks.iter().flat_map(|t| t.clips()) {
        let Some(g) = c.shared else { continue };
        let Some(q) = seq_of(c) else {
            alone.push(c.id);
            continue;
        };
        match old.get(&c.id) {
            Some((g0, q0)) if *g0 == g => {
                if *q0 != q {
                    if editing {
                        source.entry(g).or_insert_with(|| q.clone());
                    } else {
                        alone.push(c.id);
                    }
                }
            }
            // Joined the group: it stays when it holds the group's notes (or starts the group).
            _ => {
                if rep.get(&g).is_some_and(|r| *r != q) {
                    alone.push(c.id);
                }
            }
        }
    }
    for t in &mut s.tracks {
        let Some(pl) = t.playlist_mut() else { continue };
        for c in &mut pl.clips {
            if alone.contains(&c.id) {
                c.shared = None;
                continue;
            }
            if let Some(src) = c.shared.and_then(|g| source.get(&g))
                && let ClipContent::Midi { sequence } = &mut c.content
                && sequence != src
            {
                *sequence = src.clone();
            }
        }
    }
    // A group of one is no group.
    let mut count: HashMap<u64, usize> = HashMap::new();
    for c in s.tracks.iter().flat_map(|t| t.clips()) {
        if let Some(g) = c.shared {
            *count.entry(g).or_default() += 1;
        }
    }
    for t in &mut s.tracks {
        let Some(pl) = t.playlist_mut() else { continue };
        for c in &mut pl.clips {
            if c.shared.is_some_and(|g| count.get(&g).copied().unwrap_or(0) < 2) {
                c.shared = None;
            }
        }
    }
}
