//! Text → MIDI: MIDI-LLM (Wu et al., ISMIR 2026), Llama 3.2 1B with the Anticipatory Music
//! Transformer's MIDI tokens added to its vocabulary. Port of
//! `tools/ml-export/midillm/reference_infer.py`.
//!
//! - The prompt is `SYSTEM_PROMPT + text + " "`, tokenized by Llama 3's tokenizer
//!   ([`crate::llama_bpe`], `<|begin_of_text|>` first), then the MIDI start token
//!   (`LLAMA_VOCAB + MIDI_BOS`).
//! - The model writes notes as triples in AMT's arrival-time encoding: an onset time (10 ms steps
//!   from 0, up to 100 s), a duration (10 ms steps, up to 10 s) and a note (`instrument * 128 +
//!   pitch`, instrument 0–127 the General MIDI program, 128 drums). Every note is velocity 72.
//! - Sampling is temperature + top-p (1.0 / 0.98 by default) over the MIDI tokens only. Unlike
//!   the reference, which samples from all of them, each position may only be the kind of token
//!   it has to be (time, duration, note), so the output always decodes.
//! - Generation stops at `max_tokens` (2046 = 682 notes, the reference's default) or at the
//!   first onset past the length asked for.
//!
//! [`decode`] splits the notes by instrument: one [`Part`] per General MIDI program, drums apart.

use crate::job::Ctx;
use crate::llama_bpe::LlamaBpe;
use crate::lyrics::Prompt;
use crate::midi::{NoteEvent, Transcription};
use crate::models::InstalledModel;
use crate::runtime::{Device, OnnxModel};
use crate::{MlError, Result};

pub const ID: &str = "midi_llm";
/// The model's text prefix (`generate_transformers.py`).
pub const SYSTEM_PROMPT: &str = "You are a world-class composer. Please compose some music according to the following description: ";
/// Llama 3's text vocabulary; MIDI token `t` is model token `LLAMA_VOCAB + t`.
pub const LLAMA_VOCAB: u32 = 128_256;
/// The MIDI start token (AMT's `AUTOREGRESS`, the GPT-2 AMT model's BOS).
pub const MIDI_BOS: u32 = 55_026;
/// The MIDI tokens the model may write (`ALLOWED_TOKEN_IDS`): `0..MIDI_TOKENS`.
pub const MIDI_TOKENS: u32 = 55_026;

/// AMT's arrival-time vocabulary (`anticipation/vocab.py`, `config.py`).
pub const TIME_RESOLUTION: f64 = 100.0;
pub const MAX_TIME: u32 = 10_000;
pub const MAX_DUR: u32 = 1_000;
pub const MAX_NOTE: u32 = 128 * 129;
pub const DUR_OFFSET: u32 = MAX_TIME;
pub const NOTE_OFFSET: u32 = DUR_OFFSET + MAX_DUR;
/// Instrument number of the drums.
pub const DRUMS: u8 = 128;
pub const VELOCITY: u8 = 72;

pub const DEFAULT_SECONDS: f32 = 30.0;
pub const MAX_SECONDS: f32 = 100.0;
pub const DEFAULT_TEMPERATURE: f32 = 1.0;
pub const DEFAULT_TOP_P: f32 = 0.98;
pub const DEFAULT_MAX_TOKENS: u32 = 2046;
/// The model's longest output: its context was trained to about this many tokens past the prompt.
pub const MAX_TOKENS: u32 = 4095;

/// Generate MIDI from Text's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextMidiOptions {
    /// What to compose ("upbeat jazz with saxophones, B-flat major, 4/4").
    pub prompt: Prompt,
    /// Length in seconds (1–100): notes starting later are not written.
    pub seconds: f32,
    /// Sampling temperature (0.1–2).
    pub temperature: f32,
    /// Nucleus sampling threshold (0.05–1).
    pub top_p: f32,
    /// Random seed; the same seed, prompt and settings give the same notes on the same device.
    pub seed: u64,
    /// The most MIDI tokens to write (3 per note).
    pub max_tokens: u32,
    /// Notes the piece starts with (the model continues them); not part of the result.
    pub prime: Prime,
}

/// The most notes a piece can start with ([`Prime`]).
pub const MAX_PRIME_NOTES: usize = 170;

