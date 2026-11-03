//! GAME small (openvpi, Generative Adaptive MIDI Extractor): singing voice → monophonic notes.
//! Port of `ml-models/game_vocal/reference_infer.py`.
//!
//! Five graphs, mono 44.1 kHz, 10 ms frames: `encoder` (mel front end inside), `dur2bd`,
//! `segmenter` (one D3PM step; the random draws are an explicit `noise` input from a SplitMix64
//! counter, so the result is reproducible), `bd2dur` and `estimator` (presence and fractional
//! MIDI pitch per note). The audio is sliced at silences first (GAME's slicer2).

use crate::job::Ctx;
use crate::midi::{MidiOptions, NoteEvent, Transcription, np_mean_f32, round_even};
use crate::models::InstalledModel;
use crate::runtime::{AnyTensor, Device, OnnxModel};
use crate::{MlError, Result};

pub const ID: &str = "game_vocal";
pub const SR: u32 = 44_100;
/// Language: 0 unknown, 1 English (the default), 2 Japanese, 3 Cantonese, 4 Mandarin.
pub const LANG_EN: i64 = 1;
const SEG_THRESHOLD: f32 = 0.2;
const SEG_RADIUS: i64 = 2;
const EST_THRESHOLD: f32 = 0.2;
const STEPS: usize = 8;
const MAX_SLICE_S: f64 = 30.0;
pub const VELOCITY: u8 = 100;
const GRAPHS: [&str; 5] = ["encoder", "dur2bd", "segmenter", "bd2dur", "estimator"];

