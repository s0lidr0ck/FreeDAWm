//! The DAW's jobs, at any sample rate: Separate Stems, Make Karaoke, Split Drums, Clean Vocal,
//! Convert to MIDI.
//!
//! Audio comes in at the session rate, is resampled to each model's rate with the offline
//! resampler and back, and every result is exactly as long as the input and sample-aligned with
//! it. Mono input runs as dual mono through stereo models; results have the input's width (one or
//! two channels). A panic inside third-party inference code becomes an error here.

use crate::chunk::Quality;
use crate::job::Ctx;
use crate::models::{self, InstalledModel};
use crate::runtime::Device;
use crate::{MlError, Result};
use std::path::PathBuf;

/// Separate Stems' model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SepModel {
    /// HTDemucs v4: vocals, drums, bass, other (honours Quality).
    #[default]
    Demucs,
    /// BS-RoFormer SW: bass, drums, other, vocals, guitar, piano.
    Sw6,
    /// Each stem from the model that measured best for it ([`BEST_ROUTING`]): six stems.
    Best,
}

/// Where one of best mode's stems comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StemSource {
    /// Mel-Band RoFormer vocals ([`crate::vocals`]).
    MelBand,
    /// BS-RoFormer SW ([`crate::bs_roformer`]).
    Sw6,
    /// HTDemucs at the job's quality ([`crate::htdemucs`]).
    Demucs,
}

impl StemSource {
    /// The model folder it needs.
    pub fn model_id(self) -> &'static str {
        match self {
            StemSource::MelBand => crate::vocals::ID,
            StemSource::Sw6 => crate::bs_roformer::ID,
            StemSource::Demucs => crate::htdemucs::ID,
        }
    }

    /// Its name in notes.
    pub fn label(self) -> &'static str {
        match self {
            StemSource::MelBand => "Mel-Band RoFormer",
            StemSource::Sw6 => "BS-RoFormer SW",
            StemSource::Demucs => "HTDemucs",
        }
    }

    /// Its share of best mode's progress bar (its time per second of audio, measured on DirectML).
    fn weight(self) -> f32 {
        match self {
            StemSource::MelBand => 0.40,
            StemSource::Sw6 => 0.59,
            StemSource::Demucs => 0.18,
        }
    }
}

/// Separate Stems' best mode: each stem (in output order) and the model it comes from.
///
/// Measured 2026-10-30 (spec `2026-10-29-roformer-separation.md`, "Measured"): two 60 s excerpts
/// of a full band mix, Demucs High vs BS-RoFormer SW vs Mel-Band vocals, no ground truth, so
/// proxies (lower is cleaner):
/// - vocals: |corr| with the instrumental: Mel-Band 0.062 / 0.062, SW 0.082 / 0.078, Demucs High
///   0.097 / 0.090; own-stem bleed R² Mel-Band 0.38 / 0.39 %, SW 0.54 / 1.84 %, Demucs 1.33 / 2.37 %;
/// - drums: own-stem bleed R² SW 0.03 / 0.12 %, Demucs High 0.02 / 0.19 %; vocal bleed 0.00 / 0.04 %
///   vs 0.00 / 0.06 % (a tie: Demucs is not better, so SW);
/// - bass: own-stem bleed R² SW 0.29 / 0.30 %, Demucs High 0.57 / 0.87 %;
/// - other: vocal bleed R² SW 0.20 / 0.61 %, Demucs High 1.30 / 2.17 % (Demucs' other also holds
///   guitar and piano);
/// - guitar, piano: SW only.
///
/// Time per 60 s on DirectML: Mel-Band 24 s, SW 35 s, Demucs High 9-13 s.
pub const BEST_ROUTING: [(&str, StemSource); 6] = [
    ("vocals", StemSource::MelBand),
    ("drums", StemSource::Sw6),
    ("bass", StemSource::Sw6),
    ("guitar", StemSource::Sw6),
    ("piano", StemSource::Sw6),
    ("other", StemSource::Sw6),
];

/// Best mode's stems, in output order.
pub const BEST_STEMS: [&str; 6] = ["vocals", "drums", "bass", "guitar", "piano", "other"];

/// The models best mode runs, each once, in routing order.
fn best_sources() -> Vec<StemSource> {
    let mut v: Vec<StemSource> = Vec::new();
    for (_, s) in BEST_ROUTING {
        if !v.contains(&s) {
            v.push(s);
        }
    }
    v
}

impl SepModel {
    /// The command parameter value: `demucs` | `sw6` | `best`.
    pub fn id(self) -> &'static str {
        match self {
            SepModel::Demucs => "demucs",
            SepModel::Sw6 => "sw6",
            SepModel::Best => "best",
        }
    }

    pub fn parse(s: &str) -> Option<SepModel> {
        match s.trim().to_ascii_lowercase().as_str() {
            "demucs" | "htdemucs" => Some(SepModel::Demucs),
            "sw6" | "sw" | "bs_roformer_sw" => Some(SepModel::Sw6),
            "best" => Some(SepModel::Best),
            _ => None,
        }
    }

    /// The model folders it needs.
    pub fn model_ids(self) -> Vec<&'static str> {
        match self {
            SepModel::Demucs => vec![crate::htdemucs::ID],
            SepModel::Sw6 => vec![crate::bs_roformer::ID],
            SepModel::Best => best_sources().into_iter().map(StemSource::model_id).collect(),
        }
    }

    /// Its stems, lower case, in output order.
    pub fn stems(self) -> &'static [&'static str] {
        match self {
            SepModel::Demucs => &crate::htdemucs::DEFAULT_STEMS,
            SepModel::Sw6 => &crate::bs_roformer::DEFAULT_STEMS,
            SepModel::Best => &BEST_STEMS,
        }
    }

    /// The best separation the installed models allow (`installed` says whether a model folder
    /// is installed): Best when both RoFormer models are, else BS-RoFormer SW when it is, else
    /// HTDemucs. Separate Stems opens on it, and Scout's Capture per Stem separates with it.
    pub fn best_installed(installed: impl Fn(&str) -> bool) -> SepModel {
        if SepModel::Best.model_ids().into_iter().all(&installed) {
            SepModel::Best
        } else if SepModel::Sw6.model_ids().into_iter().all(&installed) {
            SepModel::Sw6
        } else {
            SepModel::Demucs
        }
    }

    /// Whether Quality (HTDemucs' shifted passes) applies.
    pub fn uses_quality(self) -> bool {
        match self {
            SepModel::Demucs => true,
            SepModel::Sw6 => false,
            SepModel::Best => BEST_ROUTING.iter().any(|(_, s)| *s == StemSource::Demucs),
        }
    }
}

