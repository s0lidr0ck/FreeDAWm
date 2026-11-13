#!/usr/bin/env python3
"""Convert your own soothe2 preset files into Rez user presets, on your machine.

    python3 import_soothe2_presets.py <soothe2 Presets folder or .preset files...> [--out DIR] [--overwrite] [--dry-run]

Reads the XML that soothe2's preset browser saves (`<SOOTHE2STATE>` with `<PARAM id value>`),
maps each preset onto Rez's parameters and writes it as a FreeDAWm plugin preset
(`{"format": "soundcraft-plugin-preset", "plugin": "rez", "params": {...}}`). Nothing is bundled
with FreeDAWm: this only converts files you already have, for your own use.

By default the presets go to Rez's folder in your FreeDAWm Library
(`<settings folder>/Presets/rez/`), where Rez's preset menu and the plugin browser list them
under User. If you moved the Library in Setup, pass `--out "<your Library>/Presets/rez"`.

A preset in a category folder is named "<Category> - <Name>". Existing files are left alone
unless `--overwrite` is given.

The mapping follows the measurements Rez was fitted to:

- depth -10..20 becomes Depth 0..100 %; sharpness and selectivity 0..10 become 0..100 %;
- attack and release 0..10 become 15 ms..2 s and 10 ms..1.5 s on a log scale;
- soft/hard, mix, delta, trim (at most 12 dB here), left|right or mid|side, link and balance
  carry over;
- the low and high cut become Rez's Low and High; the four middle bands become Rez's four bands
  as bells (frequency, sensitivity, Q, on/off).

What Rez has no equal for is left out and reported: band shapes other than a bell, a band's own
stereo balance, quality and oversampling, and the sidechain switch (pick the key in the Mix
window instead). The result is a close starting point, not a guaranteed match: check by ear.
"""
import argparse, json, math, os, pathlib, re, sys
import xml.etree.ElementTree as ET

# Rez's parameters: id -> (min, max). Kept in step with crates/dsp/src/plugins/rez.rs.
RANGES = {
    "depth": (0, 100), "sharpness": (0, 100), "selectivity": (0, 100), "attack": (1, 2000), "release": (5, 2000),
    "low": (20, 20000), "high": (20, 20000), "mode": (0, 1), "mix": (0, 100), "delta": (0, 1), "trim": (-12, 12),
    "stereo": (0, 1), "link": (0, 100), "balance": (-100, 100),
}
for _b in range(1, 5):
    RANGES.update({f"b{_b}_on": (0, 1), f"b{_b}_freq": (20, 20000), f"b{_b}_gain": (-24, 12), f"b{_b}_q": (0.3, 8)})


def log_span(a, b, t):
    return a * (b / a) ** min(max(t, 0.0), 1.0)


def convert(xml_text):
    """(name, params, notes) for one preset's text, or None if it is not a soothe2 preset."""
    try:
        root = ET.fromstring(xml_text)
    except ET.ParseError:
        return None
    if root.tag != "SOOTHE2STATE":
        return None
    read = {}
    for p in root.iter("PARAM"):
        try:
            v = float(p.get("value", ""))
        except ValueError:
            continue
        if math.isfinite(v) and p.get("id"):
            read[p.get("id")] = v
    if not read:
        return None
    params, notes = {}, []

    def note(text):
        if text not in notes:
            notes.append(text)

    def put(key, value):
        lo, hi = RANGES[key]
        params[key] = round(min(max(float(value), lo), hi), 4)

    def on(band):
        return read.get(f"band{band} on", 0.0) >= 0.5

    if "depth" in read:
        put("depth", (read["depth"] + 10.0) / 30.0 * 100.0)
    if "sharpness" in read:
        put("sharpness", read["sharpness"] * 10.0)
    if "selectivity" in read:
        put("selectivity", read["selectivity"] * 10.0)
    if "attack" in read:
        put("attack", log_span(15.0, 2000.0, read["attack"] / 10.0))
    if "release" in read:
        put("release", log_span(10.0, 1500.0, read["release"] / 10.0))
    if "mode" in read:
        put("mode", 1.0 if read["mode"] >= 0.5 else 0.0)
    if "mix" in read:
        put("mix", read["mix"])
    if "delta" in read:
        put("delta", 1.0 if read["delta"] >= 0.5 else 0.0)
    if "trim" in read:
        if abs(read["trim"]) > 12.0:
            note("a trim beyond 12 dB (set to 12)")
        put("trim", read["trim"])
    if "stereo mode" in read:
        put("stereo", 1.0 if read["stereo mode"] >= 0.5 else 0.0)
    if "stereo link" in read:
        put("link", read["stereo link"])
    if "stereo balance" in read:
        put("balance", (read["stereo balance"] - 0.5) * 200.0)

    # The low and high cuts are Rez's range; switched off, the range's own end.
    if "band0 freq" in read:
        put("low", read["band0 freq"] if on(0) else 20.0)
    if "band5 freq" in read:
        put("high", read["band5 freq"] if on(5) else 20000.0)
    if params.get("low", 20.0) >= params.get("high", 20000.0):
        note("a low cut above the high cut (range reset)")
        params["low"], params["high"] = 20.0, 20000.0

    # The four middle bands, as bells.
    for band in range(1, 5):
        if f"band{band} freq" not in read:
            continue
        put(f"b{band}_on", 1.0 if on(band) else 0.0)
        put(f"b{band}_freq", read[f"band{band} freq"])
        put(f"b{band}_gain", read.get(f"band{band} sens", 0.0))
        put(f"b{band}_q", read.get(f"band{band} q", 1.0))
        if on(band):
            if abs(read.get(f"band{band} mode", 1.0) - 1.0) > 0.5:
                note("a band shape other than a bell (brought over as a bell)")
            if abs(read.get(f"band{band} balance", 0.5) - 0.5) > 0.01:
                note("a band's own stereo balance (left out)")
    if read.get("sidechain", 0.0) >= 0.5:
        note("the sidechain switch (pick the key in the Mix window)")
    if not params:
        return None
    return (root.get("presetName") or "").strip(), params, notes


