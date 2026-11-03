"""BTC large-vocabulary chord recognition reference pipeline: numpy + onnxruntime only.
THIS IS THE SPEC THE RUST LANE PORTS.

Model: btc_chords.onnx. Inputs audio f32 [1, 220500] and n_frames i64 [1]. Outputs logits f32
[1, 108, 170] and cqt f32 [1, 108, 144] (log CQT, 24 bins/octave from C1 = 32.70 Hz).
Classes: i < 168 -> root = i // 14 (C, C#, ..., B), quality = QUALITIES[i % 14]; 168 = X
(unknown chord), 169 = N (no chord).

1. Host: mono (mean), resample to 22050 Hz. L samples.
2. Chunks of 220500 samples (10 s) at 0, 220500, ...: while L > start + 220500 take a full chunk;
   then the remainder [start, L) as the last chunk (1..220500 samples). Each chunk is zero-padded
   to 220500 and run with n_frames = 1 + len(chunk) // 2048. Keep logits[:n_frames] of each.
3. Frame labels = argmax over the 170 classes (first index on ties). Frame f lasts 10/108 s
   (BTC's time unit, not 2048/22050). Segments = runs of equal labels:
   [start_frame * 10/108, end_frame * 10/108), the last ending at total_frames * 10/108.
4. Names for the DAW chord lane (crates/band/src/chord_match.rs parse_chord_name): root name +
   DAW_SUFFIX[quality] (maj -> "C", min -> "Cm", hdim7 -> "Cm7b5", ...). N and X are no-chord
   (not written to the lane).
5. Bass (heuristic, not part of BTC): per segment, mean linear CQT magnitude exp(cqt) over the
   segment's frames in bins 0..47 (C1..B2), on-semitone bins only (even bins), folded to pitch
   classes. If the strongest bass pc is a chord tone other than the root and its energy is at
   least BASS_RATIO x the root's, the segment becomes an inversion "<name>/<bass>". Otherwise the
   bass is the root.
Output: segments [{start, end, label (BTC/mir_eval style e.g. "A:min7"), name (DAW, "" for N/X),
root_pc (-1 for N/X), quality, bass_pc, class}].
"""

from __future__ import annotations

import numpy as np
import onnxruntime as ort

SR = 22050
CHUNK = 220500
HOP = 2048
T = 108
TIME_UNIT = 10.0 / 108
ROOTS = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
QUALITIES = ["min", "maj", "dim", "aug", "min6", "maj6", "min7", "minmaj7", "maj7", "7", "dim7", "hdim7", "sus2", "sus4"]
DAW_SUFFIX = {"min": "m", "maj": "", "dim": "dim", "aug": "aug", "min6": "m6", "maj6": "6", "min7": "m7",
              "minmaj7": "mM7", "maj7": "maj7", "7": "7", "dim7": "dim7", "hdim7": "m7b5", "sus2": "sus2",
              "sus4": "sus4"}
TONES = {"min": [0, 3, 7], "maj": [0, 4, 7], "dim": [0, 3, 6], "aug": [0, 4, 8], "min6": [0, 3, 7, 9],
         "maj6": [0, 4, 7, 9], "min7": [0, 3, 7, 10], "minmaj7": [0, 3, 7, 11], "maj7": [0, 4, 7, 11],
         "7": [0, 4, 7, 10], "dim7": [0, 3, 6, 9], "hdim7": [0, 3, 6, 10], "sus2": [0, 2, 7], "sus4": [0, 5, 7]}
BASS_RATIO = 1.5


def make_session(onnx_path, providers=None):
    return ort.InferenceSession(str(onnx_path), providers=providers or ["CPUExecutionProvider"])


def chunks(audio):
    L, s, out = len(audio), 0, []
    while L > s + CHUNK:
        out.append(audio[s : s + CHUNK])
        s += CHUNK
    out.append(audio[s:])
    return out


def run_network(sess, audio):
    """-> logits [F, 170], cqt [F, 144] for all real frames."""
    lg, cq = [], []
    for c in chunks(audio.astype(np.float32)):
        n = 1 + len(c) // HOP
        x = np.zeros((1, CHUNK), np.float32)
        x[0, : len(c)] = c
        logits, cqt = sess.run(["logits", "cqt"], {"audio": x, "n_frames": np.array([n], np.int64)})
        lg.append(logits[0, :n])
        cq.append(cqt[0, :n])
    return np.concatenate(lg), np.concatenate(cq)


def label_of(c):
    if c == 169:
        return "N"
    if c == 168:
        return "X"
    r, q = ROOTS[c // 14], QUALITIES[c % 14]
    return r if q == "maj" else f"{r}:{q}"


def bass_pc(cqt_seg):
    mag = np.exp(cqt_seg[:, :48]).mean(0)[0::2]  # 24 semitone bins, C1..B2
    pc = np.zeros(12)
    for i, v in enumerate(mag):
        pc[i % 12] += v
    return pc


def segments(frames, cqt):
    segs, s = [], 0
    for f in range(1, len(frames) + 1):
        if f == len(frames) or frames[f] != frames[s]:
            segs.append((s, f, int(frames[s])))
            s = f
    out = []
    for a, b, c in segs:
        d = {"start": a * TIME_UNIT, "end": b * TIME_UNIT, "label": label_of(c), "class": c}
        if c >= 168:
            d.update(name="", root_pc=-1, quality="", bass_pc=-1)
        else:
            root, q = c // 14, QUALITIES[c % 14]
            name = ROOTS[root] + DAW_SUFFIX[q]
            e = bass_pc(cqt[a:b])
            bp = int(np.argmax(e))
            tones = {(root + t) % 12 for t in TONES[q]}
            if bp != root and bp in tones and e[bp] >= BASS_RATIO * e[root]:
                name += "/" + ROOTS[bp]
            else:
                bp = root
            d.update(name=name, root_pc=root, quality=q, bass_pc=bp)
        out.append(d)
    return out


def transcribe(sess, audio):
    """mono f32 at 22050 Hz -> (segments, logits [F, 170])."""
    logits, cqt = run_network(sess, audio)
    frames = np.argmax(logits, axis=1)
    return segments(frames, cqt), logits