/// What a job does.
// Transcribe Lyrics' prompt is held inline (400 bytes) so the kind stays `Copy`; a job is built
// once per run, so the size is of no consequence.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JobKind {
    /// Stems as new tracks: HTDemucs' four, BS-RoFormer SW's six, or the best of each ([`SepModel`]).
    Separate { model: SepModel },
    /// Lead vocal and instrumental; backing vocals go into the instrumental when `keep_backing`,
    /// else onto their own track.
    Karaoke { keep_backing: bool },
    /// Kick, snare, toms, hi-hat, ride, crash. `from_mix` separates the drums out first.
    Drums { from_mix: bool },
    /// Clean Vocal ([`crate::clean`]): Vocal mode = Smule Renaissance alone; Speech mode =
    /// DeepFilterNet de-noise, then Renaissance de-reverb when asked.
    Clean(crate::clean::CleanOptions),
    /// Audio → MIDI notes ([`crate::midi`]); the result is [`JobOutput::midi`], no stems.
    Midi(crate::midi::MidiOptions),
    /// Detect Chords ([`crate::chords`], BTC): the result is [`JobOutput::chords`].
    Chords(crate::chords::ChordOptions),
    /// Transcribe Lyrics ([`crate::lyrics`], Whisper large-v3): the result is [`JobOutput::lyrics`].
    Lyrics(crate::lyrics::LyricsOptions),
    /// Generate MIDI from Text ([`crate::textmidi`], MIDI-LLM): no audio in, the result is
    /// [`JobOutput::generated`] (one part per instrument).
    TextMidi(crate::textmidi::TextMidiOptions),
}

impl JobKind {
    /// The command / CLI name.
    pub fn id(&self) -> &'static str {
        match self {
            JobKind::Separate { .. } => "separate",
            JobKind::Karaoke { .. } => "karaoke",
            JobKind::Drums { .. } => "drums",
            JobKind::Clean(_) => "clean",
            JobKind::Midi(_) => "midi",
            JobKind::Chords(_) => "chords",
            JobKind::Lyrics(_) => "lyrics",
            JobKind::TextMidi(_) => "text_midi",
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            JobKind::Separate { .. } => "Separate Stems",
            JobKind::Karaoke { .. } => "Make Karaoke",
            JobKind::Drums { .. } => "Split Drums",
            JobKind::Clean(_) => "Clean Vocal",
            JobKind::Midi(_) => "Convert to MIDI",
            JobKind::Chords(_) => "Detect Chords",
            JobKind::Lyrics(_) => "Transcribe Lyrics",
            JobKind::TextMidi(_) => "Generate MIDI from Text",
        }
    }

    /// Models the job needs from the model folder (DeepFilterNet is built in). Karaoke works
    /// without its own model (all vocals stay together, with a note).
    pub fn required_models(&self) -> Vec<&'static str> {
        match *self {
            JobKind::Separate { model } => model.model_ids(),
            // The vocals come from Mel-Band RoFormer vocals, else HTDemucs: [`vocal_source`].
            JobKind::Karaoke { .. } => Vec::new(),
            JobKind::Drums { from_mix } => {
                if from_mix {
                    vec![crate::htdemucs::ID, crate::drumsep::ID]
                } else {
                    vec![crate::drumsep::ID]
                }
            }
            JobKind::Clean(o) => {
                if o.runs_srs() {
                    vec![crate::srs::ID]
                } else {
                    Vec::new()
                }
            }
            // The isolation's model is the first installed of [`isolation_sources`].
            JobKind::Midi(o) => vec![o.model.model_id()],
            JobKind::Chords(_) => vec![crate::chords::ID],
            JobKind::Lyrics(_) => vec![crate::lyrics::ID],
            JobKind::TextMidi(_) => vec![crate::textmidi::ID],
        }
    }
}

/// A job: the source audio (`[channel][frame]` at `rate`) and where to run it.
#[derive(Debug, Clone)]
pub struct Job {
    pub kind: JobKind,
    pub audio: Vec<Vec<f32>>,
    pub rate: u32,
    pub device: Device,
    pub models_dir: PathBuf,
    /// How carefully the HTDemucs step runs (Separate, Karaoke, Drums from a mix, Convert to
    /// MIDI's isolation): Fast is one pass, High averages shifted passes.
    pub quality: Quality,
}

/// One result: a display name ("Vocals", "Kick", "Instrumental"…) and its audio at the job's rate.
#[derive(Debug, Clone, PartialEq)]
pub struct Stem {
    pub name: String,
    pub audio: Vec<Vec<f32>>,
}

/// What a job made.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct JobOutput {
    pub stems: Vec<Stem>,
    /// Where it ran ("GPU (DirectML)", "CPU", or a fallback note) and anything else worth telling.
    pub notes: Vec<String>,
    /// Convert to MIDI: the notes (seconds from the start of the job's audio).
    pub midi: Option<crate::midi::Transcription>,
    /// Detect Chords: the segments (seconds from the start of the job's audio).
    pub chords: Option<Vec<crate::chords::ChordSegment>>,
    /// Transcribe Lyrics: words and lines (seconds from the start of the job's audio).
    pub lyrics: Option<crate::lyrics::Lyrics>,
    /// Generate MIDI from Text: one part per General MIDI instrument (seconds from the start).
    pub generated: Option<Vec<crate::textmidi::Part>>,
}

/// A stem name as a track-name word: "vocals" → "Vocals", "hihat" → "Hi-Hat".
pub fn pretty(name: &str) -> String {
    let n = name.trim().to_ascii_lowercase().replace(['_', '-'], " ");
    match n.as_str() {
        "hh" | "hihat" | "hi hat" | "hi hats" | "hihats" => return "Hi-Hat".into(),
        "toms" | "tom" => return "Toms".into(),
        "cymbals" | "cym" => return "Cymbals".into(),
        _ => {}
    }
    n.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Run `job`, catching panics from inference code.
pub fn run(job: &Job, ctx: &Ctx) -> Result<JobOutput> {
    guarded(|| run_inner(job, ctx))
}

/// Convert to MIDI's expensive half on its own: isolation (when asked) and the network, as
/// [`crate::midi::Analysed`] (decode it as often as wanted) plus the device and stem notes.
/// Catches panics from inference code.
pub fn analyse_midi(job: &Job, ctx: &Ctx) -> Result<(crate::midi::Analysed, Vec<String>)> {
    guarded(|| {
        let JobKind::Midi(opts) = job.kind else { return Err(MlError::BadInput("not a Convert to MIDI job".into())) };
        let job = checked(job)?;
        let pick = isolation_source(&job, opts.isolate)?;
        analyse_midi_with(&job, pick, ctx, best_source, |audio, rate, ctx| {
            let model = load(&job, opts.model.model_id())?;
            crate::midi::analyse(&model, audio, rate, &opts, job.device, ctx)
        })
    })
}

/// [`analyse_midi`] with its two expensive steps supplied: `separate` (the model `pick` names, see
/// [`isolation_source`]) and `network` (the model over `(audio, rate)`), so they can be stood in for.
pub fn analyse_midi_with(
    job: &Job,
    pick: Option<(StemSource, &'static str)>,
    ctx: &Ctx,
    separate: impl FnOnce(StemSource, &Job, &Ctx) -> Result<Separated>,
    network: impl FnOnce(&[Vec<f32>], u32, &Ctx) -> Result<(crate::midi::Analysed, String)>,
) -> Result<(crate::midi::Analysed, Vec<String>)> {
    let mut notes = Vec::new();
    let split = if pick.is_some() { 0.6 } else { 0.0 };
    let (audio, rate) = isolated(job, pick, &ctx.stage(0.0, split), &mut notes, separate)?;
    ctx.check()?;
    let (a, note) = network(&audio, rate, &ctx.stage(split, 1.0))?;
    drop(audio);
    ctx.check()?;
    ctx.report(1.0);
    notes.push(note);
    notes.retain(|n| !n.is_empty());
    Ok((a, notes))
}

fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => {
            let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
            Err(MlError::Runtime(format!("the model stopped unexpectedly {msg}").trim().to_string()))
        }
    }
}

