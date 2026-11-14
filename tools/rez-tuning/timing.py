#!/usr/bin/env python3
"""How fast the cut comes and goes in processed copies of the Rez test signal.

    python3 tools/rez-tuning/timing.py label=file.wav [label=file.wav ...]

From the `timing_on_1000ms` segment (a 2 kHz resonance switched on for a second, off for a
second): the cut at the resonance over time, averaged over the bursts, and from it the time to
reach half and 90 % of the settled cut after the resonance starts and to fall back to half and
10 % of it after it stops.
"""
import json, sys
import numpy as np
from analyze import HERE, envelope_db, read_any

def main():
    segs = json.load(open(HERE / "rez_test_segments.json"))["segments"]
    s = next(x for x in segs if x["name"] == "timing_on_1000ms")
    sr, dry = read_any(HERE / "rez_test_48k_24bit.wav")
    a, b = int(s["start_s"] * sr), int((s["start_s"] + s["dur_s"]) * sr)
    f0, on = s["peaks"][0][0], int(s["burst_len_ms"] * sr / 1000)
    print(f"{'file':14} {'settled':>8} {'rise 50%':>9} {'rise 90%':>9} {'fall 50%':>9} {'fall 10%':>9}   (dB, then ms)")
    for arg in sys.argv[1:]:
        label, path = arg.split("=", 1)
        _, wet = read_any(path)
        # The cut is read where there is something to cut: while the burst is on, from the
        # resonance; after it, from the noise still passing at that frequency.
        gr = envelope_db(dry[a:b], sr, f0, 0.02) - envelope_db(wet[a:b], sr, f0, 0.02)
        k = int(0.01 * sr)
        gr = np.convolve(gr, np.ones(k) / k, mode="same")
        starts = [int(t * sr) for t in s["burst_starts_rel_s"]][:-1]
        span = on + int(0.9 * sr)
        mean = np.mean([gr[i - int(0.05 * sr): i + span] for i in starts if i + span < len(gr)], axis=0)
        t0 = int(0.05 * sr)
        settled = float(np.median(mean[t0 + on // 2: t0 + on]))
        rise = mean[t0: t0 + on]
        fall = mean[t0 + on:]
        first = lambda x, cond: (np.argmax(cond(x)) / sr * 1000) if cond(x).any() else float("nan")
        print(f"{label:14} {settled:8.2f} {first(rise, lambda x: x >= 0.5 * settled):9.0f} {first(rise, lambda x: x >= 0.9 * settled):9.0f}"
              f" {first(fall, lambda x: x <= 0.5 * settled):9.0f} {first(fall, lambda x: x <= 0.1 * settled):9.0f}")

if __name__ == "__main__":
    main()
