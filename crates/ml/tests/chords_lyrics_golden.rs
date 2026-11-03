//! Detect Chords (BTC) and Transcribe Lyrics (Whisper large-v3, whisper.cpp) against the export
//! lane's golden files (`<models dir>/<id>/golden/`, made by each model's `reference_infer.py`).
//!
//! As `golden.rs`: point `SOUNDCRAFT_MODELS_DIR` at the models; a missing model prints `SKIPPED`
//! (`ML_GOLDEN_STRICT=1` makes it a failure).

use serde_json::Value;
use soundcraft_ml::chords::{self, ChordOptions, ChordSegment};
use soundcraft_ml::lyrics::{self, LyricsOptions, LyricsQuality, RawSegment, RawToken, Word};

/// The golden runs' settings: the f16 model, nothing skipped.
fn best() -> LyricsOptions {
    LyricsOptions { quality: LyricsQuality::Best, skip_silence: false, ..LyricsOptions::default() }
}
use soundcraft_ml::models::{self, InstalledModel};
use soundcraft_ml::{Cancel, Ctx, Device, MlError};
use std::path::Path;

fn read_f32(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
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

// ---- chords -------------------------------------------------------------------------------------

fn golden_segments(m: &InstalledModel) -> Vec<ChordSegment> {
    serde_json::from_value(read_json(&m.dir.join("golden/segments_12s.json"))["segments"].clone()).unwrap()
}

fn same_segments(what: &str, got: &[ChordSegment], want: &[ChordSegment]) {
    assert_eq!(got.len(), want.len(), "{what}: {got:#?}");
    for (g, w) in got.iter().zip(want) {
        assert_eq!(
            (&g.label, g.class, &g.name, g.root_pc, &g.quality, g.bass_pc),
            (&w.label, w.class, &w.name, w.root_pc, &w.quality, w.bass_pc),
            "{what}"
        );
        assert!((g.start - w.start).abs() < 1e-9 && (g.end - w.end).abs() < 1e-9, "{what}: {g:?} vs {w:?}");
    }
    println!("{what}: {} segments identical: {}", got.len(), got.iter().map(|s| s.label.as_str()).collect::<Vec<_>>().join(" "));
}

#[test]
fn btc_network_and_decoder_match_golden() {
    let Some(m) = model(chords::ID) else { return };
    let input = read_f32(&m.dir.join("golden/input_12s.f32"));
    let (gl, gc) = (read_f32(&m.dir.join("golden/logits_12s.f32")), read_f32(&m.dir.join("golden/cqt_12s.f32")));
    let want = golden_segments(&m);
    // The decoder alone on the golden outputs: exactly the golden segments.
    same_segments("btc decoder", &chords::decode(&gl, &gc, &ChordOptions { bass: true }), &want);
    // The network on the CPU.
    let (logits, cqt, dev) = chords::run_network(&m, &input, Device::Cpu, &Ctx::default()).unwrap();
    let (sl, sc) = (snr(&gl, &logits), snr(&gc, &cqt));
    println!("btc ({dev}): logits {sl:.1} dB, cqt {sc:.1} dB");
    assert!(sl > 90.0 && sc > 90.0, "logits {sl:.1} dB, cqt {sc:.1} dB");
    // The whole pipeline at 22050 Hz and at 48 kHz (resampled).
    let (segs, _) = chords::detect(&m, std::slice::from_ref(&input), chords::SR, &ChordOptions { bass: true }, Device::Cpu, &Ctx::default()).unwrap();
    same_segments("btc pipeline", &segs, &want);
    let at48 = soundcraft_dsp::offline::resample(&[input], chords::SR, 48_000);
    let (segs48, _) = chords::detect(&m, &at48, 48_000, &ChordOptions { bass: true }, Device::Cpu, &Ctx::default()).unwrap();
    let names: Vec<&str> = segs48.iter().filter(|s| s.is_chord()).map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Em", "C", "D"], "48 kHz");
}

#[test]
fn btc_on_directml_gives_the_golden_segments() {
    let Some(m) = model(chords::ID) else { return };
    let input = read_f32(&m.dir.join("golden/input_12s.f32"));
    let (segs, dev) = chords::detect(&m, &[input], chords::SR, &ChordOptions { bass: true }, Device::Gpu, &Ctx::default()).unwrap();
    if dev != "GPU (DirectML)" {
        println!("SKIPPED btc on DirectML: {dev}");
        return;
    }
    same_segments("btc (DirectML)", &segs, &golden_segments(&m));
}

#[test]
fn btc_without_bass_names_root_position_chords_and_cancels() {
    let Some(m) = model(chords::ID) else { return };
    let input = read_f32(&m.dir.join("golden/input_12s.f32"));
    let (segs, _) =
        chords::detect(&m, std::slice::from_ref(&input), chords::SR, &ChordOptions { bass: false }, Device::Cpu, &Ctx::default()).unwrap();
    assert!(segs.iter().filter(|s| s.is_chord()).all(|s| !s.name.contains('/') && s.bass_pc == s.root_pc));
    let ctx = Ctx::default();
    ctx.cancel_flag().cancel();
    assert_eq!(chords::detect(&m, &[input], chords::SR, &ChordOptions::default(), Device::Cpu, &ctx).unwrap_err(), MlError::Cancelled);
}

// ---- lyrics -------------------------------------------------------------------------------------

/// whisper-cli's JSON as the decoder's segments.
fn raw_segments(js: &Value) -> Vec<RawSegment> {
    js["transcription"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| RawSegment {
            from: s["offsets"]["from"].as_f64().unwrap() / 1000.0,
            to: s["offsets"]["to"].as_f64().unwrap() / 1000.0,
            text: s["text"].as_str().unwrap().to_string(),
            tokens: s["tokens"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| RawToken {
                    id: t["id"].as_i64().unwrap() as i32,
                    bytes: t["text"].as_str().unwrap().as_bytes().to_vec(),
                    p: t["p"].as_f64().unwrap() as f32,
                    t_dtw: t["t_dtw"].as_i64().unwrap(),
                })
                .collect(),
        })
        .collect()
}

fn golden_words(m: &InstalledModel) -> (Vec<Word>, Vec<lyrics::Line>) {
    let v = read_json(&m.dir.join("golden/words_10s.json"));
    (serde_json::from_value(v["words"].clone()).unwrap(), serde_json::from_value(v["segments"].clone()).unwrap())
}

fn same_words(what: &str, got: &[Word], want: &[Word], tol_s: f64, tol_p: f64) {
    let text = |w: &[Word]| w.iter().map(|w| w.text.clone()).collect::<Vec<_>>().join(" ");
    assert_eq!(text(got), text(want), "{what}");
    for (g, w) in got.iter().zip(want) {
        assert!((g.start - w.start).abs() <= tol_s && (g.end - w.end).abs() <= tol_s, "{what}: {g:?} vs {w:?}");
        assert!((g.prob - w.prob).abs() <= tol_p, "{what}: {g:?} vs {w:?}");
    }
    println!("{what}: \"{}\"", text(got));
}

#[test]
fn lyrics_host_steps_match_golden() {
    let Some(m) = model(lyrics::ID) else { return };
    let audio = read_f32(&m.dir.join("golden/input_10s.f32"));
    let raw = raw_segments(&read_json(&m.dir.join("golden/whisper_cpp_full_10s.json")));
    let (words, lines) = lyrics::decode(&raw, &audio, &lyrics::phrases(&m));
    let (gw, gl) = golden_words(&m);
    same_words("lyrics decoder", &words, &gw, 1e-9, 1e-6);
    assert_eq!(lines.len(), gl.len());
    for (a, b) in lines.iter().zip(&gl) {
        assert_eq!(a.text, b.text);
        assert!((a.start - b.start).abs() < 1e-9 && (a.end - b.end).abs() < 1e-9);
    }
}

#[test]
fn whisper_matches_the_golden_transcript() {
    let Some(m) = model(lyrics::ID) else { return };
    let audio = read_f32(&m.dir.join("golden/input_10s.f32"));
    let t0 = std::time::Instant::now();
    let (raw, lang, _) = lyrics::whisper::run(&m, &audio, &best(), &Ctx::default()).unwrap();
    let secs = t0.elapsed().as_secs_f64();
    let golden = raw_segments(&read_json(&m.dir.join("golden/whisper_cpp_full_10s.json")));
    let ids = |r: &[RawSegment]| r.iter().map(|s| s.tokens.iter().map(|t| t.id).collect::<Vec<_>>()).collect::<Vec<_>>();
    println!("whisper (CPU, {secs:.1} s for 10 s, language {lang}): {:?}", raw.iter().map(|s| s.text.clone()).collect::<Vec<_>>());
    assert_eq!(ids(&raw), ids(&golden), "the same tokens as whisper.cpp b5454 on the CPU");
    // DTW times within 2 centiseconds, segment spans exact.
    for (a, b) in raw.iter().zip(&golden) {
        assert_eq!((a.from, a.to), (b.from, b.to));
        for (x, y) in a.tokens.iter().zip(&b.tokens) {
            assert!((x.t_dtw - y.t_dtw).abs() <= 2, "{x:?} vs {y:?}");
        }
    }
    let (words, _) = lyrics::decode(&raw, &audio, &lyrics::phrases(&m));
    same_words("whisper pipeline", &words, &golden_words(&m).0, 0.02, 0.02);
}

#[test]
fn whisper_cancels_while_transcribing() {
    let Some(m) = model(lyrics::ID) else { return };
    let audio = read_f32(&m.dir.join("golden/input_10s.f32"));
    let cancel = Cancel::new();
    let c2 = cancel.clone();
    let at = std::sync::Arc::new(std::sync::Mutex::new(None::<std::time::Instant>));
    let a2 = std::sync::Arc::clone(&at);
    // Cancel as soon as transcription starts (the model is loaded).
    let ctx = Ctx::new(cancel, move |_, what| {
        if what.starts_with("Transcribing") {
            *a2.lock().unwrap() = Some(std::time::Instant::now());
            c2.cancel();
        }
    });
    let r = lyrics::whisper::run(&m, &audio, &best(), &ctx);
    assert_eq!(r.unwrap_err(), MlError::Cancelled);
    let stop = at.lock().unwrap().expect("transcription started").elapsed();
    println!("whisper cancelled {:.2} s after it started", stop.as_secs_f64());
    // The job returns at once; whisper.cpp itself stops at its next step on its own thread.
    assert!(stop.as_secs_f64() < 2.0, "{stop:?}");
}

#[test]
fn fast_weights_give_the_golden_words() {
    let Some(m) = model(lyrics::ID) else { return };
    let w = lyrics::weights(&m, LyricsQuality::Fast);
    if w.file == m.card.file {
        println!("SKIPPED fast lyrics weights: the card has no variants.fast ({:?})", w.note);
        return;
    }
    assert!(w.note.is_none(), "{:?}", w.note);
    let audio = read_f32(&m.dir.join("golden/input_10s.f32"));
    let t0 = std::time::Instant::now();
    let (l, note) = lyrics::transcribe(&m, &[audio], lyrics::SR, &LyricsOptions::default(), &Ctx::default()).unwrap();
    println!("{note}: {:.1} s for 10 s", t0.elapsed().as_secs_f64());
    // Same words; times within 0.1 s, probabilities may differ.
    same_words("whisper fast", &l.words, &golden_words(&m).0, 0.1, 1.0);
    // A broken fast file falls back to the full model, with a note.
    let mut bad = m.clone();
    if let Some(v) = bad.card.extra.get_mut("variants").and_then(|v| v.get_mut("fast")) {
        v["sha256"] = serde_json::json!("00");
    }
    let w = lyrics::weights(&bad, LyricsQuality::Fast);
    assert_eq!(w.file, m.card.file);
    assert!(w.note.is_some_and(|n| n.contains("full model")));
}
