//! Library loader, user overlay and band presets: MIDI Studio's rules (`LibraryLoader.cpp`, `PresetStore.cpp`).

use std::path::{Path, PathBuf};

use soundcraft_band::band_preset::{BandPreset, band_preset_json, factory_band_presets, parse_band_preset, save_band_preset, user_band_presets};
use soundcraft_band::library::{
    Driver, K_INSTRUMENTS, K_PLAYER_SLOTS, LibraryOverlay, LibrarySource, embedded, embedded_library, factory_player_names, factory_players,
    load_library,
};

fn manifest() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default())
}

/// A fresh, empty temp folder for one test.
fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("soundcraft_band_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("temp dir");
    d
}

fn embedded_text(path: &str) -> String {
    let b: &[u8] = embedded::files().iter().find(|(p, _)| *p == path).map(|(_, b)| *b).expect("embedded file");
    String::from_utf8(b.to_vec()).expect("utf-8")
}

#[test]
fn embedded_library_loads_without_errors() {
    let lib = embedded_library();
    assert_eq!(lib.errors, Vec::<String>::new());
    assert_eq!(lib.personas.len(), 152);
    assert_eq!(lib.target(0).map(|t| t.name.as_str()), Some("Generic"));
    assert_eq!(lib.target(0).map(|t| t.driver), Some(Driver::Generic));
    assert!(lib.target_slot_for_name("MODO DRUM") >= 0);
    assert_eq!(lib.target_store.len(), 28);
    // Cached: the same Arc every time.
    assert!(std::sync::Arc::ptr_eq(&lib, &embedded_library()));
    // Spot checks against MS's own LibraryTest expectations.
    assert!(lib.persona(6).is_some_and(|p| p.slap) && lib.persona(1).is_some_and(|p| !p.slap), "Dex slap flag");
    let t1 = lib.target(1).expect("slot 1");
    assert_eq!((t1.driver, t1.drum_rules.len(), t1.drum_rules[0].out, t1.drum_rules[0].side), (Driver::DrumMap, 9, 64, 2));
    let t11 = lib.target(11).expect("slot 11");
    assert_eq!(t11.name, "Embertone Joshua Bell Violin");
    assert!(t11.driver == Driver::Swam && !t11.swam_keyswitches && t11.swam_velocity_dynamics && t11.swam_custom_arts);
    assert_eq!((t11.swam_vel_lo, t11.swam_vel_hi, t11.swam_arts[0].key, t11.swam_arts[3].key), (18, 118, 36, 37));
    assert!(t11.swam_arts[3].instant);
}

/// The roster (seed per instrument and slot 1..16) as MS's loader builds it from the shipped Library. Generated
/// once by reading `MIDI Studio/Library/Players` exactly as `loadLibrary` does (files sorted by name, case-insensitive;
/// the first file keeps a slot); it agrees with MS's frozen `Tests/tables-ref.json` roster wherever that is set,
/// except Guitar slot 1, which the hand-made player Alton (seed 123) now takes before Hollis (seed 11).
const ROSTER: [[i32; K_PLAYER_SLOTS]; K_INSTRUMENTS] = [
    [1, 2, 3, 4, 28, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 0],        // Piano
    [20, 21, 22, 38, 92, 93, 96, 77, 18, 0, 0, 0, 0, 0, 0, 0],          // Synth
    [5, 6, 7, 24, 25, 29, 41, 52, 53, 54, 55, 56, 57, 58, 59, 60],      // Bass
    [1, 2, 3, 4, 28, 61, 62, 63, 64, 65, 0, 0, 0, 0, 0, 0],             // Electric Keys
    [0, 19, 0, 40, 0, 0, 0, 97, 98, 0, 0, 0, 0, 0, 0, 0],               // Harp & Mallets
    [8, 9, 10, 26, 27, 30, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51],     // Drums
    [123, 12, 13, 31, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],              // Guitar
    [14, 15, 17, 32, 33, 34, 35, 66, 67, 68, 69, 70, 71, 72, 73, 74],   // Strings
    [22, 23, 20, 39, 94, 95, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],             // Winds
    [16, 17, 36, 37, 38, 88, 89, 90, 91, 0, 0, 0, 0, 0, 0, 0],          // Brass
    [140, 141, 142, 143, 0, 145, 146, 147, 148, 149, 0, 0, 0, 0, 0, 0], // Ethnic
    [130, 131, 132, 133, 134, 135, 136, 137, 0, 0, 0, 0, 0, 0, 0, 0],   // Beat
    [100, 152, 150, 151, 144, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],         // Folk
];

