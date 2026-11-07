//! The ML commands end to end on a scripted session: results land under the source at its
//! position as one undo step, the session saves and reloads with them, and the bounced mix of the
//! result tracks is the sum of their audio.
//!
//! Separation, karaoke, drum split and de-reverb need the models: point `SOUNDCRAFT_MODELS_DIR` at
//! the folder holding them. Without them those parts print `SKIPPED` (de-noise is built in and
//! always runs).

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::ml::{self, Cancel, Ctx, MlError};
use soundcraft_model::{ChannelFormat, Clip, Session, TrackKind};
use soundcraft_time::Range;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soundcraft-ml-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 5 s of music: the export lane's HTDemucs golden input when installed, else a synthetic mix.
fn music(rate: u32) -> Vec<Vec<f32>> {
    let golden = ml::models_dir().join("htdemucs").join("golden").join("input_5s.f32");
    if let Ok(bytes) = std::fs::read(&golden) {
        let v: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let half = v.len() / 2;
        return soundcraft_dsp::offline::resample(&[v[..half].to_vec(), v[half..].to_vec()], 44_100, rate);
    }
    let n = rate as usize * 5;
    let mut seed = 3u32;
    let ch: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.05;
            0.2 * (2.0 * std::f32::consts::PI * 220.0 * t).sin() + if (t * 4.0).fract() < 0.05 { noise * 8.0 } else { noise }
        })
        .collect();
    vec![ch.clone(), ch]
}

/// A session with a stereo track "Song" holding one 5 s clip at 2 s.
fn session() -> (Engine, i64) {
    let mut s = Session::default();
    let rate = s.sample_rate.hz();
    let at = i64::from(rate) * 2;
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Song"));
    let audio = music(rate);
    let len = audio[0].len() as i64;
    let src = soundcraft_engine::io::add_source(&mut s, "Song", AudioBuffer { sample_rate: rate, channels: audio }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "Song", src, 0, at, len));
    (Engine::new(s), at)
}

fn names(e: &Engine) -> Vec<String> {
    e.session().tracks.iter().map(|t| t.name.clone()).collect()
}

fn installed(id: &str) -> bool {
    let ok = soundcraft_ml::models::load(&ml::models_dir(), id).is_ok();
    if !ok {
        println!("SKIPPED the parts that need `{id}` (not in {})", ml::models_dir().display());
    }
    ok
}

fn dir_param() -> String {
    ml::models_dir().to_string_lossy().into_owned()
}

#[test]
fn clean_vocal_replaces_the_clip_in_one_undo_step_and_can_be_cancelled() {
    let (mut e, at) = session();
    let clip = e.session().tracks[0].clips()[0].clone();
    e.session_mut().find_clip_mut(clip.id).unwrap().gain_db = -3.0;
    let p = json!({"clip": clip.id.0, "denoise": true, "dereverb": false, "amount_db": 18.0});
    // The UI path: prepare, run on a worker (here inline), apply.
    let mut job = ml::prepare(&e, "ml.clean", &p).unwrap();
    assert_eq!(job.parts.len(), 1);
    let first_audio = job.parts[0].audio.clone();
    let cancel = Cancel::new();
    cancel.cancel();
    assert!(matches!(job.run(&Ctx::new(cancel, |_, _| {})), Err(MlError::Cancelled)));
    let progress = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = std::sync::Arc::clone(&progress);
    let result = job.run(&Ctx::new(Cancel::new(), move |f, _| seen.lock().unwrap().push(f))).unwrap();
    let fr = progress.lock().unwrap().clone();
    assert!(fr.len() > 3 && fr.windows(2).all(|w| w[1] >= w[0] - 1e-6), "{fr:?}");
    ml::apply(&mut e, &job, result).unwrap();
    let c = e.session().tracks[0].clips()[0].clone();
    assert_eq!((c.start, c.length, c.gain_db), (at, clip.length, -3.0), "same place, same length, gain kept on the clip");
    // Non-destructive: the original stays, the cleaned audio sits beside it, all cleaned.
    assert_eq!(c.source(), clip.source());
    let k = c.clean_alt().copied().expect("cleaned alternate");
    assert_eq!((k.original, k.origin, k.mix, k.bypass), (clip.source().unwrap(), clip.source_offset(), 1.0, false));
    assert!(e.session().sources.iter().any(|s| s.id == k.source && s.name.ends_with("-clean")));
    assert_eq!(e.undo_label(), Some("Clean Vocal"));
    // Re-running cleans the original (not the cleaned audio) and replaces the cleaned source.
    let again = ml::prepare(&e, "ml.clean", &p).unwrap();
    assert_eq!(again.parts[0].audio, first_audio, "the re-run renders the original");
    assert!(e.undo());
    let undone = e.session().tracks[0].clips()[0].clone();
    assert_eq!((undone.source(), undone.clean), (clip.source(), None), "undo restores the original clip");
    // The command runs the same job in one go.
    let r = e.execute("ml.clean", &p).unwrap();
    assert_eq!(r["made"].as_array().unwrap().len(), 1);
    assert_eq!(e.undo_label(), Some("Clean Vocal"));
}

