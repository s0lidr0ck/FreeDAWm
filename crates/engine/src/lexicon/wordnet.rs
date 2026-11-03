//! WordNet 3.1, pruned (`assets/lexicon/wordnet.lex`): senses by word with their definitions and
//! examples, synonyms (the other words of a sense) and related words (broader, narrower, similar,
//! see also, opposites). The text is kept whole; entries are byte ranges into it.

use std::ops::Range;

type R = Range<u32>;

struct Synset {
    pos: u8,
    lemmas: R,
    def: R,
    examples: R,
    ptrs: R,
}

struct IndexEntry {
    lemma: R,
    count: u32,
    ids: R,
}

/// The pruned WordNet.
#[derive(Default)]
pub struct WordNet {
    text: String,
    synsets: Vec<Synset>,
    index: Vec<IndexEntry>,
    /// Synset ids of the index entries, end to end.
    ids: Vec<u32>,
    /// Irregular forms: (form, bases), sorted by form.
    exc: Vec<(R, R)>,
}

/// One sense of a word.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Sense {
    /// "noun", "verb", "adjective", "adverb".
    pub pos: &'static str,
    pub definition: String,
    pub examples: Vec<String>,
    /// The sense's other words.
    pub synonyms: Vec<String>,
}

/// Words related to a word, by how.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Related {
    pub synonyms: Vec<String>,
    /// Similar (adjectives) and see-also words.
    pub similar: Vec<String>,
    /// More general words.
    pub broader: Vec<String>,
    /// More specific words.
    pub narrower: Vec<String>,
    pub opposites: Vec<String>,
}

fn pos_name(p: u8) -> &'static str {
    match p {
        b'n' => "noun",
        b'v' => "verb",
        b'a' => "adjective",
        b'r' => "adverb",
        _ => "word",
    }
}

const MAX_SYNSETS: usize = 300_000;
const MAX_INDEX: usize = 400_000;