/// Notes for the model to continue, as MIDI tokens (time, duration, note per note, onsets in
/// order), held inline so job options stay `Copy`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Prime {
    len: u16,
    tokens: [u32; 3 * MAX_PRIME_NOTES],
}

impl Default for Prime {
    fn default() -> Self {
        Prime { len: 0, tokens: [0; 3 * MAX_PRIME_NOTES] }
    }
}

impl std::fmt::Debug for Prime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Prime({} notes)", self.notes())
    }
}

impl Prime {
    /// Notes `(start s, end s, instrument 0–127 or DRUMS, pitch)` from the start of the piece,
    /// as tokens: sorted by onset (then pitch), onsets past 100 s dropped, durations cut at
    /// 10 s, at most [`MAX_PRIME_NOTES`] (the first ones). Non-finite times and out-of-range
    /// instruments or pitches are skipped.
    pub fn from_notes(notes: &[(f64, f64, u8, u8)]) -> Prime {
        let mut v: Vec<(u32, u32, u32)> = notes
            .iter()
            .filter(|(a, b, i, p)| a.is_finite() && b.is_finite() && *i <= DRUMS && *p < 128)
            .filter_map(|&(a, b, i, p)| {
                let t = (a.max(0.0) * TIME_RESOLUTION).round();
                if t >= f64::from(MAX_TIME) {
                    return None;
                }
                let d = ((b - a).max(0.0) * TIME_RESOLUTION).round().clamp(0.0, f64::from(MAX_DUR - 1));
                Some((t as u32, d as u32, u32::from(i) * 128 + u32::from(p)))
            })
            .collect();
        v.sort_by_key(|&(t, _, n)| (t, n % 128, n));
        let mut prime = Prime::default();
        for (k, (t, d, n)) in v.into_iter().take(MAX_PRIME_NOTES).enumerate() {
            if let Some(slot) = prime.tokens.get_mut(3 * k..3 * k + 3) {
                slot.copy_from_slice(&[t, DUR_OFFSET + d, NOTE_OFFSET + n]);
                prime.len = u16::try_from(3 * k + 3).unwrap_or(0);
            }
        }
        prime
    }

    /// The MIDI tokens.
    pub fn tokens(&self) -> &[u32] {
        self.tokens.get(..usize::from(self.len)).unwrap_or(&[])
    }

    pub fn notes(&self) -> usize {
        usize::from(self.len) / 3
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl TextMidiOptions {
    pub fn new(prompt: &str) -> Self {
        TextMidiOptions {
            prompt: Prompt::new(prompt),
            seconds: DEFAULT_SECONDS,
            temperature: DEFAULT_TEMPERATURE,
            top_p: DEFAULT_TOP_P,
            seed: 0,
            max_tokens: DEFAULT_MAX_TOKENS,
            prime: Prime::default(),
        }
    }

    /// The settings, checked: a usable prompt and every number in range.
    pub fn validated(self) -> Result<Self> {
        if self.prompt.as_str().trim().is_empty() {
            return Err(MlError::BadInput("describe the music to compose (the prompt is empty)".into()));
        }
        let range = |name: &str, v: f32, lo: f32, hi: f32| {
            if v.is_finite() && (lo..=hi).contains(&v) { Ok(()) } else { Err(MlError::BadInput(format!("`{name}` must be between {lo} and {hi}"))) }
        };
        range("seconds", self.seconds, 1.0, MAX_SECONDS)?;
        range("temperature", self.temperature, 0.1, 2.0)?;
        range("top_p", self.top_p, 0.05, 1.0)?;
        if !(3..=MAX_TOKENS).contains(&self.max_tokens) {
            return Err(MlError::BadInput(format!("`max_tokens` must be between 3 and {MAX_TOKENS}")));
        }
        Ok(self)
    }
}

/// One instrument's notes.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    /// General MIDI program 0–127 ([`DRUMS`] for the drums).
    pub instrument: u8,
    /// The notes, seconds from the start; `drums` set for the drum part.
    pub midi: Transcription,
}

impl Part {
    pub fn is_drums(&self) -> bool {
        self.instrument == DRUMS
    }
}

/// What a run made.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Generated {
    pub parts: Vec<Part>,
    /// The MIDI tokens written (AMT ids, without the start token).
    pub tokens: Vec<u32>,
}

