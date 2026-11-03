use super::*;

fn count(prefix: &str, suffix: &str) -> usize {
    embedded::files().iter().filter(|(p, _)| p.starts_with(prefix) && p.ends_with(suffix)).count()
}

#[test]
fn embedded_library_is_complete() {
    assert_eq!(count("Players/", ".json"), 152);
    assert_eq!(count("Targets/", ".json"), 28);
    assert_eq!(count("Presets/Bands/", ".json"), 18);
    assert_eq!(count("Songs/Factory/", ".song.json"), 5);
    assert!(embedded::files().iter().any(|(p, b)| *p == "factory.json" && !b.is_empty()));
    let paths: Vec<&str> = embedded::files().iter().map(|(p, _)| *p).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
    assert!(embedded::files().iter().any(|(p, _)| *p == "Players/Clay.json"));
}

#[test]
fn make_empty_has_generic_at_slot_0() {
    let lib = LibraryData::make_empty();
    let t = lib.target(0).expect("slot 0");
    assert_eq!(t.name, "Generic");
    assert_eq!(t.driver, Driver::Generic);
    assert!(t.instruments.iter().all(|&b| b));
    assert!(lib.targets[1..].iter().all(Option::is_none));
    assert!(lib.personas.is_empty());
    assert_eq!(lib.target_slot_for_name("Generic"), 0);
    assert_eq!(lib.target_slot_for_name("nope"), -1);
    assert_eq!(lib.seed_for_name("nope"), 0);
    assert!(!lib.plays(-1, 1) && !lib.plays(99, 1) && !lib.plays(0, 1));
    assert_eq!(lib.slot_for(0, 1), 0);
}

#[test]
fn target_calibration_matches_cpp() {
    let t = TargetData { vel_lo: 20, vel_hi: 110, vel_curve: 1.5, ..TargetData::default() };
    let expect = (20.0 + 90.0 * (64.0_f64 / 127.0).powf(1.5)).round() as i32;
    assert_eq!(t.calibrated(64), expect);
    assert_eq!(TargetData::default().calibrated(64), 64);
    assert_eq!(t.calibrated(0), t.calibrated(1));
}

#[test]
fn relink_and_roster_lookups() {
    let mut lib = LibraryData::default();
    lib.personas.push(PersonaData { name: "A".into(), seed: 7, ..PersonaData::default() });
    lib.personas.push(PersonaData { name: "B".into(), seed: 300, ..PersonaData::default() });
    lib.roster[2][3] = 7;
    lib.players[2].push(7);
    lib.relink();
    assert_eq!(lib.persona(7).map(|p| p.name.as_str()), Some("A"));
    assert!(lib.persona(300).is_none() && lib.persona(-1).is_none());
    assert_eq!(lib.slot_for(2, 7), 4);
    assert!(lib.plays(2, 7));
    assert_eq!(lib.seed_for_name("B"), 300);
}

#[test]
fn ample_helpers() {
    let mut p = PersonaData { role: 3, ..PersonaData::default() };
    assert!(ample_strums(&p, 1, 0) && !ample_strums(&p, 2, 0));
    assert!(ample_strums(&p, 0, 1)); // guitarist, mode -1: auto, strumAt all true
    assert!(!ample_mixed(&p));
    p.ample.strum_at = [true, false, true, true];
    assert!(ample_mixed(&p));
    p.role = 0;
    assert!(!ample_strums(&p, 0, 0));
}
