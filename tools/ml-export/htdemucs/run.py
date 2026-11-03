"""HTDemucs: export -> parity (PyTorch vs reference_infer.py) -> golden files -> SDR on the test mix
-> 24-bit renders -> install into ml-models/htdemucs/ (card.json, golden/) -> READY.txt."""

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

ID = "htdemucs"
WORK = C.WORK / ID
ONNX = WORK / "htdemucs.onnx"


def torch_pipeline(x: np.ndarray) -> np.ndarray:
    """The original: demucs.api.Separator semantics with shifts=0, overlap=0.25, split=True."""
    from demucs.apply import apply_model

    bag, _ = EX.load_model()
    wav = torch.from_numpy(x)
    ref = wav.mean(0)
    mu, sd = ref.mean(), ref.std() + 1e-8
    with torch.no_grad():
        out = apply_model(bag, ((wav - mu) / sd)[None], shifts=0, split=True, overlap=0.25)[0]
    return (out * sd + mu).numpy()


def main(force=False):
    if not force and C.card_is_current(ID):
        print(f"{ID}: installed card matches, skipping")
        return None
    WORK.mkdir(parents=True, exist_ok=True)
    if force or not ONNX.exists():
        EX.export(ONNX)
    sess = RI.make_session(ONNX)
    report = {"id": ID}

    # ---- test material (48 kHz synthetic mix) -> 44.1 kHz
    mix48, sr = C.read_wav(C.TESTMIX / "mix.wav")
    mix = C.resample(mix48, sr, RI.SR)

    # ---- parity: 10 s excerpt (2 chunks, exercises the overlap-add path)
    ex = mix[:, 5 * RI.SR : 15 * RI.SR].copy()
    t0 = time.perf_counter()
    ref = torch_pipeline(ex)
    t_torch = time.perf_counter() - t0
    t0 = time.perf_counter()
    got = RI.separate(sess, ex)
    t_ort = time.perf_counter() - t0
    report["parity_10s"] = {
        s: {"snr_db": C.snr_db(ref[i], got[i]), "max_abs": C.max_abs(ref[i], got[i])}
        for i, s in enumerate(RI.SOURCES)
    }
    report["time_10s"] = {"torch_cpu_s": t_torch, "ort_cpu_s": t_ort}
    print("parity", report["parity_10s"])

    # ---- golden files (planar f32 LE): 5 s (single padded chunk) and 10 s (overlap path)
    gold = WORK / "golden"
    for name, a, b in (("5s", 20.0, 25.0), ("10s", 5.0, 15.0)):
        gx = mix[:, int(a * RI.SR) : int(b * RI.SR)].copy()
        gy = got if name == "10s" else RI.separate(sess, gx)
        C.write_f32(gold / f"input_{name}.f32", gx)
        C.write_f32(gold / f"output_{name}.f32", gy)
    (gold / "README.txt").write_text(
        "Golden files for htdemucs (outputs of reference_infer.py on CPU ORT).\n"
        "Raw little-endian float32, PLANAR (C order), 44100 Hz.\n"
        "input_5s.f32  : [2, 220500]  (stereo excerpt 0:20-0:25 of the synthetic test mix)\n"
        "output_5s.f32 : [4, 2, 220500] stems drums, bass, other, vocals\n"
        "input_10s.f32 : [2, 441000]  (0:05-0:15; 2 chunks, exercises the overlap-add)\n"
        "output_10s.f32: [4, 2, 441000]\n"
        "Tolerance suggestion for Rust: SNR >= 60 dB per stem vs these files.\n",
        encoding="utf-8",
    )

    # ---- quality: full 30 s mix, SDR per stem vs the true stems
    t0 = time.perf_counter()
    est = RI.separate(sess, mix)
    t_full = time.perf_counter() - t0
    report["time_30s_ort_cpu_s"] = t_full
    report["sdr_30s"] = {}
    for i, s in enumerate(RI.SOURCES):
        true48, _ = C.read_wav(C.TESTMIX / f"{s}.wav")
        est48 = C.resample(est[i], RI.SR, sr)[:, : true48.shape[1]]
        report["sdr_30s"][s] = C.sdr_db(true48, est48)
        C.write_wav24(C.RENDERS / ID / f"{s}.wav", est48, sr)
        C.write_wav24(C.RENDERS / ID / f"true_{s}.wav", true48, sr)
    C.write_wav24(C.RENDERS / ID / "mix.wav", mix48, sr)
    print("sdr", report["sdr_30s"], "time", t_full)

    card = {
        "id": ID,
        "name": "HTDemucs v4 (4 stems)",
        "task": "separate",
        "file": "htdemucs.onnx",
        "sample_rate": RI.SR,
        "channels": 2,
        "stems": RI.SOURCES,
        "chunk": {
            "samples": RI.SEG,
            "overlap": RI.OVERLAP,
            "stride": int((1 - RI.OVERLAP) * RI.SEG),
            "window": "triangular (1..SEG/2, SEG-SEG/2..1)/max, overlap-add normalised by weight sum",
            "padding": "chunk grown to SEG centred, real neighbouring samples else zeros; output centre-trimmed",
            "normalisation": "x = (x - mean(mono)) / (std(mono, ddof=1) + 1e-8) before; inverse after",
        },
        "stft": None,
        "stft_note": "STFT/iSTFT are inside the graph (n_fft 4096, hop 1024, Hann, normalized)",
        "inputs": [{"name": "mix", "shape": [1, 2, RI.SEG], "dtype": "float32"}],
        "outputs": [{"name": "stems", "shape": [1, 4, 2, RI.SEG], "dtype": "float32",
                     "layout": "[batch, stem(drums,bass,other,vocals), channel, sample]"}],
        "opset": 17,
        "dml_supported": True,
        "dml_session_config": {"ep.dml.disable_graph_fusion": "1"},
        "dml_note": "ORT DirectML 1.24 graph fusion corrupts this graph (output garbage, SNR < 0 dB vs CPU); "
                    "with graph fusion disabled DML matches CPU (~78 dB) and runs ~0.35 s per 7.8 s chunk.",
        "source_url": "https://github.com/facebookresearch/demucs (weights: https://huggingface.co/adefossez/HTDemucs, "
                      "orig. https://dl.fbaipublicfiles.com/demucs/hybrid_transformer/955717e8-8726e21a.th)",
        "licence": "Code: MIT (Meta Platforms). Weights: released with the MIT-licensed repo, no separate "
                   "weights licence stated; one third-party ONNX card claims CC BY-NC 4.0 (unverified). "
                   "Private use build.",
        "reference": "tools/ml-export/htdemucs/reference_infer.py",
        "golden": "golden/README.txt",
        "parity_snr_db": {k: round(v["snr_db"], 2) for k, v in report["parity_10s"].items()},
    }
    C.install(ID, ONNX, card, gold)
    C.mark_ready(ID)
    C.save_json(WORK / "report.json", report)
    return report


if __name__ == "__main__":
    main(force="--force" in sys.argv)
