"""Build the synthetic stereo test mix (7:00-7:30 of "Show Me Your Face") with its true stems.

Input: the owner's 48 kHz mono multitracks (read-only). Output in <work>/testmix/ (48 kHz,
32-bit float stereo WAV): mix.wav, vocals.wav, drums.wav, bass.wav, other.wav, plus piano.wav and
guitar.wav (the parts of other, for BS-RoFormer SW),
lead_vocal.wav / backing_vocals.wav (karaoke truth) and drums_<part>.wav (drum-split truth,
same pan/gain as in the mix). The stems sum exactly to the mix.

Caveat: this is a live recording, so every mic carries bleed (drums in the vocal mics, etc.).
The "true" stems are the mic groups, not isolated sources, which caps the achievable SDR.
"""

from __future__ import annotations

import json

import numpy as np

from common import MEDIA, TESTMIX, read_wav, write_wav_f32

START_S = 420.0  # 7:00
DUR_S = 30.0

# (file, group, gain dB, pan -1..1). Constant-power pan.
TRACKS = [
    ("lead.wav", "vocals:lead", 0.0, 0.0),
    ("Dessy.wav", "vocals:backing", -4.0, -0.35),
    ("backing1.wav", "vocals:backing", -2.0, 0.35),
    ("backing2.wav", "vocals:backing", -3.0, -0.55),
    ("backing3.wav", "vocals:backing", -3.0, 0.55),
    ("backing4.wav", "vocals:backing", 0.0, -0.2),
    ("backing5.wav", "vocals:backing", 0.0, 0.2),
    ("Misti.wav", "vocals:backing", 0.0, 0.7),
    ("Kick In.wav", "drums:kick", 4.0, 0.0),
    ("Snare Top.wav", "drums:snare", -4.0, 0.05),
    ("Tom H.wav", "drums:toms", 0.0, -0.3),
    ("Tom L.wav", "drums:toms", 0.0, 0.35),
    ("OH L.wav", "drums:overheads", -4.0, -0.9),
    ("OH R.wav", "drums:overheads", -2.0, 0.9),
    ("Bass.wav", "bass", 2.0, 0.0),
    ("Piano L.wav", "other:piano", -1.0, -0.8),
    ("Piano R.wav", "other:piano", -1.0, 0.8),
    ("Acoustic.wav", "other:acoustic", -3.0, -0.4),
    ("Electric L.wav", "other:electric", 2.0, -0.7),
    ("Electirc R.wav", "other:electric", 2.0, 0.7),  # sic: the file name is misspelt
]


def pan(mono: np.ndarray, p: float) -> np.ndarray:
    a = (p + 1.0) * np.pi / 4.0
    return np.stack([mono * np.cos(a), mono * np.sin(a)])


def main():
    TESTMIX.mkdir(parents=True, exist_ok=True)
    groups: dict[str, np.ndarray] = {}
    sr = None
    for name, group, gain_db, p in TRACKS:
        x, sr = read_wav(MEDIA / name, START_S, DUR_S)
        st = pan(x[0].astype(np.float64) * 10 ** (gain_db / 20), p)
        groups[group] = groups.get(group, 0) + st
    mix = sum(groups.values())
    scale = 10 ** (-1.0 / 20) / np.max(np.abs(mix))  # mix peak -1 dBFS
    out = {"mix": mix}
    for top in ("vocals", "drums", "bass", "other"):
        out[top] = sum(v for k, v in groups.items() if k.split(":")[0] == top)
    out["lead_vocal"] = groups["vocals:lead"]
    out["backing_vocals"] = groups["vocals:backing"]
    for k, v in groups.items():
        if k.startswith("drums:"):
            out["drums_" + k.split(":")[1]] = v
    # BS-RoFormer SW truth: piano, guitar (acoustic + electric); other_rest is what's left of other.
    out["piano"] = groups["other:piano"]
    out["guitar"] = groups["other:acoustic"] + groups["other:electric"]
    for k, v in out.items():
        write_wav_f32(TESTMIX / f"{k}.wav", (v * scale).astype(np.float32), sr)
    meta = {
        "source": str(MEDIA),
        "start_s": START_S,
        "dur_s": DUR_S,
        "samplerate": sr,
        "scale": scale,
        "tracks": TRACKS,
    }
    (TESTMIX / "testmix.json").write_text(json.dumps(meta, indent=2), encoding="utf-8")
    print("testmix written to", TESTMIX, "scale", scale)


if __name__ == "__main__":
    main()
