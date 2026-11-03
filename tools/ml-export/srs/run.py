"""Smule Renaissance Small: export -> parity (original main.py pipeline vs reference_infer.py) ->
golden -> renders on lead.wav excerpts -> install ml-models/srs_small/ -> READY.txt."""

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

ID = "srs_small"
WORK = C.WORK / "srs"
ONNX = WORK / "srs_small.onnx"
EXCERPTS = [("lead_0700-0730", 420.0, 30.0), ("lead_0200-0230", 120.0, 30.0)]


def torch_pipeline(x: np.ndarray, fp32_norm: bool, hp_dtype=torch.float32, hp=None) -> np.ndarray:
    """The original main.py: highpass_biquad(60) -> peak-normalise -> STFT -> model -> iSTFT -> * g.
    `hp_dtype` runs the high-pass in another precision; `hp` replaces it by a given signal."""
    import torchaudio

    EX.fetch()
    from spectral_ops import STFT, iSTFT  # original HF repo code

    model = EX.load_model(fp32_norm=fp32_norm)
    if hp is None:
        wav = torchaudio.functional.highpass_biquad(torch.from_numpy(x).to(hp_dtype)[None], 48000,
                                                    cutoff_freq=60.0).float()
    else:
        wav = torch.from_numpy(hp)[None]
    g = torch.max(torch.abs(wav))
    with torch.no_grad():
        y = iSTFT(4096, 2048, 4096)(model(STFT(4096, 2048, 4096)(wav / g)))
    return (y * g)[0].numpy()  # length 2048 * (T - 1)


