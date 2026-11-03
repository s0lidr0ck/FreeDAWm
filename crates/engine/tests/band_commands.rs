//! The `band.*` commands, and musicians as track inputs.

use serde_json::{Value, json};
use soundcraft_band::band_preset::factory_band_presets;
use soundcraft_band::chord_match::{match_chord, parse_chord_name};
use soundcraft_band::lanes::lanes_to_timeline;
use soundcraft_band::library::INSTRUMENT_NAMES;
use soundcraft_band::song::build_timeline;
use soundcraft_band::song_file::factory_songs;
use soundcraft_engine::{Engine, EngineError};
use soundcraft_mix::lane_song;
use soundcraft_model::{BandState, ChannelFormat, Route, TrackKind};

/// One 4/4 bar (the default session's meter) in samples.
fn bar(e: &Engine) -> i64 {
    let s = e.session();
    s.tempo.tick_to_samples(3840, s.sample_rate)
}

fn ok(e: &mut Engine, id: &str, p: Value) -> Value {
    e.execute(id, &p).unwrap_or_else(|err| panic!("{id} {p}: {err}"))
}

fn refused(e: &mut Engine, id: &str, p: Value) -> String {
    let before = e.session().band.clone();
    let undo = e.undo_history().len();
    let err = match e.execute(id, &p) {
        Ok(v) => panic!("{id} {p} was accepted: {v}"),
        Err(err) => err,
    };
    assert!(!matches!(err, EngineError::Internal(..)), "{id} panicked: {err}");
    assert_eq!(e.session().band, before, "{id} {p}: a refused edit changes nothing");
    assert_eq!(e.undo_history().len(), undo);
    err.to_string()
}

#[test]
fn slot_edits_are_undoable_and_validated() {
    let mut e = Engine::default();
    ok(
        &mut e,
        "band.slot",
        json!({"slot": 2, "energy": 3, "swing": 70, "speed": "double", "art": [1, 2, 3, 4], "ksoctave": null, "mute": true, "voice": "Top"}),
    );
    let s = &e.session().band.slots[1];
    assert_eq!((s.energy, s.swing, s.speed, s.art, s.ksoctave, s.mute, s.voice), (3, 70, 2, [1, 2, 3, 4], None, true, 1));
    assert_eq!(e.undo_label(), Some("Band Musician"));
    e.undo();
    assert_eq!(e.session().band, BandState::default());

    for (p, want) in [
        (json!({"slot": 7}), "1-6"),
        (json!({}), "slot"),
        (json!({"slot": 1, "energy": 9}), "1-4"),
        (json!({"slot": 1, "length": 5}), "10-100"),
        (json!({"slot": 1, "velocity": 0}), "1-127"),
        (json!({"slot": 1, "octave": 3}), "-2-2"),
        (json!({"slot": 1, "art": [16, 0, 0, 0]}), "0-15"),
        (json!({"slot": 1, "art": [1, 2]}), "four"),
        (json!({"slot": 1, "speed": "fast"}), "half"),
        (json!({"slot": 1, "instrument": 13}), "0-12"),
        (json!({"slot": 1, "instrument": "Kazoo"}), "Kazoo"),
        (json!({"slot": 1, "player": "Nobody At All"}), "Nobody At All"),
        (json!({"slot": 1, "target": "No Such Target"}), "No Such Target"),
        (json!({"slot": 1, "ksoctave": 5}), "-2-2"),
        (json!({"slot": 1, "swing": 2.5}), "0-100"),
    ] {
        let msg = refused(&mut e, "band.slot", p.clone());
        assert!(msg.contains(want), "{p}: `{msg}` should name `{want}`");
    }

    // Settings: names and numbers, undoable as one step.
    ok(
        &mut e,
        "band.settings",
        json!({"key": "A", "scale": "minor", "take": 4, "feel": {"bass_lock": true}, "song_feel": {"on": true, "groove": "swing"}, "feel_ignore": [2, 5]}),
    );
    let b = &e.session().band;
    assert_eq!((b.key, b.scale, b.take, b.feel.bass_lock, b.song_feel.on, b.song_feel.groove, b.feel_ignore), (9, 1, 4, true, true, 1, 0b10010));
    assert_eq!(e.undo_label(), Some("Band Settings"));
    assert!(refused(&mut e, "band.settings", json!({"take": 17})).contains("1-16"));
    assert!(refused(&mut e, "band.settings", json!({"key": "H"})).contains('H'));
    ok(&mut e, "band.settings", json!({"feel_preset": "Blues shuffle"}));
    assert_eq!((e.session().band.song_feel.pocket, e.session().band.song_feel.groove), (1, 2));
    assert!(refused(&mut e, "band.settings", json!({"feel_preset": "Polka Disco"})).contains("Polka Disco"));
}

