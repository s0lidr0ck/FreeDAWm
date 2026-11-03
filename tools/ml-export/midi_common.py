"""Shared helpers for the audio-to-MIDI exports (evaluation side only; reference_infer.py files are
self-contained): note lists -> .mid, a simple synth for audible checks, and accuracy proxies."""

from __future__ import annotations

import json
from pathlib import Path

import numpy as np

GM_DRUM_NAMES = {35: "Acoustic Bass Drum", 36: "Bass Drum 1", 38: "Acoustic Snare", 42: "Closed Hi-Hat",
                 46: "Open Hi-Hat", 47: "Low-Mid Tom", 49: "Crash Cymbal 1", 51: "Ride Cymbal 1"}


def write_midi(path, notes, *, drums=False, program=0, pedals=None, bends=None, tempo_bpm=120.0):
    """notes: dicts with start, end (s), pitch, velocity. pedals: list of (start, end) sustain.
    bends: list of (time_s, value -8192..8191). Single track, 480 ticks per beat."""
    import mido

    tpb = 480
    sec_per_tick = 60.0 / tempo_bpm / tpb
    ch = 9 if drums else 0
    ev = []  # (tick, order, msg)
    for n in notes:
        on = int(round(n["start"] / sec_per_tick))
        off = max(on + 1, int(round(n["end"] / sec_per_tick)))
        v = int(min(127, max(1, n["velocity"])))
        ev.append((on, 2, mido.Message("note_on", note=int(n["pitch"]), velocity=v, channel=ch)))
        ev.append((off, 0, mido.Message("note_off", note=int(n["pitch"]), velocity=0, channel=ch)))
    for a, b in pedals or []:
        ev.append((int(round(a / sec_per_tick)), 1, mido.Message("control_change", control=64, value=127, channel=ch)))
        ev.append((int(round(b / sec_per_tick)), 0, mido.Message("control_change", control=64, value=0, channel=ch)))
    for t, val in bends or []:
        ev.append((int(round(t / sec_per_tick)), 1, mido.Message("pitchwheel", pitch=int(val), channel=ch)))
    ev.sort(key=lambda e: (e[0], e[1]))
    mid = mido.MidiFile(ticks_per_beat=tpb)
    tr = mido.MidiTrack()
    mid.tracks.append(tr)
    tr.append(mido.MetaMessage("set_tempo", tempo=mido.bpm2tempo(tempo_bpm), time=0))
    if not drums:
        tr.append(mido.Message("program_change", program=program, channel=ch, time=0))
    last = 0
    for t, _, m in ev:
        tr.append(m.copy(time=t - last))
        last = t
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    mid.save(str(path))


def synth(notes, sr, length, *, drums=False, pedals=None):
    """Plain additive 'piano-ish' synth (decaying harmonics); drums: kick = pitched thump,
    snare/hats/cymbals = filtered noise bursts, toms = pitched thump."""
    y = np.zeros(length)
    rng = np.random.default_rng(0)

    def sustained_end(n):
        e = n["end"]
        for a, b in pedals or []:
            if a <= e <= b:
                e = max(e, b)
        return e

    for n in notes:
        a = int(n["start"] * sr)
        if a >= length:
            continue
        g = (n["velocity"] / 127.0) ** 2
        if drums:
            p = n["pitch"]
            dur = 0.25 if p in (35, 36, 47) else (0.15 if p in (38, 42) else 0.8)
            m = min(length - a, int(dur * sr))
            t = np.arange(m) / sr
            if p in (35, 36):
                s = np.sin(2 * np.pi * (50 + 80 * np.exp(-t * 30)) * t) * np.exp(-t * 12)
            elif p == 47:
                s = np.sin(2 * np.pi * 120 * t) * np.exp(-t * 10)
            else:
                s = rng.standard_normal(m) * np.exp(-t * (25 if p in (38, 42) else 4))
                if p != 38:
                    s = np.diff(s, prepend=0.0)  # brighter
            y[a : a + m] += 0.4 * g * s
            continue
        e = min(length, int(sustained_end(n) * sr) + int(0.05 * sr))
        m = e - a
        if m <= 0:
            continue
        t = np.arange(m) / sr
        f = 440.0 * 2 ** ((n["pitch"] - 69) / 12)
        s = sum(np.sin(2 * np.pi * f * k * t) / k ** 1.5 for k in (1, 2, 3, 4) if f * k < sr / 2)
        env = np.exp(-t * 1.5) * np.minimum(1, t / 0.005)
        rel = np.ones(m)
        r = min(m, int(0.05 * sr))
        rel[-r:] = np.linspace(1, 0, r)
        y[a:e] += 0.15 * g * s * env * rel
    return y