#[test]
fn matches_cpp_library_counts() {
    let lib = embedded_library();
    for (i, row) in ROSTER.iter().enumerate() {
        assert_eq!(&lib.roster[i], row, "roster of instrument {i}");
        // Browser order: by the player's slot number for the instrument (a hint several players may share; none =
        // last), then by name ignoring case. The roster's players appear in it in slot order.
        let key = |seed: i32| {
            let p = lib.persona(seed).expect("listed player");
            let slot = p.slots.iter().find(|(inst, _)| *inst == i as i32).map(|&(_, s)| s).expect("plays it");
            (if slot > 0 { slot } else { i32::MAX }, p.name.to_uppercase())
        };
        let keys: Vec<_> = lib.players[i].iter().map(|&s| key(s)).collect();
        assert!(keys.windows(2).all(|w| w[0] <= w[1]), "browser order of instrument {i}: {keys:?}");
        let numbered: Vec<i32> = row.iter().copied().filter(|&s| s > 0).collect();
        let listed: Vec<i32> = lib.players[i].iter().copied().filter(|s| numbered.contains(s)).collect();
        assert_eq!(listed, numbered, "roster players in slot order, instrument {i}");
        let expected = lib.personas.iter().filter(|p| p.slots.iter().any(|(inst, _)| *inst == i as i32)).count();
        assert_eq!(lib.players[i].len(), expected, "every player of instrument {i} is listed once");
    }
    // Bass browser order: numbered slots first (Cole, seed 41, is Bass slot 7).
    assert_eq!(lib.players[2].get(6), Some(&41));
}

/// Copies `tests/fixtures/library` into a temp folder and adds a 10 MB player file.
fn bad_user_folder() -> PathBuf {
    let d = temp_dir("bad");
    let players = d.join("Players");
    std::fs::create_dir_all(&players).expect("Players");
    for e in std::fs::read_dir(manifest().join("tests/fixtures/library/Players")).expect("fixtures").flatten() {
        std::fs::copy(e.path(), players.join(e.file_name())).expect("copy fixture");
    }
    std::fs::write(players.join("Big.json"), vec![b' '; 10 * 1024 * 1024]).expect("big file");
    d
}

