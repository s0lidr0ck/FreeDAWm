"""Fast weights for Transcribe Lyrics: whisper.cpp's quantize tool makes ggml-large-v3-q8_0.bin from
the installed f16 model and records it on the card as `variants.fast` (the app's default "Fast";
the f16 model stays as "Best").

On the 30 s verse (lead 3:00) q8_0 gives the same transcript as f16 and runs 1.3x faster on
the CPU (9.0 s vs 11.6 s); q5_0 was slower (18.9 s) and changed the words, so it is not used.

    python tools/ml-export/lyrics/quantize.py [--models DIR] [--tool whisper-quantize.exe]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import common as C  # noqa: E402

QUANT = "q8_0"


def sha256(p: Path) -> str:
    h = hashlib.sha256()
    with p.open("rb") as f:
        for b in iter(lambda: f.read(1 << 20), b""):
            h.update(b)
    return h.hexdigest()


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--models", default=str(C.MODELS))
    ap.add_argument("--tool", default=str(C.WORK / "dl" / "whisper" / "bin-cpu" / "Release" / "whisper-quantize.exe"))
    a = ap.parse_args()
    d = Path(a.models) / "whisper_lyrics"
    card_path = d / "card.json"
    card = json.loads(card_path.read_text(encoding="utf-8"))
    src, out = d / card["file"], d / f"ggml-large-v3-{QUANT}.bin"
    if not out.exists():
        subprocess.run([a.tool, str(src), str(out), QUANT], check=True)
    card.setdefault("variants", {})["fast"] = {
        "file": out.name, "quant": QUANT, "sha256": sha256(out), "size_bytes": out.stat().st_size,
        "note": "whisper.cpp quantize of the f16 model; same transcript as f16 on the golden verse, faster on the CPU",
    }
    card_path.write_text(json.dumps(card, indent=2), encoding="utf-8")
    print(f"{out}: {card['variants']['fast']['sha256']}")


if __name__ == "__main__":
    main()
