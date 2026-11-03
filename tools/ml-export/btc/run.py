"""BTC chords: export -> parity (original test.py pipeline: librosa.cqt features + PyTorch BTC +
its segment loop, vs reference_infer.py) -> golden -> synthetic mix + piano/acoustic stems,
consistency with notes-derived chords (Basic Pitch notes + templates) -> install ml-models/btc_chords/."""

from __future__ import annotations

import importlib.util
import json
import sys
import time
from pathlib import Path

import numpy as np
import soundfile as sf
import torch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE))

import common as C  # noqa: E402
import export as EX  # noqa: E402
import midi_common as M  # noqa: E402
import reference_infer as RI  # noqa: E402

ID = "btc_chords"
WORK = C.WORK / "btc"
ONNX = WORK / "btc_chords.onnx"
OUT = C.RENDERS / "chords"


def original_segments(wav_path):
    """test.py --voca True, verbatim logic (features, normalisation, per-108 loop, lab lines)."""
    m, mean, std, cfg = EX.load_original()
    import librosa

    w, sr = librosa.load(str(wav_path), sr=22050, mono=True)
    cur = 0
    feats = []
    while len(w) > cur + 22050 * 10.0:
        s, e = int(cur), int(cur + 22050 * 10.0)
        feats.append(librosa.cqt(w[s:e], sr=sr, n_bins=144, bins_per_octave=24, hop_length=2048))
        cur = e
    feats.append(librosa.cqt(w[cur:], sr=sr, n_bins=144, bins_per_octave=24, hop_length=2048))
    feature = np.log(np.abs(np.concatenate(feats, axis=1)) + 1e-6).T
    feature = (feature - mean) / std
    n_real = feature.shape[0]
    tu, nt = 10.0 / 108, 108
    num_pad = nt - (feature.shape[0] % nt)
    feature = np.pad(feature, ((0, num_pad), (0, 0)), mode="constant", constant_values=0)
    ni = feature.shape[0] // nt
    lines, frames, start = [], [], 0.0
    with torch.no_grad():
        ft = torch.tensor(feature, dtype=torch.float32).unsqueeze(0)
        for t in range(ni):
            h, _ = m.self_attn_layers(ft[:, nt * t : nt * (t + 1), :])
            pred, _ = m.output_layer(h)
            pred = pred.squeeze()
            frames += pred.tolist()
            for i in range(nt):
                if t == 0 and i == 0:
                    prev = pred[i].item()
                    continue
                if pred[i].item() != prev:
                    lines.append((start, tu * (nt * t + i), prev))
                    start = tu * (nt * t + i)
                    prev = pred[i].item()
                if t == ni - 1 and i + num_pad == nt:
                    if start != tu * (nt * t + i):
                        lines.append((start, tu * (nt * t + i), prev))
                    break
    return lines, np.array(frames[:n_real]), feature[:n_real]


# ------------------------------------------------------------- notes-derived chords
def note_chords(notes, segs):
    """Per chord segment: duration x velocity weighted pitch-class profile of the notes sounding,
    best of the 168 BTC templates (cosine; binary template, root weighted 1.5) or N if silent."""
    out = []
    for s in segs:
        pc = np.zeros(12)
        for n in notes:
            ov = min(n["end"], s["end"]) - max(n["start"], s["start"])
            if ov > 0:
                pc[n["pitch"] % 12] += ov * n["velocity"]
        if pc.sum() == 0:
            out.append(169)
            continue
        best, bs = 169, -1
        for c in range(168):
            r, q = c // 14, RI.QUALITIES[c % 14]
            t = np.zeros(12)
            for k in RI.TONES[q]:
                t[(r + k) % 12] = 1.0
            t[r] = 1.5
            sc = pc @ t / (np.linalg.norm(t) * np.linalg.norm(pc))
            if sc > bs:
                best, bs = c, sc
        out.append(best)
    return out


def agreement(segs, other):
    dur = tot_root = tot_mm = tot_exact = 0.0
    for s, o in zip(segs, other):
        d = s["end"] - s["start"]
        if s["class"] >= 168:
            continue
        dur += d
        if o >= 168:
            continue
        r1, q1, r2, q2 = s["class"] // 14, RI.QUALITIES[s["class"] % 14], o // 14, RI.QUALITIES[o % 14]
        if r1 == r2:
            tot_root += d
            third = lambda q: 3 if 3 in RI.TONES[q] else (4 if 4 in RI.TONES[q] else 0)  # noqa: E731
            if third(q1) == third(q2):
                tot_mm += d
            if q1 == q2:
                tot_exact += d
    return {"root": round(tot_root / max(dur, 1e-9), 3), "root+third": round(tot_mm / max(dur, 1e-9), 3),
            "exact": round(tot_exact / max(dur, 1e-9), 3), "chord_seconds": round(dur, 2)}


def frame_classes(segs, t):
    out = np.full(len(t), 169)
    for s in segs:
        out[(t >= s["start"]) & (t < s["end"])] = s["class"]
    return out


