"""CPU vs DirectML for the audio-to-MIDI models (run in the onnxruntime-directml venv):
golden input -> notes identical to golden? + time for a 30 s stem. Results in <work>/midi_bench.json."""

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
import midi_common as M  # noqa: E402

MODELS = {"basicpitch": ("basicpitch", "piano"), "piano_kong": ("pianokong", "piano"),
          "adtof_drums": ("adtof", "drums"), "game_vocal": ("game", "lead")}


def load(folder):
    spec = importlib.util.spec_from_file_location(f"ri_{folder}", HERE / folder / "reference_infer.py")
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def run(mid, ri, prov, audio):
    d = C.MODELS / mid
    if mid == "game_vocal":
        s = ri.make_sessions(d, providers=prov)
        return ri.transcribe(s, audio)
    s = ri.make_session(d / json.loads((d / "card.json").read_text())["file"], providers=prov)
    return ri.transcribe(s, audio)[0]


def same(a, b):
    if len(a) != len(b):
        return f"{len(a)} vs {len(b)} notes"
    bad = sum(1 for x, y in zip(a, b) if x["pitch"] != y["pitch"] or abs(x["start"] - y["start"]) > 1e-3
              or abs(x["end"] - y["end"]) > 1e-3 or x["velocity"] != y["velocity"])
    return f"{len(a) - bad}/{len(a)} notes equal (1 ms)"


def main():
    out = {"ort": ort.__version__}
    for mid, (folder, src) in MODELS.items():
        ri = load(folder)
        gx = np.fromfile(C.MODELS / mid / "golden" / "input_5s.f32", "<f4")
        gn = json.loads((C.MODELS / mid / "golden" / "notes_5s.json").read_text())["notes"]
        x48, sr = M.load_source(src)
        x = C.resample(x48, sr, ri.SR)
        res = {}
        for ep in ("DmlExecutionProvider", "CPUExecutionProvider"):
            prov = [ep, "CPUExecutionProvider"]
            try:
                n = run(mid, ri, prov, gx)
                t0 = time.perf_counter()
                run(mid, ri, prov, x)
                res[ep] = {"golden": same(gn, n), "stem_30s_s": round(time.perf_counter() - t0, 2)}
            except Exception as e:  # noqa: BLE001
                res[ep] = {"error": str(e)[:200]}
            print(mid, ep, res[ep], flush=True)
        out[mid] = res
    (C.WORK / "midi_bench.json").write_text(json.dumps(out, indent=2))


if __name__ == "__main__":
    main()
