//! The Band view's data (`BandMap.cpp`, the compute half): the whole song bar by bar, the
//! section's state per musician, and the Energy each musician plays there.
//!
//! MS builds its map from an export render (note-ons per bar, fills, pickups, pushes, varied
//! bars). That half needs the whole band rendered and is not ported here: the Studio's view shows
//! what the timeline decides, which is MS's section strip (`section`, `energyAdd`, `entry`) and
//! its lane marks (sat out, kept), translated line by line, plus each musician's Energy there as
//! the musician computes it on a section change (`MusicianCore.cpp`: a section's fixed Energy, else
//! the musician's own plus the section's shift and the pass's nudge, 1..4).

use crate::song::{SongTimeline, clamp_int};

/// `BandMark` (MS `BandMap.h`): only the timeline's marks are computed here.
pub const MARK_SAT_OUT: i32 = 16;
pub const MARK_KEPT: i32 = 32;

/// One bar of the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarCell {
    /// The section id there (`'\0'` without one).
    pub section: char,
    /// The arrangement's Energy nudge there (chip + Build / Drop).
    pub energy_add: i32,
    /// The timeline entry (pass) the bar belongs to, -1 without one.
    pub entry: i32,
    /// Per musician: [`MARK_SAT_OUT`] / [`MARK_KEPT`] bits.
    pub marks: [i32; 6],
    /// Per musician: the Energy played there, 1..4; 0 where the musician sits out.
    pub energy: [i32; 6],
}

impl Default for BarCell {
    fn default() -> Self {
        Self { section: '\0', energy_add: 0, entry: -1, marks: [0; 6], energy: [0; 6] }
    }
}

impl BarCell {
    /// Whether musician `k` sits this bar out.
    pub fn sat_out(&self, k: usize) -> bool {
        self.marks.get(k).is_some_and(|m| m & MARK_SAT_OUT != 0)
    }
}