#[test]
fn clean_vocal_modes_and_the_old_params() {
    use soundcraft_engine::ml::{CleanMode, CleanOptions, JobKind};
    let k = |p: serde_json::Value| ml::kind_from("ml.clean", &p);
    let opts = |p: serde_json::Value| match k(p).unwrap() {
        JobKind::Clean(o) => o,
        other => panic!("{other:?}"),
    };
    // Default and the old "both" (or de-reverb alone): Vocal, all wet.
    assert_eq!(opts(json!({})), CleanOptions::vocal(1.0));
    assert_eq!(opts(json!({"denoise": true, "dereverb": true, "amount_db": 18.0})), CleanOptions::vocal(1.0));
    assert_eq!(opts(json!({"denoise": false, "dereverb": true})).mode, CleanMode::Vocal);
    // The old de-noise alone: Speech without de-reverb, its amount kept.
    assert_eq!(opts(json!({"denoise": true, "dereverb": false, "amount_db": 18.0})), CleanOptions::speech(18.0, false));
    assert!(k(json!({"denoise": false, "dereverb": false})).is_err());
    // The new params.
    assert_eq!(opts(json!({"mode": "vocal", "mix": 40})), CleanOptions::vocal(0.4));
    assert_eq!(opts(json!({"mode": "speech", "amount_db": 12})), CleanOptions::speech(12.0, true));
    assert_eq!(opts(json!({"mode": "speech", "dereverb": false})), CleanOptions::speech(24.0, false));
    assert!(k(json!({"mode": "opera"})).is_err());
    assert!(k(json!({"mode": "vocal", "mix": 150})).is_err());
    assert!(k(json!({"mode": "vocal", "mix": "lots"})).is_err());
}

#[test]
fn clean_vocal_vocal_mode_replaces_the_clip_in_one_undo_step() {
    if !installed("srs_small") {
        return;
    }
    let (mut e, at) = session();
    let clip = e.session().tracks[0].clips()[0].clone();
    let p = json!({"clip": clip.id.0, "mode": "vocal", "mix": 80, "device": "cpu", "models_dir": dir_param()});
    let r = e.execute("ml.clean", &p).unwrap();
    assert_eq!(r["made"].as_array().unwrap().len(), 1);
    let c = e.session().tracks[0].clips()[0].clone();
    assert_eq!((c.start, c.length), (at, clip.length));
    assert_eq!(c.source(), clip.source());
    assert!(c.clean_alt().is_some_and(|k| (k.mix - 0.8).abs() < 1e-6), "the mix param sets the clip's amount");
    assert_eq!(e.undo_label(), Some("Clean Vocal"));
    assert!(e.undo());
    assert!(e.session().tracks[0].clips()[0].clean.is_none());
}

#[test]
fn ml_commands_refuse_what_they_cannot_do() {
    let (mut e, _) = session();
    let none = scratch("no-models");
    let dir = none.to_string_lossy().into_owned();
    let err = e.execute("ml.separate", &json!({"track": "Song", "models_dir": dir})).unwrap_err().to_string();
    assert!(err.contains("not installed") && err.contains("xtask ml-models"), "{err}");
    assert!(e.execute("ml.separate", &json!({"track": "Nope"})).is_err());
    assert!(e.execute("ml.separate", &json!({"track": "Song", "device": "tpu"})).is_err());
    assert!(e.execute("ml.clean", &json!({"track": "Song", "denoise": false, "dereverb": false, "models_dir": dir})).is_err());
    assert_eq!(names(&e), ["Song"], "nothing half-applied");
    let models = e.execute("ml.models", &json!({"models_dir": dir})).unwrap();
    let rows = models["models"].as_array().unwrap();
    assert!(rows.iter().any(|m| m["id"] == "htdemucs" && m["status"] == "not installed"));
    assert!(rows.iter().any(|m| m["id"] == "deepfilternet3" && m["status"] == "built in"));
    let _ = std::fs::remove_dir_all(&none);
}

