//! Local ML audio in the session (desktop only): Separate Stems, Make Karaoke, Split Drums,
//! Clean Vocal and Convert to MIDI (`soundcraft-ml` does the inference).
//!
//! A job has three steps so the UI stays live:
//! 1. [`prepare`] (UI thread): render the source — a clip as it plays, or a track pre-fader like
//!    Commit — into an [`MlJob`].
//! 2. [`MlJob::run`] (a worker thread): inference, with progress and Cancel.
//! 3. [`apply`] (UI thread): the results as ONE undo step. Separation results are new tracks
//!    directly under the source track, at the source's position, named `<track> <Stem>`; their audio
//!    is `<source> - <Stem>` (32-bit float WAV in `Audio Files/` when the session is saved, like
//!    Commit's renders). Clean Vocal replaces each clip with its cleaned audio, like Process; the
//!    original source stays in the pool, so Undo restores it. Convert to MIDI makes one new
//!    instrument track `<track> MIDI` directly under the source track, with one MIDI clip per source
//!    clip at the same position; note times become ticks on the session's tempo map.
//!
//! The `ml.*` commands run all three at once (blocking), for the CLI, control channel and MCP.

use crate::cmd::{bad, bool_or, f32_or, str_param, track_param};
use crate::{Engine, Result};
use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_model::{ChannelFormat, CleanAlt, Clip, ClipContent, ClipId, TrackId, TrackKind};
use soundcraft_time::{Range, Samples};
use std::path::PathBuf;

pub use crate::cmd::chord_lane::{Grid, KeyChoice, LaneOptions};
pub use soundcraft_ml::job::ProgressCell;
pub use soundcraft_ml::models::sha256_file;
pub use soundcraft_ml::models::{
    KNOWN_MODELS, LICENCE_NOT_STATED, ModelEntry, ModelStatus, default_models_dir, known_licence, licence_status, list as list_models, models_dir,
    read_card,
};
pub use soundcraft_ml::pipeline::isolation_sources;
pub use soundcraft_ml::{
    Analysed, Analysis, Cancel, ChordOptions, ChordSegment, CleanMode, CleanOptions, Ctx, Device, Isolate, JobKind, JobOutput, Lyrics,
    LyricsLanguage, LyricsOptions, MidiModel, MidiOptions, MlError, Prompt, Quality, SepModel, StemSource, Transcription,
};
pub use soundcraft_ml::{adtof, basicpitch, catalog, download};

/// Clean Vocal's default de-noise amount (attenuation limit), dB.
pub const DEFAULT_AMOUNT_DB: f32 = soundcraft_ml::denoise::DEFAULT_ATTEN_DB;

/// What a job's result replaces or goes under.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// New tracks under `track`, their clips starting at `start`, `len` frames long.
    Track { track: TrackId, start: Samples, len: Samples, name: String },
    /// Clean Vocal: the clip `clip` on `track` gets the cleaned audio beside its original
    /// ([`soundcraft_model::CleanAlt`]), at `mix` (0–1; `None` keeps a re-run clip's amount, else
    /// 1). `was` is the clip as it was rendered: a clip moved, trimmed or re-sourced meanwhile is
    /// refused.
    Clip { clip: ClipId, track: TrackId, name: String, was: Box<Clip>, mix: Option<f32> },
    /// Convert to MIDI: the notes of the clip `clip` (`name`) on `track` become a MIDI clip at
    /// `start`, `len` long, on the new instrument track under `track`.
    Midi { track: TrackId, clip: ClipId, start: Samples, len: Samples, name: String },
}

/// One source of a job: its audio (session rate) and where the result goes.
#[derive(Debug, Clone)]
pub struct Part {
    pub target: Target,
    pub audio: Vec<Vec<f32>>,
}

/// A prepared job: the rendered sources and the options, ready for a worker thread.
#[derive(Debug, Clone)]
pub struct MlJob {
    pub command: &'static str,
    pub kind: JobKind,
    pub parts: Vec<Part>,
    pub rate: u32,
    pub device: Device,
    pub models_dir: PathBuf,
    /// How carefully the HTDemucs step runs (`quality`: fast | high).
    pub quality: Quality,
    /// Stems to keep (lower-case names), all when empty (Separate Stems).
    pub keep: Vec<String>,
    /// Mute the source track once its results are in.
    pub mute_original: bool,
    /// Detect Chords: how the chords go onto the lane.
    pub lane: LaneOptions,
}

/// What a finished job made: per part, the stems.
#[derive(Debug, Clone)]
pub struct MlResult {
    pub parts: Vec<JobOutput>,
}

/// The command id for a job kind.
pub fn command_for(kind: &JobKind) -> &'static str {
    match kind {
        JobKind::Separate { .. } => "ml.separate",
        JobKind::Karaoke { .. } => "ml.karaoke",
        JobKind::Drums { .. } => "ml.drums",
        JobKind::Clean { .. } => "ml.clean",
        JobKind::Midi(_) => "ml.midi",
        JobKind::Chords(_) => "ml.chords",
        JobKind::Lyrics(_) => "ml.lyrics",
    }
}

