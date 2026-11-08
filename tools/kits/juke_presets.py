"""Writes Juke's built-in preset library, crates/engine/src/juke_presets.json.

usage: python3 -I tools/kits/juke_presets.py [--levels report.json]

Presets are designed here pad by pad, by role (snare, rim, ride, floor tom…), and each role is
turned into the source kit's pad number. Effects are written in real units (dB, ms, Hz, seconds)
and turned into the effects' parameter values. Each preset names the style groove it is shown off
with (`demo`): the renders play it, four bars and a tail.

Levels: every preset is rendered playing its demo and measured (`cargo test -p
soundcraft-engine --lib -- --ignored measure_every_preset`, which writes report.json). With
`--levels report.json`, each preset's kit level is moved so it plays at the library's loudness
(TARGET_DB) with its peaks under PEAK_MAX_DB; the levels are kept in juke_preset_levels.json.

Character: the drives below were calibrated by rendering (tools/kits/README.md). Tape needs 0.6+
of its drive range (about +12 dB) to be heard as tape, saturation 0.8+ (about +18 dB); under that
both are a polish, not a sound. Vint's Console circuit at a few dB is a gentle EQ; its Passive,
British, Sampler, Air and Enhancer circuits are the characters.
"""
import json, math, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "..", "crates", "engine", "src", "juke_presets.json")
LEVELS = os.path.join(HERE, "juke_preset_levels.json")
TARGET_DB = -26.0     # the demo's gated loudness (100 ms blocks, quiet ones left out), dB
PEAK_MAX_DB = -1.0    # its peak at most, dBFS

# ---- roles → pad numbers, per kit layout ----------------------------------------------------
SYNTH = {  # Juke's default slots: the starter and style kits
    "kick": 1, "rim": 2, "snare": 3, "clap": 4, "hat": 5, "pedal": 6, "open": 7,
    "tom_lo": 8, "tom_mid": 9, "tom_hi": 10, "crash": 11, "ride": 12,
    "tamb": 13, "cowbell": 14, "shaker": 15, "clave": 16,
}
RECORDED = {
    "sm_drums": {"kick": 1, "rim": 2, "snare": 3, "rimshot": 4, "hat": 5, "pedal": 6, "open": 7,
                 "tom_floor": 8, "tom_lo": 9, "tom_mid": 10, "crash": 11, "ride": 12, "bell": 13,
                 "china": 14, "crash2": 15, "tom_hi": 16},
    "sm_drums_17": {"kick": 1, "rim": 2, "snare": 3, "snare2": 4, "hat": 5, "pedal": 6, "open": 7,
                    "tom_floor": 8, "tom_lo": 9, "tom_mid": 10, "crash": 11, "ride": 12, "bell": 13,
                    "half": 14, "crash2": 15, "tom_hi": 16},
    "sm_drums_hybrid": {"kick": 1, "rim": 2, "snare": 3, "snare2": 4, "rimshot": 5, "hat": 6,
                        "pedal": 7, "open": 8, "tom_floor": 9, "tom_lo": 10, "tom_mid": 11,
                        "crash": 12, "ride": 13, "snare3": 14, "crash2": 15, "tom_hi": 16},
}
HATS = {"hat", "pedal", "open", "half"}
TOMS = ["tom_hi", "tom_mid", "tom_lo", "tom_floor"]
# The styles (juke_styles.rs) and their grooves, for the demos.
STYLES = {
    "Boom Bap": ["Classic", "Busy Hats", "Half-Time"], "Trap": ["Straight"], "Lo-fi": ["Lazy Swing", "Rimshot"],
    "House": ["Classic", "Shuffled", "Tribal"], "Techno": ["Driving", "Broken"], "Drum & Bass": ["Two-Step", "Roller"],
    "Reggaeton": ["Dembow", "Perreo"], "Funk": ["Syncopated", "Ghosted"],
    "Alternative": ["Straight 8ths", "Driving 16ths", "Tom Groove"], "Soft Rock": ["Ballad", "Cross-Stick"],
    "Hard Rock": ["Rock Beat", "Ride Groove", "Fill"], "Metal": ["Double Kick", "Gallop", "Blast Beat"],
    "'50s Rock": ["Backbeat", "Shuffle"], "'60s Rock": ["Beat Group", "Surf"], "Rockabilly": ["Slap Back", "Boogie"],
    "Pop": ["Four on the Floor", "Pop Ballad", "Dance Pop"], "Folk": ["Boom-Chick", "Stomp & Clap", "Six-Eight"],
    "Country": ["Train Beat", "Shuffle"], "Praise": ["Upbeat", "Four on the Floor"],
    "Worship": ["Build", "Ballad", "Six-Eight Swell"], "Gospel / Shout": ["Shout", "Gospel Chops", "Shout Shuffle"],
}


def roles_of(source):
    return RECORDED.get(source, SYNTH)


# ---- effects, in real units -------------------------------------------------------------------
def lin(v, lo, hi):
    return round(max(0.0, min(1.0, (v - lo) / (hi - lo))), 4)


def logn(v, lo, hi):
    return round(max(0.0, min(1.0, math.log(v / lo) / math.log(hi / lo))), 4)


def fx(bus, slot, plugin, **params):
    return {"bus": bus, "slot": slot, "plugin": plugin, "params": params}


COMP_STYLES = ["Clean", "1176", "LA-2A", "160", "G Bus", "Vari-Mu", "Ross", "EL8", "2500", "33609"]


def comp(style="G Bus", amount=40, ratio=40, attack=50, release=50, makeup=0.0, knee=25, parallel=None,
         sc_hpf=None, bus="out", slot=1):
    # amount: the threshold, -50 dB at 100 %; halved here, so 40 means about -10 dB (glue).
    p = dict(style=COMP_STYLES.index(style), amount=amount / 200, ratio=ratio / 100, attack=attack / 100,
             release=release / 100, makeup=lin(makeup, -12, 24), knee=knee / 100, autoGain=0)
    if parallel is not None:  # the dry signal mixed back at this level (dB)
        p["dry"] = lin(parallel, -30, 10)
    if sc_hpf:  # the detector's high-pass, Hz (keeps the kick from pumping the bus)
        p["scHpf"] = logn(sc_hpf, 20, 1000)
    return fx(bus, slot, "fs_comp", **p)


REVERBS = ["Room", "Hall", "Plate", "Spring", "Cloud", "Shimmer"]


def reverb(kind="Room", decay=1.2, size=50, predelay=10, low_cut=150, high_cut=9000, damping=6000,
           width=100, era="Now", bus="a", slot=1):
    return fx(bus, slot, "fs_reverb", machine=REVERBS.index(kind), decay=logn(decay, 0.2, 30), size=size / 100,
              preDelay=round(math.sqrt(predelay / 250), 4), lowCut=logn(low_cut, 20, 1000),
              highCut=logn(high_cut, 1000, 20000), damping=logn(min(damping, 20000), 1000, 20000),
              width=lin(width, 0, 200), color=["1970s", "1980s", "Now"].index(era), mix=1.0)


MATERIALS = ["Painted Concrete", "Hard Plaster", "Glazed Tile", "Bare Brick", "Rough Stone", "Steel Plate",
             "Glass", "Hardwood", "Wood Panel", "Drywall", "Packed Earth", "Carpet", "Heavy Drape"]


def room(width_ft=22, length_ft=30, height_ft=12, walls="Hardwood", floor="Hardwood", ceiling="Drywall",
         decay=1.0, predelay=0, low_cut=120, high_cut=12000, mic="Condenser", drive=0, bus="a", slot=1):
    mics = ["Condenser", "Tube Condenser", "Ribbon", "Dynamic"]
    return fx(bus, slot, "fs_chamber", mix=1.0, decay=round(0.5 + math.log2(decay) / 4, 4),
              predelay=round(math.sqrt(predelay / 200), 4), lowCut=logn(low_cut, 20, 1000),
              highCut=logn(high_cut, 1000, 20000), roomW=logn(width_ft, 4.9, 98.4),
              roomL=logn(length_ft, 4.9, 98.4), roomH=logn(height_ft, 4.9, 49.2),
              wallMat=MATERIALS.index(walls), floorMat=MATERIALS.index(floor),
              ceilMat=MATERIALS.index(ceiling), micType=mics.index(mic), drive=drive / 100)


def tape(machine="Two-Track", speed="15 ips", drive=12.0, bias=0, flutter=0, hiss=None, bus="out", slot=1):
    """Tape: drive in dB over the range -6..+24 (heard as tape from about +12); bias -100..100 %."""
    p = dict(machine=["Two-Track", "Multitrack", "Valve", "Cassette"].index(machine),
             speed=["7.5 ips", "15 ips", "30 ips"].index(speed), drive=lin(drive, -6, 24),
             bias=lin(bias, -100, 100), flutter=flutter / 100, autoGain=1, mix=1.0)
    p["hiss"] = 0.0 if hiss is None else lin(hiss, -24, 18)
    return fx(bus, slot, "fs_tape", **p)


def sat(circuit="Valve", drive=18.0, focus=0.0, air=0, mix=100, bias=0, bus="out", slot=1):
    """Saturation: drive in dB over -6..+24 (heard from about +16); focus -12 lows .. +12 highs."""
    return fx(bus, slot, "fs_sat", circuit=["Iron", "Valve", "Desk", "Bridge", "FET"].index(circuit),
              drive=lin(drive, -6, 24), focus=lin(focus, -12, 12), air=air / 100, mix=mix / 100,
              bias=lin(bias, -100, 100), autoGain=1)


def punch(attack=6.0, sustain=0.0, bus="out", slot=1):
    return fx(bus, slot, "fs_transient", source=0, attack=lin(attack, -18, 18), sustain=lin(sustain, -24, 24), mix=1.0)


