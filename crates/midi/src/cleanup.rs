//! MIDI cleanup: tidies a real pianist's transcribed take without sterilising it.
//!
//! [`cleanup`] removes wrong notes and moves note *ends* (cutting notes held over a chord change,
//! closing or trimming small legato gaps and overlaps, extending notes played too short), and
//! changes the sustain pedal (CC64) at chord changes the way a pianist does: up at the new chord's
//! attack, down again just after it. It never changes a note's start, velocity or pitch.
//! The reference for "right" is the chord track ([`ChordSpan`]s) and the key. The function is pure
//! and UI-free: the engine derives a clip's cleaned notes from its original notes, the chord track,
//! the key and the clip's [`CleanupSettings`], so the cleanup stays adjustable until committed.
//!
//! Stages, in order: Wrong notes → Held over chord change → Legato → Extend.

use serde::{Deserialize, Serialize};

use crate::{CtrlEvent, CtrlKind, Note, PPQ, Sequence};

/// Tempo assumed when the caller's `ticks_per_ms` is unusable (120 bpm at [`PPQ`]).
const FALLBACK_TICKS_PER_MS: f64 = 1.92;
/// Notes of one hand starting within this many ms of each other are one onset group (a chord).
const ONSET_MS: f64 = 40.0;
/// Hands are split per window of this many ticks (two beats).
const HAND_WINDOW: i64 = 2 * PPQ as i64;
/// Split pitch when a window gives no usable gap: notes from middle C up are the upper hand.
const SPLIT_FALLBACK: u64 = 60;
/// The smallest pitch gap (semitones) that separates the hands; a step (1–2) never does, so a
/// stepwise line stays in one hand.
const MIN_HAND_GAP: usize = 3;
/// Protect melody's top-voice rule never covers a note outside the key that is foreign to the
/// chords for at least this share of its length: such a top note is the right hand's slip, not
/// the melody (tuned on the test material, where it was the only kind of slip the rule kept).
const TOP_FOREIGN_SHARE: f64 = 0.5;
/// Held over at 0 %: only overhangs past this share of the new chord's length qualify.
const HELD_SHARE: f64 = 0.25;
/// Held over: the largest pull-back before the change (reached at 100 %, from 75 %).
const HELD_PULL_MS: f64 = 30.0;
/// The new chord's attack is its earliest note within this many ms of the change.
const ATTACK_MS: f64 = 120.0;
/// A player's pedal re-press this many ms before the change, or later, is moved after it.
const REPRESS_MS: f64 = 250.0;
/// The pedal goes down again this many ms after the new chord's attack (at most half the chord).
const PEDAL_CATCH_MS: f64 = 60.0;
/// The sustain pedal controller; a value from 64 up is down.
const SUSTAIN_CC: u8 = 64;

/// Per-clip cleanup amounts (0–100 %; 0 = no change) and the melody protection switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct CleanupSettings {
    /// Wrong notes: how far in (towards in-key, long, loud notes) removal reaches.
    pub wrong: u8,
    /// Held over chord change: how far past a change a foreign note (or the pedal) may ring.
    pub held: u8,
    /// Legato: the largest gap/overlap fixed, up to an 8th note at 100.
    pub legato: u8,
    /// Extend short notes: the length below which a note is extended, up to a 16th note at 100.
    pub extend: u8,
    /// Never remove the top voice, passing notes or sus2/sus4 colour tones.
    pub protect_melody: bool,
}

impl Default for CleanupSettings {
    fn default() -> Self {
        Self { wrong: 40, held: 70, legato: 30, extend: 40, protect_melody: true }
    }
}

impl CleanupSettings {
    /// The settings with every amount capped at 100.
    pub fn clamped(self) -> Self {
        Self { wrong: self.wrong.min(100), held: self.held.min(100), legato: self.legato.min(100), extend: self.extend.min(100), ..self }
    }
}

/// One chord of the chord track, in ticks: `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChordSpan {
    pub start: i64,
    pub end: i64,
    /// Root pitch class 0..=11.
    pub root: u8,
    /// The chord's pitch classes, including the bass note's.
    pub tones: [bool; 12],
}

/// Everything [`cleanup`] reads.
pub struct CleanupInput<'a> {
    /// The original notes.
    pub notes: &'a [Note],
    /// The original controller events (copied through).
    pub ctrls: &'a [CtrlEvent],
    /// The chord track (any order; empty, inverted and overlapping spans are tolerated).
    pub chords: &'a [ChordSpan],
    /// The key's scale pitch classes.
    pub key_tones: [bool; 12],
    /// Ticks per millisecond at the session tempo (not finite or ≤ 0 → 120 bpm).
    pub ticks_per_ms: f64,
    pub settings: CleanupSettings,
}

/// What a stage did to a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    WrongNote,
    HeldOver,
    LegatoTrim,
    LegatoClose,
    Extended,
    PedalCatch,
}

/// One changed note: `index` into [`CleanupInput::notes`], the stage that first changed it, its
/// original end and its final end (`None` = removed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteEdit {
    pub index: usize,
    pub kind: EditKind,
    pub old_end: i64,
    pub new_end: Option<i64>,
}

/// Counts for the panel's summary line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CleanupReport {
    pub total: usize,
    pub removed: usize,
    pub held_trimmed: usize,
    pub legato_trimmed: usize,
    pub legato_closed: usize,
    pub extended: usize,
    pub pedal_changes: usize,
}

impl CleanupReport {
    /// "Removed 6 · trimmed 14 · extended 3 of 412 notes", plus " · pedal changes N" when any.
    pub fn summary(&self) -> String {
        let trimmed = self.held_trimmed + self.legato_trimmed;
        let extended = self.extended + self.legato_closed;
        let mut s = format!("Removed {} · trimmed {trimmed} · extended {extended} of {} notes", self.removed, self.total);
        if self.pedal_changes > 0 {
            s.push_str(&format!(" · pedal changes {}", self.pedal_changes));
        }
        s
    }
}

/// The cleaned sequence, the per-note edits (sorted by `index`, at most one per note) and counts.
#[derive(Debug, Clone, PartialEq)]
pub struct CleanupOutput {
    pub sequence: Sequence,
    pub edits: Vec<NoteEdit>,
    pub report: CleanupReport,
}

/// Cleans `input.notes` (see the module docs). Never panics; hostile input is sanitised.
pub fn cleanup(input: &CleanupInput) -> CleanupOutput {
    let tpm = input.ticks_per_ms;
    let ctx = Ctx {
        chords: sanitize_chords(input.chords),
        key_tones: input.key_tones,
        ticks_per_ms: if tpm.is_finite() && tpm > 0.0 { tpm } else { FALLBACK_TICKS_PER_MS },
        settings: input.settings.clamped(),
    };
    let mut work: Vec<Slot> = Vec::with_capacity(input.notes.len());
    work.extend(input.notes.iter().enumerate().map(|(index, n)| {
        let note = Note { length: n.length.max(1), ..*n };
        Slot { index, note, end: Some(note.end()), kind: None }
    }));
    work.sort_by_key(|s| (s.note.start, s.note.pitch, s.index));

    wrong_notes(&mut work, &ctx);
    let pedal = held_over(&mut work, &ctx, input.ctrls);
    let voices = Voices::new(&work, ctx.ticks_per_ms);
    legato(&mut work, &voices, &ctx);
    extend(&mut work, &voices, &ctx);
    same_pitch_stop(&mut work);

    finish(input, work, pedal)
}

struct Ctx {
    /// Sorted by start, non-overlapping, each `end > start`.
    chords: Vec<ChordSpan>,
    key_tones: [bool; 12],
    ticks_per_ms: f64,
    settings: CleanupSettings,
}

/// A note being cleaned: its index in the input, the (sanitised) original, its current end
/// (`None` = removed) and the first stage that changed it.
#[derive(Debug, Clone, Copy)]
struct Slot {
    index: usize,
    note: Note,
    end: Option<i64>,
    kind: Option<EditKind>,
}

impl Slot {
    fn set_end(&mut self, end: Option<i64>, kind: EditKind) {
        if self.end != end {
            self.kind.get_or_insert(kind);
            self.end = end;
        }
    }
}

/// Rounded, never negative.
fn ms_to_ticks(ms: f64, ticks_per_ms: f64) -> i64 {
    let t = (ms * ticks_per_ms).round();
    if t.is_nan() || t <= 0.0 {
        0
    } else if t >= i64::MAX as f64 {
        i64::MAX
    } else {
        t as i64
    }
}

fn pc(pitch: u8) -> usize {
    usize::from(pitch % 12)
}