/// Convert to MIDI's options from command params: `model` (general | piano | drums | vocal),
/// `onset_threshold`, `frame_threshold`, `min_note_ms`, `pitch_bends`, `pedal`, `drum_thresholds`
/// (five numbers 0..1: kick, snare, toms, hi-hat, cymbals), `isolate` (none | drums | piano |
/// other | vocals | bass, from the best installed source: [`soundcraft_ml::pipeline::isolation_sources`]; default from the model, see [`Isolate::default_for`]; [`prepare`] looks at the
/// track name too).
pub fn midi_options(cmd: &str, p: &Value) -> Result<MidiOptions> {
    let model = match str_param(p, "model") {
        Some(m) => MidiModel::parse(m).ok_or_else(|| bad(cmd, format!("unknown model `{m}` (general, piano, drums, vocal)")))?,
        None => MidiModel::General,
    };
    let num = |k: &str| -> Result<Option<f32>> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v.as_f64().filter(|v| v.is_finite()).map(|v| Some(v as f32)).ok_or_else(|| bad(cmd, format!("`{k}` must be a number"))),
        }
    };
    let unit = |k: &str| -> Result<Option<f32>> {
        match num(k)? {
            Some(v) if !(0.0..=1.0).contains(&v) => Err(bad(cmd, format!("`{k}` must be between 0 and 1"))),
            v => Ok(v),
        }
    };
    let min_note_ms = match num("min_note_ms")? {
        Some(v) if !(0.0..=10_000.0).contains(&v) => return Err(bad(cmd, "`min_note_ms` must be between 0 and 10000")),
        v => v,
    };
    let drum_thresholds = match p.get("drum_thresholds") {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) if a.len() == 5 => {
            let mut t = [0.0f32; 5];
            for (slot, v) in t.iter_mut().zip(a) {
                *slot = match v.as_f64() {
                    Some(x) if x.is_finite() && (0.0..=1.0).contains(&x) => x as f32,
                    _ => return Err(bad(cmd, "`drum_thresholds` must be five numbers between 0 and 1")),
                };
            }
            Some(t)
        }
        Some(_) => return Err(bad(cmd, "`drum_thresholds` must be five numbers between 0 and 1 (kick, snare, toms, hi-hat, cymbals)")),
    };
    let isolate = match str_param(p, "isolate") {
        Some(i) => Isolate::parse(i).ok_or_else(|| bad(cmd, format!("unknown isolate `{i}` (none, drums, piano, other, vocals, bass)")))?,
        None => Isolate::default_for(model, None),
    };
    Ok(MidiOptions {
        model,
        onset_threshold: unit("onset_threshold")?,
        frame_threshold: unit("frame_threshold")?,
        min_note_ms,
        pitch_bends: bool_or(p, "pitch_bends", false),
        pedal: bool_or(p, "pedal", true),
        drum_thresholds,
        isolate,
    })
}

/// Clean Vocal's options from `p`. With `mode` ("vocal" | "speech"): Vocal runs Renaissance alone
/// at `mix` % wet (0..100, default 100); Speech de-noises at `amount_db` and de-reverbs unless
/// `dereverb: false`. Without `mode` (the old params): `dereverb` (default true) means Vocal, since
/// Renaissance takes the noise off too; `denoise` alone (`dereverb: false`) means Speech without
/// de-reverb; neither is an error.
pub fn clean_options(cmd: &str, p: &Value) -> Result<CleanOptions> {
    let atten_db = f32_or(p, "amount_db", soundcraft_ml::denoise::DEFAULT_ATTEN_DB).clamp(0.0, 100.0);
    let mix = match p.get("mix") {
        None | Some(Value::Null) => 1.0,
        Some(v) => match v.as_f64().filter(|v| v.is_finite() && (0.0..=100.0).contains(v)) {
            Some(m) => (m / 100.0) as f32,
            None => return Err(bad(cmd, "`mix` must be a number from 0 to 100 (%)")),
        },
    };
    match p.get("mode") {
        None | Some(Value::Null) => CleanOptions::from_legacy(bool_or(p, "denoise", true), bool_or(p, "dereverb", true), atten_db)
            .map(|o| if o.mode == CleanMode::Vocal { CleanOptions::vocal(mix) } else { o })
            .ok_or_else(|| bad(cmd, "choose De-noise, De-reverb or both (or `mode`: vocal | speech)")),
        Some(v) => match v.as_str().and_then(CleanMode::parse) {
            Some(CleanMode::Vocal) => Ok(CleanOptions::vocal(mix)),
            Some(CleanMode::Speech) => Ok(CleanOptions::speech(atten_db, bool_or(p, "dereverb", true))),
            None => Err(bad(cmd, "`mode` must be \"vocal\" or \"speech\"")),
        },
    }
}

/// Separate Stems' `model`: demucs (default: HTDemucs, four stems) | sw6 (BS-RoFormer SW, six) |
/// best (vocals from Mel-Band RoFormer, the other five from SW: [`soundcraft_ml::pipeline::BEST_ROUTING`]).
pub fn sep_model(cmd: &str, p: &Value) -> Result<SepModel> {
    match str_param(p, "model") {
        Some(m) => SepModel::parse(m).ok_or_else(|| bad(cmd, format!("unknown model `{m}` (demucs, sw6, best)"))),
        None => Ok(SepModel::Demucs),
    }
}

/// The job kind `cmd` runs with params `p`.
pub fn kind_from(cmd: &str, p: &Value) -> Result<JobKind> {
    Ok(match cmd {
        "ml.separate" => JobKind::Separate { model: sep_model(cmd, p)? },
        "ml.karaoke" => JobKind::Karaoke { keep_backing: bool_or(p, "keep_backing", false) },
        "ml.drums" => JobKind::Drums { from_mix: bool_or(p, "from_mix", false) },
        "ml.clean" => JobKind::Clean(clean_options(cmd, p)?),
        "ml.midi" => JobKind::Midi(midi_options(cmd, p)?),
        "ml.chords" => JobKind::Chords(ChordOptions { bass: bool_or(p, "bass", false) }),
        "ml.lyrics" => JobKind::Lyrics(lyrics_options(cmd, p)?),
        other => return Err(bad(other, "not an ML command")),
    })
}

fn ml_err(cmd: &str, e: MlError) -> crate::EngineError {
    match e {
        MlError::BadInput(m) | MlError::NotInstalled(m) => bad(cmd, m),
        other => crate::EngineError::Io(format!("{cmd}: {other}")),
    }
}

/// The track a job runs on: `track`, else the first selected track.
fn source_track(e: &Engine, cmd: &str, p: &Value) -> Result<TrackId> {
    if let Some(t) = track_param(e, cmd, p, "track")? {
        return Ok(t);
    }
    e.target_tracks().into_iter().next().ok_or_else(|| bad(cmd, "select an audio track or clip first"))
}

/// The clips a job runs on: `clips`/`clip`, else the selected clips; with `track` (or only a
/// track selected), none.
fn source_clips(e: &Engine, p: &Value) -> Vec<ClipId> {
    if p.get("clips").is_some() || p.get("clip").is_some() {
        return crate::cmd::clip_ids_param(e, p);
    }
    if p.get("track").is_some() {
        return Vec::new();
    }
    e.session().edit.selected_clips.clone()
}

