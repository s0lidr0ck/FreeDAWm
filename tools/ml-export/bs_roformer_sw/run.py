"""BS-RoFormer SW (6 stems): export -> parity (MSS demix with the original model, same chunk, vs
reference_infer.py, 10 s of the test mix) -> golden -> SDR on the test mix -> renders -> install
<models>/bs_roformer_sw/ -> READY.txt."""

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
import reference_infer as RI  # noqa: E402

ID = "bs_roformer_sw"
WORK = C.WORK / ID
ONNX = WORK / "bs_roformer_sw.onnx"


def torch_pipeline(x: np.ndarray) -> np.ndarray:
    cfg, model = EX.load_model()  # chunk_size set to EX.CHUNK
    cfg.inference.num_overlap = RI.NUM_OVERLAP
    from utils.model_utils import demix

    res = demix(cfg, model, x, torch.device("cpu"), "bs_roformer")
    return np.stack([res[s] for s in RI.STEMS])


def main(force=False):
    if not force and C.card_is_current(ID):
        print(f"{ID}: installed card matches, skipping")
        return None
    WORK.mkdir(parents=True, exist_ok=True)
    if force or not ONNX.exists():
        EX.export(ONNX)
    sess = RI.make_session(ONNX)
    report = {"id": ID}

    mix48, sr = C.read_wav(C.TESTMIX / "mix.wav")
    mix = C.resample(mix48, sr, RI.SR)
    ex = mix[:, 5 * RI.SR : 15 * RI.SR].copy()
    t0 = time.perf_counter()
    ref = torch_pipeline(ex)
    t_torch = time.perf_counter() - t0
    t0 = time.perf_counter()
    got = RI.separate(sess, ex)
    t_ort = time.perf_counter() - t0
    report["parity_10s"] = {s: {"snr_db": C.snr_db(ref[i], got[i]), "max_abs": C.max_abs(ref[i], got[i])}
                            for i, s in enumerate(RI.STEMS)}
    report["time_10s"] = {"torch_cpu_s": t_torch, "ort_cpu_s": t_ort}
    print("parity", report["parity_10s"], report["time_10s"], flush=True)

    gold = WORK / "golden"
    gx = mix[:, 20 * RI.SR : 25 * RI.SR].copy()
    C.write_f32(gold / "input_5s.f32", gx)
    C.write_f32(gold / "output_5s.f32", RI.separate(sess, gx))
    (gold / "README.txt").write_text(
        "Golden files for bs_roformer_sw (outputs of reference_infer.py on CPU ORT).\n"
        "Raw little-endian float32, PLANAR (C order), 44100 Hz, stereo.\n"
        "input_5s.f32  : [2, 220500]      synthetic test mix, 0:20-0:25\n"
        "output_5s.f32 : [6, 2, 220500]   stems bass, drums, other, vocals, guitar, piano\n"
        "Tolerance suggestion for Rust: SNR >= 60 dB vs these files.\n",
        encoding="utf-8",
    )

    t0 = time.perf_counter()
    est = RI.separate(sess, mix)
    report["testmix_30s_ort_cpu_s"] = time.perf_counter() - t0
    e48 = {s: C.resample(est[i], RI.SR, sr)[:, : mix48.shape[1]] for i, s in enumerate(RI.STEMS)}
    truth = {s: C.read_wav(C.TESTMIX / f"{s}.wav")[0] for s in ("vocals", "drums", "bass", "guitar", "piano", "other")}
    sdr = {s: C.sdr_db(truth[s], e48[s]) for s in ("vocals", "drums", "bass", "guitar", "piano")}
    sdr["other+guitar+piano vs other"] = C.sdr_db(truth["other"], e48["other"] + e48["guitar"] + e48["piano"])
    report["testmix_30s_sdr"] = sdr
    for s, a in e48.items():
        C.write_wav24(C.RENDERS / ID / f"{s}.wav", a, sr)
    print("sdr", sdr, flush=True)

    card = {
        "id": ID,
        "name": "BS-RoFormer SW (6 stems)",
        "task": "separate",
        "file": ONNX.name,
        "precision": "fp32",
        "sample_rate": RI.SR,
        "channels": 2,
        "stems": RI.STEMS,
        "stems_note": "all six from the graph, output rows stem * 2 + channel",
        "chunk": {"samples": RI.CHUNK, "num_overlap": RI.NUM_OVERLAP, "step": RI.CHUNK // RI.NUM_OVERLAP,
                  "fade": RI.CHUNK // 10, "border_reflect_pad": RI.CHUNK - RI.CHUNK // RI.NUM_OVERLAP,
                  "window": "same MSS demix scheme as karaoke_melroformer",
                  "note": "the model config's chunk is 588800 (1151 frames); 409600 (801 frames) runs on DirectML"},
        "stft": {"n_fft": RI.N_FFT, "hop": RI.HOP, "win_length": RI.N_FFT, "window": "hann_periodic",
                 "center": True, "pad_mode": "reflect", "normalized": False, "bins": 1025, "frames": EX.FRAMES,
                 "istft_length": RI.CHUNK},
        "inputs": [{"name": "spec", "shape": [2, 1025, EX.FRAMES, 2], "dtype": "float32",
                    "layout": "[channel, bin, frame, re/im]"}],
        "outputs": [{"name": "stems_spec", "shape": [12, 1025, EX.FRAMES, 2], "dtype": "float32",
                     "layout": "[stem * 2 + channel, bin, frame, re/im], masked spectrograms, DC bin zeroed"}],
        "opset": 17,
        "dml_supported": True,
        "dml_session_config": {"ep.dml.disable_graph_fusion": "1"},
        "dml_note": "As the Mel-Band graphs: wrong output with graph fusion, matches CPU without it.",
        "source_url": "https://huggingface.co/enerjazzer/BS-ROFO-SW-Fixed (BS-Rofo-SW-Fixed.ckpt, .yaml)",
        "licence": "Weights: licence unknown (personal use only, owner's decision). Model code (lucidrains "
                   "BS-RoFormer, ZFTurbo MSS): MIT.",
        "quality": "MVSEP: guitar 9.01, piano 7.80 SDR.",
        "reference": "tools/ml-export/bs_roformer_sw/reference_infer.py",
        "golden": "golden/README.txt",
        "parity_snr_db": {k: round(v["snr_db"], 2) for k, v in report["parity_10s"].items()},
    }
    C.install(ID, ONNX, card, gold)
    C.mark_ready(ID)
    C.save_json(WORK / "report.json", report)
    return report


if __name__ == "__main__":
    main(force="--force" in sys.argv)
