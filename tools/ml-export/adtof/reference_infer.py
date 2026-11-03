"""ADTOF Frame_RNN drum transcription (Zehren et al.; PyTorch port by xavriley) reference
pipeline: numpy + onnxruntime only. THIS IS THE SPEC THE RUST LANE PORTS.

Model: adtof_drums.onnx, input "spec" f32 [1, T, 84, 1] (T dynamic), output "act" f32
[1, T, 5] (sigmoid activations at 100 frames/s) for classes, as GM drum notes:
  35 bass drum, 38 snare, 47 toms, 42 hi-hat (open + closed), 49 cymbals (crash + ride).
The bidirectional GRUs see the whole file, so the file is processed in one call (no chunking).

1. Host: mono (mean), resample to 44100 Hz.
2. STFT: n_fft 2048, hop 441, window np.hanning(2048) (SYMMETRIC Hann, 0.5-0.5cos(2 pi n/2047)),
   center=True with ZERO padding of 1024 on both sides, frames T = 1 + L // 441;
   magnitude |rfft| of bins 0..1023 (Nyquist and above dropped).
3. Filterbank (madmom-style log triangle bank, 12 bands/octave from 20 Hz to 20 kHz on the
   1024 bin frequencies k*44100/2048, unique bins, rows normalised to sum 1) -> 84 bands:
   see filterbank(); spec = log10(1 + fb @ mag), as [1, T, 84, 1].
4. Peak picking per class (madmom NotePeakPickingProcessor, fps 100), thresholds
   [0.22, 0.24, 0.32, 0.22, 0.30]: proc = max(0, a - mean(a over [i-10, i+1], edge-padded));
   peak where proc == max(proc over [i-2, i+1], edge-padded) and proc >= threshold; peaks
   within 2 frames of the previous peak in a group are merged keeping the largest proc
   (first one on ties); onset time = frame / 100 s.
Output: notes [{start, end = start + 0.1, pitch (GM), velocity 100, strength = act at the peak}]
sorted by (start, pitch). The original exports fixed velocity 100; `strength` is extra.
"""

from __future__ import annotations

import numpy as np
import onnxruntime as ort

SR = 44100
N_FFT = 2048
HOP = 441
FPS = 100
LABELS = [35, 38, 47, 42, 49]
THRESHOLDS = [0.22, 0.24, 0.32, 0.22, 0.30]


def make_session(onnx_path, providers=None):
    return ort.InferenceSession(str(onnx_path), providers=providers or ["CPUExecutionProvider"])


def filterbank():
    freqs = []
    f, factor = 20.0, 2.0 ** (1.0 / 12)
    while f <= 20000.0 * (1.0 + 1e-12):
        freqs.append(f)
        f *= factor
    fft_freqs = np.fft.fftfreq(N_FFT, 1 / SR)[: N_FFT // 2]
    bins = [int(np.argmin(np.abs(fft_freqs - f))) for f in freqs]
    ub, last = [], -1
    for b in bins:
        if b > last:
            ub.append(b)
            last = b
    n_fft_bins = len(fft_freqs)
    fb = np.zeros((len(ub) - 2, n_fft_bins), np.float32)
    for i in range(len(ub) - 2):
        lo, c, hi = ub[i], ub[i + 1], ub[i + 2]
        if hi - lo < 2:
            if 0 <= lo < n_fft_bins:
                fb[i, lo] = 1.0
            continue
        for b in range(lo, c):
            fb[i, b] = (b - lo) / float(c - lo)
        if 0 <= c < n_fft_bins:
            fb[i, c] = 1.0
        for b in range(c + 1, min(hi, n_fft_bins)):
            fb[i, b] = (hi - b) / float(hi - c)
    s = fb.sum(1, keepdims=True)
    s[s == 0] = 1
    return (fb / s).astype(np.float32)


def features(audio):
    x = np.pad(audio.astype(np.float32), (N_FFT // 2, N_FFT // 2))
    T = 1 + len(audio) // HOP
    w = np.hanning(N_FFT).astype(np.float32)
    idx = np.arange(T)[:, None] * HOP + np.arange(N_FFT)[None, :]
    mag = np.abs(np.fft.rfft(x[idx] * w, axis=-1))[:, : N_FFT // 2].astype(np.float32)  # [T, 1024]
    spec = np.log10(1.0 + (filterbank() @ mag.T)).astype(np.float32)  # [168, T]
    return spec.T[None, :, :, None]


def peak_pick(act, thr):
    a = act.astype(np.float32)
    k = np.ones(12, np.float32) / 12
    avg = np.convolve(np.pad(a, (10, 1), mode="edge"), k, mode="valid").astype(np.float32)
    proc = np.maximum(0.0, a - avg)
    pp = np.pad(proc, (2, 1), mode="edge")
    wmax = np.stack([pp[j : j + len(proc)] for j in range(4)]).max(0)
    idx = np.where((proc >= wmax) & (proc >= thr))[0]
    if idx.size == 0:
        return []
    kept, group = [], [int(idx[0])]
    for i in idx[1:]:
        if i - group[-1] <= 2:
            group.append(int(i))
        else:
            kept.append(max(group, key=lambda j: proc[j]))
            group = [int(i)]
    kept.append(max(group, key=lambda j: proc[j]))
    return kept


def transcribe(sess, audio):
    """mono f32 at 44.1 kHz -> (notes, raw activations [T, 5])."""
    act = sess.run(["act"], {"spec": features(audio)})[0][0]
    notes = []
    for c, (lab, thr) in enumerate(zip(LABELS, THRESHOLDS)):
        for f in peak_pick(act[:, c], thr):
            t = f / float(FPS)
            notes.append({"start": t, "end": t + 0.1, "pitch": lab, "velocity": 100, "strength": float(act[f, c])})
    notes.sort(key=lambda n: (n["start"], n["pitch"]))
    return notes, act
