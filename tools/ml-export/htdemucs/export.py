"""Export HTDemucs v4 (`htdemucs`, 4 stems) to a self-contained ONNX graph (STFT/iSTFT inside).

Approach (same idea as the Mixxx GSoC 2025 export): the network is unchanged; only the complex
STFT helpers of `HTDemucs` (`_spec`, `_magnitude`, `_mask`, `_ispec`) are replaced by real-valued
equivalents. The STFT is framing (reshape + slices) times a windowed DFT basis (MatMul) and the iSTFT a
MatMul plus overlap-add (Pad + Add) and the window-energy normalisation, so the graph has no complex or STFT ops and
runs on the DirectML EP. The DFT bases are computed inside the graph from Range/Mod/Cos ops
(exported without constant folding), which keeps the file at the size of the weights.

Fixed shapes: input `mix` [1, 2, 343980] (7.8 s at 44.1 kHz, the model's training segment),
output `stems` [1, 4, 2, 343980] in source order drums, bass, other, vocals.
"""

from __future__ import annotations

import math
import sys
import types
from pathlib import Path

import torch
import torch.nn.functional as F

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

SEGMENT = 343980  # int(Fraction(39, 5) * 44100)


def _dft_basis(n_fft: int, n_bins: int, like: torch.Tensor):
    """cos/sin basis [n_bins, n_fft], the periodic Hann window and the bin ramp, built in-graph.

    The index ramp comes from `like` (ones_like + CumSum) so that the tracer records ops
    instead of baking a 64 MB constant. k*n < 2^24 and N is a power of two, so the float32
    phase index (k*n mod N) is exact.
    """
    n = torch.ones_like(like.reshape(-1)[:n_fft]).cumsum(0) - 1.0
    k = n[:n_bins]
    m = torch.remainder(k[:, None] * n[None, :], float(n_fft))
    ang = m * (2.0 * math.pi / n_fft)
    win = 0.5 - 0.5 * torch.cos(n * (2.0 * math.pi / n_fft))  # periodic Hann
    return torch.cos(ang), torch.sin(ang), win, k


def _frames(x, n_fft: int, hop: int):
    """[R, L] with L % hop == 0 -> frames [R, T, n_fft], T = L/hop - n_fft/hop + 1 (no Conv)."""
    R, L = x.shape
    q = n_fft // hop
    blocks = x.reshape(R, L // hop, hop)
    T = L // hop - q + 1
    return torch.cat([blocks[:, i : i + T] for i in range(q)], -1)


def _overlap_add(fr, hop: int):
    """frames [R, T, n_fft] -> [R, (T - 1) * hop + n_fft] (sum of shifted frames, no ConvT)."""
    R, T, N = fr.shape
    q = N // hop
    out = 0
    for i in range(q):
        part = fr[:, :, i * hop : (i + 1) * hop]  # [R, T, hop] lands at block t + i
        out = out + F.pad(part, (0, 0, i, q - 1 - i))
    return out.reshape(R, (T + q - 1) * hop)


def _spec_real(self, x):
    """Real-valued replacement of HTDemucs._spec. Returns z as [B, C, F, T, 2] (re, im)."""
    hl = self.hop_length
    nfft = self.nfft
    B, C, L = x.shape
    le = int(math.ceil(L / hl))
    pad = hl // 2 * 3
    x = x.reshape(B * C, 1, L)
    x = F.pad(x, (pad, pad + le * hl - L), mode="reflect")
    x = F.pad(x, (nfft // 2, nfft // 2), mode="reflect")  # torch.stft(center=True)
    nb = nfft // 2  # the Nyquist bin is dropped by _spec
    cos, sin, win, _ = _dft_basis(nfft, nb, x)
    scale = 1.0 / math.sqrt(nfft)  # normalized=True
    w = torch.cat([cos * win * scale, -sin * win * scale], 0)  # [2F, N]
    fr = _frames(x.reshape(B * C, -1), nfft, hl)[:, 2 : 2 + le]  # [BC, le, N]
    z = torch.matmul(fr, w.t())  # [BC, le, 2F]
    z = z.reshape(B, C, le, 2, nb).permute(0, 1, 4, 2, 3)  # [B, C, F, T, 2]
    return z


def _magnitude_real(self, z):
    B, C, Fr, T, _ = z.shape
    return z.permute(0, 1, 4, 2, 3).reshape(B, C * 2, Fr, T)


def _mask_real(self, z, m):
    B, S, C2, Fr, T = m.shape
    return m.view(B, S, C2 // 2, 2, Fr, T).permute(0, 1, 2, 4, 5, 3)  # [B, S, C, F, T, 2]


def _ispec_real(self, z, length=None, scale=0):
    hl = self.hop_length // (4**scale)
    nfft = 4 * hl
    B, S, C, Fr, T, _ = z.shape
    zz = z.reshape(B * S * C, Fr, T, 2).permute(0, 2, 3, 1).reshape(B * S * C, T, 2 * Fr)
    zz = F.pad(zz, (0, 0, 2, 2))  # _ispec pads 2 empty frames on each side
    cos, sin, win, k = _dft_basis(nfft, Fr, z)
    ck = 2.0 - (k == 0.0).to(z.dtype)[:, None]  # DC once, others twice; Nyquist bin is 0
    scale_ = 1.0 / math.sqrt(nfft)  # normalized=True: sqrt(N) * (1/N)
    w = torch.cat([ck * cos * win * scale_, -ck * sin * win * scale_], 0)  # [2F, N]
    y = _overlap_add(torch.matmul(zz, w), hl)  # [BSC, (T+3)*hl + N]
    env = _overlap_add((win * win).reshape(1, 1, nfft).expand(1, T + 4, nfft), hl)
    pad = hl // 2 * 3
    start = nfft // 2 + pad
    y = y[:, start : start + length] / env[:, start : start + length]
    return y.reshape(B, S, C, length)


def load_model():
    from demucs.pretrained import get_model

    bag = get_model("htdemucs")
    model = bag.models[0]
    model.eval()
    return bag, model


def patch_for_onnx(model):
    model._spec = types.MethodType(_spec_real, model)
    model._magnitude = types.MethodType(_magnitude_real, model)
    model._mask = types.MethodType(_mask_real, model)
    model._ispec = types.MethodType(_ispec_real, model)
    return model


def export(out_path: Path):
    _, model = load_model()
    assert int(model.segment * model.samplerate) == SEGMENT
    x = torch.randn(1, 2, SEGMENT) * 0.1
    with torch.no_grad():
        ref = model(x)
    patch_for_onnx(model)
    with torch.no_grad():
        got = model(x)
    err = (ref - got).abs().max().item()
    print(f"patched-vs-original max abs err (torch): {err:.3e}")
    assert err < 1e-3, err
    out_path.parent.mkdir(parents=True, exist_ok=True)
    torch.onnx.export(
        model,
        (x,),
        str(out_path),
        input_names=["mix"],
        output_names=["stems"],
        opset_version=17,
        do_constant_folding=False,
        dynamo=False,
    )
    print("wrote", out_path, out_path.stat().st_size / 1e6, "MB")


if __name__ == "__main__":
    from common import WORK

    export(WORK / "htdemucs" / "htdemucs.onnx")