/// The D3PM draws for slice `slice_index`, step `step`: `T` uniform `[0, 1)` values.
pub fn noise(slice_index: u64, step: u64, t: usize) -> Vec<f32> {
    let seed = slice_index.wrapping_mul(16).wrapping_add(step);
    (0..t as u64)
        .map(|i| {
            let mut z = ((seed << 32) | i).wrapping_add(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            ((z >> 40) as f64 / 16_777_216.0) as f32
        })
        .collect()
}

/// RMS of `win`-sample frames every `hop` (zero-padded by `win / 2` on both sides).
fn frame_rms(w: &[f32], win: usize, hop: usize) -> Vec<f32> {
    let half = win / 2;
    let padded = w.len() + 2 * half;
    if padded < win {
        return Vec::new();
    }
    let n = 1 + (padded - win) / hop;
    let mut buf = vec![0.0f32; win];
    (0..n)
        .map(|i| {
            for (k, b) in buf.iter_mut().enumerate() {
                let x = (i * hop + k).checked_sub(half).and_then(|j| w.get(j)).copied().unwrap_or(0.0);
                *b = x.abs() * x.abs();
            }
            np_mean_f32(&buf).sqrt()
        })
        .collect()
}

/// numpy `argmin` of `v[a..b]` (clamped), as an index into `v`.
fn argmin(v: &[f32], a: usize, b: usize) -> usize {
    let b = b.min(v.len());
    let mut best = (f32::INFINITY, a);
    for i in a..b {
        if let Some(x) = v.get(i)
            && *x < best.0
        {
            best = (*x, i);
        }
    }
    best.1
}

/// GAME's slicer2 (threshold -40 dB, min length 1 s, min interval 200 ms, hop 20 ms, max silence
/// kept 100 ms): `(offset s, start sample, end sample)` of each slice.
pub fn slice_audio(w: &[f32]) -> Vec<(f64, usize, usize)> {
    let sr = f64::from(SR);
    let thr = 10f64.powf(-40.0 / 20.0) as f32;
    let mi_f = sr * 200.0 / 1000.0;
    let hop = (sr * 20.0 / 1000.0).round() as usize;
    let win = (mi_f.round() as usize).min(4 * hop);
    let min_len = (sr * 1000.0 / 1000.0 / hop as f64).round() as usize;
    let mi = (mi_f / hop as f64).round() as usize;
    let msk = (sr * 100.0 / 1000.0 / hop as f64).round() as usize;
    let whole = vec![(0.0, 0, w.len())];
    if w.len().div_ceil(hop) <= min_len {
        return whole;
    }
    let rms = frame_rms(w, win, hop);
    let mut tags: Vec<(usize, usize)> = Vec::new();
    let (mut ss, mut cs): (Option<usize>, usize) = (None, 0);
    for (i, r) in rms.iter().enumerate() {
        if *r < thr {
            if ss.is_none() {
                ss = Some(i);
            }
            continue;
        }
        let Some(s0) = ss else { continue };
        let lead = s0 == 0 && i > msk;
        let mid = i - s0 >= mi && i.saturating_sub(cs) >= min_len && i >= cs;
        if !lead && !mid {
            ss = None;
            continue;
        }
        if i - s0 <= msk {
            let pos = argmin(&rms, s0, i + 1);
            tags.push(if s0 == 0 { (0, pos) } else { (pos, pos) });
            cs = pos;
        } else if i - s0 <= msk * 2 {
            let pos = argmin(&rms, i - msk, s0 + msk + 1);
            let pl = argmin(&rms, s0, s0 + msk + 1);
            let pr = argmin(&rms, i - msk, i + 1);
            if s0 == 0 {
                tags.push((0, pr));
                cs = pr;
            } else {
                tags.push((pl.min(pos), pr.max(pos)));
                cs = pr.max(pos);
            }
        } else {
            let pl = argmin(&rms, s0, s0 + msk + 1);
            let pr = argmin(&rms, i - msk, i + 1);
            tags.push(if s0 == 0 { (0, pr) } else { (pl, pr) });
            cs = pr;
        }
        ss = None;
    }
    let total = rms.len();
    if let Some(s0) = ss
        && total - s0 >= mi
    {
        let se = total.min(s0 + msk);
        let pos = argmin(&rms, s0, se + 1);
        tags.push((pos, total + 1));
    }
    let (Some(first), Some(last)) = (tags.first().copied(), tags.last().copied()) else { return whole };
    let cut = |b: usize, e: usize| ((b * hop) as f64 / sr, (b * hop).min(w.len()), (e * hop).min(w.len()));
    let mut out = Vec::new();
    if first.0 > 0 {
        out.push(cut(0, first.0));
    }
    for pair in tags.windows(2) {
        if let [a, b] = pair {
            out.push(cut(a.1, b.0));
        }
    }
    if last.1 < total {
        out.push(cut(last.1, total));
    }
    out
}

/// Slices over 30 s cut again at their quietest 20 ms frame between 20 and 30 s.
fn split_long(w: &[f32], slices: Vec<(f64, usize, usize)>) -> Vec<(f64, usize, usize)> {
    let hop = 882usize;
    let sr = f64::from(SR);
    let mut out = Vec::new();
    for (mut off, mut a, b) in slices {
        while (b.saturating_sub(a)) as f64 > MAX_SLICE_S * sr {
            let (lo, hi) = ((20.0 * sr) as usize / hop, (MAX_SLICE_S * sr) as usize / hop);
            let e: Vec<f32> = (lo..hi)
                .map(|i| {
                    let fr: Vec<f32> = w.get(a + i * hop..(a + (i + 1) * hop).min(b)).unwrap_or_default().iter().map(|x| x * x).collect();
                    if fr.is_empty() { 0.0 } else { np_mean_f32(&fr) }
                })
                .collect();
            let k = (lo + argmin(&e, 0, e.len())) * hop;
            out.push((off, a, a + k));
            off += k as f64 / sr;
            a += k;
        }
        out.push((off, a, b));
    }
    out
}

/// The GAME graphs, loaded.
struct Graphs {
    g: Vec<OnnxModel>,
}

fn take(out: &mut Vec<(String, AnyTensor)>, name: &str) -> Result<AnyTensor> {
    let i = out.iter().position(|(n, _)| n == name).ok_or_else(|| MlError::Model(ID.into(), format!("no output `{name}`")))?;
    Ok(out.remove(i).1)
}

fn first(out: Vec<(String, AnyTensor)>) -> Result<AnyTensor> {
    out.into_iter().next().map(|x| x.1).ok_or_else(|| MlError::Model(ID.into(), "no output".into()))
}

/// What one slice made: durations (s), presence and pitch per note.
#[derive(Debug, Clone, PartialEq)]
pub struct SliceOut {
    pub durations: Vec<f32>,
    pub presence: Vec<bool>,
    pub scores: Vec<f32>,
}

impl Graphs {
    fn open(model: &InstalledModel, device: Device, ctx: &Ctx) -> Result<Graphs> {
        let mut g = Vec::with_capacity(GRAPHS.len());
        for name in GRAPHS {
            g.push(OnnxModel::open_model_file(model, &format!("{name}.onnx"), device, ctx.cancel_flag())?);
        }
        Ok(Graphs { g })
    }

    fn graph(&mut self, i: usize) -> Result<&mut OnnxModel> {
        self.g.get_mut(i).ok_or_else(|| MlError::Model(ID.into(), "missing graph".into()))
    }

    fn slice(&mut self, w: &[f32], lang: i64, slice_index: u64, ctx: &Ctx) -> Result<SliceOut> {
        let dur = (w.len() as f64 / f64::from(SR)) as f32;
        let mut enc = self
            .graph(0)?
            .run_typed(vec![("waveform", AnyTensor::F32(vec![1, w.len() as i64], w.to_vec())), ("duration", AnyTensor::F32(vec![1], vec![dur]))])?;
        let (x_seg, x_est, mask) = (take(&mut enc, "x_seg")?, take(&mut enc, "x_est")?, take(&mut enc, "maskT")?);
        let AnyTensor::Bool(mshape, mdata) = &mask else { return Err(MlError::Model(ID.into(), "maskT is not bool".into())) };
        let t = mdata.len();
        let kb = first(self.graph(1)?.run_typed(vec![("durations", AnyTensor::F32(vec![1, 1], vec![dur])), ("maskT", mask.clone())])?)?;
        let AnyTensor::Bool(_, kbd) = kb else { return Err(MlError::Model(ID.into(), "boundaries are not bool".into())) };
        let kb: Vec<bool> = kbd.iter().zip(mdata).map(|(a, b)| *a && *b).collect();
        let kb = AnyTensor::Bool(mshape.clone(), kb);
        let mut b = kb.clone();
        for k in 0..STEPS {
            ctx.check()?;
            let tstep = k as f32 / STEPS as f32;
            b = first(self.graph(2)?.run_typed(vec![
                ("noise", AnyTensor::F32(vec![1, t as i64], noise(slice_index, k as u64, t))),
                ("x_seg", x_seg.clone()),
                ("language", AnyTensor::I64(vec![1], vec![lang])),
                ("known_boundaries", kb.clone()),
                ("prev_boundaries", b),
                ("t", AnyTensor::F32(vec![1], vec![tstep])),
                ("maskT", mask.clone()),
                ("threshold", AnyTensor::F32(Vec::new(), vec![SEG_THRESHOLD])),
                ("radius", AnyTensor::I64(Vec::new(), vec![SEG_RADIUS])),
            ])?)?;
        }
        let mut dd = self.graph(3)?.run_typed(vec![("boundaries", b.clone()), ("maskT", mask.clone())])?;
        let (durations, mask_n) = (take(&mut dd, "durations")?, take(&mut dd, "maskN")?);
        let mut est = self.graph(4)?.run_typed(vec![
            ("x_est", x_est),
            ("boundaries", b),
            ("maskT", mask),
            ("maskN", mask_n),
            ("threshold", AnyTensor::F32(Vec::new(), vec![EST_THRESHOLD])),
        ])?;
        let (presence, scores) = (take(&mut est, "presence")?, take(&mut est, "scores")?);
        match (durations, presence, scores) {
            (AnyTensor::F32(_, durations), AnyTensor::Bool(_, presence), AnyTensor::F32(_, scores)) => Ok(SliceOut { durations, presence, scores }),
            _ => Err(MlError::Model(ID.into(), "unexpected output types".into())),
        }
    }
}

/// One slice's notes `(start, end, pitch)`: `float32` cumulative durations clamped to the slice.
pub fn notes_from_slice(offset: f64, length_s: f64, o: &SliceOut) -> Vec<(f64, f64, f64)> {
    let (len32, off32) = (length_s as f32, offset as f32);
    let mut out = Vec::new();
    let mut acc = 0.0f32;
    for (i, d) in o.durations.iter().enumerate() {
        let on = acc.min(len32) + off32;
        acc += d;
        let off = acc.min(len32) + off32;
        let (a, b) = (f64::from(on), f64::from(off));
        if b - a <= 0.0 || !o.presence.get(i).copied().unwrap_or(false) {
            continue;
        }
        out.push((a, b, f64::from(o.scores.get(i).copied().unwrap_or(0.0))));
    }
    out
}

/// All slices' notes, sorted and made monophonic (each starts no earlier than the last ended).
pub fn combine(mut notes: Vec<(f64, f64, f64)>) -> Vec<(f64, f64, f64)> {
    notes.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.total_cmp(&y.1)).then(x.2.total_cmp(&y.2)));
    let mut last = 0.0f64;
    let mut res = Vec::new();
    for (a, b, p) in notes {
        let a = a.max(last);
        let b = b.max(a);
        if b <= a {
            continue;
        }
        last = b;
        res.push((a, b, p));
    }
    res
}

