//! The lanes → `SongTimeline` adapter: factory songs come back as `build_timeline` makes them, and
//! any lanes at all give a timeline the musicians can read.

use soundcraft_band::lanes::{LaneBlock, LaneChord, LaneSong, SILENT_SECTION_ID, lanes_end_half, lanes_to_timeline, song_to_lanes};
use soundcraft_band::song::{Song, SongChord, SongSection, SongTimeline, build_timeline};
use soundcraft_band::song_file::factory_songs;

fn def(id: char) -> SongSection {
    SongSection { id, name: id.to_string(), ..SongSection::default() }
}

fn block(start_half: i32, len_half: i32, def: usize) -> LaneBlock {
    LaneBlock { start_half, len_half, def, energy: 0, take: 0 }
}

fn chord(start_half: i32, len_half: i32, slot: i32) -> LaneChord {
    LaneChord { start_half, len_half, slot, bass: -1 }
}

fn lens(t: &SongTimeline) -> Vec<i32> {
    t.entries.iter().map(|e| e.len_half).collect()
}

fn ids(t: &SongTimeline) -> String {
    (0..t.entries.len()).map(|i| t.section_id_of(i)).collect()
}

/// What every timeline must hold for the musicians: contiguous entries of legal lengths, each pass
/// contiguous with chord indices 0..n.
fn check_invariants(t: &SongTimeline) {
    let mut at = 0;
    for (i, e) in t.entries.iter().enumerate() {
        assert_eq!(e.start_half, at, "entry {i} starts where the last ended");
        assert!([1, 2, 4].contains(&e.len_half), "entry {i} has a legal length");
        assert!(usize::try_from(e.section).is_ok_and(|s| s < t.sections.len()), "entry {i} names a section");
        assert!(e.chord_index >= 0 && e.chord_index < e.section_chords);
        assert!((0..=111).contains(&e.slot) && (-1..=11).contains(&e.bass));
        assert!((0..t.arrangement_length).contains(&e.arr_pos));
        let first = i - e.chord_index as usize;
        let pass = &t.entries[first..first + e.section_chords as usize];
        assert!(pass.iter().all(|p| p.arr_pos == e.arr_pos && p.section_chords == e.section_chords));
        at += e.len_half;
    }
    assert_eq!(t.total_half, at);
    // Every pass number 0..arrangement_length is used by exactly one contiguous run.
    let mut runs: Vec<i32> = Vec::new();
    for e in &t.entries {
        if runs.last() != Some(&e.arr_pos) {
            runs.push(e.arr_pos);
        }
    }
    runs.sort_unstable();
    assert_eq!(runs, (0..t.arrangement_length).collect::<Vec<_>>(), "one run per pass");
}

#[test]
fn factory_songs_round_trip_to_identical_timelines() {
    let songs = factory_songs();
    assert_eq!(songs.len(), 5);
    for f in &songs {
        let want = build_timeline(&f.song);
        let lanes = song_to_lanes(&f.song, 0);
        assert!(lanes.defs.iter().all(|d| d.chords.is_empty()), "{}: defs carry no chords", f.name);
        let got = lanes_to_timeline(&lanes).unwrap_or_else(|| panic!("{}: a timeline", f.name));
        check_invariants(&got);
        assert_eq!(got.entries.len(), want.entries.len(), "{}", f.name);
        for (i, (g, w)) in got.entries.iter().zip(&want.entries).enumerate() {
            assert_eq!(g, w, "{}: entry {i}", f.name);
        }
        assert_eq!(got.total_half, want.total_half, "{}", f.name);
        assert_eq!(got.intro_half, want.intro_half, "{}", f.name);
        assert_eq!(got.has_ending, want.has_ending, "{}", f.name);
        assert_eq!(got.uniform_len_half, want.uniform_len_half, "{}", f.name);
        assert_eq!(got.arrangement_length, want.arrangement_length, "{}", f.name);
        assert_eq!(got.sections, want.sections, "{}", f.name);
        assert_eq!(*got, *want, "{}: field for field", f.name);
        assert_eq!(lanes_end_half(&lanes), want.total_half, "{}", f.name);
    }
}

