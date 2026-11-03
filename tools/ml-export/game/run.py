"""GAME (singing voice -> MIDI): fetch repo + small checkpoint -> export with GAME's own
deploy.py -> parity (original PyTorch pipeline vs reference_infer.py) -> golden -> lead
(and the bass, for comparison) -> install ml-models/game_vocal/ (5 graphs + config.json)."""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import time
import zipfile
from pathlib import Path

import numpy as np
import soundfile as sf

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE))

import common as C  # noqa: E402
import midi_common as M  # noqa: E402
import midi_run  # noqa: E402
import reference_infer as RI  # noqa: E402

ID = "game_vocal"
COMMIT = "eb08841f74d1368d8f03086eac3d5b1a2bf2d436"
REPO_URL = f"https://github.com/openvpi/GAME/archive/{COMMIT}.zip"
PT_URL = "https://github.com/openvpi/GAME/releases/download/v1.0.0/GAME-1.0-small.zip"
DL = C.WORK / "dl" / "game"
WORK = C.WORK / "game"
ONNX_DIR = WORK / "onnx"


def fetch():
    C.download(REPO_URL, DL / "repo.zip")
    C.download(PT_URL, DL / "small-pt.zip")
    for z in ("repo", "small-pt"):
        if not (DL / z).exists():
            zipfile.ZipFile(DL / f"{z}.zip").extractall(DL / z)
    return DL / "repo" / f"GAME-{COMMIT}", DL / "small-pt" / "GAME-1.0-small" / "model.pt"