impl WordNet {
    /// Reads `FREEDAWM-WORDNET 1` text; malformed lines are skipped.
    pub fn parse(text: String) -> WordNet {
        if text.len() > u32::MAX as usize || !text.starts_with("FREEDAWM-WORDNET 1") {
            return WordNet::default();
        }
        let mut wn = WordNet::default();
        let mut section = "";
        let mut at = 0usize;
        for line in text.split('\n') {
            let start = at;
            at += line.len() + 1;
            if line.starts_with('#') {
                section = line.trim();
                continue;
            }
            let fields = split_tabs(line, start);
            match section {
                "#SYNSETS" if wn.synsets.len() < MAX_SYNSETS => {
                    let (Some(p), Some(l), Some(d), Some(e), Some(ptr)) =
                        (fields.first(), fields.get(1), fields.get(2), fields.get(3), fields.get(4))
                    else {
                        // A line that does not parse still takes its id, so later ids stay true.
                        wn.synsets.push(Synset { pos: 0, lemmas: 0..0, def: 0..0, examples: 0..0, ptrs: 0..0 });
                        continue;
                    };
                    let pos = text.as_bytes().get(p.start as usize).copied().unwrap_or(0);
                    wn.synsets.push(Synset { pos, lemmas: l.clone(), def: d.clone(), examples: e.clone(), ptrs: ptr.clone() });
                }
                "#INDEX" if wn.index.len() < MAX_INDEX => {
                    let (Some(l), Some(c), Some(ids)) = (fields.first(), fields.get(1), fields.get(2)) else { continue };
                    let count = slice(&text, c).parse::<u32>().unwrap_or(0);
                    let from = wn.ids.len() as u32;
                    wn.ids.extend(slice(&text, ids).split(',').filter_map(|s| s.parse::<u32>().ok()));
                    wn.index.push(IndexEntry { lemma: l.clone(), count, ids: from..wn.ids.len() as u32 });
                }
                "#EXC" => {
                    if let (Some(f), Some(b)) = (fields.first(), fields.get(1)) {
                        wn.exc.push((f.clone(), b.clone()));
                    }
                }
                _ => {}
            }
        }
        wn.text = text;
        let t = &wn.text;
        wn.index.sort_by(|a, b| slice(t, &a.lemma).cmp(slice(t, &b.lemma)));
        wn.exc.sort_by(|a, b| slice(t, &a.0).cmp(slice(t, &b.0)));
        wn
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    fn s(&self, r: &R) -> &str {
        slice(&self.text, r)
    }

    fn entry(&self, lemma: &str) -> Option<&IndexEntry> {
        self.index.binary_search_by(|e| self.s(&e.lemma).cmp(lemma)).ok().and_then(|i| self.index.get(i))
    }

    /// The forms to look `word` up by: itself, then its base forms (irregular ones, then by the
    /// usual endings).
    pub fn base_forms(&self, word: &str) -> Vec<String> {
        let w = word.trim().to_lowercase();
        let mut out = vec![w.clone()];
        if let Ok(i) = self.exc.binary_search_by(|(f, _)| self.s(f).cmp(&w))
            && let Some((_, b)) = self.exc.get(i)
        {
            out.extend(self.s(b).split(',').map(str::to_string));
        }
        for (suf, rep) in [
            ("ies", "y"),
            ("es", ""),
            ("s", ""),
            ("ied", "y"),
            ("ed", ""),
            ("ed", "e"),
            ("ing", ""),
            ("ing", "e"),
            ("er", ""),
            ("er", "e"),
            ("est", ""),
            ("est", "e"),
            ("ier", "y"),
            ("iest", "y"),
        ] {
            if let Some(stem) = w.strip_suffix(suf).filter(|s| s.chars().count() > 1) {
                let base = format!("{stem}{rep}");
                let b = base.as_bytes();
                // Doubled final consonant: "running" is "run".
                if rep.is_empty() && b.len() > 2 && b.last() == b.get(b.len() - 2) && !b"aeiouls".contains(b.last().unwrap_or(&b'a')) {
                    out.push(base.get(..base.len() - 1).unwrap_or("").to_string());
                }
                out.push(base);
            }
        }
        let mut seen = std::collections::HashSet::new();
        out.retain(|f| self.entry(f).is_some() && seen.insert(f.clone()));
        out
    }

    /// The synset ids of `word` (or its base form), most used first.
    fn synsets_of(&self, word: &str) -> (String, Vec<u32>) {
        for f in self.base_forms(word) {
            if let Some(e) = self.entry(&f) {
                let ids = self.ids.get(e.ids.start as usize..e.ids.end as usize).unwrap_or(&[]).to_vec();
                return (f, ids);
            }
        }
        (String::new(), Vec::new())
    }

    /// How often the word's senses were seen in WordNet's tagged texts (a commonness measure).
    pub fn count(&self, word: &str) -> u32 {
        self.base_forms(word).first().and_then(|f| self.entry(f)).map_or(0, |e| e.count)
    }

    fn lemmas(&self, id: u32) -> Vec<String> {
        self.synsets
            .get(id as usize)
            .map(|s| self.s(&s.lemmas).split('|').filter(|l| !l.is_empty()).map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// The word's senses: definitions, examples and synonyms, most used first.
    pub fn senses(&self, word: &str) -> Vec<Sense> {
        let (form, ids) = self.synsets_of(word);
        ids.iter()
            .filter_map(|&id| {
                let s = self.synsets.get(id as usize)?;
                Some(Sense {
                    pos: pos_name(s.pos),
                    definition: self.s(&s.def).to_string(),
                    examples: self.s(&s.examples).split('|').filter(|e| !e.is_empty()).map(str::to_string).collect(),
                    synonyms: self.lemmas(id).into_iter().filter(|l| !l.eq_ignore_ascii_case(&form)).collect(),
                })
            })
            .collect()
    }

    /// Synonyms and related words of `word`, each list in sense order without repeats.
    pub fn related(&self, word: &str) -> Related {
        let (form, ids) = self.synsets_of(word);
        let mut r = Related::default();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::from([form.clone()]);
        let add = |list: &mut Vec<String>, words: Vec<String>, seen: &mut std::collections::HashSet<String>| {
            for w in words {
                if seen.insert(w.to_lowercase()) {
                    list.push(w);
                }
            }
        };
        for &id in &ids {
            add(&mut r.synonyms, self.lemmas(id), &mut seen);
        }
        for &id in &ids {
            let Some(s) = self.synsets.get(id as usize) else { continue };
            for p in self.s(&s.ptrs).split_whitespace() {
                let (kind, rest) = p.split_at(p.char_indices().nth(1).map_or(p.len(), |(i, _)| i));
                let Ok(target) = rest.parse::<u32>() else { continue };
                let words = self.lemmas(target);
                match kind {
                    "&" | "^" => add(&mut r.similar, words, &mut seen),
                    "@" => add(&mut r.broader, words, &mut seen),
                    "~" => add(&mut r.narrower, words, &mut seen),
                    "!" => add(&mut r.opposites, words, &mut seen),
                    _ => {}
                }
            }
        }
        r
    }
}

fn slice<'a>(text: &'a str, r: &R) -> &'a str {
    text.get(r.start as usize..r.end as usize).unwrap_or("")
}

/// The tab-separated fields of `line` (starting at byte `start` of the text) as ranges.
fn split_tabs(line: &str, start: usize) -> Vec<R> {
    let mut out = Vec::new();
    let mut from = start;
    for f in line.split('\t') {
        let end = from + f.len();
        out.push(u32::try_from(from).unwrap_or(0)..u32::try_from(end).unwrap_or(0));
        from = end + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINI: &str = "FREEDAWM-WORDNET 1\n#SYNSETS\na\thappy\tenjoying or showing joy\ta happy smile|feeling happy\t&1 !2\na\tglad|felicitous\tshowing pleasure\t\t\na\tunhappy\tnot happy\t\t\nv\trun|go\tmove fast\the ran away\t@4\nv\ttravel|move\tchange location\t\t\n#INDEX\nglad\t3\t1\nhappy\t37\t0,1\nrun\t20\t3\nunhappy\t5\t2\n#EXC\nran\trun\n";

    #[test]
    fn senses_synonyms_and_related() {
        let wn = WordNet::parse(MINI.to_string());
        let s = wn.senses("Happy");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].pos, "adjective");
        assert_eq!(s[0].examples, vec!["a happy smile", "feeling happy"]);
        assert_eq!(s[1].synonyms, vec!["glad", "felicitous"]);
        let r = wn.related("happy");
        assert_eq!(r.synonyms, vec!["glad", "felicitous"]);
        assert_eq!(r.opposites, vec!["unhappy"]);
        // Irregular and regular forms find their base.
        assert_eq!(wn.related("ran").broader, vec!["travel", "move"]);
        assert_eq!(wn.senses("running")[0].definition, "move fast");
        assert_eq!(wn.count("happy"), 37);
        assert!(wn.senses("zzz").is_empty());
        assert!(WordNet::parse("junk".into()).is_empty());
    }
}
