//! ByteDance high-resolution piano transcription (Kong et al. 2020): notes with velocities and
//! the sustain pedal. Port of `ml-models/piano_kong/reference_infer.py`.
//!
//! Model `piano_kong.onnx` (log-mel front end inside the graph): input `audio` `[1, 160000]`
//! (10 s mono at 16 kHz); seven outputs `[1, 1001, K]` at 100 frames per second: `reg_onset`,
//! `reg_offset`, `frame`, `velocity` (K = 88 keys, MIDI 21..=108), `reg_pedal_onset`,
//! `reg_pedal_offset`, `pedal_frame` (K = 1).

use crate::job::Ctx;
use crate::midi::{MidiOptions, NoteEvent, Transcription};
use crate::models::InstalledModel;
use crate::runtime::{Device, OnnxModel, Tensor};
use crate::{MlError, Result};

pub const ID: &str = "piano_kong";
pub const SR: u32 = 16_000;
const SEG: usize = 160_000;
const FPS: f64 = 100.0;
const BEGIN_NOTE: usize = 21;
pub const KEYS: usize = 88;
pub const ONSET_THRESHOLD: f32 = 0.3;
const OFFSET_THRESHOLD: f32 = 0.3;
const FRAME_THRESHOLD: f32 = 0.1;
const PEDAL_OFFSET_THRESHOLD: f32 = 0.2;
const PEDAL_FRAME_THRESHOLD: f32 = 0.5;

/// The output names, in [`Raw`] order.
pub const OUTS: [&str; 7] = ["reg_onset", "reg_offset", "frame", "velocity", "reg_pedal_onset", "reg_pedal_offset", "pedal_frame"];

/// The network's outputs over the whole input, each `[frames][K]` row-major, in [`OUTS`] order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Raw {
    pub frames: usize,
    pub outs: Vec<Vec<f32>>,
}

impl Raw {
    fn get(&self, i: usize) -> &[f32] {
        self.outs.get(i).map_or(&[], Vec::as_slice)
    }
}

fn width(i: usize) -> usize {
    if i < 4 { KEYS } else { 1 }
}

/// Run the network over `audio` (mono f32 at 16 kHz): 10 s segments every 5 s, stitched as the
/// reference does (the first keeps its first 3/4, the middle ones their middle half, the last its
/// last 3/4).
pub fn run_network(onnx: &mut OnnxModel, audio: &[f32], ctx: &Ctx) -> Result<Raw> {
    let l = audio.len();
    let padded = l.div_ceil(SEG) * SEG;
    let mut x = audio.to_vec();
    x.resize(padded, 0.0);
    let mut starts = Vec::new();
    let mut p = 0;
    while p + SEG <= x.len() {
        starts.push(p);
        p += SEG / 2;
    }
    let idx: Vec<usize> = OUTS.iter().map(|n| onnx.output_index(n)).collect::<Result<_>>()?;
    let mut segs: Vec<Vec<Vec<f32>>> = Vec::with_capacity(starts.len());
    let mut seg_frames = 0usize;
    let n = starts.len().max(1);
    for (k, &s) in starts.iter().enumerate() {
        ctx.check()?;
        let chunk = x.get(s..s + SEG).unwrap_or_default().to_vec();
        let out = onnx.run_named(&[("audio", &Tensor::new(vec![1, SEG as i64], chunk))])?;
        let mut v = Vec::with_capacity(OUTS.len());
        for (o, &i) in idx.iter().enumerate() {
            let t = out.get(i).ok_or_else(|| MlError::Model(ID.into(), "missing output".into()))?;
            let w = width(o);
            if t.data.is_empty() || t.data.len() % w != 0 {
                return Err(MlError::Model(ID.into(), format!("output `{}` shape {:?}", OUTS[o], t.shape)));
            }
            let f = t.data.len() / w;
            if seg_frames == 0 {
                seg_frames = f;
            } else if f != seg_frames {
                return Err(MlError::Model(ID.into(), format!("output `{}` has {f} frames, expected {seg_frames}", OUTS[o])));
            }
            v.push(t.data.clone());
        }
        segs.push(v);
        ctx.report((k + 1) as f32 / n as f32);
    }
    let mut raw = Raw { frames: 0, outs: vec![Vec::new(); OUTS.len()] };
    if segs.len() == 1 {
        raw.outs = segs.pop().unwrap_or_default();
        raw.frames = seg_frames;
        return Ok(raw);
    }
    // Drop each segment's last frame, then keep [0, 3n/4) of the first, [n/4, 3n/4) of the
    // middle ones and [n/4, n) of the last.
    let nf = seg_frames.saturating_sub(1);
    let (q1, q3) = ((nf as f64 * 0.25) as usize, (nf as f64 * 0.75) as usize);
    let last = segs.len().saturating_sub(1);
    for (si, seg) in segs.iter().enumerate() {
        let (a, b) = if si == 0 {
            (0, q3)
        } else if si == last {
            (q1, nf)
        } else {
            (q1, q3)
        };
        for (o, data) in seg.iter().enumerate() {
            let w = width(o);
            if let (Some(dst), Some(src)) = (raw.outs.get_mut(o), data.get(a * w..b * w)) {
                dst.extend_from_slice(src);
            }
        }
        raw.frames += b - a;
    }
    Ok(raw)
}

