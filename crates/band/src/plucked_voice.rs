//! Port of `PluckedVoice.h`.
//!
//! PluckedVoice -- engine-free decisions shared by the evolution and indiginusStrings drivers
//! (spec 2026-10-07 section 1: strumPlan, rollPlan, tremoloPlan, legatoJoin, chooseOrnament,
//! vibratoCurve). Pure functions, no allocation. Task 2: strumPlan, legatoJoin, vibratoCurve.
//!
//! C++ `(ptr, n)` inputs are slices (the first `n` read); `out` arrays are slices written up to their length.

/// `std::clamp` as MSVC writes it (`v < lo ? lo : hi < v ? hi : v`): never panics on reversed bounds.
fn clampi(v: i32, lo: i32, hi: i32) -> i32 {
    if v < lo {
        lo
    } else if hi < v {
        hi
    } else {
        v
    }
}

/// A written count never claims more notes than `out` holds (C++ trusted `maxOut`).
fn clamp_count(n: i32, len: usize) -> i32 {
    n.min(i32::try_from(len).unwrap_or(i32::MAX))
}

fn put(out: &mut [StrumNote], i: usize, v: StrumNote) {
    if let Some(slot) = out.get_mut(i) {
        *slot = v;
    }
}

/// A momentary articulation key being held, and its owner (ruling 2026-10-07: Evolution keys are momentary,
/// held from 15 ms before the owner until 5 ms after its last note-off). One per musician.
/// `owner_end` is the owner's end in 1/960 ppq ticks; the doubles are ppq: the key's press, the owner's
/// (latest) note-on, the key's release. -1 = none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeldKey {
    pub key: i32,
    pub owner_end: i32,
    pub key_on: f64,
    pub owner_on: f64,
    pub key_off: f64,
}

impl Default for HeldKey {
    fn default() -> Self {
        Self { key: -1, owner_end: -1, key_on: -1.0, owner_on: -1.0, key_off: -1.0 }
    }
}

/// One note of a strum: its pitch, its time after the strum's first note, its velocity.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StrumNote {
    pub pitch: i32,
    pub offset_ms: f64,
    pub velocity: i32,
}

/// A strum played with notes (spec 3.5). `voicing` in any order (sorted here), up to 8 tones.
/// Down: low -> high over all kept tones; up: high -> low over the top `up_strings`, velocity x `up_weight`.
/// `strum_ms` is the whole strum's spread (x 0.7 at Energy 4), humanised +-20 % (`humanise01` 0..1, .5 = none);
/// velocity -4 % per string from `base_vel`, clamped 1..127. Without a bypass-legato key a tone 1-2 ST above the
/// last kept one is dropped (Evolution would hammer it). Returns the count written to `out` (<= 8).
pub fn strum_plan(
    voicing: &[i32],
    n: i32,
    down: bool,
    strum_ms: i32,
    up_strings: i32,
    up_weight: f32,
    base_vel: i32,
    energy: i32,
    humanise01: f64,
    bypass_legato: bool,
    out: &mut [StrumNote],
) -> i32 {
    let mut v = [0i32; 8];
    let mut m = 0usize;
    for &x in voicing.iter().take(usize::try_from(n).unwrap_or(0)) {
        if m >= 8 {
            break;
        }
        v[m] = x;
        m += 1;
    }
    v[..m].sort_unstable();
    let mut kept = [0i32; 8];
    let mut k = 0usize;
    for &x in &v[..m] {
        if k > 0 && x == kept[k - 1] {
            continue; // a doubled pitch: once
        }
        if k > 0 && !bypass_legato && x - kept[k - 1] <= 2 {
            continue; // the legato trap
        }
        kept[k] = x;
        k += 1;
    }
    if k == 0 {
        return 0;
    }
    let ki = k as i32;
    let mut first = 0i32;
    let mut count = ki;
    if !down {
        count = 1.max(up_strings.min(ki));
        first = ki - count;
    }
    let spread = f64::from(strum_ms) * (if energy >= 4 { 0.7 } else { 1.0 }) * (1.0 + (humanise01.clamp(0.0, 1.0) * 2.0 - 1.0) * 0.2);
    for i in 0..count {
        let idx = if down { first + i } else { first + count - 1 - i };
        let pitch = kept.get(idx as usize).copied().unwrap_or(0);
        let at = if count > 1 { spread * f64::from(i) / f64::from(count - 1) } else { 0.0 };
        let vel = f64::from(base_vel) * (1.0 - 0.04 * f64::from(i)) * (if down { 1.0 } else { f64::from(up_weight) });
        put(out, i as usize, StrumNote { pitch, offset_ms: at, velocity: clampi(vel.round() as i32, 1, 127) });
    }
    clamp_count(count, out.len())
}

