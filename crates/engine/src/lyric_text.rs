//! Lyric-sheet text helpers (Window › Lyrics): syllable counts, stress and end-rhyme keys from the
//! bundled pronouncing dictionary ([`crate::lexicon`], once loaded), falling back for words it
//! lacks (and until it has loaded) to heuristics with small exception tables; the rhyme letters of
//! a sheet's lines; offline rhyme suggestions.

use crate::lexicon::{self, WordLookup};
use soundcraft_model::{LineKind, line_kinds};

/// Words the vowel-group count gets wrong.
const SYLLABLE_EXCEPTIONS: &[(&str, u32)] = &[
    ("every", 2),
    ("everything", 3),
    ("everyone", 3),
    ("everybody", 4),
    ("evening", 2),
    ("business", 2),
    ("area", 3),
    ("idea", 3),
    ("ideas", 3),
    ("poem", 2),
    ("poems", 2),
    ("quiet", 2),
    ("being", 2),
    ("lion", 2),
    ("create", 2),
    ("created", 3),
    ("real", 1),
    ("really", 2),
    ("fire", 1),
    ("desire", 2),
    ("hour", 1),
    ("hours", 1),
    ("our", 1),
    ("going", 2),
    ("doing", 2),
    ("seeing", 2),
    ("crying", 2),
    ("dying", 2),
    ("flying", 2),
    ("trying", 2),
    ("lying", 2),
    ("saying", 2),
    ("playing", 2),
    ("praying", 2),
    ("staying", 2),
    ("cruel", 2),
    ("people", 2),
    ("someone", 2),
    ("somewhere", 2),
    ("something", 2),
    ("sometimes", 2),
    ("forever", 3),
    ("heaven", 2),
    ("chocolate", 3),
    ("different", 3),
    ("family", 3),
    ("naive", 2),
    ("alive", 2),
    ("jesus", 2),
    ("the", 1),
    ("are", 1),
    ("were", 1),
    ("where", 1),
    ("there", 1),
    ("here", 1),
    ("whole", 1),
    ("goes", 1),
    ("does", 1),
    ("eyes", 1),
    ("lives", 1),
    ("loves", 1),
    ("moved", 1),
    ("loved", 1),
];

fn letters(word: &str) -> String {
    word.chars().filter(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_lowercase()).collect()
}

fn vowel(c: u8) -> bool {
    matches!(c, b'a' | b'e' | b'i' | b'o' | b'u')
}

/// Whether letter `i` of `b` sounds as a vowel: a, e, i, o, u, and y unless it starts the word or
/// a vowel follows it ("you", "beyond").
fn is_v(b: &[u8], i: usize) -> bool {
    match b.get(i) {
        Some(b'y') => i > 0 && !b.get(i + 1).is_some_and(|c| vowel(*c)),
        Some(c) => vowel(*c),
        None => false,
    }
}

/// The vowel groups of `b`: (start, end) byte ranges.
fn groups(b: &[u8]) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if is_v(b, i) {
            let s = i;
            while i < b.len() && is_v(b, i) {
                i += 1;
            }
            v.push((s, i));
        } else {
            i += 1;
        }
    }
    v
}

/// The syllables of one English word (at least 1 for a word with letters; 0 for none): the
/// dictionary's count (as usually sung: "fire" is one), else the heuristic.
pub fn syllables(word: &str) -> u32 {
    lexicon::get().and_then(|l| l.syllables(word)).unwrap_or_else(|| heuristic_syllables(word))
}

/// Short words usually sung unstressed.
const FUNCTION_WORDS: &[&str] = &[
    "a", "an", "the", "and", "but", "or", "nor", "so", "as", "at", "by", "for", "from", "in", "into", "of", "off", "on", "onto", "to", "up", "with",
    "than", "that", "if", "is", "am", "are", "was", "were", "be", "been", "can", "could", "would", "should", "will", "shall", "may", "might", "must",
    "do", "does", "did", "has", "have", "had", "i", "me", "my", "you", "your", "he", "him", "his", "she", "her", "it", "its", "we", "us", "our",
    "they", "them", "their", "this", "these", "those", "there", "what", "when", "who", "whom", "whose", "which", "where", "while", "oh", "i'm",
    "i'll", "i've", "i'd", "you're", "it's", "we're", "they're", "let", "yeah", "o", "not",
];

