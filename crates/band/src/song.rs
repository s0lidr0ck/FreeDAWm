//! Port of `Song.h`.
//!
//! Song — sections, chords with lengths and slash bass, an arrangement (JUCE-free).
//! The processor keeps a Song; buildTimeline() flattens it into an immutable SongTimeline (one
//! entry per chord, in arrangement order) that the Musicians read on the audio thread.
//! LegacySong turns the Amorph song endpoints (progression, bars, chord1..8, chordbars) into the
//! one-section song that plays exactly like them: old sessions migrate through it, and the A/B
//! drives the song path through it.

use std::sync::Arc;

use crate::presets::PROGRESSION_PRESETS;
use crate::song_feel::{FeelOverride, SongFeel};

pub const MAX_SECTIONS: i32 = 8;
pub const MAX_CHORDS: i32 = 32;
pub const MAX_ARRANGEMENT: i32 = 64;

/// slot 0..111; bass -1 = root, else key-relative pc; length in half-bars 1/2/4
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SongChord {
    pub slot: i32,
    pub bass: i32,
    pub length: i32,
}

impl Default for SongChord {
    fn default() -> Self {
        Self { slot: 0, bass: -1, length: 2 }
    }
}

/// energy 0 = follow the shift, 1..4 fixed
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SongMusician {
    pub plays: bool,
    pub energy: i32,
}

impl Default for SongMusician {
    fn default() -> Self {
        Self { plays: true, energy: 0 }
    }
}

/// A kept take (Phase 5d): what one musician played over one pass of a section, replayed instead of generated.
/// ppq from the section's start; status without a channel (the musician's channel is applied when played).
/// ppq: full precision, so a take replays to the sample.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrozenEvent {
    pub ppq: f64,
    pub status: u8,
    pub d1: u8,
    pub d2: u8,
    pub keyswitch: bool,
}

/// meter: the Meter it was played in
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrozenPart {
    pub events: Vec<FrozenEvent>,
    pub length_ppq: f64,
    pub meter: u8,
}

pub const MAX_FROZEN_EVENTS: i32 = 4000;

#[derive(Clone, Debug, PartialEq)]
pub struct SongSection {
    pub id: char,
    pub name: String,
    /// -3..+3
    pub energy_shift: i32,
    /// the Original progression: chords follow each musician's Idea
    pub follow_idea: bool,
    /// 0 normal, 1 intro (played once, skipped when looping), 2 ending (the song stops after it)
    pub role: i32,
    /// 0 none, 1 build (+1 second half, +2 last quarter), 2 drop (-1 first quarter)
    pub shape: i32,
    /// 0 = the song's Take (the Take control), 1..16 = this section's own
    pub take: i32,
    pub musicians: [SongMusician; 6],
    pub chords: Vec<SongChord>,
    /// kept takes; None = plays live
    pub frozen: [Option<Arc<FrozenPart>>; 6],
    /// the section's own feel dials (-1 = the song's)
    pub feel_override: FeelOverride,
}

