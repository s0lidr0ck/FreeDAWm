//! ABC notation → notes, for the scores YuE2 writes and reads (Generate Song's score as MIDI).
//!
//! The subset YuE2 uses, and a little more: header fields `M:` (meter), `L:` (unit length), `Q:`
//! (tempo, `1/4=88` or `88`), `K:` (key: major and minor keys, the church modes, `none`), voices
//! (`V: Vocal ...` lines and inline `[V:Vocal]`, each voice keeps its own clock), notes with
//! accidentals (`^ _ =`, doubled too, held to the end of the bar), octaves (`' ,`), lengths (`2`,
//! `/2`, `3/2`, `/`), rests (`z x`), whole-bar rests (`Z4`), ties (`-`), chords in brackets
//! (`[CEG]2`), triplets and other tuplets (`(3abc`), broken rhythm (`> <`) and chord symbols
//! (`"Am7"`). Skipped: comments (`%`), lyrics lines (`w:`), grace notes (`{...}`), decorations
//! (`!..!`, `+..+`, `. ~ H L M O P S T u v`), repeats (bars only, nothing played twice).
//!
//! Nothing here panics on hostile text: unknown characters are skipped, lengths are bounded.

use crate::midi::NoteEvent;

/// A chord symbol and where it holds, seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct ChordSymbol {
    pub start: f64,
    pub end: f64,
    pub name: String,
}

/// A parsed score: one part per voice (in order of appearance), the chord symbols, the tempo.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Score {
    /// Beats per minute of the `Q:` beat unit (120 when there is none).
    pub bpm: f64,
    /// The meter's numerator and denominator (4/4 when there is none).
    pub meter: (u32, u32),
    /// (voice id, notes in seconds).
    pub parts: Vec<(String, Vec<NoteEvent>)>,
    pub chords: Vec<ChordSymbol>,
    /// Where the score ends (the longest voice), seconds.
    pub end: f64,
}

/// The most notes a score may hold (hostile input stays bounded).
const MAX_NOTES: usize = 100_000;
/// The longest score, seconds.
const MAX_SECONDS: f64 = 3600.0;
const VELOCITY: u8 = 90;

struct Voice {
    id: String,
    /// Clock in whole notes.
    t: f64,
    notes: Vec<NoteEvent>,
    /// Accidentals in force in this bar: (letter index 0-6, octave) → semitone change.
    bar_acc: Vec<((u8, i32), i32)>,
    /// Indices of the notes a `-` ties into the next ones of the same pitch.
    tied: Vec<usize>,
    /// Tuplet: notes left and the factor.
    tuplet: Option<(u32, f64)>,
    /// Broken rhythm carried to the next note.
    broken: f64,
}

impl Voice {
    fn new(id: &str) -> Self {
        Voice { id: id.to_string(), t: 0.0, notes: Vec::new(), bar_acc: Vec::new(), tied: Vec::new(), tuplet: None, broken: 1.0 }
    }
}

/// The key signature: semitone change per letter (C D E F G A B).
fn key_signature(k: &str) -> [i32; 7] {
    let k = k.trim();
    let mut sig = [0i32; 7];
    let mut chars = k.chars().peekable();
    let Some(root) = chars.next().map(|c| c.to_ascii_uppercase()) else { return sig };
    let Some(letter) = "CDEFGAB".find(root) else { return sig };
    let mut acc = 0;
    match chars.peek() {
        Some('#') => {
            acc = 1;
            chars.next();
        }
        Some('b') => {
            acc = -1;
            chars.next();
        }
        _ => {}
    }
    let rest: String = chars.collect::<String>().trim().to_ascii_lowercase();
    let mode = rest.split_whitespace().next().unwrap_or("");
    // Mode offsets from major in fifths (major 0, minor -3, ...).
    let mode_shift = match mode {
        "" | "maj" | "major" | "ion" | "ionian" => 0,
        "m" | "min" | "minor" | "aeo" | "aeolian" => -3,
        "dor" | "dorian" => -2,
        "phr" | "phrygian" => -4,
        "lyd" | "lydian" => 1,
        "mix" | "mixolydian" => -1,
        "loc" | "locrian" => -5,
        _ if mode.starts_with('m') && !mode.starts_with("mix") => -3,
        _ => 0,
    };
    if root == 'N' {
        return sig;
    }
    // Position of the tonic on the circle of fifths (C = 0).
    const FIFTHS: [i32; 7] = [0, 2, 4, -1, 1, 3, 5]; // C D E F G A B
    let fifths = FIFTHS.get(letter).copied().unwrap_or(0) + 7 * acc + mode_shift;
    // Sharps F C G D A E B, flats B E A D G C F.
    const SHARP_ORDER: [usize; 7] = [3, 0, 4, 1, 5, 2, 6];
    const FLAT_ORDER: [usize; 7] = [6, 2, 5, 1, 4, 0, 3];
    if fifths > 0 {
        for i in 0..fifths.min(14) as usize {
            if let Some(s) = SHARP_ORDER.get(i % 7).and_then(|&l| sig.get_mut(l)) {
                *s += 1;
            }
        }
    } else {
        for i in 0..(-fifths).min(14) as usize {
            if let Some(s) = FLAT_ORDER.get(i % 7).and_then(|&l| sig.get_mut(l)) {
                *s -= 1;
            }
        }
    }
    sig
}

