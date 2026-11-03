//! Port of `Tension.h`.
//!
//! Tension and release (Band feel): how much harmonic pull a chord of the song has toward the next, and whether a
//! chord is where that pull resolves. Pure functions over the song timeline, no allocation (audio thread).
//!   dominant to its resolution (V / V7 -> I, any dom7 a 5th above the next root)  0.8
//!   diminished                                                                      0.7
//!   sus4 resolving (to its own root or down a 5th)                                  0.5
//!   borrowed iv / bVI / bVII                                                        0.4
//!   the last chord before a section with more Energy                               +0.3 (max 1)
//! A release is the chord after a tension of 0.5 or more that resolves (root down a 5th, or I) or starts a new pass.

use crate::song::{SongTimeline, TimelineEntry};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChordFn {
    pub degree: i32,
    pub flavor: i32,
    pub root_pc: i32,
    pub third: i32,
    pub fifth: i32,
}

impl Default for ChordFn {
    fn default() -> Self {
        Self { degree: 0, flavor: 0, root_pc: 0, third: 4, fifth: 7 }
    }
}

pub fn scale_offset(mode: i32, degree: i32) -> i32 {
    const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
    const MINOR: [i32; 7] = [0, 2, 3, 5, 7, 8, 10];
    let d = degree.rem_euclid(7); // ((degree % 7) + 7) % 7
    let oct = (degree - d) / 7;
    let row = if mode == 0 { &MAJOR } else { &MINOR };
    row[d as usize] + 12 * oct
}

/// A slot (degree * 16 + flavor) as the engine builds it (Musician::buildChord), relative to the key.
pub fn chord_fn(slot: i32, scale_mode: i32) -> ChordFn {
    let mut c = ChordFn::default();
    let v = slot.clamp(0, 111);
    c.degree = (v / 16).clamp(0, 6);
    c.flavor = v % 16;
    if c.flavor > 8 {
        c.flavor = 0;
    }
    let mode = if c.flavor == 7 || c.flavor == 8 { 1 - scale_mode } else { scale_mode };
    let r = scale_offset(mode, c.degree);
    c.root_pc = ((r % 12) + 12) % 12;
    c.third = scale_offset(mode, c.degree + 2) - r;
    c.fifth = scale_offset(mode, c.degree + 4) - r;
    if c.flavor == 2 {
        c.third = 4;
        c.fifth = 7;
    } else if c.flavor == 3 {
        c.third = 2;
    } else if c.flavor == 4 {
        c.third = 5;
    }
    c
}

/// The entry after `i` in play order (the loop goes back to the intro's end); -1 after an Ending.
pub fn next_entry(t: &SongTimeline, i: i32) -> i32 {
    let n = t.entries.len() as i32;
    if i.saturating_add(1) < n {
        return i + 1;
    }
    if t.has_ending || n == 0 {
        return -1;
    }
    for (k, e) in t.entries.iter().enumerate() {
        if e.start_half >= t.intro_half {
            return k as i32;
        }
    }
    0
}

pub fn section_energy(t: &SongTimeline, e: &TimelineEntry) -> i32 {
    let s = e.section.clamp(0, (t.sections.len() as i32 - 1).max(0));
    let shift = t.sections.get(s as usize).map_or(0, |x| x.energy_shift);
    shift + e.energy_add
}

pub fn tension_for(t: &SongTimeline, i: i32, scale_mode: i32) -> f32 {
    if i < 0 || i >= t.entries.len() as i32 {
        return 0.0;
    }
    let Some(e) = t.entries.get(i as usize) else { return 0.0 };
    let c = chord_fn(e.slot, scale_mode);
    let ni = next_entry(t, i);
    let next = if ni >= 0 { t.entries.get(ni as usize) } else { None };
    let mut s: f32 = 0.0;
    if let Some(ne) = next {
        let n = chord_fn(ne.slot, scale_mode);
        let down5 = ((c.root_pc - n.root_pc) % 12 + 12) % 12 == 7;
        let majorish = c.third == 4;
        if down5 && majorish && (c.flavor == 2 || c.degree == 4) {
            s = s.max(0.8);
        }
        if c.flavor == 4 && (down5 || n.root_pc == c.root_pc) {
            s = s.max(0.5);
        }
    }
    if c.third == 3 && c.fifth == 6 {
        s = s.max(0.7);
    }
    if (c.flavor == 7 || c.flavor == 8) && (c.degree == 3 || c.degree == 5 || c.degree == 6) {
        s = s.max(0.4);
    }
    // The last chord of a pass going into a section with more Energy.
    if let Some(n) = next
        && n.arr_pos != e.arr_pos
        && section_energy(t, n) > section_energy(t, e)
    {
        s += 0.3;
    }
    s.clamp(0.0, 1.0)
}

/// Whether entry `from`'s tension (0.5 or more) resolves into entry `to` (played next: the following chord, the
/// loop's start or a jump's target).
pub fn resolves_into(t: &SongTimeline, from: i32, to: i32, scale_mode: i32) -> bool {
    let n = t.entries.len() as i32;
    if from < 0 || from >= n || to < 0 || to >= n || tension_for(t, from, scale_mode) < 0.5 {
        return false;
    }
    let (Some(e), Some(pe)) = (t.entries.get(to as usize), t.entries.get(from as usize)) else { return false };
    let c = chord_fn(e.slot, scale_mode);
    let p = chord_fn(pe.slot, scale_mode);
    let down5 = ((p.root_pc - c.root_pc) % 12 + 12) % 12 == 7;
    down5 || c.root_pc == 0 || e.arr_pos != pe.arr_pos
}

pub fn is_release(t: &SongTimeline, i: i32, scale_mode: i32) -> bool {
    i > 0 && resolves_into(t, i - 1, i, scale_mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::{Song, SongChord, SongSection, build_timeline};

    #[test]
    fn dominant_resolves_to_tonic() {
        let chords = vec![
            SongChord { slot: 17, bass: -1, length: 2 },
            SongChord { slot: 66, bass: -1, length: 2 },
            SongChord { slot: 1, bass: -1, length: 4 },
        ];
        let song = Song { sections: vec![SongSection { chords, ..SongSection::default() }], ..Song::default() };
        let t = build_timeline(&song);
        assert_eq!(tension_for(&t, 1, 0), 0.8);
        assert!(is_release(&t, 2, 0));
        assert!(!is_release(&t, 1, 0));
        assert_eq!(next_entry(&t, 2), 0);
        assert_eq!(next_entry(&t, i32::MAX), 0);
        assert_eq!(tension_for(&t, 9, 0), 0.0);
    }
}
