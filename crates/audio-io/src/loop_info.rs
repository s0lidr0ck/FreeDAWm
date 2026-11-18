//! What a loop file says about itself: its tempo and its key, from a WAV's `acid` chunk (the
//! loop-library standard: tempo, root note, one-shot flag) or, failing that, from its name
//! ("Drums 120bpm.wav", "Keys_95_BPM_Am.wav"). Nothing is guessed from the audio here.

use crate::pcm::{le_u16, le_u32, tag};
use crate::wav::is_wav;

/// A loop's tempo and key as its file tells them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LoopInfo {
    /// Beats per minute.
    pub bpm: Option<f64>,
    /// The root note as a pitch class (0 = C … 11 = B).
    pub root: Option<u8>,
    /// Whether that key is minor (`None`: not said).
    pub minor: Option<bool>,
}

/// The `acid` chunk's flags: a one-shot (not a loop), and a root note given.
const ONE_SHOT: u32 = 0x01;
const ROOT_SET: u32 = 0x02;

/// What `bytes` (a whole file or its first part) named `name` says: the `acid` chunk first, then
/// the name for anything it leaves out. A one-shot's tempo is not taken.
pub fn loop_info(bytes: &[u8], name: &str) -> LoopInfo {
    let mut info = acid(bytes).unwrap_or_default();
    let named = from_name(name);
    if info.bpm.is_none() {
        info.bpm = named.bpm;
    }
    if info.root.is_none() {
        info.root = named.root;
        info.minor = named.minor;
    }
    info
}

/// The `acid` chunk of a WAV, when it has one.
fn acid(b: &[u8]) -> Option<LoopInfo> {
    if !is_wav(b) {
        return None;
    }
    let mut at = 12usize;
    for _ in 0..256 {
        let (id, size) = (tag(b, at)?, le_u32(b, at.checked_add(4)?)?);
        let body = at.checked_add(8)?;
        if &id == b"acid" {
            let c = b.get(body..body.checked_add(24)?)?;
            let flags = le_u32(c, 0)?;
            let root = le_u16(c, 4)?;
            let tempo = f64::from(f32::from_le_bytes(c.get(20..24)?.try_into().ok()?));
            let bpm = (flags & ONE_SHOT == 0 && tempo.is_finite() && (20.0..=400.0).contains(&tempo)).then_some(tempo);
            let root = (flags & ROOT_SET != 0 && root <= 127).then_some((root % 12) as u8);
            return Some(LoopInfo { bpm, root, minor: None });
        }
        // The audio is usually last; a large data chunk ends the walk only when the bytes stop.
        let size = usize::try_from(size).ok()?;
        at = body.checked_add(size)?.checked_add(size % 2)?;
    }
    None
}

/// The tempo ("120bpm", "95 BPM", "128_bpm") and key ("Am", "F#min", "Ebmaj", "C") written in a
/// file name. A key is taken only as a word of its own beside a tempo, as loop libraries name
/// them, so "A Day.wav" says nothing.
pub fn from_name(name: &str) -> LoopInfo {
    let stem = std::path::Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let words: Vec<&str> = stem.split([' ', '_', '-', '.', '(', ')', '[', ']']).filter(|w| !w.is_empty()).collect();
    let mut bpm = None;
    for (i, w) in words.iter().enumerate() {
        let lower = w.to_ascii_lowercase();
        let number = if let Some(n) = lower.strip_suffix("bpm") {
            Some(n)
        } else if words.get(i + 1).is_some_and(|n| n.eq_ignore_ascii_case("bpm")) {
            Some(lower.as_str())
        } else {
            None
        };
        if let Some(v) = number.and_then(|n| n.parse::<f64>().ok()).filter(|v| (20.0..=400.0).contains(v)) {
            bpm = Some(v);
            break;
        }
    }
    let mut info = LoopInfo { bpm, ..LoopInfo::default() };
    if bpm.is_some()
        && let Some((root, minor)) = words.iter().find_map(|w| key_word(w))
    {
        info.root = Some(root);
        info.minor = minor;
    }
    info
}

/// A key written as a word: a note letter, an optional sharp or flat, then nothing, "m", "min",
/// "minor", "maj" or "major".
fn key_word(w: &str) -> Option<(u8, Option<bool>)> {
    let mut chars = w.chars();
    let base: i32 = match chars.next()? {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest: &str = chars.as_str();
    let (shift, rest) = match rest.chars().next() {
        Some('#') => (1, rest.get(1..)?),
        Some('b') if rest.len() == 1 || !rest.starts_with("bpm") => (-1, rest.get(1..)?),
        _ => (0, rest),
    };
    let minor = match rest {
        "" => None,
        "m" | "min" | "minor" | "Min" | "Minor" => Some(true),
        "maj" | "major" | "Maj" | "Major" | "M" => Some(false),
        _ => return None,
    };
    Some(((base + shift).rem_euclid(12) as u8, minor))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav_with_acid(flags: u32, root: u16, tempo: f32) -> Vec<u8> {
        let mut acid = Vec::new();
        acid.extend_from_slice(&flags.to_le_bytes());
        acid.extend_from_slice(&root.to_le_bytes());
        acid.extend_from_slice(&[0; 2]);
        acid.extend_from_slice(&[0; 4]);
        acid.extend_from_slice(&8u32.to_le_bytes());
        acid.extend_from_slice(&4u16.to_le_bytes());
        acid.extend_from_slice(&4u16.to_le_bytes());
        acid.extend_from_slice(&tempo.to_le_bytes());
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&[1, 0, 1, 0, 0x80, 0xbb, 0, 0, 0, 0x77, 1, 0, 2, 0, 16, 0]);
        b.extend_from_slice(b"acid");
        b.extend_from_slice(&24u32.to_le_bytes());
        b.extend_from_slice(&acid);
        b.extend_from_slice(b"data");
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(&[0; 4]);
        b
    }

    #[test]
    fn the_acid_chunk_gives_tempo_and_root_and_the_name_fills_in() {
        let b = wav_with_acid(ROOT_SET, 57, 92.5);
        assert_eq!(loop_info(&b, "x.wav"), LoopInfo { bpm: Some(92.5), root: Some(9), minor: None });
        // A one-shot has no tempo.
        let b = wav_with_acid(ONE_SHOT, 0, 120.0);
        assert_eq!(loop_info(&b, "hit.wav").bpm, None);
        // No chunk: the name.
        assert_eq!(loop_info(&[], "Keys_95_BPM_F#m.wav"), LoopInfo { bpm: Some(95.0), root: Some(6), minor: Some(true) });
        assert_eq!(loop_info(&[], "Drums 120bpm.wav"), LoopInfo { bpm: Some(120.0), root: None, minor: None });
        assert_eq!(from_name("Bass - Ebmaj 128bpm.aif"), LoopInfo { bpm: Some(128.0), root: Some(3), minor: Some(false) });
        // A key word without a tempo, and numbers that are not a tempo, say nothing.
        assert_eq!(from_name("A Day in Am.wav"), LoopInfo::default());
        assert_eq!(from_name("Take 3.wav"), LoopInfo::default());
        assert_eq!(from_name("loop 1000bpm.wav"), LoopInfo::default());
    }
}