DIVISIONS = ["1/16T", "1/16", "1/8T", "1/16.", "1/8", "1/4T", "1/8.", "1/4", "1/2T", "1/4.", "1/2"]


def delay(division="1/8.", feedback=30, kind="Tape", low_cut=200, high_cut=5000, routing="Ping-Pong",
          ms=None, bus="b", slot=1):
    p = dict(style=["Digital", "Rack", "BBD", "Tape", "Drum", "Oil Can"].index(kind), feedback=lin(feedback, 0, 115),
             lowCut=logn(low_cut, 20, 1000), highCut=logn(high_cut, 1000, 20000),
             routing=["Mono", "Stereo", "Ping-Pong", "Dual"].index(routing), mix=1.0)
    if ms is None:
        p.update(sync=1, division=DIVISIONS.index(division))
    else:  # a fixed time: slapback
        p.update(sync=0, time=logn(ms, 20, 2000))
    return fx(bus, slot, "fs_delay", **p)


def vint(circuit, drive=0.0, bus="out", slot=1, **more):
    p = dict(circuit=["Passive", "Console", "Proportional", "British", "Enhancer", "Air", "Sampler", "Passive Mid"].index(circuit),
             stage=1, drive=lin(drive, -6, 18), mix=1.0, output=0.5)
    p.update(more)
    return fx(bus, slot, "fs_vint", **p)


def eq(hpf=None, low=None, mid=None, high=0.0, bus="out", slot=1):
    """The console EQ (Vint › Console): hpf 50/80/160/300 Hz; low (Hz of 35/60/110/220, dB);
    mid (Hz of 360/700/1600/3200/4800/7200, dB); high shelf dB."""
    p = dict(conHpf=[None, 50, 80, 160, 300].index(hpf), conHighGain=lin(high, -16, 16))
    if low:
        p.update(conLowFreq=[None, 35, 60, 110, 220].index(low[0]), conLowGain=lin(low[1], -16, 16))
    else:
        p.update(conLowFreq=0)
    if mid:
        p.update(conMidFreq=[360, 700, 1600, 3200, 4800, 7200].index(mid[0]), conMidGain=lin(mid[1], -18, 18))
    return vint("Console", bus=bus, slot=slot, **p)


def pultec(low_hz=60, low_boost=4, low_atten=0, high_hz="8k", high_boost=3, bandwidth=5, high_atten=0, drive=6, bus="out", slot=1):
    """The passive programme EQ: the low boost and cut together give the famous low bump."""
    return vint("Passive", drive, bus, slot, pasLowFreq=["20", "30", "60", "100"].index(str(low_hz)),
                pasLowBoost=low_boost / 10, pasLowAtten=low_atten / 10,
                pasHighFreq=["3k", "4k", "5k", "8k", "10k", "12k", "16k"].index(high_hz), pasHighBoost=high_boost / 10,
                pasBandwidth=bandwidth / 10, pasHighAtten=high_atten / 10)


def british(low=(0.5, 0.0), lm=None, hm=None, high=(0.5, 0.0), hpf=0.0, lpf=0.0, drive=6, bus="out", slot=1):
    """The British console EQ: (frequency 0..1, dB) per band; hpf/lpf 0 (out) .. 1."""
    p = dict(briHpf=hpf, briLpf=lpf, briLowFreq=low[0], briLowGain=lin(low[1], -16.5, 16.5),
             briHighFreq=high[0], briHighGain=lin(high[1], -16.5, 16.5))
    if lm:
        p.update(briLmFreq=lm[0], briLmGain=lin(lm[1], -16.5, 16.5), briLmQ=0.4)
    if hm:
        p.update(briHmFreq=hm[0], briHmGain=lin(hm[1], -16.5, 16.5), briHmQ=0.4)
    return vint("British", drive, bus, slot, **p)


def sampler(machine=0, rate=0.5, bits=0.5, drive=0.5, filt=1, bus="out", slot=1):
    """The sampler circuit: a classic sampling drum machine's converters (0 .. 2 the machine)."""
    return vint("Sampler", 0, bus, slot, smpMachine=machine, smpRate=rate, smpBits=bits, smpDrive=drive, smpFilter=filt)


def air(amount=50, bands=1, bus="out", slot=1):
    return vint("Air", 0, bus, slot, airAmount=amount / 100, airBands=bands, airRef=0.5)


def enhancer(kind=0, amount=50, tune=0.5, low=0, bus="out", slot=1):
    return vint("Enhancer", 0, bus, slot, enhType=kind, enhAmount=amount / 100, enhTune=tune, enhLow=low / 100)


def crush(bits=12, rate=24000, noise=0, mix=100, bus="out", slot=1):
    return fx(bus, slot, "lofi", bits=bits, sample_rate=rate, noise=noise, mix=mix)


def grit(circuit=0, drive=3.0, tone=5.0, bus="out", slot=1):
    return fx(bus, slot, "fs_dist", circuit=circuit, drive=drive / 10, tone=tone / 10, level=0.5)


# ---- the presets -------------------------------------------------------------------------------
P = []
DRUM_PAN = {"kick": 0, "snare": -4, "rim": -6, "clap": 4, "hat": -32, "pedal": -28, "open": -34, "tom_hi": -28,
            "tom_mid": -8, "tom_lo": 18, "tom_floor": 34, "crash": -42, "crash2": 46, "ride": 38, "bell": 38,
            "china": 50, "tamb": 22, "cowbell": -18, "shaker": 26, "clave": -14}


def preset(name, cat, about, source="", roles=None, fam=None, every=None, kit=None, effects=(), borrow=None,
           spread=None, demo=None):
    """roles: role → settings; fam: family → settings; every: settings for all pads; spread: pan the pads
    like a drummer's kit (synth kits' mono voices; on by default for them); demo: (style, groove), the
    source style's first groove by default."""
    m = roles_of(source)
    pads = {}
    if every:
        pads["all"] = dict(every)
    for f, v in (fam or {}).items():
        pads[f] = dict(v)
    if spread is None:
        spread = source not in RECORDED
    merged = {}
    if source in RECORDED:  # the kit's hats choke each other, nothing else does
        for r, n in m.items():
            merged.setdefault(r, {})["choke"] = 1 if r in HATS else 0
    if spread:
        for r, pan in DRUM_PAN.items():
            if r in m:
                merged.setdefault(r, {})["pan"] = pan
    for r, v in (roles or {}).items():
        if r not in m:
            raise SystemExit(f"{name}: no {r} in {source or 'Juke'}")
        merged.setdefault(r, {}).update(v)
    for r, v in merged.items():
        pads[str(m[r])] = v
    d = {"name": name, "category": cat, "about": about}
    if source:
        d["source"] = source
    if demo is None:
        if source not in STYLES:
            raise SystemExit(f"{name}: needs a demo (style, groove)")
        demo = (source, STYLES[source][0])
    style, groove = demo if isinstance(demo, tuple) else (demo, STYLES[demo][0])
    if groove not in STYLES[style]:
        raise SystemExit(f"{name}: no groove {groove} in {style}")
    d["demo"] = {"style": style, "groove": groove}
    if borrow:
        d["borrow"] = {str(m[r]): {"kit": k, "pad": RECORDED[k][fr]} for r, (k, fr) in borrow.items()}
    d["pads"] = pads
    d["kit"] = dict(kit or {})
    d["fx"] = list(effects)
    P.append(d)


# == Acoustic: SM Drums =========================================================================
S = "sm_drums"
preset("Studio Dry", "Acoustic", "Close and dry: shells cut short, a console EQ and a gentle bus compressor", S,
       {"kick": {"decay": 650, "level": 1}, "snare": {"decay": 850}, "rimshot": {"decay": 850}, "tom_hi": {"decay": 1200},
        "tom_mid": {"decay": 1300}, "tom_lo": {"decay": 1500}, "tom_floor": {"decay": 1800}, "hat": {"decay": 900}},
       effects=[eq(hpf=50, low=(60, 2), mid=(360, -2), high=2), comp("G Bus", 35, 35, 60, 40, 2, sc_hpf=90, slot=2)],
       demo=("Alternative", "Straight 8ths"))
preset("Studio Room", "Acoustic", "A mid-size live room around the kit, glued on the bus", S,
       {"kick": {"send_a": 8}, "snare": {"send_a": 28}, "rimshot": {"send_a": 28}, "tom_hi": {"send_a": 25}, "tom_mid": {"send_a": 25},
        "tom_lo": {"send_a": 25}, "tom_floor": {"send_a": 25}, "crash": {"send_a": 15}, "ride": {"send_a": 10}, "hat": {"send_a": 8}},
       kit={"return_a": -5},
       effects=[room(24, 32, 12, "Wood Panel", "Hardwood", "Drywall", 1.0, 5), eq(hpf=50, mid=(360, -2), high=1.5, slot=1),
                comp("G Bus", 35, 40, 55, 45, 2, sc_hpf=90, slot=2)],
       demo=("Pop", "Pop Ballad"))
preset("Arena", "Acoustic", "Huge hall on the snare and toms, a driven kick, hot tape on the bus", S,
       {"kick": {"tune": -1, "drive": 12, "send_a": 5}, "snare": {"send_a": 45, "tune": -1}, "tom_hi": {"send_a": 40},
        "tom_mid": {"send_a": 40}, "tom_lo": {"send_a": 40, "decay": 2600}, "tom_floor": {"send_a": 40, "decay": 3000},
        "crash": {"send_a": 12}, "china": {"send_a": 12}},
       kit={"return_a": -3},
       effects=[reverb("Hall", 2.8, 85, 35, 180, 8000, 5000, 140), pultec(60, 4, 2, "10k", 3, 6, slot=1),
                comp("1176", 50, 55, 35, 40, 3, sc_hpf=100, slot=2), tape("Multitrack", "15 ips", 14, 20, slot=3)],
       demo=("Hard Rock", "Rock Beat"))