/// How a note follows the one before (spec 3.6): overlap 20 ms (a hammer / pull-off: 1-2 ST, the second on an
/// off-step, by the player's `hammer_amt`), a slide (by `slide_amt`, within `legato_st`, or `slide_range` when the
/// user widened it past 2), else a 3 ms gap so the engine never hammers by accident. Rolls are 0..1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    Gap,
    Overlap,
    Slide,
}

pub fn legato_join(
    from_pitch: i32,
    to_pitch: i32,
    off_step: bool,
    hammer_roll: f32,
    slide_roll: f32,
    hammer_amt: f32,
    slide_amt: f32,
    legato_st: i32,
    slide_range: i32,
) -> Join {
    let iv = (to_pitch.wrapping_sub(from_pitch)).wrapping_abs();
    if from_pitch < 0 || iv == 0 {
        return Join::Gap;
    }
    if iv <= 2.min(legato_st) && off_step && hammer_roll < hammer_amt {
        return Join::Overlap;
    }
    let reach = if slide_range > 2 { slide_range } else { legato_st };
    if iv <= reach && slide_roll < slide_amt {
        return Join::Slide;
    }
    Join::Gap
}

/// CC1 vibrato over a held note (spec 3.6): 0 for the first half, rising to depth x 90 at its end.
pub fn vibrato_curve(pos_in_note01: f64, depth: f32) -> f32 {
    if pos_in_note01 <= 0.5 {
        return 0.0;
    }
    let y = (pos_in_note01 - 0.5) / 0.5;
    let x = if y < 1.0 { y } else { 1.0 };
    (f64::from(depth) * 90.0 * x).round() as f32
}

// ---- Task 3: rolls, tremolo, ornaments ------------------------------------------------------------------------

/// One group of a banjo roll (spec 3.7): four sixteenths, the thumb on the first. `rolls` 1 forward (thumb, then
/// the picks low -> high), 2 backward (high -> low), 3 alternating (forward on even groups, backward on odd),
/// 4 foggy (forward, a reversed group every 4th: group_index % 4 == 3). The picks are the top three of `strings`
/// (fewer are cycled). Written to `out[0..3]`: `offset_ms` holds the sixteenth (0..3) inside the group, `velocity`
/// the accent over the roll's base (+8 on the thumb, 0 else). Returns 4 (0 when there is nothing to pick).
pub fn roll_plan(rolls: i32, group_index: i32, strings: &[i32], n: i32, thumb_pitch: i32, out: &mut [StrumNote]) -> i32 {
    if n <= 0 || rolls <= 0 {
        return 0;
    }
    let mut s = [0i32; 8];
    let mut m = 0usize;
    for &x in strings.iter().take(usize::try_from(n).unwrap_or(0)) {
        if m >= 8 {
            break;
        }
        s[m] = x;
        m += 1;
    }
    if m == 0 {
        return 0; // n > 0 but the slice is empty (C++ would read past it)
    }
    s[..m].sort_unstable();
    let k = 3.min(m);
    let first = m - k;
    let mut pick = [0i32; 3];
    for (i, p) in pick.iter_mut().enumerate() {
        *p = s[first + i % k];
    }
    let mut reverse = rolls == 2;
    if rolls == 3 {
        reverse = (group_index & 1) != 0;
    }
    if rolls == 4 {
        reverse = group_index.rem_euclid(4) == 3;
    }
    put(out, 0, StrumNote { pitch: thumb_pitch, offset_ms: 0.0, velocity: 8 });
    for i in 0..3usize {
        put(out, i + 1, StrumNote { pitch: if reverse { pick[2 - i] } else { pick[i] }, offset_ms: (i + 1) as f64, velocity: 0 });
    }
    clamp_count(4, out.len())
}

