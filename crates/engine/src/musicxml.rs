//! MusicXML import (File › Import › MusicXML…): a partwise score's parts become instrument tracks
//! with one MIDI clip each, its meters and tempo marks go on the tempo map and its key signatures on
//! the Key ruler, and sung words become the clips' lyrics.
//!
//! The score is read into a Standard MIDI File in model terms ([`to_smf`]) and placed by the MIDI
//! import ([`crate::io::import_midi_bytes`]), so it lands as a `.mid` file would.
//!
//! What is read: `divisions`, `note` (pitch, `alter`, `octave`, `duration`, `chord`, `rest`, ties,
//! `voice` through `backup` and `forward`), `unpitched` notes through the part's
//! `midi-unpitched` numbers (a part whose notes all have one plays on the drum channel), grace and
//! cue notes left out, `time`, `key`, `sound tempo`, `lyric` text, part names. Dynamics,
//! articulations, repeats and `score-timewise` documents are not read. The input is hostile:
//! sizes are capped and every number is checked.

use crate::dawproject::xml::Element;
use soundcraft_midi::{Note, PPQ, Sequence, Smf, SmfTrack};

/// The most notes a score may bring in.
const MAX_NOTES: usize = 500_000;
/// The furthest tick a note may start or end on (about 1000 hours at 120 bpm).
const MAX_TICK: i64 = 1 << 40;

/// A key signature change: (tick, sharps (+) / flats (-), minor).
pub type KeyChange = (i64, i32, bool);

/// What a score holds, in model terms.
#[derive(Debug, Default)]
pub struct Score {
    pub smf: Smf,
    pub keys: Vec<KeyChange>,
}

fn num(e: Option<&Element>) -> Option<f64> {
    e.and_then(|e| e.text.trim().parse::<f64>().ok()).filter(|v| v.is_finite())
}

/// The MIDI pitch of a written note, if it is one.
fn pitch_of(p: &Element) -> Option<u8> {
    let step = match p.child("step")?.text.trim() {
        "C" => 0,
        "D" => 2,
        "E" => 4,
        "F" => 5,
        "G" => 7,
        "A" => 9,
        "B" => 11,
        _ => return None,
    };
    let alter = num(p.child("alter")).unwrap_or(0.0).round().clamp(-3.0, 3.0) as i32;
    let octave = num(p.child("octave"))?.round().clamp(-2.0, 10.0) as i32;
    u8::try_from((octave + 1) * 12 + step + alter).ok().filter(|v| *v <= 127)
}

/// A key's name as the Key ruler reads it ([`crate::vocal::parse_key`]).
pub fn key_name(fifths: i32, minor: bool) -> String {
    const MAJOR: [&str; 15] = ["Cb", "Gb", "Db", "Ab", "Eb", "Bb", "F", "C", "G", "D", "A", "E", "B", "F#", "C#"];
    const MINOR: [&str; 15] = ["Ab", "Eb", "Bb", "F", "C", "G", "D", "A", "E", "B", "F#", "C#", "G#", "D#", "A#"];
    let i = (fifths.clamp(-7, 7) + 7) as usize;
    let root = if minor { MINOR[i] } else { MAJOR[i] };
    format!("{root} {}", if minor { "minor" } else { "major" })
}