/// A fraction like `1/16`, `3/8`, `C` (4/4), `C|` (2/2).
fn fraction(s: &str) -> Option<(f64, f64)> {
    let s = s.trim();
    match s {
        "C" => return Some((4.0, 4.0)),
        "C|" => return Some((2.0, 2.0)),
        _ => {}
    }
    let (a, b) = s.split_once('/')?;
    let a: f64 = a.trim().parse().ok()?;
    let b: f64 = b.trim().parse().ok()?;
    (a > 0.0 && b > 0.0 && a.is_finite() && b.is_finite()).then_some((a, b))
}

/// Parse `text`.
pub fn parse(text: &str) -> Score {
    let mut unit = 1.0 / 8.0;
    let mut unit_set = false;
    let mut meter = (4u32, 4u32);
    let mut whole_secs = 2.0; // 120 bpm quarters
    let mut bpm = 120.0;
    let mut key = [0i32; 7];
    let mut voices: Vec<Voice> = Vec::new();
    let mut cur: usize = 0;
    let mut chords: Vec<(f64, String)> = Vec::new();
    let mut total = 0usize;
    let mut in_body = false;

    let voice_index = |voices: &mut Vec<Voice>, id: &str| -> usize {
        if let Some(i) = voices.iter().position(|v| v.id == id) {
            return i;
        }
        voices.push(Voice::new(id));
        voices.len() - 1
    };

    for raw in text.lines() {
        let line = raw.split('%').next().unwrap_or("").trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Header / field lines.
        let field = trimmed.as_bytes().get(1).is_some_and(|b| *b == b':') && trimmed.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
        if field {
            let (name, value) = trimmed.split_at(2);
            let value = value.trim();
            match name {
                "M:" => {
                    if let Some((a, b)) = fraction(value) {
                        meter = (a as u32, b as u32);
                        if !unit_set {
                            unit = if a / b < 0.75 { 1.0 / 16.0 } else { 1.0 / 8.0 };
                        }
                    }
                }
                "L:" => {
                    if let Some((a, b)) = fraction(value) {
                        unit = a / b;
                        unit_set = true;
                    }
                }
                "Q:" => {
                    let (beat, n) = match value.split_once('=') {
                        Some((f, n)) => (fraction(f).map_or(0.25, |(a, b)| a / b), n),
                        None => (0.25, value),
                    };
                    if let Ok(q) = n.split_whitespace().next().unwrap_or("").parse::<f64>()
                        && q.is_finite()
                        && (1.0..=1000.0).contains(&q)
                    {
                        bpm = q;
                        whole_secs = 60.0 / q / beat;
                    }
                }
                "K:" => {
                    key = key_signature(value);
                    in_body = true;
                }
                "V:" => {
                    let id = value.split_whitespace().next().unwrap_or("1");
                    cur = voice_index(&mut voices, id);
                    in_body = true;
                }
                _ => {}
            }
            continue;
        }
        if !in_body {
            continue;
        }
        if voices.is_empty() {
            voices.push(Voice::new("1"));
            cur = 0;
        }
        // Body line.
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        // A chord in brackets: its notes start together.
        let mut chord_start: Option<f64> = None;
        let mut chord_len: Option<f64> = None;
        while i < chars.len() && total < MAX_NOTES {
            let c = chars.get(i).copied().unwrap_or(' ');
            let Some(v) = voices.get_mut(cur) else { break };
            match c {
                '"' => {
                    let end = chars.iter().skip(i + 1).position(|&x| x == '"').map_or(chars.len(), |p| i + 1 + p);
                    let name: String = chars.get(i + 1..end).unwrap_or(&[]).iter().collect();
                    let name = name.trim().to_string();
                    // Annotations (^text, _text, <text, >text, @text) are not chords.
                    if !name.is_empty() && !name.starts_with(['^', '_', '<', '>', '@']) {
                        chords.push((v.t, name));
                    }
                    i = end + 1;
                }
                '[' if chars.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic()) && chars.get(i + 2) == Some(&':') => {
                    // Inline field, e.g. [V:Ins] or [K:G].
                    let end = chars.iter().skip(i).position(|&x| x == ']').map_or(chars.len(), |p| i + p);
                    let inner: String = chars.get(i + 1..end).unwrap_or(&[]).iter().collect();
                    let (name, value) = inner.split_at(2.min(inner.len()));
                    match name {
                        "V:" => cur = voice_index(&mut voices, value.split_whitespace().next().unwrap_or("1")),
                        "K:" => key = key_signature(value),
                        "L:" => {
                            if let Some((a, b)) = fraction(value) {
                                unit = a / b;
                            }
                        }
                        _ => {}
                    }
                    i = end + 1;
                }
                '[' => {
                    chord_start = Some(v.t);
                    chord_len = None;
                    i += 1;
                }
                ']' => {
                    // The bracket's own length multiplies its notes' (`[CEG]2`).
                    let (mult, next) = length(&chars, i + 1);
                    if let Some(st) = chord_start.take() {
                        let base = chord_len.take().unwrap_or(0.0);
                        let dur = base * mult;
                        if mult != 1.0 {
                            for n in v.notes.iter_mut().rev().take_while(|n| (n.start - st * whole_secs).abs() < 1e-9) {
                                n.end = n.start + dur * whole_secs;
                            }
                        }
                        v.t = st + dur;
                    }
                    i = next;
                }
                '|' | ':' => {
                    v.bar_acc.clear();
                    i += 1;
                }
                '-' => {
                    // Tie: the last note(s) continue into the next of the same pitch.
                    let at = v.notes.len();
                    if at > 0 {
                        v.tied.push(at - 1);
                    }
                    i += 1;
                }
                '(' if chars.get(i + 1).is_some_and(char::is_ascii_digit) => {
                    let n = chars.get(i + 1).and_then(|c| c.to_digit(10)).unwrap_or(3).max(2);
                    // p notes in the time of q: (2→3, 3→2, 4→3, 6→2, others → n-1).
                    let q = match n {
                        2 => 3,
                        3 | 6 => 2,
                        4 => 3,
                        _ => n - 1,
                    };
                    v.tuplet = Some((n, f64::from(q) / f64::from(n)));
                    i += 2;
                }
                '{' => {
                    i = chars.iter().skip(i).position(|&x| x == '}').map_or(chars.len(), |p| i + p + 1);
                }
                '!' | '+' => {
                    i = chars.iter().skip(i + 1).position(|&x| x == c).map_or(chars.len(), |p| i + p + 2);
                }
                '>' | '<' => {
                    // Broken rhythm: the previous note dotted, the next halved (or the reverse).
                    let (prev, next) = if c == '>' { (1.5, 0.5) } else { (0.5, 1.5) };
                    if let Some(last) = v.notes.last_mut() {
                        let d = last.end - last.start;
                        let nd = d * prev;
                        v.t += (nd - d) / whole_secs;
                        last.end = last.start + nd;
                    }
                    v.broken = next;
                    i += 1;
                }
                'Z' => {
                    let (n, next) = number(&chars, i + 1);
                    let bars = n.unwrap_or(1).clamp(1, 1000);
                    v.t += f64::from(bars) * f64::from(meter.0) / f64::from(meter.1.max(1));
                    i = next;
                }
                'z' | 'x' => {
                    let (mult, next) = length(&chars, i + 1);
                    let d = note_len(v, unit * mult);
                    if chord_start.is_none() {
                        v.t += d;
                    }
                    i = next;
                }
                '^' | '_' | '=' | 'A'..='G' | 'a'..='g' => {
                    // Accidentals.
                    let mut acc: Option<i32> = None;
                    let mut j = i;
                    while let Some(&a) = chars.get(j) {
                        match a {
                            '^' => acc = Some(acc.unwrap_or(0) + 1),
                            '_' => acc = Some(acc.unwrap_or(0) - 1),
                            '=' => acc = Some(0),
                            _ => break,
                        }
                        j += 1;
                    }
                    let Some(&letter) = chars.get(j) else { break };
                    let Some(li) = "CDEFGAB".find(letter.to_ascii_uppercase()) else {
                        i = j + 1;
                        continue;
                    };
                    let mut octave: i32 = if letter.is_ascii_lowercase() { 1 } else { 0 };
                    j += 1;
                    while let Some(&o) = chars.get(j) {
                        match o {
                            '\'' => octave += 1,
                            ',' => octave -= 1,
                            _ => break,
                        }
                        j += 1;
                    }
                    let (mult, next) = length(&chars, j);
                    let li8 = u8::try_from(li).unwrap_or(0);
                    let semis = match acc {
                        Some(a) => {
                            v.bar_acc.retain(|(k, _)| *k != (li8, octave));
                            v.bar_acc.push(((li8, octave), a));
                            a
                        }
                        None => v.bar_acc.iter().find(|(k, _)| *k == (li8, octave)).map_or(key.get(li).copied().unwrap_or(0), |(_, a)| *a),
                    };
                    const STEPS: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
                    // C (upper case) is middle C, MIDI 60.
                    let pitch = 60 + 12 * octave + STEPS.get(li).copied().unwrap_or(0) + semis;
                    let d = note_len(v, unit * mult);
                    let start = chord_start.unwrap_or(v.t);
                    if start * whole_secs > MAX_SECONDS {
                        break;
                    }
                    if let Ok(pitch) = u8::try_from(pitch.clamp(0, 127)) {
                        // A tie continues the earlier note of this pitch instead.
                        let tied = v
                            .tied
                            .iter()
                            .position(|&k| v.notes.get(k).is_some_and(|n| n.pitch == pitch && (n.end - start * whole_secs).abs() < 1e-6));
                        if let Some(p) = tied {
                            let k = v.tied.remove(p);
                            if let Some(n) = v.notes.get_mut(k) {
                                n.end = (start + d) * whole_secs;
                            }
                        } else {
                            v.tied.clear();
                            v.notes.push(NoteEvent {
                                start: start * whole_secs,
                                end: (start + d) * whole_secs,
                                pitch,
                                velocity: VELOCITY,
                                bends: Vec::new(),
                            });
                            total += 1;
                        }
                    }
                    if chord_start.is_some() {
                        chord_len = Some(chord_len.map_or(d, |c: f64| c.max(d)));
                    } else {
                        v.t += d;
                    }
                    i = next;
                }
                _ => i += 1,
            }
        }
    }

    let end_t = voices.iter().map(|v| v.t).fold(0.0f64, f64::max);
    let end = (end_t * whole_secs).min(MAX_SECONDS);
    let mut syms: Vec<ChordSymbol> = Vec::new();
    chords.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (k, (t, name)) in chords.iter().enumerate() {
        let next = chords.get(k + 1).map_or(end_t, |n| n.0);
        if next > *t {
            syms.push(ChordSymbol { start: t * whole_secs, end: next * whole_secs, name: name.clone() });
        }
    }
    let parts = voices.into_iter().filter(|v| !v.notes.is_empty()).map(|v| (v.id, v.notes)).collect();
    Score { bpm, meter, parts, chords: syms, end }
}

