//! Task 4: session tables, chord matching and MIDI chord import against MIDI Studio.

use std::path::PathBuf;

use soundcraft_band::chord_match::{match_chord, parse_chord_name, slot_pitch_classes};
use soundcraft_band::midi_chord_import::{preview_midi_import, song_from_preview};
use soundcraft_band::session_tables;
use soundcraft_midi::{Note, PPQ, Sequence, Smf, SmfTrack, write_smf};

fn fixture(name: &str) -> PathBuf {
    let root = std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from).unwrap_or_default();
    root.join("tests").join("fixtures").join(name)
}

/// The numbers of `inline constexpr <type> <name>[N] = { ... };` in the C++ header, as text tokens.
fn cpp_table(src: &str, name: &str) -> Vec<String> {
    let needle = format!(" {name}[");
    let Some(at) = src.find(&needle) else { return Vec::new() };
    let rest = &src[at..];
    let (Some(open), Some(close)) = (rest.find('{'), rest.find('}')) else { return Vec::new() };
    rest[open + 1..close].split(',').map(|t| t.trim().trim_end_matches('f').to_string()).filter(|t| !t.is_empty()).collect()
}

#[test]
fn session_tables_match_cpp() {
    let src = std::fs::read_to_string(fixture("SessionTables.h")).unwrap_or_default();
    assert!(!src.is_empty(), "tests/fixtures/SessionTables.h is missing");
    let ints: [(&str, &[i32]); 4] = [
        ("offsets", &session_tables::OFFSETS[..]),
        ("degrees", &session_tables::DEGREES[..]),
        ("velocities", &session_tables::VELOCITIES[..]),
        ("bases", &session_tables::BASES[..]),
    ];
    for (name, table) in ints {
        let want: Vec<i32> = cpp_table(&src, name).iter().map(|t| t.parse().unwrap_or(i32::MIN)).collect();
        assert_eq!(want.len(), table.len(), "{name} length");
        assert_eq!(want, table, "{name}");
    }
    let want: Vec<f32> = cpp_table(&src, "durations").iter().map(|t| t.parse().unwrap_or(f32::NAN)).collect();
    assert_eq!(want.len(), session_tables::DURATIONS.len(), "durations length");
    for (i, (w, h)) in want.iter().zip(session_tables::DURATIONS.iter()).enumerate() {
        assert_eq!(w.to_bits(), h.to_bits(), "durations[{i}]");
    }
    // Spot checks, by hand from the header.
    assert_eq!(session_tables::OFFSETS.len(), 2561);
    assert_eq!(session_tables::DEGREES.len(), 1337);
    assert_eq!(&session_tables::BASES[..], &[48, 60, 36, 60, 60, 0, 48, 48, 72, 60]);
    assert_eq!(&session_tables::DEGREES[..5], &[-7, 2, 7, 9, 11]);
    assert_eq!(&session_tables::DURATIONS[..3], &[7.0, 5.0, 3.0]);
    assert_eq!(&session_tables::VELOCITIES[..3], &[86, 71, 98]);
}

