"""Samples an instrument through any effects chain: writes a MIDI file that plays every note to
keep, one at a time, then cuts the audio rendered from it into one file per note.

usage: python3 -I sampling.py midi <out folder> [options]
       python3 -I sampling.py split <plan.json> <render.wav> <out folder> [--name NAME]

`midi` writes <out>/sampling.mid and <out>/plan.json (what the file plays, and when). Each note
has a slot of its own: held for --hold seconds (down to --hold-high at the top note), then left to ring for --tail seconds before the
next one starts. Notes run from --low to --high every --step semitones, each at every velocity
in --vels (or --layers of them, evenly spaced), --robins times in a row (for an instrument that plays a different take each time).

`split` reads a WAV rendered from that file (16-, 24- or 32-bit, or 32-bit float), lines it up
by the first note, cuts each slot from its start to where it has rung out, fades the end, and
writes the notes beside an .sfz file that maps them. Levels are left as rendered, so the
velocity layers keep their balance. Needs nothing but Python.
"""
import argparse, array, json, math, mmap, os, struct, sys

PPQ = 480
NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
LEAD_SECS = 2.0  # silence before the first note: the splitter measures the chain's noise there
BLOCK_SECS = 0.005
FADE_SECS = 0.02
PRE_SECS = 0.002


def note_name(n):
    return f"{NAMES[n % 12]}{n // 12 - 1}"


def parse_note(text):
    t = text.strip()
    if t.lstrip("-").isdigit():
        n = int(t)
    else:
        letter, rest = t[0].upper(), t[1:]
        n = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}[letter]
        while rest[:1] in ("#", "b"):
            n += 1 if rest[0] == "#" else -1
            rest = rest[1:]
        n += (int(rest) + 1) * 12
    if not 0 <= n <= 127:
        raise ValueError(f"note {text} is outside 0-127")
    return n


def varlen(n):
    out = [n & 0x7F]
    n >>= 7
    while n:
        out.append(0x80 | (n & 0x7F))
        n >>= 7
    return bytes(reversed(out))


# ---- midi ----

def cmd_midi(a):
    low, high = parse_note(a.low), parse_note(a.high)
    if a.layers:
        vels = sorted({max(1, round(127 * i / a.layers)) for i in range(1, a.layers + 1)})
    else:
        vels = sorted({int(v) for v in a.vels.split(",")})
    top = a.hold if a.hold_high is None else a.hold_high
    if low > high or a.step < 1 or a.robins < 1 or not vels or vels[0] < 1 or vels[-1] > 127:
        sys.exit("nothing to play: check --low, --high, --step, --vels and --robins")
    if a.hold <= 0 or top <= 0 or a.tail < 0 or not 20 <= a.bpm <= 400:
        sys.exit("--hold must be above 0, --tail 0 or more, --bpm between 20 and 400")
    tick = lambda secs: round(secs * a.bpm / 60 * PPQ)
    lead = tick(LEAD_SECS)
    secs = lambda ticks: ticks * 60 / (a.bpm * PPQ)
    events = [(0, bytes([0xB0, 64, 0]))]  # (tick, message)
    slots = []
    at = lead
    for note in range(low, high + 1, a.step):
        # High notes die away sooner: the hold runs evenly from --hold at the bottom to --hold-high.
        held = a.hold + (top - a.hold) * (note - low) / max(1, high - low)
        hold, slot = tick(held), tick(held + a.tail)
        for vel in vels:
            for robin in range(1, a.robins + 1):
                events.append((at, bytes([0x90, note, vel])))
                events.append((at + hold, bytes([0x80, note, 0])))
                slots.append({"note": note, "velocity": vel, "robin": robin, "start": secs(at), "length": secs(slot)})
                at += slot
    # FreeDAWm ends the clip a tick after its last event: this one keeps the last note's tail in.
    events.append((at, bytes([0xB0, 64, 0])))
    events.sort(key=lambda e: e[0])
    track = b"\x00\xff\x03" + varlen(8) + b"Sampling"
    track += b"\x00\xff\x51\x03" + round(60_000_000 / a.bpm).to_bytes(3, "big")
    track += b"\x00\xff\x58\x04\x04\x02\x18\x08"
    last = 0
    for t, msg in events:
        track += varlen(t - last) + msg
        last = t
    track += b"\x00\xff\x2f\x00"
    os.makedirs(a.out, exist_ok=True)
    with open(os.path.join(a.out, "sampling.mid"), "wb") as f:
        f.write(b"MThd" + struct.pack(">IHHH", 6, 0, 1, PPQ) + b"MTrk" + struct.pack(">I", len(track)) + track)
    plan = {"bpm": a.bpm, "length": secs(at), "low": low, "high": high,
            "step": a.step, "velocities": vels, "robins": a.robins, "slots": slots}
    with open(os.path.join(a.out, "plan.json"), "w") as f:
        json.dump(plan, f, indent=1)
    m, s = divmod(round(secs(at)), 60)
    print(f"{len(slots)} notes ({note_name(low)}-{note_name(slots[-1]['note'])}, {len(vels)} velocities, "
          f"{a.robins} each), {m}:{s:02d} at {a.bpm:g} BPM -> {a.out}/sampling.mid")