#[test]
fn instrument_change_repicks_the_player_like_ms() {
    let mut e = Engine::default();
    let lib = e.band_library();
    let clay = lib.seed_for_name("Clay");
    assert!(clay > 0 && lib.plays(0, clay));
    let first =
        |i: usize| lib.players[i].iter().copied().find(|s| lib.persona(*s).is_some()).and_then(|s| lib.persona(s)).map(|p| (p.name.clone(), p.seed));

    // An instrument Clay plays keeps him.
    if let Some(j) = (1..INSTRUMENT_NAMES.len()).find(|&i| lib.plays(i as i32, clay)) {
        ok(&mut e, "band.slot", json!({"slot": 1, "instrument": j}));
        assert_eq!(e.session().band.slots[0].player, "Clay");
        e.undo();
    }
    // One he doesn't: the Library's first player for it.
    let i = (1..INSTRUMENT_NAMES.len())
        .find(|&i| !lib.plays(i as i32, clay) && first(i).is_some())
        .unwrap_or_else(|| panic!("an instrument Clay doesn't play"));
    ok(&mut e, "band.slot", json!({"slot": 1, "instrument": INSTRUMENT_NAMES[i]}));
    let (name, seed) = first(i).unwrap_or_default();
    let s = &e.session().band.slots[0];
    assert_eq!((s.instrument as usize, s.player.as_str(), i32::from(s.player_seed)), (i, name.as_str(), seed));
    // A player named with the instrument is checked against the new instrument.
    let msg = refused(&mut e, "band.slot", json!({"slot": 1, "instrument": 0, "player": name}));
    if lib.plays(0, seed) {
        panic!("{name} plays piano: pick another instrument for this check ({msg})");
    }
    assert!(msg.contains(&name), "{msg}");
    ok(&mut e, "band.slot", json!({"slot": 1, "instrument": 0, "player": "Clay"}));
    assert_eq!(e.session().band.slots[0].player, "Clay");
    // "Session" is the instrument's first player.
    ok(&mut e, "band.slot", json!({"slot": 1, "player": "Session"}));
    assert_eq!(e.session().band.slots[0].player, first(0).unwrap_or_default().0);
}

