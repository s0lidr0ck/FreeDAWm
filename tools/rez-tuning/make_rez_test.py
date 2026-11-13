#!/usr/bin/env python3
"""Render the Rez tuning test signal: rez_test_48k_24bit.wav plus rez_test_segments.json.

Deterministic (fixed seed). Mono content written to both channels of a stereo file.
Every segment is separated by 0.5 s of silence so release tails are visible.
"""
import json, pathlib, wave

HERE = pathlib.Path(__file__).resolve().parent
import numpy as np

SR = 48000
GAP = 0.5
rng = np.random.default_rng(20261112)

def db(x): return 10.0 ** (x / 20.0)

def bell(f, f0, gain_db, q):
    """Magnitude of an analog peaking EQ."""
    a = 10.0 ** (gain_db / 40.0)
    s = 1j * (f / f0)
    return np.abs((s * s + s * (a / q) + 1) / (s * s + s / (a * q) + 1))

def noise(dur, peaks=(), rms_db=-24.0, colour="pink"):
    """Pink (or white) noise at rms_db BEFORE the bells are added; returns (plain, peaked)."""
    n = int(dur * SR)
    spec = np.fft.rfft(rng.standard_normal(n))
    f = np.fft.rfftfreq(n, 1 / SR)
    shape = np.ones_like(f)
    if colour == "pink":
        shape[1:] = 1 / np.sqrt(f[1:])
    shape[f < 20] = 0
    plain = np.fft.irfft(spec * shape, n)
    g = db(rms_db) / np.sqrt(np.mean(plain ** 2))
    eq = np.ones_like(f)
    for (f0, gdb, q) in peaks:
        eq *= bell(np.maximum(f, 1e-9), f0, gdb, q)
    peaked = np.fft.irfft(spec * shape * eq, n)
    return plain * g, peaked * g

def fade(x, ms=5.0):
    k = int(ms * SR / 1000)
    w = 0.5 - 0.5 * np.cos(np.pi * np.arange(k) / k)
    x = x.copy(); x[:k] *= w; x[-k:] *= w[::-1]
    return x

out, segs, t = [], [], 0.0
def add(name, x, **info):
    global t
    segs.append({"name": name, "start_s": round(t, 6), "dur_s": round(len(x) / SR, 6), **info})
    out.append(x); t += len(x) / SR
    out.append(np.zeros(int(GAP * SR))); t += GAP

def impulse():
    x = np.zeros(int(0.5 * SR)); x[0] = db(-6); return x

out.append(np.zeros(int(GAP * SR))); t += GAP
add("sync_impulse_start", impulse(), note="single sample at -6 dBFS, for alignment")

# 1. Baselines: nothing should stick out.
add("pink_plain", fade(noise(6, rms_db=-24)[0]), rms_db=-24)
add("white_plain", fade(noise(6, rms_db=-24, colour="white")[0]), rms_db=-24)

# 2. Four static resonances at once.
pk = [(250, 12, 10), (1000, 12, 10), (3000, 12, 10), (8000, 12, 10)]
add("pink_4_resonances", fade(noise(6, pk)[1]), peaks=pk, rms_db=-24)

# 3. Excess ladder: one resonance at 2 kHz, Q 10, rising boost.
for g in (3, 6, 9, 12, 18, 24):
    add(f"excess_{g}dB", fade(noise(3, [(2000, g, 10)])[1]), peaks=[(2000, g, 10)], rms_db=-24)

# 4. Width ladder: 12 dB at 2 kHz, rising Q.
for q in (1, 2, 5, 10, 30, 100):
    add(f"width_q{q}", fade(noise(3, [(2000, 12, q)])[1]), peaks=[(2000, 12, q)], rms_db=-24)

# 5. Frequency ladder: 12 dB, Q 10, one frequency at a time.
for f0 in (60, 120, 250, 500, 1000, 2000, 4000, 8000, 14000):
    add(f"freq_{f0}Hz", fade(noise(3, [(f0, 12, 10)])[1]), peaks=[(f0, 12, 10)], rms_db=-24)

