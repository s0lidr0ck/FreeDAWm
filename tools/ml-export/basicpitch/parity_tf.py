"""Basic Pitch parity, run in a venv with tensorflow + basic-pitch (no-deps) + onnxruntime:
the original `basic_pitch.inference.predict` with the TF SavedModel (the original) and with the
bundled ONNX, vs reference_infer.py. Usage: python parity_tf.py <wav 22050 mono> <onnx> <out.json>"""

import json
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import reference_infer as RI  # noqa: E402


def snr(a, b):
    a = a.astype(np.float64)
    d = np.sum((a - b) ** 2)
    return float("inf") if d == 0 else float(10 * np.log10(np.sum(a**2) / d))


def to_notes(events):
    return sorted([{"start": float(s), "end": float(e), "pitch": int(p), "velocity": int(np.round(127 * a)),
                    "bends": [int(x) for x in b] if b else []} for s, e, p, a, b in events],
                  key=lambda n: (n["start"], n["pitch"]))


def cmp(a, b):
    if len(a) != len(b):
        return f"count {len(a)} vs {len(b)}"
    same = sum(1 for x, y in zip(a, b) if x["pitch"] == y["pitch"] and x["velocity"] == y["velocity"]
               and abs(x["start"] - y["start"]) < 1e-6 and abs(x["end"] - y["end"]) < 1e-6 and x["bends"] == y["bends"])
    return f"{same}/{len(a)} identical"


def tf1_model(path, Model):
    """The SavedModel through the TF1 session loader (TF 2.21's object loader fails on its
    optimizer slots); same graph and variables, graph-mode execution."""
    import tensorflow as tf

    tf.compat.v1.disable_eager_execution()
    s = tf.compat.v1.Session()
    sig = tf.compat.v1.saved_model.loader.load(s, ["serve"], str(path)).signature_def["serving_default"]
    outs = {k: v.name for k, v in sig.outputs.items()}
    inp = sig.inputs["input_2"].name
    m = Model.__new__(Model)
    m.predict = lambda x: s.run(outs, {inp: x})
    return m


def main(wav, onnx_path, out):
    import soundfile as sf
    from basic_pitch import ICASSP_2022_MODEL_PATH
    from basic_pitch.inference import Model, predict

    audio, sr = sf.read(wav, dtype="float32")
    assert sr == 22050 and audio.ndim == 1
    sess = RI.make_session(onnx_path)
    notes, raw = RI.transcribe(sess, audio)
    res = {}
    tf_path = Path(ICASSP_2022_MODEL_PATH)
    for tag, model in (("tf_savedmodel", tf1_model(tf_path, Model)), ("bundled_onnx", Model(Path(onnx_path)))):
        mo, _, ev = predict(wav, model)
        on = to_notes(ev)
        res[tag] = {
            "snr_db": {k: snr(mo[k], raw[k]) for k in ("note", "onset", "contour")},
            "notes": cmp(on, notes),
            "n_notes": len(on),
        }
    res["n_notes_reference"] = len(notes)
    Path(out).write_text(json.dumps(res, indent=2))
    print(json.dumps(res))


if __name__ == "__main__":
    main(*sys.argv[1:4])