fn load(job: &Job, id: &str) -> Result<InstalledModel> {
    models::load(&job.models_dir, id)
}

/// `audio` at `to` Hz from `from` Hz.
fn at_rate(audio: &[Vec<f32>], from: u32, to: u32) -> Vec<Vec<f32>> {
    if from == to || from == 0 || to == 0 { audio.to_vec() } else { soundcraft_dsp::offline::resample(audio, from, to) }
}

/// A stem back at the job's rate and width, exactly `len` frames.
fn back(audio: &[Vec<f32>], model_rate: u32, job: &Job, len: usize, width: usize) -> Vec<Vec<f32>> {
    let mut a = crate::fit(at_rate(audio, model_rate, job.rate), len);
    crate::sanitize(&mut a);
    crate::to_channels(&a, width)
}

/// The finished stems (`name`, audio, its rate) back at the job's rate, one at a time so progress
/// keeps moving and Cancel is honoured to the end; each model-rate stem is freed once converted.
fn finish(parts: Vec<(String, Vec<Vec<f32>>, u32)>, job: &Job, len: usize, width: usize, ctx: &Ctx) -> Result<Vec<Stem>> {
    ctx.say("Finishing");
    let n = parts.len().max(1);
    let mut out = Vec::with_capacity(parts.len());
    for (i, (name, audio, rate)) in parts.into_iter().enumerate() {
        ctx.check()?;
        out.push(Stem { name, audio: back(&audio, rate, job, len, width) });
        drop(audio);
        ctx.report((i + 1) as f32 / n as f32);
    }
    ctx.check()?;
    Ok(out)
}

fn sum(stems: &[&Vec<Vec<f32>>]) -> Vec<Vec<f32>> {
    let channels = stems.iter().map(|s| s.len()).max().unwrap_or(0);
    let len = stems.iter().map(|s| crate::frames(s)).max().unwrap_or(0);
    let mut out = vec![vec![0.0f32; len]; channels];
    for s in stems {
        for (o, c) in out.iter_mut().zip(s.iter()) {
            for (a, b) in o.iter_mut().zip(c) {
                *a += b;
            }
        }
    }
    out
}

/// HTDemucs on the job audio at the job's quality: `(name, audio at the model rate)`, the rate
/// and the device note.
pub fn demucs(job: &Job, ctx: &Ctx) -> Result<(Vec<(String, Vec<Vec<f32>>)>, u32, String)> {
    let model = load(job, crate::htdemucs::ID)?;
    let rate = if model.card.sample_rate > 0 { model.card.sample_rate } else { 44_100 };
    ctx.say("Resampling");
    let mix = at_rate(&crate::to_channels(&job.audio, 2), job.rate, rate);
    ctx.check()?;
    let (stems, note) = crate::htdemucs::separate(&model, &mix, job.quality, job.device, ctx)?;
    ctx.check()?;
    Ok((stems, rate, note))
}

/// BS-RoFormer SW on the job audio: `(name, audio at the model rate)`, the rate and the device note.
pub fn sw6(job: &Job, ctx: &Ctx) -> Result<(Vec<(String, Vec<Vec<f32>>)>, u32, String)> {
    let model = load(job, crate::bs_roformer::ID)?;
    let rate = if model.card.sample_rate > 0 { model.card.sample_rate } else { 44_100 };
    ctx.say("Resampling");
    let mix = at_rate(&crate::to_channels(&job.audio, 2), job.rate, rate);
    ctx.check()?;
    let (stems, note) = crate::bs_roformer::separate(&model, &mix, job.device, ctx)?;
    ctx.check()?;
    Ok((stems, rate, note))
}

/// One model's stems: `(name, audio at the model rate)`, the rate and the device note.
pub type Separated = (Vec<(String, Vec<Vec<f32>>)>, u32, String);

/// Separate Stems' best mode with the separation supplied (so it can be stood in for): runs each
/// model of [`BEST_ROUTING`] once through `separate` and keeps the stems routed to it.
/// `(name, audio, its rate)` in [`BEST_STEMS`] order, and the device notes.
pub fn best_with(
    job: &Job,
    ctx: &Ctx,
    mut separate: impl FnMut(StemSource, &Job, &Ctx) -> Result<Separated>,
) -> Result<(Vec<(String, Vec<Vec<f32>>, u32)>, Vec<String>)> {
    let sources = best_sources();
    let total = sources.iter().map(|s| s.weight()).sum::<f32>().max(1e-6);
    let mut at = 0.0f32;
    let mut got: Vec<(String, Vec<Vec<f32>>, u32)> = Vec::new();
    let mut notes = Vec::new();
    for src in sources {
        let end = (at + src.weight() / total).min(1.0);
        let (mut stems, rate, note) = separate(src, job, &ctx.stage(at, end))?;
        at = end;
        ctx.check()?;
        notes.push(note);
        for (name, s) in BEST_ROUTING {
            if s == src {
                let a = take(&mut stems, name).ok_or_else(|| MlError::Model(src.model_id().into(), format!("no {name} stem")))?;
                got.push((name.to_string(), a, rate));
            }
        }
        // The stems another model gives (SW's vocals in best mode) are dropped here.
        drop(stems);
    }
    let mut out = Vec::with_capacity(got.len());
    for (name, _) in BEST_ROUTING {
        if let Some(i) = got.iter().position(|(n, ..)| n == name) {
            out.push(got.swap_remove(i));
        }
    }
    Ok((out, notes))
}

/// One of best mode's models on the job audio.
fn best_source(src: StemSource, job: &Job, ctx: &Ctx) -> Result<Separated> {
    match src {
        StemSource::MelBand => {
            let m = load(job, crate::vocals::ID)?;
            let (vocals, rest, rate, note) = vocals_and_rest(job, &VocalSource::MelBand(Box::new(m)), ctx)?;
            drop(rest);
            Ok((vec![("vocals".into(), vocals)], rate, note))
        }
        StemSource::Sw6 => sw6(job, ctx),
        StemSource::Demucs => demucs(job, ctx),
    }
}

/// Where the vocals of a job come from: Mel-Band RoFormer vocals when installed, else HTDemucs.
#[derive(Debug)]
pub enum VocalSource {
    MelBand(Box<InstalledModel>),
    Demucs,
}

