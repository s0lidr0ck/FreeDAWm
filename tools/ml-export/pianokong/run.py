"""Kong piano transcription: export -> parity (piano_transcription_inference.PianoTranscription
on CPU vs reference_infer.py) -> golden -> owner's piano stems -> install ml-models/piano_kong/."""

from __future__ import annotations

import sys
import time
from pathlib import Path

import numpy as np
import torch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE))

import common as C  # noqa: E402
import export as EX  # noqa: E402
import midi_common as M  # noqa: E402
import midi_run  # noqa: E402
import reference_infer as RI  # noqa: E402

ID = "piano_kong"
WORK = C.WORK / "pianokong"
ONNX = WORK / "piano_kong.onnx"


def original(audio16k):
    from piano_transcription_inference import PianoTranscription

    pt = PianoTranscription(device=torch.device("cpu"), checkpoint_path=str(EX.CKPT))
    d = pt.transcribe(audio16k.astype(np.float32), None)
    notes = sorted([{"start": float(e["onset_time"]), "end": float(e["offset_time"]), "pitch": int(e["midi_note"]),
                     "velocity": int(e["velocity"])} for e in d["est_note_events"]],
                   key=lambda n: (n["start"], n["pitch"]))
    pedals = [{"start": float(e["onset_time"]), "end": float(e["offset_time"])} for e in d["est_pedal_events"] or []]
    return notes, pedals, d["output_dict"]


def main(force=False):
    if not force and C.card_is_current(ID):
        print(f"{ID}: installed card matches, skipping")
        return None
    WORK.mkdir(parents=True, exist_ok=True)
    if force or not ONNX.exists():
        EX.export(ONNX)
    sess = RI.make_session(ONNX)
    report = {"id": ID}

    x = C.resample(M.load_source("piano", 425.0, 15.0)[0], 48000, RI.SR)  # 15 s -> 3 segments
    t0 = time.perf_counter()
    on, op, oraw = original(x)
    t_torch = time.perf_counter() - t0
    t0 = time.perf_counter()
    rn, rp, raw = RI.transcribe(sess, x)
    t_ort = time.perf_counter() - t0
    keymap = {"reg_onset": "reg_onset_output", "reg_offset": "reg_offset_output", "frame": "frame_output",
              "velocity": "velocity_output", "reg_pedal_onset": "reg_pedal_onset_output",
              "reg_pedal_offset": "reg_pedal_offset_output", "pedal_frame": "pedal_frame_output"}
    report["parity_15s"] = {
        "snr_db": {k: C.snr_db(oraw[v], raw[k]) for k, v in keymap.items()},
        "notes": M.notes_equal(on, rn, 1e-5)[1], "n_notes": [len(on), len(rn)],
        "pedals_equal": len(op) == len(rp) and all(abs(a["start"] - b["start"]) < 1e-5 and abs(a["end"] - b["end"]) < 1e-5
                                                   for a, b in zip(op, rp)),
        "n_pedals": [len(op), len(rp)],
        "time_s": {"torch_cpu": t_torch, "ort_cpu": t_ort},
    }
    print("parity", report["parity_15s"])

    gold = WORK / "golden"
    gx = C.resample(M.load_source("piano", 430.0, 5.0)[0], 48000, RI.SR)
    gn, gp, graw = RI.transcribe(sess, gx)
    C.write_f32(gold / "input_5s.f32", gx)
    for k, v in graw.items():
        C.write_f32(gold / f"{k}_5s.f32", v)
    M.save_notes_json(gold / "notes_5s.json", gn, {"pedals": gp})
    (gold / "README.txt").write_text(
        "Golden files for piano_kong (reference_infer.py, CPU ORT). Raw little-endian float32, C order.\n"
        f"input_5s.f32 : [{len(gx)}] mono 16 kHz (piano stems 7:10-7:15; one padded 10 s segment)\n"
        "<output>_5s.f32 : [1001, 88] for reg_onset, reg_offset, frame, velocity; [1001, 1] for\n"
        "                  reg_pedal_onset, reg_pedal_offset, pedal_frame (100 frames/s)\n"
        "notes_5s.json : notes {start, end (s), pitch (MIDI), velocity} + pedals {start, end} (sustain)\n",
        encoding="utf-8")

    def tr(a):
        n, p, _ = RI.transcribe(sess, a)
        return n, p

    report["stems"] = midi_run.evaluate(ID, tr, RI.SR, ["piano", "acoustic"])

    card = {
        "id": ID, "name": "High-resolution piano transcription with pedals (ByteDance, Kong et al. 2020)",
        "task": "audio_to_midi", "role": "piano (notes + sustain pedal)", "file": "piano_kong.onnx",
        "sample_rate": RI.SR, "channels": 1,
        "chunk": {"segment_samples": RI.SEG, "hop_samples": RI.SEG // 2, "fps": RI.FPS,
                  "deframe": "drop last frame; keep [0,750) first, [250,750) middle, [250,1000) last"},
        "stft": None, "stft_note": "log-mel front end (n_fft 2048, hop 160, 229 mels, 30-8000 Hz) inside the graph",
        "inputs": [{"name": "audio", "shape": [1, RI.SEG], "dtype": "float32"}],
        "outputs": [{"name": n, "shape": [1, 1001, 1 if "pedal" in n else 88]} for n in RI.OUTS],
        "decode": {"onset_threshold": 0.3, "offset_threshold": 0.3, "frame_threshold": 0.1,
                   "pedal_offset_threshold": 0.2, "pedal_frame_threshold": 0.5, "onset_neighbour": 2,
                   "offset_neighbour": 4, "max_note_frames": 600, "velocity_scale": 128},
        "output_notes": "start/end s (sub-frame regression), MIDI 21-108, velocity 0-127; pedals = sustain on/off",
        "source_url": "https://github.com/bytedance/piano_transcription ; weights https://zenodo.org/record/4034264 ; "
                      "inference package https://github.com/qiuqiangkong/piano_transcription_inference",
        "licence": "Apache-2.0 (bytedance/piano_transcription README). Weights published on Zenodo by the authors with "
                   "the code; trained on MAESTRO (CC BY-NC-SA 4.0 dataset). Private use build.",
        "dml_supported": True,
        "dml_note": 'DirectML matches CPU (golden notes identical) but is slower here (12.5 s vs 4.5 s per 30 s); use CPU.',
        "recommended_ep": "cpu",
        "reference": "tools/ml-export/pianokong/reference_infer.py", "golden": "golden/README.txt",
    }
    C.install(ID, ONNX, card, gold)
    C.mark_ready(ID)
    C.save_json(WORK / "report.json", report)
    return report


if __name__ == "__main__":
    main(force="--force" in sys.argv)
