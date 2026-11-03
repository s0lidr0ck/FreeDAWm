//! Lyrics transcription: Whisper large-v3 in whisper.cpp (through `whisper-rs`), with the export
//! lane's settings and host steps (`tools/ml-export/lyrics/reference_infer.py`, ported exactly).
//!
//! 1. Mono (mean), resampled to 16 kHz. No VAD (it drops sung vocals): whisper.cpp windows the
//!    whole input in 30 s pieces itself.
//! 2. `whisper_full`: beam search 5, temperature 0 with no fallback, `no_context`, no text context,
//!    DTW token timestamps with the large-v3 alignment heads, flash attention off (it disables DTW),
//!    English or language detection, an optional initial prompt.
//! 3. Words from each segment's tokens: special tokens (id ≥ 50257) dropped; a token starting with
//!    a space starts a word, other tokens join the current one. A word starts at its first token's
//!    DTW time and ends where the next word of the segment starts (the segment's end for the last
//!    one; never before start + 0.05 s); its probability is its tokens' lowest. Words that are only
//!    music symbols or punctuation go.
//! 4. Gate: a word stays only while the input is within 30 dB of its loudest (the 99th percentile
//!    of 50 ms RMS) somewhere over the word: Whisper also "hears" words in silence.
//! 5. Segments whose text holds a known caption hallucination ("transcript", "subtitle", "©"…) go
//!    whole.

use crate::job::Ctx;
use crate::models::InstalledModel;
use crate::{MlError, Result};
use serde::{Deserialize, Serialize};

/// The model folder id.
pub const ID: &str = "whisper_lyrics";
/// The rate Whisper listens at.
pub const SR: u32 = 16_000;
/// The gate: words quieter than this below the input's loud parts go (dB).
pub const GATE_DB: f64 = 30.0;
/// Token ids from here up are special (`[_BEG_]`, `[_TT_n]`, language and task tokens).
pub const FIRST_SPECIAL: i32 = 50_257;
/// Caption hallucinations Whisper produces on music or silence (lower case).
pub const HALLUCINATIONS: [&str; 9] = [
    "transcript",
    "subtitle",
    "thank you for watching",
    "thanks for watching",
    "\u{a9}",
    "amara.org",
    "www.",
    "please subscribe",
    "like and subscribe",
];
/// Longest initial prompt kept, bytes (Whisper reads at most half its 448-token context).
pub const MAX_PROMPT_BYTES: usize = 400;

/// The language to transcribe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LyricsLanguage {
    /// Whisper detects it.
    Auto,
    /// English (the export lane's setting and the golden run's).
    #[default]
    English,
}

impl LyricsLanguage {
    pub fn parse(s: &str) -> Option<LyricsLanguage> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "detect" => Some(LyricsLanguage::Auto),
            "en" | "english" | "" => Some(LyricsLanguage::English),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            LyricsLanguage::Auto => "auto",
            LyricsLanguage::English => "en",
        }
    }
}

/// An initial prompt (words, names or spellings to expect), held inline so job options stay `Copy`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Prompt {
    len: u16,
    bytes: [u8; MAX_PROMPT_BYTES],
}

impl Default for Prompt {
    fn default() -> Self {
        Prompt { len: 0, bytes: [0; MAX_PROMPT_BYTES] }
    }
}

impl std::fmt::Debug for Prompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

impl Prompt {
    /// `text` trimmed, without control characters, cut to [`MAX_PROMPT_BYTES`] at a character
    /// boundary.
    pub fn new(text: &str) -> Prompt {
        let clean: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
        let clean = clean.trim();
        let mut end = clean.len().min(MAX_PROMPT_BYTES);
        while end > 0 && !clean.is_char_boundary(end) {
            end -= 1;
        }
        let mut p = Prompt::default();
        if let (Some(dst), Some(src)) = (p.bytes.get_mut(..end), clean.as_bytes().get(..end)) {
            dst.copy_from_slice(src);
            p.len = u16::try_from(end).unwrap_or(0);
        }
        p
    }

