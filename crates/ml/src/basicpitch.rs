//! Spotify Basic Pitch (ICASSP 2022): polyphonic, instrument-agnostic audio → notes (+ pitch
//! bends). Port of `ml-models/basicpitch/reference_infer.py` (network windows and the
//! `basic_pitch.note_creation` decoder with `infer_onsets` and the melodia trick).
//!
//! Model `basicpitch.onnx` (the harmonic CQT front end is inside the graph): input
//! `[N, 43844, 1]` mono at 22050 Hz (1.988 s windows); outputs note and onset `[N, 172, 88]` and
//! contour `[N, 172, 264]` (3 bins per semitone), 86 frames per second, pitches MIDI 21..=108.

use crate::job::Ctx;
use crate::midi::{MidiOptions, NoteEvent, Transcription, np_mean_f32, round_even};
use crate::models::InstalledModel;
use crate::runtime::{Device, OnnxModel, Tensor};
use crate::{MlError, Result};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

pub const ID: &str = "basicpitch";
pub const SR: u32 = 22_050;
const HOP: usize = 256;
const N_SAMPLES: usize = 43_844;
const N_OLAP_FRAMES: usize = 30;
const OLAP: usize = N_OLAP_FRAMES * HOP;
const WIN_HOP: usize = N_SAMPLES - OLAP;
/// Frames per second (integer, as Basic Pitch: 22050 // 256).
const FPS: usize = SR as usize / HOP;
const ANNOT_N_FRAMES: usize = FPS * 2;
const MIDI_OFFSET: usize = 21;
pub const N_PITCH: usize = 88;
const MAX_FREQ_IDX: usize = 87;
pub const N_CONTOUR: usize = 264;
const IN_NAME: &str = "serving_default_input_2:0";
const OUT_NOTE: &str = "StatefulPartitionedCall:1";
const OUT_ONSET: &str = "StatefulPartitionedCall:2";
const OUT_CONTOUR: &str = "StatefulPartitionedCall:0";

pub const ONSET_THRESHOLD: f32 = 0.5;
pub const FRAME_THRESHOLD: f32 = 0.3;
/// Basic Pitch's default `minimum_note_length` (ms): 11 frames.
pub const DEFAULT_MIN_NOTE_MS: f32 = 127.7;
const ENERGY_TOL: usize = 11;

/// The network's outputs over the whole input: `[frames][88]` note and onset, `[frames][264]`
/// contour, row-major.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Raw {
    pub frames: usize,
    pub note: Vec<f32>,
    pub onset: Vec<f32>,
    pub contour: Vec<f32>,
}

/// Run the network over `audio` (mono f32 at 22050 Hz).
pub fn run_network(onnx: &mut OnnxModel, audio: &[f32], ctx: &Ctx) -> Result<Raw> {
    let l = audio.len();
    let mut x = vec![0.0f32; OLAP / 2];
    x.extend_from_slice(audio);
    let i_note = onnx.output_index(OUT_NOTE)?;
    let i_onset = onnx.output_index(OUT_ONSET)?;
    let i_contour = onnx.output_index(OUT_CONTOUR)?;
    let starts: Vec<usize> = (0..x.len()).step_by(WIN_HOP).collect();
    let h = N_OLAP_FRAMES / 2;
    let keep_per = ANNOT_N_FRAMES - 2 * h;
    let mut raw = Raw::default();
    let n_keep = (l as f64 * (FPS as f64 / f64::from(SR))).floor() as usize;
    let n = starts.len().max(1);
    for (k, &i) in starts.iter().enumerate() {
        ctx.check()?;
        let mut w: Vec<f32> = x.get(i..(i + N_SAMPLES).min(x.len())).unwrap_or_default().to_vec();
        w.resize(N_SAMPLES, 0.0);
        let out = onnx.run_named(&[(IN_NAME, &Tensor::new(vec![1, N_SAMPLES as i64, 1], w))])?;
        for (idx, width, dst) in [(i_note, N_PITCH, &mut raw.note), (i_onset, N_PITCH, &mut raw.onset), (i_contour, N_CONTOUR, &mut raw.contour)] {
            let t = out.get(idx).ok_or_else(|| MlError::Model(ID.into(), "missing output".into()))?;
            if t.data.len() != ANNOT_N_FRAMES * width {
                return Err(MlError::Model(ID.into(), format!("output shape {:?}, expected [1, {ANNOT_N_FRAMES}, {width}]", t.shape)));
            }
            dst.extend_from_slice(t.data.get(h * width..(h + keep_per) * width).unwrap_or_default());
        }
        ctx.report((k + 1) as f32 / n as f32);
    }
    let have = raw.note.len() / N_PITCH;
    raw.frames = n_keep.min(have);
    raw.note.truncate(raw.frames * N_PITCH);
    raw.onset.truncate(raw.frames * N_PITCH);
    raw.contour.truncate(raw.frames * N_CONTOUR);
    Ok(raw)
}