preset("Classic Rock", "Acoustic", "A brick room, parallel 1176 and a British console EQ", S,
       {"snare": {"send_a": 30}, "tom_hi": {"send_a": 22}, "tom_mid": {"send_a": 22}, "tom_lo": {"send_a": 22}, "tom_floor": {"send_a": 22},
        "kick": {"send_a": 10, "drive": 8}},
       kit={"return_a": -4},
       effects=[room(20, 28, 14, "Bare Brick", "Painted Concrete", "Hard Plaster", 0.9, 8),
                comp("1176", 70, 75, 20, 30, 6, parallel=0, slot=1), british((0.45, 3), hm=(0.6, 2.5), high=(0.6, 2), drive=8, slot=2)],
       demo=("Hard Rock", "Ride Groove"))
preset("Motown Box", "Acoustic", "A tuned-up snare, mono and dark, slammed into valve tape in a tiled room", S,
       {"snare": {"tune": 2, "decay": 450, "send_a": 30}, "rimshot": {"tune": 2, "decay": 450}, "kick": {"decay": 500, "filter": 1, "cutoff": 5000}},
       every={"filter": 1, "cutoff": 7000}, kit={"return_a": -8},
       effects=[room(12, 16, 9, "Glazed Tile", "Hardwood", "Hard Plaster", 0.6, 0, 200, 7000),
                tape("Valve", "7.5 ips", 17, 30, 8), comp("LA-2A", 40, 40, 50, 50, 2, slot=2)],
       demo=("'60s Rock", "Beat Group"))
preset("Metal Tight", "Acoustic", "A clicky, driven kick, short toms, transients up and hard compression", S,
       {"kick": {"drive": 25, "decay": 300, "tune": 1, "level": 1.5}, "snare": {"tune": 1, "decay": 1100},
        "tom_hi": {"decay": 800, "tune": -1}, "tom_mid": {"decay": 850, "tune": -1}, "tom_lo": {"decay": 950, "tune": -1},
        "tom_floor": {"decay": 1100, "tune": -1}, "china": {"level": -1}},
       effects=[punch(8, -6), british((0.4, 2), lm=(0.35, -4), high=(0.7, 4), drive=6, slot=2), comp("1176", 60, 70, 25, 25, 3, sc_hpf=100, slot=3)],
       demo=("Metal", "Double Kick"))
preset("Gated Eighties", "Acoustic", "Snare and toms into a big room cut off short, bright '80s top", S,
       {"snare": {"send_a": 55, "decay": 700}, "tom_hi": {"send_a": 45}, "tom_mid": {"send_a": 45}, "tom_lo": {"send_a": 45},
        "tom_floor": {"send_a": 45}, "kick": {"decay": 450}},
       kit={"return_a": -2},
       effects=[reverb("Room", 0.45, 90, 0, 200, 9000, 9000, 150, "1980s"), comp("Ross", 75, 85, 10, 15, 5, bus="a", slot=2),
                air(60, 1), comp("G Bus", 40, 45, 40, 40, 2, slot=2)],
       demo=("Pop", "Dance Pop"))
preset("Nashville", "Acoustic", "Tuned-up and bright, a plate on the snare, air on top", S,
       {"snare": {"tune": 1, "send_b": 25}, "tom_hi": {"tune": 2}, "tom_mid": {"tune": 2}, "tom_lo": {"tune": 2}, "tom_floor": {"tune": 1}},
       kit={"return_b": -6},
       effects=[reverb("Plate", 1.6, 50, 20, 200, 10000, 8000, 120, bus="b"), air(45, 1),
                comp("G Bus", 30, 35, 60, 45, 1.5, sc_hpf=90, slot=2)],
       demo=("Country", "Train Beat"))
preset("Funk Pocket", "Acoustic", "A tight, tuned-up snare, crisp hats, a dead kick, desk saturation", S,
       {"snare": {"tune": 3, "decay": 380}, "rimshot": {"tune": 3, "decay": 380}, "kick": {"decay": 350, "filter": 1, "cutoff": 6000},
        "hat": {"tune": 1, "decay": 600}, "open": {"decay": 1200}},
       effects=[sat("Desk", 17, 3), eq(hpf=50, mid=(360, -3), high=3, slot=2), comp("1176", 45, 45, 40, 35, 2, slot=3)],
       demo=("Funk", "Ghosted"))
preset("Prog Toms", "Acoustic", "Long, deep toms and a big hall for fills", S,
       {"tom_hi": {"tune": -1, "decay": 3000, "send_a": 35}, "tom_mid": {"tune": -1, "decay": 3000, "send_a": 35},
        "tom_lo": {"tune": -2, "decay": 3500, "send_a": 35}, "tom_floor": {"tune": -2, "decay": 4000, "send_a": 35},
        "snare": {"send_a": 25}},
       kit={"return_a": -4},
       effects=[reverb("Hall", 2.2, 75, 25, 150, 9000, 6000), pultec(30, 3, 0, "12k", 2, 7), comp("G Bus", 35, 40, 55, 50, 2, sc_hpf=90, slot=2)],
       demo=("Alternative", "Tom Groove"))
preset("Punk Garage", "Acoustic", "Loud, trashy and driven through a fuzz pedal", S,
       {"kick": {"drive": 25}, "snare": {"drive": 30, "send_a": 20}, "crash": {"level": 1}},
       every={"drive": 15}, kit={"return_a": -8},
       effects=[room(14, 18, 9, "Painted Concrete", "Painted Concrete", "Drywall", 0.7), grit(5, 4, 6),
                comp("1176", 65, 80, 20, 25, 3, slot=2)],
       demo=("Alternative", "Driving 16ths"))
preset("Reggae Rim", "Acoustic", "Cross-stick up front into spring reverb and dub echo, warm tape", S,
       {"rim": {"level": 2, "send_a": 30, "send_b": 25}, "snare": {"tune": 3, "decay": 500, "send_b": 20},
        "kick": {"decay": 600, "filter": 1, "cutoff": 3000}},
       kit={"return_a": -6, "return_b": -6},
       effects=[reverb("Spring", 1.8, 40, 0, 200, 6000, 5000, bus="a"), delay("1/8.", 45, "Tape", 250, 3500, bus="b"),
                tape("Two-Track", "7.5 ips", 13, 10)],
       demo=("Soft Rock", "Cross-Stick"))
preset("Indie Bedroom", "Acoustic", "Mono, rolled-off, on a cassette four-track in a small room", S,
       {"kick": {"filter": 1, "cutoff": 4000}},
       every={"filter": 1, "cutoff": 6500, "send_a": 18}, kit={"return_a": -6},
       effects=[room(10, 12, 8, "Drywall", "Carpet", "Drywall", 0.5, 0, 200, 8000), tape("Cassette", "7.5 ips", 15, 0, 35, -10),
                comp("Vari-Mu", 45, 40, 50, 60, 2, slot=2)],
       demo=("Alternative", "Straight 8ths"))
preset("Breaks", "Acoustic", "SM Drums as a chopped break: a 12-bit sampler, FET crunch, squashed", S,
       {"kick": {"drive": 20, "level": 1}, "snare": {"drive": 20, "tune": 1}},
       every={"filter": 1, "cutoff": 8000},
       effects=[sampler(0, 0.45, 0.4, 0.7), comp("1176", 75, 85, 15, 20, 5, slot=2), sat("FET", 19, slot=3)],
       demo=("Funk", "Syncopated"))

# == Acoustic: SM Drums 17 ======================================================================
S = "sm_drums_17"
preset("Jazz Club", "Acoustic", "The 17\" ride forward, tuned-up shells, soft dynamics, a ribbon in a small club", S,
       {"kick": {"tune": 3, "decay": 600, "level": -2}, "snare": {"tune": 3}, "snare2": {"tune": 3},
        "tom_hi": {"tune": 4, "decay": 900}, "tom_mid": {"tune": 4, "decay": 900}, "tom_lo": {"tune": 4, "decay": 1000},
        "tom_floor": {"tune": 3, "decay": 1200}, "ride": {"level": 2}, "bell": {"level": 1}},
       every={"velocity": 55, "send_b": 18}, kit={"return_b": -6, "humanize": 15},
       effects=[room(16, 22, 10, "Wood Panel", "Hardwood", "Wood Panel", 0.7, 5, 150, 8000, "Ribbon", bus="b"),
                tape("Two-Track", "15 ips", 11), comp("Vari-Mu", 25, 30, 60, 60, 1, slot=2)],
       demo=("'50s Rock", "Shuffle"))
preset("Bebop", "Acoustic", "Bright ride and crisp snare, light and fast, a valve EQ lifting the top", S,
       {"kick": {"tune": 4, "decay": 450, "level": -3}, "snare": {"tune": 4, "decay": 600}, "ride": {"level": 3, "tune": 1},
        "tom_hi": {"tune": 5}, "tom_mid": {"tune": 5}, "hat": {"level": -1}},
       every={"velocity": 60, "hum_timing": 2}, kit={"humanize": 10},
       effects=[pultec(30, 1, 2, "12k", 4, 7, drive=4), room(16, 22, 10, "Wood Panel", "Hardwood", "Wood Panel", 0.6, bus="b"),
                comp("LA-2A", 20, 30, 60, 60, 1, slot=2)],
       demo=("Country", "Shuffle"))