/// Onset/offset regression → hits with sub-frame shifts (`f32`, as the reference).
fn binarize(reg: &[f32], t: usize, k: usize, key: usize, thr: f32, nb: usize) -> (Vec<bool>, Vec<f32>) {
    let x = |n: usize| reg.get(n * k + key).copied().unwrap_or(0.0);
    let mut out = vec![false; t];
    let mut shift = vec![0.0f32; t];
    for n in nb..t.saturating_sub(nb) {
        let v = x(n);
        if v.partial_cmp(&thr) != Some(std::cmp::Ordering::Greater) {
            continue;
        }
        let mono = (0..nb).all(|i| !(x(n - i) < x(n - i - 1) || x(n + i) < x(n + i + 1)));
        if !mono {
            continue;
        }
        if let Some(o) = out.get_mut(n) {
            *o = true;
        }
        let (prev, next) = (x(n - 1), x(n + 1));
        let s = if prev > next { (next - prev) / (v - next) / 2.0 } else { (next - prev) / (v - prev) / 2.0 };
        if let Some(o) = shift.get_mut(n) {
            *o = s;
        }
    }
    (out, shift)
}

/// `[bgn, fin, onset shift, offset shift, velocity]` of one key.
type Row = (usize, usize, f32, f32, f32);

/// Python truthiness of an optional frame index (`0` is false).
fn on(v: Option<usize>) -> bool {
    v.is_some_and(|v| v != 0)
}

/// The reference's `note_detection` for one key.
fn detect_notes(frame: &[f32], onset: &[bool], onset_shift: &[f32], offset: &[bool], offset_shift: &[f32], vel: &[f32], frame_thr: f32) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    let (mut bgn, mut fd, mut oo): (Option<usize>, Option<usize>, Option<usize>) = (None, None, None);
    let t = onset.len();
    let at = |v: &[f32], i: usize| v.get(i).copied().unwrap_or(0.0);
    let hit = |v: &[bool], i: usize| v.get(i).copied().unwrap_or(false);
    for i in 0..t {
        if hit(onset, i) {
            if let Some(b) = bgn.filter(|b| *b != 0) {
                out.push((b, i.saturating_sub(1), at(onset_shift, b), 0.0, at(vel, b)));
                fd = None;
                oo = None;
            }
            bgn = Some(i);
        }
        if let Some(b) = bgn.filter(|b| *b != 0 && i > *b) {
            if at(frame, i) <= frame_thr && !on(fd) {
                fd = Some(i);
            }
            if hit(offset, i) && !on(oo) {
                oo = Some(i);
            }
            if let Some(f) = fd.filter(|f| *f != 0) {
                let fin = match oo.filter(|o| *o != 0) {
                    Some(o) if o as isize - b as isize > f as isize - o as isize => o,
                    _ => f,
                };
                out.push((b, fin, at(onset_shift, b), at(offset_shift, fin), at(vel, b)));
                bgn = None;
                fd = None;
                oo = None;
            }
            if on(bgn) && (i - b >= 600 || i == t - 1) {
                out.push((b, i, at(onset_shift, b), at(offset_shift, i), at(vel, b)));
                bgn = None;
                fd = None;
                oo = None;
            }
        }
    }
    out.sort_by_key(|r| r.0);
    out
}

/// The reference's `pedal_detection`.
fn detect_pedals(frame: &[f32], offset: &[bool], offset_shift: &[f32], frame_thr: f32) -> Vec<(usize, usize, f32)> {
    let mut out = Vec::new();
    let (mut bgn, mut fd, mut oo): (Option<usize>, Option<usize>, Option<usize>) = (None, None, None);
    let at = |v: &[f32], i: usize| v.get(i).copied().unwrap_or(0.0);
    for i in 1..frame.len() {
        if at(frame, i) >= frame_thr && at(frame, i) > at(frame, i - 1) && !on(bgn) {
            bgn = Some(i);
        }
        if let Some(b) = bgn.filter(|b| *b != 0 && i > *b) {
            if at(frame, i) <= frame_thr && !on(fd) {
                fd = Some(i);
            }
            if offset.get(i).copied().unwrap_or(false) && !on(oo) {
                oo = Some(i);
            }
            if let Some(o) = oo.filter(|o| *o != 0) {
                out.push((b, o, at(offset_shift, o)));
                bgn = None;
                fd = None;
                oo = None;
            }
            if let Some(f) = fd.filter(|f| *f != 0)
                && i - f >= 10
            {
                out.push((b, f, at(offset_shift, f)));
                bgn = None;
                fd = None;
                oo = None;
            }
        }
    }
    out.sort_by_key(|r| r.0);
    out
}

/// Seconds as the reference computes them: `float32((frame + shift) / 100)`.
fn secs(frame: usize, shift: f32) -> f64 {
    f64::from(((frame as f64 + f64::from(shift)) / FPS) as f32)
}