#[test]
fn separate_stems_model_choice() {
    let (mut e, at) = session();
    let none = scratch("no-sw6");
    let empty = none.to_string_lossy().into_owned();
    let err = e.execute("ml.separate", &json!({"track": "Song", "model": "sw6", "models_dir": empty})).unwrap_err().to_string();
    assert!(err.contains("bs_roformer_sw"), "{err}");
    let err = e.execute("ml.separate", &json!({"track": "Song", "model": "spleeter"})).unwrap_err().to_string();
    assert!(err.contains("demucs, sw6"), "{err}");
    assert_eq!(ml::kind_from("ml.separate", &json!({})).unwrap(), ml::JobKind::Separate { model: ml::SepModel::Demucs });
    assert_eq!(ml::kind_from("ml.separate", &json!({"model": "sw6"})).unwrap(), ml::JobKind::Separate { model: ml::SepModel::Sw6 });
    let _ = std::fs::remove_dir_all(&none);
    if !installed("bs_roformer_sw") {
        println!("SKIPPED sw6 run: bs_roformer_sw not installed");
        return;
    }
    // sw6: six tracks under the source, at its position, one undo step; `stems` keeps a subset.
    let len = e.session().tracks[0].clips()[0].length;
    let dir = dir_param();
    e.execute("ml.separate", &json!({"track": "Song", "model": "sw6", "models_dir": dir})).unwrap();
    assert_eq!(names(&e), ["Song", "Song Bass", "Song Drums", "Song Other", "Song Vocals", "Song Guitar", "Song Piano"]);
    for t in &e.session().tracks[1..] {
        assert_eq!((t.clips()[0].start, t.clips()[0].length), (at, len), "{}", t.name);
    }
    assert!(e.undo());
    assert_eq!(names(&e), ["Song"]);
    e.execute("ml.separate", &json!({"track": "Song", "model": "sw6", "stems": ["piano", "guitar"], "models_dir": dir})).unwrap();
    assert_eq!(names(&e), ["Song", "Song Guitar", "Song Piano"]);
}

#[test]
fn separate_stems_best_mode() {
    let (mut e, at) = session();
    assert_eq!(ml::kind_from("ml.separate", &json!({"model": "best"})).unwrap(), ml::JobKind::Separate { model: ml::SepModel::Best });
    // Best needs both RoFormer models: SW alone is not enough.
    let none = scratch("no-best");
    let empty = none.to_string_lossy().into_owned();
    let err = e.execute("ml.separate", &json!({"track": "Song", "model": "best", "models_dir": empty})).unwrap_err().to_string();
    assert!(err.contains("vocals_melroformer"), "{err}");
    let _ = std::fs::remove_dir_all(&none);
    if !installed("bs_roformer_sw") || !installed("vocals_melroformer") {
        println!("SKIPPED best run: bs_roformer_sw or vocals_melroformer not installed");
        return;
    }
    // Six tracks in best order (vocals first), at the source's position; `stems` keeps a subset.
    let len = e.session().tracks[0].clips()[0].length;
    let dir = dir_param();
    e.execute("ml.separate", &json!({"track": "Song", "model": "best", "models_dir": dir})).unwrap();
    assert_eq!(names(&e), ["Song", "Song Vocals", "Song Drums", "Song Bass", "Song Guitar", "Song Piano", "Song Other"]);
    for t in &e.session().tracks[1..] {
        assert_eq!((t.clips()[0].start, t.clips()[0].length), (at, len), "{}", t.name);
    }
    assert!(e.undo());
    e.execute("ml.separate", &json!({"track": "Song", "model": "best", "stems": ["vocals", "bass"], "models_dir": dir})).unwrap();
    assert_eq!(names(&e), ["Song", "Song Vocals", "Song Bass"]);
}

