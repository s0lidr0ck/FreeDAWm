//! The Vocal Studio's music theory (spec 2026-11-03-vocal-studio-design.md §2–3), on MIDI note
//! numbers and pitch classes only (no audio): which intervals clash, the nearest tone a clashing
//! note can move to (Fit to lead), and the harmony note a lead note gets (Make harmony: diatonic
//! in the key, following the chord).

use crate::PitchClassSet;
use serde::{Deserialize, Serialize};

/// Why a note clashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClashKind {
    /// A minor second (or minor ninth…) against the other note.
    MinorSecond,
    /// A major seventh (or its octaves).
    MajorSeventh,
    /// A tritone.
    Tritone,
    /// Outside the key (and not a tone of the chord).
    OutOfKey,
}

impl ClashKind {
    /// "minor 2nd", "major 7th", "tritone", "out of key".
    pub fn name(self) -> &'static str {
        match self {
            ClashKind::MinorSecond => "minor 2nd",
            ClashKind::MajorSeventh => "major 7th",
            ClashKind::Tritone => "tritone",
            ClashKind::OutOfKey => "out of key",
        }
    }
}

/// The clash between two sounding notes `a` and `b` (MIDI), by their distance in semitones
/// folded into an octave: 1 (and 13, 25…) a minor 2nd, 11 a major 7th, 6 a tritone.
pub fn interval_clash(a: i32, b: i32) -> Option<ClashKind> {
    match a.abs_diff(b) % 12 {
        1 => Some(ClashKind::MinorSecond),
        11 => Some(ClashKind::MajorSeventh),
        6 => Some(ClashKind::Tritone),
        _ => None,
    }
}

/// The clash of note `midi` against a chord's tones (`tones[pc]`): none when the note is a chord
/// tone, else the first tone it makes a minor 2nd, major 7th or tritone with (the clash and that
/// tone's pitch class).
pub fn chord_clash(midi: i32, tones: &[bool; 12]) -> Option<(ClashKind, u8)> {
    let pc = midi.rem_euclid(12);
    if tones.get(pc as usize).copied().unwrap_or(false) {
        return None;
    }
    // Seconds first: a minor 2nd against a chord tone is the harshest.
    for kind in [ClashKind::MinorSecond, ClashKind::MajorSeventh, ClashKind::Tritone] {
        for (t, on) in tones.iter().enumerate() {
            if !on {
                continue;
            }
            let up = (pc - t as i32).rem_euclid(12);
            let hit = match kind {
                // The note a semitone above the tone, or below it.
                ClashKind::MinorSecond => up == 1 || up == 11,
                ClashKind::MajorSeventh => false,
                ClashKind::Tritone => up == 6,
                ClashKind::OutOfKey => false,
            };
            if hit {
                return Some((kind, t as u8));
            }
        }
    }
    None
}

/// Whether `midi` is outside `scale` and not a tone of `chord`.
pub fn out_of_key(midi: i32, scale: PitchClassSet, chord: Option<&[bool; 12]>) -> bool {
    let pc = midi.rem_euclid(12);
    let in_chord = chord.is_some_and(|c| c.get(pc as usize).copied().unwrap_or(false));
    !scale.contains(pc as u8) && !in_chord
}

/// Where Fit to lead moves note `midi`: the nearest semitone (it may stay) within `reach`
/// semitones that clashes with none of `others` (the lead's notes sounding with it) nor with the
/// chord, and is in the key or the chord when a key is given. Chord tones win ties; then the
/// smaller move, then down. `None` when nothing within reach fits.
pub fn fit_note(midi: i32, others: &[i32], chord: Option<&[bool; 12]>, scale: Option<PitchClassSet>, reach: i32) -> Option<i32> {
    let ok = |m: i32| {
        others.iter().all(|o| interval_clash(m, *o).is_none())
            && chord.is_none_or(|c| chord_clash(m, c).is_none())
            && scale.is_none_or(|s| !out_of_key(m, s, chord))
    };
    let is_chord = |m: i32| chord.is_some_and(|c| c.get(m.rem_euclid(12) as usize).copied().unwrap_or(false));
    (-reach.abs()..=reach.abs()).map(|d| midi + d).filter(|m| ok(*m)).min_by_key(|m| {
        let d = (m - midi).abs();
        // A move of one semitone onto a chord tone beats staying put on a non-chord tone that
        // only just passes; staying on a chord tone beats everything.
        (d * 2 - i32::from(is_chord(*m)), i32::from(*m > midi))
    })
}

