"""ADTOF drums: export -> parity (ADTOF-pytorch original: librosa features + torch model + its
peak picker vs reference_infer.py) -> golden -> summed drum mics -> install ml-models/adtof_drums/."""

from __future__ import annotations

import sys
import time
from pathlib import Path

import numpy as np
import soundfile as sf
import torch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE))

import common as C  # noqa: E402
import export as EX  # noqa: E402
import midi_common as M  # noqa: E402
import midi_run  # noqa: E402
import reference_infer as RI  # noqa: E402

ID = "adtof_drums"
WORK = C.WORK / "adtof"
ONNX = WORK / "adtof_drums.onnx"


def original(wav_path):
    m = EX.load_model()
    from adtof_pytorch import FRAME_RNN_THRESHOLDS, LABELS_5, PeakPicker, load_audio_for_model

    feats = load_audio_for_model(str(wav_path))
    with torch.no_grad():
        act = m(feats).numpy()
    peaks = PeakPicker(thresholds=FRAME_RNN_THRESHOLDS, fps=100).pick(act, labels=LABELS_5)[0]
    notes = sorted([{"start": float(t), "end": float(t) + 0.1, "pitch": int(p), "velocity": 100}
                    for p, ts in peaks.items() for t in ts], key=lambda n: (n["start"], n["pitch"]))
    return notes, act[0], feats.numpy()


def main(force=False):
    if not force and C.card_is_current(ID):
        print(f"{ID}: installed card matches, skipping")
        return None
    WORK.mkdir(parents=True, exist_ok=True)
    if force or not ONNX.exists():
        EX.export(ONNX)
    sess = RI.make_session(ONNX)
    report = {"id": ID}

    x = C.resample(M.load_source("drums", 425.0, 10.0)[0], 48000, RI.SR)
    wav = WORK / "parity_drums10_44100.wav"
    sf.write(str(wav), x, RI.SR, subtype="FLOAT")
    x = sf.read(str(wav), dtype="float32")[0]
    t0 = time.perf_counter()
    on, oact, ofeat = original(wav)
    t_torch = time.perf_counter() - t0
    t0 = time.perf_counter()
    rn, ract = RI.transcribe(sess, x)
    t_ort = time.perf_counter() - t0
    report["parity_10s"] = {
        "features_snr_db": C.snr_db(ofeat, RI.features(x)),
        "act_snr_db": C.snr_db(oact, ract),
        "notes": M.notes_equal(on, [{k: v for k, v in n.items() if k != "strength"} for n in rn], 1e-6)[1],
        "n_notes": [len(on), len(rn)],
        "time_s": {"torch_cpu": t_torch, "ort_cpu": t_ort},
    }
    print("parity", report["parity_10s"])

    gold = WORK / "golden"
    gx = C.resample(M.load_source("drums", 430.0, 5.0)[0], 48000, RI.SR)
    gn, gact = RI.transcribe(sess, gx)
    C.write_f32(gold / "input_5s.f32", gx)
    C.write_f32(gold / "features_5s.f32", RI.features(gx))
    C.write_f32(gold / "act_5s.f32", gact)
    C.write_f32(gold / "filterbank.f32", RI.filterbank())
    M.save_notes_json(gold / "notes_5s.json", gn)
    T = gact.shape[0]
    (gold / "README.txt").write_text(
        "Golden files for adtof_drums (reference_infer.py, CPU ORT). Raw little-endian float32, C order.\n"
        f"input_5s.f32    : [{len(gx)}] mono 44100 Hz (summed drum mics 7:10-7:15)\n"
        "filterbank.f32  : [84, 1024] log filterbank\n"
        f"features_5s.f32 : [1, {T}, 84, 1] network input\n"
        f"act_5s.f32      : [{T}, 5] activations (GM 35, 38, 47, 42, 49)\n"
        "notes_5s.json   : notes {start, end = start + 0.1, pitch (GM drum), velocity 100, strength}\n",
        encoding="utf-8")

    report["stems"] = midi_run.evaluate(ID, lambda a: (RI.transcribe(sess, a)[0], None), RI.SR, ["drums"], drums=True)

    card = {
        "id": ID, "name": "ADTOF Frame_RNN drum transcription (Zehren et al.; PyTorch port)",
        "task": "audio_to_midi", "role": "drums (5 classes -> GM notes)", "file": "adtof_drums.onnx",
        "sample_rate": RI.SR, "channels": 1,
        "chunk": None, "chunk_note": "whole file in one call (bidirectional GRUs); frames dynamic, 100 fps",
        "stft": {"n_fft": RI.N_FFT, "hop": RI.HOP, "window": "np.hanning (symmetric)", "center": True,
                 "pad_mode": "zeros", "bins": 1024, "filterbank": "madmom log 12/oct 20 Hz-20 kHz, 84 bands, rows sum 1",
                 "compression": "log10(1 + x)"},
        "inputs": [{"name": "spec", "shape": [1, "frames", 84, 1], "dtype": "float32"}],
        "outputs": [{"name": "act", "shape": [1, "frames", 5], "classes_gm": RI.LABELS}],
        "decode": {"thresholds": RI.THRESHOLDS, "pre_avg_frames": 10, "post_avg_frames": 1, "pre_max_frames": 2,
                   "post_max_frames": 1, "combine_frames": 2, "note_length_s": 0.1, "velocity": 100},
        "gm_map": {"35": "bass drum", "38": "snare", "47": "toms", "42": "hi-hat", "49": "cymbals (crash+ride)"},
        "source_url": "https://github.com/xavriley/ADTOF-pytorch (commit " + EX.COMMIT + "), weights converted from "
                      "https://github.com/MZehren/ADTOF",
        "licence": "ADTOF: CC BY-NC-SA 4.0 (non-commercial, share-alike); the PyTorch port has no licence file. "
                   "Private use build only.",
        "dml_supported": True,
        "dml_note": 'DirectML matches CPU (golden notes identical) but is much slower (dynamic-length GRU: 8.2 s vs 0.25 s per 30 s); use CPU.',
        "recommended_ep": "cpu",
        "reference": "tools/ml-export/adtof/reference_infer.py", "golden": "golden/README.txt",
    }
    C.install(ID, ONNX, card, gold)
    C.mark_ready(ID)
    C.save_json(WORK / "report.json", report)
    return report


if __name__ == "__main__":
    main(force="--force" in sys.argv)