#[test]
fn every_command_on_a_scripted_session_saves_reloads_and_bounces() {
    if !installed("htdemucs") {
        return;
    }
    let (mut e, at) = session();
    let len = e.session().tracks[0].clips()[0].length;
    let dir = dir_param();

    // Separate Stems on the track: four tracks right under it, at the clip's position.
    let r = e.execute("ml.separate", &json!({"track": "Song", "device": "cpu", "models_dir": dir})).unwrap();
    println!("ml.separate: {r}");
    assert_eq!(names(&e), ["Song", "Song Drums", "Song Bass", "Song Other", "Song Vocals"]);
    for t in &e.session().tracks[1..] {
        let c = &t.clips()[0];
        assert_eq!((c.start, c.length), (at, len), "{}", t.name);
        assert_eq!(t.channels(), 2);
    }
    assert_eq!(e.undo_label(), Some("Separate Stems"));
    assert!(e.undo());
    assert_eq!(names(&e), ["Song"], "one undo step");
    assert!(e.redo());
    assert_eq!(names(&e).len(), 5);
    // A second run makes uniquely named tracks (under the source, above the first run's), and the
    // stems take the source's pan.
    let pan0 = e.session().tracks[0].mixer.pan.clone();
    e.session_mut().tracks[0].mixer.pan = vec![-0.5, 0.25];
    e.execute("ml.separate", &json!({"track": "Song", "stems": ["drums"], "device": "cpu", "models_dir": dir})).unwrap();
    assert_eq!(names(&e)[..3], ["Song", "Song Drums 1", "Song Drums"]);
    assert_eq!(e.session().tracks[1].mixer.pan, vec![-0.5, 0.25]);
    assert!(e.undo());
    e.session_mut().tracks[0].mixer.pan = pan0;

    // The bounced mix of the four stems is their sum (the source muted).
    {
        let s = e.session_mut();
        s.tracks[0].mixer.mute = true;
    }
    let out = scratch("bounce");
    let path = out.join("stems.wav");
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None, bitrate_kbps: 0 };
    let range = Range::new(at, at + len);
    soundcraft_engine::io::Bounce::new(e.session_arc(), &path, range, &opts, None, false, false).unwrap().run().unwrap();
    let (_, mix) = soundcraft_audio_io::decode(&std::fs::read(&path).unwrap(), None).unwrap();
    let s = e.session();
    let mut sum = vec![vec![0.0f32; len as usize]; 2];
    for t in &s.tracks[1..] {
        let buf = &s.pool.get(t.clips()[0].source().unwrap()).unwrap().buffer();
        for (o, c) in sum.iter_mut().zip(&buf.channels) {
            for (a, b) in o.iter_mut().zip(c) {
                *a += b;
            }
        }
    }
    let err = mix.channels.iter().zip(&sum).flat_map(|(m, s)| m.iter().zip(s).map(|(a, b)| (a - b).abs())).fold(0.0f32, f32::max);
    assert!(err < 1e-4, "bounce vs sum of stems: {err}");
    // And the stems add back up to the source (HTDemucs is close to transparent).
    let src = &s.pool.get(s.tracks[0].clips()[0].source().unwrap()).unwrap().buffer().channels;
    let (sig, res) = src.iter().zip(&sum).fold((0.0f64, 0.0f64), |(p, q), (a, b)| {
        (p + a.iter().map(|v| f64::from(v * v)).sum::<f64>(), q + a.iter().zip(b).map(|(x, y)| f64::from((x - y) * (x - y))).sum::<f64>())
    });
    let snr = 10.0 * (sig / res.max(1e-20)).log10();
    println!("sum of stems vs source: {snr:.1} dB");
    assert!(snr > 10.0, "{snr}");

    // Make Karaoke on the clip: lead, instrumental, backing right under the source, source muted.
    let song_clip = s.tracks[0].clips()[0].id;
    if installed("karaoke_melroformer") {
        let r = e.execute("ml.karaoke", &json!({"clip": song_clip.0, "device": "cpu", "models_dir": dir})).unwrap();
        println!("ml.karaoke: {r}");
        assert_eq!(&names(&e)[..4], ["Song", "Song Lead Vocal", "Song Instrumental", "Song Backing Vocals"]);
        assert!(e.session().tracks[0].mixer.mute);
    }

    // Split Drums on the drum stem.
    if installed("drumsep_mdx23c") {
        let r = e.execute("ml.drums", &json!({"track": "Song Drums", "device": "cpu", "models_dir": dir})).unwrap();
        println!("ml.drums: {r}");
        let n = names(&e);
        let i = n.iter().position(|x| x == "Song Drums").unwrap();
        assert_eq!(
            &n[i + 1..i + 7],
            ["Song Drums Kick", "Song Drums Snare", "Song Drums Toms", "Song Drums Hi-Hat", "Song Drums Ride", "Song Drums Crash"]
        );
        assert!(e.session().tracks[i + 1..i + 7].iter().all(|t| t.clips()[0].start == at));
    }

    // Clean Vocal (de-noise, then de-reverb when installed) on the vocal stem.
    let vocals = e.session().track_by_name("Song Vocals").unwrap().clips()[0].clone();
    let dereverb = installed("srs_small");
    let r = e.execute("ml.clean", &json!({"clip": vocals.id.0, "dereverb": dereverb, "device": "cpu", "models_dir": dir})).unwrap();
    println!("ml.clean: {r}");
    let cleaned = e.session().track_by_name("Song Vocals").unwrap().clips()[0].clone();
    assert_eq!((cleaned.start, cleaned.length), (vocals.start, vocals.length));
    assert_eq!(cleaned.source(), vocals.source());
    assert!(cleaned.clean_alt().is_some_and(|k| Some(k.source) != vocals.source()));

    // Save, reload: every result track and its audio file is there.
    let before = names(&e);
    let file = out.join("ml.scraft");
    soundcraft_engine::io::save_session(&mut e, &file.to_string_lossy()).unwrap();
    assert!(out.join("Audio Files").join("Song - Drums.wav").is_file());
    assert!(out.join("Audio Files").join("Song - Vocals-clean.wav").is_file());
    let mut again = Engine::new(Session::default());
    let notes = soundcraft_engine::io::open_session(&mut again, &file.to_string_lossy()).unwrap();
    assert_eq!(names(&again), before, "{notes:?}");
    for t in &again.session().tracks {
        for c in t.clips() {
            let a = again.session().pool.get(c.source().unwrap()).unwrap();
            assert_eq!(a.frames() as i64, c.length, "{} reloads its audio", t.name);
        }
    }
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn cancel_at_any_point_changes_nothing() {
    for at in [0.0f32, 0.5, 0.93, 0.999] {
        let (e, _) = session();
        let clip = e.session().tracks[0].clips()[0].id;
        let before = (e.session().clone(), e.undo_history());
        let mut job = ml::prepare(&e, "ml.clean", &json!({"clip": clip.0, "dereverb": false})).unwrap();
        let cancel = Cancel::new();
        let c = cancel.clone();
        let r = job.run(&Ctx::new(cancel, move |f, _| {
            if f >= at {
                c.cancel();
            }
        }));
        assert!(matches!(r, Err(MlError::Cancelled)), "cancel at {at}: {r:?}");
        // Nothing was applied: no tracks, no new sources (so no files on save), no undo step.
        assert_eq!((e.session().clone(), e.undo_history()), before, "cancel at {at}");
    }
}