/// Whether `word` is a function word (articles, pronouns, prepositions, auxiliaries…).
pub fn is_function_word(word: &str) -> bool {
    let w = lexicon::pron::normalise(word);
    FUNCTION_WORDS.contains(&w.as_str())
}

/// The stress of each syllable of `word` (true: stressed). One-syllable function words are
/// unstressed, other one-syllable words stressed; longer words from the dictionary, else the
/// first syllable stressed.
pub fn word_stresses(word: &str) -> Vec<bool> {
    let n = syllables(word);
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![!is_function_word(word)];
    }
    if let Some(s) = lexicon::get().and_then(|l| l.stresses(word)).filter(|s| s.len() == n as usize) {
        return s;
    }
    (0..n).map(|i| i == 0).collect()
}

/// The words of a line as sung (split on spaces, hyphens and slashes; pieces without letters or
/// digits dropped).
pub fn line_words(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| c.is_whitespace() || c == '-' || c == '/').filter(|w| w.chars().any(char::is_alphanumeric))
}

/// The stress pattern of a line, syllable by syllable.
pub fn line_stresses(line: &str) -> Vec<bool> {
    line_words(line).flat_map(word_stresses).collect()
}

/// The heuristic syllable count (vowel groups with English spelling rules and an exceptions list).
pub fn heuristic_syllables(word: &str) -> u32 {
    let w = letters(word);
    if w.is_empty() {
        // Numbers and symbols: count each digit run as one word's worth.
        return u32::from(word.chars().any(|c| c.is_ascii_digit()));
    }
    if let Some((_, n)) = SYLLABLE_EXCEPTIONS.iter().find(|(e, _)| *e == w) {
        return *n;
    }
    let b = w.as_bytes();
    let g = groups(b);
    let mut n = g.len() as i64;
    let len = b.len();
    let ends = |s: &str| w.ends_with(s);
    let before = |k: usize| len.checked_sub(k + 1).and_then(|i| b.get(i).copied());
    // A silent final e ("there", "smile"), not consonant + "le" ("table").
    let silent_e = ends("e") && !is_v(b, len - 2) && !(before(1) == Some(b'l') && before(2).is_some_and(|c| !vowel(c)));
    // "-ed" after anything but t or d is no syllable ("loved", not "wanted").
    let silent_ed = ends("ed") && before(2).is_some_and(|c| c != b't' && c != b'd' && !vowel(c));
    // "-es" is silent after most consonants ("hopes"), not after s, x, z, ch, sh, ce, ge.
    let silent_es =
        ends("es") && before(2).is_some_and(|c| !matches!(c, b's' | b'x' | b'z' | b'c' | b'g') && !vowel(c)) && !ends("ches") && !ends("shes");
    if n > 1 && (silent_e || silent_ed || silent_es) {
        n -= 1;
    }
    // A silent e before -ly, -ful, -ment, -ness, -less ("lonely", "hopeful").
    for suf in ["ly", "ful", "ment", "ness", "less"] {
        if let Some(stem) = w.strip_suffix(suf)
            && stem.len() >= 3
            && stem.ends_with('e')
            && stem.as_bytes().get(stem.len() - 2).is_some_and(|c| !vowel(*c))
            && n > 1
        {
            n -= 1;
        }
    }
    // Two vowels sounded apart: "-ia", "-io" ("piano", "lion"), except -tion, -sion, -cious…
    for pat in ["ia", "io", "iu", "ua", "uo"] {
        let mut from = 0;
        while let Some(k) = w.get(from..).and_then(|r| r.find(pat)) {
            let at = from + k;
            let prev = at.checked_sub(1).and_then(|i| b.get(i).copied());
            let joined =
                matches!(prev, Some(b't' | b's' | b'c' | b'g' | b'x')) || (pat == "ua" && prev == Some(b'q')) || (pat == "uo" && prev == Some(b'q'));
            if !joined {
                n += 1;
            }
            from = at + pat.len();
        }
    }
    // "-ying", "-oing", "-eing" ("crying", "going").
    if ends("ing") && len > 4 && before(3).is_some_and(|c| matches!(c, b'y' | b'o' | b'e' | b'a')) {
        n += 1;
    }
    n.max(1) as u32
}

