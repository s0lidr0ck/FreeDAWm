"""Export MDX23C DrumSep 6-stem (aufr33 + jarredou) to a self-contained ONNX graph.

The network (ZFTurbo TFC_TDF_net v3) is unchanged; its complex STFT helper is replaced by the
real-valued STFT/iSTFT of stft_onnx.py (n_fft 2048, hop 512, periodic Hann, center=True with
reflect padding, not normalised, 1024 of 1025 bins kept; the iSTFT zero-fills the Nyquist bin).

Fixed shapes: input `mix` [1, 2, 130560] (2.96 s at 44.1 kHz), output `stems`
[1, 6, 2, 130560] in order kick, snare, toms, hh, ride, crash.
"""

from __future__ import annotations

import sys
from pathlib import Path

import torch
import torch.nn.functional as F
import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import common as C  # noqa: E402
import mss_fetch  # noqa: E402
import stft_onnx as S  # noqa: E402

CKPT_URL = ("https://github.com/nomadkaraoke/python-audio-separator/releases/download/"
            "model-configs/MDX23C-DrumSep-aufr33-jarredou.ckpt")
CFG_URL = ("https://github.com/nomadkaraoke/python-audio-separator/releases/download/"
           "model-configs/config_drumsep_mdx23c.yaml")
DL = C.WORK / "dl" / "w"


def load_config():
    from ml_collections import ConfigDict

    p = C.download(CFG_URL, DL / "config_drumsep_mdx23c.yaml")
    return ConfigDict(yaml.load(open(p), Loader=yaml.FullLoader))


def load_model():
    mss_fetch.fetch()
    from models.mdx23c_tfc_tdf_v3 import TFC_TDF_net

    cfg = load_config()
    model = TFC_TDF_net(cfg)
    ck = C.download(CKPT_URL, DL / "MDX23C-DrumSep-aufr33-jarredou.ckpt")
    model.load_state_dict(torch.load(ck, map_location="cpu", weights_only=False))
    model.eval()
    return cfg, model


class RealSTFT:
    """Drop-in for models.mdx23c_tfc_tdf_v3.STFT without complex ops."""

    def __init__(self, audio):
        self.n_fft = audio.n_fft
        self.hop = audio.hop_length
        self.dim_f = audio.dim_f

    def __call__(self, x):  # [B, c, L] -> [B, c*2, F, T]
        B, c, L = x.shape
        x = F.pad(x.reshape(B * c, 1, L), (self.n_fft // 2, self.n_fft // 2), mode="reflect")
        re, im = S.stft(x.reshape(B * c, -1), self.n_fft, self.hop, self.dim_f, normalized=False)
        z = torch.stack([re, im], 1)  # [Bc, 2, T, F]
        T = z.shape[2]
        return z.permute(0, 1, 3, 2).reshape(B, c * 2, self.dim_f, T)

    def inverse(self, x):  # [B, S, c*2, F, T] -> [B, S, c, L]
        B, Sn, c2, Fb, T = x.shape
        z = x.reshape(B * Sn * (c2 // 2), 2, Fb, T).permute(0, 1, 3, 2)  # [R, 2, T, F]
        y, env = S.istft(z[:, 0], z[:, 1], self.n_fft, self.hop, normalized=False)
        L = self.hop * (T - 1)
        a = self.n_fft // 2
        y = y[:, a : a + L] / env[:, a : a + L]
        return y.reshape(B, Sn, c2 // 2, L)


def export(out_path: Path):
    cfg, model = load_model()
    L = cfg.audio.chunk_size
    x = torch.randn(1, 2, L) * 0.1
    with torch.no_grad():
        ref = model(x)
    model.stft = RealSTFT(cfg.audio)
    with torch.no_grad():
        got = model(x)
    err = (ref - got).abs().max().item()
    print(f"patched-vs-original max abs err (torch): {err:.3e}  (ref peak {ref.abs().max().item():.3f})")
    assert err < 1e-3, err
    out_path.parent.mkdir(parents=True, exist_ok=True)
    torch.onnx.export(model, (x,), str(out_path), input_names=["mix"], output_names=["stems"],
                      opset_version=17, do_constant_folding=False, dynamo=False)
    print("wrote", out_path, out_path.stat().st_size / 1e6, "MB")


if __name__ == "__main__":
    export(C.WORK / "drumsep" / "drumsep_mdx23c.onnx")
