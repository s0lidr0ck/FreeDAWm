//! Event-for-event fidelity: every `tests/scenarios/*.json` played through `BandRuntime` must give the lines MIDI
//! Studio's own processor gave for it (`tests/refs/<name>.events`, made by `cargo xtask band-refs`, git-ignored).
//!
//! - `"pending": true` prints `PENDING <name>` (the port task that covers it sets it to false).
//! - A missing ref, or one made from other scenario bytes (its `# case` line), prints `SKIPPED`; under
//!   `FIDELITY_STRICT=1` that is a failure.
//! - Otherwise the lines must be equal; the first mismatch is printed with three lines of context.

mod common;

use std::path::Path;

fn strict() -> bool {
    std::env::var_os("FIDELITY_STRICT").is_some_and(|v| v == "1")
}

#[test]
fn scenarios() {
    let dir = common::crate_dir().join("tests").join("scenarios");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
        .unwrap_or_default();
    files.sort();
    assert!(!files.is_empty(), "no scenarios in {}", dir.display());
    let refs = common::crate_dir().join("tests").join("refs");
    let mut failures = Vec::new();
    let (mut passed, mut pending, mut skipped) = (0, 0, 0);
    for path in &files {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("FAIL {stem}: {e}"));
                continue;
            }
        };
        let s = match common::read(&bytes) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("FAIL {stem}: {e}"));
                continue;
            }
        };
        if s.name != stem {
            failures.push(format!("FAIL {stem}: name \"{}\" is not the file's name", s.name));
            continue;
        }
        if s.pending {
            println!("PENDING {stem}");
            pending += 1;
            continue;
        }
        let reference = match read_ref(&refs.join(format!("{stem}.events")), &common::scenario_case_line(&s, &bytes)) {
            Ok(r) => r,
            Err(why) => {
                println!("SKIPPED {stem}: {why} (cargo xtask band-refs {stem})");
                skipped += 1;
                if strict() {
                    failures.push(format!("FAIL {stem}: {why} (FIDELITY_STRICT=1)"));
                }
                continue;
            }
        };
        match common::render(&s) {
            Ok(port) => match common::compare(&stem, &reference, &port) {
                None => {
                    println!("ok {stem}: {} events", port.len());
                    passed += 1;
                }
                Some(m) => failures.push(m),
            },
            Err(e) => failures.push(format!("FAIL {stem}: {e}")),
        }
    }
    println!("{passed} identical, {pending} pending, {skipped} skipped, {} failed", failures.len());
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// Task 20: every scenario is ported (`"pending": true` would skip it, even under `FIDELITY_STRICT=1`).
#[test]
fn no_scenario_is_pending() {
    let dir = common::crate_dir().join("tests").join("scenarios");
    let files: Vec<_> = std::fs::read_dir(&dir).map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).collect()).unwrap_or_default();
    let pending: Vec<String> = files
        .iter()
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter(|p| std::fs::read(p).ok().and_then(|b| common::read(&b).ok()).is_none_or(|s| s.pending))
        .map(|p| p.display().to_string())
        .collect();
    assert!(!files.is_empty(), "no scenarios in {}", dir.display());
    assert!(pending.is_empty(), "pending (or unreadable) scenarios: {pending:#?}");
}

/// Task 20: the Musician port is complete, no `PORT-STUB` body is left under `src/`.
#[test]
fn no_port_stubs_remain() {
    fn walk(dir: &Path, hits: &mut Vec<String>, files: &mut usize) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for p in rd.filter_map(Result::ok).map(|e| e.path()) {
            if p.is_dir() {
                walk(&p, hits, files);
            } else if p.extension().is_some_and(|x| x == "rs") {
                *files += 1;
                if std::fs::read_to_string(&p).is_ok_and(|t| t.contains(concat!("PORT", "-STUB"))) {
                    hits.push(p.display().to_string());
                }
            }
        }
    }
    let (mut hits, mut files) = (Vec::new(), 0);
    walk(&common::crate_dir().join("src"), &mut hits, &mut files);
    assert!(files > 20, "src/ has only {files} .rs files");
    assert!(hits.is_empty(), "PORT-STUB left in {hits:#?}");
}