impl Default for SongSection {
    fn default() -> Self {
        Self {
            id: 'A',
            name: String::new(),
            energy_shift: 0,
            follow_idea: false,
            role: 0,
            shape: 0,
            take: 0,
            musicians: [SongMusician::default(); 6],
            chords: Vec::new(),
            frozen: Default::default(),
            feel_override: FeelOverride::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Song {
    pub sections: Vec<SongSection>,
    /// section ids; empty = the first section, looping
    pub arrangement: Vec<char>,
    /// per arrangement entry, -3..+3 (missing = 0)
    pub arrangement_energy: Vec<i32>,
    /// per arrangement entry: this pass's own Take, 1..16 (missing / 0 = the section's)
    pub arrangement_take: Vec<i32>,
    /// -1 = follow the host, else a Meter (time signatures)
    pub meter_override: i32,
    /// the song's feel (off = as the musicians are set)
    pub feel: SongFeel,
    /// bit k: musician k doesn't follow the song's feel
    pub feel_ignore: u8,
}

impl Default for Song {
    fn default() -> Self {
        Self {
            sections: Vec::new(),
            arrangement: Vec::new(),
            arrangement_energy: Vec::new(),
            arrangement_take: Vec::new(),
            meter_override: -1,
            feel: SongFeel::default(),
            feel_ignore: 0,
        }
    }
}

impl Song {
    pub fn find(&self, id: char) -> Option<&SongSection> {
        self.sections.iter().find(|s| s.id == id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimelineEntry {
    /// in half-bars
    pub start_half: i32,
    pub len_half: i32,
    /// index into SongTimeline::sections
    pub section: i32,
    /// within the section
    pub chord_index: i32,
    pub section_chords: i32,
    /// position in the arrangement
    pub arr_pos: i32,
    pub slot: i32,
    pub bass: i32,
    /// the arrangement chip's energy + the section's Build / Drop at this chord
    pub energy_add: i32,
    /// this pass's Take (the chip's, else the section's); 0 = the Take control
    pub take: i32,
}

impl Default for TimelineEntry {
    fn default() -> Self {
        Self { start_half: 0, len_half: 2, section: 0, chord_index: 0, section_chords: 1, arr_pos: 0, slot: 0, bass: -1, energy_add: 0, take: 0 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SongTimeline {
    pub entries: Vec<TimelineEntry>,
    pub sections: Vec<SongSection>,
    pub total_half: i32,
    pub arrangement_length: i32,
    /// > 0 when every chord has this length (and no intro / ending): the legacy arithmetic applies
    pub uniform_len_half: i32,
    /// the intro: played once; loops wrap within [introHalf, totalHalf)
    pub intro_half: i32,
    /// the song stops at totalHalf (no loop)
    pub has_ending: bool,
    /// the song's meter: -1 = follow the host, else a Meter
    pub meter_override: i32,
    /// the song's feel, and who doesn't follow it (sections carry their own)
    pub feel: SongFeel,
    pub feel_ignore: u8,
}

impl Default for SongTimeline {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            sections: Vec::new(),
            total_half: 0,
            arrangement_length: 1,
            uniform_len_half: 0,
            intro_half: 0,
            has_ending: false,
            meter_override: -1,
            feel: SongFeel::default(),
            feel_ignore: 0,
        }
    }
}

impl SongTimeline {
    /// The entry containing half-bar position h, 0 <= h < totalHalf (clamped).
    pub fn entry_at_half(&self, h: f64) -> i32 {
        if self.entries.is_empty() {
            return 0;
        }
        let mut lo: i32 = 0;
        let mut hi: i32 = self.entries.len() as i32 - 1;
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            let start = self.entries.get(mid as usize).map_or(0, |e| e.start_half);
            if f64::from(start) <= h {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    }

    /// `sections[entries[i].section].id`, or `'\0'` (C++ char 0) when either index is out of range.
    pub fn section_id_of(&self, entry: usize) -> char {
        self.entries.get(entry).and_then(|e| usize::try_from(e.section).ok()).and_then(|s| self.sections.get(s)).map_or('\0', |s| s.id)
    }
}

pub fn clamp_int(v: i32, lo: i32, hi: i32) -> i32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}
#[allow(clippy::if_same_then_else)] // the C++ chain: 3 rounds up to 4
pub fn legal_length(l: i32) -> i32 {
    if l <= 1 {
        1
    } else if l >= 4 {
        4
    } else if l == 3 {
        4
    } else {
        2
    }
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

/// Never empty: a song with no playable chord becomes one I chord of one bar.
pub fn build_timeline(song: &Song) -> Arc<SongTimeline> {
    let mut t = SongTimeline {
        sections: song.sections.clone(),
        meter_override: if song.meter_override >= 0 && song.meter_override <= 2 { song.meter_override } else { -1 },
        feel: song.feel.clone(),
        feel_ignore: song.feel_ignore,
        ..SongTimeline::default()
    };
    for s in &mut t.sections {
        s.chords.truncate(MAX_CHORDS as usize);
        for c in &mut s.chords {
            c.slot = clamp_int(c.slot, 0, 111);
            c.bass = if c.bass < 0 { -1 } else { clamp_int(c.bass, 0, 11) };
            c.length = legal_length(c.length);
        }
    }
    t.sections.truncate(MAX_SECTIONS as usize);
    if t.sections.is_empty() {
        let s = SongSection { chords: vec![SongChord::default()], ..SongSection::default() };
        t.sections.push(s);
    }

    let mut order: Vec<i32> = Vec::new();
    let mut chip_energy: Vec<i32> = Vec::new();
    let mut chip_take: Vec<i32> = Vec::new();
    for (ai, id) in song.arrangement.iter().enumerate() {
        if order.len() as i32 >= MAX_ARRANGEMENT {
            break;
        }
        if let Some(i) = t.sections.iter().position(|s| s.id == *id) {
            order.push(i as i32);
            chip_energy.push(song.arrangement_energy.get(ai).map_or(0, |e| clamp_int(*e, -3, 3)));
            chip_take.push(song.arrangement_take.get(ai).map_or(0, |k| clamp_int(*k, 0, 16)));
        }
    }
    if order.is_empty() {
        order.push(0);
        chip_energy.push(0);
        chip_take.push(0);
    }
    // An Ending: the arrangement stops after it (later chips are never reached).
    for a in 0..order.len() {
        if t.sections.get(order[a] as usize).is_some_and(|s| s.role == 2) {
            order.truncate(a + 1);
            chip_energy.truncate(a + 1);
            chip_take.truncate(a + 1);
            t.has_ending = true;
            break;
        }
    }

    let mut start: i32 = 0;
    let mut first_len: i32 = -1;
    let mut uniform = true;
    let mut in_intro = true;
    let mut intro_half: i32 = 0;
    for (a, sec_index) in order.iter().enumerate() {
        let Some(sec) = t.sections.get(*sec_index as usize) else { continue };
        if sec.chords.is_empty() {
            continue;
        }
        let n = sec.chords.len() as i32;
        if in_intro && sec.role != 1 {
            in_intro = false;
        }
        for (i, c) in sec.chords.iter().enumerate() {
            let i = i as i32;
            let chip_e = chip_energy.get(a).copied().unwrap_or(0);
            let chip_t = chip_take.get(a).copied().unwrap_or(0);
            let e = TimelineEntry {
                start_half: start,
                len_half: c.length,
                section: *sec_index,
                chord_index: i,
                section_chords: n,
                arr_pos: a as i32,
                slot: c.slot,
                bass: c.bass,
                energy_add: chip_e + shape_add(sec.shape, i, n),
                take: if chip_t > 0 { chip_t } else { clamp_int(sec.take, 0, 16) },
            };
            t.entries.push(e);
            start += c.length;
            if in_intro {
                intro_half += c.length;
            }
            if first_len < 0 {
                first_len = c.length;
            } else if c.length != first_len {
                uniform = false;
            }
        }
    }
    if intro_half >= start {
        intro_half = 0; // an intro-only song simply loops
    }
    t.intro_half = intro_half;
    if intro_half > 0 || t.has_ending {
        uniform = false; // the legacy fast path has no intro or ending
    }
    if t.entries.is_empty() {
        let e = TimelineEntry { section: order.first().copied().unwrap_or(0), ..TimelineEntry::default() };
        t.entries.push(e);
        start = e.len_half;
        first_len = e.len_half;
    }
    t.total_half = start;
    t.arrangement_length = order.len() as i32;
    t.uniform_len_half = if uniform { first_len } else { 0 };
    Arc::new(t)
}

/// Where ppq falls in the song: entry index, whole loops, global chord ordinal, the entry's span (ppq).
/// barLen is the bar in ppq. Every chord one length (and not forced general): exactly the legacy
/// division `floor ((ppq + 0.0001) / chordLen)`, so old songs play bit-identically.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SongLoc {
    pub idx: i32,
    /// `loop` in C++ (a Rust keyword).
    pub loop_: i32,
    pub global: i32,
    pub start: f64,
    pub end: f64,
    pub ended: bool,
}

pub fn chord_len_for(len_half: i32, bar_len: f64) -> f64 {
    if len_half == 1 {
        bar_len * 0.5
    } else if len_half == 4 {
        bar_len * 2.0
    } else {
        bar_len
    }
}
/// `(int) std::floor (v)` (saturating where C++ is undefined).
pub fn floor_to_int_d(v: f64) -> i32 {
    v.floor() as i32
}
pub fn floor_div_i(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    let mut q = a.wrapping_div(b);
    if (a.wrapping_rem(b) != 0) && ((a < 0) != (b < 0)) {
        q -= 1;
    }
    q
}
pub fn pos_mod_i(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    let m = a.wrapping_rem(b);
    if m < 0 { m + b } else { m }
}

/// barOrigin: where bar 0 of this bar length would be (a mid-play meter change re-anchors the grid; bars keep counting).
pub fn song_locate(t: &SongTimeline, ppq: f64, bar_len: f64, force_general: bool, bar_origin: f64) -> SongLoc {
    if bar_origin != 0.0 {
        let mut o = song_locate(t, ppq - bar_origin, bar_len, force_general, 0.0);
        o.start += bar_origin;
        o.end += bar_origin;
        return o;
    }
    let e_count = t.entries.len() as i32;
    let mut l = SongLoc::default();
    // build_timeline never makes an empty timeline; a hand-made one locates to nothing.
    if e_count == 0 {
        return l;
    }
    if t.uniform_len_half > 0 && !force_general {
        let len = chord_len_for(t.uniform_len_half, bar_len);
        l.global = floor_to_int_d((ppq + 0.0001) / len);
        l.loop_ = floor_div_i(l.global, e_count);
        l.idx = pos_mod_i(l.global, e_count);
        l.start = f64::from(l.global) * len;
        l.end = l.start + len;
        return l;
    }
    let half = bar_len * 0.5;
    let h = (ppq + 0.0001) / half;
    let entry = |i: i32| t.entries.get(i as usize).copied().unwrap_or_default();
    if t.has_ending {
        // Played once, then over.
        if h >= f64::from(t.total_half) {
            l.idx = e_count - 1;
            l.ended = true;
            l.start = f64::from(t.total_half) * half;
            l.end = l.start;
            l.global = e_count;
            return l;
        }
        if h < 0.0 {
            // a pre-roll: silent until bar 1
            l.idx = 0;
            l.ended = true;
            l.start = 0.0;
            l.end = 0.0;
            l.global = 0;
            return l;
        }
        l.idx = t.entry_at_half(h);
        l.global = l.idx;
        let e = entry(l.idx);
        l.start = f64::from(e.start_half) * half;
        l.end = l.start + f64::from(e.len_half) * half;
        return l;
    }
    if t.intro_half > 0 {
        // The intro once, then the rest loops.
        let body = f64::from(t.total_half - t.intro_half);
        let mut within = h;
        let mut k = 0;
        if h >= f64::from(t.intro_half) {
            k = floor_to_int_d((h - f64::from(t.intro_half)) / body);
            within = h - f64::from(k) * body;
        }
        l.loop_ = k;
        l.idx = t.entry_at_half(within);
        l.global = l.loop_.wrapping_mul(e_count).wrapping_add(l.idx);
        let e = entry(l.idx);
        l.start = (f64::from(k) * body + f64::from(e.start_half)) * half;
        l.end = l.start + f64::from(e.len_half) * half;
        return l;
    }
    l.loop_ = floor_to_int_d(h / f64::from(t.total_half));
    l.idx = t.entry_at_half(h - f64::from(l.loop_) * f64::from(t.total_half));
    l.global = l.loop_.wrapping_mul(e_count).wrapping_add(l.idx);
    let e = entry(l.idx);
    l.start = (f64::from(l.loop_) * f64::from(t.total_half) + f64::from(e.start_half)) * half;
    l.end = l.start + f64::from(e.len_half) * half;
    l
}

/// Song edits for the UI. Every one names its section by id (not by position), re-checks the song it
/// is given, and returns false — changing nothing — when the request no longer fits: a menu opened
/// before the song changed can never delete the wrong thing. The last section and a section's last
/// chord can't be deleted; the limits (8 sections, 32 chords, 64 arrangement entries) always hold.
pub mod songedit {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use super::{FrozenPart, MAX_ARRANGEMENT, MAX_CHORDS, MAX_SECTIONS, Song, SongChord, SongSection, clamp_int};
    use crate::song_feel::{FeelOverride, SongFeel};

    pub fn find(s: &mut Song, id: char) -> Option<&mut SongSection> {
        s.sections.iter_mut().find(|x| x.id == id)
    }
    /// The first free id A..H, or None (C++ 0).
    pub fn free_id(s: &Song) -> Option<char> {
        ('A'..='H').find(|c| s.find(*c).is_none())
    }

    fn four_bars_of_i() -> Vec<SongChord> {
        vec![SongChord { slot: 0, bass: -1, length: 4 }; 4]
    }

    /// An Original (followIdea) section becomes ordinary chords (the musician's Idea roots) before an edit.
    pub fn materialize(sec: &mut SongSection, idea: i32) {
        if !sec.follow_idea {
            return;
        }
        const ROOTS: [[i32; 4]; 4] = [[0, 5, 3, 4], [0, 3, 5, 4], [5, 3, 0, 4], [0, 1, 3, 4]];
        let row = ROOTS[clamp_int(idea, 0, 3) as usize];
        for (i, c) in sec.chords.iter_mut().enumerate() {
            c.slot = row[i % 4] * 16;
        }
        sec.follow_idea = false;
    }
    /// Chip energies always run parallel to the chips (missing ones are 0).
    pub fn sync_energy(s: &mut Song) {
        s.arrangement_energy.resize(s.arrangement.len(), 0);
        s.arrangement_take.resize(s.arrangement.len(), 0);
    }
    /// Keeps what played when the implicit arrangement ("the first section, looping") becomes explicit.
    pub fn make_arrangement_explicit(s: &mut Song) {
        if s.arrangement.is_empty()
            && let Some(first) = s.sections.first()
        {
            s.arrangement.push(first.id);
        }
        sync_energy(s);
    }

    /// The new section's id, or None (nothing changed).
    pub fn add_section(s: &mut Song, name: &str) -> Option<char> {
        let id = free_id(s)?;
        if s.sections.len() as i32 >= MAX_SECTIONS {
            return None;
        }
        let sec = SongSection { id, name: name.to_string(), chords: four_bars_of_i(), ..SongSection::default() };
        make_arrangement_explicit(s);
        s.sections.push(sec);
        if (s.arrangement.len() as i32) < MAX_ARRANGEMENT {
            s.arrangement.push(id);
            sync_energy(s);
        }
        Some(id)
    }
    pub fn duplicate_section(s: &mut Song, from: char) -> Option<char> {
        let src = s.find(from)?.clone();
        let id = free_id(s)?;
        if s.sections.len() as i32 >= MAX_SECTIONS {
            return None;
        }
        let mut c = src;
        c.id = id;
        c.name.push_str(" 2");
        if c.role == 2 {
            c.role = 0; // only one Ending
        }
        make_arrangement_explicit(s);
        s.sections.push(c);
        if (s.arrangement.len() as i32) < MAX_ARRANGEMENT {
            s.arrangement.push(id);
            sync_energy(s);
        }
        Some(id)
    }
    pub fn delete_section(s: &mut Song, id: char) -> bool {
        if s.sections.len() <= 1 {
            return false;
        }
        let Some(pos) = s.sections.iter().position(|x| x.id == id) else { return false };
        s.sections.remove(pos);
        sync_energy(s);
        for i in (0..s.arrangement.len()).rev() {
            if s.arrangement[i] == id {
                s.arrangement.remove(i);
                s.arrangement_energy.remove(i);
                s.arrangement_take.remove(i);
            }
        }
        true
    }
    /// A transpose: every kept take of a `tuned` musician moves by semitones (keyswitches stay; a note pushed past
    /// 0..127 goes an octave back). False when nothing moved.
    pub fn transpose_kept(s: &mut Song, semitones: i32, tuned: &[bool; 6]) -> bool {
        let mut moved = false;
        for sec in &mut s.sections {
            for (k, slot) in sec.frozen.iter_mut().enumerate() {
                let Some(f) = slot.as_ref() else { continue };
                if !tuned[k] {
                    continue;
                }
                let mut fp: FrozenPart = (**f).clone();
                for e in &mut fp.events {
                    let ty = e.status & 0xF0;
                    if e.keyswitch || (ty != 0x80 && ty != 0x90 && ty != 0xA0) {
                        continue;
                    }
                    let mut n = i32::from(e.d1).saturating_add(semitones);
                    // Two steps of rem_euclid, as the C++ while loops (an octave at a time) end up in 116..127 / 0..11.
                    if n > 127 {
                        n = 116 + (n - 116).rem_euclid(12);
                    }
                    if n < 0 {
                        n = n.rem_euclid(12);
                    }
                    e.d1 = n as u8;
                }
                *slot = Some(Arc::new(fp));
                moved = true;
            }
        }
        moved
    }
    pub fn clear_section(s: &mut Song, id: char) -> bool {
        let Some(sec) = find(s, id) else { return false };
        sec.follow_idea = false;
        sec.chords = four_bars_of_i();
        true
    }
    pub fn rename_section(s: &mut Song, id: char, name: &str) -> bool {
        let Some(sec) = find(s, id) else { return false };
        if name.is_empty() {
            return false;
        }
        sec.name = name.to_string();
        true
    }
    /// Adds a chord at the end (a copy of the last, without its slash bass).
    pub fn append_chord(s: &mut Song, id: char, idea: i32) -> bool {
        let Some(sec) = find(s, id) else { return false };
        if sec.chords.len() as i32 >= MAX_CHORDS {
            return false;
        }
        materialize(sec, idea);
        let mut c = sec.chords.last().copied().unwrap_or_default();
        c.bass = -1;
        sec.chords.push(c);
        true
    }
    /// After chord i: a copy of it (copy) or a I chord of its length.
    pub fn insert_chord(s: &mut Song, id: char, i: i32, copy: bool, idea: i32) -> bool {
        let Some(sec) = find(s, id) else { return false };
        if i < 0 || i >= sec.chords.len() as i32 || sec.chords.len() as i32 >= MAX_CHORDS {
            return false;
        }
        materialize(sec, idea);
        let Some(at) = sec.chords.get(i as usize).copied() else { return false };
        let c = if copy { at } else { SongChord { slot: 0, bass: -1, length: at.length } };
        sec.chords.insert(i as usize + 1, c);
        true
    }
    /// At position `at` (0..size), as many as fit (32 per section); `inserted` = how many went in.
    pub fn insert_chords(s: &mut Song, id: char, at: i32, chords: &[SongChord], inserted: &mut i32, idea: i32) -> bool {
        *inserted = 0;
        let Some(sec) = find(s, id) else { return false };
        if at < 0 || at > sec.chords.len() as i32 {
            return false;
        }
        materialize(sec, idea);
        for c in chords {
            if sec.chords.len() as i32 >= MAX_CHORDS {
                break;
            }
            let mut c = *c;
            c.slot = clamp_int(c.slot, 0, 111);
            c.bass = clamp_int(c.bass, -1, 11);
            if c.length != 1 && c.length != 2 && c.length != 4 {
                c.length = 2;
            }
            sec.chords.insert((at + *inserted) as usize, c);
            *inserted += 1;
        }
        *inserted > 0
    }
    /// The chords at those positions; a section always keeps at least one chord.
    pub fn remove_chords(s: &mut Song, id: char, which: &BTreeSet<i32>, idea: i32) -> bool {
        let Some(sec) = find(s, id) else { return false };
        let Some(first) = which.iter().next().copied() else { return false };
        materialize(sec, idea); // an Original section: the chords as shown, then the removal
        let mut kept: Vec<SongChord> = Vec::new();
        for (i, c) in sec.chords.iter().enumerate() {
            if !which.contains(&(i as i32)) {
                kept.push(*c);
            }
        }
        if kept.is_empty() {
            let pick = usize::try_from(first).ok().filter(|f| *f < sec.chords.len()).unwrap_or(0);
            if let Some(c) = sec.chords.get(pick) {
                kept.push(*c);
            }
        }
        if kept.len() == sec.chords.len() {
            return false;
        }
        sec.chords = kept;
        true
    }
    /// Chord i becomes slot / bass; its length stays.
    pub fn replace_chord(s: &mut Song, id: char, i: i32, slot: i32, bass: i32, idea: i32) -> bool {
        let Some(sec) = find(s, id) else { return false };
        if i < 0 || i >= sec.chords.len() as i32 {
            return false;
        }
        materialize(sec, idea);
        let Some(c) = sec.chords.get_mut(i as usize) else { return false };
        c.slot = clamp_int(slot, 0, 111);
        c.bass = clamp_int(bass, -1, 11);
        true
    }
    pub fn delete_chord(s: &mut Song, id: char, i: i32) -> bool {
        let Some(sec) = find(s, id) else { return false };
        if i < 0 || i >= sec.chords.len() as i32 || sec.chords.len() <= 1 {
            return false;
        }
        sec.chords.remove(i as usize);
        true
    }
    pub fn edit_chord<F: FnOnce(&mut SongChord)>(s: &mut Song, id: char, i: i32, idea: i32, f: F) -> bool {
        let Some(sec) = find(s, id) else { return false };
        if i < 0 || i >= sec.chords.len() as i32 {
            return false;
        }
        materialize(sec, idea);
        let Some(c) = sec.chords.get_mut(i as usize) else { return false };
        f(c);
        true
    }
    pub fn append_arrangement(s: &mut Song, id: char) -> bool {
        if s.find(id).is_none() {
            return false;
        }
        make_arrangement_explicit(s);
        if s.arrangement.len() as i32 >= MAX_ARRANGEMENT {
            return false;
        }
        s.arrangement.push(id);
        sync_energy(s);
        true
    }
    /// Removes chip i only if it still shows the section the user clicked (`expect`).
    pub fn remove_arrangement_at(s: &mut Song, i: i32, expect: char) -> bool {
        if i < 0 || i >= s.arrangement.len() as i32 || s.arrangement.get(i as usize) != Some(&expect) {
            return false;
        }
        sync_energy(s);
        let i = i as usize;
        s.arrangement.remove(i);
        s.arrangement_energy.remove(i);
        s.arrangement_take.remove(i);
        true
    }
    pub fn move_arrangement(s: &mut Song, i: i32, expect: char, to: i32) -> bool {
        if i < 0 || i >= s.arrangement.len() as i32 || s.arrangement.get(i as usize) != Some(&expect) {
            return false;
        }
        let to = clamp_int(to, 0, s.arrangement.len() as i32 - 1);
        if to == i {
            return false;
        }
        sync_energy(s);
        let (i, to) = (i as usize, to as usize);
        let en = s.arrangement_energy[i];
        let tk = s.arrangement_take[i];
        s.arrangement.remove(i);
        s.arrangement.insert(to, expect);
        s.arrangement_energy.remove(i);
        s.arrangement_energy.insert(to, en);
        s.arrangement_take.remove(i);
        s.arrangement_take.insert(to, tk);
        true
    }
    /// Intro / Ending / normal. Only one section can be the Ending.
    pub fn set_role(s: &mut Song, id: char, role: i32) -> bool {
        match find(s, id) {
            Some(sec) if (0..=2).contains(&role) && sec.role != role => {}
            _ => return false,
        }
        if role == 2 {
            for o in &mut s.sections {
                if o.role == 2 {
                    o.role = 0;
                }
            }
        }
        if let Some(sec) = find(s, id) {
            sec.role = role;
        }
        true
    }
    pub fn set_shape(s: &mut Song, id: char, shape: i32) -> bool {
        let Some(sec) = find(s, id) else { return false };
        if !(0..=2).contains(&shape) || sec.shape == shape {
            return false;
        }
        sec.shape = shape;
        true
    }
    /// The song's feel. False (and nothing changes) when it is already that.
    pub fn set_feel(s: &mut Song, f: &SongFeel) -> bool {
        if s.feel == *f {
            return false;
        }
        s.feel = f.clone();
        true
    }
    /// A section's own feel dials.
    pub fn set_section_feel(s: &mut Song, id: char, o: &FeelOverride) -> bool {
        let Some(sec) = find(s, id) else { return false };
        if sec.feel_override == *o {
            return false;
        }
        sec.feel_override = *o;
        true
    }
    /// Whether musician k follows the song's feel.
    pub fn set_feel_follow(s: &mut Song, k: i32, follows: bool) -> bool {
        if !(0..6).contains(&k) {
            return false;
        }
        let bit = 1u8 << k;
        let next = if follows { s.feel_ignore & !bit } else { s.feel_ignore | bit };
        if next == s.feel_ignore {
            return false;
        }
        s.feel_ignore = next;
        true
    }
    /// A section's own Take (0 = the song's).
    pub fn set_section_take(s: &mut Song, id: char, take: i32) -> bool {
        let Some(sec) = find(s, id) else { return false };
        let take = clamp_int(take, 0, 16);
        if sec.take == take {
            return false;
        }
        sec.take = take;
        true
    }
    fn explicit_for_first_chip(s: &mut Song, i: i32, expect: char) {
        if s.arrangement.is_empty() && i == 0 && s.sections.first().is_some_and(|f| f.id == expect) {
            make_arrangement_explicit(s);
        }
    }
    /// Chip i's own Take (0 = the section's), if chip i still shows the section the user touched.
    pub fn set_chip_take(s: &mut Song, i: i32, expect: char, take: i32) -> bool {
        explicit_for_first_chip(s, i, expect);
        if i < 0 || i >= s.arrangement.len() as i32 || s.arrangement.get(i as usize) != Some(&expect) {
            return false;
        }
        sync_energy(s);
        let take = clamp_int(take, 0, 16);
        let Some(t) = s.arrangement_take.get_mut(i as usize) else { return false };
        if *t == take {
            return false;
        }
        *t = take;
        true
    }
    /// Chip i's energy, if chip i still shows the section the user touched.
    pub fn set_chip_energy(s: &mut Song, i: i32, expect: char, energy: i32) -> bool {
        explicit_for_first_chip(s, i, expect);
        if i < 0 || i >= s.arrangement.len() as i32 || s.arrangement.get(i as usize) != Some(&expect) {
            return false;
        }
        sync_energy(s);
        let energy = clamp_int(energy, -3, 3);
        let Some(e) = s.arrangement_energy.get_mut(i as usize) else { return false };
        if *e == energy {
            return false;
        }
        *e = energy;
        true
    }
}

/// The Amorph song endpoints with the engine's own update rules (Musician on_param13/14/15-22/34).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacySong {
    pub progression: i32,
    pub bars_mode: i32,
    pub chord_bars_mode: i32,
    pub slots: [i32; 8],
}

impl Default for LegacySong {
    fn default() -> Self {
        Self { progression: 17, bars_mode: 0, chord_bars_mode: 2, slots: [0, 3, 80, 48, 0, 3, 80, 48] }
    }
}

impl LegacySong {
    /// True if `endpoint` (a Band endpoint name) was a song endpoint.
    pub fn apply(&mut self, endpoint: &str, v: f32) -> bool {
        if endpoint == "progression" {
            self.progression = clamp_int(v as i32, 0, 17);
            if (1..=16).contains(&self.progression)
                && let Some(preset) = PROGRESSION_PRESETS.get((self.progression - 1) as usize)
            {
                self.slots = preset.slots;
                self.bars_mode = if preset.bars == 8 { 1 } else { 0 };
            }
            return true;
        }
        if endpoint == "bars" {
            self.bars_mode = clamp_int(v as i32, 0, 1);
            return true;
        }
        if endpoint == "chordbars" {
            self.chord_bars_mode = clamp_int(v as i32, 0, 2);
            return true;
        }
        let b = endpoint.as_bytes();
        if b.len() == 6 && endpoint.starts_with("chord") && (b'1'..=b'8').contains(&b[5]) {
            self.slots[usize::from(b[5] - b'1')] = clamp_int(v as i32, 0, 111);
            return true;
        }
        false
    }

    pub fn to_song(&self) -> Song {
        let mut s = SongSection { id: 'A', name: "Verse".to_string(), follow_idea: self.progression == 0, ..SongSection::default() };
        let n = if self.bars_mode == 1 { 8 } else { 4 };
        let len = if self.chord_bars_mode == 0 {
            1
        } else if self.chord_bars_mode == 2 {
            4
        } else {
            2
        };
        for slot in self.slots.iter().take(n) {
            s.chords.push(SongChord { slot: *slot, bass: -1, length: len });
        }
        let mut song = Song::default();
        song.sections.push(s);
        song
    }
}

#[cfg(test)]
mod tests {
    use super::songedit::*;
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn legacy_song_and_locate() {
        let mut l = LegacySong::default();
        assert!(l.apply("progression", 6.0));
        assert_eq!(l.bars_mode, 1);
        assert!(l.apply("chord3", 200.0) && l.slots[2] == 111);
        assert!(!l.apply("chord9", 1.0));
        let t = build_timeline(&l.to_song());
        assert_eq!((t.entries.len(), t.uniform_len_half, t.total_half), (8, 4, 32));
        let loc = song_locate(&t, 8.0 * 4.0 * 2.0 + 0.5, 4.0, false, 0.0);
        assert_eq!((loc.idx, loc.loop_, loc.global), (0, 1, 8));
        let g = song_locate(&t, 8.0 * 4.0 * 2.0 + 0.5, 4.0, true, 0.0);
        assert_eq!((g.idx, g.loop_, g.global, g.start), (0, 1, 8, 64.0));
        assert_eq!(floor_div_i(-7, 2), -4);
        assert_eq!(pos_mod_i(-7, 2), 1);
    }

    #[test]
    fn edits_hold_the_limits() {
        let mut s = LegacySong::default().to_song();
        let b = add_section(&mut s, "Chorus");
        assert_eq!(b, Some('B'));
        assert_eq!(s.arrangement, ['A', 'B']);
        assert!(set_chip_energy(&mut s, 1, 'B', 9) && s.arrangement_energy == [0, 3]);
        assert!(move_arrangement(&mut s, 1, 'B', 0) && s.arrangement == ['B', 'A'] && s.arrangement_energy == [3, 0]);
        assert!(!remove_arrangement_at(&mut s, 0, 'A'));
        assert!(set_role(&mut s, 'B', 2) && set_role(&mut s, 'A', 2));
        assert_eq!(s.find('B').map(|x| x.role), Some(0));
        assert!(delete_section(&mut s, 'B') && !delete_section(&mut s, 'A'));
        assert!(s.arrangement == ['A'] && s.arrangement_energy == [0]);
        let all: BTreeSet<i32> = (0..10).collect();
        assert!(remove_chords(&mut s, 'A', &all, 0));
        assert_eq!(s.sections[0].chords.len(), 1);
        assert!(!delete_chord(&mut s, 'A', 0));
        let mut n = 0;
        assert!(insert_chords(&mut s, 'A', 1, &[SongChord { slot: 500, bass: 40, length: 3 }; 40], &mut n, 0));
        assert_eq!(n, 31);
        assert_eq!(s.sections[0].chords[1], SongChord { slot: 111, bass: 11, length: 2 });
        assert!(set_feel_follow(&mut s, 3, false) && s.feel_ignore == 8 && !set_feel_follow(&mut s, 6, false));
    }

    #[test]
    fn transpose_wraps_by_octaves() {
        let mut s = LegacySong::default().to_song();
        let part = FrozenPart {
            events: vec![
                FrozenEvent { ppq: 0.0, status: 0x90, d1: 125, d2: 100, keyswitch: false },
                FrozenEvent { ppq: 0.0, status: 0x90, d1: 2, d2: 100, keyswitch: true },
                FrozenEvent { ppq: 0.0, status: 0x80, d1: 3, d2: 0, keyswitch: false },
            ],
            length_ppq: 16.0,
            meter: 0,
        };
        s.sections[0].frozen[1] = Some(Arc::new(part));
        let mut tuned = [false; 6];
        assert!(!transpose_kept(&mut s, 5, &tuned));
        tuned[1] = true;
        assert!(transpose_kept(&mut s, 5, &tuned));
        let ev: Vec<u8> = s.sections[0].frozen[1].as_ref().map(|f| f.events.iter().map(|e| e.d1).collect()).unwrap_or_default();
        assert_eq!(ev, [118, 2, 8]);
        assert!(transpose_kept(&mut s, -30, &tuned));
        let ev: Vec<u8> = s.sections[0].frozen[1].as_ref().map(|f| f.events.iter().map(|e| e.d1).collect()).unwrap_or_default();
        assert_eq!(ev, [88, 2, 2]);
    }
}
