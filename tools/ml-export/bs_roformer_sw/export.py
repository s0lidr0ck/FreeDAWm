"""Export BS-RoFormer SW (6 stems: bass, drums, other, vocals, guitar, piano) to ONNX: spectrogram in,
six masked spectrograms out; STFT/iSTFT on the host (as karaoke_melroformer, `../karaoke/export.py`).

Checkpoint: `BS-Rofo-SW-Fixed.ckpt` (https://huggingface.co/enerjazzer/BS-ROFO-SW-Fixed, licence
unknown: personal use). Its config: dim 256, depth 12, 62 bands, STFT 2048 / 512, chunk 588800
(13.35 s, 1151 frames), num_overlap 2. The export uses 409600-sample chunks (9.29 s, 801 frames, like
the Mel-Band graphs): at 1151 frames ORT-DirectML 1.24 (graph fusion off, 8 GB RTX 5060 Laptop)
stops with 887A0006 "The GPU will not respond to more commands"; at 801 frames DML matches CPU
(112 dB on one chunk). Parity is against MSS demix with the same chunk.

The graph is BSRoformer.forward between `torch.stft` and `torch.istft`, unchanged except: complex
tensors become explicit (re, im) arithmetic; the DC bin is zeroed by a multiply; no tensor has
rank > 4 (the fused qkv split uses chunk(), rotate_half is a MatMul, the six stems x two channels
are one axis); attention runs in ATTN_SPLIT batch pieces so DirectML (graph fusion off) fits the
62 x 8 x 801 x 801 time-attention scores. A published host-STFT export
(huggingface.co/elicwhite/bs-roformer-sw-6stem-onnx, 4 s chunks) is only a cross-check.

Input  `spec` f32 [2, 1025, 801, 2]  (channel L/R, bin, frame, re/im)
Output `stems_spec` f32 [12, 1025, 801, 2]  (stem * 2 + channel, bin, frame, re/im), stems in the
       config's order (STEMS), DC bin zeroed.
"""

from __future__ import annotations

import sys
from pathlib import Path

import torch
import torch.nn as nn
import torch.nn.functional as F
import yaml
from einops import pack, rearrange, unpack

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import common as C  # noqa: E402
import mss_fetch  # noqa: E402

REPO = "https://huggingface.co/enerjazzer/BS-ROFO-SW-Fixed/resolve/main/"
CKPT = "BS-Rofo-SW-Fixed.ckpt"
CFG = "BS-Rofo-SW-Fixed.yaml"
DL = C.WORK / "dl" / "w"
N_FFT = 2048
HOP = 512
CHUNK = 409600  # 9.29 s, 801 frames (the config's 588800 hangs DirectML: see the docstring)
FRAMES = CHUNK // HOP + 1  # 801
STEMS = ["bass", "drums", "other", "vocals", "guitar", "piano"]
ATTN_SPLIT = 8


def load_config():
    from ml_collections import ConfigDict

    p = C.download(REPO + CFG, DL / CFG)
    return ConfigDict(yaml.load(open(p), Loader=yaml.FullLoader))


def load_model():
    mss_fetch.fetch()
    from models.bs_roformer.bs_roformer import BSRoformer

    cfg = load_config()
    model = BSRoformer(**dict(cfg.model))
    sd = torch.load(C.download(REPO + CKPT, DL / CKPT), map_location="cpu", weights_only=False)
    if isinstance(sd, dict) and "state_dict" in sd:
        sd = sd["state_dict"]
    model.load_state_dict(sd)
    model.eval()
    assert list(cfg.training.instruments) == STEMS, cfg.training.instruments
    assert cfg.model.stft_hop_length == HOP
    cfg.audio.chunk_size = CHUNK
    return cfg, model


