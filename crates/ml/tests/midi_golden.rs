//! The audio → MIDI runners against their export lane's golden files (`<models dir>/<id>/golden/`,
//! made by each model's `reference_infer.py` on CPU ONNX Runtime): raw network outputs (SNR), the
//! decoder alone on the golden outputs (exactly the golden notes), and the whole pipeline (the
//! same notes, times within 1 ms, equal velocities).
//!
//! As `golden.rs`: point `SOUNDCRAFT_MODELS_DIR` at the models; a missing model prints `SKIPPED`
//! (`ML_GOLDEN_STRICT=1` makes it a failure).

use serde_json::Value;
use soundcraft_ml::models::{self, InstalledModel};
use soundcraft_ml::runtime::OnnxModel;
use soundcraft_ml::{Ctx, Device, MidiModel, MidiOptions, NoteEvent};
use std::path::Path;

fn read_f32(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

fn snr(reference: &[f32], x: &[f32]) -> f64 {
    assert_eq!(reference.len(), x.len());
    let (mut s, mut e) = (0.0f64, 0.0f64);
    for (r, v) in reference.iter().zip(x) {
        s += f64::from(*r) * f64::from(*r);
        e += f64::from(r - v) * f64::from(r - v);
    }
    if e == 0.0 { f64::INFINITY } else { 10.0 * (s.max(1e-30) / e).log10() }
}

fn model(id: &str) -> Option<InstalledModel> {
    let dir = models::models_dir();
    match models::load(&dir, id) {
        Ok(m) if m.dir.join("golden").is_dir() => Some(m),
        other => {
            let why = other.map_or_else(|e| e.to_string(), |_| "no golden/ folder".into());
            assert!(std::env::var_os("ML_GOLDEN_STRICT").is_none(), "{id}: {why}");
            println!("SKIPPED {id}: {why} (set SOUNDCRAFT_MODELS_DIR to the folder holding the models)");
            None
        }
    }
}

fn golden_json(m: &InstalledModel) -> Value {
    let p = m.dir.join("golden/notes_5s.json");
    serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

/// The golden notes as `NoteEvent`s.
fn golden_notes(v: &Value) -> Vec<NoteEvent> {
    v["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| NoteEvent {
            start: n["start"].as_f64().unwrap(),
            end: n["end"].as_f64().unwrap(),
            pitch: n["pitch"].as_u64().unwrap() as u8,
            velocity: n["velocity"].as_u64().unwrap() as u8,
            bends: n.get("bends").and_then(Value::as_array).map(|b| b.iter().map(|x| x.as_i64().unwrap() as i32).collect()).unwrap_or_default(),
        })
        .collect()
}

/// Same notes in the same order, times within `tol` s, equal pitches, velocities and bends.
fn same_notes(what: &str, got: &[NoteEvent], want: &[NoteEvent], tol: f64) {
    assert_eq!(got.len(), want.len(), "{what}: {} notes, golden {}", got.len(), want.len());
    let mut worst = 0.0f64;
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert_eq!((g.pitch, g.velocity), (w.pitch, w.velocity), "{what}: note {i}: {g:?} vs {w:?}");
        assert_eq!(g.bends, w.bends, "{what}: note {i} bends");
        worst = worst.max((g.start - w.start).abs()).max((g.end - w.end).abs());
        assert!(worst <= tol, "{what}: note {i}: {g:?} vs {w:?}");
    }
    println!("{what}: {} notes identical, worst time error {:.3e} s", got.len(), worst);
}