/// The model's text before the MIDI: `SYSTEM_PROMPT + prompt + " "`. `<|` is broken up so a
/// prompt can't name an added token.
pub fn prompt_text(prompt: &str) -> String {
    format!("{SYSTEM_PROMPT}{} ", prompt.trim().replace("<|", "< |"))
}

/// The model input before the first MIDI token: the prompt's text tokens and the MIDI start.
pub fn prompt_ids(tok: &LlamaBpe, prompt: &str) -> Result<Vec<i64>> {
    let mut ids: Vec<i64> = tok.encode(&prompt_text(prompt), true)?.into_iter().map(i64::from).collect();
    ids.push(i64::from(LLAMA_VOCAB + MIDI_BOS));
    Ok(ids)
}

/// Which MIDI tokens position `i` of the output may hold: times, durations, then notes.
pub fn allowed(i: usize) -> std::ops::Range<u32> {
    match i % 3 {
        0 => 0..MAX_TIME,
        1 => DUR_OFFSET..DUR_OFFSET + MAX_DUR,
        _ => NOTE_OFFSET..NOTE_OFFSET + MAX_NOTE,
    }
}

/// SplitMix64: a small seeded generator (the same seed gives the same notes).
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Sample one MIDI token from `logits` (indexed by MIDI token id) within `range`, with
/// `temperature` and top-p. Non-finite logits never win; `None` when nothing in range is usable.
pub fn sample(logits: &[f32], range: std::ops::Range<u32>, temperature: f32, top_p: f32, rng: &mut Rng) -> Option<u32> {
    let lo = usize::try_from(range.start).ok()?;
    let hi = usize::try_from(range.end).ok()?.min(logits.len());
    let slice = logits.get(lo..hi)?;
    let t = f64::from(temperature.max(1e-3));
    let max = slice.iter().copied().filter(|v| v.is_finite()).fold(f32::NEG_INFINITY, f32::max);
    if !max.is_finite() {
        return None;
    }
    let mut probs: Vec<(u32, f64)> = slice
        .iter()
        .enumerate()
        .filter(|(_, v)| v.is_finite())
        .filter_map(|(i, v)| Some((range.start.checked_add(u32::try_from(i).ok()?)?, ((f64::from(*v) - f64::from(max)) / t).exp())))
        .collect();
    let total: f64 = probs.iter().map(|p| p.1).sum();
    if total.is_nan() || total <= 0.0 {
        return None;
    }
    probs.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    // Keep the smallest set whose probability reaches top_p (at least one token).
    let keep = f64::from(top_p.clamp(0.0, 1.0)) * total;
    let mut cum = 0.0;
    let mut n = 0;
    for p in &probs {
        n += 1;
        cum += p.1;
        if cum >= keep {
            break;
        }
    }
    let kept = probs.get(..n)?;
    let mut r = rng.unit() * cum;
    for p in kept {
        r -= p.1;
        if r < 0.0 {
            return Some(p.0);
        }
    }
    kept.last().map(|p| p.0)
}

/// Generate MIDI tokens. `next` runs the model: given the new input ids (the whole prompt
/// first, then one token per call) it returns the logits of the last position over the MIDI
/// tokens (`logits[t]` for MIDI token `t`, at least [`MIDI_TOKENS`] long).
pub fn generate_with(prompt: &[i64], opts: &TextMidiOptions, ctx: &Ctx, mut next: impl FnMut(&[i64]) -> Result<Vec<f32>>) -> Result<Vec<u32>> {
    let mut rng = Rng::new(opts.seed);
    let limit = (f64::from(opts.seconds) * TIME_RESOLUTION).round().max(0.0);
    let max = usize::try_from(opts.max_tokens).unwrap_or(usize::MAX);
    let mut out: Vec<u32> = Vec::with_capacity(max);
    let mut logits = next(prompt)?;
    while out.len() < max {
        ctx.check()?;
        let i = out.len();
        let Some(tok) = sample(&logits, allowed(i), opts.temperature, opts.top_p, &mut rng) else {
            return Err(MlError::Model(ID.into(), "the model's output has no usable values".into()));
        };
        // Onsets come in time order: the first one past the end finishes the piece.
        if i.is_multiple_of(3) && f64::from(tok) >= limit {
            break;
        }
        out.push(tok);
        ctx.report(i as f32 / max as f32);
        if out.len() >= max {
            break;
        }
        logits = next(&[i64::from(LLAMA_VOCAB + tok)])?;
    }
    // Whole notes only.
    out.truncate(out.len() - out.len() % 3);
    Ok(out)
}

