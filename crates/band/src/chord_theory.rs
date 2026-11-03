//! Port of `ChordTheory.h`.
//!
//! ChordTheory — what a chord slot plays, outside the engine and the UI (JUCE-free).
//! Mirrors Musician::buildChord (the tones) and ui::describeChord (the name), so the chord track in
//! exports (5b) and the chord matcher (5c) agree with what the band plays and what the grid shows.

const STEPS: [[i32; 7]; 2] = [[0, 2, 4, 5, 7, 9, 11], [0, 2, 3, 5, 7, 8, 10]];

/// `steps[mode]`: mode is 0 or 1 from every caller; anything else reads as major (C++ would read out of bounds).
fn steps(mode: i32) -> &'static [i32; 7] {
    if mode == 1 { &STEPS[1] } else { &STEPS[0] }
}

pub fn scale_offset(mode: i32, deg: i32) -> i32 {
    let oct = (f64::from(deg) / 7.0).floor() as i32;
    12 * oct + steps(mode)[((deg % 7) + 7).rem_euclid(7) as usize]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    /// absolute pitch class (key applied)
    pub root_pc: i32,
    pub count: i32,
    /// semitones above the root
    pub tones: [i32; 4],
    /// absolute pitch class of a slash bass, -1 = the root
    pub bass_pc: i32,
}

impl Default for Shape {
    fn default() -> Self {
        Self { root_pc: 0, count: 3, tones: [0; 4], bass_pc: -1 }
    }
}

/// slot 0..111 (degree * 16 + flavour), key 0..11, scale 0 major / 1 minor, bass -1 or key-relative pc.
pub fn shape_for(slot: i32, key: i32, scale: i32, bass: i32) -> Shape {
    let mut c = Shape::default();
    let v = slot.clamp(0, 111);
    let degree = (v / 16).min(6);
    let mut flavor = v % 16;
    if flavor > 8 {
        flavor = 0;
    }
    let mode = if flavor == 7 || flavor == 8 { 1 - scale } else { scale };
    let r = scale_offset(mode, degree);
    let third = scale_offset(mode, degree + 2) - r;
    let fifth = scale_offset(mode, degree + 4) - r;
    let seventh = scale_offset(mode, degree + 6) - r;
    c.root_pc = ((key.wrapping_add(r) % 12) + 12) % 12;
    c.tones = [0, third, fifth, 0];
    if flavor == 1 || flavor == 8 {
        c.tones[3] = seventh;
        c.count = 4;
    } else if flavor == 2 {
        c.tones = [0, 4, 7, 10];
        c.count = 4;
    } else if flavor == 3 {
        c.tones[1] = 2;
    } else if flavor == 4 {
        c.tones[1] = 5;
    } else if flavor == 5 {
        c.tones[3] = 14;
        c.count = 4;
    } else if flavor == 6 {
        c.tones[3] = 9;
        c.count = 4;
    }
    c.bass_pc = if bass < 0 { -1 } else { ((key.wrapping_add(bass) % 12) + 12) % 12 };
    c
}

pub fn note_name(pc: i32) -> &'static str {
    const N: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
    N[((pc % 12) + 12).rem_euclid(12) as usize]
}

/// "Am7", "G/B", "Bbmaj7", "F#m7b5" — the grid's names, in plain ASCII.
pub fn name_for(slot: i32, key: i32, scale: i32, bass: i32) -> String {
    let v = slot.clamp(0, 111);
    let degree = (v / 16).min(6);
    let mut flavor = v % 16;
    if flavor > 8 {
        flavor = 0;
    }
    let mode = if flavor == 7 || flavor == 8 { 1 - scale } else { scale };
    let r = scale_offset(mode, degree);
    let mut third = scale_offset(mode, degree + 2) - r;
    let mut fifth = scale_offset(mode, degree + 4) - r;
    let mut seventh = scale_offset(mode, degree + 6) - r;
    if flavor == 2 {
        third = 4;
        fifth = 7;
        seventh = 10;
    }
    let minor = third == 3;
    let dim = minor && fifth == 6;
    let mut suffix: &str = if dim {
        "dim"
    } else if minor {
        "m"
    } else {
        ""
    };
    let seventh_name = || -> &'static str {
        if seventh == 11 {
            if minor { "m(maj7)" } else { "maj7" }
        } else if dim {
            "m7b5"
        } else if minor {
            "m7"
        } else {
            "7"
        }
    };
    if flavor == 1 || flavor == 8 {
        suffix = seventh_name();
    } else if flavor == 2 {
        suffix = "7";
    } else if flavor == 3 {
        suffix = "sus2";
    } else if flavor == 4 {
        suffix = "sus4";
    } else if flavor == 5 {
        suffix = if minor { "m(add9)" } else { "add9" };
    } else if flavor == 6 {
        suffix = if minor { "m6" } else { "6" };
    }
    let mut n = String::from(note_name(key.wrapping_add(r)));
    n.push_str(suffix);
    if bass >= 0 {
        n.push('/');
        n.push_str(note_name(key.wrapping_add(bass)));
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_match_build_chord() {
        let s = shape_for(66, 0, 0, -1); // V7 in C
        assert_eq!((s.root_pc, s.count, s.tones), (7, 4, [0, 4, 7, 10]));
        let s = shape_for(80, 2, 0, 4); // vi in D over F#
        assert_eq!((s.root_pc, s.tones[1], s.bass_pc), (11, 3, 6));
        assert_eq!(name_for(71, 0, 1, -1), "G"); // Andalusian V in minor: flavor 7 borrows the major triad
        assert_eq!(name_for(-5, 0, 0, -1), "C");
        assert_eq!(name_for(999, 0, 0, -1), "Bdim");
    }
}
