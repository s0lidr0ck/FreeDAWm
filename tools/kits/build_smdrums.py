"""Builds Juke's SM Drums kit folder from SMDrums_Sforzando_1.2.zip.

usage: python3 -I build_smdrums.py <zip> <out folder>
Reads the zip directly (nothing is extracted but the chosen samples), picks four velocity layers and
up to four round-robin takes per pad from the SFZ mappings, trims each take's tail with a fade, and
writes <out>/kit.json.
"""
import json, os, re, struct, sys, zipfile

ZIP, OUT = sys.argv[1], sys.argv[2]
LAYER_VELS = [40, 72, 100, 124]
TAKES = 4
# (pad name, family, note, mapping, seconds kept)
PADS = [
    ("Kick", "kick", 36, "kick", 2.5),
    ("Side stick", "percussion", 37, "sidestick", 1.5),
    ("Snare", "snare", 38, "snare65reg", 2.5),
    ("Rimshot", "snare", 40, "rimshot", 2.5),
    ("Closed hat", "hat", 42, "closedhat", 1.5),
    ("Pedal hat", "hat", 44, "foothat", 1.5),
    ("Open hat", "hat", 46, "openhat", 5.0),
    ("Floor tom", "tom", 41, "tom4", 3.0),
    ("Low tom", "tom", 45, "tom3", 3.0),
    ("Mid tom", "tom", 48, "tom2", 3.0),
    ("Crash", "cymbal", 49, "crash16", 6.0),
    ("Ride", "cymbal", 51, "ride20tip", 5.0),
    ("Ride bell", "cymbal", 53, "ride20bell", 5.0),
    ("China", "cymbal", 52, "china", 6.0),
    ("Crash 13", "cymbal", 57, "crash13", 6.0),
    ("High tom", "tom", 50, "tom1", 3.0),
]


def regions(text):
    """(sample path in the zip, lovel, hivel, take) per region."""
    out, group, cur, where = [], {}, None, None
    for tok in re.split(r"(<group>|<region>|<control>|<global>|<master>)", text):
        if tok in ("<group>", "<control>", "<global>", "<master>"):
            if cur is not None:
                out.append(cur)
                cur = None
            where = tok
            if tok == "<group>":
                group = {}
            continue
        if tok == "<region>":
            if cur is not None:
                out.append(cur)
            cur = dict(group)
            where = tok
            continue
        for line in tok.splitlines():
            line = line.split("//")[0].strip()
            if "=" not in line:
                continue
            k, v = line.split("=", 1)
            target = cur if where == "<region>" and cur is not None else group if where == "<group>" else None
            if target is not None:
                target[k.strip()] = v.strip()
    if cur is not None:
        out.append(cur)
    res = []
    for r in out:
        s = r.get("sample")
        if not s:
            continue
        path = os.path.normpath(os.path.join("Programs", s.replace("\\", "/"))).replace("\\", "/")
        path = re.sub(r"^(\.\./)+", "", path)
        m = re.search(r"RR(\d+)", s)
        res.append((path, int(r.get("lovel", 0)), int(r.get("hivel", 127)), int(m.group(1)) if m else 1))
    return res


def trim_wav(data, seconds):
    """The WAV cut to `seconds`, its last 20 ms faded out. PCM 16/24/32 or float 32."""
    if data[:4] != b"RIFF" or data[8:12] != b"WAVE":
        raise ValueError("not a WAV")
    pos, fmt, chunks = 12, None, []
    while pos + 8 <= len(data):
        cid, size = data[pos:pos + 4], struct.unpack("<I", data[pos + 4:pos + 8])[0]
        body = data[pos + 8:pos + 8 + size]
        if cid == b"fmt ":
            fmt = body
        chunks.append((cid, body))
        pos += 8 + size + (size & 1)
    tag, ch, rate, _, align, bits = struct.unpack("<HHIIHH", fmt[:16])
    if tag == 0xFFFE:
        tag = struct.unpack("<H", fmt[24:26])[0]
    pcm = [c for c in chunks if c[0] == b"data"][0][1]
    frames = min(len(pcm) // align, int(rate * seconds))
    pcm = bytearray(pcm[:frames * align])
    fade = min(frames, rate // 50)
    bps = bits // 8
    for i in range(fade):
        g = 1.0 - (i + 1) / fade
        f = frames - fade + i
        for c in range(ch):
            at = f * align + c * bps
            raw = pcm[at:at + bps]
            if tag == 3 and bps == 4:
                v = struct.unpack("<f", raw)[0] * g
                pcm[at:at + 4] = struct.pack("<f", v)
            else:
                v = int.from_bytes(raw, "little", signed=bps > 1)
                v = int(v * g)
                pcm[at:at + bps] = v.to_bytes(bps, "little", signed=bps > 1)
    fmt_out = struct.pack("<HHIIHH", 3 if tag == 3 else 1, ch, rate, rate * align, align, bits)
    body = b"WAVE" + b"fmt " + struct.pack("<I", 16) + fmt_out + b"data" + struct.pack("<I", len(pcm)) + bytes(pcm)
    return b"RIFF" + struct.pack("<I", len(body)) + body, rate, ch, bits


z = zipfile.ZipFile(ZIP)
names = set(z.namelist())
os.makedirs(OUT, exist_ok=True)
pads, total = [], 0
for i, (name, family, note, mapping, secs) in enumerate(PADS):
    regs = regions(z.read(f"Programs/mappings/smdrums_sfz_{mapping}.sfz").decode("utf-8", "replace"))
    folder = f"{i + 1:02d}_{re.sub(r'[^a-z0-9]+', '_', name.lower()).strip('_')}"
    os.makedirs(os.path.join(OUT, folder), exist_ok=True)
    samples = {}
    for layer, vel in enumerate(LAYER_VELS):
        for take in range(1, TAKES + 1):
            hit = [r for r in regs if r[3] == take and r[1] <= vel <= r[2]]
            if not hit:
                continue
            src = hit[0][0]
            if src not in names:
                raise SystemExit(f"{name}: {src} is not in the zip")
            wav, rate, ch, bits = trim_wav(z.read(src), secs)
            rel = f"{folder}/v{layer + 1}r{take}.wav"
            with open(os.path.join(OUT, rel), "wb") as f:
                f.write(wav)
            total += len(wav)
            samples[f"v{layer + 1}.r{take}"] = rel
    if not samples:
        raise SystemExit(f"{name}: no samples picked")
    pads.append({"name": name, "family": family, "note": note, "samples": samples})
    print(f"{name}: {len(samples)} samples ({rate} Hz, {ch} ch, {bits} bit)")
kit = {
    "name": "SM Drums",
    "license": "Public domain (as listed by SFZ Instruments; the authors offer it free of charge for any use: smmdrums.wordpress.com)",
    "credit": "SM Drums by Scott McLean, Tod Stillwell and Suleiman Ali (samples, editing), SFZ programming by DSmolken; trimmed to Juke's 4 velocity layers x 4 takes",
    "pads": pads,
}
with open(os.path.join(OUT, "kit.json"), "w") as f:
    json.dump(kit, f, indent=2)
print(f"total {total / 1048576:.1f} MiB")
