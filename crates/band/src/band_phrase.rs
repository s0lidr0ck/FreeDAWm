//! Phrases: how a musician plays a part over a section the way a player does (part of "As a band"; not a port of
//! MIDI Studio).
//!
//! Without this, each musician plays his one- or two-bar pattern over and over (at low Energy literally the same bar;
//! at higher Energy every hit is a fresh coin toss, so no two bars, and no two choruses, are alike). A player instead
//! has a part and shapes it in phrases:
//!
//! - **The part.** The hits a pattern leaves to chance are drawn once for the section, by where the bar sits in its
//!   phrase ([`model_bar`]): a phrase's third bar restates its first, the second half of an eight-bar phrase restates
//!   the first half, and every pass of a section draws the same, so a chorus repeat is the same chorus.
//! - **The phrase.** Four bars are statement, answer, restatement (with a small change) and turnaround; eight bars
//!   are that twice, the second time with a variation in bar seven and a bigger turnaround ([`bar_kind`]). The
//!   section's last bar turns into the next one.
//! - **The change.** On a bar that varies, a player picks one device of his instrument ([`Device`]): the drummer an
//!   added or a dropped kick, ghost notes, an opening hi-hat or a snare pick-up; the bass an approach note, an octave
//!   or a walk into the next chord; keys, guitar and organ another inversion, a held chord instead of the comping, a
//!   chord struck again, a thinner bar or a pick-up note; strings and brass much the same. A part with a chord a bar
//!   strikes again or revoices rather than hold; a busy one holds rather than strike again. The downbeat, the kick on
//!   it and the chord's root are never touched.
//! - **Space.** In each bar one musician has the spotlight and may play something busy ([`spotlight`]; the players
//!   take turns, the bar before a new section is the drummer's); while he is busy the others add nothing (they hold,
//!   thin out, revoice or play as written).
//! - **Again, a little different.** The second and later passes of a section change one bar per player (a bar he
//!   played as written the first time, each player his own), so the repeat is recognisably the same part.
//!
//! Everything here is a pure function of the Take, the section, its pass, the bar and how busy the part was in the
//! bar before (no allocation: it runs on the audio thread), so a Take always plays the same.

/// The four kinds of player, for the devices each has.
pub const GROUP_DRUMS: i32 = 0;
pub const GROUP_BASS: i32 = 1;
/// Keys, guitar and organ.
pub const GROUP_CHORDS: i32 = 2;
/// Strings, brass, winds (and a melody player).
pub const GROUP_SECTION: i32 = 3;

fn mix(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    x
}

fn hash(parts: &[i32]) -> u32 {
    let mut h = 0x2545_F491u32;
    for &p in parts {
        h = mix(h ^ mix(p as u32 ^ 0x9E37_79B9));
    }
    h
}

/// A value in [0, 1) from the parts.
fn u01(parts: &[i32]) -> f32 {
    (hash(parts) >> 8) as f32 / 16_777_216.0
}

/// Where a bar sits in its phrase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarKind {
    /// The phrase's opening (and the opening restated at the middle of an eight-bar phrase): played as written.
    Statement,
    /// The bar that answers it.
    Answer,
    /// The statement again, with a small change.
    Restate,
    /// An eight-bar phrase's seventh bar: the variation.
    Vary,
    /// A phrase's last bar inside the section: a turnaround into the next phrase.
    Turn,
    /// The section's last bar: the turn into the next section (the drummer's).
    SectionTurn,
}

/// The phrase length for a section of `bars` bars: eight when it has eight or more, four when four or more.
pub fn phrase_len(bars: i32) -> i32 {
    if bars >= 8 {
        8
    } else if bars >= 4 {
        4
    } else {
        bars.max(1)
    }
}

