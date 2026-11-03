//! Audio → MIDI transcription: the shared note types, the options, and the job that runs one of
//! the three transcribers on audio at any rate.
//!
//! - **General** ([`crate::basicpitch`], Spotify Basic Pitch): polyphonic and instrument-agnostic,
//!   with optional pitch bends.
//! - **Piano** ([`crate::pianokong`], ByteDance high-resolution piano transcription, Kong et al.):
//!   notes with velocities and the sustain pedal.
//! - **Drums** ([`crate::adtof`], ADTOF Frame_RNN): kick, snare, toms, hi-hat and cymbals as GM drum
//!   notes.
//! - **Vocal** ([`crate::game`], openvpi GAME small): a singing voice (or any one-note-at-a-time
//!   melody), with the fractional pitch as an optional pitch bend.
//!
//! Every runner returns notes in seconds from the start of its input; the engine places them on
//! the session's tempo map.

use crate::job::Ctx;
use crate::models::InstalledModel;
use crate::runtime::Device;
use crate::{MlError, Result};
use serde::{Deserialize, Serialize};

/// Which transcriber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MidiModel {
    /// Basic Pitch: any pitched instrument or voice.
    #[default]
    General,
    /// Kong et al. piano transcription: notes and sustain pedal.
    Piano,
    /// ADTOF: a drum kit.
    Drums,
    /// GAME: a singing voice.
    Vocal,
}

impl MidiModel {
    pub const ALL: [MidiModel; 4] = [MidiModel::General, MidiModel::Piano, MidiModel::Drums, MidiModel::Vocal];

    /// `general`, `piano` or `drums` (also the model ids and a few spellings).
    pub fn parse(s: &str) -> Option<MidiModel> {
        match s.trim().to_ascii_lowercase().as_str() {
            "general" | "basicpitch" | "basic_pitch" | "basic-pitch" | "" => Some(MidiModel::General),
            "piano" | "piano_kong" | "pianokong" | "kong" => Some(MidiModel::Piano),
            "drums" | "drum" | "adtof" | "adtof_drums" => Some(MidiModel::Drums),
            "vocal" | "vocals" | "voice" | "game" | "game_vocal" => Some(MidiModel::Vocal),
            _ => None,
        }
    }

    /// The command / CLI name.
    pub fn id(self) -> &'static str {
        match self {
            MidiModel::General => "general",
            MidiModel::Piano => "piano",
            MidiModel::Drums => "drums",
            MidiModel::Vocal => "vocal",
        }
    }

    /// The model folder id.
    pub fn model_id(self) -> &'static str {
        match self {
            MidiModel::General => crate::basicpitch::ID,
            MidiModel::Piano => crate::pianokong::ID,
            MidiModel::Drums => crate::adtof::ID,
            MidiModel::Vocal => crate::game::ID,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MidiModel::General => "General (Basic Pitch)",
            MidiModel::Piano => "Piano (with pedal)",
            MidiModel::Drums => "Drums",
            MidiModel::Vocal => "Vocal (singing voice)",
        }
    }

    /// The sample rate the model listens at.
    pub fn sample_rate(self) -> u32 {
        match self {
            MidiModel::General => crate::basicpitch::SR,
            MidiModel::Piano => crate::pianokong::SR,
            MidiModel::Drums => crate::adtof::SR,
            MidiModel::Vocal => crate::game::SR,
        }
    }

    /// The onset threshold the model's own decoder uses (Drums: a scale for its five per-drum
    /// thresholds; this value keeps them as published).
    pub fn default_onset(self) -> f32 {
        match self {
            MidiModel::General => crate::basicpitch::ONSET_THRESHOLD,
            MidiModel::Piano => crate::pianokong::ONSET_THRESHOLD,
            MidiModel::Drums => crate::adtof::ONSET_REFERENCE,
            // GAME has no onset threshold of its own.
            MidiModel::Vocal => 0.5,
        }
    }

    /// The shortest note kept by default, ms (General: Basic Pitch's 11 frames).
    pub fn default_min_note_ms(self) -> f32 {
        match self {
            MidiModel::General => crate::basicpitch::DEFAULT_MIN_NOTE_MS,
            MidiModel::Piano | MidiModel::Drums | MidiModel::Vocal => 0.0,
        }
    }
}