    pub fn as_str(&self) -> &str {
        self.bytes.get(..usize::from(self.len)).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or("")
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Which weights: the full f16 model, or a quantized copy (card `variants`) that runs faster on
/// the CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LyricsQuality {
    /// The card's `variants.fast` (q5_0 large-v3); the f16 model when the card has none.
    #[default]
    Fast,
    /// The full f16 large-v3 (the export lane's golden runs).
    Best,
}

impl LyricsQuality {
    pub fn parse(s: &str) -> Option<LyricsQuality> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fast" | "" => Some(LyricsQuality::Fast),
            "best" | "f16" | "full" => Some(LyricsQuality::Best),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            LyricsQuality::Fast => "fast",
            LyricsQuality::Best => "best",
        }
    }
}

/// How to transcribe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LyricsOptions {
    pub language: LyricsLanguage,
    pub prompt: Prompt,
    pub quality: LyricsQuality,
    /// Leave long silent stretches out before decoding (times are mapped back). Off by default:
    /// on the 4-minute test vocal it made the run slower (125 s vs 107 s, f16), not faster.
    pub skip_silence: bool,
}

impl Default for LyricsOptions {
    fn default() -> Self {
        LyricsOptions { language: LyricsLanguage::English, prompt: Prompt::default(), quality: LyricsQuality::Fast, skip_silence: false }
    }
}

/// Silence shorter than this stays in (s).
pub const SKIP_MIN_SILENCE: f64 = 2.0;
/// Audio kept on each side of a skipped stretch (s).
pub const SKIP_PAD: f64 = 0.5;
/// A 50 ms frame is silent this far under the input's loud parts (dB; well under the word gate,
/// so soft singing is never cut).
pub const SKIP_DB: f64 = 50.0;

/// The input with its long silent stretches left out: the kept audio, and the pieces as
/// `(start in the input, start in the kept audio, length)` in samples.
pub fn skip_silence(audio: &[f32]) -> (Vec<f32>, Vec<(usize, usize, usize)>) {
    let hop = (SR / 20) as usize;
    let n = audio.len() / hop;
    let whole = (audio.to_vec(), vec![(0, 0, audio.len())]);
    if n == 0 {
        return whole;
    }
    let rms: Vec<f64> =
        audio.chunks_exact(hop).take(n).map(|c| (c.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / hop as f64).sqrt() + 1e-9).collect();
    let floor = percentile(&rms, 99.0) * 10f64.powf(-SKIP_DB / 20.0);
    let (min_frames, pad) = ((SKIP_MIN_SILENCE * 20.0) as usize, (SKIP_PAD * f64::from(SR)) as usize);
    // Silent runs of whole frames, long enough to skip.
    let mut cuts: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < n {
        if rms[i] < floor {
            let s = i;
            while i < n && rms[i] < floor {
                i += 1;
            }
            if i - s >= min_frames {
                // Keep `pad` of the silence on each side (none before the start or after the end).
                let a = if s == 0 { 0 } else { s * hop + pad };
                let b = if i == n { audio.len() } else { (i * hop).saturating_sub(pad) };
                if b > a {
                    cuts.push((a, b));
                }
            }
        } else {
            i += 1;
        }
    }
    if cuts.is_empty() {
        return whole;
    }
    let (mut out, mut map, mut at) = (Vec::with_capacity(audio.len()), Vec::new(), 0usize);
    for (a, b) in cuts.iter().copied().chain(std::iter::once((audio.len(), audio.len()))) {
        if a > at {
            map.push((at, out.len(), a - at));
            out.extend_from_slice(audio.get(at..a).unwrap_or_default());
        }
        at = b;
    }
    if out.is_empty() {
        return whole;
    }
    (out, map)
}

/// A start time in the kept audio (s) as a time in the input: a time on a join belongs to the
/// later piece.
pub fn unskip(t: f64, map: &[(usize, usize, usize)]) -> f64 {
    let sr = f64::from(SR);
    let at = t * sr;
    let piece = map.iter().rev().find(|(_, d, _)| (*d as f64) <= at).or(map.first());
    piece.map_or(t, |(s, d, _)| (*s as f64 + (at - *d as f64)) / sr)
}

/// An end time in the kept audio (s) as a time in the input: a time on a join belongs to the
/// earlier piece, so nothing ends across a skipped stretch.
pub fn unskip_end(t: f64, map: &[(usize, usize, usize)]) -> f64 {
    let sr = f64::from(SR);
    let at = t * sr;
    let piece = map.iter().rev().find(|(_, d, _)| (*d as f64) < at).or(map.first());
    piece.map_or(t, |(s, d, _)| (*s as f64 + (at - *d as f64)) / sr)
}

