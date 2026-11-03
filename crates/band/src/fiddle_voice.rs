//! Port of `FiddleVoice.h` (namespace `bowed`).
//!
//! Fiddle decisions for the Indiginus The Fiddle target: pure functions, no engine state.

use crate::library::FiddleStyle;

use crate::meter::Meter;

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

/// `std::min` for floats: `b < a ? b : a`.
fn minf(a: f32, b: f32) -> f32 {
    if b < a { b } else { a }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Splits {
    pub vibrato: i32,
    pub portamento: i32,
    pub slur: i32,
}

impl Default for Splits {
    fn default() -> Self {
        Self { vibrato: 50, portamento: 16, slur: 61 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    Slide,
    Plain,
    Sing,
    Bow,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NoteCtx {
    pub len_beats: f32,
    pub connected: bool,
    pub accented: bool,
    pub downbeat: bool,
    pub strong_beat: bool,
    pub slide_in: bool,
    pub lead_in: bool,
    pub phrase_start: bool,
    pub energy: i32,
}

pub fn band_for(c: &NoteCtx) -> Band {
    if c.slide_in && c.connected {
        return Band::Slide;
    }
    if c.lead_in && c.connected && c.len_beats < 1.0 {
        return Band::Sing;
    }
    if c.len_beats >= 1.0 || (c.accented && c.downbeat) {
        return Band::Bow;
    }
    Band::Plain
}

pub fn band_velocity(b: Band, s: &Splits, r: f32, strong_beat: bool, weight: f32) -> i32 {
    let jitter = ((r - 0.5) * 8.0).round() as i32; // +-4
    let lift = if strong_beat { 6 } else { 0 };
    let (lo, hi) = match b {
        Band::Slide => (6, s.portamento.wrapping_sub(4)),
        Band::Plain => (s.portamento.wrapping_add(4), s.vibrato.wrapping_sub(5)),
        Band::Sing => (s.vibrato.wrapping_add(4), s.slur.wrapping_sub(1)),
        Band::Bow => (s.slur.wrapping_add(5), 118),
    };
    let centre = if b == Band::Bow {
        lo.wrapping_add((hi.wrapping_sub(lo) as f32 * (0.15 + 0.85 * weight.clamp(0.0, 1.0))).round() as i32)
    } else {
        lo.wrapping_add(hi) / 2
    };
    clampi(centre.wrapping_add(jitter).wrapping_add(lift), lo, hi)
}

pub fn fold_to_line(note: i32, prev: i32, lo: i32, hi: i32) -> i32 {
    let anchor = if prev < 0 { 74 } else { prev };
    let mut best = -1;
    let mut n = note % 12;
    while n <= 127 {
        if n >= lo && n <= hi && (best < 0 || (i64::from(n) - i64::from(anchor)).abs() < (i64::from(best) - i64::from(anchor)).abs()) {
            best = n;
        }
        n += 12;
    }
    if best < 0 { clampi(note, lo, hi) } else { best }
}

pub fn double_stop_partner<F: Fn(i32) -> bool>(melody: i32, is_chord_tone: F, lo: i32, hi: i32) -> i32 {
    let m = i64::from(melody);
    for open in [76, 69] {
        // E5, A4 ringing over the melody
        let gap = i64::from(open) - m;
        if open > melody && (3..=9).contains(&gap) && open <= hi && is_chord_tone(open) {
            return open;
        }
    }
    for d in 3..=9 {
        let below = melody.wrapping_sub(d);
        if m - i64::from(d) >= i64::from(lo) && is_chord_tone(below) {
            return below;
        }
    }
    -1
}

/// The fiddler's vocabulary (spec vocabulary 1): licks as scale degrees relative to the target chord's root, timed in
/// beats relative to the target's downbeat (0 = the downbeat itself, negative = before it).
/// C++ returns `(count, *out)`; here the lick's notes as a slice (empty: no such lick).
pub mod licks {
    use super::Meter;

    /// semis: chromatic, after the degree
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct LickNote {
        pub degree_offset: i32,
        pub start_beats: f32,
        pub len_beats: f32,
        pub double_stop: bool,
        pub accent: bool,
        pub slide_in: bool,
        pub semis: i32,
    }

    const fn ln(degree_offset: i32, start_beats: f32, len_beats: f32, double_stop: bool, accent: bool, slide_in: bool) -> LickNote {
        LickNote { degree_offset, start_beats, len_beats, double_stop, accent, slide_in, semis: 0 }
    }
    const fn lns(degree_offset: i32, start_beats: f32, len_beats: f32, double_stop: bool, accent: bool, slide_in: bool, semis: i32) -> LickNote {
        LickNote { degree_offset, start_beats, len_beats, double_stop, accent, slide_in, semis }
    }

    // Potatoes: quarter-note double stops on the V (its root + 5th), the last one accented, on the downbeat.
    static POTATOES: [LickNote; 4] = [
        ln(4, -3.0, 1.0, true, false, false),
        ln(4, -2.0, 1.0, true, false, false),
        ln(4, -1.0, 1.0, true, false, false),
        ln(4, 0.0, 1.0, true, true, false),
    ];
    // Walk-up: scale steps in 8ths from the 5th below the root up to it, sliding into the last.
    static WALK_UP: [LickNote; 5] = [
        ln(-4, -2.0, 0.5, false, true, false),
        ln(-3, -1.5, 0.5, false, false, false),
        ln(-2, -1.0, 0.5, false, false, false),
        ln(-1, -0.5, 0.5, false, false, false),
        ln(0, 0.0, 1.0, false, true, true),
    ];
    // Drop-in: a long double stop over the last two beats (the step above the root, the V's 3rd under it), slurring
    // down a step into the root.
    static DROP_IN: [LickNote; 2] = [ln(1, -2.0, 2.0, true, true, false), ln(0, 0.0, 1.0, false, false, false)]; // a slur: connected, the normal band

    /// Kick-offs (1.1), chosen by Take: 0 potatoes, 1 walk-up, 2 drop-in. Returns the notes (empty: no such lick).
    pub fn kickoff(which: i32) -> &'static [LickNote] {
        match which {
            0 => &POTATOES,
            1 => &WALK_UP,
            2 => &DROP_IN,
            _ => &[],
        }
    }

    static THIRDS: [LickNote; 4] = [
        ln(5, -2.0, 0.5, false, false, false),
        ln(3, -1.5, 0.5, false, false, false),
        ln(4, -1.0, 0.5, false, false, false),
        ln(2, -0.5, 0.5, false, false, false),
    ];
    static RUN: [LickNote; 4] = [
        ln(-3, -1.0, 0.25, false, false, false),
        ln(-2, -0.75, 0.25, false, false, false),
        ln(-1, -0.5, 0.25, false, false, false),
        ln(0, -0.25, 0.25, false, false, false),
    ];
    static ARP_DOWN: [LickNote; 3] =
        [ln(7, -1.5, 0.5, false, true, false), ln(4, -1.0, 0.5, false, false, false), ln(2, -0.5, 0.5, false, false, false)];
    static PULL: [LickNote; 2] = [ln(1, -2.0, 1.5, true, true, false), ln(0, -0.5, 0.5, false, false, false)]; // a slur down into the root
    static CHROMA: [LickNote; 4] = [
        lns(0, -1.5, 0.25, false, false, false, -3),
        lns(0, -1.25, 0.25, false, false, false, -2),
        lns(0, -1.0, 0.25, false, false, false, -1),
        ln(0, -0.75, 0.75, false, true, false),
    ];
    static SAW: [LickNote; 6] = [
        ln(4, -2.0, 0.5, false, true, false),
        ln(4, -1.5, 0.25, false, false, false),
        ln(4, -1.25, 0.25, false, false, false),
        ln(4, -1.0, 0.5, false, true, false),
        ln(4, -0.5, 0.25, false, false, false),
        ln(4, -0.25, 0.25, false, false, false),
    ];

    /// Fills between phrases (1.2), chosen by Take: inside the last two beats of a phrase-end bar (beats count from the next
    /// downbeat, which they never reach), degrees from the current chord's root. Each has its own attack pattern.
    /// 0 descending 3rds, 1 ascending scale run, 2 chord-tone arpeggio down, 3 double-stop pull into the root,
    /// 4 chromatic approach from below, 5 repeated-note sawing. Returns the notes (empty: no such fill).
    pub fn fill(which: i32) -> &'static [LickNote] {
        match which {
            0 => &THIRDS,
            1 => &RUN,
            2 => &ARP_DOWN,
            3 => &PULL,
            4 => &CHROMA,
            5 => &SAW,
            _ => &[],
        }
    }

    static LINE: [LickNote; 5] = [
        ln(-1, -4.0, 1.0, true, false, false),
        ln(-2, -3.0, 1.0, true, false, false),
        ln(-3, -2.0, 1.0, true, false, false),
        ln(-4, -1.0, 1.0, true, false, false),
        ln(0, 0.0, 1.0, false, true, false),
    ];

    /// Turnarounds (1.4): a descending line of double stops in quarters through the V (3rd/root, 7th/5th ... the 6ths of the
    /// spec, inverted), degrees from the new section's root (the top note; its partner a 3rd below in the scale), the last note the root on the downbeat. One line.
    pub fn turnaround(which: i32) -> &'static [LickNote] {
        if which != 0 {
            return &[];
        }
        &LINE
    }

    /// Lick sets per meter (time signatures, Task 16). A set declares the meter it is written for and its length in that
    /// meter's bars: the bar before the target's downbeat (the lead-in) plus the bars its landing reaches into. Every note
    /// lies inside those bars; the 12-step sets fill them exactly (the landing held to the end of its bar). The 4/4 sets are
    /// the tables above, unchanged (2 bars: a kick-off's or turnaround's landing note sits in the second; a fill 1 bar).
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct LickSet {
        pub meter: Meter,
        pub bars: i32,
        pub notes: &'static [LickNote],
        pub count: i32,
    }

    fn set(meter: Meter, bars: i32, notes: &'static [LickNote]) -> LickSet {
        LickSet { meter, bars, notes, count: notes.len() as i32 }
    }

    // 3/4, the country waltz (2 bars = 6.0 ppq): 0 waltz potatoes (V double stops on 1-2-3), 1 walk-up (a quarter, then
    // eighths, from the 5th below), 2 drop-in (a half-note double stop on 2-3 slurring into the root); each lands held a bar.
    static K_W0: [LickNote; 4] = [
        ln(4, -3.0, 1.0, true, false, false),
        ln(4, -2.0, 1.0, true, false, false),
        ln(4, -1.0, 1.0, true, false, false),
        ln(4, 0.0, 3.0, true, true, false),
    ];
    static K_W1: [LickNote; 5] = [
        ln(-4, -3.0, 1.0, false, true, false),
        ln(-3, -2.0, 0.5, false, false, false),
        ln(-2, -1.5, 0.5, false, false, false),
        ln(-1, -1.0, 1.0, false, false, false),
        ln(0, 0.0, 3.0, false, true, true),
    ];
    static K_W2: [LickNote; 2] = [ln(1, -2.0, 2.0, true, true, false), ln(0, 0.0, 3.0, false, false, false)];
    // 6/8, the jig (2 bars = 6.0 ppq): 0 potatoes in dotted quarters, 1 walk-up in eighths (accents on the dotted beats),
    // 2 drop-in (a dotted-quarter double stop on the second dotted beat); each lands held a bar.
    static K_J0: [LickNote; 3] = [ln(4, -3.0, 1.5, true, false, false), ln(4, -1.5, 1.5, true, false, false), ln(4, 0.0, 3.0, true, true, false)];
    static K_J1: [LickNote; 7] = [
        ln(-4, -3.0, 0.5, false, true, false),
        ln(-3, -2.5, 0.5, false, false, false),
        ln(-2, -2.0, 0.5, false, false, false),
        ln(-3, -1.5, 0.5, false, true, false),
        ln(-2, -1.0, 0.5, false, false, false),
        ln(-1, -0.5, 0.5, false, false, false),
        ln(0, 0.0, 3.0, false, true, true),
    ];
    static K_J2: [LickNote; 2] = [ln(1, -1.5, 1.5, true, true, false), ln(0, 0.0, 3.0, false, false, false)];

    pub fn kickoff_set(m: Meter, which: i32) -> LickSet {
        if m == Meter::M44 {
            return set(m, 2, kickoff(which));
        }
        let waltz = m == Meter::M34;
        let n: &'static [LickNote] = match which {
            0 => {
                if waltz {
                    &K_W0
                } else {
                    &K_J0
                }
            }
            1 => {
                if waltz {
                    &K_W1
                } else {
                    &K_J1
                }
            }
            2 => {
                if waltz {
                    &K_W2
                } else {
                    &K_J2
                }
            }
            _ => &[],
        };
        set(m, 2, n)
    }

    // 3/4: on beat 3, 0 a step down in eighths into the root, 1 the 5th and 3rd. 6/8: on the last dotted beat, 0 3rds
    // down in eighths, 1 sawing the 5th. 1 bar each, ending at the downbeat.
    static F_W0: [LickNote; 2] = [ln(2, -1.0, 0.5, false, true, false), ln(1, -0.5, 0.5, false, false, false)];
    static F_W1: [LickNote; 2] = [ln(4, -1.0, 0.5, false, true, false), ln(2, -0.5, 0.5, false, false, false)];
    static F_J0: [LickNote; 3] = [ln(4, -1.5, 0.5, false, true, false), ln(2, -1.0, 0.5, false, false, false), ln(1, -0.5, 0.5, false, false, false)];
    static F_J1: [LickNote; 3] = [ln(4, -1.5, 0.5, false, true, false), ln(4, -1.0, 0.5, false, false, false), ln(4, -0.5, 0.5, false, false, false)];

    pub fn fill_set(m: Meter, which: i32) -> LickSet {
        if m == Meter::M44 {
            return set(m, 1, fill(which));
        }
        let waltz = m == Meter::M34;
        let n: &'static [LickNote] = if which == 0 {
            if waltz { &F_W0 } else { &F_J0 }
        } else if which == 1 {
            if waltz { &F_W1 } else { &F_J1 }
        } else {
            &[]
        };
        set(m, 1, n)
    }

    pub fn fill_count(m: Meter) -> i32 {
        if m == Meter::M44 { 6 } else { 2 }
    }

    // 6/8, the jig turnaround (2 bars = 6.0 ppq): the descending 6ths (inverted, as 4/4's) in dotted quarters through
    // the bar before, the new root held a bar. 3/4 has none.
    static T_J: [LickNote; 3] = [ln(-1, -3.0, 1.5, true, false, false), ln(-2, -1.5, 1.5, true, false, false), ln(0, 0.0, 3.0, false, true, false)];

    pub fn turnaround_set(m: Meter, which: i32) -> LickSet {
        if m == Meter::M44 {
            return set(m, 2, turnaround(which));
        }
        let n: &'static [LickNote] = if m == Meter::M68 && which == 0 { &T_J } else { &[] };
        set(m, 2, n)
    }
}

