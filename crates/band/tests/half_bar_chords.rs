//! Half-bar chords are voiced (spec `docs/superpowers/specs/2026-10-28-band-picker-chords.md` §2): with chords every
//! half bar, every chord slot gets at least one chordal note-on from every keys persona and every strummed (guitar)
//! persona, over 8 bars, at 109 bpm and at 60 bpm.

use std::collections::BTreeSet;

use soundcraft_band::library::{LibraryData, load::embedded_library};
use soundcraft_band::midi_types::{MidiEvent, MidiSink};
use soundcraft_band::runtime::{BandRuntime, BandSettings, HostTransport};
use soundcraft_band::song::{Song, SongChord, SongSection, build_timeline};

#[derive(Default)]
struct Notes {
    /// (ppq, pitch) of every note-on
    ons: Vec<(f64, i32)>,
}

struct Block<'a> {
    ppq: f64,
    bpm: f64,
    out: &'a mut Notes,
}

impl MidiSink for Block<'_> {
    fn event(&mut self, e: &MidiEvent) {
        if e.status & 0xf0 == 0x90 && e.d2 > 0 {
            let at = self.ppq + f64::from(e.offset) / 48000.0 * self.bpm / 60.0;
            self.out.ons.push((at, i32::from(e.d1)));
        }
    }
}

/// C, Am, F, G as half-bar chords, twice: eight chords in two bars, looping.
fn half_bar_song(length: i32) -> Song {
    let chords: Vec<SongChord> = [0, 80, 48, 64, 0, 80, 48, 64].iter().map(|&slot| SongChord { slot, bass: -1, length }).collect();
    let a = SongSection { id: 'A', name: "Verse".into(), chords, ..SongSection::default() };
    Song { sections: vec![a], arrangement: vec!['A'], ..Song::default() }
}

/// Pitch classes of a chord slot in C major (triads and sevenths of the key).
fn chord_pcs(slot: i32) -> BTreeSet<i32> {
    let pcs = soundcraft_band::chord_match::slot_pitch_classes(slot, 0, 0);
    (0..12).filter(|&i| pcs.get(i as usize).copied().unwrap_or(false)).collect()
}

fn render(lib: &std::sync::Arc<LibraryData>, instrument: i32, player: &str, bpm: f64, bars: f64, len_half: i32) -> Notes {
    let mut s = BandSettings::processor_default(lib.clone());
    s.song = Some(build_timeline(&half_bar_song(len_half)));
    for k in 0..6 {
        s.set_param(&format!("m{}_enabled", k + 1), if k == 0 { 1.0 } else { 0.0 });
    }
    s.set_param("m1_instrument", instrument as f32);
    // As the app's musician slots default (the engine's own defaults are Half speed and Energy 1).
    s.set_param("m1_speed", 1.0);
    s.set_param("m1_energy", 3.0);
    assert!(s.set_player(0, player), "{player} plays instrument {instrument}");
    let mut rt = BandRuntime::new(48000.0, s);
    let per = 512.0 / 48000.0 * bpm / 60.0;
    let mut notes = Notes::default();
    let mut ppq = 0.0f64;
    while ppq < bars * 4.0 {
        rt.process_block(&HostTransport { playing: true, bpm, ppq, num: 4, den: 4 }, 512, &mut Block { ppq, bpm, out: &mut notes });
        ppq += per;
    }
    notes
}

/// The chord slots (half bars) of `bars` bars with no chordal note-on, as "bar.half".
fn silent_slots(notes: &Notes, bars: i32) -> Vec<String> {
    let slots = [0, 80, 48, 64];
    let mut silent = Vec::new();
    for h in 0..bars * 2 {
        let (from, to) = (f64::from(h) * 2.0, f64::from(h + 1) * 2.0);
        let pcs = chord_pcs(slots[(h % 4) as usize]);
        // A push strikes the next chord an 8th early (and Natural timing a little earlier still): a chord tone in the
        // window is this chord's wherever it lands in it.
        let any = notes.ons.iter().any(|(at, p)| *at >= from - 0.7 && *at < to - 1.0e-6 && pcs.contains(&(p.rem_euclid(12))));
        if !any {
            let near: Vec<String> = notes.ons.iter().filter(|(at, _)| *at >= from - 0.7 && *at < to).map(|(at, p)| format!("{p}@{at:.2}")).collect();
            silent.push(format!("{}.{} [{}]", h / 2 + 1, h % 2 + 1, near.join(" ")));
        }
    }
    silent
}

/// Every keys and every guitar persona, with the first instrument it lists.
fn chordal_personas(lib: &LibraryData) -> Vec<(String, i32)> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for seeds in &lib.players {
        for &seed in seeds {
            let Some(p) = lib.persona(seed) else { continue };
            // 12-step-only players (waltz, jig) sit 4/4 out.
            if (p.role == 0 || p.role == 3) && !p.only12 && seen.insert(seed) {
                let inst = p.slots.first().map_or(0, |s| s.0);
                out.push((p.name.clone(), inst));
            }
        }
    }
    out
}

#[test]
fn every_half_bar_chord_is_voiced_by_every_chordal_persona() {
    let lib = embedded_library();
    let personas = chordal_personas(&lib);
    assert!(personas.len() >= 50, "{} chordal personas", personas.len());
    let mut failures = Vec::new();
    for bpm in [109.0, 60.0] {
        for (name, inst) in &personas {
            let notes = render(&lib, *inst, name, bpm, 8.0, 1);
            let silent = silent_slots(&notes, 8);
            if !silent.is_empty() {
                failures.push(format!("{name} (instrument {inst}) at {bpm} bpm: no chord tone in {}", silent.join(" ")));
            }
        }
    }
    assert!(failures.is_empty(), "{} silent chord slots:\n{}", failures.len(), failures.join("\n"));
}

/// `BAND_DUMP=<player> BAND_INST=<instrument> [BAND_LEN=1|2] cargo test -p soundcraft-band --test half_bar_chords
/// dump_one -- --ignored --nocapture`: one player's note-ons over four bars at 109 bpm.
#[test]
#[ignore]
fn dump_one_persona() {
    let lib = embedded_library();
    let name = std::env::var("BAND_DUMP").unwrap_or_default();
    let inst: i32 = std::env::var("BAND_INST").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let len_half: i32 = std::env::var("BAND_LEN").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let notes = render(&lib, inst, &name, 109.0, 4.0, len_half);
    for (at, p) in &notes.ons {
        println!("{at:8.3} {p}");
    }
}
