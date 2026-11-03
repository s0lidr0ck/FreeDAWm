"""Smule Renaissance Small reference pipeline: numpy + onnxruntime only. THE SPEC THE RUST LANE PORTS.

Model: srs_small.onnx, input `spec` f32 [1, 2049, T, 2], output `spec_out` f32 [1, 2049, T, 2]
(T is dynamic; last axis = re, im). Mono, 48000 Hz. Whole-file semantics of the original
main.py, made memory-bounded by exact frame-domain chunking.

  0. Host: downmix to mono (mean of channels) and resample to 48000 Hz.
  1. High-pass: RBJ biquad, fc = 60 Hz, Q = 0.707 (torchaudio.functional.highpass_biquad):
       w0 = 2*pi*60/48000, alpha = sin(w0)/(2*Q), c = cos(w0)
       b = [(1+c)/2, -(1+c), (1+c)/2], a = [1+alpha, -2c, 1-alpha]; divide b and a by a[0]
       (coefficients rounded to float32 as torchaudio does), Direct Form I, zero initial state,
       then CLAMP the result to [-1, 1] (torchaudio lfilter clamp=True).
  2. Peak-normalise: g = max|x|; if g > 0: x = x / g.
  3. STFT: N = 4096, H = 2048, periodic Hann w[n] = 0.5 - 0.5 cos(2 pi n / N); reflect-pad N/2 on
     both sides (edge sample not repeated); frames t = 0..T-1, T = L // H + 1, frame t starts at
     t*H of the padded signal; X[k, t] = rfft(w * frame)[k], k = 0..2048 (no normalisation).
     spec = [1, 2049, T, 2] = (Re, Im).
  4. Network, chunked over frames: CORE = 256, CTX = 129 (= the network's exact receptive
     field). For c0 = 0, 256, 512, ...: a = max(0, c0 - CTX), b = min(T, c0 + CORE + CTX);
     y = model(spec[:, :, a:b]); out[:, :, c0 : min(T, c0 + CORE)] = y[:, :, c0 - a : ...].
     (Any CORE gives the same result to float rounding: the context covers the receptive field.)
  5. iSTFT (torch.istft, center=True, length=L): y_t = irfft(Y[:, t], n=N) * w; overlap-add at
     t*H; divide by the overlap-added w^2; drop the first N/2 samples; keep L samples.
  6. Undo the normalisation: y = y * g.
Output: mono f32 [L] at 48 kHz, sample-aligned with the input.
(The original main.py returns only H*(T-1) samples; those match ours, we also fill the tail.)
"""

from __future__ import annotations

import numpy as np
import onnxruntime as ort

SR = 48000
N_FFT = 4096
HOP = 2048
CORE = 256
CTX = 129


def make_session(onnx_path, providers=None, threads: int = 0):
    so = ort.SessionOptions()
    if threads:
        so.intra_op_num_threads = threads
    return ort.InferenceSession(str(onnx_path), so, providers=providers or ["CPUExecutionProvider"])


def highpass60(x: np.ndarray) -> np.ndarray:
    w0 = np.float32(2 * np.pi * 60.0 / SR)
    q = np.float32(0.707)
    alpha = np.sin(w0) / np.float32(2.0) / q
    c = np.cos(w0)
    b = np.array([(1 + c) / 2, -1 - c, (1 + c) / 2], np.float32)
    a = np.array([1 + alpha, -2 * c, 1 - alpha], np.float32)
    b = (b / a[0]).astype(np.float64)
    a = (a / a[0]).astype(np.float64)
    y = np.zeros(len(x), np.float64)
    x1 = x2 = y1 = y2 = 0.0
    for n, xn in enumerate(x.astype(np.float64)):  # Direct Form I
        yn = b[0] * xn + b[1] * x1 + b[2] * x2 - a[1] * y1 - a[2] * y2
        x2, x1, y2, y1 = x1, xn, y1, yn
        y[n] = yn
    return np.clip(y, -1.0, 1.0).astype(np.float32)


def hann(n: int) -> np.ndarray:
    return (0.5 - 0.5 * np.cos(2 * np.pi * np.arange(n) / n)).astype(np.float32)


def stft(x: np.ndarray) -> np.ndarray:
    L = len(x)
    xp = np.pad(x, (N_FFT // 2, N_FFT // 2), mode="reflect")
    T = L // HOP + 1
    w = hann(N_FFT)
    fr = np.stack([xp[t * HOP : t * HOP + N_FFT] for t in range(T)])  # [T, N]
    X = np.fft.rfft(fr * w, axis=-1)  # [T, 2049]
    return np.stack([X.real, X.imag], -1).transpose(1, 0, 2)[None].astype(np.float32)  # [1, F, T, 2]


def istft(spec: np.ndarray, L: int) -> np.ndarray:
    Y = spec[0, ..., 0].astype(np.float64) + 1j * spec[0, ..., 1]  # [F, T]
    T = Y.shape[1]
    w = hann(N_FFT).astype(np.float64)
    fr = np.fft.irfft(Y.T, n=N_FFT, axis=-1) * w  # [T, N]
    total = (T - 1) * HOP + N_FFT
    y = np.zeros(total)
    env = np.zeros(total)
    for t in range(T):
        y[t * HOP : t * HOP + N_FFT] += fr[t]
        env[t * HOP : t * HOP + N_FFT] += w * w
    y = y[N_FFT // 2 : N_FFT // 2 + L] / np.where(env > 1e-11, env, 1.0)[N_FFT // 2 : N_FFT // 2 + L]
    return y.astype(np.float32)


def run_network(sess, spec: np.ndarray, progress=None) -> np.ndarray:
    T = spec.shape[2]
    out = np.zeros_like(spec)
    starts = list(range(0, T, CORE))
    for i, c0 in enumerate(starts):
        a, b = max(0, c0 - CTX), min(T, c0 + CORE + CTX)
        y = sess.run(["spec_out"], {"spec": np.ascontiguousarray(spec[:, :, a:b])})[0]
        e = min(T, c0 + CORE)
        out[:, :, c0:e] = y[:, :, c0 - a : e - a]
        if progress:
            progress(i + 1, len(starts))
    return out


def restore(sess, x: np.ndarray, progress=None) -> np.ndarray:
    """x: mono f32 [L] at 48 kHz -> restored mono f32 [L]."""
    x = highpass60(np.asarray(x, np.float32))
    g = float(np.max(np.abs(x)))
    if g > 0:
        x = x / np.float32(g)
    spec = stft(x)
    y = istft(run_network(sess, spec, progress), len(x))
    return y * np.float32(g) if g > 0 else y


if __name__ == "__main__":
    import sys

    import soundfile as sf

    onnx_path, wav_in, wav_out = sys.argv[1:4]
    audio, sr = sf.read(wav_in, dtype="float32", always_2d=True)
    assert sr == SR, "resample to 48000 Hz first"
    sf.write(wav_out, restore(make_session(onnx_path), audio.mean(1)), SR, subtype="FLOAT")