preset("Seventies Dead", "Acoustic", "Deadened shells, everything short, slow tape and a dark console", S,
       {"snare": {"tune": -2, "decay": 380}, "snare2": {"decay": 380}, "kick": {"filter": 1, "cutoff": 4000, "decay": 380},
        "tom_hi": {"tune": -2, "decay": 500}, "tom_mid": {"tune": -2, "decay": 500}, "tom_lo": {"tune": -2, "decay": 550},
        "tom_floor": {"tune": -2, "decay": 600}, "hat": {"decay": 500}, "crash": {"decay": 2200}, "crash2": {"decay": 2200}},
       effects=[tape("Multitrack", "7.5 ips", 15, 25, 8), british((0.4, 3), hm=(0.6, -3), high=(0.5, -3), drive=8, slot=2),
                comp("LA-2A", 40, 40, 50, 50, 2, slot=3)],
       demo=("Funk", "Ghosted"))
preset("Ballad Soft", "Acoustic", "Softened attacks, a rounded top and a long plate", S,
       {"kick": {"send_a": 8}, "snare": {"attack": 3, "send_a": 35}, "snare2": {"attack": 3, "send_a": 35}},
       every={"filter": 1, "cutoff": 9000, "send_a": 25}, kit={"return_a": -4},
       effects=[reverb("Plate", 2.6, 60, 30, 200, 9000, 6000, 140), comp("LA-2A", 30, 30, 60, 60, 1.5)],
       demo=("Soft Rock", "Ballad"))
preset("Live 17", "Acoustic", "As played: a little loose in level and time, in the room, light desk warmth", S,
       {"kick": {"send_a": 8}},
       every={"hum_level": 1.0, "hum_timing": 3, "hum_pitch": 5, "send_a": 20}, kit={"return_a": -6},
       effects=[room(22, 30, 12, "Wood Panel", "Hardwood", "Drywall", 1.0, 5), sat("Desk", 14), comp("G Bus", 30, 35, 55, 45, 2, sc_hpf=90, slot=2)],
       demo=("Hard Rock", "Rock Beat"))
preset("Garage 17", "Acoustic", "Pushed hard: driven pads into iron saturation", S,
       {"crash": {"drive": 10}, "crash2": {"drive": 10}, "ride": {"drive": 10}},
       every={"drive": 35, "send_a": 15}, kit={"return_a": -8},
       effects=[room(14, 18, 9, "Painted Concrete", "Painted Concrete", "Drywall", 0.6), sat("Iron", 20, -3),
                comp("1176", 55, 65, 25, 30, 3, slot=2)],
       demo=("Alternative", "Driving 16ths"))
preset("Worship Wash", "Acoustic", "Long cymbals and toms washed in a hall, gently compressed", S,
       {"crash": {"send_a": 50}, "crash2": {"send_a": 50}, "ride": {"send_a": 35}, "snare": {"send_a": 30},
        "tom_hi": {"send_a": 40}, "tom_mid": {"send_a": 40}, "tom_lo": {"send_a": 40}, "tom_floor": {"send_a": 40}, "kick": {"send_a": 10}},
       kit={"return_a": -3},
       effects=[reverb("Hall", 3.5, 85, 40, 200, 8000, 5000, 160), comp("G Bus", 25, 30, 60, 55, 1.5, sc_hpf=90)],
       demo=("Worship", "Six-Eight Swell"))
preset("Singer-Songwriter", "Acoustic", "Quiet and close: the dry 6.5\" snare, soft hats, a little room, light tape", S,
       {"snare2": {"level": 1}, "snare": {"level": -3}, "kick": {"level": -1, "decay": 600}, "hat": {"level": -2}},
       every={"velocity": 45, "send_a": 12, "filter": 1, "cutoff": 10000}, kit={"return_a": -8},
       effects=[room(16, 20, 10, "Wood Panel", "Carpet", "Drywall", 0.6), tape("Two-Track", "15 ips", 10), comp("LA-2A", 25, 30, 60, 60, 1, slot=2)],
       demo=("Folk", "Boom-Chick"))
preset("Soul Seventies", "Acoustic", "A fat, low snare and thumpy kick through hot tape and a passive EQ", S,
       {"snare": {"tune": -3, "decay": 600}, "kick": {"decay": 450, "filter": 1, "cutoff": 3500}, "half": {"level": 1}},
       every={"filter": 1, "cutoff": 8000},
       effects=[tape("Multitrack", "15 ips", 16, 20, 10), pultec(100, 4, 3, "5k", 2, 4, high_atten=3, slot=2), comp("LA-2A", 45, 40, 50, 55, 2, slot=3)],
       demo=("Funk", "Ghosted"))
preset("Half-Open Funk", "Acoustic", "Sizzling half-open hats over a tight 6.7\" snare, FET snap", S,
       {"half": {"level": 1, "decay": 1200}, "snare": {"tune": 2, "decay": 450}, "kick": {"decay": 380}},
       effects=[sat("FET", 18, 2), eq(hpf=50, mid=(360, -3), high=3, slot=2), comp("1176", 45, 45, 40, 35, 2, slot=3)],
       demo=("Funk", "Syncopated"))
preset("Cymbal Swells", "Acoustic", "Cymbals in a shimmering cloud for ambient builds", S,
       {"crash": {"send_a": 70, "attack": 40}, "crash2": {"send_a": 70, "attack": 40}, "ride": {"send_a": 50}, "kick": {"send_a": 5}},
       kit={"return_a": -2},
       effects=[reverb("Shimmer", 6.0, 90, 50, 250, 9000, 6000, 180), reverb("Cloud", 4.0, 80, 20, 200, 8000, 6000, 160, bus="b"),
                comp("Vari-Mu", 25, 30, 60, 60, 1)],
       demo=("Worship", "Build"))

# == Acoustic: SM Drums Hybrid ==================================================================
S = "sm_drums_hybrid"
preset("Hybrid Pop", "Acoustic", "The hybrid snares bright and tight, a plate on the snare, air on top", S,
       {"snare": {"decay": 900, "send_b": 30}, "snare2": {"decay": 900, "send_b": 30}, "hat": {"tune": 1}, "kick": {"decay": 650}},
       kit={"return_b": -5},
       effects=[reverb("Plate", 1.8, 50, 20, 200, 10000, 8000, 130, bus="b"), punch(5, -2), air(50, 1, slot=2),
                comp("G Bus", 45, 45, 50, 45, 2, sc_hpf=90, slot=3)],
       demo=("Pop", "Dance Pop"))
preset("Big Hybrid", "Acoustic", "Hybrid snares over a heavy kick, valve-saturated and glued", S,
       {"kick": {"drive": 20, "tune": -1}, "snare": {"send_a": 30}, "snare3": {"send_a": 30},
        "tom_hi": {"send_a": 25}, "tom_mid": {"send_a": 25}, "tom_lo": {"send_a": 25}, "tom_floor": {"send_a": 25}},
       kit={"return_a": -4},
       effects=[room(30, 40, 16, "Bare Brick", "Hardwood", "Hard Plaster", 1.2, 10), sat("Valve", 19, -2), comp("G Bus", 50, 50, 45, 45, 2.5, sc_hpf=90, slot=2)],
       demo=("Hard Rock", "Rock Beat"))
preset("Gospel Live", "Acoustic", "A tuned-up hybrid snare and a lively room, a little swing in the hands", S,
       {"snare": {"tune": 2}, "snare2": {"tune": 2}, "kick": {"send_a": 8}},
       every={"send_a": 22, "hum_timing": 2, "hum_level": 0.8}, kit={"humanize": 10, "return_a": -5},
       effects=[room(30, 42, 18, "Hard Plaster", "Hardwood", "Hard Plaster", 1.3, 10), comp("1176", 45, 45, 40, 40, 2)],
       demo=("Gospel / Shout", "Gospel Chops"))
preset("Hybrid Rock", "Acoustic", "SM Drums' kick and toms with the hybrid snare, punchy in a room", "sm_drums",
       {"snare": {"send_a": 35}, "kick": {"drive": 10}},
       kit={"return_a": -5},
       effects=[room(22, 30, 12, "Bare Brick", "Hardwood", "Drywall", 1.0, 5), punch(5, -2), comp("1176", 50, 55, 35, 35, 2.5, slot=2)],
       borrow={"snare": ("sm_drums_hybrid", "snare")},
       demo=("Alternative", "Driving 16ths"))
preset("Modern Worship", "Acoustic", "Big hybrid snare 2, deep toms and a long hall", S,
       {"snare2": {"level": 2, "send_a": 35}, "snare": {"level": -4}, "tom_lo": {"tune": -2, "decay": 3500, "send_a": 35},
        "tom_floor": {"tune": -3, "decay": 4000, "send_a": 35}, "kick": {"tune": -1, "send_a": 8}},
       kit={"return_a": -3},
       effects=[reverb("Hall", 3.2, 85, 35, 180, 8500, 5500, 150), pultec(60, 3, 1, "10k", 2, 6, slot=2),
                comp("G Bus", 35, 40, 55, 50, 2, sc_hpf=90, slot=3)],
       demo=("Worship", "Build"))
preset("Stadium Pop", "Acoustic", "Hybrid snare 3 cracking through a huge room, enhanced top", S,
       {"snare3": {"level": 2, "send_a": 40}, "snare": {"level": -3}, "kick": {"drive": 10, "level": 1}},
       kit={"return_a": -3},
       effects=[reverb("Room", 1.6, 95, 15, 200, 9000, 7000, 170), punch(6, -3), enhancer(0, 45, 0.6, 20, slot=2), comp("1176", 55, 60, 30, 35, 3, slot=3)],
       demo=("Pop", "Four on the Floor"))