/// What bar `bar` (0-based) of a section of `bars` bars is in its phrase.
pub fn bar_kind(bar: i32, bars: i32) -> BarKind {
    let bars = bars.max(1);
    if bars >= 2 && bar >= bars - 1 {
        return BarKind::SectionTurn;
    }
    let len = phrase_len(bars);
    let p = bar.rem_euclid(len);
    match (len, p) {
        (8, 1) | (8, 5) | (4, 1) => BarKind::Answer,
        (8, 2) | (4, 2) => BarKind::Restate,
        (8, 6) => BarKind::Vary,
        (8, 3) | (8, 7) | (4, 3) => BarKind::Turn,
        (2 | 3, 1) => BarKind::Answer,
        _ => BarKind::Statement,
    }
}

/// The bar whose chance hits bar `bar` plays again: the phrase's statement for a restatement, the first half of an
/// eight-bar phrase for the second half, the first phrase for every later phrase of a long section; the bar itself
/// for a variation and a turnaround.
pub fn model_bar(bar: i32, bars: i32) -> i32 {
    const EIGHT: [i32; 8] = [0, 1, 0, 3, 0, 1, 6, 7];
    const FOUR: [i32; 4] = [0, 1, 0, 3];
    let bars = bars.max(1);
    if bar >= bars - 1 || bar < 0 {
        return bar; // the section's turn plays its own
    }
    let len = phrase_len(bars);
    let p = bar.rem_euclid(len) as usize;
    match len {
        8 => EIGHT.get(p).copied().unwrap_or(p as i32),
        4 => FOUR.get(p).copied().unwrap_or(p as i32),
        _ => bar,
    }
}

/// A musician's place in the song for the phrase layer: who, which part, which section and pass, which bar.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PhraseAt {
    pub take: i32,
    /// The musician's slot (0..6).
    pub musician: i32,
    /// [`GROUP_DRUMS`] .. [`GROUP_SECTION`].
    pub group: i32,
    /// The section (its index in the song).
    pub section: i32,
    /// How many times the section was played before this pass (0 = its first time).
    pub occurrence: i32,
    pub bar: i32,
    pub bars: i32,
    /// How many steps of the bar before had notes (-1 not known): how busy the part is.
    pub density: i32,
}

impl PhraseAt {
    /// A sparse part: a chord or two a bar.
    pub fn sparse(&self) -> bool {
        (0..=2).contains(&self.density)
    }
    /// A busy part: five or more strikes a bar.
    pub fn dense(&self) -> bool {
        self.density >= 5
    }
}

/// How likely the bar's spotlight is to play something busy, by bar kind.
fn spot_chance(kind: BarKind) -> f32 {
    match kind {
        BarKind::Statement => 0.0,
        BarKind::Answer => 0.35,
        BarKind::Restate => 0.6,
        BarKind::Vary => 0.8,
        BarKind::Turn => 0.75,
        BarKind::SectionTurn => 0.55,
    }
}

/// The spotlight's value for the drummer (a section's turn is his, whichever slot he is in).
pub const SPOT_DRUMMER: i32 = -1;

/// Who has the bar (may play something busy): the drummer at a section's turn ([`SPOT_DRUMMER`]); else the
/// musicians (slots 0..6) take turns bar by bar, in an order drawn per section (the same on every pass), so everyone
/// gets his moments and two players of one kind (two guitars, strings and brass) do not vary together.
pub fn spotlight(take: i32, section: i32, bar: i32, bars: i32) -> i32 {
    if bar_kind(bar, bars) == BarKind::SectionTurn {
        return SPOT_DRUMMER;
    }
    let h = hash(&[take, section, 0x51]);
    // A step through the six slots coprime with six (1 or 5), from a drawn start.
    let stride = if h & 1 == 0 { 1 } else { 5 };
    let start = ((h >> 4) % 6) as i32;
    (start + stride * bar).rem_euclid(6)
}

/// Whether `p` has the bar's spotlight.
pub fn has_spotlight(p: &PhraseAt) -> bool {
    let s = spotlight(p.take, p.section, p.bar, p.bars);
    if s == SPOT_DRUMMER { p.group == GROUP_DRUMS } else { s == p.musician }
}