#[test]
fn songs_laid_later_start_later() {
    let songs = factory_songs();
    let Some(blues) = songs.iter().find(|f| f.name == "12-bar Blues") else { panic!("the blues") };
    let lanes = song_to_lanes(&blues.song, 16);
    assert_eq!(lanes.blocks.first().map(|b| b.start_half), Some(16));
    assert_eq!(lanes.chords.first().map(|c| c.start_half), Some(16));
    assert_eq!(lanes_end_half(&lanes), 16 + 24);
    let t = lanes_to_timeline(&lanes).unwrap_or_else(|| panic!("a timeline"));
    check_invariants(&t);
    // The time before the song: the default section's own pass, with no chord.
    assert_eq!(t.arrangement_length, 2);
    assert_eq!(&ids(&t)[..4], "----");
    // The block is pass 0 (its index); the gap before it comes after the blocks.
    assert_eq!((t.entries[0].arr_pos, t.entries[4].arr_pos), (1, 0));
}

#[test]
fn chords_split_at_blocks_and_into_legal_lengths() {
    let l = LaneSong {
        defs: vec![def('A'), def('B')],
        blocks: vec![block(0, 4, 0), block(4, 4, 1)],
        chords: vec![chord(0, 6, 17)],
        ..LaneSong::default()
    };
    let t = lanes_to_timeline(&l).unwrap_or_else(|| panic!("a timeline"));
    check_invariants(&t);
    assert_eq!(lens(&t), [4, 2, 2]);
    assert_eq!(ids(&t), "AB-");
    assert_eq!(t.entries[1].slot, 17);
    assert_eq!((t.entries[1].chord_index, t.entries[1].section_chords, t.entries[1].arr_pos), (0, 2, 1));

    let l = LaneSong { defs: vec![def('A')], blocks: vec![block(0, 8, 0)], chords: vec![chord(0, 7, 5)], ..LaneSong::default() };
    let t = lanes_to_timeline(&l).unwrap_or_else(|| panic!("a timeline"));
    check_invariants(&t);
    assert_eq!(lens(&t), [4, 2, 1, 1]);
    assert_eq!(ids(&t), "AAA-");
    assert!(t.entries[..3].iter().all(|e| e.slot == 5));
    assert_eq!(t.uniform_len_half, 0);
}

#[test]
fn gaps_are_default_section_or_silent() {
    // B is the only block, at 4..8; chords at 0..2 and 10..12.
    let l = LaneSong {
        defs: vec![def('B'), def('A')],
        blocks: vec![block(4, 4, 0)],
        chords: vec![chord(0, 2, 3), chord(10, 2, 9)],
        ..LaneSong::default()
    };
    assert_eq!(lanes_end_half(&l), 12);
    let t = lanes_to_timeline(&l).unwrap_or_else(|| panic!("a timeline"));
    check_invariants(&t);
    assert_eq!(t.total_half, 12);
    assert_eq!(lens(&t), [2, 2, 4, 2, 2]);
    assert_eq!(ids(&t), "A---A");
    assert_eq!(t.arrangement_length, 3);
    assert_eq!(t.entries.iter().map(|e| e.arr_pos).collect::<Vec<_>>(), [1, 1, 0, 2, 2], "the block is pass 0, the gaps come after");
    assert_eq!((t.entries[0].slot, t.entries[4].slot), (3, 9));
    // Sections in A–H order, then the silent one: nobody plays in it.
    let secs: String = t.sections.iter().map(|s| s.id).collect();
    assert_eq!(secs, format!("AB{SILENT_SECTION_ID}"));
    assert!(t.sections[2].musicians.iter().all(|m| !m.plays));
    // The silent section is only there when it is used.
    let l = LaneSong { defs: vec![def('A')], chords: vec![chord(0, 4, 0)], ..LaneSong::default() };
    let t = lanes_to_timeline(&l).unwrap_or_else(|| panic!("a timeline"));
    assert_eq!(t.sections.len(), 1);
    assert_eq!(t.uniform_len_half, 4);
    // No definitions: the default section is a plain A.
    let l = LaneSong { chords: vec![chord(0, 2, 0)], ..LaneSong::default() };
    let t = lanes_to_timeline(&l).unwrap_or_else(|| panic!("a timeline"));
    assert_eq!(ids(&t), "A");
    // Nothing on the lanes: no timeline, the band stays silent.
    assert!(lanes_to_timeline(&LaneSong::default()).is_none());
    assert_eq!(lanes_end_half(&LaneSong::default()), 0);
}

