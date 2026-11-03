//! ARPAbet phones (the CMU Pronouncing Dictionary's alphabet) packed in a byte each, and what the
//! Lyrics tab reads from a pronunciation: syllables, stress, the rhyme part, and how alike two
//! words' endings are (perfect, multi-syllable or slant rhymes).

/// The 39 ARPAbet phonemes; the first 15 are vowels.
pub const PHONES: [&str; 39] = [
    "AA", "AE", "AH", "AO", "AW", "AY", "EH", "ER", "EY", "IH", "IY", "OW", "OY", "UH", "UW", // vowels
    "B", "CH", "D", "DH", "F", "G", "HH", "JH", "K", "L", "M", "N", "NG", "P", "R", "S", "SH", "T", "TH", "V", "W", "Y", "Z", "ZH",
];
const VOWELS: u8 = 15;
/// The stress field of a consonant.
const CONSONANT: u8 = 3;

/// One phone: `code * 4 + stress` (stress 0, 1, 2 for vowels, 3 for consonants).
pub type Phone = u8;

pub fn code(p: Phone) -> u8 {
    p / 4
}

pub fn stress(p: Phone) -> u8 {
    p % 4
}

pub fn is_vowel(p: Phone) -> bool {
    code(p) < VOWELS
}

/// The phone's ARPAbet name (stress digits left off).
pub fn name(p: Phone) -> &'static str {
    PHONES.get(usize::from(code(p))).copied().unwrap_or("?")
}

/// Parses one ARPAbet phone ("AH0", "K").
pub fn parse_phone(s: &str) -> Option<Phone> {
    let (base, digit) = match s.as_bytes().last() {
        Some(d @ b'0'..=b'2') => (s.get(..s.len() - 1)?, Some(d - b'0')),
        _ => (s, None),
    };
    let c = u8::try_from(PHONES.iter().position(|p| *p == base)?).ok()?;
    let st = if c < VOWELS { digit.unwrap_or(0) } else { CONSONANT };
    Some(c * 4 + st)
}

/// Parses a space-separated pronunciation ("L AH1 V"); `None` if any phone is unknown.
pub fn parse(s: &str) -> Option<Vec<Phone>> {
    let v: Option<Vec<Phone>> = s.split_whitespace().map(parse_phone).collect();
    v.filter(|v| !v.is_empty())
}

/// A pronunciation written back as ARPAbet ("L AH1 V").
pub fn to_arpabet(p: &[Phone]) -> String {
    p.iter().map(|&x| if is_vowel(x) { format!("{}{}", name(x), stress(x)) } else { name(x).to_string() }).collect::<Vec<_>>().join(" ")
}

/// Syllables: one per vowel.
pub fn syllables(p: &[Phone]) -> u32 {
    p.iter().filter(|x| is_vowel(**x)).count() as u32
}

/// The stress of each syllable: true for a primary stress.
pub fn stresses(p: &[Phone]) -> Vec<bool> {
    p.iter().filter(|x| is_vowel(**x)).map(|x| stress(*x) == 1).collect()
}

/// The vowel a rhyme hears: AA and AO are one sound for most singers ("falling", "calling").
fn vowel_key(p: Phone) -> u8 {
    let c = code(p);
    if c == 3 { 0 } else { c }
}

/// The index of the vowel a rhyme starts from: the last primary stress, else the last secondary,
/// else the last vowel.
fn rhyme_start(p: &[Phone]) -> Option<usize> {
    let last = |s: u8| p.iter().rposition(|x| is_vowel(*x) && stress(*x) == s);
    last(1).or_else(|| last(2)).or_else(|| p.iter().rposition(|x| is_vowel(*x)))
}

/// The rhyme part: from the rhyme's vowel to the end, stress left off (AA as AO). Words that rhyme
/// perfectly share it.
pub fn rhyme_part(p: &[Phone]) -> Vec<u8> {
    let Some(i) = rhyme_start(p) else { return Vec::new() };
    p.get(i..).unwrap_or(&[]).iter().map(|x| if is_vowel(*x) { vowel_key(*x) } else { code(*x) }).collect()
}

/// The consonant class of a consonant code: stops, fricatives (and affricates), nasals, l, r,
/// glides. Slant rhymes match classes ("home" and "alone": m and n are both nasals).
fn class(c: u8) -> u8 {
    match PHONES.get(usize::from(c)).copied().unwrap_or("") {
        "P" | "B" | "T" | "D" | "K" | "G" => 1,
        "F" | "V" | "TH" | "DH" | "S" | "Z" | "SH" | "ZH" | "CH" | "JH" => 2,
        "M" | "N" | "NG" => 3,
        "L" => 4,
        "R" => 5,
        _ => 6,
    }
}

