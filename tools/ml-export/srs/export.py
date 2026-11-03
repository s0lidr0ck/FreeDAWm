"""Export Smule Renaissance Small (SRS, vocal restoration: de-reverb / de-noise / de-clip, 48 kHz)
to ONNX: the network only, with a dynamic number of STFT frames; the STFT stays on the host.

Why host STFT here: SRS was made to run on a whole file at once. Its only cross-frame paths are
the depthwise time convolutions (receptive field exactly +-129 frames), so the host can chunk in
the STFT-frame domain with 129 frames of context and reproduce whole-file output exactly. That
needs a dynamic frame count, and frame-domain chunking is easiest with the STFT on the host.

Only change to the network: RMSNorm computes its statistic in float32. The original casts to
float16 inside RMSNorm (`input.half()`), even on CPU. Keeping fp16 there is not robust (x^2
overflows fp16 above 256) and the task asks for an fp32 export, so parity is reported both against
the unmodified original and against the fp32-RMSNorm PyTorch model.

Input `spec` f32 [1, 2049, T, 2] (re, im of torch.stft(n_fft=4096, hop=2048, Hann, center,
reflect, not normalised) of the peak-normalised mono signal); output `spec_out` same shape.
"""

from __future__ import annotations

import sys
import types
from pathlib import Path

import torch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import common as C  # noqa: E402

REPO = "smulelabs/Smule-Renaissance-Small"
REV = "12024e3250ace99e566662a2963e6158d104e69c"
DL = C.WORK / "dl" / "srs"


def fetch() -> Path:
    from huggingface_hub import snapshot_download

    snapshot_download(REPO, revision=REV, local_dir=str(DL))
    if str(DL) not in sys.path:
        sys.path.insert(0, str(DL))
    return DL


def load_model(fp32_norm: bool):
    fetch()
    import model as srs_model  # the original model.py from the HF repo

    m = srs_model.Renaissance()
    m.load_state_dict(torch.load(DL / "smule-renaissance-small.pt", map_location="cpu"))
    m.eval()
    if fp32_norm:
        def fwd(self, x):
            var = x.pow(2).mean(dim=1, keepdim=True)
            return x * torch.rsqrt(var + self.eps) * self.weight.unsqueeze(0).unsqueeze(-1)

        for mod in m.modules():
            if isinstance(mod, srs_model.RMSNorm):
                mod.forward = types.MethodType(fwd, mod)
    return m


def export(out_path: Path):
    m = load_model(fp32_norm=True)
    x = torch.randn(1, 2049, 120, 2) * 0.05
    out_path.parent.mkdir(parents=True, exist_ok=True)
    torch.onnx.export(m, (x,), str(out_path), input_names=["spec"], output_names=["spec_out"],
                      dynamic_axes={"spec": {2: "frames"}, "spec_out": {2: "frames"}},
                      opset_version=17, do_constant_folding=True, dynamo=False)
    print("wrote", out_path, out_path.stat().st_size / 1e6, "MB")
    # dynamic-length check against torch
    import onnxruntime as ort

    sess = ort.InferenceSession(str(out_path), providers=["CPUExecutionProvider"])
    for T in (7, 120, 333):
        x = torch.randn(1, 2049, T, 2) * 0.05
        with torch.no_grad():
            ref = m(x).numpy()
        got = sess.run(None, {"spec": x.numpy()})[0]
        print(f"T={T}: snr {C.snr_db(ref, got):.1f} dB")
        assert C.snr_db(ref, got) > 60


if __name__ == "__main__":
    export(C.WORK / "srs" / "srs_small.onnx")
