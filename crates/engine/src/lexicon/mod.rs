//! The Lyrics tab's offline word data: the CMU Pronouncing Dictionary (syllables, stress, rhymes)
//! and WordNet 3.1 (definitions, synonyms, related words), bundled compressed in
//! `assets/lexicon/` (built by `tools/lexicon/build_lexicon.py`; ATTRIBUTION.md has the licences).
//!
//! Loaded once, lazily: [`get`] never blocks (it starts the load on a background thread and says
//! `None` until it is done), [`wait`] blocks until loaded (commands, tests). Lookups go through the
//! [`WordLookup`] trait so another source (an online API) could stand in later.

pub mod phones;
pub mod pron;
pub mod wordnet;

pub use phones::RhymeKind;
pub use pron::{Found, Prons, stress_text};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
pub use wordnet::{Related, Sense, WordNet};

const PRON_LEX: &[u8] = include_bytes!("../../../../assets/lexicon/pron.lex");
const WORDNET_LEX: &[u8] = include_bytes!("../../../../assets/lexicon/wordnet.lex");
/// Largest decompressed asset accepted (the files are ~4 and ~17 MB).
const MAX_TEXT: usize = 64 << 20;

/// What a word source answers.
pub trait WordLookup: Send + Sync {
    /// Syllables of the word as usually sung; `None` when the word is unknown.
    fn syllables(&self, word: &str) -> Option<u32>;
    /// The stress of each syllable (true: primary stress); `None` when unknown.
    fn stresses(&self, word: &str) -> Option<Vec<bool>>;
    /// How two words rhyme, if they do; `None` also when either is unknown.
    fn rhyme_kind(&self, a: &str, b: &str) -> Option<RhymeKind>;
    /// The perfect rhyme part of the word (words sharing it rhyme perfectly).
    fn rhyme_part(&self, word: &str) -> Option<Vec<u8>>;
    /// Rhymes for `word`, at most `max` of each kind.
    fn rhymes(&self, word: &str, max: usize) -> Vec<Found>;
    /// Synonyms and related words.
    fn related(&self, word: &str) -> Related;
    /// Senses with definitions and examples.
    fn senses(&self, word: &str) -> Vec<Sense>;
}

/// The offline lexicon.
#[derive(Default)]
pub struct Lexicon {
    pub prons: Prons,
    pub wordnet: WordNet,
}

impl Lexicon {
    /// Decompresses and reads the bundled assets (about a second in a debug build; a damaged asset
    /// leaves that half empty, never a panic).
    pub fn load_bundled() -> Lexicon {
        let prons = inflate(PRON_LEX).map(|t| Prons::parse(&t)).unwrap_or_default();
        let wordnet = inflate(WORDNET_LEX).map(WordNet::parse).unwrap_or_default();
        Lexicon { prons, wordnet }
    }
}

fn inflate(z: &[u8]) -> Option<String> {
    let raw = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(z, MAX_TEXT).ok()?;
    String::from_utf8(raw).ok()
}

impl WordLookup for Lexicon {
    fn syllables(&self, word: &str) -> Option<u32> {
        self.prons.shortest(word).map(phones::syllables)
    }

    fn stresses(&self, word: &str) -> Option<Vec<bool>> {
        self.prons.shortest(word).map(phones::stresses)
    }

    fn rhyme_kind(&self, a: &str, b: &str) -> Option<RhymeKind> {
        self.prons.classify(a, b)
    }

    fn rhyme_part(&self, word: &str) -> Option<Vec<u8>> {
        self.prons.prons(word)?.first().map(|p| phones::rhyme_part(p)).filter(|r| !r.is_empty())
    }

    fn rhymes(&self, word: &str, max: usize) -> Vec<Found> {
        self.prons.rhymes(word, max)
    }

    fn related(&self, word: &str) -> Related {
        self.wordnet.related(word)
    }

    fn senses(&self, word: &str) -> Vec<Sense> {
        self.wordnet.senses(word)
    }
}

static LEXICON: OnceLock<Lexicon> = OnceLock::new();
static STARTED: AtomicBool = AtomicBool::new(false);

/// The lexicon if loaded; otherwise starts loading it on a background thread (once) and returns
/// `None`. Never blocks for more than a lookup.
pub fn get() -> Option<&'static Lexicon> {
    if let Some(l) = LEXICON.get() {
        return Some(l);
    }
    if !STARTED.swap(true, Ordering::AcqRel) {
        let spawned = std::thread::Builder::new().name("lexicon-load".into()).spawn(|| {
            LEXICON.get_or_init(Lexicon::load_bundled);
        });
        // No threads (the web build): load here, once.
        if spawned.is_err() {
            return Some(LEXICON.get_or_init(Lexicon::load_bundled));
        }
    }
    None
}

/// The lexicon, loading it here if need be (blocks; for commands and tests).
pub fn wait() -> &'static Lexicon {
    STARTED.store(true, Ordering::Release);
    LEXICON.get_or_init(Lexicon::load_bundled)
}

/// Whether the lexicon is loaded.
pub fn ready() -> bool {
    LEXICON.get().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_lexicon_loads_and_answers() {
        let l = wait();
        assert!(l.prons.len() > 100_000, "{} words", l.prons.len());
        assert!(!l.wordnet.is_empty());
        assert!(get().is_some() && ready());
        // Syllables and stress from cmudict for a fixed list.
        for (w, n) in [
            ("love", 1),
            ("beautiful", 3),
            ("fire", 1),
            ("every", 2),
            ("hallelujah", 4),
            ("emotional", 4),
            ("remember", 3),
            ("jesus", 2),
            ("piano", 3),
        ] {
            assert_eq!(l.syllables(w), Some(n), "{w}");
        }
        assert_eq!(l.stresses("alone"), Some(vec![false, true]));
        assert_eq!(l.stresses("happy"), Some(vec![true, false]));
        assert_eq!(l.stresses("remember"), Some(vec![false, true, false]));
        assert_eq!(l.syllables("qwxzt"), None);
        // Rhyme kinds.
        assert_eq!(l.rhyme_kind("long", "song"), Some(RhymeKind::Perfect));
        assert_eq!(l.rhyme_kind("home", "alone"), Some(RhymeKind::Slant));
        assert_eq!(l.rhyme_kind("falling", "calling"), Some(RhymeKind::Multi));
        for (a, b) in [("long", "hill"), ("night", "day"), ("song", "sing"), ("love", "move")] {
            assert_eq!(l.rhyme_kind(a, b), None, "{a}/{b}");
        }
        // The finder: rhymes with syllable counts, WordNet synonyms, definitions.
        let r = l.rhymes("hill", 40);
        assert!(r.iter().any(|f| f.word == "still" && f.syllables == 1 && f.kind == RhymeKind::Perfect), "{r:?}");
        assert!(r.iter().any(|f| f.syllables >= 2), "{r:?}");
        let syn = l.related("happy");
        assert!(syn.synonyms.iter().any(|w| w == "glad"), "{syn:?}");
        let still = l.senses("still");
        assert!(still.iter().any(|s| s.pos == "adjective") && still.iter().any(|s| s.pos == "adverb"));
        assert!(still.iter().any(|s| !s.examples.is_empty()));
    }
}
