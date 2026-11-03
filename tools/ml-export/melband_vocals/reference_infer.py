"""Mel-Band RoFormer vocals (Kimberley Jensen, MIT) reference pipeline: numpy + onnxruntime only.

Same graph I/O, STFT and chunking as `../karaoke/reference_infer.py` (the spec the Rust lane ports;
`crates/ml/src/karaoke.rs` runs both), with two differences:
  - the stems are `vocals` (the graph's target) and `instrumental` = input - vocals;
  - NUM_OVERLAP = 2 (the model's MSS config `inference.num_overlap`): STEP = CHUNK // 2 = 176400,
    BORDER = CHUNK - STEP = 176400 (FADE stays CHUNK // 10 = 35280).
The installed graph may be fp16 inside (`card.json` -> `precision`); its input and output stay f32.
"""

from __future__ import annotations

import importlib.util
from pathlib import Path

import numpy as np

_spec = importlib.util.spec_from_file_location("_karaoke_ri", Path(__file__).resolve().parents[1] / "karaoke" / "reference_infer.py")
_k = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_k)

SR = _k.SR
CHUNK = _k.CHUNK
NUM_OVERLAP = 2
N_FFT = _k.N_FFT
HOP = _k.HOP
STEMS = ["vocals", "instrumental"]
make_session = _k.make_session
stft = _k.stft
istft = _k.istft
model_chunk = _k.model_chunk


def separate(sess, x: np.ndarray, progress=None) -> np.ndarray:
    """x: f32 [2, L] at 44.1 kHz -> f32 [2 (vocals, instrumental), 2, L]."""
    return _k.separate(sess, x, progress, num_overlap=NUM_OVERLAP)


if __name__ == "__main__":
    import sys

    import soundfile as sf

    onnx_path, wav_in, out_prefix = sys.argv[1:4]
    audio, sr = sf.read(wav_in, dtype="float32", always_2d=True)
    assert sr == SR, "resample to 44100 Hz first"
    audio = audio.T if audio.shape[1] == 2 else np.repeat(audio.T, 2, 0)
    for name, s in zip(STEMS, separate(make_session(onnx_path), audio)):
        sf.write(f"{out_prefix}{name}.wav", s.T, SR, subtype="FLOAT")
