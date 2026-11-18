//! Per-note expression (MPE): a note's tuning, pressure and brightness over its length.
//!
//! The model is the one MPE itself uses: a note with expression sounds on a channel of its own,
//! and that channel's pitch bend (tuning), channel pressure and CC 74 (brightness) while it sounds
//! are its expression. [`expression`] reads them as points from the note's start; [`set`] writes
//! them, replacing what the channel held over the note; [`own_channels`] gives each of a clip's
//! notes a member channel of its own (2-16, the lower MPE zone; the channel used longest ago
//! first), so a plain clip can carry per-note expression. Recording an MPE controller keeps its
//! channels as played (section 6), so what it recorded reads back here unchanged.

use crate::{CtrlEvent, CtrlKind, CtrlShape, Sequence};

/// What a note's expression moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExprKind {
    /// Pitch bend: centred, -8192..=8191.
    Tuning,
    /// Channel pressure 0..=127.
    Pressure,
    /// CC 74 0..=127 (MPE's timbre).
    Brightness,
}

impl ExprKind {
    pub const ALL: [ExprKind; 3] = [ExprKind::Tuning, ExprKind::Pressure, ExprKind::Brightness];

    pub fn ctrl(self) -> CtrlKind {
        match self {
            ExprKind::Tuning => CtrlKind::PitchBend,
            ExprKind::Pressure => CtrlKind::ChannelPressure,
            ExprKind::Brightness => CtrlKind::Cc(74),
        }
    }

    /// The values it takes.
    pub fn range(self) -> (i32, i32) {
        match self {
            ExprKind::Tuning => (-8192, 8191),
            _ => (0, 127),
        }
    }

    /// The value it rests at when nothing moves it (centre bend, no pressure, CC 74 centre).
    pub fn rest(self) -> i32 {
        match self {
            ExprKind::Tuning | ExprKind::Pressure => 0,
            ExprKind::Brightness => 64,
        }
    }
}

/// Whether another note shares note `index`'s channel while it sounds (then its channel's
/// controllers are not its own).
pub fn shares_channel(seq: &Sequence, index: usize) -> bool {
    let Some(n) = seq.notes.get(index) else { return false };
    seq.notes.iter().enumerate().any(|(i, m)| i != index && m.channel == n.channel && m.start < n.end() && n.start < m.end())
}

/// Note `index`'s expression of `kind`: the points (ticks from its start, value) its channel
/// holds while it sounds, the value in force at its start first (when one is).
pub fn expression(seq: &Sequence, index: usize, kind: ExprKind) -> Vec<(i64, i32)> {
    let Some(n) = seq.notes.get(index) else { return Vec::new() };
    let ck = kind.ctrl();
    let lane = || seq.ctrls.iter().filter(|c| c.channel == n.channel && c.kind == ck);
    let mut out = Vec::new();
    if let Some(before) = lane().filter(|c| c.tick <= n.start).max_by_key(|c| c.tick) {
        out.push((0, before.value));
    }
    for c in lane().filter(|c| c.tick > n.start && c.tick < n.end()) {
        out.push((c.tick - n.start, c.value));
    }
    out
}

/// Writes note `index`'s expression of `kind`: `points` (ticks from its start, values clamped to
/// the kind's range) replace what its channel held over the note, and the value goes back to
/// rest at its end when nothing follows there. `Err` when the note shares its channel (its
/// channel's controllers are not its own: [`own_channels`] first) or there is no such note.
pub fn set(seq: &mut Sequence, index: usize, kind: ExprKind, points: &[(i64, i32)]) -> Result<(), &'static str> {
    let n = *seq.notes.get(index).ok_or("no such note")?;
    if shares_channel(seq, index) {
        return Err("the note shares its channel with another note: give the notes their own channels first");
    }
    let ck = kind.ctrl();
    let (lo, hi) = kind.range();
    let (a, b) = (n.start, n.end());
    seq.ctrls.retain(|c| !(c.channel == n.channel && c.kind == ck && c.tick >= a && c.tick < b));
    for (t, v) in points.iter().take(4096) {
        let tick = a.saturating_add((*t).clamp(0, (b - a - 1).max(0)));
        seq.ctrls.push(CtrlEvent { tick, channel: n.channel, kind: ck, value: (*v).clamp(lo, hi), shape: CtrlShape::default() });
    }
    // Back to rest at the end, unless the channel says something there already.
    let at_end = seq.ctrls.iter().any(|c| c.channel == n.channel && c.kind == ck && c.tick == b);
    if !points.is_empty() && !at_end {
        seq.ctrls.push(CtrlEvent { tick: b, channel: n.channel, kind: ck, value: kind.rest(), shape: CtrlShape::default() });
    }
    seq.sort();
    Ok(())
}

