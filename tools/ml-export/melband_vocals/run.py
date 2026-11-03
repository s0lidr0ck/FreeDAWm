"""Mel-Band RoFormer vocals (Kimberley Jensen, MIT): export (through ../karaoke/export.py with the
KJ checkpoint) -> fp16 conversion -> parity of fp32 and fp16 (MSS demix with the original model vs
reference_infer.py, 10 s of the test mix) -> golden -> SDR on the test mix -> renders -> install
<models>/vocals_melroformer/ -> READY.txt.

fp16 ships when its parity holds (SNR >= FP16_MIN_SNR_DB vs PyTorch for both stems), else fp32.
`--fp32` forces fp32 (e.g. when DirectML disagrees with the fp16 graph: check with bench.py).
The installed model is fp32 (`--fp32`, see FP16_NOTE): fp16 held parity on CPU, not on DirectML.
"""

from __future__ import annotations

import importlib.util
import sys
import time
from pathlib import Path

import numpy as np
import torch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE))

import common as C  # noqa: E402
import reference_infer as RI  # noqa: E402

_spec = importlib.util.spec_from_file_location("karaoke_export", HERE.parent / "karaoke" / "export.py")
EX = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(EX)

ID = "vocals_melroformer"
WORK = C.WORK / ID
ONNX32 = WORK / "vocals_melroformer_fp32.onnx"
ONNX16 = WORK / "vocals_melroformer_fp16.onnx"
FP16_MIN_SNR_DB = 40.0
FP16_NOTE = ("fp16 (onnxruntime.transformers.float16, Casts kept fp32, 460 MB) held parity on CPU (62.8 dB "
             "vocals / 59.5 dB instrumental vs PyTorch on 10 s) but not on DirectML: 16.5 dB vs the CPU golden "
             "on 5 s (one 8 s chunk: 60 dB), fp16 overflow on DML. fp32 ships (`run.py --fp32`).")


def torch_pipeline(x: np.ndarray) -> np.ndarray:
    cfg, model = EX.load_model(EX.KJ_VOCALS)
    cfg.inference.num_overlap = RI.NUM_OVERLAP
    from utils.model_utils import demix

    res = demix(cfg, model, x, torch.device("cpu"), "mel_band_roformer")
    voc = res["vocals"]
    return np.stack([voc, x - voc])


def to_fp16(src: Path, dst: Path):
    import onnx
    from onnxruntime.transformers.float16 import convert_float_to_float16

    # onnxconverter-common's converter leaves the graph's own Casts mistyped (load fails); ORT's
    # converter with the Casts blocked (kept fp32) gives a valid graph.
    m = onnx.load(str(src))
    m16 = convert_float_to_float16(m, keep_io_types=True, op_block_list=["Cast"])
    onnx.save(m16, str(dst))
    print("wrote", dst, dst.stat().st_size / 1e6, "MB")


