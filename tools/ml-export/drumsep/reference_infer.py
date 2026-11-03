"""MDX23C DrumSep reference pipeline: numpy + onnxruntime only. THIS IS THE SPEC THE RUST LANE PORTS.

Model: drumsep_mdx23c.onnx, input `mix` f32 [1, 2, 130560], output `stems` f32
[1, 6, 2, 130560] (kick, snare, toms, hh, ride, crash). STFT/iSTFT are inside the graph.

Pipeline (identical to ZFTurbo MSS `demix`, generic mode, num_overlap 4, batch 1, fp32):
  0. Input stereo at 44100 Hz (host resamples; mono -> duplicate). Feed a drum stem.
  1. CHUNK = 130560, STEP = CHUNK // 4 = 32640, FADE = CHUNK // 10 = 13056,
     BORDER = CHUNK - STEP = 97920.
  2. If L > 2 * BORDER: x = reflect-pad(x, BORDER, BORDER) (numpy mode "reflect", i.e. the edge
     sample is not repeated). Lp = padded length.
  3. For i = 0, STEP, 2*STEP, ... while i < Lp:
       part = x[:, i : i + CHUNK]; n = its length;
       if n < CHUNK: pad the right by CHUNK - n, reflect if n > CHUNK // 2 else zeros;
       y = model(part) [6, 2, CHUNK];
       w = window: linear fade-in 0..1 over FADE samples (np.linspace(0, 1, FADE)), ones,
           linear fade-out 1..0 over the last FADE samples;
           if this is the first chunk (i == 0): w[:FADE] = 1
           elif this is the last chunk (i + STEP >= Lp): w[-FADE:] = 1
       out[..., i : i + n] += y[..., :n] * w[:n]; cnt[i : i + n] += w[:n]
  4. out /= cnt (0/0 -> NaN -> 0); remove BORDER from both ends if step 2 padded.
"""

from __future__ import annotations

import numpy as np
import onnxruntime as ort

SR = 44100
CHUNK = 130560
NUM_OVERLAP = 4
STEMS = ["kick", "snare", "toms", "hh", "ride", "crash"]


def make_session(onnx_path, providers=None, threads: int = 0):
    so = ort.SessionOptions()
    if threads:
        so.intra_op_num_threads = threads
    return ort.InferenceSession(str(onnx_path), so, providers=providers or ["CPUExecutionProvider"])


def fade_window(size: int, fade: int) -> np.ndarray:
    w = np.ones(size, np.float32)
    w[-fade:] = np.linspace(1, 0, fade, dtype=np.float32)
    w[:fade] = np.linspace(0, 1, fade, dtype=np.float32)
    return w


def separate(sess, x: np.ndarray, progress=None) -> np.ndarray:
    """x: f32 [2, L] at 44.1 kHz -> f32 [6, 2, L]."""
    x = np.asarray(x, np.float32)
    L = x.shape[1]
    step = CHUNK // NUM_OVERLAP
    fade = CHUNK // 10
    border = CHUNK - step
    padded = L > 2 * border and border > 0
    if padded:
        x = np.pad(x, ((0, 0), (border, border)), mode="reflect")
    Lp = x.shape[1]
    base = fade_window(CHUNK, fade)
    out = np.zeros((len(STEMS), 2, Lp), np.float32)
    cnt = np.zeros(Lp, np.float32)
    starts = list(range(0, Lp, step))
    for j, i in enumerate(starts):
        part = x[:, i : i + CHUNK]
        n = part.shape[1]
        if n < CHUNK:
            mode = "reflect" if n > CHUNK // 2 else "constant"
            part = np.pad(part, ((0, 0), (0, CHUNK - n)), mode=mode)
        y = sess.run(["stems"], {"mix": part[None]})[0][0]
        w = base.copy()
        if i == 0:
            w[:fade] = 1
        elif i + step >= Lp:
            w[-fade:] = 1
        out[..., i : i + n] += y[..., :n] * w[:n]
        cnt[i : i + n] += w[:n]
        if progress:
            progress(j + 1, len(starts))
    with np.errstate(invalid="ignore", divide="ignore"):
        out = out / cnt
    if padded:
        out = out[..., border:-border]
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
