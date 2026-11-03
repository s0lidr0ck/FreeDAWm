//! ADTOF Frame_RNN drum transcription (Zehren et al.; PyTorch port by xavriley). Port of
//! `ml-models/adtof_drums/reference_infer.py`.
//!
//! Model `adtof_drums.onnx`: input `spec` `[1, T, 84, 1]` (a log-filterbank spectrogram, T
//! dynamic), output `act` `[1, T, 5]` (sigmoid activations at 100 frames per second) for GM drum
//! notes 35 kick, 38 snare, 47 toms, 42 hi-hat and 49 cymbals. The bidirectional GRUs see the
//! whole input in one run.
//!
//! The reference writes every hit at velocity 100. Here each hit's velocity comes from the
//! source's level (the RMS of the [`LEVEL_WINDOW_MS`] after the onset, of the drums stem when the
//! job isolated one), mapped per class so the loudest hit of the pass is [`VELOCITY_TOP`] and a
//! hit at a quarter of its power is half as loud in MIDI terms (square-root mapping).

use crate::job::Ctx;
use crate::midi::{MidiOptions, NoteEvent, Transcription, np_sum_f32};
use crate::models::InstalledModel;
use crate::runtime::{Device, OnnxModel, Tensor};
use crate::{MlError, Result};
use rustfft::FftPlanner;
use rustfft::num_complex::Complex64;

pub const ID: &str = "adtof_drums";
pub const SR: u32 = 44_100;
const N_FFT: usize = 2048;
const HOP: usize = 441;
const FPS: f64 = 100.0;
pub const LABELS: [u8; 5] = [35, 38, 47, 42, 49];
pub const THRESHOLDS: [f32; 5] = [0.22, 0.24, 0.32, 0.22, 0.30];
/// The onset-threshold value that keeps [`THRESHOLDS`] as published; another value scales them.
pub const ONSET_REFERENCE: f32 = 0.25;
/// Velocity of every hit when no source level is known (the original exports a fixed 100).
pub const VELOCITY: u8 = 100;
/// The velocity of each class's loudest hit when velocities follow the source's level.
pub const VELOCITY_TOP: u8 = 120;
/// The window after an onset whose RMS is the hit's level, ms.
pub const LEVEL_WINDOW_MS: f64 = 30.0;
/// The class names, in [`LABELS`] order.
pub const CLASS_NAMES: [&str; 5] = ["Kick", "Snare", "Toms", "Hi-hat", "Cymbals"];

/// The network's output over the whole input, plus the source's level per frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Raw {
    pub frames: usize,
    /// Activations `[frames][5]`, row-major.
    pub act: Vec<f32>,
    /// The RMS of the [`LEVEL_WINDOW_MS`] of source after each frame (empty: unknown, every hit
    /// plays at [`VELOCITY`]).
    pub level: Vec<f32>,
}

/// The five per-class thresholds for `onset` (the dialog's one Threshold: [`ONSET_REFERENCE`]
/// keeps [`THRESHOLDS`] as published, another value scales them).
pub fn scaled_thresholds(onset: f32) -> [f32; 5] {
    let scale = if onset.is_finite() { onset / ONSET_REFERENCE } else { 1.0 };
    let mut out = THRESHOLDS;
    if scale != 1.0 {
        for t in &mut out {
            *t = (*t * scale).clamp(0.01, 0.99);
        }
    }
    out
}

/// The RMS of the [`LEVEL_WINDOW_MS`] of `audio` (mono at 44.1 kHz) after each of `frames`
/// frames at 100 fps.
pub fn levels(audio: &[f32], frames: usize) -> Vec<f32> {
    let win = ((LEVEL_WINDOW_MS / 1000.0) * f64::from(SR)) as usize;
    (0..frames)
        .map(|f| {
            let a = f * HOP;
            let seg = audio.get(a..(a + win).min(audio.len())).unwrap_or_default();
            if seg.is_empty() { 0.0 } else { (seg.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / seg.len() as f64).sqrt() as f32 }
        })
        .collect()
}

