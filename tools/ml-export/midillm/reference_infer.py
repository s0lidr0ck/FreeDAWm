"""MIDI-LLM text -> MIDI: the complete inference pipeline in numpy + onnxruntime (+ `tokenizers` for
the prompt). The spec `crates/ml/src/textmidi.rs` follows.

Model: slseanwu/MIDI-LLM_Llama-3.2-1B (Wu et al., ISMIR 2026), Llama 3.2 1B whose vocabulary is
extended by the Anticipatory Music Transformer's MIDI tokens: model token LLAMA_VOCAB + t is MIDI
token t. Graph: onnxruntime-genai's model builder (CPU, GroupQueryAttention), 4-bit weights:
    input_ids [1, n] int64, attention_mask [1, past + n] int64 (all ones),
    past_key_values.{i}.key/value [1, 8, past, 64] float32  (i = 0..15)
 -> logits [1, n, 55030] float32 (the LM head keeps only the MIDI vocabulary: logit j is MIDI
    token j; export.py), present.{i}.key/value [1, 8, past + n, 64].

Steps (generate_transformers.py + anticipation.convert.events_to_midi):
 1. text = SYSTEM_PROMPT + prompt + " "; ids = tokenizer(text) (Llama 3, <|begin_of_text|> first)
    + [LLAMA_VOCAB + MIDI_BOS].  ("<|" in the prompt is written "< |" so no added token is named.)
 2. Decode one token at a time with the key/value cache. Logits of the last position, MIDI tokens
    only (indices LLAMA_VOCAB .. LLAMA_VOCAB + 55026). Position i of the output may only be a time
    (i % 3 == 0: 0..10000), a duration (1: 10000..11000) or a note (2: 11000..27512); the
    reference script samples among all 55026 and can produce triples that don't decode.
 3. Sample: softmax(logits / temperature) (1.0), keep the most probable tokens until their mass
    reaches top_p (0.98), draw. Stop at max_tokens (2046) or at the first onset >= seconds * 100.
 4. Decode triples: start = time / 100 s, end = start + max(dur, 1) / 100 s (cut at `seconds`),
    note = instrument * 128 + pitch, instrument 0..127 = General MIDI program, 128 = drums,
    velocity 72. One part per instrument, ascending, drums last.
"""

from __future__ import annotations

import numpy as np
import onnxruntime as ort

SYSTEM_PROMPT = "You are a world-class composer. Please compose some music according to the following description: "
LLAMA_VOCAB = 128256
MIDI_BOS = 55026
MIDI_TOKENS = 55026
MAX_TIME, MAX_DUR, MAX_NOTE = 10000, 1000, 128 * 129
DUR_OFFSET = MAX_TIME
NOTE_OFFSET = DUR_OFFSET + MAX_DUR
DRUMS = 128
VELOCITY = 72
LAYERS, KV_HEADS, HEAD_DIM = 16, 8, 64


def make_session(onnx_path, threads: int | None = None):
    so = ort.SessionOptions()
    if threads:
        so.intra_op_num_threads = threads
    return ort.InferenceSession(str(onnx_path), so, providers=["CPUExecutionProvider"])


def prompt_text(prompt: str) -> str:
    return SYSTEM_PROMPT + prompt.strip().replace("<|", "< |") + " "


def prompt_ids(tokenizer, prompt: str) -> list[int]:
    """`tokenizer` is a `tokenizers.Tokenizer` loaded from the model's tokenizer.json."""
    return tokenizer.encode(prompt_text(prompt)).ids + [LLAMA_VOCAB + MIDI_BOS]


def allowed(i: int) -> tuple[int, int]:
    return [(0, MAX_TIME), (DUR_OFFSET, DUR_OFFSET + MAX_DUR), (NOTE_OFFSET, NOTE_OFFSET + MAX_NOTE)][i % 3]