#[test]
fn typed_chord_entry() {
    let mut e = Engine::default();
    let bar = bar(&e);
    let want = match_chord(&parse_chord_name("Am7").unwrap_or_default(), 0, 0);
    assert!(want.exact);
    let r = ok(&mut e, "band.chord.add", json!({"at": 0, "chord": "Am7"}));
    assert_eq!(r["exact"], json!(true));
    assert_eq!(r["slot"], json!(want.slot));
    let c = e.session().band.chords[0].clone();
    assert_eq!((c.tick, c.length_ticks, i32::from(c.slot)), (0, 3840, want.slot));
    assert_eq!(e.undo_label(), Some("Add Chord"));

    // A chord the key doesn't hold: the nearest, and says so. (MS's slots hold Bb in C, as the bVII.)
    let r = ok(&mut e, "band.chord.add", json!({"at": 2 * bar, "chord": "Bb"}));
    assert_eq!(r["exact"], json!(true));
    let r = ok(&mut e, "band.chord.add", json!({"at": 2 * bar, "chord": "F#"}));
    assert_eq!(r["exact"], json!(false));
    assert_eq!(r["slot"], json!(match_chord(&parse_chord_name("F#").unwrap_or_default(), 0, 0).slot));
    assert_eq!(e.session().band.chords.iter().filter(|c| c.tick == 2 * 3840).count(), 1, "the later chord replaces the one it covers");
    // Inserted before it, a chord ends at the next one or after a bar, whichever is first.
    let at = bar + bar / 2;
    ok(&mut e, "band.chord.add", json!({"at": at, "chord": "F"}));
    let f = e.session().band.chords.iter().find(|c| c.tick == 3840 + 1920).cloned().unwrap_or_default();
    assert_eq!(f.length_ticks, 1920);
    // A slot, and an explicit end, snapped to half bars.
    let r = ok(&mut e, "band.chord.add", json!({"at": 4 * bar + 10, "end": 5 * bar - 10, "chord": 5, "bass": 4}));
    assert_eq!((r["slot"].clone(), r["bass"].clone()), (json!(5), json!(4)));
    let id = r["chord"].as_u64().unwrap_or_default();
    let c = e.session().band.chords.iter().find(|c| u64::from(c.id) == id).cloned().unwrap_or_default();
    assert_eq!((c.tick, c.length_ticks), (4 * 3840, 3840));

    assert!(refused(&mut e, "band.chord.add", json!({"at": 0, "chord": "zz"})).contains("zz"));
    assert!(refused(&mut e, "band.chord.add", json!({"at": 0, "chord": 112})).contains("0-111"));
    assert!(refused(&mut e, "band.chord.add", json!({"at": 0, "chord": "C", "bass": 12})).contains("-1-11"));

    ok(&mut e, "band.chord.set", json!({"chord": id, "chord_text": "G7"}));
    let g7 = match_chord(&parse_chord_name("G7").unwrap_or_default(), 0, 0);
    let c = e.session().band.chords.iter().find(|c| u64::from(c.id) == id).cloned().unwrap_or_default();
    assert_eq!(i32::from(c.slot), g7.slot);
    ok(&mut e, "band.chord.remove", json!({"chord": id}));
    assert!(e.session().band.chords.iter().all(|c| u64::from(c.id) != id));
    assert!(refused(&mut e, "band.chord.remove", json!({"chord": id})).contains(&id.to_string()));
}