/// Whether the bar's spotlight plays something busy in it (the band's decision, so the others know to leave room).
pub fn spotlight_busy(take: i32, section: i32, bar: i32, bars: i32) -> bool {
    u01(&[take, section, bar, 0x52]) < spot_chance(bar_kind(bar, bars))
}

/// The bar of a later pass of a section that this musician plays differently from the first pass: one he played as
/// written the first time (never the first bar; a change he made then stays); -1 on the first pass, in a one-bar
/// section, or when he changed every bar already.
pub fn changed_bar(p: &PhraseAt) -> i32 {
    if p.occurrence <= 0 || p.bars < 2 {
        return -1;
    }
    let span = p.bars - 1;
    let from = (hash(&[p.take, p.musician, p.section, p.occurrence, 0x53]) % span.max(1) as u32) as i32;
    for i in 0..span {
        let bar = 1 + (from + i).rem_euclid(span);
        if choose(&PhraseAt { occurrence: 0, bar, ..*p }).0 == Device::None {
            return bar;
        }
    }
    -1
}

/// What a musician changes in a bar. The downbeat, its kick and the chord's root are never touched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    None,
    /// Drums: an added kick on the "and" of 4 (into the next bar).
    KickAnd4,
    /// Drums: an added kick on the "and" of 3.
    KickAnd3,
    /// Drums: two ghost notes on the snare.
    Ghosts,
    /// Drums: the hi-hat opens on the "and" of 4.
    HatOpen,
    /// Drums: a two-note snare pick-up into the next bar.
    SnarePickup,
    /// Drums: the kicks after beat 3 are left out.
    DropKick,
    /// Bass: an approach note on the "and" of 4 into the next chord's root.
    Approach,
    /// Bass: the root an octave up on beat 3.
    Octave,
    /// Bass: a walk into the next chord (the fifth on beat 4, then the approach).
    Walk,
    /// Anyone but the drummer: only the notes on the beats (leaving room).
    Thin,
    /// Keys, organ, strings, brass: another inversion (the top voice an octave down, or the bottom one up).
    Inversion,
    /// Keys, guitar, organ, strings, brass: the bar's first chord held through, instead of comped.
    Hold,
    /// Keys, guitar, organ, strings, brass: a pick-up note on the "and" of 4 into the next chord.
    Lead,
    /// Keys, guitar, organ, strings, brass: the bar's first chord struck again, softer, on beat 3 (or the "and" of
    /// 2): a comp where the part holds.
    Restrike,
}

impl Device {
    /// Fills the bar with more than a note (only the spotlight plays one).
    pub fn busy(self) -> bool {
        matches!(self, Device::KickAnd3 | Device::Ghosts | Device::SnarePickup | Device::Walk | Device::Lead)
    }
}

fn pick<const N: usize>(menu: [Device; N], r: f32) -> Device {
    let i = ((r * N as f32) as usize).min(N.saturating_sub(1));
    menu.get(i).copied().unwrap_or(Device::None)
}

/// The device a musician plays in a bar, and its variant (0 or 1: which beat, which direction).
pub fn device(p: &PhraseAt) -> (Device, i32) {
    let (d, variant) = choose(p);
    // Fit the device to the part: a sparse part (a chord a bar or two) has no comping to hold or thin out (it moves
    // its voicing or strikes again instead); a busy one has no room for another strike (it holds).
    let d = match d {
        Device::Hold | Device::Thin if p.sparse() && p.group != GROUP_BASS && p.group != GROUP_DRUMS => {
            if variant == 0 {
                Device::Inversion
            } else {
                Device::Restrike
            }
        }
        Device::Restrike if p.dense() => Device::Hold,
        d => d,
    };
    (d, variant)
}