def pads(segs, sr, n):
    """Chord pads: root in octave 3 + chord tones in octave 4, soft attack/release."""
    y = np.zeros(n)
    for s in segs:
        if s["class"] >= 168:
            continue
        a, b = int(s["start"] * sr), min(n, int(s["end"] * sr))
        if b <= a:
            continue
        t = np.arange(b - a) / sr
        env = np.minimum(1, t / 0.05) * np.minimum(1, (t[::-1]) / 0.05)
        ps = [48 + s["bass_pc"]] + [60 + (s["root_pc"] + k) % 12 for k in RI.TONES[s["quality"]]]
        for p in ps:
            f = 440 * 2 ** ((p - 69) / 12)
            y[a:b] += 0.08 * env * (np.sin(2 * np.pi * f * t) + 0.3 * np.sin(4 * np.pi * f * t))
    return y


def load_bp():
    spec = importlib.util.spec_from_file_location("bp_ri", HERE.parent / "basicpitch" / "reference_infer.py")
    bp = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(bp)
    return bp, bp.make_session(C.MODELS / "basicpitch" / "basicpitch.onnx")


def source(name):
    if name == "mix":
        x, sr = sf.read(str(C.TESTMIX / "mix.wav"), dtype="float32")
        return x.mean(1), sr
    return M.load_source(name)


