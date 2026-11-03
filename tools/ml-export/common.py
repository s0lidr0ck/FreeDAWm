"""Shared helpers for the ML export scripts (paths, WAV I/O, metrics, model cards, install).

Only the export / parity / evaluation scripts use this module. Each model's
`reference_infer.py` is self-contained on purpose (numpy + onnxruntime only),
because it is the spec the Rust lane ports.
"""

from __future__ import annotations

import datetime as _dt
import hashlib
import json
import os
import shutil
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.signal import resample_poly

# ---------------------------------------------------------------- paths
# The folder next to the checkout (outside the repo) unless ML_DAW_ROOT names one.
DAW = Path(os.environ.get("ML_DAW_ROOT") or Path(__file__).resolve().parents[3])
WORK = Path(os.environ.get("ML_WORK_DIR", DAW / "ml-export-work"))
MODELS = Path(os.environ.get("ML_MODELS_DIR", DAW / "ml-models"))
RENDERS = Path(os.environ.get("ML_RENDERS_DIR", DAW / "ml-renders"))


class MediaDir:
    """ML_MEDIA_DIR, the real-song stems for the listening checks (no default). The scripts name
    the stems by role: `lead.wav` and `backing1.wav` .. `backing5.wav`. An optional `stems.json`
    in that folder maps each role's name to the real file (`{"lead.wav": "Take 3 vox.wav"}`), so
    no singer's name has to appear in the scripts."""

    def __init__(self, root: str | None):
        self.root = Path(root) if root else None
        self.names: dict = {}
        if self.root is not None and (self.root / "stems.json").is_file():
            import json

            self.names = json.loads((self.root / "stems.json").read_text(encoding="utf-8"))

    def __truediv__(self, name):
        if self.root is None:
            raise SystemExit(f"set ML_MEDIA_DIR to the folder holding {name} (real-song test media; no default)")
        return self.root / self.names.get(str(name), str(name))

    def __str__(self):
        return str(self.root) if self.root is not None else "(ML_MEDIA_DIR unset)"


MEDIA = MediaDir(os.environ.get("ML_MEDIA_DIR"))
TESTMIX = WORK / "testmix"  # synthetic mix + true stems (make_testmix.py)

for _p in (WORK, MODELS, RENDERS):
    _p.mkdir(parents=True, exist_ok=True)


# ---------------------------------------------------------------- audio I/O
def read_wav(path, start_s: float | None = None, dur_s: float | None = None):
    """Return (float32 array [channels, samples], samplerate)."""
    info = sf.info(str(path))
    sr = info.samplerate
    start = int(round((start_s or 0.0) * sr))
    frames = -1 if dur_s is None else int(round(dur_s * sr))
    x, sr = sf.read(str(path), start=start, frames=frames, dtype="float64", always_2d=True)
    return x.T.astype(np.float32), sr


def write_wav24(path, x: np.ndarray, sr: int):
    """x: [channels, samples] float. Written as 24-bit PCM (clipped to +-1)."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    y = np.clip(np.asarray(x, dtype=np.float64), -1.0, 1.0 - 2.0**-23)
    sf.write(str(path), y.T, sr, subtype="PCM_24")


def write_wav_f32(path, x: np.ndarray, sr: int):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    sf.write(str(path), np.asarray(x, dtype=np.float32).T, sr, subtype="FLOAT")


def resample(x: np.ndarray, sr_in: int, sr_out: int) -> np.ndarray:
    """Polyphase resampling along the last axis (scipy resample_poly, Kaiser 5.0)."""
    if sr_in == sr_out:
        return x.astype(np.float32)
    g = np.gcd(sr_in, sr_out)
    return resample_poly(x.astype(np.float64), sr_out // g, sr_in // g, axis=-1).astype(np.float32)


# ---------------------------------------------------------------- metrics
def snr_db(ref: np.ndarray, est: np.ndarray) -> float:
    """Parity SNR: 10 log10(|ref|^2 / |ref - est|^2)."""
    ref = np.asarray(ref, np.float64)
    err = ref - np.asarray(est, np.float64)
    num = np.sum(ref**2)
    den = np.sum(err**2)
    if den == 0:
        return float("inf")
    return float(10 * np.log10(max(num, 1e-30) / den))


def sdr_db(ref: np.ndarray, est: np.ndarray) -> float:
    """Separation SDR as used by the MDX/MVSEP challenges ('new' SDR, no projection)."""
    ref = np.asarray(ref, np.float64)
    est = np.asarray(est, np.float64)
    return float(10 * np.log10((np.sum(ref**2) + 1e-8) / (np.sum((ref - est) ** 2) + 1e-8)))


def max_abs(a, b) -> float:
    return float(np.max(np.abs(np.asarray(a, np.float64) - np.asarray(b, np.float64))))


# ---------------------------------------------------------------- files
def sha256(path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for blk in iter(lambda: f.read(1 << 20), b""):
            h.update(blk)
    return h.hexdigest()


def write_f32(path, arr: np.ndarray):
    """Raw little-endian float32, C order (the layout is described in golden/README.txt)."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    np.ascontiguousarray(arr, dtype="<f4").tofile(path)


def card_is_current(model_id: str) -> bool:
    """True when ml-models/<id>/card.json exists and its sha256 matches the installed .onnx."""
    card_path = MODELS / model_id / "card.json"
    if not card_path.exists():
        return False
    card = json.loads(card_path.read_text(encoding="utf-8"))
    files = [card["file"]] + [f["file"] for f in card.get("extra_files", [])]
    shas = [card["sha256"]] + [f["sha256"] for f in card.get("extra_files", [])]
    for f, s in zip(files, shas):
        p = MODELS / model_id / f
        if not p.exists() or sha256(p) != s:
            return False
    return (MODELS / model_id / "golden" / "README.txt").exists()


def install(model_id: str, onnx_path, card: dict, golden_src: Path | None = None):
    """Copy the .onnx (+ golden dir) into ml-models/<id>/ and write card.json."""
    dst = MODELS / model_id
    dst.mkdir(parents=True, exist_ok=True)
    onnx_path = Path(onnx_path)
    target = dst / card["file"]
    if onnx_path.resolve() != target.resolve():
        shutil.copy2(onnx_path, target)
    card["sha256"] = sha256(target)
    card["size_bytes"] = target.stat().st_size
    if golden_src is not None and Path(golden_src).resolve() != (dst / "golden").resolve():
        if (dst / "golden").exists():
            shutil.rmtree(dst / "golden")
        shutil.copytree(golden_src, dst / "golden")
    card["exported_utc"] = _dt.datetime.now(_dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    (dst / "card.json").write_text(json.dumps(card, indent=2) + "\n", encoding="utf-8")
    return card


def mark_ready(model_id: str):
    ts = _dt.datetime.now().astimezone().strftime("%Y-%m-%dT%H:%M:%S%z")
    with open(MODELS / "READY.txt", "a", encoding="utf-8") as f:
        f.write(f"{model_id} ready {ts}\n")


def download(url: str, dst: Path) -> Path:
    """Download url to dst once (skips when present)."""
    import requests

    dst = Path(dst)
    if dst.exists() and dst.stat().st_size > 0:
        return dst
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp = dst.with_suffix(dst.suffix + ".part")
    with requests.get(url, stream=True, timeout=60) as r:
        r.raise_for_status()
        with open(tmp, "wb") as f:
            for blk in r.iter_content(1 << 20):
                f.write(blk)
    tmp.replace(dst)
    return dst


def save_json(path, obj):
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text(json.dumps(obj, indent=2) + "\n", encoding="utf-8")
