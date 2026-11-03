//! The stock effects are hidden from the plugin lists (`soundcraft_dsp::HIDDEN_STOCK`), not
//! removed: `mix.module_add` still takes one by id, a session holding one saves and opens again,
//! and the module still runs in the mix.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use soundcraft_audio_io::{BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::io::{self, Bounce};
use soundcraft_engine::{Engine, demo};
use soundcraft_time::Range;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soundcraft-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn rms_db(e: &Engine, dir: &Path, name: &str, range: Range) -> f64 {
    let path = dir.join(name);
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None };
    Bounce::new(e.session_arc(), &path, range, &opts, None, false, false).unwrap().run().unwrap();
    let (_, file) = soundcraft_audio_io::decode(&std::fs::read(&path).unwrap(), None).unwrap();
    let (mut sum, mut n) = (0.0f64, 0usize);
    for c in &file.channels {
        sum += c.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>();
        n += c.len();
    }
    10.0 * (sum / n.max(1) as f64).max(1e-20).log10()
}

#[test]
fn a_hidden_stock_module_is_added_by_id_saved_reopened_and_runs() {
    let info = soundcraft_dsp::plugin_info("eq_7band").unwrap();
    assert!(!info.listed(), "the stock EQ is hidden from the lists");
    // One audio track alone, so the module's effect is what is measured.
    let mut s = demo::demo_engine().session().clone();
    let t = s.tracks.iter().position(|t| !t.clips().is_empty() && t.instrument.is_none()).unwrap();
    for (i, track) in s.tracks.iter_mut().enumerate() {
        track.mixer.solo = i == t;
    }
    let from = s.tracks[t].clips().iter().map(|c| c.start).min().unwrap();
    let range = Range::new(from, from + s.sample_rate.samples(2.0));
    let track = s.tracks[t].id.0;
    let dir = scratch("hidden-stock");
    let mut e = Engine::new(s);
    let dry = rms_db(&e, &dir, "dry.wav", range);
    // Added by id, as the control protocol does.
    let added = e.execute("mix.module_add", &json!({"track": track, "plugin": "eq_7band", "params": {"output_gain": -12.0}})).unwrap();
    assert!(added["module"].as_u64().is_some());
    // Saved and opened again: the module is still there and still turns the track down.
    let file = dir.join("Old").join("Old.fdawm");
    e.execute("session.save_as", &json!({"path": file.to_string_lossy()})).unwrap();
    let mut r = Engine::default();
    io::open_session(&mut r, &file.to_string_lossy()).unwrap();
    let rt = r.session().tracks.iter().find(|x| x.id.0 == track).unwrap();
    assert!(rt.mixer.inserts.iter().any(|i| i.plugin == "eq_7band"), "the hidden module loads");
    let wet = rms_db(&r, &dir, "wet.wav", range);
    assert!(dry > -60.0, "the track plays ({dry:.1} dBFS)");
    assert!((dry - wet - 12.0).abs() < 1.5, "the module runs: dry {dry:.1} dBFS, with it {wet:.1} dBFS");
    let _ = std::fs::remove_dir_all(&dir);
}