def main(force=False):
    if not force and C.card_is_current(ID):
        print(f"{ID}: installed card matches, skipping")
        return None
    WORK.mkdir(parents=True, exist_ok=True)
    OUT.mkdir(parents=True, exist_ok=True)
    if force or not ONNX.exists():
        EX.export(ONNX)
    sess = RI.make_session(ONNX)
    report = {"id": ID}

    # parity on the synthetic mix (30 s = 3 chunks) and on 25 s of piano (partial last chunk)
    par = {}
    for name, dur in (("mix", None), ("piano", 25.0)):
        x, sr = source(name)
        if dur:
            x = x[: int(dur * sr)]
        x22 = C.resample(x, sr, RI.SR)
        wav = WORK / f"parity_{name}_22050.wav"
        sf.write(str(wav), x22, RI.SR, subtype="FLOAT")
        x22 = sf.read(str(wav), dtype="float32")[0]
        t0 = time.perf_counter()
        lines, oframes, ofeat = original_segments(wav)
        t_orig = time.perf_counter() - t0
        t0 = time.perf_counter()
        segs, logits = RI.transcribe(sess, x22)
        t_ort = time.perf_counter() - t0
        # (a) model export alone: original torch model on the reference features
        m, mean, std, _ = EX.load_original()
        _, cqt = RI.run_network(sess, x22)
        f = (cqt - mean) / std
        nf = len(f)
        f = np.pad(f, ((0, (-nf) % 108), (0, 0)))
        with torch.no_grad():
            tl = torch.cat([m.output_layer.output_projection(m.self_attn_layers(torch.tensor(f[i:i + 108][None]))[0])[0]
                            for i in range(0, len(f), 108)]).numpy()[:nf]
        rframes = np.argmax(logits, 1)
        par[name] = {
            "model_logits_snr_db": C.snr_db(tl, logits),
            "cqt_feature_snr_db_vs_librosa": C.snr_db(ofeat * std + mean, cqt),
            "frame_label_agreement_vs_original": round(float(np.mean(oframes == rframes)), 4),
            "frames": int(nf),
            "segments": [len(lines), len(segs)],
            "time_s": {"original_cpu": t_orig, "ort_cpu": t_ort},
        }
        print("parity", name, par[name], flush=True)
    report["parity"] = par

    gold = WORK / "golden"
    gx = C.resample(source("mix")[0][: 48000 * 12], 48000, RI.SR)  # 12 s: one full + one partial chunk
    gs, gl = RI.transcribe(sess, gx)
    C.write_f32(gold / "input_12s.f32", gx)
    C.write_f32(gold / "logits_12s.f32", gl)
    C.write_f32(gold / "cqt_12s.f32", RI.run_network(sess, gx)[1])
    C.save_json(gold / "segments_12s.json", {"segments": gs})
    (gold / "README.txt").write_text(
        "Golden files for btc_chords (reference_infer.py, CPU ORT). Raw little-endian float32, C order.\n"
        f"input_12s.f32   : [{len(gx)}] mono 22050 Hz (synthetic mix 7:00-7:12): chunks of 220500 + {len(gx) - 220500}\n"
        f"logits_12s.f32  : [{len(gl)}, 170] real frames of both chunks (108 + {len(gl) - 108})\n"
        f"cqt_12s.f32     : [{len(gl)}, 144] log CQT\n"
        "segments_12s.json : decoded segments {start, end, label, name, root_pc, quality, bass_pc, class}\n",
        encoding="utf-8")

    # evaluation: mix, piano, acoustic; consistency with Basic Pitch notes
    bp, bps = load_bp()
    ev = {}
    res = {}
    for name in ("mix", "piano", "acoustic"):
        x, sr = source(name)
        x22 = C.resample(x, sr, RI.SR)
        segs, _ = RI.transcribe(sess, x22)
        notes, _ = bp.transcribe(bps, C.resample(x, sr, bp.SR))
        nc = note_chords(notes, segs)
        res[name] = segs
        ev[name] = {"segments": len(segs), "chords": [s["name"] or s["label"] for s in segs],
                    "vs_notes_derived": agreement(segs, nc),
                    "slash_chords": sum(1 for s in segs if "/" in s["name"])}
        C.save_json(OUT / f"{name}_btc.json", {"segments": segs, "notes_derived": [RI.label_of(c) for c in nc]})
        (OUT / f"{name}_btc.lab").write_text("".join(f"{s['start']:.3f} {s['end']:.3f} {s['label']}\n" for s in segs))
        y = pads(segs, sr, len(x))
        C.write_wav24(OUT / f"{name}_btc_check.wav", np.stack([x / (np.abs(x).max() + 1e-9) * 0.8,
                                                                y / (np.abs(y).max() + 1e-9) * 0.8]), sr)
        print("eval", name, {k: v for k, v in ev[name].items() if k != "chords"}, flush=True)
    t = np.arange(0, 30, 0.05)
    fm, fp, fa = (frame_classes(res[n], t) for n in ("mix", "piano", "acoustic"))
    both = (fm < 168) & (fp < 168)
    ev["mix_vs_piano_root_agreement"] = round(float(np.mean((fm // 14 == fp // 14)[both])), 3)
    both = (fm < 168) & (fa < 168)
    ev["mix_vs_acoustic_root_agreement"] = round(float(np.mean((fm // 14 == fa // 14)[both])), 3)
    # bass heuristic vs the bass stem (Basic Pitch lowest-note pitch class per segment)
    xb, sr = M.load_source("bass")
    bnotes, _ = bp.transcribe(bps, C.resample(xb, sr, bp.SR))
    agree = tot = 0
    for s in res["mix"]:
        if s["class"] >= 168:
            continue
        w = np.zeros(12)
        for n in bnotes:
            ov = min(n["end"], s["end"]) - max(n["start"], s["start"])
            if ov > 0:
                w[n["pitch"] % 12] += ov
        if w.sum() == 0:
            continue
        tot += 1
        agree += int(np.argmax(w) == s["bass_pc"])
    ev["bass_pc_vs_bass_stem"] = {"agree": agree, "segments": tot}
    print("eval", {k: v for k, v in ev.items() if not isinstance(v, dict) or "segments" not in v}, flush=True)
    report["eval"] = ev

    card = {
        "id": ID, "name": "BTC large-vocabulary chord recognition (Park et al., ISMIR 2019)", "task": "chord_recognition",
        "role": "audio -> time-stamped chords for the Band chord lane", "file": "btc_chords.onnx",
        "sample_rate": RI.SR, "channels": 1,
        "chunk": {"samples": RI.CHUNK, "frames": RI.T, "time_unit_s": RI.TIME_UNIT, "last_chunk": "partial, zero-padded, n_frames = 1 + len // 2048"},
        "stft": None, "stft_note": "CQT inside the graph: 144 bins, 24/oct from C1, hop 2048, full-rate wavelet kernels (librosa.filters.wavelet, hann, norm 1) x sqrt(length), log(|C|+1e-6)",
        "inputs": [{"name": "audio", "shape": [1, RI.CHUNK], "dtype": "float32"},
                   {"name": "n_frames", "shape": [1], "dtype": "int64"}],
        "outputs": [{"name": "logits", "shape": [1, RI.T, 170]}, {"name": "cqt", "shape": [1, RI.T, 144]}],
        "vocabulary": {"roots": RI.ROOTS, "qualities": RI.QUALITIES, "class": "root*14 + quality; 168 = X, 169 = N",
                       "daw_suffix": RI.DAW_SUFFIX, "inversions": "not predicted by BTC; host bass heuristic adds /bass"},
        "decode": {"argmax_per_frame": True, "segments": "runs of equal labels", "bass_ratio": RI.BASS_RATIO},
        "source_url": f"https://github.com/jayg996/BTC-ISMIR19 (commit {EX.COMMIT}), test/btc_model_large_voca.pt",
        "licence": "MIT (code and bundled checkpoint). Trained on Isophonics/RWC/uspop-style annotated pop datasets.",
        "export_note": "The original computes librosa.cqt (multirate, soxr resampling) on the host; here a full-rate "
                       "kernel CQT is in the graph so the Rust side needs no CQT. See parity.",
        "dml_supported": True, "recommended_ep": "cpu",
        "dml_note": 'DirectML matches CPU (golden segments identical) and is faster; either is fine (36 s of audio: CPU 0.1 s).',
        "reference": "tools/ml-export/btc/reference_infer.py", "golden": "golden/README.txt",
    }
    C.install(ID, ONNX, card, gold)
    C.mark_ready(ID)
    C.save_json(WORK / "report.json", report)
    return report


if __name__ == "__main__":
    main(force="--force" in sys.argv)
