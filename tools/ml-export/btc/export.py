"""Export BTC (Bi-directional Transformer for Chord recognition, Park et al. ISMIR 2019), large
vocabulary (170 classes), to ONNX with its CQT front end inside the graph.

Graph: audio f32 [1, 220500] (one 10 s chunk at 22050 Hz, zero-padded at the end if shorter)
       n_frames i64 [1]  (= 1 + true_chunk_length // 2048, <= 108)
  -> CQT (full-rate complex kernels = librosa.filters.wavelet, 144 bins, 24/oct from C1, hop
     2048, centred with K/2 zero padding, magnitude * sqrt(filter length): equals librosa.cqt up to
     its multirate resampling error) -> log(|C| + 1e-6) -> (x - mean) / std (checkpoint stats)
  -> frames >= n_frames set to 0 (the original pads the normalised features with zeros)
  -> BTC (8 bi-directional self-attention layers) -> logits f32 [1, 108, 170], cqt f32 [1, 108, 144]
     (the log-CQT before normalisation, used by the host for the bass-note estimate).
Usage: python export.py (fetches the repo at COMMIT into <work>/dl/btc)."""

from __future__ import annotations

import sys
import zipfile
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import common as C  # noqa: E402

COMMIT = "2682317be668032e6e4b269ded36adaa2ad57df0"
DL = C.WORK / "dl" / "btc"
SR, HOP, N_BINS, BPO, CHUNK, T = 22050, 2048, 144, 24, 220500, 108


def repo_dir():
    C.download(f"https://github.com/jayg996/BTC-ISMIR19/archive/{COMMIT}.zip", DL / "repo.zip")
    d = DL / "repo" / f"BTC-ISMIR19-{COMMIT}"
    if not d.exists():
        zipfile.ZipFile(DL / "repo.zip").extractall(DL / "repo")
    return d


def load_original():
    """BTC_model + checkpoint (mean, std), exactly as test.py --voca True builds them."""
    d = repo_dir()
    sys.path.insert(0, str(d))
    import os

    cwd = os.getcwd()
    os.chdir(d)
    import yaml

    if not getattr(yaml, "_btc_patched", False):  # utils/hparams.py calls yaml.load(f) (PyYAML < 6 API)
        _load = yaml.load
        yaml.load = lambda stream, Loader=yaml.SafeLoader: _load(stream, Loader=Loader)
        yaml._btc_patched = True
    if not hasattr(np, "float"):  # transformer_modules uses np.float (removed in NumPy 1.24); = builtin float
        np.float = float
    try:
        from btc_model import BTC_model, HParams

        cfg = HParams.load("run_config.yaml")
        cfg.feature["large_voca"] = True
        cfg.model["num_chords"] = 170
        m = BTC_model(config=cfg.model)
        ck = torch.load(d / "test" / "btc_model_large_voca.pt", map_location="cpu", weights_only=False)
        m.load_state_dict(ck["model"])
    finally:
        os.chdir(cwd)
    return m.eval(), float(ck["mean"]), float(ck["std"]), cfg


def cqt_kernels():
    import librosa

    freqs = librosa.cqt_frequencies(N_BINS, fmin=librosa.note_to_hz("C1"), bins_per_octave=BPO)
    filt, lengths = librosa.filters.wavelet(freqs=freqs, sr=SR, window="hann", filter_scale=1, norm=1)
    assert filt.shape[1] % 2 == 0
    return filt, lengths  # [144, K] complex, centred at K // 2


class Graph(nn.Module):
    def __init__(self, m, mean, std):
        super().__init__()
        self.m = m
        filt, lengths = cqt_kernels()
        k = np.conj(filt)
        w = np.concatenate([k.real, k.imag]).astype(np.float32)[:, None, :]  # [288, 1, K]
        self.register_buffer("w", torch.from_numpy(w))
        self.register_buffer("scale", torch.from_numpy(np.sqrt(lengths).astype(np.float32))[None, :, None])
        self.pad = filt.shape[1] // 2
        self.mean, self.std = mean, std

    def forward(self, audio, n_frames):
        y = F.conv1d(F.pad(audio[:, None, :], (self.pad, self.pad)), self.w, stride=HOP)  # [1, 288, 108]
        mag = torch.sqrt(y[:, :N_BINS] ** 2 + y[:, N_BINS:] ** 2) * self.scale
        cqt = torch.log(mag + 1e-6).transpose(1, 2)  # [1, 108, 144]
        x = (cqt - self.mean) / self.std
        keep = (torch.arange(T)[None, :, None] < n_frames[:, None, None]).to(x.dtype)
        x = x * keep
        h, _ = self.m.self_attn_layers(x)
        return self.m.output_layer.output_projection(h), cqt


def export(out_path: Path):
    m, mean, std, _ = load_original()
    g = Graph(m, mean, std).eval()
    a = torch.randn(1, CHUNK) * 0.1
    n = torch.tensor([T], dtype=torch.int64)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with torch.no_grad():
        torch.onnx.export(g, (a, n), str(out_path), input_names=["audio", "n_frames"], output_names=["logits", "cqt"],
                          opset_version=17, do_constant_folding=True, dynamo=False)
    print("wrote", out_path, out_path.stat().st_size / 1e6, "MB")


if __name__ == "__main__":
    export(C.WORK / "btc" / "btc_chords.onnx")