/// A clip's audio as it plays (gain, fades and clip effects included) over its own range.
fn render_clip(e: &Engine, track: TrackId, clip: &Clip, bare: bool) -> Vec<Vec<f32>> {
    let mut tmp = e.session().clone();
    if let Some(pl) = tmp.track_mut(track).and_then(|t| t.playlist_mut()) {
        pl.clips.retain(|c| c.id == clip.id);
    }
    if bare && let Some(c) = tmp.find_clip_mut(clip.id) {
        // Clean Vocal keeps the clip's fades and gain on the new clip: render without them, and
        // from the original audio (a re-run replaces the cleaned audio, it doesn't clean it twice).
        c.clean = None;
        c.fade_in = Default::default();
        c.fade_out = Default::default();
        c.gain_db = 0.0;
        c.gain_env.clear();
        c.muted = false;
    }
    if bare {
        // ... and without its clip effects, which stay on the clip and keep playing live.
        tmp.edit.set_flag(&format!("clip_fx.bypass.{}", clip.id.0), true);
    }
    soundcraft_mix::render_clips(&tmp, track, clip.range())
}

/// Step 1: render the sources of `cmd` (an `ml.*` command id) with params `p`.
pub fn prepare(e: &Engine, cmd: &str, p: &Value) -> Result<MlJob> {
    let command = match cmd {
        "ml.separate" => "ml.separate",
        "ml.karaoke" => "ml.karaoke",
        "ml.drums" => "ml.drums",
        "ml.clean" => "ml.clean",
        "ml.midi" => "ml.midi",
        "ml.chords" => "ml.chords",
        "ml.lyrics" => "ml.lyrics",
        other => return Err(bad(other, "not an ML command")),
    };
    let mut kind = kind_from(cmd, p)?;
    // Detect Chords' lane options (`mode` is Clean Vocal's own there).
    let lane = if cmd == "ml.chords" { lane_options(cmd, p)? } else { LaneOptions::default() };
    let device = match str_param(p, "device") {
        Some(d) => Device::parse(d).ok_or_else(|| bad(cmd, format!("unknown device `{d}` (auto, gpu, cpu)")))?,
        None => Device::Auto,
    };
    let models_dir = str_param(p, "models_dir").map_or_else(models_dir, PathBuf::from);
    let quality = quality_param(cmd, p)?;
    let s = e.session();
    let rate = s.sample_rate.hz();
    let mut parts = Vec::new();
    let clips = source_clips(e, p);
    if let JobKind::Midi(o) = &mut kind {
        // Convert to MIDI: each clip (or every audio clip of the track), as it plays.
        let ids: Vec<ClipId> = if clips.is_empty() {
            let t = source_track(e, cmd, p)?;
            let Some(tr) = s.track(t) else { return Err(bad(cmd, "no such track")) };
            if tr.kind != TrackKind::Audio {
                return Err(bad(cmd, format!("`{}` is not an audio track", tr.name)));
            }
            tr.clips().iter().filter(|c| c.is_audio()).map(|c| c.id).collect()
        } else {
            clips
        };
        // Without an explicit `isolate`, the General model looks at the source track's name too
        // (a track called bass or vocal transcribes that stem).
        if p.get("isolate").is_none_or(Value::is_null)
            && let Some(name) = ids.first().and_then(|id| s.find_clip(*id)).and_then(|(t, _)| s.track(t)).map(|t| t.name.as_str())
        {
            o.isolate = Isolate::default_for(o.model, Some(name));
        }
        for id in ids {
            let Some((t, c)) = s.find_clip(id).map(|(t, c)| (t, c.clone())) else { return Err(bad(cmd, format!("no clip with id {}", id.0))) };
            if !c.is_audio() {
                return Err(bad(cmd, format!("`{}` is not an audio clip", c.name)));
            }
            let channels = s.track(t).map_or(1, |x| x.channels());
            crate::io::refuse_longer(s, cmd, &c.name, "process", c.length, crate::io::render_limit(channels))?;
            parts.push(Part {
                audio: render_clip(e, t, &c, false),
                target: Target::Midi { track: t, clip: id, start: c.start, len: c.length, name: c.name.clone() },
            });
        }
    } else if let JobKind::Clean(o) = &mut kind {
        // Clean Vocal: each clip (or every audio clip of the track) gets its cleaned audio beside
        // the original; the Amount is the clip's (`clip.clean_mix`), so the job renders all wet.
        let clip_mix = p.get("mix").filter(|v| !v.is_null()).map(|_| o.mix);
        o.mix = 1.0;
        let ids: Vec<ClipId> = if clips.is_empty() {
            let t = source_track(e, cmd, p)?;
            s.track(t).map(|t| t.clips().iter().map(|c| c.id).collect()).unwrap_or_default()
        } else {
            clips
        };
        for id in ids {
            let Some((t, c)) = s.find_clip(id).map(|(t, c)| (t, c.clone())) else { continue };
            if !c.is_audio() {
                continue;
            }
            let channels = s.track(t).map_or(1, |x| x.channels());
            crate::io::refuse_longer(s, cmd, &c.name, "process", c.length, crate::io::render_limit(channels))?;
            parts.push(Part {
                audio: render_clip(e, t, &c, true),
                target: Target::Clip { clip: id, track: t, name: c.name.clone(), was: Box::new(c.clone()), mix: clip_mix },
            });
        }
    } else if let Some(&id) = clips.first() {
        let Some((t, c)) = s.find_clip(id).map(|(t, c)| (t, c.clone())) else { return Err(bad(cmd, format!("no clip with id {}", id.0))) };
        if !c.is_audio() {
            return Err(bad(cmd, "choose an audio clip"));
        }
        let channels = s.track(t).map_or(1, |x| x.channels());
        crate::io::refuse_longer(s, cmd, &c.name, "process", c.length, crate::io::render_limit(channels))?;
        parts.push(Part {
            audio: render_clip(e, t, &c, false),
            target: Target::Track { track: t, start: c.start, len: c.length, name: c.name.clone() },
        });
    } else {
        let t = source_track(e, cmd, p)?;
        let Some(tr) = s.track(t) else { return Err(bad(cmd, "no such track")) };
        if tr.kind != TrackKind::Audio {
            return Err(bad(cmd, format!("`{}` is not an audio track", tr.name)));
        }
        let start = tr.clips().iter().map(|c| c.start).min().unwrap_or(0);
        let end = tr.clips().iter().map(Clip::end).max().unwrap_or(0);
        if end <= start {
            return Err(bad(cmd, format!("`{}` has no audio", tr.name)));
        }
        let range = Range::new(start, end);
        crate::io::refuse_longer(s, cmd, &tr.name, "process", range.len(), crate::io::render_limit(tr.channels().max(2)))?;
        let audio = soundcraft_mix::render_track_pre_fader(s, t, range);
        parts.push(Part { audio, target: Target::Track { track: t, start, len: range.len(), name: tr.name.clone() } });
    }
    if parts.is_empty() {
        return Err(bad(cmd, "select an audio track or clip first"));
    }
    if parts.iter().any(|p| soundcraft_ml::frames(&p.audio) == 0) {
        return Err(bad(cmd, "the source is silent or empty"));
    }
    let keep = p
        .get("stems")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(|s| s.trim().to_ascii_lowercase()).collect())
        .unwrap_or_default();
    let mute_original = bool_or(p, "mute_original", matches!(kind, JobKind::Karaoke { .. }));
    if let JobKind::Chords(o) = &mut kind
        && p.get("bass").is_none()
        && let Some(Target::Track { track, .. }) = parts.first().map(|p| &p.target)
    {
        // The bass guess is only right on a full mix.
        o.bass = s.track(*track).is_some_and(looks_like_full_mix);
    }
    Ok(MlJob { command, kind, parts, rate, device, models_dir, quality, keep, mute_original, lane })
}