/// Velocities from the source's level: per class, the loudest hit is [`VELOCITY_TOP`] and the
/// others follow the square root of their level ratio (power → loudness), at least 1. Notes keep
/// [`VELOCITY`] when `level` is empty.
pub fn velocities(notes: &mut [NoteEvent], level: &[f32]) {
    if level.is_empty() {
        return;
    }
    let at = |n: &NoteEvent| level.get((n.start * FPS).round().max(0.0) as usize).copied().unwrap_or(0.0);
    for lab in LABELS {
        let top = notes.iter().filter(|n| n.pitch == lab).map(at).fold(0.0f32, f32::max);
        for n in notes.iter_mut().filter(|n| n.pitch == lab) {
            n.velocity =
                if top > 0.0 { (f32::from(VELOCITY_TOP) * (at(n) / top).clamp(0.0, 1.0).sqrt()).round().clamp(1.0, 127.0) as u8 } else { VELOCITY };
        }
    }
}
/// Longest input, frames: the whole file runs at once (30 minutes).
const MAX_FRAMES: usize = 30 * 60 * 100;

/// The madmom-style log filterbank (12 bands per octave, 20 Hz–20 kHz, rows normalised to sum 1),
/// `[bands][1024]` row-major.
pub fn filterbank() -> (usize, Vec<f32>) {
    let mut freqs = Vec::new();
    let (mut f, factor) = (20.0f64, 2f64.powf(1.0 / 12.0));
    while f <= 20000.0 * (1.0 + 1e-12) {
        freqs.push(f);
        f *= factor;
    }
    let half = N_FFT / 2;
    let val = 1.0 / (N_FFT as f64 * (1.0 / f64::from(SR)));
    let fft_freqs: Vec<f64> = (0..half).map(|k| k as f64 * val).collect();
    let mut ub: Vec<usize> = Vec::new();
    for f in &freqs {
        let mut best = (f64::INFINITY, 0usize);
        for (k, ff) in fft_freqs.iter().enumerate() {
            let d = (ff - f).abs();
            if d < best.0 {
                best = (d, k);
            }
        }
        if ub.last().is_none_or(|l| best.1 > *l) {
            ub.push(best.1);
        }
    }
    let bands = ub.len().saturating_sub(2);
    let mut fb = vec![0.0f32; bands * half];
    for i in 0..bands {
        let (lo, c, hi) = (ub[i], ub[i + 1], ub[i + 2]);
        let row = &mut fb[i * half..(i + 1) * half];
        if hi - lo < 2 {
            if let Some(v) = row.get_mut(lo) {
                *v = 1.0;
            }
            continue;
        }
        for b in lo..c {
            if let Some(v) = row.get_mut(b) {
                *v = ((b - lo) as f64 / (c - lo) as f64) as f32;
            }
        }
        if let Some(v) = row.get_mut(c) {
            *v = 1.0;
        }
        for b in c + 1..hi.min(half) {
            if let Some(v) = row.get_mut(b) {
                *v = ((hi - b) as f64 / (hi - c) as f64) as f32;
            }
        }
    }
    for i in 0..bands {
        let row = &mut fb[i * half..(i + 1) * half];
        let mut s = np_sum_f32(row);
        if s == 0.0 {
            s = 1.0;
        }
        row.iter_mut().for_each(|v| *v /= s);
    }
    (bands, fb)
}

/// The network input for `audio` (mono at 44.1 kHz): `log10(1 + fb · |STFT|)`, `[frames][bands]`.
pub fn features(audio: &[f32]) -> (usize, usize, Vec<f32>) {
    let (bands, fb) = filterbank();
    let half = N_FFT / 2;
    let t = 1 + audio.len() / HOP;
    // np.hanning: symmetric.
    let w: Vec<f32> = (0..N_FFT).map(|n| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / (N_FFT - 1) as f64).cos()) as f32).collect();
    let fft = FftPlanner::<f64>::new().plan_fft_forward(N_FFT);
    let mut buf = vec![Complex64::new(0.0, 0.0); N_FFT];
    let mut mag = vec![0.0f32; half];
    let mut out = vec![0.0f32; t * bands];
    let pad = N_FFT / 2;
    for f in 0..t {
        for (k, (b, wv)) in buf.iter_mut().zip(&w).enumerate() {
            // x is the audio with N_FFT/2 zeros on both sides.
            let i = (f * HOP + k).checked_sub(pad);
            let s = i.and_then(|i| audio.get(i)).copied().unwrap_or(0.0);
            *b = Complex64::new(f64::from(s * wv), 0.0);
        }
        fft.process(&mut buf);
        for (m, c) in mag.iter_mut().zip(&buf) {
            *m = c.norm() as f32;
        }
        for (band, o) in out.get_mut(f * bands..(f + 1) * bands).unwrap_or_default().iter_mut().enumerate() {
            let row = fb.get(band * half..(band + 1) * half).unwrap_or_default();
            let acc: f64 = row.iter().zip(&mag).map(|(a, b)| f64::from(*a) * f64::from(*b)).sum();
            *o = (1.0f32 + acc as f32).log10();
        }
    }
    (t, bands, out)
}