/// The map of `t`: `max(1, (totalHalf + 1) / 2)` bars. `plays[k]`: musician k is in the band (on
/// and playing; a musician who isn't sits the whole song out); `energies[k]`: their own Energy, 1..4.
pub fn band_map(t: &SongTimeline, plays: &[bool; 6], energies: &[i32; 6]) -> Vec<BarCell> {
    let tl = t;
    let bars = 1.max((tl.total_half + 1) / 2);
    let n = usize::try_from(bars).unwrap_or(1);
    let mut d = vec![BarCell::default(); n];
    let bar_of = |h: i32| usize::try_from(h / 2).ok().filter(|b| *b < n);

    for (i, e) in tl.entries.iter().enumerate() {
        let id = tl.section_id_of(i);
        let mut h = e.start_half;
        while h < e.start_half.saturating_add(e.len_half) && h / 2 < bars {
            if let Some(cell) = bar_of(h).and_then(|b| d.get_mut(b)) {
                if h % 2 == 0 || cell.entry < 0 {
                    cell.entry = i as i32;
                }
                cell.section = id;
                cell.energy_add = e.energy_add;
            }
            h += 1;
        }
    }

    for (k, (&on, &base)) in plays.iter().zip(energies).enumerate() {
        if !on {
            for cell in &mut d {
                cell.marks[k] |= MARK_SAT_OUT;
            }
            continue;
        }
        for e in &tl.entries {
            let Some(sec) = usize::try_from(e.section).ok().and_then(|s| tl.sections.get(s)) else { continue };
            let plays_here = sec.musicians.get(k).is_some_and(|m| m.plays);
            let kept = sec.frozen.get(k).and_then(Option::as_ref).is_some_and(|f| i32::from(f.meter) == 0.max(tl.meter_override));
            let mark = (if plays_here { 0 } else { MARK_SAT_OUT }) | if kept { MARK_KEPT } else { 0 };
            if mark != 0 {
                let mut h = e.start_half;
                while h < e.start_half.saturating_add(e.len_half) && h / 2 < bars {
                    if let Some(cell) = bar_of(h).and_then(|b| d.get_mut(b)) {
                        cell.marks[k] |= mark;
                    }
                    h += 1;
                }
            }
        }
        // The Energy where they play: the bar's pass decides (`MusicianCore.cpp` on a section change).
        for cell in &mut d {
            if cell.marks[k] & MARK_SAT_OUT != 0 {
                continue;
            }
            let Some(e) = usize::try_from(cell.entry).ok().and_then(|i| tl.entries.get(i)) else { continue };
            let Some(sec) = usize::try_from(e.section).ok().and_then(|s| tl.sections.get(s)) else { continue };
            let sm = sec.musicians.get(k).copied().unwrap_or_default();
            cell.energy[k] = if sm.energy > 0 {
                clamp_int(sm.energy, 1, 4)
            } else {
                clamp_int(base.saturating_add(sec.energy_shift).saturating_add(e.energy_add), 1, 4)
            };
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::song::{FrozenPart, SongMusician, SongSection, TimelineEntry};
    use std::sync::Arc;

    fn entry(start_half: i32, len_half: i32, section: i32, energy_add: i32) -> TimelineEntry {
        TimelineEntry { start_half, len_half, section, energy_add, ..TimelineEntry::default() }
    }

    /// Eight bars, checked by hand: A over bars 0-3 (the last pass starts on an odd half), B over
    /// bars 4-7 with a +1 shift, musician 1's Energy fixed at 4, musician 3 out and musician 4 kept,
    /// and a +1 chip on its second pass. Musician 6 is off.
    #[test]
    fn eight_bars_by_hand() {
        let a = SongSection { id: 'A', ..SongSection::default() };
        let mut b = SongSection { id: 'B', energy_shift: 1, ..SongSection::default() };
        b.musicians[0] = SongMusician { plays: true, energy: 4 };
        b.musicians[2] = SongMusician { plays: false, energy: 0 };
        b.frozen[3] = Some(Arc::new(FrozenPart { meter: 0, ..FrozenPart::default() }));
        let t = SongTimeline {
            entries: vec![entry(0, 2, 0, 0), entry(2, 2, 0, 0), entry(4, 3, 0, 0), entry(7, 1, 0, -1), entry(8, 4, 1, 0), entry(12, 4, 1, 1)],
            sections: vec![a, b],
            total_half: 16,
            ..SongTimeline::default()
        };
        let plays = [true, true, true, true, true, false];
        let base = [2, 3, 1, 4, 1, 1];
        let m = band_map(&t, &plays, &base);
        assert_eq!(m.len(), 8);
        let sections: String = m.iter().map(|c| c.section).collect();
        assert_eq!(sections, "AAAABBBB");
        // Bar 3: half 6 is pass 2 (an even half wins the bar), half 7 pass 3 writes the nudge.
        assert_eq!(m.iter().map(|c| c.entry).collect::<Vec<_>>(), [0, 1, 2, 2, 4, 4, 5, 5]);
        assert_eq!(m.iter().map(|c| c.energy_add).collect::<Vec<_>>(), [0, 0, 0, -1, 0, 0, 1, 1]);
        let s = MARK_SAT_OUT;
        let kept = MARK_KEPT;
        for (bar, c) in m.iter().enumerate() {
            let (energy, marks) = match bar {
                0..4 => ([2, 3, 1, 4, 1, 0], [0, 0, 0, 0, 0, s]),
                4..6 => ([4, 4, 0, 4, 2, 0], [0, 0, s, kept, 0, s]),
                _ => ([4, 4, 0, 4, 3, 0], [0, 0, s, kept, 0, s]),
            };
            assert_eq!(c.energy, energy, "bar {bar}");
            assert_eq!(c.marks, marks, "bar {bar}");
            assert_eq!(c.sat_out(2), bar >= 4);
        }
    }

    #[test]
    fn an_empty_timeline_is_one_bar() {
        let m = band_map(&SongTimeline::default(), &[true; 6], &[2; 6]);
        assert_eq!(m, vec![BarCell::default()]);
    }
}