/// The syllables of a line: the sum over its words.
pub fn line_syllables(line: &str) -> u32 {
    line_words(line).map(syllables).sum()
}

/// End-rhyme keys the spelling gets wrong.
const RHYME_EXCEPTIONS: &[(&str, &str)] = &[
    ("one", "UN"),
    ("done", "UN"),
    ("none", "UN"),
    ("won", "UN"),
    ("son", "UN"),
    ("sun", "UN"),
    ("love", "UV"),
    ("above", "UV"),
    ("of", "UV"),
    ("glove", "UV"),
    ("dove", "UV"),
    ("come", "UM"),
    ("some", "UM"),
    ("become", "UM"),
    ("gone", "ON"),
    ("on", "ON"),
    ("move", "OOV"),
    ("prove", "OOV"),
    ("lose", "OOZ"),
    ("whose", "OOZ"),
    ("do", "OO"),
    ("to", "OO"),
    ("two", "OO"),
    ("who", "OO"),
    ("you", "OO"),
    ("through", "OO"),
    ("though", "OH"),
    ("although", "OH"),
    ("enough", "UF"),
    ("tough", "UF"),
    ("rough", "UF"),
    ("heart", "ART"),
    ("apart", "ART"),
    ("start", "ART"),
    ("heard", "ERD"),
    ("word", "ERD"),
    ("world", "ERLD"),
    ("work", "ERK"),
    ("were", "ER"),
    ("there", "AIR"),
    ("where", "AIR"),
    ("their", "AIR"),
    ("here", "EER"),
    ("hear", "EER"),
    ("year", "EER"),
    ("eye", "AI"),
    ("i", "AI"),
    ("by", "AI"),
    ("head", "ED"),
    ("dead", "ED"),
    ("bread", "ED"),
    ("said", "ED"),
    ("instead", "ED"),
    ("ahead", "ED"),
    ("spread", "ED"),
    ("thread", "ED"),
    ("breath", "ETH"),
    ("death", "ETH"),
    ("friend", "END"),
    ("good", "UUD"),
    ("could", "UUD"),
    ("would", "UUD"),
    ("should", "UUD"),
    ("stood", "UUD"),
    ("wood", "UUD"),
    ("hood", "UUD"),
    ("understood", "UUD"),
    ("our", "OWR"),
    ("hour", "OWR"),
    ("power", "OWR"),
    ("flower", "OWR"),
    ("tower", "OWR"),
    ("how", "OW"),
    ("now", "OW"),
    ("cow", "OW"),
    ("wow", "OW"),
    ("allow", "OW"),
    ("vow", "OW"),
    ("bow", "OW"),
    ("down", "OWN"),
    ("town", "OWN"),
    ("crown", "OWN"),
    ("brown", "OWN"),
    ("clown", "OWN"),
    ("frown", "OWN"),
    ("drown", "OWN"),
    ("only", "OHN-LEE"),
    ("lonely", "OHN-LEE"),
    ("are", "AR"),
    ("have", "AV"),
    ("give", "IV"),
    ("live", "IV"),
    ("been", "IN"),
    ("great", "AYT"),
    ("break", "AYK"),
    ("steak", "AYK"),
    ("they", "AY"),
    ("obey", "AY"),
    ("grey", "AY"),
    ("prey", "AY"),
    ("key", "EE"),
    ("people", "EE-PUL"),
    ("fire", "AIER"),
    ("desire", "AIER"),
    ("higher", "AIER"),
    ("liar", "AIER"),
    ("choir", "AIER"),
    ("door", "OR"),
    ("floor", "OR"),
    ("four", "OR"),
    ("your", "OR"),
    ("pour", "OR"),
    ("more", "OR"),
    ("war", "OR"),
    ("touch", "UCH"),
    ("much", "UCH"),
    ("such", "UCH"),
    ("blood", "UD"),
    ("flood", "UD"),
    ("mud", "UD"),
    ("was", "UZ"),
    ("because", "UZ"),
    ("does", "UZ"),
    ("what", "UT"),
    ("child", "AILD"),
    ("wild", "AILD"),
    ("mild", "AILD"),
];