preset("Pop-Punk", "Acoustic", "Fast and bright: a tight hybrid snare, driven kick, crashes up", S,
       {"snare": {"tune": 2, "decay": 800}, "kick": {"drive": 20, "decay": 450}, "crash": {"level": 1}, "crash2": {"level": 1}},
       effects=[punch(7, -4), british((0.4, 2), lm=(0.35, -3), high=(0.7, 3), drive=6, slot=2), comp("1176", 60, 65, 25, 30, 3, sc_hpf=100, slot=3)],
       demo=("Alternative", "Driving 16ths"))
preset("R&B Pocket", "Acoustic", "A soft hybrid rim and snare 2, dark and warm, valve tape", S,
       {"rimshot": {"level": 2}, "snare2": {"tune": -2, "decay": 700}, "kick": {"tune": -2, "decay": 600}, "hat": {"level": -2}},
       every={"filter": 1, "cutoff": 7500, "send_b": 15}, kit={"return_b": -7},
       effects=[reverb("Plate", 1.4, 40, 25, 250, 7000, 5000, bus="b"), tape("Valve", "15 ips", 14, 15), comp("LA-2A", 40, 40, 50, 55, 2, slot=2)],
       demo=("Lo-fi", "Rimshot"))
preset("Neo-Soul", "Acoustic", "Loose and dusty: humanized hands, slow warm tape, a short room", S,
       {"snare": {"tune": -1}, "rimshot": {"level": 1}},
       every={"hum_timing": 5, "hum_level": 1.2, "filter": 1, "cutoff": 7000, "send_a": 15}, kit={"humanize": 20, "return_a": -7},
       effects=[room(14, 18, 10, "Wood Panel", "Carpet", "Drywall", 0.6), tape("Multitrack", "7.5 ips", 15, 20, 15), comp("Vari-Mu", 40, 40, 50, 60, 2, slot=2)],
       demo=("Lo-fi", "Lazy Swing"))

# == Acoustic: mixed kits =======================================================================
preset("Mixed Kit", "Acoustic", "SM Drums 17's cymbals and hats with SM Drums' snare and the hybrid rim", "sm_drums_17",
       {"kick": {"send_a": 8}}, every={"send_a": 15}, kit={"return_a": -7},
       effects=[room(22, 30, 12, "Wood Panel", "Hardwood", "Drywall", 1.0), comp("G Bus", 35, 40, 55, 45, 2, sc_hpf=90)],
       borrow={"snare": ("sm_drums", "snare"), "snare2": ("sm_drums_hybrid", "rimshot")},
       demo=("Pop", "Pop Ballad"))
preset("Ride Kit", "Acoustic", "SM Drums with the 17\" ride and its bell", "sm_drums",
       {"ride": {"level": 1}, "kick": {"send_a": 8}}, every={"send_a": 15}, kit={"return_a": -7},
       effects=[room(22, 30, 12, "Wood Panel", "Hardwood", "Drywall", 1.0), comp("G Bus", 30, 35, 55, 45, 2, sc_hpf=90)],
       borrow={"ride": ("sm_drums_17", "ride"), "bell": ("sm_drums_17", "bell")},
       demo=("Hard Rock", "Ride Groove"))
preset("Big Room 17", "Acoustic", "SM Drums 17 in a tall hall-sized room, the 15\" crash wide", "sm_drums_17",
       {"kick": {"send_a": 12, "tune": -1}, "snare": {"send_a": 35}, "crash": {"send_a": 30}, "ride": {"send_a": 20}},
       every={"send_a": 25}, kit={"return_a": -3},
       effects=[room(50, 70, 30, "Rough Stone", "Painted Concrete", "Hard Plaster", 1.8, 15, 120, 10000, "Tube Condenser"),
                comp("G Bus", 40, 40, 30, 50, 2, sc_hpf=90)],
       demo=("Praise", "Upbeat"))
preset("Vintage Studio", "Acoustic", "SM Drums through ribbon room mics, a passive EQ and valve tape", "sm_drums",
       {"kick": {"filter": 1, "cutoff": 5000, "send_a": 10}, "snare": {"send_a": 25}, "hat": {"level": -1}},
       every={"send_a": 18}, kit={"return_a": -5},
       effects=[room(22, 30, 14, "Wood Panel", "Hardwood", "Hard Plaster", 0.9, 5, 150, 9000, "Ribbon"),
                pultec(60, 3, 2, "8k", 2, 5, high_atten=2), tape("Valve", "15 ips", 15, 20, 6, slot=2), comp("Vari-Mu", 35, 35, 40, 55, 1.5, slot=3)],
       demo=("'60s Rock", "Beat Group"))
preset("Tight Hybrid Funk", "Acoustic", "The hybrid rim and a short snare 2, crisp hats, dry, desk-driven", "sm_drums_hybrid",
       {"snare2": {"tune": 3, "decay": 400, "level": 1}, "rimshot": {"decay": 400}, "kick": {"decay": 380},
        "hat": {"tune": 1, "decay": 700}, "open": {"decay": 1300}},
       effects=[sat("Desk", 16, 2), eq(hpf=50, mid=(360, -3), high=3, slot=2), comp("1176", 40, 45, 30, 35, 2, slot=3)],
       demo=("Funk", "Syncopated"))
preset("Dub Wise", "Acoustic", "Cross-stick and snare into a big spring and long, dark echoes, tape on everything", "sm_drums_17",
       {"rim": {"level": 2, "send_a": 35, "send_b": 35}, "snare": {"tune": 3, "decay": 500, "send_b": 30},
        "kick": {"decay": 550, "filter": 1, "cutoff": 2500}, "hat": {"filter": 1, "cutoff": 6000}},
       kit={"return_a": -5, "return_b": -4},
       effects=[reverb("Spring", 2.5, 50, 0, 200, 5000, 4000, bus="a"), delay("1/4.", 65, "Tape", 300, 2500, bus="b"),
                tape("Two-Track", "7.5 ips", 14, 10, 10)],
       demo=("Soft Rock", "Cross-Stick"))

# == Rock (style kits) ===========================================================================
preset("Alt Rock Room", "Rock", "The Alternative kit in a brick room, compressed", "Alternative",
       {"snare": {"send_a": 30}, "tom_hi": {"send_a": 25}, "tom_mid": {"send_a": 25}, "tom_lo": {"send_a": 25}, "kick": {"send_a": 8}},
       kit={"return_a": -5},
       effects=[room(20, 28, 14, "Bare Brick", "Hardwood", "Hard Plaster", 0.9), comp("1176", 45, 50, 35, 35, 2)])
preset("Hard Rock Stack", "Rock", "A driven, tuned-down kick and toms through iron saturation", "Hard Rock",
       {"kick": {"drive": 30, "tune": -2}, "snare": {"drive": 15, "send_a": 25}, "tom_hi": {"tune": -2, "drive": 15},
        "tom_mid": {"tune": -2, "drive": 15}, "tom_lo": {"tune": -2, "drive": 15}},
       kit={"return_a": -6},
       effects=[room(26, 36, 16, "Bare Brick", "Painted Concrete", "Hard Plaster", 1.1), sat("Iron", 19, -2), comp("1176", 50, 60, 0, 35, 2, slot=2)])
preset("Metal Machine", "Rock", "Clicky kick, cutting snare, a scooped British EQ, everything tight and loud", "Metal",
       {"kick": {"decay": 250, "drive": 20, "tone": 70}, "snare": {"snappy": 80, "decay": 260}, "tom_hi": {"decay": 400},
        "tom_mid": {"decay": 450}, "tom_lo": {"decay": 500}},
       effects=[punch(3, -4), british((0.4, 3), lm=(0.4, -5), high=(0.7, 4), drive=8, slot=2), comp("1176", 50, 65, 0, 35, 2, slot=3)],
       demo=("Metal", "Gallop"))
preset("Rockabilly Slap", "Rock", "A short, bright snare with slapback echo, hot two-track tape", "Rockabilly",
       {"snare": {"decay": 180, "tune": 3, "send_b": 35}, "kick": {"decay": 300}},
       kit={"return_b": -6}, spread=False,
       effects=[delay(ms=110, feedback=10, kind="Tape", low_cut=300, high_cut=4500, routing="Mono"), tape("Two-Track", "15 ips", 15, 20)])
preset("Fifties Echo", "Rock", "Mono and boxy with a tiled chamber and tape echo, through a valve machine", "'50s Rock",
       {"snare": {"send_a": 30, "send_b": 20}, "kick": {"filter": 1, "cutoff": 3000}},
       every={"filter": 1, "cutoff": 5500}, kit={"return_a": -6, "return_b": -8}, spread=False,
       effects=[room(12, 16, 10, "Glazed Tile", "Painted Concrete", "Hard Plaster", 1.4), delay(ms=140, feedback=20, kind="Tape", high_cut=3500, routing="Mono"),
                tape("Valve", "7.5 ips", 16, 30, 15, -8)])
preset("Sixties Mono", "Rock", "Mono, dark and slammed into tape", "'60s Rock",
       every={"filter": 1, "cutoff": 5000}, spread=False,
       effects=[tape("Multitrack", "7.5 ips", 18, 30, 20, -14), comp("LA-2A", 45, 40, 50, 50, 2, slot=2)])
preset("Soft Rock Ballad", "Rock", "Gentle and warm with a plate", "Soft Rock",
       {"snare": {"send_a": 30, "attack": 2}, "kick": {"send_a": 8}},
       every={"filter": 1, "cutoff": 9000}, kit={"return_a": -5},
       effects=[reverb("Plate", 2.2, 55, 25, 200, 9000, 6000), comp("LA-2A", 30, 30, 60, 60, 1.5)])