#[test]
fn section_copy_shares_its_definition() {
    let mut e = Engine::default();
    let bar = bar(&e);
    let b4 = 4 * bar;
    ok(&mut e, "band.def", json!({"id": "B", "name": "Chorus", "musicians": [{"plays": false}, {"energy": 4}], "feel": [-1, 2, -1, -1, 1]}));
    assert_eq!(e.undo_label(), Some("Section Definition"));
    let r = ok(&mut e, "band.section.add", json!({"section": "B", "start": 0, "end": b4}));
    let first = r["block"].as_u64().unwrap_or_default();
    let r = ok(&mut e, "band.section.copy", json!({"block": first, "to": b4}));
    let copy = r["block"].as_u64().unwrap_or_default();
    assert_ne!(first, copy);
    ok(&mut e, "band.def", json!({"id": "B", "energy_shift": 2}));
    let band = &e.session().band;
    assert_eq!(band.defs.iter().filter(|d| d.id == 'B').count(), 1);
    let d = band.def('B').cloned().unwrap_or_default();
    assert_eq!((d.name.as_str(), d.energy_shift, d.musicians[0].plays, d.musicians[1].energy, d.feel), ("Chorus", 2, false, 4, [-1, 2, -1, -1, 1]));
    let blocks: Vec<(char, i64, i64)> = band.sections.iter().map(|b| (b.section, b.start_tick, b.length_ticks)).collect();
    assert_eq!(blocks, vec![('B', 0, 4 * 3840), ('B', 4 * 3840, 4 * 3840)]);

    // The chip, a move, and the definition guarded while used.
    ok(&mut e, "band.section.set", json!({"block": copy, "energy": -2, "take": 3, "start": 8 * bar}));
    let c = e.session().band.sections.iter().find(|b| u64::from(b.id) == copy).cloned().unwrap_or_default();
    assert_eq!((c.energy, c.take, c.start_tick, c.length_ticks), (-2, 3, 8 * 3840, 4 * 3840));
    assert!(refused(&mut e, "band.def.delete", json!({"id": "B"})).contains('B'));
    assert!(refused(&mut e, "band.section.add", json!({"section": "Q", "start": 0, "end": b4})).contains('Q'));
    assert!(refused(&mut e, "band.section.add", json!({"section": "A", "start": b4, "end": 0})).contains("end"));
    ok(&mut e, "band.section.remove", json!({"block": first}));
    ok(&mut e, "band.section.remove", json!({"block": copy}));
    ok(&mut e, "band.def.delete", json!({"id": "B"}));
    assert!(e.session().band.def('B').is_none());
    assert!(refused(&mut e, "band.def.delete", json!({"id": "A"})).contains("one"));

    // A block laid over another cuts it.
    ok(&mut e, "band.section.add", json!({"section": "A", "start": 0, "end": b4}));
    ok(&mut e, "band.section.add", json!({"section": "A", "start": bar, "end": 2 * bar}));
    let spans: Vec<(i64, i64)> = e.session().band.sections.iter().map(|b| (b.start_tick, b.length_ticks)).collect();
    assert_eq!(spans, vec![(0, 3840), (3840, 3840), (2 * 3840, 2 * 3840)]);
}

#[test]
fn factory_song_lays_identical_lanes() {
    let songs = factory_songs();
    assert_eq!(songs.len(), 5);
    for f in &songs {
        let mut e = Engine::default();
        ok(&mut e, "band.chord.add", json!({"at": 0, "chord": "C"}));
        let slots = e.session().band.slots.clone();
        ok(&mut e, "band.song.factory", json!({"name": f.name}));
        assert_eq!(e.undo_label(), Some("Factory Song"));
        let s = e.session();
        assert_eq!((i32::from(s.band.key), i32::from(s.band.scale), i32::from(s.band.take)), (f.key, f.scale, f.take.clamp(1, 16)), "{}", f.name);
        assert_eq!(s.band.feel.tension, f.tension_on(), "{}", f.name);
        assert_eq!(s.band.slots, slots, "{}: players and targets untouched", f.name);
        let lanes = lane_song(s).unwrap_or_else(|| panic!("{}: lanes", f.name));
        let got = lanes_to_timeline(&lanes).unwrap_or_else(|| panic!("{}: a timeline", f.name));
        let mut song = f.song.clone();
        // The session keeps the dials, not the preset's name.
        song.feel.preset.clear();
        song.feel.edited = false;
        let want = build_timeline(&song);
        assert_eq!(*got, *want, "{}", f.name);
        let mut ids: Vec<u32> = s.band.sections.iter().map(|b| b.id).chain(s.band.chords.iter().map(|c| c.id)).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "{}: unique ids", f.name);
        assert!(ids.iter().all(|i| *i < s.band.next_id));
    }
    let mut e = Engine::default();
    assert!(refused(&mut e, "band.song.factory", json!({"name": "Nope"})).contains("Nope"));
}