#[test]
fn typed_chords_match() {
    // C major: Am7 is degree 6 (index 5) with the seventh flavour (1): slot 81, exact.
    let am7 = parse_chord_name("Am7");
    assert!(am7.is_some());
    let m = am7.map(|c| match_chord(&c, 0, 0));
    assert_eq!(m.map(|m| (m.slot, m.bass, m.exact)), Some((5 * 16 + 1, -1, true)));

    // F/C: F (degree 4, slot 48) over C, the bass key-relative 0.
    let m = parse_chord_name("F/C").map(|c| match_chord(&c, 0, 0));
    assert_eq!(m.map(|m| (m.slot, m.bass, m.exact)), Some((48, 0, true)));

    // Bb in C major is the borrowed bVII (degree 7 of the parallel minor, flavour 7): exact.
    let m = parse_chord_name("Bb").map(|c| match_chord(&c, 0, 0));
    assert_eq!(m.map(|m| (m.slot, m.exact)), Some((6 * 16 + 7, true)));

    // F# major is not one of the key's chords: the nearest slot, not exact.
    let m = parse_chord_name("F#").map(|c| match_chord(&c, 0, 0));
    assert_eq!(m.map(|m| m.exact), Some(false));
    if let Some(m) = m {
        let pcs = slot_pitch_classes(m.slot, 0, 0);
        assert!(pcs[6] || pcs[1] || pcs[10], "the snapped chord shares a tone with F#: slot {}", m.slot);
    }

    // Unicode tokens: B♭Δ7 is Bbmaj7.
    let uni = parse_chord_name("B\u{266d}\u{394}7");
    assert_eq!(uni.map(|c| (c.root_pc, c.pcs)), parse_chord_name("Bbmaj7").map(|c| (c.root_pc, c.pcs)));

    assert!(parse_chord_name("").is_none());
    assert!(parse_chord_name("   ").is_none());
    assert!(parse_chord_name("H#13zz").is_none());
    assert!(parse_chord_name("C#b").is_none());
    assert!(parse_chord_name("C/").is_none());
    assert!(parse_chord_name("C maj").is_none());
    assert!(parse_chord_name("\u{266f}C").is_none());
    // "♭♯" reads as "b#": B sharp, C major (as in MS).
    assert_eq!(parse_chord_name("\u{266d}\u{266f}").map(|c| c.root_pc), Some(0));
}

#[test]
fn slot_pitch_classes_in_c_major() {
    let c = slot_pitch_classes(0, 0, 0);
    let on: Vec<usize> = (0..12).filter(|&i| c[i]).collect();
    assert_eq!(on, vec![0, 4, 7]);
    let am = slot_pitch_classes(80, 0, 0);
    let on: Vec<usize> = (0..12).filter(|&i| am[i]).collect();
    assert_eq!(on, vec![0, 4, 9]);
    // Hostile slots and keys never panic.
    let _ = slot_pitch_classes(-5, -100, 7);
    let _ = slot_pitch_classes(i32::MAX, i32::MIN, i32::MIN);
}

fn chord_file(chords: &[&[u8]]) -> Vec<u8> {
    let bar = i64::from(PPQ) * 4;
    let mut seq = Sequence::default();
    for (i, pitches) in chords.iter().enumerate() {
        for &p in pitches.iter() {
            seq.notes.push(Note { pitch: p, velocity: 100, release_velocity: 64, channel: 0, start: i as i64 * bar, length: bar });
        }
    }
    let smf = Smf {
        format: 1,
        tracks: vec![SmfTrack { name: "Chords".into(), sequence: seq }],
        tempos: vec![(0, 120.0)],
        meters: vec![(0, 4, 4)],
        markers: Vec::new(),
        key_sigs: Vec::new(),
        lyrics: Vec::new(),
    };
    write_smf(&smf).unwrap_or_default()
}

#[test]
fn midi_import_of_a_four_chord_file() {
    // C, Am, F, G as whole notes (a bar each = two half bars).
    let bytes = chord_file(&[&[48, 60, 64, 67], &[45, 57, 60, 64], &[41, 57, 60, 65], &[43, 55, 59, 62]]);
    let p = preview_midi_import(&bytes, None);
    assert!(p.is_ok(), "{:?}", p.as_ref().err());
    let Ok(p) = p else { return };
    assert_eq!((p.key, p.scale), (0, 0), "C major detected");
    assert_eq!(p.sections.len(), 1);
    let sec = &p.sections[0];
    assert_eq!(sec.name, "Part 1");
    let slots: Vec<i32> = sec.chords.iter().map(|c| c.slot).collect();
    assert_eq!(slots, vec![0, 80, 48, 64]);
    assert!(sec.chords.iter().all(|c| c.length == 2 && c.bass == -1));
    assert_eq!(sec.snapped, vec![false; 4]);

    let song = song_from_preview(&p);
    assert_eq!(song.sections.len(), 1);
    assert_eq!(song.sections[0].id, 'A');
    assert_eq!(song.arrangement, vec!['A']);
    assert_eq!(song.sections[0].chords.len(), 4);

    // A key override is used as given.
    let p2 = preview_midi_import(&bytes, Some((9, 1)));
    assert_eq!(p2.map(|p| (p.key, p.scale)), Ok((9, 1)));
}