/// newBow: 6/8, a bow change before it (no slur)
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShuffleNote {
    pub start_beats: f32,
    pub len_beats: f32,
    pub accent: bool,
    pub partner: bool,
    pub new_bow: bool,
}

/// A written count never claims more notes than `out` holds (C++ trusted `maxOut`).
fn clamp_count(n: i32, len: usize) -> i32 {
    n.min(i32::try_from(len).unwrap_or(i32::MAX))
}

fn put(out: &mut [ShuffleNote], i: i32, v: ShuffleNote) {
    if let Some(slot) = usize::try_from(i).ok().and_then(|i| out.get_mut(i)) {
        *slot = v;
    }
}

fn sn(start_beats: f32, len_beats: f32, accent: bool, partner: bool) -> ShuffleNote {
    ShuffleNote { start_beats, len_beats, accent, partner, new_bow: false }
}

/// Splits a note into swung eighth pairs. start_eighth_in_bar (0..7) places the note in the bar so the 3+3+2 accents
/// (eighths 0, 3, 6) hold wherever it starts. Returns 0 for len_beats < 0.5: the caller then plays the note unsplit.
/// (C++ default `style = 0`.)
pub fn shuffle_split(len_beats: f32, swing: f32, start_eighth_in_bar: i32, out: &mut [ShuffleNote], max_out: i32, style: i32) -> i32 {
    if len_beats < 0.5 - 1.0e-4 || style == 2 {
        return 0; // longbow: the caller holds one bow per beat
    }
    if style == 1 {
        // georgia: per beat an eighth (accent) then two slurred sixteenths, the first rocking to the partner
        let mut g = 0;
        let mut b = 0.0f32;
        while b + 0.5 <= len_beats + 1.0e-4 && g + 2 < max_out {
            put(out, g, sn(b, 0.5, true, false));
            g += 1;
            if b + 1.0 <= len_beats + 1.0e-4 {
                put(out, g, sn(b + 0.5, 0.25, false, true));
                g += 1;
                put(out, g, sn(b + 0.75, minf(0.25, len_beats - (b + 0.75)), false, false));
                g += 1;
            }
            b += 1.0;
        }
        return clamp_count(g, out.len());
    }
    let mut n = 0;
    let off = 0.5 + 0.25 * swing.clamp(0.0, 1.0) * 0.9; // 0.5 straight ... ~0.73 swung
    let accent_at = |eighth: i32| {
        let e = start_eighth_in_bar.wrapping_add(eighth) % 8;
        e == 0 || e == 3 || e == 6
    };
    let mut b = 0.0f32;
    while b + 0.5 <= len_beats + 1.0e-4 && n + 1 < max_out {
        let eighth = (b * 2.0).round() as i32;
        put(out, n, sn(b, minf(off, len_beats - b), accent_at(eighth), false));
        n += 1;
        if b + 1.0 <= len_beats + 1.0e-4 {
            put(out, n, sn(b + off, minf(1.0 - off, len_beats - (b + off)), accent_at(eighth + 1), true));
            n += 1;
        }
        b += 1.0;
    }
    clamp_count(n, out.len())
}