def chroma_corr(src, syn, sr):
    """Mean frame-wise cosine similarity of CQT chroma (frames where the source is above -50 dB of
    its peak). 1.0 = same pitch classes over time. A proxy, not a note F1."""
    import librosa

    cs = librosa.feature.chroma_cqt(y=src.astype(np.float32), sr=sr, hop_length=2048)
    cy = librosa.feature.chroma_cqt(y=syn.astype(np.float32), sr=sr, hop_length=2048)
    n = min(cs.shape[1], cy.shape[1])
    cs, cy = cs[:, :n], cy[:, :n]
    rms = librosa.feature.rms(y=src.astype(np.float32), hop_length=2048)[0][:n]
    act = rms > rms.max() * 10 ** (-50 / 20)
    num = np.sum(cs * cy, 0)
    den = np.linalg.norm(cs, axis=0) * np.linalg.norm(cy, axis=0) + 1e-9
    return float(np.mean((num / den)[act]))


def onset_fmeasure(ref_times, est_times, tol=0.05):
    """Greedy one-to-one onset matching within +-tol s (used for the drums vs mic-onset proxy)."""
    ref = sorted(ref_times)
    est = sorted(est_times)
    used = set()
    hit = 0
    j0 = 0
    for r in ref:
        while j0 < len(est) and est[j0] < r - tol:
            j0 += 1
        j = j0
        while j < len(est) and est[j] <= r + tol:
            if j not in used:
                used.add(j)
                hit += 1
                break
            j += 1
    p = hit / max(1, len(est))
    rc = hit / max(1, len(ref))
    return {"precision": p, "recall": rc, "f": 2 * p * rc / max(1e-9, p + rc), "n_ref": len(ref), "n_est": len(est)}


def save_notes_json(path, notes, extra=None):
    obj = {"notes": notes}
    if extra:
        obj.update(extra)
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text(json.dumps(obj, indent=1) + "\n", encoding="utf-8")


def notes_equal(a, b, tol_s=1e-6):
    """Compare two note lists (same order) for parity: count of exact matches (pitch, velocity,
    start/end within tol)."""
    keys = lambda n: (round(n["start"], 4), n["pitch"])  # noqa: E731
    sa = sorted(a, key=keys)
    sb = sorted(b, key=keys)
    if len(sa) != len(sb):
        return False, f"count {len(sa)} vs {len(sb)}"
    bad = 0
    for x, y in zip(sa, sb):
        if (x["pitch"] != y["pitch"] or x["velocity"] != y["velocity"] or abs(x["start"] - y["start"]) > tol_s
                or abs(x["end"] - y["end"]) > tol_s):
            bad += 1
    return bad == 0, f"{len(sa) - bad}/{len(sa)} identical"


SOURCES = {  # name: (files, gain) - mono sums of the owner's stems, 7:00-7:30
    "piano": (["Piano L.wav", "Piano R.wav"], 0.5),
    "bass": (["Bass.wav"], 1.0),
    "lead": (["lead.wav"], 1.0),
    "acoustic": (["Acoustic.wav"], 1.0),
    "drums": (["Kick In.wav", "Kick Out.wav", "Snare Top.wav", "Snare Bottom.wav", "Tom H.wav", "Tom L.wav",
               "OH L.wav", "OH R.wav"], 0.5),
}


def load_source(name, start_s=420.0, dur_s=30.0):
    """Mono 48 kHz float32 sum of the source's mic files (read-only media)."""
    import common as C

    files, g = SOURCES[name]
    y = 0
    for f in files:
        x, sr = C.read_wav(C.MEDIA / f, start_s, dur_s)
        y = y + x[0].astype(np.float64)
    y = y * g
    peak = np.max(np.abs(y))
    if peak > 0.99:
        y = y * 0.99 / peak
    return y.astype(np.float32), sr