/// Every slice of `audio` (mono at 44.1 kHz) through the graphs: `(offset s, length s, outputs)`,
/// and the device it ran on.
pub fn run_slices(model: &InstalledModel, audio: &[f32], device: Device, ctx: &Ctx) -> Result<(Vec<(f64, f64, SliceOut)>, String)> {
    ctx.say("Loading GAME");
    let mut g = Graphs::open(model, device, ctx)?;
    let label = g.g.first().map_or("CPU", OnnxModel::device_label).to_string();
    ctx.say(&format!("Transcribing on the {label}"));
    let slices = split_long(audio, slice_audio(audio));
    let n = slices.len().max(1);
    let mut out = Vec::new();
    for (i, (off, a, b)) in slices.into_iter().enumerate() {
        ctx.check()?;
        let w = audio.get(a..b).unwrap_or_default();
        if !w.is_empty() {
            out.push((off, w.len() as f64 / f64::from(SR), g.slice(w, LANG_EN, i as u64, ctx)?));
        }
        ctx.report((i + 1) as f32 / n as f32);
    }
    let note = g.g.first().and_then(|m| m.fallback_note.clone()).unwrap_or(label);
    Ok((out, note))
}

/// The notes of `audio` (mono at 44.1 kHz): `(start, end, fractional MIDI pitch)`.
pub fn run(model: &InstalledModel, audio: &[f32], device: Device, ctx: &Ctx) -> Result<(Vec<(f64, f64, f64)>, String)> {
    let (slices, note) = run_slices(model, audio, device, ctx)?;
    let raw = slices.iter().flat_map(|(off, len, o)| notes_from_slice(*off, *len, o)).collect();
    Ok((combine(raw), note))
}