/// Which stem to transcribe instead of the source as it is: a piano model fed a band mix hears
/// bass, guitar and voice as keys, so Convert to MIDI can separate the instrument out first, from
/// the best installed source ([`crate::pipeline::isolation_sources`]: vocals from Mel-Band
/// RoFormer, piano, drums and bass from BS-RoFormer SW, else HTDemucs at the job's quality).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Isolate {
    /// The source as it is.
    #[default]
    None,
    Drums,
    /// Everything but drums, bass and vocals: piano, keys, guitars (HTDemucs' other).
    Other,
    Vocals,
    Bass,
    /// BS-RoFormer SW's piano stem (HTDemucs' other when SW is not installed).
    Piano,
}

impl Isolate {
    pub const ALL: [Isolate; 6] = [Isolate::None, Isolate::Drums, Isolate::Piano, Isolate::Other, Isolate::Vocals, Isolate::Bass];

    pub fn parse(s: &str) -> Option<Isolate> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "no" | "off" | "" => Some(Isolate::None),
            "drums" | "drum" => Some(Isolate::Drums),
            "piano" | "keys" => Some(Isolate::Piano),
            "other" => Some(Isolate::Other),
            "vocals" | "vocal" | "voice" => Some(Isolate::Vocals),
            "bass" => Some(Isolate::Bass),
            _ => None,
        }
    }

    /// The command / CLI name (and the stem name, except `none`).
    pub fn id(self) -> &'static str {
        match self {
            Isolate::None => "none",
            Isolate::Drums => "drums",
            Isolate::Other => "other",
            Isolate::Vocals => "vocals",
            Isolate::Bass => "bass",
            Isolate::Piano => "piano",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Isolate::None => "No (the source as it is)",
            Isolate::Drums => "Drums stem",
            Isolate::Other => "Other stem (piano, keys, guitars)",
            Isolate::Vocals => "Vocals stem",
            Isolate::Bass => "Bass stem",
            Isolate::Piano => "Piano stem",
        }
    }

    /// The stem to separate out, if any.
    pub fn stem(self) -> Option<&'static str> {
        (self != Isolate::None).then_some(self.id())
    }

    /// The default for `model` on a source named `track`: Drums → the drums stem, Piano → piano,
    /// Vocal → vocals, General → none unless the name says bass or vocal.
    pub fn default_for(model: MidiModel, track: Option<&str>) -> Isolate {
        match model {
            MidiModel::Drums => Isolate::Drums,
            MidiModel::Piano => Isolate::Piano,
            MidiModel::Vocal => Isolate::Vocals,
            MidiModel::General => {
                let n = track.unwrap_or_default().to_ascii_lowercase();
                if n.contains("bass") {
                    Isolate::Bass
                } else if ["vocal", "vox", "voice", "sing"].iter().any(|w| n.contains(w)) {
                    Isolate::Vocals
                } else {
                    Isolate::None
                }
            }
        }
    }
}

/// How to transcribe. `None` = the model's own default.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct MidiOptions {
    pub model: MidiModel,
    /// Onset threshold 0..1 (lower = more notes). Drums: scales the five per-drum thresholds
    /// (the default keeps them as published).
    #[serde(default)]
    pub onset_threshold: Option<f32>,
    /// General only: the note-activation threshold that keeps a note going (default 0.3).
    #[serde(default)]
    pub frame_threshold: Option<f32>,
    /// Shortest note kept, ms.
    #[serde(default)]
    pub min_note_ms: Option<f32>,
    /// General: pitch bends (on notes that overlap no other note, as Basic Pitch writes them).
    /// Vocal: each note's fractional pitch as a bend at its start.
    #[serde(default)]
    pub pitch_bends: bool,
    /// Piano only: the sustain pedal (CC64).
    #[serde(default = "yes")]
    pub pedal: bool,
    /// Drums only: one threshold per class (kick, snare, toms, hi-hat, cymbals); `None` = the
    /// published five scaled by `onset_threshold`.
    #[serde(default)]
    pub drum_thresholds: Option<[f32; 5]>,
    /// Separate this stem out of the source first and transcribe that.
    #[serde(default)]
    pub isolate: Isolate,
}