/// A transcribed word: seconds from the start of the input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub prob: f64,
}

/// A line (one whisper.cpp segment, re-joined from its kept words).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// What a transcription made.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Lyrics {
    pub words: Vec<Word>,
    pub segments: Vec<Line>,
    /// The language whisper.cpp used (`en`, or the detected one).
    #[serde(default)]
    pub language: String,
}

/// A whisper.cpp token as the decoder sees it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RawToken {
    pub id: i32,
    /// The token's bytes (may split a UTF-8 character).
    pub bytes: Vec<u8>,
    pub p: f32,
    /// DTW time, centiseconds (-1 for special tokens).
    pub t_dtw: i64,
}

/// A whisper.cpp segment: its span (seconds), text and tokens.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RawSegment {
    pub from: f64,
    pub to: f64,
    pub text: String,
    pub tokens: Vec<RawToken>,
}

/// Only music symbols, spaces and punctuation.
fn junk(text: &str) -> bool {
    text.chars().all(|c| c.is_whitespace() || "\u{266a}*.,!?;:'\"()[]-".contains(c))
}

/// A segment the hallucination list rejects.
fn hallucination(text: &str, phrases: &[String]) -> bool {
    let t = text.to_lowercase();
    phrases.iter().any(|h| t.contains(h.as_str()))
}

/// Step 3 and 5: words of the kept segments.
pub fn words_from(raw: &[RawSegment], phrases: &[String]) -> Vec<Word> {
    let mut words = Vec::new();
    for seg in raw {
        if hallucination(&seg.text, phrases) {
            continue;
        }
        let mut cur: Vec<Vec<&RawToken>> = Vec::new();
        for tok in &seg.tokens {
            if tok.id >= FIRST_SPECIAL {
                continue;
            }
            match cur.last_mut() {
                Some(w) if !tok.bytes.starts_with(b" ") => w.push(tok),
                _ => cur.push(vec![tok]),
            }
        }
        for (i, w) in cur.iter().enumerate() {
            let t0 = w.first().map_or(0.0, |t| t.t_dtw as f64 / 100.0);
            let t1 = cur.get(i + 1).and_then(|n| n.first()).map_or(seg.to, |t| t.t_dtw as f64 / 100.0);
            let bytes: Vec<u8> = w.iter().flat_map(|t| t.bytes.iter().copied()).collect();
            let text = String::from_utf8_lossy(&bytes).trim().to_string();
            if junk(&text) {
                continue;
            }
            let prob = w.iter().map(|t| f64::from(t.p)).fold(f64::INFINITY, f64::min);
            words.push(Word { start: t0, end: t1.max(t0 + 0.05), text, prob: if prob.is_finite() { prob } else { 0.0 } });
        }
    }
    words
}

/// numpy's linear-interpolation percentile `q` (0..=100) of `v`.
fn percentile(v: &[f64], q: f64) -> f64 {
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if s.is_empty() {
        return 0.0;
    }
    s.sort_by(f64::total_cmp);
    let pos = (q / 100.0) * (s.len() - 1) as f64;
    let (lo, frac) = (pos.floor() as usize, pos - pos.floor());
    let a = s.get(lo).copied().unwrap_or(0.0);
    let b = s.get(lo + 1).copied().unwrap_or(a);
    a + (b - a) * frac
}

/// Step 4: words where the singer is actually singing (`audio`: mono at 16 kHz).
pub fn gate(words: Vec<Word>, audio: &[f32]) -> Vec<Word> {
    let hop = (SR / 20) as usize;
    let n = audio.len() / hop;
    if n == 0 {
        return words;
    }
    let rms: Vec<f64> =
        audio.chunks_exact(hop).take(n).map(|c| (c.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>() / hop as f64).sqrt() + 1e-9).collect();
    let reference = percentile(&rms, 99.0);
    words
        .into_iter()
        .filter(|w| {
            let a = (w.start * 20.0).floor().max(0.0) as usize;
            let b = ((w.end * 20.0).ceil().max(0.0) as usize).max(a + 1).min(rms.len());
            match rms.get(a..b).filter(|s| !s.is_empty()) {
                Some(s) => 20.0 * (s.iter().copied().fold(0.0, f64::max) / reference).log10() >= -GATE_DB,
                None => false,
            }
        })
        .collect()
}