/// madmom's `NotePeakPickingProcessor` (fps 100, pre-avg 0.1 s, pre-max 0.02 s, combine 0.02 s)
/// on one class's activations: the peak frames.
pub fn peak_pick(act: &[f32], thr: f32) -> Vec<usize> {
    let n = act.len();
    if n == 0 {
        return Vec::new();
    }
    let edge = |i: isize| act.get(i.clamp(0, n as isize - 1) as usize).copied().unwrap_or(0.0);
    let k = 1.0f32 / 12.0;
    let proc: Vec<f32> = (0..n)
        .map(|i| {
            let mut s = 0.0f32;
            for j in 0..12isize {
                s += edge(i as isize - 10 + j) * k;
            }
            (edge(i as isize) - s).max(0.0)
        })
        .collect();
    let pe = |i: isize| proc.get(i.clamp(0, n as isize - 1) as usize).copied().unwrap_or(0.0);
    let idx: Vec<usize> = (0..n)
        .filter(|&i| {
            let p = pe(i as isize);
            let wmax = (-2..=1).map(|d| pe(i as isize + d)).fold(f32::NEG_INFINITY, f32::max);
            p >= wmax && p >= thr
        })
        .collect();
    let mut kept = Vec::new();
    let mut group: Vec<usize> = Vec::new();
    let best = |g: &[usize]| -> Option<usize> {
        let mut b: Option<usize> = None;
        for &j in g {
            if b.is_none_or(|x| pe(j as isize) > pe(x as isize)) {
                b = Some(j);
            }
        }
        b
    };
    for i in idx {
        if group.last().is_some_and(|l| i - l <= 2) {
            group.push(i);
        } else {
            kept.extend(best(&group));
            group = vec![i];
        }
    }
    kept.extend(best(&group));
    kept
}

/// The notes of `raw` with one threshold per class ([`LABELS`] order), velocities from the
/// source's level when `raw` has it.
pub fn decode(raw: &Raw, thresholds: &[f32; 5]) -> Vec<NoteEvent> {
    let (act, frames) = (&raw.act, raw.frames);
    let mut notes = Vec::new();
    for (c, (lab, thr)) in LABELS.iter().zip(thresholds).enumerate() {
        let col: Vec<f32> = (0..frames).map(|f| act.get(f * LABELS.len() + c).copied().unwrap_or(0.0)).collect();
        let thr = if thr.is_finite() { thr.clamp(0.01, 0.99) } else { THRESHOLDS[c] };
        for f in peak_pick(&col, thr) {
            let t = f as f64 / FPS;
            notes.push(NoteEvent { start: t, end: t + 0.1, pitch: *lab, velocity: VELOCITY, bends: Vec::new() });
        }
    }
    velocities(&mut notes, &raw.level);
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    notes
}

/// Run the network on features `[frames][bands]`: the activations `[frames][5]`.
pub fn run_network(onnx: &mut OnnxModel, t: usize, bands: usize, feats: Vec<f32>) -> Result<Vec<f32>> {
    let out = onnx.run_named(&[("spec", &Tensor::new(vec![1, t as i64, bands as i64, 1], feats))])?;
    let a = out.into_iter().next().ok_or_else(|| MlError::Model(ID.into(), "no output".into()))?;
    if a.data.len() != t * LABELS.len() {
        return Err(MlError::Model(ID.into(), format!("output shape {:?}, expected [1, {t}, 5]", a.shape)));
    }
    Ok(a.data)
}

/// The network over `audio` (mono at 44.1 kHz) and the source's levels: everything a decode
/// needs, so settings can be tried without running the network again. Returns the raw outputs
/// and where it ran.
pub fn analyse(model: &InstalledModel, audio: &[f32], device: Device, ctx: &Ctx) -> Result<(Raw, String)> {
    if audio.len() / HOP > MAX_FRAMES {
        return Err(MlError::BadInput("the drum model takes at most 30 minutes at once".into()));
    }
    ctx.say("Analysing the drums");
    let (t, bands, feats) = features(audio);
    ctx.check()?;
    ctx.report(0.2);
    ctx.say("Loading the drum model");
    let mut onnx = OnnxModel::open_model(model, device, ctx.cancel_flag())?;
    ctx.say(&format!("Transcribing on the {}", onnx.device_label()));
    let act = run_network(&mut onnx, t, bands, feats)?;
    ctx.report(0.95);
    let level = levels(audio, t);
    let note = onnx.fallback_note.clone().unwrap_or_else(|| onnx.device_label().to_string());
    Ok((Raw { frames: t, act, level }, note))
}