/// Drops empty spans, sorts by start and clips each end to the next span's start.
fn sanitize_chords(chords: &[ChordSpan]) -> Vec<ChordSpan> {
    let mut v: Vec<ChordSpan> = chords.iter().copied().filter(|c| c.end > c.start).collect();
    v.sort_by_key(|c| c.start);
    let mut next_start = i64::MAX;
    for c in v.iter_mut().rev() {
        c.end = c.end.min(next_start);
        next_start = c.start;
    }
    v.retain(|c| c.end > c.start);
    v
}

/// The chord sounding at `t`, if any.
fn chord_at(chords: &[ChordSpan], t: i64) -> Option<&ChordSpan> {
    let i = chords.partition_point(|c| c.start <= t);
    i.checked_sub(1).and_then(|i| chords.get(i)).filter(|c| t < c.end)
}

/// Spans overlapping `[from, to)`, in order.
fn chords_over(chords: &[ChordSpan], from: i64, to: i64) -> impl Iterator<Item = &ChordSpan> {
    chords.iter().skip(chords.partition_point(|c| c.end <= from)).take_while(move |c| c.start < to)
}

/// The share of `[start, end)` spent over chords that lack the pitch's class.
fn foreign_share(chords: &[ChordSpan], pitch: u8, start: i64, end: i64) -> f64 {
    let len = end.saturating_sub(start);
    if len <= 0 {
        return 0.0;
    }
    let foreign: f64 =
        chords_over(chords, start, end).filter(|c| !c.tones[pc(pitch)]).map(|c| c.end.min(end).saturating_sub(c.start.max(start)) as f64).sum();
    foreign / len as f64
}

/// Whether sounding `[from, to)` would carry the note into a chord that starts after the note
/// does and lacks its pitch class.
fn crosses_foreign_change(chords: &[ChordSpan], note: &Note, from: i64, to: i64) -> bool {
    chords_over(chords, from, to).any(|c| c.start > note.start && !c.tones[pc(note.pitch)])
}

/// Notes of one hand starting together: `Voices::order[first..first + len]`.
struct Group {
    upper: bool,
    /// The earliest start in the group.
    start: i64,
    first: usize,
    len: usize,
}

/// The live notes split into hands and onset groups. Each hand's groups in start order are its
/// voice; the lower hand's groups come first in `groups`, then the upper hand's.
struct Voices {
    /// Slot positions, lower hand then upper hand, each in start order.
    order: Vec<usize>,
    groups: Vec<Group>,
    /// Slot position → index into `groups` (`usize::MAX` for removed notes).
    group_of: Vec<usize>,
}

impl Voices {
    /// `work` must be sorted by start.
    fn new(work: &[Slot], ticks_per_ms: f64) -> Self {
        let live: Vec<usize> = work.iter().enumerate().filter(|(_, s)| s.end.is_some()).map(|(p, _)| p).collect();
        let upper = hand_split(work, &live);
        let mut order = Vec::with_capacity(live.len());
        order.extend(live.iter().zip(&upper).filter(|(_, u)| !**u).map(|(p, _)| *p));
        let lower_len = order.len();
        order.extend(live.iter().zip(&upper).filter(|(_, u)| **u).map(|(p, _)| *p));

        let window = ms_to_ticks(ONSET_MS, ticks_per_ms);
        let mut groups: Vec<Group> = Vec::with_capacity(order.len());
        let mut group_of = vec![usize::MAX; work.len()];
        for (k, &p) in order.iter().enumerate() {
            let Some(slot) = work.get(p) else { continue };
            let up = k >= lower_len;
            let start = slot.note.start;
            match groups.last_mut() {
                Some(g) if g.upper == up && start.saturating_sub(g.start) <= window => g.len += 1,
                _ => groups.push(Group { upper: up, start, first: k, len: 1 }),
            }
            if let Some(slot_group) = group_of.get_mut(p) {
                *slot_group = groups.len() - 1;
            }
        }
        Self { order, groups, group_of }
    }

    fn members(&self, g: &Group) -> &[usize] {
        self.order.get(g.first..g.first.saturating_add(g.len)).unwrap_or(&[])
    }

    /// The next onset group of the same hand.
    fn next(&self, g: usize) -> Option<&Group> {
        let this = self.groups.get(g)?;
        self.groups.get(g.checked_add(1)?).filter(|n| n.upper == this.upper)
    }

    /// The previous onset group of the same hand.
    fn prev(&self, g: usize) -> Option<&Group> {
        let this = self.groups.get(g)?;
        self.groups.get(g.checked_sub(1)?).filter(|n| n.upper == this.upper)
    }
}

/// Upper hand or not, for each of `live` (slot positions in start order). Per two-beat window the
/// split is the middle of the largest gap between the distinct pitches starting in it; a window
/// without a gap of at least [`MIN_HAND_GAP`] is one hand, upper when its mean pitch is from middle
/// C up.
fn hand_split(work: &[Slot], live: &[usize]) -> Vec<bool> {
    let pitch_of = |p: &usize| work.get(*p).map_or(0, |s| s.note.pitch);
    let window_of = |p: &usize| work.get(*p).map_or(0, |s| s.note.start.div_euclid(HAND_WINDOW));
    let mut upper = Vec::with_capacity(live.len());
    let mut rest = live;
    while let Some(first) = rest.first() {
        let w = window_of(first);
        let (chunk, tail) = rest.split_at(rest.iter().take_while(|p| window_of(p) == w).count());
        rest = tail;

        let mut seen = [false; 256];
        let mut sum = 0u64;
        for p in chunk {
            let pitch = pitch_of(p);
            seen[usize::from(pitch)] = true;
            sum += u64::from(pitch);
        }
        let mut prev: Option<usize> = None;
        let mut widest: Option<(usize, usize)> = None;
        for pitch in (0..seen.len()).filter(|&i| seen[i]) {
            if let Some(lo) = prev
                && widest.is_none_or(|(a, b)| pitch - lo > b - a)
            {
                widest = Some((lo, pitch));
            }
            prev = Some(pitch);
        }
        // Compare doubled pitches against lo + hi so the midpoint needs no fractions.
        let split_x2 = widest.filter(|(lo, hi)| hi - lo >= MIN_HAND_GAP).map(|(lo, hi)| lo + hi);
        let all_upper = sum >= SPLIT_FALLBACK * chunk.len() as u64;
        upper.extend(chunk.iter().map(|p| match split_x2 {
            Some(x2) => 2 * usize::from(pitch_of(p)) >= x2,
            None => all_upper,
        }));
    }
    upper
}

/// Protected by "Protect melody": the top voice (when `top` allows it), a short passing note
/// between chord tones a step away, or a sus2/sus4 colour tone.
fn is_protected(work: &[Slot], voices: &Voices, chords: &[ChordSpan], pos: usize, top: bool) -> bool {
    let Some(note) = work.get(pos).map(|s| s.note) else { return false };
    let Some(gi) = voices.group_of.get(pos).copied() else { return false };
    let Some(group) = voices.groups.get(gi) else { return false };
    let pitch_at = |p: &usize| work.get(*p).map(|s| s.note.pitch);

    let top = top && group.upper && voices.members(group).iter().all(|p| pitch_at(p).is_some_and(|q| q <= note.pitch));
    if top {
        return true;
    }

    let near_chord_tone = |g: Option<&Group>| {
        g.is_some_and(|g| {
            let Some(c) = chord_at(chords, g.start) else { return false };
            voices.members(g).iter().filter_map(pitch_at).any(|q| (1..=2).contains(&q.abs_diff(note.pitch)) && c.tones[pc(q)])
        })
    };
    if note.length <= i64::from(PPQ) && near_chord_tone(voices.prev(gi)) && near_chord_tone(voices.next(gi)) {
        return true;
    }

    chord_at(chords, note.start).is_some_and(|c| matches!((pc(note.pitch) + 12 - pc(c.root)) % 12, 2 | 5))
}

/// Removes notes whose wrongness score passes the threshold the `wrong` amount sets.
fn wrong_notes(work: &mut [Slot], ctx: &Ctx) {
    let a = f64::from(ctx.settings.wrong) / 100.0;
    if a == 0.0 || ctx.chords.is_empty() {
        return;
    }
    let threshold = 1.0 - 0.9 * a;
    let voices = Voices::new(work, ctx.ticks_per_ms);
    let doomed: Vec<usize> = (0..work.len())
        .filter(|&p| {
            let Some(slot) = work.get(p) else { return false };
            let Some(end) = slot.end else { return false };
            let note = &slot.note;
            let f = foreign_share(&ctx.chords, note.pitch, note.start, end);
            let harm = f * if ctx.key_tones[pc(note.pitch)] { 0.6 } else { 1.0 };
            let dur_ms = end.saturating_sub(note.start) as f64 / ctx.ticks_per_ms;
            let short = (1.0 - dur_ms / 250.0).clamp(0.0, 1.0);
            let quiet = (1.0 - f64::from(note.velocity) / 80.0).clamp(0.0, 1.0);
            let score = harm * (0.6 + 0.4 * (short + quiet) / 2.0);
            let top = ctx.key_tones[pc(note.pitch)] || f < TOP_FOREIGN_SHARE;
            score > threshold && !(ctx.settings.protect_melody && is_protected(work, &voices, &ctx.chords, p, top))
        })
        .collect();
    for p in doomed {
        if let Some(slot) = work.get_mut(p) {
            slot.set_end(None, EditKind::WrongNote);
        }
    }
}

