"""Mel-Band RoFormer karaoke: export -> parity (MSS demix with the original model vs
reference_infer.py) -> golden -> SDR (lead vs backing on the true vocal bus, and chained after
htdemucs) -> renders -> install ml-models/karaoke_melroformer/ -> READY.txt."""

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

ID = "karaoke_melroformer"
WORK = C.WORK / "karaoke"
ONNX = WORK / "karaoke_melroformer.onnx"


def torch_pipeline(x: np.ndarray) -> np.ndarray:
    cfg, model = EX.load_model()
    from utils.model_utils import demix

    res = demix(cfg, model, x, torch.device("cpu"), "mel_band_roformer")
    lead = res["Vocals"]
    return np.stack([lead, x - lead])


def main(force=False):
    if not force and C.card_is_current(ID):
        print(f"{ID}: installed card matches, skipping")
        return None
    WORK.mkdir(parents=True, exist_ok=True)
    if force or not ONNX.exists():
        EX.export(ONNX)
    sess = RI.make_session(ONNX)
    report = {"id": ID}

    voc48, sr = C.read_wav(C.TESTMIX / "vocals.wav")
    voc = C.resample(voc48, sr, RI.SR)

    ex = voc[:, 5 * RI.SR : 15 * RI.SR].copy()
    t0 = time.perf_counter()
    ref = torch_pipeline(ex)
    t_torch = time.perf_counter() - t0
    t0 = time.perf_counter()
    got = RI.separate(sess, ex)
    t_ort = time.perf_counter() - t0
    report["parity_10s"] = {s: {"snr_db": C.snr_db(ref[i], got[i]), "max_abs": C.max_abs(ref[i], got[i])}
                            for i, s in enumerate(RI.STEMS)}
    report["time_10s"] = {"torch_cpu_s": t_torch, "ort_cpu_s": t_ort}
    print("parity", report["parity_10s"], report["time_10s"])

    gold = WORK / "golden"
    gx = voc[:, 20 * RI.SR : 25 * RI.SR].copy()
    C.write_f32(gold / "input_5s.f32", gx)
    C.write_f32(gold / "output_5s.f32", RI.separate(sess, gx))
    cx = voc[:, : RI.CHUNK].copy()
    C.write_f32(gold / "chunk0_in.f32", cx)
    C.write_f32(gold / "chunk0_out.f32", RI.model_chunk(sess, cx))
    (gold / "README.txt").write_text(
        "Golden files for karaoke_melroformer (outputs of reference_infer.py on CPU ORT).\n"
        "Raw little-endian float32, PLANAR (C order), 44100 Hz, stereo.\n"
        "input_5s.f32  : [2, 220500]   vocal bus of the synthetic test mix, 0:20-0:25\n"
        "output_5s.f32 : [2, 2, 220500] stems lead_vocal, backing (= input - lead)\n"
        "chunk0_in.f32 : [2, 352800]   one model chunk (test mix vocal bus 0:00-0:08)\n"
        "chunk0_out.f32: [2, 352800]   model_chunk() = STFT -> ONNX -> iSTFT, no demix windowing\n"
        "Tolerance suggestion for Rust: SNR >= 60 dB vs these files.\n",
        encoding="utf-8",
    )

    def evaluate(tag, inp44):
        t0 = time.perf_counter()
        est = RI.separate(sess, inp44)
        dt = time.perf_counter() - t0
        lead48 = C.resample(est[0], RI.SR, sr)[:, : voc48.shape[1]]
        back48 = C.resample(est[1], RI.SR, sr)[:, : voc48.shape[1]]
        tl = C.read_wav(C.TESTMIX / "lead_vocal.wav")[0]
        tb = C.read_wav(C.TESTMIX / "backing_vocals.wav")[0]
        C.write_wav24(C.RENDERS / "karaoke" / f"{tag}_lead_vocal.wav", lead48, sr)
        C.write_wav24(C.RENDERS / "karaoke" / f"{tag}_backing.wav", back48, sr)
        return {"sdr_lead": C.sdr_db(tl, lead48), "sdr_backing": C.sdr_db(tb, back48), "ort_cpu_s": dt}

    report["on_true_vocal_bus_30s"] = evaluate("truevocals", voc)
    C.write_wav24(C.RENDERS / "karaoke" / "truevocals_input.wav", voc48, sr)
    hv = C.RENDERS / "htdemucs" / "vocals.wav"
    if hv.exists():
        hv48 = C.read_wav(hv)[0]
        report["chained_after_htdemucs_30s"] = evaluate("htdemucs_vocals", C.resample(hv48, sr, RI.SR))
    print("sdr", report.get("on_true_vocal_bus_30s"), report.get("chained_after_htdemucs_30s"))

    card = {
        "id": ID,
        "name": "Mel-Band RoFormer Karaoke (aufr33 + viperx), lead vs backing",
        "task": "karaoke",
        "file": "karaoke_melroformer.onnx",
        "sample_rate": RI.SR,
        "channels": 2,
        "stems": RI.STEMS,
        "stems_note": "the graph outputs the lead vocal; backing = input - lead (host)",
        "chunk": {"samples": RI.CHUNK, "num_overlap": RI.NUM_OVERLAP, "step": RI.CHUNK // RI.NUM_OVERLAP,
                  "fade": RI.CHUNK // 10, "border_reflect_pad": RI.CHUNK - RI.CHUNK // RI.NUM_OVERLAP,
                  "window": "same MSS demix scheme as drumsep_mdx23c"},
        "stft": {"n_fft": RI.N_FFT, "hop": RI.HOP, "win_length": RI.N_FFT, "window": "hann_periodic",
                 "center": True, "pad_mode": "reflect", "normalized": False, "bins": 1025, "frames": 801,
                 "istft_length": RI.CHUNK},
        "inputs": [{"name": "spec", "shape": [2, 1025, 801, 2], "dtype": "float32",
                    "layout": "[channel, bin, frame, re/im]"}],
        "outputs": [{"name": "vocals_spec", "shape": [2, 1025, 801, 2], "dtype": "float32",
                     "layout": "masked spectrogram of the lead vocal, DC bin zeroed"}],
        "opset": 17,
        "dml_supported": True,
        "dml_session_config": {"ep.dml.disable_graph_fusion": "1"},
        "dml_note": "ORT DirectML 1.24 graph fusion gives wrong output on this graph; with fusion disabled "
                    "DML matches CPU (~100 dB). Attention is split in 6 batch pieces so the 801-frame chunk "
                    "fits in 8 GB.",
        "source_url": "UVR model repo: " + EX.BASE + EX.CKPT + " ; config: " + EX.CFG_URL,
        "licence": "Model code (lucidrains BS/Mel-RoFormer, ZFTurbo MSS): MIT. Weights (aufr33, viperx): no "
                   "licence stated. Private use build only; ask the authors before any redistribution.",
        "reference": "tools/ml-export/karaoke/reference_infer.py",
        "golden": "golden/README.txt",
        "parity_snr_db": {k: round(v["snr_db"], 2) for k, v in report["parity_10s"].items()},
    }
    C.install(ID, ONNX, card, gold)
    C.mark_ready(ID)
    C.save_json(WORK / "report.json", report)
    return report


if __name__ == "__main__":
    main(force="--force" in sys.argv)
