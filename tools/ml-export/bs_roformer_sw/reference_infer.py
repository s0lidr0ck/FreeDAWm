"""BS-RoFormer SW (6 stems) reference pipeline: numpy + onnxruntime only. THE SPEC THE RUST LANE PORTS
(`crates/ml/src/bs_roformer.rs`, through `crates/ml/src/roformer.rs`).

Model: bs_roformer_sw.onnx, input `spec` f32 [2, 1025, 801, 2] (channel, bin, frame, re/im), output
`stems_spec` f32 [12, 1025, 801, 2] (stem * 2 + channel, ...). Stereo, 44100 Hz. Stems in order:
bass, drums, other, vocals, guitar, piano (all from the graph; nothing is a remainder).

Chunking (ZFTurbo MSS `demix`, generic mode, num_overlap 2, batch 1, fp32), as karaoke's with:
  CHUNK = 409600 (9.29 s; the model config says 588800, see export.py), STEP = CHUNK // 2 = 204800,
  FADE = CHUNK // 10 = 40960, BORDER = CHUNK - STEP = 204800; reflect-pad BORDER both sides when L > 2 * BORDER; chunks every
  STEP, the last padded (reflect if n > CHUNK // 2 else zeros); linear fades (none at the first
  chunk's start / last chunk's end); out /= summed window; NaN -> 0; remove the border.

model_chunk(part [2, CHUNK]):
  1. STFT per channel: N = 2048, H = 512, periodic Hann, center=True, reflect pad N/2:
     T = CHUNK // H + 1 = 801 frames, X = rfft(w * frame) -> spec [2, 1025, 801, 2].
  2. ONNX -> Y [12, 1025, 801, 2] (DC bin already zeroed).
  3. iSTFT per row (torch.istft center=True semantics, length CHUNK) -> [6, 2, CHUNK].
"""

from __future__ import annotations

import numpy as np
import onnxruntime as ort

SR = 44100
CHUNK = 409600  # 9.29 s, 801 frames (the config's 588800 hangs DirectML: see the docstring)
NUM_OVERLAP = 2
N_FFT = 2048
HOP = 512
STEMS = ["bass", "drums", "other", "vocals", "guitar", "piano"]


def make_session(onnx_path, providers=None, threads: int = 0, dml_config=None):
    so = ort.SessionOptions()
    if threads:
        so.intra_op_num_threads = threads
    for k, v in (dml_config or {}).items():
        so.add_session_config_entry(k, v)
    return ort.InferenceSession(str(onnx_path), so, providers=providers or ["CPUExecutionProvider"])


def hann(n: int) -> np.ndarray:
    return 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(n) / n)


def stft(x: np.ndarray) -> np.ndarray:
    """x [C, L] -> [C, 1025, T, 2] f32."""
    C, L = x.shape
    xp = np.pad(x.astype(np.float64), ((0, 0), (N_FFT // 2, N_FFT // 2)), mode="reflect")
    T = L // HOP + 1
    idx = np.arange(T)[:, None] * HOP + np.arange(N_FFT)[None, :]
    X = np.fft.rfft(xp[:, idx] * hann(N_FFT), axis=-1)
    return np.stack([X.real, X.imag], -1).transpose(0, 2, 1, 3).astype(np.float32)


def istft(spec: np.ndarray, L: int) -> np.ndarray:
    """[C, F, T, 2] -> [C, L] f32 (torch.istft center=True semantics)."""
    Y = spec[..., 0].astype(np.float64) + 1j * spec[..., 1]
    C, _, T = Y.shape
    w = hann(N_FFT)
    fr = np.fft.irfft(Y.transpose(0, 2, 1), n=N_FFT, axis=-1) * w
    total = (T - 1) * HOP + N_FFT
    y = np.zeros((C, total))
    env = np.zeros(total)
    for t in range(T):
        y[:, t * HOP : t * HOP + N_FFT] += fr[:, t]
        env[t * HOP : t * HOP + N_FFT] += w * w
    a = N_FFT // 2
    return (y[:, a : a + L] / np.where(env > 1e-11, env, 1.0)[a : a + L]).astype(np.float32)


def model_chunk(sess, part: np.ndarray) -> np.ndarray:
    """part [2, CHUNK] -> [6, 2, CHUNK]."""
    y = sess.run(["stems_spec"], {"spec": stft(part)})[0]
    return istft(y.astype(np.float32), part.shape[1]).reshape(len(STEMS), 2, part.shape[1])


def fade_window(size: int, fade: int) -> np.ndarray:
    w = np.ones(size, np.float32)
    w[-fade:] = np.linspace(1, 0, fade, dtype=np.float32)
    w[:fade] = np.linspace(0, 1, fade, dtype=np.float32)
    return w


def separate(sess, x: np.ndarray, progress=None) -> np.ndarray:
    """x: f32 [2, L] at 44.1 kHz -> f32 [6 (STEMS), 2, L]."""
    x = np.asarray(x, np.float32)
    L = x.shape[1]
    step = CHUNK // NUM_OVERLAP
    fade = CHUNK // 10
    border = CHUNK - step
    padded = L > 2 * border and border > 0
    xp = np.pad(x, ((0, 0), (border, border)), mode="reflect") if padded else x
    Lp = xp.shape[1]
    base = fade_window(CHUNK, fade)
    out = np.zeros((len(STEMS), 2, Lp), np.float32)
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
        out[:, :, i : i + n] += y[:, :, :n] * w[:n]
        cnt[i : i + n] += w[:n]
        if progress:
            progress(j + 1, len(starts))
    with np.errstate(invalid="ignore", divide="ignore"):
        out = out / cnt
    if padded:
        out = out[:, :, border:-border]
    return np.nan_to_num(out, nan=0.0)


if __name__ == "__main__":
    import sys

    import soundfile as sf

    onnx_path, wav_in, out_prefix = sys.argv[1:4]
    audio, sr = sf.read(wav_in, dtype="float32", always_2d=True)
    assert sr == SR, "resample to 44100 Hz first"
    audio = audio.T if audio.shape[1] == 2 else np.repeat(audio.T, 2, 0)
    for name, s in zip(STEMS, separate(make_session(onnx_path), audio)):
        sf.write(f"{out_prefix}{name}.wav", s.T, SR, subtype="FLOAT")