/// `basic_pitch.note_creation.get_infered_onsets` (n_diff 2), in `f64`.
fn inferred_onsets(onsets: &[f32], frames: &[f32], t: usize) -> Vec<f64> {
    let n_diff = 2usize;
    let at = |r: isize, f: usize| -> f64 { if r < 0 { 0.0 } else { f64::from(frames.get(r as usize * N_PITCH + f).copied().unwrap_or(0.0)) } };
    let mut fd = vec![0.0f64; t * N_PITCH];
    for r in 0..t {
        for f in 0..N_PITCH {
            let mut m = f64::INFINITY;
            for n in 1..=n_diff {
                let d = at(r as isize, f) - at(r as isize - n as isize, f);
                m = m.min(d);
            }
            let v = if r < n_diff || m < 0.0 { 0.0 } else { m };
            if let Some(c) = fd.get_mut(r * N_PITCH + f) {
                *c = v;
            }
        }
    }
    let max_on = onsets.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let max_fd = fd.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if max_fd.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        // numpy: 0/0 is NaN everywhere, and np.max([...]) propagates it: no onsets at all.
        return vec![f64::NAN; t * N_PITCH];
    }
    let max_on = f64::from(max_on);
    onsets.iter().zip(&fd).map(|(o, d)| f64::from(*o).max(max_on * d / max_fd)).collect()
}

/// `(start frame, end frame, MIDI pitch, amplitude)`.
pub type FrameNote = (usize, usize, u8, f32);

