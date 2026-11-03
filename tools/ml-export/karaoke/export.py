"""Export Mel-Band RoFormer karaoke (aufr33 + viperx, SDR 10.1956) to ONNX: spectrogram in,
masked spectrogram out; STFT/iSTFT on the host.

Why host STFT: hop 441 does not divide n_fft 2048, so the reshape-based framing used for
htdemucs/drumsep does not apply, and the in-graph alternative (Conv1d with a computed DFT kernel)
cannot be traced. The host STFT is a plain rfft per frame.

The graph is MelBandRoformer.forward between `torch.stft` and `torch.istft`, unchanged except:
complex tensors are replaced by explicit (re, im) arithmetic; the overlapping-band mask average
(scatter_add / num_bands_per_freq) is two Gathers + Add (every frequency is covered by one or
two bands); the DC bin is zeroed by a multiply; no tensor has rank > 4; attention runs in 6
batch pieces (see rank4_patches). DirectML: run with ep.dml.disable_graph_fusion=1 (with fusion
the output is wrong, cause not isolated; ORT-DML 1.24).

Input  `spec` f32 [2, 1025, 801, 2]  (channel L/R, bin, frame, re/im)
Output `vocals_spec` f32 [2, 1025, 801, 2]  masked spectrogram of the lead vocal (DC zeroed).

The checkpoint is a parameter (`Variant`): KARAOKE (this folder's model) or KJ_VOCALS (Kimberley
Jensen's Mel-Band RoFormer vocals, MIT, same architecture and STFT; `../melband_vocals/run.py`).
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

BASE = "https://github.com/TRvlvr/model_repo/releases/download/all_public_uvr_models/"
CKPT = "mel_band_roformer_karaoke_aufr33_viperx_sdr_10.1956.ckpt"
CFG_URL = ("https://github.com/nomadkaraoke/python-audio-separator/releases/download/model-configs/"
           "mel_band_roformer_karaoke_aufr33_viperx_sdr_10.1956_config.yaml")
DL = C.WORK / "dl" / "w"
FRAMES = 801


class Variant:
    """A Mel-Band RoFormer checkpoint with this architecture (60 bands, STFT 2048/441, 801 frames)."""

    def __init__(self, ckpt_url: str, ckpt_name: str, cfg_url: str, cfg_name: str):
        self.ckpt_url, self.ckpt_name, self.cfg_url, self.cfg_name = ckpt_url, ckpt_name, cfg_url, cfg_name


KARAOKE = Variant(BASE + CKPT, CKPT, CFG_URL, "mel_band_roformer_karaoke_aufr33_viperx_sdr_10.1956_config.yaml")
# Kimberley Jensen's Mel-Band RoFormer vocals (MIT), config from ZFTurbo MSS at the pinned commit.
KJ_VOCALS = Variant(
    "https://huggingface.co/KimberleyJSN/melbandroformer/resolve/main/MelBandRoformer.ckpt",
    "MelBandRoformer.ckpt",
    "https://raw.githubusercontent.com/ZFTurbo/Music-Source-Separation-Training/"
    f"{mss_fetch.COMMIT}/configs/KimberleyJensen/config_vocals_mel_band_roformer_kj.yaml",
    "config_vocals_mel_band_roformer_kj.yaml",
)


def load_config(variant: Variant = KARAOKE):
    from ml_collections import ConfigDict

    p = C.download(variant.cfg_url, DL / variant.cfg_name)
    return ConfigDict(yaml.load(open(p), Loader=yaml.FullLoader))


def load_model(variant: Variant = KARAOKE):
    mss_fetch.fetch()
    from models.bs_roformer.mel_band_roformer import MelBandRoformer

    cfg = load_config(variant)
    model = MelBandRoformer(**dict(cfg.model))
    sd = torch.load(C.download(variant.ckpt_url, DL / variant.ckpt_name), map_location="cpu", weights_only=False)
    if isinstance(sd, dict) and "state_dict" in sd:
        sd = sd["state_dict"]
    model.load_state_dict(sd)
    model.eval()
    return cfg, model


class SpecNet(nn.Module):
    """MelBandRoformer.forward without torch.stft / torch.istft (batch 1, stereo, 1 stem)."""

    def __init__(self, m):
        super().__init__()
        self.m = m
        denom = m.num_bands_per_freq.repeat_interleave(2).float().clamp(min=1e-8)  # (f s) order
        self.register_buffer("denom", denom[None, :, None, None], persistent=False)
        # scatter_add as two Gathers: every (f s) index is covered by 1 or 2 bands; the missing
        # second source points at an appended zero row. Same sums as scatter_add (0 + a + b).
        fi = m.freq_indices
        n_out = int(fi.max()) + 1
        assert int(m.num_bands_per_freq.max()) <= 2
        src = torch.full((2, n_out), len(fi), dtype=torch.long)
        cnt = torch.zeros(n_out, dtype=torch.long)
        for p, j in enumerate(fi.tolist()):
            src[cnt[j], j] = p
            cnt[j] += 1
        self.register_buffer("src0", src[0], persistent=False)
        self.register_buffer("src1", src[1], persistent=False)
        dc_mask = torch.ones(1, n_out // 2, 1, 1)
        dc_mask[:, 0] = 0.0
        self.register_buffer("dc_mask", dc_mask, persistent=False)

    def forward(self, spec):  # [2 (s), F, T, 2 (c)]
        m = self.m
        S, Fb, T, _ = spec.shape
        stft_repr = spec.permute(1, 0, 2, 3).reshape(1, Fb * S, T, 2)  # b (f s) t c
        x = stft_repr[:, m.freq_indices]
        x = rearrange(x, "b f t c -> b t (f c)")
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
        mask = m.mask_estimators[0](x)
        mask = rearrange(mask, "b t (f c) -> b f t c", c=2)  # f = gathered index
        mask = torch.cat([mask, torch.zeros_like(mask[:, :1])], 1)
        avg = (mask[:, self.src0] + mask[:, self.src1]) / self.denom
        sr, si = stft_repr[..., 0], stft_repr[..., 1]
        mr, mi = avg[..., 0], avg[..., 1]
        out = torch.stack([sr * mr - si * mi, sr * mi + si * mr], -1)  # complex multiply
        out = out.reshape(Fb, S, T, 2).permute(1, 0, 2, 3)  # [s, f, t, c]
        return out * self.dc_mask  # zero_dc as a multiply


ATTN_SPLIT = 6  # attention batch pieces (time attention: 60 bands -> 6 x 10)


class rank4_patches:
    """Exact rewrites for the export: every tensor at rank <= 4 (the fused qkv projection is split
    with chunk() instead of a 5-D rearrange, rotate_half is a MatMul with a +-1 permutation matrix
    instead of a 5-D stack), and attention runs in ATTN_SPLIT batch pieces so that DirectML
    (graph fusion off) does not run out of memory on the 60 x 8 x 801 x 801 time-attention scores."""

    def __enter__(self):
        import rotary_embedding_torch.rotary_embedding_torch as rot
        from models.bs_roformer import mel_band_roformer as mbr

        self.rot, self.mbr = rot, mbr
        from models.bs_roformer import attend as att

        self.att = att
        self.old_rot, self.old_fwd, self.old_attend = rot.rotate_half, mbr.Attention.forward, att.Attend.forward

        def attend_forward(self_, q, k, v):
            assert self_.flash and self_.scale is None
            outs = [F.scaled_dot_product_attention(qq, kk, vv)
                    for qq, kk, vv in zip(q.chunk(ATTN_SPLIT, 0), k.chunk(ATTN_SPLIT, 0), v.chunk(ATTN_SPLIT, 0))]
            return torch.cat(outs, 0)

        def rotate_half(x):
            d = x.shape[-1]
            r = torch.zeros(d, d, dtype=x.dtype)
            i = torch.arange(0, d, 2)
            r[i + 1, i] = -1.0  # out[2i] = -x[2i+1]
            r[i, i + 1] = 1.0  # out[2i+1] = x[2i]
            return torch.matmul(x, r)

        def attn_forward(self_, x):
            x = self_.norm(x)
            q, k, v = (rearrange(t, "b n (h d) -> b h n d", h=self_.heads)
                       for t in self_.to_qkv(x).chunk(3, dim=-1))
            q = self_.rotary_embed.rotate_queries_or_keys(q)
            k = self_.rotary_embed.rotate_queries_or_keys(k)
            out = self_.attend(q, k, v)
            gates = self_.to_gates(x)
            out = out * rearrange(gates, "b n h -> b h n 1").sigmoid()
            out = rearrange(out, "b h n d -> b n (h d)")
            return self_.to_out(out)

        rot.rotate_half = rotate_half
        mbr.Attention.forward = attn_forward
        att.Attend.forward = attend_forward
        return self

    def __exit__(self, *exc):
        self.rot.rotate_half = self.old_rot
        self.mbr.Attention.forward = self.old_fwd
        self.att.Attend.forward = self.old_attend


def export(out_path: Path, frames: int = FRAMES, variant: Variant = KARAOKE):
    cfg, model = load_model(variant)
    net = SpecNet(model).eval()
    # check SpecNet (+ patches) against the original forward on one chunk (torch STFT around it)
    L = cfg.audio.chunk_size
    wav = torch.randn(1, 2, L) * 0.1
    win = torch.hann_window(2048)
    with torch.no_grad():
        ref = model(wav)
        z = torch.stft(wav.reshape(2, L), 2048, 441, 2048, win, return_complex=True)
        spec = torch.view_as_real(z)  # [2, F, T, 2]
        with rank4_patches():
            y = net(spec)
        got = torch.istft(torch.view_as_complex(y.contiguous()), 2048, 441, 2048, win)[None]
    err = (ref - got).abs().max().item()
    print(f"SpecNet-vs-original max abs err (torch): {err:.3e}")
    assert err < 1e-3
    out_path.parent.mkdir(parents=True, exist_ok=True)
    spec = spec[:, :, :frames].contiguous()
    with rank4_patches():
        torch.onnx.export(net, (spec,), str(out_path), input_names=["spec"], output_names=["vocals_spec"],
                          opset_version=17, do_constant_folding=True, dynamo=False)
    print("wrote", out_path, out_path.stat().st_size / 1e6, "MB")


if __name__ == "__main__":
    export(C.WORK / "karaoke" / "karaoke_melroformer.onnx")
