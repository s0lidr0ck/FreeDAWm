//! The DAW's two song lanes (section blocks and chords, in half bars) as MIDI Studio's
//! [`SongTimeline`], and a [`Song`] laid onto lanes.
//!
//! Not a port: MIDI Studio has no lanes. The timeline it builds is the musicians' only view of the
//! song, so this module makes the same timeline `build_timeline` would, and a factory song laid onto
//! lanes at half 0 comes back field for field.
//!
//! - The blocks, in start order, are the arrangement: block i is pass `arr_pos` i, with its chip's
//!   energy and take. Time no block covers (up to the lanes' end) is a pass of the default section
//!   (A, else the first definition); those passes are numbered after the blocks (n, n+1, ...).
//!   Every pass is one contiguous run of entries, in time order.
//! - Chords are cut at pass boundaries and split greedily into 4, 2 and 1 half-bar entries. Time
//!   with no chord plays the synthetic silent section (`'-'`, nobody plays).
//! - `chord_index`, `section_chords`, the Build / Drop shape, `intro_half`, `has_ending` (the lanes
//!   stop after the first Ending block) and `uniform_len_half` follow `build_timeline`.
//! - Overlaps: the later start wins from its start (the earlier item ends there).

use std::sync::Arc;

use crate::song::{
    MAX_ARRANGEMENT, MAX_CHORDS, MAX_SECTIONS, Song, SongChord, SongMusician, SongSection, SongTimeline, TimelineEntry, clamp_int, legal_length,
};
use crate::song_feel::SongFeel;

/// Lanes end here at the latest (32768 bars): bounds the timeline whatever the lanes hold.
pub const MAX_LANE_HALF: i32 = 1 << 16;
/// Most blocks / chords read (the session's own caps).
pub const MAX_LANE_BLOCKS: usize = 512;
pub const MAX_LANE_CHORDS: usize = 4096;
/// The id of the synthetic section that plays time with no chord.
pub const SILENT_SECTION_ID: char = '-';

/// A section block: `def` indexes [`LaneSong::defs`]; `energy` -3..3 and `take` 0..16 are its chip's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LaneBlock {
    pub start_half: i32,
    pub len_half: i32,
    pub def: usize,
    pub energy: i32,
    pub take: i32,
}

/// A chord on the chord lane (slot 0..111; bass -1 = root, else a key-relative pc).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LaneChord {
    pub start_half: i32,
    pub len_half: i32,
    pub slot: i32,
    pub bass: i32,
}

impl Default for LaneChord {
    fn default() -> Self {
        Self { start_half: 0, len_half: 2, slot: 0, bass: -1 }
    }
}

/// The song as the lanes hold it, in half bars. `defs` carry no chords (the chord lane has them).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaneSong {
    pub defs: Vec<SongSection>,
    pub blocks: Vec<LaneBlock>,
    pub chords: Vec<LaneChord>,
    pub feel: SongFeel,
    pub feel_ignore: u8,
}

/// A half-bar span `[start, end)` on the clipped grid.
#[derive(Clone, Copy, Debug)]
struct Span {
    start: i32,
    end: i32,
}

fn clip(start: i32, len: i32) -> Option<Span> {
    let end = start.saturating_add(len.max(0));
    let (start, end) = (start.clamp(0, MAX_LANE_HALF), end.clamp(0, MAX_LANE_HALF));
    (end > start).then_some(Span { start, end })
}

/// Sorted by start (ties keep list order); each item ends where the next one starts.
fn cut_overlaps<T>(items: &mut Vec<(Span, T)>) {
    items.sort_by_key(|(s, _)| s.start);
    for i in 1..items.len() {
        let next = items.get(i).map(|(s, _)| s.start);
        if let (Some(next), Some((prev, _))) = (next, items.get_mut(i - 1)) {
            prev.end = prev.end.min(next);
        }
    }
    items.retain(|(s, _)| s.end > s.start);
}

struct Pass {
    span: Span,
    arr_pos: i32,
    section: usize,
    energy: i32,
    take: i32,
}

/// The lanes made regular: the timeline's sections (defs A–H, chords empty), the passes covering
/// `[0, end)`, and the chords, sorted and not overlapping.
struct Normal {
    sections: Vec<SongSection>,
    passes: Vec<Pass>,
    chords: Vec<(Span, (i32, i32))>,
    end: i32,
    has_ending: bool,
}