def main(force=False, fp32=False):
    if not force and C.card_is_current(ID):
        print(f"{ID}: installed card matches, skipping")
        return None
    WORK.mkdir(parents=True, exist_ok=True)
    if force or not ONNX32.exists():
        EX.export(ONNX32, variant=EX.KJ_VOCALS)
    if not fp32 and (force or not ONNX16.exists()):
        to_fp16(ONNX32, ONNX16)
    report = {"id": ID}

    mix48, sr = C.read_wav(C.TESTMIX / "mix.wav")
    mix = C.resample(mix48, sr, RI.SR)
    ex = mix[:, 5 * RI.SR : 15 * RI.SR].copy()
    t0 = time.perf_counter()
    ref = torch_pipeline(ex)
    report["time_10s"] = {"torch_cpu_s": time.perf_counter() - t0}
    report["parity_10s"] = {}
    for tag, path in (("fp32", ONNX32), ("fp16", ONNX16)):
        if not path.exists() or (tag == "fp16" and fp32):
            continue
        sess = RI.make_session(path)
        t0 = time.perf_counter()
        got = RI.separate(sess, ex)
        report["time_10s"][f"ort_cpu_{tag}_s"] = time.perf_counter() - t0
        report["parity_10s"][tag] = {s: {"snr_db": C.snr_db(ref[i], got[i]), "max_abs": C.max_abs(ref[i], got[i])}
                                     for i, s in enumerate(RI.STEMS)}
        print("parity", tag, report["parity_10s"][tag], flush=True)
    p16 = report["parity_10s"].get("fp16")
    use16 = p16 is not None and all(v["snr_db"] >= FP16_MIN_SNR_DB for v in p16.values())
    onnx = ONNX16 if use16 else ONNX32
    report["shipped"] = "fp16" if use16 else "fp32"
    print("shipping", report["shipped"])
    sess = RI.make_session(onnx)

    gold = WORK / "golden"
    gx = mix[:, 20 * RI.SR : 25 * RI.SR].copy()
    C.write_f32(gold / "input_5s.f32", gx)
    C.write_f32(gold / "output_5s.f32", RI.separate(sess, gx))
    cx = mix[:, : RI.CHUNK].copy()
    C.write_f32(gold / "chunk0_in.f32", cx)
    C.write_f32(gold / "chunk0_out.f32", RI.model_chunk(sess, cx))
    (gold / "README.txt").write_text(
        "Golden files for vocals_melroformer (outputs of reference_infer.py on CPU ORT).\n"
        "Raw little-endian float32, PLANAR (C order), 44100 Hz, stereo.\n"
        "input_5s.f32  : [2, 220500]   synthetic test mix, 0:20-0:25\n"
        "output_5s.f32 : [2, 2, 220500] stems vocals, instrumental (= input - vocals), num_overlap 2\n"
        "chunk0_in.f32 : [2, 352800]   one model chunk (test mix 0:00-0:08)\n"
        "chunk0_out.f32: [2, 352800]   model_chunk() = STFT -> ONNX -> iSTFT, no demix windowing\n"
        "Tolerance suggestion for Rust: SNR >= 60 dB vs these files.\n",
        encoding="utf-8",
    )

    t0 = time.perf_counter()
    est = RI.separate(sess, mix)
    dt = time.perf_counter() - t0
    v48 = C.resample(est[0], RI.SR, sr)[:, : mix48.shape[1]]
    i48 = C.resample(est[1], RI.SR, sr)[:, : mix48.shape[1]]
    tv = C.read_wav(C.TESTMIX / "vocals.wav")[0]
    ti = mix48 - tv
    report["testmix_30s"] = {"sdr_vocals": C.sdr_db(tv, v48), "sdr_instrumental": C.sdr_db(ti, i48), "ort_cpu_s": dt}
    C.write_wav24(C.RENDERS / ID / "vocals.wav", v48, sr)
    C.write_wav24(C.RENDERS / ID / "instrumental.wav", i48, sr)
    print("sdr", report["testmix_30s"])

    card = {
        "id": ID,
        "name": "Mel-Band RoFormer vocals (Kimberley Jensen)",
        "task": "vocals",
        "file": onnx.name,
        "precision": report["shipped"],
        "precision_note": FP16_NOTE,
        "sample_rate": RI.SR,
        "channels": 2,
        "stems": RI.STEMS,
        "stems_note": "the graph outputs the vocals; instrumental = input - vocals (host)",
        "chunk": {"samples": RI.CHUNK, "num_overlap": RI.NUM_OVERLAP, "step": RI.CHUNK // RI.NUM_OVERLAP,
                  "fade": RI.CHUNK // 10, "border_reflect_pad": RI.CHUNK - RI.CHUNK // RI.NUM_OVERLAP,
                  "window": "same MSS demix scheme as karaoke_melroformer"},
        "stft": {"n_fft": RI.N_FFT, "hop": RI.HOP, "win_length": RI.N_FFT, "window": "hann_periodic",
                 "center": True, "pad_mode": "reflect", "normalized": False, "bins": 1025, "frames": 801,
                 "istft_length": RI.CHUNK},
        "inputs": [{"name": "spec", "shape": [2, 1025, 801, 2], "dtype": "float32",
                    "layout": "[channel, bin, frame, re/im]"}],
        "outputs": [{"name": "vocals_spec", "shape": [2, 1025, 801, 2], "dtype": "float32",
                     "layout": "masked spectrogram of the vocals, DC bin zeroed"}],
        "opset": 17,
        "dml_supported": True,
        "dml_session_config": {"ep.dml.disable_graph_fusion": "1"},
        "dml_note": "Same graph as karaoke_melroformer (other weights): run with graph fusion disabled.",
        "source_url": "https://huggingface.co/KimberleyJSN/melbandroformer (MelBandRoformer.ckpt); config: "
                      + EX.KJ_VOCALS.cfg_url,
        "licence": "MIT (weights: Kimberley Jensen; model code: lucidrains Mel-RoFormer, ZFTurbo MSS).",
        "quality": "Multisong SDR 11.01 vocals / 17.32 instrumental (MVSEP).",
        "reference": "tools/ml-export/melband_vocals/reference_infer.py",
        "golden": "golden/README.txt",
        "parity_snr_db": {k: round(v["snr_db"], 2) for k, v in report["parity_10s"][report["shipped"]].items()},
    }
    C.install(ID, onnx, card, gold)
    C.mark_ready(ID)
    C.save_json(WORK / "report.json", report)
    return report


if __name__ == "__main__":
    main(force="--force" in sys.argv, fp32="--fp32" in sys.argv)