#[test]
fn basicpitch_matches_golden() {
    let Some(m) = model("basicpitch") else { return };
    let g = |n: &str| read_f32(&m.dir.join(format!("golden/{n}_5s.f32")));
    let want = golden_json(&m);
    let want_notes = golden_notes(&want);
    let opts = MidiOptions::new(MidiModel::General);
    // The decoder alone on the golden network outputs.
    let t = g("note").len() / 88;
    let raw = soundcraft_ml::basicpitch::Raw { frames: t, note: g("note"), onset: g("onset"), contour: g("contour") };
    same_notes("basicpitch decoder", &soundcraft_ml::basicpitch::decode(&raw, &opts), &want_notes, 1e-9);
    // The network.
    let input = g("input");
    let mut onnx = OnnxModel::open_model(&m, Device::Cpu, &soundcraft_ml::Cancel::new()).unwrap();
    let got = soundcraft_ml::basicpitch::run_network(&mut onnx, &input, &Ctx::default()).unwrap();
    assert_eq!(got.frames, t);
    for (name, a, b) in [("note", &got.note, &raw.note), ("onset", &got.onset, &raw.onset), ("contour", &got.contour, &raw.contour)] {
        let v = snr(b, a);
        println!("basicpitch {name}: SNR {v:.1} dB");
        assert!(v >= 60.0, "basicpitch {name}: SNR {v:.1} dB");
    }
    // The whole pipeline (no resampling: the golden input is at 22050 Hz).
    let (tr, _) =
        soundcraft_ml::midi::transcribe_as_published(&m, &[input], 22_050, &MidiOptions { pitch_bends: true, ..opts }, Device::Cpu, &Ctx::default())
            .unwrap();
    same_notes("basicpitch", &tr.notes, &want_notes, 1e-3);
    let want_bends: Vec<(f64, i32)> =
        want["midi_bends"].as_array().unwrap().iter().map(|b| (b[0].as_f64().unwrap(), b[1].as_i64().unwrap() as i32)).collect();
    assert_eq!(tr.bends.len(), want_bends.len());
    for (a, b) in tr.bends.iter().zip(&want_bends) {
        assert!((a.0 - b.0).abs() < 1e-3 && a.1 == b.1, "{a:?} vs {b:?}");
    }
}

#[test]
fn piano_kong_matches_golden() {
    let Some(m) = model("piano_kong") else { return };
    let g = |n: &str| read_f32(&m.dir.join(format!("golden/{n}_5s.f32")));
    let want = golden_json(&m);
    let want_notes = golden_notes(&want);
    let want_pedals: Vec<(f64, f64)> =
        want["pedals"].as_array().unwrap().iter().map(|p| (p["start"].as_f64().unwrap(), p["end"].as_f64().unwrap())).collect();
    let outs: Vec<Vec<f32>> = soundcraft_ml::pianokong::OUTS.iter().map(|n| g(n)).collect();
    let raw = soundcraft_ml::pianokong::Raw { frames: outs[0].len() / 88, outs };
    let (notes, pedals) = soundcraft_ml::pianokong::decode(&raw, 0.3, true);
    same_notes("piano_kong decoder", &notes, &want_notes, 1e-9);
    assert_eq!(pedals, want_pedals);
    let input = g("input");
    let mut onnx = OnnxModel::open_model(&m, Device::Cpu, &soundcraft_ml::Cancel::new()).unwrap();
    let got = soundcraft_ml::pianokong::run_network(&mut onnx, &input, &Ctx::default()).unwrap();
    assert_eq!(got.frames, raw.frames);
    for (i, name) in soundcraft_ml::pianokong::OUTS.iter().enumerate() {
        let v = snr(&raw.outs[i], &got.outs[i]);
        println!("piano_kong {name}: SNR {v:.1} dB");
        assert!(v >= 60.0, "piano_kong {name}: SNR {v:.1} dB");
    }
    let (tr, _) =
        soundcraft_ml::midi::transcribe_as_published(&m, &[input], 16_000, &MidiOptions::new(MidiModel::Piano), Device::Cpu, &Ctx::default())
            .unwrap();
    // The pipeline lifts velocity 0 to 1 and cuts at the input's end; the golden notes are inside.
    let clipped: Vec<NoteEvent> =
        want_notes.iter().filter(|n| n.start < 5.0).map(|n| NoteEvent { end: n.end.min(5.0), velocity: n.velocity.max(1), ..n.clone() }).collect();
    same_notes("piano_kong", &tr.notes, &clipped, 1e-3);
    assert_eq!(tr.pedals.len(), want_pedals.len());
    for (a, b) in tr.pedals.iter().zip(&want_pedals) {
        assert!((a.0 - b.0).abs() < 1e-3 && (a.1 - b.1.min(5.0)).abs() < 1e-3, "{a:?} vs {b:?}");
    }
}

