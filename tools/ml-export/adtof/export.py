"""Export ADTOF Frame_RNN (xavriley/ADTOF-pytorch port of Zehren et al.'s ADTOF) to ONNX with a
dynamic frame axis. The forward is the original's, with the time-flattening reshape written
with -1 so the frame count is not baked into the graph. Input "spec" [1, T, 84, 1] ->
output "act" [1, T, 5]."""

from __future__ import annotations

import sys
from pathlib import Path

import requests
import torch
import torch.nn as nn

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import common as C  # noqa: E402

COMMIT = "85c192e78f716ea0b111cc8a5ee4a8f6a3a4f8a9"
FILES = ["__init__.py", "audio.py", "model.py", "post_processing.py", "data/adtof_frame_rnn_pytorch_weights.pth"]
DL = C.WORK / "dl" / "adtof"


def fetch():
    for f in FILES:
        dst = DL / "adtof_pytorch" / f
        if not dst.exists():
            dst.parent.mkdir(parents=True, exist_ok=True)
            r = requests.get(f"https://raw.githubusercontent.com/xavriley/ADTOF-pytorch/{COMMIT}/src/adtof_pytorch/{f}",
                             timeout=60)
            r.raise_for_status()
            dst.write_bytes(r.content)
    if str(DL) not in sys.path:
        sys.path.insert(0, str(DL))


def load_model():
    fetch()
    from adtof_pytorch import calculate_n_bins, create_frame_rnn_model, get_default_weights_path, load_pytorch_weights

    m = create_frame_rnn_model(calculate_n_bins())
    m = load_pytorch_weights(m, get_default_weights_path(), strict=False)
    return m.eval()


class Wrap(nn.Module):
    def __init__(self, m):
        super().__init__()
        self.m = m

    def forward(self, spec):
        m = self.m
        x = spec.permute(0, 3, 1, 2)
        for blk in m.cnn_blocks:
            x = blk(x)
        x = x.permute(0, 2, 3, 1)
        x = x.reshape(1, -1, x.shape[2] * x.shape[3])
        assert m.context_layer is None
        for gru in m.gru_layers:
            x, _ = gru(x)
        return torch.sigmoid(m.output_layer(x))


def export(out_path: Path):
    m = load_model()
    w = Wrap(m).eval()
    x = torch.randn(1, 300, m.n_bins, 1)
    with torch.no_grad():
        assert torch.allclose(m(x), w(x))
    out_path.parent.mkdir(parents=True, exist_ok=True)
    torch.onnx.export(w, (x,), str(out_path), input_names=["spec"], output_names=["act"],
                      dynamic_axes={"spec": {1: "frames"}, "act": {1: "frames"}},
                      opset_version=17, do_constant_folding=True, dynamo=False)
    import onnxruntime as ort

    s = ort.InferenceSession(str(out_path), providers=["CPUExecutionProvider"])
    for T in (37, 300, 4001):
        x = torch.rand(1, T, m.n_bins, 1)
        with torch.no_grad():
            ref = m(x).numpy()
        got = s.run(None, {"spec": x.numpy()})[0]
        print(f"T={T}: snr {C.snr_db(ref, got):.1f} dB")
        assert C.snr_db(ref, got) > 60
    print("wrote", out_path, out_path.stat().st_size / 1e6, "MB")


if __name__ == "__main__":
    export(C.WORK / "adtof" / "adtof_drums.onnx")