/// The vowel family (front, back, central, each diphthong its own) of a vowel code.
fn family(c: u8) -> u8 {
    match PHONES.get(usize::from(c)).copied().unwrap_or("") {
        "IY" | "IH" | "EY" | "EH" | "AE" => 1,
        "UW" | "UH" | "OW" | "AO" | "AA" => 2,
        "AH" | "ER" => 3,
        "AY" => 4,
        "AW" => 5,
        _ => 6,
    }
}

/// The slant-rhyme key: the rhyme's vowel, then the classes of what follows (later vowels as one
/// mark). Words sharing it (and not the rhyme part) are near rhymes.
pub fn slant_key(p: &[Phone]) -> Vec<u8> {
    let Some(i) = rhyme_start(p) else { return Vec::new() };
    let mut out = Vec::new();
    for (k, x) in p.get(i..).unwrap_or(&[]).iter().enumerate() {
        out.push(if k == 0 {
            vowel_key(*x)
        } else if is_vowel(*x) {
            100
        } else {
            200 + class(code(*x))
        });
    }
    out
}

/// The last two syllables from the second-to-last vowel, stress left off; `None` for one-syllable
/// words or when neither of those vowels is stressed.
pub fn multi_part(p: &[Phone]) -> Option<Vec<u8>> {
    let vowels: Vec<usize> = p.iter().enumerate().filter(|(_, x)| is_vowel(**x)).map(|(i, _)| i).collect();
    let from = *vowels.get(vowels.len().checked_sub(2)?)?;
    let tail = p.get(from..)?;
    if !tail.iter().any(|x| is_vowel(*x) && stress(*x) > 0) {
        return None;
    }
    Some(tail.iter().map(|x| if is_vowel(*x) { vowel_key(*x) } else { code(*x) }).collect())
}

/// How two words' endings rhyme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RhymeKind {
    /// Near: the same vowel with alike consonants, or the same consonants after a kindred vowel.
    Slant,
    /// The same rhyme part ("long", "song").
    Perfect,
    /// Two or more trailing syllables match ("falling", "calling").
    Multi,
}

/// How pronunciations `a` and `b` rhyme, if they do.
pub fn classify(a: &[Phone], b: &[Phone]) -> Option<RhymeKind> {
    let (ra, rb) = (rhyme_part(a), rhyme_part(b));
    if ra.is_empty() || rb.is_empty() {
        return None;
    }
    if let (Some(ma), Some(mb)) = (multi_part(a), multi_part(b))
        && ma == mb
    {
        return Some(RhymeKind::Multi);
    }
    if ra == rb {
        // A rhyme part of two or more syllables ("beautiful", "dutiful") is a multi rhyme.
        let vowels = ra.iter().filter(|c| **c < VOWELS).count();
        return Some(if vowels >= 2 { RhymeKind::Multi } else { RhymeKind::Perfect });
    }
    if slant_key(a) == slant_key(b) {
        return Some(RhymeKind::Slant);
    }
    // The same consonants after a kindred vowel ("cold", "called" no; "time", "mine" are perfect).
    let (va, vb) = (ra.first().copied().unwrap_or(0), rb.first().copied().unwrap_or(0));
    let (ca, cb) = (ra.get(1..).unwrap_or(&[]), rb.get(1..).unwrap_or(&[]));
    if !ca.is_empty() && ca == cb && ca.iter().all(|c| *c >= VOWELS) && family(va) == family(vb) {
        return Some(RhymeKind::Slant);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Vec<Phone> {
        parse(s).unwrap()
    }

    #[test]
    fn phones_parse_and_print() {
        assert_eq!(to_arpabet(&p("L AH1 V")), "L AH1 V");
        assert_eq!(syllables(&p("B Y UW1 T AH0 F AH0 L")), 3);
        assert_eq!(stresses(&p("AH0 L OW1 N")), vec![false, true]);
        assert!(parse("L XX1 V").is_none());
        assert!(parse("").is_none());
    }

    #[test]
    fn classify_rhyme_kinds() {
        let k = |a: &str, b: &str| classify(&p(a), &p(b));
        assert_eq!(k("L AO1 NG", "S AO1 NG"), Some(RhymeKind::Perfect));
        assert_eq!(k("HH OW1 M", "AH0 L OW1 N"), Some(RhymeKind::Slant));
        assert_eq!(k("F AA1 L IH0 NG", "K AO1 L IH0 NG"), Some(RhymeKind::Multi));
        assert_eq!(k("L AO1 NG", "HH IH1 L"), None);
        assert_eq!(k("S AO1 NG", "S IH1 NG"), None);
        assert_eq!(k("L AH1 V", "M UW1 V"), None);
    }
}
