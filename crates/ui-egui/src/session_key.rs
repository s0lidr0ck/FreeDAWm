//! The session's key, as the Key ruler and the Vocal Editor read it: whether one is set
//! ([`has_key`]), the key in force at a time ([`key_at`]), the names the key picker spells and
//! the empty Key ruler's text. One rule for both, so they never disagree.

use soundcraft_model::Session;
use soundcraft_time::Samples;

const SHARPS: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
/// Roots as major keys spell them (fewest accidentals; F# over Gb).
const MAJOR_ROOTS: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
/// Roots as minor keys spell them (fewest accidentals; Eb over D#, G# over Ab).
const MINOR_ROOTS: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "G#", "A", "Bb", "B"];

/// "E major" for a tonic pitch class and scale (0 major, 1 minor), with sharps.
pub fn key_name(pc: u8, scale: u8) -> String {
    format!("{} {}", SHARPS.get(usize::from(pc % 12)).copied().unwrap_or("C"), if scale == 1 { "minor" } else { "major" })
}

/// A root as a key of that mode spells it ("Bb" in major, "G#" in minor).
pub fn root_spelling(pc: u8, minor: bool) -> &'static str {
    let names = if minor { &MINOR_ROOTS } else { &MAJOR_ROOTS };
    names.get(usize::from(pc % 12)).copied().unwrap_or("C")
}

/// The key change text the key picker writes: "Bb major", "G# minor".
pub fn spelled_key(pc: u8, minor: bool) -> String {
    format!("{} {}", root_spelling(pc, minor), if minor { "minor" } else { "major" })
}

/// Whether the session sets a key to judge notes by: a key change that parses, band chords or
/// sections, or a band key other than the default. Without one the Vocal Editor says "No key set",
/// greys out Snap to key and outlines no note as off key (the engine's `scale_at` still falls
/// back to the band key, but Snap to key can't be turned on there without one), and the empty
/// Key ruler says "No key set" too.
pub fn has_key(s: &Session) -> bool {
    soundcraft_engine::vocal::has_key(s)
}

/// The key in force at `at` as `(tonic pitch class, scale)`: the latest key change at or before
/// it that parses, else the band's key (as the engine's `scale_at`).
pub fn key_pc_at(s: &Session, at: Samples) -> (u8, u8) {
    soundcraft_engine::vocal::key_pc_at(s, at)
}

/// The key in force at `at`, named ("E major").
pub fn key_at(s: &Session, at: Samples) -> String {
    let (pc, sc) = key_pc_at(s, at);
    key_name(pc, sc)
}

/// What the Key ruler shows with no key changes: the band key (spelled as the key picker spells
/// it) "(band key)" when the session sets
/// a key ([`has_key`]), else "No key set".
pub fn empty_ruler_text(s: &Session) -> String {
    if has_key(s) { format!("{} (band key)", spelled_key(s.band.key, s.band.scale == 1)) } else { "No key set".to_string() }
}
