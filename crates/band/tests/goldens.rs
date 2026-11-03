//! MIDI Studio's checked-in Classic goldens (`MS/Tests/golden/*.txt`, copied to `tests/golden/`): the band rendered
//! by `Tests/Harness.h renderBand` (512-sample blocks at 48 kHz, 120 bpm, `ppq += 512 / 48000.0 * bpm / 60.0`, then
//! one block with the transport stopped), one line per event: `ppq` (6 decimals) `status` `channel` `d1` `d2`.
//! MS's own rule: Classic feel stays byte-identical.

mod common;

use soundcraft_band::library::load::embedded_library;
use soundcraft_band::midi_types::{MidiEvent, MidiSink};
use soundcraft_band::runtime::{BandRuntime, BandSettings, HostTransport};
use soundcraft_band::song::{Song, SongChord, SongSection, build_timeline};

/// One block's events as the host's MidiBuffer holds them (stably sorted by sample position).
#[derive(Default)]
struct Block {
    events: Vec<MidiEvent>,
}

impl MidiSink for Block {
    fn event(&mut self, e: &MidiEvent) {
        self.events.push(*e);
    }
}

/// `renderBand (p, bars)` at 120 bpm, as text (`toText`).
fn render_band(s: BandSettings, bars: f64) -> String {
    let bpm = 120.0;
    let mut rt = BandRuntime::new(48000.0, s);
    let per = 512.0 / 48000.0 * bpm / 60.0;
    let mut text = String::new();
    let mut line = |ppq: f64, e: &MidiEvent| {
        text += &format!("{ppq:.6} {} {} {} {}\n", e.status & 0xf0, (e.status & 15) + 1, e.d1, e.d2);
    };
    let mut ppq = 0.0f64;
    while ppq < bars * 4.0 {
        let mut b = Block::default();
        rt.process_block(&HostTransport { playing: true, bpm, ppq, num: 4, den: 4 }, 512, &mut b);
        b.events.sort_by_key(|e| e.offset);
        for e in &b.events {
            line(ppq + f64::from(e.offset) / 48000.0 * bpm / 60.0, e);
        }
        ppq += per;
    }
    let mut b = Block::default();
    rt.process_block(&HostTransport { playing: false, bpm, ppq, num: 4, den: 4 }, 512, &mut b);
    b.events.sort_by_key(|e| e.offset);
    for e in &b.events {
        line(ppq, e);
    }
    text
}

fn chord(slot: i32, bass: i32, length: i32) -> SongChord {
    SongChord { slot, bass, length }
}

/// A B A: Verse (`a`) and Chorus (`b`).
fn verse_chorus(a: Vec<SongChord>, b: Vec<SongChord>) -> Song {
    let a = SongSection { id: 'A', name: "Verse".into(), chords: a, ..SongSection::default() };
    let b = SongSection { id: 'B', name: "Chorus".into(), chords: b, ..SongSection::default() };
    Song { sections: vec![a, b], arrangement: vec!['A', 'B', 'A'], ..Song::default() }
}

fn verse() -> Vec<SongChord> {
    vec![chord(0, -1, 2), chord(80, -1, 2), chord(48, -1, 2), chord(64, -1, 2), chord(0, -1, 2), chord(80, -1, 2), chord(48, -1, 2), chord(64, 11, 2)]
}

/// feeltest.cpp's own `twoSections`: Verse (8 chords) and Chorus (4), arranged A B A.
fn two_sections_feeltest() -> Song {
    verse_chorus(verse(), vec![chord(48, -1, 2), chord(64, -1, 2), chord(0, -1, 2), chord(80, -1, 2)])
}

/// `Tests/Harness.h Songs::twoSections` (expressiontest, fiddletest): Verse (8 chords) and Chorus (5), A B A.
fn two_sections_harness() -> Song {
    verse_chorus(verse(), vec![chord(48, -1, 2), chord(64, -1, 1), chord(0, -1, 1), chord(0, -1, 2), chord(80, -1, 2)])
}

fn golden(name: &str) -> String {
    std::fs::read_to_string(common::crate_dir().join("tests").join("golden").join(format!("{name}.txt"))).unwrap_or_default().replace("\r\n", "\n")
}

/// The render equals `tests/golden/<name>.txt` byte for byte (CRLF normalized, as MS compares).
fn assert_golden(name: &str, got: &str, what: &str) {
    let want = golden(name);
    assert!(!want.is_empty(), "tests/golden/{name}.txt is missing");
    let (w, g): (Vec<&str>, Vec<&str>) = (want.lines().collect(), got.lines().collect());
    let first = w.iter().zip(&g).position(|(a, b)| a != b);
    assert!(
        first.is_none() && w.len() == g.len() && want == got,
        "{what}: {} golden lines vs {}, first difference at {:?}",
        w.len(),
        g.len(),
        first.map(|i| (i, w[i], g[i]))
    );
}

fn inst(s: &BandSettings, k: usize) -> i32 {
    s.param(&format!("m{}_instrument", k + 1)).map_or(-1, |v| v.round() as i32)
}

