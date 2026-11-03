//! Scout sound profiles: measuring from a capture's strip, a clip, a time range and a reference
//! file (same audio, same numbers), the library (save/load, list and filter, rename, duplicate,
//! delete, export/import) and per-stem capture through a fake separator.

use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::cmd::scout_profile::{capture_with, capture_with_model};
use soundcraft_engine::scout_profiles as lib;
use soundcraft_model::{ChannelFormat, Clip, ProfileKind, ScoutCapture, ScoutMetrics, ScoutProfile, ScoutStrip, Session, TrackKind};
use soundcraft_time::Range;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("scout-profiles-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 1.5 s of a stereo tone with a little noise and some width.
fn audio(rate: u32) -> Vec<Vec<f32>> {
    let n = rate as usize * 3 / 2;
    let mut seed = 7u32;
    let mut l = Vec::with_capacity(n);
    let mut r = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / rate as f32;
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.05;
        let tone = 0.25 * (2.0 * std::f32::consts::PI * 330.0 * t).sin();
        l.push(tone + noise);
        r.push(tone * 0.8 - noise);
    }
    vec![l, r]
}

/// A session with a stereo track "Gtr" holding `audio` as one clip at 1 s.
fn session(audio: &[Vec<f32>]) -> (Engine, Range) {
    let mut s = Session { name: "Song".into(), ..Session::default() };
    let rate = s.sample_rate.hz();
    let at = i64::from(rate);
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Gtr"));
    let len = audio[0].len() as i64;
    let src = soundcraft_engine::io::add_source(&mut s, "Gtr", AudioBuffer { sample_rate: rate, channels: audio.to_vec() }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "Gtr riff", src, 0, at, len));
    s.edit.selected_clips = vec![id];
    s.edit.selected_tracks = vec![t];
    (Engine::new(s), Range::new(at, at + len))
}

fn metrics(v: &Value) -> ScoutMetrics {
    serde_json::from_value(v["metrics"].clone()).unwrap()
}

fn close(a: &ScoutMetrics, b: &ScoutMetrics) {
    let tol = 0.02;
    for (x, y, what) in [
        (a.lufs_integrated, b.lufs_integrated, "lufs"),
        (a.rms_dbfs, b.rms_dbfs, "rms"),
        (a.peak_dbfs, b.peak_dbfs, "peak"),
        (a.crest_factor_db, b.crest_factor_db, "crest"),
        (a.stereo_width, b.stereo_width, "width"),
        (a.stereo_correlation, b.stereo_correlation, "corr"),
        (a.lra, b.lra, "lra"),
    ] {
        assert!((x - y).abs() < tol, "{what}: {x} vs {y}");
    }
    for (i, (x, y)) in a.spectrum31.iter().zip(&b.spectrum31).enumerate() {
        assert!((x - y).abs() < tol, "band {i}: {x} vs {y}");
    }
}