# ---- split ----

class Wav:
    """A WAV file's audio, read in place."""

    def __init__(self, path):
        self.file = open(path, "rb")
        self.map = mmap.mmap(self.file.fileno(), 0, access=mmap.ACCESS_READ)
        m = self.map
        if m[0:4] != b"RIFF" or m[8:12] != b"WAVE":
            sys.exit(f"{path}: not a WAV file")
        pos, fmt, self.start = 12, None, None
        while pos + 8 <= len(m):
            cid, size = m[pos:pos + 4], struct.unpack("<I", m[pos + 4:pos + 8])[0]
            if cid == b"fmt " and size >= 16:
                fmt = m[pos + 8:pos + 8 + size]
            elif cid == b"data":
                self.start, data = pos + 8, min(size, len(m) - pos - 8)
                break
            pos += 8 + size + (size & 1)
        if fmt is None or self.start is None:
            sys.exit(f"{path}: no audio found")
        tag, self.channels, self.rate, _, _, bits = struct.unpack("<HHIIHH", fmt[:16])
        if tag == 0xFFFE and len(fmt) >= 26:
            tag = struct.unpack("<H", fmt[24:26])[0]
        self.fmt = fmt
        self.width = bits // 8
        self.float = tag == 3
        if self.channels < 1 or self.rate < 1 or (tag, bits) not in ((1, 16), (1, 24), (1, 32), (3, 32)):
            sys.exit(f"{path}: {bits}-bit format {tag} is not supported (use 16-, 24- or 32-bit, or 32-bit float)")
        self.frame = self.width * self.channels
        self.frames = data // self.frame
        self.scale = 1.0 if self.float else float(1 << (15 if self.width < 4 else 31))

    def raw(self, a, b):
        a, b = max(0, min(a, self.frames)), max(0, min(b, self.frames))
        return self.map[self.start + a * self.frame:self.start + b * self.frame]

    def levels(self, a, b, block):
        """The peak (0..1) of each `block` frames from frame a to b."""
        raw = self.raw(a, b)
        if self.width == 3:  # the top two bytes of each sample are enough to measure it
            top = bytearray(len(raw) // 3 * 2)
            top[0::2], top[1::2] = raw[1::3], raw[2::3]
            vals = array.array("h", bytes(top))
        else:
            vals = array.array("f" if self.float else ("h" if self.width == 2 else "i"), raw)
        if sys.byteorder == "big":
            vals.byteswap()
        step = block * self.channels
        out = []
        for i in range(0, len(vals) - step + 1, step):
            chunk = vals[i:i + step]
            out.append(max(max(chunk), -min(chunk)) / self.scale)
        return out

    def faded(self, a, b, fade):
        """Frames a..b as bytes, the last `fade` frames fading to silence."""
        raw = self.raw(a, b)
        fade = min(fade, len(raw) // self.frame)
        if fade == 0:
            return raw
        cut = len(raw) - fade * self.frame
        tail, out = raw[cut:], bytearray()
        for i in range(fade):
            g = 0.5 + 0.5 * math.cos(math.pi * (i + 1) / fade)
            for c in range(self.channels):
                p = (i * self.channels + c) * self.width
                if self.float:
                    out += struct.pack("<f", struct.unpack_from("<f", tail, p)[0] * g)
                else:
                    v = int.from_bytes(tail[p:p + self.width], "little", signed=True)
                    out += round(v * g).to_bytes(self.width, "little", signed=True)
        return raw[:cut] + bytes(out)

    def write(self, path, data):
        with open(path, "wb") as f:
            f.write(b"RIFF" + struct.pack("<I", 20 + len(self.fmt) + len(data)) + b"WAVE")
            f.write(b"fmt " + struct.pack("<I", len(self.fmt)) + self.fmt)
            f.write(b"data" + struct.pack("<I", len(data)) + data + (b"\x00" if len(data) & 1 else b""))


def onset(levels, floor):
    """The first block a note sounds in: where it first reaches a tenth of the loudest block,
    walked back to where it rose out of the floor. None when nothing rises above the floor."""
    peak = max(levels, default=0.0)
    if peak <= floor * 2:
        return None
    i = next(k for k, v in enumerate(levels) if v >= peak * 0.1)
    while i > 0 and levels[i - 1] > max(floor * 2, peak * 0.01):
        i -= 1
    return i


def cmd_split(a):
    with open(a.plan) as f:
        plan = json.load(f)
    wav, slots = Wav(a.wav), plan["slots"]
    sr = wav.rate
    block = max(1, round(BLOCK_SECS * sr))
    frames = lambda secs: round(secs * sr)

    # The chain's noise, from the quietest half second of the lead-in (hum and hiss never stop).
    lead = wav.levels(0, frames(LEAD_SECS), block)
    half = max(1, frames(0.5) // block)
    floor = min((max(lead[i:i + half]) for i in range(0, max(1, len(lead) - half + 1), half)), default=0.0)

    # Line the render up by its first note: effects delay it, and a render may not start at the clip.
    first = frames(slots[0]["start"])
    found = onset(wav.levels(0, first + frames(slots[0]["length"]) // 2, block), floor)
    if found is None:
        sys.exit("no note found where the first one should be: is the render silent?")
    shift = found * block - first
    # The last note says whether the render ran at the plan's tempo.
    last = frames(slots[-1]["start"]) + shift
    near = frames(0.5)
    found = onset(wav.levels(last - near, last + near, block), floor) if len(slots) > 1 else near // block
    if last + near > wav.frames or found is None or abs(found * block - near) > frames(0.03):
        sys.exit(f"the last note is not where the plan puts it ({last / sr:.2f} s): the render must play "
                 f"sampling.mid from its start to its end at {plan['bpm']:g} BPM")

    os.makedirs(a.out, exist_ok=True)
    name = a.name or os.path.basename(os.path.normpath(a.out))
    quiet = max(floor * 1.5, 10 ** (a.silence / 20))
    notes = sorted({s["note"] for s in slots})
    vels = plan["velocities"]
    regions, ringing, silent, loudest, total = [], 0, 0, 0.0, 0
    for s in slots:
        start = frames(s["start"]) + shift - frames(PRE_SECS)
        slot_len = frames(s["length"])
        levels = wav.levels(start, start + slot_len, block)
        peak = max(levels, default=0.0)
        if peak <= quiet:
            silent += 1
            continue
        loudest = max(loudest, peak)
        end = max(k for k, v in enumerate(levels) if v > quiet) + 1
        # Still sounding when the next note starts: the fade then ends just before that note.
        if end >= len(levels) - 1:
            ringing += 1
            length = slot_len - frames(PRE_SECS) - block
        else:
            length = min(end * block + frames(0.05), slot_len - frames(PRE_SECS) - block)
        file = f"{s['note']:03d}_{note_name(s['note']).replace('#', 's')}_v{s['velocity']:03d}_rr{s['robin']}.wav"
        data = wav.faded(start, start + length, frames(FADE_SECS))
        wav.write(os.path.join(a.out, file), data)
        total += len(data)
        # Each note covers the keys nearer to it than to its neighbours, each velocity up from the one below.
        i, v = notes.index(s["note"]), vels.index(s["velocity"])
        lokey = 0 if i == 0 else (notes[i - 1] + s["note"]) // 2 + 1
        hikey = 127 if i == len(notes) - 1 else (s["note"] + notes[i + 1]) // 2
        lovel = 1 if v == 0 else vels[v - 1] + 1
        hivel = 127 if v == len(vels) - 1 else s["velocity"]
        robin = f" seq_length={plan['robins']} seq_position={s['robin']}" if plan["robins"] > 1 else ""
        regions.append(f"<region> sample={file} pitch_keycenter={s['note']} lokey={lokey} hikey={hikey} "
                       f"lovel={lovel} hivel={hivel}{robin}")
    with open(os.path.join(a.out, f"{name}.sfz"), "w") as f:
        f.write(f"// {name}: cut by tools/sampling/sampling.py\n"
                "// The layers hold the dynamics as rendered, so velocity does not scale them again.\n"
                "<global> amp_veltrack=0 ampeg_release=0.4\n" + "\n".join(regions) + "\n")
    print(f"{len(regions)} notes, {total / 1e6:.0f} MB -> {a.out}/{name}.sfz "
          f"(shift {shift / sr * 1000:+.1f} ms, noise {20 * math.log10(max(floor, 1e-9)):.0f} dB, "
          f"peak {20 * math.log10(max(loudest, 1e-9)):.1f} dB)")
    if silent:
        print(f"warning: {silent} notes were silent and left out (outside the instrument's range?)")
    if ringing:
        print(f"warning: {ringing} notes were still sounding when the next began: they end in a fade. "
              "For their whole tail, make the MIDI again with a longer --tail")
    if loudest >= 0.999:
        print("warning: the render reaches full scale: it may be clipped")


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = p.add_subparsers(dest="cmd", required=True)
    m = sub.add_parser("midi", help="write the MIDI file and its plan")
    m.add_argument("out")
    m.add_argument("--low", default="A0", help="lowest note, a name or a number (default A0)")
    m.add_argument("--high", default="C8", help="highest note (default C8)")
    m.add_argument("--step", type=int, default=3, help="semitones between notes (default 3)")
    m.add_argument("--vels", default="24,48,72,96,112,127", help="velocities, soft to hard")
    m.add_argument("--layers", type=int, help="this many evenly spaced velocities up to 127, instead of --vels")
    m.add_argument("--robins", type=int, default=1, help="times each note is struck (default 1)")
    m.add_argument("--hold", type=float, default=8.0, help="seconds each note is held (default 8)")
    m.add_argument("--hold-high", type=float, help="seconds the highest note is held (default: as --hold)")
    m.add_argument("--tail", type=float, default=4.0, help="seconds left to ring after it (default 4)")
    m.add_argument("--bpm", type=float, default=120.0)
    m.set_defaults(run=cmd_midi)
    s = sub.add_parser("split", help="cut a render into notes")
    s.add_argument("plan")
    s.add_argument("wav")
    s.add_argument("out")
    s.add_argument("--name", help="the instrument's name (default: the out folder's)")
    s.add_argument("--silence", type=float, default=-72.0, help="level a note has rung out at, dBFS (default -72)")
    s.set_defaults(run=cmd_split)
    a = p.parse_args()
    a.run(a)


if __name__ == "__main__":
    main()
