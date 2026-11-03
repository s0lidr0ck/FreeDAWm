"""Export GAME with its own deployment exporter, except that the segmenter's D3PM random draws
(torch.rand_like in modules.d3pm.remove_boundaries) become an explicit graph input `noise`
f32 [1, T] in [0, 1). That makes the pipeline deterministic and reproducible from Rust: the host
generates `noise` with the SplitMix64 scheme in reference_infer.noise(). Run with cwd = the GAME
repo checkout: python export.py <model.pt> <out_dir>"""

import pathlib
import sys

sys.path.insert(0, str(pathlib.Path.cwd()))

import torch  # noqa: E402

import modules.d3pm as d3pm  # noqa: E402
from deployment.context import export_mode  # noqa: E402
from deployment.exporter import Exporter  # noqa: E402
from inference.api import load_inference_model  # noqa: E402

NOISE = [None]


def remove_boundaries(boundaries, p):
    p = p.unsqueeze(-1)
    q = 1 - p
    rnd = NOISE[0] if NOISE[0] is not None else torch.rand_like(boundaries, dtype=torch.float32)
    return (rnd <= q) & boundaries


def patch():
    d3pm.remove_boundaries = remove_boundaries  # remove_mutable_boundaries looks it up by global name


class Segmenter(torch.nn.Module):
    def __init__(self, model):
        super().__init__()
        self.model = model

    def forward(self, x_seg, language, known_boundaries, prev_boundaries, t, maskT, threshold, radius, noise):
        NOISE[0] = noise
        try:
            return self.model.forward_and_decode_boundaries(
                x_seg=x_seg, known_boundaries=known_boundaries, prev_boundaries=prev_boundaries, t=t,
                language=language, mask=maskT, threshold=threshold, radius=radius)
        finally:
            NOISE[0] = None


class NoiseExporter(Exporter):
    def export_segmenter(self):
        assert self.model.model_config.use_languages and self.model.model_config.mode == "d3pm"
        names = ["language", "known_boundaries", "prev_boundaries", "t", "maskT", "threshold", "radius", "noise"]
        args = (torch.randn(1, 100, self.model.model_config.embedding_dim), torch.zeros((1,), dtype=torch.int64),
                torch.ones(1, 100, dtype=torch.bool), torch.ones(1, 100, dtype=torch.bool), torch.rand((1,)),
                torch.ones(1, 100, dtype=torch.bool), torch.tensor(0.2), torch.tensor(2, dtype=torch.int64),
                torch.rand(1, 100))
        axes = {"x_seg": {0: "B", 1: "T"}, "language": {0: "B"}, "known_boundaries": {0: "B", 1: "T"},
                "prev_boundaries": {0: "B", 1: "T"}, "t": {0: "B"}, "maskT": {0: "B", 1: "T"},
                "noise": {0: "B", 1: "T"}, "boundaries": {0: "B", 1: "T"}}
        self._export(Segmenter(self.model), args, self.segmenter_path, input_names=["x_seg", *names],
                     output_names=["boundaries"], dynamic_axes=axes)


def main(pt, out_dir):
    patch()
    with export_mode():
        model, lang_map = load_inference_model(pathlib.Path(pt))
        NoiseExporter(model, pathlib.Path(out_dir), dynamo=False, opset_version=17).export()
        import json

        cfg = {"samplerate": model.inference_config.features.audio_sample_rate, "timestep": model.timestep,
               "languages": lang_map, "loop": True, "embedding_dim": model.model_config.embedding_dim,
               "noise_input": True}
        (pathlib.Path(out_dir) / "config.json").write_text(json.dumps(cfg, indent=2))


if __name__ == "__main__":
    main(*sys.argv[1:3])
