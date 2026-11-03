//! Task 3: songs, the timeline, song files, meter, chord theory and the Performer.

use soundcraft_band::chord_theory::name_for;
use soundcraft_band::live::Performer;
use soundcraft_band::meter::{Meter, meter_from_host};
use soundcraft_band::song::{SongLoc, SongTimeline, build_timeline, song_locate};
use soundcraft_band::song_file::{FeelFlagsFile, factory_songs, song_file_from_json};

#[test]
fn factory_songs_parse() {
    let songs = factory_songs();
    assert_eq!(songs.len(), 5);
    let names: Vec<&str> = songs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["12-bar Blues", "Ballad with Intro and Ending", "Jazz ii-V-I", "Pop Verse Chorus Bridge", "Rock Chorus Builds"]);

    let ballad = &songs[1];
    let roles: Vec<i32> = ballad.song.sections.iter().map(|s| s.role).collect();
    assert!(roles.contains(&1), "the Ballad has an intro: {roles:?}");
    assert!(roles.contains(&2), "the Ballad has an ending: {roles:?}");
    assert_eq!(ballad.song.sections.first().map(|s| s.role), Some(1));

    let jazz = &songs[2];
    let lens: Vec<i32> = jazz.song.sections[0].chords.iter().map(|c| c.length).collect();
    assert_eq!(lens, [2, 2, 4]);

    for s in &songs {
        assert_eq!(s.feel, Some(FeelFlagsFile::Natural), "{}", s.name);
        assert!((0..=11).contains(&s.key) && (0..=1).contains(&s.scale) && (1..=16).contains(&s.take), "{}", s.name);
        let t = build_timeline(&s.song);
        assert!(!t.entries.is_empty() && t.total_half > 0, "{}", s.name);
    }
    assert_eq!(songs[0].key, 7);
    assert_eq!(songs[0].tension, Some(true));
}