#[test]
fn hostile_smf_is_an_error_not_a_panic() {
    assert!(preview_midi_import(&[], None).is_err());
    assert!(preview_midi_import(b"not a midi file at all", None).is_err());
    assert!(preview_midi_import(b"MThd\x00\x00\x00\x06\x00\x01\x00\x01\x01\xe0", None).is_err(), "no notes");
    // SMPTE time.
    let mut smpte = chord_file(&[&[60, 64, 67]]);
    if let Some(b) = smpte.get_mut(12) {
        *b = 0xE7;
    }
    if let Some(b) = smpte.get_mut(13) {
        *b = 40;
    }
    assert!(preview_midi_import(&smpte, None).is_err(), "SMPTE");
    // Drums only (channel 10) have no chords.
    let mut seq = Sequence::default();
    seq.notes.push(Note { pitch: 36, velocity: 100, release_velocity: 64, channel: 9, start: 0, length: 960 });
    let drums = write_smf(&Smf { format: 1, tracks: vec![SmfTrack { name: String::new(), sequence: seq }], ..Smf::default() }).unwrap_or_default();
    assert!(preview_midi_import(&drums, None).is_err());
    // Chords that start very far in (drum notes walk the time out): bounded work, no huge allocation.
    let step = 0x0F00_0000i64;
    let mut seq = Sequence::default();
    for k in 1..=1000 {
        seq.notes.push(Note { pitch: 36, velocity: 100, release_velocity: 64, channel: 9, start: k * step, length: 1 });
    }
    for p in [60u8, 64, 67] {
        seq.notes.push(Note { pitch: p, velocity: 100, release_velocity: 64, channel: 0, start: 1000 * step, length: 3840 });
    }
    let far = write_smf(&Smf { format: 1, tracks: vec![SmfTrack { name: String::new(), sequence: seq }], ..Smf::default() });
    assert!(far.is_ok());
    let far = preview_midi_import(&far.unwrap_or_default(), Some((0, 0)));
    assert_eq!(far.map(|p| p.sections.first().map(|s| s.chords.iter().map(|c| (c.slot, c.length)).collect::<Vec<_>>())), Ok(Some(vec![(0, 2)])));
    // Every single-byte corruption of a valid file: Ok or Err, never a panic.
    let good = chord_file(&[&[48, 60, 64, 67], &[45, 57, 60, 64]]);
    for i in 0..good.len() {
        for v in [0x00u8, 0x7F, 0x80, 0xFF] {
            let mut bad = good.clone();
            bad[i] = v;
            let _ = preview_midi_import(&bad, None);
            let _ = preview_midi_import(&bad[..i], None);
        }
    }
}

#[test]
fn voice_helpers_smoke() {
    use soundcraft_band::driver_kit::{DriverCtx, K_BELOW_RANGE, tap_key};
    use soundcraft_band::guitar_shapes::{best, notes};
    use soundcraft_band::plucked_voice::{StrumNote, strum_plan};

    // C major in standard tuning: an open-position shape, C lowest, only C E G.
    let open = [40, 45, 50, 55, 59, 64];
    let s = best(0b0000_1001_0001, 0b0000_0001_0001, 0, 0, &open, None, 0.0);
    assert!(s.valid);
    assert!(s.position() <= 3, "{:?}", s.fret);
    let ns = notes(&s, &open, 0);
    let sounding: Vec<i32> = ns.iter().copied().filter(|&n| n >= 0).collect();
    assert_eq!(sounding.first().map(|n| n % 12), Some(0));
    assert!(sounding.iter().all(|n| [0, 4, 7].contains(&(n % 12))));

    // A down strum keeps every string, spread over strum_ms, velocity -4 % per string.
    let mut out = [StrumNote::default(); 8];
    let n = strum_plan(&[60, 52, 48, 55, 64], 5, true, 20, 3, 0.8, 100, 2, 0.5, true, &mut out);
    assert_eq!(n, 5);
    assert_eq!(out[0], StrumNote { pitch: 48, offset_ms: 0.0, velocity: 100 });
    assert_eq!(out[4].pitch, 64);
    assert!((out[4].offset_ms - 20.0).abs() < 1e-9);
    assert_eq!(out[4].velocity, 84);

    // A key tap goes through the scheduler with the below-range flag, its length in ppq.
    let mut sent = Vec::new();
    let mut sink = |kind: i32, d1: i32, d2: i32, at: f64, dur: f32, flags: i32| sent.push((kind, d1, d2, at, dur, flags));
    let mut c = DriverCtx { sink: &mut sink, now: 0.0, ms: 0.002 };
    tap_key(&mut c, 24, 100, 1.0, 5.0);
    assert_eq!(sent, vec![(0, 24, 100, 1.0, 0.01f32, K_BELOW_RANGE)]);
}

