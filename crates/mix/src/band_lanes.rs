//! The session's band lanes (ticks) as the band crate's [`LaneSong`] (half bars), and a song laid
//! onto the lanes.
//!
//! The song grid is half bars of the band's meter for the session's meter at tick 0, so 6/8 has
//! halves of three eighths, and a meter the band doesn't play (5/4, 7/8, 12/8) has 4/4 halves.
//! Ticks become halves with starts rounded to the nearest half and ends rounded up; a chord left
//! with no half drops, as does a block of a section with no definition.

use soundcraft_band::lanes::{LaneBlock, LaneChord, LaneSong, MAX_LANE_HALF, lanes_end_half, song_to_lanes};
use soundcraft_band::meter;
use soundcraft_band::song::{Song, SongMusician, SongSection};
use soundcraft_band::song_feel::{FeelOverride, SongFeel};
use soundcraft_model::{ChordEvent, SectionBlock, SectionDef, SectionMusician, Session};
use soundcraft_time::{Samples, TICKS_PER_QUARTER};

/// Ticks per half bar of the meter the band plays for the meter at tick 0: 4/4, 3/4 and 6/8 as
/// they are, any other as 4/4 (`meterFromHost`). A step is a 16th note. Positions on the lanes
/// snap to this grid.
pub fn half_ticks(s: &Session) -> i64 {
    let m = s.tempo.meter_at_tick(0);
    let num = i32::try_from(m.numerator).unwrap_or(i32::MAX);
    let den = i32::try_from(m.denominator).unwrap_or(i32::MAX);
    let steps = i64::from(meter::info(meter::meter_from_host(num, den).0).steps_per_bar);
    (steps * (TICKS_PER_QUARTER / 4) / 2).max(1)
}

fn to_half(v: i64) -> i32 {
    v.clamp(0, i64::from(MAX_LANE_HALF)) as i32
}

/// The nearest half (ties go later).
fn round_half(tick: i64, half: i64) -> i32 {
    to_half(tick.saturating_mul(2).saturating_add(half).div_euclid(half.saturating_mul(2)))
}

fn ceil_half(tick: i64, half: i64) -> i32 {
    let q = tick.div_euclid(half);
    to_half(if tick.rem_euclid(half) != 0 { q.saturating_add(1) } else { q })
}

fn def_to_section(d: &SectionDef) -> SongSection {
    SongSection {
        id: d.id,
        name: d.name.clone(),
        energy_shift: i32::from(d.energy_shift),
        follow_idea: d.follow_idea,
        role: i32::from(d.role),
        shape: i32::from(d.shape),
        take: i32::from(d.take),
        musicians: d.musicians.map(|m| SongMusician { plays: m.plays, energy: i32::from(m.energy) }),
        feel_override: FeelOverride { density: d.feel[1], pocket: d.feel[2], groove: d.feel[3], touch: d.feel[4] },
        ..SongSection::default()
    }
}

fn section_to_def(s: &SongSection) -> SectionDef {
    let o = s.feel_override;
    SectionDef {
        id: s.id,
        name: s.name.clone(),
        energy_shift: s.energy_shift.clamp(-3, 3) as i8,
        shape: s.shape.clamp(0, 2) as u8,
        role: s.role.clamp(0, 2) as u8,
        take: s.take.clamp(0, 16) as u8,
        follow_idea: s.follow_idea,
        musicians: s.musicians.map(|m| SectionMusician { plays: m.plays, energy: m.energy.clamp(0, 4) as u8 }),
        feel: [-1, o.density, o.pocket, o.groove, o.touch],
    }
}

/// The lanes in half bars, or `None` when they hold nothing.
pub fn lane_song(s: &Session) -> Option<LaneSong> {
    let band = &s.band;
    let half = half_ticks(s);
    let defs: Vec<SongSection> = band.defs.iter().map(def_to_section).collect();
    let blocks: Vec<LaneBlock> = band
        .sections
        .iter()
        .filter_map(|b| {
            let def = band.defs.iter().position(|d| d.id == b.section)?;
            let start_half = round_half(b.start_tick, half);
            let len_half = ceil_half(b.start_tick.saturating_add(b.length_ticks), half) - start_half;
            (len_half > 0).then_some(LaneBlock { start_half, len_half, def, energy: i32::from(b.energy), take: i32::from(b.take) })
        })
        .collect();
    let mut chords: Vec<LaneChord> = band
        .chords
        .iter()
        .filter_map(|c| {
            let start_half = round_half(c.tick, half);
            let len_half = ceil_half(c.tick.saturating_add(c.length_ticks), half) - start_half;
            (len_half > 0).then_some(LaneChord { start_half, len_half, slot: i32::from(c.slot), bass: i32::from(c.bass) })
        })
        .collect();
    // Off-grid chords (Detect Chords "As played", an Alt-drag) can round onto the same half: each
    // ends where the next one starts, and one left with no half drops.
    chords.sort_by_key(|c| c.start_half);
    let starts: Vec<i32> = chords.iter().map(|c| c.start_half).collect();
    for (i, c) in chords.iter_mut().enumerate() {
        if let Some(next) = starts.get(i + 1) {
            c.len_half = c.len_half.min(next - c.start_half);
        }
    }
    chords.retain(|c| c.len_half > 0);
    if blocks.is_empty() && chords.is_empty() {
        return None;
    }
    let f = &band.song_feel;
    let feel = SongFeel { on: f.on, time: f.time, density: f.density, pocket: f.pocket, groove: f.groove, touch: f.touch, ..SongFeel::default() };
    Some(LaneSong { defs, blocks, chords, feel, feel_ignore: band.feel_ignore })
}

/// Where the band's song ends (after an Ending block at the latest), or `None` with nothing on the lanes.
pub fn band_song_end(s: &Session) -> Option<Samples> {
    let end = lanes_end_half(&lane_song(s)?);
    (end > 0).then(|| s.tempo.tick_to_samples(i64::from(end).saturating_mul(half_ticks(s)), s.sample_rate))
}

/// `song` laid onto the lanes from `at` (rounded to the nearest half): its section definitions, its
/// arrangement as blocks and its chords. Ids count up from the session's `next_id`.
pub fn lanes_from_song(s: &Session, song: &Song, at: Samples) -> (Vec<SectionDef>, Vec<SectionBlock>, Vec<ChordEvent>) {
    let half = half_ticks(s);
    let start_half = round_half(s.tempo.samples_to_ticks(at.max(0), s.sample_rate), half);
    let lanes = song_to_lanes(song, start_half);
    let defs: Vec<SectionDef> = lanes.defs.iter().map(section_to_def).collect();
    let mut id = s.band.next_id.max(1);
    let mut next = || {
        let this = id;
        id = id.saturating_add(1);
        this
    };
    let blocks = lanes
        .blocks
        .iter()
        .filter_map(|b| {
            let def = defs.get(b.def)?;
            Some(SectionBlock {
                id: next(),
                start_tick: i64::from(b.start_half).saturating_mul(half),
                length_ticks: i64::from(b.len_half).saturating_mul(half),
                section: def.id,
                energy: b.energy.clamp(-3, 3) as i8,
                take: b.take.clamp(0, 16) as u8,
            })
        })
        .collect();
    let chords = lanes
        .chords
        .iter()
        .map(|c| ChordEvent {
            id: next(),
            tick: i64::from(c.start_half).saturating_mul(half),
            length_ticks: i64::from(c.len_half).saturating_mul(half),
            slot: c.slot.clamp(0, 111) as u8,
            bass: c.bass.clamp(-1, 11) as i8,
        })
        .collect();
    (defs, blocks, chords)
}
