//! Port of `AmpleSlots.h`.
//!
//! AmpleSlots -- "Ample chords = Slots": the song's distinct chords on Ample's 12 chord slots
//! (keys C1-B1 = 36-47 select slot 1-12, Chord Detect off). Allocation-free once
//! the song's chords are collected (collect_ample_chords, on the message thread).

use crate::song::SongSection;

pub const K_AMPLE_SLOTS: i32 = 12;
pub const K_AMPLE_MAX_CHORDS: usize = 64;

/// A chord as Ample sees it: absolute root pc, the other tones as a 12-bit interval set over the root, absolute bass pc (-1 none).
/// The derived order is the C++ `operator<`: root, then mask, then bass.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AmpleChordKey {
    pub root: i32,
    pub mask: i32,
    pub bass: i32,
}

impl Default for AmpleChordKey {
    fn default() -> Self {
        Self { root: 0, mask: 0, bass: -1 }
    }
}

/// One chord of the song: its slot (0..111) and key-relative bass (-1 = root).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmpleSongChord {
    pub slot: i32,
    pub bass: i32,
}

impl Default for AmpleSongChord {
    fn default() -> Self {
        Self { slot: 0, bass: -1 }
    }
}

#[derive(Clone, Debug)]
pub struct AmpleSongChords {
    pub n: usize,
    pub c: [AmpleSongChord; K_AMPLE_MAX_CHORDS],
}

impl Default for AmpleSongChords {
    fn default() -> Self {
        Self { n: 0, c: [AmpleSongChord::default(); K_AMPLE_MAX_CHORDS] }
    }
}

pub fn ample_mod12(v: i32) -> i32 {
    v.rem_euclid(12)
}

/// The key from a chord's key-relative root, its tones (tones[0] = 0) and key-relative bass: the tones the Strummer sends.
pub fn ample_chord_key(rel_root: i32, tones: &[i32], count: i32, rel_bass: i32, key_root: i32) -> AmpleChordKey {
    let mut k = AmpleChordKey { root: ample_mod12(key_root.wrapping_add(rel_root)), ..AmpleChordKey::default() };
    let upto = if count < 4 { count } else { 4 };
    for i in 1..upto.max(1) {
        if let Some(&t) = tones.get(i as usize) {
            k.mask |= 1 << ample_mod12(t);
        }
    }
    k.bass = if rel_bass >= 0 { ample_mod12(key_root.wrapping_add(rel_bass)) } else { -1 };
    k
}

/// The key of a song chord slot: mirrors the engine's buildChord (and the UI's describeChord).
pub fn ample_chord_key_of_slot(slot: i32, bass: i32, key_root: i32, scale_mode: i32) -> AmpleChordKey {
    let semis = |mode: usize, deg: i32| -> i32 {
        const STEPS: [[i32; 7]; 2] = [[0, 2, 4, 5, 7, 9, 11], [0, 2, 3, 5, 7, 8, 10]];
        let d = deg.rem_euclid(7);
        let oct = (deg - d) / 7;
        12 * oct + STEPS[mode & 1][d as usize]
    };
    let v = slot.clamp(0, 111);
    let degree = if v / 16 > 6 { 6 } else { v / 16 };
    let mut flavor = v % 16;
    if flavor > 8 {
        flavor = 0;
    }
    let minor_scale = usize::from(scale_mode != 0);
    let mode = if flavor == 7 || flavor == 8 { 1 - minor_scale } else { minor_scale };
    let r = semis(mode, degree);
    let mut t = [0, semis(mode, degree + 2) - r, semis(mode, degree + 4) - r, semis(mode, degree + 6) - r];
    let mut count = 3;
    if flavor == 1 || flavor == 8 {
        count = 4;
    } else if flavor == 2 {
        t[1] = 4;
        t[2] = 7;
        t[3] = 10;
        count = 4;
    } else if flavor == 3 {
        t[1] = 2;
    } else if flavor == 4 {
        t[1] = 5;
    } else if flavor == 5 {
        t[3] = 14;
        count = 4;
    } else if flavor == 6 {
        t[3] = 9;
        count = 4;
    }
    ample_chord_key(ample_mod12(r), &t, count, bass, key_root)
}