/// The transcription of the notes `run` found. With pitch bends on, each note's fractional pitch
/// becomes a pitch-wheel value at its start (±2 semitones = ±8192).
pub fn decode_all(raw: &[(f64, f64, f64)], opts: &MidiOptions) -> Transcription {
    let mut notes = Vec::with_capacity(raw.len());
    let mut bends = Vec::new();
    for &(a, b, p) in raw {
        let pitch = round_even(p).clamp(0.0, 127.0);
        if opts.pitch_bends {
            bends.push((a, ((p - pitch) * 4096.0).round().clamp(-8192.0, 8191.0) as i32));
        }
        notes.push(NoteEvent { start: a, end: b, pitch: pitch as u8, velocity: VELOCITY, bends: Vec::new() });
    }
    Transcription { notes, pedals: Vec::new(), bends, drums: false }
}

/// Transcribe `audio` (mono at 44.1 kHz).
pub fn transcribe(model: &InstalledModel, audio: &[f32], opts: &MidiOptions, device: Device, ctx: &Ctx) -> Result<(Transcription, String)> {
    let (raw, note) = run(model, audio, device, ctx)?;
    Ok((decode_all(&raw, opts), note))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_splitmix64() {
        let n = noise(0, 0, 3);
        assert!(n.iter().all(|v| (0.0..1.0).contains(v)));
        assert_ne!(n[0], n[1]);
        assert_eq!(noise(1, 2, 3), noise(1, 2, 3));
        assert_ne!(noise(1, 2, 3), noise(1, 3, 3));
    }

    #[test]
    fn slices_at_silence() {
        let sr = SR as usize;
        let tone = |n: usize| (0..n).map(|i| 0.3 * (i as f32 * 0.05).sin()).collect::<Vec<f32>>();
        let mut w = tone(2 * sr);
        w.extend(vec![0.0; sr]);
        w.extend(tone(2 * sr));
        let s = slice_audio(&w);
        assert_eq!(s.len(), 2, "{s:?}");
        assert_eq!(s[0].1, 0);
        assert!(s[1].0 > 2.0 && s[1].0 < 3.0);
        // Short input: one slice.
        assert_eq!(slice_audio(&tone(1000)), vec![(0.0, 0, 1000)]);
    }

    #[test]
    fn combine_is_monophonic() {
        let c = combine(vec![(1.0, 2.0, 60.0), (0.0, 1.5, 62.0), (1.2, 1.4, 64.0)]);
        assert_eq!(c, vec![(0.0, 1.5, 62.0), (1.5, 2.0, 60.0)]);
    }
}
