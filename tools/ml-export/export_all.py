"""Export, check and install every ML model, idempotently. Entry point for `cargo xtask ml-models`.

    <venv>/python tools/ml-export/export_all.py [--force] [ids...]

Order (priority): htdemucs, drumsep_mdx23c, srs_small, karaoke_melroformer, vocals_melroformer,
bs_roformer_sw, then the audio-to-MIDI
models basicpitch, piano_kong, adtof_drums, game_vocal, then btc_chords (chords) and whisper_lyrics
(Whisper large-v3 via whisper.cpp; downloads the whisper.cpp b5454 Windows binaries), then
midi_llm (MIDI-LLM, text to MIDI) (Basic Pitch's TF parity needs the extra venv
<DAW>/ml-venv-tf with tensorflow-cpu; it is skipped with a note when that venv is missing). A model whose
installed card.json sha256 matches its .onnx (and has golden/) is skipped. Each model runs in its
own subprocess (the per-model folders reuse module names). Steps per model: download weights,
export ONNX, parity vs the PyTorch original, golden files, quality numbers, 24-bit renders,
install into <models>/<id>/ and append "<id> ready <timestamp>" to <models>/READY.txt.

Paths (env overrides): ML_DAW_ROOT, ML_WORK_DIR, ML_MODELS_DIR, ML_RENDERS_DIR, ML_MEDIA_DIR.
"""

from __future__ import annotations

import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import common as C  # noqa: E402

MODELS = [
    ("htdemucs", "htdemucs"),
    ("drumsep_mdx23c", "drumsep"),
    ("srs_small", "srs"),
    ("karaoke_melroformer", "karaoke"),
    ("vocals_melroformer", "melband_vocals"),
    ("bs_roformer_sw", "bs_roformer_sw"),
    # audio to MIDI
    ("basicpitch", "basicpitch"),
    ("piano_kong", "pianokong"),
    ("adtof_drums", "adtof"),
    ("game_vocal", "game"),
    # chords and lyrics
    ("btc_chords", "btc"),
    ("whisper_lyrics", "lyrics"),
    # text to MIDI
    ("midi_llm", "midillm"),
]


def main(argv):
    force = "--force" in argv
    ids = [a for a in argv if not a.startswith("--")] or [m for m, _ in MODELS]
    if not (C.TESTMIX / "mix.wav").exists():
        subprocess.run([sys.executable, str(HERE / "make_testmix.py")], check=True, cwd=HERE)
    failed = []
    for mid, folder in MODELS:
        if mid not in ids:
            continue
        if not force and C.card_is_current(mid):
            print(f"[{mid}] up to date (card sha256 matches), skipped")
            continue
        print(f"[{mid}] exporting ...", flush=True)
        t0 = time.time()
        cmd = [sys.executable, str(HERE / folder / "run.py")] + (["--force"] if force else [])
        r = subprocess.run(cmd, cwd=HERE)
        if r.returncode != 0:
            failed.append(mid)
            print(f"[{mid}] FAILED (exit {r.returncode})")
        else:
            print(f"[{mid}] done in {time.time() - t0:.0f} s")
    if failed:
        print("failed:", ", ".join(failed))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
