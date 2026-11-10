#!/usr/bin/env python3
"""Render the guide's screenshots from the current app: `python3 docs/guide/shots.py [name…]`.

Each shot is a ui_shot script in docs/guide/shots/ that writes docs/guide/images/<name>.png, then a
crop (left, top, right, bottom in window points; ui_shot renders two pixels per point). Needs
Pillow for the crops. Run from anywhere in the repository.
"""
import pathlib
import subprocess
import sys

from PIL import Image

ROOT = pathlib.Path(__file__).resolve().parents[2]
# script: (images it writes, window size, extra ui_shot flags, crops)
# A crop is (output image, source image, box in points); a source with no crop is kept whole.
SHOTS = {
    "windows": (["edit-window", "mix-window"], "1440x900", [], [("markers-ruler", "edit-window", (180, 114, 1188, 199))]),
    "engine": (["playback-engine"], "1440x900", [], [("playback-engine", "playback-engine", (16, 16, 476, 376))]),
    "io": (["io-setup"], "1440x900", [], [("io-setup", "io-setup", (16, 16, 577, 546))]),
    "new-tracks": (["new-tracks"], "1440x900", [], [("new-tracks", "new-tracks", (438, 317, 1002, 423))]),
    "midi": (["midi-editor"], "1000x640", [], [("midi-editor", "midi-editor", (8, 331, 992, 589))]),
    "markers": (["memory-locations"], "1440x900", [], [("memory-locations", "memory-locations", (160, 120, 641, 288))]),
    "mix-menus": (
        ["add-effect", "send-menu"],
        "1440x900",
        [],
        [("add-effect", "add-effect", (180, 101, 634, 360)), ("send-menu", "send-menu", (180, 364, 634, 536))],
    ),
    "automation": (["automation-lane"], "1440x900", [], [("automation-lane", "automation-lane", (180, 200, 760, 290))]),
    "bounce": (["bounce-mix"], "1440x900", [], [("bounce-mix", "bounce-mix", (524, 238, 917, 502))]),
    "armed": (
        ["armed-track"],
        "1720x900",
        ["--blank"],
        [("record-transport", "armed-track", (1114, 33, 1664, 101)), ("armed-track", "armed-track", (176, 200, 447, 291))],
    ),
}


def main() -> int:
    names = sys.argv[1:] or list(SHOTS)
    subprocess.run(["cargo", "build", "-p", "soundcraft-ui-egui", "--example", "ui_shot"], cwd=ROOT, check=True)
    for name in names:
        images, size, flags, crops = SHOTS[name]
        last = ROOT / "target" / "guide-shot-last.png"
        subprocess.run(
            [ROOT / "target/debug/examples/ui_shot", last, f"docs/guide/shots/{name}.jsonl", "--size", size, "--system-theme", "dark", *flags],
            cwd=ROOT,
            check=True,
            stdout=subprocess.DEVNULL,
        )
        folder = ROOT / "docs/guide/images"
        sources = {image: Image.open(folder / f"{image}.png").copy() for image in images}
        for out, source, box in crops:
            sources[source].crop(tuple(2 * v for v in box)).save(folder / f"{out}.png", optimize=True)
        for image in {*images, *(c[0] for c in crops)}:
            print(f"wrote docs/guide/images/{image}.png")
    return 0


if __name__ == "__main__":
    sys.exit(main())