/// A chord change: the start of a chord span that directly follows another.
struct Change {
    at: i64,
    /// The new chord's length.
    len: i64,
    /// The new chord's pitch classes.
    tones: [bool; 12],
}

/// The controllers after Held over (`None` = the input's, unchanged) and the pedal changes made.
#[derive(Default)]
struct PedalOut {
    ctrls: Option<Vec<CtrlEvent>>,
    changes: usize,
}

/// One sustain pedal event of the input: its index in the controllers, tick, state and channel.
struct PedalEvent {
    idx: usize,
    tick: i64,
    down: bool,
    channel: u8,
}

/// Held over chord change: cuts notes that ring foreign into a new chord, then changes the sustain
/// pedal at changes where it carries foreign notes.
fn held_over(work: &mut [Slot], ctx: &Ctx, ctrls: &[CtrlEvent]) -> PedalOut {
    let amount = f64::from(ctx.settings.held) / 100.0;
    if amount == 0.0 {
        return PedalOut::default();
    }
    let changes: Vec<Change> = ctx
        .chords
        .windows(2)
        .filter_map(|w| match w {
            [prev, next] if prev.end == next.start => Some(Change { at: next.start, len: next.end.saturating_sub(next.start), tones: next.tones }),
            _ => None,
        })
        .collect();
    if changes.is_empty() {
        return PedalOut::default();
    }
    let voices = ctx.settings.protect_melody.then(|| Voices::new(work, ctx.ticks_per_ms));
    let held = Held { ctx, changes, amount, voices };
    held.cut_notes(work);
    held.change_pedal(work, ctrls)
}

/// The Held over stage's state: the chord changes, the amount (0..=1) and, with melody
/// protection, the voices that say which notes are protected.
struct Held<'a> {
    ctx: &'a Ctx,
    changes: Vec<Change>,
    amount: f64,
    /// Built when melody protection is on.
    voices: Option<Voices>,
}

impl Held<'_> {
    /// Whether something ringing `overhang` ticks into a chord `len` long is cut at this amount.
    fn qualifies(&self, overhang: i64, len: i64) -> bool {
        overhang as f64 > (1.0 - self.amount) * HELD_SHARE * len as f64
    }

    /// A protected note released within a beat of the change at `at` is never foreign there.
    fn resolves(&self, work: &[Slot], pos: usize, end: i64, at: i64) -> bool {
        end <= at.saturating_add(i64::from(PPQ)) && self.voices.as_ref().is_some_and(|v| is_protected(work, v, &self.ctx.chords, pos, true))
    }

    /// Cuts each note at the first change it overhangs, foreign, by a qualifying amount.
    fn cut_notes(&self, work: &mut [Slot]) {
        let pull = ms_to_ticks(HELD_PULL_MS * ((self.amount - 0.75) / 0.25).clamp(0.0, 1.0), self.ctx.ticks_per_ms);
        let mut cuts = Vec::new();
        for (pos, slot) in work.iter().enumerate() {
            let Some(end) = slot.end else { continue };
            let start = slot.note.start;
            let first = self.changes.partition_point(|c| c.at <= start);
            let cut =
                self.changes.iter().skip(first).take_while(|c| c.at < end).find(|c| {
                    !c.tones[pc(slot.note.pitch)] && !self.resolves(work, pos, end, c.at) && self.qualifies(end.saturating_sub(c.at), c.len)
                });
            if let Some(c) = cut {
                cuts.push((pos, c.at.saturating_sub(pull).max(start.saturating_add(1))));
            }
        }
        for (pos, end) in cuts {
            if let Some(slot) = work.get_mut(pos) {
                slot.set_end(Some(end), EditKind::HeldOver);
            }
        }
    }

    /// Whether the pedal, down since `seg`, carries into the change a note foreign to it: one
    /// struck before `attack` whose key was still down at `seg` or went down after it.
    fn foreign_sustain(&self, work: &[Slot], change: &Change, seg: i64, attack: i64, max_len: i64) -> bool {
        let lo = work.partition_point(|s| s.note.start < seg.saturating_sub(max_len));
        let hi = work.partition_point(|s| s.note.start < attack);
        work.get(lo..hi).unwrap_or(&[]).iter().enumerate().any(|(k, s)| {
            s.end.is_some_and(|end| {
                end > seg && !change.tones[pc(s.note.pitch)] && !(end > change.at && self.resolves(work, lo.saturating_add(k), end, change.at))
            })
        })
    }

    /// Pedal up at each qualifying change's attack and down again just after it, replacing the
    /// player's own re-press there; notes of the new chord released before the pedal catches them
    /// are held until it does.
    fn change_pedal(&self, work: &mut [Slot], ctrls: &[CtrlEvent]) -> PedalOut {
        let is_sustain = |c: &CtrlEvent| c.kind == CtrlKind::Cc(SUSTAIN_CC);
        let mut events: Vec<PedalEvent> = ctrls
            .iter()
            .enumerate()
            .filter(|(_, c)| is_sustain(c))
            .map(|(idx, c)| PedalEvent { idx, tick: c.tick, down: c.value >= 64, channel: c.channel })
            .collect();
        if events.is_empty() {
            return PedalOut::default();
        }
        events.sort_by_key(|e| e.tick);
        // After each event: where the pedal-down run it is part of began (None = pedal up) and
        // the next pedal-up.
        let mut run = None;
        let seg_start: Vec<Option<i64>> = events
            .iter()
            .map(|e| {
                run = if e.down { run.or(Some(e.tick)) } else { None };
                run
            })
            .collect();
        let mut up = None;
        let mut next_up: Vec<Option<i64>> = events
            .iter()
            .rev()
            .map(|e| {
                let next = up;
                if !e.down {
                    up = Some(e.tick);
                }
                next
            })
            .collect();
        next_up.reverse();

        let tpm = self.ctx.ticks_per_ms;
        let (attack_win, repress_win, catch_delay) = (ms_to_ticks(ATTACK_MS, tpm), ms_to_ticks(REPRESS_MS, tpm), ms_to_ticks(PEDAL_CATCH_MS, tpm));
        let last_tick = work.iter().map(|s| s.note.end()).chain(ctrls.iter().map(|c| c.tick)).max().unwrap_or(0);
        let mut max_len = work.iter().filter_map(|s| s.end.map(|e| e.saturating_sub(s.note.start))).max().unwrap_or(0);
        let mut removed = vec![false; events.len()];
        // Inserted events, each with its rank among same-tick events: ups 0, downs 2.
        let mut added: Vec<(CtrlEvent, u8)> = Vec::new();
        let mut last_catch = i64::MIN;
        // The end of the previous change's re-press window, processed or not.
        let mut prev_window_end = i64::MIN;
        let mut count = 0;
        for change in &self.changes {
            let b = change.at;
            let first = work.partition_point(|s| s.note.start < b.saturating_sub(attack_win));
            let attack = work
                .get(first..)
                .unwrap_or(&[])
                .iter()
                .take_while(|s| s.note.start <= b.saturating_add(attack_win))
                .find(|s| s.end.is_some() && change.tones[pc(s.note.pitch)])
                .map_or(b, |s| s.note.start);
            // Never past the clip's last event.
            let catch_at = attack.saturating_add(catch_delay.min(change.len / 2).max(1)).min(last_tick).max(attack.saturating_add(1));
            let window_floor = prev_window_end;
            prev_window_end = prev_window_end.max(catch_at);
            if attack <= last_catch {
                continue;
            }
            let Some(j) = events.partition_point(|e| e.tick <= attack).checked_sub(1) else { continue };
            let Some(Some(seg)) = seg_start.get(j).copied() else { continue };
            if !self.foreign_sustain(work, change, seg.max(last_catch), attack, max_len) {
                continue;
            }
            let overhang = next_up.get(j).copied().flatten().unwrap_or(last_tick).saturating_sub(attack);
            if !self.qualifies(overhang, change.len) {
                continue;
            }
            // The player's own re-press near the change (an up, then a down) is replaced; the
            // window never reaches back into the previous change's.
            let win_lo = events.partition_point(|e| e.tick < b.saturating_sub(repress_win) || e.tick <= window_floor);
            let win_hi = events.partition_point(|e| e.tick <= catch_at);
            let kept = |i: usize| removed.get(i).is_some_and(|r| !r);
            let in_window = || (win_lo..win_hi).filter(|&i| kept(i)).filter_map(|i| events.get(i).map(|e| (i, e)));
            let repress = in_window()
                .find(|(_, e)| !e.down)
                .and_then(|(u, ue)| in_window().find(|(i, e)| *i > u && e.down).map(|(d, de)| (u, ue.channel, d, de.channel)));
            // The player's own next up before the catch: the pedal stays up, as he left it.
            let player_up = in_window().any(|(i, e)| !e.down && e.tick > attack && e.tick <= catch_at && repress.is_none_or(|(u, ..)| u != i));
            // Any other press between the attack and the catch is absorbed by the new one.
            let absorbed: Vec<(usize, u8)> = if player_up {
                Vec::new()
            } else {
                in_window()
                    .filter(|(i, e)| e.down && e.tick >= attack && repress.is_none_or(|(_, _, d, _)| d != *i))
                    .map(|(i, e)| (i, e.channel))
                    .collect()
            };
            let (up_channel, down_channel) = match repress {
                Some((_, u, _, d)) => (u, d),
                None => (0, absorbed.first().map_or(0, |(_, ch)| *ch)),
            };
            for i in repress.iter().flat_map(|(u, _, d, _)| [*u, *d]).chain(absorbed.iter().map(|(i, _)| *i)) {
                if let Some(r) = removed.get_mut(i) {
                    *r = true;
                }
            }
            added.push((CtrlEvent { tick: attack, channel: up_channel, kind: CtrlKind::Cc(SUSTAIN_CC), value: 0 }, 0));
            if !player_up {
                added.push((CtrlEvent { tick: catch_at, channel: down_channel, kind: CtrlKind::Cc(SUSTAIN_CC), value: 127 }, 2));
                let held_to = catch_at.saturating_add(1);
                let from = work.partition_point(|s| s.note.start < attack);
                for slot in work.iter_mut().skip(from).take_while(|s| s.note.start < catch_at) {
                    if let Some(end) = slot.end
                        && end < held_to
                        && change.tones[pc(slot.note.pitch)]
                    {
                        slot.set_end(Some(held_to), EditKind::PedalCatch);
                        max_len = max_len.max(held_to.saturating_sub(slot.note.start));
                    }
                }
            }
            last_catch = catch_at;
            count += 1;
        }
        if count == 0 {
            return PedalOut::default();
        }

        let mut dropped = vec![false; ctrls.len()];
        for (e, _) in events.iter().zip(&removed).filter(|(_, r)| **r) {
            if let Some(d) = dropped.get_mut(e.idx) {
                *d = true;
            }
        }
        let mut out: Vec<(CtrlEvent, u8)> = ctrls.iter().zip(&dropped).filter(|(_, d)| !**d).map(|(c, _)| (*c, 1)).collect();
        out.extend(added);
        // Stable: the player's own events keep their order; at one tick an inserted up comes
        // first and an inserted down last.
        out.sort_by_key(|(c, rank)| (c.tick, *rank));
        PedalOut { ctrls: Some(out.into_iter().map(|(c, _)| c).collect()), changes: count }
    }
}

