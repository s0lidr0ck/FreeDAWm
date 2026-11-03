//! Chord recognition: BTC (Park et al., ISMIR 2019, large vocabulary), exported with its CQT inside
//! the graph, and its decoder (`tools/ml-export/btc/reference_infer.py`, ported exactly).
//!
//! 1. Mono (mean), resampled to 22050 Hz.
//! 2. Chunks of 220500 samples (10 s); the last holds the rest, zero-padded, with
//!    `n_frames = 1 + len / 2048`. Each chunk gives `n_frames` rows of 170 logits and of the 144-bin
//!    log CQT.
//! 3. Frame label = argmax (first index on ties); a frame lasts 10/108 s. Segments are runs of
//!    equal labels.
//! 4. Names for the chord lane: root + suffix (`Em`, `Cmaj7`, `F#m7b5`); N and X are no chord.
//! 5. Bass (a heuristic, only meaningful on a full mix): the strongest pitch class of C1..B2 in the
//!    segment's mean CQT; a chord tone other than the root at ≥ 1.5× the root's energy makes the
//!    segment an inversion `<name>/<bass>`.

use crate::job::Ctx;
use crate::models::InstalledModel;
use crate::runtime::{AnyTensor, Device, OnnxModel};
use crate::{MlError, Result};
use serde::{Deserialize, Serialize};

/// The model folder id.
pub const ID: &str = "btc_chords";
/// The rate BTC listens at.
pub const SR: u32 = 22_050;
/// Samples per chunk (10 s).
pub const CHUNK: usize = 220_500;
/// CQT hop.
pub const HOP: usize = 2048;
/// Frames per full chunk.
pub const FRAMES: usize = 108;
/// Seconds per frame (BTC's time unit, not 2048/22050).
pub const TIME_UNIT: f64 = 10.0 / 108.0;
/// Classes: 12 roots × 14 qualities, then X (unknown) and N (no chord).
pub const CLASSES: usize = 170;
/// CQT bins.
pub const BINS: usize = 144;
/// The class of "unknown chord".
pub const X: usize = 168;
/// The class of "no chord".
pub const N: usize = 169;
/// A chord tone other than the root becomes the bass at this ratio of the root's energy.
pub const BASS_RATIO: f64 = 1.5;

pub const ROOTS: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
pub const QUALITIES: [&str; 14] = ["min", "maj", "dim", "aug", "min6", "maj6", "min7", "minmaj7", "maj7", "7", "dim7", "hdim7", "sus2", "sus4"];
/// The chord lane's spelling of each quality (`chord_match::parse_chord_name` reads them all).
pub const DAW_SUFFIX: [&str; 14] = ["m", "", "dim", "aug", "m6", "6", "m7", "mM7", "maj7", "7", "dim7", "m7b5", "sus2", "sus4"];
/// Chord tones (semitones above the root) of each quality.
const TONES: [&[usize]; 14] = [
    &[0, 3, 7],
    &[0, 4, 7],
    &[0, 3, 6],
    &[0, 4, 8],
    &[0, 3, 7, 9],
    &[0, 4, 7, 9],
    &[0, 3, 7, 10],
    &[0, 3, 7, 11],
    &[0, 4, 7, 11],
    &[0, 4, 7, 10],
    &[0, 3, 6, 9],
    &[0, 3, 6, 10],
    &[0, 2, 7],
    &[0, 5, 7],
];

/// How to detect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChordOptions {
    /// Guess the bass (inversions, `C/E`). Only right on a full mix: on a single instrument it
    /// reports that instrument's lowest voice.
    #[serde(default)]
    pub bass: bool,
}

/// A run of equal frame labels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChordSegment {
    /// Seconds from the start of the input.
    pub start: f64,
    pub end: f64,
    /// BTC / mir_eval style: `E:min`, `C`, `A:min7`, `N`, `X`.
    pub label: String,
    /// 0..=169.
    pub class: usize,
    /// The chord lane's name (`Em`, `Cmaj7/B`); empty for N and X.
    pub name: String,
    /// -1 for N and X.
    pub root_pc: i32,
    /// `min`, `maj`… (empty for N and X).
    pub quality: String,
    /// The bass pitch class (the root unless an inversion was found); -1 for N and X.
    pub bass_pc: i32,
}

impl ChordSegment {
    /// A chord (not N or X).
    pub fn is_chord(&self) -> bool {
        self.class < X
    }
}

