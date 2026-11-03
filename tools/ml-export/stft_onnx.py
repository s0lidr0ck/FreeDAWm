"""Export-time helpers: an ONNX-friendly real-valued STFT / iSTFT for torch models.

No complex tensors, no STFT/DFT ONNX ops, no Conv with a computed kernel: framing is
reshape + slices, the DFT is a MatMul with a windowed cos/sin basis, and the inverse is a
MatMul + overlap-add (Pad + Add) divided by the overlap-added squared window (exactly what
torch.istft does). The basis is computed inside the graph from a ones_like + CumSum ramp so the
tracer does not bake it into a large constant; k*n < 2^24 and N is a power of two, so the
float32 phase index (k*n mod N) is exact. Runs on the CPU and DirectML EPs.
"""

from __future__ import annotations

import math

import torch
import torch.nn.functional as F


def dft_basis(n_fft: int, n_bins: int, like: torch.Tensor):
    """cos/sin [n_bins, n_fft], periodic Hann window [n_fft] and the bin ramp k [n_bins]."""
    n = torch.ones_like(like.reshape(-1)[:n_fft]).cumsum(0) - 1.0
    k = n[:n_bins]
    m = torch.remainder(k[:, None] * n[None, :], float(n_fft))
    ang = m * (2.0 * math.pi / n_fft)
    win = 0.5 - 0.5 * torch.cos(n * (2.0 * math.pi / n_fft))
    return torch.cos(ang), torch.sin(ang), win, k


def frames(x: torch.Tensor, n_fft: int, hop: int) -> torch.Tensor:
    """[R, L] with L % hop == 0 and n_fft % hop == 0 -> [R, T, n_fft], T = L/hop - n_fft/hop + 1."""
    R, L = x.shape
    q = n_fft // hop
    blocks = x.reshape(R, L // hop, hop)
    T = L // hop - q + 1
    return torch.cat([blocks[:, i : i + T] for i in range(q)], -1)


def overlap_add(fr: torch.Tensor, hop: int) -> torch.Tensor:
    """[R, T, n_fft] -> [R, (T - 1) * hop + n_fft]."""
    R, T, N = fr.shape
    q = N // hop
    out = 0
    for i in range(q):
        part = fr[:, :, i * hop : (i + 1) * hop]
        out = out + F.pad(part, (0, 0, i, q - 1 - i))
    return out.reshape(R, (T + q - 1) * hop)


def stft(x: torch.Tensor, n_fft: int, hop: int, n_bins: int, normalized: bool):
    """torch.stft(center=False) of an already padded [R, L] signal, Hann window.
    Returns (re, im), each [R, T, n_bins]."""
    cos, sin, win, _ = dft_basis(n_fft, n_bins, x)
    s = 1.0 / math.sqrt(n_fft) if normalized else 1.0
    w = torch.cat([cos * win * s, -sin * win * s], 0)  # [2F, N]
    z = torch.matmul(frames(x, n_fft, hop), w.t())  # [R, T, 2F]
    return z[..., :n_bins], z[..., n_bins:]


def istft(re: torch.Tensor, im: torch.Tensor, n_fft: int, hop: int, normalized: bool):
    """Inverse of `stft` before trimming: re/im [R, T, F] with F <= n_fft/2 (missing bins incl.
    Nyquist are zero) -> (y [R, (T-1)*hop + n_fft], env [1, same]); the caller slices and
    divides y by env (torch.istft semantics)."""
    R, T, Fb = re.shape
    cos, sin, win, k = dft_basis(n_fft, Fb, re)
    ck = 2.0 - (k == 0.0).to(re.dtype)[:, None]  # DC once, other bins twice
    s = (1.0 / math.sqrt(n_fft)) if normalized else (1.0 / n_fft)
    w = torch.cat([ck * cos * win * s, -ck * sin * win * s], 0)  # [2F, N]
    y = overlap_add(torch.matmul(torch.cat([re, im], -1), w), hop)
    env = overlap_add((win * win).reshape(1, 1, n_fft).expand(1, T, n_fft), hop)
    return y, env
