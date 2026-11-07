//! Bounces stream the main mix or a bus to their file, exactly as rendered in memory.

use soundcraft_audio_io::{BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::io::{Bounce, bounce_stems};
use soundcraft_time::Range;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soundcraft-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn bounces_write_exactly_the_mix_and_report_its_levels() {
    let e = soundcraft_engine::demo::demo_engine();
    let s = e.session();
    let dir = scratch("bounce-exact");
    let path = dir.join("Mix.wav");
    let r = Range::new(s.sample_rate.samples(10.0) + 7, s.sample_rate.samples(13.0));
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None, bitrate_kbps: 0 };
    let verb = s.bus_by_name("Verb").unwrap().id;
    for bus in [None, Some(verb)] {
        let mut mix = match bus {
            Some(b) => soundcraft_mix::render_bus(s, b, r, 1024),
            None => soundcraft_mix::render_range(s, r, 1024),
        };
        assert!(mix.iter().flatten().any(|v| v.abs() > 1e-3), "{bus:?}");
        for normalize in [false, true] {
            let mut bounce = Bounce::new(e.session_arc(), &path, r, &opts, bus, normalize, false).unwrap();
            let mut progress = vec![bounce.progress()];
            let done = loop {
                if let Some(done) = bounce.step(|| false).unwrap() {
                    break done;
                }
                progress.push(bounce.progress());
            };
            let last = progress[progress.len() - 1];
            assert!(progress.len() > 100 && progress.is_sorted() && progress[0] < 0.01 && last > 0.99 && last <= 1.0, "{progress:?}");
            if normalize {
                soundcraft_dsp::offline::normalize(&mut mix, -0.1, false);
            }
            let bytes = std::fs::read(&path).unwrap();
            let (_, file) = soundcraft_audio_io::decode(&bytes, None).unwrap();
            assert_eq!(file.channels, mix, "{bus:?} normalize: {normalize}");
            let mut lm = soundcraft_dsp::meter::LoudnessMeter::new(s.sample_rate.as_f64() as f32, mix.len());
            lm.process(&mix, mix[0].len());
            let levels = (soundcraft_dsp::offline::peak_db(&mix), lm.integrated_lufs(), lm.true_peak_db());
            assert_eq!((done.bytes, (done.peak_db, done.lufs, done.true_peak_db)), (bytes.len() as u64, levels));
        }
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bounce_too_big_for_its_format_fails_and_keeps_the_old_file() {
    let e = soundcraft_engine::demo::demo_engine();
    let s = e.session();
    let dir = scratch("bounce-too-big");
    let path = dir.join("Mix.aif");
    std::fs::write(&path, b"old").unwrap();
    let opts = EncodeOptions { format: FileFormat::Aiff, bit_depth: BitDepth::Float32, dither: false, bwf: None, bitrate_kbps: 0 };
    let err = Bounce::new(e.session_arc(), &path, Range::new(0, s.sample_rate.samples(3.9 * 3600.0)), &opts, None, false, false).err().unwrap();
    assert!(err.to_string().contains("4 GiB"), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), b"old");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let err = bounce_stems(&e, &dir.to_string_lossy(), Range::new(0, s.sample_rate.samples(4.1 * 3600.0)), &opts).unwrap_err();
    assert!(err.to_string().contains("4 hours"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dropping_an_unfinished_bounce_cancels_it_and_keeps_the_old_file() {
    let e = soundcraft_engine::demo::demo_engine();
    let dir = scratch("bounce-cancel");
    let path = dir.join("Mix.wav");
    std::fs::write(&path, b"old").unwrap();
    let mut bounce = Bounce::new(e.session_arc(), &path, Range::new(0, 48_000), &EncodeOptions::default(), None, false, false).unwrap();
    assert_eq!(bounce.step(|| false).unwrap(), None);
    assert!(dir.join("Mix.wav.part").exists());
    drop(bounce);
    assert_eq!(std::fs::read(&path).unwrap(), b"old");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mp3_and_opus_bounces_hold_the_mix_and_import_back() {
    let mut e = soundcraft_engine::demo::demo_engine();
    let s = e.session();
    let dir = scratch("bounce-lossy");
    let r = Range::new(s.sample_rate.samples(10.0), s.sample_rate.samples(13.0));
    let mix = soundcraft_mix::render_range(s, r, 1024);
    let rate = s.sample_rate.hz();
    assert_eq!(mix.len(), 2, "the demo's main is stereo");
    for (name, bitrate, file_rate) in [("Mix.mp3", 192, soundcraft_audio_io::mp3_rate(rate)), ("Mix.opus", 160, 48_000)] {
        let path = dir.join(name);
        let p = serde_json::json!({"path": path.to_string_lossy(), "start": r.start, "end": r.end, "bitrate": bitrate});
        let done = e.execute("file.bounce_mix", &p).unwrap();
        assert_eq!(done["bytes"].as_u64(), Some(std::fs::metadata(&path).unwrap().len()), "{name}");
        let (info, file) = soundcraft_audio_io::decode(&std::fs::read(&path).unwrap(), None).unwrap();
        assert_eq!((info.sample_rate, file.num_channels()), (file_rate, 2), "{name}");
        let want = (mix[0].len() as u64 * u64::from(file_rate) / u64::from(rate)) as usize;
        assert!(file.frames().abs_diff(want) <= 1, "{name}: {} frames, want {want}", file.frames());
        if file_rate == rate {
            // In time and close to the mix: a codec's error, not a shift or a gain change.
            for (got, want) in file.channels.iter().zip(&mix) {
                let sig: f64 = want.iter().map(|v| f64::from(*v).powi(2)).sum();
                let err: f64 = want.iter().zip(got).map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2)).sum();
                let snr = 10.0 * (sig / err.max(1e-20)).log10();
                assert!(snr > 10.0, "{name}: {snr:.1} dB");
            }
        }
        let tracks = e.session().tracks.len();
        e.execute("file.import_audio", &serde_json::json!({"path": path.to_string_lossy()})).unwrap();
        assert_eq!(e.session().tracks.len(), tracks + 1, "{name} imports as a new track");
    }
    let err = e.execute("file.bounce_mix", &serde_json::json!({"path": dir.join("x.mp3").to_string_lossy(), "start": 0, "end": 480, "bitrate": 999}));
    assert!(err.is_err_and(|err| err.to_string().contains("999 kbps")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_surround_main_bounces_to_mp3_only_folded_down() {
    let mut e = soundcraft_engine::demo::demo_engine();
    e.execute("setup.main_format", &serde_json::json!({"format": "5.1"})).unwrap();
    let dir = scratch("bounce-lossy-51");
    let path = dir.join("Mix.mp3");
    let p = serde_json::json!({"path": path.to_string_lossy(), "start": 0, "end": 4800});
    let err = e.execute("file.bounce_mix", &p).unwrap_err().to_string();
    assert!(err.contains("fold the bounce down to stereo"), "{err}");
    assert!(!path.exists());
    let mut p = p;
    p["fold_down"] = "stereo".into();
    e.execute("file.bounce_mix", &p).unwrap();
    let (info, _) = soundcraft_audio_io::decode(&std::fs::read(&path).unwrap(), None).unwrap();
    assert_eq!(info.channels, 2);
    let _ = std::fs::remove_dir_all(&dir);
}