/// A note's length in whole notes after a tuplet or broken rhythm in force.
fn note_len(v: &mut Voice, base: f64) -> f64 {
    let mut d = base * v.broken;
    v.broken = 1.0;
    if let Some((left, f)) = v.tuplet {
        d *= f;
        v.tuplet = if left > 1 { Some((left - 1, f)) } else { None };
    }
    d.clamp(0.0, 64.0)
}

/// A number at `i`: (value, the index after it).
fn number(chars: &[char], mut i: usize) -> (Option<u32>, usize) {
    let mut n: Option<u32> = None;
    while let Some(d) = chars.get(i).and_then(|c| c.to_digit(10)) {
        n = Some(n.unwrap_or(0).saturating_mul(10).saturating_add(d).min(100_000));
        i += 1;
    }
    (n, i)
}

/// A length multiplier at `i` (`2`, `/2`, `3/2`, `/`, `//`): (factor, the index after it).
fn length(chars: &[char], i: usize) -> (f64, usize) {
    let (num, mut i) = number(chars, i);
    let mut f = f64::from(num.unwrap_or(1).max(1));
    while chars.get(i) == Some(&'/') {
        let (den, next) = number(chars, i + 1);
        f /= f64::from(den.unwrap_or(2).max(1));
        i = next;
    }
    (f.clamp(1.0 / 64.0, 64.0), i)
}