/// 6/8 (Task 17, spec 3.2 / 6): one bow per eighth, slurred 2 + 1 per dotted beat: the dotted beat's 1st eighth (accent)
/// slurs into the 2nd, the 3rd takes its own bow. start_eighth_in_bar (0..5) places the note in the bar. Straight eighths:
/// the lilt is the step's own (swingDelay). With a double-stop partner the slurred 2nd rocks across to it (one bow,
/// two strings). Returns 0 for len_beats < 1 (the caller plays the note unsplit).
pub fn shuffle_split68(len_beats: f32, start_eighth_in_bar: i32, out: &mut [ShuffleNote], max_out: i32) -> i32 {
    if len_beats < 1.0 - 1.0e-4 {
        return 0;
    }
    let mut n = 0;
    let mut b = 0.0f32;
    while b + 0.5 <= len_beats + 1.0e-4 && n < max_out {
        let e = start_eighth_in_bar.wrapping_add((b * 2.0).round() as i32) % 3;
        put(out, n, ShuffleNote { start_beats: b, len_beats: minf(0.5, len_beats - b), accent: e == 0, partner: e == 1, new_bow: e != 1 }); // the slurred 2nd rocks to the partner
        n += 1;
        b += 0.5;
    }
    clamp_count(n, out.len())
}