/// `MidiStudioProcessor p;` in the MS tests. Its constructor differs from the offline clone's
/// ([`BandSettings::processor_default`]) only in `migrateSetups` and `applyDefaultBand`, which do nothing there: the
/// suites point `MIDISTUDIO_SETUPS` at an empty folder (or leave `MIDISTUDIO_LIBRARY` set without it, so
/// `dataRootsConsistent` refuses), and the Library has no `Presets/default-band.json`. The timer only empties kept
/// takes, and `actsNow()` holds on the test's message thread as it does for the clone.
fn ms_processor() -> BandSettings {
    BandSettings::processor_default(embedded_library())
}

/// feeltest.cpp `classicGuard`: the factory band, twoSections, Natural and Tension off, 20 bars.
#[test]
fn classic_factory() {
    let mut s = ms_processor();
    s.song = Some(build_timeline(&two_sections_feeltest()));
    s.feel.natural = false;
    s.feel.tension = false; // Classic as before: no Band feel switch on
    assert_golden("classic-factory", &render_band(s, 20.0), "Classic feel no longer plays exactly as before (factory band, 20 bars)");
}

/// expressiontest.cpp `classicGuard`: the factory band on the real targets (`onRealTargets`: drums on MODO DRUM, bass
/// on MODO BASS 2, piano / keys on Pianoteq), Harness twoSections, Natural and Tension off, 20 bars.
#[test]
fn classic_targets() {
    let mut s = ms_processor();
    s.song = Some(build_timeline(&two_sections_harness()));
    let mut set = 0;
    for k in 0..6 {
        let i = inst(&s, k);
        let t = match i {
            5 => "MODO DRUM",
            2 => "MODO BASS 2",
            0 | 3 => "Pianoteq",
            _ => continue,
        };
        set += usize::from(s.set_target(k, t));
    }
    assert!(set >= 3, "the factory band should go onto MODO DRUM, MODO BASS 2 and Pianoteq ({set} set)");
    s.feel.natural = false;
    s.feel.tension = false; // Classic as before: no Band feel switch on
    assert_golden("classic-targets", &render_band(s, 20.0), "Classic no longer plays exactly as before on the real targets (20 bars)");
}

/// fiddletest.cpp `fiddleOnly`: Harness twoSections, Natural on or off, the last Strings musician (else musician 6
/// made Strings) alone with `player` on The Fiddle, Energy, Take. Returns that musician.
fn fiddle_only(s: &mut BandSettings, natural: bool, player: &str, energy: i32, take: i32) -> usize {
    s.song = Some(build_timeline(&two_sections_harness()));
    s.feel.natural = natural;
    let mut fk = None;
    for k in 0..6 {
        if inst(s, k) == 7 {
            fk = Some(k);
        }
    }
    let fk = fk.unwrap_or_else(|| {
        s.set_param("m6_instrument", 7.0);
        5
    });
    for k in (0..6).filter(|&k| k != fk) {
        s.set_param(&format!("m{}_enabled", k + 1), 0.0);
    }
    s.set_player(fk, player);
    s.set_target(fk, "Indiginus The Fiddle");
    s.set_param(&format!("m{}_energy", fk + 1), energy as f32);
    s.set_param("take", take as f32);
    fk
}

/// fiddletest.cpp `embertoneTests`' Classic golden: `embOnly (p, false, "Ilse", 2)` (fiddleOnly, then the Embertone
/// target), 16 bars. fiddletest runs it on a Library copy whose Fiddle return mode is "unconfirmed"; The Fiddle is
/// replaced by Embertone before any block plays, so the shipped Library gives the same render.
#[test]
fn embertone_ilse_classic() {
    let mut s = ms_processor();
    let k = fiddle_only(&mut s, false, "Ilse", 2, 1);
    assert!(s.set_target(k, "Embertone Joshua Bell Violin"));
    assert_golden("embertone-ilse-classic", &render_band(s, 16.0), "Embertone Classic changed");
}

/// fiddletest.cpp `embertoneGuard`: Josie on Embertone, Natural, the first Strings musician (else musician 6 made
/// Strings) alone, Harness twoSections, 8 bars.
#[test]
fn fiddle_josie_embertone() {
    let mut s = ms_processor();
    s.song = Some(build_timeline(&two_sections_harness()));
    s.feel.natural = true;
    let k = (0..6).find(|&i| inst(&s, i) == 7).unwrap_or_else(|| {
        s.set_param("m6_instrument", 7.0);
        5
    });
    for i in (0..6).filter(|&i| i != k) {
        s.set_param(&format!("m{}_enabled", i + 1), 0.0);
    }
    assert!(s.set_player(k, "Josie"), "Josie set");
    assert!(s.set_target(k, "Embertone Joshua Bell Violin"), "Embertone target set");
    let text = render_band(s, 8.0);
    assert!(!text.is_empty(), "Josie on Embertone plays something");
    assert_golden("fiddle-josie-embertone", &text, "Josie on Embertone changed (fiddle block must not affect other targets)");
}

#[test]
fn golden_files_are_present() {
    for name in ["classic-factory", "classic-targets", "embertone-ilse-classic", "fiddle-josie-embertone"] {
        let g = golden(name);
        assert!(g.lines().count() > 100, "{name}");
        assert!(g.lines().all(|l| l.split(' ').count() == 5), "{name}: ppq status channel d1 d2");
    }
}
