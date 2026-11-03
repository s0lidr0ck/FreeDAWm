"""Basic Pitch (Spotify, ICASSP 2022) reference pipeline: numpy + onnxruntime only.
THIS IS THE SPEC THE RUST LANE PORTS. Polyphonic, instrument-agnostic audio -> notes (+ pitch bends).

Model: basicpitch.onnx (Spotify's own nmp.onnx; the harmonic CQT front end is inside the graph).
  input  "serving_default_input_2:0" f32 [N, 43844, 1]   (mono, 22050 Hz, 1.988 s windows)
  output "StatefulPartitionedCall:1" note    f32 [N, 172, 88]
         "StatefulPartitionedCall:2" onset   f32 [N, 172, 88]
         "StatefulPartitionedCall:0" contour f32 [N, 172, 264]  (3 bins per semitone)
  88 pitches = MIDI 21..108; 86 frames per second (hop 256 at 22050 Hz).

1. Host: downmix to mono (mean), resample to 22050 Hz. L = number of samples.
2. Windows: x = [3840 zeros] + audio (3840 = 15 frames * 256). For i = 0, 36164, 72328, ... while
   i < len(x): w = x[i : i + 43844], zero-padded on the right to 43844. Run each window.
3. Unwrap: drop 15 frames at the start and end of every window's output (172 -> 142),
   concatenate in order, keep the first floor(L * 86 / 22050) frames.
4. Decode (basic_pitch.note_creation, defaults onset 0.5, frame 0.3, min length 11 frames,
   infer_onsets, melodia_trick, energy_tol 11) -> notes in frames, see output_to_notes().
5. Pitch bends: for each note, contour rows [start, end) around the note's contour bin
   (+-25 bins) weighted by a Gaussian (std 5); argmax - offset per frame, in 1/3 semitones.
6. Times: frame f -> f * 256 / 22050 - floor(f / 172) * (256/22050 * (172 - 43844/256) + 0.0018)
   (Basic Pitch's own frame-to-time mapping, including its magic 0.0018 s). Velocity =
   round(127 * mean(note activation over the note)).
Output: notes = [{start, end, pitch, velocity, bends}] sorted by (start, pitch). `bends` are
per-frame offsets in 1/3 semitone; MIDI pitchwheel = round(bend * 4096 / 3) clipped to
[-8192, 8191], evenly spaced from start to end (only when the note overlaps no other note).
"""

from __future__ import annotations

import numpy as np
import onnxruntime as ort

SR = 22050
HOP = 256
N_SAMPLES = 43844
N_OLAP_FRAMES = 30
OLAP = N_OLAP_FRAMES * HOP  # 7680
WIN_HOP = N_SAMPLES - OLAP  # 36164
FPS = SR // HOP  # 86
ANNOT_N_FRAMES = FPS * 2  # 172
MIDI_OFFSET = 21
MAX_FREQ_IDX = 87
N_CONTOUR = 264
IN_NAME = "serving_default_input_2:0"
OUT_NAMES = ["StatefulPartitionedCall:1", "StatefulPartitionedCall:2", "StatefulPartitionedCall:0"]


def make_session(onnx_path, providers=None):
    return ort.InferenceSession(str(onnx_path), providers=providers or ["CPUExecutionProvider"])


