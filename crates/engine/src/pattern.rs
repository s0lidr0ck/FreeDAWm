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

/// Recording over a pattern clip with MIDI Merge on: the take's notes become steps of the clip's
/// pattern instead of replacing the clip. A pattern clip (not muted) on `track` must cover the
/// whole of `range`. Each note-on lands on the nearest step of the pattern's grid (its phase
/// kept), on the lane that plays its note (a new lane, as long as the longest, when none does and
/// there is room), at its velocity; a step already on stays as it was (overdub). A lane loops over
/// its own length at its own rate and in its own direction, so a note lands on the step the lane
/// plays where it was played (a Random lane: the step at that position going forward). Returns the clip, `None` when
/// there is no such clip (the take is merged as usual). The clip's notes are expanded again by
/// [`refresh`] with the undo step.
pub fn record_into(s: &mut Session, track: soundcraft_model::TrackId, range: soundcraft_time::Range, take: &Sequence) -> Option<ClipId> {
    use soundcraft_midi::pattern::{Lane, MAX_LANES, Step};
    let clip = s.track(track)?.clips().iter().find(|c| !c.muted && c.pattern.is_some() && c.start <= range.start && c.end() >= range.end)?.clone();
    let pc = clip.pattern.as_ref()?;
    let phase = phase_for(s, &clip, pc);
    let (clip_tick, _, ticks_per_bar) = clip_ticks(s, clip.start, clip.end());
    let grid = ticks_per_bar as f64 / f64::from(pc.pattern.steps_per_bar.max(1));
    if !(grid.is_finite() && grid > 0.0) {
        return None;
    }
    let take_tick = s.tempo.samples_to_ticks(range.start, s.sample_rate);
    let mut pattern = pc.pattern.clone();
    let width = pattern.lanes.iter().map(|l| l.steps.len()).max().unwrap_or(16);
    for n in take.notes.iter().filter(|n| n.start >= 0 && !n.muted) {
        let at = take_tick.saturating_add(n.start).saturating_sub(clip_tick).saturating_add(phase);
        let lane = match pattern.lanes.iter().position(|l| l.note == n.pitch) {
            Some(i) => i,
            None if pattern.lanes.len() < MAX_LANES => {
                pattern.lanes.push(Lane::new(n.pitch, width));
                pattern.lanes.len() - 1
            }
            None => continue,
        };
        let Some(l) = pattern.lanes.get_mut(lane) else { continue };
        // The nearest step on the lane's own grid (half or double rate), and the step that plays
        // there (a reversed or ping-pong lane plays its steps in another order).
        let k = (at as f64 / soundcraft_midi::pattern::step_ticks(l, grid)).round() as i64;
        let i = soundcraft_midi::pattern::step_at(l, k);
        if let Some(step) = l.steps.get_mut(i)
            && !step.on
        {
            *step = Step::hit(n.velocity.clamp(1, 127));
        }
    }
    let c = s.find_clip_mut(clip.id)?;
    if let Some(pc) = c.pattern.as_mut() {
        pc.pattern = pattern;
    }
    Some(clip.id)
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
