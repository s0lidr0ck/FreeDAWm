//! Port of `GuitarShapes.h`.
//!
//! Real guitar fingerings (Ample notes mode): a chord as a guitarist would finger it - one note per string, the
//! fretted notes within 4 frets, muted strings only at the bottom, the root (or a slash chord's bass) lowest - chosen
//! near the previous shape, the way a player stays in position. Pure functions, no allocation (audio thread).
//!
//! Never-crash notes: a bit test or shift by a pitch class outside 0..16 (only from out-of-range open strings or
//! capo, undefined behaviour in C++) reads as "not in the set" / sets nothing.

#![allow(clippy::needless_range_loop)] // indexed loops kept as the C++ writes them

/// `(pcs >> pc) & 1`, false for a shift outside the word.
fn has_pc(pcs: u16, pc: i32) -> bool {
    u32::try_from(pc).ok().and_then(|s| pcs.checked_shr(s)).is_some_and(|v| v & 1 != 0)
}

/// `1u << pc` as a 16-bit set, 0 for a shift outside the word.
fn pc_bit(pc: i32) -> u16 {
    u32::try_from(pc).ok().and_then(|s| 1u16.checked_shl(s)).unwrap_or(0)
}

/// Frets from the capo for strings 6th (low) .. 1st (high); -1 = muted, 0 = open (at the capo).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub fret: [i32; 6],
    pub valid: bool,
}

impl Default for Shape {
    fn default() -> Self {
        Self { fret: [-1, -1, -1, -1, -1, -1], valid: false }
    }
}

impl Shape {
    pub fn lowest_fretted(&self) -> i32 {
        let mut lo = 99;
        for &f in &self.fret {
            if f > 0 && f < lo {
                lo = f;
            }
        }
        if lo == 99 { 0 } else { lo }
    }
    pub fn highest_fretted(&self) -> i32 {
        let mut hi = 0;
        for &f in &self.fret {
            if f > hi {
                hi = f;
            }
        }
        hi
    }
    pub fn sounding(&self) -> i32 {
        let mut n = 0;
        for &f in &self.fret {
            if f >= 0 {
                n += 1;
            }
        }
        n
    }
    /// Where the hand is: the lowest fretted fret (0 for an all-open-strings shape).
    pub fn position(&self) -> i32 {
        self.lowest_fretted()
    }
}

/// The shape's pitches per string (-1 = muted): open string + capo + fret.
pub fn notes(s: &Shape, open: &[i32; 6], capo: i32) -> [i32; 6] {
    let mut out = [-1, -1, -1, -1, -1, -1];
    for i in 0..6 {
        if s.fret[i] >= 0 {
            out[i] = open[i].wrapping_add(capo).wrapping_add(s.fret[i]);
        }
    }
    out
}

/// The string (0 = 6th) a player would play a single `note` on, its fret (from the capo) in `fret`; -1 if none.
/// The current chord shape's string when the note is in it; else, in position, the lowest string whose fret is
/// 1..7, else an open string.
pub fn string_for(note: i32, open: &[i32; 6], capo: i32, shape: Option<&Shape>, fret: &mut i32) -> i32 {
    *fret = -1;
    if let Some(shape) = shape
        && shape.valid
    {
        let ns = notes(shape, open, capo);
        for s in 0..6 {
            if ns[s] == note {
                *fret = shape.fret[s];
                return s as i32;
            }
        }
    }
    for s in 0..6 {
        let f = note.wrapping_sub(open[s]).wrapping_sub(capo);
        if (1..=7).contains(&f) {
            *fret = f;
            return s as i32;
        }
    }
    for s in 0..6 {
        if note == open[s].wrapping_add(capo) {
            *fret = 0;
            return s as i32;
        }
    }
    -1
}

/// The best fingering of a chord: `pcs` = its pitch classes (bit per pc, absolute), `required` = the ones it must
/// contain (root and 3rd), `bass_pc` = the lowest note's pitch class. Near `prev` when given; `r` (0..1) breaks ties.
/// Invalid when nothing on the neck (frets 0..15) fits.
pub fn best(pcs: u16, required: u16, bass_pc: i32, capo: i32, open: &[i32; 6], prev: Option<&Shape>, r: f32) -> Shape {
    let mut best_shape = Shape::default();
    let mut best_score = -1.0e9_f64;
    let prev_pos = match prev {
        Some(p) if p.valid => p.position(),
        _ => -1,
    };
    for base in 1..=12 {
        // the four-fret window [base, base + 3] (+ open strings)
        // Each string: muted, open (if its pc fits and the window is low), or a fret in the window with a chord pc.
        let mut opts = [[0i32; 6]; 6];
        let mut n_opts = [0usize; 6];
        for s in 0..6 {
            let mut n = 0usize;
            opts[s][n] = -1;
            n += 1;
            let open_pc = open[s].wrapping_add(capo) % 12;
            if base <= 4 && has_pc(pcs, open_pc) {
                opts[s][n] = 0;
                n += 1;
            }
            let mut f = base;
            while f <= base + 3 && n < 6 {
                if has_pc(pcs, (open_pc + f) % 12) {
                    opts[s][n] = f;
                    n += 1;
                }
                f += 1;
            }
            n_opts[s] = n;
        }
        let mut idx = [0usize; 6];
        loop {
            let mut c = Shape::default();
            for s in 0..6 {
                c.fret[s] = opts[s][idx[s]];
            }
            // Rules: mutes only below the lowest sounding string; at least 4 strings; root / bass lowest; root + 3rd.
            let mut lowest: i32 = -1;
            let mut ok = true;
            for s in 0..6 {
                if c.fret[s] >= 0 {
                    if lowest < 0 {
                        lowest = s as i32;
                    }
                } else if lowest >= 0 {
                    ok = false;
                    break;
                }
            }
            let lowest_note = usize::try_from(lowest).ok().map(|l| open[l].wrapping_add(capo).wrapping_add(c.fret[l]) % 12);
            if ok && lowest >= 0 && c.sounding() >= 4 && lowest_note == Some(bass_pc) {
                let mut have: u16 = 0;
                for s in 0..6 {
                    if c.fret[s] >= 0 {
                        have |= pc_bit(open[s].wrapping_add(capo).wrapping_add(c.fret[s]) % 12);
                    }
                }
                let lo = c.lowest_fretted();
                let hi = c.highest_fretted();
                if (have & required) == required && (lo == 0 || hi - lo <= 3) {
                    let mut tones = 0;
                    for b in 0..12 {
                        if has_pc(have, b) {
                            tones += 1;
                        }
                    }
                    let mut uses_open = false;
                    for &f in &c.fret {
                        if f == 0 {
                            uses_open = true;
                        }
                    }
                    let mut score = 3.0 * f64::from(tones) + 1.0 * f64::from(c.sounding()) - 0.15 * f64::from(lo);
                    if uses_open && hi <= 4 {
                        score += 2.0; // open chords where they're natural
                    }
                    if prev_pos >= 0 {
                        score -= 1.5 * f64::from((c.position() - prev_pos).abs());
                    }
                    score += 0.01 * f64::from(r) * f64::from((base * 7 + idx[0] as i32 * 3 + idx[5] as i32) % 11);
                    if score > best_score {
                        best_score = score;
                        best_shape = c;
                        best_shape.valid = true;
                    }
                }
            }
            let mut s = 0;
            while s < 6 {
                idx[s] += 1;
                if idx[s] < n_opts[s] {
                    break;
                }
                idx[s] = 0;
                s += 1;
            }
            if s == 6 {
                break;
            }
        }
    }
    best_shape
}
