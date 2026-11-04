#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
//! FreeDAWm local machine-learning audio (desktop only; the web app has no ML).
//!
//! - [`models`]: the model folder (`%LOCALAPPDATA%\FreeDAWm\models`, or `FREEDAWM_MODELS_DIR`),
//!   one `<id>/card.json` per model, sha256 checked on first load, installed/missing listing.
//! - [`catalog`]: the models FreeDAWm runs and the downloads manifest; [`download`]: installing
//!   them from the FreeDAWm models release (after a click, on a worker).
//! - [`runtime`]: ONNX Runtime sessions on DirectML (the GPU, no CUDA install) or the CPU, with a
//!   graceful CPU fallback when DirectML can't start or fails mid-run. Never used on the audio
//!   thread: every job runs on a worker.
//! - [`stft`]: a `torch.stft`/`torch.istft`-compatible STFT for models whose STFT is on the host.
//! - [`chunk`]: chunked overlap-add over long audio.
//! - [`job`]: cancellation and progress for long jobs.
//! - Runners: [`denoise`] (DeepFilterNet 3 in pure Rust), [`htdemucs`], [`drumsep`], [`srs`]
//!   (de-reverb), [`karaoke`], [`vocals`] and [`bs_roformer`] (Mel-Band RoFormers and
//!   BS-RoFormer SW on the host-STFT runner [`roformer`]); [`pipeline`] chains them into the DAW's jobs (separate,
//!   karaoke, drum split, clean vocal) at any sample rate.
//! - Audio → MIDI ([`midi`]): [`basicpitch`] (general), [`pianokong`] (piano with pedal) and
//!   [`adtof`] (drums) and [`game`] (singing voice), each with its reference decoder.
//! - Chords ([`chords`], BTC) for the Band's chord lane, and lyrics ([`lyrics`], Whisper large-v3
//!   in whisper.cpp) with word times.
//!
//! Audio is planar `f32`: `audio[channel][frame]`. Nothing here panics on hostile input; panics
//! inside third-party inference code are caught at the [`pipeline`] boundary.

pub mod adtof;

/// `FREEDAWM_<suffix>` when set, else the older `SOUNDCRAFT_<suffix>` (the app's rename).
pub(crate) fn env_var_os(suffix: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(format!("FREEDAWM_{suffix}")).or_else(|| std::env::var_os(format!("SOUNDCRAFT_{suffix}")))
}
pub mod basicpitch;
pub mod bs_roformer;
pub mod catalog;
pub mod chords;
pub mod chunk;
pub mod clean;
pub mod denoise;
pub mod download;
pub mod drumsep;
pub mod game;
pub mod htdemucs;
pub mod job;
pub mod karaoke;
pub mod lyrics;
pub mod midi;
pub mod models;
pub mod pianokong;
pub mod pipeline;
pub mod roformer;
pub mod runtime;
pub mod srs;
pub mod stft;
pub mod vocals;

pub use chords::{ChordOptions, ChordSegment};
pub use chunk::Quality;
pub use clean::{CleanMode, CleanOptions};
pub use job::{Cancel, Ctx};
pub use lyrics::{Lyrics, LyricsLanguage, LyricsOptions, Prompt};
pub use midi::{Analysed, Analysis, Isolate, MidiModel, MidiOptions, NoteEvent, Transcription};
pub use models::{ModelCard, ModelEntry, ModelStatus};
pub use pipeline::{Job, JobKind, JobOutput, SepModel, Stem, StemSource};
pub use runtime::Device;

/// What can go wrong in a model job.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MlError {
    /// The model is not in the model folder (or its card is unreadable).
    #[error("{0}")]
    NotInstalled(String),
    /// The model's files are there but wrong (checksum, shapes, missing inputs).
    #[error("model `{0}`: {1}")]
    Model(String, String),
    /// ONNX Runtime / tract failed.
    #[error("inference failed: {0}")]
    Runtime(String),
    /// The person cancelled the job.
    #[error("cancelled")]
    Cancelled,
    /// The input audio can't be processed (empty, too long, bad rate).
    #[error("{0}")]
    BadInput(String),
    /// Reading or writing a file failed.
    #[error("{0}")]
    Io(String),
}

pub type Result<T> = std::result::Result<T, MlError>;

/// Longest input one job accepts, in seconds (memory: the stems of a 4-stem split of a 30-minute
/// stereo song at 44.1 kHz are ~2.5 GB of f32).
pub const MAX_JOB_SECONDS: f64 = 30.0 * 60.0;

/// Frames in the longest channel of `audio`.
pub fn frames(audio: &[Vec<f32>]) -> usize {
    audio.iter().map(Vec::len).max().unwrap_or(0)
}

/// `audio` with every channel exactly `len` frames (zero-padded or cut).
pub fn fit(mut audio: Vec<Vec<f32>>, len: usize) -> Vec<Vec<f32>> {
    for c in &mut audio {
        c.resize(len, 0.0);
    }
    audio
}

/// `audio` as exactly `channels` channels: mono is copied to every channel ("dual mono"), more
/// channels than wanted fold down by averaging.
pub fn to_channels(audio: &[Vec<f32>], channels: usize) -> Vec<Vec<f32>> {
    let n = frames(audio);
    let have = audio.len();
    if have == channels {
        return fit(audio.to_vec(), n);
    }
    if have == 0 {
        return vec![vec![0.0; n]; channels];
    }
    if have == 1 {
        let mono = fit(audio.to_vec(), n).into_iter().next().unwrap_or_default();
        return vec![mono; channels];
    }
    if channels == 1 {
        let mut m = vec![0.0f32; n];
        for c in audio {
            for (o, x) in m.iter_mut().zip(c) {
                *o += *x;
            }
        }
        let g = 1.0 / have as f32;
        m.iter_mut().for_each(|x| *x *= g);
        return vec![m];
    }
    // N > channels > 1: keep the first `channels`.
    (0..channels).map(|i| audio.get(i).cloned().map_or_else(|| vec![0.0; n], |c| fit(vec![c], n).into_iter().next().unwrap_or_default())).collect()
}

/// `audio` replaced by silence wherever it is not finite (hostile input never reaches a model).
pub fn sanitize(audio: &mut [Vec<f32>]) {
    for c in audio {
        for x in c.iter_mut() {
            if !x.is_finite() {
                *x = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_conversion() {
        let mono = vec![vec![1.0, 2.0]];
        assert_eq!(to_channels(&mono, 2), vec![vec![1.0, 2.0], vec![1.0, 2.0]]);
        let st = vec![vec![1.0, 3.0], vec![3.0, 5.0]];
        assert_eq!(to_channels(&st, 1), vec![vec![2.0, 4.0]]);
        assert_eq!(to_channels(&st, 2), st);
        assert_eq!(to_channels(&[], 2), vec![Vec::<f32>::new(), Vec::new()]);
        let mut bad = vec![vec![f32::NAN, 1.0, f32::INFINITY]];
        sanitize(&mut bad);
        assert_eq!(bad, vec![vec![0.0, 1.0, 0.0]]);
    }
}