fn yes() -> bool {
    true
}

impl MidiOptions {
    /// The defaults for `model`: bends off, pedal on, the model's isolation
    /// ([`Isolate::default_for`] without a track name).
    pub fn new(model: MidiModel) -> Self {
        MidiOptions {
            model,
            onset_threshold: None,
            frame_threshold: None,
            min_note_ms: None,
            pitch_bends: false,
            pedal: true,
            drum_thresholds: None,
            isolate: Isolate::default_for(model, None),
        }
    }

    /// Drums: the five per-class thresholds in use.
    pub fn drum_thresholds(&self) -> [f32; 5] {
        match self.drum_thresholds {
            Some(t) if t.iter().all(|v| v.is_finite()) => t.map(|v| v.clamp(0.01, 0.99)),
            _ => crate::adtof::scaled_thresholds(self.onset()),
        }
    }

    /// The onset threshold in use, clamped to a sane range.
    pub fn onset(&self) -> f32 {
        clamp_or(self.onset_threshold, self.model.default_onset(), 0.01, 0.99)
    }

    /// The frame threshold in use (General).
    pub fn frame(&self) -> f32 {
        clamp_or(self.frame_threshold, crate::basicpitch::FRAME_THRESHOLD, 0.01, 0.99)
    }

    /// The shortest note kept, ms.
    pub fn min_ms(&self) -> f32 {
        clamp_or(self.min_note_ms, self.model.default_min_note_ms(), 0.0, 10_000.0)
    }
}

fn clamp_or(v: Option<f32>, d: f32, lo: f32, hi: f32) -> f32 {
    v.filter(|v| v.is_finite()).map_or(d, |v| v.clamp(lo, hi))
}

/// One transcribed note, in seconds from the start of the input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteEvent {
    pub start: f64,
    pub end: f64,
    /// MIDI pitch (GM drum note for drums).
    pub pitch: u8,
    /// 1..=127.
    pub velocity: u8,
    /// General: per-frame pitch offsets in 1/3 semitone (Basic Pitch's contour), else empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bends: Vec<i32>,
}

/// What a transcriber heard.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Transcription {
    /// Sorted by (start, pitch).
    pub notes: Vec<NoteEvent>,
    /// Sustain pedal down/up times, seconds (Piano, when the pedal option is on).
    pub pedals: Vec<(f64, f64)>,
    /// Pitch-wheel events `(seconds, -8192..=8191)` (General, when pitch bends are on).
    pub bends: Vec<(f64, i32)>,
    /// GM drum notes (they go on MIDI channel 10).
    pub drums: bool,
}

impl Transcription {
    /// Drops notes and pedals that are not finite or don't fit `0..=max_s` (cut at the end), and
    /// notes shorter than `min_s`.
    pub fn tidy(&mut self, max_s: f64, min_s: f64) {
        self.notes.retain(|n| n.start.is_finite() && n.end.is_finite() && n.start >= 0.0 && n.start < max_s);
        for n in &mut self.notes {
            n.end = n.end.min(max_s);
        }
        self.notes.retain(|n| n.end > n.start && n.end - n.start >= min_s);
        self.pedals.retain(|(a, b)| a.is_finite() && b.is_finite() && *a >= 0.0 && a < b && *a < max_s);
        for p in &mut self.pedals {
            p.1 = p.1.min(max_s);
        }
        self.bends.retain(|(t, _)| t.is_finite() && *t >= 0.0 && *t <= max_s);
    }
}

/// Round half to even, as `numpy.round`.
pub(crate) fn round_even(x: f64) -> f64 {
    x.round_ties_even()
}