def main(force=False):
    if not force and C.card_is_current(ID):
        print(f"{ID}: installed card matches, skipping")
        return None
    WORK.mkdir(parents=True, exist_ok=True)
    if force or not ONNX.exists():
        EX.export(ONNX)
    sess = RI.make_session(ONNX)
    report = {"id": ID}

    dan, sr = C.read_wav(C.MEDIA / "lead.wav", 425.0, 10.0)
    x = dan[0]
    t0 = time.perf_counter()
    ref16 = torch_pipeline(x, fp32_norm=False)
    t_torch = time.perf_counter() - t0
    ref32 = torch_pipeline(x, fp32_norm=True)
    ref32_same_hp = torch_pipeline(x, fp32_norm=True, hp=RI.highpass60(x))
    ref16_hp64 = torch_pipeline(x, fp32_norm=False, hp_dtype=torch.float64)
    t0 = time.perf_counter()
    got = RI.restore(sess, x)
    t_ort = time.perf_counter() - t0
    n = len(ref16)
    report["parity_10s"] = {
        "vs_original_fp16_rmsnorm": {"snr_db": C.snr_db(ref16, got[:n]), "max_abs": C.max_abs(ref16, got[:n])},
        "vs_torch_fp32_rmsnorm": {"snr_db": C.snr_db(ref32, got[:n]), "max_abs": C.max_abs(ref32, got[:n])},
        "vs_torch_fp32_rmsnorm_same_highpass": {"snr_db": C.snr_db(ref32_same_hp, got[:n]),
                                                "max_abs": C.max_abs(ref32_same_hp, got[:n])},
        "original_fp16_vs_fp32_rmsnorm_torch": {"snr_db": C.snr_db(ref16, ref32)},
        "original_highpass_f32_vs_f64_torch": {"snr_db": C.snr_db(ref16, ref16_hp64)},
        "compared_samples": n,
    }
    report["time_10s"] = {"torch_cpu_s": t_torch, "ort_cpu_s": t_ort}
    print("parity", report["parity_10s"])

    gold = WORK / "golden"
    for name, a, d in (("5s", 430.0, 5.0), ("12s", 400.0, 12.0)):
        gx = C.read_wav(C.MEDIA / "lead.wav", a, d)[0][0]
        C.write_f32(gold / f"input_{name}.f32", gx)
        C.write_f32(gold / f"output_{name}.f32", RI.restore(sess, gx))
        if name == "5s":
            hp = RI.highpass60(gx)
            g = np.max(np.abs(hp))
            spec = RI.stft(hp / g)
            C.write_f32(gold / "spec_in_5s.f32", spec)
            C.write_f32(gold / "spec_out_5s.f32", RI.run_network(sess, spec))
            C.write_f32(gold / "highpass_5s.f32", hp)
    (gold / "README.txt").write_text(
        "Golden files for srs_small (outputs of reference_infer.py on CPU ORT).\n"
        "Raw little-endian float32, C order. Mono, 48000 Hz. Inputs are raw lead.wav excerpts.\n"
        "input_5s.f32     : [240000]  (lead.wav 7:10-7:15)\n"
        "highpass_5s.f32  : [240000]  after step 1 (60 Hz high-pass + clamp)\n"
        "spec_in_5s.f32   : [1, 2049, 118, 2] step 3 STFT of the peak-normalised signal (re, im)\n"
        "spec_out_5s.f32  : [1, 2049, 118, 2] network output\n"
        "output_5s.f32    : [240000]  final output (step 6)\n"
        "input_12s.f32    : [576000]  (6:40-6:52; 282 frames = 2 network chunks)\n"
        "output_12s.f32   : [576000]\n"
        "Tolerance suggestion for Rust: SNR >= 60 dB vs these files.\n",
        encoding="utf-8",
    )

    report["renders"] = {}
    for name, a, d in EXCERPTS:
        xin = C.read_wav(C.MEDIA / "lead.wav", a, d)[0][0]
        t0 = time.perf_counter()
        y = RI.restore(sess, xin)
        report["renders"][name] = {"ort_cpu_s": time.perf_counter() - t0,
                                   "in_rms_db": float(20 * np.log10(np.sqrt(np.mean(xin**2)) + 1e-12)),
                                   "out_rms_db": float(20 * np.log10(np.sqrt(np.mean(y**2)) + 1e-12))}
        C.write_wav24(C.RENDERS / "srs" / f"{name}_in.wav", xin[None], sr)
        C.write_wav24(C.RENDERS / "srs" / f"{name}_srs.wav", y[None], sr)
    print("renders", report["renders"])

    card = {
        "id": ID,
        "name": "Smule Renaissance Small (vocal restoration: de-reverb, de-noise, de-clip)",
        "task": "vocal_clean",
        "file": "srs_small.onnx",
        "sample_rate": RI.SR,
        "channels": 1,
        "stems": ["restored"],
        "chunk": {"unit": "stft_frames", "core": RI.CORE, "context": RI.CTX,
                  "note": "exact: context = receptive field (+-129 frames); any core size gives the same output"},
        "preprocess": {"downmix": "mean of channels", "highpass": {"type": "RBJ biquad", "fc_hz": 60.0, "q": 0.707,
                                                                   "clamp": [-1.0, 1.0]},
                       "normalise": "divide by peak |x| after the high-pass, multiply back after iSTFT"},
        "stft": {"n_fft": RI.N_FFT, "hop": RI.HOP, "win_length": RI.N_FFT, "window": "hann_periodic",
                 "center": True, "pad_mode": "reflect", "normalized": False, "bins": 2049,
                 "istft_length": "= input length"},
        "inputs": [{"name": "spec", "shape": [1, 2049, "frames", 2], "dtype": "float32", "layout": "[1, bin, frame, re/im]"}],
        "outputs": [{"name": "spec_out", "shape": [1, 2049, "frames", 2], "dtype": "float32"}],
        "opset": 17,
        "dml_supported": True,
        "dml_note": "DirectML default settings match CPU (~121 dB vs golden); dynamic frame count is fine.",
        "source_url": f"https://huggingface.co/{EX.REPO} (revision {EX.REV}); paper arXiv:2510.21659",
        "licence": "MIT (code and weights), Copyright (c) 2025 smulelabs. Full text: LICENSE in the HF repo.",
        "export_note": "RMSNorm statistic computed in float32 (the original uses float16 even on CPU)",
        "reference": "tools/ml-export/srs/reference_infer.py",
        "golden": "golden/README.txt",
        "parity_snr_db": {k: round(v["snr_db"], 2) for k, v in report["parity_10s"].items() if isinstance(v, dict)},
    }
    lic = EX.DL / "LICENSE"
    if lic.exists():
        card["licence_text"] = lic.read_text(encoding="utf-8")
    C.install(ID, ONNX, card, gold)
    C.mark_ready(ID)
    C.save_json(WORK / "report.json", report)
    return report


if __name__ == "__main__":
    main(force="--force" in sys.argv)
