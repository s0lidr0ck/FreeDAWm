//! Track colours: the 64-colour palette behind `track.color` (the 16 originals keep indexes
//! 0–15), RGB and `#rrggbb` colours, new tracks taking the most distinct colour, Spread Track
//! Colors, and old sessions opening with exactly the colours they were saved with.

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::palette::{self, GRID, MAIN_SHADE, grid_index, palette_color};
use soundcraft_model::{ChannelFormat, Session, TRACK_COLORS, TrackId, TrackKind};

fn engine(n: usize) -> (Engine, Vec<u64>) {
    let mut e = Engine::default();
    let ids = (0..n).map(|_| e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None).0).collect();
    (e, ids)
}

fn color(e: &Engine, t: u64) -> [u8; 3] {
    e.session().track(TrackId(t)).unwrap().color
}

#[test]
fn a_palette_index_means_what_it_always_meant_and_rgb_works_too() {
    let (mut e, ids) = engine(1);
    let t = ids[0];
    // 0–15: the original colours, exactly as before.
    for (i, c) in TRACK_COLORS.iter().enumerate() {
        e.execute("track.color", &json!({"tracks": [t], "color": i})).unwrap();
        assert_eq!(color(&e, t), *c, "index {i}");
    }
    // 16–63: the grid, row by row.
    e.execute("track.color", &json!({"tracks": [t], "color": grid_index(MAIN_SHADE, 3)})).unwrap();
    assert_eq!(color(&e, t), GRID[MAIN_SHADE][3]);
    let r = e.execute("track.color", &json!({"tracks": [t], "color": 63})).unwrap();
    assert_eq!(color(&e, t), GRID[3][11]);
    assert_eq!(r["color"], json!(GRID[3][11]));
    // Past the palette it wraps round (old clients sent any number).
    e.execute("track.color", &json!({"tracks": [t], "color": 64 + 2})).unwrap();
    assert_eq!(color(&e, t), TRACK_COLORS[2]);
    // Any colour: [r, g, b] (clamped) or "#rrggbb".
    e.execute("track.color", &json!({"tracks": [t], "color": [12, 300, 7]})).unwrap();
    assert_eq!(color(&e, t), [12, 255, 7]);
    e.execute("track.color", &json!({"tracks": [t], "color": "#1a2B3c"})).unwrap();
    assert_eq!(color(&e, t), [0x1a, 0x2b, 0x3c]);
    // Nonsense is refused and changes nothing.
    for bad in [json!("#12"), json!(-1), json!(1.5), json!([1, 2]), json!({"r": 1}), Value::Null] {
        assert!(e.execute("track.color", &json!({"tracks": [t], "color": bad})).is_err(), "{bad}");
        assert_eq!(color(&e, t), [0x1a, 0x2b, 0x3c]);
    }
    // One undo step each.
    assert!(e.undo());
    assert_eq!(color(&e, t), [12, 255, 7]);
}

