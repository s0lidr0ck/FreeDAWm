"""GAME (openvpi, Generative Adaptive MIDI Extractor, small) singing-voice -> notes reference
pipeline: numpy + onnxruntime only. THIS IS THE SPEC THE RUST LANE PORTS.

Five graphs (exported with GAME's own deployment/exporter.py, opset 17), mono 44100 Hz, 10 ms
frames (T = round(L / 441) + ...), C = 128:
  encoder.onnx   waveform f32 [1, L], duration f32 [1] -> x_seg [1,T,C], x_est [1,T,C], maskT bool [1,T]
  dur2bd.onnx    durations f32 [1, 1], maskT -> boundaries bool [1, T]          (known boundaries)
  segmenter.onnx x_seg, language i64 [1], known_boundaries, prev_boundaries, t f32 [1], maskT,
                 threshold f32 (), radius i64 (), noise f32 [1, T] -> boundaries bool [1, T]
                 (one D3PM step; `noise` replaces the original's torch.rand_like, see noise())
  bd2dur.onnx    boundaries, maskT -> durations f32 [1, N] (s), maskN bool [1, N]
  estimator.onnx x_est, boundaries, maskT, maskN, threshold f32 () -> presence bool [1, N],
                 scores f32 [1, N] (MIDI pitch, fractional, A4 = 69)

1. Host: mono (mean), resample to 44100 Hz.
2. Slice at silences (GAME's slicer2.Slicer: threshold -40 dB, min_length 1000 ms,
   min_interval 200 ms, hop 20 ms, max_sil_kept 100 ms); see slice_audio(). Each slice has an
   offset in seconds. Extra (not in GAME): a slice longer than MAX_SLICE_S (30 s) is cut again at
   its lowest-RMS 20 ms frame between 20 and 30 s from its start, repeatedly, to bound memory
   (attention over all frames of a slice). Slices <= 30 s are untouched.
3. Per slice (length n samples, dur = n / 44100):
   x_seg, x_est, maskT = encoder(w[None], [dur]); kb = dur2bd([[dur]], maskT) & maskT;
   b = kb; for k, t in enumerate([0, 1/8, ..., 7/8]):
       b = segmenter(x_seg, [LANG], kb, b, [t], maskT, 0.2, 2, noise(slice_index, k, T));
   durations, maskN = bd2dur(b, maskT); presence, scores = estimator(x_est, b, maskT, maskN, 0.2).
   onset_i = offset + min(sum(durations[:i]), dur); offset_i = offset + min(sum(durations[:i+1]), dur)
   (all in float32: cumsum, clamp, add); keep notes with presence and offset > onset.
4. All slices: sort by (onset, offset, pitch); then walk: onset = max(onset, last_end),
   offset = max(offset, onset), drop if offset <= onset, else last_end = offset.
Output: notes [{start, end, pitch = round(score), pitch_f = score, velocity 100}].
LANG: 0 = unknown, 1 = en (default here: the owner sings English), 2 = ja, 3 = yue, 4 = zh.
"""

from __future__ import annotations

from pathlib import Path

import numpy as np
import onnxruntime as ort

SR = 44100
LANG_EN = 1
SEG_THRESHOLD = 0.2
SEG_RADIUS = 2  # round(0.02 s / 0.01 s)
EST_THRESHOLD = 0.2
TS = [i / 8 for i in range(8)]  # D3PM t0 = 0, 8 steps
MAX_SLICE_S = 30.0
GRAPHS = ["encoder", "dur2bd", "segmenter", "bd2dur", "estimator"]


def make_sessions(onnx_dir, providers=None):
    p = providers or ["CPUExecutionProvider"]
    return {g: ort.InferenceSession(str(Path(onnx_dir) / f"{g}.onnx"), providers=p) for g in GRAPHS}


def noise(slice_index: int, step: int, T: int) -> np.ndarray:
    """Deterministic uniform [0, 1) draws for the D3PM boundary removal (GAME draws them with
    torch.rand_like; we fix them so Rust and Python agree). SplitMix64 on a counter:
    z = ((seed << 32) | i) + 0x9E3779B97F4A7C15, seed = slice_index * 16 + step;
    z = (z ^ z >> 30) * 0xBF58476D1CE4E5B9; z = (z ^ z >> 27) * 0x94D049BB133111EB; z ^= z >> 31;
    u = (z >> 40) / 2^24 (all uint64 with wrap-around). Returns f32 [1, T]."""
    seed = np.uint64(slice_index * 16 + step)
    with np.errstate(over="ignore"):
        z = (seed << np.uint64(32)) | np.arange(T, dtype=np.uint64)
        z = z + np.uint64(0x9E3779B97F4A7C15)
        z = (z ^ (z >> np.uint64(30))) * np.uint64(0xBF58476D1CE4E5B9)
        z = (z ^ (z >> np.uint64(27))) * np.uint64(0x94D049BB133111EB)
        z = z ^ (z >> np.uint64(31))
    return ((z >> np.uint64(40)).astype(np.float64) / 16777216.0).astype(np.float32)[None]