/// The BTC label of class `c`.
pub fn label_of(c: usize) -> String {
    match c {
        N => "N".into(),
        X => "X".into(),
        c => {
            let (r, q) = (ROOTS.get(c / 14).copied().unwrap_or("C"), QUALITIES.get(c % 14).copied().unwrap_or("maj"));
            if q == "maj" { r.to_string() } else { format!("{r}:{q}") }
        }
    }
}

/// The input split into chunks: `(start, len)`. Full chunks while more than a chunk is left, then
/// the rest (an empty input gives one empty chunk, as the reference does).
fn chunks(len: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut s = 0usize;
    while len > s + CHUNK {
        out.push((s, CHUNK));
        s += CHUNK;
    }
    out.push((s, len - s));
    out
}

/// The network on mono 22050 Hz audio: logits `[frames][170]` and log CQT `[frames][144]` of
/// every real frame.
pub fn run_network(model: &InstalledModel, audio: &[f32], device: Device, ctx: &Ctx) -> Result<(Vec<f32>, Vec<f32>, String)> {
    let mut m = OnnxModel::open_model(model, device, ctx.cancel_flag())?;
    let parts = chunks(audio.len());
    let n = parts.len().max(1);
    let (mut logits, mut cqt) = (Vec::new(), Vec::new());
    for (i, (s, len)) in parts.into_iter().enumerate() {
        ctx.check()?;
        let frames = 1 + len / HOP;
        let mut x = vec![0.0f32; CHUNK];
        if let (Some(dst), Some(src)) = (x.get_mut(..len), audio.get(s..s + len)) {
            dst.copy_from_slice(src);
        }
        let out =
            m.run_typed(vec![("audio", AnyTensor::F32(vec![1, CHUNK as i64], x)), ("n_frames", AnyTensor::I64(vec![1], vec![frames as i64]))])?;
        let get = |name: &str, width: usize| -> Result<Vec<f32>> {
            let Some((_, AnyTensor::F32(shape, d))) = out.iter().find(|(k, _)| k == name) else {
                return Err(MlError::Model(ID.into(), format!("no f32 output `{name}`")));
            };
            if shape.last().copied() != Some(width as i64) || d.len() < frames * width {
                return Err(MlError::Model(ID.into(), format!("output `{name}` has shape {shape:?}")));
            }
            Ok(d.get(..frames * width).map(<[f32]>::to_vec).unwrap_or_default())
        };
        logits.extend(get("logits", CLASSES)?);
        cqt.extend(get("cqt", BINS)?);
        ctx.report((i + 1) as f32 / n as f32);
    }
    let note = m.fallback_note.clone().unwrap_or_else(|| m.device_label().to_string());
    Ok((logits, cqt, note))
}

/// numpy's argmax of one row: the first maximum (NaN counts as the maximum, as in numpy).
fn argmax(row: &[f32]) -> usize {
    let mut best = 0usize;
    let mut bv = f32::NEG_INFINITY;
    for (i, &v) in row.iter().enumerate() {
        if v.is_nan() {
            return i;
        }
        if v > bv || i == 0 {
            bv = v;
            best = i;
        }
    }
    best
}

/// The bass energy per pitch class over frames `a..b`: the mean linear CQT magnitude of the
/// on-semitone bins of C1..B2, folded to 12 classes.
fn bass_profile(cqt: &[f32], a: usize, b: usize) -> [f64; 12] {
    // numpy reduces axis 0 row by row in f32, then divides by the count.
    let mut mean = [0.0f32; 48];
    for f in a..b {
        if let Some(row) = cqt.get(f * BINS..f * BINS + 48) {
            for (m, v) in mean.iter_mut().zip(row) {
                *m += v.exp();
            }
        }
    }
    let n = (b - a).max(1) as f32;
    let mut pc = [0.0f64; 12];
    for (i, k) in (0..48).step_by(2).enumerate() {
        pc[i % 12] += f64::from(mean[k] / n);
    }
    pc
}

