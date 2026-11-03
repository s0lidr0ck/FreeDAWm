"""Lyrics: Whisper large-v3 via whisper.cpp. Fetch runtime + model -> determinism + CPU/CUDA check ->
cross-check against the original openai-whisper implementation -> golden -> transcribe the
isolated vocal mics (lead, backing4, backing1; 7:00-7:30 and 3:00-3:30) and the HTDemucs vocals of the
synthetic mix -> transcripts in ml-renders/lyrics -> install ml-models/whisper_lyrics/."""

from __future__ import annotations

import json
import sys
import time
import zipfile
from pathlib import Path

import numpy as np
import soundfile as sf

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE))

import common as C  # noqa: E402
import reference_infer as RI  # noqa: E402

ID = "whisper_lyrics"
BUILD = "b5454"
DL = C.WORK / "dl" / "whisper"
WORK = C.WORK / "lyrics"
OUT = C.RENDERS / "lyrics"
MODEL = DL / "ggml-large-v3.bin"
TURBO = DL / "ggml-large-v3-turbo.bin"
URLS = {
    "cuda.zip": f"https://github.com/ggml-org/whisper.cpp/releases/download/{BUILD}/whisper-bin-win-cuda-12.4.0-x64.zip",
    "cpu.zip": f"https://github.com/ggml-org/whisper.cpp/releases/download/{BUILD}/whisper-blas-bin-x64.zip",
    "ggml-large-v3.bin": "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin",
    "ggml-large-v3-turbo.bin": "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
}
CLIPS = [(s, t) for s in ("lead", "backing4", "backing1") for t in (420.0, 180.0)]


def fetch():
    for name, url in URLS.items():
        if name.endswith(".zip"):
            d = DL / ("bin-cuda" if name.startswith("cuda") else "bin-cpu")
            if not (d / "Release" / "whisper-cli.exe").exists():
                C.download(url, DL / name)
                zipfile.ZipFile(DL / name).extractall(d)
                (DL / name).unlink()
        else:
            C.download(url, DL / name)
    return DL / "bin-cuda" / "Release" / "whisper-cli.exe", DL / "bin-cpu" / "Release" / "whisper-cli.exe"


def clip(singer, start):
    x, sr = C.read_wav(C.MEDIA / f"{singer}.wav", start, 30.0)
    return C.resample(x[0], sr, RI.SR).astype(np.float32)


def sep_vocals():
    x, sr = sf.read(str(C.RENDERS / "htdemucs" / "vocals.wav"), dtype="float32")
    return C.resample(x.mean(1) if x.ndim == 2 else x, sr, RI.SR).astype(np.float32)


def fmt(t):
    return f"{int(t // 60)}:{t % 60:05.2f}"


def wer(ref, hyp):
    norm = lambda s: [w for w in "".join(ch.lower() if ch.isalnum() or ch == " " else " " for ch in s).split()]  # noqa: E731
    r, h = norm(ref), norm(hyp)
    d = np.arange(len(h) + 1)
    for i in range(1, len(r) + 1):
        prev, d[0] = d.copy(), i
        for j in range(1, len(h) + 1):
            d[j] = min(prev[j] + 1, d[j - 1] + 1, prev[j - 1] + (r[i - 1] != h[j - 1]))
    return float(d[len(h)] / max(1, len(r)))


def openai_whisper(audio):
    """The original implementation (openai-whisper, PyTorch CPU) with the same decoding choices."""
    import whisper

    m = whisper.load_model("large-v3", device="cpu", download_root=str(DL / "openai"))
    r = m.transcribe(audio, language="en", beam_size=5, temperature=0.0, condition_on_previous_text=False,
                     word_timestamps=True, fp16=False)
    words = [{"start": w["start"], "end": w["end"], "text": w["word"].strip()} for s in r["segments"] for w in s["words"]]
    return r["text"], words