def library_presets_dir():
    """Rez's folder in the default FreeDAWm Library (the settings folder)."""
    home = pathlib.Path.home()
    if sys.platform.startswith("win"):
        base = pathlib.Path(os.environ.get("APPDATA") or home / "AppData" / "Roaming")
    elif sys.platform == "darwin":
        base = home / "Library" / "Application Support"
    else:
        base = pathlib.Path(os.environ.get("XDG_CONFIG_HOME") or home / ".config")
    return base / "FreeDAWm" / "Presets" / "rez"


def safe_name(name):
    name = re.sub(r'[<>:"/\\|?*\x00-\x1f]', " ", name)
    return re.sub(r"\s+", " ", name).strip(" .")[:120] or "preset"


def main():
    ap = argparse.ArgumentParser(description="Convert your own soothe2 presets into Rez user presets.")
    ap.add_argument("sources", nargs="+", help="a soothe2 Presets folder (searched for *.preset) or .preset files")
    ap.add_argument("--out", help="where to write (default: Rez's folder in the FreeDAWm Library)")
    ap.add_argument("--overwrite", action="store_true", help="replace presets that already exist")
    ap.add_argument("--dry-run", action="store_true", help="show what would be written, write nothing")
    args = ap.parse_args()
    out = pathlib.Path(args.out) if args.out else library_presets_dir()

    files = []
    for src in map(pathlib.Path, args.sources):
        if src.is_dir():
            files += [(f, src) for f in sorted(src.rglob("*.preset"))]
        elif src.is_file():
            files.append((src, src.parent))
        else:
            print(f"not found: {src}", file=sys.stderr)
    done = skipped = failed = 0
    approximated = {}
    for f, top in files:
        try:
            got = convert(f.read_text(encoding="utf-8", errors="replace"))
        except OSError:
            got = None
        if got is None:
            failed += 1
            print(f"not a soothe2 preset: {f}", file=sys.stderr)
            continue
        name, params, notes = got
        name = name or f.stem
        # A preset in a category folder carries the category in its name.
        category = f.parent.name if f.parent != top else ""
        full = safe_name(f"{category} - {name}" if category else name)
        target = out / f"{full}.json"
        if target.exists() and not args.overwrite:
            skipped += 1
            continue
        if not args.dry_run:
            out.mkdir(parents=True, exist_ok=True)
            target.write_text(json.dumps({"format": "soundcraft-plugin-preset", "plugin": "rez", "params": params}, indent=1), encoding="utf-8")
        done += 1
        for n in notes:
            approximated.setdefault(n, []).append(full)
    verb = "would write" if args.dry_run else "wrote"
    print(f"{verb} {done} presets to {out}; {skipped} already there, {failed} not readable")
    for n, names in approximated.items():
        shown = ", ".join(names[:4]) + (f" and {len(names) - 4} more" if len(names) > 4 else "")
        print(f"  approximated in {len(names)}: {n}: {shown}")
    return 0 if done or skipped else 1


if __name__ == "__main__":
    sys.exit(main())
