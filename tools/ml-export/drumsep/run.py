"""MDX23C DrumSep: export -> parity (MSS demix in PyTorch vs reference_infer.py) -> golden ->
proxy SDR on the summed drum mics -> renders -> install ml-models/drumsep_mdx23c/ -> READY.txt."""

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

ID = "drumsep_mdx23c"
WORK = C.WORK / "drumsep"
ONNX = WORK / "drumsep_mdx23c.onnx"


def torch_pipeline(x: np.ndarray) -> np.ndarray:
    """The original: MSS utils.model_utils.demix (generic mode) with the original PyTorch model."""
    cfg, model = EX.load_model()
    from utils.model_utils import demix

    res = demix(cfg, model, x, torch.device("cpu"), "mdx23c")
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

    drums48, sr = C.read_wav(C.TESTMIX / "drums.wav")
    drums = C.resample(drums48, sr, RI.SR)

    ex = drums[:, 5 * RI.SR : 15 * RI.SR].copy()
    t0 = time.perf_counter()
    ref = torch_pipeline(ex)
    t_torch = time.perf_counter() - t0
    t0 = time.perf_counter()
    got = RI.separate(sess, ex)
    t_ort = time.perf_counter() - t0
    report["parity_10s"] = {s: {"snr_db": C.snr_db(ref[i], got[i]), "max_abs": C.max_abs(ref[i], got[i])}
                            for i, s in enumerate(RI.STEMS)}
    report["time_10s"] = {"torch_cpu_s": t_torch, "ort_cpu_s": t_ort}
    print("parity", report["parity_10s"])

    gold = WORK / "golden"
    gx = drums[:, 20 * RI.SR : 25 * RI.SR].copy()
    gy = RI.separate(sess, gx)
    C.write_f32(gold / "input_5s.f32", gx)
    C.write_f32(gold / "output_5s.f32", gy)
    (gold / "README.txt").write_text(
        "Golden files for drumsep_mdx23c (outputs of reference_infer.py on CPU ORT).\n"
        "Raw little-endian float32, PLANAR (C order), 44100 Hz.\n"
        "input_5s.f32 : [2, 220500] (summed drum mics of the synthetic test mix, 0:20-0:25;\n"
        "               > 2*BORDER so the reflect border padding and 7 chunks are exercised)\n"
        "output_5s.f32: [6, 2, 220500] stems kick, snare, toms, hh, ride, crash\n"
        "Tolerance suggestion for Rust: SNR >= 60 dB per stem vs these files.\n",
        encoding="utf-8",
    )

    t0 = time.perf_counter()
    est = RI.separate(sess, drums)
    report["time_30s_ort_cpu_s"] = time.perf_counter() - t0
    est48 = {s: C.resample(est[i], RI.SR, sr)[:, : drums48.shape[1]] for i, s in enumerate(RI.STEMS)}
    truth = {k: C.read_wav(C.TESTMIX / f"drums_{k}.wav")[0] for k in ("kick", "snare", "toms", "overheads")}
    cymbals = est48["hh"] + est48["ride"] + est48["crash"]
    report["proxy_sdr_30s"] = {
        "kick_vs_KickIn": C.sdr_db(truth["kick"], est48["kick"]),
        "snare_vs_SnareTop": C.sdr_db(truth["snare"], est48["snare"]),
        "toms_vs_TomH+L": C.sdr_db(truth["toms"], est48["toms"]),
        "hh+ride+crash_vs_OH": C.sdr_db(truth["overheads"], cymbals),
        "note": "mic groups carry bleed (OH hear the whole kit), so these are proxies only",
    }
    report["energy_share_db"] = {s: float(10 * np.log10(np.sum(est48[s] ** 2) / np.sum(drums48**2) + 1e-12))
                                 for s in RI.STEMS}
    for s in RI.STEMS:
        C.write_wav24(C.RENDERS / "drumsep" / f"{s}.wav", est48[s], sr)
    C.write_wav24(C.RENDERS / "drumsep" / "input_drums.wav", drums48, sr)
    print("proxy sdr", report["proxy_sdr_30s"], "time", report["time_30s_ort_cpu_s"])

    card = {
        "id": ID,
        "name": "MDX23C DrumSep 6-stem (aufr33 + jarredou)",
        "task": "drum_split",
        "file": "drumsep_mdx23c.onnx",
        "sample_rate": RI.SR,
        "channels": 2,
        "stems": RI.STEMS,
        "chunk": {
            "samples": RI.CHUNK,
            "num_overlap": RI.NUM_OVERLAP,
            "step": RI.CHUNK // RI.NUM_OVERLAP,
            "fade": RI.CHUNK // 10,
            "border_reflect_pad": RI.CHUNK - RI.CHUNK // RI.NUM_OVERLAP,
            "window": "linear fade-in/out of `fade` samples (none at the first chunk start / last chunk end), "
                      "overlap-add normalised by window sum",
        },
        "stft": None,
        "stft_note": "STFT/iSTFT inside the graph (n_fft 2048, hop 512, Hann, center, not normalised, 1024 bins)",
        "inputs": [{"name": "mix", "shape": [1, 2, RI.CHUNK], "dtype": "float32"}],
        "outputs": [{"name": "stems", "shape": [1, 6, 2, RI.CHUNK], "dtype": "float32",
                     "layout": "[batch, stem(kick,snare,toms,hh,ride,crash), channel, sample]"}],
        "opset": 17,
        "dml_supported": True,
        "dml_note": "DirectML default settings match CPU (~125 dB vs golden).",
        "source_url": "https://github.com/jarredou/models/releases (aufr33-jarredou MDX23C DrumSep v0.1); "
                      "file mirrored at " + EX.CKPT_URL,
        "licence": "Training code (ZFTurbo MSS): MIT. Weights: no licence stated by the authors "
                   "(aufr33, jarredou). Private use build only; ask the authors before any redistribution.",
        "reference": "tools/ml-export/drumsep/reference_infer.py",
        "golden": "golden/README.txt",
        "parity_snr_db": {k: round(v["snr_db"], 2) for k, v in report["parity_10s"].items()},
    }
    C.install(ID, ONNX, card, gold)
    C.mark_ready(ID)
    C.save_json(WORK / "report.json", report)
    return report


if __name__ == "__main__":
    main(force="--force" in sys.argv)