/// `quality`: fast (default: one HTDemucs pass) | high (shift averaging).
pub fn quality_param(cmd: &str, p: &Value) -> Result<Quality> {
    match str_param(p, "quality") {
        Some(q) => Quality::parse(q).ok_or_else(|| bad(cmd, format!("unknown quality `{q}` (fast, high)"))),
        None => Ok(Quality::Fast),
    }
}

impl MlJob {
    /// What the progress window calls the job.
    pub fn title(&self) -> String {
        let name = match self.parts.as_slice() {
            [p] => match &p.target {
                Target::Track { name, .. } | Target::Clip { name, .. } | Target::Midi { name, .. } => name.clone(),
            },
            more => format!("{} clips", more.len()),
        };
        format!("{} — {name}", self.kind.title())
    }

    /// Step 2 (worker thread): run the models. The parts' audio is moved out (the job keeps its
    /// targets for [`apply`]). Errors are messages for the person; a cancel is honoured up to the
    /// very end, so a cancelled job never returns results.
    pub fn run(&mut self, ctx: &Ctx) -> std::result::Result<MlResult, MlError> {
        let n = self.parts.len().max(1);
        let mut out = Vec::with_capacity(n);
        for (i, part) in self.parts.iter_mut().enumerate() {
            ctx.check()?;
            let job = soundcraft_ml::Job {
                kind: self.kind,
                audio: std::mem::take(&mut part.audio),
                rate: self.rate,
                device: self.device,
                models_dir: self.models_dir.clone(),
                quality: self.quality,
            };
            out.push(soundcraft_ml::pipeline::run(&job, &ctx.stage(i as f32 / n as f32, (i + 1) as f32 / n as f32))?);
        }
        ctx.check()?;
        Ok(MlResult { parts: out })
    }

    /// Convert to MIDI's Analyse (worker thread): the isolation and the network per part, as
    /// analyses to decode as often as wanted ([`Analysed::decode`]); the parts' audio is moved
    /// out. A cancel is honoured to the end.
    pub fn analyse(&mut self, ctx: &Ctx) -> std::result::Result<Vec<(Analysed, Vec<String>)>, MlError> {
        let n = self.parts.len().max(1);
        let mut out = Vec::with_capacity(n);
        for (i, part) in self.parts.iter_mut().enumerate() {
            ctx.check()?;
            let job = soundcraft_ml::Job {
                kind: self.kind,
                audio: std::mem::take(&mut part.audio),
                rate: self.rate,
                device: self.device,
                models_dir: self.models_dir.clone(),
                quality: self.quality,
            };
            out.push(soundcraft_ml::pipeline::analyse_midi(&job, &ctx.stage(i as f32 / n as f32, (i + 1) as f32 / n as f32))?);
        }
        ctx.check()?;
        Ok(out)
    }

    /// This job without its source audio (what the UI keeps while a worker runs a copy).
    pub fn without_audio(&self) -> MlJob {
        MlJob {
            parts: self.parts.iter().map(|p| Part { target: p.target.clone(), audio: Vec::new() }).collect(),
            keep: self.keep.clone(),
            models_dir: self.models_dir.clone(),
            ..*self
        }
    }
}

/// Step 3 (UI thread): put `result` into the session as one undo step named after the command.
pub fn apply(e: &mut Engine, job: &MlJob, result: MlResult) -> Result<Value> {
    let before = e.session_arc();
    let dirty = e.dirty;
    match apply_inner(e, job, result) {
        Ok(v) => {
            let label = crate::find_command(job.command).map_or("ML", |c| c.label);
            e.push_undo(label, before);
            Ok(v)
        }
        Err(err) => {
            // No half-applied results.
            if !std::sync::Arc::ptr_eq(&before, &e.doc) {
                e.doc = before;
                e.revision = e.revision.wrapping_add(1);
                e.dirty = dirty;
            }
            Err(err)
        }
    }
}