/// The vowel sound of the stressed syllable, and how many letters of the coda it takes with it
/// (the r of "ar", the w of "aw").
fn sound(group: &str, magic: bool, coda: &str, one_syllable: bool) -> (&'static str, usize) {
    let r = coda.starts_with('r');
    let w = coda.starts_with('w');
    let v = match group {
        "a" if r => return (if magic { "AIR" } else { "AR" }, 1),
        "e" | "i" | "u" if r && !magic => return ("ER", 1),
        "e" if r => return ("EER", 1),
        "i" if r => return ("AIER", 1),
        "u" if r => return ("OOR", 1),
        "o" | "oa" | "oo" | "ou" if r => return ("OR", 1),
        "ea" if r => {
            return (
                if coda == "r" {
                    "EER"
                } else if coda.starts_with("rt") {
                    "AR"
                } else {
                    "ER"
                },
                1,
            );
        }
        "ee" if r => return ("EER", 1),
        "ai" if r => return ("AIR", 1),
        "a" if w => return ("AW", 1),
        "e" if w => return ("OO", 1),
        "o" if w => return ("OH", 1),
        "a" if magic => "AY",
        "a" if coda.is_empty() => "AH",
        "a" if coda.starts_with("ll") || coda.starts_with("lk") => "AW",
        "a" => "A",
        "e" if magic || coda.is_empty() => "EE",
        "e" => "E",
        "i" if magic || coda.is_empty() => "AI",
        "i" if coda == "nd" || coda == "ld" => "AI",
        "i" => "I",
        "o" if magic || coda.is_empty() || coda == "ld" || coda == "lt" => "OH",
        "o" => "O",
        "u" if magic || coda.is_empty() => "OO",
        "u" => "U",
        "y" | "ey" if one_syllable => "AI",
        "y" => "EE",
        "ai" | "ay" | "ei" => "AY",
        "ey" => "EE",
        "ee" | "ea" => "EE",
        "ie" if coda.is_empty() && one_syllable => "AI",
        "ie" => "EE",
        "oa" | "oe" => "OH",
        "oo" | "ue" | "ui" | "ew" | "eu" => "OO",
        "ou" => "OW",
        "au" | "aw" => "AW",
        "oi" | "oy" => "OY",
        "igh" => "AI",
        _ => "",
    };
    (v, 0)
}

/// The consonants after the vowel, as heard: doubled letters single, "ck" k, "ph" f, a final
/// silent "gh" gone; after a silent e, c is s and g is j.
fn coda_sound(coda: &str, magic: bool) -> String {
    let mut c = coda.replace("ck", "k").replace("ph", "f").replace("tch", "ch").replace("dg", "j");
    if c.ends_with("gh") {
        c.truncate(c.len() - 2);
    }
    let mut out = String::new();
    for ch in c.chars() {
        if !out.ends_with(ch) {
            out.push(ch);
        }
    }
    if magic {
        if out.ends_with('c') {
            out.pop();
            out.push('s');
        } else if out.ends_with('g') {
            out.pop();
            out.push('j');
        } else if out.ends_with('s') {
            out.pop();
            out.push('z');
        }
    }
    out.to_ascii_uppercase()
}

/// The rhyme part of a stressed final syllable.
fn single_key(w: &str, one_syllable: bool) -> Option<String> {
    let b = w.as_bytes();
    let len = b.len();
    // A silent final e that makes the vowel before it long ("time", "home").
    let magic = len >= 3 && w.ends_with('e') && !is_v(b, len - 2) && !groups(b.get(..len - 1)?).is_empty() && {
        let g = groups(b.get(..len - 1)?);
        // The last vowel is a single letter followed by one or two consonants.
        g.last().is_some_and(|(s, e)| e - s == 1 && len - 1 - e <= 2) && !(b.get(len - 2) == Some(&b'l') && !is_v(b, len - 3))
    };
    let core = if magic || (w.ends_with('e') && len >= 3 && !is_v(b, len - 2) && groups(b).len() > 1) { w.get(..len - 1)? } else { w };
    let cb = core.as_bytes();
    let (s, e) = *groups(cb).last()?;
    let mut group = core.get(s..e)?.to_string();
    let mut coda = core.get(e..)?.to_string();
    if group.ends_with('i') && coda.starts_with("gh") {
        group = "igh".into();
        coda = coda.get(2..)?.to_string();
    }
    let (v, eats) = sound(&group, magic, &coda, one_syllable);
    let v = if v.is_empty() { group.to_ascii_uppercase() } else { v.to_string() };
    Some(format!("{v}{}", coda_sound(coda.get(eats..).unwrap_or(""), magic)))
}

