"""Lyrics transcription reference pipeline: Whisper large-v3 through whisper.cpp (the runtime the
Rust side uses via the whisper-rs crate). THIS IS THE SPEC THE RUST LANE PORTS.

Model: ggml-large-v3.bin (OpenAI Whisper large-v3, MIT, converted by ggml-org). Runtime:
whisper.cpp b5454 (= v1.9.5); CUDA build for NVIDIA, CPU build otherwise (whisper-rs: features
"cuda" or "vulkan").

1. Host: mono (mean), resample to 16000 Hz f32. No VAD: Silero VAD (whisper.cpp --vad) rejects
   sung vocals (it kept 0.8 s of a 30 s sung clip), so the whole input goes to whisper.cpp, which
   windows it in 30 s chunks itself. Feed one vocal mic or a separated vocal stem, not a mix.
2. whisper_full with (whisper-rs names in brackets):
   context: flash_attn = false, dtw_token_timestamps = true, dtw_aheads_preset = LARGE_V3
            [WhisperContextParameters { flash_attn: false, dtw_parameters: DtwParameters { mode:
            DtwMode::ModelPreset { model_preset: DtwModelPreset::LargeV3 }, .. } }]
   full:    strategy BEAM_SEARCH beam_size 5, patience -1  [SamplingStrategy::BeamSearch {beam_size: 5, patience: -1.0}]
            language "en", translate false, no_context true (= -mc 0, condition_on_previous_text
            off: stops repetition loops on songs), n_max_text_ctx 0,
            temperature 0, temperature_inc 0 (= no fallback: deterministic), token_timestamps true,
            thold_pt 0.01, entropy_thold 2.4, logprob_thold -1.0, no_speech_thold 0.6,
            suppress_nst false, single_segment false, initial prompt none.
   CLI equivalent: whisper-cli -m ggml-large-v3.bin -f in.wav -l en -mc 0 -bs 5 -nf -tp 0
                   --dtw large.v3 -nfa -ojf
3. Words from the tokens of each segment: drop special tokens (id >= 50257: [_BEG_], [_TT_n], ...).
   A token whose text starts with a space starts a new word; other tokens (word pieces,
   punctuation) are appended to the current word. Word start = t_dtw of its first token / 100 s;
   word end = start of the next word in the same segment, or the segment end for the last word
   (and never before start + 0.05 s). prob = min token p. Drop words that are only "♪" / "*" /
   punctuation (music symbols Whisper emits on instrumental parts).
4. Hallucination guard (host): a word is kept only if the vocal input's RMS over [start, end]
   is >= GATE_DB below the input's 99th-percentile 50 ms RMS (the singer is actually singing).
5. Known Whisper hallucinations (training-data captions it produces on music or silence): a
   whisper.cpp segment whose text, lower-cased, contains any of HALLUCINATIONS is dropped whole.
Output: words [{start, end, text, prob}], segments [{start, end, text}] (text re-joined from
the kept words); times in seconds of the input.
"""

from __future__ import annotations

import json
import re
import subprocess
import tempfile
from pathlib import Path

import numpy as np
import soundfile as sf

SR = 16000
GATE_DB = 30.0
ARGS = ["-l", "en", "-mc", "0", "-bs", "5", "-nf", "-tp", "0", "--dtw", "large.v3", "-nfa", "-ojf"]


def args_for(model):
    """ARGS with the DTW alignment-head preset matching the model file (turbo has its own heads)."""
    a = list(ARGS)
    if "turbo" in Path(model).name:
        a[a.index("large.v3")] = "large.v3.turbo"
    return a
HALLUCINATIONS = ["transcript", "subtitle", "thank you for watching", "thanks for watching", "©", "amara.org",
                  "www.", "please subscribe", "like and subscribe"]
_JUNK = re.compile(r"^[\s♪*.,!?;:'\"()\[\]-]*$")


def run_whisper_cpp(cli, model, audio, gpu=True, threads=4):
    """audio mono f32 16 kHz -> whisper.cpp full JSON (dict)."""
    with tempfile.TemporaryDirectory() as td:
        wav = Path(td) / "in.wav"
        sf.write(str(wav), audio.astype(np.float32), SR, subtype="FLOAT")
        cmd = [str(cli), "-m", str(model), "-f", str(wav), "-t", str(threads), *args_for(model), "-of", str(Path(td) / "out")]
        if not gpu:
            cmd.append("-ng")
        subprocess.run(cmd, check=True, capture_output=True)
        return json.loads((Path(td) / "out.json").read_text(encoding="utf-8"))


def words_from_json(js):
    words = []
    for seg in js["transcription"]:
        if any(h in seg["text"].lower() for h in HALLUCINATIONS):
            continue
        seg_end = seg["offsets"]["to"] / 1000.0
        cur = []
        for tok in seg["tokens"]:
            if tok["id"] >= 50257:
                continue
            if tok["text"].startswith(" ") or not cur:
                cur.append({"tokens": [tok]})
            else:
                cur[-1]["tokens"].append(tok)
        for i, w in enumerate(cur):
            t0 = w["tokens"][0]["t_dtw"] / 100.0
            t1 = cur[i + 1]["tokens"][0]["t_dtw"] / 100.0 if i + 1 < len(cur) else seg_end
            text = "".join(t["text"] for t in w["tokens"]).strip()
            if _JUNK.match(text):
                continue
            words.append({"start": t0, "end": max(t1, t0 + 0.05), "text": text,
                          "prob": float(min(t["p"] for t in w["tokens"]))})
    return words


def gate(words, audio):
    hop = SR // 20
    n = len(audio) // hop
    if n == 0:
        return words
    rms = np.sqrt(np.mean(audio[: n * hop].reshape(n, hop) ** 2, axis=1)) + 1e-9
    ref = np.percentile(rms, 99)
    out = []
    for w in words:
        a, b = int(w["start"] * 20), max(int(w["start"] * 20) + 1, int(np.ceil(w["end"] * 20)))
        seg = rms[a:b]
        if len(seg) and 20 * np.log10(seg.max() / ref) >= -GATE_DB:
            out.append(w)
    return out


def segments_from(js, words):
    segs = []
    for seg in js["transcription"]:
        a, b = seg["offsets"]["from"] / 1000.0, seg["offsets"]["to"] / 1000.0
        ws = [w for w in words if a <= w["start"] < b]
        if ws:
            segs.append({"start": ws[0]["start"], "end": ws[-1]["end"], "text": " ".join(w["text"] for w in ws)})
    return segs


def transcribe(cli, model, audio, gpu=True):
    """mono f32 16 kHz -> (words, segments, raw whisper.cpp JSON)."""
    js = run_whisper_cpp(cli, model, audio, gpu=gpu)
    words = gate(words_from_json(js), audio)
    return words, segments_from(js, words), js