class Decoder:
    """The ONNX decoder with its key/value cache."""

    def __init__(self, sess):
        self.sess = sess
        self.past = {}
        for i in range(LAYERS):
            for kind in ("key", "value"):
                self.past[f"past_key_values.{i}.{kind}"] = np.zeros((1, KV_HEADS, 0, HEAD_DIM), np.float32)
        self.len = 0

    def step(self, ids) -> np.ndarray:
        """Feed `ids`; return the last position's logits over the MIDI tokens [MIDI_TOKENS]."""
        ids = np.asarray(ids, np.int64)[None, :]
        total = self.len + ids.shape[1]
        feeds = {"input_ids": ids, "attention_mask": np.ones((1, total), np.int64), **self.past}
        names = [o.name for o in self.sess.get_outputs()]
        outs = dict(zip(names, self.sess.run(names, feeds)))
        for i in range(LAYERS):
            for kind in ("key", "value"):
                self.past[f"past_key_values.{i}.{kind}"] = outs[f"present.{i}.{kind}"]
        self.len = total
        logits = outs["logits"][0, -1]
        start = LLAMA_VOCAB if logits.shape[0] >= LLAMA_VOCAB + MIDI_TOKENS else 0  # full head or MIDI only
        return logits[start:start + MIDI_TOKENS].astype(np.float32)


def sample(logits, lo_hi, temperature, top_p, rng) -> int:
    lo, hi = lo_hi
    x = logits[lo:hi].astype(np.float64)
    x = np.exp((x - x.max()) / max(temperature, 1e-3))
    order = np.lexsort((np.arange(len(x)), -x))  # descending, ties by id
    p = x[order]
    cum = np.cumsum(p)
    n = int(np.searchsorted(cum, top_p * cum[-1]) + 1)
    kept, kept_ids = p[:n], order[:n]
    return lo + int(kept_ids[rng.choice(n, p=kept / kept.sum())])


def generate(sess, tokenizer, prompt: str, seconds=30.0, temperature=1.0, top_p=0.98, max_tokens=2046, seed=0):
    rng = np.random.default_rng(seed)
    dec = Decoder(sess)
    logits = dec.step(prompt_ids(tokenizer, prompt))
    limit = round(seconds * 100)
    out = []
    while len(out) < max_tokens:
        i = len(out)
        tok = sample(logits, allowed(i), temperature, top_p, rng)
        if i % 3 == 0 and tok >= limit:
            break
        out.append(tok)
        if len(out) >= max_tokens:
            break
        logits = dec.step([LLAMA_VOCAB + tok])
    return out[: len(out) - len(out) % 3]


def teacher_forced_logits(sess, ids: list[int], tokens: list[int]):
    """Logits (MIDI tokens) after the prompt and after each of `tokens`: [len(tokens) + 1, MIDI_TOKENS]."""
    dec = Decoder(sess)
    rows = [dec.step(ids)]
    for t in tokens:
        rows.append(dec.step([LLAMA_VOCAB + t]))
    return np.stack(rows)


def decode(tokens, seconds=100.0):
    """[(instrument, drums, [{start, end, pitch, velocity}])], instrument ascending (drums last)."""
    parts: dict[int, list] = {}
    for j in range(0, len(tokens) - len(tokens) % 3, 3):
        time, dur, note = tokens[j:j + 3]
        if not (0 <= time < MAX_TIME and DUR_OFFSET <= dur < DUR_OFFSET + MAX_DUR and NOTE_OFFSET <= note < NOTE_OFFSET + MAX_NOTE):
            continue
        note -= NOTE_OFFSET
        start = time / 100.0
        if start >= seconds:
            continue
        end = min(start + max(dur - DUR_OFFSET, 1) / 100.0, seconds)
        parts.setdefault(note // 128, []).append({"start": start, "end": end, "pitch": note % 128, "velocity": VELOCITY})
    out = []
    for instrument in sorted(parts):
        notes = sorted(parts[instrument], key=lambda n: (n["start"], n["pitch"]))
        out.append((instrument, instrument == DRUMS, notes))
    return out