#[test]
fn adtof_drums_match_golden() {
    let Some(m) = model("adtof_drums") else { return };
    let g = |n: &str| read_f32(&m.dir.join(format!("golden/{n}.f32")));
    let want_notes = golden_notes(&golden_json(&m));
    let (bands, fb) = soundcraft_ml::adtof::filterbank();
    let want_fb = g("filterbank");
    assert_eq!(fb.len(), want_fb.len());
    let worst = fb.iter().zip(&want_fb).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    println!("adtof filterbank: max error {worst:e}");
    assert!(worst < 1e-6);
    let act_want = g("act_5s");
    let t = act_want.len() / 5;
    // The decoder on the golden activations with no source level: the reference's notes exactly
    // (every hit at velocity 100).
    let raw = soundcraft_ml::adtof::Raw { frames: t, act: act_want.clone(), level: Vec::new() };
    same_notes("adtof decoder", &soundcraft_ml::adtof::decode(&raw, &soundcraft_ml::adtof::THRESHOLDS), &want_notes, 1e-9);
    let input = g("input_5s");
    let (ft, fbands, feats) = soundcraft_ml::adtof::features(&input);
    assert_eq!((ft, fbands), (t, bands));
    let v = snr(&g("features_5s"), &feats);
    println!("adtof features: SNR {v:.1} dB");
    assert!(v >= 80.0);
    let mut onnx = OnnxModel::open_model(&m, Device::Cpu, &soundcraft_ml::Cancel::new()).unwrap();
    let act = soundcraft_ml::adtof::run_network(&mut onnx, ft, fbands, feats).unwrap();
    let v = snr(&act_want, &act);
    println!("adtof activations: SNR {v:.1} dB");
    assert!(v >= 60.0);
    let (tr, _) =
        soundcraft_ml::midi::transcribe_as_published(&m, &[input], 44_100, &MidiOptions::new(MidiModel::Drums), Device::Cpu, &Ctx::default())
            .unwrap();
    assert!(tr.drums);
    // The whole pipeline sets velocities from the source's level: the same hits, each class's
    // loudest at 120.
    let mut flat = tr.notes.clone();
    flat.iter_mut().for_each(|n| n.velocity = soundcraft_ml::adtof::VELOCITY);
    same_notes("adtof_drums", &flat, &want_notes, 1e-3);
    assert!(tr.notes.iter().any(|n| n.velocity == soundcraft_ml::adtof::VELOCITY_TOP));
    assert!(tr.notes.iter().any(|n| n.velocity != soundcraft_ml::adtof::VELOCITY_TOP), "velocities follow the hits' levels");
}

#[test]
fn transcription_cancels() {
    let Some(m) = model("piano_kong") else { return };
    let input = read_f32(&m.dir.join("golden/input_5s.f32"));
    let ctx = Ctx::default();
    ctx.cancel_flag().cancel();
    let r = soundcraft_ml::midi::transcribe_as_published(&m, &[input], 16_000, &MidiOptions::new(MidiModel::Piano), Device::Cpu, &ctx);
    assert_eq!(r.map(|_| ()), Err(soundcraft_ml::MlError::Cancelled));
}