/// A word's end-rhyme key: words that rhyme share it ("long" and "song", "still" and "hill"). The
/// dictionary's rhyme part when it knows the word, else the spelling heuristic. `None` for a word
/// with no letters.
pub fn rhyme_key(word: &str) -> Option<String> {
    if let Some(r) = lexicon::get().and_then(|l| l.rhyme_part(word)) {
        return Some(r.iter().map(|c| lexicon::phones::PHONES.get(usize::from(*c)).copied().unwrap_or("?")).collect::<Vec<_>>().join("."));
    }
    heuristic_rhyme_key(word)
}

/// The spelling-based end-rhyme key (with an exceptions table).
pub fn heuristic_rhyme_key(word: &str) -> Option<String> {
    let w = letters(word);
    if w.is_empty() {
        return None;
    }
    if let Some((_, k)) = RHYME_EXCEPTIONS.iter().find(|(e, _)| *e == w) {
        return Some((*k).to_string());
    }
    // A plural or third person: the singular's rhyme, voiced ("eyes" and "skies").
    if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("us") && !w.ends_with("is") {
        let one = if let Some(stem) = w.strip_suffix("ies") { format!("{stem}y") } else { w[..w.len() - 1].to_string() };
        if let Some(k) = heuristic_rhyme_key(&one) {
            return Some(format!("{}Z", k.trim_end_matches(['S', 'Z'])));
        }
    }
    let n = heuristic_syllables(&w);
    // An unstressed ending carries the stressed syllable before it: "singing", "lonely", "rider".
    if n >= 2 {
        for (suf, tail) in [("ing", "ING"), ("ly", "LEE"), ("er", "ER"), ("ed", "ED")] {
            if let Some(stem) = w.strip_suffix(suf)
                && stem.len() >= 2
                && !groups(stem.as_bytes()).is_empty()
            {
                let stem_key = heuristic_rhyme_key(stem).or_else(|| single_key(stem, true))?;
                return Some(format!("{stem_key}-{tail}"));
            }
        }
    }
    single_key(&w, n <= 1)
}

/// The last word of a line (punctuation trimmed).
pub fn end_word(line: &str) -> Option<&str> {
    line.split_whitespace().rev().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')).find(|w| w.chars().any(char::is_alphabetic))
}

/// One sheet line as the Lyrics tab shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct LineInfo {
    /// The lyric line number (none for headings and blank lines).
    pub number: Option<u32>,
    pub heading: bool,
    pub syllables: u32,
    /// The rhyme letter (lines whose end words rhyme share one).
    pub rhyme: Option<char>,
    /// Whether another line shares the letter.
    pub rhymes: bool,
    /// The rhyme word: its byte range in the line.
    pub word: Option<(usize, usize)>,
}

/// Syllables and rhyme letters for each of `lines`: letters go A, B, C… in order of first use.
pub fn analyse<'a>(lines: impl IntoIterator<Item = &'a str> + Clone) -> Vec<LineInfo> {
    let kinds = line_kinds(lines.clone());
    let mut keys: Vec<String> = Vec::new();
    let mut infos: Vec<(LineInfo, Option<usize>)> = Vec::new();
    for (text, kind) in lines.into_iter().zip(kinds) {
        let mut info = LineInfo { number: None, heading: kind == LineKind::Heading, syllables: 0, rhyme: None, rhymes: false, word: None };
        let mut slot = None;
        if let LineKind::Lyric(n) = kind {
            info.number = Some(n);
            info.syllables = line_syllables(text);
            if let Some(w) = end_word(text) {
                let at = text.rfind(w).unwrap_or(0);
                info.word = Some((at, at + w.len()));
                if let Some(k) = rhyme_key(w) {
                    let i = keys.iter().position(|x| *x == k).unwrap_or_else(|| {
                        keys.push(k);
                        keys.len() - 1
                    });
                    slot = Some(i);
                }
            }
        }
        infos.push((info, slot));
    }
    let mut counts = vec![0usize; keys.len()];
    for (_, s) in &infos {
        if let Some(c) = s.and_then(|s| counts.get_mut(s)) {
            *c += 1;
        }
    }
    infos
        .into_iter()
        .map(|(mut info, slot)| {
            if let Some(s) = slot {
                info.rhyme = Some(letter(s));
                info.rhymes = counts.get(s).is_some_and(|c| *c > 1);
            }
            info
        })
        .collect()
}