/// Closes small gaps to, and trims small overlaps with, the next onset group of the same hand.
fn legato(work: &mut [Slot], voices: &Voices, ctx: &Ctx) {
    // 0..=480 ticks: the cast cannot truncate.
    let limit = (f64::from(ctx.settings.legato) / 100.0 * f64::from(PPQ) / 2.0).round() as i64;
    if limit == 0 {
        return;
    }
    for (gi, group) in voices.groups.iter().enumerate() {
        let Some(next) = voices.next(gi) else { continue };
        let s2 = next.start;
        for &p in voices.members(group) {
            let Some(slot) = work.get_mut(p) else { continue };
            let Some(end) = slot.end else { continue };
            if s2 <= slot.note.start {
                continue;
            }
            let gap = s2.saturating_sub(end);
            // A held-over cut (with its pull-back) is never closed again, and a note held until
            // the pedal catches it is never trimmed back into the gap.
            if gap > 0 && gap <= limit {
                if slot.kind != Some(EditKind::HeldOver) && !crosses_foreign_change(&ctx.chords, &slot.note, end, s2) {
                    slot.set_end(Some(s2), EditKind::LegatoClose);
                }
            } else if gap < 0 && gap >= -limit && slot.kind != Some(EditKind::PedalCatch) {
                slot.set_end(Some(s2), EditKind::LegatoTrim);
            }
        }
    }
}

/// Extends notes much shorter than the hand's typical length to the next onset group, the next
/// chord change or one beat, whichever comes first.
fn extend(work: &mut [Slot], voices: &Voices, ctx: &Ctx) {
    let a = f64::from(ctx.settings.extend) / 100.0;
    if a == 0.0 {
        return;
    }
    let threshold = |upper: bool| {
        let mut lengths: Vec<i64> = voices
            .groups
            .iter()
            .filter(|g| g.upper == upper)
            .flat_map(|g| voices.members(g))
            .filter_map(|p| work.get(*p).map(|s| s.note.length))
            .collect();
        lengths.sort_unstable();
        let median = lengths.get(lengths.len() / 2).copied().unwrap_or(0);
        a * (f64::from(PPQ) / 4.0).min(median as f64 / 2.0)
    };
    let thresholds = [threshold(false), threshold(true)];
    for (gi, group) in voices.groups.iter().enumerate() {
        let threshold = thresholds[usize::from(group.upper)];
        let next_start = voices.next(gi).map(|g| g.start);
        for &p in voices.members(group) {
            let Some(slot) = work.get_mut(p) else { continue };
            let Some(end) = slot.end else { continue };
            let start = slot.note.start;
            if end.saturating_sub(start) as f64 >= threshold {
                continue;
            }
            let change = ctx.chords.get(ctx.chords.partition_point(|c| c.start <= start)).map(|c| c.start);
            let beat = Some(start.saturating_add(i64::from(PPQ)));
            if let Some(target) = [next_start, change, beat].into_iter().flatten().min()
                && target > end
            {
                slot.set_end(Some(target), EditKind::Extended);
            }
        }
    }
}

/// A lengthened note (Extend, Legato close, the pedal catch) stops at the next surviving note of
/// its pitch, from either hand: the instrument's note-off for it would cut that note short. Never
/// shorter than the take's own note. `work` is sorted by start.
fn same_pitch_stop(work: &mut [Slot]) {
    for i in 0..work.len() {
        let Some(&Slot { note, end: Some(end), .. }) = work.get(i) else { continue };
        let own = note.end();
        if end <= own {
            continue;
        }
        let next = work
            .iter()
            .skip(i.saturating_add(1))
            .take_while(|o| o.note.start < end)
            .find(|o| o.end.is_some() && o.note.pitch == note.pitch && o.note.start > note.start)
            .map(|o| o.note.start);
        if let (Some(next), Some(slot)) = (next, work.get_mut(i)) {
            slot.end = Some(next.max(own));
        }
    }
}

