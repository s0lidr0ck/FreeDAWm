"""ByteDance high-resolution piano transcription (Kong et al. 2020, notes + sustain pedal)
reference pipeline: numpy + onnxruntime only. THIS IS THE SPEC THE RUST LANE PORTS.

Model: piano_kong.onnx (log-mel front end inside the graph), input "audio" f32 [1, 160000]
(10 s, mono, 16 kHz); outputs, each f32 [1, 1001, K] at 100 frames/s:
  reg_onset [88], reg_offset [88], frame [88], velocity [88], reg_pedal_onset [1],
  reg_pedal_offset [1], pedal_frame [1].   Key k = MIDI note 21 + k.

1. Host: mono (mean), resample to 16000 Hz. L samples. Pad with zeros to a multiple of
   160000. Segments start at 0, 80000, 160000, ... while start + 160000 <= padded length.
2. Deframe: one segment -> its 1001 frames. Several -> drop each segment's last frame
   (1000 left), keep [0:750) of the first, [250:750) of the middle ones, [250:1000) of the last;
   concatenate. (The original then slices [0:L] with L in samples, i.e. keeps everything.)
3. Note decoding per key (thresholds onset 0.3, offset 0.3, frame 0.1):
   a. Binarise onset/offset regression with local maxima: frame n (neighbour <= n <
      T - neighbour; neighbour 2 for onsets, 4 for offsets) is a hit if x[n] > thr and
      x is non-decreasing over the `neighbour` frames before n and non-increasing over the
      `neighbour` frames after n. Sub-frame shift: if x[n-1] > x[n+1]:
      (x[n+1]-x[n-1]) / (x[n]-x[n+1]) / 2 else (x[n+1]-x[n-1]) / (x[n]-x[n-1]) / 2.
   b. Walk frames (note_detection): see detect_notes(); NB "if bgn" is Python truthiness,
      so an onset at frame 0 never opens a note (kept for parity).
   c. onset_time = (bgn + onset_shift[bgn]) / 100, offset_time = (fin + offset_shift[fin]) / 100
      (both rounded to float32), velocity = int(float32(velocity[bgn]) * 128) (truncated).
4. Pedal: offsets binarised as in 3a (threshold 0.2, neighbour 4); detect_pedals() with
   frame threshold 0.5; onset = bgn / 100, offset = (fin + shift[fin]) / 100 (float32).
Output: notes [{start, end, pitch, velocity}] sorted by (start, pitch); pedals [{start, end}]
(sustain, CC64 127 / 0).
"""

from __future__ import annotations

import numpy as np
import onnxruntime as ort

SR = 16000
SEG = 160000
FPS = 100
BEGIN_NOTE = 21
OUTS = ["reg_onset", "reg_offset", "frame", "velocity", "reg_pedal_onset", "reg_pedal_offset", "pedal_frame"]


def make_session(onnx_path, providers=None):
    return ort.InferenceSession(str(onnx_path), providers=providers or ["CPUExecutionProvider"])


def run_network(sess, audio):
    L = len(audio)
    pad = int(np.ceil(L / SEG)) * SEG - L
    x = np.concatenate([audio.astype(np.float32), np.zeros(pad, np.float32)])
    segs = []
    p = 0
    while p + SEG <= len(x):
        segs.append(x[p : p + SEG])
        p += SEG // 2
    outs = {k: [] for k in OUTS}
    for s in segs:
        for k, v in zip(OUTS, sess.run(OUTS, {"audio": s[None]})):
            outs[k].append(v[0])
    res = {}
    for k, v in outs.items():
        v = np.stack(v)
        if len(v) == 1:
            res[k] = v[0]
            continue
        v = v[:, :-1]
        n = v.shape[1]
        parts = [v[0, : int(n * 0.75)]] + [v[i, int(n * 0.25) : int(n * 0.75)] for i in range(1, len(v) - 1)]
        parts.append(v[-1, int(n * 0.25) :])
        res[k] = np.concatenate(parts)
    return res