/// Gives each note in `indices` (every note when empty) a member channel of its own, 2-16
/// (index 1-15): the one free longest. Returns how many notes changed channel. Controllers on the
/// old channel stay where they were; a note's expression moves with it only when it had its
/// channel to itself (its tuning, pressure and brightness while it sounds go along).
pub fn own_channels(seq: &mut Sequence, indices: &[usize]) -> usize {
    let mut order: Vec<usize> = if indices.is_empty() { (0..seq.notes.len()).collect() } else { indices.to_vec() };
    order.retain(|i| *i < seq.notes.len());
    order.sort_by_key(|i| seq.notes.get(*i).map_or((0, 0), |n| (n.start, i64::from(n.pitch))));
    order.dedup();
    // When each member channel is free again.
    let mut free_at = [i64::MIN; 15];
    let mut changed = 0;
    for i in order {
        let Some(n) = seq.notes.get(i).copied() else { continue };
        let k = (0..15)
            .filter(|k| free_at[*k] <= n.start)
            .min_by_key(|k| free_at[*k])
            .unwrap_or_else(|| (0..15).min_by_key(|k| free_at[*k]).unwrap_or(0));
        free_at[k] = n.end();
        let ch = (k + 1) as u8;
        if n.channel == ch {
            continue;
        }
        let alone = !shares_channel(seq, i);
        if alone {
            for c in seq.ctrls.iter_mut().filter(|c| c.channel == n.channel && c.tick > n.start && c.tick < n.end()) {
                if matches!(c.kind, CtrlKind::PitchBend | CtrlKind::ChannelPressure | CtrlKind::Cc(74)) {
                    c.channel = ch;
                }
            }
        }
        if let Some(m) = seq.notes.get_mut(i) {
            m.channel = ch;
            changed += 1;
        }
    }
    seq.sort();
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Note;

    fn note(start: i64, length: i64, pitch: u8, channel: u8) -> Note {
        Note { probability: 100, start, length, pitch, velocity: 100, release_velocity: 64, channel, muted: false }
    }

    #[test]
    fn a_notes_expression_is_its_channels_controllers_while_it_sounds() {
        let mut seq = Sequence { notes: vec![note(0, 960, 60, 1), note(480, 960, 64, 2)], ..Sequence::default() };
        set(&mut seq, 0, ExprKind::Tuning, &[(0, 0), (480, 4096), (2000, 8191)]).unwrap();
        assert_eq!(expression(&seq, 0, ExprKind::Tuning), vec![(0, 0), (480, 4096), (959, 8191)], "a point past the end lands on its last tick");
        // The bend comes back to the centre where the note ends; the other note is untouched.
        assert!(seq.ctrls.iter().any(|c| c.tick == 960 && c.channel == 1 && c.value == 0));
        assert!(expression(&seq, 1, ExprKind::Tuning).is_empty());
        set(&mut seq, 1, ExprKind::Brightness, &[(0, 200)]).unwrap();
        assert_eq!(expression(&seq, 1, ExprKind::Brightness), vec![(0, 127)], "clamped to the range");
        // Replacing: a second write takes the first one's place.
        set(&mut seq, 0, ExprKind::Tuning, &[(100, -100)]).unwrap();
        assert_eq!(expression(&seq, 0, ExprKind::Tuning), vec![(100, -100)]);
    }

    #[test]
    fn notes_sharing_a_channel_get_their_own_first() {
        let mut seq = Sequence { notes: vec![note(0, 960, 60, 0), note(0, 960, 64, 0), note(960, 960, 67, 0)], ..Sequence::default() };
        assert!(set(&mut seq, 0, ExprKind::Pressure, &[(0, 90)]).is_err());
        assert_eq!(own_channels(&mut seq, &[]), 3);
        let chans: Vec<u8> = seq.notes.iter().map(|n| n.channel).collect();
        assert!(chans.iter().all(|c| (1..=15).contains(c)));
        assert_ne!(chans[0], chans[1], "two notes at once on two channels");
        assert!((0..seq.notes.len()).all(|i| !shares_channel(&seq, i)));
        assert!(set(&mut seq, 0, ExprKind::Pressure, &[(0, 90)]).is_ok());
        // Done again, nothing moves.
        assert_eq!(own_channels(&mut seq, &[]), 0);
    }
}
