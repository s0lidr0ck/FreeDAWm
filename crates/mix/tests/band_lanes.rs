//! The session's band lanes in half bars of the first meter.

use soundcraft_band::lanes::lanes_to_timeline;
use soundcraft_band::song_file::factory_songs;
use soundcraft_mix::{band_song_end, lane_song, lanes_from_song};
use soundcraft_model::{ChordEvent, SectionBlock, Session};
use soundcraft_time::TempoMap;

fn six_eight() -> Session {
    Session { tempo: TempoMap::new(120.0, 6, 8).unwrap_or_default(), ..Session::default() }
}

#[test]
fn lanes_follow_the_first_meter() {
    let mut s = six_eight();
    let eighth = 480;
    let half = 3 * eighth;
    let bar = 2 * half;
    // Nothing on the lanes: no song.
    assert!(lane_song(&s).is_none());
    assert_eq!(band_song_end(&s), None);

    // A song laid at bar 9 starts at half 16.
    let songs = factory_songs();
    let Some(blues) = songs.iter().find(|f| f.name == "12-bar Blues") else { panic!("the blues") };
    let at = s.tempo.tick_to_samples(8 * bar, s.sample_rate);
    let (defs, blocks, chords) = lanes_from_song(&s, &blues.song, at);
    assert_eq!(defs.iter().map(|d| d.id).collect::<String>(), "A");
    assert_eq!(blocks.len(), 1);
    assert_eq!((blocks[0].start_tick, blocks[0].length_ticks), (8 * bar, 12 * bar));
    assert_eq!(chords.len(), 12);
    assert!(chords.iter().all(|c| c.length_ticks == bar));
    let mut ids: Vec<u32> = blocks.iter().map(|b| b.id).chain(chords.iter().map(|c| c.id)).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 13);
    assert!(ids.iter().all(|i| *i >= s.band.next_id));

    s.band.defs = defs;
    s.band.sections = blocks;
    s.band.chords = chords;
    let l = lane_song(&s).unwrap_or_else(|| panic!("a song"));
    assert_eq!(l.blocks.first().map(|b| (b.start_half, b.len_half)), Some((16, 24)));
    assert_eq!(l.chords.first().map(|c| (c.start_half, c.len_half, c.slot)), Some((16, 2, 2)));
    let t = lanes_to_timeline(&l).unwrap_or_else(|| panic!("a timeline"));
    assert_eq!(t.total_half, 40);
    assert_eq!(band_song_end(&s), Some(s.tempo.tick_to_samples(40 * half, s.sample_rate)));
}

#[test]
fn ticks_round_to_halves() {
    let mut s = six_eight();
    let half = 1440;
    // Starts round to the nearest half, ends round up; a chord with no half left drops.
    s.band.sections.push(SectionBlock { id: 1, start_tick: half / 2 - 1, length_ticks: half + 2, section: 'A', energy: 2, take: 3 });
    s.band.sections.push(SectionBlock { id: 2, start_tick: 4 * half, length_ticks: half, section: 'C', ..SectionBlock::default() });
    s.band.chords.push(ChordEvent { id: 3, tick: half / 2, length_ticks: half, slot: 7, bass: 4 });
    s.band.chords.push(ChordEvent { id: 4, tick: 3 * half - 100, length_ticks: 50, slot: 9, bass: -1 });
    let l = lane_song(&s).unwrap_or_else(|| panic!("a song"));
    assert_eq!(l.blocks.len(), 1, "a block of a section with no definition drops");
    assert_eq!((l.blocks[0].start_half, l.blocks[0].len_half, l.blocks[0].energy, l.blocks[0].take), (0, 2, 2, 3));
    assert_eq!(l.chords.len(), 1);
    assert_eq!((l.chords[0].start_half, l.chords[0].len_half, l.chords[0].slot, l.chords[0].bass), (1, 1, 7, 4));
}

#[test]
fn other_meters_use_the_four_four_half_the_band_plays() {
    // 5/4 plays as 4/4 (meterFromHost), so a half is 1920 ticks, not 2400.
    let mut s = Session { tempo: TempoMap::new(120.0, 5, 4).unwrap_or_default(), ..Session::default() };
    let half = 1920;
    s.band.chords.push(ChordEvent { id: 1, tick: 4 * half, length_ticks: 2 * half, slot: 3, bass: -1 });
    let l = lane_song(&s).unwrap_or_else(|| panic!("a song"));
    assert_eq!(l.chords.first().map(|c| (c.start_half, c.len_half)), Some((4, 2)));
    assert_eq!(band_song_end(&s), Some(s.tempo.tick_to_samples(6 * half, s.sample_rate)));
    // A song laid at half 4 puts its first chord at tick 7680.
    let songs = factory_songs();
    let Some(blues) = songs.iter().find(|f| f.name == "12-bar Blues") else { panic!("the blues") };
    let (_, blocks, chords) = lanes_from_song(&s, &blues.song, s.tempo.tick_to_samples(7680, s.sample_rate));
    assert_eq!(chords.first().map(|c| (c.tick, c.length_ticks)), Some((7680, 2 * half)));
    assert_eq!(blocks.first().map(|b| b.start_tick), Some(7680));
    // 7/8 and 12/8 too.
    for (n, d) in [(7, 8), (12, 8)] {
        let s = Session { tempo: TempoMap::new(120.0, n, d).unwrap_or_default(), ..Session::default() };
        let (_, _, chords) = lanes_from_song(&s, &blues.song, 0);
        assert_eq!(chords.first().map(|c| c.length_ticks), Some(2 * half), "{n}/{d}");
    }
}

/// Detect Chords "As played" lays chords at off-grid ticks, end to end: the band plays them on
/// half bars without two chords on one half (a chord ends where the next one starts).
#[test]
fn off_grid_neighbours_do_not_overlap() {
    let mut s = Session::default();
    let half = 1920;
    // 0.1 s to 1.45 s, then 1.45 s to 4 s at 120 bpm (1920 ticks a second).
    s.band.chords.push(ChordEvent { id: 1, tick: 192, length_ticks: 2784 - 192, slot: 5, bass: -1 });
    s.band.chords.push(ChordEvent { id: 2, tick: 2784, length_ticks: 4 * half - 2784, slot: 3, bass: -1 });
    let l = lane_song(&s).unwrap_or_else(|| panic!("a song"));
    let spans: Vec<(i32, i32)> = l.chords.iter().map(|c| (c.start_half, c.len_half)).collect();
    assert_eq!(spans, [(0, 1), (1, 3)]);
}
