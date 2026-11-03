//! A hosted plugin's parameters as automation lanes: the lane picker lists them with the plugin's
//! own names, and the lane reads them with its own range. Uses the CLAP test plugin built from
//! `crates/clap-host/tests/fixture` (Fixture Gain: param "7" Gain, 0..2).

use serde_json::json;
use soundcraft_model::AutoParam;
use soundcraft_ui_egui::{auto_lanes, edit_window};
use std::path::Path;
use std::process::Command;

fn install_fixture() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let manifest = Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").into()))
            .join("../clap-host/tests/fixture/Cargo.toml");
        let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("clap-fixture-target");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let st = Command::new(cargo)
            .args(["build", "--quiet", "--manifest-path"])
            .arg(&manifest)
            .arg("--target-dir")
            .arg(&target)
            .env_remove("RUSTFLAGS")
            .status()
            .unwrap();
        assert!(st.success());
        let lib = if cfg!(target_os = "macos") {
            "libclap_fixture.dylib"
        } else if cfg!(windows) {
            "clap_fixture.dll"
        } else {
            "libclap_fixture.so"
        };
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("ui-clap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(target.join("debug").join(lib), dir.join("Fixture.clap")).unwrap();
        assert!(soundcraft_clap_host::add_search_dir(&dir) > 0);
    });
}

#[test]
fn a_hosted_plugins_parameters_are_lanes_with_its_names() {
    install_fixture();
    let mut e = soundcraft_engine::demo::demo_engine();
    e.execute("mix.insert", &json!({"track": "Kick", "plugin": "clap:org.soundcraft.test.gain"})).unwrap();
    let kick = e.session().track_by_name("Kick").unwrap().clone();
    let module = kick.mixer.inserts.iter().find(|i| i.plugin == "clap:org.soundcraft.test.gain").unwrap().id;
    let gain = AutoParam::Plugin { module, param: "7".into() };
    let all = auto_lanes::choices(e.session(), &kick);
    let c = all.iter().find(|c| c.param == gain).expect("the hosted plugin's Gain is offered");
    assert_eq!(c.label, "Gain");
    assert_eq!(c.group.as_deref(), Some("Fixture Gain"));
    // The lane takes points in the plugin's own range and shows its own names.
    e.execute("track.auto_lane", &json!({"track": kick.id.0, "param": gain.id(), "show": true})).unwrap();
    e.execute("automation.set_point", &json!({"track": kick.id.0, "param": gain.id(), "at": 0, "value": 1.5})).unwrap();
    let kick = e.session().track(kick.id).unwrap();
    assert_eq!(kick.lane(&gain).unwrap().points[0].value, 1.5);
    assert_eq!(auto_lanes::shown_lanes(kick), std::slice::from_ref(&gain));
    assert_eq!(edit_window::view_label(kick, &gain.id()), "Fixture Gain Gain");
    assert!(auto_lanes::menu_label(kick, c).ends_with('\u{2022}'), "marked as automated");
}
