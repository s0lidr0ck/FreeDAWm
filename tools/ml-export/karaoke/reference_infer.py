"""Mel-Band RoFormer karaoke reference pipeline: numpy + onnxruntime only. THE SPEC THE RUST LANE PORTS.

Model: karaoke_melroformer.onnx, input `spec` f32 [2, 1025, 801, 2], output `vocals_spec` f32
[2, 1025, 801, 2] (channel, bin, frame, re/im). Stereo, 44100 Hz. Output stems:
lead vocal (the model's target) and "backing" = input - lead (MSS --extract_instrumental). Run it
on a vocal stem to split lead from backing vocals (or on a full mix for lead vs everything else).

Chunking (identical to ZFTurbo MSS `demix`, generic mode, num_overlap 4, batch 1, fp32):
  CHUNK = 352800 (8 s), STEP = CHUNK // 4 = 88200, FADE = CHUNK // 10 = 35280,
  BORDER = CHUNK - STEP = 264600.
  If L > 2 * BORDER: reflect-pad (numpy "reflect") BORDER on both sides.
  For i = 0, STEP, ... while i < Lp: part = x[:, i : i + CHUNK], n = len; pad right to CHUNK
  (reflect if n > CHUNK // 2 else zeros); y = model_chunk(part); window = linear fade-in over
  FADE (np.linspace(0, 1, FADE)), ones, linear fade-out; first chunk (i == 0): no fade-in;
  elif last chunk (i + STEP >= Lp): no fade-out. out[:, i:i+n] += y[:, :n] * w[:n]; cnt += w[:n].
  out /= cnt (NaN -> 0); remove BORDER if padded.

model_chunk(part [2, CHUNK]):
  1. STFT per channel: N = 2048, H = 441, periodic Hann (0.5 - 0.5 cos(2 pi n / N)), center=True:
     reflect-pad N/2 = 1024 both sides, T = CHUNK // H + 1 = 801 frames at t*H,
     X = rfft(w * frame) (1025 bins, not normalised) -> spec [2, 1025, 801, (re, im)].
  2. ONNX -> Y (DC bin already zeroed by the graph).
  3. iSTFT per channel: irfft(Y[:, t], n=N) * w, overlap-add at t*H, divide by the overlap-added
     w^2, drop the first N/2 samples, keep H * (T - 1) = CHUNK samples.
"""

from __future__ import annotations

import numpy as np
import onnxruntime as ort

SR = 44100
CHUNK = 352800
NUM_OVERLAP = 4
N_FFT = 2048
HOP = 441
STEMS = ["lead_vocal", "backing"]


def make_session(onnx_path, providers=None, threads: int = 0):
    so = ort.SessionOptions()
    if threads:
        so.intra_op_num_threads = threads
    return ort.InferenceSession(str(onnx_path), so, providers=providers or ["CPUExecutionProvider"])


def hann(n: int) -> np.ndarray:
    return 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(n) / n)


def stft(x: np.ndarray) -> np.ndarray:
    """x [C, L] -> [C, 1025, T, 2] f32."""
    C, L = x.shape
    xp = np.pad(x.astype(np.float64), ((0, 0), (N_FFT // 2, N_FFT // 2)), mode="reflect")
    T = L // HOP + 1
    idx = np.arange(T)[:, None] * HOP + np.arange(N_FFT)[None, :]
    X = np.fft.rfft(xp[:, idx] * hann(N_FFT), axis=-1)  # [C, T, F]
    return np.stack([X.real, X.imag], -1).transpose(0, 2, 1, 3).astype(np.float32)


def istft(spec: np.ndarray, L: int) -> np.ndarray:
    """[C, F, T, 2] -> [C, L] f32 (torch.istft center=True semantics)."""
    Y = spec[..., 0].astype(np.float64) + 1j * spec[..., 1]  # [C, F, T]
    C, _, T = Y.shape
    w = hann(N_FFT)
    fr = np.fft.irfft(Y.transpose(0, 2, 1), n=N_FFT, axis=-1) * w  # [C, T, N]
    total = (T - 1) * HOP + N_FFT
    y = np.zeros((C, total))
    env = np.zeros(total)
    for t in range(T):
        y[:, t * HOP : t * HOP + N_FFT] += fr[:, t]
        env[t * HOP : t * HOP + N_FFT] += w * w
    a = N_FFT // 2
    return (y[:, a : a + L] / np.where(env > 1e-11, env, 1.0)[a : a + L]).astype(np.float32)


def model_chunk(sess, part: np.ndarray) -> np.ndarray:
    name = sess.get_outputs()[0].name  # "vocals_spec"
    y = sess.run([name], {"spec": stft(part)})[0]
    return istft(y.astype(np.float32), part.shape[1])


def fade_window(size: int, fade: int) -> np.ndarray:
    w = np.ones(size, np.float32)
    w[-fade:] = np.linspace(1, 0, fade, dtype=np.float32)
    w[:fade] = np.linspace(0, 1, fade, dtype=np.float32)
    return w


def separate(sess, x: np.ndarray, progress=None, num_overlap: int = NUM_OVERLAP) -> np.ndarray:
    """x: f32 [2, L] at 44.1 kHz -> f32 [2 (lead_vocal, backing), 2, L]. `num_overlap` is the card's
    (4 for karaoke, 2 for vocals_melroformer)."""
    x = np.asarray(x, np.float32)
    L = x.shape[1]
    step = CHUNK // num_overlap
    fade = CHUNK // 10
    border = CHUNK - step
    padded = L > 2 * border and border > 0
    xp = np.pad(x, ((0, 0), (border, border)), mode="reflect") if padded else x
    Lp = xp.shape[1]
    base = fade_window(CHUNK, fade)
    out = np.zeros((2, Lp), np.float32)
    cnt = np.zeros(Lp, np.float32)
    starts = list(range(0, Lp, step))
    for j, i in enumerate(starts):
        part = xp[:, i : i + CHUNK]
        n = part.shape[1]
        if n < CHUNK:
            part = np.pad(part, ((0, 0), (0, CHUNK - n)), mode="reflect" if n > CHUNK // 2 else "constant")
        y = model_chunk(sess, part)
        w = base.copy()
        if i == 0:
            w[:fade] = 1
        elif i + step >= Lp:
            w[-fade:] = 1
        out[:, i : i + n] += y[:, :n] * w[:n]
        cnt[i : i + n] += w[:n]
        if progress:
            progress(j + 1, len(starts))
    with np.errstate(invalid="ignore", divide="ignore"):
        out = out / cnt
    if padded:
        out = out[:, border:-border]
    lead = np.nan_to_num(out, nan=0.0)
    return np.stack([lead, x - lead])


if __name__ == "__main__":
    import sys

    import soundfile as sf

    onnx_path, wav_in, out_prefix = sys.argv[1:4]
    audio, sr = sf.read(wav_in, dtype="float32", always_2d=True)
    assert sr == SR, "resample to 44100 Hz first"
    audio = audio.T if audio.shape[1] == 2 else np.repeat(audio.T, 2, 0)
    for name, s in zip(STEMS, separate(make_session(onnx_path), audio)):
        sf.write(f"{out_prefix}{name}.wav", s.T, SR, subtype="FLOAT")