def main(force=False):
    if not force and C.card_is_current(ID):
        print(f"{ID}: installed card matches, skipping")
        return None
    repo, pt = fetch()
    if force or not (ONNX_DIR / "segmenter.onnx").exists():
        subprocess.run([sys.executable, str(HERE / "export.py"), str(pt), str(ONNX_DIR)], cwd=repo, check=True)
    s = RI.make_sessions(ONNX_DIR)
    report = {"id": ID}

    # parity: 20 s of lead (several slices), language en
    x = C.resample(M.load_source("lead", 425.0, 20.0)[0], 48000, RI.SR)
    wav = WORK / "parity_lead20_44100.wav"
    sf.write(str(wav), x, RI.SR, subtype="FLOAT")
    x = sf.read(str(wav), dtype="float32")[0]
    pj = WORK / "parity_torch.json"
    t0 = time.perf_counter()
    subprocess.run([sys.executable, str(HERE / "parity_torch.py"), str(pt), str(wav), str(RI.LANG_EN), str(pj)],
                   cwd=repo, check=True)
    t_torch = time.perf_counter() - t0
    orig = json.loads(pj.read_text())
    t0 = time.perf_counter()
    notes = RI.transcribe(s, x)
    t_ort = time.perf_counter() - t0
    sl = RI.split_long(RI.slice_audio(x))
    same_slices = len(sl) == len(orig["slices"]) and all(
        abs(a[0] - b["offset"]) < 1e-9 and len(a[1]) == b["n"] for a, b in zip(sl, orig["slices"]))
    o = orig["notes"]
    match = len(o) == len(notes) and all(abs(a["start"] - b["start"]) < 1e-5 and abs(a["end"] - b["end"]) < 1e-5
                                         and abs(a["pitch_f"] - b["pitch_f"]) < 1e-3 for a, b in zip(o, notes))
    pitch_err = [abs(a["pitch_f"] - b["pitch_f"]) for a, b in zip(o, notes)]
    report["parity_20s"] = {"slices_identical": same_slices, "n_slices": len(sl), "n_notes": [len(o), len(notes)],
                            "notes_match": match, "max_pitch_err_semitones": max(pitch_err, default=0.0),
                            "time_s": {"torch_cpu_incl_load": t_torch, "ort_cpu": t_ort}}
    print("parity", report["parity_20s"])

    gold = WORK / "golden"
    gx = C.resample(M.load_source("lead", 430.0, 5.0)[0], 48000, RI.SR)
    gsl = RI.split_long(RI.slice_audio(gx))
    C.write_f32(gold / "input_5s.f32", gx)
    raw = []
    for i, (off, w) in enumerate(gsl):
        d, pr, sc, b = RI.infer_slice(s, w, RI.LANG_EN, i)
        raw.append({"offset": off, "n": len(w), "durations": d.tolist(), "presence": pr.tolist(),
                    "scores": sc.tolist(), "boundaries_frames": np.nonzero(b)[0].tolist()})
    M.save_notes_json(gold / "notes_5s.json", RI.transcribe(s, gx), {"slices": raw})
    (gold / "README.txt").write_text(
        "Golden files for game_vocal (reference_infer.py, CPU ORT, language en = 1).\n"
        f"input_5s.f32  : [{len(gx)}] mono 44100 Hz raw little-endian float32 (lead.wav 7:10-7:15)\n"
        "notes_5s.json : notes {start, end (s), pitch (rounded), pitch_f (fractional MIDI), velocity 100}\n"
        "                + slices [{offset s, n samples, durations, presence, scores, boundaries_frames}]\n"
        "                (the raw per-slice graph outputs; boundaries after the 8th D3PM step).\n",
        encoding="utf-8")

    report["stems"] = midi_run.evaluate(ID, lambda a: (RI.transcribe(s, a), None), RI.SR, ["lead", "bass"])

    # install: 5 graphs; card "file" = segmenter (the loop graph), others in extra_files
    dst = C.MODELS / ID
    dst.mkdir(parents=True, exist_ok=True)
    extra = []
    for g in RI.GRAPHS:
        shutil.copy2(ONNX_DIR / f"{g}.onnx", dst / f"{g}.onnx")
        if g != "segmenter":
            extra.append({"file": f"{g}.onnx", "sha256": C.sha256(dst / f"{g}.onnx")})
    shutil.copy2(ONNX_DIR / "config.json", dst / "game_config.json")
    card = {
        "id": ID, "name": "GAME small (openvpi) - singing voice to notes", "task": "audio_to_midi",
        "role": "vocal / monophonic melody", "file": "segmenter.onnx", "extra_files": extra,
        "sample_rate": RI.SR, "channels": 1, "frame_s": 0.01,
        "chunk": {"slicer": {"threshold_db": -40, "min_length_ms": 1000, "min_interval_ms": 200, "hop_ms": 20,
                             "max_sil_kept_ms": 100}, "max_slice_s": RI.MAX_SLICE_S},
        "stft": None, "stft_note": "mel front end (n_fft 2048, hop 441, 80 mels, 0-8 kHz) inside encoder.onnx",
        "graphs": {
            "encoder": {"in": {"waveform": [1, "L"], "duration": [1]}, "out": {"x_seg": [1, "T", 128],
                        "x_est": [1, "T", 128], "maskT": [1, "T"]}},
            "dur2bd": {"in": {"durations": [1, 1], "maskT": [1, "T"]}, "out": {"boundaries": [1, "T"]}},
            "segmenter": {"in": {"x_seg": [1, "T", 128], "language": [1], "known_boundaries": [1, "T"],
                                 "prev_boundaries": [1, "T"], "t": [1], "maskT": [1, "T"], "threshold": [],
                                 "radius": [], "noise": [1, "T"]}, "out": {"boundaries": [1, "T"]}},
            "bd2dur": {"in": {"boundaries": [1, "T"], "maskT": [1, "T"]}, "out": {"durations": [1, "N"], "maskN": [1, "N"]}},
            "estimator": {"in": {"x_est": [1, "T", 128], "boundaries": [1, "T"], "maskT": [1, "T"], "maskN": [1, "N"],
                                 "threshold": []}, "out": {"presence": [1, "N"], "scores": [1, "N"]}},
        },
        "decode": {"language": {"unknown": 0, "en": 1, "ja": 2, "yue": 3, "zh": 4, "default": 1},
                   "d3pm_ts": RI.TS, "boundary_threshold": RI.SEG_THRESHOLD, "boundary_radius_frames": RI.SEG_RADIUS,
                   "presence_threshold": RI.EST_THRESHOLD, "velocity": 100},
        "output_notes": "start/end s, MIDI pitch rounded + fractional pitch_f (for pitch bend), monophonic",
        "source_url": f"https://github.com/openvpi/GAME (commit {COMMIT}); weights {PT_URL}; exported with the "
                      "repo's own Exporter (opset 17, non-dynamo) plus an explicit `noise` input on the segmenter",
        "determinism": "GAME's D3PM sampler draws torch.rand_like; here the draws are an input generated by "
                       "SplitMix64 (reference_infer.noise), so outputs are reproducible. Parity is against the "
                       "original PyTorch code fed the same draws.",
        "licence": "MIT (code and released models), openvpi. Trained mostly on Chinese/Japanese singing datasets.",
        "dml_supported": True,
        "dml_note": 'DirectML matches CPU (golden notes identical) but is slower (7.7 s vs 3.4 s per 30 s); use CPU.',
        "recommended_ep": "cpu",
        "reference": "tools/ml-export/game/reference_infer.py", "golden": "golden/README.txt",
    }
    C.install(ID, dst / "segmenter.onnx", card, gold)
    C.mark_ready(ID)
    C.save_json(WORK / "report.json", report)
    return report


if __name__ == "__main__":
    main(force="--force" in sys.argv)