/// The vocal source for `job` (Make Karaoke, Separate Stems' best mode, vocal isolation): the
/// Mel-Band vocals model when it is installed, else HTDemucs (an error when neither is). A broken
/// Mel-Band install is an error, not a silent fallback.
pub fn vocal_source(job: &Job) -> Result<VocalSource> {
    match load(job, crate::vocals::ID) {
        Ok(m) => Ok(VocalSource::MelBand(Box::new(m))),
        Err(MlError::NotInstalled(_)) => load(job, crate::htdemucs::ID).map(|_| VocalSource::Demucs),
        Err(e) => Err(e),
    }
}

/// The job audio split into `(vocals, everything else)` at the returned rate, by `source`, plus
/// the device note.
pub fn vocals_and_rest(job: &Job, source: &VocalSource, ctx: &Ctx) -> Result<(Vec<Vec<f32>>, Vec<Vec<f32>>, u32, String)> {
    match source {
        VocalSource::MelBand(m) => {
            let rate = if m.card.sample_rate > 0 { m.card.sample_rate } else { 44_100 };
            ctx.say("Resampling");
            let mix = at_rate(&crate::to_channels(&job.audio, 2), job.rate, rate);
            ctx.check()?;
            let ((vocals, rest), note) = crate::vocals::run(m, &mix, job.device, ctx)?;
            ctx.check()?;
            Ok((vocals, rest, rate, note))
        }
        VocalSource::Demucs => {
            let (mut stems, rate, note) = demucs(job, ctx)?;
            let vocals = take(&mut stems, "vocals").unwrap_or_default();
            let rest = sum(&stems.iter().map(|(_, a)| a).collect::<Vec<_>>());
            Ok((vocals, rest, rate, note))
        }
    }
}

fn take(stems: &mut Vec<(String, Vec<Vec<f32>>)>, name: &str) -> Option<Vec<Vec<f32>>> {
    let i = stems.iter().position(|(n, _)| n.eq_ignore_ascii_case(name))?;
    Some(stems.remove(i).1)
}

/// Peak memory a job needs, as copies of the source (stereo at about 48 kHz), from measurements
/// (karaoke holds HTDemucs' stems while the RoFormer runs). Quality High holds one more set of
/// stems (the running sum) and the shifted input.
fn copies(kind: JobKind, quality: Quality) -> usize {
    let extra = if quality == Quality::High { 10 } else { 0 };
    match kind {
        JobKind::Separate { model: SepModel::Demucs } => 14 + extra,
        // Six stems at the model rate, the input and the six back at the job rate.
        JobKind::Separate { model: SepModel::Sw6 } => 18,
        // SW's six plus the Mel-Band vocals held while SW runs.
        JobKind::Separate { model: SepModel::Best } => 20 + if SepModel::Best.uses_quality() { extra } else { 0 },
        JobKind::Karaoke { .. } => 16 + extra,
        JobKind::Drums { from_mix } => {
            if from_mix {
                18 + extra
            } else {
                16
            }
        }
        JobKind::Clean(_) => 10,
        JobKind::Midi(o) if o.isolate.stem().is_some() => 14 + extra,
        JobKind::Midi(_) | JobKind::Chords(_) | JobKind::Lyrics(_) => 2,
        JobKind::TextMidi(_) => 0,
    }
}

/// The audio Convert to MIDI transcribes: the source itself, or, with `pick` = `(source, stem)`,
/// that one stem of `separate(source)` at the stem's rate. `(audio, rate)`; the stem's device
/// note goes into `notes`.
pub fn isolated<'a>(
    job: &'a Job,
    pick: Option<(StemSource, &'static str)>,
    ctx: &Ctx,
    notes: &mut Vec<String>,
    separate: impl FnOnce(StemSource, &Job, &Ctx) -> Result<Separated>,
) -> Result<(std::borrow::Cow<'a, [Vec<f32>]>, u32)> {
    let Some((src, stem)) = pick else { return Ok((std::borrow::Cow::Borrowed(&job.audio), job.rate)) };
    ctx.say(&format!("Separating the {stem} ({})", src.label()));
    let (mut stems, rate, note) = separate(src, job, ctx)?;
    notes.push(note);
    let s = take(&mut stems, stem).ok_or_else(|| MlError::Model(src.model_id().into(), format!("no {stem} stem")))?;
    drop(stems);
    notes.push(format!("transcribed the {stem} stem ({})", src.label()));
    Ok((std::borrow::Cow::Owned(s), rate))
}

/// Where Convert to MIDI's isolation takes `isolate` from, best first: `(source, its stem name)`.
/// Vocals, drums and bass from their [`BEST_ROUTING`] model; piano from BS-RoFormer SW's piano;
/// HTDemucs (its `other` for piano) when those are not installed. `other` is HTDemucs' alone (SW's
/// other leaves out guitar and piano).
pub fn isolation_sources(isolate: crate::midi::Isolate) -> Vec<(StemSource, &'static str)> {
    use crate::midi::Isolate;
    let routed = |stem: &str| BEST_ROUTING.iter().find(|(n, _)| *n == stem).map(|(_, s)| *s);
    let mut v = Vec::new();
    match isolate {
        Isolate::None => {}
        Isolate::Vocals | Isolate::Drums | Isolate::Bass => {
            let stem = isolate.id();
            if let Some(s) = routed(stem) {
                v.push((s, stem));
            }
            v.push((StemSource::Demucs, stem));
        }
        Isolate::Piano => {
            v.push((StemSource::Sw6, "piano"));
            v.push((StemSource::Demucs, "other"));
        }
        Isolate::Other => v.push((StemSource::Demucs, "other")),
    }
    v.dedup();
    v
}

/// The first installed source of [`isolation_sources`] for `isolate` (`None` for no isolation).
/// None installed: not installed (naming HTDemucs, the fallback); a broken install is an error,
/// not a silent fallback.
pub fn isolation_source(job: &Job, isolate: crate::midi::Isolate) -> Result<Option<(StemSource, &'static str)>> {
    let mut missing = None;
    for (src, stem) in isolation_sources(isolate) {
        match load(job, src.model_id()) {
            Ok(_) => return Ok(Some((src, stem))),
            Err(MlError::NotInstalled(m)) => missing = Some(MlError::NotInstalled(m)),
            Err(e) => return Err(e),
        }
    }
    missing.map_or(Ok(None), Err)
}

/// Refuses a job the machine can't hold, before starting it (an allocation failure later would
/// abort the app): reserves the estimated working memory for a moment.
fn preflight_memory(job: &Job, len: usize) -> Result<()> {
    let per_copy = (len as u64).saturating_mul(2).saturating_mul(4).saturating_mul(u64::from(job.rate.max(44_100))) / u64::from(job.rate.max(1));
    let need = per_copy.saturating_mul(copies(job.kind, job.quality) as u64);
    let need = usize::try_from(need).unwrap_or(usize::MAX);
    let mut probe: Vec<u8> = Vec::new();
    if probe.try_reserve_exact(need).is_err() {
        return Err(MlError::BadInput(format!(
            "not enough free memory for this job (about {:.1} GB): close other programs or use a shorter selection",
            need as f64 / 1e9
        )));
    }
    Ok(())
}

