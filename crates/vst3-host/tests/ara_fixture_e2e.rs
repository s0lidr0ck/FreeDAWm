//! ARA through VST3, end to end: build the ARA test plug-in in `tests/ara_fixture` (a cdylib
//! outside the workspace whose ARA side is soundcraft-ara-host's test plug-in), install it as a
//! `.vst3` bundle, scan it (its ARA main factories and entry points), then make an ARA document
//! with the factory its `ARA::IMainFactory` gives and check, through ARA's own content reading,
//! that the plug-in read exactly the audio the host served.

use soundcraft_ara_host::test_plugin::checksum;
use soundcraft_ara_host::{Document, Graph, MemorySource, RegionDesc, SequenceDesc, SourceDesc};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

/// Builds the ARA fixture once, installs it as `AraFixture.vst3` in a temp folder and returns it.
fn fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ara_fixture/Cargo.toml");
        let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("vst3-ara-fixture-target");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let st = Command::new(cargo)
            .args(["build", "--quiet", "--manifest-path"])
            .arg(&manifest)
            .arg("--target-dir")
            .arg(&target)
            .env_remove("RUSTFLAGS")
            .status()
            .expect("run cargo");
        assert!(st.success(), "building the ARA VST3 fixture failed");
        let (lib, folder, exe) = if cfg!(target_os = "macos") {
            ("libvst3_ara_fixture.dylib", "MacOS", "AraFixture")
        } else if cfg!(windows) {
            ("vst3_ara_fixture.dll", "x86_64-win", "AraFixture.vst3")
        } else {
            ("libvst3_ara_fixture.so", if cfg!(target_arch = "aarch64") { "aarch64-linux" } else { "x86_64-linux" }, "AraFixture.so")
        };
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("vst3-ara-fixture-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bin = dir.join("AraFixture.vst3/Contents").join(folder);
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::copy(target.join("debug").join(lib), bin.join(exe)).unwrap();
        assert_eq!(soundcraft_vst3_host::add_search_dir(&dir), 5);
        dir
    })
}

fn find(name: &str) -> soundcraft_vst3_host::Vst3Descriptor {
    soundcraft_vst3_host::scan_paths(&[fixture_dir().to_path_buf()]).into_iter().find(|d| d.name == name).unwrap()
}

#[test]
fn the_scan_finds_ara_and_its_roles() {
    let d = find("Fixture ARA");
    assert!(d.ara_checked);
    let a = d.ara.expect("offers ARA");
    assert!(a.usable, "{}", a.problem);
    assert_eq!(a.name, "FreeDAWm Test ARA");
    assert_eq!(a.manufacturer, "FreeDAWm tests");
    assert_eq!(a.factory_id, "org.freedawm.test.ara");
    assert_eq!((a.lowest_generation, a.highest_generation), (4, 6));
    assert_eq!(a.roles, vec!["playback renderer", "editor renderer", "editor view"]);
    assert!(a.ara1_binding);
    assert_eq!(a.content, vec!["notes"]);
    assert_eq!(a.main_class_id.len(), 32);
    let plain = find("Fixture Plain");
    assert!(plain.ara.is_none() && plain.ara_checked);
    // Only the usable one is offered.
    let offered = soundcraft_vst3_host::ara_plugins();
    assert!(offered.iter().any(|p| p.name == "Fixture ARA"));
    assert!(!offered.iter().any(|p| p.name.starts_with("Fixture ARA ") || p.name == "Fixture Plain"), "{offered:?}");
}

#[test]
fn plug_ins_that_lie_about_ara_are_listed_with_the_problem() {
    let null = find("Fixture ARA Null").ara.unwrap();
    assert!(!null.usable && null.problem.contains("no ARA factory"), "{null:?}");
    let tiny = find("Fixture ARA Tiny").ara.unwrap();
    assert!(!tiny.usable && tiny.problem.contains("smaller"), "{tiny:?}");
    let one = find("Fixture ARA One").ara.unwrap();
    assert!(!one.usable, "{one:?}");
    assert!(one.roles.is_empty() && one.ara1_binding);
    assert_eq!((one.lowest_generation, one.highest_generation), (1, 2));
    assert!(one.problem.contains("ARA 2"), "{one:?}");
    for name in ["Fixture ARA Null", "Fixture ARA Tiny", "Fixture ARA One", "Fixture Plain"] {
        let e = soundcraft_vst3_host::ara_factory(&find(name).id).unwrap_err();
        assert!(e.to_string().contains("ARA"), "{name}: {e}");
    }
}

#[test]
fn the_vst3_factory_hosts_an_ara_document_that_reads_the_audio() {
    let id = find("Fixture ARA").id;
    let factory = soundcraft_vst3_host::ara_factory(&id).unwrap();
    let mut doc = Document::new(factory.ptr(), "vst3 ara e2e").unwrap();
    assert_eq!(doc.info().name, "FreeDAWm Test ARA");
    let channels: Vec<Vec<f32>> = (0..2).map(|c| (0..9_000).map(|i| ((i * (c + 3)) % 101) as f32 / 101.0 - 0.5).collect()).collect();
    let audio = Arc::new(MemorySource { channels: channels.clone(), rate: 44_100.0 });
    let g = Graph {
        name: "vst3 ara e2e".into(),
        sequences: vec![SequenceDesc { key: 1, name: "Vox".into(), order: 0, color: None }],
        sources: vec![SourceDesc { key: 7, name: "take.wav".into(), persistent_id: "take".into(), audio }],
        regions: vec![RegionDesc {
            key: 3,
            source: 7,
            sequence: 1,
            name: "take".into(),
            persistent_id: "clip-3".into(),
            start_in_source: 0.0,
            duration: 0.2,
            start_in_song: 1.0,
            color: None,
        }],
        ..Graph::default()
    };
    assert!(doc.sync(&g).changed());
    let notes = doc.source_notes(7).expect("the plug-in read the source");
    let want = checksum(&channels.iter().map(|c| c.iter().map(|x| f64::from(*x)).collect()).collect::<Vec<_>>());
    assert_eq!(notes.len(), 2);
    for n in &notes {
        assert_eq!(n.start, (want >> 11) as f64, "32- and 64-bit reads both match the audio");
        assert_eq!((n.pitch, n.duration), (2, 9_000.0));
    }
    assert_eq!(doc.read_stats().failed, 0);
    drop(doc);
    drop(factory);
}