/// half_bar_always: half-bar points at any tempo (the Embertone); else each bar below 90 BPM (the fiddle).
/// (C++ default `half_bar_always = false`.)
pub fn rebow_points(
    start_ppq: f64,
    len_ppq: f64,
    bar_ppq: f64,
    bpm: f64,
    bow_seconds: f32,
    out: &mut [f64],
    max_out: i32,
    half_bar_always: bool,
) -> i32 {
    let bow_ppq = f64::from(bow_seconds) * bpm / 60.0;
    if len_ppq <= bow_ppq + 1.0e-6 {
        return 0;
    }
    let grid = if bpm < 90.0 && !half_bar_always { bar_ppq } else { bar_ppq * 0.5 };
    let mut n = 0;
    let mut p = ((start_ppq + 1.0e-6) / grid).ceil() * grid;
    while p < start_ppq + len_ppq - 1.0e-6 && n < max_out {
        if p > start_ppq + 1.0e-6 {
            if let Some(slot) = usize::try_from(n).ok().and_then(|i| out.get_mut(i)) {
                *slot = p;
            }
            n += 1;
        }
        let next = p + grid;
        if next <= p {
            break; // a zero / NaN / vanishing grid never advances (an endless loop in C++)
        }
        p = next;
    }
    clamp_count(n, out.len())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ornament {
    None,
    Scoop,
    GraceUp,
    GraceDown,
    GraceSlideUp,
    GraceSlideDown,
    TrillUp,
    TrillDown,
    Staccato,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OrnamentCtx {
    pub phrase_start: bool,
    pub downbeat: bool,
    pub accented: bool,
    pub in_fill: bool,
    pub phrase_end: bool,
    pub first_bar_of_section: bool,
    pub line_falling: bool,
    pub next_lower: bool,
    pub staccato_flag: bool,
    pub leap_semis: i32,
    pub energy: i32,
    pub last_had_ornament: bool,
    pub beat_has_ornament: bool,
}

pub fn choose_ornament(c: &OrnamentCtx, st: &FiddleStyle, r: f32) -> Ornament {
    // Staccato (flag 1024 / chop) articulates the written note, it adds nothing: exempt from the spacing rules (spec 7a.H).
    if c.staccato_flag {
        return Ornament::Staccato;
    }
    if c.last_had_ornament || c.beat_has_ornament {
        return Ornament::None;
    }
    let scale = if c.energy <= 1 {
        0.5f32
    } else if c.energy >= 4 {
        1.3
    } else {
        1.0
    };
    if c.first_bar_of_section {
        return if c.downbeat && c.accented && r < st.scoops * scale { Ornament::Scoop } else { Ornament::None };
    }
    if (c.phrase_start || c.downbeat) && c.accented && r < st.scoops * scale {
        return Ornament::Scoop;
    }
    if c.phrase_end && c.energy >= 2 && r < st.trills * scale {
        return if c.next_lower { Ornament::TrillDown } else { Ornament::TrillUp };
    }
    if c.leap_semis >= 3 && r < st.grace_slides * scale {
        return Ornament::GraceSlideUp;
    }
    if c.leap_semis <= -3 && r < st.grace_slides * scale {
        return Ornament::GraceSlideDown;
    }
    if c.in_fill && c.accented && c.energy >= 2 && r < st.graces * scale {
        return if c.line_falling { Ornament::GraceDown } else { Ornament::GraceUp };
    }
    Ornament::None
}

// Bowed-solo layer (Embertone Joshua Bell Violin): vibrato and portamento decisions, pure.
pub fn wants_natural_vibrato(len_beats: f32, accented: bool, energy: i32) -> bool {
    len_beats >= 1.0 - 1.0e-4 || (accented && len_beats >= 0.5 - 1.0e-4) || (energy >= 3 && len_beats >= 0.5 - 1.0e-4)
}

/// Natural vibrato CC1 72..104, scaled by the player's depth and deeper for longer or accented notes; otherwise 0..20.
pub fn vibrato_cc(natural_vib: bool, depth01: f32, len_beats: f32, accented: bool, r01: f32) -> i32 {
    let r = r01.clamp(0.0, 1.0);
    if !natural_vib {
        return (20.0 * r).round() as i32;
    }
    let t = (0.2 + 0.6 * (len_beats / 4.0).clamp(0.0, 1.0) + (if accented { 0.2 } else { 0.0 }) + (r - 0.5) * 0.1).clamp(0.0, 1.0);
    72 + (32.0 * t * depth01.clamp(0.0, 1.0)).round() as i32
}

/// A slide into a note: a leap of 3+ semitones into a note of a beat or more, in a line that averages half a beat or more, never from silence.
pub fn portamento_here(interval: i32, len_beats: f32, line_avg_beats: f32, from_silence: bool, energy: i32, chance: f32, r01: f32) -> bool {
    if from_silence || interval.wrapping_abs() < 3 || len_beats < 1.0 - 1.0e-4 || line_avg_beats < 0.5 - 1.0e-4 {
        return false;
    }
    r01 < chance * (if energy >= 3 { 0.5 } else { 1.0 })
}