/// `numpy.sum` of `float32` values (pairwise summation in `f32`, numpy's block size 128), so
/// means match the reference decoders bit for bit.
pub(crate) fn np_sum_f32(a: &[f32]) -> f32 {
    let n = a.len();
    if n < 8 {
        let mut r = 0.0f32;
        for v in a {
            r += *v;
        }
        r
    } else if n <= 128 {
        let mut r = [0.0f32; 8];
        for (j, v) in r.iter_mut().enumerate() {
            *v = a.get(j).copied().unwrap_or(0.0);
        }
        let full = n - n % 8;
        let mut i = 8;
        while i < full {
            for (j, v) in r.iter_mut().enumerate() {
                *v += a.get(i + j).copied().unwrap_or(0.0);
            }
            i += 8;
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        for v in a.get(full..).unwrap_or_default() {
            res += *v;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        let (x, y) = a.split_at(n2);
        np_sum_f32(x) + np_sum_f32(y)
    }
}

/// `numpy.mean` of `float32` values (as `float32`).
pub(crate) fn np_mean_f32(a: &[f32]) -> f32 {
    np_sum_f32(a) / a.len() as f32
}

/// A model's raw output over one source: everything its decoder needs, so settings (thresholds,
/// the shortest note, pedal, bends) can be tried in milliseconds without running the network
/// again (the Convert to MIDI dialog's Analyse).
#[derive(Debug, Clone, PartialEq)]
pub enum Analysis {
    General(crate::basicpitch::Raw),
    Piano(crate::pianokong::Raw),
    Drums(crate::adtof::Raw),
    /// GAME decodes in one go: its notes as `(start, end, fractional MIDI pitch)`.
    Vocal(Vec<(f64, f64, f64)>),
    /// No audio: nothing to decode.
    Empty,
}

/// One source analysed: the model's raw output and the source's length in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysed {
    pub analysis: Analysis,
    pub secs: f64,
}

impl Analysed {
    /// The transcription of this analysis with `opts` (which must be for the model that made it:
    /// another model's options decode with the model's own defaults). Times are seconds from the
    /// start of the source; notes are cut at its end.
    pub fn decode(&self, opts: &MidiOptions) -> Transcription {
        let mut t = match &self.analysis {
            Analysis::General(raw) => crate::basicpitch::decode_all(raw, &opts.for_model(MidiModel::General)),
            Analysis::Piano(raw) => crate::pianokong::decode_all(raw, &opts.for_model(MidiModel::Piano)),
            Analysis::Drums(raw) => crate::adtof::decode_all(raw, &opts.for_model(MidiModel::Drums)),
            Analysis::Vocal(raw) => crate::game::decode_all(raw, &opts.for_model(MidiModel::Vocal)),
            Analysis::Empty => Transcription { drums: opts.model == MidiModel::Drums, ..Default::default() },
        };
        let min_s = if self.model() == Some(MidiModel::General) { 0.0 } else { f64::from(opts.min_ms()) / 1000.0 };
        t.tidy(self.secs, min_s);
        t
    }

    /// The model that made it.
    pub fn model(&self) -> Option<MidiModel> {
        match self.analysis {
            Analysis::General(_) => Some(MidiModel::General),
            Analysis::Piano(_) => Some(MidiModel::Piano),
            Analysis::Drums(_) => Some(MidiModel::Drums),
            Analysis::Vocal(_) => Some(MidiModel::Vocal),
            Analysis::Empty => None,
        }
    }
}

impl MidiOptions {
    /// These options for `model`: as they are when they are its, else its defaults.
    fn for_model(&self, model: MidiModel) -> MidiOptions {
        if self.model == model { *self } else { MidiOptions::new(model) }
    }
}

/// The level the transcription models hear, dB (gated RMS, a loudness without the K filter).
/// ADTOF, Kong and Basic Pitch find more notes the louder their input (ADTOF on a −29 dB drums
/// stem: 14 hits in a minute; the same stem 12 dB up: 63); at a fixed level the count no longer
/// depends on the take's gain. −16 dB *(call)*: the published golden inputs keep their counts
/// (drums 24 of 25; piano within the test's range).
pub const TRANSCRIBE_LEVEL_DB: f32 = -16.0;
/// The most [`normalised`] turns a quiet source up (or a loud one down), dB: silence and noise
/// floors are not blown up.
pub const MAX_NORMALISE_DB: f32 = 30.0;

/// `audio`'s gated RMS level in dB (400 ms blocks, hop 100 ms; blocks under −70 dB and then
/// those more than 10 dB under the mean of the rest are left out), or `None` for silence.
pub fn gated_level_db(audio: &[f32], rate: u32) -> Option<f32> {
    let block = (rate as usize * 2 / 5).max(1);
    let hop = (block / 4).max(1);
    let energy = |a: &[f32]| a.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / a.len().max(1) as f64;
    let mut blocks: Vec<f64> = if audio.len() <= block {
        vec![energy(audio)]
    } else {
        (0..=(audio.len() - block) / hop).filter_map(|i| audio.get(i * hop..i * hop + block)).map(energy).collect()
    };
    blocks.retain(|e| e.is_finite() && *e > 1e-7);
    if blocks.is_empty() {
        return None;
    }
    let mean = blocks.iter().sum::<f64>() / blocks.len() as f64;
    let kept: Vec<f64> = blocks.into_iter().filter(|e| *e >= mean * 0.1).collect();
    let m = kept.iter().sum::<f64>() / kept.len().max(1) as f64;
    (m > 0.0).then(|| (10.0 * m.log10()) as f32)
}

/// `audio` (mono at `rate`) brought to [`TRANSCRIBE_LEVEL_DB`], the gain capped at
/// ±[`MAX_NORMALISE_DB`]; silence is returned as it is.
pub fn normalised(audio: &[f32], rate: u32) -> Vec<f32> {
    let Some(level) = gated_level_db(audio, rate) else { return audio.to_vec() };
    let gain_db = (TRANSCRIBE_LEVEL_DB - level).clamp(-MAX_NORMALISE_DB, MAX_NORMALISE_DB);
    let g = 10f32.powf(gain_db / 20.0);
    audio.iter().map(|v| v * g).collect()
}

/// Run `opts.model`'s network over `audio` (`[channel][frame]` at `rate`): folded to mono,
/// resampled to the model's rate. Returns the analysis (decode it with [`Analysed::decode`]) and
/// where it ran.
pub fn analyse(model: &InstalledModel, audio: &[Vec<f32>], rate: u32, opts: &MidiOptions, device: Device, ctx: &Ctx) -> Result<(Analysed, String)> {
    analyse_at(model, audio, rate, opts, device, ctx, true)
}

/// [`analyse`], with the input at [`TRANSCRIBE_LEVEL_DB`] (`normalise`) or as it is (the
/// published reference implementations' behaviour, which the golden tests pin).
pub fn analyse_at(
    model: &InstalledModel,
    audio: &[Vec<f32>],
    rate: u32,
    opts: &MidiOptions,
    device: Device,
    ctx: &Ctx,
    normalise: bool,
) -> Result<(Analysed, String)> {
    let len = crate::frames(audio);
    if len == 0 || rate == 0 {
        return Ok((Analysed { analysis: Analysis::Empty, secs: 0.0 }, String::new()));
    }
    let mut mono = crate::to_channels(audio, 1).into_iter().next().unwrap_or_default();
    crate::sanitize(std::slice::from_mut(&mut mono));
    let mrate = opts.model.sample_rate();
    ctx.say("Resampling");
    let mono = if rate == mrate { mono } else { soundcraft_dsp::offline::resample(&[mono], rate, mrate).into_iter().next().unwrap_or_default() };
    ctx.check()?;
    let level = |a: &[f32]| if normalise { normalised(a, mrate) } else { a.to_vec() };
    let (analysis, note) = match opts.model {
        MidiModel::General => {
            let (r, n) = crate::basicpitch::analyse(model, &level(&mono), device, ctx)?;
            (Analysis::General(r), n)
        }
        MidiModel::Piano => {
            let (r, n) = crate::pianokong::analyse(model, &level(&mono), device, ctx)?;
            (Analysis::Piano(r), n)
        }
        MidiModel::Drums => {
            // The network hears the normalised source; the velocities follow the original's level.
            let (mut r, n) = crate::adtof::analyse(model, &level(&mono), device, ctx)?;
            r.level = crate::adtof::levels(&mono, r.frames);
            (Analysis::Drums(r), n)
        }
        MidiModel::Vocal => {
            let (r, n) = crate::game::run(model, &mono, device, ctx)?;
            (Analysis::Vocal(r), n)
        }
    };
    ctx.check()?;
    Ok((Analysed { analysis, secs: len as f64 / f64::from(rate) }, note))
}

/// Transcribe `audio` (`[channel][frame]` at `rate`): [`analyse`], then [`Analysed::decode`].
/// Times are seconds from the start of `audio`. Returns the transcription and where it ran.
pub fn transcribe(
    model: &InstalledModel,
    audio: &[Vec<f32>],
    rate: u32,
    opts: &MidiOptions,
    device: Device,
    ctx: &Ctx,
) -> Result<(Transcription, String)> {
    let (a, note) = analyse(model, audio, rate, opts, device, ctx)?;
    ctx.say("Decoding notes");
    Ok((a.decode(opts), note))
}

/// [`transcribe`] without bringing the input to [`TRANSCRIBE_LEVEL_DB`]: what the published
/// reference implementations do (the golden tests compare against them).
pub fn transcribe_as_published(
    model: &InstalledModel,
    audio: &[Vec<f32>],
    rate: u32,
    opts: &MidiOptions,
    device: Device,
    ctx: &Ctx,
) -> Result<(Transcription, String)> {
    let (a, note) = analyse_at(model, audio, rate, opts, device, ctx, false)?;
    Ok((a.decode(opts), note))
}

/// The installed model for `opts`, or why not.
pub fn load(dir: &std::path::Path, opts: &MidiOptions) -> Result<InstalledModel> {
    crate::models::load(dir, opts.model.model_id()).map_err(|e| match e {
        MlError::NotInstalled(_) => MlError::NotInstalled(crate::models::how_to_install(opts.model.model_id(), dir)),
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairwise_sum_matches_a_plain_sum_closely_and_handles_every_size() {
        for n in [0usize, 1, 7, 8, 9, 15, 16, 127, 128, 129, 300, 1000] {
            let a: Vec<f32> = (0..n).map(|i| ((i * 37 % 101) as f32) * 0.013 - 0.4).collect();
            let plain: f64 = a.iter().map(|v| f64::from(*v)).sum();
            assert!((f64::from(np_sum_f32(&a)) - plain).abs() < 1e-3, "{n}");
        }
        // Order matters in f32: the 8-lane blocks of numpy.
        let a = [1.0e8f32, 1.0, -1.0e8, 1.0, 0.5, 0.25, 0.125, 1.0, 3.0];
        assert_eq!(np_sum_f32(&a), ((1.0e8f32 + 1.0) + (-1.0e8 + 1.0)) + ((0.5 + 0.25) + (0.125 + 1.0)) + 3.0);
    }

    #[test]
    fn options_and_names() {
        for m in MidiModel::ALL {
            assert_eq!(MidiModel::parse(m.id()), Some(m));
            assert_eq!(MidiModel::parse(m.model_id()), Some(m));
        }
        assert_eq!(MidiModel::parse("kazoo"), None);
        for i in Isolate::ALL {
            assert_eq!(Isolate::parse(i.id()), Some(i));
        }
        assert_eq!(Isolate::default_for(MidiModel::Piano, Some("Mix")), Isolate::Piano);
        assert_eq!(Isolate::default_for(MidiModel::Drums, None), Isolate::Drums);
        assert_eq!(Isolate::default_for(MidiModel::General, Some("Bass DI")), Isolate::Bass);
        assert_eq!(Isolate::default_for(MidiModel::General, Some("Lead Vox")), Isolate::Vocals);
        assert_eq!(Isolate::default_for(MidiModel::General, Some("Guitar")), Isolate::None);
        assert_eq!(MidiOptions::new(MidiModel::Drums).isolate, Isolate::Drums);
        let o = MidiOptions::new(MidiModel::General);
        assert_eq!((o.onset(), o.frame()), (0.5, 0.3));
        assert!(!o.pitch_bends && o.pedal);
        let o = MidiOptions { onset_threshold: Some(f32::NAN), min_note_ms: Some(-5.0), ..MidiOptions::new(MidiModel::Piano) };
        assert_eq!((o.onset(), o.min_ms()), (0.3, 0.0));
        assert_eq!(round_even(2.5), 2.0);
        assert_eq!(round_even(-0.5), 0.0);
    }

    #[test]
    fn the_transcription_input_is_level_free_and_silence_stays_silent() {
        // A drum-like input (bursts with a quiet bed) at -29 dB and at -9 dB peak-ish levels.
        let rate = 44_100;
        let take: Vec<f32> = (0..rate * 4)
            .map(|i| {
                let t = i % (rate / 2);
                let burst = if t < 2_000 { (1.0 - t as f32 / 2_000.0) * ((i * 7919) % 200) as f32 / 100.0 - 0.5 } else { 0.0 };
                burst + 0.01 * ((i as f32) * 0.05).sin()
            })
            .collect();
        let at = |db: f32| {
            let g = 10f32.powf((db - gated_level_db(&take, rate).unwrap()) / 20.0);
            take.iter().map(|v| v * g).collect::<Vec<f32>>()
        };
        let (quiet, loud) = (at(-29.0), at(-9.0));
        assert!((gated_level_db(&quiet, rate).unwrap() + 29.0).abs() < 0.01);
        let (a, b) = (normalised(&quiet, rate), normalised(&loud, rate));
        assert!((gated_level_db(&a, rate).unwrap() - TRANSCRIBE_LEVEL_DB).abs() < 0.01);
        // What the network hears is the same, so the hit count is too.
        assert!(a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= 1e-4 * x.abs().max(1e-3)));
        // The gain is capped; silence is untouched.
        let faint = at(-60.0);
        let n = normalised(&faint, rate);
        assert!((gated_level_db(&n, rate).unwrap() - (-60.0 + MAX_NORMALISE_DB)).abs() < 0.01);
        assert_eq!(normalised(&vec![0.0; 1000], rate), vec![0.0; 1000]);
        assert_eq!(gated_level_db(&[], rate), None);
    }

    #[test]
    fn an_analysis_decodes_again_with_new_settings_without_the_network() {
        // A drums analysis with two kicks; the thresholds decide which are kept.
        let frames = 100;
        let mut act = vec![0.0f32; frames * 5];
        act[10 * 5] = 0.9;
        act[60 * 5] = 0.3;
        let a = Analysed { analysis: Analysis::Drums(crate::adtof::Raw { frames, act, level: Vec::new() }), secs: 1.0 };
        let mut opts = MidiOptions::new(MidiModel::Drums);
        opts.drum_thresholds = Some([0.2; 5]);
        assert_eq!(a.decode(&opts).notes.len(), 2);
        opts.drum_thresholds = Some([0.5; 5]);
        let t = a.decode(&opts);
        assert_eq!(t.notes.len(), 1);
        assert!(t.drums);
        // Another model's options fall back to the analysis' own defaults.
        assert_eq!(a.decode(&MidiOptions::new(MidiModel::Piano)).notes.len(), 2);
        assert_eq!(a.model(), Some(MidiModel::Drums));
        let empty = Analysed { analysis: Analysis::Empty, secs: 0.0 };
        assert!(empty.decode(&opts).notes.is_empty());
    }

    #[test]
    fn tidy_cuts_and_drops() {
        let n = |s: f64, e: f64| NoteEvent { start: s, end: e, pitch: 60, velocity: 80, bends: Vec::new() };
        let mut t = Transcription { notes: vec![n(0.0, 0.5), n(f64::NAN, 1.0), n(0.9, 3.0), n(2.5, 3.0), n(1.0, 1.01)], ..Default::default() };
        t.pedals = vec![(0.1, 5.0), (3.0, 4.0), (1.0, 0.5)];
        t.tidy(2.0, 0.05);
        assert_eq!(t.notes, vec![n(0.0, 0.5), n(0.9, 2.0)]);
        assert_eq!(t.pedals, vec![(0.1, 2.0)]);
    }
}
