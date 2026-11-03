"""Speed + EP agreement bench: run each installed model's reference pipeline on CPU and DirectML.

Run with a venv that has `onnxruntime-directml` (it cannot share a venv with `onnxruntime`):
    <DAW>/ml-venv-dml/Scripts/python.exe tools/ml-export/bench.py [ids...]
Per model: session-creation time, 30 s of test material end-to-end, and the DML-vs-golden SNR
(the golden files are CPU outputs). Results go to <work>/bench.json.
"""

from __future__ import annotations

import importlib.util
import json
import sys
import time
from pathlib import Path

import numpy as np
import onnxruntime as ort

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import common as C  # noqa: E402

MODELS = {  # id: (folder, run fn, golden shapes, 30 s test input)
    "htdemucs": ("htdemucs", "separate", ([2, 220500], [4, 2, 220500]), "mix"),
    "drumsep_mdx23c": ("drumsep", "separate", ([2, 220500], [6, 2, 220500]), "drums"),
    "srs_small": ("srs", "restore", ([240000], [240000]), "lead"),
    "karaoke_melroformer": ("karaoke", "separate", ([2, 220500], [2, 2, 220500]), "vocals"),
    "vocals_melroformer": ("melband_vocals", "separate", ([2, 220500], [2, 2, 220500]), "mix"),
    "bs_roformer_sw": ("bs_roformer_sw", "separate", ([2, 220500], [6, 2, 220500]), "mix"),
}


def load_ri(folder):
    spec = importlib.util.spec_from_file_location(f"ri_{folder}", HERE / folder / "reference_infer.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def test_input(kind, sr_model):
    if kind == "lead":
        return C.read_wav(C.MEDIA / "lead.wav", 420.0, 30.0)[0][0]
    x, sr = C.read_wav(C.TESTMIX / f"{kind}.wav")
    return C.resample(x, sr, sr_model)


def main(ids):
    out = {"ort": ort.__version__, "providers": ort.get_available_providers()}
    bench_path = C.WORK / "bench.json"
    if bench_path.exists():
        out = {**json.loads(bench_path.read_text()), **out}
    for mid in ids:
        folder, fn, (gin, gout), kind = MODELS[mid]
        ri = load_ri(folder)
        card = json.loads((C.MODELS / mid / "card.json").read_text())
        onnx = C.MODELS / mid / card["file"]
        gx = np.fromfile(C.MODELS / mid / "golden" / "input_5s.f32", "<f4").reshape(gin)
        gy = np.fromfile(C.MODELS / mid / "golden" / "output_5s.f32", "<f4").reshape(gout)
        x30 = test_input(kind, ri.SR)
        res = {}
        for ep in ("DmlExecutionProvider", "CPUExecutionProvider"):
            if ep not in ort.get_available_providers():
                continue
            t0 = time.perf_counter()
            try:
                so = ort.SessionOptions()
                if ep == "DmlExecutionProvider":
                    for k, v in card.get("dml_session_config", {}).items():
                        so.add_session_config_entry(k, v)
                sess = ort.InferenceSession(str(onnx), so, providers=[ep, "CPUExecutionProvider"])
            except Exception as e:  # noqa: BLE001
                res[ep] = {"error": str(e)[:300]}
                continue
            t_load = time.perf_counter() - t0
            y = getattr(ri, fn)(sess, gx)
            snr = C.snr_db(gy, y)
            t0 = time.perf_counter()
            getattr(ri, fn)(sess, x30)
            t30 = time.perf_counter() - t0
            res[ep] = {"session_s": round(t_load, 2), "golden_snr_db": round(snr, 1),
                       "audio_30s_s": round(t30, 2), "x_realtime": round(30.0 / t30, 2)}
            print(mid, ep, res[ep], flush=True)
            del sess
        out[mid] = res
        bench_path.write_text(json.dumps(out, indent=2))
    return out


if __name__ == "__main__":
    main(sys.argv[1:] or list(MODELS))