/// Longer inputs (several windows / segments), when the folder has `golden/input_long.f32` and
/// `golden/notes_long.json` from `reference_infer.py` (made by hand; not part of the export).
#[test]
fn long_inputs_match_the_reference() {
    for (id, rate, kind) in [("basicpitch", 22_050, MidiModel::General), ("piano_kong", 16_000, MidiModel::Piano)] {
        let Some(m) = model(id) else { continue };
        let path = m.dir.join("golden/notes_long.json");
        if !path.is_file() {
            println!("SKIPPED {id}: no golden/notes_long.json");
            continue;
        }
        let want: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let input = read_f32(&m.dir.join("golden/input_long.f32"));
        let secs = input.len() as f64 / f64::from(rate);
        let (tr, _) =
            soundcraft_ml::midi::transcribe_as_published(&m, &[input], rate, &MidiOptions::new(kind), Device::Cpu, &Ctx::default()).unwrap();
        let want: Vec<NoteEvent> = golden_notes(&want)
            .into_iter()
            .filter(|n| n.start < secs)
            .map(|n| NoteEvent { end: n.end.min(secs), velocity: n.velocity.max(1), ..n })
            .collect();
        same_notes(&format!("{id} long"), &tr.notes, &want, 1e-3);
    }
}

#[test]
fn game_vocal_matches_golden() {
    let Some(m) = model("game_vocal") else { return };
    let want = golden_json(&m);
    let input = read_f32(&m.dir.join("golden/input_5s.f32"));
    // The slicer cuts where the reference does.
    let slices = soundcraft_ml::game::slice_audio(&input);
    let want_slices = want["slices"].as_array().unwrap();
    assert_eq!(slices.len(), want_slices.len(), "{slices:?}");
    for ((off, a, b), w) in slices.iter().zip(want_slices) {
        assert!((off - w["offset"].as_f64().unwrap()).abs() < 1e-9, "{off}");
        assert_eq!((b - a) as u64, w["n"].as_u64().unwrap());
    }
    // The graphs' outputs per slice.
    let (outs, _) = soundcraft_ml::game::run_slices(&m, &input, Device::Cpu, &Ctx::default()).unwrap();
    for ((_, _, o), w) in outs.iter().zip(want_slices) {
        let f = |k: &str| -> Vec<f64> { w[k].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect() };
        assert_eq!(o.presence, w["presence"].as_array().unwrap().iter().map(|v| v.as_bool().unwrap()).collect::<Vec<_>>());
        for (a, b) in o.durations.iter().zip(f("durations")) {
            assert!((f64::from(*a) - b).abs() < 1e-6, "durations {a} vs {b}");
        }
        // Pitches of the notes that are there (an absent note's score means nothing).
        for ((a, b), on) in o.scores.iter().zip(f("scores")).zip(&o.presence) {
            assert!(!on || (f64::from(*a) - b).abs() < 1e-3, "scores {a} vs {b}");
        }
    }
    let (raw, dev) = soundcraft_ml::game::run(&m, &input, Device::Cpu, &Ctx::default()).unwrap();
    assert_eq!(dev, "CPU");
    let notes = want["notes"].as_array().unwrap();
    assert_eq!(raw.len(), notes.len(), "{raw:?}");
    for ((a, b, p), w) in raw.iter().zip(notes) {
        assert!((a - w["start"].as_f64().unwrap()).abs() < 1e-6 && (b - w["end"].as_f64().unwrap()).abs() < 1e-6, "{a} {b} vs {w}");
        assert!((p - w["pitch_f"].as_f64().unwrap()).abs() < 1e-3, "{p} vs {w}");
    }
    println!("game_vocal: {} notes identical", raw.len());
    let (tr, _) =
        soundcraft_ml::midi::transcribe_as_published(&m, &[input], 44_100, &MidiOptions::new(MidiModel::Vocal), Device::Cpu, &Ctx::default())
            .unwrap();
    same_notes("game_vocal", &tr.notes, &golden_notes(&want), 1e-3);
}