fn choose(p: &PhraseAt) -> (Device, i32) {
    let kind = bar_kind(p.bar, p.bars);
    let changed = changed_bar(p) == p.bar;
    // The first pass's choices again on every pass, except on this player's changed bar.
    let pass_key = if changed { p.occurrence + 1 } else { 0 };
    let r = u01(&[p.take, p.musician, p.section, p.bar, pass_key, 0x61]);
    let which = u01(&[p.take, p.musician, p.section, p.bar, pass_key, 0x62]);
    let variant = (u01(&[p.take, p.musician, p.section, p.bar, pass_key, 0x63]) < 0.5) as i32;
    let spot_busy = spotlight_busy(p.take, p.section, p.bar, p.bars);
    let me_busy = spot_busy && has_spotlight(p);
    let turn = matches!(kind, BarKind::Turn | BarKind::SectionTurn);
    let drums = p.group == GROUP_DRUMS;
    if me_busy {
        let d = match p.group {
            GROUP_DRUMS if turn => pick([Device::SnarePickup, Device::KickAnd4, Device::SnarePickup, Device::HatOpen], which),
            GROUP_DRUMS => pick([Device::Ghosts, Device::KickAnd3, Device::HatOpen, Device::Ghosts, Device::KickAnd4], which),
            GROUP_BASS if turn => pick([Device::Walk, Device::Approach], which),
            GROUP_BASS => pick([Device::Octave, Device::Approach, Device::Octave], which),
            GROUP_CHORDS if turn => pick([Device::Lead, Device::Lead, Device::Restrike], which),
            GROUP_CHORDS => pick([Device::Lead, Device::Restrike, Device::Inversion], which),
            _ if turn => pick([Device::Lead, Device::Restrike], which),
            _ => pick([Device::Lead, Device::Restrike, Device::Inversion], which),
        };
        return (d, variant);
    }
    // Not the spotlight: something small now and then; leaving room when the spotlight is busy.
    let chance: f32 = match kind {
        BarKind::Statement => 0.0,
        BarKind::Answer => 0.35,
        BarKind::Restate => 0.6,
        BarKind::Vary => 0.7,
        BarKind::Turn => 0.6,
        BarKind::SectionTurn if drums => 0.0, // the drummer's own fill (or his groove) is his
        BarKind::SectionTurn => 0.25,
    };
    // A sparse part has room: it varies a little more often.
    let chance = if p.sparse() && !drums { (chance * 1.4).min(0.85) } else { chance };
    if changed {
        // A later pass, this player's changed bar: something mild, more often than not.
        if r >= 0.65 {
            return (Device::None, variant);
        }
        let d = match p.group {
            GROUP_DRUMS => pick([Device::HatOpen, Device::KickAnd4], which),
            GROUP_BASS => pick([Device::Approach, Device::Octave], which),
            _ if p.dense() => Device::Inversion,
            _ => pick([Device::Inversion, Device::Restrike], which),
        };
        return (d, variant);
    }
    if r >= chance {
        return (Device::None, variant);
    }
    let d = if spot_busy {
        // Someone else is busy: nothing added; hold or thin out, or only move the voicing.
        match p.group {
            GROUP_DRUMS => pick([Device::DropKick, Device::HatOpen], which),
            GROUP_BASS => Device::Thin,
            GROUP_CHORDS => pick([Device::Hold, Device::Thin, Device::Inversion], which),
            _ => pick([Device::Hold, Device::Inversion], which),
        }
    } else {
        match p.group {
            GROUP_DRUMS => pick([Device::HatOpen, Device::KickAnd4, Device::DropKick], which),
            GROUP_BASS if turn => Device::Approach,
            GROUP_BASS => pick([Device::Octave, Device::Approach], which),
            GROUP_CHORDS => pick([Device::Inversion, Device::Restrike, Device::Hold, Device::Thin], which),
            _ => pick([Device::Restrike, Device::Hold, Device::Restrike, Device::Inversion], which),
        }
    };
    (d, variant)
}