/// The session edit of [`apply`], without the undo step (the `ml.*` commands get theirs from
/// `Engine::execute`).
pub(crate) fn apply_inner(e: &mut Engine, job: &MlJob, result: MlResult) -> Result<Value> {
    let cmd = job.command;
    let rate = e.session().sample_rate.hz();
    if rate != job.rate {
        return Err(bad(cmd, "the session's sample rate changed while the job ran"));
    }
    let mut made = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    // Convert to MIDI: the new instrument track of each source track.
    let mut midi_tracks: Vec<(TrackId, TrackId)> = Vec::new();
    for (part, out) in job.parts.iter().zip(result.parts) {
        for n in out.notes {
            if !notes.contains(&n) {
                notes.push(n);
            }
        }
        match &part.target {
            Target::Track { track, start, len, name } if matches!(job.kind, JobKind::Chords(_) | JobKind::Lyrics(_)) => {
                let v = apply_text(e, job, (*track, *start, *len, name), out.chords.as_ref(), out.lyrics.as_ref())?;
                made.push(v);
            }
            Target::Track { track, start, len, name } => {
                let s = e.session_mut();
                let Some(src) = s.track(*track).cloned() else { return Err(bad(cmd, format!("the track `{name}` is gone"))) };
                let mut at = s.track_index(*track).unwrap_or(s.tracks.len());
                for stem in out.stems {
                    if !job.keep.is_empty()
                        && !job.keep.iter().any(|k| stem.name.to_ascii_lowercase() == *k || stem.name.to_ascii_lowercase().replace('-', "") == *k)
                    {
                        continue;
                    }
                    let channels = stem.audio.len().clamp(1, 2);
                    let frames = soundcraft_ml::frames(&stem.audio);
                    let file = format!("{name} - {}", stem.name);
                    let buf = AudioBuffer { sample_rate: rate, channels: stem.audio };
                    let source = crate::io::add_source(s, &file, buf, None, FileFormat::Wav);
                    let track_name = format!("{} {}", src.name, stem.name);
                    let nt = s.add_track(TrackKind::Audio, ChannelFormat::for_channels(channels), Some(&track_name));
                    // Directly under the source (and the results placed before this one).
                    if let Some(i) = s.track_index(nt) {
                        let t = s.tracks.remove(i);
                        at = (at + 1).min(s.tracks.len());
                        s.tracks.insert(at, t);
                    }
                    let clip_len = Samples::try_from(frames).unwrap_or(*len).min(*len);
                    let cid = s.new_clip_id();
                    if let Some(t) = s.track_mut(nt) {
                        t.mixer.volume_db = src.mixer.volume_db;
                        t.mixer.output = src.mixer.output.clone();
                        t.mixer.sends = src.mixer.sends.clone();
                        // Pan as the source (not its automation): a panned mono source stays panned.
                        if t.mixer.pan.len() == src.mixer.pan.len() {
                            t.mixer.pan = src.mixer.pan.clone();
                        }
                        if let Some(pl) = t.playlist_mut() {
                            pl.clips.push(Clip::audio(cid, file.clone(), source, 0, *start, clip_len));
                        }
                    }
                    made.push(json!({"track": nt, "name": s.track(nt).map(|t| t.name.clone()), "clip": cid, "source": source}));
                }
                if job.mute_original
                    && let Some(t) = s.track_mut(*track)
                {
                    t.mixer.mute = true;
                }
            }
            Target::Clip { clip, track, name, was, mix } => {
                let Some(old) = e.session().find_clip(*clip).map(|(_, c)| c.clone()) else {
                    return Err(bad(cmd, format!("the clip `{name}` is gone")));
                };
                if (old.start, old.length, &old.content, old.stretch) != (was.start, was.length, &was.content, was.stretch) {
                    return Err(bad(cmd, format!("the clip `{name}` was moved, trimmed or changed while the job ran; run it again")));
                }
                let Some(stem) = out.stems.into_iter().next() else { continue };
                // A stretched clip: the job ran on its stretched sound, so that (dry) becomes the
                // clip's original, unstretched, and the cleaned audio sits beside it.
                let stretched = (old.stretch - 1.0).abs() > 1e-9;
                let dry = if stretched { Some(render_clip(e, *track, &old, true)) } else { None };
                let s = e.session_mut();
                let width = s.track(*track).map_or(1, |t| t.channels().max(1));
                let audio = soundcraft_ml::to_channels(&stem.audio, width);
                let frames = soundcraft_ml::frames(&audio);
                let clean_name = format!("{name}-clean");
                let source = crate::io::add_source(s, &clean_name, AudioBuffer { sample_rate: rate, channels: audio }, None, FileFormat::Wav);
                let mut nc = old.clone();
                let keep = old.clean_alt().copied();
                if let Some(dry) = dry {
                    let dry_src =
                        crate::io::add_source(s, &format!("{name}-dry"), AudioBuffer { sample_rate: rate, channels: dry }, None, FileFormat::Wav);
                    nc.content = ClipContent::Audio { source: dry_src, offset: 0 };
                    // The new audio starts where the clip did: its lyrics move into the new source's time.
                    soundcraft_model::rebase_lyrics(&mut nc, old.source_offset());
                    nc.length = Samples::try_from(frames).unwrap_or(old.length);
                    nc.stretch = 1.0;
                }
                let (Some(original), origin) = (nc.source(), nc.source_offset()) else { continue };
                nc.clean = Some(CleanAlt { source, original, origin, mix: mix.or(keep.map(|k| k.amount())).unwrap_or(1.0), bypass: false });
                if let Some(c) = s.find_clip_mut(*clip) {
                    *c = nc;
                }
                made.push(json!({"clip": clip, "source": source, "original": original}));
            }
            Target::Midi { track, clip: _, start, len, name } => {
                let JobKind::Midi(opts) = job.kind else { return Err(bad(cmd, "not a MIDI job")) };
                let t = out.midi.unwrap_or_default();
                let s = e.session_mut();
                let Some(src) = s.track(*track).cloned() else { return Err(bad(cmd, format!("the track of `{name}` is gone"))) };
                let nt = match midi_tracks.iter().find(|(a, _)| a == track) {
                    Some((_, b)) => *b,
                    None => {
                        let nt = new_midi_track(s, &src, opts.model);
                        midi_tracks.push((*track, nt));
                        nt
                    }
                };
                let seq = midi_sequence(s, *start, *len, &t);
                let n_notes = seq.notes.len();
                let cid = s.new_clip_id();
                crate::edit::place_clip(s, nt, Clip::midi(cid, format!("{name} MIDI"), *start, *len, seq));
                made.push(json!({"track": nt, "name": s.track(nt).map(|t| t.name.clone()), "clip": cid, "notes": n_notes, "pedals": t.pedals.len()}));
            }
        }
    }
    Ok(json!({"command": cmd, "made": made, "notes": notes}))
}

/// The subtractive synth voiced as soft keys (the built-in instruments have no piano): triangle
/// and sine an octave up, no sustain level (notes ring out), a gentle filter envelope.
const KEYS_VOICE: [(&str, f32); 12] = [
    ("osc1_wave", 3.0),
    ("osc2_wave", 2.0),
    ("osc2_semitones", 12.0),
    ("osc2_detune", 0.0),
    ("osc_mix", 30.0),
    ("cutoff", 3500.0),
    ("filter_env_amount", 35.0),
    ("amp_attack", 2.0),
    ("amp_decay", 2500.0),
    ("amp_sustain", 0.0),
    ("amp_release", 450.0),
    ("velocity", 90.0),
];