/// Every chord the song's sections play (an old "Original" section: the first Idea's roots, as the UI shows them).
pub fn collect_ample_chords(sections: &[SongSection], out: &mut AmpleSongChords) {
    const AUTHORED_ROOTS: [i32; 4] = [0, 5, 3, 4];
    out.n = 0;
    for s in sections {
        let count = if s.follow_idea { 4 } else { s.chords.len() };
        let mut i = 0;
        while i < count && out.n < K_AMPLE_MAX_CHORDS {
            let mut c = AmpleSongChord::default();
            if s.follow_idea {
                c.slot = AUTHORED_ROOTS[i] * 16;
            } else if let Some(sc) = s.chords.get(i) {
                c.slot = sc.slot;
                c.bass = sc.bass;
            }
            let mut seen = false; // the cap counts distinct chords
            let mut j = 0;
            while j < out.n && !seen {
                seen = out.c[j].slot == c.slot && out.c[j].bass == c.bass;
                j += 1;
            }
            if !seen {
                out.c[out.n] = c;
                out.n += 1;
            }
            i += 1;
        }
    }
}

/// The song's distinct chords, sorted (root pc from C, then tones, then bass), each on its slot 1..12.
/// Past twelve a chord shares the slot of the first chord with its root (else slot 12) and is marked shared.
#[derive(Clone, Debug)]
pub struct AmpleSlotTable {
    pub n: usize,
    pub key: [AmpleChordKey; K_AMPLE_MAX_CHORDS],
    /// the first song chord with this key (for naming)
    pub from: [AmpleSongChord; K_AMPLE_MAX_CHORDS],
    pub slot: [i32; K_AMPLE_MAX_CHORDS],
    pub shared: [bool; K_AMPLE_MAX_CHORDS],
}

impl Default for AmpleSlotTable {
    fn default() -> Self {
        Self {
            n: 0,
            key: [AmpleChordKey::default(); K_AMPLE_MAX_CHORDS],
            from: [AmpleSongChord::default(); K_AMPLE_MAX_CHORDS],
            slot: [0; K_AMPLE_MAX_CHORDS],
            shared: [false; K_AMPLE_MAX_CHORDS],
        }
    }
}

pub fn build_ample_slots(chords: &AmpleSongChords, key_root: i32, scale_mode: i32, t: &mut AmpleSlotTable) {
    t.n = 0;
    for c in chords.c.iter().take(chords.n.min(K_AMPLE_MAX_CHORDS)) {
        let k = ample_chord_key_of_slot(c.slot, c.bass, key_root, scale_mode);
        let mut at = 0;
        let mut dup = false;
        while at < t.n {
            if t.key[at] == k {
                dup = true;
                break;
            }
            if k < t.key[at] {
                break;
            }
            at += 1;
        }
        if dup {
            continue;
        }
        let mut j = t.n;
        while j > at {
            t.key[j] = t.key[j - 1];
            t.from[j] = t.from[j - 1];
            j -= 1;
        }
        t.key[at] = k;
        t.from[at] = *c;
        t.n += 1;
    }
    let slots = K_AMPLE_SLOTS as usize;
    for i in 0..t.n {
        t.shared[i] = i >= slots;
        if i < slots {
            t.slot[i] = i as i32 + 1;
            continue;
        }
        let mut first = i;
        for j in 0..i {
            if t.key[j].root == t.key[i].root {
                first = j;
                break;
            }
        }
        t.slot[i] = if first < slots { first as i32 + 1 } else { K_AMPLE_SLOTS };
        if first < slots {
            t.shared[first] = true;
        }
    }
}

/// The slot (1..12) a chord plays from; a chord the song doesn't have takes its root's first slot, else 12.
pub fn ample_slot_for(t: &AmpleSlotTable, k: &AmpleChordKey) -> i32 {
    let n = t.n.min(K_AMPLE_MAX_CHORDS);
    for i in 0..n {
        if t.key[i] == *k {
            return t.slot[i];
        }
    }
    let slots = K_AMPLE_SLOTS as usize;
    for i in 0..n.min(slots) {
        if t.key[i].root == k.root {
            return i as i32 + 1;
        }
    }
    K_AMPLE_SLOTS
}
