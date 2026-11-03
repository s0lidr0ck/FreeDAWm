"""HTDemucs v4 reference pipeline: numpy + onnxruntime only. THIS IS THE SPEC THE RUST LANE PORTS.

Model: htdemucs.onnx, input `mix` f32 [1, 2, 343980], output `stems` f32 [1, 4, 2, 343980]
(source order: drums, bass, other, vocals). STFT/iSTFT are inside the graph.

Pipeline (identical to demucs.api.Separator with shifts=0, overlap=0.25, split=True):
  0. The input must be stereo at 44100 Hz (the host resamples; mono -> duplicate to 2 channels).
  1. Normalise: ref = mean over channels; mu = mean(ref); sd = std(ref, ddof=1) + 1e-8;
     x = (x - mu) / sd.
  2. Chunk: SEG = 343980, STRIDE = int(0.75 * SEG) = 257985, offsets 0, STRIDE, 2*STRIDE, ...
     while offset < L. Each chunk is the window [offset, offset + min(SEG, L - offset)) whose
     length n is grown to SEG *centred*: start = offset - (SEG - n) // 2, taking real samples of
     x where they exist and zeros outside [0, L). (For the last chunk this reads samples before
     `offset`.)
  3. Run the model on each padded chunk, then centre-trim the output back to n samples:
     d = SEG - n; keep [d // 2, d // 2 + n).
  4. Overlap-add with a triangular weight w[i] (length SEG): w = [1, 2, ..., SEG//2,
     SEG - SEG//2, ..., 2, 1] / max, truncated to n for a short chunk; out[offset:offset+n] +=
     w[:n] * y; wsum[offset:offset+n] += w[:n]. Finally out /= wsum.
  5. Denormalise: out = out * sd + mu.
All arithmetic is float32 except steps 1/5 statistics (float64 is fine; differences < 1e-7).
"""

from __future__ import annotations

import numpy as np
import onnxruntime as ort

SR = 44100
SEG = 343980
OVERLAP = 0.25
SOURCES = ["drums", "bass", "other", "vocals"]


def make_session(onnx_path, providers=None, threads: int = 0):
    so = ort.SessionOptions()
    if threads:
        so.intra_op_num_threads = threads
    return ort.InferenceSession(str(onnx_path), so, providers=providers or ["CPUExecutionProvider"])


def triangular_weight(seg: int) -> np.ndarray:
    w = np.concatenate([np.arange(1, seg // 2 + 1), np.arange(seg - seg // 2, 0, -1)])
    return (w / w.max()).astype(np.float32)


def separate(sess, x: np.ndarray, progress=None) -> np.ndarray:
    """x: f32 [2, L] at 44.1 kHz. Returns f32 [4, 2, L] (drums, bass, other, vocals)."""
    x = np.asarray(x, np.float32)
    C, L = x.shape
    assert C == 2
    ref = x.mean(0)
    mu = np.float32(ref.mean())
    sd = np.float32(ref.std(ddof=1) + 1e-8)
    x = (x - mu) / sd

    stride = int((1 - OVERLAP) * SEG)
    w = triangular_weight(SEG)
    out = np.zeros((4, 2, L), np.float32)
    wsum = np.zeros(L, np.float32)
    offsets = list(range(0, L, stride))
    for i, off in enumerate(offsets):
        n = min(SEG, L - off)
        start = off - (SEG - n) // 2
        chunk = np.zeros((2, SEG), np.float32)
        a, b = max(0, start), min(L, start + SEG)
        chunk[:, a - start : b - start] = x[:, a:b]
        y = sess.run(["stems"], {"mix": chunk[None]})[0][0]  # [4, 2, SEG]
        d = SEG - n
        y = y[..., d // 2 : d // 2 + n]
        out[..., off : off + n] += w[:n] * y
        wsum[off : off + n] += w[:n]
        if progress:
            progress(i + 1, len(offsets))
    out /= wsum
    return out * sd + mu


if __name__ == "__main__":
    import sys

    import soundfile as sf

    onnx_path, wav_in, out_prefix = sys.argv[1:4]
    audio, sr = sf.read(wav_in, dtype="float32", always_2d=True)
    assert sr == SR, "resample to 44100 Hz first"
    audio = audio.T if audio.shape[1] == 2 else np.repeat(audio.T, 2, 0)
    stems = separate(make_session(onnx_path), audio)
    for name, s in zip(SOURCES, stems):
        sf.write(f"{out_prefix}{name}.wav", s.T, SR, subtype="FLOAT")