/// The pitches of a chord symbol (`C`, `F#m7`, `Bb/D`, `Gsus4`, `Cmaj7`, `Edim`, `Aaug`, `D9`),
/// voiced around middle C with the bass an octave below; `None` for what isn't a chord ("N.C.").
pub fn chord_pitches(name: &str) -> Option<Vec<u8>> {
    let name = name.trim();
    let (main, bass) = match name.split_once('/') {
        Some((m, b)) => (m, Some(b)),
        None => (name, None),
    };
    let root = |s: &str| -> Option<(i32, usize)> {
        let mut cs = s.chars();
        let l = cs.next()?.to_ascii_uppercase();
        let base = [0, 2, 4, 5, 7, 9, 11].get("CDEFGAB".find(l)?).copied()?;
        match cs.next() {
            Some('#') => Some((base + 1, 2)),
            Some('b') => Some((base - 1, 2)),
            _ => Some((base, 1)),
        }
    };
    let (r, used) = root(main)?;
    let q = main.get(used..).unwrap_or("").to_ascii_lowercase();
    let mut iv: Vec<i32> = if q.starts_with("maj7") || q.starts_with("ma7") || q.starts_with("m7b5") {
        if q.starts_with("m7b5") { vec![0, 3, 6, 10] } else { vec![0, 4, 7, 11] }
    } else if q.starts_with("m7") || q.starts_with("min7") || q.starts_with("-7") {
        vec![0, 3, 7, 10]
    } else if q.starts_with("dim7") || q.starts_with("o7") {
        vec![0, 3, 6, 9]
    } else if q.starts_with("dim") || q.starts_with('o') {
        vec![0, 3, 6]
    } else if q.starts_with("aug") || q.starts_with('+') {
        vec![0, 4, 8]
    } else if q.starts_with("sus2") {
        vec![0, 2, 7]
    } else if q.starts_with("sus") {
        vec![0, 5, 7]
    } else if q.starts_with("m6") {
        vec![0, 3, 7, 9]
    } else if q.starts_with('m') && !q.starts_with("maj") {
        vec![0, 3, 7]
    } else if q.starts_with("13") || q.starts_with("11") || q.starts_with('9') {
        vec![0, 4, 7, 10, 14]
    } else if q.starts_with('7') {
        vec![0, 4, 7, 10]
    } else if q.starts_with('6') {
        vec![0, 4, 7, 9]
    } else if q.starts_with("maj") || q.is_empty() || q.starts_with('5') || q.starts_with("add") {
        if q.starts_with('5') { vec![0, 7] } else { vec![0, 4, 7] }
    } else {
        return None;
    };
    if q.contains("add9") {
        iv.push(14);
    }
    // Close position from the root at or above G3 (55).
    let mut root_pitch = 48 + r.rem_euclid(12);
    if root_pitch < 55 {
        root_pitch += 12;
    }
    let mut out: Vec<u8> = iv.iter().filter_map(|i| u8::try_from(root_pitch + i).ok()).collect();
    let b = bass.and_then(root).map_or(r, |(b, _)| b);
    if let Ok(bp) = u8::try_from(36 + b.rem_euclid(12)) {
        out.insert(0, bp);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCORE: &str = r#"X:1
T:
M:4/4
L:1/16
Q:1/4=88
V: Vocal clef=treble name="Vocal Melody" snm="Vocal"
V: Ins clef=treble name="Ins Melody" snm="Inst."
K:C
% verse
V: Vocal
"C"E2G2A2G2E2D2C4|"G"D2E2G2E2D2C2D4|
V: Ins
Z2|
% chorus
V: Vocal
"F"F2A2c2B2A2G2E4|
V: Ins
C,8G,8|
"#;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn a_yue2_score_becomes_voices_chords_and_seconds() {
        let s = parse(SCORE);
        assert!(approx(s.bpm, 88.0));
        assert_eq!(s.meter, (4, 4));
        let names: Vec<&str> = s.parts.iter().map(|p| p.0.as_str()).collect();
        assert_eq!(names, ["Vocal", "Ins"]);
        let vocal = &s.parts[0].1;
        assert_eq!(vocal.len(), 21);
        // E4 an eighth at 88 bpm: 60/88/2 s.
        let eighth = 60.0 / 88.0 / 2.0;
        assert_eq!(vocal[0].pitch, 64);
        assert!(approx(vocal[0].end, eighth));
        // The chorus follows the two verse bars; c is C5.
        let bar = 4.0 * 60.0 / 88.0;
        let chorus: Vec<&NoteEvent> = vocal.iter().filter(|n| n.start >= 2.0 * bar - 1e-9).collect();
        assert_eq!(chorus.iter().map(|n| n.pitch).collect::<Vec<_>>(), [65, 69, 72, 71, 69, 67, 64]);
        // Ins rests two bars (Z2), then C3 and G3, a half note each.
        let ins = &s.parts[1].1;
        assert_eq!(ins.iter().map(|n| n.pitch).collect::<Vec<_>>(), [48, 55]);
        assert!(approx(ins[0].start, 2.0 * bar) && approx(ins[0].end - ins[0].start, bar / 2.0));
        // Chords: C, G, F, each to the next.
        assert_eq!(s.chords.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["C", "G", "F"]);
        assert!(approx(s.chords[0].end, bar) && approx(s.chords[2].start, 2.0 * bar) && approx(s.chords[2].end, 3.0 * bar));
        assert!(approx(s.end, 3.0 * bar));
    }

    #[test]
    fn keys_accidentals_ties_tuplets_and_chords() {
        // G major: F is F#; =F naturalises to the bar line; ^^ is a double sharp.
        let s = parse("X:1\nL:1/4\nK:G\nF =F F|F ^^C c'|");
        let p: Vec<u8> = s.parts[0].1.iter().map(|n| n.pitch).collect();
        assert_eq!(p, [66, 65, 65, 66, 62, 84]);
        // Bb major: B and E flat; minor keys: Am has none, Dm has Bb.
        assert_eq!(key_signature("Bb"), [0, 0, -1, 0, 0, 0, -1]);
        assert_eq!(key_signature("Am"), [0; 7]);
        assert_eq!(key_signature("Dm"), [0, 0, 0, 0, 0, 0, -1]);
        assert_eq!(key_signature("F#"), [1, 1, 1, 1, 1, 1, 0]);
        assert_eq!(key_signature("D dorian"), [0; 7]);
        // A tie joins two notes; a triplet is three in the time of two.
        let s = parse("L:1/8\nK:C\nC2-C2 (3DEF G");
        let n = &s.parts[0].1;
        assert_eq!(n.len(), 5);
        assert!(approx(n[0].end, 4.0 * 0.25), "C held for a half note at 120 bpm: {}", n[0].end);
        assert!(approx(n[1].end - n[1].start, 0.25 * 2.0 / 3.0));
        assert!(approx(n[4].start, 1.0 + 0.25 * 2.0));
        // A bracket chord: three notes at once, then the clock moves on by its length.
        let s = parse("L:1/4\nK:C\n[CEG]2 A");
        let n = &s.parts[0].1;
        assert_eq!(n.iter().map(|x| x.pitch).collect::<Vec<_>>(), [60, 64, 67, 69]);
        assert!(n[..3].iter().all(|x| approx(x.start, 0.0) && approx(x.end, 1.0)));
        assert!(approx(n[3].start, 1.0));
        // Broken rhythm: A>B is a dotted A and a short B.
        let s = parse("L:1/8\nK:C\nA>B c");
        let n = &s.parts[0].1;
        assert!(approx(n[0].end, 0.375) && approx(n[1].start, 0.375) && approx(n[1].end, 0.5) && approx(n[2].start, 0.5));
    }

    #[test]
    fn chord_symbols_have_pitches() {
        assert_eq!(chord_pitches("C"), Some(vec![36, 60, 64, 67]));
        assert_eq!(chord_pitches("Am7"), Some(vec![45, 57, 60, 64, 67]));
        assert_eq!(chord_pitches("Bb/D"), Some(vec![38, 58, 62, 65]));
        assert_eq!(chord_pitches("F#dim"), Some(vec![42, 66, 69, 72]));
        assert_eq!(chord_pitches("Gsus4"), Some(vec![43, 55, 60, 62]));
        assert_eq!(chord_pitches("N.C."), None);
        assert_eq!(chord_pitches(""), None);
    }

    #[test]
    fn hostile_text_does_not_panic() {
        for t in [
            "",
            "K:",
            "V:",
            "K:C\n[",
            "K:C\n\"",
            "K:C\n(9",
            "K:C\n^^^^^^",
            "K:C\nZ99999999999",
            "L:0/0\nQ:1/4=0\nK:C\nC",
            "K:C\n]]]2",
            &"K:C\nC64".repeat(3),
        ] {
            let _ = parse(t);
        }
        let long = format!("K:C\n{}", "C".repeat(200_000));
        assert!(parse(&long).parts[0].1.len() <= MAX_NOTES);
    }
}