/// Segments from the network's outputs (`logits` `[frames][170]`, `cqt` `[frames][144]`).
pub fn decode(logits: &[f32], cqt: &[f32], opts: &ChordOptions) -> Vec<ChordSegment> {
    let frames: Vec<usize> = logits.chunks_exact(CLASSES).map(argmax).collect();
    let mut runs = Vec::new();
    let mut s = 0usize;
    for f in 1..=frames.len() {
        if f == frames.len() || frames.get(f) != frames.get(s) {
            runs.push((s, f, frames.get(s).copied().unwrap_or(N)));
            s = f;
        }
    }
    runs.into_iter()
        .map(|(a, b, c)| {
            let (start, end, label) = (a as f64 * TIME_UNIT, b as f64 * TIME_UNIT, label_of(c));
            if c >= X {
                return ChordSegment { start, end, label, class: c, name: String::new(), root_pc: -1, quality: String::new(), bass_pc: -1 };
            }
            let (root, q) = (c / 14, c % 14);
            let mut name = format!("{}{}", ROOTS.get(root).copied().unwrap_or("C"), DAW_SUFFIX.get(q).copied().unwrap_or(""));
            let mut bp = root;
            if opts.bass {
                let e = bass_profile(cqt, a, b);
                let mut best = 0usize;
                for (i, v) in e.iter().enumerate() {
                    if *v > e[best] {
                        best = i;
                    }
                }
                let is_tone = TONES.get(q).is_some_and(|t| t.iter().any(|k| (root + k) % 12 == best));
                if best != root && is_tone && e[best] >= BASS_RATIO * e[root] {
                    name.push('/');
                    name.push_str(ROOTS.get(best).copied().unwrap_or("C"));
                    bp = best;
                }
            }
            ChordSegment {
                start,
                end,
                label,
                class: c,
                name,
                root_pc: root as i32,
                quality: QUALITIES.get(q).copied().unwrap_or("").into(),
                bass_pc: bp as i32,
            }
        })
        .collect()
}

/// Chords of `audio` (`[channel][frame]` at `rate`): segments in seconds from its start, and where
/// the model ran.
pub fn detect(
    model: &InstalledModel,
    audio: &[Vec<f32>],
    rate: u32,
    opts: &ChordOptions,
    device: Device,
    ctx: &Ctx,
) -> Result<(Vec<ChordSegment>, String)> {
    let len = crate::frames(audio);
    if len == 0 || rate == 0 {
        return Ok((Vec::new(), String::new()));
    }
    let mut mono = crate::to_channels(audio, 1).into_iter().next().unwrap_or_default();
    crate::sanitize(std::slice::from_mut(&mut mono));
    ctx.say("Resampling");
    let mono = if rate == SR { mono } else { soundcraft_dsp::offline::resample(&[mono], rate, SR).into_iter().next().unwrap_or_default() };
    ctx.check()?;
    ctx.say("Listening for chords (BTC)");
    let (logits, cqt, note) = run_network(model, &mono, device, &ctx.stage(0.0, 0.95))?;
    ctx.check()?;
    // As the reference: the last segment ends with the last frame (up to one frame past the
    // input's end); the engine cuts at the source's end.
    let segs = decode(&logits, &cqt, opts);
    ctx.report(1.0);
    Ok((segs, note))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_names_and_chunks() {
        assert_eq!(label_of(56), "E:min");
        assert_eq!(label_of(1), "C");
        assert_eq!(label_of(N), "N");
        assert_eq!(label_of(X), "X");
        assert_eq!(chunks(0), vec![(0, 0)]);
        assert_eq!(chunks(CHUNK), vec![(0, CHUNK)]);
        assert_eq!(chunks(CHUNK + 1), vec![(0, CHUNK), (CHUNK, 1)]);
        assert_eq!(argmax(&[1.0, 3.0, 3.0]), 1);
    }

    #[test]
    fn decode_runs_and_bass() {
        // Three frames of C major (class 1), two of N; the bass of C1..B2 strongest on E.
        let mut logits = vec![0.0f32; 5 * CLASSES];
        for f in 0..3 {
            logits[f * CLASSES + 1] = 5.0;
        }
        for f in 3..5 {
            logits[f * CLASSES + N] = 5.0;
        }
        let mut cqt = vec![-10.0f32; 5 * BINS];
        for f in 0..3 {
            cqt[f * BINS + 8] = 2.0; // E1 (bin 2·4)
            cqt[f * BINS] = 0.5; // C1
        }
        let plain = decode(&logits, &cqt, &ChordOptions { bass: false });
        assert_eq!(plain.len(), 2);
        assert_eq!((plain[0].name.as_str(), plain[0].bass_pc), ("C", 0));
        assert!((plain[0].end - 3.0 * TIME_UNIT).abs() < 1e-12);
        assert!(!plain[1].is_chord());
        let inv = decode(&logits, &cqt, &ChordOptions { bass: true });
        assert_eq!((inv[0].name.as_str(), inv[0].bass_pc), ("C/E", 4));
    }
}