#[test]
fn preset_load_is_one_undo_step() {
    let mut e = Engine::default();
    let presets = factory_band_presets();
    let p = presets.iter().find(|p| p.name != "Factory Band").unwrap_or_else(|| panic!("a second factory band"));
    let undo = e.undo_history().len();
    let r = ok(&mut e, "band.preset.load", json!({"name": p.name}));
    assert_eq!(r["band"], json!(p.name));
    assert_eq!(e.undo_history().len(), undo + 1);
    assert_eq!(e.undo_label(), Some("Load Band Preset"));
    let b = &e.session().band;
    assert_eq!(b.band_name, p.name);
    for (k, m) in p.musicians.iter().enumerate() {
        assert_eq!(b.slots[k].enabled, m.enabled, "musician {}", k + 1);
        assert_eq!(INSTRUMENT_NAMES[usize::from(b.slots[k].instrument)].to_lowercase(), m.instrument.to_lowercase(), "musician {}", k + 1);
    }
    e.undo();
    assert_eq!(e.session().band, BandState::default());
    assert!(refused(&mut e, "band.preset.load", json!({"name": "No Such Band"})).contains("No Such Band"));
    let list = ok(&mut e, "band.preset.list", json!({}));
    assert!(list["presets"].as_array().is_some_and(|a| a.iter().any(|x| x["name"] == json!(p.name))));
}