/// `job` checked (audio, rate, length, models, memory) and its audio sanitised.
fn checked(job: &Job) -> Result<Job> {
    let len = crate::frames(&job.audio);
    if job.audio.is_empty() || len == 0 {
        return Err(MlError::BadInput("there is no audio to process".into()));
    }
    if job.rate < 8_000 || job.rate > 384_000 {
        return Err(MlError::BadInput(format!("unsupported sample rate {} Hz", job.rate)));
    }
    if len as f64 / f64::from(job.rate) > crate::MAX_JOB_SECONDS {
        return Err(MlError::BadInput(format!("the audio is longer than {} minutes", crate::MAX_JOB_SECONDS / 60.0)));
    }
    for id in job.kind.required_models() {
        load(job, id)?;
    }
    if matches!(job.kind, JobKind::Karaoke { .. }) {
        vocal_source(job)?;
    }
    if let JobKind::Midi(o) = job.kind {
        isolation_source(job, o.isolate)?;
    }
    preflight_memory(job, len)?;
    let mut audio = job.audio.clone();
    crate::sanitize(&mut audio);
    Ok(Job { audio, ..job.clone() })
}

fn run_inner(job: &Job, ctx: &Ctx) -> Result<JobOutput> {
    ctx.say("Checking the model files");
    // No audio in: the prompt is the input.
    if let JobKind::TextMidi(opts) = job.kind {
        let model = load(job, crate::textmidi::ID)?;
        let (g, device) = crate::textmidi::generate(&model, &opts, job.device, ctx)?;
        ctx.check()?;
        let n: usize = g.parts.iter().map(|p| p.midi.notes.len()).sum();
        let notes = vec![device, format!("{n} notes on {} instruments", g.parts.len())];
        return Ok(JobOutput { notes, generated: Some(g.parts), ..JobOutput::default() });
    }
    let job = checked(job)?;
    let len = crate::frames(&job.audio);
    let width = job.audio.len().clamp(1, 2);
    let mut notes = Vec::new();
    let stems = match job.kind {
        JobKind::Separate { model: SepModel::Sw6 } => {
            let (stems, rate, note) = sw6(&job, &ctx.stage(0.0, 0.92))?;
            notes.push(note);
            let parts = stems.into_iter().map(|(n, a)| (pretty(&n), a, rate)).collect();
            finish(parts, &job, len, width, &ctx.stage(0.92, 1.0))?
        }
        JobKind::Separate { model: SepModel::Best } => {
            let (parts, more) = best_with(&job, &ctx.stage(0.0, 0.92), best_source)?;
            notes.extend(more);
            let parts = parts.into_iter().map(|(n, a, r)| (pretty(&n), a, r)).collect();
            finish(parts, &job, len, width, &ctx.stage(0.92, 1.0))?
        }
        JobKind::Separate { model: SepModel::Demucs } => {
            let (stems, rate, note) = demucs(&job, &ctx.stage(0.0, 0.92))?;
            notes.push(note);
            let parts = stems.into_iter().map(|(n, a)| (pretty(&n), a, rate)).collect();
            finish(parts, &job, len, width, &ctx.stage(0.92, 1.0))?
        }
        JobKind::Karaoke { keep_backing } => {
            // Karaoke works without its own model (with a note); a broken install is an error.
            let karaoke = match load(&job, crate::karaoke::ID) {
                Ok(k) => Some(k),
                Err(MlError::NotInstalled(_)) => None,
                Err(e) => return Err(e),
            };
            let split = if karaoke.is_some() { 0.45 } else { 0.92 };
            // The vocals from Mel-Band RoFormer (cleaner than HTDemucs' for the lead/backing
            // split: no double-separation artefacts), the band = the rest.
            let source = vocal_source(&job)?;
            let (vocals, band, rate, note) = vocals_and_rest(&job, &source, &ctx.stage(0.0, split))?;
            notes.push(note);
            if matches!(source, VocalSource::Demucs) {
                notes.push("vocals from HTDemucs (install Mel-Band RoFormer vocals for cleaner karaoke)".into());
            }
            match karaoke {
                Some(k) => {
                    let krate = if k.card.sample_rate > 0 { k.card.sample_rate } else { 44_100 };
                    let v = at_rate(&vocals, rate, krate);
                    drop(vocals);
                    let (mut parts, knote) = crate::karaoke::run(&k, &v, job.device, &ctx.stage(split, 0.92))?;
                    drop(v);
                    ctx.check()?;
                    if knote != notes.first().cloned().unwrap_or_default() {
                        notes.push(knote);
                    }
                    let (lead, backing) = crate::karaoke::take_lead_and_backing(&mut parts)?;
                    drop(parts);
                    let fin = ctx.stage(0.92, 1.0);
                    let mut out = finish(
                        vec![("Lead Vocal".into(), lead, krate), ("Backing Vocals".into(), backing, krate)],
                        &job,
                        len,
                        width,
                        &fin.stage(0.0, 0.66),
                    )?;
                    let band = finish(vec![("Instrumental".into(), band, rate)], &job, len, width, &fin.stage(0.66, 1.0))?;
                    let mut band = band.into_iter().next().map(|s| s.audio).unwrap_or_default();
                    if keep_backing && let Some(b) = out.pop() {
                        band = sum(&[&band, &b.audio]);
                    }
                    out.insert(1, Stem { name: "Instrumental".into(), audio: band });
                    out
                }
                None => {
                    notes.push("the karaoke model is not installed: the vocal track holds lead and backing vocals together".into());
                    finish(vec![("Vocals".into(), vocals, rate), ("Instrumental".into(), band, rate)], &job, len, width, &ctx.stage(0.92, 1.0))?
                }
            }
        }
        JobKind::Drums { from_mix } => {
            let model = load(&job, crate::drumsep::ID)?;
            let drate = if model.card.sample_rate > 0 { model.card.sample_rate } else { 44_100 };
            let drums = if from_mix {
                let (mut stems, rate, note) = demucs(&job, &ctx.stage(0.0, 0.45))?;
                notes.push(note);
                let d = take(&mut stems, "drums").ok_or_else(|| MlError::Model(crate::htdemucs::ID.into(), "no drums stem".into()))?;
                drop(stems);
                at_rate(&d, rate, drate)
            } else {
                at_rate(&crate::to_channels(&job.audio, 2), job.rate, drate)
            };
            ctx.check()?;
            let (parts, note) = crate::drumsep::run(&model, &drums, job.device, &ctx.stage(if from_mix { 0.45 } else { 0.0 }, 0.92))?;
            drop(drums);
            notes.push(note);
            let parts = parts.into_iter().map(|(n, a)| (pretty(&n), a, drate)).collect();
            finish(parts, &job, len, width, &ctx.stage(0.92, 1.0))?
        }
        JobKind::Clean(o) => {
            use crate::clean::CleanMode;
            let mut a = job.audio.clone();
            let mut rate = job.rate;
            let speech = o.mode == CleanMode::Speech;
            let srs = o.runs_srs();
            let split = if speech && srs { 0.4 } else { 0.92 };
            if speech {
                ctx.say("De-noising (DeepFilterNet 3, CPU)");
                a = at_rate(&a, rate, crate::denoise::SAMPLE_RATE);
                rate = crate::denoise::SAMPLE_RATE;
                a = crate::denoise::denoise_48k(&a, o.atten_db, &ctx.stage(0.0, split))?;
                notes.push("de-noise: CPU".into());
            }
            ctx.check()?;
            if srs {
                let model = load(&job, crate::srs::ID)?;
                let srate = if model.card.sample_rate > 0 { model.card.sample_rate } else { 48_000 };
                a = at_rate(&a, rate, srate);
                rate = srate;
                let (parts, note) = crate::srs::run(&model, &a, job.device, &ctx.stage(if speech { split } else { 0.0 }, 0.92))?;
                notes.push(format!("{}: {note}", if speech { "de-reverb" } else { "restore" }));
                a = parts.into_iter().next().map(|(_, a)| a).ok_or_else(|| MlError::Model(crate::srs::ID.into(), "no output".into()))?;
            }
            let mut out = finish(vec![("Clean".into(), a, rate)], &job, len, width, &ctx.stage(0.92, 1.0))?;
            if srs && let Some(s) = out.first_mut() {
                // One fixed make-up gain against the source (at the job's rate and width).
                let dry = crate::to_channels(&job.audio, width);
                let g = crate::clean::makeup_gain(&dry, &s.audio, job.rate);
                crate::clean::apply_gain(&mut s.audio, g);
                notes.push(format!("make-up gain {:+.1} dB", 20.0 * g.max(1e-9).log10()));
                if !speech && o.mix < 1.0 {
                    crate::clean::mix_dry(&mut s.audio, &dry, o.mix);
                    notes.push(format!("amount {:.0} %", o.mix * 100.0));
                }
            }
            out
        }
        JobKind::Midi(opts) => {
            let pick = isolation_source(&job, opts.isolate)?;
            let (a, more) = analyse_midi_with(&job, pick, ctx, best_source, |audio, rate, ctx| {
                let model = load(&job, opts.model.model_id())?;
                crate::midi::analyse(&model, audio, rate, &opts, job.device, ctx)
            })?;
            notes.extend(more);
            return Ok(JobOutput { notes, midi: Some(a.decode(&opts)), ..JobOutput::default() });
        }
        JobKind::Chords(opts) => {
            let model = load(&job, crate::chords::ID)?;
            let (segs, note) = crate::chords::detect(&model, &job.audio, job.rate, &opts, job.device, ctx)?;
            ctx.check()?;
            ctx.report(1.0);
            notes.push(note);
            notes.retain(|n| !n.is_empty());
            return Ok(JobOutput { notes, chords: Some(segs), ..JobOutput::default() });
        }
        JobKind::Lyrics(opts) => {
            let model = load(&job, crate::lyrics::ID)?;
            let (lyrics, note) = crate::lyrics::transcribe(&model, &job.audio, job.rate, &opts, ctx)?;
            ctx.check()?;
            ctx.report(1.0);
            notes.push(note);
            notes.retain(|n| !n.is_empty());
            return Ok(JobOutput { notes, lyrics: Some(lyrics), ..JobOutput::default() });
        }
        // Handled before the audio checks.
        JobKind::TextMidi(_) => return Err(MlError::BadInput("Generate MIDI from Text takes no audio".into())),
    };
    ctx.check()?;
    ctx.report(1.0);
    notes.retain(|n| !n.is_empty());
    notes.dedup();
    Ok(JobOutput { stems, notes, ..JobOutput::default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_names() {
        assert_eq!(pretty("vocals"), "Vocals");
        assert_eq!(pretty("hh"), "Hi-Hat");
        assert_eq!(pretty("lead_vocal"), "Lead Vocal");
    }

    fn job(kind: JobKind, dir: PathBuf) -> Job {
        let tone: Vec<f32> = (0..44_100).map(|i| 0.2 * (i as f32 * 0.05).sin()).collect();
        Job { kind, audio: vec![tone], rate: 44_100, device: Device::Cpu, models_dir: dir, quality: Quality::Fast }
    }

    /// A model folder `dir/<id>` with a card and a placeholder ONNX file (no checksum).
    pub(crate) fn fake_model(dir: &std::path::Path, id: &str) {
        let m = dir.join(id);
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(m.join("m.onnx"), b"not a graph").unwrap();
        std::fs::write(m.join("card.json"), format!(r#"{{"id":"{id}","name":"{id}","file":"m.onnx","sample_rate":44100}}"#)).unwrap();
    }

    #[test]
    fn karaoke_takes_its_vocals_from_mel_band_when_installed() {
        let dir = std::env::temp_dir().join(format!("sc-ml-vsrc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let k = job(JobKind::Karaoke { keep_backing: false }, dir.clone());
        // Neither model: not installed.
        assert!(matches!(vocal_source(&k), Err(MlError::NotInstalled(_))));
        // HTDemucs only: its vocals.
        fake_model(&dir, crate::htdemucs::ID);
        assert!(matches!(vocal_source(&k), Ok(VocalSource::Demucs)));
        // Mel-Band vocals installed: it wins.
        fake_model(&dir, crate::vocals::ID);
        match vocal_source(&k) {
            Ok(VocalSource::MelBand(m)) => assert_eq!(m.card.id, crate::vocals::ID),
            other => panic!("{other:?}"),
        }
        // A broken Mel-Band install is an error, not a silent fallback.
        std::fs::write(dir.join(crate::vocals::ID).join("card.json"), "{").unwrap();
        assert!(matches!(vocal_source(&k), Err(MlError::Model(..))));
        // Karaoke no longer needs HTDemucs when Mel-Band is there.
        assert!(!JobKind::Karaoke { keep_backing: false }.required_models().contains(&crate::htdemucs::ID));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn separate_model_choice() {
        assert_eq!(SepModel::parse("SW6"), Some(SepModel::Sw6));
        assert_eq!(SepModel::parse("demucs"), Some(SepModel::Demucs));
        assert_eq!(SepModel::parse("Best"), Some(SepModel::Best));
        assert_eq!(SepModel::parse("auto"), None);
        assert_eq!(SepModel::default(), SepModel::Demucs);
        assert_eq!(JobKind::Separate { model: SepModel::Sw6 }.required_models(), vec![crate::bs_roformer::ID]);
        assert_eq!(JobKind::Separate { model: SepModel::Demucs }.required_models(), vec![crate::htdemucs::ID]);
        assert_eq!(SepModel::Sw6.stems(), ["bass", "drums", "other", "vocals", "guitar", "piano"]);
        // sw6 without its model: not installed, even with HTDemucs there.
        let dir = std::env::temp_dir().join(format!("sc-ml-sw6-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        fake_model(&dir, crate::htdemucs::ID);
        let r = run(&job(JobKind::Separate { model: SepModel::Sw6 }, dir.clone()), &Ctx::default());
        assert!(matches!(&r, Err(MlError::NotInstalled(m)) if m.contains(crate::bs_roformer::ID)), "{r:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_models_and_bad_input_are_errors() {
        let dir = std::env::temp_dir().join(format!("sc-ml-none-{}", std::process::id()));
        for kind in [JobKind::Separate { model: SepModel::Demucs }, JobKind::Karaoke { keep_backing: true }, JobKind::Drums { from_mix: false }] {
            assert!(matches!(run(&job(kind, dir.clone()), &Ctx::default()), Err(MlError::NotInstalled(_))), "{kind:?}");
        }
        let clean = JobKind::Clean(crate::clean::CleanOptions::vocal(1.0));
        assert!(matches!(run(&job(clean, dir.clone()), &Ctx::default()), Err(MlError::NotInstalled(_))));
        let speech = JobKind::Clean(crate::clean::CleanOptions::speech(24.0, true));
        assert!(matches!(run(&job(speech, dir.clone()), &Ctx::default()), Err(MlError::NotInstalled(_))));
        let mut j = job(JobKind::Separate { model: SepModel::Demucs }, dir);
        j.audio = vec![Vec::new()];
        assert!(matches!(run(&j, &Ctx::default()), Err(MlError::BadInput(_))));
    }

    #[test]
    fn convert_to_midi_transcribes_the_isolated_stem_not_the_mix() {
        use crate::midi::{MidiModel, MidiOptions};
        let dir = std::env::temp_dir().join(format!("sc-ml-none-{}", std::process::id()));
        let mut j = job(JobKind::Midi(MidiOptions::new(MidiModel::Piano)), dir.clone());
        j.audio = vec![vec![0.5; 1000], vec![0.5; 1000]];
        let other: Vec<Vec<f32>> = vec![vec![0.1; 1000], vec![0.2; 1000]];
        let other2 = other.clone();
        let fake = move |src: StemSource, _: &Job, _: &Ctx| {
            assert_eq!(src, StemSource::Demucs);
            Ok((
                vec![
                    ("drums".to_string(), vec![vec![0.0; 1000]; 2]),
                    ("other".to_string(), other2.clone()),
                    ("vocals".to_string(), vec![vec![0.3; 1000]; 2]),
                ],
                44_100,
                "fake".to_string(),
            ))
        };
        let mut notes = Vec::new();
        let (audio, rate) = isolated(&j, Some((StemSource::Demucs, "other")), &Ctx::default(), &mut notes, fake).unwrap();
        assert_eq!((audio.as_ref(), rate), (other.as_slice(), 44_100));
        assert_eq!(notes, vec!["fake".to_string(), "transcribed the other stem (HTDemucs)".to_string()]);
        // Without isolation the source goes through untouched, and no separator is called.
        let mut notes = Vec::new();
        let (audio, rate) = isolated(&j, None, &Ctx::default(), &mut notes, |_, _, _| panic!("no separation asked for")).unwrap();
        assert_eq!((audio.as_ref(), rate), (j.audio.as_slice(), j.rate));
        assert!(notes.is_empty());
        // A separator without the stem is an error, not a silent fall-back to the mix.
        let mut notes = Vec::new();
        let r = isolated(&j, Some((StemSource::Sw6, "bass")), &Ctx::default(), &mut notes, |_, _, _| Ok((Vec::new(), 44_100, String::new())));
        assert!(r.is_err());
        // Isolation needs a separator: none installed here.
        assert!(matches!(run(&j, &Ctx::default()), Err(MlError::NotInstalled(_))));
        assert_eq!(JobKind::Midi(MidiOptions::new(MidiModel::Piano)).required_models(), vec![crate::pianokong::ID]);
        assert_eq!(JobKind::Midi(MidiOptions::new(MidiModel::General)).required_models(), vec![crate::basicpitch::ID]);
    }

    #[test]
    fn convert_to_midi_isolates_from_the_best_installed_source() {
        use crate::midi::{Isolate, MidiModel, MidiOptions};
        assert_eq!(isolation_sources(Isolate::None), vec![]);
        assert_eq!(isolation_sources(Isolate::Piano), vec![(StemSource::Sw6, "piano"), (StemSource::Demucs, "other")]);
        assert_eq!(isolation_sources(Isolate::Drums), vec![(StemSource::Sw6, "drums"), (StemSource::Demucs, "drums")]);
        assert_eq!(isolation_sources(Isolate::Vocals), vec![(StemSource::MelBand, "vocals"), (StemSource::Demucs, "vocals")]);
        assert_eq!(isolation_sources(Isolate::Bass), vec![(StemSource::Sw6, "bass"), (StemSource::Demucs, "bass")]);
        assert_eq!(isolation_sources(Isolate::Other), vec![(StemSource::Demucs, "other")]);
        let dir = std::env::temp_dir().join(format!("sc-ml-iso-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let j = job(JobKind::Midi(MidiOptions::new(MidiModel::Piano)), dir.clone());
        // Nothing installed: not installed, naming HTDemucs.
        assert!(matches!(isolation_source(&j, Isolate::Piano), Err(MlError::NotInstalled(m)) if m.contains(crate::htdemucs::ID)));
        assert_eq!(isolation_source(&j, Isolate::None).unwrap(), None);
        // HTDemucs only: everything from it (piano = its other).
        fake_model(&dir, crate::htdemucs::ID);
        assert_eq!(isolation_source(&j, Isolate::Piano).unwrap(), Some((StemSource::Demucs, "other")));
        assert_eq!(isolation_source(&j, Isolate::Vocals).unwrap(), Some((StemSource::Demucs, "vocals")));
        // The RoFormers installed: piano and drums from SW, vocals from Mel-Band.
        fake_model(&dir, crate::bs_roformer::ID);
        fake_model(&dir, crate::vocals::ID);
        assert_eq!(isolation_source(&j, Isolate::Piano).unwrap(), Some((StemSource::Sw6, "piano")));
        assert_eq!(isolation_source(&j, Isolate::Drums).unwrap(), Some((StemSource::Sw6, "drums")));
        assert_eq!(isolation_source(&j, Isolate::Vocals).unwrap(), Some((StemSource::MelBand, "vocals")));
        assert_eq!(isolation_source(&j, Isolate::Other).unwrap(), Some((StemSource::Demucs, "other")));
        // The piano model transcribes SW's piano stem.
        let opts = MidiOptions::new(MidiModel::Piano);
        let pick = isolation_source(&j, opts.isolate).unwrap();
        let sep = |src: StemSource, _: &Job, _: &Ctx| {
            assert_eq!(src, StemSource::Sw6);
            Ok((vec![("other".to_string(), vec![vec![0.1; 8]; 2]), ("piano".to_string(), vec![vec![0.7; 8]; 2])], 44_100, "gpu".to_string()))
        };
        let net = |audio: &[Vec<f32>], _: u32, _: &Ctx| {
            assert_eq!(audio[0][0], 0.7);
            let raw = crate::basicpitch::Raw { frames: 0, note: Vec::new(), onset: Vec::new(), contour: Vec::new() };
            Ok((crate::midi::Analysed { analysis: crate::midi::Analysis::General(raw), secs: 0.0 }, String::new()))
        };
        let (_, notes) = analyse_midi_with(&j, pick, &Ctx::default(), sep, net).unwrap();
        assert_eq!(notes, vec!["gpu".to_string(), "transcribed the piano stem (BS-RoFormer SW)".to_string()]);
        // A broken SW install is an error, not a silent fallback to HTDemucs.
        std::fs::write(dir.join(crate::bs_roformer::ID).join("card.json"), "{").unwrap();
        assert!(matches!(isolation_source(&j, Isolate::Piano), Err(MlError::Model(..))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn analyse_once_decode_many_times() {
        use crate::midi::{Analysed, Analysis, MidiModel, MidiOptions};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = std::env::temp_dir().join(format!("sc-ml-none-{}", std::process::id()));
        let j = job(JobKind::Midi(MidiOptions::new(MidiModel::General)), dir);
        let runs = AtomicUsize::new(0);
        let fake = |audio: &[Vec<f32>], rate: u32, _: &Ctx| {
            runs.fetch_add(1, Ordering::SeqCst);
            let frames = 60;
            let mut note = vec![0.0f32; frames * 88];
            let mut onset = vec![0.0f32; frames * 88];
            for r in 10..40 {
                note[r * 88 + 39] = 0.8;
            }
            onset[10 * 88 + 39] = 0.9;
            let secs = crate::frames(audio) as f64 / f64::from(rate);
            Ok((Analysed { analysis: Analysis::General(crate::basicpitch::Raw { frames, note, onset, contour: Vec::new() }), secs }, "fake".into()))
        };
        let opts = MidiOptions::new(MidiModel::General);
        let (a, notes) = analyse_midi_with(&j, None, &Ctx::default(), |_, _, _| panic!("no isolation"), fake).unwrap();
        assert_eq!(notes, vec!["fake".to_string()]);
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        // Two decodes with different settings: the network is not run again.
        let loose = a.decode(&MidiOptions { onset_threshold: Some(0.5), ..opts });
        let strict = a.decode(&MidiOptions { frame_threshold: Some(0.95), ..opts });
        assert_eq!(loose.notes.len(), 1);
        assert!(strict.notes.is_empty());
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        // The committed transcription is the decode of the same analysis.
        assert_eq!(a.decode(&opts), loose);
    }

    #[test]
    fn clean_vocal_denoise_keeps_length_width_and_rate() {
        let dir = std::env::temp_dir().join(format!("sc-ml-none-{}", std::process::id()));
        let kind = JobKind::Clean(crate::clean::CleanOptions::speech(24.0, false));
        let j = job(kind, dir);
        let out = run(&j, &Ctx::default()).unwrap();
        assert_eq!(out.stems.len(), 1);
        assert_eq!(out.stems[0].name, "Clean");
        assert_eq!(out.stems[0].audio.len(), 1);
        assert_eq!(out.stems[0].audio[0].len(), 44_100);
    }

    #[test]
    fn best_routing_table_is_consistent() {
        // Every stem once, in BEST_STEMS order; vocals from Mel-Band; SW gives the rest.
        let names: Vec<&str> = BEST_ROUTING.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, BEST_STEMS);
        assert_eq!(SepModel::Best.stems(), BEST_STEMS);
        assert_eq!(BEST_ROUTING[0], ("vocals", StemSource::MelBand));
        for (n, s) in BEST_ROUTING.iter().skip(1) {
            assert_eq!(*s, StemSource::Sw6, "{n}");
            assert!(crate::bs_roformer::DEFAULT_STEMS.contains(n), "{n}");
        }
        assert_eq!(JobKind::Separate { model: SepModel::Best }.required_models(), vec![crate::vocals::ID, crate::bs_roformer::ID]);
        assert!(!SepModel::Best.uses_quality());
        assert!(SepModel::Demucs.uses_quality() && !SepModel::Sw6.uses_quality());
    }

    #[test]
    fn best_mode_takes_each_stem_from_its_model() {
        use std::cell::RefCell;
        let dir = std::env::temp_dir().join(format!("sc-ml-best-{}", std::process::id()));
        let j = job(JobKind::Separate { model: SepModel::Best }, dir.clone());
        let level = |v: f32| vec![vec![v; 10]; 2];
        let calls = RefCell::new(Vec::new());
        let fake = |src: StemSource, _: &Job, _: &Ctx| {
            calls.borrow_mut().push(src);
            Ok(match src {
                // Mel-Band: its vocals at 44.1 kHz.
                StemSource::MelBand => (vec![("vocals".to_string(), level(1.0))], 44_100, "gpu".to_string()),
                // SW: six stems (its vocals must not be used) at 48 kHz.
                StemSource::Sw6 => (
                    ["bass", "drums", "other", "vocals", "guitar", "piano"]
                        .iter()
                        .enumerate()
                        .map(|(i, n)| (n.to_string(), level(10.0 + i as f32)))
                        .collect(),
                    48_000,
                    "gpu".to_string(),
                ),
                StemSource::Demucs => panic!("HTDemucs is not in best mode's routing"),
            })
        };
        let (out, notes) = best_with(&j, &Ctx::default(), fake).unwrap();
        // Each model once, Mel-Band first.
        assert_eq!(*calls.borrow(), vec![StemSource::MelBand, StemSource::Sw6]);
        let got: Vec<(&str, f32, u32)> = out.iter().map(|(n, a, r)| (n.as_str(), a[0][0], *r)).collect();
        assert_eq!(
            got,
            vec![
                ("vocals", 1.0, 44_100),
                ("drums", 11.0, 48_000),
                ("bass", 10.0, 48_000),
                ("guitar", 14.0, 48_000),
                ("piano", 15.0, 48_000),
                ("other", 12.0, 48_000)
            ]
        );
        assert_eq!(notes, vec!["gpu".to_string(), "gpu".to_string()]);
        // A model without its routed stem is an error, not a silent gap.
        let r = best_with(&j, &Ctx::default(), |src, _, _| {
            Ok((if src == StemSource::MelBand { vec![("vocals".to_string(), level(1.0))] } else { Vec::new() }, 44_100, String::new()))
        });
        assert!(matches!(r, Err(MlError::Model(m, _)) if m == crate::bs_roformer::ID));
        // Without the models: not installed (SW alone is not enough).
        fake_model(&dir, crate::bs_roformer::ID);
        let r = run(&j, &Ctx::default());
        assert!(matches!(&r, Err(MlError::NotInstalled(m)) if m.contains(crate::vocals::ID)), "{r:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
