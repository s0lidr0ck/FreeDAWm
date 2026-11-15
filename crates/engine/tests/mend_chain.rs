//! Mend in a track's chain, without its model: the sound passes through, its 2.9 s of latency is
//! made up for (the mix is what it is without Mend), and a render does not wait on a model that
//! is not there. (With the model: `crates/ml/tests/mend_live.rs`.)
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use soundcraft_time::Range;

/// Runs [`inner`] in a process of its own whose model folder is empty, whatever this machine
/// has installed.
#[test]
fn without_its_model_mend_passes_the_sound_through_in_time() {
    let empty = std::env::temp_dir().join(format!("freedawm-mend-none-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "inner", "--ignored", "--nocapture", "--test-threads=1"])
        .env("FREEDAWM_MODELS_DIR", &empty)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&empty);
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && text.contains("1 passed"), "{text}");
}

#[test]
#[ignore = "run by without_its_model_mend_passes_the_sound_through_in_time with an empty model folder"]
fn inner() {
    let mut e = soundcraft_engine::demo::demo_engine();
    // The Verb aux's plate reverb runs free of the timeline, so it differs between renders.
    let verb = e.session().tracks.iter().find(|t| t.name == "Verb").unwrap().id;
    e.session_mut().track_mut(verb).unwrap().inactive = true;
    let range = Range::new(0, e.session().sample_rate.samples(14.0));
    let plain = soundcraft_mix::render_range(e.session(), range, 1024);

    let m = e.execute("mix.module_add", &json!({"track": "Bass", "plugin": "mend"})).unwrap()["module"].as_u64().unwrap();
    e.execute("mix.module_param", &json!({"track": "Bass", "module": m, "param": "amount", "value": 100.0})).unwrap();
    let started = std::time::Instant::now();
    let with = soundcraft_mix::render_range(e.session(), range, 1024);
    assert!(started.elapsed() < std::time::Duration::from_secs(60), "the render waited for a model that is not installed");

    assert_eq!(plain.len(), with.len());
    let mut worst = 0.0f32;
    for (a, b) in plain.iter().zip(&with) {
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            worst = worst.max((x - y).abs());
        }
    }
    assert!(plain.iter().flatten().any(|v| v.abs() > 1e-3), "the mix is not silent");
    assert!(worst < 1e-4, "Mend with no model changed the mix by {worst}");
}