#[test]
fn bad_user_files_are_skipped_and_named() {
    let d = bad_user_folder();
    let lib = load_library(&LibrarySource::with_user_folder(&d), &LibraryOverlay::default());
    let mut errors = lib.errors.clone();
    errors.sort();
    assert_eq!(
        errors,
        vec![
            "Players/Big.json: too large (over 4 MB)".to_string(),
            "Players/NanGrid.json: not valid JSON (99:10: error: Syntax error)".to_string(),
            "Players/SeedHigh.json: seed must be 1..255 (got 999)".to_string(),
            "Players/SeedZero.json: seed must be 1..255 (got 0)".to_string(),
            "Players/Truncated.json: not valid JSON (112:1: error: Expected ',' or ']')".to_string(),
            "Players/ZzDupSeed.json: duplicate seed 41 (also Players/Cole.json)".to_string(),
        ]
    );
    // The rest loads: the 152 factory players, and Cole keeps seed 41 and Bass slot 7.
    assert_eq!(lib.personas.len(), 152);
    assert_eq!(lib.persona(41).map(|p| p.name.as_str()), Some("Cole"));
    assert_eq!(lib.roster[2][6], 41);
    assert_eq!(lib.seed_for_name("ZzDup"), 0);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn stray_target_data_keys_are_rejected_on_every_driver() {
    let mut src = LibrarySource::embedded();
    let mut drivers = Vec::new();
    for (path, bytes) in &mut src.files {
        if !path.starts_with("Targets/") {
            continue;
        }
        let mut j: serde_json::Value = serde_json::from_slice(bytes).expect("target json");
        drivers.push(j["driver"].as_str().unwrap_or_default().to_string());
        if !j["data"].is_object() {
            j["data"] = serde_json::json!({});
        }
        j["data"]["bogus"] = serde_json::json!(1);
        *bytes = serde_json::to_vec(&j).expect("json");
    }
    drivers.sort();
    drivers.dedup();
    assert!(drivers.len() >= 5);
    let lib = load_library(&src, &LibraryOverlay::default());
    for d in &drivers {
        let needle = format!("bogus is not a known target setting for driver {d} (see README.md)");
        assert!(lib.errors.iter().any(|e| e.ends_with(&needle)), "{d}: {:?}", lib.errors);
    }
    // Every target was rejected, so slot 0 falls back to the built-in Generic.
    assert_eq!(lib.target(0).map(|t| t.file.as_str()), Some("(built in)"));
}

#[test]
fn user_file_overrides_embedded_by_name() {
    let d = temp_dir("override");
    std::fs::create_dir_all(d.join("Players")).expect("Players");
    std::fs::create_dir_all(d.join("Targets")).expect("Targets");
    let cole = embedded_text("Players/Cole.json").replace("Pop · steady 8th roots", "user copy");
    std::fs::write(d.join("Players/Cole.json"), cole).expect("write");
    let drum = embedded_text("Targets/MODO DRUM.json").replacen("\"name\": \"MODO DRUM\"", "\"name\": \"My Drums\"", 1);
    std::fs::write(d.join("Targets/MODO DRUM.json"), drum).expect("write");
    let lib = load_library(&LibrarySource::with_user_folder(&d), &LibraryOverlay::default());
    assert_eq!(lib.errors, Vec::<String>::new());
    assert_eq!(lib.personas.len(), 152);
    let cole = lib.persona(41).expect("Cole");
    assert_eq!((cole.name.as_str(), cole.desc.as_str(), cole.file.as_str()), ("Cole", "user copy", "Players/Cole.json"));
    assert_eq!(lib.target_slot_for_name("MODO DRUM"), -1);
    assert_eq!(lib.target(1).map(|t| t.name.as_str()), Some("My Drums"));
    assert_eq!(lib.target_store.len(), 28);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn overlay_wins_and_shadows() {
    let cole = embedded_text("Players/Cole.json").replace("Pop · steady 8th roots", "song copy");
    let drum = embedded_text("Targets/MODO DRUM.json");
    let overlay = LibraryOverlay { players: vec![cole, "{ broken".to_string()], targets: vec![drum] };
    let lib = load_library(&LibrarySource::embedded(), &overlay);
    // MS's loader names the slot clash before it sees the Library target is shadowed by the song's copy, so a
    // shadowed target (unlike a shadowed player) is also an error. Ported as is.
    assert_eq!(lib.errors, vec!["Targets/MODO DRUM.json: target slot 1 taken by (song)".to_string()]);
    assert_eq!(lib.personas.len(), 152);
    let cole = lib.persona(41).expect("Cole");
    assert_eq!((cole.desc.as_str(), cole.file.as_str()), ("song copy", "(song)"));
    assert_eq!(lib.roster[2][6], 41);
    assert_eq!(lib.overlay_names, vec!["Cole".to_string(), "MODO DRUM".to_string()]);
    assert_eq!(lib.shadowed, vec!["Players/Cole.json".to_string(), "Targets/MODO DRUM.json".to_string()]);
    let slot = lib.target_slot_for_name("MODO DRUM");
    assert_eq!(lib.target(slot).map(|t| t.file.as_str()), Some("(song)"));
    assert_eq!(lib.target_store.len(), 28);
}

#[test]
fn factory_players_come_from_factory_json() {
    let names = factory_players();
    assert_eq!(names.len(), 152);
    assert_eq!(names.first().map(String::as_str), Some("Clay"));
    let lib = embedded_library();
    assert!(names.iter().all(|n| lib.seed_for_name(n) > 0));
    // Parsed once: every call reads the same names.
    assert_eq!(factory_player_names(), names.as_slice());
    assert!(std::ptr::eq(factory_player_names(), factory_player_names()));
}

#[test]
fn factory_band_presets_parse_and_round_trip() {
    let presets = factory_band_presets();
    assert_eq!(presets.len(), 18);
    assert!(presets.iter().all(|p| p.factory));
    assert_eq!(presets.first().map(|p| p.name.as_str()), Some("Acoustic Worship"));
    for p in &presets {
        let back = parse_band_preset(&band_preset_json(p)).unwrap_or_else(|e| panic!("{}: {e}", p.name));
        assert_eq!(&back, p);
    }
    let blue = presets.iter().find(|p| p.name == "Bluegrass").expect("Bluegrass");
    let m = &blue.musicians[0];
    assert_eq!((m.enabled, m.mute, m.channel), (true, false, 1));
    assert_eq!((m.instrument.as_str(), m.player.as_str(), m.target.as_str()), ("Guitar", "Clem", "Ample Guitar M 4"));
    assert_eq!(m.params.get("energy"), Some(&3.0));
    assert_eq!(m.params.get("idea"), None, "fields outside musicianPresetFields are dropped");
    assert_eq!(blue.tags, vec!["bluegrass".to_string(), "country".to_string()]);
}

#[test]
fn band_preset_errors_are_mss() {
    assert_eq!(parse_band_preset("{ nope").err().as_deref(), Some("not a band preset"));
    assert_eq!(parse_band_preset(r#"{"kind":"band","name":" "}"#).err().as_deref(), Some("the preset has no name"));
    assert_eq!(parse_band_preset(r#"{"kind":"band","name":"X","musicians":[]}"#).err().as_deref(), Some("a band preset needs six musicians"));
    let six = r#"{"kind":"band","name":"X","musicians":[{"instrument":"Piano"},{"instrument":"Kazoo"},{},{},{},{}]}"#;
    assert_eq!(parse_band_preset(six).err().as_deref(), Some("musician 2: unknown instrument \"Kazoo\""));
    let ok = r#"{"kind":"band","name":"X","musicians":[{"instrument":"piano","channel":40},{"instrument":"Bass"},
        {"instrument":"Drums"},{"instrument":"Guitar"},{"instrument":"Strings"},{"instrument":"Brass"}]}"#;
    let b = parse_band_preset(ok).expect("parses");
    assert_eq!((b.musicians[0].channel, b.musicians[1].channel, b.musicians[0].player.as_str()), (16, 2, "Session"));
    assert_eq!(b.musicians[0].instrument, "piano");
}

fn preset_named(name: &str) -> BandPreset {
    let mut p = factory_band_presets().into_iter().find(|p| p.name == "Bluegrass").expect("Bluegrass");
    p.name = name.to_string();
    p
}

#[test]
fn user_band_presets_save_and_list() {
    let d = temp_dir("bands");
    let p = preset_named("My: Band");
    save_band_preset(&d, &p, false).expect("save");
    assert!(Path::new(&d.join("My Band.json")).is_file(), "the file name is made legal as MS does");
    assert_eq!(save_band_preset(&d, &p, false).err().as_deref(), Some("A band called \"My: Band\" already exists."));
    save_band_preset(&d, &p, true).expect("replace");
    std::fs::write(d.join("Fac.json"), band_preset_json(&BandPreset { factory: true, ..preset_named("Fac") })).expect("write");
    assert_eq!(save_band_preset(&d, &preset_named("Fac"), true).err().as_deref(), Some("\"Fac\" is a factory band: save under another name."));
    assert_eq!(save_band_preset(&d, &preset_named("  "), true).err().as_deref(), Some("Give the band a name."));
    std::fs::write(d.join("Junk.json"), "{ \"kind\": \"song\" }").expect("write");
    let (list, skipped) = user_band_presets(&d);
    assert_eq!(list.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(), vec!["Fac", "My: Band"]);
    let mut saved = list[1].clone();
    assert!(!saved.factory);
    saved.factory = p.factory;
    assert_eq!(saved, p);
    assert_eq!(skipped, vec!["Junk.json: not a band preset".to_string()]);
    assert!(!d.join("My Band.json.tmp").exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(windows)]
#[test]
fn unreadable_user_files_are_named() {
    use std::os::windows::fs::OpenOptionsExt as _;
    let d = temp_dir("unreadable");
    std::fs::create_dir_all(d.join("Players")).expect("Players");
    let path = d.join("Players/Cole.json");
    std::fs::write(&path, embedded_text("Players/Cole.json")).expect("write");
    // Held open with no sharing: reading it fails while this handle lives.
    let lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(&path).expect("lock");
    let lib = load_library(&LibrarySource::with_user_folder(&d), &LibraryOverlay::default());
    drop(lock);
    assert_eq!(lib.errors, vec!["Players/Cole.json: could not be read".to_string()]);
    assert_eq!(lib.seed_for_name("Cole"), 0, "the user's file stands in for the embedded one");
    assert_eq!(lib.personas.len(), 151);
    let _ = std::fs::remove_dir_all(&d);
}
