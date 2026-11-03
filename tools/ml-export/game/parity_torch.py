"""GAME original PyTorch pipeline (run with cwd = the GAME repo checkout):
Slicer -> SegmentationEstimationInferenceModel.forward per slice (batch 1, as InferenceModule
.predict_step) -> SaveCombinedFileCallback note assembly. Writes notes + per-slice outputs as JSON.
Usage: python parity_torch.py <model.pt> <wav 44100 mono> <language id> <out.json>"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path.cwd()))

import numpy as np  # noqa: E402
import soundfile as sf  # noqa: E402
import torch  # noqa: E402

from inference.api import load_inference_model  # noqa: E402
from inference.callbacks import SaveCombinedFileCallback  # noqa: E402
from inference.slicer2 import Slicer  # noqa: E402
from lib.config.schema import ValidationConfig  # noqa: E402
import modules.d3pm as d3pm  # noqa: E402

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import reference_infer as RI  # noqa: E402  (only its noise() generator)

QUEUE = []


def remove_boundaries(boundaries, p):
    """The original d3pm.remove_boundaries with torch.rand_like replaced by the fixed draws."""
    p = p.unsqueeze(-1)
    q = 1 - p
    rnd = torch.from_numpy(QUEUE.pop(0)).reshape(boundaries.shape)
    return (rnd <= q) & boundaries


d3pm.remove_boundaries = remove_boundaries


class Collect(SaveCombinedFileCallback):
    def flush(self, key, notes, logger_fn):
        self.result = [{"start": n.onset, "end": n.offset, "pitch_f": n.pitch} for n in notes]


def main(model_path, wav, lang, out):
    model, _ = load_inference_model(pathlib.Path(model_path))
    cfg = ValidationConfig(d3pm_sample_t0=0.0, d3pm_sample_steps=8, d3pm_sample_ts=None,
                           boundary_decoding_threshold=0.2, boundary_decoding_radius=round(0.02 / model.timestep),
                           note_presence_threshold=0.2)
    w, sr = sf.read(wav, dtype="float32")
    assert sr == 44100 and w.ndim == 1
    chunks = Slicer(sr=sr, threshold=-40.0, min_length=1000, min_interval=200, max_sil_kept=100).slice(w)
    cb = Collect(output_dir=pathlib.Path("."))
    slices = []
    for si, ch in enumerate(chunks):
        wav_t = torch.from_numpy(ch["waveform"]).float()[None]
        dur = wav_t.shape[1] / sr
        T = model.forward_encoder(wav_t, torch.FloatTensor([dur]))[2].shape[1]
        QUEUE[:] = [RI.noise(si, k, T) for k in range(8)]
        with torch.no_grad():
            durations, presence, scores = model(
                waveform=wav_t, known_durations=torch.FloatTensor([[dur]]),
                language=torch.LongTensor([int(lang)]), t=torch.tensor(cfg.d3pm_sample_ts_resolved),
                boundary_threshold=torch.tensor(cfg.boundary_decoding_threshold),
                boundary_radius=torch.tensor(cfg.boundary_decoding_radius),
                score_threshold=torch.tensor(cfg.note_presence_threshold))
        batch = {"key": ["k"], "offset": [ch["offset"]], "length": [dur], "num_parts": [len(chunks)], "size": 1}
        cb._process_item("k", batch, {"durations": durations, "presence": presence, "scores": scores}, 0)
        slices.append({"offset": ch["offset"], "n": int(wav_t.shape[1]), "durations": durations[0].tolist(),
                       "presence": presence[0].tolist(), "scores": scores[0].tolist()})
    cb._counters["k"] = len(chunks)
    cb.save_file("k", print)
    pathlib.Path(out).write_text(json.dumps({"notes": cb.result, "slices": slices}))
    print("original notes:", len(cb.result), "slices:", len(slices))


if __name__ == "__main__":
    main(*sys.argv[1:5])