/// Notes and pedals of the raw outputs. `onset` is the onset threshold.
pub fn decode(raw: &Raw, onset: f32, pedal: bool) -> (Vec<NoteEvent>, Vec<(f64, f64)>) {
    let t = raw.frames;
    let mut notes = Vec::new();
    for key in 0..KEYS {
        let (on_hit, on_sh) = binarize(raw.get(0), t, KEYS, key, onset, 2);
        let (off_hit, off_sh) = binarize(raw.get(1), t, KEYS, key, OFFSET_THRESHOLD, 4);
        let col = |o: usize| -> Vec<f32> { (0..t).map(|n| raw.get(o).get(n * KEYS + key).copied().unwrap_or(0.0)).collect() };
        for (b, f, s_on, s_off, v) in detect_notes(&col(2), &on_hit, &on_sh, &off_hit, &off_sh, &col(3), FRAME_THRESHOLD) {
            let vel = (v * 128.0).trunc().clamp(0.0, 127.0);
            notes.push(NoteEvent {
                start: secs(b, s_on),
                end: secs(f, s_off),
                pitch: (key + BEGIN_NOTE) as u8,
                velocity: vel as u8,
                bends: Vec::new(),
            });
        }
    }
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    let mut pedals = Vec::new();
    if pedal {
        let (p_off, p_sh) = binarize(raw.get(5), t, 1, 0, PEDAL_OFFSET_THRESHOLD, 4);
        for (b, f, sh) in detect_pedals(raw.get(6), &p_off, &p_sh, PEDAL_FRAME_THRESHOLD) {
            pedals.push((secs(b, 0.0), secs(f, sh)));
        }
    }
    (notes, pedals)
}

/// The network over `audio` (mono at 16 kHz): the raw outputs (everything a decode needs) and
/// where it ran.
pub fn analyse(model: &InstalledModel, audio: &[f32], device: Device, ctx: &Ctx) -> Result<(Raw, String)> {
    ctx.say("Loading the piano model");
    let mut onnx = OnnxModel::open_model(model, device, ctx.cancel_flag())?;
    ctx.say(&format!("Transcribing on the {}", onnx.device_label()));
    let raw = run_network(&mut onnx, audio, &ctx.stage(0.0, 0.95))?;
    let note = onnx.fallback_note.clone().unwrap_or_else(|| onnx.device_label().to_string());
    Ok((raw, note))
}

/// The transcription of `raw` with `opts` (onset threshold, pedal).
pub fn decode_all(raw: &Raw, opts: &MidiOptions) -> Transcription {
    let (notes, pedals) = decode(raw, opts.onset(), opts.pedal);
    // The model can't hear a note under velocity 1.
    let notes = notes
        .into_iter()
        .map(|mut n| {
            n.velocity = n.velocity.max(1);
            n
        })
        .collect();
    Transcription { notes, pedals, bends: Vec::new(), drums: false }
}

/// Transcribe `audio` (mono at 16 kHz).
pub fn transcribe(model: &InstalledModel, audio: &[f32], opts: &MidiOptions, device: Device, ctx: &Ctx) -> Result<(Transcription, String)> {
    let (raw, note) = analyse(model, audio, device, ctx)?;
    ctx.say("Decoding notes");
    Ok((decode_all(&raw, opts), note))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binarize_finds_a_peak_with_its_shift() {
        let reg = [0.0f32, 0.1, 0.4, 0.9, 0.5, 0.2, 0.0, 0.0];
        let (hit, sh) = binarize(&reg, reg.len(), 1, 0, 0.3, 2);
        assert_eq!(hit.iter().position(|h| *h), Some(3));
        assert!((sh[3] - (0.5f32 - 0.4) / (0.9 - 0.4) / 2.0).abs() < 1e-7);
        assert_eq!(hit.iter().filter(|h| **h).count(), 1);
    }

    #[test]
    fn a_note_runs_from_onset_to_where_the_frame_drops() {
        let t = 40;
        let mut frame = vec![0.0f32; t];
        frame[5..20].iter_mut().for_each(|v| *v = 0.9);
        let mut onset = vec![false; t];
        onset[5] = true;
        let rows = detect_notes(&frame, &onset, &vec![0.25; t], &vec![false; t], &vec![0.0; t], &vec![0.5; t], 0.1);
        assert_eq!(rows, vec![(5, 20, 0.25, 0.0, 0.5)]);
        // An onset at frame 0 never opens a note (the reference's `if bgn`).
        let mut onset0 = vec![false; t];
        onset0[0] = true;
        assert!(detect_notes(&frame, &onset0, &vec![0.0; t], &vec![false; t], &vec![0.0; t], &vec![0.5; t], 0.1).is_empty());
    }

    #[test]
    fn pedal_down_and_up() {
        let mut frame = vec![0.0f32; 60];
        frame[10..30].iter_mut().for_each(|v| *v = 0.9);
        let p = detect_pedals(&frame, &[false; 60], &[0.0; 60], 0.5);
        assert_eq!(p, vec![(10, 30, 0.0)]);
    }
}