def run_network(sess, audio: np.ndarray):
    """audio: mono f32 at 22050 Hz -> (note [T,88], onset [T,88], contour [T,264])."""
    L = len(audio)
    x = np.concatenate([np.zeros(OLAP // 2, np.float32), audio.astype(np.float32)])
    outs = {"note": [], "onset": [], "contour": []}
    for i in range(0, len(x), WIN_HOP):
        w = x[i : i + N_SAMPLES]
        if len(w) < N_SAMPLES:
            w = np.pad(w, (0, N_SAMPLES - len(w)))
        note, onset, contour = sess.run(OUT_NAMES, {IN_NAME: w[None, :, None]})
        outs["note"].append(note)
        outs["onset"].append(onset)
        outs["contour"].append(contour)
    n_keep = int(np.floor(L * (FPS / SR)))
    h = N_OLAP_FRAMES // 2
    res = {}
    for k, v in outs.items():
        v = np.concatenate(v)[:, h:-h, :]
        res[k] = v.reshape(-1, v.shape[2])[:n_keep]
    return res["note"], res["onset"], res["contour"]


def _argrelmax0(a):
    """scipy.signal.argrelmax(a, axis=0) (order 1, mode 'clip'): strict local maxima in time."""
    m = np.zeros(a.shape, bool)
    m[1:-1] = (a[1:-1] > a[:-2]) & (a[1:-1] > a[2:])
    return np.nonzero(m)


def inferred_onsets(onsets, frames, n_diff=2):
    diffs = []
    for n in range(1, n_diff + 1):
        fa = np.concatenate([np.zeros((n, frames.shape[1])), frames])
        diffs.append(fa[n:, :] - fa[:-n, :])
    fd = np.min(diffs, axis=0)
    fd[fd < 0] = 0
    fd[:n_diff, :] = 0
    fd = np.max(onsets) * fd / np.max(fd)
    return np.max([onsets, fd], axis=0)


def output_to_notes(frames, onsets, onset_thresh=0.5, frame_thresh=0.3, min_note_len=11, energy_tol=11):
    """Polyphonic decode (identical to basic_pitch output_to_notes_polyphonic with infer_onsets
    and melodia_trick). Returns [(start_frame, end_frame, midi_pitch, amplitude)]."""
    n_frames = frames.shape[0]
    onsets = inferred_onsets(onsets, frames)
    peak = np.zeros(onsets.shape)
    pk = _argrelmax0(onsets)
    peak[pk] = onsets[pk]
    oi = np.where(peak >= onset_thresh)
    on_t, on_f = oi[0][::-1], oi[1][::-1]  # backwards in time
    rem = np.zeros(frames.shape)
    rem[:, :] = frames[:, :]
    notes = []
    for s, f in zip(on_t, on_f):
        if s >= n_frames - 1:
            continue
        i, k = s + 1, 0
        while i < n_frames - 1 and k < energy_tol:
            k = k + 1 if rem[i, f] < frame_thresh else 0
            i += 1
        i -= k
        if i - s <= min_note_len:
            continue
        rem[s:i, f] = 0
        if f < MAX_FREQ_IDX:
            rem[s:i, f + 1] = 0
        if f > 0:
            rem[s:i, f - 1] = 0
        notes.append((int(s), int(i), int(f + MIDI_OFFSET), float(np.mean(frames[s:i, f]))))
    # melodia trick: grow notes from the remaining energy maxima
    while np.max(rem) > frame_thresh:
        im, f = np.unravel_index(np.argmax(rem), rem.shape)
        rem[im, f] = 0
        i, k = im + 1, 0
        while i < n_frames - 1 and k < energy_tol:
            k = k + 1 if rem[i, f] < frame_thresh else 0
            rem[i, f] = 0
            if f < MAX_FREQ_IDX:
                rem[i, f + 1] = 0
            if f > 0:
                rem[i, f - 1] = 0
            i += 1
        i_end = i - 1 - k
        i, k = im - 1, 0
        while i > 0 and k < energy_tol:
            k = k + 1 if rem[i, f] < frame_thresh else 0
            rem[i, f] = 0
            if f < MAX_FREQ_IDX:
                rem[i, f + 1] = 0
            if f > 0:
                rem[i, f - 1] = 0
            i -= 1
        i_start = i + 1 + k
        if i_end - i_start <= min_note_len:
            continue
        notes.append((int(i_start), int(i_end), int(f + MIDI_OFFSET), float(np.mean(frames[i_start:i_end, f]))))
    return notes


def pitch_bends(contours, notes, tol=25):
    wl = 2 * tol + 1
    gauss = np.exp(-0.5 * (np.arange(wl) - tol) ** 2 / 5.0**2)  # scipy gaussian(51, std=5)
    out = []
    for s, e, p, _ in notes:
        hz = 440.0 * 2 ** ((p - 69) / 12)
        fi = int(np.round(12.0 * 3 * np.log2(hz / 27.5)))
        a, b = max(fi - tol, 0), min(N_CONTOUR, fi + tol + 1)
        sub = contours[s:e, a:b] * gauss[max(0, tol - fi) : wl - max(0, fi - (N_CONTOUR - tol - 1))]
        shift = tol - max(0, tol - fi)
        out.append([int(v) for v in (np.argmax(sub, axis=1) - shift)])
    return out


def frame_times(n):
    t = np.arange(n) * HOP / SR
    off = (HOP / SR) * (ANNOT_N_FRAMES - (N_SAMPLES / HOP)) + 0.0018
    return t - off * np.floor(np.arange(n) / ANNOT_N_FRAMES)


def transcribe(sess, audio: np.ndarray):
    """mono f32 at 22050 Hz -> (notes, raw) ; notes sorted by (start, pitch)."""
    note, onset, contour = run_network(sess, audio)
    ev = output_to_notes(note.copy(), onset.copy())
    bends = pitch_bends(contour, ev)
    ts = frame_times(contour.shape[0])
    notes = [{"start": float(ts[s]), "end": float(ts[e]), "pitch": p, "velocity": int(np.round(127 * a)),
              "bends": b} for (s, e, p, a), b in zip(ev, bends)]
    notes.sort(key=lambda n: (n["start"], n["pitch"]))
    return notes, {"note": note, "onset": onset, "contour": contour}


def midi_bends(notes):
    """Pitchwheel events [(t, value)] for notes that overlap no other note (Basic Pitch default)."""
    ok = [True] * len(notes)
    srt = sorted(range(len(notes)), key=lambda i: (notes[i]["start"], notes[i]["end"], notes[i]["pitch"]))
    for a in range(len(srt)):
        for b in range(a + 1, len(srt)):
            if notes[srt[b]]["start"] >= notes[srt[a]]["end"]:
                break
            ok[srt[a]] = ok[srt[b]] = False
    ev = []
    for n, good in zip(notes, ok):
        if good and n["bends"]:
            ts = np.linspace(n["start"], n["end"], len(n["bends"]))
            v = np.clip(np.round(np.array(n["bends"]) * 4096 / 3), -8192, 8191).astype(int)
            ev += [(float(t), int(x)) for t, x in zip(ts, v)]
    return ev
