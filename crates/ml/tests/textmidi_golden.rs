//! MIDI-LLM (`midi_llm`) against its export lane's golden files (`<models dir>/midi_llm/golden/`,
//! made by `tools/ml-export/midillm/reference_infer.py` on CPU ONNX Runtime): the prompt's token
//! ids, the first steps' logits (SNR), the most likely token along a piece (teacher-forced), the
//! decoded parts, and a short seeded generation.
//!
//! As `golden.rs`: point `SOUNDCRAFT_MODELS_DIR` (or `FREEDAWM_MODELS_DIR`) at the models; a missing
//! model prints `SKIPPED` (`ML_GOLDEN_STRICT=1` makes it a failure).

use serde_json::Value;
use soundcraft_ml::models::{self, InstalledModel};
use soundcraft_ml::textmidi::{self, MidiLlm, TextMidiOptions};
use soundcraft_ml::{Cancel, Ctx, Device};
use std::path::Path;

fn model() -> Option<InstalledModel> {
    let dir = models::models_dir();
    match models::load(&dir, textmidi::ID) {
        Ok(m) if m.dir.join("golden").is_dir() => Some(m),
        other => {
            let why = other.map_or_else(|e| e.to_string(), |_| "no golden/ folder".into());
            assert!(std::env::var_os("ML_GOLDEN_STRICT").is_none(), "{}: {why}", textmidi::ID);
            println!("SKIPPED {}: {why} (set SOUNDCRAFT_MODELS_DIR to the folder holding the models)", textmidi::ID);
            None
        }
    }
}

fn json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
}

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

fn ints(v: &Value) -> Vec<i64> {
    v.as_array().unwrap().iter().map(|x| x.as_i64().unwrap()).collect()
}

#[test]
fn midi_llm_matches_its_reference() {
    let Some(m) = model() else { return };
    let gold = m.dir.join("golden");
    let mut llm = MidiLlm::open(&m, Device::Cpu, &Cancel::new()).unwrap();

    // The prompt's ids, exactly.
    let p = json(&gold.join("prompt.json"));
    let ids = llm.prompt_ids(p["prompt"].as_str().unwrap()).unwrap();
    assert_eq!(ids, ints(&p["ids"]), "prompt token ids");

    // Teacher-forced along the golden piece: the first logits close, the best token of each step's
    // kind almost always the same.
    let t = json(&gold.join("tokens.json"));
    let tokens: Vec<u32> = ints(&t["tokens"]).into_iter().map(|v| v as u32).collect();
    let argmax = ints(&t["argmax"]);
    assert_eq!(argmax.len(), tokens.len() + 1, "one step after the prompt and one after each token");
    let want = read_f32(&gold.join("logits_4.f32"));
    let n = textmidi::MIDI_TOKENS as usize;
    let mut same = 0;
    let mut logits = llm.step(&ids).unwrap();
    for (i, want_best) in argmax.iter().enumerate() {
        if let Some(w) = want.get(i * n..(i + 1) * n) {
            let s = snr(w, &logits);
            assert!(s >= 30.0, "step {i}: logit SNR {s:.1} dB");
        }
        let r = textmidi::allowed(i);
        let best = r.start + logits[r.start as usize..r.end as usize].iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0 as u32;
        if i64::from(best) == *want_best {
            same += 1;
        }
        if let Some(&tok) = tokens.get(i) {
            logits = llm.step(&[i64::from(textmidi::LLAMA_VOCAB + tok)]).unwrap();
        }
    }
    let agree = f64::from(same) / argmax.len() as f64;
    assert!(agree >= 0.97, "most likely token agrees on {agree:.3} of the steps");

    // The golden tokens decode to the golden parts.
    let parts = textmidi::decode(&tokens, 100.0);
    let notes = json(&gold.join("notes.json"));
    let notes = notes.as_array().unwrap();
    assert_eq!(parts.len(), notes.len());
    for (part, g) in parts.iter().zip(notes) {
        assert_eq!(i64::from(part.instrument), g["instrument"].as_i64().unwrap());
        assert_eq!(part.midi.drums, g["drums"].as_bool().unwrap());
        let gn = g["notes"].as_array().unwrap();
        assert_eq!(part.midi.notes.len(), gn.len());
        for (a, b) in part.midi.notes.iter().zip(gn) {
            assert!((a.start - b["start"].as_f64().unwrap()).abs() < 1e-9 && (a.end - b["end"].as_f64().unwrap()).abs() < 1e-9);
            assert_eq!(i64::from(a.pitch), b["pitch"].as_i64().unwrap());
        }
    }

    // A short seeded piece: whole notes, within the length, the same again for the same seed.
    let opts = TextMidiOptions { seconds: 8.0, max_tokens: 90, seed: 7, ..TextMidiOptions::new("A slow piano ballad in C major") };
    let a = llm.generate(&opts, &Ctx::default()).unwrap();
    let b = llm.generate(&opts, &Ctx::default()).unwrap();
    assert_eq!(a.tokens, b.tokens, "same seed, same notes");
    assert_eq!(a.tokens.len() % 3, 0);
    assert!(!a.parts.is_empty());
    for part in &a.parts {
        for n in &part.midi.notes {
            assert!(n.start >= 0.0 && n.end <= 8.0 && n.end > n.start, "{n:?}");
        }
    }
    println!("parts: {:?}", a.parts.iter().map(|p| (p.instrument, p.midi.notes.len())).collect::<Vec<_>>());
}