# ---------------------------------------------------------------- slicing (GAME slicer2)
def _rms(y, win, hop):
    y = np.pad(y, (win // 2, win // 2))
    n = 1 + (len(y) - win) // hop
    return np.array([np.sqrt(np.mean(np.abs(y[i * hop : i * hop + win]) ** 2)) for i in range(n)])


def slice_audio(w, threshold_db=-40.0, min_length=1000, min_interval=200, hop_ms=20, max_sil_kept=100):
    thr = 10 ** (threshold_db / 20.0)
    mi = SR * min_interval / 1000
    hop = round(SR * hop_ms / 1000)
    win = min(round(mi), 4 * hop)
    min_len = round(SR * min_length / 1000 / hop)
    mi = round(mi / hop)
    msk = round(SR * max_sil_kept / 1000 / hop)
    if (len(w) + hop - 1) // hop <= min_len:
        return [(0.0, w)]
    rms = _rms(w, win, hop)
    tags, ss, cs = [], None, 0
    for i, r in enumerate(rms):
        if r < thr:
            if ss is None:
                ss = i
            continue
        if ss is None:
            continue
        lead = ss == 0 and i > msk
        mid = i - ss >= mi and i - cs >= min_len
        if not lead and not mid:
            ss = None
            continue
        if i - ss <= msk:
            pos = int(rms[ss : i + 1].argmin()) + ss
            tags.append((0, pos) if ss == 0 else (pos, pos))
            cs = pos
        elif i - ss <= msk * 2:
            pos = int(rms[i - msk : ss + msk + 1].argmin()) + i - msk
            pl = int(rms[ss : ss + msk + 1].argmin()) + ss
            pr = int(rms[i - msk : i + 1].argmin()) + i - msk
            if ss == 0:
                tags.append((0, pr))
                cs = pr
            else:
                tags.append((min(pl, pos), max(pr, pos)))
                cs = max(pr, pos)
        else:
            pl = int(rms[ss : ss + msk + 1].argmin()) + ss
            pr = int(rms[i - msk : i + 1].argmin()) + i - msk
            tags.append((0, pr) if ss == 0 else (pl, pr))
            cs = pr
        ss = None
    total = rms.shape[0]
    if ss is not None and total - ss >= mi:
        se = min(total, ss + msk)
        pos = int(rms[ss : se + 1].argmin()) + ss
        tags.append((pos, total + 1))
    if not tags:
        return [(0.0, w)]

    def cut(b, e):
        return (b * hop / SR, w[b * hop : min(len(w), e * hop)])

    out = []
    if tags[0][0] > 0:
        out.append(cut(0, tags[0][0]))
    for a, b in zip(tags[:-1], tags[1:]):
        out.append(cut(a[1], b[0]))
    if tags[-1][1] < total:
        out.append(cut(tags[-1][1], total))
    return out


def split_long(slices, hop=882):
    """Extra memory guard: cut slices > MAX_SLICE_S at the quietest 20 ms frame in [20 s, 30 s)."""
    out = []
    for off, w in slices:
        while len(w) > MAX_SLICE_S * SR:
            a, b = int(20 * SR) // hop, int(MAX_SLICE_S * SR) // hop
            e = np.array([np.mean(w[i * hop : (i + 1) * hop] ** 2) for i in range(a, b)])
            k = (a + int(np.argmin(e))) * hop
            out.append((off, w[:k]))
            off, w = off + k / SR, w[k:]
        out.append((off, w))
    return out


# ---------------------------------------------------------------- one slice
def infer_slice(s, w, lang=LANG_EN, slice_index=0):
    dur = np.array([len(w) / SR], np.float32)
    x_seg, x_est, mask = s["encoder"].run(None, {"waveform": w[None].astype(np.float32), "duration": dur})
    kb = s["dur2bd"].run(None, {"durations": dur[None], "maskT": mask})[0] & mask
    b = kb
    T = mask.shape[1]
    for k, t in enumerate(TS):
        b = s["segmenter"].run(None, {"noise": noise(slice_index, k, T),
            "x_seg": x_seg, "language": np.array([lang], np.int64), "known_boundaries": kb,
            "prev_boundaries": b, "t": np.array([t], np.float32), "maskT": mask,
            "threshold": np.array(SEG_THRESHOLD, np.float32), "radius": np.array(SEG_RADIUS, np.int64)})[0]
    durations, mask_n = s["bd2dur"].run(None, {"boundaries": b, "maskT": mask})
    presence, scores = s["estimator"].run(None, {
        "x_est": x_est, "boundaries": b, "maskT": mask, "maskN": mask_n,
        "threshold": np.array(EST_THRESHOLD, np.float32)})
    return durations[0], presence[0], scores[0], b[0]


def notes_from_slice(offset, length_s, durations, presence, scores):
    d = np.asarray(durations, np.float32)
    on = np.cumsum(np.concatenate([np.zeros(1, np.float32), d]), dtype=np.float32)[:-1]
    on = np.minimum(on, np.float32(length_s)) + np.float32(offset)  # all float32, like the original
    off = np.minimum(np.cumsum(d, dtype=np.float32), np.float32(length_s)) + np.float32(offset)
    out = []
    for a, b, sc, ok in zip(on.tolist(), off.tolist(), scores.tolist(), presence.tolist()):
        if b - a <= 0 or not ok:
            continue
        out.append([a, b, float(sc)])
    return out


def combine(notes):
    notes = sorted(notes, key=lambda n: (n[0], n[1], n[2]))
    last, res = 0.0, []
    for a, b, p in notes:
        a = max(a, last)
        b = max(b, a)
        if b <= a:
            continue
        last = b
        res.append({"start": a, "end": b, "pitch": int(round(p)), "pitch_f": p, "velocity": 100})
    return res


def transcribe(s, audio, lang=LANG_EN):
    """mono f32 at 44.1 kHz -> notes."""
    raw = []
    for i, (off, w) in enumerate(split_long(slice_audio(audio.astype(np.float32)))):
        d, pr, sc, _ = infer_slice(s, w, lang, i)
        raw += notes_from_slice(off, len(w) / SR, d, pr, sc)
    return combine(raw)