fn normalize(l: &LaneSong) -> Normal {
    // The definitions in A–H order, one per id; `map[i]` = where l.defs[i] went.
    let mut sections: Vec<SongSection> = Vec::new();
    for d in &l.defs {
        if ('A'..='H').contains(&d.id) && !sections.iter().any(|s| s.id == d.id) {
            sections.push(SongSection { chords: Vec::new(), ..d.clone() });
        }
    }
    sections.sort_by_key(|s| s.id);
    sections.truncate(MAX_SECTIONS as usize);
    if sections.is_empty() {
        sections.push(SongSection::default());
    }
    let map: Vec<Option<usize>> = l.defs.iter().map(|d| sections.iter().position(|s| s.id == d.id)).collect();
    let default = sections.iter().position(|s| s.id == 'A').unwrap_or(0);

    let mut blocks: Vec<(Span, (usize, i32, i32))> = l
        .blocks
        .iter()
        .take(MAX_LANE_BLOCKS)
        .filter_map(|b| {
            let sec = map.get(b.def).copied().flatten()?;
            Some((clip(b.start_half, b.len_half)?, (sec, clamp_int(b.energy, -3, 3), clamp_int(b.take, 0, 16))))
        })
        .collect();
    cut_overlaps(&mut blocks);
    let mut chords: Vec<(Span, (i32, i32))> = l
        .chords
        .iter()
        .take(MAX_LANE_CHORDS)
        .filter_map(|c| {
            let bass = if c.bass < 0 { -1 } else { clamp_int(c.bass, 0, 11) };
            Some((clip(c.start_half, c.len_half)?, (clamp_int(c.slot, 0, 111), bass)))
        })
        .collect();
    cut_overlaps(&mut chords);

    // An Ending: the lanes stop after it (later blocks and chords are never reached).
    let mut has_ending = false;
    if let Some(a) = blocks.iter().position(|(_, (sec, _, _))| sections.get(*sec).is_some_and(|s| s.role == 2)) {
        blocks.truncate(a + 1);
        has_ending = true;
    }
    let block_end = blocks.last().map_or(0, |(s, _)| s.end);
    let end = if has_ending { block_end } else { block_end.max(chords.last().map_or(0, |(s, _)| s.end)) };
    chords.retain_mut(|(s, _)| {
        s.end = s.end.min(end);
        s.end > s.start
    });

    let mut passes = Vec::new();
    let mut at = 0;
    let mut gap_pos = blocks.len() as i32;
    let mut gap = |start: i32, end: i32| {
        let p = Pass { span: Span { start, end }, arr_pos: gap_pos, section: default, energy: 0, take: 0 };
        gap_pos += 1;
        p
    };
    for (i, (span, (section, energy, take))) in blocks.iter().enumerate() {
        if at < span.start {
            passes.push(gap(at, span.start));
        }
        passes.push(Pass { span: *span, arr_pos: i as i32, section: *section, energy: *energy, take: *take });
        at = span.end;
    }
    if at < end {
        passes.push(gap(at, end));
    }
    Normal { sections, passes, chords, end, has_ending }
}

/// Where the lanes end (half bars from 0, after an Ending block at the latest); 0 = nothing on them.
pub fn lanes_end_half(l: &LaneSong) -> i32 {
    normalize(l).end
}

fn shape_add(shape: i32, i: i32, n: i32) -> i32 {
    if shape == 1 {
        // build: +1 second half, +2 last quarter
        return if i * 4 >= 3 * n {
            2
        } else if i * 2 >= n {
            1
        } else {
            0
        };
    }
    if shape == 2 {
        // drop: -1 first quarter
        return if i * 4 < n { -1 } else { 0 };
    }
    0
}

/// The span split greedily into 4, 2 and 1 half-bar lengths.
fn split(mut start: i32, end: i32, mut push: impl FnMut(i32, i32)) {
    while start < end {
        let len = if end - start >= 4 {
            4
        } else if end - start >= 2 {
            2
        } else {
            1
        };
        push(start, len);
        start += len;
    }
}