#[test]
fn user_folder_presets_round_trip() {
    let dir = std::env::temp_dir().join(format!("soundcraft-band-t10-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut e = Engine::default();
    // No user folder: saving is refused.
    assert!(e.execute("band.preset.save", &json!({"name": "Mine"})).is_err());
    e.set_band_user_folder(Some(dir.clone()));
    assert_eq!(e.band_user_folder(), Some(dir.as_path()));
    ok(&mut e, "band.slot", json!({"slot": 2, "energy": 2}));
    ok(&mut e, "band.preset.save", json!({"name": "Mine"}));
    assert!(dir.join("Bands").join("Mine.json").is_file());
    assert!(e.execute("band.preset.save", &json!({"name": "Mine"})).is_err(), "no silent overwrite");
    ok(&mut e, "band.preset.save", json!({"name": "Mine", "replace": true}));
    ok(&mut e, "band.slot", json!({"slot": 2, "energy": 4}));
    ok(&mut e, "band.preset.load", json!({"name": "Mine"}));
    assert_eq!(e.session().band.slots[1].energy, 2);
    assert_eq!(e.session().band.band_name, "Mine");

    ok(&mut e, "band.settings", json!({"song_feel": {"on": true, "density": 3, "touch": 2}}));
    ok(&mut e, "band.feel_preset.save", json!({"name": "Loud"}));
    assert!(dir.join("FeelPresets.json").is_file());
    ok(&mut e, "band.settings", json!({"song_feel": {"density": 0, "touch": 0}}));
    ok(&mut e, "band.settings", json!({"feel_preset": "Loud"}));
    assert_eq!((e.session().band.song_feel.density, e.session().band.song_feel.touch), (3, 2));
    let list = ok(&mut e, "band.list", json!({}));
    for k in ["state", "players_by_instrument", "targets", "presets", "factory_songs", "feel_presets", "library_errors"] {
        assert!(list.get(k).is_some(), "band.list has {k}");
    }
    assert!(list["feel_presets"].as_array().is_some_and(|a| a.iter().any(|x| x["name"] == json!("Loud"))));
    ok(&mut e, "band.feel_preset.delete", json!({"name": "Loud"}));
    assert!(e.execute("band.feel_preset.delete", &json!({"name": "Loud"})).is_err());
    ok(&mut e, "band.library.reload", json!({}));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn musician_is_never_an_output() {
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    for name in ["musician:1", "Musician 2"] {
        let err = e.execute("track.output", &json!({"tracks": [t.0], "output": name})).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("A musician can only be a track input"), "{name}: {err}");
    }
    assert_ne!(e.session().track(t).map(|t| t.mixer.output.clone()), Some(Route::Musician(0)));
}

#[test]
fn a_musician_is_only_a_midi_or_instrument_tracks_input() {
    let mut e = Engine::default();
    let audio = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let aux = e.session_mut().add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
    let midi = e.session_mut().add_track(TrackKind::Midi, ChannelFormat::Stereo, None);
    let undo = e.undo_history().len();
    for t in [audio, aux] {
        let before = e.session().track(t).map(|t| t.mixer.input.clone());
        let err = e.execute("track.input", &json!({"tracks": [t.0], "input": "musician:1"})).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("A musician plays into a MIDI or instrument track"), "{err}");
        assert_eq!(e.session().track(t).map(|t| t.mixer.input.clone()), before);
    }
    // One of them in the list refuses the whole call.
    let err = e.execute("track.input", &json!({"tracks": [midi.0, audio.0], "input": "musician:1"})).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(err.contains("A musician plays into a MIDI or instrument track"), "{err}");
    assert_eq!(e.undo_history().len(), undo);
    ok(&mut e, "track.input", json!({"tracks": [midi.0], "input": "musician:1"}));
    assert_eq!(e.session().track(midi).map(|t| t.mixer.input.clone()), Some(Route::Musician(0)));
}

#[test]
fn input_musician_parses_both_spellings() {
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let input = |e: &Engine| e.session().track(t).map(|t| t.mixer.input.clone());
    let before = input(&e);
    ok(&mut e, "track.input", json!({"tracks": [t.0], "input": "musician:3"}));
    assert_eq!(input(&e), Some(Route::Musician(2)));
    ok(&mut e, "track.input", json!({"tracks": [t.0], "input": "Musician 6"}));
    assert_eq!(input(&e), Some(Route::Musician(5)));
    assert_eq!(e.session().busses.len(), 0, "never a bus");
    e.undo();
    assert_eq!(input(&e), Some(Route::Musician(2)));
    e.undo();
    assert_eq!(input(&e), before);
    for bad in ["musician:7", "Musician 0", "musician:x"] {
        assert!(e.execute("track.input", &json!({"tracks": [t.0], "input": bad})).is_err(), "{bad}");
    }
}

/// A format-0 MIDI file at 480 ppq in 4/4: each chord a whole note.
fn chord_file(chords: &[[u8; 3]]) -> Vec<u8> {
    fn vlq(mut v: u32, out: &mut Vec<u8>) {
        let mut stack = vec![(v & 0x7f) as u8];
        v >>= 7;
        while v > 0 {
            stack.push((v & 0x7f) as u8 | 0x80);
            v >>= 7;
        }
        out.extend(stack.iter().rev());
    }
    let mut trk = vec![0x00, 0xff, 0x58, 0x04, 0x04, 0x02, 0x18, 0x08];
    for c in chords {
        for n in c {
            trk.extend([0x00, 0x90, *n, 90]);
        }
        for (i, n) in c.iter().enumerate() {
            vlq(if i == 0 { 1920 } else { 0 }, &mut trk);
            trk.extend([0x80, *n, 0]);
        }
    }
    trk.extend([0x00, 0xff, 0x2f, 0x00]);
    let mut f = b"MThd".to_vec();
    f.extend([0, 0, 0, 6, 0, 0, 0, 1, 0x01, 0xe0]);
    f.extend(b"MTrk");
    f.extend((trk.len() as u32).to_be_bytes());
    f.extend(trk);
    f
}

#[test]
fn imported_chords_land_on_the_chord_lane_only() {
    let path = std::env::temp_dir().join(format!("soundcraft-band-t10-{}.mid", std::process::id()));
    std::fs::write(&path, chord_file(&[[60, 64, 67], [65, 69, 72], [67, 71, 74], [60, 64, 67]])).unwrap_or_else(|err| panic!("{err}"));
    let mut e = Engine::default();
    let defs = e.session().band.defs.clone();
    let at = bar(&e);
    let r = ok(&mut e, "band.chords.import", json!({"path": path.to_string_lossy(), "at": at}));
    let _ = std::fs::remove_file(&path);
    assert_eq!(e.undo_label(), Some("Import Chords"));
    let b = &e.session().band;
    assert_eq!(r["chords"], json!(b.chords.len()));
    assert!(!b.chords.is_empty());
    assert!(b.sections.is_empty() && b.defs == defs && b.key == 0, "the chord lane only");
    assert_eq!(b.chords[0].tick, 3840, "laid from `at`");
    let c = match_chord(&parse_chord_name("C").unwrap_or_default(), 0, 0).slot;
    assert_eq!(i32::from(b.chords[0].slot), c);
    assert!(e.execute("band.chords.import", &json!({"path": "Z:/no/such/file.mid"})).is_err());
}

fn ids_unique(e: &Engine) {
    let b = &e.session().band;
    let mut ids: Vec<u32> = b.sections.iter().map(|x| x.id).chain(b.chords.iter().map(|c| c.id)).collect();
    let n = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), n, "unique ids: {:?} / {:?}", b.sections, b.chords);
    assert!(ids.iter().all(|i| *i < b.next_id));
}