/// A harmony interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interval {
    Third,
    Fifth,
    Sixth,
    Octave,
}

impl Interval {
    /// "3rd", "5th", "6th", "octave" (also "3", "third"…).
    pub fn parse(s: &str) -> Option<Interval> {
        match s.trim().to_lowercase().as_str() {
            "3rd" | "3" | "third" => Some(Interval::Third),
            "5th" | "5" | "fifth" => Some(Interval::Fifth),
            "6th" | "6" | "sixth" => Some(Interval::Sixth),
            "octave" | "8ve" | "8" | "oct" => Some(Interval::Octave),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Interval::Third => "3rd",
            Interval::Fifth => "5th",
            Interval::Sixth => "6th",
            Interval::Octave => "octave",
        }
    }

    /// Scale steps (a 3rd is two steps along the scale).
    fn steps(self) -> i32 {
        match self {
            Interval::Third => 2,
            Interval::Fifth => 4,
            Interval::Sixth => 5,
            Interval::Octave => 7,
        }
    }

    /// The parallel (chromatic) distance in semitones, up: a major 3rd, a perfect 5th, a
    /// major 6th, an octave.
    fn semitones(self) -> i32 {
        match self {
            Interval::Third => 4,
            Interval::Fifth => 7,
            Interval::Sixth => 9,
            Interval::Octave => 12,
        }
    }

    /// The smallest and largest semitone distance the interval spans (a 3rd: 3–4).
    fn span(self) -> (i32, i32) {
        match self {
            Interval::Third => (3, 4),
            Interval::Fifth => (6, 8),
            Interval::Sixth => (8, 9),
            Interval::Octave => (12, 12),
        }
    }
}

/// How a harmony note is chosen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HarmonyRule {
    pub interval: Interval,
    pub up: bool,
    /// Count the interval along the key's scale (a diatonic 3rd), not in fixed semitones.
    pub stay_in_key: bool,
    /// When the lead sings a chord tone, the harmony takes the chord tone nearest the interval.
    pub follow_chords: bool,
    /// The lowest and highest MIDI note the harmony may sing (folded in by octaves).
    pub range: (i32, i32),
}

/// The scale's pitch classes in order from C (empty for an empty set).
fn scale_pcs(scale: PitchClassSet) -> Vec<i32> {
    (0..12).filter(|pc| scale.contains(*pc as u8)).collect()
}

/// `midi` moved `steps` along `scale` (a note outside it first snaps down to the scale tone below).
fn scale_step(midi: i32, steps: i32, scale: PitchClassSet) -> Option<i32> {
    let pcs = scale_pcs(scale);
    let n = i32::try_from(pcs.len()).ok().filter(|n| *n > 0)?;
    // The scale degree at or below `midi`, as an index into an endless scale.
    let oct = midi.div_euclid(12);
    let pc = midi.rem_euclid(12);
    let below = pcs.iter().rposition(|p| *p <= pc);
    let (idx, base_oct) = match below {
        Some(i) => (i32::try_from(i).ok()?, oct),
        None => (n - 1, oct - 1),
    };
    let target = idx + steps;
    let t_oct = base_oct + target.div_euclid(n);
    let t_pc = *pcs.get(usize::try_from(target.rem_euclid(n)).ok()?)?;
    Some(t_oct * 12 + t_pc)
}

