//! The pronouncing dictionary (from cmudict, `assets/lexicon/pron.lex`): pronunciations by word,
//! a commonness score, and indexes from rhyme parts to words for the rhyme finder.

use super::phones::{self, Phone, RhymeKind};
use std::collections::HashMap;

struct Entry {
    word: Box<str>,
    prons: Vec<Box<[Phone]>>,
    /// 0..9 (0: not a WordNet word: names, abbreviations, rare forms).
    common: u8,
}

/// The dictionary.
#[derive(Default)]
pub struct Prons {
    words: Vec<Entry>,
    perfect: HashMap<Vec<u8>, Vec<u32>>,
    slant: HashMap<Vec<u8>, Vec<u32>>,
}

/// A word found by the rhyme finder.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Found {
    pub word: String,
    pub syllables: u32,
    /// "da-DUM" style.
    pub stress: String,
    pub kind: RhymeKind,
    pub common: u8,
}

/// Most entries read (cmudict has ~135k).
const MAX_WORDS: usize = 400_000;

impl Prons {
    /// Reads `FREEDAWM-PRON 1` text; malformed lines are skipped.
    pub fn parse(text: &str) -> Prons {
        let mut lines = text.lines();
        if lines.next().map(str::trim) != Some("FREEDAWM-PRON 1") {
            return Prons::default();
        }
        let mut words: Vec<Entry> = Vec::new();
        for line in lines.take(MAX_WORDS) {
            let mut f = line.split('\t');
            let (Some(w), Some(ps)) = (f.next(), f.next()) else { continue };
            let common = f.next().and_then(|c| c.trim().parse::<u8>().ok()).unwrap_or(0).min(9);
            let prons: Vec<Box<[Phone]>> = ps.split(',').filter_map(phones::parse).map(Vec::into_boxed_slice).collect();
            if w.is_empty() || prons.is_empty() {
                continue;
            }
            words.push(Entry { word: w.to_lowercase().into_boxed_str(), prons, common });
        }
        words.sort_by(|a, b| a.word.cmp(&b.word));
        words.dedup_by(|a, b| a.word == b.word);
        let mut perfect: HashMap<Vec<u8>, Vec<u32>> = HashMap::new();
        let mut slant: HashMap<Vec<u8>, Vec<u32>> = HashMap::new();
        for (i, e) in words.iter().enumerate() {
            let Ok(i) = u32::try_from(i) else { break };
            for p in &e.prons {
                let r = phones::rhyme_part(p);
                if !r.is_empty() {
                    let v = perfect.entry(r).or_default();
                    if v.last() != Some(&i) {
                        v.push(i);
                    }
                }
                let s = phones::slant_key(p);
                if !s.is_empty() {
                    let v = slant.entry(s).or_default();
                    if v.last() != Some(&i) {
                        v.push(i);
                    }
                }
            }
        }
        Prons { words, perfect, slant }
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    fn find(&self, word: &str) -> Option<&Entry> {
        let w = normalise(word);
        let at = |w: &str| self.words.binary_search_by(|e| (*e.word).cmp(w)).ok().and_then(|i| self.words.get(i));
        at(&w).or_else(|| {
            // "dreamin'" as "dreaming"; a possessive as its word.
            if let Some(stem) = w.strip_suffix("in'") {
                return at(&format!("{stem}ing"));
            }
            w.strip_suffix("'s").and_then(at)
        })
    }

    /// The word's pronunciations, most usual first.
    pub fn prons(&self, word: &str) -> Option<&[Box<[Phone]>]> {
        self.find(word).map(|e| e.prons.as_slice())
    }

    /// The pronunciation with the fewest syllables (how a word is usually sung: "fire" as one).
    pub fn shortest(&self, word: &str) -> Option<&[Phone]> {
        self.prons(word)?.iter().min_by_key(|p| phones::syllables(p)).map(|p| &**p)
    }

    /// 0..9; `None` for an unknown word.
    pub fn commonness(&self, word: &str) -> Option<u8> {
        self.find(word).map(|e| e.common)
    }

    /// How two words rhyme (the best over their pronunciations).
    pub fn classify(&self, a: &str, b: &str) -> Option<RhymeKind> {
        let (pa, pb) = (self.prons(a)?, self.prons(b)?);
        pa.iter().flat_map(|x| pb.iter().filter_map(move |y| phones::classify(x, y))).max()
    }

    /// Words that rhyme with `word`: perfect and multi-syllable rhymes (same rhyme part), then slant
    /// rhymes (same vowel, alike consonants). Common words only (in WordNet), most common first, at
    /// most `max` of each kind.
    pub fn rhymes(&self, word: &str, max: usize) -> Vec<Found> {
        let Some(me) = self.find(word) else { return Vec::new() };
        let mut out: Vec<Found> = Vec::new();
        let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
        let mut take = |ids: &[u32], slant_pass: bool, out: &mut Vec<Found>| {
            let mut cands: Vec<(u32, RhymeKind)> = Vec::new();
            for &i in ids {
                let Some(e) = self.words.get(i as usize) else { continue };
                if e.word == me.word || e.common == 0 || !e.word.chars().all(|c| c.is_alphabetic() || c == '\'') || seen.contains(&i) {
                    continue;
                }
                let kind = me.prons.iter().flat_map(|x| e.prons.iter().filter_map(move |y| phones::classify(x, y))).max();
                match kind {
                    Some(RhymeKind::Slant) if slant_pass => cands.push((i, RhymeKind::Slant)),
                    Some(k @ (RhymeKind::Perfect | RhymeKind::Multi)) if !slant_pass => cands.push((i, k)),
                    _ => {}
                }
            }
            cands.sort_by(|a, b| {
                let (ea, eb) = (self.words.get(a.0 as usize), self.words.get(b.0 as usize));
                let key = |e: Option<&Entry>| e.map(|e| (std::cmp::Reverse(e.common), e.word.len())).unwrap_or((std::cmp::Reverse(0), 0));
                key(ea).cmp(&key(eb)).then(a.0.cmp(&b.0))
            });
            let mut counts: HashMap<RhymeKind, usize> = HashMap::new();
            for (i, k) in cands {
                let c = counts.entry(k).or_insert(0);
                if *c >= max {
                    continue;
                }
                *c += 1;
                seen.insert(i);
                if let Some(e) = self.words.get(i as usize) {
                    let p = e.prons.iter().min_by_key(|p| phones::syllables(p)).map(|p| &**p).unwrap_or(&[]);
                    out.push(Found {
                        word: e.word.to_string(),
                        syllables: phones::syllables(p),
                        stress: stress_text(&phones::stresses(p)),
                        kind: k,
                        common: e.common,
                    });
                }
            }
        };
        for p in &me.prons {
            if let Some(ids) = self.perfect.get(&phones::rhyme_part(p)) {
                take(ids, false, &mut out);
            }
        }
        for p in &me.prons {
            if let Some(ids) = self.slant.get(&phones::slant_key(p)) {
                take(ids, true, &mut out);
            }
        }
        out
    }
}

/// A stress pattern as text: "da-DUM-da".
pub fn stress_text(s: &[bool]) -> String {
    s.iter().map(|b| if *b { "DUM" } else { "da" }).collect::<Vec<_>>().join("-")
}

/// A word as the dictionary keys it: lower case, curly apostrophes straight, outer punctuation
/// trimmed.
pub fn normalise(word: &str) -> String {
    word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '\u{2019}')
        .trim_start_matches(['\'', '\u{2019}'])
        .chars()
        .map(|c| if c == '\u{2019}' { '\'' } else { c })
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINI: &str = "FREEDAWM-PRON 1\nlong\tL AO1 NG\t9\nsong\tS AO1 NG\t6\nthrong\tTH R AO1 NG\t2\nlongue\tL AO1 NG\t0\nhome\tHH OW1 M\t8\nalone\tAH0 L OW1 N\t7\nbad line\nfire\tF AY1 ER0,F AY1 R\t8\nzz\tQQ1\t3\n";

    #[test]
    fn parses_and_finds_rhymes() {
        let d = Prons::parse(MINI);
        assert_eq!(d.len(), 7);
        assert_eq!(d.shortest("Fire!").map(phones::syllables), Some(1));
        assert_eq!(d.classify("long", "song"), Some(RhymeKind::Perfect));
        assert_eq!(d.classify("home", "alone"), Some(RhymeKind::Slant));
        let r: Vec<String> = d.rhymes("long", 10).into_iter().map(|f| f.word).collect();
        // Most common first; uncommon (not in WordNet) left out.
        assert_eq!(r, vec!["song", "throng"]);
        assert!(d.rhymes("home", 10).iter().any(|f| f.word == "alone" && f.kind == RhymeKind::Slant));
        assert!(Prons::parse("nope\nlong\tL AO1 NG\t9").is_empty());
        assert_eq!(normalise("\u{2018}Don\u{2019}t,"), "don't");
    }
}