/// Reads a partwise MusicXML document.
pub fn to_smf(text: &str) -> Result<Score, String> {
    let root = Element::parse(text)?;
    if root.name == "score-timewise" {
        return Err("MusicXML: a timewise score is not read; save it as partwise".into());
    }
    if root.name != "score-partwise" {
        return Err(format!("MusicXML: expected a score, found <{}>", root.name));
    }
    let names: Vec<(String, String)> = root
        .child("part-list")
        .map(|pl| {
            pl.children_named("score-part")
                .map(|sp| (sp.attr("id").unwrap_or("").to_string(), sp.child("part-name").map_or(String::new(), |n| n.text.clone())))
                .collect()
        })
        .unwrap_or_default();
    let mut score = Score::default();
    score.smf.format = 1;
    let mut notes_total = 0usize;
    for (pi, part) in root.children_named("part").enumerate() {
        let id = part.attr("id").unwrap_or("");
        let mut name = names.iter().find(|(i, _)| i == id).map(|(_, n)| n.trim().to_string()).unwrap_or_default();
        if name.is_empty() {
            name = format!("Part {}", pi + 1);
        }
        // The part's percussion: instrument id to MIDI note (`midi-unpitched` counts from 1).
        let mut unpitched: Vec<(String, u8)> = Vec::new();
        if let Some(sp) = root.child("part-list").and_then(|pl| pl.children_named("score-part").find(|sp| sp.attr("id") == Some(id))) {
            for mi in sp.children_named("midi-instrument") {
                if let Some(n) = num(mi.child("midi-unpitched")).filter(|n| (1.0..=128.0).contains(n)) {
                    unpitched.push((mi.attr("id").unwrap_or("").to_string(), (n as u8).saturating_sub(1)));
                }
            }
        }
        let first = pi == 0;
        let mut divisions = 1.0f64;
        // Position in quarter notes (exact for any division the file uses, rounded to ticks).
        let mut pos = 0.0f64;
        let mut measure_start = 0.0f64;
        let mut last_start = 0.0f64;
        let mut seq = Sequence::default();
        let mut drum_notes = 0usize;
        let mut lyrics: Vec<(i64, String)> = Vec::new();
        let tick = |q: f64| -> Option<i64> {
            Some((q * PPQ as f64).round()).filter(|t| t.is_finite() && (0.0..MAX_TICK as f64).contains(t)).map(|t| t as i64)
        };
        for measure in part.children_named("measure") {
            let mut longest = 0.0f64;
            for el in &measure.children {
                match el.name.as_str() {
                    "attributes" => {
                        if let Some(d) = num(el.child("divisions")).filter(|d| *d >= 1.0 && *d <= 1e6) {
                            divisions = d;
                        }
                        if first {
                            let at = tick(pos).ok_or("MusicXML: the score is too long")?;
                            if let Some(t) = el.child("time") {
                                let beats = num(t.child("beats")).filter(|b| (1.0..=64.0).contains(b));
                                let kind = num(t.child("beat-type")).filter(|b| [1.0, 2.0, 4.0, 8.0, 16.0, 32.0].contains(b));
                                if let (Some(b), Some(k)) = (beats, kind) {
                                    score.smf.meters.push((at, b as u8, k as u8));
                                }
                            }
                            if let Some(k) = el.child("key")
                                && let Some(f) = num(k.child("fifths")).filter(|f| (-7.0..=7.0).contains(f))
                            {
                                let minor = k.child("mode").is_some_and(|m| m.text.trim().eq_ignore_ascii_case("minor"));
                                score.keys.push((at, f as i32, minor));
                            }
                        }
                    }
                    "direction" | "sound" if first => {
                        let sound = if el.name == "sound" { Some(el) } else { el.child("sound") };
                        if let Some(bpm) = sound.and_then(|s| s.num("tempo")).filter(|b| (1.0..=999.0).contains(b)) {
                            score.smf.tempos.push((tick(pos).ok_or("MusicXML: the score is too long")?, bpm));
                        }
                    }
                    "backup" => pos = (pos - num(el.child("duration")).unwrap_or(0.0).max(0.0) / divisions).max(measure_start),
                    "forward" => pos += num(el.child("duration")).unwrap_or(0.0).max(0.0) / divisions,
                    "note" => {
                        if el.child("grace").is_some() || el.child("cue").is_some() {
                            continue;
                        }
                        let dur = num(el.child("duration")).unwrap_or(0.0).max(0.0) / divisions;
                        let chord = el.child("chord").is_some();
                        let start = if chord { last_start } else { pos };
                        if !chord {
                            last_start = pos;
                            pos += dur;
                        }
                        if el.child("rest").is_some() {
                            continue;
                        }
                        let pitch = if let Some(p) = el.child("pitch") {
                            pitch_of(p)
                        } else if el.child("unpitched").is_some() {
                            let inst = el.child("instrument").and_then(|i| i.attr("id"));
                            let n = unpitched.iter().find(|(i, _)| Some(i.as_str()) == inst).or(unpitched.first()).map(|(_, n)| *n);
                            if n.is_some() {
                                drum_notes += 1;
                            }
                            n
                        } else {
                            None
                        };
                        let (Some(pitch), Some(a), Some(b)) = (pitch, tick(start), tick(start + dur)) else { continue };
                        // A tied note's continuation lengthens the note it is tied from.
                        let tie_stop = el.children_named("tie").any(|t| t.attr("type") == Some("stop"));
                        if tie_stop && let Some(prev) = seq.notes.iter_mut().rev().find(|n| n.pitch == pitch && (n.end() - a).abs() <= 1) {
                            prev.length = (b - prev.start).max(1);
                            continue;
                        }
                        if b <= a {
                            continue;
                        }
                        notes_total += 1;
                        if notes_total > MAX_NOTES {
                            return Err(format!("MusicXML: more than {MAX_NOTES} notes"));
                        }
                        seq.notes.push(Note {
                            probability: 100,
                            pitch,
                            velocity: 90,
                            muted: false,
                            release_velocity: 64,
                            channel: 0,
                            start: a,
                            length: b - a,
                        });
                        if let Some(text) = el
                            .children_named("lyric")
                            .next()
                            .and_then(|l| l.child("text"))
                            .map(|t| t.text.trim().to_string())
                            .filter(|t| !t.is_empty())
                        {
                            lyrics.push((a, text));
                        }
                    }
                    _ => {}
                }
                longest = longest.max(pos - measure_start);
            }
            // The next measure starts after the furthest any voice reached in this one.
            pos = measure_start + longest;
            measure_start = pos;
        }
        if drum_notes > 0 && drum_notes == seq.notes.len() {
            for n in &mut seq.notes {
                n.channel = 9;
            }
        }
        seq.sort();
        let ti = score.smf.tracks.len();
        score.smf.lyrics.extend(lyrics.into_iter().map(|(t, w)| (ti, t, w)));
        score.smf.tracks.push(SmfTrack { name, sequence: seq });
    }
    if score.smf.tracks.iter().all(|t| t.sequence.notes.is_empty()) {
        return Err("MusicXML: the score has no notes".into());
    }
    Ok(score)
}