/// The rhyme letter of group `i`: A..Z, then a..z, then '?'.
fn letter(i: usize) -> char {
    match i {
        0..=25 => char::from(b'A' + i as u8),
        26..=51 => char::from(b'a' + (i - 26) as u8),
        _ => '?',
    }
}

/// Words to suggest rhymes from (common in songs).
const WORDS: &str = "a away day say way stay play pray today yesterday okay grey stray sway they
night light right sight might fight bright tight flight white bite quite write invite tonight delight
time mine line shine fine sign divine climb rhyme design wine pine
long song strong wrong along belong
still hill will fill chill kill thrill until spill skill mill
heart apart start part art dark spark star far are car bar guitar
love above of glove dove enough
fire desire higher liar
free me see be sea tree three key believe agree knee degree eternity
know go show slow flow grow so low below snow glow though road home alone stone bone phone
blue true you do too through new knew view few grew
one sun done none run fun gun begun undone won
rain again pain gain chain plain remain vain train
eyes skies cries lies rise wise surprise goodbyes disguise
cold old hold gold told fold bold
soul whole control roll goal hole
mind find kind blind behind remind
heaven seven
fall all call wall small tall ball
end friend send bend mend pretend
dream seem team stream scream
feel real deal heal steal reveal kneel
sky fly high cry why lie die try goodbye
place space face grace race embrace
dance chance glance
hand stand land understand band sand
dead head said bed red instead ahead spread
breath death
door floor more before shore for war core sure
bring sing ring thing king spring wing everything
lonely only
alive survive drive arrive
sound ground found around down town crown
care where there air stare dare prayer
near here fear clear year tear dear
mean seen green between clean dream
done won sun one
word heard world turn burn learn return
cold hold
loving moving
way";

