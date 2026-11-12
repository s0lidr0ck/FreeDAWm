#!/usr/bin/env python3
"""Side-by-side table of the cuts in several processed copies of the Rez test signal.

    python3 tools/rez-tuning/compare.py label=file.wav [label=file.wav ...]

One row per segment and resonance, one column per file: the cut in dB in a sixth-octave band at
the resonance (input minus output, the first 0.75 s of the segment skipped); segments without a
listed resonance show the overall level change instead.
"""
import json, sys
import numpy as np
from analyze import HERE, band_db, read_any, rms_db

def main():
    segs = json.load(open(HERE / "rez_test_segments.json"))["segments"]
    sr, dry = read_any(HERE / "rez_test_48k_24bit.wav")
    cols = []
    for arg in sys.argv[1:]:
        label, path = arg.split("=", 1)
        _, wet = read_any(path)
        cols.append((label, wet[: len(dry)]))
    print(f"{'segment':22} {'at':>8} " + " ".join(f"{l:>9}" for l, _ in cols))
    for s in segs:
        if s["name"].startswith("sync"):
            continue
        a, b = int(s["start_s"] * sr), int((s["start_s"] + s["dur_s"]) * sr)
        skip = min(int(0.75 * sr), (b - a) // 2)
        peaks = s.get("peaks", [])
        if len(peaks) > 4:
            peaks = peaks[::3]
        rows = [(f"{p[0]:g}Hz", p[0]) for p in peaks] or [("level", None)]
        for tag, f0 in rows:
            vals = []
            for _, wet in cols:
                d, w = dry[a + skip:b], wet[a + skip:b]
                vals.append(rms_db(d) - rms_db(w) if f0 is None else band_db(d, sr, f0, 1 / 6) - band_db(w, sr, f0, 1 / 6))
            print(f"{s['name']:22} {tag:>8} " + " ".join(f"{v:9.2f}" for v in vals))

if __name__ == "__main__":
    main()