preset("Grunge", "Rock", "Fat, low snare and a dirty room, through a fuzz", "Alternative",
       {"snare": {"tune": -3, "drive": 20, "send_a": 30}, "kick": {"drive": 20, "tune": -1}},
       every={"drive": 10}, kit={"return_a": -5},
       effects=[room(20, 26, 12, "Painted Concrete", "Painted Concrete", "Drywall", 0.9, drive=30), grit(6, 4, 4), comp("1176", 60, 70, 25, 30, 3, slot=2)],
       demo=("Alternative", "Straight 8ths"))
preset("Surf Spring", "Rock", "Bright and splashy, mono, drowned in spring", "'60s Rock",
       {"snare": {"send_a": 40}, "tom_hi": {"send_a": 35}, "tom_mid": {"send_a": 35}, "tom_lo": {"send_a": 35}},
       every={"send_a": 20, "pan": 0}, kit={"return_a": -4}, spread=False,
       effects=[reverb("Spring", 2.2, 50, 0, 200, 6000, 5000), tape("Two-Track", "15 ips", 13, 10)],
       demo=("'60s Rock", "Surf"))
preset("Arena Synth Rock", "Rock", "Hard Rock's kit in a big hall with hot tape on the bus", "Hard Rock",
       {"snare": {"send_a": 40}, "tom_hi": {"send_a": 35}, "tom_mid": {"send_a": 35}, "tom_lo": {"send_a": 35}, "kick": {"drive": 15}},
       kit={"return_a": -4},
       effects=[reverb("Hall", 2.6, 85, 30, 200, 8500, 6000, 150), tape("Multitrack", "15 ips", 14, 20), comp("G Bus", 40, 45, 30, 45, 2, sc_hpf=90, slot=2)],
       demo=("Hard Rock", "Fill"))

# == Pop & Folk ==================================================================================
preset("Pop Punch", "Pop & Folk", "Transients up, compressed, a plate on the snare and clap", "Pop",
       {"snare": {"send_b": 30}, "clap": {"send_b": 25}, "kick": {"level": 1}},
       kit={"return_b": -5},
       effects=[reverb("Plate", 1.6, 45, 20, 250, 10000, 8000, 130, bus="b"), punch(2, 0), comp("G Bus", 45, 50, 5, 40, 2, sc_hpf=90, slot=2)])
preset("Pop Glossy", "Pop & Folk", "Bright, wide and polished: air and an enhancer on top", "Pop",
       {"hat": {"tune": 2}, "clap": {"send_a": 25}, "snare": {"send_a": 20}},
       kit={"return_a": -6},
       effects=[reverb("Hall", 1.8, 70, 30, 250, 11000, 9000, 170), air(65, 2), comp("G Bus", 40, 45, 5, 45, 2, sc_hpf=90, slot=2)],
       demo=("Pop", "Dance Pop"))
preset("Folk Porch", "Pop & Folk", "Soft and round, on a wooden porch, light tape", "Folk",
       {"kick": {"level": -2}}, every={"filter": 1, "cutoff": 7000, "attack": 2, "send_a": 20}, kit={"return_a": -7, "humanize": 15},
       effects=[room(12, 16, 9, "Wood Panel", "Hardwood", "Wood Panel", 0.6), tape("Two-Track", "15 ips", 11)])
preset("Folk Brushy", "Pop & Folk", "Soft attacks and short hats, like brushes", "Folk",
       {"snare": {"attack": 6, "decay": 250, "snappy": 90}, "hat": {"attack": 4, "decay": 60}},
       every={"velocity": 50}, kit={"humanize": 20},
       effects=[room(12, 16, 9, "Wood Panel", "Carpet", "Wood Panel", 0.5), comp("LA-2A", 25, 30, 60, 60, 1)],
       demo=("Folk", "Six-Eight"))
preset("Country Two-Step", "Pop & Folk", "A dry kit with a quick slap on the snare", "Country",
       {"snare": {"send_b": 25, "decay": 300}},
       kit={"return_b": -7},
       effects=[delay(ms=95, feedback=5, kind="Tape", routing="Mono"), comp("1176", 35, 40, 45, 40, 1.5)],
       demo=("Country", "Shuffle"))
preset("Country Arena", "Pop & Folk", "Big and bright for the big stage", "Country",
       {"snare": {"send_a": 35}, "tom_hi": {"send_a": 30}, "tom_mid": {"send_a": 30}, "tom_lo": {"send_a": 30}},
       kit={"return_a": -4},
       effects=[reverb("Hall", 2.0, 80, 25, 200, 9000, 6000, 150), air(40, 1), comp("G Bus", 45, 45, 5, 45, 2, sc_hpf=90, slot=2)])
preset("Synth Pop Eighties", "Pop & Folk", "A gated snare, bright hats and a glossy bus", "Pop",
       {"snare": {"send_a": 50}, "clap": {"send_a": 40}, "hat": {"tune": 2}},
       kit={"return_a": -3},
       effects=[reverb("Room", 0.45, 90, 0, 200, 9000, 9000, 160, "1980s"), comp("Ross", 75, 85, 10, 15, 4, bus="a", slot=2),
                air(55, 1), comp("G Bus", 35, 40, 30, 40, 1.5, slot=2)],
       demo=("Pop", "Dance Pop"))
preset("Indie Pop Lo-fi", "Pop & Folk", "Pop's kit on a cassette four-track", "Pop",
       every={"filter": 1, "cutoff": 7000, "pan": 0}, spread=False,
       effects=[tape("Cassette", "7.5 ips", 16, 0, 40, -8), comp("Vari-Mu", 40, 40, 40, 60, 2, slot=2)],
       demo=("Pop", "Pop Ballad"))

# == Church ======================================================================================
preset("Praise Big", "Church", "A big room and glue for a full band", "Praise",
       {"kick": {"send_a": 10, "drive": 10}}, every={"send_a": 30}, kit={"return_a": -4},
       effects=[room(40, 60, 24, "Hard Plaster", "Hardwood", "Hard Plaster", 1.6, 12), comp("G Bus", 45, 45, 5, 45, 2, sc_hpf=90)])
preset("Praise Bright", "Church", "Crisp and forward for a lively set", "Praise",
       {"snare": {"tune": 1}, "hat": {"tune": 1}},
       effects=[air(50, 1), punch(4, -2, slot=2), comp("1176", 40, 45, 40, 40, 2, slot=3)],
       demo=("Praise", "Four on the Floor"))
preset("Worship Pad Wash", "Church", "Long cymbals and a huge reverb under quiet verses", "Worship",
       {"crash": {"send_a": 60, "decay": 4000}, "ride": {"send_a": 45}, "kick": {"send_a": 10}},
       every={"send_a": 30}, kit={"return_a": -2},
       effects=[reverb("Hall", 4.5, 90, 40, 200, 8000, 5000, 170), comp("G Bus", 25, 30, 60, 55, 1.5, sc_hpf=90)],
       demo=("Worship", "Ballad"))
preset("Shout Church", "Church", "Loud and loose: humanized hands, valve-driven, a lively room", "Gospel / Shout",
       {"kick": {"send_a": 8}}, every={"hum_timing": 3, "hum_level": 1.2, "send_a": 25, "drive": 12},
       kit={"humanize": 15, "return_a": -5},
       effects=[room(30, 42, 18, "Hard Plaster", "Hardwood", "Hard Plaster", 1.3), sat("Valve", 18), comp("1176", 50, 55, 35, 35, 2.5, slot=2)],
       demo=("Gospel / Shout", "Shout"))
preset("Choir Loft", "Church", "Worship's kit from the back of a stone church", "Worship",
       {"kick": {"send_a": 25}}, every={"send_a": 45}, kit={"return_a": -2},
       effects=[room(60, 90, 40, "Rough Stone", "Rough Stone", "Hard Plaster", 2.5, 25, 150, 8000, "Tube Condenser"),
                comp("Vari-Mu", 30, 30, 40, 60, 1.5)],
       demo=("Worship", "Six-Eight Swell"))

# == Hip-Hop ======================================================================================
preset("Dusty Boom Bap", "Hip-Hop", "Through a 12-bit sampler, FET crunch and a hard 1176: a sampled record", "Boom Bap",
       every={"filter": 1, "cutoff": 6000, "drive": 20},
       effects=[sampler(0, 0.4, 0.4, 0.7), sat("FET", 18, slot=2), comp("1176", 55, 60, 30, 35, 2.5, slot=3)])
preset("Boom Bap Crisp", "Hip-Hop", "A cleaner boom bap: crisp snare, thick kick, a passive low bump", "Boom Bap",
       {"kick": {"drive": 15, "level": 1}, "snare": {"tune": 1, "decay": 220}},
       effects=[punch(6, -3), pultec(60, 5, 2, "5k", 2, 5, slot=2), comp("1176", 50, 55, 30, 35, 2.5, slot=3)],
       demo=("Boom Bap", "Busy Hats"))
preset("Boom Bap on Wood", "Hip-Hop", "Boom Bap's kick under SM Drums' snare and hats, through the sampler", "Boom Bap",
       every={"filter": 1, "cutoff": 7000},
       effects=[sampler(1, 0.5, 0.5, 0.6), comp("1176", 55, 60, 30, 35, 2.5, slot=2)],
       borrow={"snare": ("sm_drums", "snare"), "hat": ("sm_drums", "hat"), "pedal": ("sm_drums", "pedal"), "open": ("sm_drums", "open")},
       demo=("Boom Bap", "Half-Time"))
