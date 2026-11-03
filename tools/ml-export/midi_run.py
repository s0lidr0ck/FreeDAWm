"""Shared evaluation for the audio-to-MIDI models: transcribe the owner's stems (7:00-7:30),
write .mid + an audible check WAV (stereo: L = source, R = simple synth of the MIDI) into
ml-renders/midi/, and compute proxies (chroma correlation, drum onset F vs mic onsets)."""

from __future__ import annotations

import time

import numpy as np

import common as C
import midi_common as M

OUT = C.RENDERS / "midi"


def evaluate(model_id, transcribe, sr_model, sources, *, drums=False):
    """transcribe(mono f32 at sr_model) -> (notes, pedals or None). Returns per-source metrics."""
    res = {}
    for name in sources:
        x48, sr = M.load_source(name)
        x = C.resample(x48, sr, sr_model)
        t0 = time.perf_counter()
        notes, pedals = transcribe(x)
        dt = time.perf_counter() - t0
        tag = f"{name}_{model_id}"
        M.write_midi(OUT / f"{tag}.mid", notes, drums=drums, pedals=[(p["start"], p["end"]) for p in pedals or []])
        M.save_notes_json(OUT / f"{tag}.notes.json", notes, {"pedals": pedals} if pedals else None)
        syn = M.synth(notes, sr, len(x48), drums=drums, pedals=[(p["start"], p["end"]) for p in pedals or []])
        g = np.max(np.abs(x48)) / (np.max(np.abs(syn)) + 1e-9)
        C.write_wav24(OUT / f"{tag}_check.wav", np.stack([x48, syn * g * 0.9]), sr)
        r = {"notes": len(notes), "cpu_s_30s": round(dt, 2)}
        if pedals is not None:
            r["pedals"] = len(pedals)
        if drums:
            r.update(drum_proxy(notes))
        else:
            r["chroma_corr"] = round(M.chroma_corr(x48, syn, sr), 3)
            r["pitch_range"] = [min((n["pitch"] for n in notes), default=0), max((n["pitch"] for n in notes), default=0)]
        res[name] = r
        print(model_id, name, r, flush=True)
    return res


def drum_proxy(notes):
    """Onset F-measure (+-50 ms) of the transcribed kick/snare/toms against onsets detected on the
    close mics (librosa onset_detect, backtrack off). Bleed makes the reference itself noisy."""
    import librosa

    out = {}
    for gm, files in ((35, ["Kick In.wav"]), (38, ["Snare Top.wav"]), (47, ["Tom H.wav", "Tom L.wav"])):
        ref = []
        for f in files:
            x, sr = C.read_wav(C.MEDIA / f, 420.0, 30.0)
            on = librosa.onset.onset_detect(y=x[0], sr=sr, units="time", hop_length=256, delta=0.2)
            ref += list(on)
        est = [n["start"] for n in notes if n["pitch"] == gm]
        out[f"onsetF_{gm}"] = {k: (round(v, 3) if isinstance(v, float) else v)
                               for k, v in M.onset_fmeasure(ref, est).items()}
    return out