/// Notes by instrument from MIDI tokens: parts in General MIDI program order, drums last.
/// Triples that aren't (time, duration, note) are skipped; notes are cut at `seconds`.
pub fn decode(tokens: &[u32], seconds: f64) -> Vec<Part> {
    let mut by_instrument: std::collections::BTreeMap<u8, Vec<NoteEvent>> = std::collections::BTreeMap::new();
    for t in tokens.chunks_exact(3) {
        let (Some(&time), Some(&dur), Some(&note)) = (t.first(), t.get(1), t.get(2)) else { continue };
        if time >= MAX_TIME || !(DUR_OFFSET..DUR_OFFSET + MAX_DUR).contains(&dur) || !(NOTE_OFFSET..NOTE_OFFSET + MAX_NOTE).contains(&note) {
            continue;
        }
        let note = note - NOTE_OFFSET;
        let (Ok(pitch), Ok(instrument)) = (u8::try_from(note % 128), u8::try_from(note / 128)) else { continue };
        let start = f64::from(time) / TIME_RESOLUTION;
        if start >= seconds {
            continue;
        }
        // A zero duration still sounds for one step.
        let end = (start + f64::from((dur - DUR_OFFSET).max(1)) / TIME_RESOLUTION).min(seconds);
        by_instrument.entry(instrument).or_default().push(NoteEvent { start, end, pitch, velocity: VELOCITY, bends: Vec::new() });
    }
    by_instrument
        .into_iter()
        .map(|(instrument, mut notes)| {
            notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
            Part { instrument, midi: Transcription { notes, pedals: Vec::new(), bends: Vec::new(), drums: instrument == DRUMS } }
        })
        .collect()
}

/// MIDI-LLM loaded: the ONNX decoder with its key/value cache (graph from onnxruntime-genai's
/// model builder: `input_ids`, `attention_mask`, `past_key_values.{i}.key/value` → `logits`,
/// `present.{i}.*`; `export.py` keeps only the MIDI vocabulary's logits) and the tokenizer.
pub struct MidiLlm {
    onnx: OnnxModel,
    tok: LlamaBpe,
    layers: usize,
    kv_heads: i64,
    head_dim: i64,
    past: Vec<ort::value::DynValue>,
    len: i64,
}

impl std::fmt::Debug for MidiLlm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiLlm").field("onnx", &self.onnx).field("len", &self.len).finish()
    }
}

fn rt(what: &str, e: ort::Error) -> MlError {
    MlError::Runtime(format!("`{ID}`: {what}: {e}"))
}

fn model_err(msg: impl Into<String>) -> MlError {
    MlError::Model(ID.into(), msg.into())
}

impl MidiLlm {
    /// Load the installed model on `device` (its card keeps it on the CPU unless asked).
    pub fn open(model: &InstalledModel, device: Device, cancel: &crate::Cancel) -> Result<Self> {
        let tokenizer_file = model.card.text("tokenizer").unwrap_or("tokenizer.json");
        let tok = LlamaBpe::open(&model.file(tokenizer_file)?)?;
        let onnx = OnnxModel::open_model(model, device, cancel)?;
        let int = |key: &str, d: f64| {
            let v = model.card.num(key, d);
            if v.fract() == 0.0 && (1.0..=4096.0).contains(&v) { Ok(v as i64) } else { Err(model_err(format!("card: `{key}` is not a size"))) }
        };
        let layers = usize::try_from(int("layers", 16.0)?).map_err(|_| model_err("card: `layers`"))?;
        Ok(MidiLlm { onnx, tok, layers, kv_heads: int("kv_heads", 8.0)?, head_dim: int("head_dim", 64.0)?, past: Vec::new(), len: 0 })
    }