/// What a Convert to MIDI audition plays: the source alone, the notes alone, or both (the source
/// at [`AUDITION_SOURCE_DB`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuditionMix {
    Source,
    Notes,
    #[default]
    Both,
}

impl AuditionMix {
    pub const ALL: [AuditionMix; 3] = [AuditionMix::Source, AuditionMix::Notes, AuditionMix::Both];

    pub fn label(self) -> &'static str {
        match self {
            AuditionMix::Source => "Source",
            AuditionMix::Notes => "Notes",
            AuditionMix::Both => "Both",
        }
    }
}

/// The source's level under the notes in an audition, dB.
pub const AUDITION_SOURCE_DB: f32 = -12.0;

/// A copy of `s` for auditioning `decoded` (one transcription per Convert to MIDI part of `job`):
/// the notes on the instrument track Convert would make (one per source track, placed as
/// [`apply`] places them), that track and the source soloed, the source at
/// [`AUDITION_SOURCE_DB`] (`Both`), muted (`Notes`), or the notes muted (`Source`). Returns the
/// copy and the range the parts cover; `None` when a source is gone or the job is not MIDI.
pub fn audition_session(
    s: &soundcraft_model::Session,
    job: &MlJob,
    decoded: &[Transcription],
    mix: AuditionMix,
) -> Option<(soundcraft_model::Session, Range)> {
    let JobKind::Midi(opts) = job.kind else { return None };
    let mut s = s.clone();
    let mut midi_tracks: Vec<(TrackId, TrackId)> = Vec::new();
    let mut sources: Vec<TrackId> = Vec::new();
    let mut range: Option<Range> = None;
    for (part, t) in job.parts.iter().zip(decoded) {
        let Target::Midi { track, start, len, name, .. } = &part.target else { continue };
        let src = s.track(*track)?.clone();
        let nt = match midi_tracks.iter().find(|(a, _)| a == track) {
            Some((_, b)) => *b,
            None => {
                let nt = new_midi_track(&mut s, &src, opts.model);
                midi_tracks.push((*track, nt));
                nt
            }
        };
        let seq = midi_sequence(&s, *start, *len, t);
        let cid = s.new_clip_id();
        crate::edit::place_clip(&mut s, nt, Clip::midi(cid, format!("{name} MIDI"), *start, *len, seq));
        let r = Range::new(*start, start.saturating_add(*len));
        range = Some(range.map_or(r, |x| Range::new(x.start.min(r.start), x.end.max(r.end))));
        if !sources.contains(track) {
            sources.push(*track);
        }
    }
    let range = range?;
    for t in &mut s.tracks {
        let is_src = sources.contains(&t.id);
        let is_notes = midi_tracks.iter().any(|(_, m)| *m == t.id);
        t.mixer.solo = is_src || is_notes;
        if is_src {
            match mix {
                AuditionMix::Source => {}
                AuditionMix::Notes => t.mixer.mute = true,
                AuditionMix::Both => t.mixer.volume_db = AUDITION_SOURCE_DB,
            }
        }
        if is_notes && mix == AuditionMix::Source {
            t.mixer.mute = true;
        }
    }
    Some((s, range))
}

/// Convert to MIDI's commit of an audition: `decoded` (one per part of `job`) into the session as
/// one undo step, exactly as the job's own result would land.
pub fn commit_midi(e: &mut Engine, job: &MlJob, decoded: Vec<Transcription>, notes: Vec<String>) -> Result<Value> {
    let parts = decoded.into_iter().map(|t| JobOutput { midi: Some(t), notes: notes.clone(), ..JobOutput::default() }).collect();
    apply(e, job, MlResult { parts })
}

/// A new instrument track `<source> MIDI` directly under `src`, playing the default instrument
/// (drums: the Drum Synth; piano: the synth voiced as keys), sent where the source goes.
pub fn new_midi_track(s: &mut soundcraft_model::Session, src: &soundcraft_model::Track, model: MidiModel) -> TrackId {
    let name = format!("{} MIDI", src.name);
    let nt = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(&name));
    if let (Some(i), Some(at)) = (s.track_index(nt), s.track_index(src.id)) {
        let t = s.tracks.remove(i);
        let at = if i <= at { at } else { at + 1 };
        s.tracks.insert(at.min(s.tracks.len()), t);
    }
    let plugin = if model == MidiModel::Drums { "drum_synth" } else { "subtractive_synth" };
    let mut ins = soundcraft_model::Insert::new(plugin);
    if let Some(info) = soundcraft_dsp::plugin_info(plugin) {
        for pi in info.params {
            ins.params.insert(pi.id.to_string(), pi.default);
        }
    }
    if model == MidiModel::Piano {
        for (k, v) in KEYS_VOICE {
            ins.params.insert(k.to_string(), v);
        }
        ins.preset = "Keys".into();
    }
    if let Some(t) = s.track_mut(nt) {
        t.instrument = Some(ins);
        t.mixer.output = src.mixer.output.clone();
    }
    nt
}