/// Builds the output: surviving notes in input order (then sorted; unchanged ones exactly as
/// input), the edits, the controllers and the counts.
fn finish(input: &CleanupInput, mut work: Vec<Slot>, pedal: PedalOut) -> CleanupOutput {
    work.sort_by_key(|s| s.index);
    let mut notes = Vec::with_capacity(work.len());
    let mut edits = Vec::new();
    let mut report = CleanupReport { total: input.notes.len(), pedal_changes: pedal.changes, ..CleanupReport::default() };
    for slot in &work {
        let original = input.notes.get(slot.index).copied().unwrap_or(slot.note);
        if slot.end == Some(slot.note.end()) {
            notes.push(original);
            continue;
        }
        if let Some(end) = slot.end {
            notes.push(Note { length: end.saturating_sub(slot.note.start).max(1), ..slot.note });
        }
        let Some(kind) = slot.kind else { continue };
        edits.push(NoteEdit { index: slot.index, kind, old_end: original.end(), new_end: slot.end });
        match (slot.end, kind) {
            (None, _) => report.removed += 1,
            (Some(_), EditKind::HeldOver) => report.held_trimmed += 1,
            (Some(_), EditKind::LegatoTrim) => report.legato_trimmed += 1,
            (Some(_), EditKind::LegatoClose) => report.legato_closed += 1,
            (Some(_), EditKind::Extended) => report.extended += 1,
            (Some(_), EditKind::WrongNote | EditKind::PedalCatch) => {}
        }
    }
    let ctrls = pedal.ctrls.unwrap_or_else(|| input.ctrls.to_vec());
    let mut sequence = Sequence { notes, ctrls };
    sequence.sort();
    CleanupOutput { sequence, edits, report }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::XorShift;
    use std::time::{Duration, Instant};

    const TPM: f64 = 1.92;

    fn n(pitch: u8, start: i64, len: i64, vel: u8) -> Note {
        Note { pitch, velocity: vel, release_velocity: 64, channel: 0, start, length: len }
    }

    fn chord(start: i64, end: i64, root: u8, pcs: &[u8]) -> ChordSpan {
        let mut tones = [false; 12];
        for &p in pcs {
            tones[usize::from(p % 12)] = true;
        }
        ChordSpan { start, end, root, tones }
    }

    fn c_major() -> [bool; 12] {
        let mut k = [false; 12];
        for p in [0, 2, 4, 5, 7, 9, 11] {
            k[p] = true;
        }
        k
    }

    fn settings(wrong: u8, held: u8, legato: u8, extend: u8, protect_melody: bool) -> CleanupSettings {
        CleanupSettings { wrong, held, legato, extend, protect_melody }
    }

    fn input<'a>(notes: &'a [Note], chords: &'a [ChordSpan], settings: CleanupSettings) -> CleanupInput<'a> {
        CleanupInput { notes, ctrls: &[], chords, key_tones: c_major(), ticks_per_ms: TPM, settings }
    }

    fn edit(index: usize, kind: EditKind, old_end: i64, new_end: Option<i64>) -> NoteEdit {
        NoteEdit { index, kind, old_end, new_end }
    }

    fn pedal(tick: i64, value: i32) -> CtrlEvent {
        CtrlEvent { tick, channel: 0, kind: CtrlKind::Cc(64), value }
    }

    #[test]
    fn zero_amounts_change_nothing() {
        let notes = [n(60, 0, 960, 90), n(61, 480, 60, 30), n(64, 0, 400, 80), n(62, 1000, 100, 50), n(40, -200, 30, 5)];
        let chords = [chord(0, 960, 0, &[0, 4, 7]), chord(960, 3840, 5, &[5, 9, 0])];
        let ctrls = [pedal(0, 127), pedal(900, 0), pedal(1000, 127)];
        let out = cleanup(&CleanupInput { ctrls: &ctrls, ..input(&notes, &chords, settings(0, 0, 0, 0, true)) });
        let mut expect = Sequence { notes: notes.to_vec(), ctrls: ctrls.to_vec() };
        expect.sort();
        assert_eq!(out.sequence, expect);
        assert!(out.edits.is_empty());
        assert_eq!(out.report, CleanupReport { total: notes.len(), ..CleanupReport::default() });
    }

    #[test]
    fn defaults_are_the_spec_values() {
        assert_eq!(CleanupSettings::default(), CleanupSettings { wrong: 40, held: 70, legato: 30, extend: 40, protect_melody: true });
        // Old sessions (no field) and partial settings load with the defaults filled in.
        assert_eq!(serde_json::from_str::<CleanupSettings>("{}").ok(), Some(CleanupSettings::default()));
        assert_eq!(serde_json::from_str::<CleanupSettings>(r#"{"wrong":7}"#).ok(), Some(CleanupSettings { wrong: 7, ..CleanupSettings::default() }));
        assert_eq!(settings(101, 255, 100, 0, false).clamped(), settings(100, 100, 100, 0, false));
    }

    #[test]
    fn short_quiet_out_of_key_note_removed_at_default() {
        let notes = [n(60, 0, 960, 90), n(64, 0, 960, 90), n(67, 0, 960, 90), n(61, 480, 60, 30)];
        let chords = [chord(0, 3840, 0, &[0, 4, 7])];
        let out = cleanup(&input(&notes, &chords, CleanupSettings::default()));
        assert_eq!(out.edits, vec![edit(3, EditKind::WrongNote, 540, None)]);
        assert_eq!(out.report.removed, 1);
        assert_eq!(out.sequence.notes, notes[..3].to_vec());
    }

    #[test]
    fn in_key_non_chord_note_kept_at_40_removed_at_100() {
        let notes = [n(62, 0, 960, 100)];
        let chords = [chord(0, 3840, 0, &[0, 4, 7])];
        let kept = cleanup(&input(&notes, &chords, settings(40, 0, 0, 0, false)));
        assert_eq!(kept.sequence.notes, notes.to_vec());
        let removed = cleanup(&input(&notes, &chords, settings(100, 0, 0, 0, false)));
        assert!(removed.sequence.notes.is_empty());
        assert_eq!(removed.edits, vec![edit(0, EditKind::WrongNote, 960, None)]);
    }

    #[test]
    fn chord_tones_never_removed_even_at_100() {
        let notes = [n(64, 0, 30, 10)];
        let chords = [chord(0, 3840, 0, &[0, 4, 7])];
        let out = cleanup(&input(&notes, &chords, settings(100, 0, 0, 0, false)));
        assert_eq!(out.sequence.notes, notes.to_vec());
        assert!(out.edits.is_empty());
    }

    #[test]
    fn wrongness_weighted_over_chords() {
        let notes = [n(65, 0, 1920, 100)];
        let s = settings(100, 0, 0, 0, false);
        let mostly_f = [chord(0, 192, 0, &[0, 4, 7]), chord(192, 3840, 5, &[5, 9, 0])];
        assert_eq!(cleanup(&input(&notes, &mostly_f, s)).sequence.notes, notes.to_vec());
        let mostly_c = [chord(0, 1728, 0, &[0, 4, 7]), chord(1728, 3840, 5, &[5, 9, 0])];
        assert!(cleanup(&input(&notes, &mostly_c, s)).sequence.notes.is_empty());
    }

    #[test]
    fn protect_melody_keeps_top_voice_and_colour_tones() {
        let chords = [chord(0, 3840, 0, &[0, 4, 7])];
        // D5 is the top of its onset group.
        let top = [n(60, 0, 960, 90), n(64, 0, 960, 90), n(67, 0, 960, 90), n(74, 0, 960, 90)];
        assert_eq!(cleanup(&input(&top, &chords, settings(100, 0, 0, 0, true))).sequence.notes.len(), 4);
        assert_eq!(cleanup(&input(&top, &chords, settings(100, 0, 0, 0, false))).edits, vec![edit(3, EditKind::WrongNote, 960, None)]);
        // F4 is a sus4 colour tone inside the texture (C5 is the top voice).
        let colour = [n(60, 0, 960, 90), n(64, 0, 960, 90), n(65, 0, 960, 90), n(67, 0, 960, 90), n(72, 0, 960, 90)];
        assert_eq!(cleanup(&input(&colour, &chords, settings(100, 0, 0, 0, true))).sequence.notes.len(), 5);
        assert_eq!(cleanup(&input(&colour, &chords, settings(100, 0, 0, 0, false))).edits, vec![edit(2, EditKind::WrongNote, 960, None)]);
    }

    #[test]
    fn out_of_key_top_note_over_a_chord_without_it_is_not_melody() {
        let chords = [chord(0, 3840, 0, &[0, 4, 7])];
        // A held C#5 alone above a C triad: out of the key and foreign to the chord.
        let slip = [n(60, 0, 960, 90), n(64, 0, 960, 90), n(67, 0, 960, 90), n(73, 960, 1920, 50)];
        assert_eq!(cleanup(&input(&slip, &chords, settings(40, 0, 0, 0, true))).edits, vec![edit(3, EditKind::WrongNote, 2880, None)]);
        // Over a chord holding it (A major), the same note is the melody and stays.
        let a_major = [chord(0, 3840, 9, &[9, 1, 4])];
        let slip = [n(57, 0, 960, 90), n(61, 0, 960, 90), n(64, 0, 960, 90), n(73, 960, 1920, 50)];
        assert_eq!(cleanup(&input(&slip, &a_major, settings(100, 0, 0, 0, true))).sequence.notes.len(), 4);
    }

    #[test]
    fn protect_melody_keeps_passing_notes() {
        // Lower hand G3 A3 G3 over C: the A3 is a short neighbour note between chord tones (and not
        // a colour tone, so only the passing-note rule protects it).
        let chords = [chord(0, 3840, 0, &[0, 4, 7])];
        let notes = [n(55, 0, 480, 90), n(57, 480, 240, 90), n(55, 960, 480, 90), n(84, 0, 1920, 90)];
        assert!(cleanup(&input(&notes, &chords, settings(100, 0, 0, 0, true))).edits.is_empty());
        assert_eq!(cleanup(&input(&notes, &chords, settings(100, 0, 0, 0, false))).edits, vec![edit(1, EditKind::WrongNote, 720, None)]);
    }

    #[test]
    fn legato_closes_small_gaps_and_trims_small_overlaps() {
        let notes = [n(60, 0, 400, 90), n(62, 480, 420, 90), n(64, 880, 520, 90)];
        let out = cleanup(&input(&notes, &[], settings(0, 0, 100, 0, true)));
        assert_eq!(out.edits, vec![edit(0, EditKind::LegatoClose, 400, Some(480)), edit(1, EditKind::LegatoTrim, 900, Some(880))]);
        assert_eq!(out.report.legato_closed, 1);
        assert_eq!(out.report.legato_trimmed, 1);
        let out = cleanup(&input(&notes, &[], settings(0, 0, 10, 0, true)));
        assert_eq!(out.edits, vec![edit(1, EditKind::LegatoTrim, 900, Some(880))]);
        // A close never carries C4 into a chord (G) that lacks it; the overlap is still trimmed.
        let chords = [chord(0, 440, 0, &[0, 4, 7]), chord(440, 3840, 7, &[7, 11, 2])];
        let out = cleanup(&input(&notes, &chords, settings(0, 0, 100, 0, true)));
        assert_eq!(out.edits, vec![edit(1, EditKind::LegatoTrim, 900, Some(880))]);
    }

    #[test]
    fn legato_leaves_rests() {
        let notes = [n(60, 0, 480, 90), n(62, 1440, 480, 90)];
        assert!(cleanup(&input(&notes, &[], settings(0, 0, 100, 0, true))).edits.is_empty());
    }

    #[test]
    fn extend_short_note_to_next_note_not_past_chord_change() {
        // The change (600) comes before the next note (1200) and the one-beat cap (960).
        let chords = [chord(0, 600, 0, &[0, 4, 7]), chord(600, 1920, 5, &[5, 9, 0])];
        let notes = [n(64, 0, 30, 80), n(65, 1200, 960, 80)];
        let out = cleanup(&input(&notes, &chords, settings(0, 0, 0, 100, true)));
        assert_eq!(out.edits, vec![edit(0, EditKind::Extended, 30, Some(600))]);
        assert_eq!(out.report.extended, 1);
        let notes = [n(64, 0, 30, 80), n(65, 480, 960, 80)];
        let out = cleanup(&input(&notes, &[], settings(0, 0, 0, 100, true)));
        assert_eq!(out.edits, vec![edit(0, EditKind::Extended, 30, Some(480))]);
        // Never more than a beat.
        let notes = [n(64, 0, 30, 80), n(65, 1800, 960, 80)];
        let out = cleanup(&input(&notes, &[], settings(0, 0, 0, 100, true)));
        assert_eq!(out.edits, vec![edit(0, EditKind::Extended, 30, Some(960))]);
    }

    #[test]
    fn extend_ignores_a_players_normal_staccato() {
        let notes: Vec<Note> = (0..8).map(|i| n(if i % 2 == 0 { 60 } else { 62 }, i * 240, 60, 80)).collect();
        assert!(cleanup(&input(&notes, &[], settings(0, 0, 0, 100, true))).edits.is_empty());
    }

    #[test]
    fn no_chords_only_legato_and_extend() {
        let notes = [n(60, 0, 400, 90), n(61, 480, 60, 20), n(62, 960, 960, 90)];
        let out = cleanup(&input(&notes, &[], settings(100, 100, 100, 100, true)));
        assert_eq!(out.report.removed, 0);
        assert_eq!(out.sequence.notes.len(), 3);
        assert_eq!(out.edits, vec![edit(0, EditKind::LegatoClose, 400, Some(480)), edit(1, EditKind::LegatoClose, 540, Some(960))]);
    }

    fn random_material(rng: &mut XorShift, notes: usize, chords: usize, ticks_per_note: i64) -> (Vec<Note>, Vec<ChordSpan>) {
        let span = (notes as i64).saturating_mul(ticks_per_note).max(1);
        let mut r = |m: u64| rng.next_u64() % m.max(1);
        let ns = (0..notes)
            .map(|_| Note {
                pitch: 21 + r(88) as u8,
                velocity: 1 + r(127) as u8,
                release_velocity: 64,
                channel: r(2) as u8,
                start: r(span as u64) as i64,
                length: 1 + r(1920) as i64,
            })
            .collect();
        let chord_len = (span / chords.max(1) as i64).max(1);
        let cs = (0..chords)
            .map(|i| {
                let root = r(12) as u8;
                let third = if r(2) == 0 { 3 } else { 4 };
                chord(i as i64 * chord_len, (i as i64 + 1) * chord_len, root, &[root, root + third, root + 7])
            })
            .collect();
        (ns, cs)
    }

    #[test]
    fn starts_velocities_pitches_never_change() {
        let mut rng = XorShift::new(7);
        for seed_round in 0..3 {
            let (notes, chords) = random_material(&mut rng, 2000, 50, 96);
            let out = cleanup(&input(&notes, &chords, settings(100, 100, 100, 100, seed_round % 2 == 0)));
            let mut originals: Vec<(u8, u8, u8, i64)> = notes.iter().map(|n| (n.pitch, n.velocity, n.channel, n.start)).collect();
            originals.sort_unstable();
            for o in &out.sequence.notes {
                assert!(originals.binary_search(&(o.pitch, o.velocity, o.channel, o.start)).is_ok(), "{o:?} not in the input");
                assert!(o.length >= 1);
            }
            assert_eq!(out.sequence.notes.len() + out.report.removed, out.report.total);
            assert_eq!(out.report.total, notes.len());
            assert!(out.edits.windows(2).all(|w| w[0].index < w[1].index));
            for e in &out.edits {
                assert_eq!(e.old_end, notes[e.index].end());
            }
        }
    }

    #[test]
    fn lengthened_notes_stop_at_the_next_note_of_the_same_pitch() {
        // The instrument's note-off for a note cuts a later note of its pitch it still overlaps, so
        // Extend, Legato close and the pedal catch never carry a note past the next one's start.
        let mut rng = XorShift::new(11);
        let bar = i64::from(PPQ) * 4;
        for round in 0..4 {
            let (notes, chords) = random_material(&mut rng, 1500, 40, 96);
            let ctrls: Vec<CtrlEvent> = (0..40).flat_map(|b| [pedal(b * bar + 60, 127), pedal(b * bar + bar - 30, 0)]).collect();
            let inp = CleanupInput { ctrls: &ctrls, ..input(&notes, &chords, settings(40 * (round % 2), 100, 100, 100, round < 2)) };
            let out = cleanup(&inp);
            let mut lengthened = 0;
            for e in out.edits.iter().filter(|e| e.new_end.is_some_and(|end| end > e.old_end)) {
                let (a, end) = (notes[e.index], e.new_end.unwrap());
                lengthened += 1;
                let cut = out.sequence.notes.iter().find(|o| o.pitch == a.pitch && o.start > a.start && o.start < end);
                assert!(cut.is_none(), "round {round}: {:?} {a:?} now ends at {end}, past {cut:?}", e.kind);
            }
            assert!(lengthened > 0, "round {round}: something was lengthened");
        }
    }

    #[test]
    fn hostile_input_is_safe() {
        let notes = [
            n(60, i64::MIN / 2, i64::MAX / 4, 90),
            n(61, 0, 0, 10),
            n(62, 100, -50, 10),
            n(200, 50, 30, 0),
            n(64, i64::MAX - 10, 1000, 50),
            n(65, i64::MIN, 1, 127),
        ];
        let chords = [
            chord(100, 50, 0, &[0, 4, 7]),
            chord(0, 1000, 0, &[0, 4, 7]),
            chord(500, 2000, 5, &[5, 9, 0]),
            chord(500, 500, 7, &[7]),
            chord(i64::MIN, i64::MAX, 3, &[]),
        ];
        for tpm in [f64::NAN, -1.0, 0.0, f64::INFINITY, 1e300, 1e-300] {
            for protect in [false, true] {
                let inp = CleanupInput { ticks_per_ms: tpm, ..input(&notes, &chords, settings(100, 100, 100, 100, protect)) };
                let out = cleanup(&inp);
                // Only unedited notes keep a non-positive length.
                assert!(out.sequence.notes.iter().all(|o| o.length >= 1 || notes.contains(o)));
                assert_eq!(out.sequence.notes.len() + out.report.removed, notes.len());
            }
        }
    }

    #[test]
    fn twenty_thousand_notes_is_fast() {
        let mut rng = XorShift::new(42);
        let (notes, chords) = random_material(&mut rng, 20_000, 1_000, i64::from(PPQ) / 10);
        let bar = i64::from(PPQ) * 4;
        let ctrls: Vec<CtrlEvent> = (0..notes.len() as i64 / 40).flat_map(|b| [pedal(b * bar + 60, 127), pedal(b * bar + bar - 30, 0)]).collect();
        let inp = CleanupInput { ctrls: &ctrls, ..input(&notes, &chords, settings(100, 100, 100, 100, true)) };
        let t = Instant::now();
        let out = cleanup(&inp);
        let elapsed = t.elapsed();
        assert_eq!(out.report.total, 20_000);
        assert!(out.report.pedal_changes > 0, "the pedal stage ran");
        // Debug builds under a parallel test run are slow and noisy; the tight bound is for release.
        let limit = Duration::from_millis(if cfg!(debug_assertions) { 2000 } else { 250 });
        assert!(elapsed < limit, "took {elapsed:?}");
    }

    fn held(amount: u8) -> CleanupSettings {
        settings(0, amount, 0, 0, false)
    }

    /// C 0..1920 then F 1920..3840.
    fn c_then_f() -> [ChordSpan; 2] {
        [chord(0, 1920, 0, &[0, 4, 7]), chord(1920, 3840, 5, &[5, 9, 0])]
    }

    /// The full pull-back (30 ms) and pedal catch delay (60 ms) at [`TPM`].
    const PULL: i64 = 58;
    const D: i64 = 115;

    fn pedal_events(out: &CleanupOutput) -> Vec<(i64, i32)> {
        out.sequence.ctrls.iter().filter(|c| c.kind == CtrlKind::Cc(64)).map(|c| (c.tick, c.value)).collect()
    }

    #[test]
    fn pedal_constants_match_the_tempo() {
        assert_eq!(ms_to_ticks(30.0, TPM), PULL);
        assert_eq!(ms_to_ticks(60.0, TPM), D);
    }

    #[test]
    fn foreign_note_cut_at_change_chord_tone_rings_on() {
        let notes = [n(64, 0, 2880, 90), n(60, 0, 2880, 90)];
        let out = cleanup(&input(&notes, &c_then_f(), held(100)));
        assert_eq!(out.edits, vec![edit(0, EditKind::HeldOver, 2880, Some(1920 - PULL))]);
        assert_eq!(out.report.held_trimmed, 1);
        assert!(out.sequence.notes.contains(&notes[1]));
    }

    #[test]
    fn low_amount_cuts_only_long_overhangs() {
        // held 10: the bar is 0.9 * 0.25 * 1920 = 432 ticks past the change.
        let short = [n(64, 0, 1920 + 300, 90)];
        assert!(cleanup(&input(&short, &c_then_f(), held(10))).edits.is_empty());
        let long = [n(64, 0, 1920 + 600, 90)];
        // No pull-back below 75 %.
        assert_eq!(cleanup(&input(&long, &c_then_f(), held(10))).edits, vec![edit(0, EditKind::HeldOver, 2520, Some(1920))]);
    }

    #[test]
    fn note_spanning_many_chords_cut_at_first_foreign_change() {
        let chords =
            [chord(0, 960, 0, &[0, 4, 7]), chord(960, 1920, 9, &[9, 0, 4]), chord(1920, 2880, 5, &[5, 9, 0]), chord(2880, 3840, 7, &[7, 11, 2])];
        let notes = [n(60, 0, 3840, 90)];
        let out = cleanup(&input(&notes, &chords, held(100)));
        assert_eq!(out.edits, vec![edit(0, EditKind::HeldOver, 3840, Some(2880 - PULL))]);
    }

    #[test]
    fn note_in_every_chord_never_cut() {
        let chords =
            [chord(0, 960, 0, &[0, 4, 7]), chord(960, 1920, 4, &[4, 7, 11]), chord(1920, 2880, 7, &[7, 11, 2]), chord(2880, 3840, 0, &[0, 4, 7])];
        let notes = [n(67, 0, 3840, 90)];
        assert!(cleanup(&input(&notes, &chords, held(100))).edits.is_empty());
    }

    #[test]
    fn protected_colour_tone_resolving_within_a_beat_not_cut() {
        // D4 is a sus2 colour tone over C (C5 is the top voice), released 400 ticks into F.
        let notes = [n(60, 0, 1900, 90), n(62, 0, 1920 + 400, 90), n(72, 0, 1900, 90)];
        assert!(cleanup(&input(&notes, &c_then_f(), settings(0, 100, 0, 0, true))).edits.is_empty());
        assert_eq!(cleanup(&input(&notes, &c_then_f(), held(100))).edits, vec![edit(1, EditKind::HeldOver, 2320, Some(1920 - PULL))]);
        // Held a whole beat and more into F, it is cut even when protected.
        let late = [n(60, 0, 1900, 90), n(62, 0, 1920 + 1000, 90), n(72, 0, 1900, 90)];
        assert_eq!(
            cleanup(&input(&late, &c_then_f(), settings(0, 100, 0, 0, true))).edits,
            vec![edit(1, EditKind::HeldOver, 2920, Some(1920 - PULL))]
        );
    }

    #[test]
    fn pedal_legato_change_at_attack() {
        let notes = [n(64, 0, 300, 90), n(65, 1920, 960, 90)];
        let ctrls = [pedal(0, 127), pedal(3800, 0)];
        for protect in [false, true] {
            // A melody note released before the change is not "resolving": the pedal still carries it.
            let out = cleanup(&CleanupInput { ctrls: &ctrls, ..input(&notes, &c_then_f(), settings(0, 100, 0, 0, protect)) });
            assert_eq!(pedal_events(&out), vec![(0, 127), (1920, 0), (1920 + D, 127), (3800, 0)]);
            assert_eq!(out.report.pedal_changes, 1);
            assert!(out.edits.is_empty());
        }
    }

    #[test]
    fn attack_slightly_early_moves_the_change() {
        let notes = [n(64, 0, 300, 90), n(65, 1900, 960, 90), n(69, 1910, 960, 90)];
        let ctrls = [pedal(0, 127), pedal(3800, 0)];
        let out = cleanup(&CleanupInput { ctrls: &ctrls, ..input(&notes, &c_then_f(), held(100)) });
        assert_eq!(pedal_events(&out), vec![(0, 127), (1900, 0), (1900 + D, 127), (3800, 0)]);
    }

    #[test]
    fn players_early_repress_moved_after_the_change() {
        // E4's key is still down at the re-press (1880), so the pedal catches the old chord.
        let notes = [n(64, 0, 1900, 90), n(65, 1920, 960, 90)];
        let ctrls = [pedal(0, 127), pedal(1850, 0), pedal(1880, 127), pedal(3800, 0)];
        let out = cleanup(&CleanupInput { ctrls: &ctrls, ..input(&notes, &c_then_f(), held(100)) });
        assert_eq!(pedal_events(&out), vec![(0, 127), (1920, 0), (1920 + D, 127), (3800, 0)]);
        assert_eq!(out.report.pedal_changes, 1);
    }

    #[test]
    fn no_gap_after_change() {
        let notes = [n(64, 0, 1900, 90), n(53, 1920, 10, 90), n(57, 1920, 10, 90), n(60, 1920, 10, 90)];
        let ctrls = [pedal(0, 127), pedal(3840, 0)];
        let chords = c_then_f();
        let inp = |amount| CleanupInput { ctrls: &ctrls, ..input(&notes, &chords, held(amount)) };
        let new_chord_held = |out: &CleanupOutput, t: i64| {
            let in_f = |p: u8| [5, 9, 0].contains(&(p % 12));
            let key = out.sequence.notes.iter().any(|n| in_f(n.pitch) && n.start <= t && t < n.end());
            let events = pedal_events(out);
            let down = events.iter().rfind(|(k, _)| *k <= t).is_some_and(|(_, v)| *v >= 64);
            let caught = events
                .iter()
                .rfind(|(k, v)| *k <= t && *v >= 64)
                .is_some_and(|(p, _)| out.sequence.notes.iter().any(|n| in_f(n.pitch) && n.start <= *p && *p <= n.end()));
            key || (down && caught)
        };
        let before = cleanup(&inp(0));
        assert!(!(1920..3840).all(|t| new_chord_held(&before, t)));
        let out = cleanup(&inp(100));
        for t in 1920..3840 {
            assert!(new_chord_held(&out, t), "gap at {t}");
        }
        let events = pedal_events(&out);
        assert!(events.contains(&(1920, 0)) && events.contains(&(1920 + D, 127)), "{events:?}");
        let caught = 1920 + D + 1;
        assert_eq!(
            out.edits,
            vec![
                edit(1, EditKind::PedalCatch, 1930, Some(caught)),
                edit(2, EditKind::PedalCatch, 1930, Some(caught)),
                edit(3, EditKind::PedalCatch, 1930, Some(caught))
            ]
        );
        assert_eq!(out.report, CleanupReport { total: 4, pedal_changes: 1, ..CleanupReport::default() });
    }

    #[test]
    fn section_long_pedal_gets_change_at_each_foreign_change() {
        // C G C G C G C G, a beat each... (960 ticks); the 4th chord (G) plays only Gs, which C keeps.
        let chords: Vec<ChordSpan> = (0..8)
            .map(|i| if i % 2 == 0 { chord(i * 960, (i + 1) * 960, 0, &[0, 4, 7]) } else { chord(i * 960, (i + 1) * 960, 7, &[7, 11, 2]) })
            .collect();
        let notes: Vec<Note> = (0..8i64)
            .flat_map(|i| {
                let pitches: &[u8] = match i {
                    3 => &[55, 67],
                    _ if i % 2 == 0 => &[60, 64, 67],
                    _ => &[55, 59, 62],
                };
                pitches.iter().map(move |&p| n(p, i * 960, 480, 90))
            })
            .collect();
        let ctrls = [pedal(0, 127), pedal(7680, 0)];
        let out = cleanup(&CleanupInput { ctrls: &ctrls, ..input(&notes, &chords, held(100)) });
        let mut expect = vec![(0, 127)];
        for b in [960, 1920, 2880, 4800, 5760, 6720] {
            expect.extend([(b, 0), (b + D, 127)]);
        }
        expect.push((7680, 0));
        assert_eq!(pedal_events(&out), expect);
        assert_eq!(out.report.pedal_changes, 6);
        assert!(out.sequence.ctrls.windows(2).all(|w| w[0].tick <= w[1].tick));
        assert_eq!(out.sequence.ctrls.last().map(|c| c.value), Some(0));
    }

    #[test]
    fn no_chords_leaves_pedal_alone() {
        let notes = [n(64, 0, 300, 90), n(65, 1920, 960, 90)];
        let ctrls =
            [pedal(0, 127), CtrlEvent { tick: 400, channel: 0, kind: CtrlKind::Cc(1), value: 30 }, pedal(1850, 0), pedal(1880, 127), pedal(3800, 0)];
        let out = cleanup(&CleanupInput { ctrls: &ctrls, ..input(&notes, &[], settings(100, 100, 100, 100, true)) });
        assert_eq!(out.sequence.ctrls, ctrls.to_vec());
        assert_eq!(out.report.pedal_changes, 0);
    }

    #[test]
    fn held_zero_is_identity_for_pedal_and_ends() {
        let notes = [n(64, 0, 2880, 90), n(60, 0, 2880, 90), n(65, 1920, 10, 90)];
        let ctrls = [pedal(0, 127), pedal(1850, 0), pedal(1880, 127), pedal(3800, 0)];
        let out = cleanup(&CleanupInput { ctrls: &ctrls, ..input(&notes, &c_then_f(), held(0)) });
        assert_eq!(out.sequence.ctrls, ctrls.to_vec());
        assert!(out.edits.is_empty());
        assert_eq!(out.report, CleanupReport { total: 3, ..CleanupReport::default() });
    }

    #[test]
    fn non_positive_lengths_untouched_unless_edited() {
        let notes = [n(64, 0, 0, 80), n(62, 100, -5, 80), n(65, 480, 960, 80)];
        let out = cleanup(&input(&notes, &[], settings(0, 0, 0, 0, true)));
        let mut expect = Sequence { notes: notes.to_vec(), ctrls: vec![] };
        expect.sort();
        assert_eq!(out.sequence, expect);
        assert!(out.edits.is_empty());
        // Edited, the old end is the input's own end.
        let pair = [notes[0], notes[2]];
        let out = cleanup(&input(&pair, &[], settings(0, 0, 0, 100, true)));
        assert_eq!(out.edits, vec![edit(0, EditKind::Extended, 0, Some(480))]);
    }

    #[test]
    fn legato_keeps_the_held_over_pull_back() {
        let notes = [n(64, 0, 2880, 90), n(65, 1920, 960, 90)];
        let out = cleanup(&input(&notes, &c_then_f(), settings(0, 100, 30, 0, false)));
        assert_eq!(out.edits, vec![edit(0, EditKind::HeldOver, 2880, Some(1920 - PULL))]);
    }

    #[test]
    fn repress_window_stops_at_the_previous_change() {
        // C (0..1920, only Gs played, so its change to G is not processed), G for half a beat, C.
        // The player lifts correctly just after 1920; the change at 2400 must not take that lift.
        let chords = [chord(0, 1920, 0, &[0, 4, 7]), chord(1920, 2400, 7, &[7, 11, 2]), chord(2400, 3840, 0, &[0, 4, 7])];
        let notes = [n(55, 0, 480, 90), n(67, 0, 480, 90), n(55, 1920, 200, 90), n(59, 1920, 200, 90), n(62, 1920, 200, 90), n(60, 2400, 960, 90)];
        let ctrls = [pedal(0, 127), pedal(1930, 0), pedal(1980, 127), pedal(3840, 0)];
        let out = cleanup(&CleanupInput { ctrls: &ctrls, ..input(&notes, &chords, held(100)) });
        assert_eq!(pedal_events(&out), vec![(0, 127), (1930, 0), (1980, 127), (2400, 0), (2400 + D, 127), (3840, 0)]);
        assert_eq!(out.report.pedal_changes, 1);
    }

    #[test]
    fn players_up_at_the_catch_tick_wins() {
        let notes = [n(64, 0, 300, 90), n(65, 1920, 960, 90)];
        let ctrls = [pedal(0, 127), pedal(1920 + D, 0), pedal(2500, 127), pedal(3800, 0)];
        let out = cleanup(&CleanupInput { ctrls: &ctrls, ..input(&notes, &c_then_f(), held(100)) });
        assert_eq!(pedal_events(&out), vec![(0, 127), (1920, 0), (1920 + D, 0), (2500, 127), (3800, 0)]);
    }

    #[test]
    fn legato_keeps_the_pedal_catch() {
        let notes = [n(64, 0, 1900, 90), n(65, 1920, 10, 90), n(67, 2000, 400, 90)];
        let ctrls = [pedal(0, 127), pedal(3840, 0)];
        let out = cleanup(&CleanupInput { ctrls: &ctrls, ..input(&notes, &c_then_f(), settings(0, 100, 30, 0, false)) });
        assert!(out.edits.contains(&edit(1, EditKind::PedalCatch, 1930, Some(1920 + D + 1))), "{:?}", out.edits);
    }

    #[test]
    fn inserted_down_never_past_the_clip() {
        // The pedal is never released and the clip ends at 1970, before the catch would come.
        let notes = [n(64, 0, 300, 90), n(65, 1920, 50, 90)];
        let ctrls = [pedal(0, 127)];
        let out = cleanup(&CleanupInput { ctrls: &ctrls, ..input(&notes, &c_then_f(), held(100)) });
        assert_eq!(pedal_events(&out), vec![(0, 127), (1920, 0), (1970, 127)]);
    }

    #[test]
    fn players_own_same_tick_order_kept() {
        let notes = [n(64, 0, 300, 90), n(65, 1920, 960, 90)];
        let ctrls = [pedal(0, 127), pedal(3000, 127), pedal(3000, 0), pedal(3100, 127), pedal(3800, 0)];
        let out = cleanup(&CleanupInput { ctrls: &ctrls, ..input(&notes, &c_then_f(), held(100)) });
        assert_eq!(pedal_events(&out), vec![(0, 127), (1920, 0), (1920 + D, 127), (3000, 127), (3000, 0), (3100, 127), (3800, 0)]);
    }

    #[test]
    fn summary_line() {
        let r = CleanupReport { total: 412, removed: 6, held_trimmed: 10, legato_trimmed: 4, legato_closed: 1, extended: 2, pedal_changes: 0 };
        assert_eq!(r.summary(), "Removed 6 · trimmed 14 · extended 3 of 412 notes");
        let r = CleanupReport { pedal_changes: 3, ..r };
        assert_eq!(r.summary(), "Removed 6 · trimmed 14 · extended 3 of 412 notes · pedal changes 3");
    }

    #[test]
    fn ms_to_ticks_rounds_and_clamps() {
        assert_eq!(ms_to_ticks(40.0, TPM), 77);
        assert_eq!(ms_to_ticks(-5.0, TPM), 0);
        assert_eq!(ms_to_ticks(f64::NAN, TPM), 0);
        assert_eq!(ms_to_ticks(1e300, TPM), i64::MAX);
    }
}