#[test]
fn ample_slots_share_past_twelve() {
    use soundcraft_band::ample_slots::{
        AmpleSlotTable, AmpleSongChords, ample_chord_key_of_slot, ample_slot_for, build_ample_slots, collect_ample_chords,
    };
    use soundcraft_band::song::{SongChord, SongSection};

    // C with flavours 0..8 (9 chords) and G with 0, 1, 3..6 (6: G's diatonic 7th is the dominant 7th, flavour 2):
    // 15 distinct chords, 9 rooted on C then 6 on G once sorted.
    let mut sec = SongSection::default();
    for (degree, flavors) in [(0, &[0, 1, 2, 3, 4, 5, 6, 7, 8][..]), (4, &[0, 1, 3, 4, 5, 6][..])] {
        for &flavor in flavors {
            sec.chords.push(SongChord { slot: degree * 16 + flavor, bass: -1, length: 2 });
        }
    }
    sec.chords.push(SongChord { slot: 0, bass: -1, length: 2 }); // a repeat counts once
    let mut chords = AmpleSongChords::default();
    collect_ample_chords(std::slice::from_ref(&sec), &mut chords);
    assert_eq!(chords.n, 15);
    let mut t = AmpleSlotTable::default();
    build_ample_slots(&chords, 0, 0, &mut t);
    assert_eq!(t.n, 15);
    assert!(t.key[..9].iter().all(|k| k.root == 0) && t.key[9..15].iter().all(|k| k.root == 7));
    assert_eq!(&t.slot[..15], &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 10, 10, 10]);
    assert!(t.shared[9] && t.shared[12] && t.shared[13] && t.shared[14]);
    assert!(!t.shared[0] && !t.shared[10] && !t.shared[11]);
    // A chord the song doesn't have: its root's first slot, else 12.
    assert_eq!(ample_slot_for(&t, &ample_chord_key_of_slot(4 * 16 + 7, -1, 0, 0)), 10);
    assert_eq!(ample_slot_for(&t, &ample_chord_key_of_slot(16, -1, 0, 0)), 12);
    assert_eq!(ample_slot_for(&t, &t.key[14]), 10);
}

#[test]
fn return_mode_after_drop() {
    use soundcraft_band::driver_kit::{DriverCtx, ReturnMode};

    let mut sink = |_: i32, _: i32, _: i32, _: f64, _: f32, _: i32| {};
    let mut c = DriverCtx { sink: &mut sink, now: 0.0, ms: 0.002 };

    // The latch press itself was dropped: nothing is latched any more.
    let mut r = ReturnMode::default();
    r.latch(&mut c, 60, 1.0, 2.0);
    r.after_drop(true, false, 1.5);
    assert_eq!((r.latched, r.latch_at, r.due), (-1, -1.0, -1.0));

    // The press went out, nothing queued was dropped: unchanged.
    let mut r = ReturnMode::default();
    r.latch(&mut c, 60, 1.0, 2.0);
    r.after_drop(false, false, 1.5);
    assert_eq!((r.latched, r.latch_at, r.due), (60, 1.0, 2.0));

    // The return was dropped: the key is still in effect, its return due at the cut.
    let mut r = ReturnMode::default();
    r.latch(&mut c, 60, 1.0, 2.0);
    r.fire(&mut c, 2.0, ReturnMode::SAME, -1);
    assert_eq!((r.latched, r.ret_key), (-1, 60));
    r.after_drop(false, true, 1.5);
    assert_eq!((r.latched, r.latch_at, r.due, r.ret_key), (60, -1.0, 1.5, -1));
}