/// Words that rhyme with `word` (up to `max`): perfect rhymes from the dictionary when it is loaded
/// and knows the word, else from the built-in list.
pub fn suggest_rhymes(word: &str, max: usize) -> Vec<String> {
    if let Some(l) = lexicon::get() {
        let found: Vec<String> = l.rhymes(word, max).into_iter().filter(|f| f.kind != lexicon::RhymeKind::Slant).map(|f| f.word).take(max).collect();
        if !found.is_empty() {
            return found;
        }
    }
    let Some(key) = heuristic_rhyme_key(word) else { return Vec::new() };
    let me = letters(word);
    let mut out: Vec<String> = Vec::new();
    for w in WORDS.split_whitespace() {
        if w != me && !out.iter().any(|o| o == w) && heuristic_rhyme_key(w).as_deref() == Some(key.as_str()) {
            out.push(w.to_string());
            if out.len() >= max {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syllables_within_one_of_the_dictionary() {
        let words = [
            ("when", 1),
            ("the", 1),
            ("night", 1),
            ("was", 1),
            ("long", 1),
            ("heart", 1),
            ("could", 1),
            ("still", 1),
            ("there", 1),
            ("sing", 1),
            ("song", 1),
            ("standing", 2),
            ("upon", 2),
            ("jesus", 2),
            ("only", 2),
            ("one", 1),
            ("table", 2),
            ("smile", 1),
            ("hopes", 1),
            ("kisses", 2),
            ("wanted", 2),
            ("loved", 1),
            ("lonely", 2),
            ("beautiful", 3),
            ("piano", 3),
            ("nation", 2),
            ("crying", 2),
            ("going", 2),
            ("everything", 3),
            ("remember", 3),
            ("harmony", 3),
            ("tomorrow", 3),
            ("emotional", 4),
            ("hallelujah", 4),
            ("fire", 1),
            ("people", 2),
            ("believe", 2),
            ("you", 1),
            ("yesterday", 3),
            ("forgiven", 3),
        ];
        let mut exact = 0;
        for (w, want) in words {
            let got = heuristic_syllables(w);
            assert!(got.abs_diff(want) <= 1, "{w}: {got}, want {want}");
            exact += usize::from(got == want);
        }
        assert!(exact * 10 >= words.len() * 9, "only {exact} of {} exact", words.len());
        // The mockup's lines.
        let line_syllables = |l: &str| -> u32 { line_words(l).map(heuristic_syllables).sum() };
        assert_eq!(line_syllables("When the night was long"), 5);
        assert_eq!(line_syllables("And my heart could not be still"), 7);
        assert_eq!(line_syllables("You were there to sing the song"), 7);
        assert_eq!(line_syllables("Standing strong upon the hill"), 7);
        assert_eq!(line_syllables("Jesus, the only one"), 6);
    }

    #[test]
    fn rhymes_group_known_pairs_and_part_non_rhymes() {
        let rhyme = |a: &str, b: &str| heuristic_rhyme_key(a) == heuristic_rhyme_key(b);
        for (a, b) in [
            ("long", "song"),
            ("still", "hill"),
            ("night", "bite"),
            ("light", "sight"),
            ("time", "rhyme"),
            ("day", "away"),
            ("free", "me"),
            ("know", "slow"),
            ("blue", "true"),
            ("heart", "apart"),
            ("love", "above"),
            ("fire", "desire"),
            ("rain", "again"),
            ("go", "show"),
            ("singing", "ringing"),
            ("cold", "hold"),
            ("one", "sun"),
            ("eyes", "skies"),
        ] {
            assert!(rhyme(a, b), "{a} / {b}: {:?} {:?}", heuristic_rhyme_key(a), heuristic_rhyme_key(b));
        }
        for (a, b) in
            [("long", "hill"), ("night", "day"), ("song", "sing"), ("still", "stall"), ("heart", "hurt"), ("love", "move"), ("cold", "could")]
        {
            assert!(!rhyme(a, b), "{a} / {b} rhyme: {:?}", heuristic_rhyme_key(a));
        }
        assert_eq!(heuristic_rhyme_key("!!"), None);
    }

    #[test]
    fn lines_get_letters_numbers_and_words() {
        let sheet = [
            "[Verse 1]",
            "When the night was long",
            "And my heart could not be still",
            "You were there to sing the song",
            "Standing strong upon the hill",
            "",
            "[Chorus]",
            "Jesus, the only one",
            "Jesus, the only one",
            "Nothing else",
        ];
        lexicon::wait();
        let a = analyse(sheet.iter().copied());
        let letters: Vec<Option<char>> = a.iter().map(|i| i.rhyme).collect();
        assert_eq!(letters, vec![None, Some('A'), Some('B'), Some('A'), Some('B'), None, None, Some('C'), Some('C'), Some('D')]);
        assert_eq!(a[1].number, Some(1));
        assert_eq!(a[8].number, Some(6));
        assert!(a[0].heading);
        assert!(a[1].rhymes && !a[9].rhymes);
        assert_eq!(a[1].word.and_then(|(s, e)| sheet[1].get(s..e)), Some("long"));
        assert_eq!(a[7].word.and_then(|(s, e)| sheet[7].get(s..e)), Some("one"));
    }

    #[test]
    fn rhyme_suggestions_come_from_the_dictionary() {
        lexicon::wait();
        let s = suggest_rhymes("long", 10);
        assert!(s.contains(&"song".to_string()) && s.contains(&"strong".to_string()), "{s:?}");
        assert!(!s.contains(&"long".to_string()));
        assert!(suggest_rhymes("night", 40).contains(&"light".to_string()));
        assert!(suggest_rhymes("???", 5).is_empty());
    }

    #[test]
    fn dictionary_syllables_stress_and_oov_fallback() {
        lexicon::wait();
        assert_eq!(syllables("beautiful"), 3);
        assert_eq!(syllables("fire"), 1);
        assert_eq!(line_syllables("Hallelujah, every evening"), 4 + 2 + 2);
        // Not in the dictionary: the heuristic, first syllable stressed.
        assert_eq!(syllables("glorpington"), heuristic_syllables("glorpington"));
        assert_eq!(word_stresses("blorfy"), vec![true, false]);
        assert_eq!(word_stresses("the"), vec![false]);
        assert_eq!(word_stresses("night"), vec![true]);
        assert_eq!(word_stresses("alone"), vec![false, true]);
        assert_eq!(line_stresses("When the night was long"), vec![false, false, true, false, true]);
        assert_eq!(rhyme_key("long"), rhyme_key("song"));
        assert_ne!(rhyme_key("long"), rhyme_key("sing"));
    }
}