preset("Trap 808 Long", "Hip-Hop", "A long, sliding 808 with tight, bright hats, valve warmth under it", "Trap",
       {"kick": {"decay": 1800, "pitch_amount": 12, "pitch_decay": 60, "drive": 15}, "hat": {"tune": 2, "decay": 70},
        "pedal": {"tune": 2, "decay": 60}},
       effects=[sat("Valve", 16, -4), comp("G Bus", 45, 45, 40, 45, 2, slot=2)])
preset("Trap Hard", "Hip-Hop", "Hard transients, FET saturation and a short room on the claps", "Trap",
       {"clap": {"send_a": 30}, "snare": {"send_a": 25}},
       kit={"return_a": -6},
       effects=[reverb("Room", 0.6, 30, 0, 250, 9000, 8000, bus="a"), punch(9, -4), sat("FET", 19, slot=2), comp("1176", 55, 60, 25, 30, 2.5, slot=3)])
preset("Drill Slide", "Hip-Hop", "A gliding, distorted 808 and quick, dry hats", "Trap",
       {"kick": {"decay": 2400, "pitch_amount": 7, "pitch_decay": 180, "drive": 35, "tune": -2}, "hat": {"decay": 45, "tune": 3},
        "snare": {"tune": 2, "send_a": 20}, "rim": {"level": 2}},
       kit={"return_a": -7},
       effects=[reverb("Room", 0.5, 25, 0, 300, 9000, 8000), grit(1, 4, 5), comp("1176", 50, 55, 30, 35, 2, slot=2)])
preset("Lo-fi Tape", "Hip-Hop", "Wobbly cassette, hiss and a rolled-off top", "Lo-fi",
       every={"filter": 1, "cutoff": 3500},
       effects=[tape("Cassette", "7.5 ips", 18, 0, 55, -4), comp("Vari-Mu", 45, 40, 50, 60, 2, slot=2)])
preset("Lo-fi Vinyl", "Hip-Hop", "Dusty and narrow with crackle and swing in the hands, a 12-bit sampler", "Lo-fi",
       every={"filter": 1, "cutoff": 4500, "hum_timing": 6, "pan": 0}, kit={"humanize": 25},
       effects=[sampler(0, 0.35, 0.35, 0.6), crush(10, 22050, 10, 60, slot=2), comp("Vari-Mu", 40, 40, 50, 60, 2, slot=3)],
       demo=("Lo-fi", "Rimshot"))
preset("West Coast Bounce", "Hip-Hop", "A round kick, a snappy clap and bright hats, through a desk", "Funk",
       {"kick": {"decay": 700, "pitch_amount": 6, "tone": 30}, "clap": {"level": 1, "send_a": 20}, "hat": {"tune": 2}},
       kit={"return_a": -7},
       effects=[reverb("Plate", 1.0, 40, 10, 250, 9000, 7000), sat("Desk", 15, -2), comp("G Bus", 45, 45, 40, 40, 2, sc_hpf=90, slot=2)],
       demo=("Boom Bap", "Half-Time"))
preset("Memphis Cowbell", "Hip-Hop", "A dark, rattling 808, cowbell up front, crushed and taped", "Trap",
       {"kick": {"decay": 1500, "pitch_amount": 10, "drive": 25}, "cowbell": {"level": 3, "send_b": 20}, "hat": {"decay": 50}},
       every={"filter": 1, "cutoff": 6000}, kit={"return_b": -8},
       effects=[delay("1/8", 30, "BBD", 300, 4000), crush(12, 22050, 3), tape("Cassette", "7.5 ips", 14, 0, 20, slot=2), comp("1176", 45, 55, 30, 35, 2, slot=3)])
preset("Jazz Hop", "Hip-Hop", "A soft, dusty kit with SM Drums 17's ride, on worn cassette", "Lo-fi",
       {"ride": {"level": 2}}, every={"filter": 1, "cutoff": 5000, "hum_timing": 5}, kit={"humanize": 20},
       effects=[tape("Cassette", "7.5 ips", 15, 0, 45, -10), comp("Vari-Mu", 35, 35, 40, 60, 1.5, slot=2)],
       borrow={"ride": ("sm_drums_17", "ride")},
       demo=("Lo-fi", "Lazy Swing"))
preset("Lo-fi Live Room", "Hip-Hop", "The Lo-fi kit with SM Drums' snare and hats, in a small room on slow tape", "Lo-fi",
       {"snare": {"send_a": 20}}, every={"filter": 1, "cutoff": 6000}, kit={"return_a": -7},
       effects=[room(12, 16, 9, "Wood Panel", "Carpet", "Drywall", 0.6), tape("Multitrack", "7.5 ips", 14, 20, 20, slot=1),
                comp("Vari-Mu", 35, 35, 40, 60, 1.5, slot=2)],
       borrow={"snare": ("sm_drums", "snare"), "hat": ("sm_drums", "hat"), "open": ("sm_drums", "open")},
       demo=("Lo-fi", "Rimshot"))

# == Electronic ===================================================================================
preset("House Classic", "Electronic", "Long open hats, a clap in a small room, a sampler's converters", "House",
       {"open": {"decay": 450}, "clap": {"send_a": 25}},
       kit={"return_a": -6},
       effects=[reverb("Room", 0.9, 40, 10, 250, 9000, 7000), sampler(2, 0.7, 0.7, 0.4), comp("G Bus", 40, 40, 45, 40, 2, sc_hpf=90, slot=2)])
preset("Deep House", "Electronic", "Soft, round kick, shaker and rim in a dub space, valve-warm", "House",
       {"kick": {"tone": 25, "decay": 550}, "rim": {"send_b": 30}, "shaker": {"level": 2}, "hat": {"filter": 1, "cutoff": 8000}},
       kit={"return_b": -6},
       effects=[delay("1/8.", 40, "Tape", 300, 4000), sat("Valve", 15, -3), comp("Vari-Mu", 35, 35, 50, 55, 2, slot=2)],
       demo=("House", "Tribal"))
preset("Techno Warehouse", "Electronic", "A driven kick, big reverb and dub delay on the tops, iron on the bus", "Techno",
       {"kick": {"drive": 30, "decay": 600}, "hat": {"send_a": 30, "send_b": 20}, "open": {"send_a": 30}, "clap": {"send_a": 35},
        "rim": {"send_b": 30}, "clave": {"send_b": 30}},
       kit={"return_a": -5, "return_b": -7},
       effects=[reverb("Hall", 3.0, 90, 30, 300, 8000, 6000, 170), delay("1/8.", 55, "Tape", 300, 4000), sat("Iron", 17, -4),
                comp("G Bus", 50, 50, 40, 45, 2, sc_hpf=90, slot=2)])
preset("Minimal Click", "Electronic", "Short, clicky and tuned up, echoes on the percussion", "Techno",
       {"kick": {"decay": 350}, "rim": {"tune": 4, "send_b": 35}, "clave": {"tune": 4, "send_b": 35}, "cowbell": {"tune": 4, "send_b": 25}},
       every={"decay": 150}, kit={"return_b": -6},
       effects=[delay("1/16.", 45, "Digital", 400, 6000), enhancer(1, 40, 0.7, 0), comp("G Bus", 40, 40, 40, 40, 1.5, slot=2)],
       demo=("Techno", "Broken"))
preset("Industrial", "Electronic", "Distorted, metallic and loud: a fuzz and a steel room", "Techno",
       {"kick": {"drive": 50}, "snare": {"drive": 40}, "clap": {"drive": 40}},
       every={"drive": 25}, kit={"return_a": -6},
       effects=[room(30, 40, 20, "Steel Plate", "Painted Concrete", "Steel Plate", 1.4), grit(4, 6, 6), comp("1176", 60, 70, 20, 25, 3, slot=2)])
preset("Breakbeat Crunch", "Electronic", "Drum & Bass with FET crunch and snap", "Drum & Bass",
       {"snare": {"drive": 20}}, effects=[punch(8, -3), sat("FET", 19, slot=2), comp("1176", 55, 60, 25, 30, 2.5, slot=3)],
       demo=("Drum & Bass", "Roller"))
preset("Jungle Rinse", "Electronic", "Tuned-up breaks with a short room and a 12-bit sampler", "Drum & Bass",
       {"snare": {"tune": 3}, "kick": {"tune": 1}},
       every={"send_a": 15}, kit={"return_a": -8},
       effects=[room(14, 18, 10, "Bare Brick", "Painted Concrete", "Drywall", 0.6), sampler(0, 0.5, 0.45, 0.6, slot=1), comp("1176", 50, 60, 0, 35, 2, slot=2)],
       demo=("Drum & Bass", "Two-Step"))
preset("Liquid DnB", "Electronic", "Smooth and wide with a plate on the snare and air on top", "Drum & Bass",
       {"snare": {"send_b": 30}, "hat": {"tune": 1}},
       kit={"return_b": -6},
       effects=[reverb("Plate", 1.8, 60, 20, 250, 10000, 8000, 160, bus="b"), air(45, 2), comp("G Bus", 40, 40, 45, 45, 2, sc_hpf=90, slot=2)],
       demo=("Drum & Bass", "Roller"))
preset("Dembow Club", "Electronic", "Reggaeton tight and loud, a desk pushed", "Reggaeton",
       {"kick": {"drive": 10}}, effects=[punch(6, -2), sat("Desk", 16, slot=2), comp("1176", 50, 55, 30, 35, 2.5, slot=3)])
preset("Dembow Lite", "Electronic", "Lighter and roomier for pop reggaeton", "Reggaeton",
       {"snare": {"send_a": 20}, "clap": {"send_a": 20}}, kit={"return_a": -7},
       effects=[reverb("Room", 1.0, 50, 10, 250, 9000, 7000), comp("G Bus", 35, 40, 45, 45, 2)],
       demo=("Reggaeton", "Perreo"))