/// What the phrase layer knows of the step a musician is making (set around each generated step, like the band
/// step; `on` only with "As a band").
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PhraseStep {
    pub on: bool,
    pub at: PhraseAt,
    /// Added to a step index to give the key its chance hits are drawn with (the model bar's step).
    pub hit_step_delta: i32,
    /// The pass key the chance hits are drawn with (the same for every pass of a section).
    pub hit_pass: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrases_have_a_statement_an_answer_a_restatement_and_a_turn() {
        use BarKind::*;
        let four: Vec<BarKind> = (0..4).map(|b| bar_kind(b, 4)).collect();
        assert_eq!(four, vec![Statement, Answer, Restate, SectionTurn]);
        let eight: Vec<BarKind> = (0..8).map(|b| bar_kind(b, 8)).collect();
        assert_eq!(eight, vec![Statement, Answer, Restate, Turn, Statement, Answer, Vary, SectionTurn]);
        let twelve: Vec<BarKind> = (0..12).map(|b| bar_kind(b, 12)).collect();
        assert_eq!(&twelve[8..], &[Statement, Answer, Restate, SectionTurn]);
        assert_eq!((0..8).map(|b| model_bar(b, 8)).collect::<Vec<_>>(), vec![0, 1, 0, 3, 0, 1, 6, 7]);
        assert_eq!((0..4).map(|b| model_bar(b, 4)).collect::<Vec<_>>(), vec![0, 1, 0, 3]);
        assert_eq!(model_bar(9, 12), 1);
        assert_eq!(model_bar(11, 12), 11);
    }

    fn at(take: i32, musician: i32, group: i32, section: i32, occurrence: i32, bar: i32, bars: i32) -> PhraseAt {
        PhraseAt { take, musician, group, section, occurrence, bar, bars, density: -1 }
    }

    #[test]
    fn the_statement_is_never_changed_and_only_the_spotlight_is_busy() {
        for take in 1..40 {
            for section in 0..3 {
                for bars in [4, 8, 12] {
                    for bar in 0..bars {
                        let mut busy = 0;
                        for m in 0..6 {
                            let p = at(take, m, [2, 0, 1, 3, 3, 2][m as usize], section, take % 3, bar, bars);
                            let (d, _) = device(&p);
                            if bar == 0 {
                                assert_eq!(d, Device::None);
                            }
                            if d.busy() {
                                busy += 1;
                                assert!(has_spotlight(&p));
                            }
                        }
                        assert!(busy <= 1, "take {take} section {section} bar {bar}/{bars}: {busy} players busy");
                    }
                }
            }
        }
    }

    #[test]
    fn a_repeat_is_the_same_part_with_one_bar_changed() {
        let mut changed_total = 0;
        for take in 1..30 {
            for m in 0..6 {
                let g = m % 4;
                let first: Vec<(Device, i32)> = (0..8).map(|b| device(&at(take, m, g, 1, 0, b, 8))).collect();
                let again: Vec<(Device, i32)> = (0..8).map(|b| device(&at(take, m, g, 1, 1, b, 8))).collect();
                let differ = first.iter().zip(&again).filter(|(a, b)| a != b).count();
                assert!(differ <= 1, "take {take} musician {m}: {differ} bars differ between passes");
                changed_total += differ;
                assert_eq!(first, (0..8).map(|b| device(&at(take, m, g, 1, 0, b, 8))).collect::<Vec<_>>());
            }
        }
        assert!(changed_total > 60, "a later pass should usually change a bar ({changed_total})");
    }

    #[test]
    fn the_drummer_has_the_turn_into_a_new_section() {
        for take in 1..20 {
            assert_eq!(spotlight(take, 0, 7, 8), SPOT_DRUMMER);
            assert_eq!(spotlight(take, 2, 3, 4), SPOT_DRUMMER);
        }
        // The spotlight moves around the band: six bars running are six players.
        for take in 1..50 {
            let mut seen: Vec<i32> = (1..7).map(|bar| spotlight(take, 0, bar, 12)).collect();
            seen.sort();
            assert_eq!(seen, vec![0, 1, 2, 3, 4, 5]);
        }
    }
}