class SpecNet(nn.Module):
    """BSRoformer.forward without torch.stft / torch.istft (batch 1, stereo, all stems)."""

    def __init__(self, m):
        super().__init__()
        self.m = m
        assert len(m.layers[0]) == 2, "linear_transformer_depth must be 0"
        dc = torch.ones(1, 1, 1, 1)
        self.register_buffer("dc_mask", torch.cat([torch.zeros(1, 1, 1, 1), dc.expand(1, N_FFT // 2, 1, 1)], 1), persistent=False)

    def forward(self, spec):  # [2 (s), F, T, 2 (c)]
        m = self.m
        S, Fb, T, _ = spec.shape
        stft_repr = spec.permute(1, 0, 2, 3).reshape(1, Fb * S, T, 2)  # b (f s) t c
        x = rearrange(stft_repr, "b f t c -> b t (f c)")
        x = m.band_split(x)
        for time_transformer, freq_transformer in m.layers:
            x = rearrange(x, "b t f d -> b f t d")
            x, ps = pack([x], "* t d")
            x = time_transformer(x)
            (x,) = unpack(x, ps, "* t d")
            x = rearrange(x, "b f t d -> b t f d")
            x, ps = pack([x], "* f d")
            x = freq_transformer(x)
            (x,) = unpack(x, ps, "* f d")
        x = m.final_norm(x)
        sr, si = stft_repr[..., 0], stft_repr[..., 1]  # [1, (f s), t]
        outs = []
        for est in m.mask_estimators:
            mask = rearrange(est(x), "b t (f c) -> b f t c", c=2)  # [1, (f s), t, 2]
            mr, mi = mask[..., 0], mask[..., 1]
            outs.append(torch.stack([sr * mr - si * mi, sr * mi + si * mr], -1))  # complex multiply
        n = len(outs)
        out = torch.cat(outs, 0)  # [n, (f s), t, 2]
        out = out.reshape(n, Fb, S, T * 2).permute(0, 2, 1, 3).reshape(n * S, Fb, T, 2)  # [(n s), f, t, c]
        return out * self.dc_mask  # zero_dc as a multiply


class rank4_patches:
    """Exact rewrites for the export (see ../karaoke/export.py): rank <= 4 everywhere and attention
    in ATTN_SPLIT batch pieces."""

    def __enter__(self):
        import rotary_embedding_torch.rotary_embedding_torch as rot
        from models.bs_roformer import attend as att
        from models.bs_roformer import bs_roformer as bsr

        self.rot, self.bsr, self.att = rot, bsr, att
        self.old_rot, self.old_fwd, self.old_attend = rot.rotate_half, bsr.Attention.forward, att.Attend.forward

        def attend_forward(self_, q, k, v):
            assert self_.flash and self_.scale is None
            n = max(1, -(-q.shape[0] // ATTN_SPLIT))
            outs = [F.scaled_dot_product_attention(qq, kk, vv) for qq, kk, vv in zip(q.split(n, 0), k.split(n, 0), v.split(n, 0))]
            return torch.cat(outs, 0)

        def rotate_half(x):
            d = x.shape[-1]
            r = torch.zeros(d, d, dtype=x.dtype)
            i = torch.arange(0, d, 2)
            r[i + 1, i] = -1.0
            r[i, i + 1] = 1.0
            return torch.matmul(x, r)

        def attn_forward(self_, x):
            assert self_.rotary_embed is not None and self_.pope_embed is None
            x = self_.norm(x)
            q, k, v = (rearrange(t, "b n (h d) -> b h n d", h=self_.heads) for t in self_.to_qkv(x).chunk(3, dim=-1))
            q = self_.rotary_embed.rotate_queries_or_keys(q)
            k = self_.rotary_embed.rotate_queries_or_keys(k)
            out = self_.attend(q, k, v)
            gates = self_.to_gates(x)
            out = out * rearrange(gates, "b n h -> b h n 1").sigmoid()
            out = rearrange(out, "b h n d -> b n (h d)")
            return self_.to_out(out)

        rot.rotate_half = rotate_half
        bsr.Attention.forward = attn_forward
        att.Attend.forward = attend_forward
        return self

    def __exit__(self, *exc):
        self.rot.rotate_half = self.old_rot
        self.bsr.Attention.forward = self.old_fwd
        self.att.Attend.forward = self.old_attend


def export(out_path: Path):
    cfg, model = load_model()
    net = SpecNet(model).eval()
    L = CHUNK
    torch.manual_seed(0)
    wav = torch.randn(1, 2, L) * 0.1
    win = torch.hann_window(N_FFT)
    with torch.no_grad():
        ref = model(wav)  # [1, 6, 2, L]
        z = torch.stft(wav.reshape(2, L), N_FFT, HOP, N_FFT, win, return_complex=True)
        spec = torch.view_as_real(z)  # [2, F, T, 2]
        with rank4_patches():
            y = net(spec)  # [12, F, T, 2]
        got = torch.istft(torch.view_as_complex(y.contiguous()), N_FFT, HOP, N_FFT, win, length=L).reshape(1, 6, 2, L)
    err = (ref - got).abs().max().item()
    print(f"SpecNet-vs-original max abs err (torch): {err:.3e}", flush=True)
    assert err < 1e-3
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with rank4_patches():
        torch.onnx.export(net, (spec.contiguous(),), str(out_path), input_names=["spec"], output_names=["stems_spec"],
                          opset_version=17, do_constant_folding=True, dynamo=False)
    print("wrote", out_path, out_path.stat().st_size / 1e6, "MB")


if __name__ == "__main__":
    export(C.WORK / "bs_roformer_sw" / "bs_roformer_sw.onnx")