/// Lines: each raw segment's kept words (by start), re-joined.
pub fn segments_from(raw: &[RawSegment], words: &[Word]) -> Vec<Line> {
    raw.iter()
        .filter_map(|seg| {
            let ws: Vec<&Word> = words.iter().filter(|w| seg.from <= w.start && w.start < seg.to).collect();
            let (first, last) = (ws.first()?, ws.last()?);
            Some(Line { start: first.start, end: last.end, text: ws.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ") })
        })
        .collect()
}

/// Steps 3-5 on whisper.cpp's segments for `audio` (mono, 16 kHz).
pub fn decode(raw: &[RawSegment], audio: &[f32], phrases: &[String]) -> (Vec<Word>, Vec<Line>) {
    let words = gate(words_from(raw, phrases), audio);
    let segments = segments_from(raw, &words);
    (words, segments)
}

/// The hallucination phrases on the card (`params.hallucination_phrases`), else the built-in list.
pub fn phrases(model: &InstalledModel) -> Vec<String> {
    let from_card: Vec<String> = model
        .card
        .extra
        .get("params")
        .and_then(|p| p.get("hallucination_phrases"))
        .and_then(serde_json::Value::as_array)
        .map(|a| a.iter().filter_map(serde_json::Value::as_str).map(str::to_lowercase).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    if from_card.is_empty() { HALLUCINATIONS.iter().map(|s| s.to_string()).collect() } else { from_card }
}

/// Words and lines of `audio` (`[channel][frame]` at `rate`), seconds from its start, and where
/// it ran.
pub fn transcribe(model: &InstalledModel, audio: &[Vec<f32>], rate: u32, opts: &LyricsOptions, ctx: &Ctx) -> Result<(Lyrics, String)> {
    let len = crate::frames(audio);
    if len == 0 || rate == 0 {
        return Ok((Lyrics::default(), String::new()));
    }
    let mut mono = crate::to_channels(audio, 1).into_iter().next().unwrap_or_default();
    crate::sanitize(std::slice::from_mut(&mut mono));
    ctx.say("Resampling");
    let mono = if rate == SR { mono } else { soundcraft_dsp::offline::resample(&[mono], rate, SR).into_iter().next().unwrap_or_default() };
    ctx.check()?;
    let (raw, language, w) = if opts.skip_silence {
        let (kept, map) = skip_silence(&mono);
        let (raw, lang, w) = whisper::run(model, &kept, opts, &ctx.stage(0.0, 0.98))?;
        (unskip_segments(raw, &map), lang, w)
    } else {
        whisper::run(model, &mono, opts, &ctx.stage(0.0, 0.98))?
    };
    ctx.check()?;
    let (words, segments) = decode(&raw, &mono, &phrases(model));
    ctx.report(1.0);
    let mut note = format!("CPU (whisper.cpp, {})", w.label);
    if let Some(n) = w.note {
        note = format!("{note}; {n}");
    }
    Ok((Lyrics { words, segments, language }, note))
}

/// Segments of the kept audio with their times (spans and DTW times) back on the input's clock.
pub fn unskip_segments(mut raw: Vec<RawSegment>, map: &[(usize, usize, usize)]) -> Vec<RawSegment> {
    if map.len() <= 1 && map.first().is_none_or(|m| m.0 == m.1) {
        return raw;
    }
    for s in &mut raw {
        s.from = unskip(s.from, map);
        s.to = unskip_end(s.to, map);
        for t in &mut s.tokens {
            if t.t_dtw >= 0 {
                t.t_dtw = (unskip(t.t_dtw as f64 / 100.0, map) * 100.0).round() as i64;
            }
        }
    }
    raw
}

/// The weights a run uses.
#[derive(Debug, Clone, PartialEq)]
pub struct Weights {
    /// Relative to the model folder.
    pub file: String,
    pub size: u64,
    /// `large-v3 q8_0`, `large-v3 f16`.
    pub label: String,
    /// Why the fast file was not used, when it was asked for and could not be.
    pub note: Option<String>,
}

/// The weights for `quality`: the card's `variants.fast` (checked against its sha256 on first
/// use), else, or when that file is missing or broken (with a note), the card's f16 model.
pub fn weights(model: &InstalledModel, quality: LyricsQuality) -> Weights {
    let full = |note: Option<String>| Weights { file: model.card.file.clone(), size: model.card.size_bytes, label: "large-v3 f16".into(), note };
    let v = model.card.extra.get("variants").and_then(|v| v.get("fast"));
    let (Some(v), LyricsQuality::Fast) = (v, quality) else { return full(None) };
    let file = v.get("file").and_then(serde_json::Value::as_str).unwrap_or_default();
    let sha = v.get("sha256").and_then(serde_json::Value::as_str).unwrap_or_default();
    match crate::models::check_file(model, file, sha) {
        Ok(()) => Weights {
            file: file.to_string(),
            size: v.get("size_bytes").and_then(serde_json::Value::as_u64).unwrap_or(model.card.size_bytes),
            label: format!("large-v3 {}", v.get("quant").and_then(serde_json::Value::as_str).unwrap_or("quantized")),
            note: None,
        },
        Err(e) => full(Some(format!("the fast lyrics model can't be used ({e}); used the full model"))),
    }
}

/// whisper.cpp through `whisper-rs`.
pub mod whisper {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// One transcription at a time: the model is 3 GB, and the abort flag below is shared.
    static RUN: Mutex<()> = Mutex::new(());
    /// Set while the running job is cancelled; read by whisper.cpp's abort callback (a closure
    /// without captures: `whisper-rs` 0.16 hands a captured one the wrong pointer).
    static ABORT: AtomicBool = AtomicBool::new(false);

    fn rt(e: impl std::fmt::Display) -> MlError {
        MlError::Runtime(format!("whisper.cpp: {e}"))
    }

    /// Threads for whisper.cpp: half the logical CPUs (the UI and audio keep the rest), at least 4.
    fn threads() -> i32 {
        if let Some(t) = crate::env_var_os("WHISPER_THREADS")
            .and_then(|v| v.into_string().ok())
            .and_then(|v| v.parse::<i32>().ok())
            .filter(|t| (1..=64).contains(t))
        {
            return t;
        }
        let n = std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get);
        i32::try_from((n / 2).clamp(4, 16)).unwrap_or(4)
    }

    /// Run whisper.cpp on mono 16 kHz audio: its segments and the language it used.
    pub fn run(model: &InstalledModel, audio: &[f32], opts: &LyricsOptions, ctx: &Ctx) -> Result<(Vec<RawSegment>, String, Weights)> {
        // whisper.cpp checks its abort flag only between encoder and decoder steps (the encoder of
        // one 30 s window runs several seconds on the CPU). So the work runs on its own thread and
        // a cancel returns at once; that thread stops at whisper.cpp's next check and releases the
        // model (the next job waits for it on `RUN`).
        let (tx, rx) = std::sync::mpsc::channel();
        let (model, audio, opts, wctx) = (model.clone(), audio.to_vec(), *opts, ctx.clone());
        std::thread::Builder::new()
            .name("whisper".into())
            .spawn(move || {
                let _ = tx.send(run_here(&model, &audio, &opts, &wctx));
            })
            .map_err(|e| MlError::Runtime(format!("could not start whisper.cpp: {e}")))?;
        loop {
            match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(r) => return r,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if ctx.cancel_flag().is_cancelled() {
                        ABORT.store(true, Ordering::Relaxed);
                        return Err(MlError::Cancelled);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err(MlError::Runtime("whisper.cpp stopped unexpectedly".into())),
            }
        }
    }

    /// [`run`] on the calling thread.
    fn run_here(model: &InstalledModel, audio: &[f32], opts: &LyricsOptions, ctx: &Ctx) -> Result<(Vec<RawSegment>, String, Weights)> {
        use whisper_rs::{DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};
        let _one = RUN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        ABORT.store(false, Ordering::Relaxed);
        whisper_rs::install_logging_hooks();
        ctx.check()?;
        // The model takes about 3.5 GB once loaded: refuse up front rather than abort later.
        let w = super::weights(model, opts.quality);
        let need = usize::try_from(w.size.saturating_add(1 << 30)).unwrap_or(usize::MAX);
        let mut probe: Vec<u8> = Vec::new();
        if probe.try_reserve_exact(need).is_err() {
            return Err(MlError::BadInput(format!("not enough free memory to load Whisper (about {:.1} GB)", need as f64 / 1e9)));
        }
        drop(probe);
        ctx.say("Loading Whisper large-v3");
        let path = model.dir.join(&w.file);
        let path_s = path.to_str().ok_or_else(|| MlError::Model(ID.into(), format!("{} is not a UTF-8 path", path.display())))?;
        let mut cp = WhisperContextParameters::default();
        cp.use_gpu(false)
            .flash_attn(false)
            .dtw_parameters(DtwParameters { mode: DtwMode::ModelPreset { model_preset: DtwModelPreset::LargeV3 }, ..DtwParameters::default() });
        let wctx =
            WhisperContext::new_with_params(path_s, cp).map_err(|e| MlError::Model(ID.into(), format!("can't load {}: {e}", path.display())))?;
        let mut state = wctx.create_state().map_err(rt)?;
        ctx.check()?;
        ctx.say("Transcribing (Whisper large-v3, CPU)");

        let mut p = FullParams::new(SamplingStrategy::BeamSearch { beam_size: 5, patience: -1.0 });
        p.set_n_threads(threads());
        p.set_language(Some(opts.language.id()));
        p.set_translate(false);
        p.set_no_context(true);
        p.set_n_max_text_ctx(0);
        p.set_temperature(0.0);
        p.set_temperature_inc(0.0);
        p.set_token_timestamps(true);
        p.set_thold_pt(0.01);
        p.set_entropy_thold(2.4);
        p.set_logprob_thold(-1.0);
        p.set_no_speech_thold(0.6);
        p.set_suppress_nst(false);
        p.set_single_segment(false);
        p.set_print_progress(false);
        p.set_print_realtime(false);
        p.set_print_special(false);
        p.set_print_timestamps(false);
        if !opts.prompt.is_empty() {
            // `Prompt` holds no NUL (control characters are replaced), so this cannot panic.
            p.set_initial_prompt(opts.prompt.as_str());
        }
        let pctx = ctx.clone();
        p.set_progress_callback_safe(move |pct: i32| pctx.report(pct as f32 / 100.0));
        p.set_abort_callback_safe(|| ABORT.load(Ordering::Relaxed));

        // A watcher raises the abort flag when the job is cancelled.
        let done = Arc::new(AtomicBool::new(false));
        let (d, cancel) = (Arc::clone(&done), ctx.cancel_flag().clone());
        let watcher = std::thread::Builder::new().name("whisper-cancel".into()).spawn(move || {
            while !d.load(Ordering::Relaxed) {
                if cancel.is_cancelled() {
                    ABORT.store(true, Ordering::Relaxed);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
        let r = state.full(p, audio);
        done.store(true, Ordering::Relaxed);
        if let Ok(w) = watcher {
            let _ = w.join();
        }
        if ctx.cancel_flag().is_cancelled() || ABORT.load(Ordering::Relaxed) {
            ABORT.store(false, Ordering::Relaxed);
            return Err(MlError::Cancelled);
        }
        r.map_err(rt)?;

        let lang_id = state.full_lang_id_from_state();
        let language = whisper_rs::get_lang_str(lang_id).unwrap_or(opts.language.id()).to_string();
        let mut out = Vec::new();
        for seg in state.as_iter() {
            let mut tokens = Vec::new();
            for i in 0..seg.n_tokens() {
                let Some(t) = seg.get_token(i) else { continue };
                let d = t.token_data();
                tokens.push(RawToken { id: d.id, bytes: t.to_bytes().map(<[u8]>::to_vec).unwrap_or_default(), p: d.p, t_dtw: d.t_dtw });
            }
            out.push(RawSegment {
                from: seg.start_timestamp() as f64 / 100.0,
                to: seg.end_timestamp() as f64 / 100.0,
                text: seg.to_str_lossy().map(|s| s.into_owned()).unwrap_or_default(),
                tokens,
            });
        }
        Ok((out, language, w))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(id: i32, text: &str, p: f32, t_dtw: i64) -> RawToken {
        RawToken { id, bytes: text.as_bytes().to_vec(), p, t_dtw }
    }

    #[test]
    fn prompt_is_clean_and_bounded() {
        assert_eq!(Prompt::new("  Yahweh,\0 Elohim\n").as_str(), "Yahweh,  Elohim");
        let long = "é".repeat(400);
        let p = Prompt::new(&long);
        assert!(p.as_str().len() <= MAX_PROMPT_BYTES && p.as_str().chars().all(|c| c == 'é'));
        assert!(Prompt::default().is_empty());
        assert_eq!(LyricsLanguage::parse("English"), Some(LyricsLanguage::English));
        assert_eq!(LyricsLanguage::parse("auto"), Some(LyricsLanguage::Auto));
        assert_eq!(LyricsLanguage::parse("fr"), None);
    }

    #[test]
    fn words_join_pieces_drop_symbols_and_hallucinations() {
        let phrases: Vec<String> = HALLUCINATIONS.iter().map(|s| s.to_string()).collect();
        let raw = vec![
            RawSegment {
                from: 0.0,
                to: 5.0,
                text: " \u{266a} Show me your face".into(),
                tokens: vec![
                    tok(50365, "[_BEG_]", 0.9, -1),
                    tok(931, " \u{266a}", 0.5, 0),
                    tok(1, " Show", 0.8, 10),
                    tok(2, " me", 0.9, 60),
                    tok(3, " your", 0.7, 100),
                    tok(4, " fa", 0.6, 150),
                    tok(5, "ce", 0.95, 170),
                    tok(6, ",", 0.99, 190),
                ],
            },
            RawSegment { from: 5.0, to: 9.0, text: " \u{a9} transcript Emily".into(), tokens: vec![tok(7, " Emily", 0.2, 600)] },
        ];
        let w = words_from(&raw, &phrases);
        let texts: Vec<&str> = w.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, ["Show", "me", "your", "face,"]);
        assert_eq!((w[0].start, w[0].end), (0.1, 0.6));
        assert_eq!((w[3].start, w[3].end, w[3].prob), (1.5, 5.0, f64::from(0.6f32)));
        let lines = segments_from(&raw, &w);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Show me your face,");
    }

    #[test]
    fn silence_is_skipped_and_times_map_back() {
        let sr = SR as usize;
        // 1 s sound, 4 s silence, 1 s sound.
        let mut a: Vec<f32> = (0..sr).map(|i| 0.3 * (i as f32 * 0.05).sin()).collect();
        a.extend(std::iter::repeat_n(0.0, 4 * sr));
        a.extend((0..sr).map(|i| 0.3 * (i as f32 * 0.05).sin()));
        let (kept, map) = skip_silence(&a);
        // 0.5 s of silence kept on each side: 1 + 0.5 + 0.5 + 1 s.
        assert_eq!(kept.len(), 3 * sr);
        assert_eq!(map, vec![(0, 0, 3 * sr / 2), (9 * sr / 2, 3 * sr / 2, 3 * sr / 2)]);
        assert!((unskip(2.0, &map) - 5.0).abs() < 1e-9);
        // On the join: a start goes to the later piece, an end to the earlier one.
        assert!((unskip(1.5, &map) - 4.5).abs() < 1e-9);
        assert!((unskip_end(1.5, &map) - 1.5).abs() < 1e-9);
        // Nothing to skip: the input as it is.
        let (k, m) = skip_silence(&a[..sr]);
        assert_eq!((k.len(), m), (sr, vec![(0, 0, sr)]));
    }

    #[test]
    fn the_gate_drops_words_in_silence() {
        // 1 s loud, 1 s near-silent.
        let mut audio: Vec<f32> = (0..16_000).map(|i| 0.5 * (i as f32 * 0.07).sin()).collect();
        audio.extend((0..16_000).map(|i| 1e-4 * (i as f32 * 0.07).sin()));
        let w = |s: f64, e: f64| Word { start: s, end: e, text: "x".into(), prob: 1.0 };
        let kept = gate(vec![w(0.2, 0.5), w(1.3, 1.6), w(0.9, 1.4)], &audio);
        assert_eq!(kept.iter().map(|w| w.start).collect::<Vec<_>>(), vec![0.2, 0.9]);
        assert!((percentile(&[1.0, 2.0, 3.0, 4.0], 50.0) - 2.5).abs() < 1e-12);
    }
}
