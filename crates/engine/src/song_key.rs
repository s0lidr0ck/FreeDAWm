//! One key for the song. The Key ruler's key at bar 1 (a key change at the session start) and the
//! band's key (`BandState::key`/`scale`, what the chord lane's degrees are read in) are one value:
//!
//! - an edit to the Key ruler that leaves a key change at the start puts the band in that key;
//! - an edit that changes the band's key (a factory song laid, `band.settings {key, scale}`) puts
//!   that key at the start of the Key ruler, replacing the key change there;
//! - a session that kept them apart opens with the ruler's key (or, with no key change at the start
//!   and a band key of its own, with that key written onto the ruler).
//!
//! Key changes later in the song are the ruler's alone (the Vocal Editor and MIDI Cleanup read
//! them); the band plays the whole song in the key at bar 1.

use crate::vocal::parse_key;
use soundcraft_model::{BandState, Session};

/// Roots as major keys spell them (fewest accidentals; F# over Gb).
const MAJOR_ROOTS: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
/// Roots as minor keys spell them (fewest accidentals; Eb over D#, G# over Ab).
const MINOR_ROOTS: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "G#", "A", "Bb", "B"];

/// A root as a key of that mode spells it ("Bb" in major, "G#" in minor).
pub fn root_spelling(pc: u8, minor: bool) -> &'static str {
    let names = if minor { &MINOR_ROOTS } else { &MAJOR_ROOTS };
    names.get(usize::from(pc % 12)).copied().unwrap_or("C")
}

/// The key change text the key picker writes: "Bb major", "G# minor".
pub fn spelled_key(pc: u8, minor: bool) -> String {
    format!("{} {}", root_spelling(pc, minor), if minor { "minor" } else { "major" })
}

/// The key change at the session start that sets the song's key: the last one at or before 0 that
/// parses, as its index and `(tonic, scale)`.
fn start_key(s: &Session) -> Option<(usize, (u8, u8))> {
    s.key_signatures.iter().enumerate().filter(|(_, (t, _))| *t <= 0).filter_map(|(i, (_, n))| parse_key(n).map(|k| (i, k))).next_back()
}

/// After an edit (`before` → `s`): whichever of the two changed, the other follows. A no-op when
/// neither did.
pub fn follow(before: &Session, s: &mut Session) {
    if before.key_signatures != s.key_signatures {
        if let Some((_, (key, scale))) = start_key(s) {
            s.band.key = key;
            s.band.scale = scale;
        }
    } else if (before.band.key, before.band.scale) != (s.band.key, s.band.scale) {
        write_band_key(s);
    }
}

/// Whether an edit from `before` to `s` touched either owner of the key (cheap: no copy).
pub fn changed(before: &Session, s: &Session) -> bool {
    before.key_signatures != s.key_signatures || (before.band.key, before.band.scale) != (s.band.key, s.band.scale)
}

/// The band's key onto the Key ruler at the session start.
fn write_band_key(s: &mut Session) {
    let name = spelled_key(s.band.key, s.band.scale == 1);
    match start_key(s).and_then(|(i, _)| s.key_signatures.get_mut(i)) {
        Some(entry) => entry.1 = name,
        None => {
            s.key_signatures.push((0, name));
            s.key_signatures.sort_by_key(|k| k.0);
        }
    }
}

/// A session as it opens: a key change at the start wins over the band's key; with none, a band
/// key other than the default is written onto the ruler.
pub fn on_load(s: &mut Session) {
    match start_key(s) {
        Some((_, (key, scale))) => {
            s.band.key = key;
            s.band.scale = scale;
        }
        None => {
            let d = BandState::default();
            if (s.band.key, s.band.scale) != (d.key, d.scale) {
                write_band_key(s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spelling_matches_the_key_picker() {
        assert_eq!(spelled_key(10, false), "Bb major");
        assert_eq!(spelled_key(8, true), "G# minor");
        assert_eq!(parse_key(&spelled_key(3, true)), Some((3, 1)));
    }

    #[test]
    fn the_ruler_at_bar_one_and_the_band_key_follow_each_other() {
        let mut s = Session::default();
        // A key change at the start: the band follows.
        let before = s.clone();
        s.key_signatures.push((0, "D minor".into()));
        follow(&before, &mut s);
        assert_eq!((s.band.key, s.band.scale), (2, 1));
        // A later key change alone leaves the band's key.
        let before = s.clone();
        s.key_signatures.push((96_000, "F major".into()));
        follow(&before, &mut s);
        assert_eq!((s.band.key, s.band.scale), (2, 1));
        // The band's key changes: the ruler's first key change follows; the later one stays.
        let before = s.clone();
        s.band.key = 7;
        s.band.scale = 0;
        follow(&before, &mut s);
        assert_eq!(s.key_signatures, vec![(0, "G major".to_string()), (96_000, "F major".to_string())]);
    }

    #[test]
    fn a_band_key_with_no_ruler_key_is_written_at_the_start() {
        let mut s = Session::default();
        let before = s.clone();
        s.band.key = 4;
        follow(&before, &mut s);
        assert_eq!(s.key_signatures, vec![(0, "E major".to_string())]);
    }

    #[test]
    fn loading_makes_one_key() {
        // The ruler wins over a band key that disagrees.
        let mut s = Session::default();
        s.band.key = 9;
        s.key_signatures.push((0, "F major".into()));
        on_load(&mut s);
        assert_eq!((s.band.key, s.band.scale), (5, 0));
        // With no key change at the start, a band key of its own goes onto the ruler.
        let mut s = Session::default();
        s.band.key = 9;
        s.band.scale = 1;
        on_load(&mut s);
        assert_eq!(s.key_signatures, vec![(0, "A minor".to_string())]);
        // The default band key adds nothing.
        let mut s = Session::default();
        on_load(&mut s);
        assert!(s.key_signatures.is_empty());
    }
}