    /// "CPU", "GPU (DirectML)" or why the GPU wasn't used.
    pub fn device_label(&self) -> String {
        self.onnx.fallback_note.clone().unwrap_or_else(|| self.onnx.device_label().to_string())
    }

    /// The model input for `prompt` ([`prompt_ids`]).
    pub fn prompt_ids(&self, prompt: &str) -> Result<Vec<i64>> {
        prompt_ids(&self.tok, prompt)
    }

    /// Forget the cache: the next [`MidiLlm::step`] starts a new piece.
    pub fn reset(&mut self) {
        self.past.clear();
        self.len = 0;
    }

    /// Feed `ids` (model token ids) after what came before; the last position's logits over the
    /// MIDI tokens (`[MIDI_TOKENS]`, index = MIDI token).
    pub fn step(&mut self, ids: &[i64]) -> Result<Vec<f32>> {
        if ids.is_empty() {
            return Err(MlError::BadInput("nothing to feed the model".into()));
        }
        let n = i64::try_from(ids.len()).map_err(|_| MlError::BadInput("prompt too long".into()))?;
        let total = self.len.checked_add(n).ok_or_else(|| MlError::BadInput("too long".into()))?;
        let mut inputs: Vec<(String, ort::session::SessionInputValue<'static>)> = Vec::with_capacity(2 + 2 * self.layers);
        inputs.push(("input_ids".into(), ort::value::Tensor::from_array((vec![1, n], ids.to_vec())).map_err(|e| rt("input_ids", e))?.into()));
        let mask = vec![1i64; usize::try_from(total).map_err(|_| MlError::BadInput("too long".into()))?];
        inputs.push(("attention_mask".into(), ort::value::Tensor::from_array((vec![1, total], mask)).map_err(|e| rt("attention_mask", e))?.into()));
        let mut past = std::mem::take(&mut self.past);
        if past.len() != 2 * self.layers {
            past.clear();
            for _ in 0..2 * self.layers {
                let t = ort::value::Tensor::from_array((vec![1, self.kv_heads, 0, self.head_dim], Vec::<f32>::new()))
                    .map_err(|e| rt("empty cache", e))?;
                past.push(t.into_dyn());
            }
        }
        for (i, v) in past.into_iter().enumerate() {
            let name = format!("past_key_values.{}.{}", i / 2, if i % 2 == 0 { "key" } else { "value" });
            inputs.push((name, v.into()));
        }
        // On failure the cache is gone: the next step starts over.
        self.len = 0;
        let mut out = self.onnx.run_values(inputs)?;
        let logits = out.remove("logits").ok_or_else(|| model_err("no output `logits`"))?;
        let (shape, data) = logits.try_extract_tensor::<f32>().map_err(|e| rt("logits", e))?;
        // [1, n, vocab]: the last position's MIDI tokens. A full-vocabulary head (183286) or the
        // MIDI-only head export.py makes (55030).
        let vocab = shape.last().and_then(|v| usize::try_from(*v).ok()).unwrap_or(0);
        let from = data.len().checked_sub(vocab).ok_or_else(|| model_err("empty logits"))?;
        let full = usize::try_from(LLAMA_VOCAB + MIDI_TOKENS).unwrap_or(usize::MAX);
        let lo = from + if vocab >= full { usize::try_from(LLAMA_VOCAB).unwrap_or(usize::MAX) } else { 0 };
        let hi = lo + usize::try_from(MIDI_TOKENS).unwrap_or(0);
        let midi = data.get(lo..hi).ok_or_else(|| model_err(format!("logits have {vocab} entries, not the MIDI vocabulary")))?.to_vec();
        let mut kept = Vec::with_capacity(2 * self.layers);
        for l in 0..self.layers {
            for kind in ["key", "value"] {
                kept.push(out.remove(&format!("present.{l}.{kind}")).ok_or_else(|| model_err(format!("no output `present.{l}.{kind}`")))?);
            }
        }
        self.past = kept;
        self.len = total;
        Ok(midi)
    }

    /// Generate a piece for `opts` (from a fresh cache).
    pub fn generate(&mut self, opts: &TextMidiOptions, ctx: &Ctx) -> Result<Generated> {
        let opts = opts.validated()?;
        self.reset();
        let mut prompt = self.prompt_ids(opts.prompt.as_str())?;
        // Notes to continue go after the MIDI start, as if the model had written them.
        prompt.extend(opts.prime.tokens().iter().map(|&t| i64::from(LLAMA_VOCAB + t)));
        let tokens = generate_with(&prompt, &opts, ctx, |ids| self.step(ids))?;
        self.reset();
        let parts = decode(&tokens, f64::from(opts.seconds));
        Ok(Generated { parts, tokens })
    }
}

/// Generate MIDI for `opts` with the installed model: notes by instrument plus the tokens, and
/// where it ran.
pub fn generate(model: &InstalledModel, opts: &TextMidiOptions, device: Device, ctx: &Ctx) -> Result<(Generated, String)> {
    let opts = opts.validated()?;
    ctx.say("Loading MIDI-LLM");
    let mut m = MidiLlm::open(model, device, ctx.cancel_flag())?;
    ctx.check()?;
    ctx.say("Composing");
    let g = m.generate(&opts, ctx)?;
    ctx.report(1.0);
    Ok((g, m.device_label()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triple(time_s: f64, dur_s: f64, instrument: u32, pitch: u32) -> [u32; 3] {
        [(time_s * 100.0) as u32, DUR_OFFSET + (dur_s * 100.0) as u32, NOTE_OFFSET + instrument * 128 + pitch]
    }

    #[test]
    fn notes_split_by_instrument_drums_last() {
        let mut toks = Vec::new();
        toks.extend(triple(0.0, 0.5, 128, 36)); // kick
        toks.extend(triple(0.0, 1.0, 0, 60)); // piano C4
        toks.extend(triple(0.5, 0.25, 33, 40)); // fingered bass
        toks.extend(triple(1.0, 0.0, 0, 64)); // zero duration
        let parts = decode(&toks, 100.0);
        let instruments: Vec<u8> = parts.iter().map(|p| p.instrument).collect();
        assert_eq!(instruments, vec![0, 33, 128]);
        assert!(parts[2].is_drums() && parts[2].midi.drums && !parts[0].midi.drums);
        assert_eq!(parts[0].midi.notes.len(), 2);
        let n = &parts[0].midi.notes[1];
        assert_eq!((n.pitch, n.velocity), (64, VELOCITY));
        assert!((n.end - n.start - 0.01).abs() < 1e-9, "zero duration sounds one step");
        assert!((parts[1].midi.notes[0].end - 0.75).abs() < 1e-9);
    }

    #[test]
    fn bad_triples_are_skipped_and_notes_cut_at_the_length() {
        let mut toks = vec![MAX_TIME + 5, DUR_OFFSET, NOTE_OFFSET]; // time out of range
        toks.extend([0, 5, NOTE_OFFSET + 60]); // duration in the wrong place
        toks.extend(triple(2.0, 5.0, 0, 60));
        toks.extend(triple(9.0, 1.0, 0, 62)); // starts after the end
        toks.push(3); // half a note
        let parts = decode(&toks, 4.0);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].midi.notes.len(), 1);
        assert!((parts[0].midi.notes[0].end - 4.0).abs() < 1e-9);
        assert!(decode(&[], 10.0).is_empty());
    }

    #[test]
    fn sampling_stays_in_range_and_follows_the_seed() {
        let mut logits = vec![0.0f32; MIDI_TOKENS as usize];
        logits[12_345] = 50.0; // a note far ahead of everything
        logits[7] = 40.0; // a time
        let mut rng = Rng::new(1);
        assert_eq!(sample(&logits, allowed(2), 1.0, 0.98, &mut rng), Some(12_345));
        assert_eq!(sample(&logits, allowed(0), 1.0, 0.98, &mut rng), Some(7));
        // Flat logits: same seed, same tokens; always within the range.
        let flat = vec![0.0f32; MIDI_TOKENS as usize];
        let draw = |seed| {
            let mut r = Rng::new(seed);
            (0..20).map(|i| sample(&flat, allowed(i), 1.0, 1.0, &mut r).unwrap()).collect::<Vec<_>>()
        };
        assert_eq!(draw(9), draw(9));
        assert_ne!(draw(9), draw(10));
        for (i, t) in draw(3).into_iter().enumerate() {
            assert!(allowed(i).contains(&t));
        }
        // Nothing usable.
        let nan = vec![f32::NAN; MIDI_TOKENS as usize];
        assert_eq!(sample(&nan, allowed(0), 1.0, 0.9, &mut rng), None);
        assert_eq!(sample(&[], allowed(0), 1.0, 0.9, &mut rng), None);
    }

    #[test]
    fn generation_stops_at_the_length_and_writes_whole_notes() {
        // A fake model: times count up by a second, then 0.5 s, then piano C4.
        let opts = TextMidiOptions { seconds: 3.0, ..TextMidiOptions::new("test") };
        let mut step = 0u32;
        let mut calls = Vec::new();
        let toks = generate_with(&[1, 2, 3], &opts, &Ctx::default(), |ids| {
            calls.push(ids.len());
            let mut l = vec![0.0f32; MIDI_TOKENS as usize];
            let want = match step % 3 {
                0 => (step / 3) * 100,
                1 => DUR_OFFSET + 50,
                _ => NOTE_OFFSET + 60,
            };
            l[want as usize] = 100.0;
            step += 1;
            Ok(l)
        })
        .unwrap();
        assert_eq!(toks.len(), 9, "notes at 0, 1 and 2 s; the 3 s onset ends it");
        assert_eq!(calls[0], 3);
        assert!(calls[1..].iter().all(|&n| n == 1));
        let opts = TextMidiOptions { max_tokens: 5, ..opts };
        step = 0;
        let toks = generate_with(&[1], &opts, &Ctx::default(), |_| {
            let mut l = vec![0.0f32; MIDI_TOKENS as usize];
            let want = match step % 3 {
                0 => 0,
                1 => DUR_OFFSET + 50,
                _ => NOTE_OFFSET + 60,
            };
            l[want as usize] = 100.0;
            step += 1;
            Ok(l)
        })
        .unwrap();
        assert_eq!(toks.len(), 3);
    }

    #[test]
    fn prime_notes_become_sorted_whole_tokens() {
        let p = Prime::from_notes(&[
            (1.0, 1.5, 0, 64),
            (0.0, 0.25, 0, 60),
            (0.0, 20.0, DRUMS, 36),
            (f64::NAN, 1.0, 0, 1),
            (150.0, 151.0, 0, 60),
            (0.5, 0.5, 200, 60),
        ]);
        assert_eq!(p.notes(), 3);
        assert_eq!(
            p.tokens(),
            &[0, DUR_OFFSET + 999, NOTE_OFFSET + 128 * 128 + 36, 0, DUR_OFFSET + 25, NOTE_OFFSET + 60, 100, DUR_OFFSET + 50, NOTE_OFFSET + 64]
        );
        // They decode back to the notes (durations cut at 10 s).
        let parts = decode(p.tokens(), 100.0);
        assert_eq!(parts.iter().map(|x| (x.instrument, x.midi.notes.len())).collect::<Vec<_>>(), vec![(0, 2), (DRUMS, 1)]);
        let many: Vec<(f64, f64, u8, u8)> = (0..500).map(|i| (f64::from(i) * 0.1, f64::from(i) * 0.1 + 0.1, 0, 60)).collect();
        assert_eq!(Prime::from_notes(&many).notes(), MAX_PRIME_NOTES);
        assert!(Prime::default().is_empty() && Prime::default().tokens().is_empty());
    }

    #[test]
    fn options_are_checked() {
        assert!(TextMidiOptions::new("jazz").validated().is_ok());
        assert!(TextMidiOptions::new("   ").validated().is_err());
        for bad in [
            TextMidiOptions { seconds: 0.0, ..TextMidiOptions::new("x") },
            TextMidiOptions { seconds: f32::NAN, ..TextMidiOptions::new("x") },
            TextMidiOptions { temperature: 5.0, ..TextMidiOptions::new("x") },
            TextMidiOptions { top_p: 0.0, ..TextMidiOptions::new("x") },
            TextMidiOptions { max_tokens: 99_999, ..TextMidiOptions::new("x") },
        ] {
            assert!(bad.validated().is_err(), "{bad:?}");
        }
        assert_eq!(prompt_text(" a <|begin_of_text|> b "), format!("{SYSTEM_PROMPT}a < |begin_of_text|> b "));
    }
}