/// The harmony note for lead note `midi` under `rule`, with the key's `scale` (if any) and the
/// chord's tones (if any) at that note.
///
/// - The interval is counted along the scale when `stay_in_key` and a scale is given (a diatonic
///   3rd: major or minor as the key has it), else in fixed semitones (major 3rd, perfect 5th,
///   major 6th).
/// - With `follow_chords` and the lead on a chord tone, the harmony is the chord tone (other than
///   the lead's pitch class) nearest that diatonic note within the interval's span in the asked
///   direction ("3rd up" from the root is the chord's 3rd, from its 3rd the 5th, from its 5th the
///   root above), else the diatonic note.
/// - Folded by octaves into `range` when it falls outside (a range under an octave keeps the
///   nearest note).
pub fn harmony_note(midi: i32, rule: &HarmonyRule, scale: Option<PitchClassSet>, chord: Option<&[bool; 12]>) -> i32 {
    let dir = if rule.up { 1 } else { -1 };
    let base = match (rule.interval, scale.filter(|s| rule.stay_in_key && !s.is_empty_set())) {
        (Interval::Octave, _) => midi + dir * 12,
        (iv, Some(sc)) => scale_step(midi, dir * iv.steps(), sc).unwrap_or(midi + dir * iv.semitones()),
        (iv, None) => midi + dir * iv.semitones(),
    };
    let mut h = base;
    if rule.follow_chords
        && rule.interval != Interval::Octave
        && let Some(c) = chord
        && c.get(midi.rem_euclid(12) as usize).copied().unwrap_or(false)
    {
        let (lo, hi) = rule.interval.span();
        // Chord tones in the interval's reach (a 3rd: 3–5 semitones, so a 5th's root above counts).
        let cands: Vec<i32> = (lo..=hi + 1)
            .map(|d| midi + dir * d)
            .filter(|m| m.rem_euclid(12) != midi.rem_euclid(12) && c.get(m.rem_euclid(12) as usize).copied().unwrap_or(false))
            .collect();
        if let Some(best) = cands.into_iter().min_by_key(|m| ((m - base).abs(), (m - midi).abs())) {
            h = best;
        }
    }
    fold_into(h, rule.range)
}

/// `m` moved by octaves into `[lo, hi]` when it can be (the nearest such octave of it); else
/// the octave of it nearest the range.
pub fn fold_into(m: i32, (lo, hi): (i32, i32)) -> i32 {
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    if (lo..=hi).contains(&m) {
        return m;
    }
    let outside = |v: i32| if v < lo { lo - v } else { (v - hi).max(0) };
    (-11..=11).map(|k| m + 12 * k).min_by_key(|v| (outside(*v), (v - m).abs())).unwrap_or(m)
}