/// The timeline the musicians play; `None` when the lanes hold nothing (the band stays silent).
pub fn lanes_to_timeline(l: &LaneSong) -> Option<Arc<SongTimeline>> {
    let Normal { mut sections, passes, chords, end, has_ending } = normalize(l);
    if end <= 0 || passes.is_empty() {
        return None;
    }
    let silent = sections.len() as i32;
    let mut silent_used = false;

    let mut entries: Vec<TimelineEntry> = Vec::new();
    let mut ci = 0usize;
    let mut first_len: i32 = -1;
    let mut uniform = true;
    let mut in_intro = true;
    let mut intro_half: i32 = 0;
    for pass in &passes {
        let Some(sec) = sections.get(pass.section) else { continue };
        let first = entries.len();
        // The pass's pieces: each chord cut to the pass, the time between them silent.
        let mut at = pass.span.start;
        while at < pass.span.end {
            while chords.get(ci).is_some_and(|(s, _)| s.end <= at) {
                ci += 1;
            }
            let (piece_end, section, slot, bass) = match chords.get(ci) {
                Some((s, (slot, bass))) if s.start <= at => (s.end.min(pass.span.end), pass.section as i32, *slot, *bass),
                next => {
                    silent_used = true;
                    (next.map_or(pass.span.end, |(s, _)| s.start.min(pass.span.end)), silent, 0, -1)
                }
            };
            split(at, piece_end, |start_half, len_half| {
                entries.push(TimelineEntry { start_half, len_half, section, arr_pos: pass.arr_pos, slot, bass, ..TimelineEntry::default() });
            });
            at = piece_end;
        }
        let pass_entries = entries.get_mut(first..).unwrap_or_default();
        let n = pass_entries.len() as i32;
        if n == 0 {
            continue;
        }
        if in_intro && sec.role != 1 {
            in_intro = false;
        }
        for (i, e) in pass_entries.iter_mut().enumerate() {
            let i = i as i32;
            e.chord_index = i;
            e.section_chords = n;
            e.energy_add = pass.energy + shape_add(sec.shape, i, n);
            e.take = if pass.take > 0 { pass.take } else { clamp_int(sec.take, 0, 16) };
            if in_intro {
                intro_half += e.len_half;
            }
            if first_len < 0 {
                first_len = e.len_half;
            } else if e.len_half != first_len {
                uniform = false;
            }
        }
    }
    let total_half = entries.last().map_or(0, |e| e.start_half + e.len_half);
    if intro_half >= total_half {
        intro_half = 0; // an intro-only song simply loops
    }
    if intro_half > 0 || has_ending {
        uniform = false; // the legacy fast path has no intro or ending
    }

    // Each section's chords (for whoever reads a section's chord list, as Ample's slot table does): its
    // first pass as played, then any other chord a later pass plays.
    let mut first_pass: Vec<Option<i32>> = vec![None; sections.len()];
    for e in &entries {
        let (Some(sec), Some(fp)) = (sections.get_mut(e.section as usize), first_pass.get_mut(e.section as usize)) else { continue };
        let first = *fp.get_or_insert(e.arr_pos);
        let fits = (sec.chords.len() as i32) < MAX_CHORDS;
        if fits && (first == e.arr_pos || !sec.chords.iter().any(|c| c.slot == e.slot && c.bass == e.bass)) {
            sec.chords.push(SongChord { slot: e.slot, bass: e.bass, length: legal_length(e.len_half) });
        }
    }
    if silent_used {
        sections.push(SongSection { id: SILENT_SECTION_ID, musicians: [SongMusician { plays: false, energy: 0 }; 6], ..SongSection::default() });
    }

    Some(Arc::new(SongTimeline {
        entries,
        sections,
        total_half,
        arrangement_length: passes.len() as i32,
        uniform_len_half: if uniform { first_len } else { 0 },
        intro_half,
        has_ending,
        meter_override: -1,
        feel: l.feel.clone(),
        feel_ignore: l.feel_ignore,
    }))
}

/// The song laid onto lanes from `start_half`: its arrangement as blocks (as `build_timeline` reads
/// it: unknown ids skipped, 64 chips at most, nothing after an Ending), each pass's chords in turn.
/// A section with no chord takes no time, so it gets no block.
pub fn song_to_lanes(s: &Song, start_half: i32) -> LaneSong {
    let sections: Vec<&SongSection> = s.sections.iter().take(MAX_SECTIONS as usize).collect();
    let defs: Vec<SongSection> = sections.iter().map(|sec| SongSection { chords: Vec::new(), ..(*sec).clone() }).collect();

    let mut order: Vec<(usize, i32, i32)> = Vec::new();
    for (ai, id) in s.arrangement.iter().enumerate() {
        if order.len() as i32 >= MAX_ARRANGEMENT {
            break;
        }
        if let Some(i) = sections.iter().position(|sec| sec.id == *id) {
            let energy = s.arrangement_energy.get(ai).map_or(0, |e| clamp_int(*e, -3, 3));
            let take = s.arrangement_take.get(ai).map_or(0, |k| clamp_int(*k, 0, 16));
            order.push((i, energy, take));
        }
    }
    if order.is_empty() && !sections.is_empty() {
        order.push((0, 0, 0));
    }
    if let Some(a) = order.iter().position(|(i, _, _)| sections.get(*i).is_some_and(|sec| sec.role == 2)) {
        order.truncate(a + 1);
    }

    let mut lanes = LaneSong { defs, feel: s.feel.clone(), feel_ignore: s.feel_ignore, ..LaneSong::default() };
    let mut at = start_half.clamp(0, MAX_LANE_HALF);
    for (def, energy, take) in order {
        let Some(sec) = sections.get(def) else { continue };
        let block_start = at;
        for c in sec.chords.iter().take(MAX_CHORDS as usize) {
            let len_half = legal_length(c.length);
            let bass = if c.bass < 0 { -1 } else { clamp_int(c.bass, 0, 11) };
            lanes.chords.push(LaneChord { start_half: at, len_half, slot: clamp_int(c.slot, 0, 111), bass });
            at = at.saturating_add(len_half);
        }
        if at > block_start {
            lanes.blocks.push(LaneBlock { start_half: block_start, len_half: at - block_start, def, energy, take });
        }
    }
    lanes
}
