"""MIDI-LLM in PyTorch (the original), teacher-forced: <work>/parity_torch.npy, the MIDI logits after
the prompt and after each token, [len(tokens) + 1, 55026].

    python parity_torch.py <hf model dir> <work dir>    (reads parity_ids.npy, parity_tokens.npy)
"""

import sys
from pathlib import Path

import numpy as np
import torch
from transformers import AutoModelForCausalLM

LLAMA_VOCAB, MIDI_TOKENS = 128256, 55026

hf, work = Path(sys.argv[1]), Path(sys.argv[2])
ids = [int(i) for i in np.load(work / "parity_ids.npy")]
tokens = [int(t) for t in np.load(work / "parity_tokens.npy")]
model = AutoModelForCausalLM.from_pretrained(str(hf), dtype=torch.float32).eval()
with torch.no_grad():
    logits = model(torch.tensor([ids + [LLAMA_VOCAB + t for t in tokens]])).logits[0]
first = len(ids) - 1
np.save(work / "parity_torch.npy", logits[first:first + len(tokens) + 1, LLAMA_VOCAB:LLAMA_VOCAB + MIDI_TOKENS].float().numpy())