def main(force=False):
    if not force and C.card_is_current(ID):
        print(f"{ID}: installed card matches, skipping")
        return None
    cli_gpu, cli_cpu = fetch()
    WORK.mkdir(parents=True, exist_ok=True)
    OUT.mkdir(parents=True, exist_ok=True)
    report = {"id": ID}

    # determinism and CPU vs CUDA on lead 7:00-7:30
    a = clip("lead", 420.0)
    w1, s1, j1 = RI.transcribe(cli_gpu, MODEL, a)
    w2, s2, j2 = RI.transcribe(cli_gpu, MODEL, a)
    t0 = time.perf_counter()
    wc, sc, jc = RI.transcribe(cli_cpu, MODEL, a, gpu=False)
    t_cpu = time.perf_counter() - t0
    txt = lambda ws: " ".join(w["text"] for w in ws)  # noqa: E731
    report["determinism"] = {
        "cuda_run1_eq_run2": j1["transcription"] == j2["transcription"],
        "cpu_vs_cuda_text_wer": wer(txt(wc), txt(w1)),
        "cpu_vs_cuda_max_word_start_diff_s": (max(abs(x["start"] - y["start"]) for x, y in zip(wc, w1))
                                              if len(wc) == len(w1) else None),
        "cpu_time_30s": round(t_cpu, 1),
        "cpu_text": txt(wc), "cuda_text": txt(w1),
    }
    print("determinism", report["determinism"], flush=True)

    # cross-check vs openai-whisper (original implementation), CPU
    t0 = time.perf_counter()
    otext, owords = openai_whisper(a)
    report["vs_openai_whisper"] = {"text_openai": otext.strip(), "text_whisper_cpp": txt(w1),
                                   "wer_between": wer(otext, txt(w1)), "openai_cpu_time_s": round(time.perf_counter() - t0, 1)}
    print("vs_openai", report["vs_openai_whisper"], flush=True)

    # golden: 10 s of lead 3:00-3:10 (clear sung words), CPU build (bit-reproducible run to run)
    gold = WORK / "golden"
    g = clip("lead", 180.0)[: 10 * RI.SR]
    gw, gs, gj = RI.transcribe(cli_cpu, MODEL, g, gpu=False)
    C.write_f32(gold / "input_10s.f32", g)
    C.save_json(gold / "whisper_cpp_full_10s.json", gj)
    C.save_json(gold / "words_10s.json", {"words": gw, "segments": gs})
    (gold / "README.txt").write_text(
        "Golden files for whisper_lyrics (reference_infer.py driving whisper.cpp b5454 CPU build).\n"
        f"input_10s.f32            : [{len(g)}] mono 16 kHz raw little-endian float32 (lead.wav 3:00-3:10)\n"
        "whisper_cpp_full_10s.json: whisper-cli -ojf output (segments, tokens with id, p, t_dtw)\n"
        "words_10s.json           : words {start, end, text, prob} + segments after step 3-4 of the spec\n"
        "Rust (whisper-rs) with the same parameters should give the same tokens on CPU; CUDA may differ\n"
        "in a word or two (see card).\n", encoding="utf-8")

    # transcripts
    items = [(f"{s}_{int(t) // 60}m{int(t) % 60:02d}", clip(s, t), t) for s, t in CLIPS]
    items.append(("mix_htdemucs_vocals_7m00", sep_vocals(), 420.0))
    res, lines = {}, []
    for name, audio, off in items:
        t0 = time.perf_counter()
        words, segs, js = RI.transcribe(cli_gpu, MODEL, audio)
        dt = time.perf_counter() - t0
        tw, _, _ = RI.transcribe(cli_gpu, TURBO, audio)
        raw_words = RI.words_from_json(js)
        raw_text = " ".join(t["text"].strip() for t in js["transcription"])
        res[name] = {"words": len(words), "gated_out": len(raw_words) - len(words), "raw_whisper_text": raw_text, "cuda_s": round(dt, 1),
                     "text": txt(words), "turbo_text": txt(tw), "turbo_vs_v3_wer": wer(txt(words), txt(tw))}
        C.save_json(OUT / f"{name}.json", {"song_offset_s": off, "words": words, "segments": segs,
                                           "turbo_words": tw})
        body = [f"# {name} (song time = clip time + {fmt(off)})", ""]
        for s in segs:
            body.append(f"[{fmt(off + s['start'])} - {fmt(off + s['end'])}] {s['text']}")
        body += ["", "words: " + " ".join(f"{w['text']}@{fmt(off + w['start'])}" for w in words), "",
                 "large-v3-turbo (comparison): " + txt(tw), ""]
        (OUT / f"{name}.txt").write_text("\n".join(body), encoding="utf-8")
        lines += body
        print("clip", name, {k: v for k, v in res[name].items()}, flush=True)
    (OUT / "ALL_TRANSCRIPTS.txt").write_text("\n".join(lines), encoding="utf-8")
    report["clips"] = res

    card = {
        "id": ID, "name": "Whisper large-v3 (OpenAI) via whisper.cpp - sung lyrics with word timestamps",
        "task": "lyrics_transcription", "role": "vocal mic / separated vocal -> words with times",
        "file": "ggml-large-v3.bin", "format": "ggml (whisper.cpp), not ONNX", "sample_rate": RI.SR, "channels": 1,
        "runtime": {"whisper_cpp": f"{BUILD} (v1.9.5)", "rust_crate": "whisper-rs (features cuda or vulkan)",
                    "binaries": {k: v for k, v in URLS.items() if k.endswith(".zip")}},
        "params": {"cli": " ".join(RI.ARGS), "beam_size": 5, "patience": -1, "language": "en", "no_context": True,
                   "temperature": 0.0, "temperature_inc": 0.0, "token_timestamps": True, "dtw_preset": "large.v3",
                   "flash_attn": False, "entropy_thold": 2.4, "logprob_thold": -1.0, "no_speech_thold": 0.6,
                   "vad": False, "gate_db": RI.GATE_DB, "hallucination_phrases": RI.HALLUCINATIONS},
        "source_url": URLS["ggml-large-v3.bin"] + " ; https://github.com/openai/whisper ; https://github.com/ggml-org/whisper.cpp",
        "licence": "MIT (Whisper weights and code; whisper.cpp MIT).",
        "dml_supported": False, "dml_note": "Not an ONNX model; whisper.cpp runs on CUDA (tested RTX 5060) or CPU, Vulkan in whisper-rs.",
        "reference": "tools/ml-export/lyrics/reference_infer.py", "golden": "golden/README.txt",
    }
    C.install(ID, MODEL, card, gold)
    C.mark_ready(ID)
    C.save_json(WORK / "report.json", report)
    return report


if __name__ == "__main__":
    main(force="--force" in sys.argv)