#[derive(PartialEq)]
struct Cell(f64, usize);
impl Eq for Cell {}
impl PartialOrd for Cell {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Cell {
    /// Largest value first; on ties the lowest index (numpy's argmax).
    fn cmp(&self, o: &Self) -> Ordering {
        self.0.total_cmp(&o.0).then_with(|| o.1.cmp(&self.1))
    }
}

/// The polyphonic decoder (`output_to_notes_polyphonic` with `infer_onsets` and
/// `melodia_trick`) on `[t][88]` note and onset activations.
pub fn output_to_notes(frames: &[f32], onsets: &[f32], t: usize, onset_thresh: f64, frame_thresh: f64, min_note_len: usize) -> Vec<FrameNote> {
    let n_frames = t;
    if t == 0 || frames.len() < t * N_PITCH || onsets.len() < t * N_PITCH {
        return Vec::new();
    }
    let on = inferred_onsets(onsets, frames, t);
    let get = |v: &[f64], r: usize, f: usize| v.get(r * N_PITCH + f).copied().unwrap_or(0.0);
    // Strict local maxima in time (scipy argrelmax, order 1, mode clip), at or above the threshold,
    // visited backwards in time (and in pitch).
    let mut peaks: Vec<(usize, usize)> = Vec::new();
    for r in 1..n_frames.saturating_sub(1) {
        for f in 0..N_PITCH {
            let v = get(&on, r, f);
            if v > get(&on, r - 1, f) && v > get(&on, r + 1, f) && v >= onset_thresh {
                peaks.push((r, f));
            }
        }
    }
    peaks.reverse();
    let mut rem: Vec<f64> = frames.iter().take(t * N_PITCH).map(|v| f64::from(*v)).collect();
    let col = |r0: usize, r1: usize, f: usize| -> Vec<f32> { (r0..r1).filter_map(|r| frames.get(r * N_PITCH + f).copied()).collect() };
    let zero = |rem: &mut Vec<f64>, r: usize, f: usize| {
        if let Some(c) = rem.get_mut(r * N_PITCH + f) {
            *c = 0.0;
        }
    };
    let mut notes = Vec::new();
    for (s, f) in peaks {
        if s >= n_frames - 1 {
            continue;
        }
        let (mut i, mut k) = (s + 1, 0usize);
        while i < n_frames - 1 && k < ENERGY_TOL {
            k = if get(&rem, i, f) < frame_thresh { k + 1 } else { 0 };
            i += 1;
        }
        i -= k;
        if i - s <= min_note_len {
            continue;
        }
        for r in s..i {
            zero(&mut rem, r, f);
            if f < MAX_FREQ_IDX {
                zero(&mut rem, r, f + 1);
            }
            if f > 0 {
                zero(&mut rem, r, f - 1);
            }
        }
        notes.push((s, i, (f + MIDI_OFFSET) as u8, np_mean_f32(&col(s, i, f))));
    }
    // The melodia trick: grow notes from the remaining energy maxima. Values only ever drop to
    // zero, so a max-heap of the cells above the threshold gives numpy's argmax in order.
    let mut heap: BinaryHeap<Cell> = rem.iter().enumerate().filter(|(_, v)| **v > frame_thresh).map(|(i, v)| Cell(*v, i)).collect();
    while let Some(Cell(v, idx)) = heap.pop() {
        if rem.get(idx).copied() != Some(v) {
            continue; // zeroed since
        }
        let (im, f) = (idx / N_PITCH, idx % N_PITCH);
        zero(&mut rem, im, f);
        let clear = |rem: &mut Vec<f64>, i: usize| {
            zero(rem, i, f);
            if f < MAX_FREQ_IDX {
                zero(rem, i, f + 1);
            }
            if f > 0 {
                zero(rem, i, f - 1);
            }
        };
        let (mut i, mut k) = (im + 1, 0usize);
        while i < n_frames - 1 && k < ENERGY_TOL {
            k = if get(&rem, i, f) < frame_thresh { k + 1 } else { 0 };
            clear(&mut rem, i);
            i += 1;
        }
        let i_end = i as isize - 1 - k as isize;
        let (mut i, mut k) = (im as isize - 1, 0usize);
        while i > 0 && k < ENERGY_TOL {
            k = if get(&rem, i as usize, f) < frame_thresh { k + 1 } else { 0 };
            clear(&mut rem, i as usize);
            i -= 1;
        }
        let i_start = i + 1 + k as isize;
        if i_end - i_start <= min_note_len as isize {
            continue;
        }
        let (a, b) = (i_start.max(0) as usize, i_end.max(0) as usize);
        notes.push((a, b, (f + MIDI_OFFSET) as u8, np_mean_f32(&col(a, b, f))));
    }
    notes
}

/// Per-note pitch-bend offsets (1/3 semitone per frame) from the contour.
pub fn pitch_bends(contour: &[f32], notes: &[FrameNote]) -> Vec<Vec<i32>> {
    const TOL: usize = 25;
    let wl = 2 * TOL + 1;
    let gauss: Vec<f64> = (0..wl).map(|i| (-0.5 * (i as f64 - TOL as f64).powi(2) / 25.0).exp()).collect();
    notes
        .iter()
        .map(|&(s, e, p, _)| {
            let hz = 440.0 * 2f64.powf((f64::from(p) - 69.0) / 12.0);
            let fi = round_even(12.0 * 3.0 * (hz / 27.5).log2()) as isize;
            let a = (fi - TOL as isize).max(0) as usize;
            let b = ((fi + TOL as isize + 1).max(0) as usize).min(N_CONTOUR);
            let g0 = (TOL as isize - fi).max(0) as usize;
            let shift = TOL as isize - (TOL as isize - fi).max(0);
            (s..e)
                .map(|r| {
                    let mut best = (f64::NEG_INFINITY, 0usize);
                    for (j, c) in (a..b).enumerate() {
                        let v = f64::from(contour.get(r * N_CONTOUR + c).copied().unwrap_or(0.0)) * gauss.get(g0 + j).copied().unwrap_or(0.0);
                        if v > best.0 {
                            best = (v, j);
                        }
                    }
                    best.1 as i32 - shift as i32
                })
                .collect()
        })
        .collect()
}

/// Basic Pitch's frame → time mapping (with its 0.0018 s correction).
pub fn frame_time(f: usize) -> f64 {
    let t = (f * HOP) as f64 / f64::from(SR);
    let off = (HOP as f64 / f64::from(SR)) * (ANNOT_N_FRAMES as f64 - (N_SAMPLES as f64 / HOP as f64)) + 0.0018;
    t - off * (f as f64 / ANNOT_N_FRAMES as f64).floor()
}

/// Notes of the raw outputs with `opts` (thresholds, shortest note).
pub fn decode(raw: &Raw, opts: &MidiOptions) -> Vec<NoteEvent> {
    let min_len = round_even(f64::from(opts.min_ms()) / 1000.0 * (f64::from(SR) / HOP as f64)).max(0.0) as usize;
    let ev = output_to_notes(&raw.note, &raw.onset, raw.frames, f64::from(opts.onset()), f64::from(opts.frame()), min_len);
    let bends = pitch_bends(&raw.contour, &ev);
    let mut notes: Vec<NoteEvent> = ev
        .iter()
        .zip(bends)
        .map(|(&(s, e, p, a), b)| NoteEvent {
            start: frame_time(s),
            end: frame_time(e),
            pitch: p,
            velocity: round_even(127.0 * f64::from(a)).clamp(0.0, 127.0) as u8,
            bends: b,
        })
        .collect();
    notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    notes
}

/// Pitch-wheel events for the notes that overlap no other note (Basic Pitch's default MIDI).
pub fn midi_bends(notes: &[NoteEvent]) -> Vec<(f64, i32)> {
    let mut ok = vec![true; notes.len()];
    let mut srt: Vec<usize> = (0..notes.len()).collect();
    let key = |i: usize| notes.get(i).map_or((0.0, 0.0, 0), |n| (n.start, n.end, n.pitch));
    srt.sort_by(|&a, &b| {
        let (x, y) = (key(a), key(b));
        x.0.total_cmp(&y.0).then(x.1.total_cmp(&y.1)).then(x.2.cmp(&y.2))
    });
    for a in 0..srt.len() {
        for b in a + 1..srt.len() {
            let (ia, ib) = (srt.get(a).copied().unwrap_or(0), srt.get(b).copied().unwrap_or(0));
            if key(ib).0 >= key(ia).1 {
                break;
            }
            if let Some(v) = ok.get_mut(ia) {
                *v = false;
            }
            if let Some(v) = ok.get_mut(ib) {
                *v = false;
            }
        }
    }
    let mut ev = Vec::new();
    for (n, good) in notes.iter().zip(ok) {
        if !good || n.bends.is_empty() {
            continue;
        }
        let num = n.bends.len();
        let step = if num > 1 { (n.end - n.start) / (num - 1) as f64 } else { 0.0 };
        for (i, b) in n.bends.iter().enumerate() {
            let t = if num > 1 && i == num - 1 { n.end } else { i as f64 * step + n.start };
            let v = round_even(f64::from(*b) * 4096.0 / 3.0).clamp(-8192.0, 8191.0) as i32;
            ev.push((t, v));
        }
    }
    ev
}

/// The network over `audio` (mono at 22050 Hz): the raw outputs (everything a decode needs) and
/// where it ran.
pub fn analyse(model: &InstalledModel, audio: &[f32], device: Device, ctx: &Ctx) -> Result<(Raw, String)> {
    ctx.say("Loading Basic Pitch");
    let mut onnx = OnnxModel::open_model(model, device, ctx.cancel_flag())?;
    ctx.say(&format!("Transcribing on the {}", onnx.device_label()));
    let raw = run_network(&mut onnx, audio, &ctx.stage(0.0, 0.9))?;
    let note = onnx.fallback_note.clone().unwrap_or_else(|| onnx.device_label().to_string());
    Ok((raw, note))
}

/// The transcription of `raw` with `opts` (thresholds, shortest note, pitch bends).
pub fn decode_all(raw: &Raw, opts: &MidiOptions) -> Transcription {
    let notes = decode(raw, opts);
    let bends = if opts.pitch_bends { midi_bends(&notes) } else { Vec::new() };
    Transcription { notes, pedals: Vec::new(), bends, drums: false }
}

/// Transcribe `audio` (mono at 22050 Hz).
pub fn transcribe(model: &InstalledModel, audio: &[f32], opts: &MidiOptions, device: Device, ctx: &Ctx) -> Result<(Transcription, String)> {
    let (raw, note) = analyse(model, audio, device, ctx)?;
    ctx.say("Decoding notes");
    Ok((decode_all(&raw, opts), note))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_times_follow_basic_pitch() {
        assert_eq!(frame_time(0), 0.0);
        assert!((frame_time(1) - 256.0 / 22050.0).abs() < 1e-12);
        // Each 172-frame window shifts back by Basic Pitch's offset.
        let off = (256.0 / 22050.0) * (172.0 - 43844.0 / 256.0) + 0.0018;
        assert!((frame_time(172) - (172.0 * 256.0 / 22050.0 - off)).abs() < 1e-12);
    }

    #[test]
    fn decodes_a_held_note_and_ignores_noise() {
        let t = 60;
        let mut frames = vec![0.0f32; t * N_PITCH];
        let mut onsets = vec![0.0f32; t * N_PITCH];
        // MIDI 60 (index 39) from frame 10 to 40.
        for r in 10..40 {
            frames[r * N_PITCH + 39] = 0.8;
        }
        onsets[10 * N_PITCH + 39] = 0.9;
        let notes = output_to_notes(&frames, &onsets, t, 0.5, 0.3, 11);
        assert_eq!(notes.len(), 1, "{notes:?}");
        let (s, e, p, a) = notes[0];
        assert_eq!((s, e, p), (10, 40, 60));
        assert!((a - 0.8).abs() < 1e-6);
        assert!(output_to_notes(&[], &[], 0, 0.5, 0.3, 11).is_empty());
        // Silence: no notes, no NaN trouble.
        assert!(output_to_notes(&vec![0.0; t * N_PITCH], &vec![0.0; t * N_PITCH], t, 0.5, 0.3, 11).is_empty());
    }

    #[test]
    fn bends_only_on_lone_notes() {
        let n = |s: f64, e: f64, b: Vec<i32>| NoteEvent { start: s, end: e, pitch: 60, velocity: 90, bends: b };
        let ev = midi_bends(&[n(0.0, 1.0, vec![0, 3]), n(0.5, 2.0, vec![1]), n(3.0, 4.0, vec![0, 3, -3])]);
        assert_eq!(ev, vec![(3.0, 0), (3.5, 4096), (4.0, -4096)]);
    }
}
