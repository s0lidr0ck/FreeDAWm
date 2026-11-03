"""Fetch the few files of ZFTurbo/Music-Source-Separation-Training (MIT) the MDX23C, Mel-Band
RoFormer and BS-RoFormer exports need, at a pinned commit, into <work>/dl/mss (not committed here).
They provide the original PyTorch model classes and the original `demix` chunking used as the
parity reference."""

from __future__ import annotations

import sys

import requests

from common import WORK

COMMIT = "6fdad13686bb7e84d029ec66f9dbe76e65c93ebf"
FILES = [
    "models/mdx23c_tfc_tdf_v3.py",
    "models/bs_roformer/mel_band_roformer.py",
    "models/bs_roformer/bs_roformer.py",
    "models/bs_roformer/attend.py",
    "utils/model_utils.py",
]
MSS = WORK / "dl" / "mss"


def fetch() -> str:
    for f in FILES:
        dst = MSS / f
        if dst.exists():
            continue
        dst.parent.mkdir(parents=True, exist_ok=True)
        url = f"https://raw.githubusercontent.com/ZFTurbo/Music-Source-Separation-Training/{COMMIT}/{f}"
        r = requests.get(url, timeout=60)
        r.raise_for_status()
        dst.write_bytes(r.content)
    for pkg in ("models", "models/bs_roformer", "utils"):
        (MSS / pkg / "__init__.py").write_text("", encoding="utf-8")  # no eager sub-imports
    if str(MSS) not in sys.path:
        sys.path.insert(0, str(MSS))
    return str(MSS)


if __name__ == "__main__":
    print(fetch())