#[test]
fn clean_vocal_refuses_a_clip_changed_while_it_ran_and_keeps_clip_effects_live() {
    let (mut e, _) = session();
    let clip = e.session().tracks[0].clips()[0].clone();
    // A clip effect (gain) on the clip: Clean renders without it, and it stays on the clip.
    e.session_mut().edit.values.insert(format!("clip_fx.{}.gain", clip.id.0), -12.0);
    let p = json!({"clip": clip.id.0, "dereverb": false, "amount_db": 0.0});
    let mut job = ml::prepare(&e, "ml.clean", &p).unwrap();
    let result = job.run(&Ctx::default()).unwrap();
    // Moved meanwhile: refused, nothing changed.
    e.session_mut().find_clip_mut(clip.id).unwrap().start += 100;
    let snapshot = (e.session().clone(), e.undo_history());
    let err = ml::apply(&mut e, &job, result.clone()).unwrap_err().to_string();
    assert!(err.contains("moved, trimmed or changed"), "{err}");
    assert_eq!((e.session().clone(), e.undo_history()), snapshot);
    // Back in place: applied; amount 0 is an identity, so the new audio is the clip's own audio
    // without the effect (the effect is not baked in, so it is not applied twice).
    e.session_mut().find_clip_mut(clip.id).unwrap().start -= 100;
    ml::apply(&mut e, &job, result).unwrap();
    let c = e.session().tracks[0].clips()[0].clone();
    let new = &e.session().pool.get(c.clean_alt().unwrap().source).unwrap().buffer().channels;
    let old = &e.session().pool.get(clip.source().unwrap()).unwrap().buffer().channels;
    let err = new.iter().zip(old).flat_map(|(a, b)| a.iter().zip(b).map(|(x, y)| (x - y).abs())).fold(0.0f32, f32::max);
    assert!(err < 1e-6, "{err}");
    assert_eq!(e.session().edit.values.get(&format!("clip_fx.{}.gain", clip.id.0)), Some(&-12.0));
}