/// The `# case` line covers the overlay's files too: editing one makes its scenarios' refs stale, so strict mode fails
/// them (never a silent pass against a ref rendered from the old file).
#[test]
fn an_edited_overlay_file_makes_its_refs_stale() {
    let src = common::crate_dir().join("tests").join("fixtures").join("libraries").join("hammond-lower");
    let dir = std::env::temp_dir().join(format!("band-overlay-case-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let target = dir.join("Targets").join("Hammond B-3X.json");
    let text = std::fs::read(src.join("Targets").join("Hammond B-3X.json")).unwrap_or_default();
    assert!(!text.is_empty());
    assert!(std::fs::create_dir_all(target.parent().unwrap_or(&dir)).is_ok() && std::fs::write(&target, &text).is_ok());
    let scenario = br#"{"name": "x", "library_overlay": "hammond-lower"}"#;
    let plain = common::case_line(scenario);
    let rendered = common::case_line(&common::case_bytes(scenario, Some(&dir)));
    assert_ne!(rendered, plain, "the overlay's files are hashed");
    // The fixture as committed hashes the same as the copy.
    let s = common::read(scenario).ok();
    assert!(s.is_some_and(|s| common::scenario_case_line(&s, scenario) == rendered));
    // A ref rendered before an edit is stale after it.
    let r = dir.join("x.events");
    assert!(std::fs::write(&r, format!("{rendered}\n0 0 144 60 100\n")).is_ok());
    assert!(read_ref(&r, &rendered).is_ok());
    let mut edited = text.clone();
    edited.extend_from_slice(b" ");
    assert!(std::fs::write(&target, &edited).is_ok());
    let now = common::case_line(&common::case_bytes(scenario, Some(&dir)));
    assert!(read_ref(&r, &now).is_err_and(|e| e.contains("stale")));
    // So is one after a file is added.
    assert!(std::fs::write(&target, &text).is_ok() && std::fs::write(dir.join("Targets").join("Extra.json"), b"{}").is_ok());
    assert_ne!(common::case_line(&common::case_bytes(scenario, Some(&dir))), rendered);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The ref's event lines, after checking its `# case` line against the scenario's bytes.
fn read_ref(path: &Path, case: &str) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(path).map_err(|_| "no ref".to_string())?;
    let mut lines = text.lines();
    if lines.next() != Some(case) {
        return Err("stale ref (made from other scenario bytes)".into());
    }
    Ok(lines.filter(|l| !l.is_empty()).map(str::to_string).collect())
}

/// Every scenario, pending or not, reads and sets up (BandRef renders pending ones too: a bad name fails it).
#[test]
fn every_scenario_sets_up() {
    let dir = common::crate_dir().join("tests").join("scenarios");
    let files: Vec<_> = std::fs::read_dir(&dir).map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).collect()).unwrap_or_default();
    let mut bad = Vec::new();
    for p in files.iter().filter(|p| p.extension().is_some_and(|x| x == "json")) {
        let bytes = std::fs::read(p).unwrap_or_default();
        let r = common::read(&bytes).and_then(|s| common::library(&s).and_then(|lib| common::settings(&s, lib)).map(|_| s));
        match r {
            Ok(s) if Some(s.name.as_str()) == p.file_stem().and_then(|x| x.to_str()) => {}
            Ok(s) => bad.push(format!("{}: name {}", p.display(), s.name)),
            Err(e) => bad.push(format!("{}: {e}", p.display())),
        }
    }
    assert!(files.len() > 90, "the Task 20 coverage list: {} scenarios", files.len());
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn runner_mirrors_band_ref_checks() {
    // A typo is an error, never a silent default (band_ref readScenario).
    assert!(common::read(br#"{"name": "x", "tempo": 120}"#).is_err());
    assert!(common::read(br#"{"name": "x", "feel": {"natural": 1}}"#).is_err());
    assert!(common::read(br#"{"name": "x", "events": [{"at_ppq": 1, "stopped_blocks": 2}]}"#).is_err());
    assert!(common::read(br#"{"name": "x", "block": 512.5}"#).is_err());
    // library_overlay names a folder of tests/fixtures/libraries, never a path; a missing folder fails the setup.
    for bad in [r#""../x""#, r#""a/b""#, r#""""#, "3"] {
        assert!(common::read(format!(r#"{{"name": "x", "library_overlay": {bad}}}"#).as_bytes()).is_err(), "{bad}");
    }
    let s = common::read(br#"{"name": "x", "library_overlay": "no-such-overlay"}"#);
    assert!(s.is_ok_and(|s| common::library(&s).is_err()));
    // An overlay replaces a factory file of the same name and adds new ones (the keyswitch test targets).
    let s = common::read(br#"{"name": "x", "library_overlay": "hammond-lower", "targets": ["KS Test Modes", null, null, null, null, null]}"#);
    let lib = s.as_ref().ok().and_then(|s| common::library(s).ok());
    assert!(lib.as_ref().is_some_and(|l| l.errors.is_empty() && l.target_slot_for_name("KS Test Modes") < 0));
    let s = common::read(br#"{"name": "x", "library_overlay": "keyswitch"}"#);
    let lib = s.as_ref().ok().and_then(|s| common::library(s).ok());
    assert!(lib.as_ref().is_some_and(|l| l.errors.is_empty() && l.target_slot_for_name("KS Test Modes") == 28));
    let s = common::read(br#"{"name": "x", "bars": 1, "players": [null, null, "Otis", null, null, null]}"#);
    assert!(s.as_ref().is_ok_and(|s| s.players[2].as_deref() == Some("Otis") && s.block == 512 && !s.pending));
    // Unknown players / targets / parameters are refused.
    for bad in [
        r#"{"name": "x", "players": ["Otis", null, null, null, null, null]}"#,
        r#"{"name": "x", "targets": [null, "Nope", null, null, null, null]}"#,
        r#"{"name": "x", "params": {"m1_nope": 1}}"#,
        r#"{"name": "x", "events": [{"at_ppq": 0, "param": "nope", "value": 1}]}"#,
        r#"{"name": "x", "song": "No Such Song"}"#,
    ] {
        let s = common::read(bad.as_bytes());
        assert!(s.is_ok(), "{bad}");
        if let Ok(s) = s {
            assert!(common::settings(&s, soundcraft_band::library::load::embedded_library()).is_err(), "{bad}");
        }
    }
    // A factory song brings its Key / Scale / Take / Natural / Tension unless the scenario sets them.
    let s = common::read(br#"{"name": "x", "song": "Rock Chorus Builds", "scale": 1}"#);
    let b = s.ok().and_then(|s| common::settings(&s, soundcraft_band::library::load::embedded_library()).ok());
    assert!(b.as_ref().is_some_and(|b| b.param("key") == Some(4.0) && b.param("scale") == Some(1.0)));
}

#[test]
fn runner_steps_like_band_ref() {
    let blocks = |json: &str| common::read(json.as_bytes()).and_then(|s| common::render_counted(&s)).map(|(_, n)| n);
    // 1 bar at 120 bpm, 512 at 48 kHz: 0.021333 ppq a block -> 188 playing blocks, then the stopped one.
    assert_eq!(blocks(r#"{"name": "x", "bars": 1}"#), Ok(189));
    // A stop with no stopped_blocks event after it ends the run (47 blocks reach ppq 1); with one, the stopped
    // blocks play, then the rest of the 16.4 ppq.
    assert_eq!(blocks(r#"{"name": "x", "bars": 4, "events": [{"at_ppq": 1, "stop": true}]}"#), Ok(48));
    assert_eq!(
        blocks(r#"{"name": "x", "bars": 4.1, "events": [{"at_ppq": 1, "stop": true}, {"stopped_blocks": 4, "play": true}]}"#),
        Ok(47 + 4 + (769 - 47) + 1)
    );
    // The smoke scenario: BandRef printed "smoke-factory-44: 4319 blocks" (seek, tempo, meter, stop and restart).
    let smoke = std::fs::read(common::crate_dir().join("tests/scenarios/smoke-factory-44.json")).unwrap_or_default();
    assert_eq!(common::read(&smoke).and_then(|s| common::render_counted(&s)).map(|(_, n)| n), Ok(4319));
}