/// Imports a MusicXML document at `at`: a track per part, the tempo map and the Key ruler set
/// from where it lands. Returns the new tracks.
pub fn import(e: &mut crate::Engine, text: &str, at: soundcraft_time::Samples) -> crate::Result<Vec<soundcraft_model::TrackId>> {
    let score = to_smf(text).map_err(crate::EngineError::Io)?;
    let bytes = soundcraft_midi::write_smf(&score.smf).map_err(|err| crate::EngineError::Io(err.to_string()))?;
    let base = {
        let s = e.session();
        s.tempo.samples_to_ticks(at, s.sample_rate)
    };
    let tracks = crate::io::import_midi_bytes(e, &bytes, at, true)?;
    let s = e.session_mut();
    for (tick, fifths, minor) in score.keys.iter().take(1000) {
        let pos = s.tempo.tick_to_samples(base.saturating_add(*tick), s.sample_rate);
        s.key_signatures.retain(|(t, _)| *t != pos);
        s.key_signatures.push((pos, key_name(*fifths, *minor)));
    }
    s.key_signatures.sort_by_key(|k| k.0);
    Ok(tracks)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE score-partwise PUBLIC "-//Recordare//DTD MusicXML 4.0 Partwise//EN" "http://www.musicxml.org/dtds/partwise.dtd">
<score-partwise version="4.0">
  <part-list>
    <score-part id="P1"><part-name>Flute</part-name></score-part>
    <score-part id="P2"><part-name>Drums</part-name>
      <score-instrument id="P2-I36"><instrument-name>Kick</instrument-name></score-instrument>
      <midi-instrument id="P2-I36"><midi-channel>10</midi-channel><midi-unpitched>37</midi-unpitched></midi-instrument>
    </score-part>
  </part-list>
  <part id="P1">
    <measure number="1">
      <attributes><divisions>2</divisions><key><fifths>-1</fifths><mode>major</mode></key><time><beats>3</beats><beat-type>4</beat-type></time></attributes>
      <direction><sound tempo="90"/></direction>
      <note><pitch><step>B</step><alter>-1</alter><octave>4</octave></pitch><duration>2</duration><voice>1</voice><lyric><text>Hel</text></lyric></note>
      <note><pitch><step>D</step><octave>5</octave></pitch><duration>2</duration><voice>1</voice></note>
      <note><chord/><pitch><step>F</step><octave>5</octave></pitch><duration>2</duration><voice>1</voice></note>
      <note><pitch><step>C</step><octave>5</octave></pitch><duration>2</duration><tie type="start"/><voice>1</voice></note>
      <backup><duration>6</duration></backup>
      <note><rest/><duration>2</duration><voice>2</voice></note>
      <note><pitch><step>G</step><octave>3</octave></pitch><duration>4</duration><voice>2</voice></note>
    </measure>
    <measure number="2">
      <attributes><key><fifths>2</fifths></key></attributes>
      <note><pitch><step>C</step><octave>5</octave></pitch><duration>1</duration><tie type="stop"/><voice>1</voice></note>
      <note><grace/><pitch><step>E</step><octave>5</octave></pitch><voice>1</voice></note>
      <note><rest/><duration>5</duration><voice>1</voice></note>
    </measure>
  </part>
  <part id="P2">
    <measure number="1">
      <attributes><divisions>1</divisions></attributes>
      <note><unpitched><display-step>F</display-step><display-octave>4</display-octave></unpitched><duration>1</duration><instrument id="P2-I36"/></note>
      <note><rest/><duration>2</duration></note>
    </measure>
  </part>
</score-partwise>"#;

    /// Parity section 7: a MusicXML score reads into parts of notes (chords, ties, a second voice
    /// after a backup, grace notes left out), with its meter, tempo, key changes and lyrics; an
    /// unpitched part plays its drums on the drum channel.
    #[test]
    fn a_score_reads_into_parts_meters_keys_and_lyrics() {
        let s = to_smf(DOC).unwrap();
        assert_eq!(s.smf.tracks.len(), 2);
        assert_eq!(s.smf.tracks[0].name, "Flute");
        let notes: Vec<(i64, i64, u8)> = s.smf.tracks[0].sequence.notes.iter().map(|n| (n.start, n.length, n.pitch)).collect();
        // Quarters at 960 ticks: Bb4, D5+F5, C5 tied into bar 2 by an eighth; G3 in voice 2.
        assert_eq!(notes, vec![(0, 960, 70), (960, 1920, 55), (960, 960, 74), (960, 960, 77), (1920, 1440, 72)]);
        assert_eq!(s.smf.meters, vec![(0, 3, 4)]);
        assert_eq!(s.smf.tempos, vec![(0, 90.0)]);
        assert_eq!(s.keys, vec![(0, -1, false), (2880, 2, false)]);
        assert_eq!(s.smf.lyrics, vec![(0, 0, "Hel".to_string())]);
        let drums = &s.smf.tracks[1].sequence.notes;
        assert_eq!(drums.iter().map(|n| (n.pitch, n.channel)).collect::<Vec<_>>(), vec![(36, 9)]);
        assert_eq!(key_name(-1, false), "F major");
        assert_eq!(key_name(3, true), "F# minor");
        assert!(crate::vocal::parse_key(&key_name(-6, false)).is_some());
    }

    #[test]
    fn hostile_or_empty_scores_are_refused() {
        assert!(to_smf("<score-timewise/>").is_err());
        assert!(to_smf("<html/>").is_err());
        assert!(to_smf("<score-partwise><part id=\"P1\"><measure/></part></score-partwise>").is_err(), "no notes");
        assert!(to_smf("<score-partwise><part").is_err());
        let huge = "<score-partwise><part id=\"P1\"><measure><attributes><divisions>1</divisions></attributes><note><pitch><step>C</step><octave>4</octave></pitch><duration>1e300</duration></note></measure></part></score-partwise>";
        assert!(to_smf(huge).is_err() || to_smf(huge).is_ok_and(|s| s.smf.tracks[0].sequence.notes.is_empty()));
    }

    /// Imported through the engine: a track per part, the meter on the tempo map, the keys on the
    /// Key ruler and the words on the clip.
    #[test]
    fn importing_a_score_makes_tracks_and_sets_the_rulers() {
        let mut e = crate::Engine::default();
        let tracks = import(&mut e, DOC, 0).unwrap();
        assert_eq!(tracks.len(), 2);
        let s = e.session();
        assert_eq!(s.track(tracks[0]).unwrap().name, "Flute");
        assert_eq!(s.track(tracks[1]).unwrap().instrument.as_ref().unwrap().plugin, "drum_synth");
        assert_eq!(s.tempo.meter_at_tick(0).numerator, 3);
        assert_eq!(s.key_signatures.iter().map(|k| k.1.as_str()).collect::<Vec<_>>(), vec!["F major", "D major"]);
        let clip = &s.track(tracks[0]).unwrap().clips()[0];
        assert_eq!(clip.lyrics.first().map(|l| l.text()), Some("Hel".to_string()));
    }
}
