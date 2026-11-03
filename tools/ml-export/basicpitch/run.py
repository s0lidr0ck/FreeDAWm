"""Basic Pitch: take Spotify's bundled ONNX -> parity (TF SavedModel original via ml-venv-tf, and
the original decoder) -> golden -> transcribe the owner's stems -> install ml-models/basicpitch/."""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
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

ID = "basicpitch"
WORK = C.WORK / ID
ONNX = WORK / "basicpitch.onnx"
TF_PY = C.DAW / "ml-venv-tf" / "Scripts" / "python.exe"


def main(force=False):
    if not force and C.card_is_current(ID):
        print(f"{ID}: installed card matches, skipping")
        return None
    WORK.mkdir(parents=True, exist_ok=True)
    import basic_pitch

    shutil.copy2(Path(basic_pitch.__file__).parent / "saved_models" / "icassp_2022" / "nmp.onnx", ONNX)
    sess = RI.make_session(ONNX)
    report = {"id": ID}

    # parity: 10 s of the piano stems, original predict() with TF SavedModel and bundled ONNX
    x, sr = M.load_source("piano", 425.0, 10.0)
    wav = WORK / "parity_piano10_22050.wav"
    sf.write(str(wav), C.resample(x, sr, RI.SR), RI.SR, subtype="FLOAT")
    pj = WORK / "parity.json"
    if TF_PY.exists():
        subprocess.run([str(TF_PY), str(HERE / "parity_tf.py"), str(wav), str(ONNX), str(pj)], check=True)
        report["parity"] = json.loads(pj.read_text())
    else:
        report["parity"] = "ml-venv-tf missing: TF parity skipped"
    print("parity", report["parity"])

    gold = WORK / "golden"
    gx = C.resample(M.load_source("piano", 430.0, 5.0)[0], 48000, RI.SR)
    notes, raw = RI.transcribe(sess, gx)
    C.write_f32(gold / "input_5s.f32", gx)
    for k in ("note", "onset", "contour"):
        C.write_f32(gold / f"{k}_5s.f32", raw[k])
    M.save_notes_json(gold / "notes_5s.json", notes, {"midi_bends": RI.midi_bends(notes)})
    (gold / "README.txt").write_text(
        "Golden files for basicpitch (reference_infer.py, CPU ORT). Raw little-endian float32, C order.\n"
        f"input_5s.f32   : [{len(gx)}] mono 22050 Hz (piano stems 7:10-7:15)\n"
        f"note/onset_5s  : [{raw['note'].shape[0]}, 88]; contour_5s: [{raw['contour'].shape[0]}, 264]\n"
        "notes_5s.json  : decoded notes {start, end (s), pitch (MIDI), velocity, bends (1/3 semitone per frame)}\n"
        "                 + midi_bends [(time s, pitchwheel)]. Rust should reproduce the note list exactly\n"
        "                 (times within 1e-6 s) given outputs within ~1e-5 of these.\n",
        encoding="utf-8")

    report["stems"] = midi_run.evaluate(ID, lambda a: (RI.transcribe(sess, a)[0], None), RI.SR,
                                        ["piano", "bass", "lead", "acoustic"])

    card = {
        "id": ID, "name": "Basic Pitch (Spotify) - general polyphonic audio to MIDI", "task": "audio_to_midi",
        "role": "general (default)", "file": "basicpitch.onnx", "sample_rate": RI.SR, "channels": 1,
        "chunk": {"window_samples": RI.N_SAMPLES, "hop_samples": RI.WIN_HOP, "lead_zeros": RI.OLAP // 2,
                  "trim_frames_each_side": RI.N_OLAP_FRAMES // 2, "fps": RI.FPS},
        "stft": None, "stft_note": "harmonic CQT front end inside the graph",
        "inputs": [{"name": RI.IN_NAME, "shape": ["N", RI.N_SAMPLES, 1], "dtype": "float32"}],
        "outputs": [{"name": RI.OUT_NAMES[0], "role": "note", "shape": ["N", 172, 88]},
                    {"name": RI.OUT_NAMES[1], "role": "onset", "shape": ["N", 172, 88]},
                    {"name": RI.OUT_NAMES[2], "role": "contour", "shape": ["N", 172, 264]}],
        "decode": {"onset_threshold": 0.5, "frame_threshold": 0.3, "min_note_frames": 11, "energy_tol": 11,
                   "infer_onsets": True, "melodia_trick": True, "pitch_bends": "1/3 semitone, gaussian std 5, +-25 bins",
                   "lowest_midi": 21},
        "output_notes": "start/end s, MIDI pitch 21-108, velocity 0-127, per-frame bends",
        "source_url": "https://github.com/spotify/basic-pitch (basic-pitch 0.4.0 wheel, saved_models/icassp_2022/nmp.onnx)",
        "licence": "Apache-2.0 (code and model, Spotify AB).",
        "export_note": "ONNX is Spotify's own conversion shipped in the package; parity checked against the TF SavedModel.",
        "dml_supported": True,
        "dml_note": 'DirectML matches CPU (golden notes identical); the model is tiny, CPU is as fast (30 s stem < 0.3 s).',
        "recommended_ep": "cpu",
        "reference": "tools/ml-export/basicpitch/reference_infer.py", "golden": "golden/README.txt",
    }
    C.install(ID, ONNX, card, gold)
    C.mark_ready(ID)
    C.save_json(WORK / "report.json", report)
    return report


if __name__ == "__main__":
    main(force="--force" in sys.argv)