# 6. Level independence: same material at four input levels.
for lvl in (-18, -30, -42, -54):
    add(f"level_{lvl}dB", fade(noise(3, pk, rms_db=lvl)[1]), peaks=pk, rms_db=lvl)

# 7. Timing: the 2 kHz resonance switches on and off over steady pink noise.
for on_ms in (1000, 250, 50, 10):
    plain, peaked = noise(6, [(2000, 15, 10)])
    extra = peaked - plain
    period = int(2 * on_ms * SR / 1000) if on_ms < 1000 else 2 * SR
    onlen = int(on_ms * SR / 1000)
    gate = np.zeros(len(plain)); starts = []
    for s in range(SR // 2, len(plain) - onlen, period):
        gate[s:s + onlen] = 1; starts.append(round(s / SR, 6))
    k = int(0.001 * SR)  # 1 ms edges
    gate = np.convolve(gate, np.ones(k) / k, mode="same")
    add(f"timing_on_{on_ms}ms", fade(plain + extra * gate), peaks=[(2000, 15, 10)],
        rms_db=-24, burst_starts_rel_s=starts, burst_len_ms=on_ms)

# 8. Moving resonance: log sine sweep over quiet pink noise, then alone.
def sweep(dur, f1=40.0, f2=16000.0, amp_db=-18.0):
    n = int(dur * SR); tt = np.arange(n) / SR
    k = np.log(f2 / f1)
    return db(amp_db) * np.sin(2 * np.pi * f1 * dur / k * (np.exp(tt / dur * k) - 1))
add("sweep_over_pink", fade(sweep(20) + noise(20, rms_db=-36)[0]),
    f1=40, f2=16000, sweep_peak_db=-18, noise_rms_db=-36)
add("sweep_alone", fade(sweep(10)), f1=40, f2=16000, sweep_peak_db=-18)

# 9. Two close resonances (resolution) and many at once (selectivity).
add("two_close_2k_2k4", fade(noise(4, [(2000, 12, 20), (2400, 12, 20)])[1]),
    peaks=[(2000, 12, 20), (2400, 12, 20)], rms_db=-24)
many = [(float(f0), 9, 15) for f0 in np.geomspace(150, 12000, 12).round()]
add("twelve_resonances", fade(noise(4, many)[1]), peaks=many, rms_db=-24)

# 10. Harmonic material: band-limited 110 Hz saw with one ringing formant.
n = 4 * SR; tt = np.arange(n) / SR; saw = np.zeros(n)
for h in range(1, int(20000 / 110)):
    saw += np.sin(2 * np.pi * 110 * h * tt) / h * bell(np.array([110.0 * h]), 2500, 14, 8)[0]
saw *= db(-18) / np.max(np.abs(saw))
add("saw_110Hz_formant_2k5", fade(saw), peaks=[(2500, 14, 8)], peak_db=-18)

add("sync_impulse_end", impulse(), note="single sample at -6 dBFS")

x = np.concatenate(out)
peak = np.max(np.abs(x))
for s in segs:
    a=int(s["start_s"]*SR); seg=x[a:a+int(s["dur_s"]*SR)]; pk_=20*np.log10(np.max(np.abs(seg)))
    if pk_>-6: print("hot", s["name"], round(pk_,2))
assert peak < db(-1), f"peak too hot: {20*np.log10(peak):.2f} dBFS"
pcm = np.round(x * (2 ** 23 - 1)).astype(np.int32)
st = np.repeat(pcm, 2)
b = st.astype("<i4").tobytes()
b = b"".join(b[i:i + 3] for i in range(0, len(b), 4))
with wave.open(str(HERE / "rez_test_48k_24bit.wav"), "wb") as w:
    w.setnchannels(2); w.setsampwidth(3); w.setframerate(SR); w.writeframes(b)
json.dump({"sample_rate": SR, "channels": 2, "bits": 24, "samples": len(x), "segments": segs},
          open(HERE / "rez_test_segments.json", "w"), indent=1)
print(f"{len(x)/SR:.2f} s, {len(x)} samples, peak {20*np.log10(peak):.2f} dBFS, {len(segs)} segments")