/// A tremolo played as repeated notes (spec 3.7): 32nds at <= 110 BPM, else 16ths, over `len_beats`; down / up
/// strokes alternate `vel_down` / `vel_up`; every stroke after the first jittered +-6 ms (a hash of `seed` and its
/// index: the same seed, the same tremolo). `offset_ms` from the first stroke; `pitch` 0 (the caller's).
/// (C++ default `seed = 0`.)
pub fn tremolo_plan(len_beats: f64, bpm: f64, vel_down: i32, vel_up: i32, out: &mut [StrumNote], max_out: i32, seed: u32) -> i32 {
    if bpm <= 0.0 || len_beats <= 0.0 || max_out <= 0 {
        return 0;
    }
    let sub = if bpm <= 110.0 { 0.125 } else { 0.25 };
    let ms_per_beat = 60000.0 / bpm;
    let count = clampi((len_beats / sub + 1.0e-6).floor() as i32, 1, max_out);
    for i in 0..count {
        let mut h: u32 = seed.wrapping_mul(2654435761).wrapping_add((i as u32).wrapping_mul(40503)).wrapping_add(0x9e3779b9);
        h ^= h >> 15;
        h = h.wrapping_mul(2246822519);
        h ^= h >> 13;
        let jitter = if i == 0 { 0.0 } else { f64::from(h % 1201) / 100.0 - 6.0 };
        put(
            out,
            i as usize,
            StrumNote { pitch: 0, offset_ms: f64::from(i) * sub * ms_per_beat + jitter, velocity: if (i & 1) == 0 { vel_down } else { vel_up } },
        );
    }
    clamp_count(count, out.len())
}

/// The one-note ornaments (spec 3.7): each held as a momentary key over its note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orn {
    None,
    GraceUp,
    GraceDown,
    Buzz,
    Harmonics,
    ReleaseSlide,
    Squeal,
}

/// What a single note is, and what the player and target have. Amounts 0..1; `has_*`: the target's key exists.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrnContext {
    pub phrase_start: bool,
    pub lick_end: bool,
    pub phrase_end: bool,
    pub section_end: bool,
    pub tonic_or_fifth: bool,
    pub accented_high: bool,
    pub from_below: bool,
    /// Hollowbody / Strat
    pub release_target: bool,
    pub len_beats: f32,
    pub energy: i32,
    pub graces: f32,
    pub buzz: f32,
    pub slides: f32,
    pub harmonics: f32,
    pub has_grace_up: bool,
    pub has_grace_down: bool,
    pub has_buzz: bool,
    pub has_harmonics: bool,
    pub has_release_slide: bool,
    pub has_squeal: bool,
}

impl Default for OrnContext {
    fn default() -> Self {
        Self {
            phrase_start: false,
            lick_end: false,
            phrase_end: false,
            section_end: false,
            tonic_or_fifth: false,
            accented_high: false,
            from_below: true,
            release_target: false,
            len_beats: 0.0,
            energy: 2,
            graces: 0.0,
            buzz: 0.0,
            slides: 0.0,
            harmonics: 0.0,
            has_grace_up: false,
            has_grace_down: false,
            has_buzz: false,
            has_harmonics: false,
            has_release_slide: false,
            has_squeal: false,
        }
    }
}

/// First that applies: harmonics (a section's end on the tonic / fifth), buzz trill (a phrase's last note of a
/// beat or more as written: >= .85 after humanize), release slide (.15 at a phrase end, players with slides, Hollowbody / Strat), grace (a phrase
/// start or a lick's end), squeal (an accented high note at Energy 4, by the harmonics amount). `rolls`: 0..1
/// draws for graces, buzz, release slide, harmonics / squeal (salts 1981-1984).
pub fn choose_ornament(c: &OrnContext, rolls: &[f32; 4]) -> Orn {
    if c.section_end && c.tonic_or_fifth && c.has_harmonics && rolls[3] < c.harmonics {
        return Orn::Harmonics;
    }
    if c.phrase_end && c.len_beats >= 0.85 && c.has_buzz && rolls[1] < c.buzz {
        return Orn::Buzz;
    }
    if c.phrase_end && c.release_target && c.slides > 0.0 && c.has_release_slide && rolls[2] < 0.15 {
        return Orn::ReleaseSlide;
    }
    if (c.phrase_start || c.lick_end) && rolls[0] < c.graces && (if c.from_below { c.has_grace_up } else { c.has_grace_down }) {
        return if c.from_below { Orn::GraceUp } else { Orn::GraceDown };
    }
    if c.accented_high && c.energy >= 4 && c.has_squeal && rolls[3] < c.harmonics {
        return Orn::Squeal;
    }
    Orn::None
}
