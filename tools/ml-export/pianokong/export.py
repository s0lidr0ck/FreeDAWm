"""Export ByteDance's high-resolution piano transcription model (notes + pedal, Kong et al. 2020)
to ONNX. The network (piano_transcription_inference.models.Note_pedal) is unchanged; its log-mel
front end (torchlibrosa Conv1d STFT + mel matrix) is part of the graph.
Input "audio" f32 [1, 160000] (10 s @ 16 kHz); outputs (each [1, 1001, 88] or [1, 1001, 1]):
reg_onset, reg_offset, frame, velocity, reg_pedal_onset, reg_pedal_offset, pedal_frame."""

from __future__ import annotations

import sys
from pathlib import Path

import torch
import torch.nn as nn

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import common as C  # noqa: E402

CKPT_URL = "https://zenodo.org/record/4034264/files/CRNN_note_F1%3D0.9677_pedal_F1%3D0.9186.pth?download=1"
CKPT = C.WORK / "dl" / "w" / "kong_piano_note_pedal.pth"
KEYS = ["reg_onset_output", "reg_offset_output", "frame_output", "velocity_output",
        "reg_pedal_onset_output", "reg_pedal_offset_output", "pedal_frame_output"]
NAMES = ["reg_onset", "reg_offset", "frame", "velocity", "reg_pedal_onset", "reg_pedal_offset", "pedal_frame"]


def load_model():
    from piano_transcription_inference.models import Note_pedal

    C.download(CKPT_URL, CKPT)
    m = Note_pedal(frames_per_second=100, classes_num=88)
    m.load_state_dict(torch.load(CKPT, map_location="cpu", weights_only=False)["model"], strict=False)
    return m.eval()


class Wrap(nn.Module):
    def __init__(self, m):
        super().__init__()
        self.m = m

    def forward(self, audio):
        d = self.m(audio)
        return tuple(d[k] for k in KEYS)


def export(out_path: Path):
    m = Wrap(load_model()).eval()
    x = torch.randn(1, 160000) * 0.1
    out_path.parent.mkdir(parents=True, exist_ok=True)
    torch.onnx.export(m, (x,), str(out_path), input_names=["audio"], output_names=NAMES,
                      opset_version=17, do_constant_folding=True, dynamo=False)
    print("wrote", out_path, out_path.stat().st_size / 1e6, "MB")


if __name__ == "__main__":
    export(C.WORK / "pianokong" / "piano_kong.onnx")