#[test]
fn new_tracks_take_the_most_distinct_colour_and_never_recolour_others() {
    let mut e = Engine::default();
    let r = e.execute("track.new", &json!({"count": 12, "kind": "audio"})).unwrap();
    let ids: Vec<u64> = r["tracks"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    let colours: Vec<[u8; 3]> = ids.iter().map(|t| color(&e, *t)).collect();
    // The first track is the Main blue; twelve tracks take the twelve Main hues, each once.
    assert_eq!(colours[0], GRID[MAIN_SHADE][8]);
    let mut main = GRID[MAIN_SHADE].to_vec();
    let mut got = colours.clone();
    main.sort_unstable();
    got.sort_unstable();
    assert_eq!(got, main);
    // Neighbours look clearly different (the old palette's neighbours were near-identical).
    for w in colours.windows(2) {
        assert!(palette::distance(w[0], w[1]) > 0.12, "{:?} then {:?}", w[0], w[1]);
    }
    // More tracks: the other shades; the first twelve keep their colours.
    e.execute("track.new", &json!({"count": 8, "kind": "midi"})).unwrap();
    assert_eq!(ids.iter().map(|t| color(&e, *t)).collect::<Vec<_>>(), colours);
    let all: Vec<[u8; 3]> = e.session().tracks.iter().filter(|t| t.kind != TrackKind::Master).map(|t| t.color).collect();
    assert_eq!(all.len(), 20);
    for (i, c) in all.iter().enumerate() {
        assert!(!all[..i].contains(c), "track {} repeats {c:?}", i + 1);
    }
    // Deterministic: the same steps give the same colours.
    let mut again = Engine::default();
    again.execute("track.new", &json!({"count": 12, "kind": "audio"})).unwrap();
    again.execute("track.new", &json!({"count": 8, "kind": "midi"})).unwrap();
    let again: Vec<[u8; 3]> = again.session().tracks.iter().filter(|t| t.kind != TrackKind::Master).map(|t| t.color).collect();
    assert_eq!(again, all);
    // A master keeps its grey and does not count.
    let m = e.session_mut().add_track(TrackKind::Master, ChannelFormat::Stereo, None);
    assert_eq!(e.session().track(m).unwrap().color, [140, 140, 140]);
}

#[test]
fn a_new_track_avoids_the_colours_an_old_session_uses() {
    // An old session's tracks hold old-palette colours: a new track looks unlike all of them.
    let mut s = Session::default();
    for c in TRACK_COLORS.iter().take(4) {
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.track_mut(t).unwrap().color = *c;
    }
    let next = s.next_track_color();
    for c in TRACK_COLORS.iter().take(4) {
        assert!(palette::distance(next, *c) > 0.1, "{next:?} is close to {c:?}");
    }
}

#[test]
fn spread_recolours_the_selected_tracks_in_one_step() {
    let (mut e, ids) = engine(6);
    // Six tracks all one colour, one clip following its track's colour and one with its own.
    for t in &ids {
        e.session_mut().track_mut(TrackId(*t)).unwrap().color = TRACK_COLORS[0];
    }
    let before: Vec<[u8; 3]> = ids.iter().map(|t| color(&e, *t)).collect();
    // Nothing selected: greyed, with a reason.
    e.session_mut().edit.selected_tracks.clear();
    let err = e.execute("track.spread_colors", &json!({})).unwrap_err().to_string();
    assert!(err.contains("Select the tracks"), "{err}");
    let spec = soundcraft_engine::find_command("track.spread_colors").unwrap();
    assert_eq!((spec.enabled)(&e), Err("Select the tracks to give distinct colors".to_string()));
    // Tracks 2–5 selected (out of order): they take distinct colours in track order, unlike
    // tracks 1 and 6, which keep theirs.
    let sel: Vec<TrackId> = [ids[4], ids[1], ids[3], ids[2]].map(TrackId).to_vec();
    e.session_mut().edit.selected_tracks = sel;
    let undo = e.undo_history().len();
    let r = e.execute("track.spread_colors", &json!({})).unwrap();
    assert_eq!(e.undo_history().len(), undo + 1, "one undo step");
    let got: Vec<u64> = r["tracks"].as_array().unwrap().iter().map(|v| v["track"].as_u64().unwrap()).collect();
    assert_eq!(got, ids[1..5], "in track order");
    assert_eq!(color(&e, ids[0]), TRACK_COLORS[0]);
    assert_eq!(color(&e, ids[5]), TRACK_COLORS[0]);
    let new: Vec<[u8; 3]> = ids[1..5].iter().map(|t| color(&e, *t)).collect();
    assert_eq!(new, palette::spread(&[TRACK_COLORS[0], TRACK_COLORS[0]], 4));
    for (i, c) in new.iter().enumerate() {
        assert!(palette::distance(*c, TRACK_COLORS[0]) > 0.1);
        assert!(!new[..i].contains(c));
        assert!(palette::palette_index_of(*c).is_some_and(|i| i >= palette::GRID_START));
        assert_eq!(r["tracks"][i]["color"], json!(palette::hex(*c)));
    }
    // Undo brings back every colour at once.
    assert!(e.undo());
    assert_eq!(ids.iter().map(|t| color(&e, *t)).collect::<Vec<_>>(), before);
    // Named tracks work without a selection; a master alone is refused.
    e.session_mut().edit.selected_tracks.clear();
    e.execute("track.spread_colors", &json!({"tracks": [ids[0], ids[5]]})).unwrap();
    assert_ne!(color(&e, ids[0]), color(&e, ids[5]));
    let m = e.session_mut().add_track(TrackKind::Master, ChannelFormat::Stereo, None).0;
    assert!(e.execute("track.spread_colors", &json!({"tracks": [m]})).is_err());
    assert_eq!(color(&e, m), [140, 140, 140]);
}

/// A session as saved before the bigger palette (format version 2, every original colour, a
/// custom colour, a grey and a master) opens with exactly the colours it was saved with, saves
/// them back unchanged, and a track added to it leaves them alone.
#[test]
fn an_old_session_opens_with_its_colours_unchanged() {
    let mut s = Session::default();
    for _ in 0..19 {
        s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    }
    s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
    let mut v = serde_json::to_value(&s).unwrap();
    v["format_version"] = json!(2);
    v.as_object_mut().unwrap().remove("inputs");
    let mut saved: Vec<[u8; 3]> = TRACK_COLORS.to_vec();
    saved.extend([[1, 2, 3], [128, 128, 128], [250, 240, 200], [140, 140, 140]]);
    for (i, c) in saved.iter().enumerate() {
        v["tracks"][i]["color"] = json!(c);
    }
    let text = v.to_string();
    let mut back = Session::from_json(&text).unwrap();
    let opened: Vec<[u8; 3]> = back.tracks.iter().map(|t| t.color).collect();
    assert_eq!(opened, saved);
    let resaved: Value = serde_json::from_str(&back.to_json().unwrap()).unwrap();
    for (i, c) in saved.iter().enumerate() {
        assert_eq!(resaved["tracks"][i]["color"], json!(c), "track {i}");
    }
    // A track added goes in before the master and takes a grid colour; the rest keep theirs.
    let t = back.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    assert_eq!(back.tracks[..19].iter().map(|t| t.color).collect::<Vec<_>>(), saved[..19]);
    assert_eq!(back.tracks[20].color, saved[19], "the master");
    let new = back.track(t).unwrap().color;
    assert!(palette::palette_index_of(new).is_some_and(|i| i >= palette::GRID_START && palette_color(i) == Some(new)));
}
