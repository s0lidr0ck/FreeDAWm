"""MIDI-LLM: download -> export (export.py, 8-bit) -> parity against the PyTorch original -> golden
files -> install ml-models/midi_llm/.

Needs `transformers`, `tokenizers`, `safetensors` and `onnxruntime-genai==0.8.3` (its model builder)
in the venv. Peak about 11 GB of RAM (the builder) and 10 GB of disk (the fp32 intermediate).
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import time
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE))

import common as C  # noqa: E402
import reference_infer as RI  # noqa: E402

ID = "midi_llm"
REPO = "slseanwu/MIDI-LLM_Llama-3.2-1B"
REVISION = "main"
WORK = C.WORK / ID
HF = WORK / "hf"
OUT = WORK / "onnx"
PROMPT = "Upbeat and playful jazz music with lively saxophones, like you're going out on a Sunday picnic."
GOLDEN_TOKENS = 150
GOLDEN_LOGIT_STEPS = 4


def torch_logits(ids: list[int], tokens: list[int]) -> np.ndarray:
    """The original (PyTorch, bfloat16 weights in float32) teacher-forced MIDI logits, as
    RI.teacher_forced_logits; in its own process (the model takes 7 GB)."""
    np.save(WORK / "parity_ids.npy", np.array(ids, np.int64))
    np.save(WORK / "parity_tokens.npy", np.array(tokens, np.int64))
    subprocess.run([sys.executable, str(HERE / "parity_torch.py"), str(HF), str(WORK)], check=True)
    return np.load(WORK / "parity_torch.npy")


def main(force=False):
    if not force and C.card_is_current(ID):
        print(f"{ID}: installed card matches, skipping")
        return None
    from huggingface_hub import snapshot_download
    from tokenizers import Tokenizer

    WORK.mkdir(parents=True, exist_ok=True)
    snapshot_download(REPO, revision=REVISION, local_dir=str(HF),
                      allow_patterns=["*.json", "model.safetensors", "README.md"])
    if not (OUT / "model.onnx").exists():
        # Its own process: the builder holds the PyTorch model.
        subprocess.run([sys.executable, str(HERE / "export.py"), str(HF), str(WORK), str(OUT)], check=True)
        shutil.rmtree(WORK / "fp32", ignore_errors=True)
    shutil.copy2(HF / "tokenizer.json", OUT / "tokenizer.json")
    tok = Tokenizer.from_file(str(OUT / "tokenizer.json"))
    sess = RI.make_session(OUT / "model.onnx")
    report = {"id": ID}

    # A seeded piece, then the logits along it (teacher forcing) from ONNX and from PyTorch.
    ids = RI.prompt_ids(tok, PROMPT)
    t0 = time.time()
    tokens = RI.generate(sess, tok, PROMPT, seconds=100.0, max_tokens=GOLDEN_TOKENS, seed=1)
    report["tokens_per_second_cpu"] = round(len(tokens) / (time.time() - t0), 1)
    onnx_l = RI.teacher_forced_logits(sess, ids, tokens)
    del sess
    torch_l = torch_logits(ids, tokens)
    # Agreement where it matters: the most likely token of the kind each position takes.
    def top(l):
        return [RI.allowed(i)[0] + int(np.argmax(row[slice(*RI.allowed(i))])) for i, row in enumerate(l)]
    agree = float(np.mean(np.array(top(onnx_l)) == np.array(top(torch_l))))
    report["parity_vs_pytorch"] = {"logit_snr_db": round(C.snr_db(torch_l, onnx_l), 2), "top1_agreement": round(agree, 4),
                                   "steps": len(tokens) + 1, "note": "8-bit weights (block 32, int8 compute), float16 embedding"}
    print("parity", report["parity_vs_pytorch"])
    if agree < 0.85:
        raise SystemExit(f"{ID}: 8-bit export disagrees with PyTorch too often ({agree:.2%})")

    gold = WORK / "golden"
    gold.mkdir(parents=True, exist_ok=True)
    C.save_json(gold / "prompt.json", {"prompt": PROMPT, "text": RI.prompt_text(PROMPT), "ids": ids})
    C.save_json(gold / "tokens.json", {"tokens": tokens, "argmax": top(onnx_l)})
    C.write_f32(gold / "logits_4.f32", onnx_l[:GOLDEN_LOGIT_STEPS])
    parts = RI.decode(tokens, 100.0)
    C.save_json(gold / "notes.json", [{"instrument": i, "drums": d, "notes": n} for i, d, n in parts])
    (gold / "README.txt").write_text(
        "Golden files for midi_llm (reference_infer.py, CPU ORT). Raw little-endian float32, C order.\n"
        "prompt.json  : the prompt, its full text and the model input ids (tokenizer + MIDI start).\n"
        f"tokens.json  : tokens = {len(tokens)} MIDI tokens (seed 1, temperature 1, top_p 0.98);\n"
        "               argmax = per step the most likely token of the kind the step takes, teacher-forced\n"
        "               along `tokens` (step 0 = after the prompt).\n"
        f"logits_4.f32 : [{GOLDEN_LOGIT_STEPS}, {RI.MIDI_TOKENS}] MIDI logits of the first steps (teacher-forced). Rust: SNR >= 30 dB (int8 compute differs a little between ONNX Runtime versions).\n"
        "notes.json   : decode(tokens): one entry per instrument, notes {start, end (s), pitch, velocity}.\n",
        encoding="utf-8")

    card = {
        "id": ID, "name": "MIDI-LLM (Llama 3.2 1B) - text to multitrack MIDI", "task": "text_to_midi",
        "file": "model.onnx",
        "extra_files": [{"file": "model.onnx.data"}, {"file": "tokenizer.json"}],
        "tokenizer": "tokenizer.json",
        "layers": RI.LAYERS, "kv_heads": RI.KV_HEADS, "head_dim": RI.HEAD_DIM,
        "logits_from": RI.LLAMA_VOCAB, "midi_tokens": RI.MIDI_TOKENS,
        "inputs": [{"name": "input_ids", "shape": [1, "n"], "dtype": "int64"},
                   {"name": "attention_mask", "shape": [1, "past+n"], "dtype": "int64"},
                   {"name": "past_key_values.{i}.key|value", "shape": [1, RI.KV_HEADS, "past", RI.HEAD_DIM], "dtype": "float32"}],
        "outputs": [{"name": "logits", "shape": [1, "n", 55030]},
                    {"name": "present.{i}.key|value", "shape": [1, RI.KV_HEADS, "past+n", RI.HEAD_DIM]}],
        "sampling": {"temperature": 1.0, "top_p": 0.98, "max_tokens": 2046},
        "source_url": f"https://huggingface.co/{REPO}",
        "licence": "Llama 3.2 Community License Agreement (weights; the inference code is under the same agreement). "
                   "Built with Llama. MIDI-LLM by Shih-Lun Wu, Dave Carlton, Ryan Mikayawa, Yoon Kim, Chris Donahue "
                   "and Cheng-Zhi Anna Huang.",
        "export_note": "onnxruntime-genai model builder (fp32, CPU, GroupQueryAttention), then export.py: 8-bit MatMulNBits "
                       "(block 32), float16 token embedding, LM head cut to the MIDI vocabulary.",
        "dml_supported": False,
        "dml_note": "Not checked on DirectML yet (MatMulNBits and GroupQueryAttention graph built for the CPU).",
        "recommended_ep": "cpu",
        "reference": "tools/ml-export/midillm/reference_infer.py", "golden": "golden/README.txt",
    }
    dst = C.MODELS / ID
    dst.mkdir(parents=True, exist_ok=True)
    for f in ("model.onnx.data", "tokenizer.json"):
        shutil.copy2(OUT / f, dst / f)
    for ef in card["extra_files"]:
        ef["sha256"] = C.sha256(dst / ef["file"])
    C.install(ID, OUT / "model.onnx", card, gold)
    C.mark_ready(ID)
    C.save_json(WORK / "report.json", report)
    return report


if __name__ == "__main__":
    main(force="--force" in sys.argv)