/// `t` (seconds from `start`) as a MIDI clip's sequence: ticks relative to the clip start on the
/// session's tempo map, so notes land where they sound across tempo changes. Notes past the clip
/// end are dropped (cut at it); the sustain pedal is CC64, pitch bends the wheel (centred again
/// before any note that has no bend of its own). Drums play on channel 10.
pub fn midi_sequence(s: &soundcraft_model::Session, start: Samples, len: Samples, t: &Transcription) -> soundcraft_midi::Sequence {
    use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence};
    let sr = s.sample_rate;
    let len = len.max(1);
    let end = start.saturating_add(len);
    let base = s.tempo.samples_to_ticks(start, sr);
    let last = s.tempo.samples_to_ticks(end, sr).saturating_sub(base).max(1);
    let tick = |secs: f64| -> i64 {
        let off = if secs.is_finite() { (secs * sr.as_f64()).round().clamp(0.0, len as f64) as Samples } else { 0 };
        s.tempo.samples_to_ticks(start.saturating_add(off), sr).saturating_sub(base)
    };
    let channel = if t.drums { 9 } else { 0 };
    let mut seq = Sequence::default();
    for n in &t.notes {
        let a = tick(n.start);
        if a >= last {
            continue;
        }
        let b = tick(n.end).clamp(a + 1, last);
        seq.notes.push(Note { pitch: n.pitch.min(127), velocity: n.velocity.clamp(1, 127), release_velocity: 64, channel, start: a, length: b - a });
    }
    trim_same_pitch_overlaps(&mut seq.notes);
    for (a, b) in &t.pedals {
        let a = tick(*a);
        if a >= last {
            continue;
        }
        // Up again before the clip ends, so the pedal never hangs past it.
        let b = tick(*b).clamp(a, last - 1);
        seq.ctrls.push(CtrlEvent { tick: a, channel, kind: CtrlKind::Cc(64), value: 127 });
        seq.ctrls.push(CtrlEvent { tick: b, channel, kind: CtrlKind::Cc(64), value: 0 });
    }
    if !t.bends.is_empty() {
        let mut bends: Vec<(i64, i32)> = t.bends.iter().map(|(at, v)| (tick(*at), (*v).clamp(-8192, 8191))).filter(|(at, _)| *at < last).collect();
        bends.sort_by_key(|b| b.0);
        // Centre the wheel for notes that start while it is bent and bring no bend of their own.
        let mut extra = Vec::new();
        let (mut wheel, mut bi) = (0, 0);
        let mut starts: Vec<i64> = seq.notes.iter().map(|n| n.start).collect();
        starts.sort_unstable();
        starts.dedup();
        for at in starts {
            while let Some(&(bt, v)) = bends.get(bi) {
                if bt >= at {
                    break;
                }
                wheel = v;
                bi += 1;
            }
            let own = bends.get(bi).is_some_and(|(bt, _)| *bt == at);
            if wheel != 0 && !own {
                extra.push((at, 0));
                wheel = 0;
            }
        }
        // ... and at the clip's end.
        if bends.last().is_some_and(|b| b.1 != 0) {
            extra.push((last - 1, 0));
        }
        bends.extend(extra);
        bends.sort_by_key(|b| b.0);
        seq.ctrls.extend(bends.into_iter().map(|(tick, value)| CtrlEvent { tick, channel, kind: CtrlKind::PitchBend, value }));
    }
    seq.sort();
    seq
}

/// Ends each note no later than the next note of the same pitch and channel starts: in a MIDI
/// file the first note-off of a pitch ends whichever note of that pitch is sounding, so an
/// overlapping pair would cut the later note short. Notes starting on the same tick as an
/// earlier one of the same pitch are dropped (they would be the same key pressed twice at once).
fn trim_same_pitch_overlaps(notes: &mut Vec<soundcraft_midi::Note>) {
    notes.sort_by_key(|n| (n.channel, n.pitch, n.start));
    let mut out: Vec<soundcraft_midi::Note> = Vec::with_capacity(notes.len());
    for n in notes.drain(..) {
        if let Some(prev) = out.last_mut().filter(|p| p.channel == n.channel && p.pitch == n.pitch) {
            if prev.start == n.start {
                prev.length = prev.length.max(n.length);
                continue;
            }
            prev.length = prev.length.min(n.start - prev.start).max(1);
        }
        out.push(n);
    }
    out.sort_by_key(|n| (n.start, n.channel, n.pitch));
    *notes = out;
}

/// Detect Chords' lane options: `grid` (half_bar | bar), `mode` (replace | fill), `key`, `scale`.
pub fn lane_options(cmd: &str, p: &Value) -> Result<LaneOptions> {
    let grid = match str_param(p, "grid") {
        Some(g) => Grid::parse(g).ok_or_else(|| bad(cmd, format!("unknown grid `{g}` (half_bar, bar, free)")))?,
        None => Grid::HalfBar,
    };
    let replace = match str_param(p, "mode") {
        None | Some("replace") => true,
        Some("fill") | Some("fill_empty") => false,
        Some(m) => return Err(bad(cmd, format!("unknown mode `{m}` (replace, fill)"))),
    };
    let key = crate::cmd::chord_lane::key_choice(p).map_err(|m| bad(cmd, m))?;
    Ok(LaneOptions { grid, replace, key })
}

/// Transcribe Lyrics' options: `language` (auto | en), `prompt`.
pub fn lyrics_options(cmd: &str, p: &Value) -> Result<LyricsOptions> {
    let language = match str_param(p, "language") {
        Some(l) => LyricsLanguage::parse(l).ok_or_else(|| bad(cmd, format!("unknown language `{l}` (auto, en)")))?,
        None => LyricsLanguage::English,
    };
    let prompt = match p.get("prompt") {
        None | Some(Value::Null) => Prompt::default(),
        Some(Value::String(s)) => Prompt::new(s),
        Some(_) => return Err(bad(cmd, "`prompt` must be text")),
    };
    let quality = match str_param(p, "quality") {
        Some(q) => soundcraft_ml::lyrics::LyricsQuality::parse(q).ok_or_else(|| bad(cmd, format!("unknown quality `{q}` (fast, best)")))?,
        None => soundcraft_ml::lyrics::LyricsQuality::Fast,
    };
    Ok(LyricsOptions { language, prompt, quality, skip_silence: bool_or(p, "skip_silence", false) })
}

/// Words that name one instrument or voice: a track named so is a stem, not a full mix.
const STEM_WORDS: [&str; 28] = [
    "vocal", "vox", "voice", "sing", "lead", "bv", "choir", "bass", "drum", "kick", "snare", "tom", "hat", "cymbal", "overhead", "perc", "guitar",
    "gtr", "piano", "keys", "organ", "synth", "pad", "string", "violin", "cello", "horn", "brass",
];

/// Whether a track looks like a full mix (Detect Chords' bass guess defaults on): stereo, and not
/// named after one instrument or voice.
pub fn looks_like_full_mix(t: &soundcraft_model::Track) -> bool {
    let n = t.name.to_ascii_lowercase();
    t.channels() >= 2 && !STEM_WORDS.iter().any(|w| n.contains(w))
}

