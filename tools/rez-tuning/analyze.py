#!/usr/bin/env python3
"""Compare a processed export with the Rez test signal, segment by segment.

    python3 tools/rez-tuning/analyze.py <processed.wav> [more.wav ...]

For each segment: the overall level change, and at each resonance listed for it the cut in dB
(third-octave band level, input minus output). Timing segments also get the time to reach 90 %
of the final cut after a burst starts and to fall back to 10 % after it ends.
The export must be sample-aligned with the original (same length); a constant offset is found
from the sync impulse and removed.
"""
import json, pathlib, sys, wave
import numpy as np

HERE = pathlib.Path(__file__).resolve().parent

def read(path):
    with wave.open(str(path), "rb") as w:
        n, ch, width, sr = w.getnframes(), w.getnchannels(), w.getsampwidth(), w.getframerate()
        raw = w.readframes(n)
    if width == 3:
        a = np.frombuffer(raw, dtype=np.uint8).reshape(-1, 3)
        x = (a[:, 0].astype(np.int32) | (a[:, 1].astype(np.int32) << 8) | (a[:, 2].astype(np.int8).astype(np.int32) << 16)) / 2 ** 23
    elif width == 2:
        x = np.frombuffer(raw, dtype="<i2") / 2 ** 15
    else:
        x = np.frombuffer(raw, dtype="<i4") / 2 ** 31
    return sr, x.reshape(-1, ch)[:, 0].astype(np.float64)

def read_any(path):
    """`wave` refuses float files: read those by hand (44-byte header, as plugin_render writes)."""
    try:
        return read(path)
    except wave.Error:
        b = pathlib.Path(path).read_bytes()
        at = b.find(b"data")
        ch = int.from_bytes(b[22:24], "little"); sr = int.from_bytes(b[24:28], "little")
        x = np.frombuffer(b[at + 8:], dtype="<f4")
        return sr, x[: len(x) // ch * ch].reshape(-1, ch)[:, 0].astype(np.float64)

def band_db(x, sr, f0, width_oct=1 / 3):
    spec = np.abs(np.fft.rfft(x * np.hanning(len(x)))) ** 2
    f = np.fft.rfftfreq(len(x), 1 / sr)
    sel = (f >= f0 * 2 ** (-width_oct / 2)) & (f <= f0 * 2 ** (width_oct / 2))
    return 10 * np.log10(spec[sel].sum() + 1e-30)

def rms_db(x):
    return 10 * np.log10(np.mean(x ** 2) + 1e-30)

def envelope_db(x, sr, f0, win=0.004):
    """Level of `x` near f0 over time (heterodyne and a short moving average), in dB."""
    t = np.arange(len(x)) / sr
    z = x * np.exp(-2j * np.pi * f0 * t)
    k = max(1, int(win * sr))
    z = np.convolve(z, np.ones(k) / k, mode="same")
    return 20 * np.log10(np.abs(z) + 1e-12)

def main():
    segs = json.load(open(HERE / "rez_test_segments.json"))
    sr, dry = read_any(HERE / "rez_test_48k_24bit.wav")
    for path in sys.argv[1:]:
        sr2, wet = read_any(path)
        assert sr2 == sr, f"{path}: {sr2} Hz, expected {sr}"
        s0 = next(s for s in segs["segments"] if s["name"] == "sync_impulse_start")
        a = int(s0["start_s"] * sr)
        offset = int(np.argmax(np.abs(wet[: a + sr]))) - a
        if offset > 0:
            wet = wet[offset:]
        elif offset < 0:
            wet = np.concatenate([np.zeros(-offset), wet])
        n = min(len(dry), len(wet))
        print(f"\n== {path}  (offset {offset} samples)")
        print(f"{'segment':26} {'level':>7}   cuts at the listed resonances (dB)")
        for s in segs["segments"]:
            a, b = int(s["start_s"] * sr), int((s["start_s"] + s["dur_s"]) * sr)
            if b > n or s["name"].startswith("sync"):
                continue
            # Skip the first 0.75 s so the reading is the settled one.
            skip = min(int(0.75 * sr), (b - a) // 2)
            d, w = dry[a + skip:b], wet[a + skip:b]
            line = f"{s['name']:26} {rms_db(w) - rms_db(d):+7.2f}  "
            for (f0, gain, q) in s.get("peaks", [])[:12]:
                line += f" {f0:g}Hz:{band_db(d, sr, f0, 1 / 6) - band_db(w, sr, f0, 1 / 6):5.1f}"
            if "burst_starts_rel_s" in s:
                f0 = s["peaks"][0][0]
                d_all, w_all = dry[a:b], wet[a:b]
                gr = envelope_db(d_all, sr, f0, 0.01) - envelope_db(w_all, sr, f0, 0.01)
                on = int(s["burst_len_ms"] * sr / 1000)
                rises, falls = [], []
                for st in s["burst_starts_rel_s"][1:-1]:
                    i = int(st * sr)
                    burst = gr[i:i + on]
                    final = np.median(burst[len(burst) // 2:])
                    if final < 1.0:
                        continue
                    up = np.argmax(burst >= 0.9 * final)
                    rises.append(up / sr * 1000)
                line += f"   90% of the cut after {np.median(rises):.0f} ms" if rises else "   (no settled cut)"
            print(line)

if __name__ == "__main__":
    main()