preset("Disco Funk", "Electronic", "Open hats, tambourine and a punchy four-on-the-floor kick, on 15 ips tape", "Funk",
       {"open": {"decay": 400, "level": 1}, "tamb": {"level": 2}, "kick": {"decay": 400, "drive": 10}},
       effects=[tape("Multitrack", "15 ips", 13, 10), eq(hpf=50, high=3, slot=2), comp("1176", 45, 50, 35, 35, 2, slot=3)],
       demo=("House", "Classic"))
preset("Acid Warehouse", "Electronic", "A hard kick, filtered hats and a wide, dark hall, iron-saturated", "Techno",
       {"kick": {"drive": 35, "decay": 500}, "hat": {"filter": 2, "cutoff": 6000, "resonance": 40, "send_a": 25},
        "open": {"filter": 2, "cutoff": 5000, "resonance": 40, "send_a": 25}, "clap": {"send_a": 35}},
       kit={"return_a": -5},
       effects=[reverb("Hall", 3.5, 95, 40, 400, 7000, 5000, 180), sat("Iron", 18, -3), comp("G Bus", 40, 45, 30, 45, 2, sc_hpf=90, slot=2)],
       demo=("Techno", "Driving"))
preset("UK Garage", "Electronic", "Skippy and bright: short hats, a crisp rim, a little room, enhanced top", "House",
       {"hat": {"decay": 50, "tune": 3}, "rim": {"level": 2, "send_a": 25}, "snare": {"tune": 2, "decay": 160}, "kick": {"decay": 400}},
       kit={"return_a": -7},
       effects=[reverb("Room", 0.8, 40, 5, 300, 10000, 8000), enhancer(0, 40, 0.6, 0), comp("G Bus", 35, 40, 30, 40, 1.5, sc_hpf=90, slot=2)],
       demo=("House", "Shuffled"))
preset("Dub Techno", "Electronic", "A deep kick, chords of echo on the rim and a dark concrete chamber", "Techno",
       {"kick": {"tone": 25, "decay": 600}, "rim": {"send_b": 50, "send_a": 30}, "hat": {"filter": 1, "cutoff": 7000, "send_a": 20}},
       kit={"return_a": -5, "return_b": -4},
       effects=[room(30, 40, 20, "Painted Concrete", "Painted Concrete", "Painted Concrete", 2.0, 20, 250, 6000),
                delay("1/8.", 75, "Tape", 400, 2500), tape("Two-Track", "15 ips", 12, 10), comp("Vari-Mu", 30, 30, 40, 60, 1.5, slot=2)],
       demo=("Techno", "Broken"))

# == Drum machines (Juke's own voices) =============================================================
preset("Analog 808", "Drum Machines", "A long, boomy kick, snappy snare, cowbell and claves, a passive low bump", "Porch Light",
       {"kick": {"decay": 1200, "pitch_amount": 12, "pitch_decay": 80, "tone": 30}, "snare": {"snappy": 80}, "hat": {"decay": 60},
        "cowbell": {"level": 1}},
       effects=[pultec(60, 5, 2, "8k", 2, 5), tape("Two-Track", "15 ips", 12, slot=2)],
       demo=("Trap", "Straight"))
preset("Analog 909", "Drum Machines", "A punchy kick, bright hats, a clap in a small room, desk-driven", "Porch Light",
       {"kick": {"decay": 450, "pitch_amount": 8, "drive": 15}, "hat": {"decay": 90, "tune": 2}, "clap": {"send_a": 20}},
       kit={"return_a": -7},
       effects=[reverb("Room", 0.7, 30, 5, 250, 9000, 8000), sat("Desk", 16), comp("1176", 45, 50, 35, 35, 2, slot=2)],
       demo=("House", "Classic"))
preset("Seven-Oh", "Drum Machines", "Clean, bright '80s machine: tight kick, crisp snare", "Porch Light",
       {"kick": {"decay": 300, "pitch_amount": 4, "tone": 60}, "snare": {"decay": 200, "snappy": 70, "tone": 70}, "hat": {"decay": 70, "tune": 3},
        "open": {"decay": 300, "tune": 3}, "tamb": {"level": 1}},
       effects=[air(50, 1), comp("G Bus", 35, 40, 45, 40, 1.5, slot=2)],
       demo=("Pop", "Dance Pop"))
preset("Rhythm Box", "Drum Machines", "A soft, ticking preset box on cassette: thin and charming", "Porch Light",
       {"kick": {"decay": 220, "tone": 80, "level": -2}, "snare": {"decay": 120, "snappy": 50}, "hat": {"decay": 40, "tune": 5},
        "rim": {"level": 1}, "clave": {"level": 1}},
       every={"filter": 1, "cutoff": 6000}, spread=False,
       effects=[tape("Cassette", "7.5 ips", 13, 0, 25, -12), room(12, 14, 9, "Wood Panel", "Carpet", "Drywall", 0.5, bus="a")],
       demo=("'50s Rock", "Backbeat"))
preset("Eighties Machine", "Drum Machines", "Big gated snare and toms from a classic sampling machine", "Porch Light",
       {"snare": {"send_a": 50, "decay": 250}, "tom_hi": {"send_a": 40}, "tom_mid": {"send_a": 40}, "tom_lo": {"send_a": 40},
        "clap": {"send_a": 40}},
       kit={"return_a": -3},
       effects=[reverb("Room", 0.4, 90, 0, 200, 9000, 9000, 160, "1980s"), comp("Ross", 75, 85, 10, 15, 5, bus="a", slot=2),
                sampler(1, 0.6, 0.6, 0.4), comp("G Bus", 40, 45, 40, 40, 2, slot=2)],
       demo=("Pop", "Dance Pop"))
preset("Tin Roof Garage", "Drum Machines", "Tin Roof driven into iron and squashed", "Tin Roof",
       {"kick": {"attack": 0.5}, "snare": {"attack": 0.5}},
       every={"drive": 25}, effects=[sat("Iron", 20, -2), comp("1176", 50, 60, 0, 40, 2, slot=2)],
       demo=("Alternative", "Straight 8ths"))
preset("Revival Hall", "Drum Machines", "Revival Tent in a big hall", "Revival Tent",
       {"kick": {"send_a": 10}}, every={"send_a": 35}, kit={"return_a": -3},
       effects=[reverb("Hall", 3.0, 85, 30, 200, 8000, 6000, 160)],
       demo=("Gospel / Shout", "Shout"))
preset("Muddy Water Swamp", "Drum Machines", "Muddy Water through a spring and valve tape", "Muddy Water",
       every={"send_a": 25, "filter": 1, "cutoff": 6000}, kit={"return_a": -6},
       effects=[reverb("Spring", 2.0, 40, 0, 200, 6000, 5000), tape("Valve", "7.5 ips", 16, 30, 15)],
       demo=("'50s Rock", "Shuffle"))
preset("Back Forty Dust", "Drum Machines", "Back Forty bit-crushed and narrow", "Back Forty",
       every={"pan": 0},
       effects=[crush(10, 18000, 6), comp("1176", 50, 55, 30, 35, 2, slot=2)],
       demo=("Country", "Train Beat"))
preset("Porch Light Clean", "Drum Machines", "Juke's own kit, balanced and gently glued", "Porch Light",
       effects=[comp("G Bus", 30, 35, 50, 45, 1.5, sc_hpf=90)],
       demo=("Pop", "Four on the Floor"))
preset("Glitch Box", "Drum Machines", "Short, pitched-up clicks into stutter delays", "Porch Light",
       {"kick": {"decay": 180}, "snare": {"decay": 90, "tune": 7}, "hat": {"decay": 25, "tune": 9}, "clave": {"tune": 7, "send_b": 40},
        "rim": {"tune": 5, "send_b": 40}},
       kit={"return_b": -5},
       effects=[delay("1/16", 60, "Digital", 500, 8000), crush(8, 16000, 0, 60)],
       demo=("Techno", "Broken"))
preset("Analog 606", "Drum Machines", "A small, bright machine: snappy, tight and buzzy hats, desk-warm", "Porch Light",
       {"kick": {"decay": 260, "pitch_amount": 5, "tone": 55}, "snare": {"decay": 150, "snappy": 85, "tone": 75},
        "hat": {"decay": 45, "tune": 6}, "open": {"decay": 250, "tune": 6}, "tom_hi": {"decay": 200}, "tom_mid": {"decay": 220},
        "tom_lo": {"decay": 250}},
       effects=[eq(hpf=50, high=3), sat("Desk", 15, slot=2)],
       demo=("Techno", "Driving"))


# ---- levels and output ---------------------------------------------------------------------------
def main():
    levels = {}
    if os.path.exists(LEVELS):
        levels = json.load(open(LEVELS))
    if "--levels" in sys.argv:
        report = json.load(open(sys.argv[sys.argv.index("--levels") + 1]))
        for r in report:
            name, loud, peak, level = r["name"], r["loud_db"], r["peak_db"], r["level"]
            gain = min(TARGET_DB - loud, PEAK_MAX_DB - peak)
            levels[name] = round(max(-30.0, min(6.0, level + gain)), 1)
        json.dump(dict(sorted(levels.items())), open(LEVELS, "w"), indent=1)
    for d in P:
        if d["name"] in levels:
            d["kit"]["level"] = levels[d["name"]]
    names = [d["name"] for d in P]
    dupes = {n for n in names if names.count(n) > 1}
    if dupes:
        raise SystemExit(f"duplicate names: {dupes}")
    json.dump(P, open(OUT, "w"), indent=1)
    print(f"{len(P)} presets -> {os.path.relpath(OUT)}")


main()