/// The transcription of `raw` with `opts` (the five thresholds).
pub fn decode_all(raw: &Raw, opts: &MidiOptions) -> Transcription {
    Transcription { notes: decode(raw, &opts.drum_thresholds()), pedals: Vec::new(), bends: Vec::new(), drums: true }
}

/// Transcribe `audio` (mono at 44.1 kHz).
pub fn transcribe(model: &InstalledModel, audio: &[f32], opts: &MidiOptions, device: Device, ctx: &Ctx) -> Result<(Transcription, String)> {
    let (raw, note) = analyse(model, audio, device, ctx)?;
    Ok((decode_all(&raw, opts), note))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filterbank_has_84_normalised_bands() {
        let (bands, fb) = filterbank();
        assert_eq!(bands, 84);
        for b in 0..bands {
            let s: f32 = fb[b * 1024..(b + 1) * 1024].iter().sum();
            assert!((s - 1.0).abs() < 1e-5, "band {b}: {s}");
        }
    }

    #[test]
    fn peaks_are_picked_and_merged() {
        let mut a = vec![0.0f32; 100];
        a[20] = 0.9;
        a[21] = 0.8;
        a[60] = 0.5;
        a[61] = 0.6;
        a[80] = 0.1;
        assert_eq!(peak_pick(&a, 0.22), vec![20, 61]);
        assert!(peak_pick(&[], 0.2).is_empty());
    }

    #[test]
    fn velocities_follow_the_source_level_per_class() {
        // Two kicks (frames 10 and 60) at different levels, one snare: the louder kick is 120,
        // the quieter one lower, and the lone snare is 120 too (the loudest of its class).
        let frames = 100;
        let mut act = vec![0.0f32; frames * 5];
        act[10 * 5] = 0.9;
        act[60 * 5] = 0.9;
        act[30 * 5 + 1] = 0.9;
        let mut audio = vec![0.0f32; frames * HOP];
        audio[10 * HOP..10 * HOP + 1000].iter_mut().for_each(|v| *v = 0.8);
        audio[60 * HOP..60 * HOP + 1000].iter_mut().for_each(|v| *v = 0.2);
        audio[30 * HOP..30 * HOP + 1000].iter_mut().for_each(|v| *v = 0.1);
        let raw = Raw { frames, act: act.clone(), level: levels(&audio, frames) };
        let notes = decode(&raw, &THRESHOLDS);
        let v: Vec<(u8, u8)> = notes.iter().map(|n| (n.pitch, n.velocity)).collect();
        assert_eq!(v, vec![(35, 120), (38, 120), (35, 60)], "{notes:?}");
        // No level known: the reference's fixed 100.
        let plain = decode(&Raw { frames, act, level: Vec::new() }, &THRESHOLDS);
        assert!(plain.iter().all(|n| n.velocity == VELOCITY));
        // A hit on silence is still a hit, at the floor.
        let mut quiet = vec![NoteEvent { start: 0.9, end: 1.0, pitch: 42, velocity: 0, bends: Vec::new() }];
        velocities(&mut quiet, &[0.0; 100]);
        assert_eq!(quiet[0].velocity, VELOCITY);
    }

    #[test]
    fn thresholds_filter_each_class_on_its_own() {
        let frames = 50;
        let mut act = vec![0.0f32; frames * 5];
        act[10 * 5] = 0.3; // kick
        act[20 * 5 + 3] = 0.3; // hi-hat
        let raw = Raw { frames, act, level: Vec::new() };
        let both = decode(&raw, &[0.2; 5]);
        assert_eq!(both.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![35, 42]);
        let hats_only = decode(&raw, &[0.5, 0.2, 0.2, 0.2, 0.2]);
        assert_eq!(hats_only.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![42]);
        let kick_only = decode(&raw, &[0.2, 0.2, 0.2, 0.5, 0.2]);
        assert_eq!(kick_only.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![35]);
        assert_eq!(scaled_thresholds(ONSET_REFERENCE), THRESHOLDS);
        assert!((scaled_thresholds(0.5)[0] - 0.44).abs() < 1e-6);
    }

    #[test]
    fn features_shape_and_silence() {
        let (t, bands, f) = features(&vec![0.0; 4410]);
        assert_eq!((t, bands, f.len()), (11, 84, 11 * 84));
        assert!(f.iter().all(|v| *v == 0.0));
    }
}