#[test]
fn laying_inside_a_long_item_keeps_ids_unique() {
    let mut e = Engine::default();
    let bar = bar(&e);
    ok(&mut e, "band.section.add", json!({"section": "A", "start": 0, "end": 32 * bar}));
    ok(&mut e, "band.chord.add", json!({"at": 0, "end": 32 * bar, "chord": "C"}));
    ok(&mut e, "band.song.factory", json!({"name": "12-bar Blues", "at": 4 * bar, "replace": false}));
    ids_unique(&e);
    let b = &e.session().band;
    assert!(b.sections.iter().any(|x| x.start_tick == 16 * 3840), "the long block's tail is kept");
    assert!(b.chords.iter().any(|c| c.tick == 16 * 3840), "the long chord's tail is kept");

    let path = std::env::temp_dir().join(format!("soundcraft-band-t10-ids-{}.mid", std::process::id()));
    std::fs::write(&path, chord_file(&[[60, 64, 67], [65, 69, 72]])).unwrap_or_else(|err| panic!("{err}"));
    let mut e = Engine::default();
    ok(&mut e, "band.chord.add", json!({"at": 0, "end": 32 * bar, "chord": "C"}));
    ok(&mut e, "band.chords.import", json!({"path": path.to_string_lossy(), "at": 4 * bar}));
    let _ = std::fs::remove_file(&path);
    ids_unique(&e);
    assert_eq!(e.session().band.chords.len(), 4, "head, two imported chords, tail");
}

#[test]
fn a_full_lane_counts_after_the_cut() {
    let mut e = Engine::default();
    let half = bar(&e) / 2;
    e.session_mut().band.chords = (0..soundcraft_model::MAX_CHORDS as u32 - 1)
        .map(|i| soundcraft_model::ChordEvent { id: i + 1, tick: i64::from(i) * 7680, length_ticks: 7680, slot: 0, bass: -1 })
        .collect();
    e.session_mut().band.next_id = soundcraft_model::MAX_CHORDS as u32 + 1;
    // Inside one two-bar chord: it splits, so the lane would overflow.
    assert!(refused(&mut e, "band.chord.add", json!({"at": 2 * half, "end": 3 * half, "chord": "F"})).contains("full"));
    // Exactly over one: it goes, so there is room.
    ok(&mut e, "band.chord.add", json!({"at": 4 * half, "end": 8 * half, "chord": "F"}));
    assert_eq!(e.session().band.chords.len(), soundcraft_model::MAX_CHORDS - 1);
}

#[test]
fn musician_names_are_reserved_and_time_is_the_songs() {
    let mut e = Engine::default();
    for (action, name) in [("create_input", "Musician 1"), ("create_bus", "musician:3"), ("create_output", "MUSICIAN 2")] {
        let err = e.execute("setup.io", &json!({"action": action, "name": name})).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("reserved"), "{action} {name}: {err}");
    }
    assert!(refused(&mut e, "band.def", json!({"id": "A", "feel": [1, -1, -1, -1, -1]})).contains("time"));
}