impl PitchClassSet {
    /// Whether no pitch class is in the set.
    pub fn is_empty_set(&self) -> bool {
        self.0 & 0x0fff == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const E_MAJOR: [u8; 7] = [4, 6, 8, 9, 11, 1, 3];

    fn e_major() -> PitchClassSet {
        PitchClassSet::from_pcs(&E_MAJOR)
    }

    fn triad(root: i32, minor: bool) -> [bool; 12] {
        let mut t = [false; 12];
        for d in [0, if minor { 3 } else { 4 }, 7] {
            t[(root + d).rem_euclid(12) as usize] = true;
        }
        t
    }

    #[test]
    fn clashing_intervals() {
        assert_eq!(interval_clash(64, 65), Some(ClashKind::MinorSecond));
        assert_eq!(interval_clash(65, 64), Some(ClashKind::MinorSecond));
        assert_eq!(interval_clash(64, 77), Some(ClashKind::MinorSecond), "a minor 9th");
        assert_eq!(interval_clash(60, 71), Some(ClashKind::MajorSeventh));
        assert_eq!(interval_clash(60, 66), Some(ClashKind::Tritone));
        for ok in [0, 3, 4, 5, 7, 8, 9, 12, 2, 10] {
            assert_eq!(interval_clash(60, 60 + ok), None, "{ok}");
        }
        let e = triad(4, false);
        assert_eq!(chord_clash(64, &e), None, "a chord tone");
        assert_eq!(chord_clash(65, &e), Some((ClashKind::MinorSecond, 4)), "F against E");
        assert_eq!(chord_clash(69, &e), Some((ClashKind::MinorSecond, 8)), "A against G#");
        assert_eq!(chord_clash(66, &e), None, "F# (a 9th) is fine");
        assert!(out_of_key(67, e_major(), None));
        assert!(!out_of_key(68, e_major(), None));
        assert!(!out_of_key(67, e_major(), Some(&triad(4, true))), "G is the chord's in E minor");
    }

    #[test]
    fn fit_moves_to_the_nearest_tone_that_does_not_clash() {
        let a = triad(9, false); // A major over E major's key
        // BV on G (67) against a lead G# (68), over A: G clashes (m2 with G#, out of key):
        // F# (66) is in key but a 7th under G#? 68-66 = 2: fine, and a 6th of A: not a chord
        // tone; A (69) is a chord tone but a m2 with G#. E (64) is a chord tone 4 under G#.
        let got = fit_note(67, &[68], Some(&a), Some(e_major()), 4);
        assert_eq!(got, Some(66), "F#: in key, no clash, one semitone");
        // Already fine: stays.
        assert_eq!(fit_note(64, &[68], Some(&a), Some(e_major()), 4), Some(64));
        // Without key or chord, only the lead counts.
        assert_eq!(fit_note(67, &[68], None, None, 3), Some(66), "F#: a 2nd under G# is no clash");
        assert_eq!(fit_note(61, &[62, 60, 61 + 6], None, None, 0), None, "nothing within reach");
    }

    #[test]
    fn diatonic_thirds_follow_the_key_and_chords() {
        let up3 = HarmonyRule { interval: Interval::Third, up: true, stay_in_key: true, follow_chords: false, range: (0, 127) };
        // E major: E→G#, F#→A, G#→B, A→C#, B→D#, C#→E, D#→F#.
        let got: Vec<i32> = [64, 66, 68, 69, 71, 73, 75].iter().map(|m| harmony_note(*m, &up3, Some(e_major()), None)).collect();
        assert_eq!(got, [68, 69, 71, 73, 75, 76, 78]);
        // Down a third.
        let down3 = HarmonyRule { up: false, ..up3 };
        assert_eq!(harmony_note(68, &down3, Some(e_major()), None), 64);
        assert_eq!(harmony_note(64, &down3, Some(e_major()), None), 61);
        // Without a key: parallel major thirds.
        assert_eq!(harmony_note(64, &up3, None, None), 68);
        assert_eq!(harmony_note(66, &HarmonyRule { stay_in_key: false, ..up3 }, Some(e_major()), None), 70);
        // Follow chords: over B major (B D# F#) the lead's F# (the 5th) takes the root above,
        // and over A the lead's A (root) takes C#, its E (5th) the A above.
        let follow = HarmonyRule { follow_chords: true, ..up3 };
        let b = triad(11, false);
        assert_eq!(harmony_note(66, &follow, Some(e_major()), Some(&b)), 71, "F# over B: B above");
        let a = triad(9, false);
        assert_eq!(harmony_note(69, &follow, Some(e_major()), Some(&a)), 73);
        assert_eq!(harmony_note(64, &follow, Some(e_major()), Some(&a)), 69);
        // A passing (non-chord) lead note keeps the diatonic 3rd.
        assert_eq!(harmony_note(66, &follow, Some(e_major()), Some(&a)), 69);
        // Fifths and sixths.
        let up5 = HarmonyRule { interval: Interval::Fifth, ..up3 };
        assert_eq!(harmony_note(64, &up5, Some(e_major()), None), 71);
        assert_eq!(harmony_note(75, &up5, Some(e_major()), None), 81, "D# up a diatonic 5th: A (diminished)");
        let down6 = HarmonyRule { interval: Interval::Sixth, up: false, ..up3 };
        assert_eq!(harmony_note(64, &down6, Some(e_major()), None), 56);
        let oct = HarmonyRule { interval: Interval::Octave, ..up3 };
        assert_eq!(harmony_note(64, &oct, Some(e_major()), Some(&a)), 76);
    }

    #[test]
    fn range_folds_by_octaves() {
        let rule = HarmonyRule { interval: Interval::Third, up: true, stay_in_key: true, follow_chords: false, range: (59, 76) };
        assert_eq!(harmony_note(75, &rule, Some(e_major()), None), 66, "F#5 folds down to F#4");
        assert_eq!(fold_into(50, (59, 76)), 62);
        assert_eq!(fold_into(64, (59, 76)), 64);
        assert_eq!(fold_into(71, (60, 70)), 71, "no octave of B fits: one semitone out either way, the note kept");
        assert_eq!(fold_into(73, (60, 70)), 61);
        assert_eq!(Interval::parse("3rd"), Some(Interval::Third));
        assert_eq!(Interval::parse("Octave"), Some(Interval::Octave));
        assert_eq!(Interval::parse("9th"), None);
    }
}