#[test]
fn ending_block_truncates_like_build_timeline() {
    let a = SongSection { chords: vec![SongChord { slot: 0, bass: -1, length: 2 }; 2], ..def('A') };
    let b = SongSection { role: 2, chords: vec![SongChord { slot: 48, bass: 4, length: 4 }], ..def('B') };
    let intro = SongSection { role: 1, chords: vec![SongChord { slot: 80, bass: -1, length: 1 }], ..def('C') };
    let song = Song {
        sections: vec![a, b, intro],
        arrangement: vec!['C', 'A', 'B', 'A'],
        arrangement_energy: vec![0, 2, -1, 3],
        arrangement_take: vec![0, 5, 0, 0],
        ..Song::default()
    };
    let want = build_timeline(&song);
    assert!(want.has_ending);
    // The song on the lanes keeps the block after the Ending: the timeline drops it.
    let mut l = song_to_lanes(&song, 0);
    assert_eq!(l.blocks.len(), 3);
    l.blocks.push(block(9, 4, 0));
    l.chords.push(chord(9, 4, 0));
    assert_eq!(lanes_end_half(&l), want.total_half);
    let got = lanes_to_timeline(&l).unwrap_or_else(|| panic!("a timeline"));
    assert_eq!(got.entries, want.entries);
    assert_eq!((got.total_half, got.intro_half, got.has_ending), (want.total_half, want.intro_half, true));
    assert_eq!((got.uniform_len_half, got.arrangement_length), (want.uniform_len_half, want.arrangement_length));
}

#[test]
fn degenerate_lanes_never_panic() {
    let defs = vec![def('C'), def('A'), def('A'), SongSection { role: 1, ..def('B') }, def('Z')];
    // Overlapping blocks: the later start wins from its start.
    let l = LaneSong {
        defs: defs.clone(),
        blocks: vec![block(0, 8, 0), block(4, 8, 3), block(4, 0, 1), block(-4, 6, 1), block(2, 2, 99), block(20, 4, 4)],
        chords: vec![chord(-2, 3, 500), chord(0, 0, 1), chord(1, 10, 2), chord(3, 2, 3), chord(i32::MAX - 1, 9, 1)],
        ..LaneSong::default()
    };
    let t = lanes_to_timeline(&l).unwrap_or_else(|| panic!("a timeline"));
    check_invariants(&t);
    assert_eq!(t.total_half, lanes_end_half(&l));
    // A (-4..2, clipped to 0..2) starts with C (0..8) and comes later in the list, so it wins; the
    // unknown defs (99, Z) drop.
    assert_eq!(t.entries[0].section, t.sections.iter().position(|s| s.id == 'A').map_or(-1, |p| p as i32));

    // The most a session keeps: 4096 chords and 512 blocks, every one odd.
    let blocks: Vec<LaneBlock> = (0..512)
        .map(|i| LaneBlock { start_half: i * 7 - 3, len_half: (i % 9) - 1, def: (i % 6) as usize, energy: i * 3 - 700, take: i - 40 })
        .collect();
    let chords: Vec<LaneChord> =
        (0..4096).map(|i| LaneChord { start_half: (i * 13) % 4000 - 5, len_half: i % 11 - 2, slot: i * 31 - 500, bass: i % 30 - 10 }).collect();
    let l = LaneSong { defs, blocks, chords, ..LaneSong::default() };
    let t = lanes_to_timeline(&l).unwrap_or_else(|| panic!("a timeline"));
    check_invariants(&t);
    assert!(t.entries.iter().all(|e| (-3..=5).contains(&e.energy_add) && (0..=16).contains(&e.take)));

    // Huge lengths stay bounded.
    let l = LaneSong { blocks: vec![block(0, i32::MAX, 0)], chords: vec![chord(i32::MIN, i32::MAX, 0)], defs: vec![def('A')], ..LaneSong::default() };
    let t = lanes_to_timeline(&l).unwrap_or_else(|| panic!("a timeline"));
    check_invariants(&t);
    assert_eq!(t.total_half, lanes_end_half(&l));
    // Only zero-length items: nothing.
    let l = LaneSong { blocks: vec![block(3, 0, 0)], chords: vec![chord(5, -2, 0)], defs: vec![def('A')], ..LaneSong::default() };
    assert!(lanes_to_timeline(&l).is_none());
}