#[test]
fn song_file_strict_rejects_and_reports() {
    assert!(song_file_from_json("not json").is_err());
    assert_eq!(song_file_from_json(r#"{"kind":"song","format":2,"song":{}}"#).err().as_deref(), Some("This song was saved by a newer MIDI Studio."));
    let bad_len = r#"{"kind":"song","format":1,"song":{"sections":[{"id":"A","chords":[{"slot":0,"length":3}]}]}}"#;
    assert_eq!(song_file_from_json(bad_len).err().as_deref(), Some("The song is damaged: section A: chord length must be half, 1 or 2 bars"));
    let ok = r#"{"kind":"song","format":1,"song":{"sections":[{"id":"A","chords":[{"slot":0,"length":4}]}]}}"#;
    let d = song_file_from_json(ok).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(d.take, 1);
    assert_eq!(d.feel, None);
    assert_eq!(d.tension, None);
}

#[test]
fn timeline_basics_never_empty_and_ending_truncates() {
    let empty = soundcraft_band::song::Song::default();
    let t = build_timeline(&empty);
    assert_eq!(t.entries.len(), 1);
    assert_eq!(t.total_half, 2);
    assert_eq!(t.uniform_len_half, 2);

    let songs = factory_songs();
    let ballad = build_timeline(&songs[1].song);
    assert!(ballad.has_ending);
    assert_eq!(ballad.uniform_len_half, 0);
    // Past the end: over.
    let l = song_locate(&ballad, f64::from(ballad.total_half) * 2.0 + 1.0, 4.0, false, 0.0);
    assert!(l.ended);
}

fn fixture() -> Option<serde_json::Value> {
    let dir = std::env::var_os("CARGO_MANIFEST_DIR")?;
    let path = std::path::Path::new(&dir).join("tests").join("fixtures").join("timelines.json");
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

/// The dump for one factory song: the fixture is a JSON array of `band-ref --dump-timeline` outputs.
fn dump_for<'a>(fx: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    fx.as_array()
        .and_then(|a| a.iter().find(|d| d.get("song").and_then(|s| s.as_str()) == Some(name)))
        .unwrap_or_else(|| panic!("no dump for {name}"))
}

fn int(v: &serde_json::Value, k: &str) -> i64 {
    v.get(k).and_then(|x| x.as_i64()).unwrap_or_else(|| panic!("missing int {k}"))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-12 * b.abs().max(1.0)
}

#[test]
fn timeline_matches_cpp_for_factory_songs() {
    let fx = fixture().unwrap_or_else(|| panic!("tests/fixtures/timelines.json missing or unreadable"));
    for s in factory_songs() {
        let d = dump_for(&fx, &s.name);
        let t = build_timeline(&s.song);
        assert_eq!(i64::from(t.total_half), int(d, "total_half"), "{}", s.name);
        assert_eq!(i64::from(t.arrangement_length), int(d, "arrangement_length"), "{}", s.name);
        assert_eq!(i64::from(t.uniform_len_half), int(d, "uniform_len_half"), "{}", s.name);
        assert_eq!(i64::from(t.intro_half), int(d, "intro_half"), "{}", s.name);
        assert_eq!(i64::from(t.meter_override), int(d, "meter_override"), "{}", s.name);
        assert_eq!(Some(t.has_ending), d.get("has_ending").and_then(|x| x.as_bool()), "{}", s.name);
        let entries = d.get("entries").and_then(|e| e.as_array()).unwrap_or_else(|| panic!("no entries"));
        assert_eq!(t.entries.len(), entries.len(), "{}", s.name);
        for (i, (e, c)) in t.entries.iter().zip(entries).enumerate() {
            let got = [e.start_half, e.len_half, e.section, e.chord_index, e.section_chords, e.arr_pos, e.slot, e.bass, e.energy_add, e.take];
            let keys = ["start_half", "len_half", "section", "chord_index", "section_chords", "arr_pos", "slot", "bass", "energy_add", "take"];
            for (g, k) in got.iter().zip(keys) {
                assert_eq!(i64::from(*g), int(c, k), "{} entry {i} {k}", s.name);
            }
        }
    }
}

fn check_loc(name: &str, i: usize, l: &SongLoc, c: &serde_json::Value) {
    assert_eq!(i64::from(l.idx), int(c, "idx"), "{name} probe {i} idx");
    assert_eq!(i64::from(l.loop_), int(c, "loop"), "{name} probe {i} loop");
    assert_eq!(i64::from(l.global), int(c, "global"), "{name} probe {i} global");
    let f = |k: &str| c.get(k).and_then(|x| x.as_f64()).unwrap_or_else(|| panic!("missing {k}"));
    assert!(close(l.start, f("start")), "{name} probe {i} start {} vs {}", l.start, f("start"));
    assert!(close(l.end, f("end")), "{name} probe {i} end {} vs {}", l.end, f("end"));
    assert_eq!(Some(l.ended), c.get("ended").and_then(|x| x.as_bool()), "{name} probe {i} ended");
}

#[test]
fn song_locate_uniform_and_general_agree_with_cpp() {
    let fx = fixture().unwrap_or_else(|| panic!("tests/fixtures/timelines.json missing or unreadable"));
    for s in factory_songs() {
        let d = dump_for(&fx, &s.name);
        let t: std::sync::Arc<SongTimeline> = build_timeline(&s.song);
        let probes = d.get("locate").and_then(|e| e.as_array()).unwrap_or_else(|| panic!("no locate"));
        assert_eq!(probes.len(), 200, "{}", s.name);
        for (i, p) in probes.iter().enumerate() {
            let ppq = p.get("ppq").and_then(|x| x.as_f64()).unwrap_or_else(|| panic!("ppq"));
            let bar = p.get("bar_len").and_then(|x| x.as_f64()).unwrap_or_else(|| panic!("bar_len"));
            let uni = p.get("uniform").unwrap_or_else(|| panic!("uniform"));
            let general = p.get("general").unwrap_or_else(|| panic!("general"));
            check_loc(&s.name, i, &song_locate(&t, ppq, bar, false, 0.0), uni);
            check_loc(&s.name, i, &song_locate(&t, ppq, bar, true, 0.0), general);
        }
    }
}

#[test]
fn meter_from_host_maps_unsupported_to_44() {
    assert_eq!(meter_from_host(5, 4), (Meter::M44, true));
    assert_eq!(meter_from_host(7, 8), (Meter::M44, true));
    assert_eq!(meter_from_host(12, 8), (Meter::M44, true));
    assert_eq!(meter_from_host(6, 8), (Meter::M68, false));
    assert_eq!(meter_from_host(3, 4), (Meter::M34, false));
    assert_eq!(meter_from_host(4, 4), (Meter::M44, false));
}

#[test]
fn chord_names() {
    assert_eq!(name_for(0, 0, 0, -1), "C");
    assert_eq!(name_for(80, 0, 0, -1), "Am");
    assert_eq!(name_for(48, 0, 0, 0), "F/C");
    assert_eq!(name_for(5 * 16 + 1, 0, 0, -1), "Am7");
    assert_eq!(name_for(4 * 16 + 2, 0, 0, -1), "G7");
    assert_eq!(name_for(6 * 16, 0, 0, -1), "Bdim");
    assert_eq!(name_for(6 * 16 + 1, 0, 0, -1), "Bm7b5");
    assert_eq!(name_for(1, 10, 0, -1), "Bbmaj7");
}

#[test]
fn performer_with_live_off_publishes_feel_flags() {
    let songs = factory_songs();
    let t = build_timeline(&songs[3].song);
    let mut p = Performer::default();
    p.set_settings(false, false, 4, 24);
    p.set_key(0, 0);
    p.set_interplay(true, false, true, false);
    p.set_natural(true);
    p.set_tension(true);
    // A block at 120 BPM, 48 kHz, 512 samples.
    let blk = 512.0 / 48000.0 * 2.0;
    let mut ppq = 0.0;
    for _ in 0..50 {
        p.advance(true, ppq, ppq + blk, 4.0, 1.0, &t);
        ppq += blk;
    }
    let st = p.state();
    assert!(!st.live);
    assert!(st.bass_lock && !st.fills_yield && st.leave_space && !st.dynamics_together);
    assert!(st.interplay());
    assert!(st.natural && st.tension);
    assert!(!st.any(), "with Live off and nothing queued the band just plays the song");
    assert_eq!(st.bar_len, 4.0);
    assert_eq!(st.jump_offset, 0.0);

    p.set_interplay(false, false, false, false);
    p.set_natural(false);
    p.set_tension(false);
    p.advance(false, ppq, ppq, 4.0, 1.0, &t);
    let st = p.state();
    assert!(!st.interplay() && !st.natural && !st.tension);
    assert!(!st.any());
}

#[test]
fn performer_fill_key_and_section_jump() {
    let songs = factory_songs();
    let t = build_timeline(&songs[3].song);
    let mut p = Performer::default();
    p.set_settings(false, false, 4, 24);
    let blk = 0.125;
    let mut b = 0.0;
    while b < 2.0 {
        if b == 1.0 {
            p.note(true, 25, b); // key base + 1 = Fill: the rest of this bar
        }
        if b == 1.5 {
            p.note(true, 26, b); // key base + 2 = section B, on the next bar line
        }
        p.advance(true, b, b + blk, 4.0, 1.0, &t);
        b += blk;
    }
    assert_eq!(p.state().fill_from, 1.0);
    assert_eq!(p.state().fill_to, 4.0);
    assert!(p.cue().fill);
    assert_eq!(p.state().jump_land, 4.0);
    assert_eq!(p.state().jump_entry, 4);
    assert_eq!(p.state().jump_new_offset, 12.0);
    assert_eq!(p.cue().section, 'B');
    while b <= 4.0 {
        p.advance(true, b, b + blk, 4.0, 1.0, &t);
        b += blk;
    }
    assert!(p.state().jump_land < 0.0);
    assert_eq!(p.state().jump_offset, 12.0);
    assert!(p.state().any());
}