#[test]
fn strip_clip_range_and_file_give_the_same_numbers_for_the_same_audio() {
    let dir = scratch("same");
    let a = audio(48_000);
    let (mut e, range) = session(&a);
    let d = dir.to_string_lossy().to_string();
    let reference = lib::measure(&a, 48_000.0);
    assert!(reference.valid && reference.lufs_integrated > -40.0);

    let clip = e.execute("scout.profile_capture", &json!({"from": "clip", "dir": d, "role": "guitar"})).unwrap();
    let clip_m = metrics(&clip["profiles"][0]);
    close(&clip_m, &reference);
    assert_eq!(clip["profiles"][0]["name"], "Gtr riff");
    assert_eq!(clip["profiles"][0]["role"], "guitar");
    assert_eq!(clip["profiles"][0]["source"]["start_s"], 1.0);

    let r = e.execute("scout.profile_capture", &json!({"from": "range", "dir": d, "track": "Gtr", "start": range.start, "end": range.end})).unwrap();
    close(&metrics(&r["profiles"][0]), &reference);

    // The same audio as a reference file (32-bit float WAV), measured without touching the session.
    let wav = dir.join("ref.wav");
    let bytes = soundcraft_audio_io::encode(
        &AudioBuffer { sample_rate: 48_000, channels: a.clone() },
        &EncodeOptions { bit_depth: BitDepth::Float32, ..Default::default() },
    )
    .unwrap();
    std::fs::write(&wav, bytes).unwrap();
    let before = e.session().clone();
    let f = e.execute("scout.profile_capture", &json!({"from": "file", "dir": d, "path": wav.to_string_lossy()})).unwrap();
    close(&metrics(&f["profiles"][0]), &reference);
    assert_eq!(f["profiles"][0]["kind"], "mix", "a reference file is a mix by default");
    assert_eq!(f["profiles"][0]["source"]["file"], "ref.wav");
    assert_eq!(*e.session(), before, "the file is not added to the session");

    // A capture's strip: its stored POST numbers, as the meter took them.
    let mut s = e.session().clone();
    let t = s.tracks[0].id;
    s.scout.captures.push(ScoutCapture {
        id: 1,
        name: "Chorus".into(),
        created: String::new(),
        sample_rate: 48_000.0,
        tempo: 120.0,
        played: vec![range],
        loop_range: None,
        played_truncated: false,
        strips: vec![ScoutStrip {
            track: t,
            name: "Gtr".into(),
            kind: TrackKind::Audio,
            channel_type: "channel".into(),
            role: "Channel".into(),
            channels: 2,
            mixer: s.tracks[0].mixer.clone(),
            automated: Vec::new(),
            base: ScoutMetrics::default(),
            post: reference.clone(),
            note: None,
            target: None,
        }],
        bands_version: 2,
    });
    e.replace_session(s);
    let st = e.execute("scout.profile_capture", &json!({"from": "strip", "dir": d, "capture": 1, "track": "Gtr"})).unwrap();
    assert_eq!(metrics(&st["profiles"][0]), reference);
    assert_eq!(st["profiles"][0]["name"], "Chorus · Gtr");
    assert_eq!(st["profiles"][0]["source"]["position"], "post");

    // Four saved, names made unique where they clash; every file loads back equal.
    let listed = lib::list(&dir);
    assert_eq!(listed.len(), 4);
    for (path, p) in &listed {
        assert_eq!(&lib::read(path).unwrap(), p);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_library_lists_filters_renames_duplicates_deletes_exports_and_imports() {
    let dir = scratch("lib");
    let d = dir.to_string_lossy().to_string();
    let mut e = Engine::new(Session::default());
    for (name, role, kind) in
        [("Warm Bass", "bass", ProfileKind::Track), ("Airy Vox", "vocal", ProfileKind::Track), ("Loud Master", "mix", ProfileKind::Mix)]
    {
        let mut p = lib::make(name, kind, role, vec!["ref".into()], ScoutMetrics::default(), String::new());
        lib::save_new(&dir, &mut p).unwrap();
    }
    let names = |e: &mut Engine, p: Value| -> Vec<String> {
        e.execute("scout.profile_list", &p).unwrap()["profiles"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!(names(&mut e, json!({"dir": d})), ["Airy Vox", "Loud Master", "Warm Bass"]);
    assert_eq!(names(&mut e, json!({"dir": d, "role": "vocal"})), ["Airy Vox"]);
    assert_eq!(names(&mut e, json!({"dir": d, "query": "warm"})), ["Warm Bass"]);
    assert_eq!(names(&mut e, json!({"dir": d, "kind": "mix"})), ["Loud Master"]);

    e.execute("scout.profile_rename", &json!({"dir": d, "name": "warm bass", "to": "Round Bass"})).unwrap();
    assert!(e.execute("scout.profile_rename", &json!({"dir": d, "name": "Round Bass", "to": "Airy Vox"})).is_err());
    e.execute("scout.profile_duplicate", &json!({"dir": d, "name": "Round Bass"})).unwrap();
    e.execute("scout.profile_edit", &json!({"dir": d, "name": "Round Bass copy", "role": "keys", "tags": ["a", "b"]})).unwrap();
    assert_eq!(names(&mut e, json!({"dir": d, "role": "keys"})), ["Round Bass copy"]);
    e.execute("scout.profile_delete", &json!({"dir": d, "name": "Round Bass copy"})).unwrap();
    assert!(e.execute("scout.profile_delete", &json!({"dir": d, "name": "nope"})).is_err());

    let outside = scratch("lib-out");
    let out = outside.join("exported.json");
    e.execute("scout.profile_export", &json!({"dir": d, "name": "Airy Vox", "path": out.to_string_lossy()})).unwrap();
    let r = e.execute("scout.profile_import", &json!({"dir": d, "path": out.to_string_lossy()})).unwrap();
    assert_eq!(r["name"], "Airy Vox 2", "an imported name is made unique");
    assert_eq!(names(&mut e, json!({"dir": d})), ["Airy Vox", "Airy Vox 2", "Loud Master", "Round Bass"]);
    // A file that is not a profile is refused; nothing changes; the session never changes.
    std::fs::write(outside.join("junk.txt"), "{").unwrap();
    assert!(e.execute("scout.profile_import", &json!({"dir": d, "path": outside.join("junk.txt").to_string_lossy()})).is_err());
    std::fs::write(dir.join("broken.json"), "not json").unwrap();
    assert_eq!(names(&mut e, json!({"dir": d})).len(), 4, "a broken file in the folder is skipped");
    assert_eq!(*e.session(), Session::default());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn capture_per_stem_saves_one_profile_per_stem_plus_the_mix() {
    let dir = scratch("stems");
    let d = dir.to_string_lossy().to_string();
    let a = audio(48_000);
    let (mut e, _) = session(&a);
    // A fake separator: "vocals" is the left channel, "bass" the right, at half level.
    let fake = |audio: Vec<Vec<f32>>, _rate: u32| -> Result<Vec<(String, Vec<Vec<f32>>)>, String> {
        let l: Vec<f32> = audio[0].iter().map(|x| x * 0.5).collect();
        let r: Vec<f32> = audio[1].iter().map(|x| x * 0.5).collect();
        Ok(vec![("vocals".into(), vec![l.clone(), l]), ("bass".into(), vec![r.clone(), r])])
    };
    let v = capture_with(&mut e, &json!({"from": "stems", "dir": d, "track": "Gtr", "name": "Hit Song"}), &fake).unwrap();
    let names: Vec<&str> = v["profiles"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Hit Song · mix", "Hit Song · vocals", "Hit Song · bass"]);
    let ps = v["profiles"].as_array().unwrap();
    assert_eq!(ps[0]["kind"], "mix");
    assert_eq!(ps[1]["role"], "vocal");
    assert_eq!(ps[2]["role"], "bass");
    assert_eq!(ps[1]["source"]["stem"], "vocals");
    assert_eq!(ps[1]["source"]["model"], "demucs");
    close(&metrics(&ps[0]), &lib::measure(&a, 48_000.0));
    // Half level: about 6 dB down.
    let mix_rms = metrics(&ps[0]).rms_dbfs;
    assert!(metrics(&ps[1]).rms_dbfs < mix_rms - 4.0);
    assert_eq!(lib::list(&dir).len(), 3);
    // A failing separator fails the command and saves nothing.
    let broken = |_: Vec<Vec<f32>>, _: u32| -> Result<Vec<(String, Vec<Vec<f32>>)>, String> { Err("no model".into()) };
    let dir2 = scratch("stems2");
    assert!(capture_with(&mut e, &json!({"from": "stems", "dir": dir2.to_string_lossy(), "track": "Gtr"}), &broken).is_err());
    assert!(lib::list(&dir2).is_empty());
    // The separator's model is recorded (Capture per Stem uses the best installed).
    let dir3 = scratch("stems3");
    let v = capture_with_model(&mut e, &json!({"from": "stems", "dir": dir3.to_string_lossy(), "track": "Gtr"}), &fake, "best").unwrap();
    assert_eq!(v["profiles"][1]["source"]["model"], "best");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
    let _ = std::fs::remove_dir_all(&dir3);
}

#[test]
fn hostile_requests_are_refused_not_crashes() {
    let dir = scratch("hostile");
    let d = dir.to_string_lossy().to_string();
    let mut e = Engine::new(Session::default());
    for p in [
        json!({"from": "strip", "dir": d}),
        json!({"from": "strip", "dir": d, "capture": 99}),
        json!({"from": "clip", "dir": d}),
        json!({"from": "range", "dir": d}),
        json!({"from": "file", "dir": d, "path": dir.join("missing.wav").to_string_lossy()}),
        json!({"from": "nonsense", "dir": d}),
        json!({"from": "clip", "dir": d, "kind": "stem"}),
    ] {
        assert!(e.execute("scout.profile_capture", &p).is_err(), "{p}");
    }
    let empty: Vec<ScoutProfile> = lib::list(&dir).into_iter().map(|x| x.1).collect();
    assert!(empty.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