def binarize(reg, thr, nb):
    T, K = reg.shape
    out = np.zeros_like(reg)
    shift = np.zeros_like(reg)
    for k in range(K):
        x = reg[:, k]
        for n in range(nb, T - nb):
            if not x[n] > thr:
                continue
            mono = True
            for i in range(nb):
                if x[n - i] < x[n - i - 1] or x[n + i] < x[n + i + 1]:
                    mono = False
            if not mono:
                continue
            out[n, k] = 1
            if x[n - 1] > x[n + 1]:
                shift[n, k] = (x[n + 1] - x[n - 1]) / (x[n] - x[n + 1]) / 2
            else:
                shift[n, k] = (x[n + 1] - x[n - 1]) / (x[n] - x[n - 1]) / 2
    return out, shift


def detect_notes(frame, onset, onset_shift, offset, offset_shift, vel, frame_thr=0.1):
    """One key. Returns [bgn, fin, onset_shift, offset_shift, velocity]."""
    out = []
    bgn = fd = oo = None
    T = onset.shape[0]
    for i in range(T):
        if onset[i] == 1:
            if bgn:
                out.append([bgn, max(i - 1, 0), onset_shift[bgn], 0, vel[bgn]])
                fd, oo = None, None
            bgn = i
        if bgn and i > bgn:
            if frame[i] <= frame_thr and not fd:
                fd = i
            if offset[i] == 1 and not oo:
                oo = i
            if fd:
                fin = oo if (oo and oo - bgn > fd - oo) else fd
                out.append([bgn, fin, onset_shift[bgn], offset_shift[fin], vel[bgn]])
                bgn, fd, oo = None, None, None
            if bgn and (i - bgn >= 600 or i == T - 1):
                out.append([bgn, i, onset_shift[bgn], offset_shift[i], vel[bgn]])
                bgn, fd, oo = None, None, None
    out.sort(key=lambda t: t[0])
    return out


def detect_pedals(frame, offset, offset_shift, frame_thr=0.5):
    out = []
    bgn = fd = oo = None
    for i in range(1, frame.shape[0]):
        if frame[i] >= frame_thr and frame[i] > frame[i - 1] and not bgn:
            bgn = i
        if bgn and i > bgn:
            if frame[i] <= frame_thr and not fd:
                fd = i
            if offset[i] == 1 and not oo:
                oo = i
            if oo:
                out.append([bgn, oo, 0.0, offset_shift[oo]])
                bgn, fd, oo = None, None, None
            if fd and i - fd >= 10:
                out.append([bgn, fd, 0.0, offset_shift[fd]])
                bgn, fd, oo = None, None, None
    out.sort(key=lambda t: t[0])
    return out


def decode(o):
    on, on_sh = binarize(o["reg_onset"], 0.3, 2)
    off, off_sh = binarize(o["reg_offset"], 0.3, 4)
    rows, keys = [], []
    for k in range(o["frame"].shape[1]):
        t = detect_notes(o["frame"][:, k], on[:, k], on_sh[:, k], off[:, k], off_sh[:, k], o["velocity"][:, k])
        rows += t
        keys += [k + BEGIN_NOTE] * len(t)
    notes = []
    if rows:
        a = np.array(rows, dtype=np.float64)
        arr = np.stack([(a[:, 0] + a[:, 2]) / FPS, (a[:, 1] + a[:, 3]) / FPS, np.array(keys, np.float64), a[:, 4]],
                       -1).astype(np.float32)
        notes = [{"start": float(r[0]), "end": float(r[1]), "pitch": int(r[2]), "velocity": int(r[3] * 128)}
                 for r in arr]
    p_off, p_sh = binarize(o["reg_pedal_offset"], 0.2, 4)
    prow = detect_pedals(o["pedal_frame"][:, 0], p_off[:, 0], p_sh[:, 0])
    pedals = []
    if prow:
        a = np.array(prow, dtype=np.float64)
        arr = np.stack([(a[:, 0] + a[:, 2]) / FPS, (a[:, 1] + a[:, 3]) / FPS], -1).astype(np.float32)
        pedals = [{"start": float(r[0]), "end": float(r[1])} for r in arr]
    notes.sort(key=lambda n: (n["start"], n["pitch"]))
    return notes, pedals


def transcribe(sess, audio):
    """mono f32 at 16 kHz -> (notes, pedals, raw outputs)."""
    o = run_network(sess, audio)
    notes, pedals = decode(o)
    return notes, pedals, o