/// Detect Chords and Transcribe Lyrics results into the session: chords onto the Band's chord
/// lane, words onto the source track's lyrics, at the source's position.
fn apply_text(
    e: &mut Engine,
    job: &MlJob,
    src: (TrackId, Samples, Samples, &str),
    chords: Option<&Vec<ChordSegment>>,
    lyrics: Option<&Lyrics>,
) -> Result<Value> {
    let (track, start, len, name) = src;
    let cmd = job.command;
    if e.session().track(track).is_none() {
        return Err(bad(cmd, format!("the track `{name}` is gone")));
    }
    if let Some(segs) = chords {
        let chords: Vec<crate::cmd::chord_lane::TimedChord> = segs
            .iter()
            .filter(|s| s.is_chord())
            .map(|s| crate::cmd::chord_lane::TimedChord { start: s.start, end: s.end, name: s.name.clone() })
            .collect();
        let mut v = crate::cmd::chord_lane::place_chords(e.session_mut(), start, len, &chords, &job.lane).map_err(|m| bad(cmd, m))?;
        crate::cmd::chord_lane::show_rulers(e);
        if let Some(o) = v.as_object_mut() {
            o.insert("segments".into(), json!(segs.len()));
            o.insert("bass".into(), json!(matches!(job.kind, JobKind::Chords(c) if c.bass)));
        }
        return Ok(v);
    }
    let lyrics = lyrics.cloned().unwrap_or_default();
    // One line per whisper.cpp segment, holding its kept words (each word in the first line
    // that starts at or before it).
    let mut lines: Vec<Vec<crate::lyrics::TimedWord>> = vec![Vec::new(); lyrics.segments.len()];
    for w in &lyrics.words {
        let i = lyrics.segments.iter().rposition(|l| l.start <= w.start + 1e-9).unwrap_or(0);
        if let Some(line) = lines.get_mut(i) {
            line.push(crate::lyrics::TimedWord { start: w.start, end: w.end, text: w.text.clone() });
        }
    }
    let (n_lines, n_words) = crate::lyrics::place_lyrics(e.session_mut(), track, start, len, &lines);
    Ok(json!({"track": track, "lines": n_lines, "words": n_words, "language": lyrics.language,
              "text": lyrics.segments.iter().map(|l| l.text.clone()).collect::<Vec<_>>()}))
}

/// The `ml.*` command: prepare, run (blocking) and apply.
pub(crate) fn run_command(e: &mut Engine, cmd: &str, p: &Value) -> Result<Value> {
    let mut job = prepare(e, cmd, p)?;
    let t0 = std::time::Instant::now();
    let result = job.run(&Ctx::default()).map_err(|err| ml_err(cmd, err))?;
    let secs = t0.elapsed().as_secs_f64();
    let mut v = apply_inner(e, &job, result)?;
    if let Some(o) = v.as_object_mut() {
        o.insert("seconds".into(), json!((secs * 100.0).round() / 100.0));
    }
    Ok(v)
}

/// `ml.models`: the model folder and every model's status.
pub(crate) fn models_command(_: &mut Engine, p: &Value) -> Result<Value> {
    let dir = str_param(p, "models_dir").map_or_else(models_dir, PathBuf::from);
    let rows: Vec<Value> = list_models(&dir)
        .into_iter()
        .map(|m| {
            let (status, why, license, source) = match &m.status {
                ModelStatus::Installed(c) => ("installed", String::new(), c.license.clone(), c.source.clone()),
                ModelStatus::Builtin => {
                    ("built in", String::new(), "MIT OR Apache-2.0".to_string(), "https://github.com/Rikorose/DeepFilterNet".to_string())
                }
                ModelStatus::Missing => ("not installed", String::new(), String::new(), String::new()),
                ModelStatus::Broken(w) => ("broken", w.clone(), String::new(), String::new()),
            };
            json!({"id": m.model.id, "name": m.model.name, "job": m.model.job, "status": status, "problem": why, "bytes": m.size,
                   "license": license, "licence_status": m.licence(), "source": source, "folder": m.dir})
        })
        .collect();
    Ok(json!({"folder": dir, "env": soundcraft_ml::models::MODELS_DIR_ENV, "install": "cargo xtask ml-models", "models": rows}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_ml::NoteEvent;

    #[test]
    fn an_audition_plays_exactly_what_convert_commits() {
        let mut e = crate::demo::demo_engine();
        let clip = e.session().tracks.iter().find(|t| t.name == "Bass").and_then(|t| t.clips().first()).map(|c| c.id).unwrap();
        let job = prepare(&e, "ml.midi", &json!({"clip": clip, "model": "general", "isolate": "none"})).unwrap().without_audio();
        let n = |s: f64, p: u8| NoteEvent { start: s, end: s + 0.4, pitch: p, velocity: 90, bends: Vec::new() };
        let t = Transcription { notes: vec![n(0.0, 40), n(0.5, 43), n(1.0, 47)], ..Default::default() };
        let (pv, range) = audition_session(e.session(), &job, std::slice::from_ref(&t), AuditionMix::Both).unwrap();
        let bass = pv.tracks.iter().find(|t| t.name == "Bass").unwrap();
        let notes = pv.tracks.iter().find(|t| t.name == "Bass MIDI").unwrap();
        assert!(bass.mixer.solo && notes.mixer.solo && !bass.mixer.mute);
        assert_eq!(bass.mixer.volume_db, AUDITION_SOURCE_DB);
        let previewed = notes.clips().first().and_then(|c| c.midi_playing().cloned()).unwrap();
        assert_eq!(range, Range::new(previewed_start(&pv), previewed_start(&pv) + e.session().find_clip(clip).unwrap().1.length));
        let (muted, _) = audition_session(e.session(), &job, std::slice::from_ref(&t), AuditionMix::Notes).unwrap();
        assert!(muted.tracks.iter().find(|t| t.name == "Bass").unwrap().mixer.mute);
        let (src_only, _) = audition_session(e.session(), &job, std::slice::from_ref(&t), AuditionMix::Source).unwrap();
        assert!(src_only.tracks.iter().find(|t| t.name == "Bass MIDI").unwrap().mixer.mute);
        // The document is untouched until Convert; then the clip holds the previewed notes.
        assert!(e.session().tracks.iter().all(|t| t.name != "Bass MIDI"));
        let v = commit_midi(&mut e, &job, vec![t], vec!["CPU".into()]).unwrap();
        assert_eq!(v["made"][0]["notes"], json!(3));
        let made = e.session().tracks.iter().find(|t| t.name == "Bass MIDI").unwrap();
        assert_eq!(made.clips().first().and_then(|c| c.midi_playing().cloned()), Some(previewed));
        assert_eq!(e.undo_label(), Some("Convert to MIDI"));
    }

    fn previewed_start(s: &soundcraft_model::Session) -> Samples {
        s.tracks.iter().find(|t| t.name == "Bass MIDI").and_then(|t| t.clips().first()).map_or(0, |c| c.start)
    }
}
