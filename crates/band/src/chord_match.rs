//! Port of `ChordMatch.h` / `ChordMatch.cpp`.
//!
//! ChordMatch — a chord from outside (a typed name, MIDI notes, keys held live) to a slot the song can
//! hold (Phase 5c). Snaps to the key when the chord isn't one of the song's 63 chords per key.
//!
//! Text is handled as UTF-8 bytes, as the C++ `std::string` is: the C locale's `isspace` / `tolower` / `toupper`.

#![allow(clippy::needless_range_loop)] // indexed loops kept as the C++ writes them

/// Absolute pitch classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChordPcs {
    pub root_pc: i32,
    pub pcs: [bool; 12],
    pub bass_pc: i32,
}

impl Default for ChordPcs {
    fn default() -> Self {
        Self { root_pc: 0, pcs: [false; 12], bass_pc: -1 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchResult {
    pub slot: i32,
    pub bass: i32,
    pub exact: bool,
}

impl Default for MatchResult {
    fn default() -> Self {
        Self { slot: 0, bass: -1, exact: false }
    }
}

use crate::chord_theory::shape_for;

/// Simpler first: triad, 7th, dom 7, borrowed, sus, add9 / 6 (ties go to the lower rank).
fn rank_of(slot: i32) -> i32 {
    const R: [i32; 9] = [0, 1, 2, 4, 4, 5, 5, 3, 3];
    R[8.min(slot.rem_euclid(16)) as usize]
}

/// The 63 chords per key, in slot order (the C++ builds this as a vector).
fn candidates() -> impl Iterator<Item = i32> {
    (0..112).filter(|s| s % 16 <= 8)
}

fn rel_bass(bass_pc: i32, root_pc: i32, key: i32) -> i32 {
    if bass_pc >= 0 && bass_pc != root_pc { (bass_pc.wrapping_sub(key) % 12 + 12) % 12 } else { -1 }
}

/// ♯ ♭ Δ ° ø as ASCII tokens: # b ^ o h.
fn ascii_tokens(s: &[u8]) -> Vec<u8> {
    const MAP: [(&[u8], u8); 6] = [
        (b"\xe2\x99\xaf", b'#'),
        (b"\xe2\x99\xad", b'b'),
        (b"\xce\x94", b'^'),
        (b"\xe2\x88\x86", b'^'), // the increment sign macOS types for a triangle
        (b"\xc2\xb0", b'o'),
        (b"\xc3\xb8", b'h'),
    ];
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        if let Some(&(seq, tok)) = MAP.iter().find(|(seq, _)| rest.starts_with(seq)) {
            out.push(tok);
            i += seq.len();
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

/// C locale `isspace`: space, \t \n \v \f \r.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

fn trim(s: &[u8]) -> &[u8] {
    let mut a = 0;
    let mut b = s.len();
    while a < b && is_space(s[a]) {
        a += 1;
    }
    while b > a && is_space(s[b - 1]) {
        b -= 1;
    }
    &s[a..b]
}

/// "C", "c#", "Bb" -> pitch class; consumed = characters used; -1 if not a note.
fn note_pc(s: &[u8], consumed: &mut usize) -> i32 {
    const BASE: [i32; 7] = [9, 11, 0, 2, 4, 5, 7]; // A B C D E F G
    let Some(&first) = s.first() else { return -1 };
    let l = first.to_ascii_uppercase();
    if !(b'A'..=b'G').contains(&l) {
        return -1;
    }
    let mut pc = BASE[usize::from(l - b'A')];
    *consumed = 1;
    let mut sharp = false;
    let mut flat = false;
    while let Some(&c) = s.get(*consumed) {
        if c != b'#' && c != b'b' {
            break;
        }
        if c == b'#' {
            sharp = true;
        } else {
            flat = true;
        }
        if sharp && flat {
            return -1; // "C#b": not a note
        }
        // "Cb" is C-flat; but a lone 'b' must not eat a quality like "b5" (none starts with b, so it's safe).
        pc += if c == b'#' { 1 } else { -1 };
        *consumed += 1;
    }
    pc.rem_euclid(12)
}

fn quality_tones(q: &[u8]) -> &'static [i32] {
    // Case-sensitive first: an upper-case M means major.
    match q {
        b"M" | b"" => return &[0, 4, 7],
        b"M7" | b"^" | b"^7" => return &[0, 4, 7, 11],
        b"M9" | b"^9" => return &[0, 4, 7, 11, 2],
        b"M6" => return &[0, 4, 7, 9],
        b"Madd9" | b"Madd2" => return &[0, 4, 7, 2],
        b"mM7" | b"-^7" | b"m^7" => return &[0, 3, 7, 11],
        _ => {}
    }
    let l = q.to_ascii_lowercase();
    match l.as_slice() {
        b"maj" | b"major" => &[0, 4, 7],
        b"m" | b"min" | b"-" | b"mi" => &[0, 3, 7],
        b"dim" | b"o" => &[0, 3, 6],
        b"dim7" | b"o7" => &[0, 3, 6, 9],
        b"m7b5" | b"min7b5" | b"-7b5" | b"mi7b5" | b"h" | b"h7" => &[0, 3, 6, 10],
        b"aug" | b"+" => &[0, 4, 8],
        b"7" | b"dom7" => &[0, 4, 7, 10],
        b"maj7" | b"ma7" | b"j7" => &[0, 4, 7, 11],
        b"m7" | b"min7" | b"-7" | b"mi7" => &[0, 3, 7, 10],
        b"sus" | b"sus4" => &[0, 5, 7],
        b"sus2" => &[0, 2, 7],
        b"7sus4" | b"7sus" => &[0, 5, 7, 10],
        b"6" | b"maj6" => &[0, 4, 7, 9],
        b"m6" | b"min6" | b"-6" => &[0, 3, 7, 9],
        b"add9" | b"add2" | b"2" => &[0, 4, 7, 2],
        b"madd9" | b"m(add9)" | b"minadd9" => &[0, 3, 7, 2],
        b"9" => &[0, 4, 7, 10, 2],
        b"m9" | b"min9" | b"-9" => &[0, 3, 7, 10, 2],
        b"maj9" => &[0, 4, 7, 11, 2],
        b"mmaj7" | b"m(maj7)" | b"minmaj7" => &[0, 3, 7, 11],
        _ => &[],
    }
}

/// The pitch classes a slot plays in `key` / `scale`.
/// (Ported here from `ChordMatch.cpp`, where MS defines it; `live.rs` (Task 3) uses it from here.)
pub fn slot_pitch_classes(slot: i32, key: i32, scale: i32) -> [bool; 12] {
    let mut p = [false; 12];
    let sh = shape_for(slot, key, scale, -1);
    for &t in sh.tones.iter().take(usize::try_from(sh.count).unwrap_or(0)) {
        p[sh.root_pc.wrapping_add(t).rem_euclid(12) as usize] = true;
    }
    p
}

pub fn match_chord(c: &ChordPcs, key: i32, scale: i32) -> MatchResult {
    let mut best = MatchResult::default();
    let mut best_score = -1.0e9_f64;
    for s in candidates() {
        let sp = slot_pitch_classes(s, key, scale);
        let root = shape_for(s, key, scale, -1).root_pc;
        let mut common = 0;
        let mut slot_only = 0;
        let mut in_only = 0;
        for i in 0..12 {
            if sp[i] && c.pcs[i] {
                common += 1;
            } else if sp[i] {
                slot_only += 1;
            } else if c.pcs[i] {
                in_only += 1;
            }
        }
        let exact = slot_only == 0 && in_only == 0 && root == c.root_pc;
        let score = if exact {
            1000.0 - f64::from(rank_of(s))
        } else {
            (if root == c.root_pc { 100.0 } else { 0.0 }) + 10.0 * f64::from(common)
                - 4.0 * f64::from(slot_only)
                - 4.0 * f64::from(in_only)
                - 0.1 * f64::from(rank_of(s))
        };
        if score > best_score {
            best_score = score;
            best.slot = s;
            best.exact = exact;
        }
    }
    best.bass = rel_bass(c.bass_pc, c.root_pc, key);
    best
}

pub fn match_profile(w: &[f32; 12], lowest_pc: i32, key: i32, scale: i32) -> MatchResult {
    let mut total = 0.0f32;
    let mut max_w = 0.0f32;
    for &x in w {
        total += x;
        max_w = if max_w < x { x } else { max_w };
    }
    let mut best = MatchResult::default();
    if max_w <= 0.0 {
        return best;
    }
    let mut heard = [false; 12];
    for i in 0..12 {
        heard[i] = w[i] > 0.2 * max_w;
    }
    let mut best_score = -1.0e9_f64;
    let mut best_root = 0;
    let max_wd = f64::from(max_w);
    for s in candidates() {
        let sp = slot_pitch_classes(s, key, scale);
        let root = shape_for(s, key, scale, -1).root_pc;
        let mut inw = 0.0f64;
        let mut missing = 0;
        for i in 0..12 {
            if sp[i] {
                inw += f64::from(w[i]);
                if !heard[i] {
                    missing += 1;
                }
            }
        }
        let score = inw - 0.6 * (f64::from(total) - inw) - 0.25 * max_wd * f64::from(missing) + (if root == lowest_pc { 0.35 * max_wd } else { 0.0 })
            - 0.01 * max_wd * f64::from(rank_of(s));
        if score > best_score {
            best_score = score;
            best.slot = s;
            best.exact = sp == heard;
            best_root = root;
        }
    }
    best.bass = rel_bass(lowest_pc, best_root, key);
    best
}

pub fn parse_chord_name(text: &str) -> Option<ChordPcs> {
    let tokens = ascii_tokens(text.as_bytes());
    let s = trim(&tokens);
    if s.is_empty() || s.contains(&b' ') {
        return None;
    }
    let mut used = 0usize;
    let root = note_pc(s, &mut used);
    if root < 0 {
        return None;
    }
    let mut rest = s.get(used..).unwrap_or(&[]);
    let mut bass_text: &[u8] = &[];
    if let Some(slash) = rest.iter().position(|&c| c == b'/') {
        bass_text = &rest[slash + 1..];
        rest = &rest[..slash];
        if bass_text.is_empty() {
            return None;
        }
    }
    let tones = quality_tones(rest);
    if tones.is_empty() {
        return None;
    }
    let mut c = ChordPcs { root_pc: root, bass_pc: root, ..ChordPcs::default() };
    for &t in tones {
        c.pcs[((root + t) % 12) as usize] = true;
    }
    if !bass_text.is_empty() {
        let mut bu = 0usize;
        let b = note_pc(bass_text, &mut bu);
        if b < 0 || bu != bass_text.len() {
            return None;
        }
        c.bass_pc = b;
    }
    Some(c)
}
